#!/bin/bash
set -e

# 1. Clean and Build User App (Ring-3 Window Manager/Terminal)
echo "--- Building User App ---"
cd nyx-user
# 🚨 THE FIX: Added BACK the -Z json-target-spec flag!
cargo build --release --target ../x86_64-nyx.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec
cd ..

# 2. Build the Standard Rust App (Musl Target)
echo "--- Building Rust App ---"
cd nyx-rust-app
cargo build --target x86_64-unknown-linux-musl --release
cd ..

# 3. DELETE OLD BINARIES (Force update)
rm -f nyx-kernel/src/nyx-user.bin
rm -f nyx-kernel/src/hello.elf
rm -f nyx-kernel/src/rust.elf

# 4. COPY NEW BINARIES
echo "--- Copying Binaries to Kernel RAM Disk ---"
cp target/x86_64-nyx/release/nyx-user nyx-kernel/src/nyx-user.bin
cp nyx-c/hello.elf nyx-kernel/src/hello.elf
cp target/x86_64-unknown-linux-musl/release/nyx-rust-app nyx-kernel/src/rust.elf

# 5. Verify the Copy
echo "--- verifying nyx-user.bin ELF Headers ---"
readelf -h nyx-kernel/src/nyx-user.bin | grep "Entry point" || true

# 6. Rebuild and Run Kernel
echo "--- Running Kernel ---"
touch nyx-kernel/src/main.rs
cargo run --package nyx-kernel --release --target x86_64-unknown-none