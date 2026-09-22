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

    // Prevent QEMU from launching in GitHub Actions to avoid hangs/crashes
    if env::var("CI").is_ok() {
        println!("CI environment detected. Skipping QEMU execution.");
        return;
    }

    // 2. Locate the UEFI firmware.
    //
    // ★ This used to be the single hardcoded path `/usr/share/OVMF/OVMF_CODE.fd`, and on Ubuntu
    // 24.04 that file does not exist — the package splits the image by size and ships
    // `OVMF_CODE_4M.fd` instead. QEMU then failed to start, which read as "there is no QEMU on this
    // machine" and hardened into a project-wide assumption that every test costs a power cycle on
    // the real laptop. Both QEMU and OVMF were installed the whole time.
    //
    // Searched in order rather than hardcoded, because the name differs per distro and release, and
    // a missing firmware should say so instead of looking like a missing emulator.
    const OVMF_CANDIDATES: &[&str] = &[
        "/usr/share/OVMF/OVMF_CODE_4M.fd",   // Debian/Ubuntu 24.04+
        "/usr/share/OVMF/OVMF_CODE.fd",      // older Debian/Ubuntu
        "/usr/share/ovmf/OVMF.fd",           // Debian `ovmf` package
        "/usr/share/qemu/OVMF.fd",           // some distros ship it under qemu
        "/usr/share/edk2/x64/OVMF_CODE.fd",  // Arch
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
    ];
    let firmware = env::var("NYX_OVMF").ok().filter(|p| PathBuf::from(p).exists()).or_else(|| {
        OVMF_CANDIDATES
            .iter()
            .find(|p| PathBuf::from(p).exists())
            .map(|p| p.to_string())
    });
    let firmware = match firmware {
        Some(f) => f,
        None => {
            eprintln!("No UEFI firmware found. Looked for:");
            for c in OVMF_CANDIDATES {
                eprintln!("  {c}");
            }
            eprintln!("Install it (Debian/Ubuntu: `apt install ovmf`) or set NYX_OVMF=<path>.");
            eprintln!("The image is still built and usable: {}", image_path.display());
            return;
        }
    };

    // 3. Launch QEMU with UEFI Support
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-bios").arg(&firmware);
    cmd.arg("-drive").arg(format!("format=raw,file={}", image_path.display()));
    cmd.arg("-serial").arg("stdio");
    // Extra knobs the caller may want without editing this file — notably `-display none` for a
    // headless boot whose serial output can be captured, which is the shape every automated check
    // of this kernel needs.
    if let Ok(extra) = env::var("NYX_QEMU_ARGS") {
        for a in extra.split_whitespace() {
            cmd.arg(a);
        }
    }

    println!("Launching QEMU with firmware {firmware}");
    let mut child = cmd.spawn().expect("Failed to start QEMU");
    child.wait().unwrap();
}
