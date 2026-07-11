use crate::font;
use crate::effects::{blend_color, box_blur};

/// Draws a simple solid color rectangle
pub fn draw_rect_simple(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, rw: usize, rh: usize, color: u32) {
    for row in 0..rh {
        let sy = y + row;
        if sy >= h { break; }
        for col in 0..rw {
            let sx = x + col;
            if sx >= w { break; }
            fb[sy * w + sx] = color;
        }
    }
}

/// Draws a single character using the font module (proportional TTF, antialiased). `x,y` is the
/// top-left of the line cell; the glyph sits at its bearing relative to the baseline (y + ascent).
pub fn draw_char(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, c: char, color: u32) {
    let px = font::px_for(1);
    let baseline = y as i32 + font::ascent(1) as i32;
    font::with_glyph(c, px, |g| {
        for row in 0..g.h {
            let py = baseline - g.bearing_y + row as i32;
            if py < 0 || py as usize >= h { continue; }
            for col in 0..g.w {
                let cov = g.cov[row * g.w + col];
                if cov == 0 { continue; }
                let sx = x as i32 + g.bearing_x + col as i32;
                if sx < 0 || sx as usize >= w { continue; }
                let idx = py as usize * w + sx as usize;
                fb[idx] = 0xFF00_0000 | blend_color(color, fb[idx], cov);
            }
        }
    });
}

/// Draws a string of text (proportional advance).
pub fn draw_text(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, text: &str, color: u32) {
    let mut cx = x;
    for c in text.chars() {
        draw_char(fb, w, h, cx, y, c, color);
        cx += font::advance(c, 1);
    }
}

/// Draws a filled rectangle on the framebuffer with strict bounds checking.
pub fn draw_rect(
    fb: &mut [u32], 
    screen_w: usize, 
    screen_h: usize, 
    x: usize, 
    y: usize, 
    w: usize, 
    h: usize, 
    color: u32
) {
    for dy in 0..h {
        let py = y + dy;
        // Stop if we go off the bottom of the screen
        if py >= screen_h { break; } 

        for dx in 0..w {
            let px = x + dx;
            // Stop if we go off the right side of the screen
            if px >= screen_w { break; } 

            let idx = py * screen_w + px;
            if idx < fb.len() {
                fb[idx] = color;
            }
        }
    }
}

/// Restores the wallpaper for a specific dirty rectangle.
/// UPDATED: Sets the background to PITCH BLACK (0xFF000000) as requested.
pub fn restore_wallpaper_rect(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let sy = y + row;
        if sy >= h { break; }
        // Optimization: Standard loop for safety
        for col in 0..dw {
            let sx = x + col;
            if sx >= w { break; }
            fb[sy * w + sx] = 0xFF000000; // Pure Black
        }
    }
}

/// Draws a rounded glass rectangle with a border
pub fn draw_glass_rounded_rect(
    fb: &mut [u32], 
    screen_w: usize, 
    screen_h: usize, 
    x: usize, y: usize, w: usize, h: usize, 
    radius: isize,
    tint_color: u32, 
    alpha: u8
) {
    // 1. Blur the background region (Rectangle)
    for _ in 0..3 {
        box_blur(fb, screen_w, screen_h, x, y, w, h, 1);
    }

    // Pre-calculate border colors
    let border_light = 0x88FFFFFF; 
    let border_dark  = 0x44FFFFFF;

    let r = radius as isize;

    // 2. Scan every pixel in the box
    for row in 0..h {
        let sy = y + row;
        if sy >= screen_h { break; }
        
        for col in 0..w {
            let sx = x + col;
            if sx >= screen_w { break; }

            let cx = col as isize;
            let cy = row as isize;
            let w_i = w as isize;
            let h_i = h as isize;

            // --- Rounded Corner Math (Mathematically Safe) ---
            let mut in_corner = false;
            let mut on_border = false;

            let dist_sq = if cx < r && cy < r {
                // Top-Left
                (r - cx - 1).pow(2) + (r - cy - 1).pow(2)
            } else if cx >= w_i - r && cy < r {
                // Top-Right
                (cx - (w_i - r)).pow(2) + (r - cy - 1).pow(2)
            } else if cx < r && cy >= h_i - r {
                // Bottom-Left
                (r - cx - 1).pow(2) + (cy - (h_i - r)).pow(2)
            } else if cx >= w_i - r && cy >= h_i - r {
                // Bottom-Right
                (cx - (w_i - r)).pow(2) + (cy - (h_i - r)).pow(2)
            } else {
                // Not near any corner
                0 
            };

            if dist_sq > r * r {
                in_corner = true; 
            } 
            else if dist_sq >= (r - 2).max(0).pow(2) && dist_sq <= r * r {
                on_border = true;
            } 
            else if col < 1 || col >= w - 1 || row < 1 || row >= h - 1 {
                on_border = true;
            }

            // 3. Pixel Writing
            if in_corner { 
                // Skip corners so the background (black) shows through
                continue; 
            }

            if on_border {
                // Draw Border
                let is_top_left = row < h/2 && col < w/2;
                let c = if is_top_left { border_light } else { border_dark };
                // Blend border on top of blur
                let bg = fb[sy * screen_w + sx];
                fb[sy * screen_w + sx] = blend_color(c, bg, 150);
            } else {
                // Draw Glass Body
                let bg = fb[sy * screen_w + sx];
                fb[sy * screen_w + sx] = blend_color(tint_color, bg, alpha);
            }
        }
    }
}