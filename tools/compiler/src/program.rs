//! The QIR flattened into the straight line of quantum operations a **circuit** is.
//!
//! `QirModule` is a control-flow graph of blocks holding twenty-odd kinds of operation, most of them
//! classical. A circuit diagram and a statevector both want the same much smaller thing: the quantum
//! ops, in execution order, each naming the qubits it touches. Producing that twice — once for the
//! picture and once for the maths — is how a diagram ends up disagreeing with the numbers printed
//! beside it. So it is produced once, here, and [`statevector`](crate::statevector) and the
//! `QCLang` window both consume it.
//!
//! Portable: no `rand`, no `num-complex`, no CLI. It is compiled into the on-device app.

use crate::qir::{BitState, QirGate, QirModule, QirOp, QirValue};

/// One quantum operation, with the qubits it acts on resolved to plain indices.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A unitary. `qubits` is in the gate's own argument order, so for `CNOT` it is
    /// `[control, target]` and the two are not interchangeable.
    Gate { gate: QirGate, qubits: Vec<usize> },
    /// A measurement of one qubit into one classical bit.
    Measure { qubit: usize, cbit: usize },
}

impl Step {
    /// Every qubit this step touches, in argument order.
    pub fn qubits(&self) -> Vec<usize> {
        match self {
            Step::Gate { qubits, .. } => qubits.clone(),
            Step::Measure { qubit, .. } => vec![*qubit],
        }
    }

    /// The letter the design draws inside the gate box: `H`, `X`, `M`, …
    ///
    /// Deliberately derived from [`QirGate::to_qasm_name`] rather than from a second match, so a
    /// gate the codegen knows how to emit cannot be drawn as something else. Multi-character QASM
    /// names (`ccx`, `swap`, `rx(0)`) are cut to their first two characters — a 32px box holds two
    /// glyphs of 12px mono and no more.
    pub fn letter(&self) -> String {
        match self {
            Step::Measure { .. } => "M".to_string(),
            Step::Gate { gate, .. } => {
                let name = gate.to_qasm_name();
                let stem: String = name.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                let mut up = stem.to_uppercase();
                up.truncate(2);
                up
            }
        }
    }
}

/// How far the walk is allowed to follow jumps before it calls the program non-terminating.
///
/// The same guard `simulator.rs` uses, for the same reason: `qfor`/`while` lower to back-edges, and
/// a circuit is a finite list of gates, so an unbounded loop has no picture. Reported as an error
/// rather than silently truncated.
const MAX_STEPS: usize = 100_000;

/// Flatten `main`'s control-flow graph into the ordered list of quantum operations it executes.
///
/// ## What this does with branches
///
/// It takes the `then` arm, which is what the statevector simulator has always done. That is a real
/// limitation and it is worth being exact about it: `Branch` carries a *classical* condition, and
/// nothing in this IR lets a measured bit reach a gate argument (`QirOp::ApplyGate` takes
/// [`QirValue::Qubit`] only), so the condition can only come from classical arithmetic the QIR does
/// not evaluate here. A circuit whose shape depends on a runtime `if` therefore has more than one
/// picture, and this returns the first of them. Callers that care should surface
/// [`Flattened::branched`].
pub fn flatten(module: &QirModule) -> Result<Flattened, String> {
    let func = module
        .functions
        .iter()
        .find(|f| f.name == "main")
        .ok_or_else(|| "no `main` function to draw".to_string())?;

    let mut out = Flattened::default();
    let mut block_id = func.entry_block;
    let mut steps = 0usize;

    'walk: loop {
        if steps > MAX_STEPS {
            return Err("the program does not terminate — no finite circuit to draw".to_string());
        }
        let block = func
            .blocks
            .get(&block_id)
            .ok_or_else(|| format!("jump to a block that does not exist ({})", block_id.0))?;

        for op in &block.ops {
            steps += 1;
            match op {
                // ★ A qubit literal is not a no-op. `qubit a = |1>;` lowers to an AllocQubit
                // carrying `BitState::One`, and the QASM backend turns that into a real
                // `x q[k];` in its initialization block. A walk that skipped AllocQubit would
                // produce a circuit and a distribution for |0…0> while the emitted QASM ran a
                // different program — and every "probabilities sum to 1" check would still pass,
                // because an ignored X leaves a perfectly valid distribution of the wrong state.
                //
                // The qubit's index is its ALLOCATION ORDER, which is the same counter the builder
                // hands to `QubitId` and the same one the QASM generator maps `temp_to_qubit` with.
                QirOp::AllocQubit { init_state, .. } => {
                    let one = match init_state {
                        None | Some(BitState::Zero) => false,
                        Some(BitState::One) => true,
                        Some(other) => {
                            return Err(format!(
                                "a qubit initialised to {:?} has no representation this viewer can draw",
                                other
                            ))
                        }
                    };
                    out.init.push(one);
                }
                QirOp::ApplyGate { gate, args, .. } => {
                    let qubits: Vec<usize> = args
                        .iter()
                        .filter_map(|a| match a {
                            QirValue::Qubit(q) => Some(q.0),
                            _ => None,
                        })
                        .collect();
                    // A gate whose arguments did not resolve to qubits is not drawable and is not
                    // simulable; dropping it silently would leave a circuit missing a gate that the
                    // emitted QASM still contains.
                    if qubits.len() != gate.arity() {
                        return Err(format!(
                            "{} takes {} qubit(s) but the IR gives {}",
                            gate.to_qasm_name(),
                            gate.arity(),
                            qubits.len()
                        ));
                    }
                    out.steps.push(Step::Gate { gate: gate.clone(), qubits });
                }
                QirOp::Measure { qubit, cbit } => {
                    out.steps.push(Step::Measure { qubit: qubit.0, cbit: cbit.0 });
                }
                QirOp::Reset { .. } => out.reset = true,
                QirOp::Jump { target } => {
                    block_id = *target;
                    continue 'walk;
                }
                QirOp::Branch { then_block, .. } => {
                    out.branched = true;
                    block_id = *then_block;
                    continue 'walk;
                }
                QirOp::Return { .. } => break 'walk,
                _ => {}
            }
        }
        break;
    }

    out.qubits = out
        .steps
        .iter()
        .flat_map(|s| s.qubits())
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    // An allocated-but-untouched qubit gets no wire (see `qubits`), so the init flags are trimmed
    // to match rather than left longer than the circuit.
    out.init.truncate(out.qubits);
    out.init.resize(out.qubits, false);
    Ok(out)
}

/// The straight-line form of a program, plus the two facts a viewer must not hide.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Flattened {
    pub steps: Vec<Step>,
    /// One past the highest qubit index any step touches. Not `QirModule::qubit_count`, which counts
    /// every qubit the *builder* allocated including ones the optimizer then dropped — drawing a
    /// wire for a qubit no gate ever touches is drawing something that is not in the program.
    pub qubits: usize,
    /// Per wire, true if the program prepares it in |1> rather than |0>.
    ///
    /// Drawn as the wire's own label (`|1>`), which is what the design's `.qw .lb` element is for,
    /// rather than as a leading X box. The two are the same circuit; the label is how a physicist
    /// writes a prepared state, and it keeps the gate columns showing only what the source wrote.
    pub init: Vec<bool>,
    /// A `Branch` was followed. The circuit shown is one of several the program can execute.
    pub branched: bool,
    /// A `Reset` was seen. It collapses state, so the deferred-measurement argument
    /// [`crate::statevector`] relies on does not hold and the distribution is not exact.
    pub reset: bool,
}

impl Flattened {
    /// Circuit depth: the number of layers when gates that touch disjoint qubits share one.
    ///
    /// The same packing the diagram draws with, so the number in the caption is the number of
    /// columns on screen rather than an independently-derived one that can disagree with it.
    pub fn depth(&self) -> usize {
        self.columns().iter().max().map(|c| c + 1).unwrap_or(0)
    }

    /// The column each step is drawn in.
    ///
    /// As-soon-as-possible packing: a step goes in the first column at or after every wire it
    /// touches is next free. It is **not** "the earliest column with a gap" — a measurement on wire
    /// 1 must not slide back into column 0 just because that column happens to be empty on that
    /// wire, since column order is execution order and the CNOT it depends on sits in between.
    ///
    /// A multi-qubit gate advances every wire it *spans*, not only the ones it names. The connector
    /// is drawn straight through the wires between control and target, so anything scheduled on a
    /// crossed wire in the same column would be drawn sitting on that connector and would read as
    /// part of the two-qubit operation.
    pub fn columns(&self) -> Vec<usize> {
        let mut next_free: Vec<usize> = vec![0; self.qubits.max(1)];
        let mut out = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let qs = step.qubits();
            let (lo, hi) = match (qs.iter().copied().min(), qs.iter().copied().max()) {
                (Some(lo), Some(hi)) => (lo, hi),
                _ => {
                    out.push(0);
                    continue;
                }
            };
            let span: core::ops::RangeInclusive<usize> =
                if qs.len() > 1 { lo..=hi } else { lo..=lo };
            if hi >= next_free.len() {
                next_free.resize(hi + 1, 0);
            }
            let col = span.clone().map(|w| next_free[w]).max().unwrap_or(0);
            for w in span {
                next_free[w] = col + 1;
            }
            out.push(col);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Compiler;

    fn flat(src: &str) -> Flattened {
        let res = Compiler::compile_with_stats(src, true).expect("the fixture must compile");
        flatten(&res.ir).expect("the fixture must flatten")
    }

    const BELL: &str = r#"
fn main() -> int {
    qubit a = |0>;
    qubit b = |0>;
    qubit c = CNOT(H(a), b);
    cbit r1 = measure(a);
    cbit r2 = measure(b);
    return 0;
}
"#;

    /// The whole point: the picture comes from the program, not from a fixture. A Bell pair is one
    /// H, one CNOT and two measurements, in that order, on two wires.
    #[test]
    fn a_bell_pair_flattens_to_the_circuit_it_is() {
        let f = flat(BELL);
        assert_eq!(f.qubits, 2);
        assert_eq!(f.steps.len(), 4, "{:?}", f.steps);
        assert_eq!(f.steps[0], Step::Gate { gate: QirGate::H, qubits: vec![0] });
        assert_eq!(f.steps[1], Step::Gate { gate: QirGate::CNOT, qubits: vec![0, 1] });
        assert!(matches!(f.steps[2], Step::Measure { qubit: 0, .. }));
        assert!(matches!(f.steps[3], Step::Measure { qubit: 1, .. }));
        assert!(!f.branched && !f.reset);
    }

    /// The design's own scene: H in column 0, CX in column 1, and **both** measurements sharing
    /// column 2 because they touch different wires. Depth 3, which is what the caption says.
    #[test]
    fn disjoint_steps_share_a_column_and_depth_counts_layers() {
        let f = flat(BELL);
        assert_eq!(f.columns(), vec![0, 1, 2, 2]);
        assert_eq!(f.depth(), 3);
    }

    /// A CNOT's connector runs through every wire between control and target, so nothing else may
    /// be scheduled in that column on a wire it crosses — otherwise a gate on the middle wire is
    /// drawn sitting on the connector and reads as part of the two-qubit operation.
    #[test]
    fn a_multi_qubit_gate_reserves_every_wire_it_spans() {
        // q0 and q2 entangled, with a gate on q1 issued after: the packer must not put the q1 gate
        // in the CNOT's column.
        let f = flat(
            r#"
fn main() -> int {
    qubit a = |0>;
    qubit b = |0>;
    qubit c = |0>;
    qubit d = CNOT(a, c);
    qubit e = H(b);
    cbit r1 = measure(a);
    cbit r2 = measure(b);
    cbit r3 = measure(c);
    return 0;
}
"#,
        );
        let cols = f.columns();
        assert_eq!(f.qubits, 3, "{:?}", f.steps);
        assert!(matches!(f.steps[0], Step::Gate { gate: QirGate::CNOT, .. }), "{:?}", f.steps);
        assert!(matches!(f.steps[1], Step::Gate { gate: QirGate::H, .. }), "{:?}", f.steps);
        assert_ne!(cols[0], cols[1], "the H on the spanned wire shared the CNOT's column");
    }

    /// The letter in the box has to come from the gate, or the diagram and the emitted QASM can
    /// drift apart without anything failing.
    #[test]
    fn a_gates_letter_comes_from_its_qasm_name() {
        assert_eq!(Step::Gate { gate: QirGate::H, qubits: vec![0] }.letter(), "H");
        assert_eq!(Step::Gate { gate: QirGate::CNOT, qubits: vec![0, 1] }.letter(), "CX");
        assert_eq!(Step::Gate { gate: QirGate::SWAP, qubits: vec![0, 1] }.letter(), "SW");
        assert_eq!(Step::Gate { gate: QirGate::Toffoli, qubits: vec![0, 1, 2] }.letter(), "CC");
        // A parameterised gate keeps its stem and drops the parenthesised angle.
        assert_eq!(Step::Gate { gate: QirGate::RX(0.0), qubits: vec![0] }.letter(), "RX");
        assert_eq!(Step::Measure { qubit: 0, cbit: 0 }.letter(), "M");
    }

    /// An empty program is a legal program. It must produce an empty circuit, not an error and not
    /// a panic on the `max()` of nothing.
    #[test]
    fn a_program_with_no_quantum_operations_is_an_empty_circuit() {
        let f = flat("fn main() -> int { return 0; }");
        assert_eq!(f.qubits, 0);
        assert!(f.steps.is_empty());
        assert_eq!(f.depth(), 0);
        assert!(f.columns().is_empty());
    }

    /// Wires are counted from the steps, not from the module's allocation counter: a qubit the
    /// optimizer proved dead has no gates and must not be drawn as an empty wire.
    #[test]
    fn only_qubits_that_are_actually_operated_on_get_a_wire() {
        let f = flat(
            r#"
fn main() -> int {
    qubit a = |0>;
    qubit b = |0>;
    qubit c = H(a);
    cbit r = measure(a);
    return 0;
}
"#,
        );
        assert_eq!(f.qubits, 1, "b is never touched, so it is not a wire: {:?}", f.steps);
    }
}
