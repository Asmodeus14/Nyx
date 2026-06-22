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
    
    // 🔥 MILESTONE 1.7: Sync/Flush to commit Journal to physical disk
    fn sync(&mut self) -> Result<(), FsError> { Ok(()) }
    
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

    pub fn write(&self, _buf: &[u8]) -> usize { 0 }

    pub fn mmap(&self, _offset: usize, _size: usize) -> Result<u64, i64> {
        Err(-12) // ENOMEM
    }

    pub fn ioctl(&self, _cmd: usize, _arg: usize) -> Result<usize, i64> {
        Err(-25) // ENOTTY (Not a terminal)
    }
}