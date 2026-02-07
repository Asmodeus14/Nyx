use x86_64::instructions::port::Port;
use spin::Mutex;
use lazy_static::lazy_static;

const PIT_COMMAND: u16 = 0x43;
const PIT_CHANNEL0: u16 = 0x40;

// 1.193182 MHz / 11932 = ~100 Hz (10ms per tick)
const DIVISOR: u16 = 11932; 

lazy_static! {
    pub static ref TICKS: Mutex<u64> = Mutex::new(0);
}

pub fn init() {
    unsafe {
        let mut command = Port::<u8>::new(PIT_COMMAND);
        let mut channel0 = Port::<u8>::new(PIT_CHANNEL0);

        command.write(0x36);
        channel0.write((DIVISOR & 0xFF) as u8);
        channel0.write((DIVISOR >> 8) as u8);
    }
}

pub fn tick() {
    let mut ticks = TICKS.lock();
    *ticks += 1;
}

pub fn get_ticks() -> u64 {
    *TICKS.lock()
}

pub fn uptime_seconds() -> f64 {
    let ticks = get_ticks();
    (ticks as f64) / 100.0 // Correct math for 100Hz
}