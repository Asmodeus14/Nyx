use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // ★★ A BUILD STAMP, because "is the machine running the image I just built?" has now cost a
    // power cycle — and on a box with no serial console, bare metal only, that is the most expensive
    // question there is. It was asked and answered wrongly: the panic/diagnostic output was
    // identical between two boots, which looked like a code bug and was actually a stale flash.
    //
    // The image file is a fixed size every build (24,182,784 bytes), so neither its size nor its
    // timestamp is a reliable tell at a glance. A stamp printed on screen is.
    //
    // ⚠️ `rerun-if-changed` on a path that does not exist makes cargo re-run this script on EVERY
    // build. That is deliberate and required: a stamp that is only refreshed when some other file
    // changes is a stamp that lies, which is worse than not having one.
    println!("cargo:rerun-if-changed=.nyx-always-rebuild");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=NYX_BUILD_STAMP={}", stamp);

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=acpica-core");
    println!("cargo:rerun-if-changed=acpica-includes");
    println!("cargo:rerun-if-changed=ext4_wrapper.c");
    println!("cargo:rerun-if-changed=lwext4");

    // ==========================================
    // 1. COMPILE THE ACPICA C CODE (libacpica.a)
    // ==========================================
    let mut build = cc::Build::new();

    build.warnings(false);
    build.flag_if_supported("-w"); 

    let dirs = vec![
        "acpica-core/dispatcher",
        "acpica-core/events",
        "acpica-core/executer",
        "acpica-core/hardware",
        "acpica-core/namespace",
        "acpica-core/parser",
        "acpica-core/tables",
        "acpica-core/utilities",
        // Vendored all along but never compiled, so `AcpiWalkResources` was an undefined symbol.
        // Needed by the EC driver: the EC's IO ports must be read from the EVALUATED `_CRS` (this
        // machine patches them in at runtime via _Y5A/_Y5B, so the DSDT template reads 0x0000), and
        // walking a resource template without this module means hand-parsing AML resource
        // descriptors — a job ACPICA already does correctly.
        "acpica-core/resources",
    ];

    for dir in dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("c") {
                    build.file(path);
                }
            }
        }
    }

    build.file("custom_acpi.c");
    build.include("acpica-includes");

    build.define("ACPI_MACHINE_WIDTH", "64");
    // ⚠️ TRAP: this reads as "no system C library" and means the OPPOSITE.
    //
    // ACPICA tests it with `#ifdef`/`#ifndef`, never `#if`, so defining it to "0" still counts as
    // defined. `accommon.h` then SKIPS `acclib.h`, ACPICA's own `isdigit`/`strlen`/`memcpy` macros
    // never appear, and the host glibc declarations win instead. That is how an all-zero
    // `__ctype_b_loc` table (c_stubs.rs) silently broke `isdigit`, which broke width parsing in
    // ACPICA's `vsnprintf`, which misaligned varargs and page-faulted the kernel at 0x20251212.
    //
    // Left defined deliberately. Undefining it compiles `utilities/utclib.c`, which defines its own
    // `memcpy`/`memset`/`memcmp`/`strlen` and would collide with `compiler-builtins-mem`. If that
    // is ever untangled, the correct end state is to NOT define this at all.
    build.define("ACPI_USE_SYSTEM_CLIBRARY", "0");
    build.define("ACPI_LIBRARY", None); 
    build.define("_KERNEL", None);
    
    build.flag("-ffreestanding");
    build.flag("-nostdlib");
    build.flag("-fno-builtin");
    build.flag("-mno-red-zone");
    build.flag("-fno-strict-aliasing");
    // The host gcc defaults _FORTIFY_SOURCE on, which rewrites snprintf/vsnprintf calls to
    // __snprintf_chk/__vsnprintf_chk — glibc symbols that do not exist in a freestanding kernel.
    // ACPICA supplies its own snprintf (utilities/utprint.c) and we want exactly that one.
    // The lwext4 build below already does this; ACPICA needed it as soon as it called either.
    build.flag("-U_FORTIFY_SOURCE");

    build.compile("acpica");

    // ==========================================
    // 1.5 COMPILE THE EXT4 C-LIBRARY (libnyx_ext4.a)
    // ==========================================
    // Dynamically generate the missing CMake config file required by lwext4
    let ext4_gen_dir = PathBuf::from("lwext4/include/generated");
    if !ext4_gen_dir.exists() {
        fs::create_dir_all(&ext4_gen_dir).unwrap();
    }
    
    // 🔥 FIX: Increased CONFIG_BLOCK_DEV_CACHE_SIZE from 16 to 256 (Milestone 1.4)
    fs::write(
        ext4_gen_dir.join("ext4_config.h"),
        "
#ifndef EXT4_CONFIG_GENERATED_H_
#define EXT4_CONFIG_GENERATED_H_

#define CONFIG_DIR_INDEX_ENABLE 1
#define CONFIG_EXTENT_ENABLE 1
#define CONFIG_JOURNAL_ENABLE 1
#define CONFIG_BLOCK_DEV_CACHE_SIZE 256
#define CONFIG_HAVE_OWN_OFLAGS 1
#define CONFIG_HAVE_OWN_ASSERT 0

#endif
        ",
    ).expect("Failed to write lwext4 config file!");

    // We use a fresh cc::Build block to prevent macro conflicts with ACPICA
    let mut ext4_build = cc::Build::new();
    ext4_build.warnings(false)
        .flag_if_supported("-w")
        .flag("-ffreestanding")
        .flag("-nostdlib")
        .flag("-fno-builtin")
        .flag("-mno-red-zone")
        // STRIP UBUNTU'S LINUX SECURITY WRAPPERS
        .flag("-fno-stack-protector") 
        .flag("-U_FORTIFY_SOURCE")    
        .include("lwext4/include")
        .define("CONFIG_HAVE_OWN_OFLAGS", "1");

    // Include our Rust-to-C wrapper
    ext4_build.file("ext4_wrapper.c");

    // Dynamically include ALL lwext4 source files to prevent Linker errors
    if let Ok(entries) = fs::read_dir("lwext4/src") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("c") {
                ext4_build.file(path);
            }
        }
    }

    ext4_build.compile("nyx_ext4");

    // ==========================================
    // 2. GENERATE THE RUST BINDINGS (ACPICA)
    // ==========================================
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .use_core() 
        .ctypes_prefix("core::ffi") 
        .layout_tests(false) // Disable layout tests to prevent #![no_std] errors
        .clang_arg("-Iacpica-includes")
        .clang_arg("-DACPI_MACHINE_WIDTH=64")
        .clang_arg("-DACPI_USE_SYSTEM_CLIBRARY=0")
        .clang_arg("-DACPI_LIBRARY")
        .clang_arg("-D_KERNEL")
        .clang_arg("-D__linux__")
        .clang_arg("--target=x86_64-unknown-linux-gnu") 
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("FATAL: bindgen failed to translate Intel ACPICA headers into Rust!");
        
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("acpi_bindings.rs"))
        .expect("Couldn't write bindings!");
}