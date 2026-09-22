# The circuit representation

`libs/quantum/src/circuit.rs`.

## Why there is a second circuit type in this tree

`tools/compiler` already has one: `qclang_compiler::program::Flattened`, a `Vec<Step>` produced by
walking the QIR control-flow graph. It is correct, it is tested, and `apps/qcstudio` draws from it on
real hardware.

It is not reused directly because it lives in a `std` compiler crate that also carries a lexer, a
parser, a semantic analyser and an ownership checker. The OS needs a circuit at a layer *below* the
compiler: the kernel's device model, a `no_std` app, and a provider serialising to a vendor's JSON all
need to talk about gates, and none of them should link a programming language to do it.

So `nyx_quantum::Circuit` is deliberately a **lossless subset** of `program::Step`. Every gate has an
exact counterpart with the same argument order, and `libs/quantum-rt/src/qclang.rs` is a match arm per
variant rather than a translation.

**`tools/compiler` is not modified.**

## The gate set

Fifteen gates: the nine the brief names (X, Y, Z, H, S, T, CX, SWAP, measurement) plus the adjoints
and rotations any real device or qclang program will actually produce.

```rust
pub enum Gate { H, X, Y, Z, S, Sdg, T, Tdg, Rx(f64), Ry(f64), Rz(f64), U3(f64,f64,f64), Cx, Swap, Ccx }
```

Growth rule: add a gate when a backend can execute it *and* a caller needs it. `Fredkin` (CSWAP) is
representable and deliberately absent — the adapter refuses it by name, which is the honest state for
"we could, and have not".

`Gate::qasm_name()` is canonical. `letter()` (the diagram label) and IonQ's serialiser both derive
from it rather than from a second match, so a gate the codegen can emit cannot be drawn or transmitted
as something else.

`Gate::bit()` positions are **stable and append-only** — they cross the syscall boundary inside
`QpuInfo::gate_set`, so reordering them would silently change which gates a device claims.

## Operations

```rust
pub enum Op {
    Gate { gate: Gate, qubits: [u8; 3] },   // only the first gate.arity() entries are meaningful
    Measure { qubit: u8, cbit: u8 },
    Reset { qubit: u8 },
}
```

There is deliberately **no separate arity field**. An earlier draft had one; it can disagree with the
gate, which is a class of bug with no upside, because the gate already knows.

`Op::gate()` returns `None` on an arity mismatch rather than padding or truncating. A `Cx` given one
qubit is a caller bug, and silently making it an `X` or a no-op would be worse than failing.

## Qubit indices are `u8`

255 qubits. The simulator caps at 20 (2²⁰ amplitudes is 16 MB), the largest device any provider
currently exposes is well under 100, and a `u8` keeps `Op` small and `Copy` so a circuit is a flat
`Vec` with no indirection.

## ⚠️ Bit order: wire 0 is leftmost

A basis-state label reads left to right as wire 0 first, so `"10"` means wire 0 measured 1 and wire 1
measured 0. This lines the label up with the circuit drawn above it, top wire at the top.

**This is the opposite of Qiskit's convention**, which prints the highest-index qubit first. Matching
the diagram beat matching another tool, because the two are side by side on screen and only one of
them can be right. Inherited verbatim from `qclang_compiler::statevector`, so the two agree.

`measured_qubits()` returns ascending **wire** order, not the order measurements appear in the
program, and deduplicates — measuring the same qubit twice yields the same bit and contributes one
column, not two.

## Validation catches the two plausible-wrong-answer bugs

`Circuit::validate()` is linear and every backend calls it before execution. It rejects:

- **an out-of-range qubit or classical bit**, naming the operation index;
- **a multi-qubit gate given the same qubit twice**. ★ A CNOT whose control is its target is the
  identity in most implementations, so it produces a *believable histogram* rather than an error. That
  is exactly the kind of bug that survives review.

`CircuitError` variants all carry the offending index, because "invalid circuit" is not something a
caller can act on.

## Depth

`Circuit::depth()` packs operations as early as possible: a gate shares a layer with another only if
they touch disjoint qubits. This is the number compared against a device's `max_depth` and printed
beside a circuit.

The Bell circuit's depth is **3**, not 4 — its two measurements are on different wires and therefore
simultaneous.

## The demonstration circuits

`Circuit::bell()` — `H(q0); CX(q0,q1); measure both`. Measuring must yield only `00` and `11` at
roughly equal rates and **never** `01` or `10` on a noiseless backend. That is what makes it a useful
end-to-end test: a wrong answer is obviously wrong.

`Circuit::ghz(n)` generalises it; `ghz(2) == bell()`, asserted.

## The qclang boundary

`libs/quantum-rt/src/qclang.rs`. Two things it must get right:

**Prepared states are real instructions.** `qubit a = |1>;` does not produce a gate step — `Flattened`
hoists it into `init: Vec<bool>` so a diagram can draw it as a wire label. An adapter that only walked
`steps` would produce a circuit for |0⟩ and a perfectly normalised distribution *of the wrong
program*. The adapter materialises `init` as leading `X` gates, and
`port_agrees_with_qclang_on_a_prepared_one_state` is the test that catches the alternative.

**⚠️ Rotations are refused, and that is not this adapter's limitation.** `QirGate::from_ast_gate`
matches `AstGate::RX(expr)` and constructs `QirGate::RX(0.0)`, *discarding the angle* — an upstream
placeholder, visible as a live `unused variable: expr` warning in `tools/compiler`. So `qrx(1.57) q;`
arrives as `RX(0.0)`, the identity. Converting it would produce a circuit that silently ignores a gate
and still returns a normalised, wrong answer.

`qclang_compiler::statevector` refuses these gates for the same reason. The adapter does too, by name.
`nyx_quantum`'s own simulator **does** implement rotations — its IR carries the angle. The problem is
purely the qclang → Nyx boundary, and `rotations_are_refused_while_their_angles_are_lost` is a canary:
when upstream starts carrying real angles that test fails, and the fix is to delete the refusal.
