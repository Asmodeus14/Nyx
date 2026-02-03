use bootloader_api::info::{FrameBufferInfo, PixelFormat};
use noto_sans_mono_bitmap::{get_raster, RasterizedChar, FontWeight, RasterHeight};
use alloc::vec::Vec;
use alloc::vec;

pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255 };
    pub const BLUE: Color  = Color { r: 0, g: 0, b: 255 };
    pub const RED: Color   = Color { r: 255, g: 0, b: 0 };
    pub const GREEN: Color = Color { r: 0, g: 255, b: 0 };
    pub const CYAN: Color  = Color { r: 0, g: 255, b: 255 };
    pub const GRAY: Color  = Color { r: 128, g: 128, b: 128 };

    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

pub trait Painter {
    fn clear(&mut self, color: Color);
    fn draw_rect(&mut self, rect: Rect, color: Color);
    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color);
    fn draw_string(&mut self, x: usize, y: usize, s: &str, color: Color);
    fn width(&self) -> usize;
    fn height(&self) -> usize;

    fn draw_string_centered(&mut self, y: usize, s: &str, color: Color) {
        let text_width = s.len() * 8;
        let x = (self.width() - text_width) / 2;
        self.draw_string(x, y, s, color);
    }

    // NEW: Draw a simple cursor
    fn draw_cursor(&mut self, x: usize, y: usize) {
        let cursor_pixels = [
            (0,0), (0,1), (0,2), (0,3), (0,4), (0,5),
            (1,1), (1,2), (2,2), (2,3), (3,3)
        ];
        
        for (dx, dy) in cursor_pixels.iter() {
            self.draw_rect(Rect::new(x + dx, y + dy, 2, 2), Color::RED);
        }
    }
}

// --- HARDWARE PAINTER (VGA) ---
pub struct VgaPainter<'a> {
    pub buffer: &'a mut [u8],
    pub info: FrameBufferInfo,
}

impl<'a> VgaPainter<'a> {
    fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.info.width || y >= self.info.height { return; }
        let offset = y * self.info.stride + x;
        let pixel_offset = offset * self.info.bytes_per_pixel;
        if pixel_offset + 2 >= self.buffer.len() { return; }

        let (r, g, b) = match self.info.pixel_format {
            PixelFormat::Rgb => (color.r, color.g, color.b),
            PixelFormat::Bgr => (color.b, color.g, color.r),
            PixelFormat::U8 => (color.g, color.g, color.g),
            _ => (color.r, color.g, color.b),
        };

        self.buffer[pixel_offset] = r;
        self.buffer[pixel_offset + 1] = g;
        self.buffer[pixel_offset + 2] = b;
    }
}

impl<'a> Painter for VgaPainter<'a> {
    fn width(&self) -> usize { self.info.width }
    fn height(&self) -> usize { self.info.height }

    fn clear(&mut self, color: Color) {
        self.draw_rect(Rect::new(0, 0, self.info.width, self.info.height), color);
    }

    fn draw_rect(&mut self, rect: Rect, color: Color) {
        for y in rect.y..(rect.y + rect.h) {
            for x in rect.x..(rect.x + rect.w) {
                self.draw_pixel(x, y, color);
            }
        }
    }

    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color) {
        let char_raster = get_raster(c, FontWeight::Regular, RasterHeight::Size16).unwrap_or_else(|| get_raster('?', FontWeight::Regular, RasterHeight::Size16).unwrap());
        for (row_i, row) in char_raster.raster().iter().enumerate() {
            for (col_i, pixel_intensity) in row.iter().enumerate() {
                if *pixel_intensity > 0 {
                    self.draw_pixel(x + col_i, y + row_i, color);
                }
            }
        }
    }

    fn draw_string(&mut self, x: usize, y: usize, s: &str, color: Color) {
        let mut curr_x = x;
        for c in s.chars() {
            self.draw_char(curr_x, y, c, color);
            curr_x += 8;
        }
    }
}

// --- SOFTWARE BACKBUFFER (RAM) ---
pub struct BackBuffer {
    pub buffer: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub bytes_per_pixel: usize,
    pub stride: usize,
    pub pixel_format: PixelFormat,
}

impl BackBuffer {
    pub fn new(info: FrameBufferInfo) -> Self {
        let size = info.stride * info.height * info.bytes_per_pixel;
        let buffer = vec![0; size]; 
        
        Self {
            buffer,
            width: info.width,
            height: info.height,
            bytes_per_pixel: info.bytes_per_pixel,
            stride: info.stride,
            pixel_format: info.pixel_format,
        }
    }

    pub fn present(&self, screen: &mut VgaPainter) {
        let len = self.buffer.len();
        if len <= screen.buffer.len() {
            screen.buffer[..len].copy_from_slice(&self.buffer);
        }
    }
}

impl Painter for BackBuffer {
    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }

    fn clear(&mut self, color: Color) {
        self.draw_rect(Rect::new(0, 0, self.width, self.height), color);
    }

    fn draw_rect(&mut self, rect: Rect, color: Color) {
        for y in rect.y..(rect.y + rect.h) {
            for x in rect.x..(rect.x + rect.w) {
                if x >= self.width || y >= self.height { continue; }
                
                let offset = y * self.stride + x;
                let pixel_offset = offset * self.bytes_per_pixel;
                
                if pixel_offset + 2 >= self.buffer.len() { continue; }

                let (r, g, b) = match self.pixel_format {
                    PixelFormat::Rgb => (color.r, color.g, color.b),
                    PixelFormat::Bgr => (color.b, color.g, color.r),
                    _ => (color.r, color.g, color.b),
                };

                self.buffer[pixel_offset] = r;
                self.buffer[pixel_offset + 1] = g;
                self.buffer[pixel_offset + 2] = b;
            }
        }
    }

    fn draw_char(&mut self, x: usize, y: usize, c: char, color: Color) {
        let char_raster = get_raster(c, FontWeight::Regular, RasterHeight::Size16).unwrap_or_else(|| get_raster('?', FontWeight::Regular, RasterHeight::Size16).unwrap());

        for (row_i, row) in char_raster.raster().iter().enumerate() {
            for (col_i, pixel_intensity) in row.iter().enumerate() {
                if *pixel_intensity > 0 {
                    let final_color = Color::new(
                        (color.r as u16 * *pixel_intensity as u16 / 255) as u8,
                        (color.g as u16 * *pixel_intensity as u16 / 255) as u8,
                        (color.b as u16 * *pixel_intensity as u16 / 255) as u8,
                    );
                    
                    if x + col_i < self.width && y + row_i < self.height {
                         let offset = (y + row_i) * self.stride + (x + col_i);
                         let pixel_offset = offset * self.bytes_per_pixel;
                         
                         if pixel_offset + 2 < self.buffer.len() {
                            let (r, g, b) = match self.pixel_format {
                                PixelFormat::Rgb => (final_color.r, final_color.g, final_color.b),
                                PixelFormat::Bgr => (final_color.b, final_color.g, final_color.r),
                                _ => (final_color.r, final_color.g, final_color.b),
                            };
                            self.buffer[pixel_offset] = r;
                            self.buffer[pixel_offset + 1] = g;
                            self.buffer[pixel_offset + 2] = b;
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
            curr_x += 8;
        }
    }
}