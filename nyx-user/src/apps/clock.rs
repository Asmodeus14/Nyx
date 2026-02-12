// nyx-user/src/apps/clock.rs

use crate::syscalls::sys_get_time;
use crate::gfx::draw::{draw_char, draw_glass_rounded_rect, draw_rect_simple};

pub struct Clock;

impl Clock {
    pub fn draw(fb: &mut [u32], screen_w: usize, screen_h: usize) {
        let ticks = sys_get_time();
        
        // Calculate Time
        let total_seconds = ticks / 1000; 
        let mins = (total_seconds / 60) % 60;
        let hrs = (total_seconds / 3600) % 24;

        // --- WIDGET DIMENSIONS ---
        // Keeping the wider glass look
        let w = 220;
        let h = 80;
        
        // Position: Top Center
        let x = (screen_w / 2) - (w / 2);
        let y = 60; 

        // 1. Draw Background (Glass Effect)
        draw_glass_rounded_rect(fb, screen_w, screen_h, x, y, w, h, 20, 0x050520, 180);

        // 2. Draw Text (HH : MM only)
        // Standard char width is 9px.
        // "00 : 00" is approx 45px wide.
        
        let center_x = x + (w / 2);
        let center_y = y + (h / 2) - 8; // -8 to center the 16px high text

        // Draw Hours (Left of center)
        // Shift left by ~25px
        draw_two_digits(fb, screen_w, screen_h, center_x - 25, center_y, hrs);

        // Draw Separator (Colon)
        draw_separator(fb, screen_w, screen_h, center_x - 4, center_y);

        // Draw Minutes (Right of center)
        // Shift right by ~8px (separator width)
        draw_two_digits(fb, screen_w, screen_h, center_x + 8, center_y, mins);
    }
}

fn draw_separator(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
    // A nice glowing colon
    draw_rect_simple(fb, w, h, x + 2, y + 3, 2, 2, 0xFFFFFFFF);
    draw_rect_simple(fb, w, h, x + 2, y + 10, 2, 2, 0xFFFFFFFF);
}

fn draw_two_digits(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, val: u64) {
    let digit1 = (val / 10) as u8;
    let digit2 = (val % 10) as u8;
    
    let char1 = (digit1 + 0x30) as char;
    let char2 = (digit2 + 0x30) as char;

    draw_char(fb, w, h, x, y, char1, 0xFFFFFFFF);
    draw_char(fb, w, h, x + 9, y, char2, 0xFFFFFFFF);
}