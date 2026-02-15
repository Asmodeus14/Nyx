use crate::syscalls;

pub struct Console;

impl Console {
    pub fn print(s: &str) {
        syscalls::sys_print(s);
    }

    pub fn print_char(c: char) {
        let mut b = [0u8; 4];
        let s = c.encode_utf8(&mut b);
        syscalls::sys_print(s);
    }
}

// Helper macros
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let mut writer = $crate::console::StringWriter {};
        let _ = writer.write_fmt(format_args!($($arg)*));
    });
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub struct StringWriter;
impl core::fmt::Write for StringWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        syscalls::sys_print(s);
        Ok(())
    }
}