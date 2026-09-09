//! The **exact** output distribution of a QCLang program — no sampling, no RNG, no noise model.
//!
//! ## Why this exists next to `simulator.rs`
//!
//! [`crate::simulator`] answers a different question and cannot answer this one. It is gated behind
//! `feature = "sim"` (which pulls `num-complex` and `rand`, neither of which the on-device build
//! links), it runs **one shot** and returns a log of what that shot happened to measure, and it
//! collapses the state as it goes. A viewer that wants "what does this program do" would have to run
//! it a thousand times through an RNG the machine does not have a good entropy source for, and would
//! still only get an estimate of a number that can be computed exactly.
//!
//! So this module computes the distribution directly from the amplitudes. It is portable — plain
//! `f64`, no dependencies — which is the reason it can run inside the QCLang window on the device.
//!
//! ## Deferred measurement, and why it is exact rather than an approximation
//!
//! Measurements are not applied as collapses. The circuit is evolved unitarily to the end and the
//! result is the **marginal** distribution over the measured qubits. That is exact here, and the
//! reason is a property of this IR rather than a general truth: `QirOp::ApplyGate` takes
//! [`QirValue::Qubit`](crate::qir::QirValue) arguments only, so no measured bit can ever reach a
//! gate. With no classical feedback, the deferred-measurement principle says a mid-circuit
//! measurement is indistinguishable from the same measurement at the end.
//!
//! `QirOp::Reset` breaks that, because it collapses unconditionally. [`Flattened::reset`] records
//! when one was seen and [`evaluate`] refuses rather than quietly returning a wrong distribution.
//!
//! ## What it deliberately does not report
//!
//! **Fidelity.** There is no noise model anywhere in this compiler — no depolarising channel, no
//! gate-error rates, no decoherence. An ideal statevector has fidelity 1 against itself by
//! construction, so any number other than 1.000 would have to be invented. A viewer that wants a
//! fidelity figure needs a noise model first.

use crate::program::{Flattened, Step};
use crate::qir::QirGate;

/// Above this the statevector stops fitting in a sensible amount of memory (2^20 amplitudes is
/// 16 MB) and, more to the point, no circuit that large has a readable picture. Reported as an
/// error so the caller can say why rather than hang.
pub const MAX_QUBITS: usize = 20;

/// One complex amplitude. A two-field struct rather than `num_complex::Complex<f64>` so this module
/// carries no dependency and compiles into the on-device app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    const ZERO: C = C { re: 0.0, im: 0.0 };
    const ONE: C = C { re: 1.0, im: 0.0 };
    fn add(self, o: C) -> C {
        C { re: self.re + o.re, im: self.im + o.im }
    }
    fn sub(self, o: C) -> C {
        C { re: self.re - o.re, im: self.im - o.im }
    }
    fn mul(self, o: C) -> C {
        C { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
    fn scale(self, k: f64) -> C {
        C { re: self.re * k, im: self.im * k }
    }
    fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// The exact distribution over the measured qubits.
#[derive(Clone, Debug, PartialEq)]
pub struct Distribution {
    /// The qubits that were measured, in **ascending wire order** — not in the order the
    /// measurements appear in the program. A basis-state label is read left to right as wire 0
    /// first, so that it lines up with the circuit drawn above it, top wire at the top.
    ///
    /// ⚠️ This is the opposite of Qiskit's convention, which prints the highest-index qubit first.
    /// Matching the diagram was judged more valuable than matching another tool, since the two are
    /// side by side on screen and only one of them can be right.
    pub measured: Vec<usize>,
    /// `probs[i]` is the probability of the outcome whose bit `k` (counting from the **left** of the
    /// label) is `measured[k]`'s result. Length `1 << measured.len()`.
    pub probs: Vec<f64>,
    /// The program can execute more than one circuit and this is the distribution of the one that
    /// was drawn. Set from [`Flattened::branched`].
    pub branched: bool,
}

impl Distribution {
    /// The label the design prints for outcome `i`, e.g. `|01>`.
    ///
    /// ASCII `>` rather than the ket's `⟩` (U+27E9) on purpose: `|0>` is QCLang's own literal
    /// syntax — it is what the source file three inches to the left actually says — and JetBrains
    /// Mono has no angle-bracket ket to fall back on anyway.
    pub fn label(&self, i: usize) -> String {
        let n = self.measured.len();
        let mut s = String::with_capacity(n + 2);
        s.push('|');
        for k in 0..n {
            s.push(if i >> (n - 1 - k) & 1 == 1 { '1' } else { '0' });
        }
        s.push('>');
        s
    }

    /// Outcomes sorted by descending probability, as `(index, p)`. Long tails of impossible states
    /// are the norm — a 4-qubit circuit has 16 rows and usually two of them matter.
    pub fn ranked(&self) -> Vec<(usize, f64)> {
        let mut v: Vec<(usize, f64)> = self.probs.iter().copied().enumerate().collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        v
    }
}

/// Evolve `flat` from |0…0> and return the exact distribution over its measured qubits.
///
/// `Err` carries a sentence fit to put on screen. Every failure here is a real limitation of this
/// evaluator or of the IR, and saying which one is the whole value of returning an error instead of
/// a plausible-looking number.
pub fn evaluate(flat: &Flattened) -> Result<Distribution, String> {
    if flat.reset {
        return Err("this program resets a qubit, which collapses the state — no exact distribution".into());
    }
    if flat.qubits > MAX_QUBITS {
        return Err(format!("{} qubits is too many to evaluate exactly (limit {})", flat.qubits, MAX_QUBITS));
    }

    let n = flat.qubits;
    let mut state = vec![C::ZERO; 1usize << n];
    // Start from the basis state the program PREPARES, not from |0…0>. `qubit a = |1>;` is a real
    // instruction — the QASM backend emits an `x q[0];` for it — so evolving from |0…0> would
    // compute the distribution of a different program while still summing to 1.
    let start = flat.init.iter().enumerate().fold(0usize, |b, (q, &one)| if one { b | 1 << q } else { b });
    state[start] = C::ONE;

    for step in &flat.steps {
        if let Step::Gate { gate, qubits } = step {
            apply(&mut state, gate, qubits)?;
        }
    }

    // Measured qubits, de-duplicated and in wire order. Measuring the same qubit twice is legal and
    // yields the same bit both times, so it contributes one column, not two.
    let mut measured: Vec<usize> = flat
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Measure { qubit, .. } => Some(*qubit),
            _ => None,
        })
        .collect();
    measured.sort_unstable();
    measured.dedup();

    let m = measured.len();
    let mut probs = vec![0.0f64; 1usize << m];
    for (basis, amp) in state.iter().enumerate() {
        let p = amp.norm_sqr();
        if p == 0.0 {
            continue;
        }
        // Project the full basis index onto just the measured wires, most significant bit first so
        // the label reads wire 0, wire 1, ... left to right.
        let mut out = 0usize;
        for (k, &q) in measured.iter().enumerate() {
            if basis >> q & 1 == 1 {
                out |= 1 << (m - 1 - k);
            }
        }
        probs[out] += p;
    }

    Ok(Distribution { measured, probs, branched: flat.branched })
}

/// Apply one gate in place.
fn apply(state: &mut [C], gate: &QirGate, qubits: &[usize]) -> Result<(), String> {
    // Single-qubit unitaries are all the same loop over disjoint index pairs, differing only in the
    // 2x2 matrix — writing that loop once is what keeps a sign error from living in only one gate.
    let one = |m: [C; 4], t: usize, state: &mut [C]| {
        for i in 0..state.len() {
            if i >> t & 1 != 0 {
                continue;
            }
            let j = i | 1 << t;
            let (a, b) = (state[i], state[j]);
            state[i] = m[0].mul(a).add(m[1].mul(b));
            state[j] = m[2].mul(a).add(m[3].mul(b));
        }
    };
    let r = |v: f64| C { re: v, im: 0.0 };
    let im = |v: f64| C { re: 0.0, im: v };
    // 1/sqrt(2), written out because `core` has no const sqrt.
    const ISQRT2: f64 = 0.707_106_781_186_547_52;

    match gate {
        QirGate::H => one([r(ISQRT2), r(ISQRT2), r(ISQRT2), r(-ISQRT2)], qubits[0], state),
        QirGate::X => one([C::ZERO, C::ONE, C::ONE, C::ZERO], qubits[0], state),
        QirGate::Y => one([C::ZERO, im(-1.0), im(1.0), C::ZERO], qubits[0], state),
        QirGate::Z => one([C::ONE, C::ZERO, C::ZERO, r(-1.0)], qubits[0], state),
        QirGate::S => one([C::ONE, C::ZERO, C::ZERO, im(1.0)], qubits[0], state),
        QirGate::Sdg => one([C::ONE, C::ZERO, C::ZERO, im(-1.0)], qubits[0], state),
        QirGate::T => one([C::ONE, C::ZERO, C::ZERO, C { re: ISQRT2, im: ISQRT2 }], qubits[0], state),
        QirGate::Tdg => one([C::ONE, C::ZERO, C::ZERO, C { re: ISQRT2, im: -ISQRT2 }], qubits[0], state),
        QirGate::CNOT => controlled_x(state, qubits[0], qubits[1]),
        QirGate::Toffoli => {
            for i in 0..state.len() {
                if i >> qubits[0] & 1 == 1 && i >> qubits[1] & 1 == 1 && i >> qubits[2] & 1 == 0 {
                    state.swap(i, i | 1 << qubits[2]);
                }
            }
        }
        QirGate::SWAP => {
            let (a, b) = (qubits[0], qubits[1]);
            for i in 0..state.len() {
                if i >> a & 1 == 1 && i >> b & 1 == 0 {
                    state.swap(i, (i & !(1 << a)) | 1 << b);
                }
            }
        }
        QirGate::Fredkin => {
            let (c, a, b) = (qubits[0], qubits[1], qubits[2]);
            for i in 0..state.len() {
                if i >> c & 1 == 1 && i >> a & 1 == 1 && i >> b & 1 == 0 {
                    state.swap(i, (i & !(1 << a)) | 1 << b);
                }
            }
        }
        // ⚠️ The rotation gates arrive here with their angle already lost. `QirGate::from_ast_gate`
        // matches `AstGate::RX(expr)` and constructs `QirGate::RX(0.0)`, discarding `expr` — an
        // upstream placeholder, not a decision of this module. Evaluating them as the identity they
        // literally are would draw a circuit whose numbers silently ignore three of its gates, so
        // they are refused by name instead.
        QirGate::RX(_) | QirGate::RY(_) | QirGate::RZ(_) | QirGate::U3(..) => {
            return Err(format!(
                "{} carries no angle in this IR yet, so its effect cannot be computed",
                gate.to_qasm_name()
            ))
        }
        QirGate::Custom { name, .. } => {
            return Err(format!("the custom gate `{}` has no definition this evaluator can apply", name))
        }
    }
    Ok(())
}

fn controlled_x(state: &mut [C], control: usize, target: usize) {
    for i in 0..state.len() {
        if i >> control & 1 == 1 && i >> target & 1 == 0 {
            state.swap(i, i | 1 << target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::flatten;
    use crate::Compiler;

    fn dist(src: &str) -> Distribution {
        let res = Compiler::compile_with_stats(src, true).expect("the fixture must compile");
        let flat = flatten(&res.ir).expect("the fixture must flatten");
        evaluate(&flat).expect("the fixture must evaluate")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The one every quantum textbook opens with, and the one the design's scene draws: an H and a
    /// CNOT put half the probability on |00> and half on |11>, and **none** on |01> or |10>. That
    /// last part is the whole content of entanglement — a simulator that got the marginals right but
    /// the correlation wrong would show 25% four times and look perfectly reasonable.
    #[test]
    fn a_bell_pair_is_half_00_and_half_11_and_nothing_else() {
        let d = dist(
            r#"
fn main() -> int {
    qubit a = |0>;
    qubit b = |0>;
    qubit c = CNOT(H(a), b);
    cbit r1 = measure(a);
    cbit r2 = measure(b);
    return 0;
}
"#,
        );
        assert_eq!(d.measured, vec![0, 1]);
        assert_eq!(d.probs.len(), 4);
        assert!(close(d.probs[0b00], 0.5), "{:?}", d.probs);
        assert!(close(d.probs[0b11], 0.5), "{:?}", d.probs);
        assert!(close(d.probs[0b01], 0.0) && close(d.probs[0b10], 0.0), "{:?}", d.probs);
        assert_eq!(d.label(0b00), "|00>");
        assert_eq!(d.label(0b11), "|11>");
    }

    /// X on a qubit prepared in |0> is a certainty, not a coin flip. Catches a transposed matrix,
    /// which a symmetric gate like H would not.
    #[test]
    fn x_flips_a_qubit_with_certainty() {
        let d = dist(
            r#"
fn main() -> int {
    qubit a = |0>;
    qubit b = X(a);
    cbit r = measure(a);
    return 0;
}
"#,
        );
        assert_eq!(d.probs.len(), 2);
        assert!(close(d.probs[1], 1.0), "{:?}", d.probs);
    }

    /// ★ `qubit a = |1>;` prepares |1>, and the QASM backend emits an `x q[0];` to say so. An
    /// evaluator that always started from |0…0> would report this circuit as measuring 0 with
    /// certainty — the exact opposite of what the program does — and every "sums to 1" check would
    /// still pass, because an ignored preparation leaves a perfectly normalised wrong answer.
    #[test]
    fn a_qubit_prepared_in_one_starts_there() {
        let d = dist(
            r#"
fn main() -> int {
    qubit a = |1>;
    cbit r = measure(a);
    return 0;
}
"#,
        );
        assert!(close(d.probs[1], 1.0), "prepared |1> but measured {:?}", d.probs);
    }

    /// ...and it has to compose with the gates that follow. H on |1> is |->, which measures 50/50 —
    /// the same *marginal* as H on |0>, so this only differs from the bug once a second gate reads
    /// the phase. The Bell pair built from |1> is the cheapest circuit where it does: it lands on
    /// |01> and |10> rather than |00> and |11>.
    #[test]
    fn a_prepared_state_composes_with_the_gates_after_it() {
        let d = dist(
            r#"
fn main() -> int {
    qubit a = |0>;
    qubit b = |1>;
    qubit c = CNOT(H(a), b);
    cbit r1 = measure(a);
    cbit r2 = measure(b);
    return 0;
}
"#,
        );
        assert!(close(d.probs[0b01], 0.5), "{:?}", d.probs);
        assert!(close(d.probs[0b10], 0.5), "{:?}", d.probs);
        assert!(close(d.probs[0b00], 0.0) && close(d.probs[0b11], 0.0), "{:?}", d.probs);
    }

    /// Probabilities are a distribution. If a gate is not unitary — a dropped normalisation, a sign
    /// on the wrong half of H — this is what notices, on every fixture at once.
    #[test]
    fn every_distribution_sums_to_one() {
        for src in [
            "fn main() -> int { qubit a = |0>; qubit b = H(a); cbit r = measure(a); return 0; }",
            "fn main() -> int { qubit a = |0>; qubit b = |0>; qubit c = CNOT(H(a), b); cbit r = measure(a); cbit s = measure(b); return 0; }",
            "fn main() -> int { qubit a = |1>; qubit b = H(a); cbit r = measure(a); return 0; }",
        ] {
            let d = dist(src);
            let total: f64 = d.probs.iter().sum();
            assert!(close(total, 1.0), "{} sums to {}", src, total);
        }
    }

    /// A qubit that is never measured must not appear as a column in the outcome labels. Otherwise a
    /// 2-qubit circuit with one measurement shows four rows, two of which can never occur.
    #[test]
    fn only_measured_qubits_become_columns_of_the_outcome() {
        let d = dist(
            r#"
fn main() -> int {
    qubit a = |0>;
    qubit b = |0>;
    qubit c = CNOT(H(a), b);
    cbit r = measure(a);
    return 0;
}
"#,
        );
        assert_eq!(d.measured, vec![0]);
        assert_eq!(d.probs.len(), 2);
        assert!(close(d.probs[0], 0.5) && close(d.probs[1], 0.5), "{:?}", d.probs);
    }

    /// A gate this evaluator cannot honour must be REFUSED. The rotation gates lose their angle in
    /// `QirGate::from_ast_gate`, so treating `RX(0.0)` as the identity it literally is would print a
    /// confident distribution for a circuit whose rotations were silently skipped.
    #[test]
    fn a_gate_with_a_lost_angle_is_refused_rather_than_treated_as_identity() {
        let mut state = vec![C::ONE, C::ZERO];
        let err = apply(&mut state, &QirGate::RX(0.0), &[0]).unwrap_err();
        assert!(err.contains("angle"), "{}", err);
    }

    /// Labels read wire 0 first, left to right, so they line up with the circuit above them.
    #[test]
    fn labels_read_the_top_wire_first() {
        let d = Distribution { measured: vec![0, 1, 2], probs: vec![0.0; 8], branched: false };
        assert_eq!(d.label(0b100), "|100>", "bit for wire 0 must be leftmost");
        assert_eq!(d.label(0b001), "|001>");
    }

    /// Ranking is what puts the two outcomes that matter at the top of a 16-row table. Ties keep
    /// their basis order, so a uniform distribution does not shuffle between frames.
    #[test]
    fn ranking_orders_by_probability_and_keeps_ties_stable() {
        let d = Distribution { measured: vec![0, 1], probs: vec![0.1, 0.4, 0.0, 0.4], branched: false };
        assert_eq!(d.ranked(), vec![(1, 0.4), (3, 0.4), (0, 0.1), (2, 0.0)]);
    }
}
