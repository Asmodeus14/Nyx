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

// --- NEW: KERNEL BOOT LOG BUFFER ---
pub const BOOT_LOG_SIZE: usize = 16384; // 16 KB of text
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
        $crate::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}