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
    build.define("ACPI_USE_SYSTEM_CLIBRARY", "0");
    build.define("ACPI_LIBRARY", None); 
    build.define("_KERNEL", None);
    
    build.flag("-ffreestanding"); 
    build.flag("-nostdlib");      
    build.flag("-fno-builtin"); 
    build.flag("-mno-red-zone");
    build.flag("-fno-strict-aliasing");

    build.compile("acpica");

    // ==========================================
    // 2. GENERATE THE RUST BINDINGS
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