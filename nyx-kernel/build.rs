use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
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
    // 1.5 COMPILE THE EXT4 C-LIBRARY (libnyx_ext4.a)
    // ==========================================
    // Dynamically generate the missing CMake config file required by lwext4
    let ext4_gen_dir = PathBuf::from("lwext4/include/generated");
    if !ext4_gen_dir.exists() {
        fs::create_dir_all(&ext4_gen_dir).unwrap();
    }
    fs::write(
        ext4_gen_dir.join("ext4_config.h"),
        "
#ifndef EXT4_CONFIG_GENERATED_H_
#define EXT4_CONFIG_GENERATED_H_

#define CONFIG_DIR_INDEX_ENABLE 1
#define CONFIG_EXTENT_ENABLE 1
#define CONFIG_JOURNAL_ENABLE 1
#define CONFIG_BLOCK_DEV_CACHE_SIZE 16
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