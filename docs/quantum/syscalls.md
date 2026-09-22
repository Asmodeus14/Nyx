# Quantum syscalls

Two, both read-only. That is the entire kernel-facing surface of Nyx's QPU support.

| # | Name | Signature | Returns |
|---|---|---|---|
| 573 | `SYS_QUANTUM_ENUMERATE` | `(buf: *mut QpuInfo, max: usize)` | entries written, or `-EFAULT` |
| 574 | `SYS_QUANTUM_INFO` | `(id: u64, out: *mut QpuInfo)` | `0`, `-ENODEV`, or `-EFAULT` |

Userspace wrappers: `nyx_api::sys_quantum_enumerate(&mut [QpuInfo]) -> usize` and
`nyx_api::sys_quantum_info(id) -> Option<QpuInfo>`, in the `sys_get_metrics` idiom.

Before adding either, `tools/check_dup_syscall_arms.sh` was run. **501–572 were contiguous and full;
573 and 574 were the next free numbers.** `interrupts.rs` is `#![allow(warnings)]`, so
`unreachable_patterns` is suppressed and a duplicate arm shadows silently — this has bitten the project
twice. Run that script before touching the dispatcher.

Next free number after these: **575**.

## What is deliberately absent

No `submit`, no `wait`, no `cancel`, no job queue, no completion signalling.

1. **There is no local hardware to arbitrate for.** No consumer gate-model QPU exists that Nyx can
   drive over PCIe (`limitations.md`). A submission queue and a fairness policy for a device class
   absent from every machine this OS runs on would be code whose only function is to make support look
   like it exists. When a real device is bound, the queue goes in beside the driver that binds it —
   see `future-hardware.md`.

2. **The kernel has no async model.** Every syscall in Nyx is synchronous; the GPU submits and blocks
   on a polled memory fence. The one non-blocking idiom in the tree is a *userspace* state machine
   (`libs/net`'s `Fetch`), and a quantum job — which for a cloud provider sits in a queue for minutes
   — is exactly that shape.

3. **Remote devices are not in the kernel registry at all.** They are discovered over HTTPS by
   `libs/quantum-rt` and merged into the device list in userspace. No token, URL or circuit reaches
   ring 0.

## Implementation rules both arms follow

**No lock.** `FMASK = IF`, so every syscall body runs with interrupts disabled. A lock held by a kernel
task at IF=1 and taken from a syscall at IF=0 is a hard deadlock that also kills the thermal governor —
the hazard `drivers/net/mod.rs:61-105` documents and that `SysMetrics` was designed around.

The registry is a fixed `static` array written once during `pci::enumerate_pci()`, on the BSP, before
AP bring-up completes and long before userspace exists. `COUNT` is published with a `Release` store and
read with `Acquire`, so a reader that sees the count also sees the entries. Both arms copy from it and
take nothing.

**No allocation.** Same reason.

**Per-page pointer validation.** ⚠️ `is_valid_user_ptr` is only a *range* check — it asks whether an
address is below the userspace ceiling, not whether anything is mapped there. A kernel-mode read of a
valid-looking but unmapped user address takes a page fault in ring 0, which panics the **whole
machine** rather than killing the process.

573 copies an array that can span many pages, so this work added
`interrupts.rs::user_range_mapped(start, len)` — the loop `iov_array_ok` and `user_cstr_raw` already ran
inline, factored out. Both arms call `is_valid_user_ptr` **and** `user_range_mapped`.

**Overflow order.** 573 clamps the entry count to the registry size *before* multiplying by
`QPU_INFO_SIZE`, so a huge `max` cannot overflow the byte count.

## The ABI struct

`QpuInfo` is `#[repr(C)]`, 224 bytes, alignment 8, **append-only**. It is mirrored by hand in three
places, because the kernel cannot link a userspace crate:

- `libs/quantum/src/device.rs` — the real definition
- `libs/api/src/lib.rs` — the userspace mirror
- `nyx-kernel/src/quantum.rs` — the kernel mirror

All three carry `const _: () = assert!(size_of::<QpuInfo>() == 224)`. Adding a field breaks the build
in all three rather than silently reinterpreting every later field. This is the same arrangement as
`SysMetrics`, `WindowQuad` and `SystemInfo`; there is no bindgen across the ring boundary in this tree.

`libs/quantum-rt` converts between the `nyx_api` and `nyx_quantum` forms **field by field** rather than
by transmute. The layouts are identical and asserted to be, but a reinterpret would make that assertion
load-bearing for memory safety instead of merely for correctness.

## `-ENODEV`

Added to the errno block in `interrupts.rs` for 574. `-19`.

## A note on the enumeration result

`sys_quantum_enumerate` returning **0** is the normal, correct outcome on every machine Nyx currently
runs on. It is not an error and not a stub. A non-zero result on this laptop means the probe found an
unidentified accelerator, which it reports as `QPU_KIND_UNIDENTIFIED` with status
`QPU_STATUS_NOT_PRESENT` rather than guessing — see `device-model.md`.
