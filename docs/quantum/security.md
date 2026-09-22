# Security model

## The honest starting point

The brief asked this subsystem to "follow Nyx's existing security model". **There isn't one.**

- `struct Process` (`nyx-kernel/src/process.rs:450`) has **no uid, gid, euid, capability set, or
  privilege flag of any kind**. Grepping `capabilit|CAP_|euid|setuid|check_perm|is_privileged` across
  `interrupts.rs`, `process.rs` and `scheduler.rs` returns two hits, both comments.
- The syscall dispatcher branches on the syscall number alone. **Not a single arm consults the caller's
  identity.**
- The kernel says so itself, at `interrupts.rs:2141`: *"Nyx has no permission model, so this answers
  the only question it can answer truthfully: does the path exist?"* — `access(2)` returns `0` or
  `ENOENT`, never `EACCES`.

What already follows from that, before any quantum code existed: any process can map or destroy any
other process's window buffer (`sys_map_shm`/`sys_destroy_shm` take a bare id), power the machine off
(568), set the hardware clock (552), join a Wi-Fi network and learn the PSK path (545), or panic the
kernel on purpose (555).

⚠️ `Readme.md:55` claims Nyx "implements capability-based permissions" and "explicit user pointer
validation on every syscall boundary". Neither is accurate — there are no capabilities anywhere in the
tree, and syscall arms 510, 511, 541, 542 and 558 perform no pointer validation at all. This document
does not inherit that claim.

## So the defensible position is: add no attack surface

### The syscalls are read-only

573 and 574 are both pure introspection. No lock, no allocation, no device access, no user-controlled
indexing beyond a bounds-checked lookup into a fixed static array.

They validate with `is_valid_user_ptr` **and** `user_range_mapped`. ⚠️ The first is only a range check
against the userspace ceiling; a kernel-mode read of a valid-looking but unmapped address takes a page
fault in ring 0 and panics the **whole machine** rather than killing the process. 573 copies an array
that can span many pages, so per-page validation is mandatory, not defensive.

573 also clamps the entry count *before* multiplying by the struct size, so a huge `max` cannot
overflow the byte count.

### The probe never touches the device

`quantum::probe_pci` records identity a caller has already read out of config space. It maps no BAR,
enables no bus mastering, and writes no register.

⚠️ That matters more here than it would on Linux: **there is no IOMMU in this kernel.** A full-tree grep
for `iommu`, `DMAR`, `vt-d` returns nothing, and the DMAR ACPI table is never requested. Every device
that can master the bus can read and write all of physical memory. Enabling that for a card nobody has
identified would be indefensible.

### Circuit limits are enforced before execution

`QpuBackend::validate` checks qubit count, depth, shots and gate set **before** `begin` allocates or
transmits anything. `SimBackend::begin` re-validates rather than trusting that `run` did — `begin` is
public and reachable directly.

★ This is the DoS boundary that actually exists today. A state vector is `2ⁿ` complex amplitudes and `n`
comes from a circuit, which may arrive from a `.ql` file or a network peer. Without `MAX_QUBITS = 20`,
a 40-qubit circuit is a request for 17 TB. The kernel heap is 64 MB and userspace allocations come from
a bump allocator; the failure mode is `alloc_error_handler`, i.e. the process dies. Survivable, but it
should be a refusal with a reason instead — and it is.

`libs/json` has `MAX_DEPTH = 64` for the same class of reason: `[[[[…]]]]` is a few hundred bytes of
input and an unbounded amount of stack, and a remote party should not get to choose when the terminal's
stack runs out.

### No userspace path touches a hardware register

There is no `quantum` equivalent of `sys_gpu_map_shm` or `sys_gpu_copy_rect`. The kernel exposes
descriptions, not device access.

## Remote credentials

Covered in detail in `remote.md`. Summary:

- **HTTPS only** — `require_https` refuses rather than downgrading. A token is in a header on every
  request; over plaintext it is handed to anyone on the path.
- **The token is never printed in full** (`TokenStore::redact` shows four characters) and provider
  error **bodies are never echoed** — only the HTTP status — because a 4xx body can contain an echoed
  request carrying the token.
- **CR/LF in a token is refused at storage.** Otherwise `Request::header`'s injection guard drops the
  whole header later and the result is a 401 that looks like a wrong key.
- **Header injection is blocked in the transport.** `nyx_net::Request::header` drops any name or value
  containing CR or LF. Without it, a token or URL fragment carrying `\r\n` could append arbitrary
  headers or terminate the header block and forge a second request on the same connection.
- **POST is never replayed.** `Fetch` retries once on a reused keep-alive connection, which is safe
  because a GET is idempotent. `request_once` opens a fresh connection every time and never retries;
  `Request::is_idempotent` encodes the rule. A replayed job submission is a second job and, on metered
  hardware, a second charge.
- ⚠️ **The token file is readable by any process.** Nyx cannot prevent that, so
  `quantum remote login` says so at the moment of storage rather than burying it here.

## Malicious circuits

A circuit is data interpreted by a simulator or serialised to JSON. It cannot escape either. The
realistic harms are:

| harm | control |
|---|---|
| memory exhaustion via qubit count | `MAX_QUBITS`, enforced in `validate` before allocation |
| CPU exhaustion via depth/shots | `max_shots`; depth is linear in the state vector so it is bounded by the same qubit cap |
| a gate that means something else on a provider | unknown gates **refused by name**, never guessed (`ionq::gate_name`) |
| a wrong answer presented as right | `validate` rejects out-of-range qubits and duplicate operands (a CNOT whose control is its target is the identity, i.e. a *believable* wrong histogram) |

## Prerequisites for `QpuStatus::Hardware`

Recorded rather than pretended at. Before any local DMA-capable quantum accelerator is bound, Nyx needs:

1. **An IOMMU.** Otherwise the device can read and write all of physical memory. This is not a
   quantum-specific problem — it is true of every existing Nyx driver — but binding a *new* class of
   device is the wrong moment to keep ignoring it.
2. **A permission model.** Device access should not be available to every process by default. That is a
   kernel-wide change, not a quantum one.
3. **Per-process arbitration.** A shared QPU needs a queue with quotas, which is where the
   submit/wait/cancel syscalls would live. See `future-hardware.md`.

None of these exist. That is why the kernel's share of this subsystem is two read-only syscalls and a
table.
