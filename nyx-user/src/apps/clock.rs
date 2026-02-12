// nyx-user/src/apps/clock.rs

use crate::syscalls::sys_get_time;
// FIX: Update import to use the new gfx::draw module
use crate::gfx::draw::{draw_char, draw_rect_simple};

pub struct Clock;

impl Clock {
    pub fn draw(fb: &mut [u32], screen_w: usize, screen_h: usize) {
        let ticks = sys_get_time();
        
        let total_seconds = ticks / 1000; 
        let seconds = total_seconds % 60;
        let mins = (total_seconds / 60) % 60;
        let hrs = (total_seconds / 3600) % 24;

        let taskbar_height = 50;
        let x = screen_w - 110;
        let y = screen_h - taskbar_height + 17;

        draw_rect_simple(fb, screen_w, screen_h, x, y, 90, 16, 0xFF1A1A1A);
        draw_two_digits(fb, screen_w, screen_h, x, y, hrs);

        draw_rect_simple(fb, screen_w, screen_h, x + 19, y + 4, 2, 2, 0xFFFFFFFF);
        draw_rect_simple(fb, screen_w, screen_h, x + 19, y + 10, 2, 2, 0xFFFFFFFF);

        draw_two_digits(fb, screen_w, screen_h, x + 24, y, mins);

        draw_rect_simple(fb, screen_w, screen_h, x + 43, y + 4, 2, 2, 0xFFFFFFFF);
        draw_rect_simple(fb, screen_w, screen_h, x + 43, y + 10, 2, 2, 0xFFFFFFFF);

        draw_two_digits(fb, screen_w, screen_h, x + 48, y, seconds);
    }
}

fn draw_two_digits(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, val: u64) {
    let digit1 = (val / 10) as u8;
    let digit2 = (val % 10) as u8;
    
    let char1 = (digit1 + 0x30) as char;
    let char2 = (digit2 + 0x30) as char;

    draw_char(fb, w, h, x, y, char1, 0xFFFFFFFF);
    draw_char(fb, w, h, x + 9, y, char2, 0xFFFFFFFF);
}