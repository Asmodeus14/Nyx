use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use core::cmp;
use core::convert::TryInto;
use alloc::format;

use crate::drivers::nvme::NvmeDriver;
use fatfs::{Read, Write, Seek, SeekFrom, IoBase};

const BLOCK_SIZE: u64 = 512;
const MAGIC_SIG: &[u8; 4] = b"NYXZ";

// ==========================================
// NYX NATIVE LOSSLESS COMPRESSION ENGINE
// 100% Heap-safe Run-Length Encoding. 
// Never stack overflows.
// ==========================================
fn nyx_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let current = data[i];
        let mut run_len = 1;

        // Count how many identical bytes are in a row (up to 255)
        while i + run_len < data.len() && data[i + run_len] == current && run_len < 255 {
            run_len += 1;
        }

        // 0xFD is our special marker byte. 
        // We compress if we have a run of 4+ bytes, OR if the raw data actually contains 0xFD.
        if run_len >= 4 || current == 0xFD {
            out.push(0xFD); // Marker
            out.push(run_len as u8); // Length
            out.push(current); // The Byte
            i += run_len;
        } else {
            out.push(current);
            i += 1;
        }
    }
    out
}

fn nyx_decompress(data: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0xFD { // Found a compression marker!
            if i + 2 >= data.len() { return Err(()); } // Prevent out-of-bounds crash
            let run_len = data[i + 1];
            let val = data[i + 2];
            for _ in 0..run_len {
                out.push(val);
            }
            i += 3;
        } else { // Standard uncompressed byte
            out.push(data[i]);
            i += 1;
        }
    }
    Ok(out)
}
// ==========================================

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
        
        if !self.driver.read_block(current_lba, &mut temp_block) { return Err(()); }
        
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
    pub inner: Option<fatfs::FileSystem<NvmeStream, fatfs::NullTimeProvider, fatfs::LossyOemCpConverter>>,
    cache_path: String,
    cache_files: Vec<String>,
}

impl FileSystem {
    pub const fn new() -> Self { 
        Self { inner: None, cache_path: String::new(), cache_files: Vec::new() } 
    }

    pub fn init(&mut self, mut driver: NvmeDriver) {
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
        if !self.cache_path.is_empty() && self.cache_path == path { return self.cache_files.clone(); }

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
                
                // DECOMPRESSION
                if buf.len() >= 4 && &buf[0..4] == MAGIC_SIG {
                    match nyx_decompress(&buf[4..]) {
                        Ok(decompressed) => {
                            crate::serial_println!("[FS] Nyx-Decompressed {} -> {} bytes.", buf.len(), decompressed.len());
                            return Some(decompressed);
                        },
                        Err(_) => return Some(buf), // Corrupted, return raw
                    }
                } else {
                    return Some(buf); // Legacy file
                }
            }
        }
        None
    }

    pub fn write_file(&mut self, name: &str, data: &[u8]) -> bool {
        self.cache_path.clear(); 
        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
            let clean_name = if name.starts_with('/') { &name[1..] } else { name };
            
            // COMPRESSION
            crate::serial_println!("[FS] Nyx-Compressing file '{}' ({} bytes)...", clean_name, data.len());
            let compressed_payload = nyx_compress(data);
            
            let mut final_data = Vec::with_capacity(4 + compressed_payload.len());
            final_data.extend_from_slice(MAGIC_SIG);
            final_data.extend_from_slice(&compressed_payload);

            crate::serial_println!("[FS] Compressed to {} bytes.", final_data.len());

            match root.create_file(clean_name) {
                Ok(mut file) => return file.write_all(&final_data).is_ok(),
                Err(_) => return false,
            }
        }
        false
    }
}

pub static FS: Mutex<FileSystem> = Mutex::new(FileSystem::new());