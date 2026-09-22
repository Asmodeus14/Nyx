//! A state-vector simulator: exact amplitudes, exact distribution, sampled shots.
//!
//! ## Provenance
//!
//! This is a port of `tools/compiler/src/statevector.rs`, which has been running on this machine
//! inside `apps/qcstudio` since that app was written. It was already dependency-free, already used
//! its own two-field complex struct rather than `num_complex`, and already used plain `f64` —
//! deliberately, so that it would compile into an on-device window. That made it portable to
//! `no_std` almost unchanged, and the arithmetic below is the same arithmetic.
//!
//! ⚠️ **The port is cross-checked against the original**, gate for gate, in this crate's tests. If
//! the two ever disagree, the port is wrong: the original is the one that has been producing
//! histograms on real hardware.
//!
//! ## Two differences from the original, both deliberate
//!
//! **1. Rotations work here.** `statevector.rs` refuses `rx`/`ry`/`rz`/`u3` by name, and its comment
//! explains why: `QirGate::from_ast_gate` matches `AstGate::RX(expr)` and constructs
//! `QirGate::RX(0.0)`, *discarding the angle* — an upstream placeholder. Evaluating a gate whose
//! angle has been lost would silently compute a different program, so refusing was correct there.
//!
//! [`crate::Gate::Rx`] carries its angle, so there is nothing lost and no reason to refuse. That
//! means rotations cannot be part of the cross-check — the original cannot evaluate them to compare
//! against — and the tests say so where it matters.
//!
//! **2. Shots.** The original computes an exact marginal distribution, which is the right answer for
//! a diagram: a thousand sampled runs only estimate a number that can be computed exactly. But a QPU
//! backend has to be able to return what a *device* returns, which is a finite sample. So
//! [`sample`] draws from the exact distribution rather than simulating shot by shot — same
//! distribution, far cheaper, and the exact probabilities remain available.
//!
//! ## What it deliberately does not report
//!
//! **Fidelity, and any error metric.** There is no noise model here — no depolarising channel, no
//! gate-error rates, no decoherence, no readout error. An ideal state vector has fidelity 1 against
//! itself by construction, so any number other than `1.000` would have to be invented. The original
//! makes exactly this argument and it has not changed.
//!
//! ## Deferred measurement, and why [`Op::Reset`] is refused
//!
//! Measurements are not applied as collapses. The circuit is evolved unitarily to the end and the
//! result is the **marginal** distribution over the measured qubits. That is exact here rather than
//! an approximation, because [`crate::Op::Gate`] takes qubit indices only — no measured bit can
//! reach a gate argument, so there is no classical feedback, and the deferred-measurement principle
//! says a mid-circuit measurement is then indistinguishable from the same measurement at the end.
//!
//! [`crate::Op::Reset`] breaks that: it collapses unconditionally. So a circuit containing one is
//! **refused with a reason** rather than quietly given the wrong distribution — the same choice the
//! original made.

use crate::circuit::{Circuit, Gate, Op};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Above this a state vector stops fitting in a sensible amount of memory — 2²⁰ amplitudes is
/// 16 MB — and no circuit that large has a readable picture anyway.
///
/// ⚠️ This is also a **security** limit, not just an ergonomic one: the allocation is `2ⁿ` and `n`
/// comes from a circuit, which may be attacker-supplied. See `docs/quantum/security.md`.
pub const MAX_QUBITS: u8 = 20;

/// One complex amplitude.
///
/// A two-field struct rather than `num_complex::Complex<f64>` so this module carries no dependency —
/// the same decision, for the same reason, as the original.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct C {
    pub re: f64,
    pub im: f64,
}

impl C {
    pub const ZERO: C = C { re: 0.0, im: 0.0 };
    pub const ONE: C = C { re: 1.0, im: 0.0 };

    #[inline]
    pub fn add(self, o: C) -> C {
        C { re: self.re + o.re, im: self.im + o.im }
    }

    #[inline]
    pub fn mul(self, o: C) -> C {
        C {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }

    #[inline]
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// The exact distribution over a circuit's measured qubits.
#[derive(Clone, Debug, PartialEq)]
pub struct Distribution {
    /// The measured qubits in **ascending wire order** — not the order the measurements appear in
    /// the program.
    ///
    /// ⚠️ A basis-state label reads left to right as wire 0 first, so it lines up with the circuit
    /// drawn above it, top wire at the top. This is the opposite of Qiskit's convention, which
    /// prints the highest-index qubit first. Matching the diagram was judged more valuable than
    /// matching another tool, since the two are side by side on screen and only one can be right.
    /// Inherited verbatim from `qclang_compiler::statevector`.
    pub measured: Vec<u8>,
    /// `probs[i]` is the probability of the outcome whose bit `k` from the **left** of the label is
    /// `measured[k]`'s result. Length `1 << measured.len()`.
    pub probs: Vec<f64>,
}

impl Distribution {
    /// The basis-state label for outcome `i`, e.g. `"01"`.
    ///
    /// Bare digits, no ket brackets: these strings are map keys in [`crate::Readout`] and are
    /// compared against a provider's JSON keys, so they must be the plain bit string. Surfaces that
    /// want `|01⟩` add the decoration.
    pub fn label(&self, i: usize) -> String {
        let n = self.measured.len();
        let mut s = String::with_capacity(n);
        for k in 0..n {
            s.push(if (i >> (n - 1 - k)) & 1 == 1 { '1' } else { '0' });
        }
        s
    }

    /// `(label, probability)` for every outcome, in index order.
    pub fn labelled(&self) -> Vec<(String, f64)> {
        (0..self.probs.len()).map(|i| (self.label(i), self.probs[i])).collect()
    }
}

/// Evolve `c` from |0…0⟩ and return the exact distribution over its measured qubits.
///
/// `Err` carries a sentence fit to put on screen. Every failure is a real limitation of this
/// evaluator, and saying which one is the entire value of returning an error rather than a
/// plausible-looking number.
pub fn evaluate(c: &Circuit) -> Result<Distribution, String> {
    if let Err(e) = c.validate() {
        return Err(alloc::format!("{e}"));
    }
    if c.has_reset() {
        return Err(
            "this circuit resets a qubit, which collapses the state — no exact distribution"
                .to_string(),
        );
    }
    if c.qubits > MAX_QUBITS {
        return Err(alloc::format!(
            "{} qubits is too many to simulate exactly (limit {})",
            c.qubits,
            MAX_QUBITS
        ));
    }

    let n = c.qubits as usize;
    let mut state = vec![C::ZERO; 1usize << n];
    state[0] = C::ONE;

    for op in &c.ops {
        if let Op::Gate { gate, qubits } = op {
            apply(&mut state, *gate, qubits);
        }
    }

    let measured = c.measured_qubits();
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
            if (basis >> q) & 1 == 1 {
                out |= 1 << (m - 1 - k);
            }
        }
        probs[out] += p;
    }

    Ok(Distribution { measured, probs })
}

/// Draw `shots` samples from an exact distribution.
///
/// Sampling the closed-form distribution rather than re-simulating per shot: identical statistics,
/// and `O(shots · log outcomes)` instead of `O(shots · 2ⁿ · gates)`.
///
/// Returns `(label, count)` for every outcome that came up at least once, in index order. Outcomes
/// that never occurred are omitted rather than listed as zero — a 10-qubit circuit has 1024 rows
/// and typically two of them are non-empty.
pub fn sample(dist: &Distribution, shots: u32, seed: u64) -> Vec<(String, u64)> {
    let mut counts = vec![0u64; dist.probs.len()];
    if dist.probs.is_empty() {
        return Vec::new();
    }

    // Cumulative distribution, built once.
    let mut cum = vec![0.0f64; dist.probs.len()];
    let mut acc = 0.0;
    for (i, p) in dist.probs.iter().enumerate() {
        acc += *p;
        cum[i] = acc;
    }
    // Guard the last bucket against floating-point drift: the total should be 1.0, and if it is
    // 0.9999999 a draw above it would fall off the end of the search.
    if let Some(last) = cum.last_mut() {
        *last = 1.0;
    }

    let mut rng = SplitMix64::new(seed);
    for _ in 0..shots {
        let u = rng.next_f64();
        // Binary search for the first bucket whose cumulative probability exceeds u.
        let mut lo = 0usize;
        let mut hi = cum.len() - 1;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if u < cum[mid] {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        counts[lo] += 1;
    }

    (0..counts.len())
        .filter(|&i| counts[i] > 0)
        .map(|i| (dist.label(i), counts[i]))
        .collect()
}

/// Apply one gate in place.
///
/// Single-qubit unitaries all share one loop over disjoint index pairs, differing only in the 2×2
/// matrix — writing that loop once is what keeps a sign error from living in only one gate.
fn apply(state: &mut [C], gate: Gate, qubits: &[u8; 3]) {
    let q0 = qubits[0] as usize;

    let r = |v: f64| C { re: v, im: 0.0 };
    let im = |v: f64| C { re: 0.0, im: v };
    /// 1/√2, written out because `core` has no const sqrt.
    const ISQRT2: f64 = 0.707_106_781_186_547_52;

    match gate {
        Gate::H => one(state, q0, [r(ISQRT2), r(ISQRT2), r(ISQRT2), r(-ISQRT2)]),
        Gate::X => one(state, q0, [C::ZERO, C::ONE, C::ONE, C::ZERO]),
        Gate::Y => one(state, q0, [C::ZERO, im(-1.0), im(1.0), C::ZERO]),
        Gate::Z => one(state, q0, [C::ONE, C::ZERO, C::ZERO, r(-1.0)]),
        Gate::S => one(state, q0, [C::ONE, C::ZERO, C::ZERO, im(1.0)]),
        Gate::Sdg => one(state, q0, [C::ONE, C::ZERO, C::ZERO, im(-1.0)]),
        Gate::T => one(
            state,
            q0,
            [C::ONE, C::ZERO, C::ZERO, C { re: ISQRT2, im: ISQRT2 }],
        ),
        Gate::Tdg => one(
            state,
            q0,
            [C::ONE, C::ZERO, C::ZERO, C { re: ISQRT2, im: -ISQRT2 }],
        ),

        // Rx(θ) = [[cos(θ/2), -i·sin(θ/2)], [-i·sin(θ/2), cos(θ/2)]]
        Gate::Rx(theta) => {
            let (s, c) = trig::sin_cos(theta * 0.5);
            one(state, q0, [r(c), im(-s), im(-s), r(c)]);
        }
        // Ry(θ) = [[cos(θ/2), -sin(θ/2)], [sin(θ/2), cos(θ/2)]]
        Gate::Ry(theta) => {
            let (s, c) = trig::sin_cos(theta * 0.5);
            one(state, q0, [r(c), r(-s), r(s), r(c)]);
        }
        // Rz(θ) = diag(e^(-iθ/2), e^(iθ/2))
        Gate::Rz(theta) => {
            let (s, c) = trig::sin_cos(theta * 0.5);
            one(
                state,
                q0,
                [C { re: c, im: -s }, C::ZERO, C::ZERO, C { re: c, im: s }],
            );
        }
        // U3(θ,φ,λ) = [[cos(θ/2),            -e^(iλ)·sin(θ/2)],
        //              [e^(iφ)·sin(θ/2),  e^(i(φ+λ))·cos(θ/2)]]
        Gate::U3(theta, phi, lambda) => {
            let (st, ct) = trig::sin_cos(theta * 0.5);
            let (sl, cl) = trig::sin_cos(lambda);
            let (sp, cp) = trig::sin_cos(phi);
            let (spl, cpl) = trig::sin_cos(phi + lambda);
            let m01 = C { re: -cl * st, im: -sl * st };
            let m10 = C { re: cp * st, im: sp * st };
            let m11 = C { re: cpl * ct, im: spl * ct };
            one(state, q0, [r(ct), m01, m10, m11]);
        }

        Gate::Cx => {
            let (control, target) = (q0, qubits[1] as usize);
            for i in 0..state.len() {
                if (i >> control) & 1 == 1 && (i >> target) & 1 == 0 {
                    state.swap(i, i | 1 << target);
                }
            }
        }
        Gate::Swap => {
            let (a, b) = (q0, qubits[1] as usize);
            for i in 0..state.len() {
                if (i >> a) & 1 == 1 && (i >> b) & 1 == 0 {
                    state.swap(i, (i & !(1 << a)) | 1 << b);
                }
            }
        }
        Gate::Ccx => {
            let (c0, c1, t) = (q0, qubits[1] as usize, qubits[2] as usize);
            for i in 0..state.len() {
                if (i >> c0) & 1 == 1 && (i >> c1) & 1 == 1 && (i >> t) & 1 == 0 {
                    state.swap(i, i | 1 << t);
                }
            }
        }
    }
}

/// Apply a 2×2 matrix `[m00, m01, m10, m11]` to qubit `t`.
#[inline]
fn one(state: &mut [C], t: usize, m: [C; 4]) {
    for i in 0..state.len() {
        if (i >> t) & 1 != 0 {
            continue;
        }
        let j = i | 1 << t;
        let (a, b) = (state[i], state[j]);
        state[i] = m[0].mul(a).add(m[1].mul(b));
        state[j] = m[2].mul(a).add(m[3].mul(b));
    }
}

/// SplitMix64 — a small, fast, well-tested PRNG.
///
/// Used for shot sampling only. Not cryptographic and not used for anything that needs to be: the
/// question it answers is "which of these outcomes came up", and the caller supplies the seed so
/// the whole thing is reproducible in tests. Chosen because it is about five lines and needs no
/// dependency.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> SplitMix64 {
        SplitMix64(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`, using the top 53 bits — the number of bits an `f64` mantissa holds.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}

/// Sine and cosine, because `core` has neither and this crate has no dependencies.
///
/// `libs/gui` pulls `libm` for exactly this reason, so taking that dependency would have followed
/// tree precedent. It is not taken here because the device model in this crate is linked by the
/// kernel-adjacent side of the syscall boundary, and forty lines of range-reduced Taylor series with
/// known-answer tests is cheaper to audit than a maths library.
///
/// ⚠️ Accuracy degrades for very large `|x|`, as it does for every implementation that reduces by
/// subtracting a multiple of 2π. Gate angles are radians in the neighbourhood of ±2π; this is not a
/// general-purpose maths routine.
mod trig {
    const PI: f64 = core::f64::consts::PI;
    const TWO_PI: f64 = 2.0 * PI;
    const HALF_PI: f64 = PI / 2.0;

    /// `x.round()` — `f64::round` is `std`-only.
    fn nearest(x: f64) -> f64 {
        let t = (x as i64) as f64;
        let d = x - t;
        if d >= 0.5 {
            t + 1.0
        } else if d <= -0.5 {
            t - 1.0
        } else {
            t
        }
    }

    /// Taylor series for sine, valid to near machine precision on `[-π/2, π/2]`.
    ///
    /// Horner form. Terms run to `x²¹/21!`, which is what `f64` needs at the top of that range —
    /// the `x¹⁹` term alone is still ~4e-14 at π/2.
    fn sin_small(x: f64) -> f64 {
        let x2 = x * x;
        let mut s = -1.0 / 51_090_942_171_709_440_000.0; // 21!
        s = s * x2 + 1.0 / 121_645_100_408_832_000.0; // 19!
        s = s * x2 - 1.0 / 355_687_428_096_000.0; // 17!
        s = s * x2 + 1.0 / 1_307_674_368_000.0; // 15!
        s = s * x2 - 1.0 / 6_227_020_800.0; // 13!
        s = s * x2 + 1.0 / 39_916_800.0; // 11!
        s = s * x2 - 1.0 / 362_880.0; // 9!
        s = s * x2 + 1.0 / 5_040.0; // 7!
        s = s * x2 - 1.0 / 120.0; // 5!
        s = s * x2 + 1.0 / 6.0; // 3!
        x - x * x2 * s
    }

    /// Taylor series for cosine on `[-π/2, π/2]`.
    fn cos_small(x: f64) -> f64 {
        let x2 = x * x;
        let mut s = 1.0 / 2_432_902_008_176_640_000.0; // 20!
        s = s * x2 - 1.0 / 6_402_373_705_728_000.0; // 18!
        s = s * x2 + 1.0 / 20_922_789_888_000.0; // 16!
        s = s * x2 - 1.0 / 87_178_291_200.0; // 14!
        s = s * x2 + 1.0 / 479_001_600.0; // 12!
        s = s * x2 - 1.0 / 3_628_800.0; // 10!
        s = s * x2 + 1.0 / 40_320.0; // 8!
        s = s * x2 - 1.0 / 720.0; // 6!
        s = s * x2 + 1.0 / 24.0; // 4!
        s = s * x2 - 1.0 / 2.0; // 2!
        1.0 + x2 * s
    }

    /// `(sin x, cos x)`.
    ///
    /// Both at once because every caller here needs both, and the range reduction is the expensive
    /// part.
    pub fn sin_cos(x: f64) -> (f64, f64) {
        // Reduce to [-π, π].
        let k = nearest(x / TWO_PI);
        let r = x - k * TWO_PI;

        // Fold into [-π/2, π/2], tracking the sign change each fold implies.
        let (sr, cr) = if r > HALF_PI {
            let t = PI - r;
            (sin_small(t), -cos_small(t))
        } else if r < -HALF_PI {
            let t = -PI - r;
            (sin_small(t), -cos_small(t))
        } else {
            (sin_small(r), cos_small(r))
        };
        (sr, cr)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Known-answer tests. These are the whole justification for not taking a dependency.
        #[test]
        fn matches_known_values() {
            let cases: [(f64, f64, f64); 9] = [
                (0.0, 0.0, 1.0),
                (HALF_PI, 1.0, 0.0),
                (PI, 0.0, -1.0),
                (-HALF_PI, -1.0, 0.0),
                (PI + HALF_PI, -1.0, 0.0),
                (TWO_PI, 0.0, 1.0),
                (PI / 6.0, 0.5, 0.866_025_403_784_438_6),
                (PI / 4.0, 0.707_106_781_186_547_5, 0.707_106_781_186_547_5),
                (PI / 3.0, 0.866_025_403_784_438_6, 0.5),
            ];
            for (x, s, c) in cases {
                let (gs, gc) = sin_cos(x);
                assert!((gs - s).abs() < 1e-14, "sin({x}) = {gs}, want {s}");
                assert!((gc - c).abs() < 1e-14, "cos({x}) = {gc}, want {c}");
            }
        }

        #[test]
        fn identity_holds_across_the_reduction_boundaries() {
            // Walk well past ±2π so the range reduction is exercised in both directions.
            let mut x = -20.0;
            while x < 20.0 {
                let (s, c) = sin_cos(x);
                assert!((s * s + c * c - 1.0).abs() < 1e-13, "sin²+cos² failed at {x}");
                x += 0.037;
            }
        }

        #[test]
        fn is_odd_and_even_respectively() {
            for x in [0.3, 1.1, 2.9, 4.7, 6.9] {
                let (sp, cp) = sin_cos(x);
                let (sn, cn) = sin_cos(-x);
                assert!((sp + sn).abs() < 1e-14, "sin is odd");
                assert!((cp - cn).abs() < 1e-14, "cos is even");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::Gate;

    fn p(d: &Distribution, label: &str) -> f64 {
        d.labelled().into_iter().find(|(l, _)| l == label).map(|(_, v)| v).unwrap_or(0.0)
    }

    fn assert_normalised(d: &Distribution) {
        let total: f64 = d.probs.iter().sum();
        assert!((total - 1.0).abs() < 1e-12, "probabilities sum to {total}, not 1");
    }

    #[test]
    fn bell_state_is_perfectly_correlated() {
        let d = evaluate(&Circuit::bell()).unwrap();
        assert_normalised(&d);
        assert!((p(&d, "00") - 0.5).abs() < 1e-12);
        assert!((p(&d, "11") - 0.5).abs() < 1e-12);
        // ★ The whole point of a Bell pair. Anti-correlated outcomes are impossible.
        assert_eq!(p(&d, "01"), 0.0);
        assert_eq!(p(&d, "10"), 0.0);
    }

    #[test]
    fn ghz_states_are_all_zeros_or_all_ones() {
        for n in 2..=6u8 {
            let d = evaluate(&Circuit::ghz(n)).unwrap();
            assert_normalised(&d);
            let zeros = "0".repeat(n as usize);
            let ones = "1".repeat(n as usize);
            assert!((p(&d, &zeros) - 0.5).abs() < 1e-12, "ghz({n})");
            assert!((p(&d, &ones) - 0.5).abs() < 1e-12, "ghz({n})");
            // Everything else must be exactly impossible, not merely unlikely.
            let others: f64 = d.probs.iter().sum::<f64>() - p(&d, &zeros) - p(&d, &ones);
            assert!(others.abs() < 1e-12, "ghz({n}) has weight on a mixed state");
        }
    }

    #[test]
    fn x_flips_and_h_twice_is_the_identity() {
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::X, &[0]).measure(0, 0);
        let d = evaluate(&c).unwrap();
        assert!((p(&d, "1") - 1.0).abs() < 1e-12);

        let mut c = Circuit::new(1, 1);
        c.gate(Gate::H, &[0]).gate(Gate::H, &[0]).measure(0, 0);
        let d = evaluate(&c).unwrap();
        assert!((p(&d, "0") - 1.0).abs() < 1e-12, "H·H must be the identity");
    }

    #[test]
    fn adjoints_undo_their_gates() {
        for (g, gdg) in [(Gate::S, Gate::Sdg), (Gate::T, Gate::Tdg)] {
            let mut c = Circuit::new(1, 1);
            // Sandwich in Hadamards so a phase error actually shows up in the probabilities.
            c.gate(Gate::H, &[0]).gate(g, &[0]).gate(gdg, &[0]).gate(Gate::H, &[0]).measure(0, 0);
            let d = evaluate(&c).unwrap();
            assert!((p(&d, "0") - 1.0).abs() < 1e-12, "{:?} then {:?}", g, gdg);
        }
    }

    #[test]
    fn t_gate_composes_into_s() {
        // T·T = S, visible through an interference circuit.
        let mut a = Circuit::new(1, 1);
        a.gate(Gate::H, &[0]).gate(Gate::T, &[0]).gate(Gate::T, &[0]).gate(Gate::H, &[0]).measure(0, 0);
        let mut b = Circuit::new(1, 1);
        b.gate(Gate::H, &[0]).gate(Gate::S, &[0]).gate(Gate::H, &[0]).measure(0, 0);
        let (da, db) = (evaluate(&a).unwrap(), evaluate(&b).unwrap());
        assert!((p(&da, "0") - p(&db, "0")).abs() < 1e-12);
    }

    #[test]
    fn swap_exchanges_wires() {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::X, &[0]).gate(Gate::Swap, &[0, 1]).measure_all();
        let d = evaluate(&c).unwrap();
        // Wire 0 leftmost: the excitation moved from wire 0 to wire 1.
        assert!((p(&d, "01") - 1.0).abs() < 1e-12);
    }

    #[test]
    fn toffoli_flips_only_when_both_controls_are_set() {
        for (a, b, want) in [(0u8, 0u8, "000"), (1, 0, "100"), (0, 1, "010"), (1, 1, "111")] {
            let mut c = Circuit::new(3, 3);
            if a == 1 {
                c.gate(Gate::X, &[0]);
            }
            if b == 1 {
                c.gate(Gate::X, &[1]);
            }
            c.gate(Gate::Ccx, &[0, 1, 2]).measure_all();
            let d = evaluate(&c).unwrap();
            assert!((p(&d, want) - 1.0).abs() < 1e-12, "ccx({a},{b}) should give {want}");
        }
    }

    #[test]
    fn cnot_direction_matters() {
        // X on the control propagates; X on the target does not.
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::X, &[0]).gate(Gate::Cx, &[0, 1]).measure_all();
        assert!((p(&evaluate(&c).unwrap(), "11") - 1.0).abs() < 1e-12);

        let mut c = Circuit::new(2, 2);
        c.gate(Gate::X, &[1]).gate(Gate::Cx, &[0, 1]).measure_all();
        assert!((p(&evaluate(&c).unwrap(), "01") - 1.0).abs() < 1e-12);
    }

    /// Rotations are the one place this port goes beyond the original, which refuses them because
    /// its IR loses the angle. So they get their own coverage.
    #[test]
    fn rotations_use_their_angles() {
        use core::f64::consts::PI;

        // Rx(π) is X up to a global phase.
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::Rx(PI), &[0]).measure(0, 0);
        assert!((p(&evaluate(&c).unwrap(), "1") - 1.0).abs() < 1e-12);

        // Ry(π/2) makes an even superposition.
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::Ry(PI / 2.0), &[0]).measure(0, 0);
        let d = evaluate(&c).unwrap();
        assert!((p(&d, "0") - 0.5).abs() < 1e-12);
        assert!((p(&d, "1") - 0.5).abs() < 1e-12);

        // Rx(0) is the identity — the case that would pass even if angles were being discarded,
        // which is exactly why the π case above is the real test.
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::Rx(0.0), &[0]).measure(0, 0);
        assert!((p(&evaluate(&c).unwrap(), "0") - 1.0).abs() < 1e-12);

        // Rz is diagonal: invisible in the computational basis, visible between Hadamards.
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::H, &[0]).gate(Gate::Rz(PI), &[0]).gate(Gate::H, &[0]).measure(0, 0);
        assert!((p(&evaluate(&c).unwrap(), "1") - 1.0).abs() < 1e-12);
    }

    #[test]
    fn u3_reproduces_the_gates_it_generalises() {
        use core::f64::consts::PI;
        // U3(π, 0, π) == X.
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::U3(PI, 0.0, PI), &[0]).measure(0, 0);
        assert!((p(&evaluate(&c).unwrap(), "1") - 1.0).abs() < 1e-12);

        // U3(π/2, 0, π) == H.
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::U3(PI / 2.0, 0.0, PI), &[0]).gate(Gate::U3(PI / 2.0, 0.0, PI), &[0]).measure(0, 0);
        assert!((p(&evaluate(&c).unwrap(), "0") - 1.0).abs() < 1e-12, "H·H via U3");
    }

    #[test]
    fn unmeasured_circuits_have_a_single_empty_outcome() {
        let mut c = Circuit::new(2, 0);
        c.gate(Gate::H, &[0]);
        let d = evaluate(&c).unwrap();
        assert_eq!(d.measured.len(), 0);
        assert_eq!(d.probs.len(), 1);
        assert!((d.probs[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn partial_measurement_marginalises() {
        // Bell pair, but only wire 0 is measured: an even coin, no correlation visible.
        let mut c = Circuit::new(2, 1);
        c.gate(Gate::H, &[0]).gate(Gate::Cx, &[0, 1]).measure(0, 0);
        let d = evaluate(&c).unwrap();
        assert_eq!(d.measured, alloc::vec![0]);
        assert!((p(&d, "0") - 0.5).abs() < 1e-12);
        assert!((p(&d, "1") - 0.5).abs() < 1e-12);
    }

    #[test]
    fn reset_is_refused_by_name_not_silently_ignored() {
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::H, &[0]).reset(0).measure(0, 0);
        let e = evaluate(&c).unwrap_err();
        assert!(e.contains("reset"), "{e}");
    }

    #[test]
    fn the_qubit_limit_is_enforced_with_a_reason() {
        let c = Circuit::new(MAX_QUBITS + 1, 0);
        let e = evaluate(&c).unwrap_err();
        assert!(e.contains("too many"), "{e}");
        // The limit itself must still be accepted.
        assert!(evaluate(&Circuit::new(MAX_QUBITS, 0)).is_ok());
    }

    #[test]
    fn an_invalid_circuit_is_refused_before_any_allocation() {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::Cx, &[0, 0]);
        assert!(evaluate(&c).is_err());
    }

    // ── sampling ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn sampling_is_deterministic_for_a_given_seed() {
        let d = evaluate(&Circuit::bell()).unwrap();
        assert_eq!(sample(&d, 500, 0xABCD), sample(&d, 500, 0xABCD));
        assert_ne!(sample(&d, 500, 1), sample(&d, 500, 2));
    }

    #[test]
    fn sampling_conserves_the_shot_count() {
        let d = evaluate(&Circuit::ghz(3)).unwrap();
        for shots in [0u32, 1, 7, 1024] {
            let total: u64 = sample(&d, shots, 99).iter().map(|(_, n)| *n).sum();
            assert_eq!(total, shots as u64);
        }
    }

    /// ★ Sampling must never produce an outcome the distribution says is impossible. A binary
    /// search that is off by one at the bucket boundary would show up exactly here.
    #[test]
    fn sampling_never_invents_an_impossible_outcome() {
        let d = evaluate(&Circuit::bell()).unwrap();
        for seed in 0..40u64 {
            for (label, _) in sample(&d, 200, seed) {
                assert!(label == "00" || label == "11", "sampled impossible outcome {label}");
            }
        }
    }

    #[test]
    fn sampling_approximates_the_distribution() {
        let d = evaluate(&Circuit::bell()).unwrap();
        let s = sample(&d, 20_000, 0x5EED);
        for (label, n) in &s {
            let frac = *n as f64 / 20_000.0;
            assert!(
                (frac - 0.5).abs() < 0.02,
                "{label} came up {frac} of the time, expected ~0.5"
            );
        }
    }

    #[test]
    fn a_deterministic_circuit_samples_deterministically() {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::X, &[0]).measure_all();
        let d = evaluate(&c).unwrap();
        let s = sample(&d, 1000, 7);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0], ("10".to_string(), 1000));
    }
}
