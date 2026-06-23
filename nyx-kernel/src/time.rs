use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

// The unbendable wall clock.
pub static UPTIME_MS: AtomicU64 = AtomicU64::new(0);

// Default to 2 GHz, but will be dynamically calibrated on boot!
pub static TSC_MHZ: AtomicU64 = AtomicU64::new(2000);

pub fn init() {
    crate::serial_println!("[BOOT] Initializing Global Uptime Clock...");
}

/// Uses the legacy Programmable Interval Timer (PIT) Channel 2 (PC Speaker) 
/// to dynamically calculate the CPU's true clock speed!
pub fn calibrate_tsc() {
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