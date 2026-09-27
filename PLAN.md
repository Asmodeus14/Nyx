# Nyx Development Plan

*Audit date: 2026-09-27 · tree at `57cbe17` (branch `terminal-browsing`) · 251 commits.*
*Method: read the source; treat docs and comments as claims to check. Every status below cites code.
"HW-verified" means the repo contains hardware evidence (photos/video in `docs/`, or a recorded
hardware result). Everything else is "works as coded".*

Legend: ✅ working · 🟡 partial · 🔴 broken / hazardous · ⚪ missing · 🧪 experimental · 🗑️ remove

---

## 1. Executive Summary

Nyx is a monolithic `no_std` Rust kernel for x86-64/UEFI. On one real laptop (Comet Lake,
UHD `0x9BC4`), it does all of the following:

- boots to a GPU-composited desktop;
- joins WPA2 Wi-Fi through its own driver;
- fetches HTTPS pages with rustls on its own Rust `std` port;
- runs musl C and libc++ programs;
- renders a textured cube through a hand-written Gen9 3D pipeline;
- drives an I2C-HID precision touchpad discovered through ACPICA;
- submits a circuit to an IBM QPU.

That is an unusually broad **vertical slice**, and it is real: there is hardware evidence for each item.

**The foundations have not kept up with that breadth.**

- **The user/kernel boundary is not enforced.** `is_valid_user_ptr` accepts any address below
  `0x7FFF_FFFF_FFFF` (`interrupts.rs:637`). The kernel heap (`0x4444_4444_0000`) and the kernel image
  both live there. SMEP and SMAP are switched off on purpose (`main.rs:498-503`). No
  `copy_from_user` exists. So any ring-3 program can make the kernel read or write kernel memory.
- **Memory management leaks and is SMP-unsafe.**
  - No page reference counts, so every CoW fork leaks frames.
  - No TLB shootdown.
  - The global `MEMORY_MANAGER` lock is not interrupt-safe, and the CoW fault handler takes it.
- **Scheduler state is shared across cores without synchronisation.** Other cores push directly into a
  core's `static mut PER_CPU` task list (`percpu.rs:26`, `interrupts.rs:5588`). Most waits poll:
  - `wait4` every 20 ms;
  - futex every 10 ms;
  - the window server every 2 ms.
- **Nothing tests a boot automatically.** QEMU works locally, but the runner skips it when `CI` is set
  (`tools/runner/src/main.rs:18-21`). CI never runs `cargo test`, even though ~670 host tests exist.
- **One file holds the whole syscall surface.** `interrupts.rs` is 6,486 lines, and one ~4,200-line
  `match` handles 143 syscalls. The ABI is defined three times: the kernel `match`, `libs/api` and
  `vendor/nyx-std`.

**Recommendation.** Stop adding breadth for one foundation cycle, in this order:

1. **Phase 0 (safety net):** automatic tests.
2. **Phase 1 (boundary):** make ring 3 unable to corrupt ring 0.
3. **Phase 2 (memory/SMP correctness)** and **Phase 3 (a real wait/event core)**.

After that, the active goal (the Ladybird "Gate 3/4" track) has something solid to stand on. Almost
none of this means rewriting a working subsystem. It adds the layers that were skipped: a user-copy
layer, frame refcounts, wait queues, a GGTT allocator and a block layer.

One cheap, high-value finding came out of the audit. **The GGTT page at `0x1600_0000` is mapped
twice.** It is the first page of the display scanout buffer (`drivers/gpu/intel/mod.rs:583`), and
render bring-up then remaps it to the RCS ring (`render/mod.rs:619`). Present BLTs that touch the top
4 KiB of the screen write pixels into the render ring. That makes it the leading candidate for the
open "first scene of a boot hangs" GPU bug (`docs/GRAPHICS.md:154`).

---

## 2. Current State — what Nyx can actually do today

| Capability | State | Evidence |
|---|---|---|
| UEFI boot → ring-3 `init` → Meridian desktop | ✅ HW + QEMU | `main.rs:132-523`; `docs/media/nyx-on-hardware.mp4` |
| SMP, per-core round-robin scheduling | 🟡 | `scheduler.rs:383-600`; no priorities, no migration, unsynchronised cross-core mutation |
| Linux-numbered syscalls for std/musl | 🟡 | 65 Linux + 78 Nyx (501–578). Syscalls 2, 58 and 59 do **not** have Linux semantics (`interrupts.rs:1595`, `:2755`), so musl carries `0001-drop-SYS_open-use-openat.patch` |
| Rust `std` (`target_os=nyx`), musl, libc++ | ✅ HW | `vendor/nyx-std` (2,973 LOC), `vendor/musl-patches`, `tests/helloc/*.cpp` |
| ext4 on NVMe read/write | 🟡 | lwext4 via `fs.rs`. Polled NVMe at queue depth 1, no journaling, rewrite does not truncate (`ext4_wrapper.c` opens `"r+"`), mtimes always 0 |
| Wi-Fi (Intel 9462-class) + Ethernet (RTL8168) | ✅ HW | `drivers/net/iwlwifi.rs` (4,013 LOC, polled), `rtl8168.rs` |
| TCP/UDP/DNS/DHCP (smoltcp 0.9.1, in kernel) | 🟡 | client-only sockets: no `bind/listen/accept/sendto/recvfrom` (`vendor/nyx-std/sys/net_nyx.rs:12-22`) |
| HTTPS (rustls + webpki-roots, chain + hostname verified) | ✅ HW | `libs/net/src/http.rs:280-296, 471-481` |
| Intel GPU: BLT, owned scanout, HW cursor, backlight | ✅ HW | `drivers/gpu/intel/mod.rs`, `cursor.rs`, `backlight.rs` |
| Intel GPU: Gen9 3D pipeline, hand-encoded EU shaders, mini-GL | ✅ HW / 🟡 | `render/*`. Legacy ring, GGTT only, polled fences, first-scene hang recovered by reset |
| GPU compositing + GPU text | 🟡 HW | `render/compositor.rs`, `render/text.rs`, CPU fallback in `apps/shell` |
| Touchpad (I2C-HID, precision, gestures) | ✅ HW | `drivers/i2c_hid.rs`, `gesture.rs` (host tests) |
| PS/2 keyboard | 🟡 | chars only: Ctrl/Alt dropped (`shell.rs:14` `HandleControl::Ignore`), no key-up |
| USB (xHCI, boot-protocol HID) | 🧪 | `usb.rs`, polled every 8 ms; not HW-verified |
| ACPI (vendored ACPICA): shutdown, reboot, battery, backlight, thermal | ✅ HW / 🟡 | `acpi.rs`; walks are opt-in because they crashed before |
| QPU: local simulator, IBM/IonQ cloud | ✅ HW (cloud) | `libs/quantum`, `libs/quantum-rt` (149 host tests); kernel registry is empty by design (`quantum.rs:183`) |
| Audio, IPv6, suspend (S3), dynamic linking, multi-user | ⚪ | — |

**Maturity: pre-alpha research OS with a proven hardware vertical slice.** It demos well on one
machine. It is not robust against a hostile or even buggy userspace, and it has no automated
regression net.

---

## 3. Architecture Assessment

**What is right and should be kept**
- **Linux-shaped syscall numbering.** Stock `std` and musl need only a thin platform layer. This is
  the correct bet for reaching Ladybird and real software.
- **The window server is an ordinary ring-3 process** (`apps/shell`). Apps draw into SHM and the GPU
  composites. This is the right shape for a GPU-first desktop.
- **Per-core schedulers with a `WaitReason` on every blocked task, plus a reschedule IPI.** The
  structure is sound; the implementation is incomplete.
- **Diagnostics are built into the system**:
  - `schedstats` and syscall 575;
  - `postmortem.rs` writing CMOS breadcrumbs;
  - the `gpu` health report;
  - the panic screen, which does not allocate.
  This is Nyx's most distinctive engineering trait (see §12).
- **Pure logic lives in host-testable crates**: `libs/meridian` (321 tests), `libs/quantum*`,
  `libs/net`, `libs/htmltext`, and `drivers/gesture.rs`.

**What is structurally weak**
1. **The trust boundary.** Kernel and user share the lower half, and pointer validation is a range
   check (§4 Security).
2. **The kernel's address-space layout.** The heap (PML4[136]) and image (PML4[0]) sit in the lower
   half. `Process::new` copies all 512 PML4 entries, and `fork` deep-copies kernel page tables in
   0..256 (`memory.rs:618-700`). Kernel mappings added after a fork are invisible to that process,
   and the copies leak.
3. **Everything is ring-0 C or Rust with no IOMMU**: ACPICA, lwext4, the 4 kLOC Wi-Fi driver and the
   GPU driver. Isolating drivers is *not* a near-term goal (see §14), but it means kernel-side memory
   safety discipline matters more.
4. **Blocking is done by polling.** It shows up everywhere:
   - `wait4` (20 ms), futex (10 ms) and the sleep check on every tick;
   - NVMe spinning up to 10 M iterations (`nvme.rs:150`);
   - GPU fences spinning (`render/mod.rs:140`);
   - networking polled from the BSP idle task (`process.rs:643-649`);
   - the shell's 2 ms key wait.
   It costs both latency and power, and it is why "the shell must never block" is a rule rather than
   something the system guarantees.
5. **Subsystems are wired by hand, not registered.**
   - PCI binding is a `match class_code` duplicated across the legacy and ECAM scanners
     (`pci.rs:248-398` vs `424-603`).
   - MSI programming is written three times.
   - `drivers/block.rs` is a trait that nothing implements.
6. **Global singletons plus `static mut`.** There are 41 `static mut`s, and `#![allow(warnings)]`
   covers the kernel crate and 13 crate roots. It hid a duplicate-syscall-arm bug before
   (`tools/check_dup_syscall_arms.sh`).

**Is the kernel too monolithic?** As a *monolithic kernel*, it is fine. For a one-developer
research OS, that is the right choice; microkernel isolation would cost years. The problem is
*source* monolith: `interrupts.rs` mixes the IDT, ISR glue, RTC date maths and roughly 4,200 lines of
syscalls across GPU, ACPI, Wi-Fi, sockets and the filesystem. Split it by subsystem (Phase 4). Do not
change the kernel architecture.

---

## 4. Subsystem Status

### Kernel (boot, interrupts, CPU state)
| Item | State | Evidence / note |
|---|---|---|
| Boot sequence | 🟡 | Panics without NVMe+ext4 (`main.rs:340-343`). Re-extracts the embedded initrd onto ext4 **every boot** (`main.rs:336`). `init` path hard-coded (`:477`). Window server must be PID 4 by fork order |
| IDT / exceptions | 🟡 | NMI (2), #MC (18), #DB (1), 9, 10 and the APIC spurious vector 0xFF have no handler. Wi-Fi MSI vector 0x31 is programmed (`pci.rs`) with no IDT entry |
| IST stacks | 🔴 | Only a double-fault IST, and that one `DF_STACK` is shared by all cores (`gdt.rs:29-33`). #PF has no IST, so a kernel stack overflow becomes a double fault |
| AP CPU state | 🔴 *(verify)* | The trampoline only ORs bits into CR0 (`trampoline.asm:13-15, 42-44`), and `init_syscalls` sets only MP and NE (`interrupts.rs:1403-1407`). After INIT, CR0.CD/NW are set, so **APs may run with caches disabled and CR0.WP=0**. Confirm by reading CR0 on each core |
| Timekeeping | ✅ *(re-verify on HW)* | TSC from CPUID 0x15/0x16, APIC timer calibrated to 1 ms (`apic.rs:269-366`). `UPTIME_MS` is TSC-derived via `fetch_max` (`interrupts.rs:1195`). This supersedes the older "clock runs 5.5× fast" observation; still needs a HW stopwatch check |
| Panic / post-mortem | ✅ | Panic paints without locks (`main.rs:525-591`), but other cores are **not halted** (`:571`). CMOS post-mortem (`postmortem.rs`) |

### Memory
| Item | State | Evidence |
|---|---|---|
| Frame allocator | 🟡 | Bump plus an intrusive LIFO free list (`memory.rs:89-256`). No refcounts, no double-free detection; contiguous allocations never reuse frames (`:134-176`) |
| Kernel heap | 🟡 | `linked_list_allocator`, 64 MiB fixed, IRQ-masked wrapper (`allocator.rs`). Mapped **executable** (`:66`). One lock, O(n) first-fit, shared by all cores. Also holds tmpfs (8 MiB cap) and every socket buffer |
| Per-process address spaces | ✅ | One PML4 per process; threads share CR3 |
| CoW fork | 🔴 | Works, but the original frame is never freed (no refcount), so each fork leaks the parent's resident set. Parent TLB flushed on the local core only |
| mmap | 🔴 | Anonymous only, eager-zeroed, ignores `prot`. A user-supplied `addr` is **not validated** (`interrupts.rs:1681`): a non-canonical value panics the kernel, and an upper-half value maps user pages into kernel space. File `mmap` returns ENOMEM (`vfs.rs:470`) |
| W^X | 🟡 | ELF segment flags honoured, but W+X pages are allowed with only a log line (`process.rs:240-258`); `mprotect` grants W+X |
| TLB shootdown | ⚪ | none. `munmap`, `mprotect` and CoW are unsafe for multi-core threads |
| Reclaim on exit | 🟡 | User frames are freed. **Leaked:** the kernel stack (acknowledged at `interrupts.rs:2965`), the PML4, the copied kernel page tables, and CoW frames |
| `MEMORY_MANAGER` lock | 🔴 | A bare spinlock at 33 sites, including the #PF CoW path. A fault or preemption while it is held deadlocks the core (`memory.rs:573-604`) |

### Scheduling
| Item | State | Evidence |
|---|---|---|
| Algorithm | 🟡 | Round-robin over a per-core `Vec` (192 slots), idle task last. O(n) per-tick scans. No priorities, despite the "SMART PRIORITY" comment (`scheduler.rs:508`) |
| Placement | 🟡 | Least-loaded core at spawn, through an inbox (`scheduler.rs:228-266`). No migration or stealing afterwards; `fork`/`clone` placement concentrates work. The memory note "APs switch zero times" should be re-measured with `sched` |
| Cross-core mutation | 🔴 | `ipc_send`, futex wake, `wake_input_waiters` and `wait4` reap all modify another core's tasks with no lock (`interrupts.rs:5588, 3491, 3528`, `scheduler.rs:181`) |
| Sleep/wake | 🟡 | Deadline checked on every tick of the owning core. No wait queues; futex, wait4 and pipes poll |
| Keyboard/mouse IRQ routing | 🟡 | BSP only (`main.rs:267-271`) |

### Processes
| Item | State | Evidence |
|---|---|---|
| fork / clone(threads) / exit / wait4 | 🟡 | No thread groups (`getpid == gettid`, `interrupts.rs:3373`). `exit_group` does not kill siblings. No reparenting, and `init` never reaps, so zombies accumulate |
| execve | 🔴 | Non-Linux ABI `(path_ptr, len, arg_ptr, arg_len)`. Tears down the address space **before** parsing the ELF (`interrupts.rs:2834` vs `:2850`), so a bad binary leaves a gutted process. Shreds a CR3 that live threads still share |
| ELF loader | 🔴 | Does not validate class, machine, phdr bounds, `p_offset+p_filesz` against the file size (an out-of-bounds kernel read), vaddr overflow or entry point (`process.rs:113-271`). Static only |
| Signals | 🟡 | Actions, masks, sigreturn and tkill are present. Delivered **only on syscall return** (`interrupts.rs:1566`), so a CPU-bound process cannot be killed. `kill` searches the local core only |

### Syscalls
- 143 numbers in one `match` (`interrupts.rs:1571-~5750`); ENOSYS logged once per number.
- The custom range 501–578 is largely **op-multiplexed diagnostics**: GPU health, ACPI probes, EC
  dump, Wi-Fi counters, DSDT dump. It is a debug console exposed as ABI.
- Unvalidated user pointers:
  - 510, 511 and 541 (`interrupts.rs:4710-4767`); 511 is an arbitrary kernel write;
  - DNS (`:6300`);
  - 539/540 accept kernel bases;
  - `arch_prctl(SET_FS)` panics on non-canonical input (`:3017`).
- Many handlers range-check and then fault in-kernel on unmapped pages, which panics the machine.
- The ABI is defined in three places: the kernel, `libs/api` (105 wrappers), and `vendor/nyx-std`
  (its own constants plus `asm!`).

### Drivers
| Driver | State | Note |
|---|---|---|
| PCI | 🟡 | ECAM with a port-IO fallback. No BAR sizing or assignment, no MSI-X, MSI hand-coded ×3, no driver registry |
| NVMe | 🟡 | One namespace, one I/O queue of 16, command IDs fixed, one 4 KiB bounce buffer, no PRP lists, interrupts off, spin-wait. Writes are one 512 B sector per command (`fs.rs:109-118`). A 102 ms IF-off stall was measured (`nvme.rs:215`) |
| AHCI | 🗑️ | `ahci.rs` has no callers |
| xHCI | 🧪 | Boot-protocol HID, polled, no hubs or mass storage |
| I2C / I2C-HID | ✅ | Polled DesignWare controller; the touchpad is HW-verified |
| iwlwifi | ✅ HW / 🟡 | WPA2-PSK CCMP only, polled (no ISR), **logs the PSK in plaintext** (`iwlwifi.rs:2404-2405`) |
| RTL8168 | ✅ | MSI 0x30 |
| Intel GPU | see Graphics | |
| ACPI/thermal/battery/backlight/fans | ✅/🟡 | Fan override disabled by design after an overheating incident (`laptop_fans.rs`). S3 is a stub (`custom_acpi.c:1390`) |

### Storage
| Item | State |
|---|---|
| Block layer | ⚪ (`drivers/block.rs` has no implementers; lwext4 calls `GLOBAL_NVME`, a lock-free `static mut`, `fs.rs:11`) |
| VFS | 🟡 A mount table keyed by path string (`/tmp`, `/mnt/nvme`, no `/`). **The global mount lock is held across polled disk I/O** (`vfs.rs:408`). No vnodes; each `read()` reopens the file; offsets are `u32` |
| fd table | ✅ per-process, 256 slots (files, sockets, pipes, socketpairs) |
| Cache | 🟡 lwext4's 256-block cache only; no page cache |
| Durability | 🔴 Journal compiled in but never started or recovered; the Rust "WAL" is a stub (`vfs.rs:105-131`) |
| Correctness bugs | 🔴 rewrite doesn't truncate; ext4 mtimes are always 0; the tar installer does not bounds-check (`installer.rs:10,34`) |
| Raw disk writes | 🔴 `entity/seed.rs` writes NVMe **LBA 1000** outside any filesystem (its comment says 2000) |
| Permissions | ⚪ no uid/gid; `PermissionDenied` is never returned |

### Networking
| Item | State |
|---|---|
| Stack | smoltcp in kernel: IPv4, DHCPv4, TCP, UDP, DNS. ICMP compiled in but unused; IPv6 ⚪ (by decision) |
| Driving | 🟡 Polled from the BSP idle task and from inside socket syscalls' wait loops. Ignores smoltcp's `poll_at`. Wi-Fi has no interrupt |
| Sockets | 🟡 AF_INET client only; `connect` with a custom timeout argument; no server or datagram API |
| Buffers | TCP 32 KiB RX/TX from the kernel heap; `max_burst_size` fixed to 32 after a 1000× throughput bug |
| DNS | ✅ up to 4 A records (smoltcp build env in `.cargo/config.toml`); the call sites clamp server lists |
| TLS | ✅ rustls 0.23 + webpki-roots. **Provider is `rustls-rustcrypto` 0.0.2-alpha**, which is unaudited. No revocation. Expiry depends on the RTC being right |
| HTTP | ✅ HTTP/1.1, keep-alive, chunked, gzip, redirects, POST |

### Graphics
| Item | State | Evidence |
|---|---|---|
| Discovery, MMIO, forcewake, GGTT | ✅ | `intel/mod.rs`. Forcewake parked after 1 s idle |
| Command submission | 🟡 | Legacy rings (BLT, RCS); no execlists, GuC, HW contexts or PPGTT (`cmd.rs:63`). Every ring dword is `clflush`ed |
| Completion | 🔴 | Polled `MI_STORE_DATA_IMM`/`PIPE_CONTROL` fences, 5 ms / 100 ms spins. No `MI_USER_INTERRUPT` |
| Reset | 🟡 | RCS reset + `ensure_ready` re-arm (the real fix in `53d7459`). The 8-strike latch disables the RCS. **No BLT reset** |
| GGTT address management | 🔴 | GVAs are hard-coded constants with no allocator:<br>• **scanout page 0 is aliased to the RCS ring** (`mod.rs:583` / `render/mod.rs:619`);<br>• window slots are `0x2000_0000 + id*16 MiB` and never recycled (`apps/shell/src/main.rs:3328, 3366`), so window #33 lands on `GVA_SSAA_RT` and later ones on the atlas and compositor state;<br>• a window larger than 16 MiB overruns its slot |
| Shaders | ✅ | Hand-assembled EU binaries plus a self-test (`render/eu.rs`). No compiler |
| Display | 🟡 | One owned linear scanout, SURF written once. No flip or double buffering; vblank polled |
| Userspace access | 🔴 | Raw syscalls with no ownership:<br>• 509 maps any SHM at any GVA, which can clobber rings or scanout;<br>• 512 BLTs between arbitrary GVAs;<br>• 508 gives any process the backbuffer;<br>• the mini-GL context is global (`gl.rs:109`) |
| GPU vs CPU | — | GPU: fills, present, window quads, glyph runs, mini-GL. CPU: all app content (`libs/gui`); mini-GL output is copied back into SHM by the CPU (`render/gl.rs:385`) |

### Userspace
- There are three ABI tiers:
  - `no_std` + `nyx_api`: shell, init, settings, explorer, sysmon, imageviewer, glcube, wifiagent;
  - Rust `std`: terminal, notepad, qcstudio, stdgui;
  - musl C/C++: `tests/helloc`.
- Static binaries only. They are packaged as `apps/<Name>.nyx/run.bin` in a tar that is
  `include_bytes!`d into the **kernel image** (`main.rs:69`).
- The terminal is a hard-coded ~40-command dispatcher (`apps/terminal/src/main.rs:3830-4180`) with no
  pipes, scripting or job control.
- `init` forks the window server and wifiagent, then loops forever without reaping.

### UI
- Meridian (`apps/shell`, 6 kLOC, plus `libs/meridian`, 24 kLOC with 321 tests) is the only window
  server.
- Window creation:
  1. The **app** creates the SHM and sends `MSG_REQ_WINDOW` to hard-coded PID 4.
  2. The shell maps it and replies.
  (`docs/UI.md:36-38` describes this backwards.)
- A single dirty-rect damage box. GPU compositing with a CPU fallback.
- Input: a global char queue that **any process can drain**. No key-up, no modifiers, no scroll wheel.
- The shell polls with a 2 ms timeout because nothing lets it wait on "input OR IPC OR timer".

### Security
| Item | State |
|---|---|
| User-pointer validation | 🔴 range check only; kernel heap and image pass it |
| SMEP / SMAP | 🔴 explicitly disabled (`main.rs:498-503`) |
| Syscall entry hygiene | 🔴 FMASK clears only IF, so there is no `cld` and AC/TF/DF survive. RCX is not re-checked for canonicality before `sysret` after `rt_sigreturn`/signal delivery/execve (the Intel SYSRET #GP-in-ring-0 class). `rt_sigreturn` copies user RFLAGS unfiltered, so **ring 3 can set IOPL=3** (`interrupts.rs:1787`) |
| CR0.WP on APs | 🔴 likely unset (see Kernel) |
| NX | ✅ EFER.NXE everywhere; 🟡 heap, MMIO and user framebuffer mapped executable |
| ASLR / stack canaries | ⚪ fixed addresses; C built with `-fno-stack-protector`; `AT_RANDOM` from an rdtsc xorshift (`process.rs:361-372`) |
| Authority model | ⚪ no uid, no capabilities: any process can power off (568), panic the kernel on purpose (555), drive the GPU, read the keyboard, or read any SHM |
| Secrets | 🔴 Wi-Fi PSK logged (`iwlwifi.rs:2404`). Quantum API keys baked into `initrd.tar`, and so into the **kernel ELF** of every locally built image (`Build.sh:281`), then stored plaintext on disk. Deliberate, because there is no paste, but it needs a better answer |
| Entropy | ✅ RDSEED/RDRAND with a fail-closed flag for crypto callers (`random.rs`); no kernel DRBG |
| Driver isolation / IOMMU | ⚪ all ring 0, no VT-d |

### Developer Infrastructure
| Item | State |
|---|---|
| Build | ✅ `Build.sh` (no_std apps) + `Build-std.sh` + musl/libc++ scripts; pinned nightly. Builds in WSL |
| QEMU | ✅ locally (needs combined OVMF, `-cpu max`, GPT+ext4 image). 🔴 **skipped in CI** (`runner/main.rs:18-21`) although CI installs QEMU |
| Host tests | ✅ ~670 `#[test]`s across 12 crates. 🔴 **not run in CI** |
| Syscall-duplicate check | 🟡 `tools/check_dup_syscall_arms.sh` exists but is not wired into anything |
| Lints | 🔴 `#![allow(warnings)]` on the kernel and ~13 crate roots |
| Kernel tests | ⚪ only `gesture.rs` and `hid_desc.rs` have tests; no in-kernel test runner |
| Debugging | ✅ unusually strong on-device telemetry (schedstats, `gpu`, CMOS post-mortem, F12 dump); ⚪ no serial on the laptop |
| Fresh clone | 🟡 `cargo build -p nyx-kernel` fails until `Build.sh` has produced `nyx-kernel/src/initrd.tar` |
| Debt tracking | Known problems live in ★/⚠️ prose comments (TODO count ≈ 7), which cannot be grepped as a backlog |

---

## 5. Critical Bottlenecks (ranked by leverage)

**B1. No automated boot/regression testing**
- **Problem:** every kernel change is validated by hand, usually by a power cycle.
- **Evidence:** CI runs `Build.sh` only; the runner returns early when `CI` is set; no `cargo test`; the duplicate-syscall script is not wired in.
- **Why it matters:** B2–B5 are invasive refactors of 143 syscalls and the memory manager. Without a net they will regress silently, as the smoltcp DNS=1 bug did.
- **Unblocks:** every phase.
- **Fix:**
  1. CI job: `cargo test` over the host crates.
  2. The dup-syscall script.
  3. A headless QEMU boot using the image recipe from `docs/BUILD.md`, `NYX_QEMU_ARGS="-display none"` and serial capture, which asserts boot markers and runs `posixprobe` to completion.
- **Complexity / priority:** M · **P0**

**B2. The user/kernel boundary is not enforced**
- **Problem:** ring 3 can make ring 0 read or write kernel memory, set IOPL, and panic the machine.
- **Evidence:** `interrupts.rs:637`, `:1423`, `:1787`, `:1681`, `:4710-4767`; `main.rs:498-503`.
- **Why it matters:** stability first (any buggy C program under musl can scribble on the kernel heap), then security. Ladybird is millions of lines of C++; it *will* pass bad pointers.
- **Unblocks:** running untrusted or large software, and multi-app robustness.
- **Fix:**
  1. A `UserPtr<T>`/`UserSlice` type whose only accessors are `copy_from_user`/`copy_to_user`. They check the range against the user half, walk the page tables for USER+PRESENT (+WRITABLE), and return `EFAULT` instead of faulting.
  2. Later, an exception-table fixup for speed.
  3. `FMASK |= DF|AC|TF|IOPL`; `cld` on entry; a canonical-RCX check before every `sysret` (fall back to `iretq`); sanitise RFLAGS in `rt_sigreturn`.
  4. Validate the `mmap` addr and `arch_prctl`.
  5. Re-enable SMEP immediately, and SMAP once all user access goes through the copy layer (`stac`/`clac`).
- **Complexity / priority:** L (mechanical across ~100 call sites) · **P0**

**B3. The memory manager is not SMP-correct and leaks**
- **Evidence:** no refcounts (`memory.rs:683-694`, `interrupts.rs:947-970`), no shootdown, a bare-spinlock `MEMORY_MANAGER`, the kernel-stack leak (`interrupts.rs:2965`), kernel tables copied per fork.
- **Why it matters:** long uptimes exhaust memory; multi-threaded programs (libc++ `std::thread`, Ladybird) corrupt memory via stale TLBs; #PF inside the lock deadlocks.
- **Unblocks:** threads at scale, file mmap, page cache, any long-running workload.
- **Fix:**
  1. A per-frame metadata array (refcount + flags) indexed by PFN.
  2. CoW that decrements and frees.
  3. A TLB-shootdown IPI with an ack mask.
  4. An IRQ-masking `MEMORY_MANAGER` lock, with a written lock order.
  5. Move the kernel heap into the upper half, so every process shares kernel PML4 entries 256..511 and never copies them.
  6. Free kernel stacks through a deferred reaper.
- **Complexity / priority:** L · **P0**

**B4. Unsynchronised cross-core scheduler state, and polling instead of waiting**
- **Evidence:** `static mut PER_CPU` mutated remotely (`percpu.rs:26`, `interrupts.rs:5588, 3491, 3528`); `wait4` 20 ms, futex 10 ms; signals only on syscall return.
- **Why it matters:** heisenbugs under load, latency floors of 10–20 ms, burned cores, the "shell must never block" rule, and unkillable spinning processes.
- **Unblocks:** responsiveness, the Ladybird event loop (Gate 3), power.
- **Fix:**
  - Every remote operation goes through the target core's inbox (the pattern already exists for placement) or a per-core IRQ-safe lock.
  - Introduce a `WaitQueue` primitive and use it for futex, wait4, pipes, IPC, input, sockets and GPU fences.
  - Deliver signals on every return to ring 3, including interrupts.
- **Complexity / priority:** L · **P0**

**B5. Broken AP CPU state (cheap to fix, possibly huge)**
- **Evidence:** CR0.CD/NW never cleared and WP never set on APs (`trampoline.asm`, `interrupts.rs:1403-1407`); one DF IST stack shared by all cores; no NMI/#MC handlers.
- **Why it matters:** if the APs really run uncached, every multi-core number ever measured is wrong, and it may explain "APs barely switch". WP=0 means kernel writes to read-only CoW pages silently land in the *shared* frame, which is a correctness bug that the CoW design depends on not having.
- **Fix:**
  - Set CR0 explicitly on every AP (`PE|MP|ET|NE|WP|PG`, clear CD/NW).
  - Allocate per-core IST stacks for DF, NMI and #MC.
  - Install handlers for vectors 1, 2, 18 and 0xFF.
- **Complexity / priority:** S · **P0**

**B6. GPU address space and ownership**
- **Evidence:** the scanout/ring alias; window GVA slots that are never recycled; syscalls 508/509/512/514-516 with no owner checks; polled fences.
- **Why it matters:**
  - the open first-scene hang;
  - a crash after 32+ windows over one boot;
  - any process can scribble on the display or the ring.
- **Unblocks:** a reliable GPU-first desktop, multiple GL clients.
- **Fix:**
  1. Move the scanout GVA now and add a GVA-overlap assertion at every `map_ggtt_page`.
  2. Add a kernel `GgttAllocator` (range allocator + free).
  3. Make GPU objects per-process handles, freed on exit.
  4. Use `MI_USER_INTERRUPT` + a WaitQueue for fence completion.
- **Complexity / priority:** S (alias) + M (allocator/ownership) + M (IRQ fences) · **P0 / P1**

**B7. The syscall/ABI monolith**
- **Evidence:** 6,486-line `interrupts.rs`; 3 ABI definitions; syscalls 2/58/59 non-Linux; `allow(warnings)`.
- **Why it matters:** every cross-cutting change (B2, B4) touches this file. Duplicated constants drift. Non-Linux numbers force libc patches, and each patch is a port tax.
- **Fix:**
  - Split into `syscall/{mod,fs,mm,proc,signal,net,ipc,gpu,platform,diag}.rs` with a dispatch table.
  - One `abi` table (a Rust source file) that `libs/api` and `nyx-std` both import.
  - Make 2/59 real Linux `open`/`execve` and move the Nyx variants to 5xx.
  - Turn warnings back on crate by crate.
- **Complexity / priority:** M · **P1**

**B8. Input model**
- **Evidence:** `VecDeque<char>` (`shell.rs:8-23`), `HandleControl::Ignore`, syscall 506 open to all, mouse as a polled snapshot.
- **Why it matters:** no shortcuts, no paste (hence the baked credentials), no key-up for games or GL, no text-field semantics for Ladybird, keystrokes stealable by any process.
- **Fix:**
  - A kernel `InputEvent {kind, code, value, mods, tsc}` ring exposed as a pollable fd, readable only by the display owner.
  - The keymap moves to userspace; the shell routes events to the focused window.
  - Ctrl/Alt chords plus a shell-owned clipboard.
- **Complexity / priority:** M · **P1**

**B9. Storage has no block layer, no durability and no concurrency**
- **Evidence:** `GLOBAL_NVME` `static mut`, polled QD1 NVMe, VFS lock across I/O, no journal, no truncate, `entity` raw LBA write, initrd re-extracted every boot.
- **Why it matters:** a power loss can corrupt ext4; a disk read stalls everything; Ladybird's resource loading needs file `mmap` and throughput.
- **Fix:**
  1. `BlockDevice` implemented by NVMe (IRQ completion, PRP lists, several outstanding commands).
  2. Take I/O out of the mount lock.
  3. Enable the lwext4 journal and recovery.
  4. Fix truncate and mtime.
  5. Version-stamp the initrd so it is installed only when it changes.
  6. Move the entity seed into a file.
- **Complexity / priority:** L · **P1**

**B10. No unified event/readiness primitive for userspace**
- **Evidence:** the shell's 2 ms `sys_read_key_wait` loop (`apps/shell/src/main.rs:4910`); wifiagent exists only because the shell cannot wait asynchronously; IPC targets hard-coded PID 4.
- **Why it matters:** latency and power in every GUI app; it is Gate 3 of the active goal (an event loop); every new service invents its own polling.
- **Fix:**
  - Make everything an fd: input, IPC endpoint, GPU fence, timer (`timerfd`-like), eventfd.
  - Implement `poll`/`epoll`-style readiness over WaitQueues (B4).
  - Add a service registry so clients find the window server by name, not PID.
- **Complexity / priority:** M (after B4) · **P1**

---

## 6. Technical Debt

**Remove (after confirming no callers; do not delete blindly)**

| Item | Reason |
|---|---|
| Kernel deps `aml`, `ext4plus`, `futures`, `volatile`, `uart_16550` | Unreferenced in `nyx-kernel/src`; build time and supply-chain weight |
| `drivers/ahci.rs`, `partitioner.rs`, the `vfs.rs` WAL stub (`:105-131`) | No callers. Keep AHCI in history; re-add when a block layer exists |
| `IntelGpuDriver::test_blitter`, `spin_scene` (`BOOT_GPU_DEMO=false`) | Unreachable |
| Duplicate PCI scanner bodies (`pci.rs:248-398` vs `424-603`) and the three MSI writers | One `probe(dev)` path and one `msi::program(dev, vector, apic_id)` |
| `nyx-recv/` (5.8 MB, tracked) | ACPI dumps are valuable. Move them to `docs/hardware/dell-g3/` or a separate repo; keep the `nyx-recv/dsdt.dsl` references in memory/docs working |
| Root clutter | `build_*.log`, `initrd.tar` copies, the 271 MB `~/ladybird` clone (gitignored; move it outside the repo) |

**Rename or clarify**
- `nyx-kernel/src/shell.rs` is the keyboard decoder; rename it to `keyboard.rs`.
- `nyx-kernel/src/gui.rs` is the boot/fallback painter; rename it to `boot_fb.rs`.
- `libs/gui/font.rs` vs `libs/meridian/font.rs`: choose one font pipeline.

**Fix documentation that contradicts code**

| Doc | Claim | Reality |
|---|---|---|
| `docs/UI.md:36-38` | "the shell creates the SHM" | the app does (`libs/gui/src/app.rs:82`) |
| `docs/ARCHITECTURE.md` | "stock musl … without an ABI shim" | syscall 2 is non-Linux; musl is patched |
| `docs/GRAPHICS.md` GVA table (`:62-70`) | complete | omits scanout `0x1600_0000`, windows `0x2000_0000+`, cursor, paper; hides the ring alias |
| `docs/GRAPHICS.md:154` | first-scene hang cause "not yet known" | alias found (hypothesis until HW confirms) |
| `docs/quantum/syscalls.md:13`, `future-hardware.md:87` | 501–572 / "575 next free" | 501–578; next free 579 (`KERNEL.md` is right) |
| `libs/meridian/src/lib.rs:16`, root `Cargo.toml:18-19` | "this machine has no QEMU" | QEMU works (`tools/runner`, `BUILD.md`) |
| `render/mod.rs:506-509` | forcewake "never released" | parked after 1 s (`mod.rs:161-234`) |
| `docs/KERNEL.md` Memory table | CoW, heap and mmap marked 🟢 | see §4 Memory |
| `entity/seed.rs` comment | "LBA 2000" | constant is 1000 |

**Practices**
- `#![allow(warnings)]` in 13 crate roots. Remove it per crate and deny `unreachable_patterns` in
  the kernel first.
- 41 `static mut`s. Convert them opportunistically as each subsystem is touched.
- Debt recorded as ★/⚠️ prose. Add a `docs/KNOWN_ISSUES.md` list, or `// DEBT(id):` tags, so it can
  be grepped.

---

## 7. Roadmap

The phases follow dependency order. The durations are rough, for one developer working at the
repository's historical pace.

```text
Phase 0 Safety net ──► Phase 1 Boundary ──► Phase 2 Memory/SMP ──► Phase 3 Wait/Event core
                                   │                                     │
                                   └──► Phase 4 Syscall/ABI split ◄──────┤
                                                                         ├──► Phase 5 Input pipeline
                                                                         ├──► Phase 6 Storage
                                                                         ├──► Phase 7 GPU platform
                                                                         └──► Phase 8 Networking
Phases 3–8 ──► Phase 9 Ladybird (Gates 3–4) ──► Phase 10 Desktop platform ──► Phase 11 Research
```

### Phase 0 — Safety net and quick wins (1–2 weeks) · P0

| # | Task | Effort | Impact |
|---|---|---|---|
| 0.1 | CI: `cargo test` over the host crates (`libs/meridian, quantum, quantum-rt, net, htmltext, json, entity, crypto, toolchains`, `tools/compiler, icons`) | S | High |
| 0.2 | CI: run `tools/check_dup_syscall_arms.sh` | S | Medium |
| 0.3 | CI: a headless QEMU boot smoke test. Build the GPT+ext4 image and boot with `-display none -serial file:`, then assert serial markers (`pre-syscalls`, init start, shell start) and a `posixprobe` pass line within N seconds. Needs a runner flag to allow QEMU under `CI` and an exit path (ACPI S5 or `isa-debug-exit`) | M | Transformative |
| 0.4 | Move the scanout GVA off `0x1600_0000`; add an overlap assertion to `map_ggtt_page` (both copies); re-test the first-scene hang on HW with the existing `gpu` report | S | High |
| 0.5 | AP CR0: clear CD/NW, set WP; per-core IST stacks for DF/NMI/#MC; handlers for 1, 2, 18 and 0xFF. Measure `sched` context-switch counts per core before and after | S | High |
| 0.6 | Stop logging the Wi-Fi PSK. Keep baked credentials out of any image that leaves the machine: a `Build.sh --no-creds` default for release/demo images, and a warning when the creds are baked | S | Medium |
| 0.7 | Delete the unused kernel deps; fix the contradicting docs in §6 | S | Low |

### Phase 1 — Enforce the user/kernel boundary (3–5 weeks) · P0
- 1.1 `uaccess.rs`: `copy_from_user`, `copy_to_user`, `strncpy_from_user`, `UserSlice`, with
  page-walk validation and `-EFAULT`. Convert every syscall; forbid raw user derefs (grep gate in CI).
- 1.2 Entry/exit hygiene: FMASK (IF|DF|AC|TF|IOPL|NT), a canonical-RCX check with an `iretq`
  fallback, RFLAGS sanitisation in `rt_sigreturn`.
- 1.3 Re-enable SMEP now. Enable SMAP once 1.1 is complete (`stac`/`clac` inside `uaccess` only).
- 1.4 ELF loader validation (class, machine, type, phdr bounds, file-size bounds, overflow, entry).
  `execve` parses and maps the new image into a *fresh* address space before tearing down the old one.
- 1.5 `mmap` hint/fixed validation; `arch_prctl` canonical check; `unmap_shm`/`destroy_shm` accept only
  the caller's own mappings.
- 1.6 Owner checks on privileged syscalls: power (568), deliberate panic (555), GPU, raw input and
  ACPI probes. Allow only the display owner or init (PID-based for now; capabilities in Phase 10).
- 1.7 A fuzz harness in `tests/posix`: call every syscall number with kernel addresses, non-canonical
  values, NULL and unmapped pointers. The kernel must survive (runs in the Phase 0.3 QEMU job).

### Phase 2 — Memory and SMP correctness (4–6 weeks) · P0
- 2.1 Per-PFN `PageMeta` array (refcount, flags). CoW decrements and frees; SHM and MMIO become flags
  instead of PTE-bit conventions.
- 2.2 A TLB-shootdown IPI (vector + ack bitmap) used by munmap, mprotect, CoW and exec on
  multi-threaded address spaces.
- 2.3 An IRQ-safe `MEMORY_MANAGER` lock; written lock order in `docs/KERNEL.md`; no #PF possible while
  it is held.
- 2.4 Kernel address-space layout: heap, stacks and MMIO in the upper half, shared PML4 entries
  256..511 built once. `fork` and teardown touch only 0..255. Map the kernel heap NX.
- 2.5 Reclaim kernel stacks, PML4 frames and copied tables through a deferred per-core reaper.
- 2.6 Cross-core scheduler mutation: remote wake, mailbox push, reap and kill become inbox messages
  (the existing placement pattern) plus an IPI. Remove `static mut PER_CPU` access from other cores.
- 2.7 Halt the other cores on panic (NMI IPI).
- 2.8 Measure the kernel heap: allocation-latency histogram in schedstats. Swap to a size-class/slab
  allocator only if the numbers justify it.

### Phase 3 — Wait/event core (3–4 weeks) · P0/P1
- 3.1 `WaitQueue` (per object, cross-core wake via inbox + IPI). Convert futex, wait4, pipes (real
  blocking and EPIPE/SIGPIPE), IPC recv, input and socket readiness.
- 3.2 Signal delivery on every return to ring 3 (interrupts included); `EINTR` everywhere it is
  specified.
- 3.3 A timer wheel or heap per core with deadline wakeups (not per-tick scans). Expose
  `timerfd`/`clock_nanosleep`. Evaluate tickless idle afterwards.
- 3.4 Readiness: `epoll` (or a Nyx `wait_many`) over fds. Input, IPC endpoints and GPU fences become fds.
- 3.5 `init` reaps children; reparent orphans to init; `exit_group` kills the thread group.
- **Exit criterion:** the shell's main loop blocks in one wait call with no timeout-polling, and
  `tests/helloc/eventloop.cpp` still passes.

### Phase 4 — Syscall and ABI structure (2–3 weeks, can overlap with 2–3) · P1
- 4.1 Split `interrupts.rs`:
  - `idt.rs`/`isr.rs` for the IDT and ISRs;
  - `syscall/*.rs` by subsystem;
  - RTC date maths back into `rtc.rs`.
  The dispatch table is a `match` that only forwards.
- 4.2 A single ABI source (`libs/abi`, `no_std`, no deps) with numbers, structs, errno and message
  types. `libs/api`, `vendor/nyx-std` and the kernel import it. Layout assertions become `const` checks.
- 4.3 Linux conformance where the number is Linux's: make `open(2)`, `execve(59)` and `fork/vfork(57/58)`
  standard, move the Nyx variants into 5xx, and drop `0001-drop-SYS_open` from the musl patches.
- 4.4 Move the diagnostic 5xx syscalls (ACPI probe, EC dump, DSDT, Wi-Fi ring state, GPU retry)
  behind one `sys_diag(op, buf)` gated to privileged callers, or a read-only `/proc`-like tmpfs view.
  This shrinks the ABI.
- 4.5 Remove `#![allow(warnings)]` from the kernel; `deny(unreachable_patterns)`.

### Phase 5 — Input pipeline (2 weeks) · P1
- 5.1 A kernel `InputEvent` ring (raw scancode → keycode, press/release, modifier state, TSC timestamp)
  for PS/2, I2C-HID and USB HID. Read through an input fd owned by the display server.
- 5.2 Keymap in userspace (`libs/meridian` or a new `libs/input`, host-tested). `pc_keyboard` stays
  only for the kernel panic/boot console.
- 5.3 Shell routing: focused-window key events with modifiers, Ctrl/Alt chords, a clipboard owned by
  the shell (`MSG_CLIPBOARD_*`), paste into the terminal. Removes the reason for baked credentials.
- 5.4 Input-to-photon latency probe: IRQ TSC → shell present TSC, shown in `sched`.

### Phase 6 — Storage (4–6 weeks) · P1
- 6.1 A `BlockDevice` trait (`read_blocks`/`write_blocks`/`flush`, async completion) implemented by
  NVMe. lwext4's `blockdev` calls go through it; `GLOBAL_NVME` becomes a registered device.
- 6.2 NVMe: MSI/MSI-X completion into a WaitQueue, PRP lists, several outstanding commands, multi-block
  writes. Removes the 102 ms IF-off stall.
- 6.3 VFS: drop the mount lock before calling into a filesystem (per-fs locking); open-file handles
  keep the lwext4 file open rather than reopening per read; 64-bit offsets.
- 6.4 Correctness: `O_TRUNC`/truncate, mtime/ctime set from the RTC, bounds checks in the tar installer.
- 6.5 Durability: start the lwext4 journal, run `ext4_recover` at mount, and `fsync` → FLUSH. Delete the
  WAL stub.
- 6.6 Boot: install the initrd only when its version stamp differs; make "no NVMe" boot to a
  tmpfs-only recovery shell instead of panicking; move the entity seed from raw LBA 1000 to
  `/mnt/nvme/etc/entity.seed` after checking that LBA 1000 lies outside every partition.
- 6.7 File-backed `mmap` (read-only, private) with a minimal page cache. Needed by Ladybird for fonts
  and resources and by the dynamic loader later.

### Phase 7 — GPU platform (4–6 weeks) · P1
- 7.1 `GgttAllocator`: a range allocator over the aperture with named reservations (rings, fences,
  scanout, cursor, state). Replace every hard-coded `GVA_*` constant and the shell's
  `0x2000_0000 + id*16 MiB` formula. Windows get size-exact ranges and free them on destroy.
- 7.2 GPU object handles: an SHM→GVA mapping is a per-process handle (509 returns a handle, never
  accepts a GVA); 512/536/537 take handles; release on process exit. The GL context is per process.
- 7.3 Interrupt-driven completion: `MI_USER_INTERRUPT` + a GT interrupt handler → fence WaitQueue;
  `sys_gpu_sync` blocks instead of spinning.
- 7.4 A BLT engine reset path mirroring `reset_render`.
- 7.5 Double-buffered scanout with SURF flips on vblank (the flip-done interrupt), replacing
  present-by-BLT copies where the whole frame is recomposited.
- 7.6 Measure `clflush` per ring dword vs a WC-mapped ring. Keep whichever the counters prefer.
- 7.7 mini-GL renders directly into the window's GVA (the compositor samples it) instead of the CPU
  copy back into SHM (`render/gl.rs:385`).
- **Deferred to research:** execlists/HW contexts, PPGTT, GuC (see §14).

### Phase 8 — Networking (3–4 weeks) · P1
- 8.1 A network task driven by the NIC IRQ (RTL8168 MSI already; iwlwifi ISR, with vector 0x31
  properly installed) and smoltcp's `poll_at` deadline, instead of idle-task polling and polling
  inside syscalls. Socket syscalls block on WaitQueues.
- 8.2 Complete the socket API: `bind`, `listen`, `accept`, `sendto`, `recvfrom`, `shutdown`,
  `getsockopt`/`setsockopt` (SO_RCVTIMEO replaces custom 549), non-blocking + `poll` consistency. A
  loopback interface for local IPC testing.
- 8.3 ICMP echo (`ping`) as a diagnostic, since the feature is already compiled in.
- 8.4 SNTP time sync at DHCP-lease time, so TLS expiry checks do not depend on the CMOS clock.
- 8.5 TLS provider risk: track `rustls-rustcrypto` maturity. Re-evaluate once `ring` or `aws-lc-rs`
  can build for `target_os=nyx`, or pin and audit the used primitives. No change now, just a tracked
  risk.
- 8.6 Throughput and latency benchmarks in the terminal (`net bench` against a known host) before any
  buffer tuning.
- Wi-Fi WPA3/SAE and MFP stay later (P2).

### Phase 9 — Ladybird, Gates 3–4 (the active goal, resumed) · P1
**Prerequisites:** Phases 1, 2, 3, 5, 6.7 and 8.2.
- 9.1 Gate 3: Ladybird `LibCore` event loop on Nyx epoll/timerfd/signals. Headless `LibWeb` test page
  → PNG in `/tmp`.
- 9.2 Gate 4: a C ABI to the window protocol (`libs/gui` exposed through a small C header: create
  window, SHM buffer, damage, input events), then a Ladybird window.
- 9.3 Decide dynamic linking vs static monolith: static first; add a dynamic loader only if binary
  size or iteration time makes it necessary.

### Phase 10 — Desktop platform (ongoing) · P2
- A service registry (name → IPC endpoint), replacing PID 4.
- A minimal capability/handle model for devices and services. Handles already exist as fds; extend
  that to the display, input and power.
- Terminal: pipes, redirection, a small script language (or port a small shell over musl).
- Settings persistence, a file manager on real mtimes, notifications.
- App packaging decoupled from the kernel image: apps on ext4 with a manifest; the kernel carries only
  `init` + recovery.

### Phase 11 — Research track (after Phase 7) · P3 — see §13

---

## 8. Milestones (each objectively verifiable)

| ID | Milestone | Verified by |
|---|---|---|
| M0.1 | CI runs all host tests on every PR | green `cargo test` job |
| M0.2 | CI boots Nyx in QEMU to the shell and `posixprobe` passes | serial-log assertions in CI |
| M0.3 | No GGTT page mapped twice | boot-time assertion; `gpu` shows no first-scene failure across 10 HW boots |
| M0.4 | APs run cached with WP=1 | `sched` shows CR0 per core; per-core context switches > 0 under load |
| M1.1 | Kernel survives the syscall fuzzer (kernel/NX/unmapped/non-canonical pointers to every syscall) | fuzzer run in the QEMU CI job |
| M1.2 | SMEP+SMAP enabled on all cores | CR4 dump; boot test green |
| M1.3 | A failing `execve` returns an error to an intact caller | posixprobe case |
| M2.1 | 1,000 fork+exit cycles leave free frames unchanged (±1%) | posixprobe case + frame counter syscall |
| M2.2 | A 2-thread munmap test on 2 cores faults correctly (no stale-TLB access) | posixprobe case |
| M3.1 | futex/wait4/pipe wake latency < 1 tick (was 10–20 ms) | schedstats histogram |
| M3.2 | The shell main loop has zero timeout polls when idle; idle CPU ≈ 0 | `sched` context-switch rate at idle |
| M3.3 | A spinning process can be killed | posixprobe case |
| M4.1 | `interrupts.rs` < 1,000 lines; one ABI source; musl builds without the `SYS_open` patch | tree + Build-musl.sh |
| M5.1 | Ctrl+C/Ctrl+V work in terminal and notepad; paste of a 120-char string | HW demo |
| M5.2 | Input-to-photon latency reported, p99 < 1 frame | `sched` |
| M6.1 | NVMe completes by interrupt; no IF-off stall > 1 ms during `execve` | schedstats long-IF-off record |
| M6.2 | Pulling power during a write loop leaves a mountable ext4 | HW test, `fsck` on Linux |
| M6.3 | Read-only file `mmap` works | posixprobe case |
| M7.1 | 200 open/close window cycles without corrupting GPU state | scripted test in shell |
| M7.2 | `sys_gpu_sync` blocks (no spin); GPU idle → core idle | schedstats |
| M7.3 | Page-flipped scanout, tear-free | HW video |
| M8.1 | A TCP server socket accepts a connection (loopback) | posixprobe case |
| M8.2 | TLS works with a wrong CMOS clock after SNTP | HW test |
| M9.1 | Ladybird LibCore event loop runs its own test suite on Nyx | test output |
| M9.2 | Ladybird renders a page into a Meridian window | HW screenshot |

---

## 9. Dependency Graph

```text
Ladybird in a window (M9.2)
  ├─ C window/input ABI ─── Input events (P5) ─── WaitQueues + fds (P3)
  ├─ LibCore loop (M9.1) ── epoll/timerfd/signals-on-any-return (P3) ── SMP-safe sched (P2.6)
  ├─ file mmap (P6.7) ───── page refcounts (P2.1) ── IRQ-safe MM lock (P2.3)
  ├─ threads at scale ───── TLB shootdown (P2.2)
  ├─ sockets server/UDP (P8.2) ── WaitQueues (P3)
  └─ survives buggy C++ ─── uaccess + SMAP (P1) ── CI fuzz in QEMU (P0.3)

GPU-first desktop
  Desktop apps ── window protocol w/ handles ── GPU object ownership (P7.2)
     └─ compositor w/ flip (P7.5) ── IRQ fences (P7.3) ── GT interrupt + WaitQueue (P3)
                                  └─ GGTT allocator (P7.1) ── no aliasing (P0.4)

Everything ── CI boot test (P0.3)
```

---

## 10. Performance Strategy

**Measure first.** The instrumentation already exists (`schedstats`, the long-IF-off recorder, the `gpu`
counters). Add before optimising:
1. per-syscall latency histograms (entry/exit TSC in the dispatcher wrapper, which already brackets
   `note_syscall_enter`);
2. input-to-photon (P5.4);
3. frame time and damage area per present in the shell;
4. heap allocation latency;
5. wake latency per `WaitReason`.

**Likely latency sources, in order of expected payoff**

| Source | Evidence | Remedy |
|---|---|---|
| APs possibly uncached | `trampoline.asm` CR0 | P0.5 — may dwarf everything else |
| Polling waits (10–20 ms floors) | futex, wait4, shell 2 ms loop | P3 WaitQueues |
| Polled NVMe with IF=0 (102 ms measured) | `nvme.rs:215` | P6.2 |
| GPU fence spin-waits burning cores | `render/mod.rs:140`, `interrupts.rs:3691` | P7.3 |
| Network polled from idle task / syscall loops | `process.rs:643` | P8.1 |
| Global locks: heap, VFS-across-I/O, MM | `allocator.rs`, `vfs.rs:408` | P2.3, P6.3; per-CPU heap caches only if measured |
| O(n) per-tick scheduler scans | `scheduler.rs` | timer heap (P3.3); fine at ≤192 tasks until measured |
| Full-screen BLT present / CPU copy of GL output | `render/gl.rs:385` | P7.5, P7.7 |
| `clflush` per ring dword | `intel/mod.rs:374` | P7.6 (measure) |

**Zero-copy opportunities that fit the existing design**
- Apps already render into SHM that the GPU samples directly; keep that as the single path.
- Let mini-GL render into the window GVA.
- Have the network task hand received packets straight into socket buffers.

Do **not** build lock-free structures until a histogram shows contention.

---

## 11. Security Strategy

**Needed soon (Phases 0–2)**
- `uaccess` + SMEP/SMAP.
- Syscall entry hygiene (DF/AC/IOPL/canonical RCX).
- ELF validation.
- W^X enforced (refuse W+X in the loader and `mprotect`).
- Kernel heap NX.
- AP CR0.WP.
- Ownership checks on power, GPU, input and SHM syscalls.
- No secrets in logs.
- No secrets in distributable images.

**Important eventually (Phase 10+)**
- A capability/handle model: extend fds to devices and services; the display owner holds the
  input+GPU capability.
- Per-app filesystem views instead of uid/gid, since Nyx is single-user.
- ASLR for user stacks/mmap/ELF base (needs PIE); stack protector for vendored C.
- A kernel CSPRNG (ChaCha20 DRBG seeded from RDSEED) behind `getrandom` and `AT_RANDOM`.
- An encrypted secret store (credentials, Wi-Fi PSKs) keyed from a user passphrase via the existing
  `libs/crypto` PBKDF2.
- IOMMU (VT-d) for DMA isolation.
- Secure Boot signing.

**Not planned:** a multi-user permission system or driver isolation in userspace. The cost is too
high for a single-user research OS; revisit only if the North Star changes.

---

## 12. Nyx Differentiation

Grounded in what the code already shows, not in buzzwords:

1. **An observable system (core identity).** Nyx already treats diagnostics as product:
   - per-core scheduler histograms and long-IF-off attribution;
   - GPU health, with hang markers and INSTDONE;
   - CMOS post-mortems that survive a hang;
   - "honest" typing (a simulator can never be reported as QPU hardware).

   Push this into an *architectural* principle: every subsystem exports typed counters and an event
   trace through one mechanism (a `diag` fd / trace ring). Every latency the user can feel has a
   number the system can show. This is useful, cheap, and rare in hobby and even mainstream OSes.
2. **A GPU-first desktop on an owned driver stack (core identity).** A from-scratch Gen9 pipeline
   composites the desktop and draws text, with a CPU fallback. The differentiator is a *short,
   auditable* path from app pixels to scanout (SHM → GGTT → one composite pass → flip), with no Mesa
   or DRM in between. Phase 7 makes it robust.
3. **Responsiveness as a measured property.** Input-to-photon and wake latency become
   release-gated numbers (M3.1, M5.2), not adjectives.
4. **Heterogeneous compute with honest device models (research).** The QPU work established a
   device-model discipline: typed status, refusal instead of silent fallback. That discipline
   generalises. See §13.

Rejected as identity: "AI-assisted OS" (no substrate and no data; it would be a chatbot bolted on),
distributed computing, and a novel filesystem.

---

## 13. Experimental / Research Track 🧪

These are clearly labelled research. None of them blocks the roadmap, and none should start before
Phase 7.

- 🧪 **A unified accelerator queue abstraction.** GPU RCS, BLT and QPU jobs share one shape: submit a
  command buffer → get a fence → wait or poll → collect results. A kernel `AccelQueue` trait (submit,
  fence fd, cancel, device info) with GPU and QPU-cloud implementations would let userspace wait on
  CPU, GPU and QPU work with one `epoll`. **Useful because** it is exactly what P7.3 needs anyway; the
  QPU is just a second, very slow backend. **Not useful** as a scheduler across devices; there is no
  workload that needs it.
- 🧪 **QPU as a device.** Keep it userspace (`libs/quantum-rt`) over HTTPS. The kernel registry stays
  empty until a real local device exists (none is sold as PCIe today). Worth doing:
  - a job-queue daemon that owns credentials, so apps never read keys;
  - result caching;
  - a fence fd for remote jobs.
- 🧪 **Execlists / HW contexts / PPGTT on Gen9.** Needed for real per-client GPU isolation and to
  stop losing state on RC6. Large, and hardware-specific. Pursue only if multiple 3D clients become a
  goal.
- 🧪 **Deterministic replay of the input/event stream.** Timestamped input events (P5.1) plus the
  trace ring could reproduce UI bugs in QEMU. That is valuable because the laptop has no serial port.
- 🧪 **Tickless idle and power-aware scheduling** (HWP hints already exist in `thermal.rs`).

---

## 14. What NOT To Build Yet

| Tempting | Why not yet | What must happen first |
|---|---|---|
| Ladybird port proper (beyond Gate 3 probes) | It will pass bad pointers, spawn threads, mmap files and block in event loops, and each of those currently corrupts or stalls the kernel | Phases 1, 2, 3, 6.7, 8.2 |
| New apps / more Meridian views | Every app inherits the polling, input and GVA-slot limits | Phases 3, 5, 7.1 |
| Dynamic linking / a package manager | Apps ship in the kernel image; there is no file mmap and no stable ABI | Phases 4.2, 6.6, 6.7 |
| Execlists, GuC, PPGTT, Gen11/12 support | The legacy ring works; ownership and fence IRQs come first | Phase 7 |
| IPv6, HTTP/2, WPA3 | Sockets are client-only and polled | Phase 8.1–8.2 |
| Audio | No driver framework (PCI registry, IRQ-driven DMA) to host it | Phase 6.1-style device model + P3 |
| Multi-user permissions, userspace drivers, IOMMU | High cost; the boundary itself is not enforced yet | Phases 1–2 |
| S3 suspend | Needs every driver to save and restore state; there is no driver model | driver registry |
| "AI in the OS", distributed features | No credible substrate; they would distract from foundations | — |
| Local QPU drivers | No such hardware exists (`quantum.rs:183`) | real hardware |
| A custom filesystem | ext4 via lwext4 works; durability is a configuration fix | — |

---

## 15. Long-Term Vision (North Star)

> **Nyx is a single-user, real-hardware workstation OS whose every layer — from the GPU ring to the
> window server — is small enough to read and instrumented enough to interrogate, and whose
> responsiveness is a measured, enforced property.**

This guides decisions as follows:
- **Prefer owning a short path to adopting a large stack:** own drivers, one compositor pass, smoltcp
  and rustls rather than a ported networking or graphics stack.
- **Prefer Linux-compatible ABI shapes** so that large software (musl, libc++, Ladybird) arrives by
  porting, not rewriting.
- **Every new subsystem ships with its counters** and a failure mode that reports rather than lies.
- **Correctness of the boundary comes before features.** A demo that can corrupt the kernel is not
  finished.

---

## 16. Immediate Next Steps (in order)

1. **P0.3 + P0.1 + P0.2: the CI safety net.** A headless QEMU boot smoke test with serial assertions
   (including a `posixprobe` pass), plus `cargo test` for the host crates and the dup-syscall check.
   *This comes first because every later step is an invasive refactor.*
2. **P0.4:** move the scanout GVA and add the GGTT overlap assertion. Then check the first-scene hang
   on HW (`gpu` over 10 boots).
3. **P0.5:** AP CR0 (CD/NW/WP), per-core IST stacks, NMI/#MC/spurious handlers. Compare `sched` before
   and after.
4. **P0.6:** remove the PSK log line; add a no-credentials default for images that leave the machine.
5. **P1.2 + P1.3 (SMEP part):** FMASK/`cld`/canonical-RCX/RFLAGS hygiene, and turn SMEP back on. These
   are small, and each closes a ring-0 compromise.
6. **P1.1:** `uaccess.rs`, converting syscalls subsystem by subsystem, gated by the Phase 1.7 fuzzer
   in CI.
7. **P2.1–2.3:** page refcounts, TLB shootdown, an IRQ-safe MM lock.
8. **P2.6 → P3:** SMP-safe remote wakes, then WaitQueues. The shell's 2 ms polling loop then goes away.

**The first task to implement after this audit:** the CI QEMU boot smoke test (P0.3), with P0.4 (the
GGTT alias) as the same-week quick win.
