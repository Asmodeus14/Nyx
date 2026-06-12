use crate::syscalls::sys_get_time;
use crate::gfx::draw::{draw_rect_simple, draw_text};

pub struct Clock;

impl Clock {
    pub fn draw(fb: &mut [u32], w: usize, h: usize) {
        let ms = sys_get_time();
        let secs_total = ms / 1000;
        let mins_total = secs_total / 60;
        let hrs_total = mins_total / 60;

        let hrs = (hrs_total % 24) as u64;
        let mins = (mins_total % 60) as u64;
        
        let center_x = w / 2;
        let center_y = 60; // Top position

        // Background Box
        draw_rect_simple(fb, w, h, center_x - 60, center_y - 10, 120, 40, 0xAA000000);

        // Draw Digits (Zero-Allocation!)
        draw_two_digits(fb, w, h, center_x - 25, center_y, hrs);
        draw_text(fb, w, h, center_x - 5, center_y, ":", 0xFFFFFFFF);
        draw_two_digits(fb, w, h, center_x + 8, center_y, mins);
    }
}

fn draw_two_digits(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, val: u64) {
    let mut buf = [0u8; 2];
    buf[0] = b'0' + ((val / 10) % 10) as u8; // Tens digit
    buf[1] = b'0' + (val % 10) as u8;        // Ones digit
    
    // MICRO-OPTIMIZATION: We mathematically guarantee these are valid ASCII characters.
    // Bypassing the runtime UTF-8 validation saves CPU cycles in the render loop!
    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    draw_text(fb, w, h, x, y, s, 0xFFFFFFFF);
}