use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use lazy_static::lazy_static;
use crate::gui::{Painter, Rect, Color};
use crate::mouse::MouseState;
use core::sync::atomic::Ordering;

lazy_static! {
    pub static ref WINDOW_MANAGER: Mutex<WindowManager> = Mutex::new(WindowManager::new());
}

const TASKBAR_HEIGHT: usize = 40;
const TITLE_BAR_HEIGHT: usize = 28;
const SAFE_PADDING: usize = 40;
const CHAR_WIDTH: usize = 8;
const LINE_HEIGHT: usize = 18;

#[derive(Clone, PartialEq)]
// NEW: Added DebugLog type
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
            // DebugLog gets a nice dark background
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
        
        let max_chars = (self.w - 16) / CHAR_WIDTH;
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

    pub fn set_content(&mut self, lines: Vec<String>) { self.buffer = lines; }

    pub fn draw(&self, painter: &mut impl Painter, is_active: bool) {
        painter.draw_rect(Rect::new(self.x + 6, self.y + 6, self.w, self.h), Color::new(5, 5, 5));

        let border_color = if is_active { Color::new(200, 200, 200) } else { Color::new(60, 60, 60) };
        painter.draw_rect(Rect::new(self.x - 2, self.y - 2, self.w + 4, self.h + 4), border_color);

        painter.draw_rect(Rect::new(self.x, self.y, self.w, self.h), self.content_color);

        let header_color = if is_active { 
            match self.window_type {
                WindowType::Terminal => Color::new(0, 122, 204),
                WindowType::SystemMonitor => Color::new(0, 150, 136),
                WindowType::DebugLog => Color::new(100, 50, 150), // Purple Header for Debug
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
    last_update_tick: u64,
}

impl WindowManager {
    pub fn new() -> Self { 
        let mut wm = Self { windows: Vec::new(), prev_left: false, prev_right: false, 
                            screen_width: 800, screen_height: 600, last_update_tick: 0 };
        wm.add(Window::new(100, 100, 500, 300, "Nyx Terminal", WindowType::Terminal));
        wm
    }

    pub fn set_resolution(&mut self, w: usize, h: usize) { self.screen_width = w; self.screen_height = h; }
    pub fn add(&mut self, window: Window) { self.windows.push(window); }
    
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
        
        let ticks = crate::time::get_ticks();
        if ticks > self.last_update_tick + 5 {
            self.last_update_tick = ticks;
            let uptime = crate::time::uptime_seconds();
            let mut info = Vec::new();
            info.push(format!("-- SYSTEM VITALS --"));
            info.push(format!("Uptime: {:.1}s", uptime));
            info.push(format!("Windows: {}", self.windows.len()));
            info.push(format!("Mouse: {}, {}", mouse.x, mouse.y));
            
            for win in self.windows.iter_mut() {
                // FIX: Only update actual System Monitors! Don't touch DebugLog!
                if win.window_type == WindowType::SystemMonitor { 
                    win.set_content(info.clone()); 
                }
            }
        }

        let tb_y = self.screen_height - TASKBAR_HEIGHT - SAFE_PADDING;
        if click_l && mouse.y >= tb_y && mouse.x < 100 {
            let off = (self.windows.len() * 30) % 200;
            self.add(Window::new(150+off, 100+off, 500, 300, "Terminal", WindowType::Terminal));
            return;
        }

        if click_r {
             self.add(Window::new(mouse.x, mouse.y, 300, 200, "System Monitor", WindowType::SystemMonitor));
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

    pub fn draw(&self, painter: &mut impl Painter) {
        let tb_y = self.screen_height - TASKBAR_HEIGHT - SAFE_PADDING;
        painter.draw_rect(Rect::new(0, tb_y, self.screen_width, TASKBAR_HEIGHT), Color::new(30, 30, 30));
        painter.draw_rect(Rect::new(4, tb_y+4, 92, TASKBAR_HEIGHT-8), Color::new(0, 120, 215));
        painter.draw_string(28, tb_y+12, "START", Color::WHITE);

        let uptime = crate::time::uptime_seconds() as u64;
        let time_str = format!("{:02}:{:02}:{:02}", uptime/3600, (uptime%3600)/60, uptime%60);
        let clk_x = self.screen_width - 120 - SAFE_PADDING;
        painter.draw_string(clk_x, tb_y+12, &time_str, Color::GREEN);

        for (i, w) in self.windows.iter().enumerate() { w.draw(painter, i == self.windows.len()-1); }
    }
}

pub fn compositor_paint() {
    unsafe {
        if let Some(bb) = &mut crate::BACK_BUFFER {
            bb.clear(Color::new(0, 0, 30));
            WINDOW_MANAGER.lock().draw(bb);
            let mouse = crate::mouse::MOUSE_STATE.lock();
            bb.draw_rect(Rect::new(mouse.x, mouse.y, 10, 10), Color::WHITE);
            bb.draw_rect(Rect::new(mouse.x+1, mouse.y+1, 8, 8), Color::RED);
            if let Some(s) = &mut crate::SCREEN_PAINTER { bb.present(s); }
        }
    }
}