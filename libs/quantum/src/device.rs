//! The QPU device model — what a quantum compute resource *is*, to an operating system.
//!
//! This is the type that crosses the syscall boundary, so it is `#[repr(C)]`, fixed-size, and
//! mirrored by hand in `libs/api/src/lib.rs` and `nyx-kernel/src/quantum.rs`. Field order **is** the
//! ABI: there is no bindgen across the ring boundary in this tree, and the same is true of
//! `SysMetrics`, `WindowQuad` and `SystemInfo`. [`QPU_INFO_SIZE`] is asserted at compile time in all
//! three places so that adding a field breaks the build rather than silently reinterpreting every
//! later one.
//!
//! ## The rule this module exists to enforce
//!
//! > Never report `HARDWARE` when Nyx is not driving physically attached quantum hardware.
//!
//! That is stated as a requirement in `docs/quantum/limitations.md`, but a requirement written in a
//! document is a request. Here it is a property of the type system:
//!
//! * [`QpuStatus::Hardware`] is only ever produced by the kernel's PCI probe, which matches a real
//!   device. No userspace backend can construct it — the simulator's `info()` hardcodes
//!   [`QpuStatus::Simulator`] and there is no setter anywhere in the crate.
//! * A quantum random number generator gets its own [`QuantumDeviceKind`] so it can never appear
//!   where a processor is expected. It is real quantum hardware and it cannot run a circuit; those
//!   two facts have to be expressible at the same time.
//!
//! ## ⚠️ Why `Remote` is not enough on its own
//!
//! This is the subtlest part of the model and it was nearly got wrong.
//!
//! Cloud providers serve simulators and real processors **through the same API**. IonQ's
//! `qpu.aria-1` is a trapped-ion processor; its `simulator` target is a classical program on their
//! machines. Both arrive over the same HTTPS endpoint, with the same job lifecycle, returning the
//! same JSON.
//!
//! If both reported `Remote` and nothing else, a classical simulation could be presented as a
//! quantum result — the exact failure the local `Simulator` state exists to prevent, reintroduced
//! one layer out. "It came from the internet" and "it was computed by quantum mechanics" are
//! different claims and the model has to be able to make only one of them.
//!
//! Hence [`QpuInfo::remote_is_simulator`], and hence every surface rendering `REMOTE/hw` and
//! `REMOTE/sim` distinctly rather than both as `REMOTE`.
//!
//! ## Numbers that are not known
//!
//! Coherence times and calibration timestamps are `0` when the backend did not report them, and
//! [`QpuInfo::coherence_t1`] and friends return `Option`. A simulator has no T1; a provider may not
//! publish one. The alternative — a plausible default — is the thing `apps/sysmon` refuses to do
//! with GPU utilisation, in its own words: *"A plausible-looking number in the one window whose
//! entire purpose is reporting true numbers is the worst lie available here."*

use core::fmt;

/// Where a quantum device's answers actually come from.
///
/// The ordering is deliberate and is relied on by `QpuSelect::Best`: higher is more real. It is
/// **not** "better" in any performance sense — a 29-qubit cloud simulator will out-compute a
/// 2-qubit trapped-ion device on every metric except being a quantum computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u32)]
pub enum QpuStatus {
    /// No quantum device of any kind: not attached, not configured, not reachable.
    ///
    /// This is the correct and expected state on every machine this OS currently runs on.
    #[default]
    NotPresent = 0,
    /// Classical software running on this CPU. Real mathematics, no quantum mechanics.
    Simulator = 1,
    /// Reachable over the network. The hardware, if it is hardware, is not here.
    ///
    /// Always check [`QpuInfo::remote_is_simulator`] before describing this as a quantum device.
    Remote = 2,
    /// A quantum processor physically attached to this machine.
    ///
    /// ⚠️ Unreachable on this machine by design — see `docs/quantum/limitations.md`. Only the
    /// kernel's PCI probe can produce it.
    Hardware = 3,
}

impl QpuStatus {
    /// Parse the raw `u32` that crossed the syscall boundary.
    ///
    /// Unknown values become [`QpuStatus::NotPresent`] rather than panicking or transmuting: a
    /// kernel that reports a status this build does not understand is a kernel/userspace version
    /// skew, and the safe reading of "I don't know what this is" is "assume nothing is there".
    pub fn from_raw(v: u32) -> QpuStatus {
        match v {
            1 => QpuStatus::Simulator,
            2 => QpuStatus::Remote,
            3 => QpuStatus::Hardware,
            _ => QpuStatus::NotPresent,
        }
    }

    /// The short, all-caps label every user-facing surface prints.
    ///
    /// Deliberately derived from one place so the terminal, System Monitor and the demo app cannot
    /// drift into describing the same device differently.
    pub fn label(self) -> &'static str {
        match self {
            QpuStatus::NotPresent => "NOT PRESENT",
            QpuStatus::Simulator => "SIMULATOR",
            QpuStatus::Remote => "REMOTE",
            QpuStatus::Hardware => "HARDWARE",
        }
    }

    /// Whether a result from this device was produced by quantum mechanics rather than arithmetic.
    ///
    /// ⚠️ Takes `remote_is_simulator` because [`QpuStatus::Remote`] alone cannot answer the
    /// question — see the module docs. This is the predicate every "was this real?" decision should
    /// go through, rather than matching on the status directly and forgetting the remote case.
    pub fn is_quantum(self, remote_is_simulator: bool) -> bool {
        match self {
            QpuStatus::Hardware => true,
            QpuStatus::Remote => !remote_is_simulator,
            QpuStatus::NotPresent | QpuStatus::Simulator => false,
        }
    }
}

impl fmt::Display for QpuStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What kind of quantum device this is.
///
/// Exists so that a QRNG cannot be mistaken for a processor. See `docs/quantum/limitations.md`: a
/// Quantis PCIe card is genuinely quantum hardware, is genuinely a plain PCI device Nyx could bind,
/// and genuinely cannot run a circuit. All three at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum QuantumDeviceKind {
    /// A gate-model quantum processor: takes a circuit, returns measurements.
    #[default]
    Qpu = 0,
    /// Control electronics for a QPU that lives elsewhere — an AWG/digitiser rack, an FPGA
    /// controller. The classical half of a quantum computer. Nyx can see one on the bus; it cannot
    /// drive one without a vendor SDK.
    QuantumControlElectronics = 1,
    /// A quantum entropy source. Real physics, no computation. Belongs to `random.rs`, not here.
    QuantumRng = 2,
    /// Something on the PCI bus that *might* be an accelerator and which Nyx cannot identify.
    ///
    /// ⚠️ This variant exists because of an honesty problem in the kernel's probe. PCI class `0x12`
    /// is "Processing Accelerator" and class `0xFF` is "unassigned" — a PCIe quantum accelerator
    /// would plausibly appear as either, and so would an AI inference card, a research FPGA, or
    /// anything else nobody bothered to classify.
    ///
    /// **A class code cannot tell you that a device is a QPU.** Nyx has no verified vendor/device
    /// ID for any quantum processor (see `docs/quantum/limitations.md`), so guessing from the class
    /// alone would manufacture exactly the `HARDWARE` claim this subsystem exists to prevent.
    ///
    /// So an unidentified accelerator is recorded, reported, and explicitly **not** counted as a
    /// quantum device: [`QuantumDeviceKind::is_executor`] is false and the entry's status stays
    /// [`QpuStatus::NotPresent`]. It is a diagnostic — "here is what was on the bus" — which
    /// matters because this laptop has no serial console, so anything the probe learns and only
    /// logs is learned and thrown away.
    UnidentifiedAccelerator = 3,
}

impl QuantumDeviceKind {
    pub fn from_raw(v: u32) -> QuantumDeviceKind {
        match v {
            1 => QuantumDeviceKind::QuantumControlElectronics,
            2 => QuantumDeviceKind::QuantumRng,
            3 => QuantumDeviceKind::UnidentifiedAccelerator,
            _ => QuantumDeviceKind::Qpu,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QuantumDeviceKind::Qpu => "QPU",
            QuantumDeviceKind::QuantumControlElectronics => "control electronics",
            QuantumDeviceKind::QuantumRng => "quantum RNG",
            QuantumDeviceKind::UnidentifiedAccelerator => "unidentified accelerator",
        }
    }

    /// Whether this device can execute a circuit at all.
    ///
    /// A QRNG, a controller and an unidentified card all cannot — for three different reasons, all
    /// of which are reasons.
    pub fn is_executor(self) -> bool {
        matches!(self, QuantumDeviceKind::Qpu)
    }
}

/// Coarse qubit connectivity. Enough to describe a device in one word.
///
/// The full coupling map is deliberately **not** here: it is `O(n²)` and this struct is fixed-size
/// and crosses a syscall. A backend that knows its coupling map exposes it through a separate call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum Topology {
    /// Not reported. Not a claim that there is no structure.
    #[default]
    Unknown = 0,
    /// Any qubit can interact with any other — simulators, and trapped-ion devices.
    AllToAll = 1,
    /// A chain: qubit `i` couples to `i±1`.
    Linear = 2,
    /// A 2-D lattice — the usual superconducting layout.
    Grid = 3,
    /// Irregular; the coupling map must be queried separately.
    Explicit = 4,
}

impl Topology {
    pub fn from_raw(v: u32) -> Topology {
        match v {
            1 => Topology::AllToAll,
            2 => Topology::Linear,
            3 => Topology::Grid,
            4 => Topology::Explicit,
            _ => Topology::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Topology::Unknown => "unknown",
            Topology::AllToAll => "all-to-all",
            Topology::Linear => "linear",
            Topology::Grid => "grid",
            Topology::Explicit => "explicit",
        }
    }
}

/// How work reaches the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum ExecModel {
    /// Submit a circuit and a shot count, wait, collect a histogram. Every real QPU today.
    #[default]
    Batch = 0,
    /// Gates can be issued with classical feedback inside the coherence window. No backend here
    /// does this; the variant exists so the enum does not have to change when one does.
    Interactive = 1,
}

impl ExecModel {
    pub fn from_raw(v: u32) -> ExecModel {
        match v {
            1 => ExecModel::Interactive,
            _ => ExecModel::Batch,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExecModel::Batch => "batch",
            ExecModel::Interactive => "interactive",
        }
    }
}

/// Length of the fixed `vendor` and `arch` fields.
pub const VENDOR_LEN: usize = 32;
/// Length of the fixed `name` field.
pub const NAME_LEN: usize = 64;

/// The exact wire size of [`QpuInfo`].
///
/// ⚠️ Asserted in three places — here, `libs/api`, and `nyx-kernel/src/quantum.rs`. If you add a
/// field, all three break, which is the point: the struct is mirrored by hand and a silent size
/// change would reinterpret every field after the insertion point.
pub const QPU_INFO_SIZE: usize = 224;

/// Everything the OS knows about one quantum compute resource.
///
/// `#[repr(C)]`, **append-only**. See the module docs for the ABI rules.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct QpuInfo {
    /// Stable index within this boot. `QPU0`, `QPU1`, … as printed by the terminal.
    pub id: u32,
    /// [`QuantumDeviceKind`] as a raw `u32`. Use [`QpuInfo::kind`].
    pub kind: u32,
    /// [`QpuStatus`] as a raw `u32`. Use [`QpuInfo::status`].
    pub status: u32,
    /// Non-zero when `status == Remote` **and the remote backend is itself a simulator**.
    ///
    /// ⚠️ Meaningless unless the status is `Remote`. See the module docs for why this field is not
    /// optional cosmetics.
    pub remote_is_simulator: u32,

    /// Usable qubits. `0` when nothing is present — never a placeholder.
    pub qubits: u32,
    /// [`Topology`] as a raw `u32`.
    pub topology: u32,
    /// [`ExecModel`] as a raw `u32`.
    pub exec_model: u32,
    pub _reserved0: u32,

    /// Bitmask of supported gates, indexed by `Gate::bit()` in `circuit.rs`.
    pub gate_set: u64,

    /// Maximum shots per submission. `0` = not reported.
    pub max_shots: u32,
    /// Maximum circuit depth. `0` = not reported.
    pub max_depth: u32,
    /// Jobs currently queued ahead of us. `u32::MAX` = not reported.
    pub queue_depth: u32,
    /// Queue capacity. `0` = not reported.
    pub queue_capacity: u32,

    /// T1 relaxation time in nanoseconds. `0` = not reported.
    pub coherence_t1_ns: u64,
    /// T2 dephasing time in nanoseconds. `0` = not reported.
    pub coherence_t2_ns: u64,
    /// Unix time of the last calibration. `0` = not reported.
    pub calibrated_unix: u64,

    /// PCI bus/device/function packed as `bus << 16 | dev << 8 | func`. `0` for non-PCI backends.
    pub pci_bdf: u32,
    /// PCI vendor ID, or `0`.
    pub pci_vendor_id: u32,
    /// PCI device ID, or `0`.
    pub pci_device_id: u32,
    pub _reserved1: u32,

    /// Who makes it. NUL-padded ASCII. `"Nyx"` for the built-in simulator.
    pub vendor: [u8; VENDOR_LEN],
    /// What it is. NUL-padded ASCII. `"statevector"`, `"trapped-ion"`, …
    pub arch: [u8; VENDOR_LEN],
    /// Human-facing device name. NUL-padded ASCII.
    pub name: [u8; NAME_LEN],
}

const _: () = assert!(core::mem::size_of::<QpuInfo>() == QPU_INFO_SIZE);
const _: () = assert!(core::mem::align_of::<QpuInfo>() == 8);

impl Default for QpuInfo {
    /// A device that is not there.
    ///
    /// Deliberately the `Default`: the overwhelmingly common case on real hardware is that there is
    /// no QPU, and the zero value should be the honest one rather than something that has to be
    /// corrected. `queue_depth` is the one field whose "unknown" is not zero.
    fn default() -> QpuInfo {
        QpuInfo {
            id: 0,
            kind: QuantumDeviceKind::Qpu as u32,
            status: QpuStatus::NotPresent as u32,
            remote_is_simulator: 0,
            qubits: 0,
            topology: Topology::Unknown as u32,
            exec_model: ExecModel::Batch as u32,
            _reserved0: 0,
            gate_set: 0,
            max_shots: 0,
            max_depth: 0,
            queue_depth: u32::MAX,
            queue_capacity: 0,
            coherence_t1_ns: 0,
            coherence_t2_ns: 0,
            calibrated_unix: 0,
            pci_bdf: 0,
            pci_vendor_id: 0,
            pci_device_id: 0,
            _reserved1: 0,
            vendor: [0; VENDOR_LEN],
            arch: [0; VENDOR_LEN],
            name: [0; NAME_LEN],
        }
    }
}

impl QpuInfo {
    pub fn status(&self) -> QpuStatus {
        QpuStatus::from_raw(self.status)
    }

    pub fn kind(&self) -> QuantumDeviceKind {
        QuantumDeviceKind::from_raw(self.kind)
    }

    pub fn topology(&self) -> Topology {
        Topology::from_raw(self.topology)
    }

    pub fn exec_model(&self) -> ExecModel {
        ExecModel::from_raw(self.exec_model)
    }

    pub fn remote_is_simulator(&self) -> bool {
        self.remote_is_simulator != 0
    }

    /// Whether results from this device are produced by quantum mechanics.
    ///
    /// The single predicate every display should use. Handles the remote-simulator case that
    /// matching on [`QpuInfo::status`] alone would get wrong.
    pub fn is_quantum(&self) -> bool {
        self.status().is_quantum(self.remote_is_simulator())
    }

    /// The status as shown to a user, distinguishing remote hardware from a remote simulator.
    ///
    /// `NOT PRESENT` / `SIMULATOR` / `REMOTE/hw` / `REMOTE/sim` / `HARDWARE`.
    pub fn status_label(&self) -> &'static str {
        match self.status() {
            QpuStatus::Remote => {
                if self.remote_is_simulator() {
                    "REMOTE/sim"
                } else {
                    "REMOTE/hw"
                }
            }
            other => other.label(),
        }
    }

    /// One line of plain English saying what this device actually is, for the surfaces that have
    /// room for it. `None` when the status label already says everything true.
    ///
    /// The remote-simulator string is the important one: it is the case a reader is most likely to
    /// misread, so it is spelled out rather than implied by a suffix.
    pub fn caveat(&self) -> Option<&'static str> {
        match self.status() {
            QpuStatus::Simulator => Some("software; not a quantum processor"),
            QpuStatus::Remote if self.remote_is_simulator() => {
                Some("remote, but still a classical simulator")
            }
            QpuStatus::NotPresent => Some("no quantum device of this kind is attached"),
            _ => None,
        }
    }

    pub fn vendor_str(&self) -> &str {
        cstr(&self.vendor)
    }

    pub fn arch_str(&self) -> &str {
        cstr(&self.arch)
    }

    pub fn name_str(&self) -> &str {
        cstr(&self.name)
    }

    pub fn set_vendor(&mut self, s: &str) {
        set_cstr(&mut self.vendor, s);
    }

    pub fn set_arch(&mut self, s: &str) {
        set_cstr(&mut self.arch, s);
    }

    pub fn set_name(&mut self, s: &str) {
        set_cstr(&mut self.name, s);
    }

    /// T1 in nanoseconds, or `None` if the backend did not report one.
    pub fn coherence_t1(&self) -> Option<u64> {
        (self.coherence_t1_ns != 0).then_some(self.coherence_t1_ns)
    }

    /// T2 in nanoseconds, or `None` if the backend did not report one.
    pub fn coherence_t2(&self) -> Option<u64> {
        (self.coherence_t2_ns != 0).then_some(self.coherence_t2_ns)
    }

    /// Unix calibration timestamp, or `None`.
    pub fn calibrated(&self) -> Option<u64> {
        (self.calibrated_unix != 0).then_some(self.calibrated_unix)
    }

    /// Jobs queued ahead of us, or `None` if the backend does not report a queue.
    pub fn queue(&self) -> Option<u32> {
        (self.queue_depth != u32::MAX).then_some(self.queue_depth)
    }
}

impl fmt::Debug for QpuInfo {
    /// Hand-written because the three byte arrays are unreadable via derive, and because a device
    /// summary that omits the status is useless.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QpuInfo")
            .field("id", &self.id)
            .field("status", &self.status_label())
            .field("kind", &self.kind().label())
            .field("vendor", &self.vendor_str())
            .field("arch", &self.arch_str())
            .field("name", &self.name_str())
            .field("qubits", &self.qubits)
            .field("topology", &self.topology().label())
            .finish()
    }
}

/// Read a NUL-padded fixed array back as a `&str`, stopping at the first NUL.
///
/// Invalid UTF-8 yields `""` rather than panicking: these bytes may have come from the kernel or
/// from a provider's JSON, and a device with a mangled name should still be listable.
fn cstr(buf: &[u8]) -> &str {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..end]).unwrap_or("")
}

/// Write `s` into a fixed NUL-padded array, truncating on a **character** boundary.
///
/// ⚠️ Truncating by bytes would split a multi-byte UTF-8 sequence and make [`cstr`] return `""` for
/// the whole field — so a device whose name happened to be one byte too long would lose its name
/// entirely rather than lose its last character.
fn set_cstr(buf: &mut [u8], s: &str) {
    buf.fill(0);
    let mut end = s.len().min(buf.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    buf[..end].copy_from_slice(&s.as_bytes()[..end]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_device_is_absent_and_claims_nothing() {
        let d = QpuInfo::default();
        assert_eq!(d.status(), QpuStatus::NotPresent);
        assert_eq!(d.qubits, 0);
        assert!(!d.is_quantum());
        assert_eq!(d.status_label(), "NOT PRESENT");
        // "not reported" must not read as "reported zero".
        assert_eq!(d.coherence_t1(), None);
        assert_eq!(d.queue(), None);
    }

    #[test]
    fn raw_round_trips_through_the_syscall_representation() {
        for s in [
            QpuStatus::NotPresent,
            QpuStatus::Simulator,
            QpuStatus::Remote,
            QpuStatus::Hardware,
        ] {
            assert_eq!(QpuStatus::from_raw(s as u32), s);
        }
        for k in [
            QuantumDeviceKind::Qpu,
            QuantumDeviceKind::QuantumControlElectronics,
            QuantumDeviceKind::QuantumRng,
            QuantumDeviceKind::UnidentifiedAccelerator,
        ] {
            assert_eq!(QuantumDeviceKind::from_raw(k as u32), k);
        }
        for t in [
            Topology::Unknown,
            Topology::AllToAll,
            Topology::Linear,
            Topology::Grid,
            Topology::Explicit,
        ] {
            assert_eq!(Topology::from_raw(t as u32), t);
        }
    }

    #[test]
    fn an_unknown_status_from_a_newer_kernel_reads_as_absent() {
        // Version skew must fail closed: an unrecognised status is not evidence of a QPU.
        assert_eq!(QpuStatus::from_raw(9999), QpuStatus::NotPresent);
        assert!(!QpuStatus::from_raw(9999).is_quantum(false));
    }

    /// ★ The central honesty invariant. If this test ever fails, the subsystem is lying.
    #[test]
    fn only_hardware_and_real_remote_devices_count_as_quantum() {
        assert!(!QpuStatus::NotPresent.is_quantum(false));
        assert!(!QpuStatus::Simulator.is_quantum(false));
        assert!(QpuStatus::Hardware.is_quantum(false));

        // The trap: a cloud simulator arrives through the same API as a cloud QPU.
        assert!(QpuStatus::Remote.is_quantum(false), "remote hardware is quantum");
        assert!(
            !QpuStatus::Remote.is_quantum(true),
            "a remote simulator is NOT a quantum result"
        );
    }

    #[test]
    fn remote_simulator_and_remote_hardware_are_labelled_differently() {
        let mut hw = QpuInfo::default();
        hw.status = QpuStatus::Remote as u32;
        let mut sim = hw;
        sim.remote_is_simulator = 1;

        assert_eq!(hw.status_label(), "REMOTE/hw");
        assert_eq!(sim.status_label(), "REMOTE/sim");
        assert_ne!(hw.status_label(), sim.status_label());
        assert!(hw.is_quantum());
        assert!(!sim.is_quantum());
        // The one a reader is most likely to misread gets spelled out.
        assert!(sim.caveat().unwrap().contains("classical simulator"));
    }

    #[test]
    fn only_a_qpu_is_ever_an_executor() {
        assert!(QuantumDeviceKind::Qpu.is_executor());
        // A QRNG is real quantum hardware that cannot run a circuit.
        assert!(!QuantumDeviceKind::QuantumRng.is_executor());
        // A controller is the classical half of a quantum computer.
        assert!(!QuantumDeviceKind::QuantumControlElectronics.is_executor());
        // And a card we cannot identify is not evidence of anything.
        assert!(!QuantumDeviceKind::UnidentifiedAccelerator.is_executor());
    }

    #[test]
    fn strings_round_trip_and_truncate_without_destroying_the_field() {
        let mut d = QpuInfo::default();
        d.set_vendor("Nyx");
        d.set_arch("statevector");
        d.set_name("Nyx state-vector simulator");
        assert_eq!(d.vendor_str(), "Nyx");
        assert_eq!(d.arch_str(), "statevector");
        assert_eq!(d.name_str(), "Nyx state-vector simulator");

        // Exactly-full: no NUL terminator, must still read back whole.
        let full = "a".repeat(VENDOR_LEN);
        d.set_vendor(&full);
        assert_eq!(d.vendor_str(), full);

        // Over-long ASCII truncates to the field width.
        d.set_vendor(&"b".repeat(VENDOR_LEN + 10));
        assert_eq!(d.vendor_str().len(), VENDOR_LEN);
    }

    #[test]
    fn truncation_on_a_multibyte_boundary_keeps_the_rest_of_the_name() {
        // A byte-wise cut would split the final 'é' and make the WHOLE field unreadable.
        let mut d = QpuInfo::default();
        let s = "é".repeat(VENDOR_LEN); // 64 bytes, 32 chars
        d.set_vendor(&s);
        let got = d.vendor_str();
        assert!(!got.is_empty(), "a split code point destroyed the entire field");
        assert!(s.starts_with(got));
        assert!(got.chars().all(|c| c == 'é'));
    }

    #[test]
    fn abi_layout_is_pinned() {
        // Mirrored by hand in libs/api and nyx-kernel/src/quantum.rs. If this changes, so must they.
        assert_eq!(core::mem::size_of::<QpuInfo>(), QPU_INFO_SIZE);
        assert_eq!(core::mem::align_of::<QpuInfo>(), 8);
    }
}
