//! The local state-vector simulator as a [`QpuBackend`].
//!
//! ## This type cannot lie about what it is
//!
//! [`SimBackend::info`] hardcodes [`QpuStatus::Simulator`]. There is no setter, no constructor
//! argument, and no `#[cfg]` that changes it. That is the enforcement mechanism for the subsystem's
//! central rule — see `docs/quantum/limitations.md` — and it is deliberately structural rather than
//! a convention someone has to remember:
//!
//! > Never report `HARDWARE` when Nyx is not driving physically attached quantum hardware.
//!
//! [`QpuStatus::Hardware`] is produced in exactly one place in the entire tree: the kernel's PCI
//! probe, on a match against a real device. Nothing in userspace can construct it.
//!
//! ## Why it still implements the `begin`/`poll` lifecycle
//!
//! The simulator is synchronous — [`SimBackend::begin`] does all the work and [`SimBackend::poll`]
//! returns [`JobState::Done`] on the first call. It could have had a simpler interface.
//!
//! It does not, because the interface is the point: an application written against `QpuSession` must
//! not behave differently depending on which backend served it. If the simulator were the only
//! backend with a blocking API, every caller would grow a special case, and the first time a cloud
//! provider was substituted the window would freeze. Uniformity here is what makes the backend
//! genuinely swappable.

use crate::backend::{
    JobHandle, JobState, Outcome, Provenance, QpuBackend, QpuError, Readout, Shots,
};
use crate::circuit::{Circuit, Gate};
use crate::device::{ExecModel, QpuInfo, QpuStatus, QuantumDeviceKind, Topology};
use crate::sim::{self, MAX_QUBITS};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// Every gate [`crate::sim`] implements — which, deliberately, is all of them.
fn supported_gates() -> u64 {
    crate::circuit::gate_set_mask(&Gate::ALL)
}

/// The local, classical, exact state-vector simulator.
pub struct SimBackend {
    next: u64,
    /// Finished jobs waiting to be collected. Small: a job is removed on the `poll` that returns it.
    done: Vec<(JobHandle, Result<Outcome, QpuError>)>,
}

impl SimBackend {
    pub fn new() -> SimBackend {
        SimBackend { next: 1, done: Vec::new() }
    }

    /// The device description this backend advertises.
    ///
    /// Public so the kernel-free paths (`quantum devices` with no hardware and no provider) can
    /// describe the simulator without constructing a backend.
    pub fn device_info(id: u32) -> QpuInfo {
        let mut i = QpuInfo {
            id,
            kind: QuantumDeviceKind::Qpu as u32,
            status: QpuStatus::Simulator as u32,
            remote_is_simulator: 0,
            qubits: MAX_QUBITS as u32,
            topology: Topology::AllToAll as u32,
            exec_model: ExecModel::Batch as u32,
            gate_set: supported_gates(),
            max_shots: 1_000_000,
            // A depth limit would be arbitrary here — simulation cost scales with 2^qubits, not with
            // depth — so none is claimed. `0` means "not reported", per the device model.
            max_depth: 0,
            // No queue: work is done inline on the calling thread. Reporting 0 would claim an empty
            // queue exists; u32::MAX is the model's "not applicable".
            queue_depth: u32::MAX,
            queue_capacity: 0,
            // No noise model, so no coherence times. Inventing them would be the exact failure this
            // subsystem exists to avoid.
            coherence_t1_ns: 0,
            coherence_t2_ns: 0,
            calibrated_unix: 0,
            ..QpuInfo::default()
        };
        i.set_vendor("Nyx");
        i.set_arch("statevector");
        i.set_name("Nyx state-vector simulator");
        i
    }
}

impl Default for SimBackend {
    fn default() -> SimBackend {
        SimBackend::new()
    }
}

impl QpuBackend for SimBackend {
    fn name(&self) -> &str {
        "simulator"
    }

    fn describe(&self) -> &str {
        "exact state-vector simulation on this CPU; classical, not a quantum processor"
    }

    fn info(&self) -> QpuInfo {
        // ★ Hardcoded. See the module docs — this is the enforcement point.
        SimBackend::device_info(0)
    }

    fn begin(&mut self, c: &Circuit, shots: Shots, seed: u64) -> Result<JobHandle, QpuError> {
        // Re-validate rather than trusting that `run` did it: `begin` is public and a caller may
        // reach it directly. The limits it enforces are the ones that stop a 2^n allocation from
        // being attacker-controlled.
        self.validate(c, shots)?;

        let h = JobHandle(self.next);
        self.next += 1;

        let result = sim::evaluate(c)
            .map_err(QpuError::Unsupported)
            .map(|dist| {
                let counts = sim::sample(&dist, shots.0, seed);
                Outcome {
                    readout: Readout::Counts(counts),
                    shots_requested: shots.0,
                    measured: dist.measured.clone(),
                    provenance: Provenance::from_info("simulator", &self.info()),
                }
            });

        self.done.push((h, result));
        Ok(h)
    }

    fn poll(&mut self, h: JobHandle) -> JobState {
        match self.done.iter().position(|(k, _)| *k == h) {
            Some(i) => match self.done.remove(i).1 {
                Ok(o) => JobState::Done(Box::new(o)),
                Err(e) => JobState::Failed(e),
            },
            // Either the handle was never issued, or its result has already been collected. Both
            // are caller errors and both are better reported than silently spun on forever.
            None => JobState::Failed(QpuError::NoSuchJob(h)),
        }
    }

    fn cancel(&mut self, h: JobHandle) {
        self.done.retain(|(k, _)| *k != h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::Gate;

    /// ★ The invariant this whole file exists for.
    #[test]
    fn the_simulator_can_never_claim_to_be_hardware() {
        let s = SimBackend::new();
        let i = s.info();
        assert_eq!(i.status(), QpuStatus::Simulator);
        assert_ne!(i.status(), QpuStatus::Hardware);
        assert!(!i.is_quantum());
        assert_eq!(i.status_label(), "SIMULATOR");
        assert!(i.caveat().unwrap().contains("not a quantum processor"));
    }

    #[test]
    fn it_reports_no_numbers_it_cannot_measure() {
        let i = SimBackend::new().info();
        // No noise model means no coherence times and no calibration date.
        assert_eq!(i.coherence_t1(), None);
        assert_eq!(i.coherence_t2(), None);
        assert_eq!(i.calibrated(), None);
        // No queue exists; "not applicable" is not the same claim as "empty".
        assert_eq!(i.queue(), None);
    }

    #[test]
    fn bell_state_runs_end_to_end_with_honest_provenance() {
        let mut s = SimBackend::new();
        let o = s.run(&Circuit::bell(), Shots(1024), 0x5EED).unwrap();

        assert!(o.has_counts(), "the simulator really did draw shots");
        assert_eq!(o.counts_for("00").unwrap() + o.counts_for("11").unwrap(), 1024);
        assert_eq!(o.counts_for("01"), Some(0));
        assert_eq!(o.counts_for("10"), Some(0));
        assert_eq!(o.measured, alloc::vec![0, 1]);

        assert!(!o.provenance.is_quantum);
        assert_eq!(o.provenance.backend, "simulator");
        assert!(o.provenance.disclosure().contains("no quantum hardware was used"));
    }

    #[test]
    fn limits_are_enforced_before_anything_is_allocated() {
        let mut s = SimBackend::new();

        // 2^21 amplitudes is the allocation this refusal prevents.
        let big = Circuit::new(MAX_QUBITS + 1, 0);
        assert!(matches!(
            s.validate(&big, Shots(1)),
            Err(QpuError::TooManyQubits { requested: 21, available: 20 })
        ));
        assert!(s.begin(&big, Shots(1), 0).is_err());

        let mut c = Circuit::bell();
        c.cbits = 2;
        assert!(matches!(
            s.validate(&c, Shots(2_000_000)),
            Err(QpuError::TooManyShots { .. })
        ));
    }

    #[test]
    fn a_malformed_circuit_is_rejected_with_its_reason() {
        let mut s = SimBackend::new();
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::Cx, &[0, 0]);
        match s.validate(&c, Shots(8)) {
            Err(QpuError::Circuit(_)) => {}
            other => panic!("expected a circuit error, got {other:?}"),
        }
    }

    #[test]
    fn reset_is_refused_with_an_explanation_rather_than_a_wrong_answer() {
        let mut s = SimBackend::new();
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::H, &[0]).reset(0).measure(0, 0);
        match s.run(&c, Shots(8), 0) {
            Err(QpuError::Unsupported(why)) => assert!(why.contains("reset"), "{why}"),
            other => panic!("expected an explained refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_job_lifecycle_behaves_like_an_async_backend() {
        let mut s = SimBackend::new();
        let h = s.begin(&Circuit::bell(), Shots(16), 1).unwrap();
        assert!(matches!(s.poll(h), JobState::Done(_)));
        // Collected once: a second poll is an error, not an infinite wait.
        assert!(matches!(s.poll(h), JobState::Failed(QpuError::NoSuchJob(_))));
    }

    #[test]
    fn handles_are_distinct_and_cancel_is_idempotent() {
        let mut s = SimBackend::new();
        let a = s.begin(&Circuit::bell(), Shots(4), 1).unwrap();
        let b = s.begin(&Circuit::bell(), Shots(4), 2).unwrap();
        assert_ne!(a, b);

        s.cancel(a);
        s.cancel(a); // idempotent
        s.cancel(JobHandle(9999)); // unknown handle is not an error
        assert!(matches!(s.poll(a), JobState::Failed(QpuError::NoSuchJob(_))));
        assert!(matches!(s.poll(b), JobState::Done(_)));
    }

    #[test]
    fn it_advertises_exactly_the_gates_it_implements() {
        let i = SimBackend::new().info();
        for g in Gate::ALL {
            assert!(
                crate::circuit::gate_set_contains(i.gate_set, g),
                "{} is implemented but not advertised",
                g.qasm_name()
            );
        }
        // And a circuit using all of them passes validation.
        let mut c = Circuit::new(3, 0);
        for g in Gate::ALL {
            let qs: alloc::vec::Vec<u8> = (0..g.arity() as u8).collect();
            c.gate(g, &qs);
        }
        assert!(SimBackend::new().validate(&c, Shots(1)).is_ok());
    }
}
