//! The kernel's quantum device registry — discovery and introspection, nothing else.
//!
//! This module is deliberately tiny, and the reasons are worth stating because "the OS supports
//! quantum computing" invites a much larger kernel component than is justified.
//!
//! **It holds no gates, no circuits, no simulator and no networking.** It holds a table of what the
//! PCI probe actually found, and answers two read-only syscalls (573, 574). Circuit execution, the
//! state-vector simulator, backend selection and every cloud provider live in userspace —
//! `libs/quantum` and `libs/quantum-rt`.
//!
//! ## Why not more
//!
//! 1. **There is nothing here to arbitrate for.** No consumer gate-model QPU exists that Nyx can
//!    drive over PCIe — see `docs/quantum/limitations.md`. A submission queue, a job scheduler and
//!    a fairness policy for a device class that is absent from every machine this OS runs on would
//!    be code whose only function is to make support look like it exists. When a real device is
//!    bound, the queue goes in beside the driver that binds it.
//!
//! 2. **The kernel has no async model.** Every syscall in Nyx is synchronous; the GPU submits and
//!    blocks on a polled fence. The one non-blocking idiom in the tree is a *userspace* state
//!    machine (`libs/net`'s `Fetch`), and a quantum job — which for a cloud provider sits in a queue
//!    for minutes — is exactly that shape.
//!
//! 3. **Remote devices are none of the kernel's business.** They are discovered over HTTPS by the
//!    userspace runtime and merged into the device list there. No token, URL or circuit ever
//!    reaches ring 0.
//!
//! ## Lock discipline: there is no lock
//!
//! `FMASK = IF`, so every syscall body runs with interrupts disabled. A lock held by a kernel task
//! at IF=1 and taken from a syscall at IF=0 is a hard deadlock that also kills the thermal governor
//! — the hazard `drivers/net/mod.rs` documents at length and that `SysMetrics` was designed around.
//!
//! So this registry takes **no lock at all**. It is written once, during `pci::enumerate_pci()`, on
//! the BSP, before AP bring-up completes and long before userspace exists; after that it is
//! immutable and read-only. `COUNT` is published with a `Release` store and read with `Acquire`, so
//! a reader that sees the count also sees the entries.
//!
//! ## ⚠️ The probe never touches the device
//!
//! [`probe_pci`] reads the identity a caller has *already read* out of config space and records it.
//! It does not map a BAR, does not enable bus mastering, does not write a single register. There is
//! no IOMMU in this kernel (a full-tree grep for `iommu`/`DMAR` returns nothing), so a device
//! permitted to master the bus can read and write all of physical memory. Enabling that for a card
//! nobody has identified would be indefensible.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Mirror of `nyx_quantum::device::QPU_INFO_SIZE`.
///
/// ⚠️ Hand-mirrored, like `SysMetrics` and `WindowQuad`: the kernel cannot link a userspace crate,
/// so the struct below is a copy and **field order is the ABI**. Three places must agree —
/// `libs/quantum/src/device.rs`, `libs/api/src/lib.rs`, and here — and all three carry this
/// assertion so a field addition breaks the build rather than silently reinterpreting every later
/// field.
pub const QPU_INFO_SIZE: usize = 224;

/// Status values. Mirror of `nyx_quantum::QpuStatus`.
pub const QPU_STATUS_NOT_PRESENT: u32 = 0;
#[allow(dead_code)]
pub const QPU_STATUS_SIMULATOR: u32 = 1;
#[allow(dead_code)]
pub const QPU_STATUS_REMOTE: u32 = 2;
/// ★ The only place in the entire tree that may produce this value is [`probe_pci`], on a positive
/// match against [`KNOWN_QUANTUM_DEVICES`]. Userspace cannot construct it.
#[allow(dead_code)]
pub const QPU_STATUS_HARDWARE: u32 = 3;

/// Kind values. Mirror of `nyx_quantum::QuantumDeviceKind`.
#[allow(dead_code)]
pub const QPU_KIND_QPU: u32 = 0;
pub const QPU_KIND_CONTROL: u32 = 1;
pub const QPU_KIND_RNG: u32 = 2;
pub const QPU_KIND_UNIDENTIFIED: u32 = 3;

/// Topology values. Mirror of `nyx_quantum::Topology`.
pub const QPU_TOPOLOGY_UNKNOWN: u32 = 0;

/// Mirror of `nyx_quantum::device::QpuInfo`. **Append-only. Field order is the ABI.**
#[derive(Clone, Copy)]
#[repr(C)]
pub struct QpuInfo {
    pub id: u32,
    pub kind: u32,
    pub status: u32,
    pub remote_is_simulator: u32,

    pub qubits: u32,
    pub topology: u32,
    pub exec_model: u32,
    pub _reserved0: u32,

    pub gate_set: u64,

    pub max_shots: u32,
    pub max_depth: u32,
    pub queue_depth: u32,
    pub queue_capacity: u32,

    pub coherence_t1_ns: u64,
    pub coherence_t2_ns: u64,
    pub calibrated_unix: u64,

    pub pci_bdf: u32,
    pub pci_vendor_id: u32,
    pub pci_device_id: u32,
    pub _reserved1: u32,

    pub vendor: [u8; 32],
    pub arch: [u8; 32],
    pub name: [u8; 64],
}

const _: () = assert!(core::mem::size_of::<QpuInfo>() == QPU_INFO_SIZE);
const _: () = assert!(core::mem::align_of::<QpuInfo>() == 8);

impl QpuInfo {
    /// A device that is not there. `const` so the registry can be a static array.
    pub const fn absent() -> QpuInfo {
        QpuInfo {
            id: 0,
            kind: QPU_KIND_QPU,
            status: QPU_STATUS_NOT_PRESENT,
            remote_is_simulator: 0,
            qubits: 0,
            topology: QPU_TOPOLOGY_UNKNOWN,
            exec_model: 0,
            _reserved0: 0,
            gate_set: 0,
            max_shots: 0,
            max_depth: 0,
            // `u32::MAX` is the model's "no queue applies", distinct from "the queue is empty".
            queue_depth: u32::MAX,
            queue_capacity: 0,
            coherence_t1_ns: 0,
            coherence_t2_ns: 0,
            calibrated_unix: 0,
            pci_bdf: 0,
            pci_vendor_id: 0,
            pci_device_id: 0,
            _reserved1: 0,
            vendor: [0; 32],
            arch: [0; 32],
            name: [0; 64],
        }
    }

    fn set_str(dst: &mut [u8], s: &str) {
        dst.fill(0);
        let n = s.len().min(dst.len());
        dst[..n].copy_from_slice(&s.as_bytes()[..n]);
    }
}

/// How many devices the registry can hold.
///
/// Eight. A machine with more than eight quantum devices on its PCI bus is not a machine this
/// kernel is going to be running on, and a fixed array is what lets the syscall read without a lock.
const MAX_DEVICES: usize = 8;

static mut REGISTRY: [QpuInfo; MAX_DEVICES] = [QpuInfo::absent(); MAX_DEVICES];

/// Number of valid entries in [`REGISTRY`].
///
/// Published `Release` after the entry is written; read `Acquire`. See the module docs.
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// PCI devices Nyx recognises as quantum hardware.
///
/// ★ **This table is empty, and that is the correct state.**
///
/// Nyx has no verified vendor/device ID for any quantum processor. The entries that would go here
/// do not exist in any public form: cryogenic QPUs are not PCI devices at all, quantum control
/// electronics are Ethernet-attached appliances driven by vendor SDKs, and the one genuinely
/// PCI-attached piece of quantum hardware — a QRNG card — is an entropy source rather than a
/// processor. `docs/quantum/limitations.md` has the full account.
///
/// Adding a speculative entry here would be the single most damaging thing anyone could do to this
/// subsystem: it is the only code path in the tree that can produce [`QPU_STATUS_HARDWARE`], and a
/// wrong guess would make Nyx claim a quantum processor it does not have.
///
/// **Do not add an entry you have not verified against real silicon.**
static KNOWN_QUANTUM_DEVICES: &[KnownDevice] = &[];

/// One recognised device: an exact `(vendor, device)` match and what it actually is.
struct KnownDevice {
    #[allow(dead_code)]
    vendor_id: u16,
    #[allow(dead_code)]
    device_id: u16,
    #[allow(dead_code)]
    kind: u32,
    #[allow(dead_code)]
    vendor: &'static str,
    #[allow(dead_code)]
    arch: &'static str,
    #[allow(dead_code)]
    name: &'static str,
    /// Qubit count, where a fixed one is meaningful. `0` means "not known from the ID alone".
    #[allow(dead_code)]
    qubits: u32,
}

/// PCI class `0x12` — "Processing Accelerators", a real PCI-SIG assignment.
const PCI_CLASS_PROCESSING_ACCELERATOR: u8 = 0x12;
/// PCI class `0xFF` — "Unassigned". Where anything nobody classified ends up.
const PCI_CLASS_UNASSIGNED: u8 = 0xFF;

/// Initialise the registry. Call once, immediately after `pci::enumerate_pci()`.
///
/// The enumeration is what populates the table — this only reports the result, so it must run
/// after. It is called from **both** branches of `main.rs`'s ACPI test, because the degraded
/// no-ACPI path also enumerates PCI and a device that only registers on one boot path is a device
/// that works by luck.
pub fn init() {
    let n = COUNT.load(Ordering::Acquire);
    if n == 0 {
        crate::serial_println!("[QPU] no quantum devices on this machine (expected)");
        return;
    }
    for i in 0..n {
        let d = devices()[i];
        crate::serial_println!(
            "[QPU] {}: kind={} status={} {:04x}:{:04x}",
            i,
            d.kind,
            d.status,
            d.pci_vendor_id,
            d.pci_device_id
        );
    }
}

/// Offer one PCI device to the quantum subsystem.
///
/// Called from both `pci::enumerate_pci` paths with identity a caller has **already read** out of
/// config space. Reads nothing, writes nothing, maps nothing — see the module docs.
///
/// Two outcomes:
///
/// * An exact match in [`KNOWN_QUANTUM_DEVICES`] — recorded as real hardware. Cannot happen today;
///   that table is empty.
/// * A processing-accelerator or unassigned-class device — recorded as
///   [`QPU_KIND_UNIDENTIFIED`] with status [`QPU_STATUS_NOT_PRESENT`]. This is a **diagnostic, not
///   a claim**: class `0x12` is shared with AI inference cards and research FPGAs, so the class code
///   cannot tell us the device is quantum. It is recorded rather than logged because this laptop
///   has no serial console, and anything the probe learns and only logs is learned and thrown away.
///
/// Everything else is ignored.
pub fn probe_pci(bus: u8, device: u8, func: u8, vendor_id: u16, device_id: u16, class: u8, subclass: u8) {
    let _ = subclass;

    let known = KNOWN_QUANTUM_DEVICES
        .iter()
        .find(|k| k.vendor_id == vendor_id && k.device_id == device_id);

    let is_candidate =
        class == PCI_CLASS_PROCESSING_ACCELERATOR || class == PCI_CLASS_UNASSIGNED;

    if known.is_none() && !is_candidate {
        return;
    }

    let idx = COUNT.load(Ordering::Relaxed);
    if idx >= MAX_DEVICES {
        return;
    }

    let mut info = QpuInfo::absent();
    info.id = idx as u32;
    info.pci_bdf = ((bus as u32) << 16) | ((device as u32) << 8) | (func as u32);
    info.pci_vendor_id = vendor_id as u32;
    info.pci_device_id = device_id as u32;

    match known {
        Some(k) => {
            // ★ The only path to QPU_STATUS_HARDWARE anywhere in Nyx. Unreachable today.
            info.kind = k.kind;
            info.status = QPU_STATUS_HARDWARE;
            info.qubits = k.qubits;
            QpuInfo::set_str(&mut info.vendor, k.vendor);
            QpuInfo::set_str(&mut info.arch, k.arch);
            QpuInfo::set_str(&mut info.name, k.name);
        }
        None => {
            // Seen, not identified, not claimed.
            info.kind = QPU_KIND_UNIDENTIFIED;
            info.status = QPU_STATUS_NOT_PRESENT;
            QpuInfo::set_str(&mut info.vendor, "unknown");
            QpuInfo::set_str(
                &mut info.arch,
                if class == PCI_CLASS_PROCESSING_ACCELERATOR {
                    "pci-accelerator"
                } else {
                    "pci-unassigned"
                },
            );
            QpuInfo::set_str(&mut info.name, "unidentified PCI accelerator");
        }
    }

    unsafe {
        let slot = core::ptr::addr_of_mut!(REGISTRY) as *mut QpuInfo;
        core::ptr::write(slot.add(idx), info);
    }
    // Release: a reader that observes the new count also observes the entry above.
    COUNT.store(idx + 1, Ordering::Release);
}

/// The valid part of the registry.
///
/// Safe to call from a syscall arm at IF=0: no lock, no allocation, no device access.
pub fn devices() -> &'static [QpuInfo] {
    let n = COUNT.load(Ordering::Acquire).min(MAX_DEVICES);
    unsafe {
        let base = core::ptr::addr_of!(REGISTRY) as *const QpuInfo;
        core::slice::from_raw_parts(base, n)
    }
}

/// How many devices the probe found. Usually — and correctly — zero.
pub fn count() -> usize {
    COUNT.load(Ordering::Acquire).min(MAX_DEVICES)
}

/// One device by id, or `None`.
pub fn device(id: u32) -> Option<QpuInfo> {
    devices().iter().find(|d| d.id == id).copied()
}
