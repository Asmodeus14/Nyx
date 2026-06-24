# Nyx OS

**An experimental open-source operating system for the intersection of classical computing, quantum execution, and AI-driven system intelligence.**

[![Rust](https://img.shields.io/badge/Rust-nightly-000000?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Build Status](https://img.shields.io/github/actions/workflow/status/Asmodeus14/Nyx/build.yaml?branch=master&label=build&logo=github&color=green)](https://github.com/Asmodeus14/Nyx/actions/workflows/build.yaml)
[![Dev Containers](https://img.shields.io/badge/Dev%20Containers-supported-blue?logo=visualstudiocode)](https://github.com/Asmodeus14/Nyx/tree/master/.devcontainer)
[![Status: Pre-Alpha](https://img.shields.io/badge/Status-Pre--Alpha-red)](https://github.com/Asmodeus14/Nyx)
[![Stars](https://img.shields.io/github/stars/Asmodeus14/Nyx?style=social)](https://github.com/Asmodeus14/Nyx/stargazers)

---

## Table of Contents

- [Overview](#overview)
- [Design Philosophy](#design-philosophy)
- [Architecture](#architecture)
- [Kernel Capabilities](#kernel-capabilities)
- [Kernel ABI Reference](#kernel-abi-reference)
  - [System Call Interface](#system-call-interface)
  - [POSIX-Compatible Syscalls](#posix-compatible-syscalls)
  - [Nyx Native Syscalls (500–534)](#nyx-native-syscalls-500534)
  - [Interrupt Vector Table](#interrupt-vector-table)
  - [Calling Convention](#calling-convention)
- [QCLang — Quantum Programming Language](#qclang--quantum-programming-language)
- [Project Structure](#project-structure)
- [Current Status](#current-status)
- [Quick Start](#quick-start)
- [Contributing](#contributing)
- [License](#license)

---

## Overview

Nyx is a **bare-metal, monolithic operating system kernel** written in Rust, designed from the ground up as a unified execution platform for three classes of computation that have historically existed in isolation:

- **Classical computing** — conventional userspace processes running native ELF64 binaries
- **Quantum computing** — first-class quantum circuit execution via the integrated QCLang toolchain
- **AI-driven system intelligence** — an adaptive kernel entity that evolves its behavioral state based on hardware telemetry and user interaction

Nyx targets the `x86_64-unknown-none` bare-metal environment and boots via the `bootloader` crate ecosystem. It is currently in **Pre-Alpha** status and is under active development.

---

## Design Philosophy

Nyx is built on five core principles:

**Memory Safety First** — The entire kernel, userspace, and compiler toolchain are implemented in Rust. Unsafe code is isolated to hardware interface layers. The kernel enforces strict pointer validation on every userspace-supplied address before touching it.

**Quantum Execution as a Native Primitive** — Rather than treating quantum computing as an external library or simulator, Nyx integrates quantum execution directly into the OS architecture via QCLang. Quantum programs compile through a dedicated Quantum Intermediate Representation (QIR) pipeline and execute within the kernel's execution model.

**Security-First Architecture** — Nyx implements capability-based permissions, Ring 0 / Ring 3 privilege separation enforced via GDT and TSS, post-quantum cryptography primitives (SHA3-256 via hardware `RDRAND`), and a per-process address space with explicit user pointer validation on every syscall boundary.

**Simulation-First Development** — All development targets QEMU first, enabling rapid iteration without physical hardware. Partial real hardware boot is functional and actively being extended.

**Minimal Energy Footprint** — The thermal governor daemon and HWP (Hardware-managed Power Performance) subsystem actively manage CPU frequency and cooling. The idle task uses `HLT` to yield hardware power states between scheduling quanta.

---

## Architecture

Nyx is a **monolithic kernel** with a clean module boundary enforced by Rust's visibility system. All kernel subsystems run in Ring 0 under a single address space. Userspace processes run in Ring 3 under isolated per-process page tables.

```
┌─────────────────────────────────────────────────────────────────┐
│                        USERSPACE (Ring 3)                       │
│   ELF64 Processes  │  QCLang Apps  │  GUI Applications          │
├─────────────────────────────────────────────────────────────────┤
│                     SYSCALL BOUNDARY (LSTAR MSR)                │
├─────────────────────────────────────────────────────────────────┤
│                       NYX KERNEL (Ring 0)                       │
│                                                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐     │
│  │ Scheduler│  │  Memory  │  │   VFS /  │  │   Network    │     │
│  │  (SMP)   │  │  Manager │  │  ext4    │  │  (smoltcp)   │     │
│  └──────────┘  └──────────┘  └──────────┘  └──────────────┘     │
│                                                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐     │
│  │   ACPI / │  │   GUI /  │  │ Thermal  │  │    Entity    │     │
│  │   APIC   │  │Compositor│  │ Governor │  │  (AI State)  │     │
│  └──────────┘  └──────────┘  └──────────┘  └──────────────┘     │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                    HARDWARE DRIVERS                     │    │
│  │  NVMe  │  AHCI  │  RTL8168  │  Intel GPU  │  xHCI USB   │    │
│  │  Intel WiFi  │  PS/2  │  APIC Timer  │  I2C/SMBus       │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘

Target: x86_64-unknown-none
Boot:   bootloader_api (UEFI framebuffer handoff)
Build:  Rust nightly + LLD linker + ACPICA C bridge (bindgen)
```

---

## Kernel Capabilities

### Memory Management

- **Physical Frame Allocator** — `BootInfoFrameAllocator` walks boot-provided memory region descriptors to track available physical frames.
- **4-Level Page Table Management** — Full PML4/PDPT/PD/PT manipulation via the `x86_64` crate. Supports dynamic allocation of user pages at arbitrary virtual addresses.
- **Per-Process Address Space Isolation** — Each process receives its own CR3. `clone_user_address_space()` performs full PML4 duplication on `fork()`. `clear_user_address_space()` reclaims all user-mode pages on process exit.
- **Kernel Heap** — Linked-list allocator backed by a fixed heap region initialized during boot.
- **Memory-Mapped I/O** — `map_user_mmio()` maps physical MMIO regions into userspace VA for GPU and device access.
- **Shared Memory** — Kernel-managed SHM registry supports `create_shm_block()` and `map_shm_block()` for zero-copy IPC between processes.
- **mmap-style Allocation** — Per-process `mmap_bump` pointer supports demand-paged anonymous memory allocation from userspace.
- **Physical ↔ Virtual Translation** — `virt_to_phys()` and `phys_to_virt()` are available kernel-wide for DMA and framebuffer setup.
- **User Pointer Validation** — `is_valid_user_ptr(ptr, len)` enforces that all userspace-supplied pointers reference valid, mapped, Ring 3 accessible memory before any kernel operation proceeds.

### Process and Task Management

- **ELF64 Loader** — `load_elf()` parses and maps PT_LOAD segments from standard ELF64 binaries into the current process's address space.
- **Ring 3 Entry** — `enter_userspace(entry, stack)` transitions from Ring 0 to Ring 3 via `iretq` with a properly constructed interrupt stack frame.
- **Process Creation** — `Process::new()` allocates a fresh kernel stack, per-process page table, and assigns a unique PID via atomic counter.
- **Thread Spawning** — `Process::new_thread(parent_cr3)` creates a thread sharing the parent's address space.
- **fork()** — Full address space duplication via `clone_user_address_space()`.
- **Cooperative and Preemptive Scheduling** — The scheduler (`scheduler.rs`) supports both timer-preemptive context switching (via APIC timer vector `0x40`) and voluntary yielding (via `int 0x41`). Context save/restore preserves all general-purpose registers plus FXSAVE state for SSE/FP continuity.
- **Task States** — `Running`, `Ready`, `Blocked` (with `wake_tsc` deadline for timed wakeup).
- **SMP Support** — `smp.rs` boots Application Processors (APs) via INIT/SIPI sequence. Each core receives its own `PerCpu` structure with an independent scheduler instance. `ACTIVE_CORES` tracks live cores atomically.
- **Per-CPU Storage** — `percpu.rs` maintains per-core kernel stack pointer, user stack pointer (`user_rsp` for `swapgs`), and the per-core scheduler.
- **IPC via Mailbox** — Each process has a `VecDeque<IpcMessage>` mailbox. Kernel-mediated `sys_ipc_send` and `sys_ipc_recv` support both blocking and non-blocking message passing across process boundaries, including cross-core wakeup.

### Virtual Filesystem (VFS)

- **Mount-Point Architecture** — `VirtualFileSystem` maintains a dynamic mount table. Arbitrary `FileSystem` trait implementations can be mounted at any path prefix.
- **NVMe-backed ext4** — `NvmeLwExt4Fs` bridges the lwext4 C library to the Rust VFS layer via `ext4_wrapper.c`, providing full read/write ext4 filesystem access on NVMe hardware.
- **Write-Ahead Log** — `WriteAheadLog` provides journaling semantics for filesystem mutations.
- **File Descriptor Table** — Per-process FD tables support `File`, `Socket` (TCP and UDP), `PipeRead`, and `PipeWrite` descriptor types.
- **Initrd via TARFS** — `tarfs.rs` mounts a baked-in TAR archive (`initrd.tar`, embedded at compile time via `include_bytes!`) to bootstrap the userspace environment before NVMe is available.
- **VFS Operations** — `mount`, `unmount`, `read_file_alloc`, `list_dir`, `open_path`, `create_dir`, `create_file`, `write_file`, `delete_file`.

### Interrupt and Exception Handling

- **IDT** — Full 256-entry Interrupt Descriptor Table managed by the `x86_64` crate.
- **Exception Handlers** — Breakpoint, Double Fault (on dedicated IST stack), General Protection Fault, Page Fault.
- **PIC 8259** — Chained PIC initialization with explicit mask management. Legacy PIC is used during early boot before APIC takeover.
- **APIC** — Local APIC initialization and timer setup (`init_timer()`). `end_of_interrupt()` sends EOI to the local APIC after each interrupt.
- **I/O APIC** — `ioapic.rs` provides IRQ routing (`route_irq(irq, apic_id, vector)`) to direct hardware interrupts to specific CPU cores.
- **SWAPGS Protocol** — All interrupt and syscall entry points execute `swapgs` when transitioning from Ring 3 to Ring 0, allowing safe access to per-CPU kernel data structures.
- **Context Switching** — Timer, keyboard, mouse, and yield interrupts all call dedicated context switch functions that save/restore full register state including FXSAVE.

### Network Stack

- **RTL8168 Ethernet Driver** — PCIe Gigabit Ethernet driver with MSI interrupt support, DMA ring buffer management, and interrupt-driven packet processing.
- **Intel WiFi (iwlwifi)** — Prototype driver for Intel wireless adapters.
- **TCP/IP Stack** — Powered by `smoltcp 0.9` with IPv4, DHCPv4, TCP, UDP, ICMP, and DNS resolution.
- **DHCP** — Dynamic IP address acquisition on any Ethernet network.
- **DNS** — Asynchronous DNS resolution via `sys_dns_resolve` (syscall 534), defaulting to Google DNS (8.8.8.8) until DHCP provides nameserver configuration.
- **Socket API** — Kernel-managed TCP and UDP sockets with `sys_socket`, `sys_connect`, `sendto`/`recvfrom` mapped to standard Linux syscall numbers (41, 42, 44, 45).

### Graphics Subsystem

- **Framebuffer** — UEFI linear framebuffer mapped and managed by `VgaPainter`. Supports software rendering to the raw pixel buffer.
- **Double Buffering** — `BackBuffer` provides an off-screen back buffer with a `present()` blit to the screen painter.
- **Intel GPU Driver** — Hardware-accelerated 2D rendering via Intel integrated GPU MMIO interface. Supports `fill_rect()` (BLT engine fill), `copy_rect()` (BLT engine copy), `wait_for_vsync()`, and `wait_for_idle()` via GPU command ring submission.
- **GPU Fallback** — All GPU operations fall back gracefully to CPU-side software rendering if the Intel GPU driver is unavailable.
- **Window Manager** — `window.rs` maintains a list of application windows with position, size, and z-order. `WINDOW_MANAGER` is a global spinlock-protected instance.
- **Mouse Input** — PS/2 mouse driver with atomic state (`MOUSE_STATE`) tracking X/Y position and button state. Cursor rendering is handled by the compositor.
- **Pixel Format** — BGRA 32-bit (4 bytes per pixel), stride-aware layout sourced from the UEFI framebuffer info.

### ACPI and Power Management

- **ACPICA Integration** — Full ACPICA library (Intel's reference ACPI implementation) is compiled as a C static library and linked into the kernel via `bindgen`-generated FFI bindings.
- **DSDT Parsing** — ACPI DSDT table is parsed for device configuration and thermal zone information.
- **Thermal Management** — `thermal.rs` reads Intel silicon temperature via `IA32_THERM_STATUS` MSR. Activates hardware P-state throttling via `IA32_HWP_REQUEST` when temperature exceeds threshold. Controls fan speed via ACPI EC writes and Dell SMBus protocol.
- **HWP (Hardware P-states)** — Enabled via `IA32_PM_ENABLE` MSR. Configures performance hints dynamically based on thermal load.
- **ACPI Power Off** — `acpi::poweroff()` triggers a clean S5 shutdown via ACPI.
- **WiFi Power** — `power_on_wifi_via_acpi()` uses ACPI namespace evaluation to power-cycle the WiFi controller.

### Storage Drivers

- **NVMe** — Full NVMe PCIe driver with admin queue initialization, IO queue creation, and LBA block read/write. Namespace discovery via Identify Namespace command. Supports NVMe version query.
- **AHCI** — SATA AHCI driver for HBA port enumeration, device type detection, and command execution via PRDT.

### USB

- **xHCI** — eXtensible Host Controller Interface driver with capability register parsing, operational register management, command ring, event ring, and doorbell support. Handles up to the controller-reported maximum port and slot count.

### AI Entity System

- **Genetic Seed** — A 32-byte SHA3-256 cryptographic identity derived from `RDRAND` hardware entropy on first boot and persisted to a hidden LBA sector on the NVMe disk. On subsequent boots, the seed is resurrected from storage, giving the entity a persistent cryptographic identity across power cycles.
- **NyxState** — Four floating-point behavioral dimensions updated in real time by kernel subsystems:
  - `energy` — driven by CPU scheduling activity and context switch frequency
  - `entropy` — driven by memory pressure and filesystem I/O volume
  - `stability` — driven by system uptime and absence of disruption
  - `curiosity` — driven by mouse and keyboard input events
- **Kernel Exposure** — Entity state is exposed to userspace via `sys_get_entity_seed` (syscall 520) and `sys_get_entity_state` (syscall 521), enabling userspace applications to observe and react to the kernel's behavioral state.

---

## Kernel ABI Reference

### System Call Interface

Nyx implements the standard x86_64 Linux `SYSCALL`/`SYSRET` ABI. The kernel installs a syscall handler by writing to the `LSTAR` MSR. The calling convention for all syscalls is:

| Register | Role |
|----------|------|
| `RAX` | Syscall number (input) / Return value (output) |
| `RDI` | Argument 1 |
| `RSI` | Argument 2 |
| `RDX` | Argument 3 |
| `R10` | Argument 4 |
| `R8` | Argument 5 |
| `R9` | Argument 6 |

On error, `RAX` is set to a negative errno value (`EINVAL`, `EFAULT`, `EBADF`, `ENOMEM`). On success, `RAX` holds the return value (0 or a positive integer).

`swapgs` is executed on entry and exit to allow access to the per-CPU kernel stack pointer stored in the `GS` base MSR.

---

### POSIX-Compatible Syscalls

These syscalls follow Linux x86_64 numbering and semantics, allowing standard ELF binaries to run without modification.

| Number | Name | Arguments | Description |
|--------|------|-----------|-------------|
| `0` | `sys_read` | `fd, buf*, len` | Read from file descriptor |
| `1` | `sys_write` | `fd, buf*, len` | Write to file descriptor |
| `2` | `sys_open` | `path*, flags, mode` | Open a file, returns fd |
| `3` | `sys_close` | `fd` | Close a file descriptor and release socket/pipe resources |
| `9` | `sys_mmap` | `addr, len, prot, flags, fd, off` | Map memory pages into the calling process's address space |
| `10` | `sys_mprotect` | `addr, len, prot` | Stub — returns 0 |
| `12` | `sys_brk` | `addr` | Stub — returns 0 |
| `13` | `sys_rt_sigaction` | `sig, act*, oact*` | Stub — returns 0 |
| `14` | `sys_rt_sigprocmask` | `how, set*, oset*` | Stub — returns 0 |
| `16` | `sys_ioctl` | `fd, cmd, arg` | Device-specific I/O control |
| `20` | `sys_writev` | `fd, iov*, iovcnt` | Gather-write from iovec array |
| `22` | `sys_pipe` | `fds*` | Create a unidirectional pipe; returns read/write fd pair |
| `33` | `sys_dup2` | `oldfd, newfd` | Duplicate file descriptor |
| `41` | `sys_socket` | `domain, type, protocol` | Create a network socket |
| `42` | `sys_connect` | `fd, addr*, addrlen` | Connect socket to remote address |
| `44` | `sys_sendto` | `fd, buf*, len, flags, addr*, addrlen` | Send data on a socket |
| `45` | `sys_recvfrom` | `fd, buf*, len, flags, addr*, addrlen` | Receive data from a socket |
| `57` | `sys_fork` | — | Fork the calling process; duplicates address space |
| `58` | `sys_spawn_thread` | `entry, arg` | Spawn a new thread sharing the caller's address space |
| `59` | `sys_execve` | `path*, argv*, envp*` | Replace process image with a new ELF binary |
| `60` | `sys_exit` | `status` | Terminate the calling process |
| `131` | `sys_sigaltstack` | `ss*, oss*` | Stub — returns 0 |
| `158` | `sys_arch_prctl` | `code, addr` | Set/get architecture-specific thread state (TLS via `ARCH_SET_FS`) |
| `218` | `sys_set_tid_address` | `tidptr*` | Stub — returns 1 |
| `318` | `sys_getrandom` | `buf*, buflen, flags` | Fill buffer with hardware random bytes via `RDRAND` |

---

### Nyx Native Syscalls (500–534)

These syscalls are unique to Nyx OS and provide access to the kernel's quantum, graphics, AI, and system telemetry subsystems. They begin at number 500 to avoid conflicts with the Linux syscall table.

#### Graphics & Display

| Number | Name | Arguments | Returns | Description |
|--------|------|-----------|---------|-------------|
| `501` | `sys_draw_rect` | `x, y, w, h, color_idx` | — | Draw a filled rectangle. If Intel GPU is available, uses the BLT hardware engine. Falls back to CPU-side pixel fill. Color index maps: 0=Black, 1=Blue, 2=Green, 3=Cyan, 4=Red, 14=Yellow, default=White. |
| `502` | `sys_swap_buffers` | — | — | Blit the GPU back buffer (GGTT offset `0x900000`) to the visible framebuffer (GGTT offset `0x100000`) via the Intel BLT engine. |
| `503` | `sys_gpu_sync` | — | — | Block until the Intel GPU command ring is idle (`wait_for_idle()`). |
| `504` | `sys_get_uptime_ms` | — | `u64` | Return current system uptime in milliseconds derived from TSC. Immune to CPU frequency scaling. |
| `505` | `sys_get_mouse_state` | — | Packed `u64` | Returns mouse state packed as: `[x:16][y:16][lclick:1][rclick:1]` in bits 63–0. Read is interrupt-safe (disables IRQs around spinlock). |
| `506` | `sys_get_key` | — | `u64` | Pop one keypress from the kernel key queue. Returns 0 if the queue is empty. |
| `507` | `sys_get_screen_info` | `width*, height*, stride*` | `1` on success | Write screen width, height, and stride (in pixels) into caller-supplied pointers. |
| `508` | `sys_map_framebuffer` | — | `u64` | Map the physical framebuffer into the calling process's virtual address space. Returns the user-space virtual address, or 0 on failure. |
| `513` | `sys_wait_vsync` | — | `0` | Block until the Intel GPU signals vertical sync. Provides tear-free rendering synchronization. |

#### VFS and Filesystem

| Number | Name | Arguments | Returns | Description |
|--------|------|-----------|---------|-------------|
| `510` | `sys_list_dir_count` | `path*, path_len` | `u64` | Return the number of entries in the directory at the given VFS path. |
| `511` | `sys_list_dir_entry` | `index, buf*, path*, path_len` | `u64` | Copy the name of directory entry at `index` into `buf`. Returns the number of bytes written. |

#### System Information and Telemetry

| Number | Name | Arguments | Returns | Description |
|--------|------|-----------|---------|-------------|
| `517` | `sys_get_hw_discovery` | `buf*, len` | `u64` | Write a hardware discovery report containing MCFG and MADT physical addresses into the caller's buffer. |
| `518` | `sys_get_boot_log` | `buf*, len` | `u64` | Copy the kernel serial boot log (up to 8 KB) into the caller's buffer. |
| `519` | `sys_alloc_pages` | `num_pages` | `u64` | Allocate `num_pages` anonymous pages from the process's `mmap_bump` allocator. Returns the virtual base address, or 0 on failure. |
| `522` | `sys_get_active_cores` | — | `u64` | Return the number of CPU cores currently online and running in the SMP pool. |
| `523` | `sys_get_context_switches` | — | `u64` | Return the global context switch counter since boot. |
| `524` | `sys_get_system_info` | `SystemInfo*` | `0` | Populate a `SystemInfo` struct with: CPU temperature, active cooling state, CPU and GPU fan RPM, and up to 64 task descriptors (PID, name, CPU ticks, state). |
| `525` | `sys_sleep_ms` | `ms` | `0` | Sleep for `ms` milliseconds. Sets the calling task to `Blocked` state with a TSC-derived wake deadline and yields the CPU. Wakes on timer expiry or external interrupt. |
| `526` | `sys_get_dsdt_data` | `buf*, max_len` | `u64` | Copy the raw ACPI DSDT table bytes into the caller's buffer. Returns bytes copied. |

#### AI Entity and Identity

| Number | Name | Arguments | Returns | Description |
|--------|------|-----------|---------|-------------|
| `520` | `sys_get_entity_seed` | `buf*` | `1` on success | Copy the 32-byte cryptographic Genetic Seed into the caller's buffer. The seed is derived from hardware `RDRAND` entropy on first boot and persisted to NVMe across power cycles. |
| `521` | `sys_get_entity_state` | `f32[4]*` | `1` on success | Write four `f32` values into the caller's buffer: `[energy, entropy, stability, curiosity]`. These values reflect the kernel's real-time behavioral state, updated continuously by subsystem activity. |

#### Inter-Process Communication

| Number | Name | Arguments | Returns | Description |
|--------|------|-----------|---------|-------------|
| `530` | `sys_create_shm` | `size` | `shm_id` | Allocate a shared memory block of `size` bytes. Returns an integer SHM ID that other processes can use to map the same physical memory. |
| `531` | `sys_map_shm` | `shm_id` | `u64` | Map a previously created shared memory block into the calling process's address space. Returns the virtual address, aligned to a 2 MB boundary to prevent overlap with anonymous mmap. |
| `532` | `sys_ipc_send` | `target_pid, msg_type, data1, data2` | `1` on success | Deliver an `IpcMessage` to the mailbox of the process identified by `target_pid`. If the target is blocked waiting for IPC (`wake_tsc == u64::MAX`), it is immediately woken. Thread-safe across SMP cores. |
| `533` | `sys_ipc_recv` | `IpcMessage*, block` | `1` if message received | Receive one message from the calling process's mailbox. If `block == 1`, the task enters `Blocked` state and yields until a message arrives. If `block == 0`, returns 0 immediately if the mailbox is empty. |

#### Network

| Number | Name | Arguments | Returns | Description |
|--------|------|-----------|---------|-------------|
| `534` | `sys_dns_resolve` | `hostname*, hostname_len` | Packed IPv4 `u64` | Initiate a DNS A-record query for the given hostname. Returns the resolved IPv4 address packed into a `u64`, or 0 on failure or pending. |

---

### Interrupt Vector Table

| Vector | Source | Handler |
|--------|--------|---------|
| `0x03` | Breakpoint (`INT3`) | `breakpoint_handler` |
| `0x08` | Double Fault | `double_fault_handler` (dedicated IST stack) |
| `0x0D` | General Protection Fault | `gpf_handler` (prints IP and error code) |
| `0x0E` | Page Fault | `pf_handler` (prints faulting address) |
| `0x20` (32) | PIC Master — APIC Timer | `timer_interrupt_stub` → `timer_context_switch` |
| `0x21` (33) | PIC Master — PS/2 Keyboard | `keyboard_interrupt_stub` → `keyboard_handler_impl` |
| `0x2C` (44) | PIC Slave — PS/2 Mouse (IRQ 12) | `mouse_interrupt_stub` → `mouse_handler_impl` |
| `0x30` (48) | RTL8168 MSI Ethernet | `rtl8168_interrupt_handler` → `ethernet_handler_impl` |
| `0x40` (64) | APIC Timer (after APIC init) | `timer_interrupt_stub` → `timer_context_switch` |
| `0x41` (65) | Software Yield (`INT 0x41`) | `yield_interrupt_stub` → `yield_context_switch` |
| LSTAR MSR | `SYSCALL` instruction | `syscall_handler_asm` → `syscall_dispatcher` |

---

### Calling Convention

The x86_64 System V ABI is used for all Rust-to-Rust and Rust-to-C calls within the kernel. Exceptions:

- **Context switch stubs** — Assembly stubs use `extern "C"` to call into Rust context switch functions, passing the current RSP as the sole argument and receiving the next RSP as the return value.
- **ACPICA** — The ACPICA C library is compiled with `cc` and linked against `c_stubs.rs`, which provides `no_std` implementations of required C runtime functions (`memcpy`, `memset`, `strlen`, etc.).
- **x86-interrupt ABI** — Exception and IRQ handlers that are registered directly in the IDT use `extern "x86-interrupt"`, which the Rust compiler handles correctly for interrupt frame management.

---

### GDT Layout

The Global Descriptor Table is a 9-entry structure initialized by `init_hardened_gdt()`:

| Index | Segment | Privilege |
|-------|---------|-----------|
| 0 | Null | — |
| 1 | Kernel Code (`CS`) | Ring 0 |
| 2 | Kernel Data (`DS`, `ES`, `SS`) | Ring 0 |
| 3 | User Data | Ring 3 |
| 4 | User Code | Ring 3 |
| 5 | User Data (compat) | Ring 3 |
| 6 | User Code (compat) | Ring 3 |
| 7–8 | TSS (128-bit system descriptor) | Ring 0 |

The TSS provides two dedicated stacks: a Ring 0 privilege stack (32 KB) used on Ring 3 → Ring 0 transitions, and a Double Fault IST stack (20 KB).

---

### Target Configuration

```json
{
  "llvm-target": "x86_64-unknown-none",
  "arch": "x86_64",
  "target-endian": "little",
  "target-pointer-width": 64,
  "linker-flavor": "gnu-lld",
  "panic-strategy": "abort",
  "disable-redzone": true,
  "relocation-model": "static",
  "features": "+sse,+sse2,+sse3,+ssse3,+sse4.1,+sse4.2,+aes,+pclmulqdq,-avx,-avx2"
}
```

SSE through SSE4.2 and AES-NI are enabled. AVX/AVX2 are disabled to maintain compatibility with a broader range of x86_64 silicon.

---

## QCLang — Quantum Programming Language

QCLang is Nyx's native quantum programming language with a Rust-inspired syntax and a dedicated compiler toolchain.

### Pipeline

```
QCLang Source (.qcl)
        ↓
    Lexer (tokenize)
        ↓
    Parser → AST
        ↓
  Semantic Analyzer (Affine Type Checking)
        ↓
   QIR Builder → QirModule
        ↓
   QIR Optimizer (Dead Qubit Elimination, Gate Cancellation)
        ↓
   QIR Analyzer (Circuit metrics: depth, qubit count, T-count)
        ↓
  Code Generator → OpenQASM 2.0
        ↓
   Quantum Simulator (optional execution)
```

### Language Features

- **Affine Type System** — Qubits are affine types. A qubit register cannot be used after measurement, enforced at compile time by the semantic analyzer.
- **`qreg` declarations** — Quantum registers initialized to a basis state: `qreg q[2] = |00>;`
- **Gate application** — Standard quantum gates: `H`, `CNOT`, `X`, `Y`, `Z`, `S`, `T`, `Rx`, `Ry`, `Rz`, `CZ`, `SWAP`, `Toffoli`
- **Measurement** — `let c: cbit = measure(q[0]);`
- **Quantum control flow** — `qfor` loops and `qif` conditional gate application
- **Classical integration** — Classical `let` bindings, arithmetic, and control flow interoperate with quantum operations

### Example: Bell State

```rust
fn main() -> int {
    // Allocate a 2-qubit register in |00⟩
    qreg q[2] = |00>;

    // Apply Hadamard to create superposition on q[0]
    H(q[0]);

    // Entangle q[0] and q[1]
    CNOT(q[0], q[1]);

    // Measure both qubits — results are correlated
    let r1: cbit = measure(q[0]);
    let r2: cbit = measure(q[1]);

    // r1 == r2 with 100% probability due to entanglement
    return 0;
}
```

Full language specification is documented in `SYNTAX.md`.

---

## Project Structure

```
Nyx/
├── .cargo/                     # Cargo configuration (linker, target)
├── .devcontainer/              # Dev Container configuration (DOCKERFILE + devcontainer.json)
├── .github/workflows/          # CI/CD pipelines (build, release)
│
├── nyx-kernel/                 # Rust monolithic kernel (Ring 0)
│   ├── src/
│   │   ├── main.rs             # Kernel entry point, boot sequence, PID 1 launch
│   │   ├── interrupts.rs       # IDT, exception handlers, syscall dispatcher (all 500+ syscalls)
│   │   ├── memory.rs           # Frame allocator, page tables, SHM, mmap
│   │   ├── scheduler.rs        # Task states, socket kinds, file descriptors, round-robin scheduler
│   │   ├── process.rs          # ELF loader, process/thread creation, fork, IPC message
│   │   ├── vfs.rs              # VFS mount table, FileSystem trait, VirtualFileSystem
│   │   ├── fs.rs               # NvmeLwExt4Fs — ext4 driver via lwext4 C bridge
│   │   ├── pci.rs              # PCI enumeration (MCFG/legacy), BAR access
│   │   ├── apic.rs             # Local APIC init, timer, SIPI
│   │   ├── ioapic.rs           # I/O APIC IRQ routing
│   │   ├── acpi.rs             # ACPICA bridge, thermal zones, WiFi power, DSDT export
│   │   ├── smp.rs              # AP bootstrap, ACTIVE_CORES atomic
│   │   ├── percpu.rs           # Per-CPU struct, GS base, kernel/user RSP
│   │   ├── gdt.rs              # GDT, TSS, segment selectors
│   │   ├── gui.rs              # VgaPainter, BackBuffer, framebuffer management
│   │   ├── window.rs           # Window manager, z-order, WINDOW_MANAGER global
│   │   ├── mouse.rs            # PS/2 mouse driver, MOUSE_STATE global
│   │   ├── shell.rs            # Kernel key queue for userspace polling
│   │   ├── usb.rs              # xHCI host controller driver
│   │   ├── drm.rs              # DRM/KMS interface stubs (Nouveau handshake WIP)
│   │   ├── thermal.rs          # Intel silicon temperature, HWP, fan control
│   │   ├── laptop_fans.rs      # Dell SMBus fan RPM telemetry
│   │   ├── time.rs             # TSC calibration, UPTIME_MS atomic
│   │   ├── tarfs.rs            # Initrd TAR filesystem parser
│   │   ├── allocator.rs        # Kernel heap initialization
│   │   ├── serial.rs           # Serial port logging, BOOT_LOG buffer
│   │   ├── vga_log.rs          # Framebuffer text logging (vga_println! macro)
│   │   ├── executor.rs         # Async task executor (futures support)
│   │   ├── task.rs             # Async task waker
│   │   ├── c_stubs.rs          # no_std C runtime stubs for ACPICA FFI
│   │   ├── partitioner.rs      # GPT partition table parser
│   │   ├── installer.rs        # Initrd TAR extraction to ext4
│   │   ├── entity/
│   │   │   ├── seed.rs         # Genetic Seed — persistent cryptographic identity
│   │   │   └── state.rs        # NyxState — real-time AI behavioral dimensions
│   │   └── drivers/
│   │       ├── nvme.rs         # NVMe PCIe driver (admin + IO queues)
│   │       ├── ahci.rs         # SATA AHCI driver
│   │       ├── net/
│   │       │   ├── rtl8168.rs  # Realtek RTL8168 GbE driver (MSI, DMA ring)
│   │       │   ├── iwlwifi.rs  # Intel WiFi prototype driver
│   │       │   └── mod.rs      # smoltcp network interface, DHCP, DNS
│   │       └── gpu/
│   │           └── intel.rs    # Intel integrated GPU (BLT engine, GGTT, vsync)
│   ├── acpica-core/            # ACPICA C source tree (Intel reference implementation)
│   ├── acpica-includes/        # ACPICA header files
│   ├── lwext4/                 # lwext4 C library (ext4 filesystem implementation)
│   ├── ext4_wrapper.c          # Rust ↔ lwext4 bridge
│   ├── custom_acpi.c           # Custom ACPICA OS layer implementation
│   ├── build.rs                # bindgen codegen for ACPICA and lwext4 FFI
│   └── Cargo.toml              # Kernel dependencies
│
├── tools/
│   └── compiler/               # QCLang compiler toolchain
│       └── src/
│           ├── lexer.rs        # Tokenizer
│           ├── parser.rs       # Recursive-descent parser → AST
│           ├── ast.rs          # Abstract Syntax Tree definitions
│           ├── semantics/      # Semantic analysis, affine type checking, symbol table
│           ├── qir/            # Quantum Intermediate Representation
│           │   ├── types.rs    # QubitId, CbitId, QirType, BitState
│           │   ├── operations.rs # QirGate, QirOp definitions
│           │   ├── builder.rs  # AST → QIR lowering
│           │   ├── optimizer.rs # Dead qubit elimination, gate cancellation
│           │   └── analysis.rs # Circuit depth, T-count, connectivity analysis
│           ├── codegen/
│           │   └── qasm.rs     # QIR → OpenQASM 2.0 code generator
│           ├── simulator.rs    # Software quantum state vector simulator
│           └── bin/qclang.rs   # CLI: compile, run, benchmark, info, update
│
├── apps/                       # Userspace applications (ELF64, Ring 3)
│   ├── compositor/             # Windowing compositor
│   ├── terminal/               # Terminal emulator
│   ├── explorer/               # File system explorer
│   ├── sysmon/                 # System monitor (CPU, memory, tasks)
│   ├── network/                # Network configuration manager
│   ├── settings/               # System settings application
│   └── init/                   # PID 1 init process
│
├── libs/                       # Shared userspace libraries
│   ├── gui/                    # GUI widget toolkit (canvas, draw, effects, UI layout)
│   └── api/                    # Nyx OS system call bindings
│
├── nyx-recv/                   # Debug utilities
│   ├── udp.py                  # UDP packet capture
│   ├── decode.py               # Packet decoder
│   └── dsdt.dsl                # Dumped ACPI DSDT (decompiled ASL source)
│
├── targets/
│   ├── x86_64-nyx.json         # Custom Rust target specification
│   └── linker.ld               # Kernel linker script
│
├── Build.sh                    # One-command full workspace build
├── Cargo.toml                  # Rust workspace manifest
├── rust-toolchain.toml         # Pinned nightly toolchain + llvm-tools-preview
├── SYNTAX.md                   # QCLang language specification
├── CLI.md                      # QCLang CLI reference
├── CHANGELOG.md                # QCLang version history
├── CONTRIBUTING.md             # Contribution guidelines
└── PHASES.TXT                  # Detailed development roadmap (Phases 1–5)
```

---

## Current Status

**Pre-Alpha — as of March 2026**

| Component | Status | Notes |
|-----------|--------|-------|
| QCLang Compiler | ✅ Functional | v0.6.0 — full pipeline through OpenQASM output |
| QIR Optimizer | ✅ Functional | Dead qubit elimination, gate cancellation |
| Quantum Simulator | ✅ Functional | Software state vector simulator |
| Kernel Boot (QEMU) | ✅ Functional | Full boot sequence to Ring 3 userspace |
| Memory Management | ✅ Functional | 4-level paging, per-process isolation, SHM |
| VFS / ext4 | ✅ Functional | Read/write ext4 via NVMe on real hardware |
| NVMe Driver | ✅ Functional | Block read/write, IO queues |
| RTL8168 Ethernet | ✅ Functional | DHCP, TCP/UDP, DNS |
| Intel GPU (2D BLT) | ✅ Functional | fill_rect, copy_rect, vsync, double buffer |
| SMP (Multi-core) | ✅ Functional | AP bootstrap, per-CPU scheduler |
| Syscall ABI | ✅ Functional | 40+ POSIX + 30+ Nyx native syscalls |
| ACPI / Thermal | ✅ Functional | Temperature, HWP, fan control |
| xHCI USB | 🔧 Prototype | Controller init; device enumeration in progress |
| AI Entity System | ✅ Functional | Genetic seed persistence, real-time NyxState |
| Real Hardware Boot | 🔧 Partial | Boots on select x86_64 laptops |
| DRM / Nouveau | 🔧 Early | Handshake WIP for NVIDIA GPU support |
| Intel WiFi | 🔧 Prototype | Driver skeleton; association pending |
| AHCI / SATA | 🔧 Prototype | Port enumeration; R/W in progress |
| Userspace Apps | 🔧 Early stage | Compositor, terminal, sysmon, explorer |

---

## Quick Start

### Requirements

- Rust nightly toolchain (pinned via `rust-toolchain.toml`)
- QEMU (`qemu-system-x86_64`)
- `lld` linker
- `clang` and `bindgen` (for ACPICA and lwext4 C compilation)

### Build and Run

```bash
# Clone the repository
git clone https://github.com/Asmodeus14/Nyx.git
cd Nyx

# Build the entire workspace (kernel + userspace + compiler)
./Build.sh

# Run in QEMU
./runner/run-qemu.sh
```

For detailed CLI options, debug flags, and QEMU configuration see `CLI.md`.

### Dev Container

Nyx ships a fully configured Dev Container (`.devcontainer/`) that installs all build dependencies automatically. Open the repository in VS Code with the Dev Containers extension, or use GitHub Codespaces:

[![Open in GitHub Codespaces](https://github.com/codespaces/badge.svg)](https://github.com/codespaces/new?hide_repo_select=true&ref=master&repo=Asmodeus14/Nyx)

---

## Contributing

Nyx is looking for contributors with expertise in:

- Operating systems and kernel development
- Rust `no_std` programming
- Quantum programming languages and circuit optimization
- x86_64 hardware driver development
- Networking (TCP/IP stacks, device drivers)
- Compiler construction and type systems

**How to contribute:**

1. Read `CONTRIBUTING.md` and `Code_Of_Conduct.md`
2. Check open issues and the development roadmap in `PHASES.TXT`
3. Fork the repository and create a feature branch
4. Submit a pull request against `master`

Priority areas: ext4 hardening (see `PHASES.TXT` Phase 1), Ethernet stack robustness (Phase 2), USB HID device enumeration, and NVIDIA DRM handshake.

---

## License

Nyx OS is licensed under the **Apache License 2.0**.

See `License` and `NOTICE.md` for full terms.

The following third-party components are included under their respective licenses:
- **ACPICA** — Intel License (see `nyx-kernel/acpica-core/`)
- **lwext4** — BSD 2-Clause (see `nyx-kernel/lwext4/LICENSE`)
- **smoltcp** — MIT / Apache 2.0 dual license

---

> *Nyx — because every great OS needs a bit of mystery and entanglement.*
>
> **Let's build the future, one qubit at a time.**
