# The quantum runtime

`libs/quantum-rt` — where "I need quantum computation" becomes a concrete backend.

## The API

```rust
let mut dev  = QpuSession::open(QpuSelect::Best)?;
let job      = dev.submit(&Circuit::bell(), Shots(1024), seed)?;   // never blocks
loop {
    match dev.poll(&job) {
        JobState::Running(stage) => { /* name the stage on screen */ }
        JobState::Done(outcome)  => break outcome,
        JobState::Failed(e)      => return Err(e),
    }
}
```

An application is deliberately **not** told which backend served it. The only channel is
`Outcome::provenance`, and every display is required to show it.

`QpuSession::run()` exists for tests and non-interactive callers and is documented as blocking.
⚠️ Never call it from an app's `update()` or from `apps/shell` — the shell **is** the window server,
and a cloud job can sit in a provider's queue for minutes.

## Why `poll`-shaped

There is no async in the Nyx kernel. Every syscall is synchronous; the GPU submits and blocks on a
polled memory fence. The only non-blocking idiom in the tree is a userspace state machine —
`libs/net`'s `Fetch`, whose `poll()` does work bounded by a 20 ms socket timeout and is pumped from an
app's `update()` via `pump_load`.

Quantum jobs are that shape and more so. Following `Fetch` also means following its
**announce-before-executing** discipline: `Stage` is reported *before* the work it names happens,
because a label explaining a pause that arrives after the pause has explained nothing.

## Backend selection

`QpuSession::devices()` merges two sources into one list and renumbers ids densely:

1. **The kernel**, via syscall 573 — physically attached hardware. On this machine: none.
2. **Userspace backends**, in registration order — the simulator always, providers when configured.

Kernel devices come first so `QPU0` means the same thing across boots regardless of whether a provider
happened to be reachable.

On a non-Nyx target `kernel_devices()` returns empty. That is not a stub: it means **host tests
exercise the same "no hardware, fall back to the simulator" path the real machine takes.**

### `QpuSelect::Best` ranks by how little simulation is involved

```
Hardware (3) > Remote/hw (2) > Simulator (1) > Remote/sim (0)
```

⚠️ "Most real" is not "most capable". A 29-qubit cloud simulator beats a 2-qubit trapped-ion processor
on every measure except being a quantum computer, and this prefers the latter. A caller wanting the
biggest device must name it.

A remote simulator ranks **below** a local one: it is no more quantum, and it is slower and less
private.

### ★ Attached hardware without a driver refuses

`backend_for` has no `Hardware` arm. If the kernel ever reports an attached QPU, opening it fails with
*"attached but Nyx has no driver for it; running it on a simulator instead would misreport the
result"*. Silently substituting the simulator is the single worst thing this subsystem could do, so the
code path does not exist.

## `Outcome`, and the counts/probabilities distinction

⚠️ **The most important design detail in the runtime.**

The local simulator samples shots and reports **counts**: it really did draw 1024 outcomes. IonQ's API
returns a **histogram of probabilities** — `{"0": 0.497, "3": 0.503}` — and does not say how many shots
produced each bucket.

Multiplying `0.5 × 1024` into "512 shots" would invent a measurement nobody made. So:

```rust
pub enum Readout {
    Counts(Vec<(String, u64)>),
    Probabilities(Vec<(String, f64)>),
}
```

There is **no conversion** from `Probabilities` to `Counts`. `Outcome::counts_for()` returns `Option`
and is `None` for a probability readout — a caller wanting a number for display has to acknowledge
which kind it is getting. `Outcome::has_counts()` is the predicate the terminal uses to decide between
printing `512` and printing `0.497`.

The reverse direction is fine and provided: `probability_for()` works for both.

`ranked()` sorts by descending probability then by label — ties break by label so a table does not
reshuffle between renders.

## `Provenance`

```rust
pub struct Provenance {
    pub backend: String,             // "simulator", "ionq"
    pub device: String,              // "Nyx state-vector simulator", "qpu.aria-1"
    pub status: QpuStatus,
    pub remote_is_simulator: bool,
    pub is_quantum: bool,            // pre-computed, so a caller cannot forget the remote case
}
```

`disclosure()` returns the complete sentence printed under every result:

| status | disclosure |
|---|---|
| `Hardware` | executed on local quantum hardware |
| `Remote`, hw | executed on remote quantum hardware |
| `Remote`, sim | executed on a REMOTE CLASSICAL SIMULATOR; no quantum hardware was used |
| `Simulator` | executed on a classical simulator; no quantum hardware was used |

`is_quantum` is pre-computed precisely so a display cannot match on `status` and forget that
`Remote` might be a simulator.

## The registry

`nyx_quantum::Registry`, modelled on `libs/toolchains::Toolchain`/`Registry`/`Artifact` — the only
plugin registry in this tree that has tests, and whose design note says *"adding a backend is a
one-line change in `with_defaults` — the whole point of the design."*

Same shape, same trick: `QpuBackend` has variants of backend declared before implementations exist
(`Hardware`), just as `Artifact::Executable` is a reserved variant no backend produces yet. The trait
does not have to change when one lands.

`Registry::with_defaults()` registers the simulator only. Providers need credentials and network
discovery, so `libs/quantum-rt` registers those — `libs/quantum` has no networking by design.

## `validate` is where resource limits are enforced

`QpuBackend::validate`'s default implementation enforces everything `QpuInfo` declares: qubit count,
depth, shots, and gate set (naming the first unsupported gate rather than printing a bitmask). It runs
**before** anything is executed or transmitted.

This is the DoS boundary. A state-vector simulator allocates 2ⁿ complex amplitudes from an
attacker-influenced `n`; see `security.md`.

## Errors

`QpuError` variants each carry enough to print an actionable sentence. `"quantum error"` is not an error
message. `Cancelled`, `Auth`, `Provider`, `NoSuchDevice`, `NoSuchJob`, `TooManyQubits`, `TooDeep`,
`TooManyShots`, `UnsupportedGate`, `Unsupported`, `Circuit`.
