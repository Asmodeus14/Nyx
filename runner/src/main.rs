use std::{env, process::Command, path::PathBuf};
use bootloader::UefiBoot;

fn main() {
    let mut args = env::args().skip(1);
    let kernel_binary = args.next().expect("Kernel binary path not received");
    let kernel_path = PathBuf::from(&kernel_binary);

    // 1. Create UEFI Image (Required for Dell G3 GPT)
    let image_path = kernel_path.with_extension("efi.img");
    let boot = UefiBoot::new(&kernel_path);
    boot.create_disk_image(&image_path).expect("Failed to create UEFI image");

    println!("--------------------------------------------------");
    println!("UEFI IMAGE CREATED: {}", image_path.display());
    println!("--------------------------------------------------");

    // 2. Launch QEMU with UEFI Support
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-bios").arg("/usr/share/OVMF/OVMF_CODE.fd"); // Required for UEFI images
    cmd.arg("-drive").arg(format!("format=raw,file={}", image_path.display()));
    cmd.arg("-serial").arg("stdio");

    println!("Launching QEMU... If it fails, check for ovmf_code.fd in the root.");
    let mut child = cmd.spawn().expect("Failed to start QEMU");
    child.wait().unwrap();
}