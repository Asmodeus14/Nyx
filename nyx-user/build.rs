use std::env;
use std::path::PathBuf;

fn main() {
    // Check which target Cargo is currently compiling for
    let target = env::var("TARGET").unwrap_or_default();
    
    // If Cargo is targeting Linux, ban the C-runtime startup files.
    if target.contains("linux") {
        println!("cargo:rustc-link-arg=-nostartfiles");
    } else {
        //  THE FIX: Force the compiler to use our memory map for bare-metal targets!
        let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let linker_script = PathBuf::from(dir).join("linker.ld");
        
        println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    }
}