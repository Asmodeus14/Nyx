use crate::syscalls::{sys_fs_read, sys_fs_write, sys_fs_count, sys_fs_get_name};
use crate::gfx::draw::{draw_rect_simple, draw_text};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

pub struct Terminal {
    history: Vec<String>,
    input: String,
    prompt: String,
    current_dir: String,
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            input: String::new(),
            prompt: String::from("> "),
            current_dir: String::from("/"),
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for line in s.split('\n') { self.history.push(String::from(line)); }
    }

    pub fn handle_key(&mut self, c: char) {
        if c == '\n' {
            let cmd = self.input.clone();
            self.history.push(format!("{} {}", self.current_dir, cmd));
            self.execute(&cmd);
            self.input.clear();
        } else if c == '\x08' { // Backspace
            self.input.pop();
        } else {
            self.input.push(c);
        }
    }

    fn execute(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() { return; }

        match parts[0] {
            "help" => self.history.push("cmds: ls, cd <dir>, cat <file>, clear".into()),
            "clear" => self.history.clear(),
            "pwd" => self.history.push(self.current_dir.clone()),
            "ls" => {
                let count = sys_fs_count(&self.current_dir);
                if count == 0 { self.history.push("(empty)".into()); }
                for i in 0..count {
                    let mut buf = [0u8; 64];
                    let len = sys_fs_get_name(&self.current_dir, i, &mut buf);
                    if len > 0 {
                        if let Ok(name) = core::str::from_utf8(&buf[..len]) {
                            self.history.push(format!("  {}", name));
                        }
                    }
                }
            },
            "cd" => {
                if parts.len() < 2 { return; }
                let new_dir = parts[1];
                if new_dir == ".." {
                    self.current_dir = String::from("/"); 
                } else {
                    if !self.current_dir.ends_with('/') { self.current_dir.push('/'); }
                    self.current_dir.push_str(new_dir);
                }
            },
            "cat" => {
                if parts.len() < 2 { return; }
                // Construct path
                let mut path = self.current_dir.clone();
                if !path.ends_with('/') { path.push('/'); }
                path.push_str(parts[1]);
                
                let mut buf = [0u8; 1024];
                let len = sys_fs_read(&path, &mut buf);
                if len > 0 {
                    if let Ok(s) = core::str::from_utf8(&buf[..len]) { self.history.push(s.into()); }
                    else { self.history.push("[Binary]".into()); }
                } else { self.history.push("Error reading file.".into()); }
            },
            "write" => {
                if parts.len() < 3 { self.history.push("Usage: write <file> <text>".into()); return; }
                let name = parts[1];
                let text = parts[2]; // Limitation: only takes one word of text due to split
                if sys_fs_write(name, text.as_bytes()) { self.history.push("OK".into()); }
                else { self.history.push("Fail".into()); }
            },
            _ => self.history.push("Unknown".into()),
        }
    }

    pub fn draw(&self, fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
        draw_rect_simple(fb, w, h, x + 2, y + 30, 600 - 4, 400 - 32, 0xFF000000);
        draw_rect_simple(fb, w, h, x + 2, y + 2, 600 - 4, 28, 0xFF333333);
        draw_text(fb, w, h, x + 10, y + 8, "Terminal", 0xFFFFFFFF);

        let mut draw_y = y + 40;
        let start = if self.history.len() > 20 { self.history.len() - 20 } else { 0 };
        
        for i in start..self.history.len() {
            if draw_y + 16 > y + h { break; }
            draw_text(fb, w, h, x + 10, draw_y, &self.history[i], 0xFFFFFFFF);
            draw_y += 16;
        }
        
        if draw_y + 16 < y + h {
            let input_line = format!("[{}]> {}_{}", self.current_dir, self.input, ""); 
            draw_text(fb, w, h, x + 10, draw_y, &input_line, 0xFF00FF00);
        }
    }
}