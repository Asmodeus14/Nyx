use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=acpica-core");
    println!("cargo:rerun-if-changed=acpica-includes");

    // ==========================================
    // 1. COMPILE THE C CODE (libacpica.a)
    // ==========================================
    let mut build = cc::Build::new();

    // The core subsystems of Intel's ACPICA
    let dirs = vec![
        "acpica-core/dispatcher",
        "acpica-core/events",
        "acpica-core/executer",
        "acpica-core/hardware",
        "acpica-core/namespace",
        "acpica-core/parser",
        "acpica-core/tables",
        "acpica-core/utilities",
    ];

    // Hunt down every single .c file in those directories
    for dir in dirs {
        // --- NEW: Compile our custom ACPI C-Stub! ---
    build.file("custom_acpi.c");

    // Tell the C compiler where to find Intel's headers
    build.include("acpica-includes");
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("c") {
                    build.file(path);
                }
            }
        }
    }

    // Tell the C compiler where to find Intel's headers
    build.include("acpica-includes");

    // Tell the C compiler we are building a 64-bit OS Kernel without a standard C library
    build.define("ACPI_MACHINE_WIDTH", "64");
    build.define("ACPI_USE_SYSTEM_CLIBRARY", "0");
    build.define("ACPI_LIBRARY", None); // Compile as a standalone library
    build.define("_KERNEL", None);
    
    // ==========================================
    // BARE-METAL C FLAGS
    // ==========================================
    build.flag("-ffreestanding"); // We are an OS, not an app
    build.flag("-nostdlib");      // Do not link standard C libraries
    build.flag("-fno-builtin"); 
    build.flag("-mno-red-zone");
    build.flag("-fno-strict-aliasing");

    // Fire the C compiler!
    build.compile("acpica");

    // Tell the final Rust Linker to ban Linux C startup files (like crt0/crt1)
    
    // 2. GENERATE THE RUST BINDINGS
    // ==========================================
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .use_core() // Force bindgen to use #![no_std] `core` instead of `std`
        .ctypes_prefix("core::ffi") // Map C types to Rust core FFI types
        .clang_arg("-Iacpica-includes")
        .clang_arg("-DACPI_MACHINE_WIDTH=64")
        .clang_arg("-DACPI_USE_SYSTEM_CLIBRARY=0")
        .clang_arg("-DACPI_LIBRARY")
        .clang_arg("-D_KERNEL")
        .clang_arg("-D__linux__")
        // NEW: Tell bindgen to pretend it's on a standard Linux host just so it can find <stdarg.h>
        .clang_arg("--target=x86_64-unknown-linux-gnu") 
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("FATAL: bindgen failed to translate Intel ACPICA headers into Rust!");
    // Save the massive generated Rust file into Cargo's output directory
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("acpi_bindings.rs"))
        .expect("Couldn't write bindings!");
}