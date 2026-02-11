#!/bin/bash
set -e

echo "--- 1. Building User Application ---"
cd nyx-user
# Force rebuild to ensure code changes are picked up

cargo build --release --target x86_64-unknown-none
cd ..

echo "--- 2. Extracting & Moving Binary ---"
# Ensure the path matches exactly where Cargo outputs it
objcopy -O binary target/x86_64-unknown-none/release/nyx-user nyx-kernel/src/nyx-user.bin

echo "--- 3. Forcing Kernel Rebuild ---"
# Touch main.rs to force include_bytes! to reload the file
touch nyx-kernel/src/main.rs

echo "--- 4. Building Kernel & Bootable Image ---"
cargo run --package nyx-kernel --release --target x86_64-unknown-none

echo "SUCCESS! Bootable Image Ready"