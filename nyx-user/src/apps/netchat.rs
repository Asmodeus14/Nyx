use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use crate::syscalls::{UdpSocket, sys_open, sys_read, sys_close, sys_spawn_thread, sys_yield};
use crate::gfx::draw;
use core::sync::atomic::{AtomicUsize, AtomicU64, Ordering};

const EAGAIN: i64 = -11;

// --- THE FIX: EXPAND TO 8K ---
// 8192 is large enough to hold an entire 1080p frame (5925 chunks) without 
// dropping the packets that make up the bottom of the screen!
const QUEUE_SIZE: usize = 8192; 

#[derive(Clone, Copy)]
struct Packet {
    len: usize,
    data: [u8; 1450],
}

static mut PACKET_QUEUE: [Option<Packet>; QUEUE_SIZE] = [const { None }; QUEUE_SIZE];
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

static SHARED_FD: AtomicU64 = AtomicU64::new(0);

fn enqueue_packet(data: &[u8], is_eof: bool) {
    let head = HEAD.load(Ordering::Relaxed);
    let next_head = (head + 1) % QUEUE_SIZE;
    
    if next_head != TAIL.load(Ordering::Acquire) {
        let mut pkt = Packet { len: data.len(), data: [0; 1450] };
        pkt.data[..data.len()].copy_from_slice(data);
        unsafe { PACKET_QUEUE[head] = Some(pkt); }
        HEAD.store(next_head, Ordering::Release);
    } else if is_eof {
        let prev_head = if head == 0 { QUEUE_SIZE - 1 } else { head - 1 };
        let mut pkt = Packet { len: data.len(), data: [0; 1450] };
        pkt.data[..data.len()].copy_from_slice(data);
        unsafe { PACKET_QUEUE[prev_head] = Some(pkt); }
    }
}

fn flush_queue() {
    HEAD.store(0, Ordering::Relaxed);
    TAIL.store(0, Ordering::Relaxed);
}

// THIS RUNS ON A LOAD-BALANCED CORE! 
extern "C" fn udp_broadcaster_thread() {
    loop {
        let fd = SHARED_FD.load(Ordering::Relaxed);
        
        if fd == 0 { 
            sys_yield(); 
            continue; 
        } 
        
        let sock = UdpSocket { fd: fd as i64 };
        let tail = TAIL.load(Ordering::Relaxed);
        let head = HEAD.load(Ordering::Acquire);
        
        if tail != head {
            if let Some(ref pkt) = unsafe { &PACKET_QUEUE[tail] } {
                let data_slice = &pkt.data[..pkt.len];
                if sock.send(data_slice) {
                    TAIL.store((tail + 1) % QUEUE_SIZE, Ordering::Release);
                } else {
                    sys_yield();
                }
            } else {
                TAIL.store((tail + 1) % QUEUE_SIZE, Ordering::Release);
            }
        } else {
            sys_yield(); 
        }
    }
}

// --- STANDARD NETCHAT CODE ---

fn quick_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0;
    let ptr = data.as_ptr() as *const u64;
    let len = data.len() / 8; 
    unsafe {
        for i in 0..len { hash = hash.rotate_left(5) ^ *ptr.add(i); }
    }
    
    // THE FIX: Catch trailing bytes for the final chunk at the bottom right of the screen
    let remainder = data.len() % 8;
    if remainder > 0 {
        let mut tail = [0u8; 8];
        let tail_ptr = unsafe { data.as_ptr().add(len * 8) };
        unsafe { core::ptr::copy_nonoverlapping(tail_ptr, tail.as_mut_ptr(), remainder); }
        hash = hash.rotate_left(5) ^ u64::from_le_bytes(tail);
    }
    
    hash
}

#[derive(PartialEq)]
enum NetState { AutoDetect, ManualEntry, Connected }

pub struct NetChatApp {
    socket: Option<UdpSocket>,
    status_log: String,
    messages: Vec<String>,
    current_input: String,
    state: NetState,
    casting: bool,
    screenshot_pending: bool,
    cast_chunk_index: u32, 
    frame_id: u32,
    chunk_hashes: Vec<u64>, 
    keyframe_timer: u32,    
    thread_spawned: bool,
}

impl NetChatApp {
    pub fn new() -> Self {
        Self {
            socket: None,
            status_log: "Initializing Network...".into(),
            messages: Vec::new(),
            current_input: String::new(),
            state: NetState::AutoDetect,
            casting: false,
            screenshot_pending: false,
            cast_chunk_index: 0,
            frame_id: 0,
            chunk_hashes: Vec::new(),
            keyframe_timer: 60,
            thread_spawned: false, 
        }
    }

    pub fn init(&mut self) {
        if let Some(ip) = Self::try_auto_detect() {
            self.connect_to(ip[0], ip[1], ip[2], ip[3]);
        } else {
            self.status_log = "Auto-detect failed. Enter Host IP:".into();
            self.state = NetState::ManualEntry;
        }
    }

    fn try_auto_detect() -> Option<[u8; 4]> {
        let fd = sys_open("host_ip.txt");
        if fd >= 0 {
            let mut buf = [0u8; 32];
            let len = sys_read(fd, &mut buf);
            sys_close(fd);
            if len > 0 {
                if let Ok(text) = core::str::from_utf8(&buf[..len as usize]) {
                    return Self::parse_ip(text.trim());
                }
            }
        }
        None
    }

    fn parse_u8(s: &str) -> Option<u8> {
        let mut res = 0u32;
        if s.is_empty() { return None; }
        for c in s.chars() {
            if let Some(d) = c.to_digit(10) { res = res * 10 + d; if res > 255 { return None; }
            } else { return None; }
        }
        Some(res as u8)
    }

    fn parse_ip(s: &str) -> Option<[u8; 4]> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() == 4 {
            Some([Self::parse_u8(parts[0])?, Self::parse_u8(parts[1])?, Self::parse_u8(parts[2])?, Self::parse_u8(parts[3])?])
        } else { None }
    }

    fn connect_to(&mut self, a: u8, b: u8, c: u8, d: u8) {
        if let Some(sock) = UdpSocket::new() {
            if sock.connect(a, b, c, d, 8080) {
                self.status_log = format!("Connected to {}.{}.{}.{}:8080", a, b, c, d);
                SHARED_FD.store(sock.fd as u64, Ordering::SeqCst);
                
                let msg = b"NyxOS NetChat connected.\n";
                sock.send(msg);
                self.messages.push("[System] Connected! Type /help for commands.".into());
                self.socket = Some(sock);
                self.state = NetState::Connected;
                
                if !self.thread_spawned {
                    let stack = alloc::vec![0u8; 8192]; 
                    let stack_ptr = alloc::boxed::Box::leak(stack.into_boxed_slice()); 
                    let stack_top = unsafe { stack_ptr.as_mut_ptr().add(8192) };
                    
                    sys_spawn_thread(udp_broadcaster_thread, stack_top);
                    self.thread_spawned = true;
                }
            } else {
                self.status_log = "Connection Failed. Try again:".into();
                self.state = NetState::ManualEntry;
            }
        } else {
            self.status_log = "Failed to allocate socket.".into();
        }
    }

    pub fn update(&mut self) -> bool {
        let mut redraw = false;

        if self.state == NetState::Connected {
            if self.casting || self.screenshot_pending { 
                self.transmit_screen_delta(); 
                
                if self.screenshot_pending && self.cast_chunk_index == 0 {
                    self.screenshot_pending = false;
                }
            }

            if let Some(sock) = &self.socket {
                loop {
                    let mut buf = [0u8; 2048];
                    let ret = sock.recv(&mut buf);
                    if ret > 0 {
                        let size = ret as usize;
                        if let Ok(text) = core::str::from_utf8(&buf[..size]) {
                            if !text.trim().is_empty() {
                                self.messages.push(format!("Host: {}", text.trim_end()));
                                redraw = true;
                            }
                        }
                    } else if ret == EAGAIN { break;
                    } else if ret == 0 {
                        self.status_log = "Connection closed.".into();
                        self.state = NetState::ManualEntry;
                        redraw = true; break;
                    } else { break; }
                }
            }
        }
        redraw
    }
    
    fn transmit_screen_delta(&mut self) {
        let (w, h, stride) = crate::syscalls::sys_get_screen_info();
        let fb_addr = crate::syscalls::sys_map_framebuffer();
        if fb_addr == 0 { return; }
        
        let total_bytes = stride * h * 4; 
        let chunk_size = 1400; 
        let total_chunks = ((total_bytes + chunk_size - 1) / chunk_size) as u32;
        
        if self.chunk_hashes.len() < total_chunks as usize {
            self.chunk_hashes.resize(total_chunks as usize, 0);
        }
        
        let fb_ptr = fb_addr as *const u8;
        let is_keyframe = self.keyframe_timer >= 60; 
        let mut scanned_this_tick = 0; 

        if is_keyframe && self.cast_chunk_index == 0 { flush_queue(); }
        
        let scan_limit = 6000;
        
        while self.cast_chunk_index < total_chunks && scanned_this_tick < scan_limit {
            let i = self.cast_chunk_index;
            let offset = (i as usize) * chunk_size;
            let mut size = chunk_size;
            if offset + size > total_bytes { size = total_bytes - offset; }
            
            let slice = unsafe { core::slice::from_raw_parts(fb_ptr.add(offset), size) };
            let hash = quick_hash(slice);
            
            if is_keyframe || hash != self.chunk_hashes[i as usize] {
                self.chunk_hashes[i as usize] = hash; 
                
                let mut packet = [0u8; 1450]; 
                packet[0..4].copy_from_slice(&i.to_le_bytes());                  
                packet[4..8].copy_from_slice(&total_chunks.to_le_bytes());       
                packet[8..12].copy_from_slice(&self.frame_id.to_le_bytes());      
                packet[12..16].copy_from_slice(&(w as u32).to_le_bytes());         
                packet[16..20].copy_from_slice(&(h as u32).to_le_bytes());         
                packet[20..24].copy_from_slice(&(stride as u32).to_le_bytes());    
                
                let total_len = 24 + size;
                packet[24..total_len].copy_from_slice(slice);
                
                enqueue_packet(&packet[..total_len], false);
            }
            
            scanned_this_tick += 1;
            self.cast_chunk_index += 1;
        }
        
        if self.cast_chunk_index >= total_chunks {
            let mut eof_packet = [0u8; 512]; 
            let eof_flag = 0xFFFFFFFFu32; 
            
            eof_packet[0..4].copy_from_slice(&eof_flag.to_le_bytes());
            eof_packet[4..8].copy_from_slice(&total_chunks.to_le_bytes());
            eof_packet[8..12].copy_from_slice(&self.frame_id.to_le_bytes());
            eof_packet[12..16].copy_from_slice(&(w as u32).to_le_bytes());
            eof_packet[16..20].copy_from_slice(&(h as u32).to_le_bytes());
            eof_packet[20..24].copy_from_slice(&(stride as u32).to_le_bytes());
            
            enqueue_packet(&eof_packet, true);

            self.cast_chunk_index = 0;
            self.frame_id = self.frame_id.wrapping_add(1);
            if is_keyframe { self.keyframe_timer = 0; } else { self.keyframe_timer += 1; }
        }
    }

    pub fn draw(&self, fb: &mut [u32], screen_w: usize, screen_h: usize, x: usize, y: usize, w: usize, h: usize) {
        draw::draw_rect(fb, screen_w, screen_h, x, y, w, h, 0xFF181818); 
        draw::draw_rect(fb, screen_w, screen_h, x, y, w, 25, 0xFF2A2A2A);
        draw::draw_text(fb, screen_w, screen_h, x + 10, y + 5, "NyxOS NetChat Terminal", 0xFFFFFFFF);

        let pad_x = x + 10; 
        let mut current_y = y + 35;
        
        draw::draw_text(fb, screen_w, screen_h, pad_x, current_y, &self.status_log, 0xFF00AAFF);
        current_y += 20;

        if self.state == NetState::Connected {
            let stream_text = if self.casting { "[UDP STREAM: LIVE]" } 
                              else if self.screenshot_pending { "[SENDING SCREENSHOT...]" }
                              else { "[UDP STREAM: OFF]" };
                              
            let stream_color = if self.casting || self.screenshot_pending { 0xFFFF3333 } else { 0xFF888888 }; 
            draw::draw_text(fb, screen_w, screen_h, pad_x, current_y, stream_text, stream_color);
            current_y += 20;
        }

        let line_height = 16;
        let max_lines = (h.saturating_sub(110)) / line_height; 
        let skip = if self.messages.len() > max_lines { self.messages.len() - max_lines } else { 0 };

        for msg in self.messages.iter().skip(skip) {
            let color = if msg.starts_with("[System]") { 0xFFFFFF00 } 
                        else if msg.starts_with("You:") { 0xFF00FFFF } 
                        else { 0xFF00FF00 }; 
                        
            draw::draw_text(fb, screen_w, screen_h, pad_x, current_y, msg, color);
            current_y += line_height;
        }

        let input_y = y + h - 25;
        draw::draw_rect(fb, screen_w, screen_h, x + 5, input_y - 5, w - 10, 25, 0xFF222222);
        
        let ticks = crate::syscalls::sys_get_time();
        let display_text = if (ticks % 1000) < 500 { format!("> {}_", self.current_input) } else { format!("> {}", self.current_input) };
        draw::draw_text(fb, screen_w, screen_h, pad_x, input_y, &display_text, 0xFFFFFFFF);
    }
    
    pub fn handle_key(&mut self, c: char) {
        if c == '\n' {
            if !self.current_input.is_empty() {
                if self.state == NetState::ManualEntry {
                    if let Some(ip) = Self::parse_ip(&self.current_input) {
                        self.connect_to(ip[0], ip[1], ip[2], ip[3]);
                    }
                } else if self.state == NetState::Connected {
                    let cmd = self.current_input.trim();
                    
                    if cmd == "/cast" {
                        self.casting = !self.casting;
                        self.cast_chunk_index = 0; 
                        self.keyframe_timer = 60; 
                    } 
                    else if cmd == "/snap" {
                        self.screenshot_pending = true;
                        self.cast_chunk_index = 0;
                        self.keyframe_timer = 60; 
                        self.messages.push("[System] Sending high-res screenshot...".into());
                    }
                    else if cmd == "/disconnect" {
                        if let Some(sock) = self.socket.take() {
                            sys_close(sock.fd); 
                        }
                        SHARED_FD.store(0, Ordering::SeqCst); 
                        self.casting = false;
                        self.screenshot_pending = false;
                        self.state = NetState::ManualEntry;
                        self.status_log = "Disconnected. Enter new Host IP:".into();
                        self.messages.push("[System] Disconnected from host.".into());
                    }
                    else if cmd == "/help" {
                        self.messages.push("[System] Commands: /cast, /snap, /disconnect".into());
                    }
                    else if let Some(sock) = &self.socket {
                        sock.send(format!("{}\n", self.current_input).as_bytes());
                        self.messages.push(format!("You: {}", self.current_input));
                    }
                }
                self.current_input.clear();
            }
        } else if c == '\x08' { self.current_input.pop(); } else { self.current_input.push(c); }
    }
}