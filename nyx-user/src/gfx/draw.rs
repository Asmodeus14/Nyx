use crate::gfx::font;
use crate::gfx::effects::{blend_color, box_blur};

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

/// Draws a single character using the font module
pub fn draw_char(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, c: char, color: u32) {
    if let Some(raster) = font::get_char_raster(c) {
        for (ri, row) in raster.raster().iter().enumerate() {
            for (ci, val) in row.iter().enumerate() {
                if *val > 10 { // Intensity threshold
                    let px = x + ci;
                    let py = y + ri;
                    if px < w && py < h {
                        fb[py * w + px] = color;
                    }
                }
            }
        }
    }
}

/// Draws a string of text
pub fn draw_text(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, text: &str, color: u32) {
    let mut cx = x;
    for c in text.chars() {
        draw_char(fb, w, h, cx, y, c, color);
        cx += font::CHAR_WIDTH;
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
    // This creates the "frost" effect.
    for _ in 0..3 {
        box_blur(fb, screen_w, screen_h, x, y, w, h, 1);
    }

    let radius_sq = radius * radius;
    // Pre-calculate border colors
    // Top/Left is brighter (Light source), Bottom/Right is dimmer
    let border_light = 0x88FFFFFF; 
    let border_dark  = 0x44FFFFFF;

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

            // --- Rounded Corner Math ---
            let mut in_corner = false;
            let mut on_border = false;

            // Check Top-Left
            if cx < radius && cy < radius {
                let d = (radius - cx - 1).pow(2) + (radius - cy - 1).pow(2);
                if d > radius_sq { in_corner = true; }      // Outside rounded area
                else if d >= (radius - 2).pow(2) { on_border = true; } // Edge
            }
            // Check Top-Right
            else if cx >= w_i - radius && cy < radius {
                let d = (cx - (w_i - radius)).pow(2) + (radius - cy - 1).pow(2);
                if d > radius_sq { in_corner = true; }
                else if d >= (radius - 2).pow(2) { on_border = true; }
            }
            // Check Bottom-Left
            else if cx < radius && cy >= h_i - radius {
                let d = (radius - cx - 1).pow(2) + (cy - (h_i - radius)).pow(2);
                if d > radius_sq { in_corner = true; }
                else if d >= (radius - 2).pow(2) { on_border = true; }
            }
            // Check Bottom-Right
            else if cx >= w_i - radius && cy >= h_i - radius {
                let d = (cx - (w_i - radius)).pow(2) + (cy - (h_i - radius)).pow(2);
                if d > radius_sq { in_corner = true; }
                else if d >= (radius - 2).pow(2) { on_border = true; }
            }
            // Check Straight Edges
            else {
                if col < 1 || col >= w - 1 || row < 1 || row >= h - 1 { on_border = true; }
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