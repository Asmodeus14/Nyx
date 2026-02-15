use crate::gfx::draw::{draw_rect_simple, draw_text, draw_char};
use crate::syscalls::{sys_fs_write, sys_fs_read};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

pub struct Editor {
    content: String,
    cursor: usize,
    filename: String,
    save_status: u8,      // 0: None, 1: Saved, 2: Error, 3: Loaded
    status_timer: usize,
    editing_filename: bool,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            cursor: 0,
            filename: String::from("new.txt"),
            save_status: 0,
            status_timer: 0,
            editing_filename: false,
        }
    }

    pub fn load_file(&mut self) {
        self.content.clear();
        self.cursor = 0;

        let mut buf = [0u8; 2048]; 
        let len = sys_fs_read(&self.filename, &mut buf);
        
        if len > 0 {
            if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                self.content.push_str(s);
                self.cursor = self.content.len(); 
                self.save_status = 3; // LOADED
                self.status_timer = 60;
            }
        } else {
            self.save_status = 2; // ERROR
            self.status_timer = 60;
        }
    }

    pub fn save_file(&mut self) {
        if sys_fs_write(&self.filename, self.content.as_bytes()) {
            self.save_status = 1; // SAVED
            self.status_timer = 60;
        } else {
            self.save_status = 2; // ERROR
            self.status_timer = 60;
        }
    }

    pub fn handle_key(&mut self, key: char) {
        if self.editing_filename {
            match key {
                '\n' => self.editing_filename = false,
                '\x08' => { if !self.filename.is_empty() { self.filename.pop(); } },
                _ => self.filename.push(key),
            }
            return;
        }

        if self.save_status != 0 { self.save_status = 0; }
        
        match key {
            '\x08' => { 
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

    pub fn handle_click(&mut self, rel_x: usize, rel_y: usize, w: usize) -> bool {
        // OFFSET FIX: Toolbar is now at Y=30 to Y=60 (below drag area)
        if rel_y >= 30 && rel_y < 60 {
            let toolbar_y = rel_y - 30; // Local Y inside toolbar

            // 1. Filename Box (Left)
            if rel_x < 250 {
                self.editing_filename = true;
                return true;
            } else {
                self.editing_filename = false;
            }

            // 2. SAVE Button (Right: w-100 to w-50)
            if rel_x >= (w - 100) && rel_x <= (w - 50) {
                self.save_file();
                return true;
            }

            // 3. LOAD Button (Left of Save: w-160 to w-110)
            if rel_x >= (w - 160) && rel_x <= (w - 110) {
                self.load_file();
                return true;
            }
        } else {
            self.editing_filename = false;
        }
        false
    }

    pub fn draw(&mut self, fb: &mut [u32], screen_w: usize, _h: usize, x: usize, y: usize, w: usize, h: usize) {
        // OFFSET FIX: Everything is pushed down by 30px to clear the Window Title Bar
        let title_bar_h = 30;
        let toolbar_h = 30;
        let content_y_off = title_bar_h + toolbar_h;

        // 1. Toolbar Background (y + 30)
        draw_rect_simple(fb, screen_w, _h, x + 2, y + title_bar_h, w - 4, toolbar_h, 0xFFDDDDDD);
        
        // 2. Content Background (y + 60)
        draw_rect_simple(fb, screen_w, _h, x + 2, y + content_y_off, w - 4, h - content_y_off - 2, 0xFFFFFFFF);

        // --- TOOLBAR WIDGETS (Drawn relative to y + 30) ---
        let tb_y = y + title_bar_h + 4; // y + 34

        // Filename Box
        let fname_bg = if self.editing_filename { 0xFFFFFFFF } else { 0xFFDDDDDD };
        draw_rect_simple(fb, screen_w, _h, x + 5, tb_y, 240, 24, fname_bg);
        
        let display_name = if self.editing_filename && (self.status_timer / 10) % 2 == 0 {
            format!("{}_", self.filename)
        } else {
            self.filename.clone()
        };
        draw_text(fb, screen_w, _h, x + 10, tb_y + 4, &display_name, 0xFF000000);
        
        // LOAD Button
        let load_x = x + w - 160;
        draw_rect_simple(fb, screen_w, _h, load_x, tb_y, 50, 24, 0xFFCCCCCC);
        draw_text(fb, screen_w, _h, load_x + 5, tb_y + 4, "LOAD", 0xFF000000);

        // SAVE Button
        let save_x = x + w - 100;
        let (save_color, save_text) = if self.status_timer > 0 {
            self.status_timer -= 1;
            match self.save_status {
                1 => (0xFF00FF00, "OK!"),
                2 => (0xFFFF0000, "ERR"),
                3 => (0xFF00FFFF, "READ"),
                _ => (0xFFAAAAAA, "SAVE"),
            }
        } else {
            (0xFFAAAAAA, "SAVE")
        };
        draw_rect_simple(fb, screen_w, _h, save_x, tb_y, 50, 24, save_color);
        draw_text(fb, screen_w, _h, save_x + 5, tb_y + 4, save_text, 0xFF000000);

        // --- CONTENT (Drawn relative to y + 60) ---
        let mut cx = x + 10;
        let mut cy = y + content_y_off + 10; // y + 70
        
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
                draw_rect_simple(fb, screen_w, _h, cx, cy, 2, 16, 0xFF000000);
            }
        }
        if self.content.is_empty() {
             draw_rect_simple(fb, screen_w, _h, cx, cy, 2, 16, 0xFF000000);
        }
    }
}