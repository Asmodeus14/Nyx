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
    /// Initializes a new serial port on the given base I/O port.
    /// COM1 is typically 0x3F8
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
            self.int_en.write(0x00);     // Disable all interrupts
            self.line_ctrl.write(0x80);  // Enable DLAB (set baud rate divisor)
            self.data.write(0x03);       // Set divisor to 3 (lo byte) 38400 baud
            self.int_en.write(0x00);     //                  (hi byte)
            self.line_ctrl.write(0x03);  // 8 bits, no parity, one stop bit
            self.fifo_ctrl.write(0xC7);  // Enable FIFO, clear them, 14-byte threshold
            self.modem_ctrl.write(0x0B); // IRQs enabled, RTS/DSR set
        }
    }

    fn wait_for_tx_empty(&mut self) {
        unsafe {
            // Bit 5 of the line status register tells us if the transmit buffer is empty
            while (self.line_sts.read() & 0x20) == 0 {
                core::hint::spin_loop();
            }
        }
    }

    pub fn write_byte(&mut self, b: u8) {
        self.wait_for_tx_empty();
        unsafe {
            self.data.write(b);
        }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            // Standardize newlines for serial terminals (CR+LF)
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

lazy_static! {
    pub static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = SerialPort::new(0x3F8); // 0x3F8 is COM1
        serial_port.init();
        Mutex::new(serial_port)
    };
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    // We disable interrupts while printing so a timer context switch 
    // doesn't interrupt a thread mid-print and scramble the log text.
    x86_64::instructions::interrupts::without_interrupts(|| {
        SERIAL1.lock().write_fmt(args).expect("Printing to serial failed");
    });
}

/// Prints to the host through the serial interface.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*));
    };
}

/// Prints to the host through the serial interface, appending a newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}