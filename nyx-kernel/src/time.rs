use core::sync::atomic::{AtomicU64, Ordering};

// The unbendable wall clock.
pub static UPTIME_MS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    crate::serial_println!("[BOOT] Initializing Global Uptime Clock...");
}

/// Early-boot hardware delay used exclusively by SMP initialization.
/// This busy-waits using raw CPU cycles because it runs before the scheduler is alive.
pub fn sleep_ms(ms: u64) {
    let mut lo: u32; let mut hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
    let start = ((hi as u64) << 32) | (lo as u64);
    
    // Calculate target using a rough 2GHz estimate.
    // During early boot, CPU frequency scaling hasn't kicked in, so this is perfectly safe!
    let target = start + (ms * 2_000_000); 
    
    loop {
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let now = ((hi as u64) << 32) | (lo as u64);
        if now >= target { break; }
        
        // Prevents the CPU core from melting while it spins
        unsafe { core::arch::asm!("pause"); } 
    }
}