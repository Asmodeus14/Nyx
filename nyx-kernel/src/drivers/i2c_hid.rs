//! I2C-HID touchpad: bring-up probe (Phase 2).
//!
//! Brings up the LPSS I2C controller that ACPI discovery found, then reads the device's 30-byte HID
//! descriptor. A valid descriptor proves the whole I2C path end to end — PCI power, LPSS reset, bus
//! timing, addressing — before any HID parsing is written on top of it.
//!
//! ★ Runs in the `usb-hid` kernel task (IF=1), on request. Never in a syscall: bring-up sleeps
//! (the D3→D0 transition needs 10 ms), and a syscall runs with interrupts masked. The syscall only
//! raises [`REQUEST`] and copies [`RESULT`] back — the same publish/copy split the ACPI cache uses.
//!
//! ⚠️ This does NOT perform the PS/2 handover. The EC keeps the touchpad on PS/2 until the device's
//! HIDG `_DSM` is evaluated (`custom_acpi.c`, `NyxHidDescriptorRegister`), and nothing here does
//! that. So the result also answers a real question: does this ALPS part answer on I2C at all
//! while it is still in PS/2 mode? An address NACK here is an answer, not necessarily a bug.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::i2c::{BringUpError, BringUpInfo, Controller, I2cError};

/// Set by the syscall, consumed by the kernel task.
pub static REQUEST: AtomicBool = AtomicBool::new(false);

/// Last probe outcome. `try_lock` on both sides.
pub static RESULT: spin::Mutex<ProbeResult> = spin::Mutex::new(ProbeResult::EMPTY);

/// How far the probe got. Each stage implies all the ones before it.
pub mod stage {
    pub const NONE: u32 = 0;
    pub const ACPI: u32 = 1;
    pub const PCI: u32 = 2;
    pub const CONTROLLER: u32 = 3;
    pub const DESCRIPTOR_READ: u32 = 4;
    pub const DESCRIPTOR_VALID: u32 = 5;
}

/// Why it stopped. 0 = it did not stop — every stage passed.
pub mod status {
    pub const OK: u32 = 0;
    pub const NO_ACPI: u32 = 1;
    pub const PCI_ABSENT: u32 = 2;
    pub const NO_SAFE_ADDRESS: u32 = 3;
    pub const MAP_FAILED: u32 = 4;
    pub const NOT_DESIGNWARE: u32 = 5;
    pub const DISABLE_TIMEOUT: u32 = 6;
    pub const ABORT: u32 = 7;
    pub const TIMEOUT: u32 = 8;
    pub const BAD_DESCRIPTOR: u32 = 9;
    pub const TOO_LONG: u32 = 10;
    pub const UNEXPECTED_BAR: u32 = 11;
    pub const ASSIGN_FAILED: u32 = 12;
}

/// Published probe outcome. Mirrored field-for-field by `nyx_api::I2cHidProbe` — keep them in step.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProbeResult {
    /// Bumped once per completed probe, so a reader can tell a fresh result from the last one.
    pub seq: u32,
    pub stage: u32,
    pub status: u32,
    /// `IC_TX_ABRT_SOURCE` when `status == ABORT`. Bit 0 = address not acknowledged.
    pub abort_source: u32,
    pub vendor_device: u32,
    pub pmcsr_before: u32,
    pub resets_before: u32,
    pub comp_type: u32,
    pub comp_param1: u32,
    /// 1 = standard, 2 = fast.
    pub mode: u32,
    pub hcnt: u32,
    pub lcnt: u32,
    pub hold: u32,
    pub timing_from_fw: u32,
    pub slave_addr: u32,
    pub desc_reg: u32,
    pub bar0: u64,
    /// The raw descriptor bytes as read — printed whole, so a garbled one can be diagnosed.
    pub desc: [u8; 32],
    /// BAR0 as the firmware left it. 0 on this laptop — see `drivers::i2c::ASSIGN_BASE`.
    pub bar0_before: u64,
    /// The facts the BAR assignment was checked against.
    pub touud: u64,
    pub highest_bar_above_4g: u64,
    pub bar0_size: u32,
    /// 1 if this probe gave BAR0 its address.
    pub assigned: u32,
    pub phys_bits: u32,
    pub _pad: u32,
}

impl ProbeResult {
    pub const EMPTY: ProbeResult = ProbeResult {
        seq: 0, stage: 0, status: 0, abort_source: 0, vendor_device: 0, pmcsr_before: 0,
        resets_before: 0, comp_type: 0, comp_param1: 0, mode: 0, hcnt: 0, lcnt: 0, hold: 0,
        timing_from_fw: 0, slave_addr: 0, desc_reg: 0, bar0: 0, desc: [0; 32],
        bar0_before: 0, touud: 0, highest_bar_above_4g: 0, bar0_size: 0, assigned: 0,
        phys_bits: 0, _pad: 0,
    };
}

const _: () = assert!(core::mem::size_of::<ProbeResult>() == 144);

/// Length of an I2C-HID descriptor, and the only `wHIDDescLength` the spec allows.
const HID_DESC_LEN: usize = 30;

/// Called from the `usb-hid` task loop. Cheap when no probe was requested.
pub fn service() {
    if !REQUEST.swap(false, Ordering::AcqRel) {
        return;
    }
    let mut r = run();
    if let Some(mut g) = RESULT.try_lock() {
        r.seq = g.seq.wrapping_add(1);
        *g = r;
    } else {
        // The syscall is mid-copy. Ask again next tick rather than drop the answer.
        REQUEST.store(true, Ordering::Release);
    }
}

fn run() -> ProbeResult {
    let mut r = ProbeResult::EMPTY;

    // The ACPI half is published by `acpi probe 13`; take a copy and drop the lock at once.
    let hid = match crate::acpi::CACHE.try_lock() {
        Some(c) if c.i2c_hid_probed && c.i2c_hid_n > 0 => c.i2c_hid[0],
        _ => {
            r.status = status::NO_ACPI;
            return r;
        }
    };
    r.stage = stage::ACPI;
    r.slave_addr = hid.slave_addr;
    r.desc_reg = hid.hid_desc_reg;

    let (dev, func) = hid.pci_dev_func();
    let mut info = BringUpInfo::default();
    let brought = Controller::bring_up(0, dev, func, hid.speed_hz, &hid, &mut info);
    r.vendor_device = info.vendor_device;
    r.pmcsr_before = info.pmcsr_before;
    r.resets_before = info.resets_before;
    r.comp_type = info.comp_type;
    r.comp_param1 = info.comp_param1;
    r.bar0 = info.bar0;
    r.bar0_before = info.bar0_before;
    r.touud = info.touud;
    r.highest_bar_above_4g = info.highest_bar_above_4g;
    r.bar0_size = info.bar0_size;
    r.assigned = info.assigned as u32;
    r.phys_bits = info.phys_bits;
    let mut ctl = match brought {
        Ok(c) => c,
        Err(e) => {
            if !matches!(e, BringUpError::Absent) {
                r.stage = stage::PCI;
            }
            r.status = match e {
                BringUpError::Absent => status::PCI_ABSENT,
                BringUpError::NoSafeAddress => status::NO_SAFE_ADDRESS,
                BringUpError::UnexpectedBar => status::UNEXPECTED_BAR,
                BringUpError::AssignFailed => status::ASSIGN_FAILED,
                BringUpError::MapFailed => status::MAP_FAILED,
                BringUpError::NotDesignware(_) => status::NOT_DESIGNWARE,
                BringUpError::DisableTimeout => status::DISABLE_TIMEOUT,
            };
            return r;
        }
    };
    r.stage = stage::CONTROLLER;
    r.mode = ctl.timing.mode;
    r.hcnt = ctl.timing.hcnt;
    r.lcnt = ctl.timing.lcnt;
    r.hold = ctl.timing.hold;
    r.timing_from_fw = ctl.timing.from_firmware as u32;

    // The descriptor register is a 16-bit register number, sent little-endian.
    let reg = hid.hid_desc_reg as u16;
    let mut buf = [0u8; HID_DESC_LEN];
    match ctl.write_read(hid.slave_addr as u8, &reg.to_le_bytes(), &mut buf) {
        Ok(()) => {}
        Err(I2cError::Abort(src)) => {
            r.status = status::ABORT;
            r.abort_source = src;
            return r;
        }
        Err(I2cError::Timeout) => {
            r.status = status::TIMEOUT;
            return r;
        }
        Err(I2cError::TooLong) => {
            r.status = status::TOO_LONG;
            return r;
        }
    }
    r.stage = stage::DESCRIPTOR_READ;
    r.desc[..HID_DESC_LEN].copy_from_slice(&buf);

    // wHIDDescLength must be 30 and bcdVersion 1.00. Both at once is not a coincidence.
    let len = u16::from_le_bytes([buf[0], buf[1]]);
    let ver = u16::from_le_bytes([buf[2], buf[3]]);
    if len as usize != HID_DESC_LEN || ver != 0x0100 {
        r.status = status::BAD_DESCRIPTOR;
        return r;
    }
    r.stage = stage::DESCRIPTOR_VALID;
    r
}
