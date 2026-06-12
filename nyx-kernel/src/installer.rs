use alloc::boxed::Box;
use crate::vfs::VFS;
use crate::tarfs::TarFs;

/// Mounts the Initramfs TAR, extracts the binaries, and builds
/// macOS-style .nyx app bundles natively on the physical NVMe drive.
pub fn install_apps_to_nvme(tar_data: &'static [u8]) {
    crate::vga_println!("[INSTALLER] Extracting apps from RAM to SSD...");
    
    // 1. Mount the TAR archive temporarily
    VFS.mount("/tar", Box::new(TarFs::new(tar_data)));
    
    // 2. Create the root apps directory on the physical SSD
    VFS.create_dir("/mnt/nvme/apps");
    
    // 3. Define the apps we want to install and bundle
    let app_manifest = [
        ("nyx-init", "Init"),
        ("nyx-user", "WindowServer"),
        ("nyx-terminal", "Terminal"),
        ("nyx-settings", "Settings"),
        ("nyx-explorer", "Explorer"),
        ("nyx-network", "Network"),
        ("nyx-sysmon", "SystemMonitor"),
    ];

    // 4. Extract and Bundle!
    for (bin_name, app_name) in app_manifest.iter() {
        let tar_path = alloc::format!("/tar/{}", bin_name);
        
        if let Some(elf_data) = VFS.read_file_alloc(&tar_path) {
            // Create /mnt/nvme/apps/AppName.nyx
            let app_dir = alloc::format!("/mnt/nvme/apps/{}.nyx", app_name);
            VFS.create_dir(&app_dir);
            
            // Write run.bin
            let bin_path = alloc::format!("{}/run.bin", app_dir);
            VFS.create_file(&bin_path);
            VFS.write_file(&bin_path, &elf_data);
            
            // Write meta.json
            let meta_path = alloc::format!("{}/meta.json", app_dir);
            let meta_data = alloc::format!("{{\"id\": \"{}\", \"version\": \"1.0.0\"}}", app_name);
            VFS.create_file(&meta_path);
            VFS.write_file(&meta_path, meta_data.as_bytes());
        }
    }
    crate::vfs::VFS.unmount("/tar");
    
    crate::vga_println!("[INSTALLER] App Bundles Successfully Installed to NVMe!");
}