use crate::gfx::draw::{draw_rect_simple, draw_text};
// We need Window definition. For now, we can define a trait or just pass params.
// To keep it simple, we'll re-declare the struct dependency or pass specific fields.
// Ideally, Window struct should be in a shared `types` module, but let's accept parameters for now.

pub const TASKBAR_H: usize = 50;

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
    for i in 0..10 {
        if x + i < w && y + i < h { fb[(y + i) * w + (x + i)] = 0xFFFFFFFF; }
        if x + i < w && y < h { fb[y * w + (x + i)] = 0xFFFFFFFF; }
        if x < w && y + i < h { fb[(y + i) * w + x] = 0xFFFFFFFF; }
    }
}

// NOTE: We need the Window struct to be visible here.
// In main.rs, make Window public, or move Window struct to `src/gfx/ui.rs` entirely.
// Let's assume we move the Window struct here.
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
            // ... (Same math as before) ...
            if cx < radius && cy < radius { if (radius-cx-1).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy < radius { if (cx-(win.w as isize-radius)).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx < radius && cy >= (win.h as isize)-radius { if (radius-cx-1).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy >= (win.h as isize)-radius { if (cx-(win.w as isize-radius)).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            
            if in_corner { continue; }

            let mut c = if row < 30 { 0xFF3A3A3A } else { color };
            if col < 2 || col >= win.w - 2 || row >= win.h - 2 || row < 2 || (row >= 28 && row < 30) { c = border; }
            // Resize Indicators
            if row > win.h - 15 && col > win.w - 15 && (row + col) % 4 == 0 { c = 0xFF888888; }
            fb[sy * w + sx] = c;
        }
    }
    // Close Button
    draw_rect_simple(fb, w, h, win.x + win.w - 35, win.y + 5, 30, 20, 0xFFFF4444);
    draw_text(fb, w, h, win.x + win.w - 25, win.y + 7, "X", 0xFFFFFFFF);
    // Title
    draw_text(fb, w, h, win.x + 15, win.y + 7, win.title, 0xFFFFFFFF);
}