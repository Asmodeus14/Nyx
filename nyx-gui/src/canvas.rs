use crate::effects::{alpha_blend, apply_opacity};

pub struct Color;
impl Color {
    // --- THE NEW WARM WHITE THEME ---
    pub const WARM_BG: u32       = 0xFF_F9F9F6; // Warm cream background
    pub const WARM_SURFACE: u32  = 0xFF_FFFFFF; // Pure white for window cards
    pub const WARM_BORDER: u32   = 0xFF_E2E2DB; // Soft gray border
    
    pub const TEXT_DARK: u32     = 0xFF_2D2D2A; // Deep charcoal text
    pub const TEXT_MUTED: u32    = 0xFF_8A8A85; // Muted gray for subtitles
    
    pub const ACCENT_PRIMARY: u32 = 0xFF_E67E22; // Warm Bronze/Orange accent
    pub const ACCENT_HOVER: u32   = 0xFF_D35400; // Darker bronze for clicks
    pub const ACCENT_GREEN: u32   = 0xFF_27AE60; // Success toggles
    
    // Legacy mapping to keep older apps from crashing
    pub const BLACK: u32 = 0xFF_000000;
    pub const WHITE: u32 = 0xFF_FFFFFF;
    pub const NYX_ORANGE: u32  = 0xFF_FF5722;
}

pub struct Canvas<'a> {
    pub buffer: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

impl<'a> Canvas<'a> {
    pub fn new(buffer: &'a mut [u32], width: usize, height: usize) -> Self {
        Self { buffer, width, height }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let a = (color >> 24) & 0xFF;
        if a == 0 { return; }

        for cy in 0..h {
            for cx in 0..w {
                let idx = (y + cy) * self.width + (x + cx);
                if idx < self.buffer.len() { 
                    if a == 255 {
                        self.buffer[idx] = color;
                    } else {
                        self.buffer[idx] = alpha_blend(color, self.buffer[idx]);
                    }
                }
            }
        }
    }

    /// Wayland-Style Fluid Compositing: Draws an app's buffer with a master fade opacity!
    pub fn composite_buffer(&mut self, x: usize, y: usize, src: &[u32], src_w: usize, src_h: usize, opacity: u8) {
        if opacity == 0 { return; }
        
        for cy in 0..src_h {
            for cx in 0..src_w {
                let dst_idx = (y + cy) * self.width + (x + cx);
                let src_idx = cy * src_w + cx;
                
                if dst_idx < self.buffer.len() && src_idx < src.len() {
                    let pixel = src[src_idx];
                    if opacity == 255 {
                        self.buffer[dst_idx] = pixel; // Fast path
                    } else {
                        let faded_pixel = apply_opacity(pixel, opacity);
                        self.buffer[dst_idx] = alpha_blend(faded_pixel, self.buffer[dst_idx]);
                    }
                }
            }
        }
    }
 pub fn print_str(&mut self, mut cx: usize, mut cy: usize, text: &str, color: u32, scale: usize) {
        let font_w = crate::font::CHAR_WIDTH * scale;
        let font_h = crate::font::CHAR_HEIGHT * scale;
        
        let start_x = cx; // Remember where the line started for carriage returns
        
        for c in text.chars() {
            if c == '\n' {
                cx = start_x; 
                cy += font_h;
            } else {
                self.draw_char(cx, cy, c, color, scale);
                cx += font_w;
            }
            
            // Basic line wrapping
            if cx + font_w >= self.width - 10 {
                cx = start_x;
                cy += font_h;
            }
        }
    }

    // Upgraded to use the Noto Sans Rasterizer!
    pub fn draw_char(&mut self, x: usize, y: usize, c: char, color: u32, scale: usize) {
        if let Some(raster) = crate::font::get_char_raster(c) {
            for (ri, row) in raster.raster().iter().enumerate() {
                for (ci, val) in row.iter().enumerate() {
                    // Noto Sans uses grayscale anti-aliasing (0-255). 
                    // We threshold it at > 50 to get a clean, sharp pixel!
                    if *val > 50 { 
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let px = x + (ci * scale) + dx;
                                let py = y + (ri * scale) + dy;
                                
                                // Strict bounds checking prevents memory crashes at screen edges
                                if px < self.width && py < self.height {
                                    let idx = py * self.width + px;
                                    self.buffer[idx] = color; 
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}