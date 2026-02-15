use crate::syscalls::{sys_fs_count, sys_fs_get_name};
use crate::gfx::draw::{draw_rect_simple, draw_text};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

pub struct Explorer {
    files: Vec<String>,
    pub selected_file: Option<String>,
    current_path: String,
}

impl Explorer {
    pub fn new() -> Self {
        let mut app = Self {
            files: Vec::new(),
            selected_file: None,
            current_path: String::from("/"),
        };
        app.refresh();
        app
    }

    pub fn refresh(&mut self) {
        self.files.clear();
        let count = sys_fs_count(&self.current_path);
        let limit = if count > 64 { 64 } else { count };

        for i in 0..limit {
            let mut name_buf = [0u8; 64];
            let len = sys_fs_get_name(&self.current_path, i, &mut name_buf);
            if len > 0 {
                if let Ok(s) = core::str::from_utf8(&name_buf[..len]) {
                    self.files.push(String::from(s));
                }
            }
        }
    }

    pub fn handle_click(&mut self, rel_x: usize, rel_y: usize, win_w: usize) {
        // OFFSET FIX: Ignore top 30px (Title Bar). Toolbar is 30..60.
        if rel_y < 30 { return; } // Drag area handled by OS
        
        // Toolbar Logic (30..60)
        if rel_y < 60 {
             // Scan Button
            if rel_x > win_w - 100 && rel_x < win_w - 50 { 
                self.refresh(); 
            } 
            // Back Button
            if rel_x < 60 { 
                self.current_path = String::from("/"); 
                self.refresh(); 
            }
            return;
        }

        // Grid Area (60+)
        let start_y = 60; 
        let icon_w = 70;
        let icon_h = 80;
        
        if win_w < 20 { return; }
        let cols = (win_w - 20) / icon_w;
        if cols == 0 { return; }

        let grid_y = rel_y.saturating_sub(start_y);
        let row = grid_y / icon_h;
        let col = (rel_x - 10) / icon_w;

        if col >= cols { return; }

        let idx = row * cols + col;
        if idx < self.files.len() {
            let name = self.files[idx].clone();
            if name.ends_with('/') {
                if !self.current_path.ends_with('/') { self.current_path.push('/'); }
                let dir_name = &name[..name.len()-1];
                self.current_path.push_str(dir_name);
                self.refresh();
                self.selected_file = None;
            } else {
                self.selected_file = Some(name);
            }
        } else {
            self.selected_file = None;
        }
    }

    pub fn draw(&self, fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, win_w: usize, win_h: usize) {
        // OFFSET FIX: Shift everything down by 30px
        let tb_h = 30;
        let title_bar = 30;
        let offset = title_bar + tb_h; // 60

        // Background
        draw_rect_simple(fb, w, h, x + 2, y + offset, win_w - 4, win_h - offset - 2, 0xFFFFFFFF);
        
        // Toolbar Background (y + 30)
        draw_rect_simple(fb, w, h, x + 2, y + title_bar, win_w - 4, tb_h, 0xFFEEEEEE);
        
        let tb_y = y + title_bar + 4;

        // Path Text
        let title = format!("Path: {}", self.current_path);
        draw_text(fb, w, h, x + 70, tb_y + 4, &title, 0xFF000000);
        
        // Back Button
        draw_rect_simple(fb, w, h, x + 5, tb_y, 50, 22, 0xFFCCCCCC);
        draw_text(fb, w, h, x + 15, tb_y + 4, "Root", 0xFF000000);

        // Scan Button
        draw_rect_simple(fb, w, h, x + win_w - 100, tb_y, 50, 22, 0xFFCCCCCC);
        draw_text(fb, w, h, x + win_w - 95, tb_y + 4, "Scan", 0xFF000000);

        // Grid
        let start_x = x + 10;
        let start_y = y + offset + 10;
        let icon_w = 70;
        let icon_h = 80;
        
        if win_w < 20 { return; }
        let cols = (win_w - 20) / icon_w;
        if cols == 0 { return; }

        for (i, name) in self.files.iter().enumerate() {
            let row = i / cols;
            let col = i % cols;
            let px = start_x + (col * icon_w);
            let py = start_y + (row * icon_h);

            if py + icon_h > y + win_h { break; }

            if let Some(sel) = &self.selected_file {
                if sel == name {
                    draw_rect_simple(fb, w, h, px, py, icon_w - 5, icon_h - 5, 0xFFCCE5FF);
                }
            }

            let is_dir = name.ends_with('/');
            let color = if is_dir { 0xFFEEDD00 } else { 0xFF888888 }; 

            draw_rect_simple(fb, w, h, px + 15, py + 10, 30, 40, color);
            draw_rect_simple(fb, w, h, px + 35, py + 10, 10, 10, 0xFFFFFFFF);

            let display_name = if name.len() > 8 { &name[..8] } else { name };
            draw_text(fb, w, h, px + 5, py + 55, display_name, 0xFF000000);
        }
    }
}