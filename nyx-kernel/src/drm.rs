use core::slice;

// --- Linux DRM ioctl Request Codes ---
// These are the exact hex codes Mesa sends to the kernel
pub const DRM_IOCTL_VERSION: usize = 0xC0406400;
pub const DRM_IOCTL_GET_MAGIC: usize = 0x80046402;

// --- Linux DRM Structures ---
// Mesa passes a pointer to this struct, expecting the kernel to fill it out
#[repr(C)]
#[derive(Debug, Default)]
pub struct DrmVersion {
    pub version_major: i32,
    pub version_minor: i32,
    pub version_patchlevel: i32,
    pub name_len: usize,
    pub name: *mut u8,
    pub date_len: usize,
    pub date: *mut u8,
    pub desc_len: usize,
    pub desc: *mut u8,
}

pub fn handle_drm_ioctl(request: usize, arg: usize) -> Result<usize, isize> {
    match request {
        DRM_IOCTL_VERSION => {
            crate::serial_println!("[DRM] Mesa requested DRM_IOCTL_VERSION");
            
            // The 'arg' is a memory pointer provided by userspace
            let version_ptr = arg as *mut DrmVersion;
            
            unsafe {
                let version = &mut *version_ptr;
                
                // We will identify as "nouveau" (The open-source NVIDIA driver)
                // This tells Mesa to load the NVK Vulkan driver!
                let name = b"nouveau";
                let date = b"20260220";
                let desc = b"NyxOS Nouveau Wrapper";

                version.version_major = 1;
                version.version_minor = 3;
                version.version_patchlevel = 1;

                // If Mesa provided buffers, copy the strings into them
                if version.name_len > 0 && !version.name.is_null() {
                    let copy_len = core::cmp::min(version.name_len, name.len());
                    core::ptr::copy_nonoverlapping(name.as_ptr(), version.name, copy_len);
                }
                version.name_len = name.len();

                if version.date_len > 0 && !version.date.is_null() {
                    let copy_len = core::cmp::min(version.date_len, date.len());
                    core::ptr::copy_nonoverlapping(date.as_ptr(), version.date, copy_len);
                }
                version.date_len = date.len();

                if version.desc_len > 0 && !version.desc.is_null() {
                    let copy_len = core::cmp::min(version.desc_len, desc.len());
                    core::ptr::copy_nonoverlapping(desc.as_ptr(), version.desc, copy_len);
                }
                version.desc_len = desc.len();
            }
            Ok(0) // Success
        },
        _ => {
            crate::serial_println!("[DRM] UNHANDLED IOCTL: {:#x}", request);
            Err(-1) // -EINVAL
        }
    }
}