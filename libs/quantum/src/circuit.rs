//! The OS-level quantum circuit: a flat, ordered list of operations on numbered qubits.
//!
//! ## Why this exists when `qclang_compiler` already has one
//!
//! It would be reasonable to ask why Nyx does not simply use `qclang_compiler::program::Flattened`,
//! which is already in the tree, already correct, and already running on-device in `apps/qcstudio`.
//!
//! Because that type lives in a `std` compiler crate that also carries a lexer, a parser, a
//! semantic analyser and an ownership checker. The OS needs a circuit at a layer *below* the
//! compiler: the kernel's device model, a `no_std` app, and a network provider serialising to a
//! vendor's JSON all need to talk about gates, and none of them should have to link a programming
//! language to do it.
//!
//! So this is deliberately a **lossless subset** of `qclang_compiler::program::Step`. Every gate
//! here has an exact counterpart there, with the same argument order. `libs/quantum-rt`'s adapter is
//! a direct match arm per variant — not a translation — and the two gates qclang has that this does
//! not ([`QirGate::Fredkin`] and `Custom`) are rejected by name rather than silently dropped.
//!
//! `tools/compiler` is **not modified** by any of this.
//!
//! ## Argument order is load-bearing
//!
//! [`Op::Gate::qubits`] is in the gate's own argument order, so for [`Gate::Cx`] it is
//! `[control, target]` and the two are **not** interchangeable. This matches
//! `qclang_compiler::program::Step`'s documented convention exactly, because a circuit that means
//! something different after a round trip through the adapter is worse than one that fails to
//! convert.
//!
//! ## Qubit indices are `u8`
//!
//! 255 qubits. The simulator caps at 20 (2²⁰ amplitudes is 16 MB), the largest device any provider
//! currently exposes is well under 100, and a `u8` keeps [`Op`] small and `Copy` so a circuit is a
//! flat `Vec` with no indirection. [`Circuit::validate`] rejects anything out of range with a
//! reason.

use alloc::vec::Vec;
use core::fmt;

/// The gate set Nyx understands.
///
/// Fifteen gates: the nine named in the design brief (X, Y, Z, H, S, T, CX, SWAP, measurement) plus
/// the adjoints and rotations that any real device or any qclang program will actually produce.
/// Deliberately not hundreds — see the module docs of `docs/quantum/circuit-ir.md` for the growth
/// rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gate {
    /// Hadamard. Creates superposition; the first gate of a Bell pair.
    H,
    /// Pauli-X, the quantum NOT.
    X,
    /// Pauli-Y.
    Y,
    /// Pauli-Z.
    Z,
    /// Phase gate, √Z.
    S,
    /// Adjoint of [`Gate::S`].
    Sdg,
    /// π/8 gate, √S.
    T,
    /// Adjoint of [`Gate::T`].
    Tdg,
    /// Rotation about X by the given angle in radians.
    Rx(f64),
    /// Rotation about Y by the given angle in radians.
    Ry(f64),
    /// Rotation about Z by the given angle in radians.
    Rz(f64),
    /// The universal single-qubit gate, `U3(theta, phi, lambda)`.
    U3(f64, f64, f64),
    /// Controlled-NOT. Qubits are `[control, target]` — **not** interchangeable.
    Cx,
    /// Exchange two qubits.
    Swap,
    /// Toffoli / CCX. Qubits are `[control0, control1, target]`.
    Ccx,
}

impl Gate {
    /// How many qubits this gate acts on. Always 1, 2 or 3.
    pub const fn arity(self) -> usize {
        match self {
            Gate::H
            | Gate::X
            | Gate::Y
            | Gate::Z
            | Gate::S
            | Gate::Sdg
            | Gate::T
            | Gate::Tdg
            | Gate::Rx(_)
            | Gate::Ry(_)
            | Gate::Rz(_)
            | Gate::U3(..) => 1,
            Gate::Cx | Gate::Swap => 2,
            Gate::Ccx => 3,
        }
    }

    /// The OpenQASM name, lowercase.
    ///
    /// This is the canonical spelling: [`Gate::letter`] derives the diagram label from it, and the
    /// IonQ serialiser maps from it, so a gate the codegen can emit cannot be drawn or transmitted
    /// as something else. Same argument `qclang_compiler::program::Step::letter` makes.
    pub const fn qasm_name(self) -> &'static str {
        match self {
            Gate::H => "h",
            Gate::X => "x",
            Gate::Y => "y",
            Gate::Z => "z",
            Gate::S => "s",
            Gate::Sdg => "sdg",
            Gate::T => "t",
            Gate::Tdg => "tdg",
            Gate::Rx(_) => "rx",
            Gate::Ry(_) => "ry",
            Gate::Rz(_) => "rz",
            Gate::U3(..) => "u3",
            Gate::Cx => "cx",
            Gate::Swap => "swap",
            Gate::Ccx => "ccx",
        }
    }

    /// Bit position in [`crate::device::QpuInfo::gate_set`].
    ///
    /// Stable and append-only: these values cross the syscall boundary inside the gate-set bitmask,
    /// so reordering them would silently change which gates a device claims to support.
    pub const fn bit(self) -> u32 {
        match self {
            Gate::H => 0,
            Gate::X => 1,
            Gate::Y => 2,
            Gate::Z => 3,
            Gate::S => 4,
            Gate::Sdg => 5,
            Gate::T => 6,
            Gate::Tdg => 7,
            Gate::Rx(_) => 8,
            Gate::Ry(_) => 9,
            Gate::Rz(_) => 10,
            Gate::U3(..) => 11,
            Gate::Cx => 12,
            Gate::Swap => 13,
            Gate::Ccx => 14,
        }
    }

    /// Every gate, for building masks and for exhaustive tests. Angles are zero placeholders.
    pub const ALL: [Gate; 15] = [
        Gate::H,
        Gate::X,
        Gate::Y,
        Gate::Z,
        Gate::S,
        Gate::Sdg,
        Gate::T,
        Gate::Tdg,
        Gate::Rx(0.0),
        Gate::Ry(0.0),
        Gate::Rz(0.0),
        Gate::U3(0.0, 0.0, 0.0),
        Gate::Cx,
        Gate::Swap,
        Gate::Ccx,
    ];

    /// Whether this gate is in the Clifford group. Useful for a future stabiliser backend; free to
    /// record now, expensive to reconstruct later.
    pub const fn is_clifford(self) -> bool {
        matches!(
            self,
            Gate::H | Gate::X | Gate::Y | Gate::Z | Gate::S | Gate::Sdg | Gate::Cx | Gate::Swap
        )
    }

    /// The label drawn inside a gate box, at most two characters.
    ///
    /// Derived from [`Gate::qasm_name`] rather than a second match, for the reason given there. A
    /// 32px box holds two glyphs of 12px mono and no more — see `libs/meridian::circuit`.
    pub fn letter(self) -> &'static str {
        match self {
            Gate::H => "H",
            Gate::X => "X",
            Gate::Y => "Y",
            Gate::Z => "Z",
            Gate::S => "S",
            Gate::Sdg => "S\u{2020}",
            Gate::T => "T",
            Gate::Tdg => "T\u{2020}",
            Gate::Rx(_) => "RX",
            Gate::Ry(_) => "RY",
            Gate::Rz(_) => "RZ",
            Gate::U3(..) => "U3",
            Gate::Cx => "X",
            Gate::Swap => "\u{00d7}",
            Gate::Ccx => "X",
        }
    }
}

/// A bitmask over [`Gate::bit`].
pub fn gate_set_mask(gates: &[Gate]) -> u64 {
    let mut m = 0u64;
    for g in gates {
        m |= 1 << g.bit();
    }
    m
}

/// Whether `mask` claims support for `gate`.
pub fn gate_set_contains(mask: u64, gate: Gate) -> bool {
    mask & (1 << gate.bit()) != 0
}

/// One operation in a circuit.
///
/// `Copy` and pointer-free, so a circuit is a flat `Vec` and slicing it costs nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// A unitary. Only the first `gate.arity()` entries of `qubits` are meaningful; the rest are
    /// zero padding.
    ///
    /// ⚠️ There is deliberately no separate arity field. One existed in an earlier draft and it can
    /// disagree with the gate, which is a class of bug with no upside — the gate already knows.
    Gate { gate: Gate, qubits: [u8; 3] },
    /// Measure one qubit into one classical bit.
    Measure { qubit: u8, cbit: u8 },
    /// Force a qubit back to |0⟩.
    ///
    /// ⚠️ Unlike a measurement, this collapses **unconditionally**, which is why the simulator's
    /// deferred-measurement shortcut does not apply to a circuit containing one. See `sim.rs`.
    Reset { qubit: u8 },
}

impl Op {
    /// Build a gate operation from a slice, checking arity.
    ///
    /// Returns `None` on an arity mismatch rather than padding or truncating: a `Cx` given one
    /// qubit is a caller bug, and silently making it a no-op or an `X` would be worse than failing.
    pub fn gate(gate: Gate, qubits: &[u8]) -> Option<Op> {
        if qubits.len() != gate.arity() {
            return None;
        }
        let mut q = [0u8; 3];
        q[..qubits.len()].copy_from_slice(qubits);
        Some(Op::Gate { gate, qubits: q })
    }

    /// The qubits this operation touches, in argument order.
    pub fn qubits(&self) -> &[u8] {
        match self {
            Op::Gate { gate, qubits } => &qubits[..gate.arity()],
            Op::Measure { qubit, .. } => core::slice::from_ref(qubit),
            Op::Reset { qubit } => core::slice::from_ref(qubit),
        }
    }

    /// The label drawn in this operation's box.
    pub fn letter(&self) -> &'static str {
        match self {
            Op::Gate { gate, .. } => gate.letter(),
            Op::Measure { .. } => "M",
            Op::Reset { .. } => "|0",
        }
    }
}

/// Why a circuit was rejected.
///
/// Every variant names the offending index, because "invalid circuit" is not an error message a
/// caller can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitError {
    /// An operation names a qubit at or beyond [`Circuit::qubits`].
    QubitOutOfRange { op: usize, qubit: u8, declared: u8 },
    /// A measurement names a classical bit at or beyond [`Circuit::cbits`].
    CbitOutOfRange { op: usize, cbit: u8, declared: u8 },
    /// A multi-qubit gate was given the same qubit twice — a `Cx` whose control is its target.
    RepeatedQubit { op: usize, qubit: u8 },
    /// The circuit declares more qubits than this build can index.
    TooManyQubits { declared: usize },
}

impl fmt::Display for CircuitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CircuitError::QubitOutOfRange { op, qubit, declared } => write!(
                f,
                "operation {op} acts on qubit {qubit}, but the circuit declares only {declared}"
            ),
            CircuitError::CbitOutOfRange { op, cbit, declared } => write!(
                f,
                "operation {op} measures into classical bit {cbit}, but the circuit declares only {declared}"
            ),
            CircuitError::RepeatedQubit { op, qubit } => write!(
                f,
                "operation {op} names qubit {qubit} twice; a two-qubit gate needs two distinct qubits"
            ),
            CircuitError::TooManyQubits { declared } => {
                write!(f, "{declared} qubits declared; the maximum is 255")
            }
        }
    }
}

/// A quantum program: some qubits, some classical bits, and an ordered list of operations.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Circuit {
    pub qubits: u8,
    pub cbits: u8,
    pub ops: Vec<Op>,
}

impl Circuit {
    /// An empty circuit with the given width.
    pub fn new(qubits: u8, cbits: u8) -> Circuit {
        Circuit { qubits, cbits, ops: Vec::new() }
    }

    /// Append a gate, panicking on arity mismatch.
    ///
    /// For the builder style; [`Circuit::try_gate`] is the checked form. Panicking is right here
    /// because the arity of a gate is a compile-time-known property of the call site, not runtime
    /// data — `c.gate(Gate::Cx, &[0])` is a typo, not a condition to handle.
    pub fn gate(&mut self, gate: Gate, qubits: &[u8]) -> &mut Self {
        let op = Op::gate(gate, qubits)
            .expect("gate arity mismatch: check the qubit count at this call site");
        self.ops.push(op);
        self
    }

    /// Append a gate, returning `false` on arity mismatch.
    pub fn try_gate(&mut self, gate: Gate, qubits: &[u8]) -> bool {
        match Op::gate(gate, qubits) {
            Some(op) => {
                self.ops.push(op);
                true
            }
            None => false,
        }
    }

    /// Append a measurement.
    pub fn measure(&mut self, qubit: u8, cbit: u8) -> &mut Self {
        self.ops.push(Op::Measure { qubit, cbit });
        self
    }

    /// Measure every qubit into the classical bit of the same index.
    ///
    /// Widens `cbits` if needed, so `Circuit::new(2, 0).measure_all()` does the obvious thing
    /// rather than producing a circuit that fails validation.
    pub fn measure_all(&mut self) -> &mut Self {
        if self.cbits < self.qubits {
            self.cbits = self.qubits;
        }
        for q in 0..self.qubits {
            self.ops.push(Op::Measure { qubit: q, cbit: q });
        }
        self
    }

    /// Append a reset.
    pub fn reset(&mut self, qubit: u8) -> &mut Self {
        self.ops.push(Op::Reset { qubit });
        self
    }

    /// Check that every operation is in range and well-formed.
    ///
    /// Called by every backend before execution. Cheap, linear, and it catches the two mistakes
    /// that otherwise produce a plausible wrong answer instead of an error: an out-of-range qubit,
    /// and a two-qubit gate whose operands are the same qubit.
    pub fn validate(&self) -> Result<(), CircuitError> {
        for (i, op) in self.ops.iter().enumerate() {
            let qs = op.qubits();
            for (j, &q) in qs.iter().enumerate() {
                if q >= self.qubits {
                    return Err(CircuitError::QubitOutOfRange {
                        op: i,
                        qubit: q,
                        declared: self.qubits,
                    });
                }
                // A CNOT whose control is its target is not a gate. It is silently the identity in
                // most implementations, which makes it a bug that produces a believable histogram.
                if qs[..j].contains(&q) {
                    return Err(CircuitError::RepeatedQubit { op: i, qubit: q });
                }
            }
            if let Op::Measure { cbit, .. } = op {
                if *cbit >= self.cbits {
                    return Err(CircuitError::CbitOutOfRange {
                        op: i,
                        cbit: *cbit,
                        declared: self.cbits,
                    });
                }
            }
        }
        Ok(())
    }

    /// Circuit depth: the number of layers when operations are packed as early as possible.
    ///
    /// A gate may share a layer with another only if they touch disjoint qubits. This is the number
    /// a device's `max_depth` limit is compared against, and the one printed beside a circuit.
    pub fn depth(&self) -> usize {
        if self.qubits == 0 {
            return 0;
        }
        let mut busy_until = [0usize; 256];
        let mut depth = 0;
        for op in &self.ops {
            let start = op.qubits().iter().map(|&q| busy_until[q as usize]).max().unwrap_or(0);
            let end = start + 1;
            for &q in op.qubits() {
                busy_until[q as usize] = end;
            }
            if end > depth {
                depth = end;
            }
        }
        depth
    }

    /// Every gate used, as a bitmask, for checking against a device's `gate_set`.
    pub fn gate_set(&self) -> u64 {
        let mut m = 0u64;
        for op in &self.ops {
            if let Op::Gate { gate, .. } = op {
                m |= 1 << gate.bit();
            }
        }
        m
    }

    /// The measured qubits, ascending, deduplicated.
    ///
    /// ⚠️ Ascending **wire order**, not the order the measurements appear in the program — a basis
    /// state label reads left to right as wire 0 first so that it lines up with the circuit drawn
    /// above it. This is the same choice `qclang_compiler::statevector` made, and it is the
    /// opposite of Qiskit's convention. Matching the diagram beat matching another tool, because
    /// the two are side by side on screen and only one of them can be right.
    pub fn measured_qubits(&self) -> Vec<u8> {
        let mut seen = [false; 256];
        for op in &self.ops {
            if let Op::Measure { qubit, .. } = op {
                seen[*qubit as usize] = true;
            }
        }
        (0..self.qubits).filter(|&q| seen[q as usize]).collect()
    }

    /// Whether the circuit contains a [`Op::Reset`].
    ///
    /// The simulator needs this: reset collapses unconditionally, so a circuit containing one
    /// cannot use the deferred-measurement shortcut. See `sim.rs`.
    pub fn has_reset(&self) -> bool {
        self.ops.iter().any(|o| matches!(o, Op::Reset { .. }))
    }

    /// The canonical two-qubit Bell state: `H(q0); CX(q0, q1); measure both`.
    ///
    /// The demonstration circuit for the whole subsystem. Measuring it must yield only `00` and
    /// `11`, at roughly equal rates — and **never** `01` or `10` on a noiseless backend. That
    /// property is what makes it a useful end-to-end test: a wrong answer is obviously wrong.
    pub fn bell() -> Circuit {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::H, &[0]).gate(Gate::Cx, &[0, 1]).measure_all();
        c
    }

    /// An `n`-qubit GHZ state: `H(q0)` then a chain of CNOTs.
    ///
    /// Generalises [`Circuit::bell`] — `ghz(2)` is the Bell circuit. Measuring must yield only
    /// all-zeros and all-ones.
    pub fn ghz(n: u8) -> Circuit {
        let mut c = Circuit::new(n, n);
        if n == 0 {
            return c;
        }
        c.gate(Gate::H, &[0]);
        for q in 1..n {
            c.gate(Gate::Cx, &[q - 1, q]);
        }
        c.measure_all();
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn bell_circuit_is_what_it_claims() {
        let c = Circuit::bell();
        assert_eq!(c.qubits, 2);
        assert_eq!(c.cbits, 2);
        assert_eq!(c.ops.len(), 4); // H, CX, M, M
        assert_eq!(c.validate(), Ok(()));
        // H, then CX, then BOTH measurements together — they are on different wires, so they share
        // a layer. Three, not four.
        assert_eq!(c.depth(), 3);
        assert_eq!(c.measured_qubits(), vec![0, 1]);
        assert!(!c.has_reset());
    }

    #[test]
    fn ghz_of_two_is_the_bell_circuit() {
        assert_eq!(Circuit::ghz(2), Circuit::bell());
    }

    #[test]
    fn ghz_scales_and_stays_valid() {
        for n in 1..=8u8 {
            let c = Circuit::ghz(n);
            assert_eq!(c.validate(), Ok(()), "ghz({n}) failed validation");
            assert_eq!(c.measured_qubits().len(), n as usize);
        }
        assert_eq!(Circuit::ghz(0).ops.len(), 0);
    }

    #[test]
    fn arity_is_enforced_rather_than_padded() {
        assert!(Op::gate(Gate::Cx, &[0]).is_none());
        assert!(Op::gate(Gate::Cx, &[0, 1, 2]).is_none());
        assert!(Op::gate(Gate::H, &[0, 1]).is_none());
        assert!(Op::gate(Gate::Ccx, &[0, 1, 2]).is_some());

        let mut c = Circuit::new(3, 0);
        assert!(!c.try_gate(Gate::Cx, &[0]));
        assert!(c.ops.is_empty(), "a rejected gate must not be appended");
    }

    #[test]
    fn every_gates_arity_matches_its_qubit_slice() {
        for g in Gate::ALL {
            let qs: Vec<u8> = (0..g.arity() as u8).collect();
            let op = Op::gate(g, &qs).expect("ALL must be constructible at its own arity");
            assert_eq!(op.qubits().len(), g.arity(), "{}", g.qasm_name());
        }
    }

    #[test]
    fn gate_set_bits_are_unique() {
        // These cross the syscall boundary inside QpuInfo::gate_set; a collision would make two
        // different gates indistinguishable in a device's advertised capabilities.
        let mut seen = [false; 64];
        for g in Gate::ALL {
            let b = g.bit() as usize;
            assert!(!seen[b], "gate bit {b} used twice ({})", g.qasm_name());
            seen[b] = true;
        }
    }

    #[test]
    fn gate_set_masks_round_trip() {
        let m = gate_set_mask(&[Gate::H, Gate::Cx]);
        assert!(gate_set_contains(m, Gate::H));
        assert!(gate_set_contains(m, Gate::Cx));
        assert!(!gate_set_contains(m, Gate::Swap));
        // Angles must not affect identity: Rx(0.3) and Rx(1.2) are the same gate kind.
        let r = gate_set_mask(&[Gate::Rx(0.3)]);
        assert!(gate_set_contains(r, Gate::Rx(1.2)));

        assert_eq!(Circuit::bell().gate_set(), gate_set_mask(&[Gate::H, Gate::Cx]));
    }

    #[test]
    fn out_of_range_qubits_are_caught_with_the_offending_index() {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::H, &[5]);
        assert_eq!(
            c.validate(),
            Err(CircuitError::QubitOutOfRange { op: 0, qubit: 5, declared: 2 })
        );

        let mut c = Circuit::new(2, 1);
        c.measure(0, 7);
        assert_eq!(
            c.validate(),
            Err(CircuitError::CbitOutOfRange { op: 0, cbit: 7, declared: 1 })
        );
    }

    /// ★ A CNOT whose control is its target is the identity in most implementations, so it produces
    /// a believable histogram rather than an error. Catch it.
    #[test]
    fn a_two_qubit_gate_on_one_qubit_is_rejected() {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::Cx, &[1, 1]);
        assert_eq!(c.validate(), Err(CircuitError::RepeatedQubit { op: 0, qubit: 1 }));

        let mut c = Circuit::new(3, 3);
        c.gate(Gate::Ccx, &[0, 2, 0]);
        assert_eq!(c.validate(), Err(CircuitError::RepeatedQubit { op: 0, qubit: 0 }));
    }

    #[test]
    fn depth_packs_disjoint_gates_into_one_layer() {
        // Two single-qubit gates on different wires are simultaneous.
        let mut c = Circuit::new(2, 0);
        c.gate(Gate::H, &[0]).gate(Gate::X, &[1]);
        assert_eq!(c.depth(), 1);

        // Same wire: they serialise.
        let mut c = Circuit::new(2, 0);
        c.gate(Gate::H, &[0]).gate(Gate::X, &[0]);
        assert_eq!(c.depth(), 2);

        // A two-qubit gate blocks both of its wires.
        let mut c = Circuit::new(3, 0);
        c.gate(Gate::H, &[0]).gate(Gate::Cx, &[0, 1]).gate(Gate::X, &[2]);
        assert_eq!(c.depth(), 2);

        assert_eq!(Circuit::new(0, 0).depth(), 0);
    }

    #[test]
    fn measured_qubits_are_ascending_and_deduplicated() {
        let mut c = Circuit::new(4, 4);
        c.measure(3, 0).measure(1, 1).measure(3, 2);
        // Wire order, not program order — the label must line up with the drawn diagram.
        assert_eq!(c.measured_qubits(), vec![1, 3]);
    }

    #[test]
    fn measure_all_widens_the_classical_register() {
        let mut c = Circuit::new(3, 0);
        c.measure_all();
        assert_eq!(c.cbits, 3);
        assert_eq!(c.validate(), Ok(()));
    }

    #[test]
    fn reset_is_flagged_because_the_simulator_needs_to_know() {
        let mut c = Circuit::new(1, 0);
        assert!(!c.has_reset());
        c.reset(0);
        assert!(c.has_reset());
    }

    #[test]
    fn clifford_classification_is_right_where_it_matters() {
        assert!(Gate::H.is_clifford());
        assert!(Gate::Cx.is_clifford());
        assert!(!Gate::T.is_clifford(), "T is the canonical non-Clifford gate");
        assert!(!Gate::Ccx.is_clifford());
    }
}
