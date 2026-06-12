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
/// Any storage driver (NVMe, AHCI, TAR RAMFS) must implement this trait.
pub trait FileSystem: Send + Sync {
    /// Reads up to buf.len() bytes from the file at the given offset.
    fn read_file(&self, path: &str, offset: usize, buf: &mut [u8]) -> Option<usize>;
    
    /// Writes buf.len() bytes to the file at the given offset.
    fn write_file(&mut self, path: &str, offset: usize, buf: &[u8]) -> Option<usize>;
    
    /// Gets the total size of the file in bytes.
    fn get_file_size(&self, path: &str) -> Option<usize>;
    
    /// Creates a new empty file. Returns true if successful.
    fn create_file(&mut self, path: &str) -> bool;
    
    /// Creates a new directory. Returns true if successful.
    fn create_dir(&mut self, path: &str) -> bool;
    
    /// Lists all files and folders inside a directory.
    fn list_dir(&self, path: &str) -> Vec<String>;
    
    // --- WAL (Write-Ahead Logging) Hooks ---
    // Default implementations do nothing, allowing read-only filesystems 
    // (like our initial TAR Initramfs) to ignore them safely.
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

    /// Mounts a hardware driver to a specific path in the OS
    pub fn mount(&self, path: &str, fs: Box<dyn FileSystem>) -> bool {
        let mut mounts = self.mounts.lock();
        
        // Ensure path starts with '/' and strip trailing slashes for consistency
        let clean_path = if path.ends_with('/') && path.len() > 1 {
            &path[..path.len() - 1]
        } else {
            path
        };

        if mounts.contains_key(clean_path) { return false; } // Already mounted
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

        // Remove the driver from the BTreeMap and drop it from memory
        mounts.remove(clean_path).is_some()
    }

    /// Finds which hardware driver owns a specific file path
    fn resolve_mount<'a>(&'a self, path: &str) -> Option<(String, String)> {
        let mounts = self.mounts.lock();
        
        // Ensure the path is absolute
        let search_path = if !path.starts_with('/') {
            alloc::format!("/{}", path)
        } else {
            String::from(path)
        };

        // We use `.rev()` to match the deepest mount paths first.
        // e.g., "/mnt/usb" matches before "/mnt"
        for (mount_path, _fs) in mounts.iter().rev() {
            if search_path.starts_with(mount_path) {
                // Extract the remainder of the path to pass to the driver
                // If mount is "/bin" and search is "/bin/nyx-user", relative is "/nyx-user"
                let relative_path = if mount_path == "/" {
                    search_path.clone()
                } else {
                    String::from(&search_path[mount_path.len()..])
                };
                
                // Ensure relative path always starts with '/' for the driver
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

        // Find file size so we can allocate the exact right amount of RAM
        let size = driver.get_file_size(&relative_path)?;
        let mut buf = alloc::vec![0u8; size];
        
        let bytes_read = driver.read_file(&relative_path, 0, &mut buf)?;
        if bytes_read == size {
            Some(buf)
        } else {
            None
        }
    }
    pub fn list_dir(&self, path: &str) -> Vec<String> {
        let mut results = Vec::new();
        
        // 1. Check if the path itself is a virtual mount point root
        let mounts = self.mounts.lock();
        let search_path = if !path.starts_with('/') { alloc::format!("/{}", path) } else { String::from(path) };
        
        for mount_path in mounts.keys() {
            if mount_path != "/" && mount_path.starts_with(&search_path) {
                let remainder = mount_path[search_path.len()..].trim_start_matches('/');
                
                // 🚨 THE FIX: Extract just the next folder level!
                // This correctly pulls "mnt" out of "/mnt/nvme" so the Explorer can see it.
                let folder_name = remainder.split('/').next().unwrap_or("");
                
                if !folder_name.is_empty() {
                    results.push(String::from(folder_name));
                }
            }
        }
        drop(mounts);

        // 2. Ask the hardware driver for its internal files
        if let Some((mount_point, relative_path)) = self.resolve_mount(path) {
            let mounts = self.mounts.lock();
            if let Some(driver) = mounts.get(&mount_point) {
                let driver_files = driver.list_dir(&relative_path);
                results.extend(driver_files);
            }
        }

        // Deduplicate in case multiple drives are mounted in the same virtual folder
        results.sort();
        results.dedup();
        results
    }
    pub fn open_path(&self, path: &str) -> Option<String> {
        // Verify it exists in the VFS, then return the path for the FD table
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
                return driver.create_dir(&rel_path);
            }
        }
        false
    }

    pub fn create_file(&self, path: &str) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.create_file(&rel_path);
            }
        }
        false
    }

    pub fn write_file(&self, path: &str, buf: &[u8]) -> bool {
        if let Some((mount_point, rel_path)) = self.resolve_mount(path) {
            let mut mounts = self.mounts.lock();
            if let Some(driver) = mounts.get_mut(&mount_point) {
                return driver.write_file(&rel_path, 0, buf).is_some();
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
        
        // 1. Ask the VFS which driver owns this file
        if let Some((mount_point, rel_path)) = VFS.resolve_mount(&self.path) {
            let mounts = VFS.mounts.lock();
            if let Some(driver) = mounts.get(&mount_point) {
                // 2. Ask the driver to read the bytes directly!
                if let Some(bytes_read) = driver.read_file(&rel_path, *off, buf) {
                    *off += bytes_read;
                    return bytes_read;
                }
            }
        }
        0 // EOF or Error
    }

    pub fn write(&self, buf: &[u8]) -> usize {
        // Our TarFS Initramfs is Read-Only! 
        // When you write your NvmeFs driver later, you will map this to driver.write_file()
        0 
    }

    pub fn mmap(&self, _offset: usize, _size: usize) -> Result<u64, i64> {
        Err(-12) // ENOMEM
    }

    pub fn ioctl(&self, _cmd: usize, _arg: usize) -> Result<usize, i64> {
        Err(-25) // ENOTTY (Not a terminal)
    }
}