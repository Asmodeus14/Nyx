use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

// The unbendable wall clock.
pub static UPTIME_MS: AtomicU64 = AtomicU64::new(0);

// Default to 2 GHz, but will be dynamically calibrated on boot!
pub static TSC_MHZ: AtomicU64 = AtomicU64::new(2000);

/// How `TSC_MHZ` was obtained. 0 = still the hardcoded default (i.e. NOT calibrated).
///
/// ★ This exists because the failure was invisible for the life of the project. `calibrate_tsc`
/// falls back to a bare 2000 and says so with `serial_println!` — on a machine with **no serial
/// port to read**. The measured `sched` output on real hardware showed `TSC 2000 MHz`, exactly the
/// default, which is the only reason anyone noticed. A calibration that can fail must publish
/// whether it did.
pub static TSC_SOURCE: AtomicU64 = AtomicU64::new(0);
pub const TSC_SRC_DEFAULT: u64 = 0;
pub const TSC_SRC_CPUID_15: u64 = 1; // crystal clock x ratio — exact
pub const TSC_SRC_CPUID_16: u64 = 2; // base frequency — exact on Intel invariant TSC
pub const TSC_SRC_PIT: u64 = 3;      // legacy channel-2 gate measurement

/// TSC at boot, so uptime can be derived from the counter rather than accumulated from ticks.
pub static BOOT_TSC: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)) };
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    crate::serial_println!("[BOOT] Initializing Global Uptime Clock...");
    BOOT_TSC.store(rdtsc(), Ordering::SeqCst);
}

/// Milliseconds since boot, derived from the TSC.
///
/// ★★ This replaces "add 1 to `UPTIME_MS` per timer interrupt", which was wrong three ways at once:
///   1. the tick was never 1 ms (measured ~27 ms on this laptop — a hardcoded APIC count that
///      nothing ever checked against a clock),
///   2. **every core** ran that increment into the same global atomic, so the clock's rate was
///      `cores / tick_period` and changed as cores came online — on hardware the per-core tick
///      counts sum exactly to `UPTIME_MS`, which is how that was confirmed, and
///   3. a tick lost to an interrupts-off window silently lost time, so the only clock in the system
///      ran slow exactly when the machine was busiest.
///
/// Reading the TSC fixes all three: it is monotonic, identical on every core (invariant TSC), and
/// immune to missed interrupts.
#[inline]
pub fn uptime_ms_now() -> u64 {
    let mhz = TSC_MHZ.load(Ordering::Relaxed).max(1);
    let base = BOOT_TSC.load(Ordering::Relaxed);
    rdtsc().saturating_sub(base) / (mhz * 1000)
}

/// Derive the TSC frequency from CPUID rather than by timing it.
///
/// Preferred over the PIT because **this laptop has no working PIT channel 2** — the gate bit at
/// port 0x61 never asserts, so both the old `calibrate_tsc` and the APIC calibration bailed and
/// silently kept their defaults. CPUID needs no timer at all.
///
/// Leaf 0x15 gives the core crystal clock and the TSC/crystal ratio, which is exact. Intel often
/// leaves ECX (the crystal frequency) as 0 on client parts, in which case the crystal is a known
/// per-family constant — 24 MHz for everything Skylake-and-later that is not a server or Atom part.
/// Leaf 0x16 reports the base frequency, and on Intel an invariant TSC ticks at exactly that, so it
/// is a good second choice.
fn tsc_mhz_from_cpuid() -> Option<(u64, u64)> {
    let max_leaf = unsafe { core::arch::x86_64::__cpuid(0).eax } as u64;
    let plausible = |m: u64| (100..=10_000).contains(&m);

    // Leaf 0x16: base frequency. On Intel an invariant TSC ticks at exactly the base frequency, so
    // this is a direct answer and — unlike leaf 0x15 — involves no assumed constant.
    let from_16 = if max_leaf >= 0x16 {
        let m = (unsafe { core::arch::x86_64::__cpuid(0x16) }.eax & 0xFFFF) as u64;
        if plausible(m) { Some(m) } else { None }
    } else {
        None
    };

    // Leaf 0x15: core crystal clock x (numerator/denominator). Exact when the crystal is reported.
    let from_15 = if max_leaf >= 0x15 {
        let l = unsafe { core::arch::x86_64::__cpuid(0x15) };
        let (denom, numer, mut crystal) = (l.eax as u64, l.ebx as u64, l.ecx as u64);
        let mut assumed = false;
        if denom != 0 && numer != 0 {
            if crystal == 0 {
                // Intel leaves ECX zero on most client parts. 24 MHz is the crystal for
                // Skylake-and-later client silicon (this laptop is Coffee Lake). Flagged as an
                // ASSUMPTION so it is only trusted when leaf 0x16 agrees — guessing a crystal and
                // publishing the result unchecked is how you get a confidently wrong clock.
                let v = unsafe { core::arch::x86_64::__cpuid(0) };
                let is_intel =
                    v.ebx == 0x756e_6547 && v.edx == 0x4965_6e69 && v.ecx == 0x6c65_746e;
                if is_intel {
                    crystal = 24_000_000;
                    assumed = true;
                }
            }
            if crystal != 0 {
                let m = crystal.saturating_mul(numer) / denom / 1_000_000;
                if plausible(m) { Some((m, assumed)) } else { None }
            } else { None }
        } else { None }
    } else {
        None
    };

    match (from_15, from_16) {
        // Both available: agreement within 10% means the crystal (assumed or not) is right, and
        // 0x15 is the finer-grained of the two. Disagreement means the assumption was wrong, so
        // take the one that assumed nothing.
        (Some((m15, _)), Some(m16)) => {
            let (lo, hi) = if m15 < m16 { (m15, m16) } else { (m16, m15) };
            if hi.saturating_sub(lo) * 10 <= hi {
                Some((m15, TSC_SRC_CPUID_15))
            } else {
                crate::serial_println!(
                    "[TIME] CPUID 0x15 says {} MHz but 0x16 says {} MHz; trusting 0x16.",
                    m15 as u32, m16 as u32);
                Some((m16, TSC_SRC_CPUID_16))
            }
        }
        // Only 0x15, and only if it did not rest on an assumed crystal.
        (Some((m15, false)), None) => Some((m15, TSC_SRC_CPUID_15)),
        (Some((_, true)), None) => None, // assumed crystal with nothing to check it against
        (None, Some(m16)) => Some((m16, TSC_SRC_CPUID_16)),
        (None, None) => None,
    }
}

/// Uses the legacy Programmable Interval Timer (PIT) Channel 2 (PC Speaker) 
/// to dynamically calculate the CPU's true clock speed!
pub fn calibrate_tsc() {
    // CPUID first: it needs no timer, and the PIT path below does not work on every machine —
    // notably not on this laptop, where it has been silently falling back to a hardcoded 2000 MHz
    // since the function was written.
    if let Some((mhz, src)) = tsc_mhz_from_cpuid() {
        TSC_MHZ.store(mhz, Ordering::SeqCst);
        TSC_SOURCE.store(src, Ordering::SeqCst);
        crate::serial_println!(
            "[TIME] TSC {} MHz from CPUID leaf {}.", mhz as u32,
            if src == TSC_SRC_CPUID_15 { 0x15 } else { 0x16 } as u32);
        crate::vga_println!("[TIME] CPU {} MHz (CPUID)", mhz as u32);
        return;
    }
    crate::serial_println!("[TIME] CPUID gave no TSC frequency; falling back to the PIT.");
    calibrate_tsc_pit();
}

/// Legacy fallback: time the TSC against PIT channel 2.
///
/// ⚠️ Kept, but no longer trusted as the primary: on hardware with no working channel-2 gate the
/// `while` below spins out and the function keeps the default silently.
fn calibrate_tsc_pit() {
    let mut port_61: Port<u8> = Port::new(0x61); // PC Speaker Port
    let mut port_43: Port<u8> = Port::new(0x43); // PIT Command Port
    let mut port_42: Port<u8> = Port::new(0x42); // PIT Channel 2 Data Port

    let ticks: u16 = 11931;

    unsafe {
        port_43.write(0b10110000);
        port_42.write((ticks & 0xFF) as u8); 
        port_42.write((ticks >> 8) as u8);   

        let port_61_val = port_61.read();
        port_61.write((port_61_val & 0xFD) | 1);

        let mut lo: u32; let mut hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
        let start_tsc = ((hi as u64) << 32) | (lo as u64);

        let mut timeout = 0;
        while (port_61.read() & 0x20) == 0 {
            core::arch::asm!("pause");
            timeout += 1;
            // 🔥 THE FIX: Lowered from 50,000,000 to 50,000. 
            // Gives the hardware ~50ms to respond before safely bailing out!
            if timeout > 50_000 { break; } 
        }

        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
        let end_tsc = ((hi as u64) << 32) | (lo as u64);

        port_61.write(port_61_val);

        if timeout <= 50_000 {
            let tsc_hz = (end_tsc - start_tsc) * 100;
            let mut tsc_mhz = tsc_hz / 1_000_000;
            
            if tsc_mhz < 100 || tsc_mhz > 10_000 { tsc_mhz = 2000; }
            
            TSC_MHZ.store(tsc_mhz, Ordering::SeqCst);
            TSC_SOURCE.store(TSC_SRC_PIT, Ordering::SeqCst);
            crate::serial_println!("[TIME] CPU TSC Calibrated successfully to {} MHz!", tsc_mhz);
        } else {
            crate::serial_println!("[TIME] Hardware PIT missing. Defaulting to 2000 MHz.");
        }
    }
}

/// Early-boot hardware delay used exclusively by SMP initialization.
pub fn sleep_ms(ms: u64) {
    let mut lo: u32; let mut hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
    let start = ((hi as u64) << 32) | (lo as u64);
    
    let mhz = TSC_MHZ.load(Ordering::Relaxed);
    let target = start + (ms * mhz * 1000); 
    
    loop {
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let now = ((hi as u64) << 32) | (lo as u64);
        if now >= target { break; }
        unsafe { core::arch::asm!("pause"); } 
    }
}