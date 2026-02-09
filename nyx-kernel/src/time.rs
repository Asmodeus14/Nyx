use x86_64::instructions::port::Port;
use spin::Mutex;

// 1.193182 MHz base frequency
const PIT_FREQUENCY: u32 = 1193182;
const TARGET_FREQ: u32 = 1000; // 1000 Hz = 1ms per tick
const DIVISOR: u32 = PIT_FREQUENCY / TARGET_FREQ;

static TICK_COUNTER: Mutex<u64> = Mutex::new(0);

pub fn init() {
    // Set PIT Channel 0 to 1000 Hz (Mode 3 - Square Wave)
    let mut command_port = Port::<u8>::new(0x43);
    let mut data_port = Port::<u8>::new(0x40);

    unsafe {
        command_port.write(0x36);             // 00 (Chan 0) | 11 (Lo/Hi byte) | 011 (Mode 3) | 0 (Bin)
        data_port.write((DIVISOR & 0xFF) as u8);       // Low byte
        data_port.write(((DIVISOR >> 8) & 0xFF) as u8); // High byte
    }
}

pub fn tick() {
    let mut ticks = TICK_COUNTER.lock();
    *ticks += 1;
}

pub fn get_ticks() -> u64 {
    *TICK_COUNTER.lock()
}

pub fn uptime_seconds() -> f64 {
    let ticks = get_ticks();
    (ticks as f64) / (TARGET_FREQ as f64)
}