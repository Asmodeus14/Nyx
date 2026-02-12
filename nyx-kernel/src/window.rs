use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use lazy_static::lazy_static;
use crate::gui::{Painter, Rect, Color, turbo_copy}; 
use crate::mouse::MouseState;
use core::fmt::Write; 
use bootloader_api::info::PixelFormat; // Required for the green-shift fix

lazy_static! {
    pub static ref WINDOW_MANAGER: Mutex<WindowManager> = Mutex::new(WindowManager::new());
}

const TASKBAR_HEIGHT: usize = 40;
const TITLE_BAR_HEIGHT: usize = 28;
const SAFE_PADDING: usize = 40;
const LINE_HEIGHT: usize = 36; 
const CHAR_WIDTH: usize = 16; 

#[derive(Clone, PartialEq)]
pub enum WindowType { Terminal, SystemMonitor, DebugLog }

pub struct Window {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub title: String,
    pub window_type: WindowType,
    pub is_dragging: bool,
    pub drag_offset_x: usize, pub drag_offset_y: usize,
    pub content_color: Color,
    pub buffer: Vec<String>,
}

impl Window {
    pub fn new(x: usize, y: usize, w: usize, h: usize, title: &str, w_type: WindowType) -> Self {
        let color = match w_type {
            WindowType::Terminal => Color::new(15, 15, 15),
            WindowType::SystemMonitor => Color::new(0, 0, 40),
            WindowType::DebugLog => Color::new(10, 10, 10),
        };
        Self {
            x, y, w, h, title: String::from(title), window_type: w_type,
            is_dragging: false, drag_offset_x: 0, drag_offset_y: 0,
            content_color: color, buffer: Vec::new(),
        }
    }

    pub fn append_char(&mut self, c: char) {
        if self.buffer.is_empty() { self.buffer.push(String::new()); }
        
        let max_chars = (self.w.saturating_sub(16)) / CHAR_WIDTH;
        let available_height = self.h.saturating_sub(TITLE_BAR_HEIGHT + 10);
        let max_lines = available_height / LINE_HEIGHT;

        match c {
            '\n' => self.buffer.push(String::new()),
            '\x08' => { if let Some(line) = self.buffer.last_mut() { line.pop(); } },
            _ => { 
                if let Some(line) = self.buffer.last_mut() {
                    if line.len() >= max_chars {
                        self.buffer.push(String::from(c));
                    } else {
                        line.push(c); 
                    }
                } 
            }
        }
        while self.buffer.len() > max_lines { self.buffer.remove(0); }
    }

    pub fn draw(&self, painter: &mut impl Painter, is_active: bool) {
        painter.draw_rect(Rect::new(self.x + 6, self.y + 6, self.w, self.h), Color::new(5, 5, 5));

        let border_color = if is_active { Color::new(200, 200, 200) } else { Color::new(60, 60, 60) };
        painter.draw_rect(Rect::new(self.x - 2, self.y - 2, self.w + 4, self.h + 4), border_color);
        painter.draw_rect(Rect::new(self.x, self.y, self.w, self.h), self.content_color);

        let header_color = if is_active { 
            match self.window_type {
                WindowType::Terminal => Color::new(0, 122, 204),
                WindowType::SystemMonitor => Color::new(0, 150, 136),
                WindowType::DebugLog => Color::new(100, 50, 150),
            }
        } else { Color::new(45, 45, 48) };

        painter.draw_rect(Rect::new(self.x, self.y, self.w, TITLE_BAR_HEIGHT), header_color);
        painter.draw_string(self.x + 8, self.y + 6, &self.title, Color::WHITE);

        painter.draw_rect(Rect::new(self.x + self.w - 24, self.y + 4, 20, 20), Color::new(200, 60, 60));
        painter.draw_string(self.x + self.w - 17, self.y + 4, "X", Color::WHITE);

        let start_y = self.y + TITLE_BAR_HEIGHT + 4;
        let available_height = self.h.saturating_sub(TITLE_BAR_HEIGHT + 10);
        let max_draw_lines = available_height / LINE_HEIGHT;

        for (i, line) in self.buffer.iter().enumerate() {
            if i >= max_draw_lines { break; }
            painter.draw_string(self.x + 8, start_y + (i * LINE_HEIGHT), line, Color::WHITE);
        }
    }

    pub fn is_close_hit(&self, mx: usize, my: usize) -> bool {
        mx >= self.x + self.w - 24 && mx <= self.x + self.w - 4 && my >= self.y + 4 && my <= self.y + 24
    }
    pub fn is_header_hit(&self, mx: usize, my: usize) -> bool {
        mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + TITLE_BAR_HEIGHT
    }
    pub fn is_body_hit(&self, mx: usize, my: usize) -> bool {
        mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h
    }
}

pub struct WindowManager {
    windows: Vec<Window>,
    prev_left: bool, prev_right: bool,
    pub screen_width: usize, pub screen_height: usize,
    pub desktop_buffer: Vec<u32>, 
}

impl WindowManager {
    pub fn new() -> Self { 
        Self { 
            windows: Vec::new(), prev_left: false, prev_right: false, 
            screen_width: 1024, screen_height: 768,
            desktop_buffer: Vec::new(), 
        }
    }

    pub fn set_resolution(&mut self, w: usize, h: usize) { 
        self.screen_width = w; 
        self.screen_height = h; 
        self.desktop_buffer.resize(w * h, 0x00000030); 
    }

    pub fn add(&mut self, window: Window) { self.windows.push(window); }
    
    pub fn put_desktop_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.screen_width && y < self.screen_height {
            let idx = y * self.screen_width + x;
            if idx < self.desktop_buffer.len() {
                self.desktop_buffer[idx] = color;
            }
        }
    }
    
    // --- THIS IS THE MISSING METHOD CAUSING YOUR ERROR ---
    pub fn console_print(&mut self, c: char) {
        for win in self.windows.iter_mut().rev() {
            if win.window_type == WindowType::DebugLog {
                win.append_char(c);
                return;
            }
        }
    }

    pub fn update(&mut self, mouse: &MouseState) {
        let click_l = mouse.left_click && !self.prev_left;
        self.prev_left = mouse.left_click; self.prev_right = mouse.right_click;
    }

    // Handles both Stride AND Pixel Format (3 vs 4 bytes)
    pub fn draw(&self, painter: &mut crate::gui::BackBuffer) {
        // Only draw desktop if buffer is ready
        if self.desktop_buffer.len() == self.screen_width * self.screen_height {
            let stride = painter.info.stride;
            let width = self.screen_width;
            let height = self.screen_height;
            let bpp = painter.info.bytes_per_pixel;
            let format = painter.info.pixel_format;

            match bpp {
                4 => {
                    // Optimized path for 32-bit (4 byte) color
                    for y in 0..height {
                        let src_idx = y * width;
                        let dest_offset = (y * stride) * 4;
                        
                        // Bounds check
                        if src_idx < self.desktop_buffer.len() && dest_offset < painter.buffer.len() {
                            unsafe {
                                let src_ptr = self.desktop_buffer.as_ptr().add(src_idx) as *const u8;
                                let dest_ptr = painter.buffer.as_mut_ptr().add(dest_offset);
                                // We copy exactly 'width' pixels
                                turbo_copy(dest_ptr, src_ptr, width * 4);
                            }
                        }
                    }
                },
                3 => {
                    // Slow path for 24-bit (3 byte) color
                    // We must manually convert u32 (0xRRGGBB) -> 3 bytes
                    for y in 0..height {
                        let src_start = y * width;
                        let dest_start = (y * stride) * 3;
                        
                        for x in 0..width {
                            let color = self.desktop_buffer[src_start + x];
                            let dest_idx = dest_start + (x * 3);
                            
                            if dest_idx + 2 < painter.buffer.len() {
                                // Extract RGB
                                let r = ((color >> 16) & 0xFF) as u8;
                                let g = ((color >> 8) & 0xFF) as u8;
                                let b = (color & 0xFF) as u8;

                                match format {
                                    PixelFormat::Rgb => {
                                        painter.buffer[dest_idx] = r;
                                        painter.buffer[dest_idx+1] = g;
                                        painter.buffer[dest_idx+2] = b;
                                    },
                                    PixelFormat::Bgr | _ => {
                                        painter.buffer[dest_idx] = b;
                                        painter.buffer[dest_idx+1] = g;
                                        painter.buffer[dest_idx+2] = r;
                                    }
                                }
                            }
                        }
                    }
                },
                _ => {} // Not supported
            }
        } else {
            painter.clear(Color::new(0, 0, 30));
        }

        // Draw Kernel Windows
        for (i, w) in self.windows.iter().enumerate() { 
             w.draw(painter, i == self.windows.len()-1); 
        }
    }
}