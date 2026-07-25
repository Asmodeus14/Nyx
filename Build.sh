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

# [3] Terminal is now a std target_os=nyx binary (D3) — built via Build-std.sh below (step [9f]),
# not the no_std $BUILD_CMD path.

echo "[4/7] Building Settings App (settings)..."
(cd apps/settings && $BUILD_CMD)

echo "[5/7] Building Explorer Suite (explorer)..."
(cd apps/explorer && $BUILD_CMD)

echo "[6/7] Building Network Suite (network)..."
(cd apps/network && $BUILD_CMD)

echo "[7/8] Building System Monitor (sysmon)..."
(cd apps/sysmon && $BUILD_CMD)

echo "[8/9] Building GL Cube validation app (glcube)..."
(cd apps/glcube && $BUILD_CMD)

echo "[9/9] Building Image Viewer (imageviewer)..."
(cd apps/imageviewer && $BUILD_CMD)

# P8: the Wi-Fi network picker — the only way to choose a network now that the driver has no
# built-in SSID, so a failure here means a machine that can't get online. Not swallowed.
echo "[9a] Building Wi-Fi picker (wifi)..."
(cd apps/wifi && $BUILD_CMD)

# Headless auto-reconnect agent, spawned by init at boot from the saved-network file.
echo "[9a2] Building Wi-Fi auto-connect agent (wifiagent)..."
(cd apps/wifiagent && $BUILD_CMD)

# std-port smoke test (Workstream B): build a real `std` binary for target_os=nyx and bundle it so
# it can be launched on-device (B-alpha runtime check). Non-fatal if the std toolchain isn't ready.
echo "[9b] Building std hello (target_os=nyx)..."
./Build-std.sh tests/stdhello stdhello || echo "  (std build skipped/failed — continuing)"
# B-gamma.3 child binary: spawned by stdhello via std::process::Command to prove fork+execve+wait4.
echo "[9c] Building std child (target_os=nyx)..."
./Build-std.sh tests/stdchild stdchild || echo "  (std child build skipped/failed — continuing)"
# Workstream C (C2): on-device qclang front-end (std target_os=nyx; links the portable compiler core).
echo "[9d] Building qcstudio (target_os=nyx)..."
./Build-std.sh apps/qcstudio qcstudio || echo "  (qcstudio build skipped/failed — continuing)"
# Workstream D (D2): std-GUI foundation spike — a std target_os=nyx binary that links the no_std
# nyx-gui/nyx-api UNCHANGED and opens a window (proves the font pipeline builds for nyx under build-std).
echo "[9e] Building stdgui (target_os=nyx)..."
./Build-std.sh apps/stdgui stdgui || echo "  (stdgui build skipped/failed — continuing)"
# Workstream D (D3): the terminal, ported to a std target_os=nyx binary (hosts the compiler registry
# in D4). Built via build-std like the other std apps; must SUCCEED (it's the primary shell), so no
# `|| echo`-swallow here — a failure should abort the build rather than silently ship a stale terminal.
echo "[9f] Building terminal (std target_os=nyx)..."
./Build-std.sh apps/terminal nyx-terminal
# Notepad: std target_os=nyx text editor that saves to /mnt/nvme.
echo "[9g] Building notepad (std target_os=nyx)..."
./Build-std.sh apps/notepad notepad || echo "  (notepad build skipped/failed — continuing)"

echo "[10/10] Generating App Tarball..."
rm -rf build_initrd

# 1. Create the App Folders
mkdir -p build_initrd/apps/Init.nyx
mkdir -p build_initrd/apps/WindowServer.nyx
mkdir -p build_initrd/apps/Terminal.nyx
mkdir -p build_initrd/apps/Settings.nyx
mkdir -p build_initrd/apps/Explorer.nyx
mkdir -p build_initrd/apps/Network.nyx
mkdir -p build_initrd/apps/SystemMonitor.nyx
mkdir -p build_initrd/apps/GlCube.nyx
mkdir -p build_initrd/apps/ImageViewer.nyx
mkdir -p build_initrd/apps/StdHello.nyx
mkdir -p build_initrd/apps/StdChild.nyx
mkdir -p build_initrd/apps/QcStudio.nyx
mkdir -p build_initrd/apps/StdGui.nyx
mkdir -p build_initrd/apps/Notepad.nyx
mkdir -p build_initrd/apps/Wifi.nyx
mkdir -p build_initrd/apps/WifiAgent.nyx

# 2. Copy the compiled binaries into the folders as 'run.bin'
# Notice the binary name for WindowServer is now 'compositor'
cp target/x86_64-nyx/release/nyx-init build_initrd/apps/Init.nyx/run.bin
cp target/x86_64-nyx/release/compositor build_initrd/apps/WindowServer.nyx/run.bin
# D3: terminal is a std binary now — its output lives under apps/terminal/target/ (workspace-excluded),
# not the root target dir.
cp apps/terminal/target/x86_64-unknown-nyx/release/nyx-terminal build_initrd/apps/Terminal.nyx/run.bin
# D4: a bundled sample so `compile` (no args) works out of the box — the registry dispatches it to the
# qclang backend by its .ql extension and emits OpenQASM next to it.
cp apps/qcstudio/samples/sample.ql build_initrd/apps/Terminal.nyx/sample.ql 2>/dev/null || true
cp target/x86_64-nyx/release/nyx-settings build_initrd/apps/Settings.nyx/run.bin
cp target/x86_64-nyx/release/nyx-explorer build_initrd/apps/Explorer.nyx/run.bin
cp target/x86_64-nyx/release/nyx-network build_initrd/apps/Network.nyx/run.bin
cp target/x86_64-nyx/release/nyx-sysmon build_initrd/apps/SystemMonitor.nyx/run.bin
cp target/x86_64-nyx/release/nyx-glcube build_initrd/apps/GlCube.nyx/run.bin
cp target/x86_64-nyx/release/nyx-imageviewer build_initrd/apps/ImageViewer.nyx/run.bin
cp target/x86_64-nyx/release/nyx-wifi build_initrd/apps/Wifi.nyx/run.bin
cp target/x86_64-nyx/release/nyx-wifiagent build_initrd/apps/WifiAgent.nyx/run.bin
# std-port smoke test binary (built for the x86_64-unknown-nyx std target; workspace-excluded, so
# its output lives under tests/stdhello/target/, not the root target dir).
cp tests/stdhello/target/x86_64-unknown-nyx/release/stdhello build_initrd/apps/StdHello.nyx/run.bin 2>/dev/null || true
# Sample data file so the std binary can exercise std::fs::read on-device (B-alpha fs milestone).
cp tests/stdhello/hello.txt build_initrd/apps/StdHello.nyx/hello.txt 2>/dev/null || true
# B-gamma.3 child binary — spawned by stdhello via std::process::Command (fork/execve/wait4 test).
cp tests/stdchild/target/x86_64-unknown-nyx/release/stdchild build_initrd/apps/StdChild.nyx/run.bin 2>/dev/null || true
# Workstream C (C2): qcstudio on-device compiler front-end (std target_os=nyx), plus the sample .ql
# it compiles to .qasm on-device (C3 byte-compares that output against host qclang).
cp apps/qcstudio/target/x86_64-unknown-nyx/release/qcstudio build_initrd/apps/QcStudio.nyx/run.bin 2>/dev/null || true
cp apps/qcstudio/samples/sample.ql build_initrd/apps/QcStudio.nyx/sample.ql 2>/dev/null || true
# C3 reference: host qclang's QASM for sample.ql; qcstudio byte-compares its on-device output to it.
cp apps/qcstudio/samples/expected.qasm build_initrd/apps/QcStudio.nyx/expected.qasm 2>/dev/null || true
# Workstream D (D2): std-GUI foundation spike binary (std target_os=nyx; links no_std nyx-gui unchanged).
cp apps/stdgui/target/x86_64-unknown-nyx/release/stdgui build_initrd/apps/StdGui.nyx/run.bin 2>/dev/null || true
# Notepad: std target_os=nyx text editor (saves to /mnt/nvme via std::fs).
cp apps/notepad/target/x86_64-unknown-nyx/release/notepad build_initrd/apps/Notepad.nyx/run.bin 2>/dev/null || true

# 3. Copy any JSON manifests from the source folders into the App Bundles
cp apps/init/*.json build_initrd/apps/Init.nyx/ 2>/dev/null || true
cp apps/compositor/*.json build_initrd/apps/WindowServer.nyx/ 2>/dev/null || true
cp apps/terminal/*.json build_initrd/apps/Terminal.nyx/ 2>/dev/null || true
cp apps/settings/*.json build_initrd/apps/Settings.nyx/ 2>/dev/null || true
cp apps/explorer/*.json build_initrd/apps/Explorer.nyx/ 2>/dev/null || true
cp apps/network/*.json build_initrd/apps/Network.nyx/ 2>/dev/null || true
cp apps/sysmon/*.json build_initrd/apps/SystemMonitor.nyx/ 2>/dev/null || true
cp apps/glcube/*.json build_initrd/apps/GlCube.nyx/ 2>/dev/null || true
cp apps/imageviewer/*.json build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true
cp apps/wifi/*.json build_initrd/apps/Wifi.nyx/ 2>/dev/null || true

# 3b. Bundle the Image Viewer's sample image asset(s) so it has something to open on launch.
# Only ship real image files — NOT the generator script (make_sample.py).
cp apps/imageviewer/assets/*.bmp build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true
cp apps/imageviewer/assets/*.tga build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true
# A2b: PNG + JPEG samples so the viewer's click-to-cycle can exercise both decoders on-device.
cp apps/imageviewer/assets/*.png build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true
cp apps/imageviewer/assets/*.jpg build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true

# 3c. Each app carries its OWN icon inside its .nyx bundle as icon.png — the compositor decodes it
# (via libs/image) for the start menu, and apps declare the same path in their window header so the
# taskbar button matches. Committed artwork; regenerate with `python3 tools/make_icons.py`.
install_icon() { cp "assets/icons/$1.png" "build_initrd/apps/$2.nyx/icon.png" 2>/dev/null || true; }
install_icon terminal    Terminal
install_icon settings    Settings
install_icon explorer    Explorer
install_icon network     Network
install_icon sysmon      SystemMonitor
install_icon glcube      GlCube
install_icon imageviewer ImageViewer
install_icon stdhello    StdHello
install_icon qcstudio    QcStudio
install_icon stdgui      StdGui
install_icon notepad     Notepad
install_icon wifi        Wifi
# The shell's own mark, in the compositor's bundle.
install_icon nyx         WindowServer

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