use crate::gfx::draw::{draw_rect_simple, draw_text, draw_char};
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
    // Start Button
    draw_rect_simple(fb, w, h, 10, y_start + 10, 30, 30, 0xFF00AAFF);
    draw_text(fb, w, h, 50, y_start + 17, "NYX", 0xFFFFFFFF);
}

pub fn draw_cursor(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
    // Simple cursor (white cross)
    for i in 0..10 {
        if x + i < w && y + i < h { fb[(y + i) * w + (x + i)] = 0xFFFFFFFF; }
        if x + i < w && y < h { fb[y * w + (x + i)] = 0xFFFFFFFF; }
        if x < w && y + i < h { fb[(y + i) * w + x] = 0xFFFFFFFF; }
    }
}

pub fn draw_window_rounded(fb: &mut [u32], w: usize, h: usize, win: &Window) {
    let color = if win.active { 0xFF2A2A2A } else { 0xFF1A1A1A };
    let border = if win.active { 0xFF00AAFF } else { 0xFF444444 };
    let radius: isize = 12;
    let radius_sq = radius * radius;

    for row in 0..win.h {
        let sy = win.y + row; if sy >= h { break; }
        for col in 0..win.w {
            let sx = win.x + col; if sx >= w { break; }
            let cx = col as isize;
            let cy = row as isize;

            // Rounded Corners Logic
            let mut in_corner = false;
            if cx < radius && cy < radius { if (radius-cx-1).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy < radius { if (cx-(win.w as isize-radius)).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx < radius && cy >= (win.h as isize)-radius { if (radius-cx-1).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy >= (win.h as isize)-radius { if (cx-(win.w as isize-radius)).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            
            if in_corner { continue; }

            let mut c = if row < 30 { 0xFF3A3A3A } else { color };
            if col < 2 || col >= win.w - 2 || row >= win.h - 2 || row < 2 || (row >= 28 && row < 30) { c = border; }
            if row > win.h - 15 && col > win.w - 15 && (row + col) % 4 == 0 { c = 0xFF888888; } // Resize grip
            fb[sy * w + sx] = c;
        }
    }
    // Close Button
    draw_rect_simple(fb, w, h, win.x + win.w - 35, win.y + 5, 30, 20, 0xFFFF4444);
    draw_text(fb, w, h, win.x + win.w - 25, win.y + 7, "X", 0xFFFFFFFF);
    // Title
    draw_text(fb, w, h, win.x + 15, win.y + 7, win.title, 0xFFFFFFFF);
}

// --- NEW: File Icon Drawer ---
pub fn draw_file_icon(fb: &mut [u32], screen_w: usize, screen_h: usize, x: usize, y: usize, label: &str) {
    // 1. Icon Body (White sheet)
    draw_rect_simple(fb, screen_w, screen_h, x + 10, y, 40, 50, 0xFFEEEEEE);
    
    // 2. Dog-ear fold (Top right)
    draw_rect_simple(fb, screen_w, screen_h, x + 50 - 10, y, 10, 10, 0xFFCCCCCC);
    
    // 3. Text Lines (Fake text)
    draw_rect_simple(fb, screen_w, screen_h, x + 15, y + 15, 25, 2, 0xFF888888);
    draw_rect_simple(fb, screen_w, screen_h, x + 15, y + 25, 20, 2, 0xFF888888);
    draw_rect_simple(fb, screen_w, screen_h, x + 15, y + 35, 28, 2, 0xFF888888);

    // 4. Label (Filename below icon)
    // Center text roughly. 9px per char.
    let text_len = label.len() * 9; 
    // If text is wider than icon area (60px), align left, else center
    let text_x = if text_len < 60 { x + 30 - (text_len / 2) } else { x };
    
    // Draw text with shadow for visibility on wallpapers
    draw_text(fb, screen_w, screen_h, text_x + 1, y + 56, label, 0xFF000000); // Shadow
    draw_text(fb, screen_w, screen_h, text_x, y + 55, label, 0xFFFFFFFF);     // Text
}