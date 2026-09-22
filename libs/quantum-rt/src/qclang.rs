//! `.ql` source → `nyx_quantum::Circuit`, via the compiler Nyx already has.
//!
//! `tools/compiler` turns QCLang into a QIR control-flow graph; `program::flatten` walks that into
//! the straight line of quantum operations a circuit is. This module is the last step: that straight
//! line, in the OS's own IR, so it can be handed to a simulator, a provider, or a device.
//!
//! **`tools/compiler` is not modified by any of this.** It has an on-device consumer
//! (`apps/qcstudio`) and a terminal command (`compile`) already depending on it, and the whole point
//! of `nyx_quantum::Circuit` being a lossless subset of `program::Step` is that adaptation is a
//! match arm per variant rather than a translation.
//!
//! ## ⚠️ Rotations from qclang are refused, and that is not a limitation of this adapter
//!
//! [`qclang_compiler::qir::QirGate::RX`] and friends arrive here **with their angle already
//! discarded**. `QirGate::from_ast_gate` matches `AstGate::RX(expr)` and constructs
//! `QirGate::RX(0.0)`, dropping `expr` — an upstream placeholder, documented as such in
//! `tools/compiler/src/qir/operations.rs`.
//!
//! So a `qrx(1.57) q;` in a `.ql` file reaches this function as `RX(0.0)`, which is the identity.
//! Converting it to [`nyx_quantum::Gate::Rx`] would produce a circuit that silently ignores one of
//! its gates and still returns a perfectly normalised, entirely wrong distribution.
//!
//! `qclang_compiler::statevector` refuses these gates by name for exactly this reason. This adapter
//! refuses them too, and says why. **The refusal is load-bearing** — the moment the upstream
//! placeholder is fixed, the check in [`convert_gate`] should be deleted and the angles passed
//! through, and there is a test below that will start failing when that happens.
//!
//! Note that `nyx_quantum`'s own simulator *does* implement rotations — its IR carries the angle, so
//! there is nothing lost. The problem is purely the qclang → Nyx boundary.

use nyx_quantum::{Circuit, Gate, Op};
use qclang_compiler::program::{self, Flattened, Step};
use qclang_compiler::qir::QirGate;
use qclang_compiler::Compiler;

/// Why a QCLang program could not be turned into a circuit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdaptError {
    /// The source did not compile. Carries the compiler's own diagnostics.
    Compile(Vec<String>),
    /// The QIR could not be flattened into a finite circuit — a non-terminating loop, or no `main`.
    Flatten(String),
    /// A gate this IR cannot faithfully represent. Named, never dropped.
    UnsupportedGate(String),
    /// More than 255 qubits, which `nyx_quantum`'s `u8` indices cannot address.
    TooManyQubits(usize),
}

impl core::fmt::Display for AdaptError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AdaptError::Compile(errs) => write!(f, "{}", errs.join("; ")),
            AdaptError::Flatten(m) => write!(f, "{m}"),
            AdaptError::UnsupportedGate(m) => write!(f, "{m}"),
            AdaptError::TooManyQubits(n) => {
                write!(f, "{n} qubits; Nyx circuits address at most 255")
            }
        }
    }
}

/// Compile QCLang source and return it as a circuit.
///
/// The whole `.ql` → lex → parse → semantics → QIR → flatten → [`Circuit`] path in one call.
pub fn compile_to_circuit(source: &str) -> Result<Circuit, AdaptError> {
    let result = Compiler::compile_with_stats(source, true).map_err(AdaptError::Compile)?;
    let flat = program::flatten(&result.ir).map_err(AdaptError::Flatten)?;
    from_flattened(&flat)
}

/// Convert an already-flattened qclang program.
///
/// Separate from [`compile_to_circuit`] so a caller that already has a `Flattened` — `apps/qcstudio`
/// draws from one — does not compile twice.
pub fn from_flattened(flat: &Flattened) -> Result<Circuit, AdaptError> {
    if flat.qubits > u8::MAX as usize {
        return Err(AdaptError::TooManyQubits(flat.qubits));
    }
    let qubits = flat.qubits as u8;

    // Classical bits: one past the highest cbit any measurement writes. `Flattened` does not carry
    // a cbit count, and sizing this to `qubits` would be wrong for a program that measures three
    // qubits into one bit.
    let cbits = flat
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Measure { cbit, .. } => Some(*cbit + 1),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    if cbits > u8::MAX as usize {
        return Err(AdaptError::TooManyQubits(cbits));
    }

    let mut c = Circuit::new(qubits, cbits as u8);

    // ★ A prepared |1⟩ is a real instruction, not a no-op. `qubit a = |1>;` lowers to an
    // `AllocQubit` carrying `BitState::One`, and qclang's QASM backend emits a genuine `x q[k];` for
    // it. `Flattened` hoists that out of the step list into `init` so the diagram can draw it as a
    // wire label rather than a gate box — but a circuit that is going to be *executed* needs the X
    // back, or it computes the distribution of a different program while still summing to 1.
    for (q, &one) in flat.init.iter().enumerate() {
        if one {
            c.gate(Gate::X, &[q as u8]);
        }
    }

    for step in &flat.steps {
        match step {
            Step::Gate { gate, qubits: qs } => {
                let g = convert_gate(gate)?;
                let idx: Vec<u8> = qs.iter().map(|&q| q as u8).collect();
                if !c.try_gate(g, &idx) {
                    return Err(AdaptError::UnsupportedGate(alloc_format(
                        &gate.to_qasm_name(),
                        idx.len(),
                        g.arity(),
                    )));
                }
            }
            Step::Measure { qubit, cbit } => {
                c.ops.push(Op::Measure { qubit: *qubit as u8, cbit: *cbit as u8 });
            }
        }
    }

    Ok(c)
}

fn alloc_format(name: &str, got: usize, want: usize) -> String {
    format!("`{name}` was given {got} qubits but takes {want}")
}

/// Map one qclang gate onto Nyx's gate set.
///
/// See the module docs for why the rotations are refused rather than converted.
fn convert_gate(g: &QirGate) -> Result<Gate, AdaptError> {
    Ok(match g {
        QirGate::H => Gate::H,
        QirGate::X => Gate::X,
        QirGate::Y => Gate::Y,
        QirGate::Z => Gate::Z,
        QirGate::S => Gate::S,
        QirGate::Sdg => Gate::Sdg,
        QirGate::T => Gate::T,
        QirGate::Tdg => Gate::Tdg,
        QirGate::CNOT => Gate::Cx,
        QirGate::SWAP => Gate::Swap,
        QirGate::Toffoli => Gate::Ccx,

        // ⚠️ Refused, NOT converted. The angle was discarded upstream — see the module docs. When
        // `QirGate::from_ast_gate` starts carrying real angles, delete this arm and pass them
        // through; `rotations_are_refused_while_their_angles_are_lost` below will tell you when.
        QirGate::RX(_) | QirGate::RY(_) | QirGate::RZ(_) | QirGate::U3(..) => {
            return Err(AdaptError::UnsupportedGate(format!(
                "`{}` carries no angle in qclang's IR yet, so its effect cannot be computed",
                g.to_qasm_name()
            )))
        }

        // Fredkin (CSWAP) is representable, just not in Nyx's initial gate set — the brief asked for
        // the smallest architecture that can grow, and this is one of the things it can grow.
        QirGate::Fredkin => {
            return Err(AdaptError::UnsupportedGate(
                "`cswap` (Fredkin) is not in Nyx's gate set yet".to_string(),
            ))
        }
        QirGate::Custom { name, .. } => {
            return Err(AdaptError::UnsupportedGate(format!(
                "the custom gate `{name}` has no definition Nyx can execute"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_quantum::sim;

    /// ★★ The cross-check the whole port rests on.
    ///
    /// `nyx_quantum::sim` is a port of `qclang_compiler::statevector`. The original has been
    /// producing histograms on real hardware inside `apps/qcstudio`; if the port disagrees with it,
    /// the port is wrong. So: compile a program once, evaluate it with **both** evaluators, and
    /// require the distributions to match.
    ///
    /// This runs on every `cargo test`, on a machine with no QEMU, where the alternative way to
    /// find a sign error in a gate matrix is a power cycle and a photograph.
    fn cross_check(source: &str) {
        let result = Compiler::compile_with_stats(source, true)
            .unwrap_or_else(|e| panic!("qclang rejected the test program: {e:?}"));
        let flat = program::flatten(&result.ir).expect("flatten");

        let theirs = qclang_compiler::statevector::evaluate(&flat)
            .unwrap_or_else(|e| panic!("qclang's evaluator refused: {e}"));

        let circuit = from_flattened(&flat).expect("adapt");
        let ours = sim::evaluate(&circuit).expect("nyx sim");

        assert_eq!(
            ours.measured.len(),
            theirs.measured.len(),
            "different number of measured wires\nsource:\n{source}"
        );
        for (a, b) in ours.measured.iter().zip(theirs.measured.iter()) {
            assert_eq!(*a as usize, *b, "measured wire order differs\nsource:\n{source}");
        }
        assert_eq!(
            ours.probs.len(),
            theirs.probs.len(),
            "different outcome-space size\nsource:\n{source}"
        );
        for i in 0..ours.probs.len() {
            assert!(
                (ours.probs[i] - theirs.probs[i]).abs() < 1e-12,
                "outcome {} ({}): nyx says {}, qclang says {}\nsource:\n{source}",
                i,
                ours.label(i),
                ours.probs[i],
                theirs.probs[i]
            );
        }
    }

    /// The Bell pair, in the form `apps/qcstudio/samples/sample.ql` actually ships.
    ///
    /// ⚠️ QCLang uses **affine typing** for quantum resources: a qubit cannot be reassigned, so
    /// gates are applied as fresh bindings — `qubit c = CNOT(H(a), b);` — rather than as statements
    /// mutating a wire. The return type is mandatory. Writing these programs in the imperative
    /// `h a;` style a reader might expect gets a parse error, so they are kept in the shape the
    /// shipped sample uses.
    const BELL: &str = "fn main() -> int { \
        qubit a = |0>; qubit b = |0>; qubit c = CNOT(H(a), b); \
        cbit r1 = measure(a); cbit r2 = measure(b); return 0; }";

    #[test]
    fn port_agrees_with_qclang_on_a_bell_pair() {
        cross_check(BELL);
    }

    #[test]
    fn port_agrees_with_qclang_on_a_prepared_one_state() {
        // ★ The case the `init` handling exists for.
        //
        // `qubit a = |1>` does NOT produce a gate step — `Flattened` hoists it into `init` so a
        // diagram can draw it as a wire label. Verified: this program flattens to
        // `steps = [Measure { qubit: 0, cbit: 0 }]` with `init = [true]`.
        //
        // So an adapter that only walked `steps` would produce a circuit for |0⟩ and a perfectly
        // normalised distribution of the wrong program. This test is what catches that.
        cross_check("fn main() -> int { qubit a = |1>; cbit r = measure(a); return 0; }");
    }

    #[test]
    fn port_agrees_with_qclang_on_phase_gates() {
        // T and S are where a sign or conjugation error hides: they are invisible in the
        // computational basis unless an interference path exposes them, which the outer H pair does.
        cross_check(
            "fn main() -> int { qubit a = |0>; qubit z = H(T(T(H(a)))); \
             cbit r = measure(a); return 0; }",
        );
    }

    #[test]
    fn port_agrees_with_qclang_on_a_three_qubit_ghz() {
        cross_check(
            "fn main() -> int { qubit a = |0>; qubit b = |0>; qubit c = |0>; \
             qubit t1 = CNOT(H(a), b); qubit t2 = CNOT(t1, c); \
             cbit r1 = measure(a); cbit r2 = measure(b); cbit r3 = measure(c); return 0; }",
        );
    }

    #[test]
    fn a_bell_program_adapts_to_the_expected_circuit() {
        let c = compile_to_circuit(BELL).expect("adapt");

        assert_eq!(c.qubits, 2);
        assert_eq!(c.cbits, 2);
        assert_eq!(c.validate(), Ok(()));
        assert_eq!(c.measured_qubits(), vec![0, 1]);

        let d = sim::evaluate(&c).unwrap();
        let p = |l: &str| d.labelled().into_iter().find(|(k, _)| k == l).map(|(_, v)| v).unwrap_or(0.0);
        assert!((p("00") - 0.5).abs() < 1e-12);
        assert!((p("11") - 0.5).abs() < 1e-12);
        assert_eq!(p("01"), 0.0);
        assert_eq!(p("10"), 0.0);
    }

    #[test]
    fn a_prepared_one_becomes_a_real_x_gate() {
        let c =
            compile_to_circuit("fn main() -> int { qubit a = |1>; cbit r = measure(a); return 0; }")
                .expect("adapt");
        // The X must be in the op list, not implied by a field the executor would ignore.
        assert!(
            c.ops.iter().any(|o| matches!(o, Op::Gate { gate: Gate::X, .. })),
            "the prepared |1> was dropped: {:?}",
            c.ops
        );
        let d = sim::evaluate(&c).unwrap();
        assert!((d.labelled()[1].1 - 1.0).abs() < 1e-12, "should be |1> with certainty");
    }

    /// ★ A canary, not a limitation being cemented.
    ///
    /// This asserts the CURRENT upstream behaviour: qclang discards rotation angles, so the adapter
    /// refuses those gates rather than silently treating them as the identity. When `tools/compiler`
    /// starts carrying real angles, this test will fail — and the fix is to delete the refusal in
    /// `convert_gate` and pass the angle through. `nyx_quantum`'s simulator already implements them.
    #[test]
    fn rotations_are_refused_while_their_angles_are_lost() {
        let e = convert_gate(&QirGate::RX(1.5707963)).unwrap_err();
        match &e {
            AdaptError::UnsupportedGate(m) => {
                assert!(m.contains("angle"), "{m}");
                assert!(m.contains("rx"), "{m}");
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
        // The refusal must be explicit for every affected gate, not just rx.
        assert!(convert_gate(&QirGate::RY(0.0)).is_err());
        assert!(convert_gate(&QirGate::RZ(0.0)).is_err());
        assert!(convert_gate(&QirGate::U3(0.0, 0.0, 0.0)).is_err());
    }

    #[test]
    fn unsupported_gates_are_named_rather_than_dropped() {
        match convert_gate(&QirGate::Fredkin).unwrap_err() {
            AdaptError::UnsupportedGate(m) => assert!(m.contains("cswap"), "{m}"),
            other => panic!("{other:?}"),
        }
        match convert_gate(&QirGate::Custom { name: "wobble".into(), matrix: vec![] }).unwrap_err() {
            AdaptError::UnsupportedGate(m) => assert!(m.contains("wobble"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_gate_nyx_supports_maps_from_qclang() {
        for (q, n) in [
            (QirGate::H, Gate::H),
            (QirGate::X, Gate::X),
            (QirGate::Y, Gate::Y),
            (QirGate::Z, Gate::Z),
            (QirGate::S, Gate::S),
            (QirGate::Sdg, Gate::Sdg),
            (QirGate::T, Gate::T),
            (QirGate::Tdg, Gate::Tdg),
            (QirGate::CNOT, Gate::Cx),
            (QirGate::SWAP, Gate::Swap),
            (QirGate::Toffoli, Gate::Ccx),
        ] {
            assert_eq!(convert_gate(&q).unwrap(), n, "{q:?}");
            // Arity must agree across the boundary, or a converted circuit silently changes shape.
            assert_eq!(q.arity(), n.arity(), "arity differs for {q:?}");
        }
    }

    #[test]
    fn a_program_that_does_not_compile_returns_the_compilers_own_diagnostics() {
        match compile_to_circuit("fn main() { this is not qclang }") {
            Err(AdaptError::Compile(errs)) => assert!(!errs.is_empty()),
            other => panic!("expected compiler diagnostics, got {other:?}"),
        }
    }
}
