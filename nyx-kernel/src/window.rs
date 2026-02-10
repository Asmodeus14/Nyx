use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use lazy_static::lazy_static;
use crate::gui::{Painter, Rect, Color};
use crate::mouse::MouseState;
use core::fmt::Write; 

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
    // NEW: Persistent Desktop Buffer (The Wallpaper/Canvas)
    pub desktop_buffer: Vec<u32>, 
}

impl WindowManager {
    pub fn new() -> Self { 
        let mut wm = Self { 
            windows: Vec::new(), prev_left: false, prev_right: false, 
            screen_width: 1024, screen_height: 768,
            desktop_buffer: Vec::new(), 
        };
        wm.add(Window::new(50, 50, 900, 600, "Nyx Terminal", WindowType::Terminal));
        wm
    }

    pub fn set_resolution(&mut self, w: usize, h: usize) { 
        self.screen_width = w; 
        self.screen_height = h; 
        // Init buffer with dark blue background
        self.desktop_buffer.resize(w * h, 0x00000030); 
    }

    pub fn add(&mut self, window: Window) { self.windows.push(window); }
    
    // API: Draw single pixel (Old slow way)
    pub fn put_desktop_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.screen_width && y < self.screen_height {
            let idx = y * self.screen_width + x;
            if idx < self.desktop_buffer.len() {
                self.desktop_buffer[idx] = color;
            }
        }
    }

    // NEW API: Blit Buffer (Fast way)
    pub fn blit_desktop_rect(&mut self, x: usize, y: usize, w: usize, h: usize, buffer: &[u32]) {
        if buffer.len() < w * h { return; }
        
        for row in 0..h {
            let screen_y = y + row;
            if screen_y >= self.screen_height { break; }
            
            let screen_row_start = screen_y * self.screen_width + x;
            let buffer_row_start = row * w;

            for col in 0..w {
                if x + col >= self.screen_width { break; }
                
                let color = buffer[buffer_row_start + col];
                // Simple Alpha Blending: Skip 0x00000000 (Transparent)
                if color != 0 {
                    self.desktop_buffer[screen_row_start + col] = color;
                }
            }
        }
    }

    pub fn console_print(&mut self, c: char) {
        for win in self.windows.iter_mut().rev() {
            if win.window_type == WindowType::Terminal {
                win.append_char(c);
                return;
            }
        }
    }

    pub fn update(&mut self, mouse: &MouseState) {
        let click_l = mouse.left_click && !self.prev_left;
        let click_r = mouse.right_click && !self.prev_right;
        self.prev_left = mouse.left_click; self.prev_right = mouse.right_click;
        
        let tb_y = self.screen_height - TASKBAR_HEIGHT - SAFE_PADDING;
        if click_l && mouse.y >= tb_y && mouse.x < 100 {
            let off = (self.windows.len() * 30) % 200;
            self.add(Window::new(50+off, 50+off, 900, 600, "Terminal", WindowType::Terminal));
            return;
        }

        if click_r {
             self.add(Window::new(mouse.x, mouse.y, 500, 400, "System Monitor", WindowType::SystemMonitor));
        }

        for win in self.windows.iter_mut() {
            if win.is_dragging {
                if mouse.left_click {
                    if mouse.x >= win.drag_offset_x { win.x = mouse.x - win.drag_offset_x; }
                    if mouse.y >= win.drag_offset_y { win.y = mouse.y - win.drag_offset_y; }
                } else { win.is_dragging = false; }
                return;
            }
        }

        if click_l {
            let mut hit_idx = None;
            let mut close = false;
            for (i, win) in self.windows.iter_mut().enumerate().rev() {
                if win.is_close_hit(mouse.x, mouse.y) { hit_idx = Some(i); close = true; break; }
                else if win.is_header_hit(mouse.x, mouse.y) {
                    hit_idx = Some(i); win.is_dragging = true;
                    win.drag_offset_x = mouse.x.saturating_sub(win.x);
                    win.drag_offset_y = mouse.y.saturating_sub(win.y);
                    break;
                } else if win.is_body_hit(mouse.x, mouse.y) { hit_idx = Some(i); break; }
            }
            if let Some(i) = hit_idx {
                let w = self.windows.remove(i);
                if !close { self.windows.push(w); }
            }
        }
    }

    pub fn draw(&self, painter: &mut crate::gui::BackBuffer) {
        // 1. Draw Desktop Wallpaper (The User Canvas)
        if self.desktop_buffer.len() == self.screen_width * self.screen_height {
            for y in 0..self.screen_height {
                for x in 0..self.screen_width {
                    let color_u32 = self.desktop_buffer[y * self.screen_width + x];
                    let r = ((color_u32 >> 16) & 0xFF) as u8;
                    let g = ((color_u32 >> 8) & 0xFF) as u8;
                    let b = (color_u32 & 0xFF) as u8;
                    painter.put_pixel_safe(x, y, Color::new(r, g, b));
                }
            }
        } else {
            painter.clear(Color::new(0, 0, 30));
        }

        // 2. Draw Kernel UI elements on top
        let tb_y = self.screen_height - TASKBAR_HEIGHT - SAFE_PADDING;
        painter.draw_rect(Rect::new(0, tb_y, self.screen_width, TASKBAR_HEIGHT), Color::new(30, 30, 30));
        painter.draw_rect(Rect::new(4, tb_y+4, 92, TASKBAR_HEIGHT-8), Color::new(0, 120, 215));
        painter.draw_string(28, tb_y+12, "START", Color::WHITE);

        let uptime = crate::time::uptime_seconds() as u64;
        let hours = uptime / 3600;
        let mins = (uptime % 3600) / 60;
        let secs = uptime % 60;

        struct TimeBuf { buf: [u8; 32], len: usize }
        impl Write for TimeBuf {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for byte in s.bytes() {
                    if self.len < self.buf.len() { self.buf[self.len] = byte; self.len += 1; }
                }
                Ok(())
            }
        }
        let mut time_buf = TimeBuf { buf: [0; 32], len: 0 };
        let _ = write!(time_buf, "{:02}:{:02}:{:02}", hours, mins, secs);
        let time_str = core::str::from_utf8(&time_buf.buf[..time_buf.len]).unwrap_or("00:00:00");

        let clk_x = self.screen_width - 150 - SAFE_PADDING;
        painter.draw_string(clk_x, tb_y+12, time_str, Color::GREEN);

        for (i, w) in self.windows.iter().enumerate() { w.draw(painter, i == self.windows.len()-1); }
    }
}

// Helper for raw buffer manipulation
impl crate::gui::BackBuffer {
    pub fn put_pixel_safe(&mut self, x: usize, y: usize, color: Color) {
        let bpp = self.info.bytes_per_pixel;
        let idx = (y * self.info.stride + x) * bpp;
        if idx + 2 < self.buffer.len() {
            self.buffer[idx] = color.b;   
            self.buffer[idx+1] = color.g; 
            self.buffer[idx+2] = color.r; 
        }
    }
}

pub fn compositor_paint() {
    unsafe {
        if let Some(bb) = &mut crate::BACK_BUFFER {
            // Note: Background clear is now handled inside WINDOW_MANAGER.draw
            WINDOW_MANAGER.lock().draw(bb);
            let mouse = crate::mouse::MOUSE_STATE.lock();
            bb.draw_rect(Rect::new(mouse.x, mouse.y, 10, 10), Color::WHITE);
            bb.draw_rect(Rect::new(mouse.x+1, mouse.y+1, 8, 8), Color::RED);
            if let Some(s) = &mut crate::SCREEN_PAINTER { bb.present(s); }
        }
    }
}