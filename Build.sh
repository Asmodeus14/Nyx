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

# ★ The window server, and since Meridian step 20 the only one. `apps/compositor` used to be built
# here first and shipped by default, with Meridian built beside it and selected by $WINDOW_SERVER —
# the arrangement the build order asked for: "apps/compositor keeps booting the whole way, so the
# machine always has a working desktop and the two shells can be compared side by side." Step 20 is
# the end of that order, so the fallback and the variable that chose it are both gone.
echo "[2/7] Building Window Server (Meridian shell)..."
(cd apps/shell && $BUILD_CMD)

# [3] Terminal is now a std target_os=nyx binary (D3) — built via Build-std.sh below (step [9f]),
# not the no_std $BUILD_CMD path.

echo "[4/7] Building Settings App (settings)..."
(cd apps/settings && $BUILD_CMD)

echo "[5/7] Building Explorer Suite (explorer)..."
(cd apps/explorer && $BUILD_CMD)


echo "[7/8] Building System Monitor (sysmon)..."
(cd apps/sysmon && $BUILD_CMD)

echo "[8/9] Building GL Cube validation app (glcube)..."
(cd apps/glcube && $BUILD_CMD)

echo "[9/9] Building Image Viewer (imageviewer)..."
(cd apps/imageviewer && $BUILD_CMD)

# The Wi-Fi agent. Two jobs since step 20: spawned by init with no argument it rejoins the
# remembered network at boot, and spawned by the shell WITH one it performs a scan, a join or a radio
# toggle on the shell's behalf. It is the only process that blocks in the radio — a failure here
# means a machine that can't get online, so it is not swallowed.
echo "[9a] Building Wi-Fi agent (wifiagent)..."
(cd apps/wifiagent && $BUILD_CMD)

# std-port smoke test (Workstream B): build a real `std` binary for target_os=nyx and bundle it so
# it can be launched on-device (B-alpha runtime check). Non-fatal if the std toolchain isn't ready.
echo "[9b] Building std hello (target_os=nyx)..."
./Build-std.sh tests/stdhello stdhello || echo "  (std build skipped/failed — continuing)"
# B-gamma.3 child binary: spawned by stdhello via std::process::Command to prove fork+execve+wait4.
echo "[9c] Building std child (target_os=nyx)..."
./Build-std.sh tests/stdchild stdchild || echo "  (std child build skipped/failed — continuing)"
# Meridian step 14: the QCLang window (was Workstream C's headless round-trip). Still compiles the
# .ql on-device and still byte-compares against host qclang — it just SHOWS the result now, so a
# stale binary would silently ship a program that opens no window at all. Hence no `|| echo` swallow.
echo "[9d] Building QCLang (std target_os=nyx)..."
./Build-std.sh apps/qcstudio qcstudio
# Workstream D (D2): std-GUI foundation spike — a std target_os=nyx binary that links the no_std
# nyx-gui/nyx-api UNCHANGED and opens a window (proves the font pipeline builds for nyx under build-std).
echo "[9e] Building stdgui (target_os=nyx)..."
./Build-std.sh apps/stdgui stdgui || echo "  (stdgui build skipped/failed — continuing)"
# Workstream D (D3): the terminal, ported to a std target_os=nyx binary (hosts the compiler registry
# in D4). Built via build-std like the other std apps; must SUCCEED (it's the primary shell), so no
# `|| echo`-swallow here — a failure should abort the build rather than silently ship a stale terminal.
echo "[9f] Building terminal (std target_os=nyx)..."
./Build-std.sh apps/terminal nyx-terminal
# The graphical browser, libs/web and the Network Suite were DELETED. Browsing is now text-mode
# commands in the terminal (`get`/`links`/`open`/`dns`) over the same libs/net transport. The
# browser and its html5ever/cssparser/layout stack are recoverable from git history if ever wanted.
# Notepad: std target_os=nyx text editor that saves to /mnt/nvme.
echo "[9g] Building notepad (std target_os=nyx)..."
./Build-std.sh apps/notepad notepad || echo "  (notepad build skipped/failed — continuing)"
# The POSIX conformance harness (Phase 0 of the libc work). Every estimate for that work is a guess
# until this has been run on hardware, so it ships with every image.
#
# ★★ Must SUCCEED, and the previous artifact is deleted FIRST. This used to end in
# `|| echo "(skipped/failed — continuing)"`, which is uniquely dangerous for THIS binary: the probe
# is the regression floor, so a stale one does not fail — it reports `pass=39 fail=0` about a kernel
# that no longer exists. `rm -rf build_initrd` above does not protect against this, because the
# stale copy lives in tests/posix/target/, not in the initrd. Deleting it means a failed build can
# only produce a MISSING probe, never a lying one.
echo "[9i] Building posixprobe (std target_os=nyx)..."
rm -f tests/posix/target/x86_64-unknown-nyx/release/posixprobe
./Build-std.sh tests/posix posixprobe

echo "[10/10] Generating App Tarball..."
rm -rf build_initrd

# 1. Create the App Folders
mkdir -p build_initrd/apps/Init.nyx
mkdir -p build_initrd/apps/WindowServer.nyx
mkdir -p build_initrd/apps/Terminal.nyx
mkdir -p build_initrd/apps/Settings.nyx
mkdir -p build_initrd/apps/Explorer.nyx
mkdir -p build_initrd/apps/SystemMonitor.nyx
mkdir -p build_initrd/apps/GlCube.nyx
mkdir -p build_initrd/apps/ImageViewer.nyx
mkdir -p build_initrd/apps/StdHello.nyx
mkdir -p build_initrd/apps/StdChild.nyx
mkdir -p build_initrd/apps/QcStudio.nyx
mkdir -p build_initrd/apps/StdGui.nyx
mkdir -p build_initrd/apps/Notepad.nyx
mkdir -p build_initrd/apps/WifiAgent.nyx
# The harness writes ONLY inside this scratch directory (probes/mod.rs panics on any path that
# escapes it). It has to exist up front — mkdir(2) is one of the things being probed.
mkdir -p build_initrd/apps/PosixProbe.nyx/scratch
mkdir -p build_initrd/apps/HelloC.nyx

# 2. Copy the compiled binaries into the folders as 'run.bin'
cp target/x86_64-nyx/release/nyx-init build_initrd/apps/Init.nyx/run.bin

# WindowServer.nyx is Meridian. There is no longer a choice to make here: $WINDOW_SERVER selected
# between `compositor` and `shell` until step 20 retired the former.
#
# ⚠️ apps/init execs the PATH, not a binary name — but the window server is PID 4
# (`COMPOSITOR_PID` in libs/gui/src/app.rs) purely by fork order, so it must stay the FIRST thing
# init spawns. That constant's name is now the only thing on the image that remembers the
# compositor existed.
if [ ! -f "target/x86_64-nyx/release/shell" ]; then
    echo "  !! the Meridian shell did not build; expected target/x86_64-nyx/release/shell" >&2
    exit 1
fi
cp target/x86_64-nyx/release/shell build_initrd/apps/WindowServer.nyx/run.bin
# D3: terminal is a std binary now — its output lives under apps/terminal/target/ (workspace-excluded),
# not the root target dir.
cp apps/terminal/target/x86_64-unknown-nyx/release/nyx-terminal build_initrd/apps/Terminal.nyx/run.bin
# D4: a bundled sample so `compile` (no args) works out of the box — the registry dispatches it to the
# qclang backend by its .ql extension and emits OpenQASM next to it.
cp apps/qcstudio/samples/sample.ql build_initrd/apps/Terminal.nyx/sample.ql 2>/dev/null || true
cp target/x86_64-nyx/release/nyx-settings build_initrd/apps/Settings.nyx/run.bin
cp target/x86_64-nyx/release/nyx-explorer build_initrd/apps/Explorer.nyx/run.bin
cp target/x86_64-nyx/release/nyx-sysmon build_initrd/apps/SystemMonitor.nyx/run.bin
cp target/x86_64-nyx/release/nyx-glcube build_initrd/apps/GlCube.nyx/run.bin
cp target/x86_64-nyx/release/nyx-imageviewer build_initrd/apps/ImageViewer.nyx/run.bin
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
# Meridian step 15: a document to open. Text starts on /mnt/nvme/untitled.md when nothing is handed
# to it, so without this the first thing anyone sees is an empty column and no way to tell whether
# the app works. Double-click it in Files to prove the launch-argument path at the same time.
cp apps/notepad/samples/atlas-sizes.md build_initrd/atlas-sizes.md 2>/dev/null || true
# POSIX conformance harness + the fixed-content file its read-only probes measure themselves against
# (its size is cross-checked with syscall 541, which already works, so a wrong `read` is detectable).
# Not `|| true`: see [9i]. A probe that fails to ship is a missing row; a probe that ships stale is
# a false pass, and this harness exists precisely to be believed.
cp tests/posix/target/x86_64-unknown-nyx/release/posixprobe build_initrd/apps/PosixProbe.nyx/run.bin
cp tests/posix/probe.txt build_initrd/apps/PosixProbe.nyx/probe.txt

# C test binaries. Skipped rather than fatal when libc.a is absent: musl is built by
# ./Build-musl.sh, which is deliberately NOT part of this script (it takes ~90s and changes rarely).
#
# run.bin is linked at 0x40000000 where the native apps live. gcc's default 0x400000 sits INSIDE
# the kernel image (linked 0x200000..~0x15d5000, and mapped into every address space), which is
# what froze the machine. `hello_lo` keeps that broken base deliberately: it is the regression
# test for the loader's overlap check, and must now fail cleanly instead of killing the box.
# Launch the extras from the terminal with `exec <path>`.
if [ -f vendor/musl/lib/libc.a ]; then
  echo "[9j] Building C test binaries (musl + libc-free, two load addresses)..."
  # Output is NOT silenced and the destination is cleared first. Previously a failed build only
  # printed a note while leaving the PREVIOUS run.bin in place — so the next boot silently tested
  # a stale binary under the new binary's name, which is the most expensive kind of wrong.
  # ★ cpptest was being copied WITHOUT being cleared first — precisely the hazard the note above
  # describes, reintroduced for one binary. Clearing is now driven off the same list as copying,
  # so the two cannot drift apart again.
  HELLOC_BINS="hello:run.bin hello_lo:hello_lo smoke:smoke bare:bare cpptest:cpptest cxxtest:cxxtest eventloop:eventloop"
  for pair in $HELLOC_BINS; do
    rm -f "build_initrd/apps/HelloC.nyx/${pair#*:}"
  done
  if ./tests/helloc/build.sh; then
    for pair in $HELLOC_BINS; do
      src="tests/helloc/${pair%%:*}"
      # cxxtest is absent unless ./Build-libcxx.sh has been run. Note it rather than failing the
      # image build — but never silently, because a missing test binary nobody mentions is how a
      # boot gets spent running something that was never there.
      if [ -f "$src" ]; then cp "$src" "build_initrd/apps/HelloC.nyx/${pair#*:}"
      else echo "  (note: $src not built — not shipped)"; fi
    done
  else
    echo "  (helloc build FAILED — HelloC.nyx ships with no binaries)"
  fi
else
  echo "[9j] helloc skipped - run ./Build-musl.sh to build libc.a first"
fi

# 3. Copy any JSON manifests from the source folders into the App Bundles
cp apps/init/*.json build_initrd/apps/Init.nyx/ 2>/dev/null || true
cp apps/shell/*.json build_initrd/apps/WindowServer.nyx/ 2>/dev/null || true
cp apps/terminal/*.json build_initrd/apps/Terminal.nyx/ 2>/dev/null || true
cp apps/settings/*.json build_initrd/apps/Settings.nyx/ 2>/dev/null || true
cp apps/explorer/*.json build_initrd/apps/Explorer.nyx/ 2>/dev/null || true
cp apps/sysmon/*.json build_initrd/apps/SystemMonitor.nyx/ 2>/dev/null || true
cp apps/glcube/*.json build_initrd/apps/GlCube.nyx/ 2>/dev/null || true
cp apps/imageviewer/*.json build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true

# 3b. Bundle the Image Viewer's sample image asset(s) so it has something to open on launch.
# Only ship real image files — NOT the generator script (make_sample.py).
cp apps/imageviewer/assets/*.bmp build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true
cp apps/imageviewer/assets/*.tga build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true
# A2b: PNG + JPEG samples so the viewer's click-to-cycle can exercise both decoders on-device.
cp apps/imageviewer/assets/*.png build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true
cp apps/imageviewer/assets/*.jpg build_initrd/apps/ImageViewer.nyx/ 2>/dev/null || true

# 3c. Each app carries its OWN icon inside its .nyx bundle as icon.png. Meridian does not read them
# — its dock and Command are drawn from the icon set compiled into `libs/meridian` — but an app still
# declares the path in its window header, and `apps/explorer` decodes them (via libs/image) to pick a
# file's application. Committed artwork; regenerate with `python3 tools/make_icons.py`.
install_icon() { cp "assets/icons/$1.png" "build_initrd/apps/$2.nyx/icon.png" 2>/dev/null || true; }
install_icon terminal    Terminal
install_icon settings    Settings
install_icon explorer    Explorer
install_icon sysmon      SystemMonitor
install_icon glcube      GlCube
install_icon imageviewer ImageViewer
install_icon stdhello    StdHello
install_icon qcstudio    QcStudio
install_icon stdgui      StdGui
install_icon notepad     Notepad
install_icon posixprobe  PosixProbe
# Borrows the terminal glyph: it is a console program, and a bespoke icon is not worth a new drawing
# routine for a test binary.
install_icon terminal    HelloC
# The shell's own mark, in the window server's bundle.
install_icon nyx         WindowServer

# 3d. ★ The shared typeface bundle. ONE copy of the five faces for the whole system.
#
# Until 2026-09-08 `libs/meridian/src/font.rs` `include_bytes!`d these, which put 1.93 MB in the
# shell and System Monitor and 1.24 MB in every other ported app — 10.6 MB of an image that was
# 33.6 MB, eight copies of the same five files. `--gc-sections` cannot drop a byte of a static
# something reaches, so the only fix was to stop embedding them. Apps read what they need out of
# /mnt/nvme/fonts/ at startup; a missing bundle falls back to DejaVu, which is visible on screen.
#
# The OFL licences ship WITH the fonts, which is the point of putting them here rather than leaving
# them in the source tree where the binary that uses them cannot reach one.
mkdir -p build_initrd/fonts
cp libs/meridian/fonts/*.ttf build_initrd/fonts/
cp libs/meridian/fonts/LICENSE-*.txt build_initrd/fonts/

# 4. Package it into a lightweight tape archive.
#
# ⚠️ `fonts` is a second top-level member alongside `apps`. `installer::extract_tar_to_ext4` walks
# whatever the archive contains and mkdir/writes each path under /mnt/nvme, so it needed no change —
# but an archive that lists only `apps` silently ships a system with no typefaces.
cd build_initrd
tar -cf ../initrd.tar apps fonts
cd ..
cp initrd.tar nyx-kernel/src/initrd.tar
touch nyx-kernel/src/main.rs

echo "=========================================="
echo "         Compiling NyxOS Kernel           "
echo "=========================================="
unset RUSTFLAGS
cargo run --package nyx-kernel --release --target x86_64-unknown-none