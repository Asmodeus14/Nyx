use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use core::cmp;
use core::convert::TryInto;
use crate::drivers::nvme::NvmeDriver;
use fatfs::{Read, Write, Seek, SeekFrom, IoBase};

const FAT_SECTOR_SIZE: u64 = 512;
const NVME_BLOCK_SIZE: u64 = 512; 
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
            // 🚨 DMA SAFETY: Always allocate 4096 bytes for NVMe hardware transfers, 
            // even if the logical block size is only 512.
            temp_block: alloc::vec![0u8; 4096] 
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
    pub ram_disk: Vec<(String, Vec<u8>)>,
}

impl FileSystem {
    pub const fn new() -> Self { 
        Self { 
            inner: None, 
            cache_path: String::new(), 
            cache_files: Vec::new(),
            ram_disk: Vec::new()
        } 
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
             crate::serial_println!("[FS] GPT Detected. Scanning full 512-byte Partition Array...");
             let mut partition_found_count = 0;
             
             for gpt_lba in 2..=9 {
                 let mut entry_block = alloc::vec![0u8; 4096];
                 
                 if driver.read_block(gpt_lba, &mut entry_block) {
                     for i in 0..4 {
                         let offset = i * 128;
                         let type_guid_first = u64::from_le_bytes(entry_block[offset..offset+8].try_into().unwrap());
                         
                         if type_guid_first != 0 {
                             partition_found_count += 1;
                             let lba = u64::from_le_bytes(entry_block[offset+32..offset+40].try_into().unwrap());
                             crate::serial_println!("[FS] Checking Partition {} at LBA: {}", partition_found_count, lba);
                             
                             let nvme_boot_lba = (lba * FAT_SECTOR_SIZE) / NVME_BLOCK_SIZE;
                             let boot_offset = ((lba * FAT_SECTOR_SIZE) % NVME_BLOCK_SIZE) as usize;
                             
                             let mut boot_sector = alloc::vec![0u8; 4096];
                             if driver.read_block(nvme_boot_lba, &mut boot_sector) {
                                 if boot_sector[boot_offset + 510] == 0x55 && boot_sector[boot_offset + 511] == 0xAA {
                                     partition_start_lba = lba;
                                     crate::serial_println!("[FS] -> SUCCESS: Found FAT32 Boot Sector!");
                                     break;
                                 } else {
                                     crate::serial_println!("[FS] -> Skipping (Not FAT32 formatted)");
                                 }
                             }
                         }
                     }
                 }
                 if partition_start_lba != 0 { break; }
             }
        } else {
             crate::serial_println!("[FS] Standard MBR Detected.");
             partition_start_lba = u32::from_le_bytes([sector0[0x1BE+8], sector0[0x1BE+9], sector0[0x1BE+10], sector0[0x1BE+11]]) as u64;
        }

        if partition_start_lba == 0 { 
            crate::serial_println!("[FS] No FAT32 partition found. Probing NVMe for unallocated space..."); 
            
            if let Some(safe_lba) = crate::partitioner::NyxPartitioner::find_free_space(&mut driver) {
                crate::serial_println!("[FS] READY TO FORMAT: Gap secured at LBA {}.", safe_lba);
            }
            
            crate::serial_println!("[FS] Falling back to RAM Disk.");
            return; 
        }
        
        let nvme_boot_lba = (partition_start_lba * FAT_SECTOR_SIZE) / NVME_BLOCK_SIZE;
        let boot_offset = ((partition_start_lba * FAT_SECTOR_SIZE) % NVME_BLOCK_SIZE) as usize;
        
        let mut boot_sector = alloc::vec![0u8; 4096];
        if !driver.read_block(nvme_boot_lba, &mut boot_sector) { return; }
        
        if boot_sector[boot_offset + 510] != 0x55 || boot_sector[boot_offset + 511] != 0xAA { return; }

        crate::serial_println!("[FS] Valid FAT boot sector verified. Mounting NVMe...");

        let stream = NvmeStream::new(driver, partition_start_lba);
        let options = fatfs::FsOptions::new().update_accessed_date(true);
        
        // 🚨 ADDED ERROR CATCHING 🚨
        match fatfs::FileSystem::new(stream, options) {
            Ok(fs) => {
                self.inner = Some(fs);
                crate::serial_println!("[FS] FAT Filesystem successfully mounted.");
            },
            Err(e) => {
                crate::serial_println!("[FS] CRITICAL: fatfs failed to mount. Error: {:?}", e);
            }
        }
    }

    pub fn ls(&mut self, path: &str) -> Vec<String> {
        if !self.cache_path.is_empty() && self.cache_path == path { return self.cache_files.clone(); }
        
        let mut list = Vec::new();
        
        if path == "/" || path.is_empty() {
            list.push(String::from("hello.elf"));
            list.push(String::from("nyx-user.bin"));
            list.push(String::from("rust.elf")); 
        }

        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
            let res = if path == "/" || path.is_empty() { Ok(root) } else { root.open_dir(path) };
            if let Ok(dir) = res {
                for entry in dir.iter() {
                    if let Ok(e) = entry { 
                        let mut name = e.file_name();
                        if e.is_dir() { name.push('/'); }
                        if !list.contains(&name) {
                            list.push(name); 
                        }
                    }
                }
            }
        } else {
            // ONLY load RAM disk if NVMe is offline
            if path == "/" || path.is_empty() {
                for file in self.ram_disk.iter() {
                    if !list.contains(&file.0) {
                        list.push(file.0.clone());
                    }
                }
            }
        }
        
        self.cache_path = String::from(path); 
        self.cache_files = list.clone();
        list
    }

    pub fn read_file(&mut self, name: &str) -> Option<Vec<u8>> {
        if name == "hello.elf" || name == "/hello.elf" { return Some(crate::HELLO_BIN.to_vec()); }
        if name == "nyx-user.bin" || name == "/nyx-user.bin" { return None; }
        if name == "rust.elf" || name == "/rust.elf" { return Some(crate::RUST_BIN.to_vec()); }

        let clean_name = if name.starts_with('/') { &name[1..] } else { name };

        if let Some(fs) = &self.inner {
            let root = fs.root_dir();
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
            return None; // Ensure it doesn't fall back to RAM if NVMe is mounted
        }

        // Fallback to RAM Disk ONLY if NVMe is offline
        for file in self.ram_disk.iter() {
            if file.0 == clean_name {
                return Some(file.1.clone());
            }
        }
        None
    }

    pub fn write_file(&mut self, name: &str, data: &[u8]) -> bool {
        self.cache_path.clear(); 
        let clean_name = if name.starts_with('/') { &name[1..] } else { name };
        
        if let Some(fs) = &self.inner {
            crate::serial_println!("[FS] Attempting physical NVMe write for: {}", clean_name);
            let root = fs.root_dir();
            let compressed_payload = nyx_compress(data);
            let mut final_data = Vec::with_capacity(4 + compressed_payload.len());
            final_data.extend_from_slice(MAGIC_SIG);
            final_data.extend_from_slice(&compressed_payload);
            
            match root.create_file(clean_name) {
                Ok(mut file) => {
                    match file.write_all(&final_data) {
                        Ok(_) => crate::serial_println!("[FS] SUCCESS: Bytes pushed to NVMe stream."),
                        Err(_) => crate::serial_println!("[FS] ERR: NVMe Stream rejected write."),
                    }
                    let _ = file.flush();
                    crate::serial_println!("[FS] FAT32 directory entry closed.");
                },
                Err(_) => {
                    crate::serial_println!("[FS] ERR: fatfs could not allocate file!");
                    return false;
                }
            }
            return true; 
        }
        
        crate::serial_println!("[FS] NVMe offline. Storing in volatile RAM Disk.");
        let mut found = false;
        for file in self.ram_disk.iter_mut() {
            if file.0 == clean_name {
                file.1 = data.to_vec();
                found = true;
                break;
            }
        }
        if !found {
            self.ram_disk.push((String::from(clean_name), data.to_vec()));
        }
        
        true 
    }
}

pub static FS: Mutex<FileSystem> = Mutex::new(FileSystem::new());