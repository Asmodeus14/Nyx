use x86_64::instructions::port::Port;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::string::String;
use alloc::format;

// GLOBAL TICK COUNTER (Increments every ~10ms)
pub static TICKS: AtomicU64 = AtomicU64::new(0);

// Called by the Timer Interrupt Handler
pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

// Returns uptime in ticks
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

// Returns uptime in seconds (approximate)
pub fn uptime_seconds() -> f64 {
    let ticks = get_ticks();
    // PIT is set to ~100Hz in main.rs
    ticks as f64 / 100.0
}

// --- CMOS DRIVER (Wall Clock) ---
const CMOS_ADDRESS: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

lazy_static::lazy_static! {
    pub static ref CMOS: Mutex<Cmos> = Mutex::new(Cmos::new());
}

pub struct Cmos {
    addr: Port<u8>,
    data: Port<u8>,
}

impl Cmos {
    pub fn new() -> Self {
        Cmos {
            addr: Port::new(CMOS_ADDRESS),
            data: Port::new(CMOS_DATA),
        }
    }

    fn read_register(&mut self, reg: u8) -> u8 {
        unsafe {
            self.addr.write(reg);
            self.data.read()
        }
    }

    pub fn read_rtc(&mut self) -> DateTime {
        // Spin-wait safety check
        let mut timeouts = 0;
        while self.read_upip() {
            timeouts += 1;
            if timeouts > 100_000 { break; }
            core::hint::spin_loop();
        }

        let mut second = self.read_register(0x00);
        let mut minute = self.read_register(0x02);
        let mut hour = self.read_register(0x04);
        let mut day = self.read_register(0x07);
        let mut month = self.read_register(0x08);
        let mut year = self.read_register(0x09);
        let register_b = self.read_register(0x0B);

        // Binary Coded Decimal conversion
        if (register_b & 0x04) == 0 {
            second = (second & 0x0F) + ((second / 16) * 10);
            minute = (minute & 0x0F) + ((minute / 16) * 10);
            hour = (hour & 0x0F) + ((hour / 16) * 10) | (hour & 0x80);
            day = (day & 0x0F) + ((day / 16) * 10);
            month = (month & 0x0F) + ((month / 16) * 10);
            year = (year & 0x0F) + ((year / 16) * 10);
        }

        let full_year = 2000 + year as u16;

        DateTime {
            second, minute, hour, day, month, year: full_year
        }
    }

    fn read_upip(&mut self) -> bool {
        let register_a = self.read_register(0x0A);
        (register_a & 0x80) != 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    pub second: u8,
    pub minute: u8,
    pub hour: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
}

impl DateTime {
    pub fn to_string(&self) -> String {
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", 
            self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}