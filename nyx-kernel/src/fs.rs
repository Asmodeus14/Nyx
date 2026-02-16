use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;
use spin::Mutex;
use core::cmp;
use core::convert::TryInto;
use alloc::format;

use crate::drivers::nvme::NvmeDriver;
use fatfs::{Read, Write, Seek, SeekFrom, IoBase};
use crate::gui::{Painter, Color}; 

const BLOCK_SIZE: u64 = 512;

// --- ADAPTER ---
pub struct NvmeStream {
    driver: NvmeDriver,
    position: u64,
    partition_offset: u64,
}

impl NvmeStream {
    pub fn new(driver: NvmeDriver, partition_offset: u64) -> Self {
        Self { driver, position: 0, partition_offset }
    }
}

impl IoBase for NvmeStream { type Error = (); }

impl Read for NvmeStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.len() == 0 { return Ok(0); }
        let current_lba = self.partition_offset + (self.position / BLOCK_SIZE);
        let offset_in_block = (self.position % BLOCK_SIZE) as usize;
        let mut temp_block = [0u8; 4096]; 
        
        if self.driver.read_block(current_lba, &mut temp_block) {
            let bytes_available = (BLOCK_SIZE as usize) - offset_in_block;
            let bytes_to_copy = cmp::min(buf.len(), bytes_available);
            buf[..bytes_to_copy].copy_from_slice(&temp_block[offset_in_block..offset_in_block+bytes_to_copy]);
            self.position += bytes_to_copy as u64;
            Ok(bytes_to_copy)
        } else {
            Err(())
        }
    }
}

impl Write for NvmeStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let current_lba = self.partition_offset + (self.position / BLOCK_SIZE);
        let offset_in_block = (self.position % BLOCK_SIZE) as usize;
        let mut temp_block = [0u8; 4096];
        
        if !self.driver.read_block(current_lba, &mut temp_block) { 
            return Err(()); 
        }
        
        let bytes_available = (BLOCK_SIZE as usize) - offset_in_block;
        let bytes_to_copy = cmp::min(buf.len(), bytes_available);
        
        temp_block[offset_in_block..offset_in_block+bytes_to_copy].copy_from_slice(&buf[..bytes_to_copy]);
        
        if self.driver.write_block(current_lba, &temp_block) {
            self.position += bytes_to_copy as u64;
            Ok(bytes_to_copy)
        } else {
            Err(())
        }
    }
    fn flush(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

impl Seek for NvmeStream {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        match pos {
            SeekFrom::Start(i) => self.position = i,
            SeekFrom::Current(i) => self.position = (self.position as i64 + i) as u64,
            SeekFrom::End(_) => return Err(()), 
        }
        Ok(self.position)
    }
}

// --- FS MANAGER ---
pub struct FileSystem {
    inner: Option<fatfs::FileSystem<NvmeStream, fatfs::NullTimeProvider, fatfs::LossyOemCpConverter>>,
    cache_path: String,
    cache_files: Vec<String>,
}

impl FileSystem {
    pub const fn new() -> Self { 
        Self { inner: None, cache_path: String::new(), cache_files: Vec::new() } 
    }

    pub fn init(&mut self, mut driver: NvmeDriver) {
        if !driver.create_io_queues() { return; }
        
        let mut sector0 = [0u8; 4096];
        let _ = driver.find_active_namespace();
        if !driver.read_block(0, &mut sector0) { return; }

        let mut partition_start_lba = 0;
        let part_type = sector0[0x1BE + 4];

        if part_type == 0xEE { 
             let mut gpt_header = [0u8; 4096];
             if driver.read_block(1, &mut gpt_header) {
                 let entry_list_lba = u64::from_le_bytes(gpt_header[72..80].try_into().unwrap());
                 let mut entry_block = [0u8; 4096];
                 if driver.read_block(entry_list_lba, &mut entry_block) {
                     for i in 0..4 { 
                         let offset = i * 128;
                         let type_guid_first = u64::from_le_bytes(entry_block[offset..offset+8].try_into().unwrap());
                         if type_guid_first != 0 {
                             partition_start_lba = u64::from_le_bytes(entry_block[offset+32..offset+40].try_into().unwrap());
                             break; 
                         }
                     }
                 }
             }
        } else { 
             partition_start_lba = u32::from_le_bytes([sector0[0x1BE+8], sector0[0x1BE+9], sector0[0x1BE+10], sector0[0x1BE+11]]) as u64;
        }

        if partition_start_lba == 0 { return; }

        let stream = NvmeStream::new(driver, partition_start_lba);
        let options = fatfs::FsOptions::new().update_accessed_date(true);
        if let Ok(fs) = fatfs::FileSystem::new(stream, options) {
            self.inner = Some(fs);
        }
    }

    pub fn ls(&mut self, path: &str) -> Vec<String> {
        if !self.cache_path.is_empty() && self.cache_path == path {
            return self.cache_files.clone();
        }

        let mut list = Vec::new();
        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
            let res = if path == "/" || path.is_empty() { Ok(root) } else { root.open_dir(path) };

            if let Ok(dir) = res {
                for entry in dir.iter() {
                    if let Ok(e) = entry { 
                        let mut name = e.file_name();
                        if e.is_dir() { name.push('/'); } 
                        list.push(name); 
                    }
                }
            }
        }
        self.cache_path = String::from(path);
        self.cache_files = list.clone();
        list
    }

    pub fn read_file(&mut self, name: &str) -> Option<Vec<u8>> {
        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
            let clean_name = if name.starts_with('/') { &name[1..] } else { name };
            if let Ok(mut file) = root.open_file(clean_name) {
                let mut buf = Vec::new();
                let mut temp = [0u8; 512];
                loop {
                    match file.read(&mut temp) {
                        Ok(0) => break,
                        Ok(n) => buf.extend_from_slice(&temp[..n]),
                        Err(_) => return None,
                    }
                }
                return Some(buf);
            }
        }
        None
    }

    pub fn write_file(&mut self, name: &str, data: &[u8]) -> bool {
        self.cache_path.clear(); 
        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
            let clean_name = if name.starts_with('/') { &name[1..] } else { name };
            match root.create_file(clean_name) {
                Ok(mut file) => {
                    return file.write_all(data).is_ok();
                },
                Err(_) => { return false; }
            }
        }
        false
    }
}

pub static FS: Mutex<FileSystem> = Mutex::new(FileSystem::new());