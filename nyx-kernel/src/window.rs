use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use lazy_static::lazy_static;
use crate::gui::{Painter, Rect, Color, turbo_copy}; 
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

    pub fn blit_desktop_rect(&mut self, x: usize, y: usize, w: usize, h: usize, buffer: &[u32]) {
        if buffer.len() < w * h { return; }
        for row in 0..h {
            let screen_y = y + row;
            if screen_y >= self.screen_height { break; }
            let screen_row_start = screen_y * self.screen_width + x;
            let buffer_row_start = row * w;
            unsafe {
                let dest_ptr = self.desktop_buffer.as_mut_ptr().add(screen_row_start) as *mut u8;
                let src_ptr = buffer.as_ptr().add(buffer_row_start) as *const u8;
                turbo_copy(dest_ptr, src_ptr, w * 4);
            }
        }
    }

    pub fn console_print(&mut self, c: char) {
        // Kernel console logging (keep this if you want kernel logs)
        for win in self.windows.iter_mut().rev() {
            if win.window_type == WindowType::DebugLog {
                win.append_char(c);
                return;
            }
        }
    }

    pub fn update(&mut self, mouse: &MouseState) {
        // ... (Keep existing mouse update logic if you want kernel windows to work, 
        // OR empty this to fully disable kernel window management)
        
        // For now, let's keep it minimal so it doesn't crash
        let click_l = mouse.left_click && !self.prev_left;
        self.prev_left = mouse.left_click; self.prev_right = mouse.right_click;
        
        // Logic removed: Don't spawn new kernel windows on click
        // Use user space for that!
    }

    pub fn draw(&self, painter: &mut crate::gui::BackBuffer) {
        // --- CRITICAL FIX: STOP DRAWING KERNEL WALLPAPER ---
        // Comment out or remove the wallpaper drawing code.
        // If we draw here, we overwrite the User Space application.
        
        /* if self.desktop_buffer.len() == self.screen_width * self.screen_height {
            unsafe {
                turbo_copy(
                    painter.buffer.as_mut_ptr(),
                    self.desktop_buffer.as_ptr() as *const u8,
                    self.desktop_buffer.len() * 4
                );
            }
        } else {
            painter.clear(Color::new(0, 0, 30));
        }
        */

        // Only draw specific Kernel overlays if absolutely necessary (like Panic messages)
        // Otherwise, leave the screen alone for User Space.

        // Draw Debug/Kernel windows on TOP if they exist (optional)
        for (i, w) in self.windows.iter().enumerate() { 
             // Optional: w.draw(painter, i == self.windows.len()-1); 
        }
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
            // Only draw if we really need to (e.g. panic)
            // For now, we disable the kernel compositor to let User Space rule.
            
            // WINDOW_MANAGER.lock().draw(bb); <-- DISABLED
            
            // Note: If you want the Kernel Mouse Cursor to stay, keep this:
            /*
            let mouse = crate::mouse::MOUSE_STATE.lock();
            bb.draw_rect(Rect::new(mouse.x, mouse.y, 10, 10), Color::WHITE);
            if let Some(s) = &mut crate::SCREEN_PAINTER { bb.present(s); }
            */
            
            // But since User Space draws a mouse cursor too, we disable this 
            // to avoid "Double Cursor" effect.
        }
    }
}