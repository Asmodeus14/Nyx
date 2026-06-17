use alloc::string::String;
use alloc::vec::Vec;
use core::convert::TryInto;
use crate::drivers::nvme::NvmeDriver;
use alloc::boxed::Box;

// ==========================================
// C-FFI HARDWARE BRIDGE (DMA ALIGNED)
// ==========================================
pub static mut GLOBAL_NVME: Option<NvmeDriver> = None;

#[no_mangle]
pub extern "C" fn nyx_nvme_read_block(sector: u64, buf: *mut u8) -> bool {
    unsafe {
        if let Some(ref mut driver) = GLOBAL_NVME {
            let mut align_buf = alloc::vec![0u8; 8192];
            let ptr_addr = align_buf.as_ptr() as usize;
            let offset = (4096 - (ptr_addr % 4096)) % 4096;
            
            // We MUST pass 4096 to bypass the hardcoded safety check in nvme.rs!
            let slice_4k = core::slice::from_raw_parts_mut(align_buf.as_mut_ptr().add(offset), 4096);
            
            if driver.read_block(sector, slice_4k) {
                // We only copy the 512 valid logical block bytes over to the C-Library
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
            
            // Generate a 4096 byte slice
            let slice_4k = core::slice::from_raw_parts_mut(align_buf.as_mut_ptr().add(offset), 4096);
            
            // Populate the first 512 bytes with what C wants to write to the SSD
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
    fn nyx_fs_create_dir(path: *const u8) -> i32; 
    
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
        // Cast the context pointer back into a Rust Vec<String> reference
        let list = &mut *(ctx as *mut Vec<String>);
        
        // Find the length of the null-terminated C string
        let mut len = 0;
        while *name_ptr.add(len) != 0 { len += 1; }
        
        // Convert to Rust string
        let slice = core::slice::from_raw_parts(name_ptr, len);
        if let Ok(s) = core::str::from_utf8(slice) {
            // Ignore the hidden "." and ".." navigation folders
            if s != "." && s != ".." {
                let mut entry = String::from(s);
                
                // In standard Ext4, inode_type '2' means it is a Directory!
                // We append a slash so the GUI Explorer knows it is a folder.
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
                    let type_guid_1 = u64::from_le_bytes(entry_block[off..off+8].try_into().unwrap());
                    
                    if type_guid_1 != 0 {
                        let lba = u64::from_le_bytes(entry_block[off+32..off+40].try_into().unwrap());
                        let end_lba = u64::from_le_bytes(entry_block[off+40..off+48].try_into().unwrap());
                        
                        if end_lba > lba {
                            let sectors = end_lba - lba;
                            let size_mb_512 = (sectors * 512) / (1024 * 1024);
                            let size_mb_4k = (sectors * 4096) / (1024 * 1024);

                            if (size_mb_512 > 90_000 && size_mb_512 < 110_000) || (size_mb_4k > 90_000 && size_mb_4k < 110_000) {
                                start_lba = lba;
                                size_sectors = sectors;
                            }
                        }
                    }
                }
            }
        }

        if start_lba == 0 {
            panic!("VFS FATAL: GPT scanned successfully, but the 100GB Partition was missing!");
        }
        
        let err_code = unsafe { nyx_fs_mount(start_lba, size_sectors) };
        
        if err_code == 0 {
            Some(Self)
        } else {
            panic!("VFS FATAL: lwext4 rejected LBA {}! POSIX Error: {} (22=Invalid, 5=IO Error)", start_lba, err_code);
        }
    }
}

fn to_c_path(path: &str) -> Vec<u8> {
    let mut clean = path.trim_start_matches('/');
    if clean.starts_with("mnt/nvme/") { clean = &clean["mnt/nvme/".len()..]; }
    alloc::format!("/mnt/{}\0", clean).into_bytes()
}

impl crate::vfs::FileSystem for NvmeLwExt4Fs {
    fn read_file(&self, path: &str, offset: usize, buf: &mut [u8]) -> Option<usize> {
        let c_path = to_c_path(path);
        let res = unsafe { nyx_fs_read_file(c_path.as_ptr(), offset as u32, buf.as_mut_ptr(), buf.len() as u32) };
        if res > 0 { Some(res as usize) } else { None }
    }

    fn write_file(&mut self, path: &str, offset: usize, buf: &[u8]) -> Option<usize> {
        let c_path = to_c_path(path);
        let res = unsafe { nyx_fs_write_file(c_path.as_ptr(), offset as u32, buf.as_ptr(), buf.len() as u32) };
        if res > 0 { Some(res as usize) } else { None }
    }

    fn get_file_size(&self, path: &str) -> Option<usize> {
        let c_path = to_c_path(path);
        let res = unsafe { nyx_fs_get_size(c_path.as_ptr()) };
        if res >= 0 { Some(res as usize) } else { None }
    }

    fn create_file(&mut self, _path: &str) -> bool { true }
    
    fn create_dir(&mut self, path: &str) -> bool {
        let c_path = to_c_path(path);
        unsafe { nyx_fs_create_dir(c_path.as_ptr()) == 1 }
    }
    
    //  Actually use the callback to populate the array!
    fn list_dir(&self, path: &str) -> Vec<String> {
        let c_path = to_c_path(path);
        let mut list: Vec<String> = Vec::new();
        
        unsafe {
            // Pass a memory pointer of our local Vector over to C, so the callback knows where to push the strings!
            let ctx = &mut list as *mut _ as *mut u8;
            nyx_fs_list_dir(c_path.as_ptr(), dir_entry_callback, ctx);
        }
        
        list
    }
}