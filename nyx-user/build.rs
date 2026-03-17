use std::env;

fn main() {
    // Check which target Cargo is currently compiling for
    let target = env::var("TARGET").unwrap_or_default();
    
    // If Cargo is targeting Linux (which uses the 'cc' linker), ban the C-runtime startup files.
    // If it is targeting bare-metal (which uses 'rust-lld'), do absolutely nothing.
    if target.contains("linux") {
        println!("cargo:rustc-link-arg=-nostartfiles");
    }
}