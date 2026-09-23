#!/usr/bin/env bash
# Boot Nyx in QEMU and show its screen in a web browser (noVNC).
#
# This is the "try it without building it" path: by default it downloads the prebuilt image
# from the latest GitHub release, creates the NVMe disk the kernel needs, boots it headless, and serves the
# display at http://localhost:6080. The Codespaces demo (.devcontainer/demo/) runs exactly this.
#
#   tools/demo/run-demo.sh                              # download the release image and run it
#   NYX_IMG=target/x86_64-unknown-none/release/nyx-kernel.efi.img tools/demo/run-demo.sh
#                                                       # run your own build instead
#
# Needs: qemu-system-x86_64, OVMF, sgdisk (gdisk), mkfs.ext4 (e2fsprogs), curl, websockify, noVNC.
# Debian/Ubuntu: apt install qemu-system-x86 ovmf gdisk e2fsprogs curl websockify novnc
#
# What a QEMU boot does NOT have: the Intel GPU (everything is drawn on the CPU), Wi-Fi, the
# touchpad. See docs/BUILD.md.
set -euo pipefail

DIR=${NYX_DEMO_DIR:-$HOME/.nyx-demo}
IMG_URL=${NYX_IMG_URL:-https://github.com/Asmodeus14/Nyx/releases/latest/download/nyx-x86_64.img}
IMG=${NYX_IMG:-$DIR/nyx-x86_64.img}
DISK=$DIR/nvme.img
PORT=${NYX_WEB_PORT:-6080}
NOVNC_DIR=${NOVNC_DIR:-/usr/share/novnc}
WEBSOCKIFY=${WEBSOCKIFY:-websockify}

mkdir -p "$DIR"

# 1. The boot image.
if [ ! -f "$IMG" ]; then
    echo "[demo] downloading the Nyx demo image..."
    curl -fL --progress-bar -o "$IMG.part" "$IMG_URL"
    mv "$IMG.part" "$IMG"
fi

# 2. The NVMe disk. The kernel requires one: a GPT whose first partition has the Linux filesystem
#    type GUID (nyx-kernel/src/fs.rs), formatted ext4 without the features lwext4 rejects. On first
#    boot Nyx's installer unpacks its apps onto it. Built without root: the partition is formatted
#    as a plain file and spliced in with dd.
if [ ! -f "$DISK" ]; then
    echo "[demo] creating the NVMe disk image..."
    truncate -s 1G "$DISK"
    sgdisk --clear --new=1:2048:0 --typecode=1:8300 --change-name=1:nyxroot "$DISK" >/dev/null
    END=$(sgdisk --info=1 "$DISK" | grep -oE 'Last sector: [0-9]+' | grep -oE '[0-9]+')
    truncate -s $(( (END - 2048 + 1) * 512 )) "$DIR/part.img"
    mkfs.ext4 -q -F -b 4096 \
        -O ^64bit,^metadata_csum,^metadata_csum_seed,^huge_file,^dir_nlink,^extra_isize,^orphan_file \
        -L nyxroot "$DIR/part.img"
    dd if="$DIR/part.img" of="$DISK" bs=512 seek=2048 conv=notrunc status=none
    rm -f "$DIR/part.img"
fi

# 3. Firmware: the COMBINED OVMF image. The split _CODE / _4M variants are pflash images and boot
#    to a blank screen under -bios.
OVMF=${NYX_OVMF:-}
if [ -z "$OVMF" ]; then
    for f in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd; do
        [ -f "$f" ] && OVMF=$f && break
    done
fi
[ -n "$OVMF" ] || { echo "[demo] no OVMF.fd found (apt install ovmf, or set NYX_OVMF)"; exit 1; }

ACCEL=()
if [ -w /dev/kvm ]; then ACCEL=(-enable-kvm); fi

# 4. QEMU, headless, display on VNC :0 (localhost only — websockify is the public face).
#    -cpu max: userspace is built for SSE4.2 + AES-NI, which QEMU's default CPU model lacks.
echo "[demo] booting Nyx (${ACCEL[*]:-TCG, no KVM — expect about a minute to reach the desktop})..."
qemu-system-x86_64 "${ACCEL[@]}" \
    -bios "$OVMF" -cpu max -smp 2 -m 2048 \
    -drive "format=raw,file=$IMG" \
    -drive "format=raw,file=$DISK,if=none,id=nvm" -device nvme,serial=nyx0001,drive=nvm \
    -serial "file:$DIR/serial.log" \
    -display none -vnc 127.0.0.1:0 ${NYX_QEMU_EXTRA:-} &
QEMU_PID=$!
trap 'kill $QEMU_PID 2>/dev/null || true' EXIT

# 5. Serve noVNC and bridge it to QEMU's VNC port. A copy of noVNC with an index page, so opening
#    the bare URL (which is what Codespaces does) lands on a connected, scaled viewer.
rm -rf "$DIR/web" && cp -rL "$NOVNC_DIR" "$DIR/web"
cat > "$DIR/web/index.html" <<'HTML'
<!doctype html><meta charset="utf-8"><title>Nyx</title>
<meta http-equiv="refresh" content="0; url=vnc.html?autoconnect=1&resize=scale&reconnect=1&reconnect_delay=2000">
<p>Connecting to Nyx… <a href="vnc.html?autoconnect=1&resize=scale&reconnect=1">open the viewer</a></p>
HTML
echo "[demo] open  http://localhost:$PORT/   (the desktop appears once Nyx has booted)"
echo "[demo] the kernel log is in $DIR/serial.log"
$WEBSOCKIFY --web "$DIR/web" "$PORT" 127.0.0.1:5900
