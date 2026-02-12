// nyx-user/src/clock.rs

use crate::syscalls::sys_get_time;
use crate::{draw_char, draw_rect_simple};

pub struct Clock;

impl Clock {
    pub fn draw(fb: &mut [u32], screen_w: usize, screen_h: usize) {
        let ticks = sys_get_time();
        
        // Assuming your kernel timer runs at 1000Hz (1ms per tick)
        // If your timer is slower (e.g., 18.2Hz standard VGA), adjust math here.
        let total_seconds = ticks / 1000; 
        let seconds = total_seconds % 60;
        let mins = (total_seconds / 60) % 60;
        let hrs = (total_seconds / 3600) % 24;

        let taskbar_height = 50;
        // Position: Bottom right corner
        let x = screen_w - 110;
        let y = screen_h - taskbar_height + 17;

        // 1. Clear the area (Paint over with Taskbar Color)
        // This prevents the numbers from smearing on top of each other
        draw_rect_simple(fb, screen_w, screen_h, x, y, 90, 16, 0xFF1A1A1A);

        // 2. Draw Hours
        draw_two_digits(fb, screen_w, screen_h, x, y, hrs);

        // Separator ':'
        draw_rect_simple(fb, screen_w, screen_h, x + 19, y + 4, 2, 2, 0xFFFFFFFF);
        draw_rect_simple(fb, screen_w, screen_h, x + 19, y + 10, 2, 2, 0xFFFFFFFF);

        // 3. Draw Minutes
        draw_two_digits(fb, screen_w, screen_h, x + 24, y, mins);

        // Separator ':'
        draw_rect_simple(fb, screen_w, screen_h, x + 43, y + 4, 2, 2, 0xFFFFFFFF);
        draw_rect_simple(fb, screen_w, screen_h, x + 43, y + 10, 2, 2, 0xFFFFFFFF);

        // 4. Draw Seconds
        draw_two_digits(fb, screen_w, screen_h, x + 48, y, seconds);
    }
}

// Helper to draw "05" instead of "5"
fn draw_two_digits(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, val: u64) {
    let digit1 = (val / 10) as u8;
    let digit2 = (val % 10) as u8;
    
    // 0x30 is ASCII '0'
    let char1 = (digit1 + 0x30) as char;
    let char2 = (digit2 + 0x30) as char;

    draw_char(fb, w, h, x, y, char1, 0xFFFFFFFF);
    draw_char(fb, w, h, x + 9, y, char2, 0xFFFFFFFF);
}