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
    // ADDED YELLOW HERE:
    pub const YELLOW: Color = Color { r: 255, g: 255, b: 0 }; 
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

// --- HARDWARE PAINTER (Direct Video Memory) ---
pub struct VgaPainter<'a> {
    pub buffer: &'a mut [u8],
    pub info: FrameBufferInfo,
}

impl<'a> Painter for VgaPainter<'a> {
    fn width(&self) -> usize { self.info.width }
    fn height(&self) -> usize { self.info.height }

    fn clear(&mut self, color: Color) {
        self.draw_rect(Rect::new(0, 0, self.width(), self.height()), color);
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
                        PixelFormat::U8 => {
                            self.buffer[idx] = color.g;
                        }
                        _ => {
                             self.buffer[idx] = color.b;
                             self.buffer[idx+1] = color.g;
                             self.buffer[idx+2] = color.r;
                        }
                    }
                }
            }
        }
    }

    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color) {
        let char_raster = get_raster(c, FontWeight::Regular, RasterHeight::Size32)
            .unwrap_or_else(|| get_raster('?', FontWeight::Regular, RasterHeight::Size32).unwrap());
        
        for (row_i, row) in char_raster.raster().iter().enumerate() {
            for (col_i, val) in row.iter().enumerate() {
                if *val > 0 {
                    let px = x + col_i;
                    let py = y + row_i;
                    if px < self.width() && py < self.height() {
                        let bpp = self.info.bytes_per_pixel;
                        let idx = (py * self.info.stride + px) * bpp;
                        
                        if idx + 2 < self.buffer.len() {
                             match self.info.pixel_format {
                                PixelFormat::Rgb => {
                                    self.buffer[idx] = color.r;
                                    self.buffer[idx+1] = color.g;
                                    self.buffer[idx+2] = color.b;
                                },
                                PixelFormat::Bgr | _ => {
                                    self.buffer[idx] = color.b;
                                    self.buffer[idx+1] = color.g;
                                    self.buffer[idx+2] = color.r;
                                }
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
            curr_x += 16; 
        }
    }
}

// --- SOFTWARE BACKBUFFER (OPTIMIZED) ---
pub struct BackBuffer {
    pub buffer: Vec<u8>,
    pub info: FrameBufferInfo,
}

impl BackBuffer {
    pub fn new(info: FrameBufferInfo) -> Self {
        let size = info.stride * info.height * info.bytes_per_pixel;
        Self {
            buffer: vec![0; size],
            info,
        }
    }

    pub fn present(&self, screen: &mut VgaPainter) {
        if self.buffer.len() == screen.buffer.len() {
            screen.buffer.copy_from_slice(&self.buffer);
        } else {
            let len = self.buffer.len().min(screen.buffer.len());
            screen.buffer[..len].copy_from_slice(&self.buffer[..len]);
        }
    }

    #[inline(always)]
    fn put_pixel(&mut self, idx: usize, color: Color) {
        if idx + 2 < self.buffer.len() {
            match self.info.pixel_format {
                PixelFormat::Rgb => {
                    self.buffer[idx] = color.r;
                    self.buffer[idx+1] = color.g;
                    self.buffer[idx+2] = color.b;
                },
                PixelFormat::Bgr | _ => {
                    self.buffer[idx] = color.b;
                    self.buffer[idx+1] = color.g;
                    self.buffer[idx+2] = color.r;
                }
            }
        }
    }
}

impl Painter for BackBuffer {
    fn width(&self) -> usize { self.info.width }
    fn height(&self) -> usize { self.info.height }

    fn clear(&mut self, color: Color) {
        if color == Color::BLACK {
            self.buffer.fill(0);
            return;
        }
        self.draw_rect(Rect::new(0, 0, self.width(), self.height()), color);
    }

    fn draw_rect(&mut self, rect: Rect, color: Color) {
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        
        for y in rect.y..(rect.y + rect.h) {
            if y >= self.height() { break; }
            let offset = y * stride + rect.x;
            let mut idx = offset * bpp;

            for x in 0..rect.w {
                if rect.x + x >= self.width() { break; }
                self.put_pixel(idx, color);
                idx += bpp;
            }
        }
    }

    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color) {
        let char_raster = get_raster(c, FontWeight::Regular, RasterHeight::Size32)
            .unwrap_or_else(|| get_raster('?', FontWeight::Regular, RasterHeight::Size32).unwrap());
        
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;

        for (row_i, row) in char_raster.raster().iter().enumerate() {
            for (col_i, val) in row.iter().enumerate() {
                if *val > 0 {
                    let px = x + col_i;
                    let py = y + row_i;
                    if px < self.width() && py < self.height() {
                        let idx = (py * stride + px) * bpp;
                        self.put_pixel(idx, color);
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