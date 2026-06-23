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

echo "[1/7] Building Init Orchestrator (init)..."
(cd apps/init && $BUILD_CMD)

echo "[2/7] Building Window Server (compositor)..."
(cd apps/compositor && $BUILD_CMD)

echo "[3/7] Building Terminal App (terminal)..."
(cd apps/terminal && $BUILD_CMD)

echo "[4/7] Building Settings App (settings)..."
(cd apps/settings && $BUILD_CMD)

echo "[5/7] Building Explorer Suite (explorer)..."
(cd apps/explorer && $BUILD_CMD)

echo "[6/7] Building Network Suite (network)..."
(cd apps/network && $BUILD_CMD)

echo "[7/7] Building System Monitor (sysmon)..."
(cd apps/sysmon && $BUILD_CMD)

echo "[8/8] Generating App Tarball..."
rm -rf build_initrd

# 1. Create the App Folders
mkdir -p build_initrd/apps/Init.nyx
mkdir -p build_initrd/apps/WindowServer.nyx
mkdir -p build_initrd/apps/Terminal.nyx
mkdir -p build_initrd/apps/Settings.nyx
mkdir -p build_initrd/apps/Explorer.nyx
mkdir -p build_initrd/apps/Network.nyx
mkdir -p build_initrd/apps/SystemMonitor.nyx

# 2. Copy the compiled binaries into the folders as 'run.bin'
# Notice the binary name for WindowServer is now 'compositor'
cp target/x86_64-nyx/release/nyx-init build_initrd/apps/Init.nyx/run.bin
cp target/x86_64-nyx/release/compositor build_initrd/apps/WindowServer.nyx/run.bin
cp target/x86_64-nyx/release/nyx-terminal build_initrd/apps/Terminal.nyx/run.bin
cp target/x86_64-nyx/release/nyx-settings build_initrd/apps/Settings.nyx/run.bin
cp target/x86_64-nyx/release/nyx-explorer build_initrd/apps/Explorer.nyx/run.bin
cp target/x86_64-nyx/release/nyx-network build_initrd/apps/Network.nyx/run.bin
cp target/x86_64-nyx/release/nyx-sysmon build_initrd/apps/SystemMonitor.nyx/run.bin

# 3. Copy any JSON manifests from the source folders into the App Bundles
cp apps/init/*.json build_initrd/apps/Init.nyx/ 2>/dev/null || true
cp apps/compositor/*.json build_initrd/apps/WindowServer.nyx/ 2>/dev/null || true
cp apps/terminal/*.json build_initrd/apps/Terminal.nyx/ 2>/dev/null || true
cp apps/settings/*.json build_initrd/apps/Settings.nyx/ 2>/dev/null || true
cp apps/explorer/*.json build_initrd/apps/Explorer.nyx/ 2>/dev/null || true
cp apps/network/*.json build_initrd/apps/Network.nyx/ 2>/dev/null || true
cp apps/sysmon/*.json build_initrd/apps/SystemMonitor.nyx/ 2>/dev/null || true

# 4. Package it into a lightweight tape archive
cd build_initrd
tar -cf ../initrd.tar apps
cd ..
cp initrd.tar nyx-kernel/src/initrd.tar
touch nyx-kernel/src/main.rs

echo "=========================================="
echo "         Compiling NyxOS Kernel           "
echo "=========================================="
unset RUSTFLAGS
cargo run --package nyx-kernel --release --target x86_64-unknown-none