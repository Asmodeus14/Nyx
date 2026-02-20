use core::fmt;
use spin::Mutex;
use crate::gui::Color;
use crate::gui::Painter;

pub struct VgaLogger {
    pub x: usize,
    pub y: usize,
}

// Start drawing text at Y=70 so we don't overwrite your kernel boot header
pub static VGA_LOGGER: Mutex<VgaLogger> = Mutex::new(VgaLogger { x: 10, y: 70 });

impl fmt::Write for VgaLogger {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        unsafe {
            if let Some(painter) = &mut crate::SCREEN_PAINTER {
                for c in s.chars() {
                    if c == '\n' {
                        self.x = 10;
                        self.y += 20;
                    } else {
                        let mut buf = [0; 4];
                        let char_str = c.encode_utf8(&mut buf);
                        // Using YELLOW to make debug logs pop on the physical screen
                        painter.draw_string(self.x, self.y, char_str, Color::YELLOW);
                        self.x += 8; // Assuming an 8px wide font
                    }
                    
                    // Screen wrap horizontally
                    if self.x >= painter.info.width - 10 {
                        self.x = 10;
                        self.y += 20;
                    }
                    
                    // Screen wrap vertically (loop back to top, clear area)
                    if self.y >= painter.info.height - 20 {
                        self.y = 70;
                        // Optional: painter.clear(Color::BLACK); to refresh, 
                        // but skipping it leaves a trail of logs you can read
                    }
                }
            }
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _vga_print(args: fmt::Arguments) {
    use core::fmt::Write;
    // Disable interrupts so a context switch doesn't split a log message in half
    x86_64::instructions::interrupts::without_interrupts(|| {
        VGA_LOGGER.lock().write_fmt(args).unwrap();
    });
}

#[macro_export]
macro_rules! vga_print {
    ($($arg:tt)*) => ($crate::vga_log::_vga_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! vga_println {
    () => ($crate::vga_print!("\n"));
    ($($arg:tt)*) => ($crate::vga_print!("{}\n", format_args!($($arg)*)));
}