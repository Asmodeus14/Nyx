# Building and running Nyx

Everything here was checked against `Build.sh`, `tools/runner`, `rust-toolchain.toml`,
`.cargo/config.toml` and `.github/workflows/`. If a step drifts from those files, the files win.

## Requirements

| | |
|---|---|
| Host | Linux. Development happens in **WSL `Ubuntu-24.04`**; the Windows host has no Rust toolchain. |
| Rust | Pinned by `rust-toolchain.toml`: `nightly-2026-07-01`, components `rust-src` + `llvm-tools-preview`, targets `x86_64-unknown-none` (kernel) and `x86_64-unknown-uefi` (the runner's bootloader build — CI failed without it). |
| C toolchain | `clang`, `lld` — ACPICA and lwext4 are compiled into the kernel by `nyx-kernel/build.rs`. |
| QEMU (optional) | `qemu-system-x86_64` + OVMF. See [Running in QEMU](#running-in-qemu). |

A Dev Container is provided in `.devcontainer/`. It installs the C toolchain but **not QEMU**, and
its floating `nightly` is overridden by `rust-toolchain.toml` in the repo.

## Build

```bash
./Build.sh          # builds userspace, packs the initrd, builds the kernel, then launches QEMU
CI=1 ./Build.sh     # same build, but the runner skips QEMU (this is what CI gets automatically)
```

What `Build.sh` does, in order:

1. Builds the `no_std` apps (`init`, `shell`, `settings`, `explorer`, `sysmon`, `glcube`, …) for
   `targets/x86_64-nyx.json` with `-Z build-std=core,alloc`.
2. Builds the **`std`** programs (`terminal`, `notepad`, `qcstudio`, `stdgui`, and the tests in
   `tests/`) through `Build-std.sh`, which injects Nyx's std platform layer (`vendor/nyx-std/`) into
   the toolchain's `rust-src` and builds with `-Z build-std=std` for `targets/x86_64-unknown-nyx.json`.
3. Stages everything under `build_initrd/` (apps, fonts, `etc/`), tars it to `initrd.tar`, and copies
   it to `nyx-kernel/src/initrd.tar`, where the kernel embeds it.
4. `cargo run --package nyx-kernel --release --target x86_64-unknown-none`. The runner
   (`tools/runner`, set as the `cargo run` runner in `.cargo/config.toml`) wraps the kernel in a
   UEFI disk image, `target/x86_64-unknown-none/release/nyx-kernel.efi.img`, then launches QEMU
   unless `CI` is set.

### Optional C/C++ runtime (POSIX floor)

These are **not** part of `Build.sh` — they are slow and change rarely:

| Script | Produces | Needs |
|---|---|---|
| `./Build-musl.sh` | `vendor/musl/lib/libc.a` — stock musl + the patches in `vendor/musl-patches/` | a C compiler |
| `./Build-libcxx.sh` | libc++ / libc++abi / libunwind against that musl | `Build-musl.sh` first, plus an LLVM 18.1.3 source checkout at `$HOME/llvm-src` |

`Build.sh` packages the C test programs in `tests/helloc/` when those libraries are present, and
notes their absence otherwise.

### Quantum credentials

`Build.sh` bakes cloud-QPU credentials into the image from a **gitignored** `quantum-credentials.txt`
(template: `quantum-credentials.example.txt`). Without it, the build still succeeds and cloud QPU
access is unconfigured. See [`quantum/remote.md`](quantum/remote.md).

## Running in QEMU

QEMU boots Nyx to the desktop, but **plain `./Build.sh` is not enough on its own**: the runner boots
the image with no disk, and the kernel requires an NVMe drive with an ext4 partition —
without one it stops with `FATAL: No NVMe Drive Detected!`.

A boot that works needs:

- **combined OVMF firmware** (`/usr/share/ovmf/OVMF.fd`). The runner searches several paths and
  accepts `NYX_OVMF=<path>`; note that the `_4M` / `_CODE`-only variants are pflash images.
  > Status: Verification required — the runner's first candidate is `OVMF_CODE_4M.fd`, which
  > is known to produce no output with `-bios` on Ubuntu 24.04. Set `NYX_OVMF=/usr/share/ovmf/OVMF.fd`.
- **`-cpu max`** (userspace is built for SSE4.2 + AES-NI).
- an **NVMe disk** with a GPT whose first partition has the Linux filesystem type GUID
  `0FC63DAF-8483-4772-8E79-3D69D8477DE4` (checked in `nyx-kernel/src/fs.rs`), formatted ext4
  *without* the features lwext4 rejects. On first boot the kernel's installer unpacks the initrd
  onto it.

One recipe that has booted to the desktop (needs `gdisk` and `e2fsprogs`):

```bash
truncate -s 4G nyx-nvme.img
sgdisk --clear --new=1:2048:0 --typecode=1:8300 nyx-nvme.img
END=$(sgdisk --info=1 nyx-nvme.img | grep -oE 'Last sector: [0-9]+' | grep -oE '[0-9]+')
truncate -s $(( (END - 2048 + 1) * 512 )) part.img
mkfs.ext4 -q -F -b 4096 \
  -O ^64bit,^metadata_csum,^metadata_csum_seed,^huge_file,^dir_nlink,^extra_isize,^orphan_file part.img
dd if=part.img of=nyx-nvme.img bs=512 seek=2048 conv=notrunc && rm part.img

NYX_OVMF=/usr/share/ovmf/OVMF.fd \
NYX_QEMU_ARGS="-cpu max -smp 4 -m 2048 \
  -drive format=raw,file=nyx-nvme.img,if=none,id=nvm -device nvme,serial=nyx0001,drive=nvm" \
./Build.sh
```

The runner connects QEMU's serial port to stdio, so the kernel log is readable.

**Shortcut:** `tools/demo/run-demo.sh` does all of the above — disk image, firmware, flags — and
serves the display in a browser via noVNC at `http://localhost:6080/`. By default it downloads the
prebuilt image from the latest GitHub release; `NYX_IMG=target/x86_64-unknown-none/release/nyx-kernel.efi.img`
runs your own build. The same script backs the Codespaces demo (`.devcontainer/demo/`).

What QEMU does **not** exercise: the Intel GPU (no render engine, so GPU composite/text fall back
to the CPU), Intel Wi-Fi, the I2C-HID touchpad, and real timing — QEMU's APIC runs far faster than
the laptop's, so anything timing-derived must be re-checked on hardware.

## Running on hardware

The development target is a Comet Lake laptop (Intel UHD, device `0x9BC4`). Write
`nyx-kernel.efi.img` to a USB drive and boot it via UEFI. There is **no serial console** on the test
laptop, so diagnostics are built into the system itself: the terminal's `sched`, `gpu`, `touchpad`,
`acpi log` and friends print what the kernel measured.

The terminal's `sched` output ends with the **kernel build stamp** — compare it before trusting a
hardware result, because a stale flash looks exactly like a fix that did not work.

## Host tests

`cargo test --workspace` does **not** work (the `no_std` app crates cannot host-build). Test the
library crates one at a time:

```bash
for p in nyx-meridian nyx-quantum nyx-quantum-rt nyx-net nyx-htmltext nyx-json \
         nyx-entity nyx-crypto nyx-toolchains qclang_compiler nyx-image nyx-api; do
  cargo test -p "$p"
done
```

Kernel-internal logic that is pure (e.g. `nyx-kernel/src/drivers/gesture.rs`) carries `#[cfg(test)]`
modules that can be compiled standalone with `rustc --test`.

Before adding a syscall, run `tools/check_dup_syscall_arms.sh`: the dispatcher is compiled with
warnings allowed, so a duplicate match arm silently shadows instead of failing.

## CI

`.github/workflows/build.yaml` (push / PR to `master` or `main`): installs the pinned toolchain with
both targets, installs QEMU + OVMF, runs `./Build.sh` (the runner sees `CI` and skips launching QEMU),
builds the kernel ELF again as a fallback step, and uploads the kernel, `.img` and `.efi` artifacts.
`release.yaml` handles releases.
