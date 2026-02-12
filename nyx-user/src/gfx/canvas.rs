// nyx-user/src/gfx/canvas.rs

use crate::syscalls;

#[derive(Clone, Copy)]
pub struct Color {
    pub hex: u32,
}

impl Color {
    pub const BLACK: u32 = 0x000000;
    pub const WHITE: u32 = 0xFFFFFF;
    pub const RED: u32   = 0xFF0000;
    pub const GREEN: u32 = 0x00FF00;
    pub const BLUE: u32  = 0x0000FF;
    pub const GRAY: u32  = 0x303030;
    pub const TITLE_BLUE: u32 = 0x007ACC;
}

// FIX: Removed 'draw_rect' relying on sys_draw because sys_draw doesn't exist anymore.

pub struct FastPainter {
    buffer: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
}

static mut PAINTER: Option<FastPainter> = None;

pub fn init() {
    // FIX: sys_get_screen_info returns (w, h, stride). BPP is standard 4 for u32.
    let (w, h, stride) = syscalls::sys_get_screen_info();
    let addr = syscalls::sys_map_framebuffer();
    
    if addr != 0 {
        unsafe {
            PAINTER = Some(FastPainter {
                buffer: addr as *mut u8,
                width: w as usize,
                height: h as usize,
                stride: stride as usize,
                bpp: 4, // FIX: Hardcoded to 4 bytes (32-bit color)
            });
        }
    }
}

pub fn fast_rect(x: usize, y: usize, w: usize, h: usize, color: u32) {
    unsafe {
        if let Some(p) = &mut PAINTER {
            // Extract RGB
            let r = ((color >> 16) & 0xFF) as u8;
            let g = ((color >> 8) & 0xFF) as u8;
            let b = (color & 0xFF) as u8;

            for row in 0..h {
                let screen_y = y + row;
                if screen_y >= p.height { break; }
                
                let row_offset = screen_y * p.stride;
                
                for col in 0..w {
                    let screen_x = x + col;
                    if screen_x >= p.width { break; }
                    
                    let offset = (row_offset + screen_x) * p.bpp; 
                    
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
        // FIX: Removed fallback to draw_rect since it doesn't exist
    }
}

// Helper for other files if they use it (Renamed param to _title to avoid warning)
pub fn draw_window(x: usize, y: usize, w: usize, h: usize, _title: &str) {
    fast_rect(x, y, w, h, Color::GRAY);
    fast_rect(x, y, w, 2, Color::WHITE);
    fast_rect(x, y, 2, h, Color::WHITE);
    fast_rect(x + w - 2, y, 2, h, 0x000000);
    fast_rect(x, y + h - 2, w, 2, 0x000000);
    fast_rect(x + 3, y + 3, w - 6, 20, Color::TITLE_BLUE);
    fast_rect(x + w - 22, y + 5, 16, 16, Color::RED);
}

pub fn draw_cursor(x: usize, y: usize) {
    fast_rect(x, y, 10, 10, Color::WHITE);
}