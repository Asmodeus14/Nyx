#!/bin/bash
set -e

echo "=========================================="
echo "    NyxOS Master Build System Initiated   "
echo "=========================================="

ROOT_DIR=$(pwd)
TARGET_JSON="$ROOT_DIR/targets/x86_64-nyx.json"
LINKER_SCRIPT="$ROOT_DIR/targets/linker.ld"

export RUSTFLAGS="-C link-arg=-T$LINKER_SCRIPT"
BUILD_CMD="cargo build --release --target $TARGET_JSON -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec"

echo "[1/7] Building Init Orchestrator (nyx-init)..."
(cd nyx-init && $BUILD_CMD)

echo "[2/7] Building Window Server (nyx-user)..."
(cd nyx-user && $BUILD_CMD)

echo "[3/7] Building Terminal App (nyx-terminal)..."
(cd nyx-terminal && $BUILD_CMD)

echo "[4/7] Building Settings App (nyx-settings)..."
(cd nyx-settings && $BUILD_CMD)

echo "[5/7] Building Explorer Suite (nyx-explorer)..."
(cd nyx-explorer && $BUILD_CMD)

echo "[6/7] Building Network Suite (nyx-network)..."
(cd nyx-network && $BUILD_CMD)

echo "[7/7] Building System Monitor (nyx-sysmon)..."
(cd nyx-sysmon && $BUILD_CMD)

echo "[8/8] Packaging Initramfs (initrd.tar)..."
# Create a staging folder
rm -rf build_initrd
mkdir -p build_initrd

# Copy all compiled binaries into the staging folder
cp target/x86_64-nyx/release/nyx-* build_initrd/

# Pack them into a standard Uncompressed TAR archive
cd build_initrd
tar -cvf ../nyx-kernel/src/initrd.tar *
cd ..

# Cleanup staging folder
rm -rf build_initrd

# Touch main.rs to force Cargo to rebuild the kernel with the new TAR archive
touch nyx-kernel/src/main.rs

echo "=========================================="
echo " Build Complete! Ready to compile Kernel. "
echo "=========================================="

echo "=========================================="
echo "         Compiling NyxOS Kernel           "
echo "=========================================="
# 🚨 CRITICAL: Unset RUSTFLAGS so we don't inject the userspace linker into the Kernel!
unset RUSTFLAGS 

# Force recompilation of main.rs so it packs the newly copied binaries
touch nyx-kernel/src/main.rs 

# Run the Kernel via your custom Runner crate!
cargo run --package nyx-kernel --release --target x86_64-unknown-none