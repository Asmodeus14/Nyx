use crate::gfx::font;

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

/// Restores the wallpaper (gradient) for a specific dirty rectangle
pub fn restore_wallpaper_rect(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let sy = y + row;
        if sy >= h { break; }
        for col in 0..dw {
            let sx = x + col;
            if sx >= w { break; }
            // Simple Gradient Wallpaper Logic
            let b = (sy * 255 / h) as u32;
            let g = (sx * 100 / w) as u32;
            fb[sy * w + sx] = 0xFF000000 | (g << 8) | b;
        }
    }
}