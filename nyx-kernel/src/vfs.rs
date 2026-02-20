use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// The VNode trait represents anything that can act like a file.
/// This could be an NVMe text file, a Terminal, or a Hardware GPU.
pub trait VNode: Send + Sync {
    fn read(&self, offset: usize, buf: &mut [u8]) -> usize { 0 }
    fn write(&self, offset: usize, buf: &[u8]) -> usize { 0 }
    
    /// ioctl (I/O Control) is crucial for GPU drivers (DRM).
    /// Mesa uses this to send hardware-specific commands to the GPU.
    fn ioctl(&self, _request: usize, _arg: usize) -> Result<usize, isize> {
        Err(-1) // -ENOSYS (Not implemented by default)
    }
}

/// An OpenFile tracks a specific process's interaction with a VNode.
/// If two apps open the same file, they share the VNode but have different offsets.
pub struct OpenFile {
    pub node: Arc<dyn VNode>,
    pub offset: Mutex<usize>,
}

impl OpenFile {
    pub fn new(node: Arc<dyn VNode>) -> Self {
        Self {
            node,
            offset: Mutex::new(0),
        }
    }
}

// ==========================================
// MOCK IMPLEMENTATIONS FOR ROUTING DEMO
// ==========================================

/// 1. A standard File on the NVMe Drive
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

/// 2. The Direct Rendering Manager (DRM) Hardware Device for Mesa/Vulkan
pub struct DrmDevice;

impl VNode for DrmDevice {
    fn ioctl(&self, request: usize, arg: usize) -> Result<usize, isize> {
        crate::vga_println!("[DRM] Received GPU ioctl req: {:#x}, arg: {:#x}", request, arg);
        // In Phase 4, we will parse the request to allocate GPU memory, 
        // submit command buffers, and ring the hardware doorbell.
        Ok(0) // Success
    }
}

// ==========================================
// THE GLOBAL VFS ROUTER
// ==========================================

pub struct VfsManager {
    // In a real OS this is a tree, but a flat list is perfect to start
    gpu_device: Arc<DrmDevice>,
}

impl VfsManager {
    pub fn new() -> Self {
        Self { gpu_device: Arc::new(DrmDevice) }
    }

    /// Resolves a string path into a hardware VNode
    pub fn open_path(&self, path: &str) -> Option<Arc<dyn VNode>> {
        crate::vga_println!("[VFS] Intercepting open req for: {}", path);
        
        if path == "/dev/dri/card0" {
            // Route directly to the GPU Hardware!
            return Some(self.gpu_device.clone() as Arc<dyn VNode>);
        } 
        
        // Otherwise, fallback to the physical NVMe File System
        // (We will wire this back into your real `fs.rs` soon)
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