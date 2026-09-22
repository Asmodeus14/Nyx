//! # `nyx-quantum-rt` — the Nyx quantum runtime
//!
//! Where "I need quantum computation" becomes a concrete backend.
//!
//! An application asks [`QpuSession`] for a device, hands it a circuit, and polls. It is
//! deliberately **not** told whether the answer came from the local simulator, a cloud provider, or
//! physically attached hardware — except through [`nyx_quantum::Provenance`], which travels with the
//! result and which every display is required to show.
//!
//! ```text
//!   application
//!        │
//!   QpuSession            ← this crate
//!        │
//!   ┌────┴──────────────┬──────────────────┐
//!   ▼                   ▼                  ▼
//! SimBackend      provider (HTTPS)     hardware driver
//! (nyx-quantum)   libs/net + libs/json   (none exist yet)
//! ```
//!
//! ## What lives here and what does not
//!
//! `nyx_quantum` is `no_std` and has zero dependencies: the device model, the circuit IR, the
//! backend trait, and the state-vector simulator. This crate is `std`, because the qclang adapter
//! and any network provider need it.
//!
//! The kernel is not in this picture at all beyond two read-only syscalls. It knows about
//! physically-attached PCI devices and nothing else — no circuits, no simulator, no tokens, no URLs.
//! See `docs/quantum/architecture.md`.
//!
//! ## Device numbering
//!
//! Devices are merged from two sources into one list:
//!
//! 1. **The kernel**, via syscall 573. Physically attached hardware. On this machine: none, and
//!    that is correct — see `docs/quantum/limitations.md`.
//! 2. **Userspace backends**, in registration order. The simulator always; providers when
//!    configured.
//!
//! Kernel devices come first so `QPU0` means the same thing across boots regardless of whether a
//! provider happened to be reachable.

pub mod ibm;
pub mod ionq;
pub mod provider;
pub mod qclang;

pub use nyx_quantum::{
    Circuit, Gate, JobHandle, JobState, Op, Outcome, Provenance, QpuBackend, QpuError, QpuInfo,
    QpuStatus, Registry, Shots, Stage,
};

use nyx_quantum::SimBackend;

/// Ask the kernel what quantum hardware is physically attached.
///
/// Returns empty on any target that is not Nyx — and, on Nyx, returns empty anyway, because there
/// is no consumer gate-model QPU this OS can drive over PCIe. That is not a stub: it means host
/// tests exercise the same "no hardware, fall back to the simulator" path the real machine takes.
#[cfg(target_os = "nyx")]
pub fn kernel_devices() -> Vec<QpuInfo> {
    // The kernel's registry holds at most 8 entries.
    let mut buf = [nyx_api::QpuInfo::default(); 8];
    let n = nyx_api::sys_quantum_enumerate(&mut buf);
    buf[..n].iter().map(convert_api_info).collect()
}

#[cfg(not(target_os = "nyx"))]
pub fn kernel_devices() -> Vec<QpuInfo> {
    Vec::new()
}

/// Translate the syscall ABI struct into the runtime's own type.
///
/// Field-by-field rather than a transmute: the two are laid out identically and asserted to be the
/// same size in three places, but a reinterpret would make that assertion load-bearing for memory
/// safety rather than merely for correctness.
#[cfg(target_os = "nyx")]
fn convert_api_info(a: &nyx_api::QpuInfo) -> QpuInfo {
    let mut q = QpuInfo {
        id: a.id,
        kind: a.kind,
        status: a.status,
        remote_is_simulator: a.remote_is_simulator,
        qubits: a.qubits,
        topology: a.topology,
        exec_model: a.exec_model,
        _reserved0: 0,
        gate_set: a.gate_set,
        max_shots: a.max_shots,
        max_depth: a.max_depth,
        queue_depth: a.queue_depth,
        queue_capacity: a.queue_capacity,
        coherence_t1_ns: a.coherence_t1_ns,
        coherence_t2_ns: a.coherence_t2_ns,
        calibrated_unix: a.calibrated_unix,
        pci_bdf: a.pci_bdf,
        pci_vendor_id: a.pci_vendor_id,
        pci_device_id: a.pci_device_id,
        _reserved1: 0,
        ..QpuInfo::default()
    };
    q.set_vendor(a.vendor_str());
    q.set_arch(a.arch_str());
    q.set_name(a.name_str());
    q
}

/// Which device an application wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QpuSelect {
    /// The least simulated device available.
    ///
    /// ⚠️ "Most real" is not "most capable". A 29-qubit cloud simulator beats a 2-qubit trapped-ion
    /// processor on every measure except being a quantum computer, and this prefers the latter. A
    /// caller that wants the biggest device should name it.
    Best,
    /// A specific device id, as printed by `quantum devices`.
    ById(u32),
    /// The local simulator, explicitly. Fast, private, reproducible, and not quantum.
    Simulator,
}

/// An open handle to one quantum device.
///
/// Owns the backend that serves it, so a session is the unit of "who am I talking to".
pub struct QpuSession {
    backend: Box<dyn QpuBackend>,
    info: QpuInfo,
}

impl QpuSession {
    /// Build the device list: kernel hardware first, then userspace backends.
    ///
    /// Ids are reassigned densely over the merged list, so what `quantum devices` prints as `QPU1`
    /// is what [`QpuSelect::ById(1)`](QpuSelect::ById) opens.
    pub fn devices() -> Vec<QpuInfo> {
        let mut out = kernel_devices();
        let reg = Registry::with_defaults();
        out.extend(reg.devices());
        for (i, d) in out.iter_mut().enumerate() {
            d.id = i as u32;
        }
        out
    }

    /// Open a device.
    pub fn open(select: QpuSelect) -> Result<QpuSession, QpuError> {
        let devices = Self::devices();

        let info = match select {
            QpuSelect::Simulator => devices
                .iter()
                .find(|d| d.status() == QpuStatus::Simulator)
                .copied()
                .ok_or_else(|| {
                    QpuError::Unsupported("the local simulator is not available".to_string())
                })?,
            QpuSelect::ById(id) => {
                *devices.iter().find(|d| d.id == id).ok_or(QpuError::NoSuchDevice(id))?
            }
            QpuSelect::Best => {
                // Rank by how little simulation is involved. A remote simulator ranks BELOW a local
                // one: it is no more quantum, and it is slower and less private.
                *devices
                    .iter()
                    .filter(|d| d.kind().is_executor())
                    .max_by_key(|d| match (d.status(), d.remote_is_simulator()) {
                        (QpuStatus::Hardware, _) => 3u8,
                        (QpuStatus::Remote, false) => 2,
                        (QpuStatus::Simulator, _) => 1,
                        _ => 0,
                    })
                    .ok_or_else(|| {
                        QpuError::Unsupported("no quantum device is available".to_string())
                    })?
            }
        };

        let backend = Self::backend_for(&info)?;
        Ok(QpuSession { backend, info })
    }

    /// Construct the backend that serves `info`.
    ///
    /// ⚠️ [`QpuStatus::Hardware`] has no arm. The kernel can *report* attached hardware — that path
    /// exists and is exercised — but nothing in this tree can drive it, because no such device is
    /// documented well enough to write a driver for. Returning a simulator here instead would be
    /// the precise failure this subsystem is built to prevent: a device reported as `HARDWARE`
    /// whose answers came from arithmetic. So it refuses, by name.
    fn backend_for(info: &QpuInfo) -> Result<Box<dyn QpuBackend>, QpuError> {
        match info.status() {
            QpuStatus::Simulator => Ok(Box::new(SimBackend::new())),
            QpuStatus::Hardware => Err(QpuError::Unsupported(format!(
                "{} is attached but Nyx has no driver for it; \
                 running it on a simulator instead would misreport the result",
                info.name_str()
            ))),
            QpuStatus::Remote => Err(QpuError::Unsupported(
                "no quantum provider is configured; run `quantum remote login`".to_string(),
            )),
            QpuStatus::NotPresent => Err(QpuError::Unsupported(format!(
                "{} is not a usable quantum device",
                info.name_str()
            ))),
        }
    }

    /// The device this session is talking to.
    pub fn info(&self) -> &QpuInfo {
        &self.info
    }

    /// Whether results from this session are produced by quantum mechanics.
    pub fn is_quantum(&self) -> bool {
        self.info.is_quantum()
    }

    /// Check a circuit against this device without running it.
    pub fn validate(&self, c: &Circuit, shots: Shots) -> Result<(), QpuError> {
        self.backend.validate(c, shots)
    }

    /// Submit a circuit. Does not block.
    pub fn submit(&mut self, c: &Circuit, shots: Shots, seed: u64) -> Result<Job, QpuError> {
        self.backend.validate(c, shots)?;
        let handle = self.backend.begin(c, shots, seed)?;
        Ok(Job { handle })
    }

    /// Advance a job by a bounded amount of work.
    pub fn poll(&mut self, job: &Job) -> JobState {
        self.backend.poll(job.handle)
    }

    /// Abandon a job.
    pub fn cancel(&mut self, job: &Job) {
        self.backend.cancel(job.handle)
    }

    /// Submit and poll to completion. **Blocks.**
    ///
    /// For tests and non-interactive callers. ⚠️ Never call this from an app's `update()` or from
    /// `apps/shell` — the shell *is* the window server, and a cloud job can sit in a provider's
    /// queue for minutes.
    pub fn run(&mut self, c: &Circuit, shots: Shots, seed: u64) -> Result<Outcome, QpuError> {
        self.backend.run(c, shots, seed)
    }
}

/// A submitted circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Job {
    handle: JobHandle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_no_hardware_the_device_list_is_exactly_the_simulator() {
        // The real state of this machine, and of every machine Nyx runs on.
        let d = QpuSession::devices();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].status(), QpuStatus::Simulator);
        assert_eq!(d[0].id, 0);
        assert!(!d[0].is_quantum());
    }

    /// ★ `Best` must never silently upgrade a simulator into a quantum claim.
    #[test]
    fn best_falls_back_to_the_simulator_and_says_so() {
        let s = QpuSession::open(QpuSelect::Best).unwrap();
        assert_eq!(s.info().status(), QpuStatus::Simulator);
        assert!(!s.is_quantum());
        assert_eq!(s.info().status_label(), "SIMULATOR");
    }

    #[test]
    fn ids_are_dense_and_addressable() {
        let d = QpuSession::devices();
        for (i, dev) in d.iter().enumerate() {
            assert_eq!(dev.id, i as u32);
            assert!(QpuSession::open(QpuSelect::ById(dev.id)).is_ok());
        }
        assert!(matches!(
            QpuSession::open(QpuSelect::ById(99)),
            Err(QpuError::NoSuchDevice(99))
        ));
    }

    #[test]
    fn bell_state_runs_end_to_end_through_the_session_api() {
        let mut s = QpuSession::open(QpuSelect::Simulator).unwrap();
        let o = s.run(&Circuit::bell(), Shots(2048), 0x5EED).unwrap();

        assert_eq!(o.counts_for("01"), Some(0));
        assert_eq!(o.counts_for("10"), Some(0));
        assert_eq!(o.counts_for("00").unwrap() + o.counts_for("11").unwrap(), 2048);
        // The result carries what produced it.
        assert!(!o.provenance.is_quantum);
        assert!(o.provenance.disclosure().contains("no quantum hardware was used"));
    }

    #[test]
    fn the_submit_poll_lifecycle_works_without_blocking() {
        let mut s = QpuSession::open(QpuSelect::Simulator).unwrap();
        let job = s.submit(&Circuit::ghz(3), Shots(256), 7).unwrap();
        match s.poll(&job) {
            JobState::Done(o) => {
                assert_eq!(o.counts_for("010"), Some(0), "GHZ has no mixed outcomes");
                assert_eq!(
                    o.counts_for("000").unwrap() + o.counts_for("111").unwrap(),
                    256
                );
            }
            other => panic!("expected completion, got {other:?}"),
        }
    }

    #[test]
    fn limits_are_refused_before_submission() {
        let mut s = QpuSession::open(QpuSelect::Simulator).unwrap();
        let too_big = Circuit::new(30, 0);
        assert!(s.validate(&too_big, Shots(1)).is_err());
        assert!(s.submit(&too_big, Shots(1), 0).is_err());
    }

    /// ★ Attached-but-undriveable hardware must refuse, not fall back to a simulator. A simulated
    /// answer attributed to a `HARDWARE` device is the worst outcome this subsystem can produce.
    #[test]
    fn attached_hardware_without_a_driver_refuses_rather_than_simulating() {
        let mut hw = QpuInfo::default();
        hw.status = QpuStatus::Hardware as u32;
        hw.set_name("some attached QPU");

        match QpuSession::backend_for(&hw) {
            Err(QpuError::Unsupported(why)) => {
                assert!(why.contains("no driver"), "{why}");
                assert!(why.contains("misreport"), "{why}");
            }
            _ => panic!("a driverless QPU must not be served by the simulator"),
        }
    }

    #[test]
    fn a_remote_device_without_credentials_says_what_to_do_about_it() {
        let mut r = QpuInfo::default();
        r.status = QpuStatus::Remote as u32;
        match QpuSession::backend_for(&r) {
            Err(QpuError::Unsupported(why)) => assert!(why.contains("quantum remote login"), "{why}"),
            Err(e) => panic!("wrong error: {e}"),
            Ok(_) => panic!("a remote device with no provider configured must not open"),
        }
    }

    #[test]
    fn a_qclang_program_runs_through_the_session() {
        // Affine style, matching `apps/qcstudio/samples/sample.ql` — see `qclang::tests::BELL`.
        let c = qclang::compile_to_circuit(
            "fn main() -> int { qubit a = |0>; qubit b = |0>; qubit c = CNOT(H(a), b); \
             cbit r1 = measure(a); cbit r2 = measure(b); return 0; }",
        )
        .expect("adapt");
        let mut s = QpuSession::open(QpuSelect::Simulator).unwrap();
        let o = s.run(&c, Shots(1000), 3).unwrap();
        assert_eq!(o.counts_for("01"), Some(0));
        assert_eq!(o.counts_for("10"), Some(0));
    }
}
