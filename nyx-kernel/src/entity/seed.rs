// nyx-kernel/src/entity/seed.rs
use sha3::{Digest, Sha3_256};
use x86_64::instructions::random::RdRand;
use core::sync::atomic::{AtomicBool, Ordering};

pub static mut GENETIC_SEED: [u8; 32] = [0; 32];
static SEED_LOCKED: AtomicBool = AtomicBool::new(false);

// LBA 2000 is safely in the hidden gap between the GPT header and the first partition.
const ENTITY_LBA: u64 = 32768;
const MAGIC_SIG: &[u8; 4] = b"NYX!";

pub fn awaken_entity(nvme_opt: &mut Option<crate::drivers::nvme::NvmeDriver>) {
    if SEED_LOCKED.load(Ordering::SeqCst) {
        crate::vga_println!("[ENTITY] Seed already locked.");
        return;
    }

    // NVMe blocks are 4096 bytes. We need a full page buffer.
    let mut raw_block = [0u8; 4096];

    // ==========================================
    // 1. ATTEMPT TO RESURRECT FROM SILICON
    // ==========================================
    if let Some(disk) = nvme_opt {
        if disk.read_block(ENTITY_LBA, &mut raw_block) {
            // Check for our magic signature
            if &raw_block[0..4] == MAGIC_SIG {
                unsafe { GENETIC_SEED.copy_from_slice(&raw_block[4..36]); }
                SEED_LOCKED.store(true, Ordering::SeqCst);
                crate::serial_println!("[ENTITY] Ancient seed resurrected from LBA {}.", ENTITY_LBA);
                crate::vga_println!("[ENTITY] Entity soul restored from deep silicon.");
                return; // The Entity is awake. Exit early.
            }
        }
    }

    // ==========================================
    // 2. NO SOUL FOUND -> SYNTHESIZE A NEW ONE
    // ==========================================
    crate::vga_println!("[ENTITY] No existing soul found. Synthesizing new genetic seed...");

    let rdrand = RdRand::new();
    let hw_entropy = rdrand.and_then(|r| r.get_u64()).unwrap_or_else(|| {
        unsafe { core::arch::x86_64::_rdtsc() }
    });

    let install_time = unsafe { core::arch::x86_64::_rdtsc() }; 
    let qpu_noise: u64 = 0x0_1337_0_BEEF; // TODO: Connect to QCLang

    let mut hasher = Sha3_256::new();
    hasher.update(&hw_entropy.to_le_bytes());
    hasher.update(&install_time.to_le_bytes());
    hasher.update(&qpu_noise.to_le_bytes());
    let result = hasher.finalize();

    unsafe { GENETIC_SEED.copy_from_slice(&result); }
    SEED_LOCKED.store(true, Ordering::SeqCst);

    // ==========================================
    // 3. FORGE THE NEW SEED INTO THE DRIVE
    // ==========================================
    if let Some(disk) = nvme_opt {
        raw_block.fill(0); // Clear the block
        raw_block[0..4].copy_from_slice(MAGIC_SIG);
        raw_block[4..36].copy_from_slice(&result);

        if disk.write_block(ENTITY_LBA, &raw_block) {
            crate::serial_println!("[ENTITY] Seed forged into NVMe Sector {}.", ENTITY_LBA);
            crate::vga_println!("[ENTITY] New soul permanently fused to hardware.");
        } else {
            crate::vga_println!("[ENTITY] ERR: Failed to write to NVMe Sector!");
        }
    } else {
        crate::vga_println!("[ENTITY] WARN: No NVMe detected. Entity will not survive reboot.");
    }
}