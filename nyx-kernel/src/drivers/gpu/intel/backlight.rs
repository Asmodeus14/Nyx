//! Panel backlight — the PCH's PWM duty-cycle register.
//!
//! The one piece of the design's OSD that is real today. There is no audio driver in Nyx, so volume
//! is a drawn surface reporting its own absence; brightness is a single register write in the same
//! MMIO aperture the GPU driver already maps, on the identical `wr(mmio, ...)` path the cursor code
//! at +0x080 uses.
//!
//! ## Registers
//!
//! SOURCED, not guessed, from i915 (`display/intel_backlight.c`, `i915_reg.h`) for PCH-attached
//! backlight, which is what SKL/KBL/CFL — this machine's Gen9.5 — uses:
//!
//! ```text
//!   BLC_PWM_PCH_CTL1  0xC8250   bit 31 = PWM enable, bit 30 = override, bit 29 = polarity
//!   BLC_PWM_PCH_CTL2  0xC8254   bits 31:16 = period ("max"), bits 15:0 = duty cycle
//! ```
//!
//! `pch_get_max_backlight` is `CTL2 >> 16`; `pch_set_backlight` rewrites only the low 16 bits and
//! preserves the period. That preservation matters — clobbering the period changes the PWM
//! frequency, which on some panels is audible and on others is a flicker.
//!
//! ## The safety rule
//!
//! **Never write unless the hardware has already told us it is driving a backlight, and never write
//! a duty cycle that could black the panel out.**
//!
//! This machine has no serial console. A panel driven to zero looks exactly like a hang, and the
//! recovery would be a blind power cycle. So:
//!
//!   * If the period reads back as 0, or the enable bit is clear, this is not a PWM-driven panel —
//!     an external display, or firmware that drives the backlight another way. Report unavailable
//!     and touch nothing. The design says the same thing about the OSD: on a desktop panel it
//!     should say so rather than move a slider that does nothing.
//!   * A set is clamped to at least [`MIN_PERCENT`]. Dimming to invisible is not a feature anyone
//!     asked for, and it is indistinguishable from the failure it would be mistaken for.

use core::sync::atomic::{AtomicU64, Ordering};

// ★★ GEN9 USES THREE 32-BIT REGISTERS, NOT A PACKED 16/16 PAIR.
//
// This file used to model 0xC8254 the pre-Gen9 way — `bits 31:16 = period, bits 15:0 = duty` — and
// that is wrong on SKL/KBL. i915 uses:
//
//     0xC8250  BXT_BLC_PWM_CTL    bit 31 enable, bit 29 polarity
//     0xC8254  BXT_BLC_PWM_FREQ   period, FULL 32 bits
//     0xC8258  BXT_BLC_PWM_DUTY   duty cycle, FULL 32 bits
//
// Proof from the same laptop running Fedora:
//     cat /sys/class/backlight/intel_backlight/max_brightness  ->  120000
//
// 120000 is 0x1D4C0 — it does not fit in 16 bits. Under the old layout `max = CTL2 >> 16` evaluated
// to **1**, so `read()` reported a one-step backlight, and `set_percent` wrote the duty into the low
// half of the FREQUENCY register, corrupting the PWM period instead of changing brightness.
//
// This is why brightness "did not work" and why the PWM path was written off as unavailable — which
// in turn sent this project down a long detour through ACPI `_BCM` and a firmware SMI mailbox.
// Fedora exposes ONLY `intel_backlight` on this machine (no `acpi_video0`), so PWM is how the panel
// is actually meant to be driven.
const BXT_BLC_PWM_CTL: u32 = 0xC8250;
const BXT_BLC_PWM_FREQ: u32 = 0xC8254;
const BXT_BLC_PWM_DUTY: u32 = 0xC8258;
const BLM_PCH_PWM_ENABLE: u32 = 1 << 31;

/// The floor a set is clamped to, in percent. See the module note.
pub const MIN_PERCENT: u32 = 5;

/// MMIO base, cached at driver init so the syscall path never takes `INTEL_GPU`.
///
/// ★ Same reasoning as `cursor.rs`: SYSCALL runs with interrupts masked, and blocking on a lock a
/// preemptible task holds wedges the core. The address is copied once, here, and read back without
/// touching the driver.
static MMIO_BASE: AtomicU64 = AtomicU64::new(0);

/// Record the GPU's MMIO base. Called once from the driver, where it is already known.
pub fn init(mmio_base: u64) {
    MMIO_BASE.store(mmio_base, Ordering::SeqCst);
    match read() {
        Some((cur, max)) => crate::serial_println!(
            "[BACKLIGHT] PCH PWM live: duty {}/{} ({}%)",
            cur, max, cur * 100 / max.max(1)
        ),
        None => crate::serial_println!(
            "[BACKLIGHT] no PCH PWM backlight (external panel, or firmware drives it elsewhere)"
        ),
    }
}

#[inline]
unsafe fn rd(mmio: u64, off: u32) -> u32 {
    core::ptr::read_volatile((mmio + off as u64) as *const u32)
}
#[inline]
unsafe fn wr(mmio: u64, off: u32, val: u32) {
    core::ptr::write_volatile((mmio + off as u64) as *mut u32, val);
}

/// Current duty cycle and period, or `None` when there is no PWM-driven panel here.
pub fn read() -> Option<(u32, u32)> {
    let mmio = MMIO_BASE.load(Ordering::Relaxed);
    if mmio == 0 {
        return None;
    }
    unsafe {
        let ctl = rd(mmio, BXT_BLC_PWM_CTL);
        let max = rd(mmio, BXT_BLC_PWM_FREQ);
        let duty = rd(mmio, BXT_BLC_PWM_DUTY);
        // Both conditions, not either: a period with the PWM disabled is a register holding a stale
        // value, and writing a duty cycle into it changes nothing while looking like it worked.
        if max == 0 || ctl & BLM_PCH_PWM_ENABLE == 0 {
            return None;
        }
        Some((duty, max))
    }
}

/// Brightness as a percentage, or `None` when unavailable.
///
/// **PWM first**, ACPI as the fallback.
///
/// This used to read ACPI first, on the belief that "the PCH PWM path reports nothing because
/// firmware owns the panel and drives it from SMM". That belief came from `read()` returning `None`
/// — which it did because the register layout was wrong, not because the hardware was silent.
///
/// Two independent facts overturned it: Fedora on this exact laptop exposes only
/// `/sys/class/backlight/intel_backlight` with no `acpi_video0`, and its `max_brightness` is
/// **120000** — a value that cannot fit the 16-bit field the old code read.
///
/// ACPI `_BQC` remains the fallback and does work (`AE_OK`, hardware-confirmed), but note it is a
/// **cached** value on this firmware: the ASL returns a Name that only `_BCM` writes, so it reports
/// 100 until something sets the brightness. The PWM duty register is the real measurement.
pub fn percent() -> Option<u32> {
    if let Some((cur, max)) = read() {
        return Some(((cur as u64 * 100 + max as u64 / 2) / max as u64) as u32);
    }
    crate::acpi::panel_get().map(|p| p.min(100))
}

/// Set brightness to `pct` percent. Returns the percentage actually applied, or `None` when there
/// is no PWM-driven panel.
///
/// Clamped into `MIN_PERCENT..=100`, and only the duty-cycle half of the register is touched.
pub fn set_percent(pct: u32) -> Option<u32> {
    // ★ PWM FIRST. This used to try ACPI `_BCM` first, on the belief that firmware owned the panel
    // and the PWM register was dead. Fedora on this laptop exposes only `intel_backlight` and no
    // `acpi_video0`, so PWM is exactly how this panel is driven — the register looked dead because
    // we were reading the wrong layout, not because firmware had claimed it.
    //
    // ACPI stays as the fallback for machines where PWM genuinely is unavailable, but it is no
    // longer the first choice: `_BCM` ends in a firmware SMI that freezes the CPU, and a plain MMIO
    // write does not.
    let pct = pct.clamp(MIN_PERCENT, 100);

    if let Some((_, max)) = read() {
        let mmio = MMIO_BASE.load(Ordering::Relaxed);
        // Full 32-bit duty against a full 32-bit period. `max` is 120000 on this panel, so the old
        // `& 0xFFFF` mask would have truncated nearly every value above 54%.
        let duty = ((max as u64 * pct as u64) / 100).max(1) as u32;
        unsafe { wr(mmio, BXT_BLC_PWM_DUTY, duty) };
        return Some(pct);
    }

    // No usable PWM — fall back to the firmware method. Still an SMI, still freezes the machine
    // while SMM runs, which is why callers step on a 5% grid rather than tracking pointer motion.
    if crate::acpi::panel_set(pct) {
        return Some(pct);
    }
    None
}

/// Step brightness by `delta` percent, rounded onto a 5% grid so repeated taps land on round
/// numbers rather than drifting. Returns the new percentage, or `None` when unavailable.
pub fn step(delta: i32) -> Option<u32> {
    let cur = percent()? as i32;
    let stepped = ((cur + delta) / 5) * 5;
    set_percent(stepped.clamp(0, 100) as u32)
}
