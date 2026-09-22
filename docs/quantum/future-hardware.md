# Integrating real quantum hardware

Read `limitations.md` first. The short version: **no consumer gate-model QPU exists that Nyx can drive
over PCIe**, so `QpuStatus::Hardware` is defined, probed for, and unreachable on this machine by design.

This file is what to do when that changes.

## The one thing not to do

**Do not add a speculative entry to `KNOWN_QUANTUM_DEVICES`** in `nyx-kernel/src/quantum.rs`.

That table is the only code path in the entire tree that can produce `QPU_STATUS_HARDWARE`. A wrong
guess makes Nyx claim a quantum processor it does not have — the single most damaging thing anyone could
do to this subsystem. It is empty, and empty is correct.

Add an entry only after matching against real silicon.

## Prerequisites, in order

These are kernel-wide gaps, not quantum ones. A DMA-capable accelerator is simply the wrong device to
keep ignoring them for.

### 1. An IOMMU

There is none. A grep for `iommu`, `DMAR`, `vt-d` across the tree returns nothing, and the DMAR ACPI
table is never requested from ACPICA (only APIC/MADT and MCFG are). Every device that masters the bus
can read and write all of physical memory.

Nyx's existing drivers live with that. Binding a new class of device — especially one whose firmware
comes from a vendor — is the point at which it should stop being acceptable.

### 2. A permission model

`struct Process` has no uid and no capability set; no syscall arm consults caller identity. Today every
process could submit to a QPU, and on metered hardware that is a billing problem as well as a fairness
one. See `security.md`.

### 3. BAR sizing

`PciDriver::get_bar_address` does not size BARs — there is no write-all-ones/read-back. Every MMIO
window in the tree is a magic number at its call site (`0x1000000` for the GPU, `0x4000` for NVMe).
A device whose window you have to guess is a device you will eventually map wrong.

### 4. A DMA API

There is `memory::allocate_contiguous(frames, align, below_4gb)` and `virt_to_phys`, and the assumption
that bus address == physical address. No mapping abstraction, no bounce buffers, no cache maintenance
(only MMIO mappings get `NO_CACHE`; correctness relies on x86 coherent DMA).

## Where the code goes

Follow the Intel GPU's shape — it is the tree's only fully-worked accelerator:

1. `nyx-kernel/src/drivers/quantum/<vendor>/mod.rs`, added to `drivers/mod.rs`.
2. `pub struct FooDriver { mmio_base, device_id, pci_*, … }` + `unsafe impl Send/Sync`, with
   `probe(device_id) -> Generation`, `new(...)`, `initialize(&mut self)`.
3. `pub static FOO: Mutex<Option<FooDriver>> = Mutex::new(None);` — the house pattern.
4. Bind in **both** `pci.rs` enumeration paths (`scan_bus_range` ~`:507` and `enumerate_pci_legacy`
   ~`:318`). A device added to one works on one boot path only.
5. Reserve a GVA band in `render/mod.rs`'s constant table if you share the GGTT. The historical bug note
   there — a window backbuffer allocated at `GVA_RCS_RING`, so the engine fetched window pixels as
   commands and hung — is the failure mode.
6. A submission path with the wait-for-space → write → fence → doorbell shape, and a bounded spin that
   dumps HEAD/TAIL/FAULT on timeout.
7. An idle-park path. `GPU_PARK_IDLE_MS` exists because forcewake held forever kept the package out of
   RC6 and cooked the laptop.
8. A `try_lock()`-safe read path for anything the thermal governor or a UI polls — **publish, don't
   expose** (`WIFI_SNAPSHOT`, `SysMetrics`). Syscalls run at IF=0; kernel tasks hold spin locks at
   IF=1.

## Then the queue syscalls

`syscalls.md` explains why `submit`/`wait`/`cancel` do not exist: there is nothing to arbitrate for. Once
a real device is bound, they become necessary and go in beside the driver.

Suggested shape, deliberately not implemented:

```
575  sys_quantum_submit(device_id, circuit_ptr, circuit_len, shots) -> job_id | -errno
576  sys_quantum_poll(job_id, *mut JobStatus)                        -> 0 | -errno
577  sys_quantum_result(job_id, buf, len)                            -> bytes | -errno
578  sys_quantum_cancel(job_id)                                      -> 0 | -errno
```

Notes for whoever writes them:

- Run `tools/check_dup_syscall_arms.sh` first. 501–574 are taken; **575 is next free.**
- A circuit crossing the boundary needs a serialised form. `nyx_quantum::Circuit` is `Vec<Op>` where
  `Op` is `Copy` and pointer-free, so a flat array of `Op` plus a header is the obvious encoding — and
  it should get the same `#[repr(C)]`, size-asserted, three-way-mirrored treatment as `QpuInfo`.
- Per-process quotas belong in the submit arm, since that is the only place that knows who is asking.
- ⚠️ The queue itself must not be a lock a syscall takes at IF=0 while a kernel task holds it at IF=1.
  Either make it lock-free or keep the lock strictly interrupt-masked.

## Realistic near-term hardware

| candidate | what it actually is | what Nyx would need |
|---|---|---|
| **QRNG PCIe card** (IDQ Quantis) | real quantum hardware, plain PCI device, **not a processor** | a driver feeding `random.rs`. Gets `QuantumDeviceKind::QuantumRng` and must never light up the QPU path |
| **FPGA-based quantum controller** | the classical half of a QPU | vendor gateware documentation that does not exist publicly |
| **SpinQ desktop NMR** | 2–3 real qubits, room temperature | a documented register interface; it has none — it is a USB appliance with its own control OS |
| **NVQLink / DGX Quantum** | PCIe Gen 5 GPU↔controller interconnect | an HPC-scale stack. Not a hobby-OS target |

The QRNG is the only one reachable today, and it is deliberately **not** a QPU in this model — see
`device-model.md`.

## The invariant to preserve

Whatever gets built, this must stay true:

> `QpuStatus::Hardware` is produced only by a positive match against a verified device, and a device
> reported as `Hardware` that Nyx cannot actually drive **refuses** rather than falling back to a
> simulator.

`QpuSession::backend_for` has no `Hardware` arm for exactly this reason, and
`attached_hardware_without_a_driver_refuses_rather_than_simulating` is the test that keeps it that way.
