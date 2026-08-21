use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use spin::Mutex;

lazy_static::lazy_static! {
    pub static ref VFS: VirtualFileSystem = VirtualFileSystem::new();
}

// ==========================================
// 1. THE HARDWARE DRIVER ABSTRACTION
// ==========================================

// 🔥 MILESTONE 1.5: FsError Enum Added for POSIX-style OS Error Codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    IoError,
    InvalidPath,
    OutOfSpace,
    Unsupported,
    PermissionDenied,
}

// F: filesystem capacity snapshot (total/free bytes + block size), from a driver that can report it.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatFs {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub block_size: u32,
}

/// Phase 1: everything `stat(2)` needs about one path.
///
/// These are **real on-disk values**, not synthesised ones. The plan for this phase budgeted a path
/// hash for `ino` and the current RTC reading for `mtime`, on the assumption that the VFS had no
/// inode or timestamp concept — reading the vendored lwext4 headers showed it has both. That matters
/// beyond tidiness: anything that caches on mtime (make, rsync, a build system, a browser cache)
/// silently misbehaves against a clock that reports "now" for every file, and a hashed inode number
/// makes hardlink detection and `same_file` checks quietly wrong.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileStat {
    pub size: u64,
    /// Full POSIX mode word — file-type bits included, which is what makes `is_dir` real rather
    /// than a guess based on "did list_dir return anything".
    pub mode: u32,
    pub ino: u32,
    pub atime: u32,
    pub mtime: u32,
    pub ctime: u32,
}

/// POSIX file-type bits, as they appear in the top of `st_mode`.
pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;

impl FileStat {
    pub fn is_dir(&self) -> bool { self.mode & S_IFMT == S_IFDIR }
    pub fn is_file(&self) -> bool { self.mode & S_IFMT == S_IFREG }
    pub fn is_symlink(&self) -> bool { self.mode & S_IFMT == S_IFLNK }
}

/// Any storage driver (NVMe, AHCI, TAR RAMFS) must implement this trait.
pub trait FileSystem: Send + Sync {
    /// Reads up to buf.len() bytes from the file at the given offset.
    fn read_file(&self, path: &str, offset: usize, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Writes buf.len() bytes to the file at the given offset.
    fn write_file(&mut self, path: &str, offset: usize, buf: &[u8]) -> Result<usize, FsError>;

    /// Gets the total size of the file in bytes.
    fn get_file_size(&self, path: &str) -> Result<usize, FsError>;

    // Default implementations gracefully fail for read-only systems (like TarFs)
    fn create_file(&mut self, _path: &str) -> Result<(), FsError> { Err(FsError::Unsupported) }
    fn create_dir(&mut self, _path: &str) -> Result<(), FsError> { Err(FsError::Unsupported) }
    fn list_dir(&self, _path: &str) -> Result<Vec<String>, FsError> { Err(FsError::Unsupported) }

    // 🔥 MILESTONE 1.3: Delete File Added
    fn delete_file(&mut self, _path: &str) -> Result<(), FsError> { Err(FsError::Unsupported) }

    // --- Phase 1 (POSIX floor). Defaults refuse rather than pretend: a read-only driver that
    // silently returned Ok(()) from `rename` would make the syscall a STUB, which is the one
    // outcome worse than not having it. ---
    fn stat(&self, _path: &str) -> Result<FileStat, FsError> { Err(FsError::Unsupported) }
    fn remove_dir(&mut self, _path: &str) -> Result<(), FsError> { Err(FsError::Unsupported) }
    fn rename(&mut self, _from: &str, _to: &str) -> Result<(), FsError> { Err(FsError::Unsupported) }
    fn symlink(&mut self, _target: &str, _path: &str) -> Result<(), FsError> { Err(FsError::Unsupported) }

    // 🔥 MILESTONE 1.7: Sync/Flush to commit Journal to physical disk
    fn sync(&mut self) -> Result<(), FsError> { Ok(()) }

    // F: total/free capacity of this filesystem. Default None for drivers that can't report it.
    fn statfs(&self) -> Option<StatFs> { None }

    // --- WAL (Write-Ahead Logging) Hooks ---
    fn begin_transaction(&mut self) -> u64 { 0 }
    fn commit_transaction(&mut self, _tx_id: u64) -> bool { true }
    fn rollback_transaction(&mut self, _tx_id: u64) {}
}

// ==========================================
// 2. THE WRITE-AHEAD LOGGING (WAL) ENGINE
// ==========================================
#[derive(Clone, Copy)]
#[repr(C)]
pub struct WalEntry {
    pub magic: u32,       // 0x57414C21 ("WAL!")
    pub tx_id: u64,       // Unique Transaction ID
    pub operation: u8,    // 1 = Create, 2 = Write, 3 = Delete
    pub target_block: u64,// The physical disk block being modified
    pub data_length: u32, // Length of the payload
    pub checksum: u32,    // CRC32 to ensure journal entry didn't corrupt during power loss
}

pub struct WriteAheadLog {
    pub journal_start_sector: u64,
    pub current_tx: u64,
}

impl WriteAheadLog {
    pub const fn new(start_sector: u64) -> Self {
        Self {
            journal_start_sector: start_sector,
            current_tx: 1,
        }
    }
}

// ==========================================
// 3. THE MOUNT MANAGER (VFS)
// ==========================================
pub struct VirtualFileSystem {
    // Maps a path (e.g., "/bin") to its physical driver (e.g., TarFs or NvmeFs)
    mounts: Mutex<BTreeMap<String, Box<dyn FileSystem>>>,
}

impl VirtualFileSystem {
    pub const fn new() -> Self {
        Self { 
            mounts: Mutex::new(BTreeMap::new()) 
        }
    }

    pub fn mount(&self, path: &str, fs: Box<dyn FileSystem>) -> bool {
        let mut mounts = self.mounts.lock();
        let clean_path = if path.ends_with('/') && path.len() > 1 {
            &path[..path.len() - 1]
        } else {
            path
        };
        if mounts.contains_key(clean_path) { return false; } 
        mounts.insert(String::from(clean_path), fs);
        true
    }
    
    pub fn unmount(&self, path: &str) -> bool {
        let mut mounts = self.mounts.lock();
        let clean_path = if path.ends_with('/') && path.len() > 1 {
            &path[..path.len() - 1]
        } else {
            path
        };
        mounts.remove(clean_path).is_some()
    }

    fn resolve_mount<'a>(&'a self, path: &str) -> Option<(String, String)> {
        let mounts = self.mounts.lock();
        let search_path = if !path.starts_with('/') {
            alloc::format!("/{}", path)
        } else {
            String::from(path)
        };

        for (mount_path, _fs) in mounts.iter().rev() {
            if search_path.starts_with(mount_path) {
                let relative_path = if mount_path == "/" {
                    search_path.clone()
                } else {
                    String::from(&search_path[mount_path.len()..])
                };
                
                let safe_relative = if relative_path.is_empty() {
                    String::from("/")
                } else if !relative_path.starts_with('/') {
                    alloc::format!("/{}", relative_path)
                } else {
                    relative_path
                };

                return Some((mount_path.clone(), safe_relative));
            }
        }
        None
    }

    // ==========================================
    // GLOBAL VFS SYSTEM CALL ROUTERS
    // ==========================================
    
    pub fn read_file_alloc(&self, path: &str) -> Option<Vec<u8>> {
        let (mount_point, relative_path) = self.resolve_mount(path)?;
        let mounts = self.mounts.lock();
        let driver = mounts.get(&mount_point)?;

        let size = driver.get_file_size(&relative_path).ok()?;
        let mut buf = alloc::vec![0u8; size];
        
        let bytes_read = driver.read_file(&relative_path, 0, &mut buf).ok()?;
        if bytes_read == size {
            Some(buf)
        } else {
            None
        }
    }

    // F: public size lookup (the trait's get_file_size was only reachable internally before). Returns
    // None if the path doesn't resolve to a mount or the driver can't stat it (e.g. it's a directory).
    pub fn file_size(&self, path: &str) -> Option<usize> {
        let (mount_point, relative_path) = self.resolve_mount(path)?;
        let mounts = self.mounts.lock();
        let driver = mounts.get(&mount_point)?;
        driver.get_file_size(&relative_path).ok()
    }

    // F: capacity of the filesystem backing `path` (total/free bytes). None if the mount can't report.
    pub fn statfs(&self, path: &str) -> Option<StatFs> {
        let (mount_point, _relative_path) = self.resolve_mount(path)?;
        let mounts = self.mounts.lock();
        let driver = mounts.get(&mount_point)?;
        driver.statfs()
    }
    
    pub fn list_dir(&self, path: &str) -> Vec<String> {
        let mut results = Vec::new();
        
        let mounts = self.mounts.lock();
        let search_path = if !path.starts_with('/') { alloc::format!("/{}", path) } else { String::from(path) };
        
        for mount_path in mounts.keys() {
            if mount_path != "/" && mount_path.starts_with(&search_path) {
                let remainder = mount_path[search_path.len()..].trim_start_matches('/');
                let folder_name = remainder.split('/').next().unwrap_or("");
                
                if !folder_name.is_empty() {
                    results.push(String::from(folder_name));
                }
            }
        }
        drop(mounts);

        if let Some((mount_point, relative_path)) = self.resolve_mount(path) {
            let mounts = self.mounts.lock();
            if let Some(driver) = mounts.get(&mount_point) {
                if let Ok(driver_files) = driver.list_dir(&relative_path) {
                    results.extend(driver_files);
                }
            }
        }

        results.sort();
        results.dedup();
        results
    }
    
    pub fn open_path(&self, path: &str) -> Option<String> {
        if self.resolve_mount(path).is_some() {
            Some(String::from(path))
        } else {
            None
        }
    }
    
    pub fn create_dir(&self, path: &str) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.create_dir(&rel_path).is_ok();
            }
        }
        false
    }

    pub fn create_file(&self, path: &str) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.create_file(&rel_path).is_ok();
            }
        }
        false
    }

    pub fn write_file(&self, path: &str, buf: &[u8]) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.write_file(&rel_path, 0, buf).is_ok();
            }
        }
        false
    }
    
    pub fn delete_file(&self, path: &str) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.delete_file(&rel_path).is_ok();
            }
        }
        false
    }

    // --- Phase 1 (POSIX floor) ---

    /// Metadata for one path. `None` means the path does not resolve to a mount, or the driver
    /// cannot stat it — the caller turns that into ENOENT.
    pub fn stat(&self, path: &str) -> Option<FileStat> {
        let (mount_point, rel_path) = self.resolve_mount(path)?;
        let mounts = self.mounts.lock();
        let driver = mounts.get(&mount_point)?;
        driver.stat(&rel_path).ok()
    }

    /// Remove an EMPTY directory. Deliberately separate from `delete_file`: the underlying
    /// `ext4_fremove` fails on a directory, so routing rmdir through it would have reported failure
    /// for the correct case and never removed anything.
    pub fn remove_dir(&self, path: &str) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.remove_dir(&rel_path).is_ok();
            }
        }
        false
    }

    /// Rename within a single mount. **Cross-mount renames are refused**, not silently turned into
    /// a copy: `rename(2)` promises atomicity, and a copy+delete across two filesystems is neither
    /// atomic nor reversible if it fails halfway.
    pub fn rename(&self, from: &str, to: &str) -> bool {
        let (from_mount, from_rel) = match self.resolve_mount(from) { Some(v) => v, None => return false };
        let (to_mount, to_rel) = match self.resolve_mount(to) { Some(v) => v, None => return false };
        if from_mount != to_mount { return false; }

        let mut mounts = self.mounts.lock();
        if let Some(driver) = mounts.get_mut(&from_mount) {
            return driver.rename(&from_rel, &to_rel).is_ok();
        }
        false
    }

    /// Create a symlink at `path` pointing at `target`. `target` is stored verbatim (it may be
    /// relative, and it need not exist), so only `path` is resolved to a mount.
    pub fn symlink(&self, target: &str, path: &str) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.symlink(target, &rel_path).is_ok();
            }
        }
        false
    }
}

// ==========================================
// 4. LEGACY FILE DESCRIPTOR BRIDGES
// ==========================================
pub struct OpenFile {
    pub path: String,
    pub offset: spin::Mutex<usize>,
}

impl OpenFile {
    pub fn new(path: String) -> Self { 
        Self { path, offset: spin::Mutex::new(0) } 
    }

    pub fn read(&self, buf: &mut [u8]) -> usize {
        let mut off = self.offset.lock();
        
        if let Some((mount_point, rel_path)) = VFS.resolve_mount(&self.path) {
            let mounts = VFS.mounts.lock();
            if let Some(driver) = mounts.get(&mount_point) {
                if let Ok(bytes_read) = driver.read_file(&rel_path, *off, buf) {
                    *off += bytes_read;
                    return bytes_read;
                }
            }
        }
        0 // EOF or Error
    }

    // B-γ.2: real write-through at the fd's current offset (mirrors read). The ext4 driver
    // (lwext4) is R/W; only this bridge was a stub. Advances the shared offset so sequential
    // write() calls append correctly.
    pub fn write(&self, buf: &[u8]) -> usize {
        let mut off = self.offset.lock();

        if let Some((mount_point, rel_path)) = VFS.resolve_mount(&self.path) {
            let mut mounts = VFS.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                if let Ok(bytes_written) = driver.write_file(&rel_path, *off, buf) {
                    *off += bytes_written;
                    return bytes_written;
                }
            }
        }
        0
    }

    pub fn mmap(&self, _offset: usize, _size: usize) -> Result<u64, i64> {
        Err(-12) // ENOMEM
    }

    pub fn ioctl(&self, _cmd: usize, _arg: usize) -> Result<usize, i64> {
        Err(-25) // ENOTTY (Not a terminal)
    }
}