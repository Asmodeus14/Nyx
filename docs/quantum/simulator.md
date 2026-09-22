# The state-vector simulator

`libs/quantum/src/sim.rs` and `libs/quantum/src/sim_backend.rs`.

## Provenance: this is a port, not a new implementation

The arithmetic is `tools/compiler/src/statevector.rs`, which has been running on this machine inside
`apps/qcstudio` since that app was written. It was already dependency-free, already used its own
two-field complex struct rather than `num_complex`, and already used plain `f64` — deliberately, so
that it would compile into an on-device window. That made the port to `no_std` nearly mechanical.

## ★★ The cross-check

`libs/quantum-rt/src/qclang.rs` contains four tests that compile a QCLang program once and evaluate it
through **both** evaluators, requiring the distributions to agree to 1e-12:

```
port_agrees_with_qclang_on_a_bell_pair
port_agrees_with_qclang_on_a_prepared_one_state
port_agrees_with_qclang_on_phase_gates
port_agrees_with_qclang_on_a_three_qubit_ghz
```

The original is the one that has been producing histograms on real hardware, so **if they disagree,
the port is wrong**. This runs on every `cargo test`, on a machine with no QEMU, where the alternative
way to find a sign error in a gate matrix is a power cycle and a photograph.

The phase-gate case exists because S and T are invisible in the computational basis unless an
interference path exposes them — the outer Hadamard pair is what makes a conjugation error visible.

## Two deliberate differences from the original

**1. Rotations work here.** The original refuses `rx`/`ry`/`rz`/`u3` by name because qclang's IR
discards the angle (see `circuit-ir.md`). `nyx_quantum::Gate::Rx` carries its angle, so there is
nothing lost and no reason to refuse. That is also why rotations cannot be part of the cross-check —
the reference cannot evaluate them to compare against — so they have their own tests, including
`Rx(π) == X` and `Ry(π/2)` giving an even superposition. `Rx(0.0)` is tested too, but only as a
negative control: it would pass even if angles *were* being discarded, which is exactly why the π case
is the real test.

**2. Shots.** The original computes an exact marginal distribution, which is right for a diagram: a
thousand sampled runs only estimate a number that can be computed exactly. But a QPU backend must be
able to return what a *device* returns, which is a finite sample. So `sample()` draws from the exact
distribution rather than simulating shot by shot — identical statistics, and `O(shots · log outcomes)`
instead of `O(shots · 2ⁿ · gates)`.

## Deferred measurement, and why `Reset` is refused

Measurements are not applied as collapses. The circuit is evolved unitarily to the end and the result
is the **marginal** distribution over the measured qubits.

That is exact here rather than an approximation, and the reason is a property of this IR:
`Op::Gate` takes qubit indices only, so no measured bit can reach a gate argument. With no classical
feedback, the deferred-measurement principle says a mid-circuit measurement is indistinguishable from
the same measurement at the end.

`Op::Reset` breaks that — it collapses unconditionally. A circuit containing one is **refused with a
reason** rather than quietly given the wrong distribution. Same choice the original made.

## What it deliberately does not report

**Fidelity, and every other error metric.** There is no noise model here: no depolarising channel, no
gate-error rates, no decoherence, no readout error. An ideal state vector has fidelity 1 against itself
by construction, so any number other than `1.000` would have to be invented.

Consequently `SimBackend::info()` reports `coherence_t1_ns = 0`, `coherence_t2_ns = 0`,
`calibrated_unix = 0` — "not reported" — and `queue_depth = u32::MAX`, because work is done inline on
the calling thread and claiming an empty queue exists would be a different statement from "no queue
applies".

## MAX_QUBITS is a security limit

`MAX_QUBITS = 20`. 2²⁰ amplitudes is 16 MB, and no circuit that large has a readable picture.

⚠️ It is also the **only** bound on an attacker-controlled allocation. A state vector is `2ⁿ` complex
amplitudes and `n` comes from a circuit, which may arrive from a `.ql` file or a network peer.
`QpuBackend::validate` enforces it *before* `begin` allocates anything, and `SimBackend::begin`
re-validates rather than trusting that `run` did — `begin` is public and a caller can reach it
directly.

## Randomness comes from the caller

`begin(circuit, shots, seed)` takes the seed as an argument. `libs/quantum` has no opinion about where
entropy comes from: on Nyx that is syscall 318, on the host it is a test constant, and neither belongs
in a `no_std` library with zero dependencies.

The PRNG is SplitMix64 — five lines, not cryptographic, and not used for anything that needs to be.
The question it answers is "which of these outcomes came up", and the caller supplies the seed so every
test is reproducible.

## Trigonometry without a dependency

`core` has no `sin`/`cos` (they are `std`, backed by `libm`), and this crate has zero dependencies.
`libs/gui` pulls `libm` for exactly this reason, so taking that dependency would have followed tree
precedent.

It is not taken because the device model in this crate is mirrored on the kernel-adjacent side of the
syscall boundary, and forty lines of range-reduced Taylor series with known-answer tests is cheaper to
audit than a maths library. The implementation reduces to `[-π, π]`, folds into `[-π/2, π/2]`, and runs
Taylor to `x²¹/21!` — which is what `f64` needs at the top of that range, since the `x¹⁹` term alone is
still ~4e-14 at π/2.

Tested against nine known values, plus `sin²+cos² == 1` swept across ±20 radians so the range
reduction is exercised in both directions, plus oddness and evenness.

⚠️ Accuracy degrades for very large `|x|`, as it does for every implementation that reduces by
subtracting a multiple of 2π. Gate angles are radians near ±2π; this is not a general-purpose maths
routine.

## `SimBackend` cannot lie about what it is

`info()` hardcodes `QpuStatus::Simulator`. No setter, no constructor argument, no `#[cfg]`. The test
`the_simulator_can_never_claim_to_be_hardware` asserts it.

It implements the full `begin`/`poll` lifecycle even though it is synchronous — `begin` does all the
work and the first `poll` returns `Done`. That is deliberate: if the simulator were the only backend
with a blocking API, every caller would grow a special case, and the first time a cloud provider was
substituted the window would freeze. Uniformity here is what makes the backend genuinely swappable.
