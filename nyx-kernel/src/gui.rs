use bootloader_api::info::{FrameBufferInfo, PixelFormat};
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
use alloc::vec::Vec;
use alloc::vec;

pub struct Rect {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
}
impl Rect {
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> Self { Self { x, y, w, h } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8, pub g: u8, pub b: u8,
}
impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255 };
    pub const BLUE: Color  = Color { r: 0, g: 0, b: 255 };
    pub const RED: Color   = Color { r: 255, g: 0, b: 0 };
    pub const GREEN: Color = Color { r: 0, g: 255, b: 0 };
    pub const CYAN: Color  = Color { r: 0, g: 255, b: 255 };
    pub const DARK_GRAY: Color = Color { r: 45, g: 45, b: 48 };
    pub const DARK_BLUE: Color = Color { r: 0, g: 122, b: 204 };

    pub fn new(r: u8, g: u8, b: u8) -> Self { Self { r, g, b } }
}

pub trait Painter {
    fn clear(&mut self, color: Color);
    fn draw_rect(&mut self, rect: Rect, color: Color);
    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color);
    fn draw_string(&mut self, x: usize, y: usize, s: &str, color: Color);
    fn width(&self) -> usize;
    fn height(&self) -> usize;
}

// --- HARDWARE PAINTER ---
pub struct VgaPainter<'a> {
    pub buffer: &'a mut [u8],
    pub info: FrameBufferInfo,
}

impl<'a> Painter for VgaPainter<'a> {
    fn width(&self) -> usize { self.info.width }
    fn height(&self) -> usize { self.info.height }

    fn clear(&mut self, color: Color) {
        if color == Color::BLACK {
            self.buffer.fill(0);
        } else {
            self.draw_rect(Rect::new(0, 0, self.width(), self.height()), color);
        }
    }

    fn draw_rect(&mut self, rect: Rect, color: Color) {
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        for y in rect.y..(rect.y + rect.h) {
            if y >= self.info.height { break; }
            let offset = y * stride + rect.x;
            let byte_offset = offset * bpp;
            if byte_offset >= self.buffer.len() { break; }

            for x in 0..rect.w {
                if rect.x + x >= self.info.width { break; }
                let idx = byte_offset + (x * bpp);
                // Safe opaque draw
                if idx + 2 < self.buffer.len() {
                    match self.info.pixel_format {
                        PixelFormat::Rgb => {
                            self.buffer[idx] = color.r;
                            self.buffer[idx+1] = color.g;
                            self.buffer[idx+2] = color.b;
                        },
                        PixelFormat::Bgr => {
                            self.buffer[idx] = color.b;
                            self.buffer[idx+1] = color.g;
                            self.buffer[idx+2] = color.r;
                        },
                        _ => {
                             self.buffer[idx] = color.r;
                             self.buffer[idx+1] = color.g;
                             self.buffer[idx+2] = color.b;
                        }
                    }
                }
            }
        }
    }

    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color) {
        // High-DPI Font (Size32)
        let char_raster = get_raster(c, FontWeight::Regular, RasterHeight::Size32)
            .unwrap_or_else(|| get_raster('?', FontWeight::Regular, RasterHeight::Size32).unwrap());
        
        for (row_i, row) in char_raster.raster().iter().enumerate() {
            for (col_i, val) in row.iter().enumerate() {
                if *val > 0 {
                    if x + col_i < self.width() && y + row_i < self.height() {
                        let offset = (y + row_i) * self.info.stride + (x + col_i);
                        let idx = offset * self.info.bytes_per_pixel;
                        if idx + 2 < self.buffer.len() {
                            match self.info.pixel_format {
                                PixelFormat::Rgb => {
                                    self.buffer[idx] = color.r;
                                    self.buffer[idx+1] = color.g;
                                    self.buffer[idx+2] = color.b;
                                },
                                PixelFormat::Bgr => {
                                    self.buffer[idx] = color.b;
                                    self.buffer[idx+1] = color.g;
                                    self.buffer[idx+2] = color.r;
                                },
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    fn draw_string(&mut self, x: usize, y: usize, s: &str, color: Color) {
        let mut curr_x = x;
        for c in s.chars() {
            self.draw_char(curr_x, y, c, color);
            curr_x += 16; // Advance cursor
        }
    }
}

// --- SOFTWARE BACKBUFFER (FAST) ---
pub struct BackBuffer {
    pub buffer: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl BackBuffer {
    pub fn new(info: FrameBufferInfo) -> Self {
        let size = info.width * info.height * 3; // RGB
        Self {
            buffer: vec![0; size],
            width: info.width,
            height: info.height,
        }
    }

    // FAST BLITTING
    pub fn present(&self, screen: &mut VgaPainter) {
        if self.width != screen.width() || self.height != screen.height() { return; }

        let bpp = screen.info.bytes_per_pixel;
        let stride = screen.info.stride;

        for y in 0..self.height {
            let src_start = (y * self.width) * 3;
            let src_end = src_start + (self.width * 3);
            let src_row = &self.buffer[src_start..src_end];

            let dest_offset = y * stride;
            let dest_start = dest_offset * bpp;
            let dest_slice = &mut screen.buffer;

            for (i, chunk) in src_row.chunks(3).enumerate() {
                let r = chunk[0];
                let g = chunk[1];
                let b = chunk[2];
                let idx = dest_start + (i * bpp);
                
                // Optimized write (Assuming BGR for UEFI)
                if idx + 2 < dest_slice.len() {
                    dest_slice[idx] = b;
                    dest_slice[idx+1] = g;
                    dest_slice[idx+2] = r;
                }
            }
        }
    }
}

impl Painter for BackBuffer {
    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }

    fn clear(&mut self, color: Color) {
        for chunk in self.buffer.chunks_exact_mut(3) {
            chunk[0] = color.r;
            chunk[1] = color.g;
            chunk[2] = color.b;
        }
    }

    fn draw_rect(&mut self, rect: Rect, color: Color) {
        for y in rect.y..(rect.y + rect.h) {
            if y >= self.height { break; }
            let row_start = (y * self.width) * 3;
            for x in rect.x..(rect.x + rect.w) {
                if x >= self.width { break; }
                let idx = row_start + (x * 3);
                self.buffer[idx] = color.r;
                self.buffer[idx+1] = color.g;
                self.buffer[idx+2] = color.b;
            }
        }
    }

    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color) {
        let char_raster = get_raster(c, FontWeight::Regular, RasterHeight::Size32)
            .unwrap_or_else(|| get_raster('?', FontWeight::Regular, RasterHeight::Size32).unwrap());
        
        for (row_i, row) in char_raster.raster().iter().enumerate() {
            for (col_i, val) in row.iter().enumerate() {
                if *val > 0 {
                    if x + col_i < self.width && y + row_i < self.height {
                        let idx = ((y + row_i) * self.width + (x + col_i)) * 3;
                        self.buffer[idx] = color.r;
                        self.buffer[idx+1] = color.g;
                        self.buffer[idx+2] = color.b;
                    }
                }
            }
        }
    }

    fn draw_string(&mut self, x: usize, y: usize, s: &str, color: Color) {
        let mut curr_x = x;
        for c in s.chars() {
            self.draw_char(curr_x, y, c, color);
            curr_x += 16;
        }
    }
}