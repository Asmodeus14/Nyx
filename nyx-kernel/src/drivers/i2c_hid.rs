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
    pub const NO_CONTROLLER: u32 = 13;
    pub const WRONG_CLASS: u32 = 14;
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

/// What the probe learned that the live driver needs.
#[derive(Clone, Copy)]
struct Params {
    in_reg: u16,
    cmd_reg: u16,
    data_reg: u16,
    max_in: usize,
    uses_ids: bool,
    mouse: crate::drivers::hid_desc::MouseLayout,
    /// The multi-touch layout and the Input Mode feature — both needed for precision mode.
    ptp: Option<crate::drivers::hid_desc::PtpLayout>,
    input_mode: Option<crate::drivers::hid_desc::InputMode>,
}

/// The live device, owned by the `usb-hid` task once `touchpad on` (or the boot enable) succeeds.
struct Live {
    ctl: Controller,
    addr: u8,
    p: Params,
    /// Consecutive failed reads. Too many and the pointer goes back to PS/2.
    errors: u32,
    /// Sub-pixel remainder of scaled motion, in hundredths of a pixel. Carried between reports so
    /// that below 100% a slow finger still moves the pointer instead of every small delta
    /// truncating to zero.
    acc_x: i32,
    acc_y: i32,
    /// Precision mode: the device reports every contact and `gesture` interprets them.
    ptp_on: bool,
    engine: crate::drivers::gesture::Engine,
    asm: FrameAsm,
    /// Same idea as `acc_x`, for the precision path (`ptp_report`), in its scaled units.
    acc_px: i64,
    acc_py: i64,
    acc_scroll: i64,
    /// Buttons held by the clickpad itself.
    phys_buttons: u8,
    /// A tap's click, held for [`TAP_CLICK_MS`] so the desktop sees it press and release.
    pulse_buttons: u8,
    pulse_until: u64,
    /// Precision mode is on trial until this uptime; 0 once a touching finger has been decoded.
    ptp_trial_until: u64,
}

/// A precision-mode frame being assembled. In "hybrid" reporting a frame of N contacts arrives
/// spread over several reports: the first carries the contact count, the rest carry 0.
#[derive(Clone, Copy)]
struct FrameAsm {
    expected: usize,
    got: usize,
    contacts: [crate::drivers::gesture::Contact; crate::drivers::hid_desc::MAX_SLOTS],
    n: usize,
    button: bool,
}

impl FrameAsm {
    const EMPTY: FrameAsm = FrameAsm {
        expected: 0,
        got: 0,
        contacts: [crate::drivers::gesture::Contact { id: 0, x: 0, y: 0 }; crate::drivers::hid_desc::MAX_SLOTS],
        n: 0,
        button: false,
    };
}

/// How long a tap's click is held down. The desktop samples the button state; a press and release
/// inside one of its frames would never be seen.
const TAP_CLICK_MS: u64 = 60;

/// Scroll, in pixels, accumulated by two-finger motion and not yet taken by the shell (syscall 578
/// op 7). Positive = the view moves down the content.
pub static SCROLL_ACCUM: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

/// `touchpad ptp` / `touchpad mouse`: 0 = nothing asked, 1 = mouse mode, 2 = precision mode.
pub static MODE_REQUEST: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Outcome of the last mode switch: 0 none, 1 ok, 2 the device has no Input Mode feature,
/// 3 the SET_REPORT failed on the bus, 4 no live device, 5 the parsed X/Y range is implausible.
pub static MODE_RESULT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// True while the device is in precision mode.
pub static PTP_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Precision mode went silent on I2C (while PS/2 kept talking) and was reverted to mouse mode —
/// see `mouse::handle_interrupt`. Cleared by the next successful switch into precision mode.
pub static PTP_FAILED: AtomicBool = AtomicBool::new(false);

/// A switch into precision mode is a TRIAL until a touching finger is actually decoded. If none is
/// within this long, the device goes back to mouse mode on its own.
///
/// ★ Needed because the PS/2-based recovery cannot fire after `touchpad handover` — that is the
/// call that switches PS/2 emulation OFF. On the hardware, precision mode after a handover left the
/// pointer dead with nothing to bring it back but typing `touchpad mouse`.
const PTP_TRIAL_MS: u64 = 10_000;

/// One logged report: the raw bytes (from the report ID on) and what the precision path decoded.
/// Mirrored by `nyx_api::TouchpadLogEntry`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LogEntry {
    pub len: u8,
    /// Contacts decoded as touching in this report; 0xFF = not a touch pad report.
    pub n: u8,
    pub _pad: [u8; 2],
    /// The first touching contact's position, as decoded.
    pub x: i32,
    pub y: i32,
    pub raw: [u8; 16],
}
const _: () = assert!(core::mem::size_of::<LogEntry>() == 28);

pub const LOG_LEN: usize = 8;

/// Reports received since the pointer was last taken, by kind — `touchpad status`. Mouse (the
/// mouse collection), touch pad (precision), other IDs, and empty/oversized reads.
pub static COUNT_MOUSE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static COUNT_PTP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static COUNT_OTHER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static COUNT_EMPTY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The device was found sending precision-mode reports while the driver was in mouse mode, and the
/// driver followed it.
pub static ADOPTED_PTP: AtomicBool = AtomicBool::new(false);

/// IDs of the ELAN's collections, fixed per device but only known after parsing; cached here so
/// `count_report` does not need the device. 0 = not yet known.
static MOUSE_ID: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static PTP_ID: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

fn count_report(id: u8) {
    use core::sync::atomic::Ordering::Relaxed;
    let c = if id == 0xFE {
        &COUNT_EMPTY
    } else if id != 0 && id == MOUSE_ID.load(Relaxed) {
        &COUNT_MOUSE
    } else if id != 0 && id == PTP_ID.load(Relaxed) {
        &COUNT_PTP
    } else {
        &COUNT_OTHER
    };
    c.fetch_add(1, Relaxed);
}

/// The last [`LOG_LEN`] reports received in precision mode, oldest first after rotation —
/// `touchpad log`. ★ Exists because the first precision-mode build threw the pointer across the
/// screen on hardware, and the device's real report layout was not known: raw bytes beside the
/// decoded contact is what shows whether the layout was read right.
pub static PTP_LOG: spin::Mutex<([LogEntry; LOG_LEN], usize)> = spin::Mutex::new((
    [LogEntry { len: 0, n: 0, _pad: [0; 2], x: 0, y: 0, raw: [0; 16] }; LOG_LEN],
    0,
));

fn log_report(raw: &[u8], n: u8, x: i32, y: i32) {
    if let Some(mut g) = PTP_LOG.try_lock() {
        let (ref mut ring, ref mut next) = *g;
        let mut e = LogEntry { len: raw.len().min(16) as u8, n, _pad: [0; 2], x, y, raw: [0; 16] };
        e.raw[..e.len as usize].copy_from_slice(&raw[..e.len as usize]);
        ring[*next % LOG_LEN] = e;
        *next = (*next + 1) % LOG_LEN;
    }
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
    // ★ EVERY probe (re)takes the pointer, on the interrupt-at-RESET proof — the boot semantics.
    //
    // Two hardware lessons. A plain probe used to tear the live driver down and leave it down, so
    // running the `touchpad` diagnostic silently handed the pointer back to PS/2 (and then
    // `touchpad ptp` found nothing to switch). And `touchpad on` required mouse reports inside a
    // 3 s window, so it refused whenever nobody happened to touch the pad during it. The
    // interrupt firing on RESET has since proven itself on every run — it is the proof.
    ENABLE.store(false, Ordering::Release);
    let _ = boot;
    let mut r = run(true, true);
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
/// ★ Normally enables on the task's FIRST pass: discovery already ran single-threaded during boot
/// (`acpi::scan_for_modern_inputs`), so the touchpad has the pointer before the desktop appears.
/// The first version waited 8 s for the governor instead, and the PS/2 pointer it left in charge
/// until then felt slow.
///
/// If boot-time discovery produced nothing, it falls back to that original path: at
/// [`BOOT_AFTER_MS`] of uptime, ask the governor for `acpi probe 13` — never on the governor's
/// automatic first pass, which is where ACPI calls killed three boots on this machine.
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
        // The normal case: `acpi::scan_for_modern_inputs` already ran discovery during boot, so
        // enable on this task's very first pass — before the desktop is up.
        WAIT if crate::acpi::CACHE.try_lock().map_or(false, |c| c.i2c_hid_probed && c.i2c_hid_n > 0) => {
            STATE.store(DONE, Ordering::Relaxed);
            true
        }
        // Fallback: boot-time discovery found nothing usable (or never ran). Ask the governor once
        // the desktop is up, as `touchpad` does.
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
    // Re-probing tears down a live device first: the probe re-programs the same controller. The
    // RESET that follows puts the device back in mouse mode, so remember precision mode and ask for
    // it again once the pointer is retaken.
    let was_ptp = PTP_ACTIVE.swap(false, Ordering::AcqRel);
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

    // ⚠️ An unresolved controller `_ADR` is 0 — which decodes as device 0 function 0, the HOST
    // BRIDGE. Never aim the controller driver at that; refuse instead.
    if hid.ctrl_adr == 0 {
        r.status = status::NO_CONTROLLER;
        return r;
    }
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
                BringUpError::WrongClass(_) => status::WRONG_CLASS,
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
    if let Some(p) = live {
        if let Some(mut l) = LIVE.try_lock() {
            let x_max = p.ptp.map_or(1000, |t| t.x_max);
            *l = Some(Live {
                ctl, addr: hid.slave_addr as u8, p, errors: 0,
                acc_x: 0, acc_y: 0,
                // RESET puts the device back in its default mouse mode.
                ptp_on: false,
                engine: crate::drivers::gesture::Engine::new(crate::drivers::gesture::Config::for_pad(x_max)),
                asm: FrameAsm::EMPTY,
                acc_px: 0, acc_py: 0, acc_scroll: 0,
                phys_buttons: 0, pulse_buttons: 0, pulse_until: 0,
                ptp_trial_until: 0,
            });
            PTP_ACTIVE.store(false, Ordering::Release);
            MOUSE_ID.store(p.mouse.report_id, Ordering::Relaxed);
            PTP_ID.store(p.ptp.map_or(0, |t| t.report_id), Ordering::Relaxed);
            for c in [&COUNT_MOUSE, &COUNT_PTP, &COUNT_OTHER, &COUNT_EMPTY] {
                c.store(0, Ordering::Relaxed);
            }
            ADOPTED_PTP.store(false, Ordering::Relaxed);
            PS2_WHILE_SILENT.store(0, Ordering::Relaxed);
            FELL_BACK.store(false, Ordering::Relaxed);
            POINTER_ACTIVE.store(true, Ordering::Release);
            r.active = 1;
            if was_ptp {
                MODE_REQUEST.store(2, Ordering::Release);
            }
        }
    }
    r
}

/// Drain pending input reports and move the pointer. No-op unless `touchpad on` succeeded.
fn poll() {
    if !POINTER_ACTIVE.load(Ordering::Acquire) {
        // A mode switch needs a live device; answer the request rather than leave it hanging.
        if MODE_REQUEST.swap(0, Ordering::AcqRel) != 0 {
            MODE_RESULT.store(4, Ordering::Release);
        }
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
    let now = crate::time::UPTIME_MS.load(Ordering::Relaxed);

    // A tap's click ends on time whether or not another report ever arrives — after the finger
    // lifts, the device goes quiet.
    if dev.pulse_buttons != 0 && now >= dev.pulse_until {
        dev.pulse_buttons = 0;
        crate::mouse::update_relative(0, 0, dev.phys_buttons);
    }

    match MODE_REQUEST.swap(0, Ordering::AcqRel) {
        1 => set_mode(dev, false),
        2 => {
            set_mode(dev, true);
            if dev.ptp_on {
                dev.ptp_trial_until = now + PTP_TRIAL_MS;
            }
        }
        _ => {}
    }
    // The trial ran out with no finger ever decoded: precision mode is not working on this device
    // as things stand. Back to the proven mouse mode.
    if dev.ptp_on && dev.ptp_trial_until != 0 && now >= dev.ptp_trial_until {
        dev.ptp_trial_until = 0;
        set_mode(dev, false);
        PTP_FAILED.store(true, Ordering::Release);
    }

    // ★ Read ONLY when the device's interrupt says a report is waiting. Reading the input register
    // otherwise returns the LAST report again — on the hardware that re-applied the last motion on
    // every poll, and the pointer ran off to the edge of the screen on its own.
    if !PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let mut buf = [0u8; 64];
    let n = dev.p.max_in;
    match dev.ctl.write_read(dev.addr, &dev.p.in_reg.to_le_bytes(), &mut buf[..n]) {
        Err(_) => {
            dev.errors += 1;
            if dev.errors >= MAX_CONSECUTIVE_ERRORS {
                // The I2C side has gone quiet. Hand the pointer back to PS/2 rather than leave the
                // machine without one — and leave the line masked, nobody is listening.
                POINTER_ACTIVE.store(false, Ordering::Release);
                PTP_ACTIVE.store(false, Ordering::Release);
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
            if len > 2 && len <= n {
                // The I2C side is alive — the PS/2 fallback counter starts over.
                PS2_WHILE_SILENT.store(0, Ordering::Relaxed);
                let id = if dev.p.uses_ids { buf[2] } else { 0 };
                let body = &buf[if dev.p.uses_ids { 3 } else { 2 }..len];
                count_report(id);
                let is_ptp_id = dev.p.ptp.map_or(false, |t| t.report_id == id);
                // ★ A precision-mode report while we think the device is in mouse mode: the
                // DEVICE switched (after `touchpad handover` the firmware appears to do exactly
                // that — the pointer went dead in "mouse mode" with nothing but a working reset).
                // Follow the device instead of dropping every report it sends.
                if is_ptp_id && !dev.ptp_on {
                    dev.ptp_on = true;
                    PTP_ACTIVE.store(true, Ordering::Release);
                    let x_max = dev.p.ptp.map_or(1000, |t| t.x_max);
                    dev.engine = crate::drivers::gesture::Engine::new(
                        crate::drivers::gesture::Config::for_pad(x_max),
                    );
                    dev.asm = FrameAsm::EMPTY;
                    ADOPTED_PTP.store(true, Ordering::Release);
                }
                if id == dev.p.mouse.report_id {
                    mouse_report(dev, body);
                    log_report(&buf[2..len], 0xFF, 0, 0);
                } else if is_ptp_id {
                    let (n, x, y) = ptp_report(dev, body, now);
                    log_report(&buf[2..len], n, x, y);
                } else {
                    log_report(&buf[2..len], 0xFF, 0, 0);
                }
            } else {
                // Empty (len ≤ 2) or longer than the declared maximum. Logged too: after the
                // first precision-mode attempt the log was EMPTY, and "no reports" could not be
                // told apart from "reads that returned nothing". The whole buffer, length included.
                count_report(0xFE);
                log_report(&buf[..n.min(16)], 0xFE, len as i32, 0);
            }
        }
    }
    // The report is read, so the line has dropped (or, if another is queued, is still asserted and
    // will fire again the moment it is unmasked).
    unmask_irq();
}

/// A report from the mouse collection — the device's default mode, where its own firmware has
/// already turned fingers into relative motion and taps into button presses.
fn mouse_report(dev: &mut Live, body: &[u8]) {
    let m = &dev.p.mouse;
    let dx = m.x.extract(body).unwrap_or(0);
    let dy = m.y.extract(body).unwrap_or(0);
    let mut buttons = 0u8;
    for (i, b) in m.buttons.iter().enumerate() {
        if b.and_then(|f| f.extract(body)).unwrap_or(0) != 0 {
            buttons |= 1 << i;
        }
    }
    // Scale in hundredths, keeping the remainder (`/` truncates toward zero, so the remainder
    // keeps its sign and motion is symmetric in both directions).
    let pct = SPEED_PCT.load(Ordering::Relaxed) as i32;
    dev.acc_x += dx * pct;
    dev.acc_y += dy * pct;
    let mx = dev.acc_x / 100;
    let my = dev.acc_y / 100;
    dev.acc_x -= mx * 100;
    dev.acc_y -= my * 100;
    dev.phys_buttons = buttons;
    // HID relative Y is positive DOWN, the screen's convention — no flip, unlike PS/2.
    crate::mouse::update_relative(mx, my, buttons | dev.pulse_buttons);
}

/// A report from the touch pad collection (precision mode): assemble the frame, interpret it, and
/// turn the result into pointer motion, clicks, scroll and swipes.
///
/// Returns what THIS report decoded to — touching contacts in it, and the first one's position —
/// for `touchpad log`.
fn ptp_report(dev: &mut Live, body: &[u8], now: u64) -> (u8, i32, i32) {
    use crate::drivers::gesture::{Contact, Tap};
    use crate::drivers::hid_desc::MAX_SLOTS;
    let t = match dev.p.ptp {
        Some(t) => t,
        None => return (0, 0, 0),
    };
    let mut here = 0u8;
    let mut first = (0i32, 0i32);

    // Frame assembly. With a contact-count field, a non-zero count starts a frame of that many
    // contacts and zero continues one; without it, every report is a whole frame.
    match t.contact_count.and_then(|f| f.extract(body)) {
        Some(c) if c > 0 => {
            dev.asm.expected = (c as usize).min(MAX_SLOTS);
            dev.asm.got = 0;
            dev.asm.n = 0;
        }
        Some(_) => {
            if dev.asm.expected == 0 {
                return (0, 0, 0); // a continuation of a frame whose start was never seen
            }
        }
        None => {
            dev.asm.expected = t.n_slots;
            dev.asm.got = 0;
            dev.asm.n = 0;
        }
    }
    for s in t.slots[..t.n_slots].iter().flatten() {
        if dev.asm.got >= dev.asm.expected {
            break;
        }
        dev.asm.got += 1;
        let tip = s.tip.extract(body).unwrap_or(0) != 0;
        // Palm rejection: the device's own judgement, when it reports one.
        let confident = s.confidence.map_or(true, |f| f.extract(body).unwrap_or(1) != 0);
        if tip && confident && dev.asm.n < MAX_SLOTS {
            let id = s.id.and_then(|f| f.extract(body)).unwrap_or(dev.asm.got as i32) as u8;
            let x = s.x.extract(body).unwrap_or(0);
            let y = s.y.extract(body).unwrap_or(0);
            dev.asm.contacts[dev.asm.n] = Contact { id, x, y };
            dev.asm.n += 1;
            // A real finger decoded: precision mode works, the trial is over.
            dev.ptp_trial_until = 0;
            if here == 0 {
                first = (x, y);
            }
            here += 1;
        }
    }
    if let Some(b) = t.button {
        dev.asm.button = b.extract(body).unwrap_or(0) != 0;
    }
    if dev.asm.got < dev.asm.expected {
        return (here, first.0, first.1); // the rest of this frame is in the next report
    }
    dev.asm.expected = 0;
    let o = dev.engine.frame(now, &dev.asm.contacts[..dev.asm.n], dev.asm.button);

    // Scale logical units to pixels: at 100% a full pad width is 1.5 screen widths. The same
    // factor on both axes keeps motion isotropic. Remainders carry, as on the mouse path.
    let sw = crate::mouse::screen_width().max(1) as i64;
    let pct = SPEED_PCT.load(Ordering::Relaxed) as i64;
    let unit = 2 * (t.x_max.max(1) as i64) * 100;
    dev.acc_px += o.dx as i64 * sw * 3 * pct;
    dev.acc_py += o.dy as i64 * sw * 3 * pct;
    let mx = dev.acc_px / unit;
    let my = dev.acc_py / unit;
    dev.acc_px -= mx * unit;
    dev.acc_py -= my * unit;
    // Scroll is not scaled by the pointer speed: they are different preferences.
    dev.acc_scroll += o.scroll_y as i64 * sw * 3 * 100;
    let sy = dev.acc_scroll / unit;
    dev.acc_scroll -= sy * unit;
    if sy != 0 {
        SCROLL_ACCUM.fetch_add(sy as i32, Ordering::Relaxed);
    }

    let before = dev.phys_buttons | dev.pulse_buttons;
    dev.phys_buttons = o.buttons;
    match o.tap {
        Tap::Left => {
            dev.pulse_buttons = 0b01;
            dev.pulse_until = now + TAP_CLICK_MS;
        }
        Tap::Right => {
            dev.pulse_buttons = 0b10;
            dev.pulse_until = now + TAP_CLICK_MS;
        }
        Tap::None => {}
    }
    // Three-finger swipes drive the Command: up opens it (the Super key), down dismisses (Esc).
    match o.swipe3 {
        1 => crate::shell::push_key('\u{E019}'),
        -1 => crate::shell::push_key('\x1b'),
        _ => {}
    }
    let buttons = dev.phys_buttons | dev.pulse_buttons;
    if mx != 0 || my != 0 || buttons != before {
        crate::mouse::update_relative(mx as i32, my as i32, buttons);
    }
    (here, first.0, first.1)
}

/// Switch the device between its mouse emulation and precision mode, with SET_REPORT on the Input
/// Mode feature. Result in [`MODE_RESULT`].
///
/// The wire format (HID over I2C, SET_REPORT): the command register, then [type<<4 | id, opcode 3]
/// (ids of 15 and up take a third byte), then the data register, then a 16-bit length that counts
/// itself, the report ID and the report.
fn set_mode(dev: &mut Live, ptp: bool) {
    let im = match dev.p.input_mode {
        Some(m) => m,
        None => {
            MODE_RESULT.store(2, Ordering::Release);
            return;
        }
    };
    if ptp && dev.p.ptp.is_none() {
        MODE_RESULT.store(2, Ordering::Release);
        return;
    }
    // A pad whose X/Y range parsed as tiny would make the scaling (which divides by it) explode
    // into huge pointer jumps. Refuse rather than switch into that.
    if ptp && dev.p.ptp.map_or(true, |t| t.x_max < 64 || t.y_max < 64) {
        MODE_RESULT.store(5, Ordering::Release);
        return;
    }
    let value = if ptp {
        crate::drivers::hid_desc::usage::INPUT_MODE_TOUCHPAD
    } else {
        crate::drivers::hid_desc::usage::INPUT_MODE_MOUSE
    } as u32;

    // The feature report, zeroed except for Input Mode (the Device Identifier beside it is 0).
    let rlen = im.report_len.clamp(1, 16);
    let mut rep = [0u8; 16];
    for i in 0..im.field.size.min(32) {
        if value >> i & 1 != 0 {
            let bit = (im.field.bit_off + i) as usize;
            if bit / 8 < rlen {
                rep[bit / 8] |= 1 << (bit % 8);
            }
        }
    }

    const FEATURE: u8 = 3;
    const SET_REPORT: u8 = 3;
    let rid = im.report_id;
    let mut w = [0u8; 40];
    let mut k = 0usize;
    let [c0, c1] = dev.p.cmd_reg.to_le_bytes();
    let [d0, d1] = dev.p.data_reg.to_le_bytes();
    let size = (2 + if rid != 0 { 1 } else { 0 } + rlen) as u16;
    let [s0, s1] = size.to_le_bytes();
    {
        let mut push = |b: u8| {
            w[k] = b;
            k += 1;
        };
        push(c0);
        push(c1);
        if rid < 0x0F {
            push(FEATURE << 4 | rid);
            push(SET_REPORT);
        } else {
            push(FEATURE << 4 | 0x0F);
            push(SET_REPORT);
            push(rid);
        }
        push(d0);
        push(d1);
        push(s0);
        push(s1);
        if rid != 0 {
            push(rid);
        }
        for &b in &rep[..rlen] {
            push(b);
        }
    }

    match dev.ctl.write_read(dev.addr, &w[..k], &mut []) {
        Ok(()) => {
            dev.ptp_on = ptp;
            PTP_ACTIVE.store(ptp, Ordering::Release);
            if ptp {
                PTP_FAILED.store(false, Ordering::Release);
            }
            // A fresh interpretation for the new mode: no half-assembled frame, no stale fingers.
            let x_max = dev.p.ptp.map_or(1000, |t| t.x_max);
            dev.engine = crate::drivers::gesture::Engine::new(crate::drivers::gesture::Config::for_pad(x_max));
            dev.asm = FrameAsm::EMPTY;
            if let Some(mut g) = PTP_LOG.try_lock() {
                *g = ([LogEntry { len: 0, n: 0, _pad: [0; 2], x: 0, y: 0, raw: [0; 16] }; LOG_LEN], 0);
            }
            MODE_RESULT.store(1, Ordering::Release);
        }
        Err(_) => MODE_RESULT.store(3, Ordering::Release),
    }
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
) -> (bool, Option<Params>) {
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
    let _ = writeln!(out, "report descriptor: {} bytes, {} fields, report IDs {}{}",
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
    let ptp = hid_desc::ptp_layout(&d);
    let imode = hid_desc::input_mode(&d);
    match &ptp {
        Some(t) => {
            let _ = writeln!(out, "  touchpad layout: id {}, {} contact slot(s), X 0..{} Y 0..{}, \
                                   contact count {}, clickpad button {}",
                t.report_id, t.n_slots, t.x_max, t.y_max,
                t.contact_count.map_or(alloc::string::String::from("none"), |f| alloc::format!("@{}", f.bit_off)),
                t.button.map_or(alloc::string::String::from("none"), |f| alloc::format!("@{}", f.bit_off)));
            // The first finger's fields in full — bit offset + size, and range — which is what
            // `touchpad log`'s raw bytes are decoded against.
            if let Some(s) = t.slots[0] {
                let f = |o: Option<crate::drivers::hid_desc::Field>| match o {
                    Some(f) => alloc::format!("@{}+{}", f.bit_off, f.size),
                    None => alloc::string::String::from("none"),
                };
                let _ = writeln!(out, "  finger 1: tip {} conf {} id {} x @{}+{} [{}..{}] y @{}+{} [{}..{}]",
                    f(Some(s.tip)), f(s.confidence), f(s.id),
                    s.x.bit_off, s.x.size, s.x.logical_min, s.x.logical_max,
                    s.y.bit_off, s.y.size, s.y.logical_min, s.y.logical_max);
            }
        }
        None => {
            let _ = writeln!(out, "  no precision touchpad layout");
        }
    }
    match &imode {
        Some(m) => {
            let _ = writeln!(out, "  input mode: feature report {} bit {}+{} ({} byte report)",
                m.report_id, m.field.bit_off, m.field.size, m.report_len);
        }
        None => {
            let _ = writeln!(out, "  no Input Mode feature — precision mode unavailable");
        }
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
            Some(Params {
                in_reg,
                cmd_reg,
                data_reg: w(18),
                max_in,
                uses_ids: d.uses_report_ids,
                mouse: m,
                ptp,
                input_mode: imode,
            })
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
