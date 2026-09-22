//! The backend trait, the job lifecycle, and the result type.
//!
//! Modelled deliberately on `libs/toolchains::Toolchain` / `Registry` / `Artifact` — the one plugin
//! registry in this tree that has tests, and whose design note says *"adding a backend is a one-line
//! change in `with_defaults` — the whole point of the design."* Same shape here, for the same
//! reason, and with the same trick: [`QpuBackend`] has variants of backend it does not yet have
//! implementations for, declared before anything produces them, so the trait does not have to change
//! when one lands.
//!
//! ## The job lifecycle is `poll`-shaped, and that is not an accident
//!
//! There is no async in the Nyx kernel. Every syscall is synchronous; the GPU submits and blocks on
//! a polled memory fence. The only non-blocking idiom in the whole tree is a *userspace* state
//! machine — `libs/net`'s `Fetch`, whose `poll()` does a bounded amount of work and is pumped from
//! an application's `update()`.
//!
//! Quantum jobs are that shape and more so: a cloud submission sits in a provider's queue for
//! minutes. And `apps/shell` **is** the window server, so anything it calls that blocks freezes the
//! desktop. Hence [`QpuBackend::begin`] + [`QpuBackend::poll`] rather than a `run()` that blocks.
//!
//! [`QpuBackend::run`] exists as a convenience for tests and for the simulator, and is documented as
//! blocking. Nothing in an interactive path should call it.
//!
//! ## ⚠️ Counts and probabilities are different facts
//!
//! This is the single most important thing in this module.
//!
//! The local simulator samples shots and therefore reports **counts**: it really did draw 1024
//! outcomes. IonQ's API returns a **histogram of probabilities** — `{"0": 0.5, "3": 0.5}` — and does
//! not tell you how many shots produced each.
//!
//! Multiplying `0.5 × 1024` into "512 shots" would be inventing a measurement that was never
//! reported. So [`Readout`] is an enum and there is no lossy conversion between its arms:
//! [`Outcome::counts_for`] returns `Option` and is `None` for a probability readout. A caller that
//! wants a number for display has to acknowledge which kind it is getting.
//!
//! This is the same discipline `apps/sysmon` applies when it refuses to draw a GPU percentage it
//! cannot measure.

use crate::circuit::{Circuit, CircuitError};
use crate::device::{QpuInfo, QpuStatus};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// How many times to run the circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Shots(pub u32);

impl Default for Shots {
    fn default() -> Shots {
        Shots(1024)
    }
}

/// An opaque, backend-scoped job identity.
///
/// Not globally unique and not meaningful across backends — the simulator numbers its own jobs and
/// so does each provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobHandle(pub u64);

/// What a job is doing right now.
///
/// Named rather than numbered because these strings reach the screen, and because — following
/// `Fetch`'s `announced` field — a stage must be *reported before it is executed*, or the label
/// explaining a pause arrives after the pause it was meant to explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Checking the circuit against the device's limits.
    Validating,
    /// Handing the circuit to the backend. For a provider, the HTTP POST.
    Submitting,
    /// Accepted, waiting its turn. `Option<u32>` is the queue position if the backend reports one.
    Queued(Option<u32>),
    /// Executing.
    Running,
    /// Collecting results. For a provider, the results GET.
    Fetching,
}

impl Stage {
    /// The lowercase label printed on a status line.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Validating => "validating",
            Stage::Submitting => "submitting",
            Stage::Queued(_) => "queued",
            Stage::Running => "running",
            Stage::Fetching => "fetching results",
        }
    }
}

/// The result of one `poll`.
#[derive(Debug, Clone)]
pub enum JobState {
    /// Still working. Carries the stage to display.
    Running(Stage),
    /// Finished successfully.
    Done(Box<Outcome>),
    /// Finished unsuccessfully.
    Failed(QpuError),
}

/// Where a result came from — and whether it was produced by quantum mechanics.
///
/// ⚠️ Every display of an [`Outcome`] must show this. An application is deliberately unable to tell
/// which backend served it from the API alone; this is the channel through which it finds out, and
/// it exists so the answer travels *with* the numbers rather than being reconstructed later from
/// context that may have changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The backend that ran it — `"simulator"`, `"ionq"`.
    pub backend: String,
    /// The specific device — `"Nyx statevector"`, `"qpu.aria-1"`.
    pub device: String,
    pub status: QpuStatus,
    /// Only meaningful when `status == Remote`.
    pub remote_is_simulator: bool,
    /// Pre-computed [`QpuStatus::is_quantum`], so a caller cannot forget the remote-simulator case.
    pub is_quantum: bool,
}

impl Provenance {
    pub fn from_info(backend: &str, info: &QpuInfo) -> Provenance {
        Provenance {
            backend: backend.to_string(),
            device: info.name_str().to_string(),
            status: info.status(),
            remote_is_simulator: info.remote_is_simulator(),
            is_quantum: info.is_quantum(),
        }
    }

    /// The one-line statement printed under every result.
    ///
    /// Deliberately a complete sentence rather than a tag: this is the line that stops a simulated
    /// histogram from being mistaken for a measurement, and it has to survive being read quickly.
    pub fn disclosure(&self) -> &'static str {
        match (self.status, self.remote_is_simulator) {
            (QpuStatus::Hardware, _) => "executed on local quantum hardware",
            (QpuStatus::Remote, false) => "executed on remote quantum hardware",
            (QpuStatus::Remote, true) => {
                "executed on a REMOTE CLASSICAL SIMULATOR; no quantum hardware was used"
            }
            (QpuStatus::Simulator, _) => {
                "executed on a classical simulator; no quantum hardware was used"
            }
            (QpuStatus::NotPresent, _) => "no device executed this",
        }
    }
}

/// Measurement results, in whichever form the backend actually reported them.
///
/// ⚠️ There is no conversion from [`Readout::Probabilities`] to [`Readout::Counts`]. See the module
/// docs — the arithmetic is trivial and the claim it would make is false.
#[derive(Debug, Clone, PartialEq)]
pub enum Readout {
    /// Outcomes actually drawn, `(basis-state label, count)`. The simulator produces these.
    Counts(Vec<(String, u64)>),
    /// Probabilities as reported by the backend, `(basis-state label, p)`. IonQ produces these.
    Probabilities(Vec<(String, f64)>),
}

/// A completed measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub readout: Readout,
    /// The shot count that was **asked for**. For a probability readout this is not a claim about
    /// how the provider produced its numbers.
    pub shots_requested: u32,
    /// The measured qubits, ascending, matching the label order. See
    /// [`Circuit::measured_qubits`](crate::Circuit::measured_qubits).
    pub measured: Vec<u8>,
    pub provenance: Provenance,
}

impl Outcome {
    /// How many times `label` was actually measured.
    ///
    /// `None` when the backend reported probabilities rather than counts — the honest answer, since
    /// in that case nobody knows.
    pub fn counts_for(&self, label: &str) -> Option<u64> {
        match &self.readout {
            Readout::Counts(v) => {
                Some(v.iter().find(|(l, _)| l == label).map(|(_, c)| *c).unwrap_or(0))
            }
            Readout::Probabilities(_) => None,
        }
    }

    /// The probability of `label`, derived from counts or taken as reported. `0.0` if absent.
    pub fn probability_for(&self, label: &str) -> f64 {
        match &self.readout {
            Readout::Counts(v) => {
                let total: u64 = v.iter().map(|(_, c)| *c).sum();
                if total == 0 {
                    return 0.0;
                }
                let c = v.iter().find(|(l, _)| l == label).map(|(_, c)| *c).unwrap_or(0);
                c as f64 / total as f64
            }
            Readout::Probabilities(v) => {
                v.iter().find(|(l, _)| l == label).map(|(_, p)| *p).unwrap_or(0.0)
            }
        }
    }

    /// `(label, probability)` sorted by descending probability, then by label.
    ///
    /// Long tails of near-zero states are the norm — a 4-qubit circuit has 16 rows and usually two
    /// of them matter — so callers generally want this rather than the raw readout.
    pub fn ranked(&self) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> = match &self.readout {
            Readout::Counts(c) => {
                let total: u64 = c.iter().map(|(_, n)| *n).sum();
                let d = if total == 0 { 1.0 } else { total as f64 };
                c.iter().map(|(l, n)| (l.clone(), *n as f64 / d)).collect()
            }
            Readout::Probabilities(p) => p.clone(),
        };
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal).then(a.0.cmp(&b.0))
        });
        v
    }

    /// Whether the readout is counts rather than reported probabilities.
    ///
    /// The predicate a display uses to decide between printing `512` and printing `0.497`.
    pub fn has_counts(&self) -> bool {
        matches!(self.readout, Readout::Counts(_))
    }
}

/// Why a quantum job could not be run.
///
/// Every variant carries enough to print a sentence a user can act on. `"quantum error"` is not an
/// error message.
#[derive(Debug, Clone, PartialEq)]
pub enum QpuError {
    /// The circuit is malformed. See [`CircuitError`].
    Circuit(CircuitError),
    /// More qubits than this device has.
    TooManyQubits { requested: u8, available: u32 },
    /// Deeper than this device allows.
    TooDeep { requested: usize, max: u32 },
    /// More shots than this device allows.
    TooManyShots { requested: u32, max: u32 },
    /// The circuit uses a gate this device does not implement.
    UnsupportedGate(&'static str),
    /// The backend cannot run this circuit at all, with a reason.
    Unsupported(String),
    /// No such device.
    NoSuchDevice(u32),
    /// A job handle this backend does not recognise.
    NoSuchJob(JobHandle),
    /// The job was cancelled by the caller.
    Cancelled,
    /// Transport or provider failure — network, HTTP status, malformed response.
    ///
    /// ⚠️ May have happened *after* the request was sent, so a POST must **not** be replayed on
    /// this. See [`QpuError::Unreachable`] for the case where replay is safe.
    Provider(String),
    /// The provider could not be reached, and **nothing was sent**.
    ///
    /// ★ Distinct from [`QpuError::Provider`] purely so a submission can be safely retried. The name
    /// did not resolve or no connection was established, so the server cannot have seen the request
    /// — which means re-sending it cannot create a second job. On metered quantum hardware that
    /// distinction is the difference between a retry and a double charge.
    ///
    /// Anything that might have reached the wire stays `Provider`.
    Unreachable(String),
    /// Credentials missing or rejected.
    Auth(String),
}

impl From<CircuitError> for QpuError {
    fn from(e: CircuitError) -> QpuError {
        QpuError::Circuit(e)
    }
}

impl fmt::Display for QpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QpuError::Circuit(e) => write!(f, "{e}"),
            QpuError::TooManyQubits { requested, available } => write!(
                f,
                "the circuit needs {requested} qubits; this device has {available}"
            ),
            QpuError::TooDeep { requested, max } => {
                write!(f, "circuit depth {requested} exceeds this device's limit of {max}")
            }
            QpuError::TooManyShots { requested, max } => {
                write!(f, "{requested} shots exceeds this device's limit of {max}")
            }
            QpuError::UnsupportedGate(g) => {
                write!(f, "this device does not implement the `{g}` gate")
            }
            QpuError::Unsupported(why) => write!(f, "{why}"),
            QpuError::NoSuchDevice(id) => write!(f, "no quantum device with id {id}"),
            QpuError::NoSuchJob(h) => write!(f, "no job with handle {}", h.0),
            QpuError::Cancelled => write!(f, "cancelled"),
            QpuError::Provider(m) => write!(f, "provider: {m}"),
            QpuError::Unreachable(m) => write!(f, "could not reach the provider: {m}"),
            QpuError::Auth(m) => write!(f, "authentication: {m}"),
        }
    }
}

/// One way of executing a quantum circuit.
///
/// Implementors: a local simulator, a cloud provider, and eventually a driver for physically
/// attached hardware. The trait is the boundary an application never sees past — it asks for
/// quantum computation and gets it, learning what served the request only through
/// [`Outcome::provenance`].
pub trait QpuBackend {
    /// Short identifier — `"simulator"`, `"ionq"`. Shown by `quantum backends`.
    fn name(&self) -> &str;

    /// One line: what this backend is and what it talks to.
    fn describe(&self) -> &str;

    /// The device this backend represents.
    ///
    /// ⚠️ A backend that is not driving physically attached hardware must **not** return
    /// [`QpuStatus::Hardware`] here. The simulator hardcodes [`QpuStatus::Simulator`]; there is no
    /// setter anywhere in this crate that would let it do otherwise.
    fn info(&self) -> QpuInfo;

    /// Check a circuit against this device's limits **before** anything is executed or transmitted.
    ///
    /// This is where resource exhaustion is refused: a state-vector simulator allocates 2ⁿ complex
    /// amplitudes, so an unchecked qubit count is an attacker-controlled allocation. See
    /// `docs/quantum/security.md`.
    ///
    /// The default implementation enforces everything [`QpuInfo`] declares, which is the right
    /// behaviour for nearly every backend; override only to add device-specific rules.
    fn validate(&self, c: &Circuit, shots: Shots) -> Result<(), QpuError> {
        c.validate()?;
        let info = self.info();

        if info.qubits > 0 && c.qubits as u32 > info.qubits {
            return Err(QpuError::TooManyQubits { requested: c.qubits, available: info.qubits });
        }
        if info.max_depth > 0 {
            let d = c.depth();
            if d as u32 > info.max_depth {
                return Err(QpuError::TooDeep { requested: d, max: info.max_depth });
            }
        }
        if info.max_shots > 0 && shots.0 > info.max_shots {
            return Err(QpuError::TooManyShots { requested: shots.0, max: info.max_shots });
        }
        if info.gate_set != 0 {
            let used = c.gate_set();
            let missing = used & !info.gate_set;
            if missing != 0 {
                // Name the first unsupported gate rather than printing a bitmask.
                for g in crate::circuit::Gate::ALL {
                    if missing & (1 << g.bit()) != 0 {
                        return Err(QpuError::UnsupportedGate(g.qasm_name()));
                    }
                }
            }
        }
        Ok(())
    }

    /// Begin executing. Must not block.
    ///
    /// `seed` is entropy for any sampling the backend does. Passed in rather than sourced here so
    /// this crate has no opinion about where randomness comes from — on Nyx that is syscall 318,
    /// on the host it is a test constant, and neither belongs in a `no_std` library.
    fn begin(&mut self, c: &Circuit, shots: Shots, seed: u64) -> Result<JobHandle, QpuError>;

    /// Advance a job by a bounded amount of work and report where it got to.
    ///
    /// "Bounded" is the contract. `Fetch::poll` bounds itself with a 20 ms socket timeout; a
    /// simulator bounds itself by being fast. A `poll` that blocks for seconds freezes whatever is
    /// pumping it.
    fn poll(&mut self, h: JobHandle) -> JobState;

    /// Abandon a job. Idempotent; a handle that has already finished or never existed is not an
    /// error.
    fn cancel(&mut self, h: JobHandle);

    /// A short note about what is happening beneath the stage, for the status line.
    ///
    /// ★ Exists so a **retry loop cannot look like a hang**. A backend absorbing transport failures
    /// keeps reporting `Running(Queued)`, which is honest about the job and says nothing about the
    /// link — so a watch quietly retrying for two minutes is indistinguishable from a job that is
    /// simply slow. Overriding this lets the backend say "retrying (3/40)" instead.
    ///
    /// `None` when there is nothing to add, which is the normal case.
    fn status_note(&self, _h: JobHandle) -> Option<String> {
        None
    }

    /// Validate, begin, and poll to completion. **Blocks.**
    ///
    /// For tests and for backends that are effectively instantaneous. Never call this from an
    /// interactive path — see the module docs.
    fn run(&mut self, c: &Circuit, shots: Shots, seed: u64) -> Result<Outcome, QpuError> {
        self.validate(c, shots)?;
        let h = self.begin(c, shots, seed)?;
        loop {
            match self.poll(h) {
                JobState::Done(o) => return Ok(*o),
                JobState::Failed(e) => return Err(e),
                JobState::Running(_) => {}
            }
        }
    }
}

/// The set of backends available to this machine.
///
/// Same design as `libs/toolchains::Registry`: adding a backend is a one-line change in
/// [`Registry::with_defaults`].
pub struct Registry {
    backends: Vec<Box<dyn QpuBackend>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry { backends: Vec::new() }
    }

    /// Every backend that needs no configuration.
    ///
    /// The simulator only. Providers need credentials and network discovery, so `libs/quantum-rt`
    /// registers those — this crate has no networking by design.
    pub fn with_defaults() -> Registry {
        let mut r = Registry::new();
        r.register(Box::new(crate::sim_backend::SimBackend::new()));
        r
    }

    pub fn register(&mut self, b: Box<dyn QpuBackend>) {
        self.backends.push(b);
    }

    pub fn len(&self) -> usize {
        self.backends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn QpuBackend> {
        self.backends.iter().map(|b| b.as_ref())
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut (dyn QpuBackend + 'static)> {
        self.backends.get_mut(i).map(|b| b.as_mut())
    }

    pub fn by_name(&self, name: &str) -> Option<&dyn QpuBackend> {
        self.backends.iter().find(|b| b.name() == name).map(|b| b.as_ref())
    }

    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut (dyn QpuBackend + 'static)> {
        self.backends.iter_mut().find(|b| b.name() == name).map(|b| b.as_mut())
    }

    /// Every device, in registration order.
    pub fn devices(&self) -> Vec<QpuInfo> {
        self.backends.iter().map(|b| b.info()).collect()
    }

    /// The index of the most *real* backend available — hardware over remote over simulator.
    ///
    /// ⚠️ "Most real" is not "best". A 29-qubit cloud simulator beats a 2-qubit trapped-ion device
    /// on every measure except being a quantum computer, and this function prefers the latter. A
    /// caller that wants the biggest device should ask for that explicitly.
    ///
    /// A remote simulator ranks **below** a local one: it is no more quantum and it is slower and
    /// less private.
    pub fn most_real(&self) -> Option<usize> {
        self.backends
            .iter()
            .enumerate()
            .filter(|(_, b)| b.info().status() != QpuStatus::NotPresent)
            .max_by_key(|(_, b)| {
                let i = b.info();
                match (i.status(), i.remote_is_simulator()) {
                    (QpuStatus::Hardware, _) => 3u8,
                    (QpuStatus::Remote, false) => 2,
                    (QpuStatus::Simulator, _) => 1,
                    (QpuStatus::Remote, true) => 0,
                    (QpuStatus::NotPresent, _) => 0,
                }
            })
            .map(|(i, _)| i)
    }
}

impl Default for Registry {
    fn default() -> Registry {
        Registry::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn provenance(status: QpuStatus, remote_sim: bool) -> Provenance {
        Provenance {
            backend: "test".to_string(),
            device: "test".to_string(),
            status,
            remote_is_simulator: remote_sim,
            is_quantum: status.is_quantum(remote_sim),
        }
    }

    fn counts_outcome() -> Outcome {
        Outcome {
            readout: Readout::Counts(vec![
                ("00".to_string(), 500),
                ("11".to_string(), 524),
            ]),
            shots_requested: 1024,
            measured: vec![0, 1],
            provenance: provenance(QpuStatus::Simulator, false),
        }
    }

    fn probs_outcome() -> Outcome {
        Outcome {
            readout: Readout::Probabilities(vec![
                ("00".to_string(), 0.49),
                ("11".to_string(), 0.51),
            ]),
            shots_requested: 1024,
            measured: vec![0, 1],
            provenance: provenance(QpuStatus::Remote, false),
        }
    }

    /// ★ The anti-fabrication invariant. A provider reports probabilities; turning those into
    /// "512 shots" would invent a measurement nobody made.
    #[test]
    fn probabilities_never_masquerade_as_counts() {
        let p = probs_outcome();
        assert!(!p.has_counts());
        assert_eq!(p.counts_for("00"), None, "a probability readout has no count to report");
        assert!((p.probability_for("11") - 0.51).abs() < 1e-12);

        let c = counts_outcome();
        assert!(c.has_counts());
        assert_eq!(c.counts_for("00"), Some(500));
    }

    #[test]
    fn an_absent_label_is_zero_not_missing() {
        let c = counts_outcome();
        assert_eq!(c.counts_for("01"), Some(0));
        assert_eq!(c.probability_for("01"), 0.0);
        assert_eq!(probs_outcome().probability_for("10"), 0.0);
    }

    #[test]
    fn counts_convert_to_probabilities_but_not_the_reverse() {
        let c = counts_outcome();
        assert!((c.probability_for("00") - 500.0 / 1024.0).abs() < 1e-12);
    }

    #[test]
    fn ranked_is_descending_and_stable_on_ties() {
        let o = Outcome {
            readout: Readout::Counts(vec![
                ("11".to_string(), 10),
                ("00".to_string(), 10),
                ("01".to_string(), 80),
            ]),
            shots_requested: 100,
            measured: vec![0, 1],
            provenance: provenance(QpuStatus::Simulator, false),
        };
        let r = o.ranked();
        assert_eq!(r[0].0, "01");
        // Ties break by label so the table does not reshuffle between renders.
        assert_eq!(r[1].0, "00");
        assert_eq!(r[2].0, "11");
    }

    /// ★ The disclosure line is the last defence against a simulated histogram being read as a
    /// measurement. A remote simulator must not get the remote-hardware sentence.
    #[test]
    fn every_disclosure_states_what_actually_ran() {
        assert!(provenance(QpuStatus::Simulator, false).disclosure().contains("classical simulator"));
        assert!(provenance(QpuStatus::Hardware, false).disclosure().contains("local quantum hardware"));
        assert!(provenance(QpuStatus::Remote, false).disclosure().contains("remote quantum hardware"));

        let remote_sim = provenance(QpuStatus::Remote, true);
        assert!(remote_sim.disclosure().contains("SIMULATOR"));
        assert!(!remote_sim.is_quantum);
        assert_ne!(
            remote_sim.disclosure(),
            provenance(QpuStatus::Remote, false).disclosure(),
            "a remote simulator must not claim the remote-hardware disclosure"
        );
    }

    #[test]
    fn default_registry_has_exactly_the_simulator() {
        let r = Registry::with_defaults();
        assert_eq!(r.len(), 1);
        assert!(r.by_name("simulator").is_some());
        let d = r.devices();
        assert_eq!(d[0].status(), QpuStatus::Simulator);
        assert!(!d[0].is_quantum());
    }

    #[test]
    fn most_real_prefers_the_least_simulated_thing_available() {
        // With only the simulator registered, it is the answer.
        let r = Registry::with_defaults();
        assert_eq!(r.most_real(), Some(0));
    }

    #[test]
    fn errors_say_something_actionable() {
        let e = QpuError::TooManyQubits { requested: 30, available: 20 };
        let s = alloc::format!("{e}");
        assert!(s.contains("30") && s.contains("20"), "{s}");
        assert!(!s.is_empty());

        let e = QpuError::UnsupportedGate("ccx");
        assert!(alloc::format!("{e}").contains("ccx"));
    }
}
