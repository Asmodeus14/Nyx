use crate::effects::{alpha_blend, apply_opacity};

// Import x86_64 SIMD Intrinsics
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{__m128i, _mm_loadu_si128, _mm_set1_epi32, _mm_storeu_si128};

pub struct Color;
impl Color {
    pub const WARM_BG: u32       = 0xFF_F9F9F6; 
    pub const WARM_SURFACE: u32  = 0xFF_FFFFFF; 
    pub const WARM_BORDER: u32   = 0xFF_E2E2DB; 
    
    pub const TEXT_DARK: u32     = 0xFF_2D2D2A; 
    pub const TEXT_MUTED: u32    = 0xFF_8A8A85; 
    
    pub const ACCENT_PRIMARY: u32 = 0xFF_E67E22; 
    pub const ACCENT_HOVER: u32   = 0xFF_D35400; 
    pub const ACCENT_GREEN: u32   = 0xFF_27AE60; 
    
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

        // --- SIMD FAST PATH FOR OPAQUE COLORS ---
        #[cfg(target_arch = "x86_64")]
        if a == 255 {
            unsafe {
                // Broadcast our 32-bit color across all 4 slots of a 128-bit XMM register
                let color_chunk = _mm_set1_epi32(color as i32);

                for cy in 0..h {
                    let dst_y = y + cy;
                    if dst_y >= self.height { break; }
                    
                    let mut cx = 0;
                    let dst_row_start = dst_y * self.width + x;

                    // Fill 4 pixels (128 bits) per clock cycle
                    while cx + 4 <= w && (x + cx + 4) <= self.width {
                        let dst_ptr = self.buffer.as_mut_ptr().add(dst_row_start + cx) as *mut __m128i;
                        _mm_storeu_si128(dst_ptr, color_chunk);
                        cx += 4;
                    }

                    // Catch the remainder pixels that didn't fit in a 4-pixel chunk
                    while cx < w && (x + cx) < self.width {
                        self.buffer[dst_row_start + cx] = color;
                        cx += 1;
                    }
                }
            }
            return;
        }

        // --- STANDARD PATH FOR TRANSPARENT COLORS ---
        for cy in 0..h {
            for cx in 0..w {
                let idx = (y + cy) * self.width + (x + cx);
                if idx < self.buffer.len() { 
                    self.buffer[idx] = alpha_blend(color, self.buffer[idx]);
                }
            }
        }
    }

    pub fn composite_buffer(&mut self, x: usize, y: usize, src: &[u32], src_w: usize, src_h: usize, opacity: u8) {
        if opacity == 0 { return; }
        
        for cy in 0..src_h {
            let dst_y = y + cy;
            if dst_y >= self.height { break; }

            let mut cx = 0;
            let dst_row_start = dst_y * self.width + x;
            let src_row_start = cy * src_w;

            if opacity == 255 {
                // --- SIMD FAST PATH (SSE2 MEMCPY) ---
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    // Copy 4 pixels (128 bits) per clock cycle
                    while cx + 4 <= src_w && (x + cx + 4) <= self.width {
                        let src_ptr = src.as_ptr().add(src_row_start + cx) as *const __m128i;
                        let dst_ptr = self.buffer.as_mut_ptr().add(dst_row_start + cx) as *mut __m128i;

                        // Load 128-bits from source, write instantly to destination
                        let chunk = _mm_loadu_si128(src_ptr);
                        _mm_storeu_si128(dst_ptr, chunk);

                        cx += 4;
                    }
                }

                // Catch the remainder pixels
                while cx < src_w && (x + cx) < self.width {
                    self.buffer[dst_row_start + cx] = src[src_row_start + cx];
                    cx += 1;
                }
            } else {
                // --- STANDARD ALPHA BLENDING PATH ---
                // (SIMD alpha blending requires complex 8-bit to 16-bit unpacking/multiplication logic)
                while cx < src_w && (x + cx) < self.width {
                    let pixel = src[src_row_start + cx];
                    let faded_pixel = apply_opacity(pixel, opacity);
                    self.buffer[dst_row_start + cx] = alpha_blend(faded_pixel, self.buffer[dst_row_start + cx]);
                    cx += 1;
                }
            }
        }
    }

    pub fn print_str(&mut self, mut cx: usize, mut cy: usize, text: &str, color: u32, scale: usize) {
        let font_w = crate::font::CHAR_WIDTH * scale;
        let font_h = crate::font::CHAR_HEIGHT * scale;
        
        let start_x = cx; 
        
        for c in text.chars() {
            if c == '\n' {
                cx = start_x; 
                cy += font_h;
            } else {
                self.draw_char(cx, cy, c, color, scale);
                cx += font_w;
            }
            
            if cx + font_w >= self.width - 10 {
                cx = start_x;
                cy += font_h;
            }
        }
    }

    pub fn draw_char(&mut self, x: usize, y: usize, c: char, color: u32, scale: usize) {
        if let Some(raster) = crate::font::get_char_raster(c) {
            for (ri, row) in raster.raster().iter().enumerate() {
                for (ci, val) in row.iter().enumerate() {
                    if *val > 50 { 
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let px = x + (ci * scale) + dx;
                                let py = y + (ri * scale) + dy;
                                
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