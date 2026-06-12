use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use core::cmp;
use core::convert::TryInto;
use crate::drivers::nvme::NvmeDriver;
use fatfs::{Read, Write, Seek, SeekFrom, IoBase};

const FAT_SECTOR_SIZE: u64 = 512;
const NVME_BLOCK_SIZE: u64 = 512; 

// ==========================================
// FATFS STREAM WRAPPER FOR NVME HARDWARE
// ==========================================
pub struct NvmeStream {
    driver: NvmeDriver,
    position: u64,
    partition_offset_sectors: u64,
    temp_block: Vec<u8>,
}

impl NvmeStream {
    pub fn new(driver: NvmeDriver, partition_offset_sectors: u64) -> Self {
        Self { 
            driver, 
            position: 0, 
            partition_offset_sectors, 
            temp_block: alloc::vec![0u8; 4096] 
        }
    }
}

impl IoBase for NvmeStream { type Error = (); }

impl Read for NvmeStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() { return Ok(0); }
        
        let absolute_byte_pos = (self.partition_offset_sectors * FAT_SECTOR_SIZE) + self.position;
        let nvme_lba = absolute_byte_pos / NVME_BLOCK_SIZE;
        let offset_in_nvme_block = (absolute_byte_pos % NVME_BLOCK_SIZE) as usize;
        
        if self.driver.read_block(nvme_lba, &mut self.temp_block) {
            let bytes_available = (NVME_BLOCK_SIZE as usize) - offset_in_nvme_block;
            let bytes_to_copy = cmp::min(buf.len(), bytes_available);
            
            buf[..bytes_to_copy].copy_from_slice(&self.temp_block[offset_in_nvme_block..offset_in_nvme_block+bytes_to_copy]);
            self.position += bytes_to_copy as u64;
            Ok(bytes_to_copy)
        } else { Err(()) }
    }
}

impl Write for NvmeStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let absolute_byte_pos = (self.partition_offset_sectors * FAT_SECTOR_SIZE) + self.position;
        let nvme_lba = absolute_byte_pos / NVME_BLOCK_SIZE;
        let offset_in_nvme_block = (absolute_byte_pos % NVME_BLOCK_SIZE) as usize;
        
        if !self.driver.read_block(nvme_lba, &mut self.temp_block) { return Err(()); }
        
        let bytes_available = (NVME_BLOCK_SIZE as usize) - offset_in_nvme_block;
        let bytes_to_copy = cmp::min(buf.len(), bytes_available);
        
        self.temp_block[offset_in_nvme_block..offset_in_nvme_block+bytes_to_copy].copy_from_slice(&buf[..bytes_to_copy]);
        
        if self.driver.write_block(nvme_lba, &self.temp_block) {
            self.position += bytes_to_copy as u64;
            Ok(bytes_to_copy)
        } else { Err(()) }
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

// ==========================================
// THE NVME BRIDGE DRIVER FOR THE NEW VFS
// ==========================================
pub struct NvmeFs {
    inner: Mutex<fatfs::FileSystem<NvmeStream, fatfs::NullTimeProvider, fatfs::LossyOemCpConverter>>,
}

// Safety wrapper to appease the compiler for the VFS trait requirements
unsafe impl Send for NvmeFs {}
unsafe impl Sync for NvmeFs {}

impl NvmeFs {
    pub fn new(mut driver: NvmeDriver) -> Option<Self> {
        let mut sector0 = alloc::vec![0u8; 4096];
        let _ = driver.find_active_namespace();
        
        if !driver.read_block(0, &mut sector0) { return None; }

        let mut partition_start_lba = 0;
        let part_type = sector0[0x1BE + 4];

        if part_type == 0xEE {
             for gpt_lba in 2..=9 {
                 let mut entry_block = alloc::vec![0u8; 4096];
                 if driver.read_block(gpt_lba, &mut entry_block) {
                     for i in 0..4 {
                         let offset = i * 128;
                         let type_guid_first = u64::from_le_bytes(entry_block[offset..offset+8].try_into().unwrap());
                         if type_guid_first != 0 {
                             let lba = u64::from_le_bytes(entry_block[offset+32..offset+40].try_into().unwrap());
                             let nvme_boot_lba = (lba * FAT_SECTOR_SIZE) / NVME_BLOCK_SIZE;
                             let boot_offset = ((lba * FAT_SECTOR_SIZE) % NVME_BLOCK_SIZE) as usize;
                             
                             let mut boot_sector = alloc::vec![0u8; 4096];
                             if driver.read_block(nvme_boot_lba, &mut boot_sector) {
                                 if boot_sector[boot_offset + 510] == 0x55 && boot_sector[boot_offset + 511] == 0xAA {
                                     partition_start_lba = lba; break;
                                 }
                             }
                         }
                     }
                 }
                 if partition_start_lba != 0 { break; }
             }
        } else {
             partition_start_lba = u32::from_le_bytes([sector0[0x1BE+8], sector0[0x1BE+9], sector0[0x1BE+10], sector0[0x1BE+11]]) as u64;
        }

        if partition_start_lba == 0 { return None; }
        
        let stream = NvmeStream::new(driver, partition_start_lba);
        let options = fatfs::FsOptions::new().update_accessed_date(true);
        
        if let Ok(fs) = fatfs::FileSystem::new(stream, options) {
            Some(Self { inner: Mutex::new(fs) })
        } else { None }
    }
}

impl crate::vfs::FileSystem for NvmeFs {
    fn read_file(&self, path: &str, offset: usize, buf: &mut [u8]) -> Option<usize> {
        let fs = self.inner.lock();
        let clean_path = path.trim_start_matches('/');
        
        if let Ok(mut file) = fs.root_dir().open_file(clean_path) {
            if file.seek(SeekFrom::Start(offset as u64)).is_ok() {
                
                // 🚨 THE FIX: Keep reading blocks until the RAM buffer is completely full!
                let mut total_read = 0;
                while total_read < buf.len() {
                    match file.read(&mut buf[total_read..]) {
                        Ok(0) => break, // Reached End Of File
                        Ok(n) => total_read += n,
                        Err(_) => return None,
                    }
                }
                return Some(total_read);
            }
        }
        None
    }

    fn write_file(&mut self, path: &str, offset: usize, buf: &[u8]) -> Option<usize> {
        let fs = self.inner.lock();
        let clean_path = path.trim_start_matches('/');
        
        if let Ok(mut file) = fs.root_dir().create_file(clean_path) {
            if offset == 0 { let _ = file.truncate(); }
            
            if file.seek(SeekFrom::Start(offset as u64)).is_ok() {
                
                // 🚨 THE FIX: Keep writing blocks until the entire payload is on the disk!
                let mut total_written = 0;
                while total_written < buf.len() {
                    match file.write(&buf[total_written..]) {
                        Ok(0) => break, // Disk full or Hardware Error
                        Ok(n) => total_written += n,
                        Err(_) => return None,
                    }
                }
                let _ = file.flush();
                return Some(total_written);
            }
        }
        None
    }

    fn get_file_size(&self, path: &str) -> Option<usize> {
        let fs = self.inner.lock();
        let clean_path = path.trim_start_matches('/');
        if let Ok(mut file) = fs.root_dir().open_file(clean_path) {
            if let Ok(size) = file.seek(SeekFrom::End(0)) {
                return Some(size as usize);
            }
        }
        None
    }

    fn create_file(&mut self, path: &str) -> bool {
        let fs = self.inner.lock();
        let clean_path = path.trim_start_matches('/');
        // Safe binding to drop the Mutex before returning
        let result = fs.root_dir().create_file(clean_path).is_ok();
        result
    }

    fn create_dir(&mut self, path: &str) -> bool {
        let fs = self.inner.lock();
        let clean_path = path.trim_start_matches('/');
        // Safe binding to drop the Mutex before returning
        let result = fs.root_dir().create_dir(clean_path).is_ok();
        result
    }

    fn list_dir(&self, path: &str) -> Vec<String> {
        let mut results = Vec::new();
        let fs = self.inner.lock();
        let root = fs.root_dir();
        let clean_path = path.trim_start_matches('/');
        
        let dir = if clean_path.is_empty() { root } else { 
            match root.open_dir(clean_path) { Ok(d) => d, Err(_) => return results }
        };

        for entry in dir.iter() {
            if let Ok(e) = entry {
                let mut name = e.file_name();
                if name != "." && name != ".." {
                    if e.is_dir() { name.push('/'); }
                    results.push(name);
                }
            }
        }
        results
    }
}