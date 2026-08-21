use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::Port;
use core::fmt;

pub struct SerialPort {
    data: Port<u8>,
    int_en: Port<u8>,
    fifo_ctrl: Port<u8>,
    line_ctrl: Port<u8>,
    modem_ctrl: Port<u8>,
    line_sts: Port<u8>,
}

impl SerialPort {
    pub const fn new(port_base: u16) -> Self {
        Self {
            data: Port::new(port_base),
            int_en: Port::new(port_base + 1),
            fifo_ctrl: Port::new(port_base + 2),
            line_ctrl: Port::new(port_base + 3),
            modem_ctrl: Port::new(port_base + 4),
            line_sts: Port::new(port_base + 5),
        }
    }

    pub fn init(&mut self) {
        unsafe {
            self.int_en.write(0x00);     
            self.line_ctrl.write(0x80);  
            self.data.write(0x03);       
            self.int_en.write(0x00);     
            self.line_ctrl.write(0x03);  
            self.fifo_ctrl.write(0xC7);  
            self.modem_ctrl.write(0x0B); 
        }
    }

    fn wait_for_tx_empty(&mut self) {
        unsafe {
            while (self.line_sts.read() & 0x20) == 0 {
                core::hint::spin_loop();
            }
        }
    }

    pub fn write_byte(&mut self, b: u8) {
        self.wait_for_tx_empty();
        unsafe { self.data.write(b); }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' { self.write_byte(b'\r'); }
            self.write_byte(byte);
        }
        Ok(())
    }
}

lazy_static! {
    pub static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = SerialPort::new(0x3F8); // COM1
        serial_port.init();
        Mutex::new(serial_port)
    };
}

/// Breadcrumb: write to COM1 AND to `BOOT_LOG`, taking no lock and running no formatter.
///
/// `serial_println!` takes `SERIAL1`'s spin lock and runs `core::fmt`; either can swallow a
/// message if the machine dies at the wrong moment, and a swallowed message is indistinguishable
/// from code that was never reached. This avoids both.
///
/// ★ It writes `BOOT_LOG` too, and that is not a compromise — **this machine has no serial
/// capture**. `nyx-recv` is a UDP framebuffer viewer, not a console, and the laptop has no COM
/// port. Every log line anyone has ever read here came from `BOOT_LOG` via System Monitor
/// (syscall in `interrupts.rs`). A breadcrumb that only went out COM1 went nowhere at all —
/// which is exactly the mistake that made the first instrumented boot useless. The raw byte
/// copy is safe without the lock: bounds are checked, and a torn `BOOT_LOG_IDX` can only
/// interleave bytes, never write out of range.
///
/// The TX-empty wait is BOUNDED. `SerialPort::wait_for_tx_empty` spins forever, so with nothing
/// attached to COM1 it could become the very hang under investigation.
pub fn bc(s: &str) {
    unsafe {
        let mut data = Port::<u8>::new(0x3F8);
        let mut line_sts = Port::<u8>::new(0x3F8 + 5);
        for b in s.bytes() {
            let mut spins = 0u32;
            while (line_sts.read() & 0x20) == 0 && spins < 100_000 {
                spins += 1;
                core::hint::spin_loop();
            }
            data.write(b);

            if BOOT_LOG_IDX < BOOT_LOG_SIZE {
                BOOT_LOG[BOOT_LOG_IDX] = b;
                BOOT_LOG_IDX += 1;
            }
        }
    }
}

/// `bc` for a number. Hand-rolled hex because `core::fmt` is the thing being bypassed — and
/// because the formatter is where a previous bug turned every printed integer into tofu.
pub fn bc_hex(v: u64) {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..16 {
        buf[2 + i] = digits[((v >> (60 - i * 4)) & 0xF) as usize];
    }
    // Safe: every byte written above is ASCII.
    bc(unsafe { core::str::from_utf8_unchecked(&buf) });
}

// --- NEW: KERNEL BOOT LOG BUFFER ---
// 128 KB: the GPU pipeline decode (~60 lines) lands late in boot, and at 16 KB the
// buffer filled mid-decode (stopped at ~line 48) so the userspace sysmon viewer
// never saw the tail. This is a non-wrapping buffer — once full it drops the rest —
// so size it to comfortably hold a full boot plus the decode.
pub const BOOT_LOG_SIZE: usize = 131072; // 128 KB of text
pub static mut BOOT_LOG: [u8; BOOT_LOG_SIZE] = [0; BOOT_LOG_SIZE];
pub static mut BOOT_LOG_IDX: usize = 0;

struct BufWriter;
impl core::fmt::Write for BufWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        unsafe {
            for &b in s.as_bytes() {
                if BOOT_LOG_IDX < BOOT_LOG_SIZE {
                    BOOT_LOG[BOOT_LOG_IDX] = b;
                    BOOT_LOG_IDX += 1;
                }
            }
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    x86_64::instructions::interrupts::without_interrupts(|| {
        // Write to physical hardware serial
        SERIAL1.lock().write_fmt(args).expect("Printing to serial failed");
        
        // Save a copy in RAM for the userspace UI
        let mut bw = BufWriter;
        let _ = core::fmt::Write::write_fmt(&mut bw, args);
    });
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}