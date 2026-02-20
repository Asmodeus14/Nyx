use crate::syscalls::sys_get_boot_logs;
use crate::gfx::draw;
use alloc::string::String;
use alloc::vec::Vec;

pub struct BootlogApp {
    log_lines: Vec<String>,
}

impl BootlogApp {
    pub fn new() -> Self {
        // Create a 16KB buffer to hold the logs
        let mut buf = alloc::vec![0u8; 16384];
        let len = sys_get_boot_logs(&mut buf);
        
        let mut log_lines = Vec::new();
        if len > 0 {
            if let Ok(text) = core::str::from_utf8(&buf[..len]) {
                for line in text.split('\n') {
                    // Filter out empty newlines to save space
                    if !line.trim().is_empty() {
                        log_lines.push(String::from(line));
                    }
                }
            }
        }
        
        Self { log_lines }
    }

    pub fn draw(&mut self, fb: &mut [u32], screen_w: usize, screen_h: usize, window_x: usize, window_y: usize, window_w: usize, window_h: usize) {
        let content_x = window_x + 10;
        let mut content_y = window_y + 35;
        
        // Draw Hacker-Black Background for the log viewer
        draw::draw_rect_simple(fb, screen_w, screen_h, window_x + 2, window_y + 30, window_w - 4, window_h - 32, 0xFF121212);
        
        // Calculate how many lines we can fit in the window
        let max_lines = (window_h - 40) / 16;
        
        // Auto-scroll to the bottom (show the most recent logs)
        let start_idx = if self.log_lines.len() > max_lines {
            self.log_lines.len() - max_lines
        } else { 0 };

        for i in start_idx..self.log_lines.len() {
            // Draw logs in console gray
            draw::draw_text(fb, screen_w, screen_h, content_x, content_y, &self.log_lines[i], 0xFFCCCCCC);
            content_y += 16;
        }
    }
}