# Nyx quantum subsystem — architecture

Nyx models three compute substrates. Two of them it already had:

```
CPU   general-purpose classical   scheduler.rs, PerCpu, SysMetrics.cpu_pct
GPU   parallel classical          drivers/gpu/intel, syscalls 501-538
QPU   quantum                     this subsystem
```

The point of this work is not to run quantum algorithms. It is to make the OS *understand that a
quantum processor is a kind of compute resource* — discoverable, describable, and honestly
reportable — in the same way it understands the other two. Everything else follows from that.

## What already existed

This is worth stating plainly, because it determined the whole shape of the design.

Nyx already had a complete quantum software stack before this subsystem was written:

- **`tools/compiler`** (`qclang_compiler`) — a real compiler: `.ql` → AST → semantic and ownership
  analysis → **QIR** → OpenQASM. About 8,900 lines.
- **`tools/compiler/src/program.rs`** — `flatten()`, which walks the QIR control-flow graph into the
  straight line of quantum operations a *circuit* is, plus `Flattened::columns` for diagram packing.
- **`tools/compiler/src/statevector.rs`** — an exact distribution evaluator, deliberately written
  with no dependencies and its own complex-number struct so it compiles on-device.
- **`apps/qcstudio`** — runs that entire pipeline on the machine and draws the circuit.
- **`libs/meridian/src/circuit.rs`** — host-tested circuit-diagram geometry: wires, gate boxes,
  CNOT connectors, measurement rows.

So the two things a project normally builds first — a circuit representation and a simulator —
already existed and worked. **This subsystem does not rebuild them.** `libs/quantum` owns an
OS-level circuit IR shaped as a lossless subset of `program::Step`, and `libs/quantum-rt` adapts
between the two. `tools/compiler` is not modified.

## The stack

```
 apps/terminal   apps/sysmon   apps/quantumdemo   apps/qcstudio
        └─────────────┴──────┬───────┴─────────────────┘
                             │
               libs/quantum-rt  (std, target_os=nyx)
               QpuSession · Registry · qclang adapter · credentials
                             │
        ┌────────────────────┼────────────────────┬──────────────┐
        ▼                    ▼                    ▼              ▼
  SimBackend           IonqProvider           HwBackend      (future)
  implemented          implemented            trait + docs
                             │
                       libs/json ─ libs/net (POST/headers/body, TLS 1.3)
                             │
                             ▼
               libs/quantum   (no_std + alloc, ZERO dependencies)
               QpuInfo · QpuStatus · Circuit · Gate · trait QpuBackend · sim
                             │
                             ▼  syscalls 573 / 574
               nyx-kernel/src/quantum.rs
               QPU_REGISTRY · PCI probe · introspection only
```

## Why the kernel part is so small

The kernel holds a descriptor table of physically present quantum devices, a PCI probe that fills
it, and two read-only syscalls. It holds **no gates, no circuits, no simulator, and no networking**.

Three reasons, in order of importance:

1. **There is no local hardware to arbitrate for.** Adding a job queue, a scheduler, or a
   submission path to the kernel for a device class that is not present on any machine this OS runs
   on would be the structural form of faking hardware — code whose only purpose is to look like
   support exists. See `limitations.md`.

2. **The kernel has no async model.** Every syscall in Nyx is synchronous; the GPU submits and
   blocks on a polled memory fence. The one non-blocking idiom in the tree is a *userspace* state
   machine — `libs/net`'s `Fetch`, whose `poll()` does work bounded by a 20 ms socket timeout and is
   pumped from an app's `update()`. A quantum job is exactly that shape, and much longer-running, so
   it belongs on the same side of the syscall boundary.

3. **Syscalls run with interrupts disabled.** `FMASK = IF`, so every syscall body executes at IF=0.
   A lock held by a kernel task at IF=1 and taken from a syscall at IF=0 is a hard deadlock that
   also kills the thermal governor. The established answer in this tree is *publish, don't expose*
   — `sys_get_metrics` reads only published atomics and takes no lock. The quantum syscalls do the
   same: copy from a static table, take no lock, allocate nothing.

Remote devices are **not** in the kernel registry at all. They are discovered over HTTPS by the
userspace runtime and merged into the device list there. The kernel never sees a token, a URL, or a
circuit.

## The four states

```rust
#[repr(u32)]
pub enum QpuStatus {
    NotPresent = 0,   // nothing, anywhere
    Simulator  = 1,   // classical software, running on this CPU
    Remote     = 2,   // reachable over the network, not here
    Hardware   = 3,   // a real QPU, physically attached to this machine
}
```

`Hardware` is set only by a kernel probe that matched a real PCI device. `SimBackend::info()`
returns `Simulator` unconditionally and there is no setter — the distinction is enforced by
construction rather than by discipline, because discipline is what fails at 2am.

`Remote` additionally carries `remote_is_simulator`, because a cloud simulator and a cloud QPU
arrive through the same API and "it came from the internet" is not a claim about physics. See
`device-model.md`.

## Design inheritance

Each piece of this subsystem copies something that already works in this tree, deliberately:

| Piece | Modelled on | Why |
|---|---|---|
| `QpuBackend` + `Registry` | `libs/toolchains::Toolchain` | the only plugin registry in the tree with tests; `with_defaults()` one-line registration |
| `QpuInfo` over the syscall boundary | `SysMetrics` / `sys_get_metrics` | `#[repr(C)]`, append-only, hand-mirrored, `Option<T>` return |
| `JobState` polling | `libs/net::Fetch` / `Progress` | bounded work per poll, stage announced before it executes |
| long provider calls | `apps/wifiagent` + `WifiOp` | the shell must never block; offload and poll a snapshot |
| refusing to report a number | `apps/sysmon::Metric::value() -> Option` | "0%" and "not measured" are different statements |
| ABI layout guard | `WindowHeader`'s `const _: () = assert!(size_of ...)` | a field addition cannot silently change the ABI |

## Where to read next

- `limitations.md` — **read this first** if you are considering hardware work
- `device-model.md` — `QpuInfo`, the status model, and why `remote_is_simulator` exists
- `circuit-ir.md` — the gate set and its relationship to qclang's QIR
- `simulator.md` — the state-vector backend and what it deliberately does not report
- `syscalls.md` — 573 / 574 and the rules their implementations follow
- `runtime.md` — `QpuSession`, backends, job lifecycle
- `remote.md` — the provider abstraction and the IonQ implementation
- `security.md` — an honest account, including what Nyx does not protect
- `future-hardware.md` — the prerequisites for `QpuStatus::Hardware` ever being reachable
