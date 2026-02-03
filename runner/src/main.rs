use std::{env, process::Command, path::PathBuf};
// 1. CHANGE THIS IMPORT
use bootloader::UefiBoot; 

fn main() {
    let mut args = env::args().skip(1);
    let kernel_binary = args.next().expect("Kernel binary path not received");
    let kernel_path = PathBuf::from(&kernel_binary);

    // 2. Change extension to reflect it's a UEFI image
    let image_path = kernel_path.with_extension("efi.img");
    
    // 3. CHANGE THIS LINE to UefiBoot
    let boot = UefiBoot::new(&kernel_path);
    boot.create_disk_image(&image_path).expect("Failed to create UEFI image");

    println!("--------------------------------------------------");
    println!("UEFI IMAGE CREATED: {}", image_path.display());
    println!("Now Rufus will allow GPT selection!");
    println!("--------------------------------------------------");

    // 4. QEMU Launch (Remember: QEMU needs -bios for UEFI testing)
    let mut cmd = Command::new("qemu-system-x86_64");
    
    // Use your system's OVMF path or the local copy we made earlier
    cmd.arg("-bios").arg("ovmf_code.fd"); 
    
    cmd.arg("-drive").arg(format!("format=raw,file={}", image_path.display()));
    cmd.arg("-serial").arg("stdio");
    cmd.arg("-vga").arg("std");

    let mut child = cmd.spawn().expect("Failed to start QEMU");
    child.wait().unwrap();
}