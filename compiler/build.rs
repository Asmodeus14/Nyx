use std::process::Command;

fn main() {
    // 1. Run git to get the current commit hash
    let output = Command::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
        .output();

    // 2. Handle the result (use "unknown" if git fails or not a repo)
    let git_hash = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    };

    // 3. Pass this variable to the main compiler
    // This line allows env!("GIT_HASH") to work in lib.rs
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    
    // 4. Tell Cargo to re-run this script if the git state changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}