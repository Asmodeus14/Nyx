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

    /// Draw `text` with the proportional TTF font. `cx,cy` is the TOP-LEFT of the first line (the pen
    /// baseline is derived internally via the font ascent, so existing top-left call sites keep working).
    /// Advances per-glyph by each glyph's own advance (proportional). Wraps near the right edge and on
    /// '\n'. `scale` rasterizes at `BASE_PX*scale` (sharp), not pixel-replicated.
    pub fn print_str(&mut self, mut cx: usize, mut cy: usize, text: &str, color: u32, scale: usize) {
        let line_h = crate::font::line_height(scale);
        let start_x = cx;
        for c in text.chars() {
            if c == '\n' {
                cx = start_x;
                cy += line_h;
                continue;
            }
            self.draw_char(cx, cy, c, color, scale);
            cx += crate::font::advance(c, scale);
            if cx + line_h >= self.width.saturating_sub(10) {
                cx = start_x;
                cy += line_h;
            }
        }
    }

    /// Pixel width of `text` at `scale` (sum of per-glyph advances up to the first newline — callers
    /// that center/right-align use this instead of the old `len()*CHAR_WIDTH`). Multi-line text returns
    /// the width of its widest line.
    pub fn text_width(text: &str, scale: usize) -> usize {
        let mut w = 0usize;
        let mut max = 0usize;
        for c in text.chars() {
            if c == '\n' {
                if w > max { max = w; }
                w = 0;
            } else {
                w += crate::font::advance(c, scale);
            }
        }
        if w > max { max = w; }
        max
    }

    /// Draw one glyph. `x,y` is the top-left of the line cell; the glyph is placed at its bearing
    /// relative to the baseline (`y + ascent`). Each texel's grayscale COVERAGE blends `color` over the
    /// existing pixel — real antialiasing (replaces the old binary `>50` write).
    pub fn draw_char(&mut self, x: usize, y: usize, c: char, color: u32, scale: usize) {
        let px = crate::font::px_for(scale);
        let baseline = y as i32 + crate::font::ascent(scale) as i32;
        let width = self.width;
        let height = self.height;
        crate::font::with_glyph(c, px, |g| {
            if g.w == 0 || g.h == 0 {
                return;
            }
            for row in 0..g.h {
                let py = baseline - g.bearing_y + row as i32;
                if py < 0 || py as usize >= height {
                    continue;
                }
                let row_base = py as usize * width;
                let cov_row = row * g.w;
                for col in 0..g.w {
                    let cov = g.cov[cov_row + col];
                    if cov == 0 {
                        continue;
                    }
                    let sx = x as i32 + g.bearing_x + col as i32;
                    if sx < 0 || sx as usize >= width {
                        continue;
                    }
                    let idx = row_base + sx as usize;
                    let blended = crate::effects::blend_color(color, self.buffer[idx], cov);
                    self.buffer[idx] = 0xFF00_0000 | blended;
                }
            }
        });
    }
}