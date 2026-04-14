use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use crate::syscalls::UdpSocket;
use crate::gfx::draw;

// Standard POSIX Error Codes
const EAGAIN: i64 = -11;

pub struct NetChatApp {
    socket: Option<UdpSocket>,
    status_log: String,
    messages: Vec<String>,
    current_input: String,
}

impl NetChatApp {
    pub fn new() -> Self {
        Self {
            socket: None,
            status_log: "Initializing Network...".into(),
            messages: Vec::new(),
            current_input: String::new(),
        }
    }

    pub fn init(&mut self) {
        // 1. Ask the kernel for a socket
        if let Some(sock) = UdpSocket::new() {
            self.status_log = "Socket Allocated!".into();
            
            // 2. Connect to your Windows Host Machine
            // 🚨 Replace 192, 168, 1, 100 with your actual Windows IPv4 address!
            if sock.connect(192, 168, 1, 100, 8080) {
                self.status_log = "Connected to 192.168.1.100:8080".into();
                
                // 3. Send a greeting packet across the PCI bus!
                let msg = b"NyxOS NetChat connected.\n";
                sock.send(msg);
                self.messages.push("[System] Connected to host.".into());
            } else {
                self.status_log = "Connection Failed.".into();
            }
            self.socket = Some(sock);
        } else {
            self.status_log = "Failed to allocate socket.".into();
        }
    }

    /// Called every frame by the window manager
    pub fn update(&mut self) -> bool {
        let mut got_new_data = false;
        
        if let Some(sock) = &self.socket {
            let mut buf = [0u8; 2048];
            
            // 🚨 THE CONTRACT: Receive data and capture the signed return value
            let ret = sock.recv(&mut buf);
            
            // 1. Check for Errors or Empty Buffers FIRST
            if ret < 0 {
                if ret == EAGAIN {
                    // This is perfectly fine. It's a non-blocking socket and no packet has arrived yet.
                    // We just return false and let the UI render normally.
                    return false; 
                } else {
                    // A genuine hardware or network error occurred (e.g., EBADF or ENOMEM).
                    self.status_log = format!("Network Error: Code {}", ret);
                    return true; // Return true to force a UI redraw with the error message
                }
            }
            
            // 2. Handle Connection Closed
            if ret == 0 {
                self.status_log = "Connection closed by peer.".into();
                return true; 
            }

            // 3. Safe Processing: We know `ret` is > 0, so casting to usize is mathematically safe!
            let size = ret as usize;
            
            // 4. Parse the safe buffer
            if let Ok(text) = core::str::from_utf8(&buf[..size]) {
                if !text.trim().is_empty() {
                    self.messages.push(format!("Host: {}", text.trim_end()));
                    got_new_data = true;
                }
            }
        }
        
        got_new_data
    }
    
    /// Renders the chat UI
    pub fn draw(&self, fb: &mut [u32], screen_w: usize, screen_h: usize, x: usize, y: usize, w: usize, h: usize) {
        let pad_x = x + 10;
        let mut current_y = y + 30; // Start below title bar
        
        // Draw Status Bar
        draw::draw_text(fb, screen_w, screen_h, pad_x, current_y, &self.status_log, 0xFF00AAFF);
        current_y += 20;

        // Draw message history
        let line_height = 16;
        let max_lines = (h.saturating_sub(90)) / line_height; 
        
        let skip = if self.messages.len() > max_lines { self.messages.len() - max_lines } else { 0 };

        for msg in self.messages.iter().skip(skip) {
            let color = if msg.starts_with("You:") { 0xFFFFFFFF } else { 0xFF00FF00 };
            draw::draw_text(fb, screen_w, screen_h, pad_x, current_y, msg, color);
            current_y += line_height;
        }

        // Draw input box area at the bottom
        let input_y = y + h - 25;
        draw::draw_rect(fb, screen_w, screen_h, x + 5, input_y - 5, w - 10, 25, 0xFF222222);
        
        // Blink cursor logic
        let ticks = unsafe { crate::syscalls::sys_get_time() };
        let show_cursor = (ticks % 1000) < 500;
        
        let display_text = if show_cursor {
            format!("> {}_", self.current_input)
        } else {
            format!("> {}", self.current_input)
        };
        
        draw::draw_text(fb, screen_w, screen_h, pad_x, input_y, &display_text, 0xFFFFFFFF);
    }
    
    pub fn handle_key(&mut self, c: char) {
        if c == '\n' {
            if !self.current_input.is_empty() {
                // Send the message across the network!
                if let Some(sock) = &self.socket {
                    let msg = format!("{}\n", self.current_input);
                    sock.send(msg.as_bytes());
                }
                
                self.messages.push(format!("You: {}", self.current_input));
                self.current_input.clear();
            }
        } else if c == '\x08' { // Backspace
            self.current_input.pop();
        } else {
            self.current_input.push(c);
        }
    }
}