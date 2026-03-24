use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use crate::syscalls::UdpSocket;
use crate::gfx::draw;

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
            self.status_log = "Failed to allocate Socket FD.".into();
        }
    }

    /// Handles typing and sending messages
    pub fn handle_key(&mut self, c: char) {
        if c == '\n' {
            if !self.current_input.is_empty() {
                // Send over network
                if let Some(sock) = &self.socket {
                    let mut payload = self.current_input.clone();
                    payload.push('\n');
                    sock.send(payload.as_bytes());
                }
                
                // Add to local UI and clear input
                self.messages.push(format!("You: {}", self.current_input));
                self.current_input.clear();
            }
        } else if c == '\x08' { // Backspace
            self.current_input.pop();
        } else {
            self.current_input.push(c);
        }
    }

    /// Polls for incoming network packets. Returns true if the window needs a UI redraw.
    pub fn update(&mut self) -> bool {
        let mut got_new_data = false;
        if let Some(sock) = &self.socket {
            let mut rx_buf = [0u8; 512];
            
            // Non-blocking poll for incoming data
            let bytes_read = sock.recv(&mut rx_buf);
            
            if bytes_read > 0 {
                if let Ok(text) = core::str::from_utf8(&rx_buf[..bytes_read as usize]) {
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
        draw::draw_rect_simple(fb, screen_w, screen_h, x + 5, input_y - 5, w - 10, 25, 0xFF111111);
        
        let cursor = if (crate::syscalls::sys_get_time() / 500) % 2 == 0 { "_" } else { " " };
        let prompt = format!("> {}{}", self.current_input, cursor);
        draw::draw_text(fb, screen_w, screen_h, pad_x, input_y, &prompt, 0xFFFFFF00);
    }
}