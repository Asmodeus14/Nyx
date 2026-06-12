use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use crate::gfx::draw;

pub struct TextSpan {
    pub text: String,
    pub color: u32,
    pub is_newline: bool,
    pub href: Option<String>,
}

pub struct BrowserApp {
    pub url_bar: String,
    content_spans: Vec<TextSpan>,
    scroll_offset: usize,
    status: String,
    is_typing_url: bool,
    link_boxes: Vec<(usize, usize, usize, usize, String)>,
    
    // --- Background Fetch State ---
    fetch_fd: Option<i64>,
    fetch_buffer: String,
}

impl BrowserApp {
    pub fn new() -> Self {
        let mut app = Self {
            url_bar: String::from("cloudflare.com"), // THE FIX: Default to a server that accepts minimal TLS 1.3
            content_spans: Vec::new(),
            scroll_offset: 0,
            status: String::from("Ready."),
            is_typing_url: false,
            link_boxes: Vec::new(),
            fetch_fd: None,
            fetch_buffer: String::new(),
        };
        app.render_welcome();
        app
    }

    fn render_welcome(&mut self) {
        // Line 1
        self.content_spans.push(TextSpan { text: String::from("Welcome to NyxBrowser v1.2 (Multi-Process)"), color: 0xFF00AAFF, is_newline: false, href: None });
        self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None }); // Line break
        
        // Line 2
        self.content_spans.push(TextSpan { text: String::from("Powered by Dynamic DNS and TLS 1.3"), color: 0xFFAAAAAA, is_newline: false, href: None });
        self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None }); // Line break
        
        // Line 3
        self.content_spans.push(TextSpan { text: String::from("Type any domain and hit Enter!"), color: 0xFFFFFFFF, is_newline: false, href: None });
        self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None }); // Line break
    }

    pub fn parse_html(&mut self, raw_html: &str) {
        self.content_spans.clear();
        self.scroll_offset = 0;

        let mut in_tag = false;
        let mut current_tag = String::new();
        let mut current_text = String::new();
        let mut ignore_content = false;
        
        let mut current_color = 0xFFCCCCCC; 
        let mut current_href: Option<String> = None; 

        for c in raw_html.chars() {
            if c == '<' {
                if !current_text.trim().is_empty() && !ignore_content {
                    self.content_spans.push(TextSpan { 
                        text: current_text.clone(), color: current_color, is_newline: false, href: current_href.clone(),
                    });
                }
                current_text.clear();
                in_tag = true;
                current_tag.clear();
            } else if c == '>' {
                in_tag = false;
                let tag = current_tag.to_lowercase();
                
                if tag.starts_with("/script") || tag.starts_with("/style") || tag.starts_with("/head") { ignore_content = false; } 
                else if tag.starts_with("/h1") || tag.starts_with("/h2") {
                    current_color = 0xFFCCCCCC;
                    self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None });
                } else if tag.starts_with("/a") {
                    current_color = 0xFFCCCCCC; current_href = None; 
                } else if tag.starts_with("/p") || tag.starts_with("/div") {
                    self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None });
                } else if tag.starts_with("script") || tag.starts_with("style") || tag.starts_with("head") { ignore_content = true; } 
                else if tag.starts_with("h1") || tag.starts_with("h2") {
                    current_color = 0xFF00FFFF;
                    self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None });
                } else if tag.starts_with("a ") {
                    current_color = 0xFF00FF00;
                    if let Some(start) = tag.find("href='").or_else(|| tag.find("href=\"")) {
                        let quote = &tag[start+5..start+6]; 
                        let url_start = start + 6;
                        if let Some(end) = tag[url_start..].find(quote) {
                            current_href = Some(String::from(&tag[url_start..url_start+end]));
                        }
                    }
                } else if tag.starts_with("img ") {
                    let mut alt_text = String::from("Image");
                    if let Some(start) = tag.find("alt='").or_else(|| tag.find("alt=\"")) {
                        let quote = &tag[start+4..start+5];
                        let alt_start = start + 5;
                        if let Some(end) = tag[alt_start..].find(quote) {
                            alt_text = String::from(&tag[alt_start..alt_start+end]);
                        }
                    }
                    self.content_spans.push(TextSpan { text: format!("[ IMG: {} ]", alt_text), color: 0xFFFFFF00, is_newline: true, href: None });
                } else if tag == "br" {
                    self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None });
                }
            } else if in_tag {
                current_tag.push(c);
            } else if !ignore_content {
                if c.is_whitespace() {
                    if !current_text.ends_with(' ') { current_text.push(' '); }
                } else { current_text.push(c); }
            }
        }
    }

    // --- Non-Blocking Pipe Poller ---
    pub fn pump_pipe(&mut self) -> bool {
        let mut needs_redraw = false;
        
        if let Some(fd) = self.fetch_fd {
            let mut buf = [0u8; 4096];
            let bytes_read = crate::syscalls::sys_read(fd, &mut buf);
            
            if bytes_read > 0 {
                if let Ok(s) = core::str::from_utf8(&buf[..bytes_read as usize]) {
                    self.fetch_buffer.push_str(s);
                }
                self.status = format!("Downloading... {} bytes", self.fetch_buffer.len());
                needs_redraw = true;
                
            } else if bytes_read == 0 || bytes_read != -11 {
                // EOF (Child Process Exited) or Hard Error
                crate::syscalls::sys_close(fd);
                self.fetch_fd = None;
                
                let safe_response = self.fetch_buffer
                    .replace("<", "[")
                    .replace(">", "]")
                    .replace("\n", "<br>");
                    
                let formatted = format!("<html><body><h1>TLS 1.3 Response: {}</h1><p>{}</p><br><a href='home'>Go Back</a></body></html>", self.url_bar, safe_response);
                self.parse_html(&formatted);
                self.status = format!("Loaded {} bytes.", self.fetch_buffer.len());
                needs_redraw = true;
            }
        }
        needs_redraw
    }

    // --- The Forking Engine (With Double-Fetch Protection) ---
    fn initiate_fetch(&mut self, url: &str) {
        // PREVENT KERNEL TANGLES: Do not allow a second fetch if one is already running!
        if self.fetch_fd.is_some() {
            self.status = String::from("ERR: Please wait for current fetch to finish.");
            return;
        }

        // INTERNAL ROUTING
        if url == "home" {
            self.url_bar = String::from("cloudflare.com"); 
            self.content_spans.clear();
            self.scroll_offset = 0; // THE SCROLL FIX: Reset the camera to the top of the page!
            self.render_welcome();
            self.status = String::from("Ready.");
            return;
        }

        self.url_bar = String::from(url);
        self.status = format!("Forking background process for {}...", url);
        self.fetch_buffer.clear();
        self.content_spans.clear();
        self.scroll_offset = 0; // Ensure new websites also start at the top
        
        self.content_spans.push(TextSpan { 
             text: String::from("Loading securely in background..."), 
             color: 0xFF00FFFF, is_newline: false, href: None 
         });
         self.content_spans.push(TextSpan { text: String::new(), color: 0, is_newline: true, href: None });

        // Add .trim() to safely strip hidden spaces and newlines
        let clean_url = url.trim().replace("https://", "").replace("http://", "");
        
        let mut fds = [-1; 2];
        if crate::syscalls::sys_pipe(&mut fds) == 0 {
            let read_fd = fds[0] as i64;
            let write_fd = fds[1] as i64;
            
            let pid = crate::syscalls::sys_fork();

            if pid == 0 {
                // CHILD PROCESS: Close the read end, do heavy crypto, pipe it back, and die.
                crate::syscalls::sys_close(read_fd);
                let response = crate::apps::https::run_https_fetch(&clean_url);
                crate::syscalls::sys_write(write_fd, response.as_bytes());
                crate::syscalls::sys_close(write_fd);
                crate::syscalls::sys_exit(0);
            } else if pid > 0 {
                // PARENT PROCESS (UI): Close the write end, hold onto the read end.
                crate::syscalls::sys_close(write_fd);
                self.fetch_fd = Some(read_fd);
            } else {
                self.status = String::from("ERR: Fork failed!");
            }
        } else {
            self.status = String::from("ERR: Pipe creation failed!");
        }
    }

    pub fn handle_key(&mut self, c: char) {
        if self.is_typing_url {
            if c == '\n' {
                self.is_typing_url = false;
                let url = self.url_bar.clone();
                self.initiate_fetch(&url);
            } else if c == '\x08' {
                self.url_bar.pop();
            } else {
                self.url_bar.push(c);
            }
        }
    }

    pub fn handle_click(&mut self, rel_x: usize, rel_y: usize, win_w: usize, win_h: usize) {
        if rel_y >= 30 && rel_y <= 60 && rel_x < win_w - 60 { self.is_typing_url = true; } 
        else { self.is_typing_url = false; }

        let track_x = win_w.saturating_sub(20);
        if rel_x >= track_x {
            if rel_y < win_h / 2 { self.scroll_offset = self.scroll_offset.saturating_sub(1); }
            else { self.scroll_offset += 1; }
            return;
        }

        let mut clicked_url = None;
        for (lx, ly, lw, lh, url) in &self.link_boxes {
            if rel_x >= *lx && rel_x <= *lx + *lw && rel_y >= *ly && rel_y <= *ly + *lh {
                clicked_url = Some(url.clone()); break;
            }
        }

        if let Some(url) = clicked_url {
            self.initiate_fetch(&url);
        }
    }

    // --- LAYOUT & RENDER ENGINE ---
    pub fn draw(&mut self, fb: &mut [u32], screen_w: usize, screen_h: usize, x: usize, y: usize, w: usize, h: usize) {
        let tb_h = 35;
        let content_y = y + 30 + tb_h;

        self.link_boxes.clear();

        draw::draw_rect_simple(fb, screen_w, screen_h, x + 2, y + 30, w - 4, tb_h, 0xFFDDDDDD);
        let url_bg = if self.is_typing_url { 0xFFFFFFFF } else { 0xFFEEEEEE };
        draw::draw_rect_simple(fb, screen_w, screen_h, x + 10, y + 35, w - 70, 24, url_bg);
        
        let cursor = if self.is_typing_url && (crate::syscalls::sys_get_time() / 500) % 2 == 0 { "_" } else { "" };
        let url_text = format!("{}{}", self.url_bar, cursor);
        draw::draw_text(fb, screen_w, screen_h, x + 15, y + 39, &url_text, 0xFF000000);

        draw::draw_rect_simple(fb, screen_w, screen_h, x + 2, content_y, w - 4, h - tb_h - 32, 0xFF121212);

        let mut draw_x = x + 10;
        let mut draw_y = content_y + 10;
        let max_x = x + w - 30; 

        let mut current_line = 0;

        for span in &self.content_spans {
            if span.is_newline {
                draw_x = x + 10;
                current_line += 1;
                if current_line >= self.scroll_offset { draw_y += 20; }
                continue;
            }

            for word in span.text.split_whitespace() {
                let word_width = word.len() * 9 + 9; 
                
                if draw_x + word_width > max_x {
                    draw_x = x + 10;
                    current_line += 1;
                    if current_line >= self.scroll_offset { draw_y += 20; }
                }

                if current_line >= self.scroll_offset && draw_y < y + h - 25 {
                    draw::draw_text(fb, screen_w, screen_h, draw_x, draw_y, word, span.color);
                    
                    if let Some(url) = &span.href {
                        self.link_boxes.push((draw_x - x, draw_y - y, word_width, 16, url.clone()));
                    }
                }

                draw_x += word_width;
            }
        }

        let track_x = x + w - 18;
        draw::draw_rect_simple(fb, screen_w, screen_h, track_x, content_y, 14, h - tb_h - 32, 0xFF2A2A2A);

        draw::draw_rect_simple(fb, screen_w, screen_h, x + 2, y + h - 20, w - 4, 18, 0xFF333333);
        draw::draw_text(fb, screen_w, screen_h, x + 5, y + h - 17, &self.status, 0xFFFFFFFF);
    }
}