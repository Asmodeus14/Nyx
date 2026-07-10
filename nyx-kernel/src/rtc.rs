// src/rtc.rs
//
// MC146818-compatible CMOS Real-Time Clock reader (the "CMOS RTC" every PC has on the LPC/ISA
// bridge). This is the machine's battery-backed wall clock — distinct from `time::UPTIME_MS`
// (a PIT-tick monotonic counter that resets every boot) and the calibrated TSC. Userspace reads
// it via syscall 528 (see interrupts.rs) to draw a real clock in the taskbar.
//
// Access model (canonical, per the MC146818 datasheet / OSDev "CMOS" article — NOT guessed):
//   - Port 0x70 = index/address (write the register number; bit 7 is the NMI-disable bit, we keep
//     it clear so we don't inadvertently mask NMIs).
//   - Port 0x71 = data (read/write the selected register).
//   - Time/date registers: sec=0x00, min=0x02, hour=0x04, weekday=0x06, day=0x07, month=0x08,
//     year=0x09. Status A=0x0A (bit7 UIP = update-in-progress), Status B=0x0B (bit2 DM: 1=binary
//     0=BCD; bit1 24/12: 1=24-hour 0=12-hour).
//
// Correctness: the RTC updates its registers once per second; reading mid-update yields garbage.
// We poll UIP clear, read all fields, then read again and compare — retrying until two consecutive
// reads agree, which guarantees the snapshot didn't straddle an update. BCD values are converted to
// binary, and 12-hour times (with the 0x80 PM flag) are normalized to 24-hour.

use x86_64::instructions::port::Port;

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

const STATUS_A_UIP: u8 = 0x80;
const STATUS_B_BINARY: u8 = 0x04; // Data Mode: 1 = binary, 0 = BCD
const STATUS_B_24HOUR: u8 = 0x02; // Hour Format: 1 = 24-hour, 0 = 12-hour

/// One raw CMOS snapshot (still in whatever format Status B advertises).
#[derive(Clone, Copy, PartialEq, Eq)]
struct RawTime {
    sec: u8,
    min: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8, // two-digit (00..99); the century register is unreliable, so we assume 20xx.
}

/// Read a single CMOS register. Caller must hold interrupts off around a multi-register sequence.
unsafe fn read_reg(reg: u8) -> u8 {
    let mut addr: Port<u8> = Port::new(CMOS_ADDR);
    let mut data: Port<u8> = Port::new(CMOS_DATA);
    // Preserve the NMI-disable bit (bit 7) as clear.
    addr.write(reg & 0x7F);
    data.read()
}

/// True while the RTC is mid-update (its registers must not be read).
unsafe fn update_in_progress() -> bool {
    read_reg(REG_STATUS_A) & STATUS_A_UIP != 0
}

/// Take one coherent snapshot: wait for UIP to clear, then read every field.
unsafe fn read_once() -> RawTime {
    // Bounded spin so a dead/absent RTC can never hang the syscall.
    let mut guard = 0u32;
    while update_in_progress() {
        guard += 1;
        if guard > 1_000_000 {
            break;
        }
        core::hint::spin_loop();
    }
    RawTime {
        sec: read_reg(REG_SECONDS),
        min: read_reg(REG_MINUTES),
        hour: read_reg(REG_HOURS),
        day: read_reg(REG_DAY),
        month: read_reg(REG_MONTH),
        year: read_reg(REG_YEAR),
    }
}

#[inline]
fn bcd_to_bin(v: u8) -> u8 {
    ((v & 0xF0) >> 1) + ((v & 0xF0) >> 3) + (v & 0x0F)
}

/// Read the wall-clock time and pack it into a u64 for the syscall ABI:
///   bits [7:0]  seconds   [15:8]  minutes  [23:16] hour (0..23)
///   bits [31:24] day      [39:32] month    [63:40] year (full, e.g. 2026)
///
/// All fields are binary and 24-hour normalized here so userspace never has to know the RTC's
/// BCD/12-hour configuration.
pub fn read_packed() -> u64 {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        // Read twice and compare so the snapshot can't straddle a per-second update.
        let mut t = read_once();
        loop {
            let again = read_once();
            if again == t {
                break;
            }
            t = again;
        }

        let status_b = read_reg(REG_STATUS_B);
        let is_binary = status_b & STATUS_B_BINARY != 0;
        let is_24h = status_b & STATUS_B_24HOUR != 0;

        // The 12-hour PM flag lives in bit 7 of the hour register BEFORE any BCD conversion.
        let hour_pm = (t.hour & 0x80) != 0;
        let raw_hour = t.hour & 0x7F;

        let (sec, min, mut hour, day, month, year2) = if is_binary {
            (t.sec, t.min, raw_hour, t.day, t.month, t.year)
        } else {
            (
                bcd_to_bin(t.sec),
                bcd_to_bin(t.min),
                bcd_to_bin(raw_hour),
                bcd_to_bin(t.day),
                bcd_to_bin(t.month),
                bcd_to_bin(t.year),
            )
        };

        // Normalize 12-hour → 24-hour. 12 AM = 00:xx, 12 PM = 12:xx.
        if !is_24h {
            if hour == 12 {
                hour = 0;
            }
            if hour_pm {
                hour += 12;
            }
        }

        // The century register is famously unreliable across vendors; two-digit year + 2000 base is
        // correct for any realistic date this OS runs in.
        let year: u64 = 2000 + year2 as u64;

        (year << 40)
            | ((month as u64) << 32)
            | ((day as u64) << 24)
            | ((hour as u64) << 16)
            | ((min as u64) << 8)
            | (sec as u64)
    })
}
