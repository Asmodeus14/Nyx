use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use crate::gfx::draw;
use crate::syscalls;

pub struct Terminal {
    pub history: Vec<String>,
    pub current_line: String,
    pub active_child_fd: Option<i64>, 
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            current_line: String::new(),
            active_child_fd: None,
        }
    }

    pub fn pump_pipe(&mut self) {
        if let Some(fd) = self.active_child_fd {
            let mut buf = [0u8; 256];
            let bytes_read = syscalls::sys_read(fd, &mut buf);
            
            if bytes_read > 0 {
                if let Ok(s) = core::str::from_utf8(&buf[..bytes_read as usize]) {
                    self.write_str(s);
                }
            } else if bytes_read == 0 { 
                syscalls::sys_close(fd);
                self.active_child_fd = None;
            } else if bytes_read != -11 { 
                syscalls::sys_close(fd);
                self.active_child_fd = None;
            }
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
            self.write_str("Commands: clear, ls, lspci, uptime, dmesg, entity, run <file>, fetch, https");
        } 
        else if cmd == "clear" {
            self.history.clear();
        } 
        else if cmd.starts_with("run ") {
            let filename = &cmd[4..];
            let mut pipe_fds = [-1; 2];
            
            if syscalls::sys_pipe(&mut pipe_fds) == 0 {
                let read_fd = pipe_fds[0] as i64;
                let write_fd = pipe_fds[1] as i64;
                
                let pid = syscalls::sys_fork();
                
                if pid == 0 {
                    syscalls::sys_dup2(write_fd, 1);
                    syscalls::sys_close(read_fd); 
                    
                    syscalls::sys_execve(filename);
                    syscalls::sys_exit(-1);
                } else if pid > 0 {
                    syscalls::sys_close(write_fd); 
                    self.active_child_fd = Some(read_fd);
                } else {
                    self.write_str("ERR: Fork failed!");
                }
            } else {
                self.write_str("ERR: Pipe creation failed!");
            }
        }
        else if cmd == "ls" {
            let count = syscalls::sys_fs_count("/");
            if count == 0 { self.write_str("Directory is empty."); } else {
                for i in 0..count {
                    let mut buf = [0u8; 64];
                    let len = syscalls::sys_fs_get_name("/", i, &mut buf);
                    if len > 0 { if let Ok(name) = core::str::from_utf8(&buf[..len]) { self.history.push(String::from(name)); } }
                }
            }
        } 
        else if cmd == "lspci" || cmd == "hwinfo" {
            let mut buf = [0u8; 1024]; 
            let len = syscalls::sys_get_hw_info(&mut buf);
            if let Ok(s) = core::str::from_utf8(&buf[..len]) { self.write_str(s); }
        } 
        else if cmd == "uptime" {
            let ticks = syscalls::sys_get_time();
            self.write_str(&format!("Uptime: {} seconds ({} ticks)", ticks / 1000, ticks));
        }
        else if cmd == "dmesg" {
            let mut buf = [0u8; 1024];
            let len = syscalls::sys_get_boot_logs(&mut buf);
            if let Ok(s) = core::str::from_utf8(&buf[..len]) { self.write_str(s); }
        } 
        else if cmd == "entity" {
            let mut seed = [0u8; 32];
            if syscalls::sys_get_entity_state(&mut seed) {
                let mut hex_str = String::new();
                for b in seed.iter() {
                    // Quick hex formatting without relying on core::fmt directly
                    let hex_chars = b"0123456789ABCDEF";
                    hex_str.push(hex_chars[(b >> 4) as usize] as char);
                    hex_str.push(hex_chars[(b & 0x0F) as usize] as char);
                }
                self.write_str(&format!("Genetic Seed: [{}]", hex_str));
            } else {
                self.write_str("Entity seed not locked or unavailable.");
            }
        }
        else if cmd.starts_with("echo ") {
            self.write_str(&cmd[5..]);
        }
        else if cmd == "fetch" {
            self.write_str(">>> Dialing 192.168.1.100:80 via TCP...");
            let response = crate::apps::fetch::run_nyxfetch();
            self.write_str(&response);
        }
        // 🚨 THE NEW SECURE TLS COMMAND 🚨
        else if cmd == "https" {
            self.write_str(">>> Initializing TLS 1.3 to 1.1.1.1:443...");
            let response = crate::apps::https::run_https_fetch();
            self.write_str(&response);
        }
        else if cmd == "" { } 
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