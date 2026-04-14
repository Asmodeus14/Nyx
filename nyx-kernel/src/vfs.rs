use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// The VNode trait represents anything that can act like a file.
/// This could be an NVMe text file, a Terminal, or a Hardware GPU.
pub trait VNode: Send + Sync {
    fn read(&self, _offset: usize, _buf: &mut [u8]) -> usize { 0 }
    fn write(&self, _offset: usize, _buf: &[u8]) -> usize { 0 }
    
    /// ioctl (I/O Control) is crucial for GPU drivers (DRM).
    fn ioctl(&self, _request: usize, _arg: usize) -> Result<usize, isize> {
        Err(-1) // -ENOSYS
    }

    /// Memory Mapping for VRAM
    fn mmap(&self, _offset: usize, _size: usize) -> Result<u64, isize> {
        Err(-1) // -ENOSYS
    }
}

/// An OpenFile tracks a specific process's interaction with a VNode.
pub struct OpenFile {
    pub node: Arc<dyn VNode>,
    pub offset: Mutex<usize>, // Tracks read/write position
}

impl OpenFile {
    pub fn new(node: Arc<dyn VNode>) -> Self {
        Self {
            node,
            offset: Mutex::new(0),
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> usize {
        let mut off = self.offset.lock();
        let bytes_read = self.node.read(*off, buf);
        *off += bytes_read;
        bytes_read
    }

    pub fn write(&self, buf: &[u8]) -> usize {
        let mut off = self.offset.lock();
        let bytes_written = self.node.write(*off, buf);
        *off += bytes_written;
        bytes_written
    }
}

// ==========================================
// MOCK IMPLEMENTATIONS FOR ROUTING DEMO
// ==========================================
pub struct NvmeFile {
    pub name: String,
    pub data: Mutex<Vec<u8>>,
}

impl VNode for NvmeFile {
    fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        let data = self.data.lock();
        if offset >= data.len() { return 0; }
        let bytes_to_read = core::cmp::min(buf.len(), data.len() - offset);
        buf[..bytes_to_read].copy_from_slice(&data[offset..offset + bytes_to_read]);
        bytes_to_read
    }
    
    fn write(&self, offset: usize, buf: &[u8]) -> usize {
        let mut data = self.data.lock();
        if offset + buf.len() > data.len() {
            data.resize(offset + buf.len(), 0);
        }
        data[offset..offset + buf.len()].copy_from_slice(buf);
        buf.len()
    }
}

pub struct DrmDevice;

impl VNode for DrmDevice {
    fn ioctl(&self, request: usize, arg: usize) -> Result<usize, isize> {
        crate::drm::handle_drm_ioctl(request, arg)
    }

    fn mmap(&self, offset: usize, size: usize) -> Result<u64, isize> {
        crate::serial_println!("[DRM] Mesa requested mmap! Offset: {:#x}, Size: {}", offset, size);
        Ok(0x1234_0000) 
    }
}

// ==========================================
// THE GLOBAL VFS ROUTER
// ==========================================
pub struct VfsManager {
    gpu_device: Arc<DrmDevice>,
}

impl VfsManager {
    pub fn new() -> Self {
        Self { gpu_device: Arc::new(DrmDevice) }
    }

    pub fn open_path(&self, path: &str) -> Option<Arc<dyn VNode>> {
        crate::serial_println!("[VFS] Intercepting open req for: {}", path);
        if path == "/dev/dri/card0" {
            return Some(self.gpu_device.clone() as Arc<dyn VNode>);
        } 
        let mock_file = Arc::new(NvmeFile {
            name: String::from(path),
            data: Mutex::new(Vec::new()),
        });
        Some(mock_file as Arc<dyn VNode>)
    }
}

lazy_static::lazy_static! {
    pub static ref VFS: VfsManager = VfsManager::new();
}