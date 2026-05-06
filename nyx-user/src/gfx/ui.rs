use crate::gfx::draw::{draw_rect_simple, draw_text, draw_glass_rounded_rect};
use crate::gfx::font::{CHAR_WIDTH, CHAR_HEIGHT};

pub const TASKBAR_H: usize = 50;

#[derive(Clone, Copy)]
pub struct Window {
    pub id: usize,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub title: &'static str,
    pub active: bool,
    pub exists: bool
}

pub fn draw_taskbar(fb: &mut [u32], w: usize, h: usize) {
    let y_start = h - TASKBAR_H;
    for y in y_start..h {
        let color = if y == y_start { 0xFF555555 } else { 0xFF1A1A1A };
        for x in 0..w { fb[y * w + x] = color; }
    }
    
    draw_rect_simple(fb, w, h, 10, y_start + 10, 30, 30, 0xFF00AAFF);
    draw_text(fb, w, h, 50, y_start + 17, "NYX", 0xFFFFFFFF);
}

pub fn draw_cursor(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
    for i in 0..10 {
        if x + i < w && y + i < h { fb[(y + i) * w + (x + i)] = 0xFFFFFFFF; }
        if x + i < w && y < h { fb[y * w + (x + i)] = 0xFFFFFFFF; }
        if x < w && y + i < h { fb[(y + i) * w + x] = 0xFFFFFFFF; }
    }
}

pub fn draw_window_rounded(fb: &mut [u32], w: usize, h: usize, win: &Window) {
    // PILLAR 1: ALPHA COMPOSITING
    // Define the tint color and opacity (0-255) based on window focus
    let tint = if win.active { 0xFF181818 } else { 0xFF2A2A2A };
    let alpha = if win.active { 210 } else { 150 }; 
    
    // Draw the stunning blurred glass background
    draw_glass_rounded_rect(fb, w, h, win.x, win.y, win.w, win.h, 12, tint, alpha);
    
    // Draw the Border Highlight
    let border = if win.active { 0xFF00AAFF } else { 0xFF444444 };
    draw_rect_simple(fb, w, h, win.x, win.y, win.w, 2, border); // Top border
    
    // Close Button
    draw_rect_simple(fb, w, h, win.x + win.w - 35, win.y + 5, 30, 20, 0xFFFF4444);
    draw_text(fb, w, h, win.x + win.w - 25, win.y + 7, "X", 0xFFFFFFFF);
    
    // Title
    draw_text(fb, w, h, win.x + 15, win.y + 7, win.title, 0xFFFFFFFF);
}

pub fn draw_file_icon(fb: &mut [u32], screen_w: usize, screen_h: usize, x: usize, y: usize, label: &str) {
    draw_rect_simple(fb, screen_w, screen_h, x + 10, y, 40, 50, 0xFFEEEEEE);
    draw_rect_simple(fb, screen_w, screen_h, x + 50 - 10, y, 10, 10, 0xFFCCCCCC);
    draw_rect_simple(fb, screen_w, screen_h, x + 15, y + 15, 25, 2, 0xFF888888);
    draw_rect_simple(fb, screen_w, screen_h, x + 15, y + 25, 20, 2, 0xFF888888);
    draw_rect_simple(fb, screen_w, screen_h, x + 15, y + 35, 28, 2, 0xFF888888);
    
    let text_len = label.len() * 9; 
    let text_x = if text_len < 60 { x + 30 - (text_len / 2) } else { x };
    
    draw_text(fb, screen_w, screen_h, text_x + 1, y + 56, label, 0xFF000000); 
    draw_text(fb, screen_w, screen_h, text_x, y + 55, label, 0xFFFFFFFF);
}