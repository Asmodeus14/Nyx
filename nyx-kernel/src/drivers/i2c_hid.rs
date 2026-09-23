//! I2C-HID touchpad (ELAN 04f3:30cb on the test laptop): probe, initialise, and drive the pointer.
//!
//! Brings up the LPSS I2C controller that ACPI discovery found, reads the 30-byte HID descriptor and
//! the report descriptor, and — only on `touchpad on` — sends SET_POWER + RESET and, if mouse
//! reports then arrive, polls them into the pointer ([`POINTER_ACTIVE`], Phase 4).
//!
//! ★ Runs in the `usb-hid` kernel task (IF=1), on request. Never in a syscall: bring-up sleeps
//! (the D3→D0 transition needs 10 ms), and a syscall runs with interrupts masked. The syscall only
//! raises flags and copies results back — the same publish/copy split the ACPI cache uses.
//!
//! ⚠️ The firmware `_DSM` handover (EC stops PS/2 mouse emulation) is NOT done here. It is
//! `acpi probe 14`, run by `touchpad handover` — deliberate, separate, and not undoable before a
//! reboot. Hardware showed the device answers on I2C without it, but sent no input reports without
//! initialisation (750 reads, 0 reports).

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
    /// 1 if, after this probe, the I2C touchpad drives the pointer.
    pub active: u32,
}

impl ProbeResult {
    pub const EMPTY: ProbeResult = ProbeResult {
        seq: 0, stage: 0, status: 0, abort_source: 0, vendor_device: 0, pmcsr_before: 0,
        resets_before: 0, comp_type: 0, comp_param1: 0, mode: 0, hcnt: 0, lcnt: 0, hold: 0,
        timing_from_fw: 0, slave_addr: 0, desc_reg: 0, bar0: 0, desc: [0; 32],
        bar0_before: 0, touud: 0, claims_end_above_4g: 0, candidate: 0, m64_base: 0, m64_len: 0, bar0_size: 0, assigned: 0,
        phys_bits: 0, active: 0,
    };
}

const _: () = assert!(core::mem::size_of::<ProbeResult>() == 168);

/// Length of an I2C-HID descriptor, and the only `wHIDDescLength` the spec allows.
const HID_DESC_LEN: usize = 30;

/// Set with [`REQUEST`] by `touchpad on`: after probing, initialise the device (SET_POWER, RESET)
/// and, if mouse reports then arrive, make it the pointer.
pub static ENABLE: AtomicBool = AtomicBool::new(false);
/// Set by `touchpad off`: stop driving the pointer from I2C and hand it back to PS/2.
pub static DISABLE: AtomicBool = AtomicBool::new(false);

/// ★ Phase 4. True while the I2C touchpad drives the pointer. The PS/2 AUX handler drops every byte
/// while this is set — otherwise a touchpad that still reports on BOTH paths moves the cursor twice.
/// Cleared again if the I2C side stops answering, so a failure falls back to PS/2 rather than
/// leaving the machine without a pointer.
pub static POINTER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The live device, owned by the `usb-hid` task once `touchpad on` succeeds.
struct Live {
    ctl: Controller,
    addr: u8,
    in_reg: u16,
    max_in: usize,
    uses_ids: bool,
    mouse: crate::drivers::hid_desc::MouseLayout,
    /// Consecutive failed reads. Too many and the pointer goes back to PS/2.
    errors: u32,
    /// Sub-pixel remainder of scaled motion, in hundredths of a pixel. Carried between reports so
    /// that below 100% a slow finger still moves the pointer instead of every small delta
    /// truncating to zero.
    acc_x: i32,
    acc_y: i32,
}

static LIVE: spin::Mutex<Option<Live>> = spin::Mutex::new(None);

/// Consecutive read failures after which I2C is abandoned for PS/2 — 50 × 4 ms = 200 ms of silence
/// from a bus that normally never fails.
const MAX_CONSECUTIVE_ERRORS: u32 = 50;

/// Pointer speed as a percentage of raw report counts. `touchpad speed <pct>` sets it (syscall 578
/// op 5).
///
/// ★ Started life as the PS/2 path's fixed ×2, on the theory that matching it would keep the feel.
/// On the hardware that was "very fast, almost too sensitive": over I2C the ELAN part reports in
/// finer counts than its PS/2 emulation did, so the same multiplier overshoots. 100% is the
/// measured-by-hand starting point; the setting exists because the right value is a preference.
pub static SPEED_PCT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(100);
pub const SPEED_MIN: u32 = 10;
pub const SPEED_MAX: u32 = 400;

/// Called from the `usb-hid` task loop. Cheap when idle.
pub fn service() {
    if DISABLE.swap(false, Ordering::AcqRel) {
        POINTER_ACTIVE.store(false, Ordering::Release);
        mask_irq();
        if let Some(mut l) = LIVE.try_lock() {
            *l = None;
        }
    }
    poll();
    let boot = boot_step();
    if !boot && !REQUEST.swap(false, Ordering::AcqRel) {
        return;
    }
    let enable = boot || ENABLE.swap(false, Ordering::AcqRel);
    let mut r = run(enable, boot);
    // Wait out a syscall mid-copy rather than re-queue: re-running would repeat the probe WITHOUT
    // `enable` and tear down a device that had just taken the pointer.
    loop {
        if let Some(mut g) = RESULT.try_lock() {
            r.seq = g.seq.wrapping_add(1);
            *g = r;
            return;
        }
        core::hint::spin_loop();
    }
}

/// Boot-time enable, as a small state machine driven from `service()` so it never blocks the USB
/// polling that shares this task.
///
/// ★ Waits until [`BOOT_AFTER_MS`] of uptime before asking the governor for `acpi probe 13` — the
/// point at which `touchpad` used to be typed by hand. The rule this kernel learned the hard way is
/// that ACPI calls on the governor's AUTOMATIC FIRST PASS killed three boots; probe 13 is proven on
/// the hardware, but only ever after the desktop was up, so that is when it runs here too.
///
/// Returns true exactly once: on the tick the ACPI data is ready and the enable should run.
fn boot_step() -> bool {
    use core::sync::atomic::AtomicU8;
    const WAIT: u8 = 0;
    const REQUESTED: u8 = 1;
    const DONE: u8 = 2;
    static STATE: AtomicU8 = AtomicU8::new(WAIT);
    use core::sync::atomic::AtomicU64;
    static STARTED_AT: AtomicU64 = AtomicU64::new(0);
    static LAST_ASK: AtomicU64 = AtomicU64::new(0);

    let now = crate::time::UPTIME_MS.load(Ordering::Relaxed);
    match STATE.load(Ordering::Relaxed) {
        WAIT if now >= BOOT_AFTER_MS => {
            crate::acpi::request_probe(13, 0);
            STARTED_AT.store(now, Ordering::Relaxed);
            LAST_ASK.store(now, Ordering::Relaxed);
            STATE.store(REQUESTED, Ordering::Relaxed);
            false
        }
        REQUESTED => {
            let probed = crate::acpi::CACHE.try_lock().map_or(false, |c| c.i2c_hid_probed);
            if probed {
                STATE.store(DONE, Ordering::Relaxed);
                return true;
            }
            // The governor has one probe slot; a request from the terminal in the same second can
            // overwrite ours. Ask again every 3 s, and give up after 15 — `touchpad on` still works.
            if now.saturating_sub(STARTED_AT.load(Ordering::Relaxed)) >= 15_000 {
                STATE.store(DONE, Ordering::Relaxed);
            } else if now.saturating_sub(LAST_ASK.load(Ordering::Relaxed)) >= 3_000 {
                crate::acpi::request_probe(13, 0);
                LAST_ASK.store(now, Ordering::Relaxed);
            }
            false
        }
        _ => false,
    }
}

/// Uptime before the boot-time enable starts. See [`boot_step`].
const BOOT_AFTER_MS: u64 = 8_000;

fn run(enable: bool, boot: bool) -> ProbeResult {
    let mut r = ProbeResult::EMPTY;
    // Re-probing tears down a live device first: the probe re-programs the same controller.
    POINTER_ACTIVE.store(false, Ordering::Release);
    mask_irq();
    if let Some(mut l) = LIVE.try_lock() {
        *l = None;
    }
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

    // Phase 3: read-only unless `enable`.
    let (seen, live) = explore(&mut ctl, hid.slave_addr as u8, &buf, enable, hid.irq_gsi, boot);
    r.stage = if seen { stage::REPORTS_SEEN } else { stage::REPORT_DESCRIPTOR };
    if let Some((in_reg, max_in, uses_ids, mouse)) = live {
        if let Some(mut l) = LIVE.try_lock() {
            *l = Some(Live {
                ctl, addr: hid.slave_addr as u8, in_reg, max_in, uses_ids, mouse, errors: 0,
                acc_x: 0, acc_y: 0,
            });
            PS2_WHILE_SILENT.store(0, Ordering::Relaxed);
            FELL_BACK.store(false, Ordering::Relaxed);
            POINTER_ACTIVE.store(true, Ordering::Release);
            r.active = 1;
        }
    }
    r
}

/// Drain pending input reports and move the pointer. No-op unless `touchpad on` succeeded.
fn poll() {
    if !POINTER_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let mut guard = match LIVE.try_lock() {
        Some(g) => g,
        None => return,
    };
    let dev = match guard.as_mut() {
        Some(d) => d,
        None => return,
    };
    // ★ Read ONLY when the device's interrupt says a report is waiting. Reading the input register
    // otherwise returns the LAST report again — on the hardware that re-applied the last motion on
    // every poll, and the pointer ran off to the edge of the screen on its own.
    if !PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let mut buf = [0u8; 64];
    {
        let n = dev.max_in;
        match dev.ctl.write_read(dev.addr, &dev.in_reg.to_le_bytes(), &mut buf[..n]) {
            Err(_) => {
                dev.errors += 1;
                if dev.errors >= MAX_CONSECUTIVE_ERRORS {
                    // The I2C side has gone quiet. Hand the pointer back to PS/2 rather than leave
                    // the machine without one — and leave the line masked, nobody is listening.
                    POINTER_ACTIVE.store(false, Ordering::Release);
                    *guard = None;
                    return;
                }
                // Try again on the next tick: the line is still asserted, the report still there.
                PENDING.store(true, Ordering::Release);
                return;
            }
            Ok(()) => {
                dev.errors = 0;
                let len = u16::from_le_bytes([buf[0], buf[1]]) as usize;
                if len > 2 {
                    // The I2C side is alive — the PS/2 fallback counter starts over.
                    PS2_WHILE_SILENT.store(0, Ordering::Relaxed);
                }
                let id = if dev.uses_ids && len > 2 { buf[2] } else { 0 };
                if len <= 2 || len > n || id != dev.mouse.report_id {
                    // Nothing, or another collection's report: no motion, but the line still needs
                    // unmasking.
                    unmask_irq();
                    return;
                }
                let body = &buf[if dev.uses_ids { 3 } else { 2 }..len];
                let m = &dev.mouse;
                let dx = m.x.extract(body).unwrap_or(0);
                let dy = m.y.extract(body).unwrap_or(0);
                let mut buttons = 0u8;
                for (i, b) in m.buttons.iter().enumerate() {
                    if b.and_then(|f| f.extract(body)).unwrap_or(0) != 0 {
                        buttons |= 1 << i;
                    }
                }
                // Scale in hundredths, keeping the remainder (`/` truncates toward zero, so the
                // remainder keeps its sign and motion is symmetric in both directions).
                let pct = SPEED_PCT.load(Ordering::Relaxed) as i32;
                dev.acc_x += dx * pct;
                dev.acc_y += dy * pct;
                let mx = dev.acc_x / 100;
                let my = dev.acc_y / 100;
                dev.acc_x -= mx * 100;
                dev.acc_y -= my * 100;
                // HID relative Y is positive DOWN, the screen's convention — no flip, unlike PS/2.
                crate::mouse::update_relative(mx, my, buttons);
            }
        }
    }
    // The report is read, so the line has dropped (or, if another is queued, is still asserted and
    // will fire again the moment it is unmasked).
    unmask_irq();
}

/// PS/2 AUX bytes received since the last I2C report, while I2C drives the pointer. Climbs only
/// while the I2C side is silent; see `mouse::handle_interrupt`.
pub static PS2_WHILE_SILENT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Set when that fallback fired, so the diagnostic can say the pointer went back to PS/2 and why.
pub static FELL_BACK: AtomicBool = AtomicBool::new(false);

/// The IDT vector the touchpad's interrupt is delivered on. 0x30 is the RTL8168, 0x31 the Wi-Fi
/// MSI (pci.rs).
pub const IRQ_VECTOR: u8 = 0x32;
/// The GSI routed to [`IRQ_VECTOR`]; 0 = not routed. Read by the interrupt handler to mask it.
pub static IRQ_GSI: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Set by the interrupt handler: a report is waiting on the input register.
pub static PENDING: AtomicBool = AtomicBool::new(false);
/// Interrupts taken, for diagnostics.
pub static IRQ_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Route the touchpad's GSI to [`IRQ_VECTOR`] on the BSP, masked. Level-triggered, active-low: the
/// test laptop's `_CRS` returns `Interrupt (ResourceConsumer, Level, ActiveLow, ExclusiveAndWake)`
/// for TPD0 (DSDT, `SBFI`). Errors say why it cannot be.
fn route_irq(gsi: u32) -> Result<(), &'static str> {
    if gsi == 0 || gsi > 255 {
        return Err("no plain APIC interrupt in _CRS");
    }
    match crate::ioapic::entries() {
        None => return Err("IOAPIC not mapped"),
        Some(n) if gsi >= n => return Err("GSI beyond this IOAPIC's entries"),
        Some(_) => {}
    }
    let bsp = unsafe { crate::percpu::PER_CPU.as_ref().map(|p| p[0].apic_id).unwrap_or(0) };
    x86_64::instructions::interrupts::without_interrupts(|| {
        crate::ioapic::route_gsi_level_low(gsi as u8, bsp as u8, IRQ_VECTOR);
    });
    IRQ_GSI.store(gsi as u8, Ordering::Release);
    Ok(())
}

fn unmask_irq() {
    let gsi = IRQ_GSI.load(Ordering::Acquire);
    if gsi != 0 {
        x86_64::instructions::interrupts::without_interrupts(|| crate::ioapic::set_masked(gsi, false));
    }
}

fn mask_irq() {
    let gsi = IRQ_GSI.load(Ordering::Acquire);
    if gsi != 0 {
        x86_64::instructions::interrupts::without_interrupts(|| crate::ioapic::set_masked(gsi, true));
    }
    PENDING.store(false, Ordering::Release);
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

/// Read the report descriptor, parse it, optionally initialise the device, then watch the input
/// register while the user moves a finger. Returns whether any input report arrived, and — only
/// when `enable` was asked for AND mouse reports actually arrived — what `poll` needs to drive the
/// pointer: (input register, max input length, uses report IDs, mouse layout).
///
/// ⚠️ Without `enable` this is READ-ONLY: no RESET, no SET_POWER, no `_DSM`. The touchpad is the
/// machine's pointer over PS/2, and any of those may switch it off that path.
///
/// With `enable` it sends SET_POWER(ON) and RESET — the HID-over-I2C initialisation, which on the
/// hardware was shown to be necessary: 750 reads of the input register over 3 s of finger
/// movement returned nothing without it. It still does NOT evaluate `_DSM`; that is `touchpad
/// handover`, a separate and deliberate step.
fn explore(
    ctl: &mut crate::drivers::i2c::Controller,
    addr: u8,
    desc: &[u8],
    enable: bool,
    gsi: u32,
    boot: bool,
) -> (bool, Option<(u16, usize, bool, crate::drivers::hid_desc::MouseLayout)>) {
    use crate::drivers::hid_desc;
    use core::fmt::Write;

    let w = |i: usize| u16::from_le_bytes([desc[i], desc[i + 1]]);
    let (rd_len, rd_reg, in_reg, max_in) = (w(4) as usize, w(6), w(8), w(10) as usize);
    let cmd_reg = w(16);
    let mut out = Sink { len: 0, text: [0; REPORT_CAP] };

    // Whether the firmware handover (`_DSM`, `touchpad handover`) has been run this boot.
    let handover = crate::acpi::CACHE
        .try_lock()
        .map(|c| (c.i2c_hid_handover, c.i2c_hid_handover_reg))
        .unwrap_or((-1, 0));
    let _ = match handover {
        (-1, _) => writeln!(out, "firmware handover (_DSM): not run this boot"),
        (0, _) => writeln!(out, "firmware handover (_DSM): ran, but no device accepted it"),
        (n, reg) => writeln!(out, "firmware handover (_DSM): done on {} device(s), returned {:#06x}", n, reg),
    };

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
        return (false, None);
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

    // 2. Initialise, if asked. HID-over-I2C commands go to the command register as
    //    [reg lo, reg hi, report type/ID, opcode]: SET_POWER is opcode 8 with the state (0 = ON)
    //    in the low byte, RESET is opcode 1.
    //
    //    ★ The interrupt is routed FIRST, so the reset itself is observed on it: after RESET the
    //    device asserts its line and presents a zero-length report. That makes reset completion a
    //    fact rather than a timed guess, and it proves the interrupt routing before anything
    //    depends on it. No interrupt = no pointer: reading without it replays stale reports.
    let mut irq_ok = false;
    if enable {
        match route_irq(gsi) {
            Err(why) => {
                let _ = writeln!(out, "interrupt: cannot route GSI {} ({})", gsi, why);
            }
            Ok(()) => {
                let [c0, c1] = cmd_reg.to_le_bytes();
                let power = ctl.write_read(addr, &[c0, c1, 0x00, 0x08], &mut []);
                // Some parts need time to wake before the next command; Linux quirks cover 20 ms.
                crate::scheduler::kernel_sleep_ms(20);
                let before = IRQ_COUNT.load(Ordering::Relaxed);
                PENDING.store(false, Ordering::Release);
                unmask_irq();
                let reset = ctl.write_read(addr, &[c0, c1, 0x00, 0x01], &mut []);
                // The spec allows the device a generous window; wait up to a second for the line.
                let mut waited = 0u64;
                while waited < 1000 && !PENDING.load(Ordering::Acquire) {
                    crate::scheduler::kernel_sleep_ms(5);
                    waited += 5;
                }
                let fired = PENDING.swap(false, Ordering::AcqRel);
                let mut sentinel = [0u8; 64];
                let n = max_in.clamp(2, 64);
                let cleared = ctl.write_read(addr, &in_reg.to_le_bytes(), &mut sentinel[..n]);
                unmask_irq();
                let _ = writeln!(out, "init: SET_POWER(ON) {}, RESET {}, interrupt {}, sentinel {}",
                    if power.is_ok() { "ok" } else { "FAILED" },
                    if reset.is_ok() { "ok" } else { "FAILED" },
                    if fired { alloc::format!("fired after ~{} ms (GSI {})", waited, gsi) }
                    else { alloc::format!("NEVER FIRED within 1 s (GSI {}, {} irqs total)", gsi,
                           IRQ_COUNT.load(Ordering::Relaxed) - before) },
                    match cleared {
                        Ok(()) => if sentinel[0] == 0 && sentinel[1] == 0 { "00 00" } else { "not seen" },
                        Err(_) => "read failed",
                    });
                irq_ok = fired;
                if !fired {
                    mask_irq();
                }
            }
        }
    }

    // 3. Watch the input register. Each read returns [len_lo, len_hi, report...]; len 0 means
    //    nothing pending (the spec's reset sentinel is also len 0).
    let max_in = max_in.clamp(2, 64);
    let mut mouse_reports = 0u32;
    let mut buf = [0u8; 64];
    let (mut reads, mut errors, mut reports) = (0u32, 0u32, 0u32);
    let mut per_id: [(u8, u32); 8] = [(0, 0); 8];
    let mut shown = 0;
    let (mut sum_dx, mut sum_dy, mut clicks, mut right_clicks) = (0i32, 0i32, 0u32, 0u32);
    let mut elapsed = 0u64;
    // At boot nobody is touching the pad, so there is nothing to sample — the interrupt firing on
    // RESET is the proof instead, backed by the PS/2 fallback in `mouse::handle_interrupt`.
    while !boot && elapsed < SAMPLE_MS {
        // With a working interrupt, read only when it says a report is waiting — and unmask after.
        // Without one (a read-only probe) every tick reads, which is how stale replays were seen.
        let due = if irq_ok { PENDING.swap(false, Ordering::AcqRel) } else { true };
        if !due {
            crate::scheduler::kernel_sleep_ms(SAMPLE_EVERY_MS);
            elapsed += SAMPLE_EVERY_MS;
            continue;
        }
        reads += 1;
        let got = ctl.write_read(addr, &in_reg.to_le_bytes(), &mut buf[..max_in]);
        if irq_ok {
            unmask_irq();
        }
        match got {
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
                    // Show reports that CARRY something — motion or a button. All-zero reports said
                    // nothing last time, and a click is what needs seeing.
                    let body_start = if d.uses_report_ids { 3 } else { 2 };
                    if shown < 4 && buf[body_start..len].iter().any(|&b| b != 0) {
                        shown += 1;
                        let _ = write!(out, "  sample:");
                        for b in &buf[..len] {
                            let _ = write!(out, " {:02x}", b);
                        }
                        let _ = writeln!(out);
                    }
                    if let Some(m) = &mouse {
                        if id == m.report_id {
                            mouse_reports += 1;
                            let body = &buf[if d.uses_report_ids { 3 } else { 2 }..len];
                            sum_dx += m.x.extract(body).unwrap_or(0);
                            sum_dy += m.y.extract(body).unwrap_or(0);
                            if m.buttons[0].and_then(|b| b.extract(body)).unwrap_or(0) != 0 {
                                clicks += 1;
                            }
                            if m.buttons[1].and_then(|b| b.extract(body)).unwrap_or(0) != 0 {
                                right_clicks += 1;
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
        let _ = writeln!(out, "decoded as mouse: total dx {} dy {}, left-button reports {}, right-button reports {}",
            sum_dx, sum_dy, clicks, right_clicks);
    }

    // Take the pointer only on evidence: `enable` asked for, AND mouse reports actually arrived.
    let live = match mouse {
        Some(_) if enable && !irq_ok => {
            let _ = writeln!(out, "not taking the pointer: the interrupt line is not working, and \
                                   reading without it replays stale reports. PS/2 stays in charge.");
            None
        }
        Some(m) if enable && (mouse_reports > 0 || boot) => {
            let _ = writeln!(out, "★ I2C now drives the pointer; PS/2 mouse bytes are ignored. \
                                   `touchpad off` hands it back.");
            Some((in_reg, max_in, d.uses_report_ids, m))
        }
        _ if enable => {
            let _ = writeln!(out, "not taking the pointer: no mouse reports arrived. PS/2 stays in charge.");
            None
        }
        _ => None,
    };
    publish(&out);
    (reports != 0, live)
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
