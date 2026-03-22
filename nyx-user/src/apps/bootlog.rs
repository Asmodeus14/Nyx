use crate::syscalls::sys_get_boot_logs;
use crate::gfx::draw;
use alloc::string::String;
use alloc::vec::Vec;

pub struct BootlogApp {
    log_lines: Vec<String>,
    pub scroll_offset: usize,
    buf: Vec<u8>,         // Keep the buffer alive permanently!
    last_len: usize,      // Track if the logs actually changed
}

impl BootlogApp {
    pub fn new() -> Self {
        Self {
            log_lines: Vec::new(),
            scroll_offset: 0,
            buf: alloc::vec![0u8; 16384], // Allocate ONCE.
            last_len: 0,
        }
    }

    pub fn refresh(&mut self) {
        let len = sys_get_boot_logs(&mut self.buf);
        
    
        if len == self.last_len { return; }
        self.last_len = len;
        
        self.log_lines.clear();
        if len > 0 {
            if let Ok(text) = core::str::from_utf8(&self.buf[..len]) {
                for line in text.split('\n') {
                    if !line.trim().is_empty() {
                        self.log_lines.push(String::from(line));
                    }
                }
            }
        }
        
        let max_lines = 28;
        self.scroll_offset = self.log_lines.len().saturating_sub(max_lines);
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    pub fn handle_click(&mut self, mouse_x: usize, mouse_y: usize, win_x: usize, win_y: usize, win_w: usize, win_h: usize) -> bool {
        let track_x = win_x + win_w - 20;
        let track_w = 16;
        if mouse_x >= track_x && mouse_x <= track_x + track_w {
            let middle_y = win_y + (win_h / 2);
            if mouse_y < middle_y { self.scroll_up(5); }
            else { self.scroll_down(5); }
            return true;
        }
        false
    }

    pub fn draw(&mut self, fb: &mut [u32], screen_w: usize, screen_h: usize, window_x: usize, window_y: usize, window_w: usize, window_h: usize) {
        let content_x = window_x + 10;
        let mut content_y = window_y + 35;
        
        draw::draw_rect_simple(fb, screen_w, screen_h, window_x + 2, window_y + 30, window_w - 4, window_h - 32, 0xFF121212);
        
        let max_lines = (window_h - 40) / 16;
        let total_lines = self.log_lines.len();
        
        let max_scroll = total_lines.saturating_sub(max_lines);
        if self.scroll_offset > max_scroll { self.scroll_offset = max_scroll; }

        let start_idx = self.scroll_offset;
        let end_idx = core::cmp::min(start_idx + max_lines, total_lines);

        for i in start_idx..end_idx {
            draw::draw_text(fb, screen_w, screen_h, content_x, content_y, &self.log_lines[i], 0xFFCCCCCC);
            content_y += 16;
        }

        if total_lines > max_lines {
            let track_x = window_x + window_w - 18;
            let track_y = window_y + 32;
            let track_h = window_h - 36;
            let track_w = 14;

            draw::draw_rect_simple(fb, screen_w, screen_h, track_x, track_y, track_w, track_h, 0xFF2A2A2A);
            let thumb_h = core::cmp::max(20, (max_lines * track_h) / total_lines);
            let scroll_percent = if max_scroll > 0 { (self.scroll_offset * 100) / max_scroll } else { 0 };
            
            let max_thumb_y = track_h - thumb_h;
            let thumb_y_offset = (max_thumb_y * scroll_percent) / 100;
            
            draw::draw_rect_simple(fb, screen_w, screen_h, track_x + 2, track_y + thumb_y_offset, track_w - 4, thumb_h, 0xFF666666);
        }
    }
}