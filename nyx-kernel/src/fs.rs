use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use core::cmp;
use core::convert::TryInto;
use crate::drivers::nvme::NvmeDriver;
use fatfs::{Read, Write, Seek, SeekFrom, IoBase};

const FAT_SECTOR_SIZE: u64 = 512;
const NVME_BLOCK_SIZE: u64 = 4096;
const MAGIC_SIG: &[u8; 4] = b"NYXZ";

fn nyx_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let current = data[i];
        let mut run_len = 1;
        while i + run_len < data.len() && data[i + run_len] == current && run_len < 255 { run_len += 1; }
        
        if run_len >= 4 || current == 0xFD {
            out.push(0xFD); out.push(run_len as u8); out.push(current);
            i += run_len;
        } else {
            out.push(current); i += 1;
        }
    }
    out
}

fn nyx_decompress(data: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0xFD {
            if i + 2 >= data.len() { return Err(()); }
            let run_len = data[i + 1]; let val = data[i + 2];
            for _ in 0..run_len { out.push(val); }
            i += 3;
        } else {
            out.push(data[i]); i += 1;
        }
    }
    Ok(out)
}

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
            temp_block: alloc::vec![0u8; NVME_BLOCK_SIZE as usize] 
        }
    }
}

impl IoBase for NvmeStream { type Error = (); }

impl Read for NvmeStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.len() == 0 { return Ok(0); }
        
        let absolute_byte_pos = (self.partition_offset_sectors * FAT_SECTOR_SIZE) + self.position;
        let nvme_lba = absolute_byte_pos / NVME_BLOCK_SIZE;
        let offset_in_nvme_block = (absolute_byte_pos % NVME_BLOCK_SIZE) as usize;
        
        if self.driver.read_block(nvme_lba, &mut self.temp_block) {
            let bytes_available = (NVME_BLOCK_SIZE as usize) - offset_in_nvme_block;
            let bytes_to_copy = cmp::min(buf.len(), bytes_available);
            
            buf[..bytes_to_copy].copy_from_slice(&self.temp_block[offset_in_nvme_block..offset_in_nvme_block+bytes_to_copy]);
            self.position += bytes_to_copy as u64;
            Ok(bytes_to_copy)
        } else {
            Err(())
        }
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
        crate::serial_println!("[FS] Initializing FileSystem Subsystem...");
        let mut sector0 = alloc::vec![0u8; 4096];
        let _ = driver.find_active_namespace();
        
        if !driver.read_block(0, &mut sector0) { 
            crate::serial_println!("[FS] ERR: Failed to read MBR/Sector 0."); return; 
        }

        let mut partition_start_lba = 0;
        let part_type = sector0[0x1BE + 4];

        if part_type == 0xEE {
             crate::serial_println!("[FS] GPT Detected. Parsing headers...");
             let entry_list_lba = u64::from_le_bytes(sector0[512+72..512+80].try_into().unwrap());
             let mut entry_block = alloc::vec![0u8; 4096];
             
             if driver.read_block(1, &mut entry_block) {
                 for i in 0..4 {
                     let offset = i * 128;
                     let type_guid_first = u64::from_le_bytes(entry_block[offset..offset+8].try_into().unwrap());
                     if type_guid_first != 0 {
                         partition_start_lba = u64::from_le_bytes(entry_block[offset+32..offset+40].try_into().unwrap());
                         break;
                     }
                 }
             }
        } else {
             crate::serial_println!("[FS] Standard MBR Detected.");
             partition_start_lba = u32::from_le_bytes([sector0[0x1BE+8], sector0[0x1BE+9], sector0[0x1BE+10], sector0[0x1BE+11]]) as u64;
        }

        if partition_start_lba == 0 { 
            crate::serial_println!("[FS] No valid partition found. Drive is raw."); return; 
        }
        
        let nvme_boot_lba = (partition_start_lba * FAT_SECTOR_SIZE) / NVME_BLOCK_SIZE;
        let boot_offset = ((partition_start_lba * FAT_SECTOR_SIZE) % NVME_BLOCK_SIZE) as usize;
        
        let mut boot_sector = alloc::vec![0u8; 4096];
        if !driver.read_block(nvme_boot_lba, &mut boot_sector) { return; }
        
        if boot_sector[boot_offset + 510] != 0x55 || boot_sector[boot_offset + 511] != 0xAA { return; }

        crate::serial_println!("[FS] Valid FAT boot sector detected. Mounting...");

        let stream = NvmeStream::new(driver, partition_start_lba);
        let options = fatfs::FsOptions::new().update_accessed_date(true);
        
        if let Ok(fs) = fatfs::FileSystem::new(stream, options) {
            self.inner = Some(fs);
            crate::serial_println!("[FS] FAT Filesystem successfully mounted.");
        }
    }

    pub fn ls(&mut self, path: &str) -> Vec<String> {
        if !self.cache_path.is_empty() && self.cache_path == path { return self.cache_files.clone(); }
        
        let mut list = Vec::new();
        
        // 🚨 VIRTUAL FILESYSTEM INJECTION (The RAM Disk Cheat) 🚨
        if path == "/" || path.is_empty() {
            list.push(String::from("hello.elf"));
            list.push(String::from("nyx-user.bin"));
            list.push(String::from("rust.elf")); // <-- NOW VISIBLE IN `ls`!
        }

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
        // 🚨 VIRTUAL FILE INTERCEPT (The RAM Disk Cheat) 🚨
        if name == "hello.elf" || name == "/hello.elf" {
            return Some(crate::HELLO_BIN.to_vec());
        }
        if name == "nyx-user.bin" || name == "/nyx-user.bin" {
            // Note: If you renamed the INIT_FS variable in main, you can intercept it here too, 
            // but the kernel bootstraps it directly in main() anyway.
            return None; 
        }
        if name == "rust.elf" || name == "/rust.elf" {
            return Some(crate::RUST_BIN.to_vec());
        }

        // --- ACTUAL NVMe HARDWARE FATFS READ ---
        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
            let clean_name = if name.starts_with('/') { &name[1..] } else { name };
            
            if let Ok(mut file) = root.open_file(clean_name) {
                let mut buf = Vec::new();
                let mut temp = alloc::vec![0u8; 4096];
                loop {
                    match file.read(&mut temp) {
                        Ok(0) => break,
                        Ok(n) => buf.extend_from_slice(&temp[..n]),
                        Err(_) => return None,
                    }
                }
                
                if buf.len() >= 4 && &buf[0..4] == MAGIC_SIG {
                    match nyx_decompress(&buf[4..]) {
                        Ok(decompressed) => return Some(decompressed),
                        Err(_) => return Some(buf), 
                    }
                } else { return Some(buf); }
            }
        }
        None
    }

    pub fn write_file(&mut self, name: &str, data: &[u8]) -> bool {
        self.cache_path.clear(); 
        
        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
            let clean_name = if name.starts_with('/') { &name[1..] } else { name };
            
            let compressed_payload = nyx_compress(data);
            let mut final_data = Vec::with_capacity(4 + compressed_payload.len());
            final_data.extend_from_slice(MAGIC_SIG);
            final_data.extend_from_slice(&compressed_payload);
            
            match root.create_file(clean_name) {
                Ok(mut file) => return file.write_all(&final_data).is_ok(),
                Err(_) => return false,
            }
        }
        false
    }
}

pub static FS: Mutex<FileSystem> = Mutex::new(FileSystem::new());