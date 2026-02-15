use crate::syscalls::sys_get_time;
use crate::gfx::draw::{draw_rect_simple, draw_text};
use alloc::format;

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

        // Draw Digits (Using conversion)
        draw_two_digits(fb, w, h, center_x - 25, center_y, hrs);
        draw_text(fb, w, h, center_x - 5, center_y, ":", 0xFFFFFFFF);
        draw_two_digits(fb, w, h, center_x + 8, center_y, mins);
    }
}

fn draw_two_digits(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, val: u64) {
    let s = format!("{:02}", val);
    draw_text(fb, w, h, x, y, &s, 0xFFFFFFFF);
}