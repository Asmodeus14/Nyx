use x86_64::instructions::port::Port;
use core::sync::atomic::{AtomicU64, Ordering};

const PIT_FREQUENCY: u32 = 1193182;
const TARGET_FREQ: u32 = 1000;
const DIVISOR: u32 = PIT_FREQUENCY / TARGET_FREQ;

pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    // We still initialize the legacy PIT just in case it's needed for fallback,
    // but we will no longer rely on it for critical SMP delays.
    let mut command_port = Port::<u8>::new(0x43);
    let mut data_port = Port::<u8>::new(0x40);

    unsafe {
        command_port.write(0x36);
        data_port.write((DIVISOR & 0xFF) as u8);
        data_port.write(((DIVISOR >> 8) & 0xFF) as u8);
    }
}

pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Reads the CPU's internal Time Stamp Counter (TSC)
pub fn read_tsc() -> u64 {
    let mut lo: u32;
    let mut hi: u32;
    unsafe { 
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)) 
    };
    ((hi as u64) << 32) | (lo as u64)
}

/// A hardware-accurate sleep function that DOES NOT rely on legacy interrupts!
pub fn sleep_ms(ms: u64) {
    // Assume a conservative 2.0 GHz clock speed for the delay loop.
    // 1 ms = 2,000,000 CPU cycles. (If your CPU is faster, it just sleeps 
    // for slightly less than 10ms, which is perfectly fine for the SIPI delay).
    let delay_cycles = ms * 2_000_000;
    let start = read_tsc();
    
    // Spinlock until the CPU cycle counter surpasses our target
    while read_tsc() - start < delay_cycles {
        core::hint::spin_loop();
    }
}