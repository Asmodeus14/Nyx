use crate::acpi::ACPI_INFO;
use crate::memory::phys_to_virt;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;

// --- FAST HARDWARE CACHE ---
// This prevents the OS from re-parsing ACPI tables during high-speed interrupts!
static mut LOCAL_APIC_VIRT: u64 = 0;

#[repr(C, packed)]
struct MadtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
    local_apic_addr: u32, 
    flags: u32,
}

#[repr(C, packed)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub length: u8,
}

#[repr(C, packed)]
pub struct ProcessorLocalApic {
    pub entry_type: u8,
    pub length: u8,
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
}

pub fn init() {
    let madt_phys = unsafe { ACPI_INFO.madt_addr };
    if madt_phys.is_none() { 
        crate::serial_println!("[APIC] Cannot init: MADT address is missing.");
        return; 
    }

    let madt_virt = phys_to_virt(madt_phys.unwrap()).unwrap();
    let madt = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let apic_phys = madt.local_apic_addr as u64;
    
    if unsafe { crate::memory::map_mmio(apic_phys, 4096) }.is_err() { 
        crate::serial_println!("[APIC] FATAL: Failed to map Local APIC MMIO.");
        return; 
    }
    
    let apic_virt = phys_to_virt(apic_phys).unwrap();
    
    // 👉 CACHE IT FOR LIGHTNING-FAST INTERRUPTS!
    unsafe { LOCAL_APIC_VIRT = apic_virt; }

    unsafe {
        let sivr_ptr = (apic_virt + 0xF0) as *mut u32;
        let current_sivr = read_volatile(sivr_ptr);
        write_volatile(sivr_ptr, current_sivr | 0x1FF); 
    }

    crate::vga_println!("[APIC] Local APIC Enabled! Ready for MSI (Modern Interrupts).");
}

pub fn get_cpu_apic_ids() -> Vec<u32> {
    let madt_phys = unsafe { ACPI_INFO.madt_addr };
    if madt_phys.is_none() {
        crate::serial_println!("[APIC] MADT missing. Defaulting to 1 core.");
        return alloc::vec![0]; 
    }

    let madt_virt = phys_to_virt(madt_phys.unwrap()).unwrap();
    let header = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let end_ptr = madt_virt + header.length as u64;
    let mut current_ptr = madt_virt + core::mem::size_of::<MadtHeader>() as u64;
    
    let mut ids = Vec::new();

    while current_ptr < end_ptr {
        let entry = unsafe { &*(current_ptr as *const MadtEntryHeader) };
        if entry.entry_type == 0 { 
            let proc = unsafe { &*(current_ptr as *const ProcessorLocalApic) };
            if (proc.flags & 1) != 0 {
                ids.push(proc.apic_id as u32);
            }
        }
        if entry.length == 0 { break; }
        current_ptr += entry.length as u64;
    }

    crate::serial_println!("[APIC] Detected {} active CPU cores dynamically.", ids.len());
    ids
}

const ICR_LOW: u64 = 0x300;
const ICR_HIGH: u64 = 0x310;

// Uses the ultra-fast static cache instead of parsing ACPI
fn get_apic_virt_base() -> u64 {
    unsafe {
        if LOCAL_APIC_VIRT != 0 {
            return LOCAL_APIC_VIRT;
        }
    }
    // Emergency Fallback
    let madt_phys = unsafe { ACPI_INFO.madt_addr.expect("MADT missing") };
    let madt_virt = crate::memory::phys_to_virt(madt_phys).unwrap();
    let madt = unsafe { &*(madt_virt as *const MadtHeader) };
    crate::memory::phys_to_virt(madt.local_apic_addr as u64).unwrap()
}

pub fn send_init(target_apic_id: u32) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let icr_high = (apic_virt + ICR_HIGH) as *mut u32;
        let icr_low = (apic_virt + ICR_LOW) as *mut u32;
        write_volatile(icr_high, target_apic_id << 24);
        write_volatile(icr_low, 0x0000_4500); 
    }
}

pub fn send_sipi(target_apic_id: u32, vector: u8) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let icr_high = (apic_virt + ICR_HIGH) as *mut u32;
        let icr_low = (apic_virt + ICR_LOW) as *mut u32;
        write_volatile(icr_high, target_apic_id << 24);
        write_volatile(icr_low, 0x0000_4600 | (vector as u32));
    }
}

/// Vector of the reschedule IPI: "a task on your core just became Ready — schedule now".
pub const RESCHED_VECTOR: u8 = 0x42;

/// Send a fixed-delivery IPI to one core.
///
/// Waits (briefly, bounded) for the previous IPI from THIS core to leave the ICR first: writing
/// ICR_LOW while Delivery Status is still pending can drop the earlier one. Call with interrupts
/// masked — an interrupt landing between the HIGH and LOW writes, whose handler also sends an IPI,
/// would re-target this one.
pub fn send_ipi(target_apic_id: u32, vector: u8) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let icr_high = (apic_virt + ICR_HIGH) as *mut u32;
        let icr_low = (apic_virt + ICR_LOW) as *mut u32;
        let mut spins = 0u32;
        while core::ptr::read_volatile(icr_low) & (1 << 12) != 0 && spins < 10_000 {
            core::hint::spin_loop();
            spins += 1;
        }
        write_volatile(icr_high, target_apic_id << 24);
        // Fixed delivery (000), physical destination, level ASSERT (bit 14), edge.
        write_volatile(icr_low, (1 << 14) | vector as u32);
    }
}

pub fn init_ap() {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let sivr_ptr = (apic_virt + 0xF0) as *mut u32;
        let current_sivr = core::ptr::read_volatile(sivr_ptr);
        write_volatile(sivr_ptr, current_sivr | 0x1FF); 
    }
}

#[repr(C, packed)]
pub struct IoApicEntry {
    pub entry_type: u8,
    pub length: u8,
    pub io_apic_id: u8,
    pub reserved: u8,
    pub io_apic_address: u32,
    pub global_system_interrupt_base: u32,
}

pub fn get_ioapic_phys_addr() -> Option<u64> {
    let madt_phys = unsafe { ACPI_INFO.madt_addr }?;
    let madt_virt = crate::memory::phys_to_virt(madt_phys)?;
    let header = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let end_ptr = madt_virt + header.length as u64;
    let mut current_ptr = madt_virt + core::mem::size_of::<MadtHeader>() as u64;

    while current_ptr < end_ptr {
        let entry = unsafe { &*(current_ptr as *const MadtEntryHeader) };
        if entry.entry_type == 1 { 
            let io_apic = unsafe { &*(current_ptr as *const IoApicEntry) };
            return Some(io_apic.io_apic_address as u64);
        }
        if entry.length == 0 { break; }
        current_ptr += entry.length as u64;
    }
    None
}

pub fn end_of_interrupt() {
    let apic_virt = get_apic_virt_base(); 
    unsafe {
        let eoi_ptr = (apic_virt + 0xB0) as *mut u32;
        core::ptr::write_volatile(eoi_ptr, 0);
    }
}

const LVT_TIMER: u64 = 0x320;
const TIMER_INITIAL_COUNT: u64 = 0x380;
const TIMER_CURRENT_COUNT: u64 = 0x390;
const TIMER_DIVIDE_CONFIG: u64 = 0x3E0;

/// The divisor `init_timer` programs into the DCR. `0x3` is the encoding for **divide by 16**
/// (the encoding is not the divisor: bits [1:0] and [3] form a 3-bit field where 0b011 = 16).
const TIMER_DIVIDE_BY_16: u32 = 0x3;

/// The count `init_timer` used to program unconditionally, kept as the fallback for a machine where
/// calibration cannot run. A bare `0x0000_A000` with a comment claiming "a fast 1ms tick rate" — a
/// claim nothing ever checked. Measured on this laptop it is **~27 ms**.
const LEGACY_TIMER_COUNT: u32 = 0x0000_A000;

/// The scheduling quantum we aim for, in microseconds.
///
/// 1 ms is the value the old comment claimed and the rest of the kernel already assumes — notably
/// `UPTIME_MS`, and every `sleep`/timeout denominated in it. Picking anything else would require
/// retuning those at the same time; picking this makes the code finally mean what it says.
const TARGET_TICK_US: u64 = 1000;

/// Calibrated timer count, 0 until `calibrate_timer` publishes one.
static TIMER_COUNT: AtomicU64 = AtomicU64::new(0);

/// Measured APIC timer ticks per millisecond, post-divisor. 0 = calibration has not run or failed.
///
/// ★ This is the number the scheduler has been missing since it was written. `init_timer` programs
/// a hardcoded initial count and `timer_context_switch` then does `UPTIME_MS.fetch_add(1)` per
/// interrupt — i.e. it *asserts* the tick is exactly one millisecond. Nothing measured it, so the
/// quantum, every `sleep()`, every socket timeout and the compositor's frame pacing are all
/// denominated in a unit of unknown length.
///
/// Deliberately **report-only** for now: nothing reads this to change behaviour yet. Repointing the
/// clock at it is a separate change, because it must land together with making `UPTIME_MS`
/// single-sourced — today every core increments it, so correcting one without the other swaps a
/// ~6.5x-slow tick for an N-times-fast clock.
pub static APIC_TICKS_PER_MS: AtomicU64 = AtomicU64::new(0);

/// How long one scheduler tick actually lasts, in microseconds, given the count `init_timer`
/// programs. 0 = unknown. This is the headline diagnostic number.
pub static TICK_PERIOD_US: AtomicU64 = AtomicU64::new(0);

/// Measure the local APIC timer against the PIT and publish the result. Changes no behaviour.
///
/// ## Why the PIT rather than the TSC
///
/// `time::calibrate_tsc` derives TSC_MHZ from PIT channel 2, so calibrating the APIC against the
/// TSC would inherit that error and hide it. Gating both off the same independent hardware
/// reference keeps the two measurements honest about each other — if they disagree, one of them is
/// wrong and we want to know rather than average them.
///
/// Channel 2 is the PC-speaker channel: it is not wired to any IRQ, and its gate/output are
/// software-readable through port 0x61, which is exactly what makes it usable as a stopwatch this
/// early. Port 0x61 is saved and restored, same as `calibrate_tsc` does.
///
/// Must run AFTER `apic::init` (the LAPIC has to be mapped) and BEFORE `init_timer` arms the
/// periodic timer. The BSP only: the APIC bus clock is a board-level clock, identical on every core.
pub fn calibrate_timer() {
    // Read the cache directly rather than via `get_apic_virt_base`: that helper's fallback path
    // `expect`s the MADT, so on a degraded (no-ACPI) boot asking it for a base would panic the
    // machine instead of letting calibration decline.
    let apic_virt = unsafe { LOCAL_APIC_VIRT };
    if apic_virt == 0 {
        crate::serial_println!("[APIC] calibrate_timer: LAPIC not mapped, skipping.");
        return;
    }

    // ★ Timed against the TSC, not the PIT.
    //
    // The first version gated on PIT channel 2, on the reasoning that an independent reference
    // keeps the two measurements honest about each other. On this laptop that reference does not
    // exist — the channel-2 gate bit at port 0x61 never asserts, so this bailed and published
    // nothing, and `calibrate_tsc` had been silently failing the same way since it was written.
    // The `sched` report on real hardware read "tick period NOT MEASURED" and "TSC 2000 MHz", the
    // exact hardcoded default, which is what exposed it.
    //
    // `time::calibrate_tsc` now derives the TSC from CPUID, which needs no timer at all, so it is
    // available and trustworthy by the time we get here.
    let tsc_mhz = crate::time::TSC_MHZ.load(Ordering::Relaxed);
    if crate::time::TSC_SOURCE.load(Ordering::Relaxed) == crate::time::TSC_SRC_DEFAULT {
        crate::serial_println!(
            "[APIC] calibrate_timer: TSC was never calibrated, refusing to derive a tick from it.");
        return;
    }

    const WINDOW_MS: u64 = 10;
    let window_cycles = tsc_mhz * 1000 * WINDOW_MS;

    unsafe {
        // Park the APIC timer: masked (bit 16) and one-shot (bits 18:17 = 00) so nothing fires
        // while we measure. The IDT already has a handler for 0x40 by this point in boot, and an
        // interrupt mid-measurement would only add noise.
        let lvt_ptr = (apic_virt + LVT_TIMER) as *mut u32;
        let dcr_ptr = (apic_virt + TIMER_DIVIDE_CONFIG) as *mut u32;
        let icr_ptr = (apic_virt + TIMER_INITIAL_COUNT) as *mut u32;
        let ccr_ptr = (apic_virt + TIMER_CURRENT_COUNT) as *mut u32;

        write_volatile(lvt_ptr, 1 << 16);
        // Same divisor `init_timer` will use, or the number we publish describes a different clock.
        write_volatile(dcr_ptr, TIMER_DIVIDE_BY_16);

        // Max count: at divide-by-16 off any plausible reference this runs for minutes, so it
        // cannot wrap inside a 10 ms window and the delta needs no wrap handling.
        write_volatile(icr_ptr, 0xFFFF_FFFF);

        let start = crate::time::rdtsc();
        while crate::time::rdtsc().wrapping_sub(start) < window_cycles {
            core::arch::asm!("pause");
        }

        let remaining = read_volatile(ccr_ptr);

        // Stop the timer again; `init_timer` will program it properly.
        write_volatile(icr_ptr, 0);
        write_volatile(lvt_ptr, 1 << 16);

        let elapsed = 0xFFFF_FFFFu32.wrapping_sub(remaining) as u64;
        let per_ms = elapsed / WINDOW_MS;

        // Sanity band. A divide-by-16 APIC off anything from a 25 MHz to a 1 GHz reference lands
        // between ~1.5k and ~62k ticks/ms; outside that the measurement is noise, and publishing a
        // plausible-looking wrong number is worse than publishing none (a wrong tick period would
        // send the next person hunting the wrong bug entirely).
        if per_ms < 500 || per_ms > 200_000 {
            crate::serial_println!(
                "[APIC] calibrate_timer: implausible result ({} ticks/ms), discarding.",
                per_ms as u32);
            return;
        }

        APIC_TICKS_PER_MS.store(per_ms, Ordering::SeqCst);

        // ★ Now choose the count to hit TARGET_TICK_US, instead of keeping a hardcoded one and
        // merely reporting what it happened to cost.
        //
        // The old count 0xA000 (40960) at divide-16 is 655,360 APIC clocks. On this laptop the
        // LAPIC runs off the 24 MHz core crystal, so that is **~27 ms** — not the "fast 1ms tick
        // rate" the comment claimed, and therefore the scheduling quantum was ~27 ms. That is
        // directly why hover highlights lagged while the pointer stayed smooth: the cursor is drawn
        // from the mouse IRQ by the hardware cursor plane, but a highlight needs the window server
        // to be scheduled, and it could not run more than ~37 times a second.
        let count = (per_ms.saturating_mul(TARGET_TICK_US) / 1000).clamp(16, 0xFFFF_FFFF) as u32;
        TIMER_COUNT.store(count as u64, Ordering::SeqCst);

        let period_us = (count as u64 * 1000) / per_ms;
        TICK_PERIOD_US.store(period_us, Ordering::SeqCst);

        crate::serial_println!(
            "[APIC] Timer calibrated: {} ticks/ms (div16). Count {:#x} => tick period {} us (was {:#x} => {} us).",
            per_ms as u32, count, period_us as u32,
            LEGACY_TIMER_COUNT, ((LEGACY_TIMER_COUNT as u64 * 1000) / per_ms) as u32);
        crate::vga_println!(
            "[APIC] Tick {} us ({} ticks/ms)", period_us as u32, per_ms as u32);
    }
}

/// The count `init_timer` programs. Diagnostics report it so "what we asked for" and "what we got"
/// are both visible.
pub fn timer_initial_count() -> u32 {
    let c = TIMER_COUNT.load(Ordering::Relaxed);
    if c == 0 { LEGACY_TIMER_COUNT } else { c as u32 }
}

pub fn init_timer(vector: u8) {
    let apic_virt = get_apic_virt_base();
    // Calibrated if `calibrate_timer` ran and published one; otherwise the historical constant, so
    // a machine where calibration fails still gets exactly the behaviour it had before.
    let count = timer_initial_count();
    unsafe {
        // Clear Task Priority Register (TPR)
        let tpr_ptr = (apic_virt + 0x80) as *mut u32;
        core::ptr::write_volatile(tpr_ptr, 0);

        // Divide Configuration Register (Divide by 16)
        let dcr_ptr = (apic_virt + TIMER_DIVIDE_CONFIG) as *mut u32;
        core::ptr::write_volatile(dcr_ptr, TIMER_DIVIDE_BY_16);

        // LVT Timer Register — periodic (bit 17), vector in the low byte.
        let lvt_timer_ptr = (apic_virt + LVT_TIMER) as *mut u32;
        core::ptr::write_volatile(lvt_timer_ptr, 0x20000 | (vector as u32));

        let icr_ptr = (apic_virt + TIMER_INITIAL_COUNT) as *mut u32;
        core::ptr::write_volatile(icr_ptr, count);
    }
}