use x86_64::instructions::port::Port;
use core::sync::atomic::{AtomicU64, Ordering};

const PIT_FREQUENCY: u32 = 1193182;
const TARGET_FREQ: u32 = 1000;
const DIVISOR: u32 = PIT_FREQUENCY / TARGET_FREQ;

// FIX: Must be public for syscalls
pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
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