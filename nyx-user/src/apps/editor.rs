use crate::gfx::draw::{draw_char, draw_rect_simple, draw_text};
use crate::gfx::font::{CHAR_WIDTH, CHAR_HEIGHT};
use crate::syscalls::{sys_fs_write, sys_fs_read, sys_fs_open};

const MAX_BUF: usize = 1024;

#[derive(PartialEq)]
pub enum EditorFocus {
    Filename,
    Content,
}

pub struct Editor {
    buffer: [char; MAX_BUF],
    len: usize,
    cursor: usize,
    pub filename: [char; 32], 
    pub filename_len: usize,
    pub is_dirty: bool,
    pub focus: EditorFocus,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            buffer: ['\0'; MAX_BUF],
            len: 0,
            cursor: 0,
            filename: ['\0'; 32], 
            filename_len: 0,
            is_dirty: false,
            focus: EditorFocus::Content, 
        }
    }

    pub fn load_file(&mut self, name: &str) {
        self.filename_len = name.len().min(32);
        for (i, c) in name.chars().enumerate() {
            if i < 32 { self.filename[i] = c; }
        }

        let id = sys_fs_open(name);
        if id > 0 {
            let mut byte_buf = [0u8; MAX_BUF];
            let read_len = sys_fs_read(id, &mut byte_buf);
            self.len = 0;
            for i in 0..read_len {
                self.buffer[i] = byte_buf[i] as char;
                self.len += 1;
            }
        } else {
            self.len = 0;
        }
        self.cursor = self.len; 
        self.is_dirty = false;
    }

    pub fn save_file(&mut self) -> bool {
        if self.filename_len == 0 { return false; }
        
        let mut byte_buf = [0u8; MAX_BUF];
        for i in 0..self.len {
            byte_buf[i] = self.buffer[i] as u8;
        }

        let mut name_buf = [0u8; 32];
        for i in 0..self.filename_len {
            name_buf[i] = self.filename[i] as u8;
        }

        if let Ok(name) = core::str::from_utf8(&name_buf[..self.filename_len]) {
            sys_fs_write(name, &byte_buf[..self.len]);
            self.is_dirty = false;
            return true;
        }
        false
    }

    pub fn handle_click(&mut self, rel_x: usize, rel_y: usize) -> bool {
        // Offset Logic: Window Title is 30px high. 
        // We must subtract 30 from rel_y to get "client area" Y.
        if rel_y < 30 { return false; } // Clicked title bar
        let client_y = rel_y - 30;

        // 1. Filename Box (Top Left of Client Area)
        // Rect: x:5, y:5, w:200, h:25
        if rel_x >= 5 && rel_x <= 205 && client_y >= 5 && client_y <= 30 {
            self.focus = EditorFocus::Filename;
            return false;
        }

        // 2. Save Button (Top Right of Client Area)
        // Rect: x:220, y:5, w:60, h:25
        if rel_x >= 220 && rel_x <= 280 && client_y >= 5 && client_y <= 30 {
            return self.save_file(); 
        }

        // 3. Content Area
        // Below y=40 (relative to client area)
        if client_y > 40 {
            self.focus = EditorFocus::Content;
        }
        
        false
    }

    pub fn handle_key(&mut self, c: char) {
        match self.focus {
            EditorFocus::Filename => self.handle_filename_key(c),
            EditorFocus::Content => self.handle_content_key(c),
        }
    }

    fn handle_filename_key(&mut self, c: char) {
        if c == '\n' {
            self.focus = EditorFocus::Content;
        } else if c == '\x08' {
            if self.filename_len > 0 { self.filename_len -= 1; }
        } else {
            if self.filename_len < 32 {
                self.filename[self.filename_len] = c;
                self.filename_len += 1;
            }
        }
    }

    fn handle_content_key(&mut self, c: char) {
        if c == '\x08' { 
            if self.cursor > 0 {
                for i in self.cursor..self.len { self.buffer[i - 1] = self.buffer[i]; }
                self.len -= 1; self.cursor -= 1; self.is_dirty = true;
            }
        } else {
            if self.len < MAX_BUF {
                for i in (self.cursor..self.len).rev() { self.buffer[i + 1] = self.buffer[i]; }
                self.buffer[self.cursor] = c;
                self.len += 1; self.cursor += 1; self.is_dirty = true;
            }
        }
    }

    // --- DRAW FUNCTION (FIXED) ---
    pub fn draw(&self, fb: &mut [u32], screen_w: usize, screen_h: usize, win_x: usize, win_y: usize, win_w: usize, win_h: usize) {
        // CONSTANTS
        let title_h = 30;  // Height of Window Title Bar (drawn by OS)
        let tool_h = 40;   // Height of Editor Toolbar
        let padding = 4;   // Inner padding

        // 1. Calculate Client Area
        // We start drawing BELOW the title bar.
        let client_y = win_y + title_h;
        let client_h = win_h.saturating_sub(title_h);

        // 2. Draw Toolbar Background
        // Only draws inside the window bounds
        draw_rect_simple(fb, screen_w, screen_h, win_x, client_y, win_w, tool_h, 0xFF333333);

        // 3. Filename Input Box
        let name_bg = if self.focus == EditorFocus::Filename { 0xFFFFFFFF } else { 0xFFAAAAAA };
        let name_fg = 0xFF000000;
        draw_rect_simple(fb, screen_w, screen_h, win_x + 5, client_y + 5, 200, 25, name_bg);
        
        let mut fx = win_x + 8;
        for i in 0..self.filename_len {
            draw_char(fb, screen_w, screen_h, fx, client_y + 10, self.filename[i], name_fg);
            fx += CHAR_WIDTH;
        }
        if self.focus == EditorFocus::Filename {
            draw_rect_simple(fb, screen_w, screen_h, fx, client_y + 10, 2, 16, 0xFF000000);
        }

        // 4. Save Button
        let btn_color = if self.is_dirty { 0xFFFFAA00 } else { 0xFF555555 };
        draw_rect_simple(fb, screen_w, screen_h, win_x + 220, client_y + 5, 60, 25, btn_color);
        draw_text(fb, screen_w, screen_h, win_x + 235, client_y + 10, "SAVE", 0xFFFFFFFF);

        // 5. Text Area Background
        // It starts below toolbar
        let text_area_y = client_y + tool_h;
        // Height is remaining space minus bottom padding
        let text_area_h = client_h.saturating_sub(tool_h).saturating_sub(padding);
        // Width is window width minus padding
        let text_area_w = win_w.saturating_sub(padding * 2);
        
        // Draw black background for text
        draw_rect_simple(fb, screen_w, screen_h, win_x + padding, text_area_y, text_area_w, text_area_h, 0xFF000000);

        // 6. Render Text Content
        let mut cx = win_x + padding + 4;
        let mut cy = text_area_y + 4;
        let max_y = text_area_y + text_area_h - CHAR_HEIGHT;

        for i in 0..self.len {
            if cy > max_y { break; } // Clipping check

            let c = self.buffer[i];
            
            if c == '\n' {
                cx = win_x + padding + 4;
                cy += CHAR_HEIGHT + 2;
                continue;
            }

            // Draw Cursor
            if i == self.cursor && self.focus == EditorFocus::Content {
                if cy <= max_y {
                    draw_rect_simple(fb, screen_w, screen_h, cx, cy, 2, CHAR_HEIGHT, 0xFF00FF00);
                }
            }

            // Wrap text
            if cx + CHAR_WIDTH < win_x + text_area_w - 4 {
                draw_char(fb, screen_w, screen_h, cx, cy, c, 0xFFFFFFFF);
                cx += CHAR_WIDTH;
            } else {
                cx = win_x + padding + 4;
                cy += CHAR_HEIGHT + 2;
                if cy > max_y { break; }
                draw_char(fb, screen_w, screen_h, cx, cy, c, 0xFFFFFFFF);
                cx += CHAR_WIDTH;
            }
        }
        
        // Draw cursor at end
        if self.cursor == self.len && self.focus == EditorFocus::Content {
             if cy <= max_y {
                draw_rect_simple(fb, screen_w, screen_h, cx, cy, 2, CHAR_HEIGHT, 0xFF00FF00);
             }
        }
    }
}