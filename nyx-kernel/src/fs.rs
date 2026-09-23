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

/// Sectors per cached chunk. 8 x 512 = 4096, which is exactly what one NVMe command can move
/// through PRP1 alone (see `NvmeDriver::read_blocks`).
const CHUNK_SECTORS: u64 = 8;

/// One chunk of the disk, and which chunk it is. `u64::MAX` = empty.
///
/// A single entry, not a real cache: lwext4 walks a file sequentially, so consecutive 512-byte
/// requests almost always fall in the same 4 KiB chunk. One entry turns eight round trips into one;
/// anything larger buys little and would need eviction logic.
static mut CHUNK_BUF: [u8; 4096] = [0; 4096];
static mut CHUNK_BASE: u64 = u64::MAX;

/// Set once at boot by `nvme_enable_fast_reads` after the device has proven multi-block reads work.
static mut MULTIBLOCK_OK: bool = false;

/// Called after the NVMe driver is up. Gates the 4 KiB read path on the device actually agreeing
/// that 8 logical blocks are 4096 bytes — see `NvmeDriver::verify_multiblock`.
pub fn nvme_enable_fast_reads() {
    unsafe {
        if let Some(ref mut driver) = GLOBAL_NVME {
            MULTIBLOCK_OK = driver.verify_multiblock();
            CHUNK_BASE = u64::MAX;
            if MULTIBLOCK_OK {
                crate::serial_println!("[NVME] multi-block reads verified; 4 KiB chunking enabled.");
            } else {
                crate::serial_println!(
                    "[NVME] multi-block read check FAILED; staying on one block per request.");
            }
        }
    }
}

/// Drop the cached chunk. Must be called by every write path — a stale chunk would hand back
/// pre-write data and silently corrupt whatever read it next.
#[inline]
fn invalidate_chunk() {
    unsafe { CHUNK_BASE = u64::MAX; }
}

#[no_mangle]
pub extern "C" fn nyx_nvme_read_block(sector: u64, buf: *mut u8) -> bool {
    unsafe {
        let driver = match GLOBAL_NVME {
            Some(ref mut d) => d,
            None => return false,
        };

        // ★ This used to `alloc::vec![0u8; 8192]` per 512-byte sector, purely to hand the driver a
        // 4 KiB-aligned slice — which the driver never needed: it DMAs into its own aligned
        // `DATA_BUF` and copies out. So every sector paid an 8 KiB allocation AND its zeroing, then
        // a free, all under the allocator's `without_interrupts`. Reading a 1.4 MB binary meant
        // ~2,900 of those: some 23 MB of memset to deliver 1.4 MB.
        //
        // Worse, each sector was a separate NVMe round trip. With SYSCALL masking interrupts and
        // the driver polling, that serialised ~2,900 device latencies with the timer dead —
        // measured as **102 ms inside one `execve`**, the largest interrupts-off window on the
        // machine.
        if !MULTIBLOCK_OK {
            // Device did not prove multi-block reads; behave as before, minus the pointless
            // allocation.
            if driver.read_block(sector, &mut CHUNK_BUF) {
                core::ptr::copy_nonoverlapping(CHUNK_BUF.as_ptr(), buf, 512);
                return true;
            }
            return false;
        }

        let base = sector & !(CHUNK_SECTORS - 1);
        if CHUNK_BASE != base {
            if !driver.read_blocks(base, CHUNK_SECTORS as u16, &mut CHUNK_BUF) {
                CHUNK_BASE = u64::MAX;
                return false;
            }
            CHUNK_BASE = base;
        }
        let off = ((sector - base) * 512) as usize;
        core::ptr::copy_nonoverlapping(CHUNK_BUF.as_ptr().add(off), buf, 512);
        true
    }
}

#[no_mangle]
pub extern "C" fn nyx_nvme_write_block(sector: u64, buf: *const u8) -> bool {
    unsafe {
        // ⚠️ FIRST, and unconditionally. The read path caches a 4 KiB chunk, and this sector may
        // sit inside the cached one — serving a later read from a chunk captured before this write
        // would hand back stale bytes and silently corrupt the filesystem. Invalidating even on the
        // failure paths below is deliberate: a write that may have partially landed must not leave
        // a chunk we still believe in.
        invalidate_chunk();

        let driver = match GLOBAL_NVME {
            Some(ref mut d) => d,
            None => return false,
        };

        // Writes stay one block per command. The read path batches because it is the hot path and
        // its access pattern is sequential; batching writes would need read-modify-write of the
        // surrounding chunk, which is a correctness risk for no measured gain.
        //
        // A static staging buffer rather than the old per-call `vec![0u8; 8192]` — the driver DMAs
        // from its own aligned page, so the alignment that allocation existed to arrange was never
        // used. Distinct from CHUNK_BUF so a write cannot clobber a chunk mid-read.
        static mut WRITE_BUF: [u8; 4096] = [0; 4096];
        core::ptr::copy_nonoverlapping(buf, WRITE_BUF.as_mut_ptr(), 512);
        driver.write_block(sector, &WRITE_BUF)
    }
}

extern "C" {
    fn nyx_fs_mount(start_sector: u64, total_sectors: u64) -> i32;
    fn nyx_fs_read_file(path: *const u8, offset: u32, buf: *mut u8, len: u32) -> i32;
    fn nyx_fs_write_file(path: *const u8, offset: u32, buf: *const u8, len: u32) -> i32;
    fn nyx_fs_get_size(path: *const u8) -> i32;
    fn nyx_fs_statfs(total_bytes: *mut u64, free_bytes: *mut u64, block_size: *mut u32) -> i32;
    
    fn nyx_fs_create_file(path: *const u8) -> i32; 
    fn nyx_fs_create_dir(path: *const u8) -> i32; 
    
    // Milestones 1.3 & 1.7 Additions
    fn nyx_fs_delete_file(path: *const u8) -> i32;
    fn nyx_fs_sync(path: *const u8) -> i32;

    // Phase 1 (POSIX floor): metadata + namespace operations. All of these are thin covers over
    // lwext4 calls that were already compiled in and simply never reached from Rust.
    fn nyx_fs_stat(
        path: *const u8,
        out_mode: *mut u32,
        out_size: *mut u64,
        out_ino: *mut u32,
        out_atime: *mut u32,
        out_mtime: *mut u32,
        out_ctime: *mut u32,
    ) -> i32;
    fn nyx_fs_remove_dir(path: *const u8) -> i32;
    fn nyx_fs_rename(path: *const u8, new_path: *const u8) -> i32;
    fn nyx_fs_symlink(target: *const u8, path: *const u8) -> i32;
    
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

    // --- Phase 1 (POSIX floor) ---

    fn stat(&self, path: &str) -> Result<crate::vfs::FileStat, FsError> {
        let c_path = to_c_path(path);
        let mut st = crate::vfs::FileStat::default();
        let ok = unsafe {
            nyx_fs_stat(
                c_path.as_ptr(),
                &mut st.mode,
                &mut st.size,
                &mut st.ino,
                &mut st.atime,
                &mut st.mtime,
                &mut st.ctime,
            )
        };
        if ok == 1 { Ok(st) } else { Err(FsError::NotFound) }
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), FsError> {
        let c_path = to_c_path(path);
        if unsafe { nyx_fs_remove_dir(c_path.as_ptr()) == 1 } { Ok(()) } else { Err(FsError::IoError) }
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let c_from = to_c_path(from);
        let c_to = to_c_path(to);
        if unsafe { nyx_fs_rename(c_from.as_ptr(), c_to.as_ptr()) == 1 } { Ok(()) } else { Err(FsError::IoError) }
    }

    fn symlink(&mut self, target: &str, path: &str) -> Result<(), FsError> {
        // The target is stored verbatim in the link, so it must NOT go through to_c_path — that
        // rewrites /mnt/nvme/... into the driver's own /mnt/... namespace, which is correct for a
        // path being resolved and wrong for a string being recorded. It still needs a NUL.
        let mut c_target = alloc::string::String::from(target).into_bytes();
        c_target.push(0);
        let c_path = to_c_path(path);
        if unsafe { nyx_fs_symlink(c_target.as_ptr(), c_path.as_ptr()) == 1 } { Ok(()) } else { Err(FsError::IoError) }
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

    // F: total/free capacity of the ext4 partition (lwext4 ext4_mount_point_stats via the C wrapper).
    fn statfs(&self) -> Option<crate::vfs::StatFs> {
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let mut bsize: u32 = 0;
        let rc = unsafe { nyx_fs_statfs(&mut total, &mut free, &mut bsize) };
        if rc == 0 {
            Some(crate::vfs::StatFs { total_bytes: total, free_bytes: free, block_size: bsize })
        } else {
            None
        }
    }
}