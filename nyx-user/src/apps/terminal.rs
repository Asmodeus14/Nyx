use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::fmt::Write;
use crate::gfx::draw;
use crate::syscalls;

pub struct Terminal {
    pub history: Vec<String>,
    pub current_line: String,
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            current_line: String::new(),
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for line in s.split('\n') {
            self.history.push(String::from(line));
        }
    }

    pub fn handle_key(&mut self, c: char) {
        if c == '\n' {
            let cmd = self.current_line.clone();
            self.history.push(format!("> {}", cmd));
            self.current_line.clear();
            self.execute_command(&cmd);
        } else if c == '\x08' { 
            self.current_line.pop();
        } else {
            self.current_line.push(c);
        }
    }

    fn execute_command(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        
        if cmd == "help" {
            self.write_str("Available commands:");
            self.write_str("  help       - Show this message");
            self.write_str("  clear      - Clear the terminal");
            self.write_str("  ls         - List files on NVMe");
            self.write_str("  run <file> - Execute an ELF binary (e.g., 'run hello.elf')");
            self.write_str("  lspci      - List PCI devices (Hardware Scan)");
            self.write_str("  uptime     - Show system uptime");
            self.write_str("  dmesg      - Display kernel boot logs");
            self.write_str("  entity     - Commune with the Nyx Entity");
        } 
        else if cmd == "clear" {
            self.history.clear();
        } 
        // 🚨 ADDED THE RUN COMMAND FOR NATIVE C EXECUTABLES
        else if cmd.starts_with("run ") {
            let filename = &cmd[4..];
            self.write_str(&format!("Allocating Ring 3 memory for {}...", filename));
            
            let ret = syscalls::sys_execve(filename);
            
            if ret < 0 {
                self.write_str(&format!("ERR: Failed to execute {}. Code: {}", filename, ret));
            }
        }
        else if cmd == "ls" {
            let count = syscalls::sys_fs_count("/");
            if count == 0 {
                self.write_str("Directory is empty.");
            } else {
                for i in 0..count {
                    let mut buf = [0u8; 64];
                    let len = syscalls::sys_fs_get_name("/", i, &mut buf);
                    if len > 0 {
                        if let Ok(name) = core::str::from_utf8(&buf[..len]) {
                            self.history.push(String::from(name));
                        }
                    }
                }
            }
        } 
        else if cmd == "lspci" || cmd == "hwinfo" {
            let mut buf = [0u8; 1024]; 
            let len = syscalls::sys_get_hw_info(&mut buf);
            if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                self.write_str(s);
            }
        } 
        else if cmd == "uptime" {
            let ticks = syscalls::sys_get_time();
            let seconds = ticks / 1000;
            self.write_str(&format!("Uptime: {} seconds ({} ticks)", seconds, ticks));
        }
        else if cmd == "dmesg" {
            let mut buf = [0u8; 1024];
            let len = syscalls::sys_get_boot_logs(&mut buf);
            if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                self.write_str(s);
            }
        } 
        else if cmd == "entity" || cmd == "soul" {
            let mut entity_dna = [0u8; 32];
            if syscalls::sys_get_entity_state(&mut entity_dna) {
                let mut hex_string = String::new();
                for byte in entity_dna.iter() {
                    let _ = write!(&mut hex_string, "{:02X}", byte);
                }
                self.write_str("Nyx Entity Awakened.");
                self.write_str(&format!("Genetic Seed (DNA): {}", hex_string));
            } else {
                self.write_str("ERR: Failed to commune with the Entity. Is it born yet?");
            }
        } 
        else if cmd.starts_with("echo ") {
            self.write_str(&cmd[5..]);
        } 
        else if cmd == "" {
        } 
        else {
            self.write_str(&format!("Command not found: {}", cmd));
        }
    }

    pub fn draw(&self, fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
        let start_y = y + 30;
        let mut current_y = start_y;
        let line_height = 16;
        let max_lines = 24; 
        
        let total_lines = self.history.len() + 1; 
        let skip = if total_lines > max_lines { total_lines - max_lines } else { 0 };

        let mut drawn = 0;
        for line in self.history.iter().skip(skip) {
            draw::draw_text(fb, w, h, x + 10, current_y, line, 0xFF00FF00); 
            current_y += line_height;
            drawn += 1;
            if drawn >= max_lines { break; }
        }
        
        if drawn < max_lines {
            let cursor = if (syscalls::sys_get_time() / 500) % 2 == 0 { "_" } else { " " };
            let prompt = format!("> {}{}", self.current_line, cursor);
            draw::draw_text(fb, w, h, x + 10, current_y, &prompt, 0xFF00FF00);
        }
    }
}