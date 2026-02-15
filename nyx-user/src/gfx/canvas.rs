// nyx-user/src/gfx/canvas.rs

use crate::syscalls;

pub struct Color { pub hex: u32 }
impl Color {
    pub const BLACK: u32 = 0x000000;
    pub const WHITE: u32 = 0xFFFFFF;
    pub const RED: u32   = 0xFF0000;
    pub const GREEN: u32 = 0x00FF00;
    pub const BLUE: u32  = 0x0000FF;
    pub const GRAY: u32  = 0x303030;
    pub const TITLE_BLUE: u32 = 0x007ACC;
}

pub struct FastPainter {
    buffer: *mut u8,
    width: usize,
    height: usize,
    stride: usize, // In PIXELS
    bpp: usize,    // Bytes per pixel (3 or 4)
}

static mut PAINTER: Option<FastPainter> = None;

pub fn init() {
    let (w, h, stride) = syscalls::sys_get_screen_info();
    let addr = syscalls::sys_map_framebuffer();
    
    if addr != 0 {
        unsafe {
            // Assume 4 bytes (32-bit) unless we add a syscall to get BPP.
            // If distortion persists, try changing this to 3.
            PAINTER = Some(FastPainter {
                buffer: addr as *mut u8,
                width: w as usize,
                height: h as usize,
                stride: stride as usize,
                bpp: 4, 
            });
        }
    }
}

pub fn fast_rect(x: usize, y: usize, w: usize, h: usize, color: u32) {
    unsafe {
        if let Some(p) = &mut PAINTER {
            let r = ((color >> 16) & 0xFF) as u8;
            let g = ((color >> 8) & 0xFF) as u8;
            let b = (color & 0xFF) as u8;

            for row in 0..h {
                let screen_y = y + row;
                if screen_y >= p.height { break; }
                
                // Stride is in pixels, so row_start_byte = y * stride * bpp
                let row_start = screen_y * p.stride * p.bpp;
                
                for col in 0..w {
                    let screen_x = x + col;
                    if screen_x >= p.width { break; }
                    
                    let offset = row_start + (screen_x * p.bpp); 
                    
                    if p.bpp == 4 {
                        *(p.buffer.add(offset) as *mut u32) = color;
                    } else if p.bpp == 3 {
                        *p.buffer.add(offset) = b;
                        *p.buffer.add(offset+1) = g;
                        *p.buffer.add(offset+2) = r;
                    }
                }
            }
        }
    }
}

pub fn draw_cursor(x: usize, y: usize) { fast_rect(x, y, 10, 10, Color::WHITE); }