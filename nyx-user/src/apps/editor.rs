use crate::gfx::draw::{draw_rect_simple, draw_text, draw_char};
use crate::syscalls::{sys_fs_write, sys_fs_read}; // Removed sys_fs_open
use alloc::string::String;
use alloc::vec::Vec;

pub struct Editor {
    content: String,
    cursor: usize,
    filename: String,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            cursor: 0,
            filename: String::from("new.txt"),
        }
    }

    pub fn load_file(&mut self, name: &str) {
        self.filename = String::from(name);
        self.content.clear();
        self.cursor = 0;

        let mut buf = [0u8; 1024]; // 1KB Buffer
        // Direct Read without Open
        let len = sys_fs_read(name, &mut buf);
        if len > 0 {
            if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                self.content.push_str(s);
            }
        }
    }

    pub fn save_file(&self) {
        sys_fs_write(&self.filename, self.content.as_bytes());
    }

    pub fn handle_key(&mut self, key: char) {
        match key {
            '\x08' => { // Backspace
                if self.cursor > 0 {
                    self.content.remove(self.cursor - 1);
                    self.cursor -= 1;
                }
            },
            '\n' => {
                self.content.insert(self.cursor, '\n');
                self.cursor += 1;
            },
            _ => {
                self.content.insert(self.cursor, key);
                self.cursor += 1;
            }
        }
    }

    pub fn handle_click(&mut self, rel_x: usize, rel_y: usize) -> bool {
        // Toolbar Save Button (Top Right)
        if rel_y < 30 && rel_x > 400 {
            self.save_file();
            return true;
        }
        false
    }

    pub fn draw(&self, fb: &mut [u32], screen_w: usize, _h: usize, x: usize, y: usize, w: usize, h: usize) {
        // Background
        draw_rect_simple(fb, screen_w, _h, x + 2, y + 30, w - 4, h - 32, 0xFFFFFFFF);
        
        // Toolbar
        draw_rect_simple(fb, screen_w, _h, x + 2, y + 2, w - 4, 28, 0xFFDDDDDD);
        draw_text(fb, screen_w, _h, x + 10, y + 8, &self.filename, 0xFF000000);
        
        // Save Button
        draw_rect_simple(fb, screen_w, _h, x + w - 60, y + 4, 50, 24, 0xFFAAAAAA);
        draw_text(fb, screen_w, _h, x + w - 55, y + 8, "SAVE", 0xFF000000);

        // Content
        let mut cx = x + 10;
        let mut cy = y + 40;
        
        for (i, c) in self.content.chars().enumerate() {
            if c == '\n' {
                cx = x + 10;
                cy += 20;
                continue;
            }
            if cy + 20 > y + h { break; }
            draw_char(fb, screen_w, _h, cx, cy, c, 0xFF000000);
            cx += 9;
            
            if i == self.cursor - 1 {
                // Draw Cursor
                draw_rect_simple(fb, screen_w, _h, cx, cy, 2, 16, 0xFF000000);
            }
        }
    }
}