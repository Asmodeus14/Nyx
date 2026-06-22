use alloc::string::String;
use alloc::vec::Vec;
use core::convert::TryInto;
use crate::drivers::nvme::NvmeDriver;
use alloc::boxed::Box;
use crate::vfs::FsError;

// ==========================================
// C-FFI HARDWARE BRIDGE (DMA ALIGNED)
// ==========================================
pub static mut GLOBAL_NVME: Option<NvmeDriver> = None;

#[no_mangle]
pub extern "C" fn nyx_nvme_read_block(sector: u64, buf: *mut u8) -> bool {
    unsafe {
        if let Some(ref mut driver) = GLOBAL_NVME {
            // 🔥 MILESTONE 1.6 VERIFICATION:
            // The NVMe driver requires strict 4096-byte page-aligned buffers for PRP DMA transfers.
            // However, lwext4 natively addresses logical sectors in 512-byte increments.
            // We safely allocate a 4K aligned buffer, perform the DMA read, and extract ONLY 
            // the 512-byte logical sector requested by the VFS to prevent buffer overrun corruption.
            let mut align_buf = alloc::vec![0u8; 8192];
            let ptr_addr = align_buf.as_ptr() as usize;
            let offset = (4096 - (ptr_addr % 4096)) % 4096;
            
            let slice_4k = core::slice::from_raw_parts_mut(align_buf.as_mut_ptr().add(offset), 4096);
            
            if driver.read_block(sector, slice_4k) {
                core::ptr::copy_nonoverlapping(slice_4k.as_ptr(), buf, 512);
                return true;
            }
        }
    }
    false
}

#[no_mangle]
pub extern "C" fn nyx_nvme_write_block(sector: u64, buf: *const u8) -> bool {
    unsafe {
        if let Some(ref mut driver) = GLOBAL_NVME {
            let mut align_buf = alloc::vec![0u8; 8192];
            let ptr_addr = align_buf.as_ptr() as usize;
            let offset = (4096 - (ptr_addr % 4096)) % 4096;
            
            let slice_4k = core::slice::from_raw_parts_mut(align_buf.as_mut_ptr().add(offset), 4096);
            core::ptr::copy_nonoverlapping(buf, slice_4k.as_mut_ptr(), 512);
            
            return driver.write_block(sector, slice_4k);
        }
    }
    false
}

extern "C" {
    fn nyx_fs_mount(start_sector: u64, total_sectors: u64) -> i32;
    fn nyx_fs_read_file(path: *const u8, offset: u32, buf: *mut u8, len: u32) -> i32;
    fn nyx_fs_write_file(path: *const u8, offset: u32, buf: *const u8, len: u32) -> i32;
    fn nyx_fs_get_size(path: *const u8) -> i32;
    
    fn nyx_fs_create_file(path: *const u8) -> i32; 
    fn nyx_fs_create_dir(path: *const u8) -> i32; 
    
    // Milestones 1.3 & 1.7 Additions
    fn nyx_fs_delete_file(path: *const u8) -> i32;
    fn nyx_fs_sync(path: *const u8) -> i32;
    
    // The directory lister
    fn nyx_fs_list_dir(
        path: *const u8, 
        cb: extern "C" fn(*const u8, u8, *mut u8), 
        ctx: *mut u8
    );
}

// The callback that catches the C-strings and turns them into Rust Strings
extern "C" fn dir_entry_callback(name_ptr: *const u8, inode_type: u8, ctx: *mut u8) {
    unsafe {
        let list = &mut *(ctx as *mut Vec<String>);
        let mut len = 0;
        while *name_ptr.add(len) != 0 { len += 1; }
        
        let slice = core::slice::from_raw_parts(name_ptr, len);
        if let Ok(s) = core::str::from_utf8(slice) {
            if s != "." && s != ".." {
                let mut entry = String::from(s);
                if inode_type == 2 { entry.push('/'); }
                list.push(entry);
            }
        }
    }
}

// ==========================================
// THE LWEXT4 BRIDGE DRIVER FOR THE VFS
// ==========================================
pub struct NvmeLwExt4Fs;

impl NvmeLwExt4Fs {
    pub fn new() -> Option<Self> {
        let driver = unsafe { GLOBAL_NVME.as_mut()? };
        let mut start_lba = 0;
        let mut size_sectors = 0;
        let mut last_err = -1;

        for gpt_lba in 2..=33 {
            let mut align_buf = alloc::vec![0u8; 8192];
            let ptr_addr = align_buf.as_ptr() as usize;
            let offset = (4096 - (ptr_addr % 4096)) % 4096;
            
            let entry_block = unsafe { 
                core::slice::from_raw_parts_mut(align_buf.as_mut_ptr().add(offset), 4096) 
            };

            if driver.read_block(gpt_lba, entry_block) {
                for i in 0..32 {
                    let off = i * 128;
                    
                    let mut type_guid = [0u8; 16];
                    type_guid.copy_from_slice(&entry_block[off..off+16]);
                    
                    const LINUX_FS_GUID: [u8; 16] = [
                        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47,
                        0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4
                    ];

                    if type_guid == LINUX_FS_GUID {
                        let lba = u64::from_le_bytes(entry_block[off+32..off+40].try_into().unwrap());
                        let end_lba = u64::from_le_bytes(entry_block[off+40..off+48].try_into().unwrap());
                        
                        if end_lba > lba {
                            let sectors = end_lba - lba;
                            let err_code = unsafe { nyx_fs_mount(lba, sectors) };
                            
                            if err_code == 0 {
                                start_lba = lba;
                                size_sectors = sectors;
                                break;
                            } else {
                                last_err = err_code;
                            }
                        }
                    }
                }
            }
            if start_lba != 0 {
                break;
            }
        }

        if start_lba == 0 {
            panic!("VFS FATAL: GPT scanned, but no compatible Ext4 partition could be mounted! (Last POSIX Error: {})", last_err);
        }
        
        Some(Self)
    }
}

fn to_c_path(path: &str) -> Vec<u8> {
    let mut clean = path.trim_start_matches('/');
    if clean.starts_with("mnt/nvme/") { clean = &clean["mnt/nvme/".len()..]; }
    alloc::format!("/mnt/{}\0", clean).into_bytes()
}

impl crate::vfs::FileSystem for NvmeLwExt4Fs {
    fn read_file(&self, path: &str, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let c_path = to_c_path(path);
        let res = unsafe { nyx_fs_read_file(c_path.as_ptr(), offset as u32, buf.as_mut_ptr(), buf.len() as u32) };
        if res >= 0 { Ok(res as usize) } else { Err(FsError::IoError) }
    }

    fn write_file(&mut self, path: &str, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let c_path = to_c_path(path);
        let res = unsafe { nyx_fs_write_file(c_path.as_ptr(), offset as u32, buf.as_ptr(), buf.len() as u32) };
        if res >= 0 { Ok(res as usize) } else { Err(FsError::IoError) }
    }

    fn get_file_size(&self, path: &str) -> Result<usize, FsError> {
        let c_path = to_c_path(path);
        let res = unsafe { nyx_fs_get_size(c_path.as_ptr()) };
        if res >= 0 { Ok(res as usize) } else { Err(FsError::NotFound) }
    }

    fn create_file(&mut self, path: &str) -> Result<(), FsError> {
        let c_path = to_c_path(path);
        if unsafe { nyx_fs_create_file(c_path.as_ptr()) == 1 } { Ok(()) } else { Err(FsError::IoError) }
    }
    
    fn create_dir(&mut self, path: &str) -> Result<(), FsError> {
        let c_path = to_c_path(path);
        if unsafe { nyx_fs_create_dir(c_path.as_ptr()) == 1 } { Ok(()) } else { Err(FsError::IoError) }
    }
    
    fn delete_file(&mut self, path: &str) -> Result<(), FsError> {
        let c_path = to_c_path(path);
        if unsafe { nyx_fs_delete_file(c_path.as_ptr()) == 1 } { Ok(()) } else { Err(FsError::IoError) }
    }

    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let c_path = to_c_path(path);
        let mut list: Vec<String> = Vec::new();
        unsafe {
            let ctx = &mut list as *mut _ as *mut u8;
            nyx_fs_list_dir(c_path.as_ptr(), dir_entry_callback, ctx);
        }
        Ok(list)
    }

    //  Milestone 1.7: Actually flushes the Ext4 block cache to the NVMe SSD
    fn sync(&mut self) -> Result<(), FsError> {
        let c_path = alloc::format!("/mnt/\0").into_bytes();
        if unsafe { nyx_fs_sync(c_path.as_ptr()) == 1 } { Ok(()) } else { Err(FsError::IoError) }
    }
}