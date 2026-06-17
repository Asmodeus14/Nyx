use alloc::format;
use core::str;

/// Dynamically unpacks the embedded tarball natively onto the mounted Ext4 drive
pub fn extract_tar_to_ext4(tar_data: &'static [u8]) {
    crate::vga_println!("[INSTALLER] Extracting Apps to NVMe Ext4 Partition...");
    let mut offset = 0;
    
    while offset < tar_data.len() {
        let header = &tar_data[offset..offset+512];
        if header[0] == 0 { break; } // Two null blocks indicate the end of the archive
        
        // Parse the file/folder name
        let name_end = header.iter().position(|&b| b == 0).unwrap_or(100);
        let name = str::from_utf8(&header[..name_end]).unwrap_or("UNKNOWN");
        let clean_name = name.trim_end_matches('/');
        
        // Parse the file size from the octal string
        let size_str = str::from_utf8(&header[124..135]).unwrap_or("0").trim_matches(char::from(0));
        let size = usize::from_str_radix(size_str, 8).unwrap_or(0);
        
        // Type flag '5' is a directory, '0' or null is a normal file
        let typeflag = header[156];
        
        offset += 512;
        
        // Route it through the VFS to the physical SSD
        let path = format!("/mnt/nvme/{}", clean_name);
        
        if typeflag == b'5' { 
            crate::vfs::VFS.create_dir(&path);
        } else if size > 0 {  
            crate::vga_println!(" -> Installing {} ({} bytes)", path, size);
            crate::vfs::VFS.write_file(&path, &tar_data[offset..offset+size]);
        }
        
        // Move to the next 512-byte block boundary
        offset += (size + 511) & !511; 
    }
    crate::vga_println!("[INSTALLER] All Apps Installed Successfully!");
}