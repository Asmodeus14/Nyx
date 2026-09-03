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

#[inline]
fn bin_to_bcd(v: u8) -> u8 {
    ((v / 10) << 4) | (v % 10)
}

// ---------------------------------------------------------------------------------------------
// TIMEZONE (CMOS 0x6B..0x6C, little-endian i16 minutes east of UTC)
//
// ★★★ The offset is stored, and ONLY ever used, for DISPLAY. The RTC and CLOCK_REALTIME stay in
// UTC without exception, because everything that consumes time for correctness rather than for a
// human wants UTC: TLS certificate validity windows, HTTP `Date:` headers, file timestamps.
// Localising the system clock is how Windows/Linux dual-boot machines end up fighting over the
// hardware clock, and it would reintroduce exactly the "certificate not valid yet" failure this
// whole clock effort existed to fix.
//
// Minutes rather than hours because India (+5:30), Nepal (+5:45), and Chatham (+12:45) are not
// whole-hour offsets — an hours-only field is wrong for a sixth of the planet.
//
// CMOS rather than a file: the taskbar clock lives in the compositor, which starts before any
// filesystem the user could put a config in, and NVRAM needs no mount.
// ---------------------------------------------------------------------------------------------

const REG_TZ_LO: u8 = 0x6B;
const REG_TZ_HI: u8 = 0x6C;

/// Widest real-world offsets: UTC-12:00 (Baker Island) to UTC+14:00 (Kiritimati).
const TZ_MIN: i16 = -12 * 60;
const TZ_MAX: i16 = 14 * 60;

/// Minutes east of UTC, or 0 if never set / out of range.
///
/// The range check doubles as the validity marker: uninitialised NVRAM is arbitrary, and a wrong
/// timezone is cosmetic, so a plausibility test is proportionate where a magic byte would not be.
pub fn tz_offset_min() -> i16 {
    let raw = x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        (read_reg(REG_TZ_LO) as u16) | ((read_reg(REG_TZ_HI) as u16) << 8)
    }) as i16;
    if (TZ_MIN..=TZ_MAX).contains(&raw) { raw } else { 0 }
}

/// Persist the display timezone. Returns false if the offset is not a real one.
pub fn set_tz_offset_min(minutes: i16) -> bool {
    if !(TZ_MIN..=TZ_MAX).contains(&minutes) {
        return false;
    }
    let v = minutes as u16;
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        write_reg(REG_TZ_LO, (v & 0xFF) as u8);
        write_reg(REG_TZ_HI, (v >> 8) as u8);
    });
    true
}

/// Write a single CMOS register. Caller must hold interrupts off.
unsafe fn write_reg(reg: u8, val: u8) {
    let mut addr: Port<u8> = Port::new(CMOS_ADDR);
    let mut data: Port<u8> = Port::new(CMOS_DATA);
    addr.write(reg & 0x7F);
    data.write(val);
}

/// Set the battery-backed wall clock. Returns false if the RTC never went quiet.
///
/// ★★★ This is the only function in the tree that WRITES the machine's clock, and it is the
/// difference between a time correction that lasts a session and one that lasts. Without it the
/// only way to fix a wrong clock is the firmware setup screen — which is a genuinely bad answer:
/// most people do not know the key, and an OS that cannot manage its own clock is broken.
///
/// The sequence is the MC146818 datasheet one and every step is load-bearing:
///
///  1. **Set the SET bit (0x80) in Status B first.** This halts the once-per-second update cycle.
///     Writing while the RTC is mid-update races the hardware's own carry propagation and can
///     leave the clock somewhere neither we nor it intended.
///  2. **Match the format the RTC is already in.** Status B advertises BCD-vs-binary and 12-vs-24
///     hour, and writing binary into a BCD clock does not fail, it silently stores a wrong time —
///     which then reads back as a plausible but incorrect date. That is worse than refusing.
///  3. **Clear SET last**, so the clock starts ticking again from the value just written.
///
/// Interrupts stay masked throughout: the index/data pair is two ports, and `postmortem` writes
/// CMOS from the timer interrupt, so an interrupt landing between them would write our data byte
/// through its register index.
pub fn set_time(year: u16, month: u8, day: u8, hour: u8, min: u8, sec: u8) -> bool {
    // Refuse obvious nonsense rather than storing it — a bad write here is not recoverable from
    // software, because the value we would need to fix it is the one we just destroyed.
    if !(1..=12).contains(&month) || !(1..=31).contains(&day)
        || hour > 23 || min > 59 || sec > 59
        || !(2000..=2099).contains(&year)
    {
        return false;
    }

    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        // Wait for any in-flight update to finish before halting the clock.
        let mut guard = 0u32;
        while update_in_progress() {
            guard += 1;
            if guard > 1_000_000 {
                return false;
            }
            core::hint::spin_loop();
        }

        let status_b = read_reg(REG_STATUS_B);
        let is_binary = status_b & STATUS_B_BINARY != 0;
        let is_24h = status_b & STATUS_B_24HOUR != 0;

        // 12-hour clocks need the PM flag in bit 7 and 12/24 folded to 1..=12.
        let (hour_field, pm) = if is_24h {
            (hour, false)
        } else {
            let pm = hour >= 12;
            let h12 = match hour % 12 { 0 => 12, h => h };
            (h12, pm)
        };

        let enc = |v: u8| if is_binary { v } else { bin_to_bcd(v) };

        write_reg(REG_STATUS_B, status_b | 0x80); // SET: freeze updates

        write_reg(REG_SECONDS, enc(sec));
        write_reg(REG_MINUTES, enc(min));
        write_reg(REG_HOURS, enc(hour_field) | if pm { 0x80 } else { 0 });
        write_reg(REG_DAY, enc(day));
        write_reg(REG_MONTH, enc(month));
        write_reg(REG_YEAR, enc((year % 100) as u8));

        write_reg(REG_STATUS_B, status_b & !0x80); // resume ticking
        true
    })
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
