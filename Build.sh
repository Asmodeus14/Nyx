#!/bin/bash
set -e

# 1. Clean and Build User App (Force new address)
echo "--- Building User App ---"
cd nyx-user
# cargo clean  # (Uncomment if you want a fresh build every time)
cargo build --release --target x86_64-unknown-none
cd ..

# 2. DELETE OLD BINARY (Force update)
rm -f nyx-kernel/src/nyx-user.bin

# 3. COPY NEW ELF BINARY
# Since you have a Cargo Workspace, the target folder is at the root!
echo "--- Copying Binary ---"
cp target/x86_64-unknown-none/release/nyx-user nyx-kernel/src/nyx-user.bin

# 4. Verify the Copy
echo "--- verifying nyx-user.bin ELF Headers ---"
readelf -h nyx-kernel/src/nyx-user.bin | grep "Entry point"

# 5. Rebuild and Run Kernel
echo "--- Running Kernel ---"
touch nyx-kernel/src/main.rs
cargo run --package nyx-kernel --release --target x86_64-unknown-none