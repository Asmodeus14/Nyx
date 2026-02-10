use crate::syscalls;

// A simple color struct
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

// 1. Primitive: Draw Rectangle (Optimized loop of sys_draw)
pub fn draw_rect(x: usize, y: usize, w: usize, h: usize, color: u32) {
    for row in 0..h {
        for col in 0..w {
            syscalls::sys_draw(x + col, y + row, color);
        }
    }
}

// 2. Component: Draw a "Windows-95 style" Window
pub fn draw_window(x: usize, y: usize, w: usize, h: usize, title: &str) {
    // Main Body (Gray)
    draw_rect(x, y, w, h, Color::GRAY);
    
    // Border (White Highlight)
    draw_rect(x, y, w, 2, Color::WHITE);      // Top
    draw_rect(x, y, 2, h, Color::WHITE);      // Left
    
    // Border (Shadow)
    draw_rect(x + w - 2, y, 2, h, 0x000000);  // Right
    draw_rect(x, y + h - 2, w, 2, 0x000000);  // Bottom

    // Title Bar (Blue)
    draw_rect(x + 3, y + 3, w - 6, 20, Color::TITLE_BLUE);
    
    // Close Button (Red)
    draw_rect(x + w - 22, y + 5, 16, 16, Color::RED);
    
    // NOTE: We don't have font rendering in userspace yet (that requires a font file),
    // so we just draw colored blocks for now.
}

// 3. Component: Draw Cursor (Arrow)
pub fn draw_cursor(x: usize, y: usize) {
    let color = Color::WHITE;
    // Simple 10x10 cursor shape
    for i in 0..10 {
        syscalls::sys_draw(x + i, y + i, color);     // Diagonal
        syscalls::sys_draw(x, y + i, color);         // Vertical
        syscalls::sys_draw(x + i, y, color);         // Horizontal
    }
}