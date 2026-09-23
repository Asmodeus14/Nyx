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
//! that. So the result also answers a real question: does the touchpad (ELAN 04f3:30cb, not the ALPS the address suggested) answer on I2C at all
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
    /// Report descriptor read and parsed (Phase 3a), but no input report arrived while watching.
    pub const REPORT_DESCRIPTOR: u32 = 6;
    /// Input reports arrived on the I2C input register.
    pub const REPORTS_SEEN: u32 = 7;
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
    /// Where every other device's claims above 4 GiB provably end, and the address BAR0 was
    /// offered — reported even when the checks refused it.
    pub claims_end_above_4g: u64,
    pub candidate: u64,
    /// The window the candidate had to fall inside.
    pub m64_base: u64,
    pub m64_len: u64,
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
        bar0_before: 0, touud: 0, claims_end_above_4g: 0, candidate: 0, m64_base: 0, m64_len: 0, bar0_size: 0, assigned: 0,
        phys_bits: 0, _pad: 0,
    };
}

const _: () = assert!(core::mem::size_of::<ProbeResult>() == 168);

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
    // A probe that stops early must not leave the previous run's findings looking current.
    publish(&Sink { len: 0, text: [0; REPORT_CAP] });

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
    r.claims_end_above_4g = info.claims_end_above_4g;
    r.candidate = info.candidate;
    r.m64_base = hid.m64_base;
    r.m64_len = hid.m64_len;
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
        Err(I2cError::TooLong) | Err(I2cError::Underrun) => {
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

    // Phase 3a — read-only from here on.
    if explore(&mut ctl, hid.slave_addr as u8, &buf) {
        r.stage = stage::REPORTS_SEEN;
    } else {
        r.stage = stage::REPORT_DESCRIPTOR;
    }
    r
}

/// Human-readable findings from [`explore`], copied out by syscall 578 op 2.
pub static REPORT: spin::Mutex<Report> = spin::Mutex::new(Report { len: 0, text: [0; REPORT_CAP] });
pub const REPORT_CAP: usize = 1536;

pub struct Report {
    pub len: usize,
    pub text: [u8; REPORT_CAP],
}

/// A bounded `fmt::Write` sink — the report text simply stops at the cap rather than failing.
struct Sink {
    len: usize,
    text: [u8; REPORT_CAP],
}

impl core::fmt::Write for Sink {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < REPORT_CAP {
                self.text[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

/// How long to watch the input register, and how often.
const SAMPLE_MS: u64 = 3000;
const SAMPLE_EVERY_MS: u64 = 4;

/// Phase 3a: read the report descriptor, parse it, then watch the input register while the user
/// moves a finger. Returns true if any input report arrived.
///
/// ⚠️ READ-ONLY. No RESET, no SET_POWER, no Set Feature, no `_DSM`. The touchpad is currently the
/// machine's pointer over PS/2, and any of those could switch it off that path before this driver
/// can replace it. What this answers is whether reports already flow on I2C as things stand.
fn explore(ctl: &mut crate::drivers::i2c::Controller, addr: u8, desc: &[u8]) -> bool {
    use crate::drivers::hid_desc;
    use core::fmt::Write;

    let w = |i: usize| u16::from_le_bytes([desc[i], desc[i + 1]]);
    let (rd_len, rd_reg, in_reg, max_in) = (w(4) as usize, w(6), w(8), w(10) as usize);
    let mut out = Sink { len: 0, text: [0; REPORT_CAP] };

    // 1. The report descriptor. Bounded: a real one is a few hundred bytes.
    let rd_len = rd_len.min(2048);
    let mut rdesc = alloc::vec![0u8; rd_len];
    let mut read = Err(crate::drivers::i2c::I2cError::Timeout);
    for _ in 0..3 {
        read = ctl.write_read(addr, &rd_reg.to_le_bytes(), &mut rdesc);
        if !matches!(read, Err(crate::drivers::i2c::I2cError::Underrun)) {
            break;
        }
    }
    if let Err(e) = read {
        let _ = writeln!(out, "report descriptor: read FAILED ({:?})", e);
        publish(&out);
        return false;
    }
    let d = hid_desc::parse(&rdesc);
    let _ = writeln!(out, "report descriptor: {} bytes, {} input fields, report IDs {}{}",
        rd_len, d.fields.len(), if d.uses_report_ids { "yes" } else { "no" },
        if d.trailing != 0 { " (TRUNCATED)" } else { "" });
    for &(page, u, id) in &d.apps {
        let name = match (page, u) {
            (0x01, 0x02) => "mouse",
            (0x01, 0x01) => "pointer",
            (0x0D, 0x05) => "touch pad",
            (0x0D, 0x04) => "touch screen",
            (0x0D, 0x0E) => "device config",
            (0x01, 0x06) => "keyboard",
            _ => "other",
        };
        let _ = writeln!(out, "  app {:02x}:{:02x} {:<13} report id {}", page, u, name, id);
    }
    let mouse = hid_desc::mouse_layout(&d);
    match &mouse {
        Some(m) => {
            let _ = writeln!(out, "  mouse layout: id {} x@{}+{} y@{}+{} btn1@{}",
                m.report_id, m.x.bit_off, m.x.size, m.y.bit_off, m.y.size,
                m.buttons[0].map_or(-1, |b| b.bit_off as i32));
        }
        None => {
            let _ = writeln!(out, "  no relative mouse collection");
        }
    }

    // 2. Watch the input register. Each read returns [len_lo, len_hi, report...]; len 0 means
    //    nothing pending (the spec's reset sentinel is also len 0).
    let max_in = max_in.clamp(2, 64);
    let mut buf = [0u8; 64];
    let (mut reads, mut errors, mut reports) = (0u32, 0u32, 0u32);
    let mut per_id: [(u8, u32); 8] = [(0, 0); 8];
    let mut shown = 0;
    let (mut sum_dx, mut sum_dy, mut clicks) = (0i32, 0i32, 0u32);
    let mut elapsed = 0u64;
    while elapsed < SAMPLE_MS {
        reads += 1;
        match ctl.write_read(addr, &in_reg.to_le_bytes(), &mut buf[..max_in]) {
            Err(_) => errors += 1,
            Ok(()) => {
                let len = u16::from_le_bytes([buf[0], buf[1]]) as usize;
                if len > 2 && len <= max_in {
                    reports += 1;
                    let id = if d.uses_report_ids { buf[2] } else { 0 };
                    // Slots fill in order, so a matching slot always precedes the first empty one.
                    if let Some(slot) = per_id.iter_mut().find(|(i, n)| *i == id || *n == 0) {
                        slot.0 = id;
                        slot.1 += 1;
                    }
                    if shown < 4 {
                        shown += 1;
                        let _ = write!(out, "  sample:");
                        for b in &buf[..len] {
                            let _ = write!(out, " {:02x}", b);
                        }
                        let _ = writeln!(out);
                    }
                    if let Some(m) = &mouse {
                        if id == m.report_id {
                            let body = &buf[if d.uses_report_ids { 3 } else { 2 }..len];
                            sum_dx += m.x.extract(body).unwrap_or(0);
                            sum_dy += m.y.extract(body).unwrap_or(0);
                            if m.buttons[0].and_then(|b| b.extract(body)).unwrap_or(0) != 0 {
                                clicks += 1;
                            }
                        }
                    }
                }
            }
        }
        crate::scheduler::kernel_sleep_ms(SAMPLE_EVERY_MS);
        elapsed += SAMPLE_EVERY_MS;
    }
    let _ = write!(out, "input register {:#06x}: {} reads, {} errors, {} reports", in_reg, reads, errors, reports);
    for (id, n) in per_id.iter().filter(|(_, n)| *n != 0) {
        let _ = write!(out, "  [id {}: {}]", id, n);
    }
    let _ = writeln!(out);
    if mouse.is_some() && reports != 0 {
        let _ = writeln!(out, "decoded as mouse: total dx {} dy {}, button-1 reports {}", sum_dx, sum_dy, clicks);
    }
    publish(&out);
    reports != 0
}

fn publish(s: &Sink) {
    // `try_lock` in a loop rather than `lock`: the syscall side holds it at IF=0, only for a copy.
    loop {
        if let Some(mut g) = REPORT.try_lock() {
            g.len = s.len;
            g.text = s.text;
            return;
        }
        core::hint::spin_loop();
    }
}
