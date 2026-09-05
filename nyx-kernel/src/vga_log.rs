use core::fmt;
use core::fmt::Write;
use spin::Mutex;
use crate::gui::Color;
use crate::gui::Painter;

// ─────────────────────────────────────────────────────────────────────────
// TYPOGRAPHY & SPACING CONSTANTS
// ─────────────────────────────────────────────────────────────────────────
const MARGIN_LEFT: usize = 10;
const MARGIN_TOP: usize = 70;

// 🚨 THE FIX: Explicitly add spacing so characters and lines don't touch
const FONT_WIDTH: usize = 8;
const CHAR_SPACING: usize = 5; 
const CHAR_ADVANCE: usize = FONT_WIDTH + CHAR_SPACING; // 13px total

const FONT_HEIGHT: usize = 16;
const LINE_SPACING: usize = 8;
const LINE_ADVANCE: usize = FONT_HEIGHT + LINE_SPACING; // 24px total

pub struct VgaLogger {
    pub x: usize,
    pub y: usize,
}

// Start drawing text at Y=70 so we don't overwrite your kernel boot header
pub static VGA_LOGGER: Mutex<VgaLogger> = Mutex::new(VgaLogger { x: MARGIN_LEFT, y: MARGIN_TOP });

impl fmt::Write for VgaLogger {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        unsafe {
            if let Some(painter) = &mut crate::SCREEN_PAINTER {
                for c in s.chars() {
                    
                    if c == '\n' {
                        self.x = MARGIN_LEFT;
                        self.y += LINE_ADVANCE;
                    } else {
                        // 🚨 THE FIX: Check boundaries BEFORE drawing to prevent edge-clipping
                        if self.x + CHAR_ADVANCE >= painter.info.width - MARGIN_LEFT {
                            self.x = MARGIN_LEFT;
                            self.y += LINE_ADVANCE;
                        }

                        let mut buf = [0; 4];
                        let char_str = c.encode_utf8(&mut buf);
                        
                        // Using YELLOW to make debug logs pop on the physical screen
                        painter.draw_string(self.x, self.y, char_str, Color::YELLOW);
                        
                        // Move cursor forward with our new spacing math
                        self.x += CHAR_ADVANCE; 
                    }
                    
                    // Screen wrap vertically (loop back to top)
                    if self.y + LINE_ADVANCE >= painter.info.height - 20 {
                        self.y = MARGIN_TOP;
                        
                        // Optional: clear a block here if the text turns into a smeared mess
                        // painter.clear(Color::BLACK); 
                    }
                }
            }
        }
        Ok(())
    }
}

/// Put the log cursor back at the top-left.
///
/// Used when the cold-start screen hands the framebuffer back mid-boot: the log should start at
/// the top of a freshly cleared screen, not wherever it had reached before the screen took over.
pub fn reset_cursor() {
    let mut l = VGA_LOGGER.lock();
    l.x = MARGIN_LEFT;
    l.y = MARGIN_TOP;
}

#[doc(hidden)]
pub fn _vga_print(args: fmt::Arguments) {
    // ★ The cold-start screen and this logger draw to the same pixels, and the loser is whichever
    // ran second. While the graphical screen is up the `[BOOT]` stream goes to serial only — which
    // is where every one of these lines was already going, so nothing is lost. Holding a key at
    // power-on suppresses the screen instead and the log comes back; see `boot_screen::begin`.
    if crate::boot_screen::is_quiet() {
        return;
    }
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