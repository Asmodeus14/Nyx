use sha3::{Digest, Sha3_256};
use x86_64::instructions::random::RdRand;
use core::sync::atomic::{AtomicBool, Ordering};

pub static mut GENETIC_SEED: [u8; 32] = [0; 32];
static SEED_LOCKED: AtomicBool = AtomicBool::new(false);

pub fn awaken_entity() {
    if SEED_LOCKED.load(Ordering::SeqCst) {
        crate::vga_println!("[ENTITY] Seed already locked. Evolution continues.");
        return;
    }

    crate::vga_println!("[ENTITY] Synthesizing genetic seed...");

    let rdrand = RdRand::new();
    let hw_entropy = rdrand.and_then(|r| r.get_u64()).unwrap_or_else(|| {
        unsafe { core::arch::x86_64::_rdtsc() }
    });

    let install_time = unsafe { core::arch::x86_64::_rdtsc() }; 
    let qpu_noise: u64 = 0x0_1337_0_BEEF; 

    let mut hasher = Sha3_256::new();
    hasher.update(&hw_entropy.to_le_bytes());
    hasher.update(&install_time.to_le_bytes());
    hasher.update(&qpu_noise.to_le_bytes());
    
    let result = hasher.finalize();

    unsafe {
        GENETIC_SEED.copy_from_slice(&result);
    }

    SEED_LOCKED.store(true, Ordering::SeqCst);

    // TODO: Persistence logic goes here

    crate::serial_println!("Genetic seed locked. Nyx Entity born.");
    crate::vga_println!("[ENTITY] Genetic seed locked. Nyx Entity born.");
}