#!/bin/bash
set -e

echo "--- 1. Building User Application ---"
# We enter the directory so Cargo finds .cargo/config.toml
cd nyx-user
cargo build --release --target x86_64-unknown-none
cd ..

echo "--- 2. Extracting & Moving Binary ---"
# FIX: The binary is in the ROOT target folder, not nyx-user/target
objcopy -O binary target/x86_64-unknown-none/release/nyx-user nyx-kernel/src/nyx-user.bin

echo "--- 3. Forcing Kernel Rebuild ---"
touch nyx-kernel/src/main.rs

echo "--- 4. Building Kernel & Bootable Image ---"
cargo run --package nyx-kernel --release --target x86_64-unknown-none

echo "SUCCESS! Bootable Image Ready"
