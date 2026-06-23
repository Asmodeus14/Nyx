#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};
// 🚨 FIX 1: Import the Widget trait
use nyx_gui::ui::{Button, Widget};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

#[derive(PartialEq, Clone, Copy)]
enum NetState { Dns, Fetch, Chat, Https, Browser }

#[derive(PartialEq)]
enum AsyncState { Idle, WaitingForData }

// ─────────────────────────────────────────────────────────────────────────
// 1. APP STATE
// ─────────────────────────────────────────────────────────────────────────
struct NetworkSuite {
    state: NetState,
    input_buffer: String,
    log_buffer: String,
    async_status: AsyncState,
    active_fd: i64,
    request_start_time: usize,
    receive_buffer: [u8; 1024],
}

impl NetworkSuite {
    fn new() -> Self {
        Self {
            state: NetState::Dns,
            input_buffer: String::from("nyxos.org"),
            log_buffer: String::from("Welcome to the Nyx Network Suite.\nSystems nominal. Select a module."),
            async_status: AsyncState::Idle,
            active_fd: -1,
            request_start_time: 0,
            receive_buffer: [0; 1024],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 2. COMPONENT FRAMEWORK IMPLEMENTATION
// ─────────────────────────────────────────────────────────────────────────
impl NyxApp for NetworkSuite {
    fn title(&self) -> &str { "Nyx Network Suite" }
    fn initial_width(&self) -> usize { 700 }
    fn initial_height(&self) -> usize { 480 }

    fn update(&mut self) -> bool {
        let mut requested_redraw = false;

        if self.async_status == AsyncState::WaitingForData && self.active_fd >= 0 {
            let res = sys_read(self.active_fd, &mut self.receive_buffer);
            
            if res > 0 {
                sys_close(self.active_fd);
                self.active_fd = -1;
                self.async_status = AsyncState::Idle;
                
                let len = res as usize;
                if self.state == NetState::Dns {
                    if len > 12 && self.receive_buffer[3] & 0x0F == 0 {
                        let ip1 = self.receive_buffer[len - 4]; let ip2 = self.receive_buffer[len - 3];
                        let ip3 = self.receive_buffer[len - 2]; let ip4 = self.receive_buffer[len - 1];
                        self.log_buffer = alloc::format!("Resolved Target Address:\n{}.{}.{}.{}", ip1, ip2, ip3, ip4);
                    } else {
                        self.log_buffer = String::from("DNS Query Failed: Name Server returned error configuration.");
                    }
                } else if self.state == NetState::Fetch || self.state == NetState::Browser {
                    self.log_buffer = String::from_utf8_lossy(&self.receive_buffer[..len]).into_owned();
                }
                requested_redraw = true; 
            } else if res < 0 && res != -11 {
                sys_close(self.active_fd);
                self.active_fd = -1;
                self.async_status = AsyncState::Idle;
                self.log_buffer = alloc::format!("Hardware Socket Fault: Error code {}", res);
                requested_redraw = true; 
            } else if sys_get_time().wrapping_sub(self.request_start_time) > 2500 {
                sys_close(self.active_fd);
                self.active_fd = -1;
                self.async_status = AsyncState::Idle;
                self.log_buffer = String::from("Network Error: Connection timed out.");
                requested_redraw = true; 
            }
        }
        requested_redraw
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let width = canvas.width;
        let height = canvas.height;

        canvas.fill_rect(0, 0, width, height, Color::WARM_BG);
        canvas.fill_rect(0, 0, 150, height, Color::WARM_SURFACE);
        canvas.fill_rect(150, 0, 1, height, Color::WARM_BORDER); 

        canvas.print_str(15, 20, "NET SUITE", Color::ACCENT_PRIMARY, 2);

        let tabs = [
            (NetState::Dns, "DNS Lookup", 80),
            (NetState::Fetch, "HTTP Fetch", 120),
            (NetState::Chat, "NetChat", 160),
            (NetState::Https, "HTTPS Info", 200),
            (NetState::Browser, "Web Browser", 240),
        ];

        for (s, text, y) in tabs.iter() {
            let is_active = self.state == *s;
            if is_active { canvas.fill_rect(10, *y - 5, 130, 30, Color::ACCENT_PRIMARY); }
            let text_color = if is_active { Color::WHITE } else { Color::TEXT_MUTED };
            canvas.print_str(20, *y + 2, text, text_color, 1);
        }

        let cx = 170; let cw = width.saturating_sub(cx + 20);

        match self.state {
            NetState::Dns | NetState::Fetch | NetState::Browser => {
                let title_text = match self.state {
                    NetState::Dns => "Asynchronous DNS Domain Resolver",
                    NetState::Fetch => "HTTP Raw Resource Fetch Engine",
                    _ => "NyxOS Vector Web Browser",
                };
                canvas.print_str(cx, 20, title_text, Color::TEXT_DARK, 1);
                
                canvas.fill_rect(cx, 50, cw.saturating_sub(80), 30, Color::WHITE);
                canvas.fill_rect(cx, 50, cw.saturating_sub(80), 1, Color::WARM_BORDER);
                canvas.print_str(cx + 10, 57, &self.input_buffer, Color::TEXT_DARK, 1);

                let btn_label = if self.async_status == AsyncState::Idle { "EXEC" } else { "WAIT" };
                
                // 🚨 FIX 2: Use String::from() and make the button mut
                let mut go_btn = Button { 
                    x: cx + cw - 70, y: 50, w: 70, h: 30, 
                    text: String::from(btn_label), 
                    is_hovered: false, is_pressed: false 
                };
                go_btn.draw(canvas);

                let log_height = height.saturating_sub(120);
                canvas.fill_rect(cx, 100, cw, log_height, 0xFF_171717); 
                draw_text_wrapped(canvas, cx + 15, 115, cw.saturating_sub(30), log_height.saturating_sub(20), &self.log_buffer, Color::ACCENT_GREEN);
            },
            _ => {
                canvas.print_str(cx, 20, "Module Standby", Color::TEXT_DARK, 2);
                draw_text_wrapped(canvas, cx, 80, cw, height.saturating_sub(100), "This service requires structural adjustments.", Color::TEXT_MUTED);
            }
        }
    }

    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let mut requested_redraw = false;

        // 1. Check for Sidebar Tab Clicks
        if mx < 150 && clicked {
            let old_state = self.state;
            if my >= 75 && my <= 105 { self.state = NetState::Dns; }
            else if my >= 115 && my <= 145 { self.state = NetState::Fetch; }
            else if my >= 155 && my <= 185 { self.state = NetState::Chat; }
            else if my >= 195 && my <= 225 { self.state = NetState::Https; }
            else if my >= 235 && my <= 265 { self.state = NetState::Browser; }
            
            if self.state != old_state {
                self.log_buffer = String::from("Module safe mode switched. Ready.");
                return true; 
            }
        }

        // 2. Check for "EXEC" Button Clicks
        let width = 700; // initial width
        let cx = 170; 
        let cw = width - (cx + 20);
        let btn_x = cx + cw - 70;
        
        if clicked && mx >= btn_x && mx <= btn_x + 70 && my >= 50 && my <= 80 {
            if self.async_status == AsyncState::Idle {
                if self.state == NetState::Dns {
                    // 🔥 CALL THE NEW SYSCALL!
                    self.log_buffer = alloc::format!("Querying DNS for: {} ...", self.input_buffer);
                    
                    if let Some(ip) = nyx_api::sys_dns_resolve(&self.input_buffer) {
                        self.log_buffer = alloc::format!(
                            "SUCCESS!\nHost: {}\nIP Address: {}.{}.{}.{}", 
                            self.input_buffer, ip[0], ip[1], ip[2], ip[3]
                        );
                    } else {
                        self.log_buffer = alloc::format!("FAILED to resolve '{}'.\nCheck connection.", self.input_buffer);
                    }
                    requested_redraw = true;
                }
            }
        }
        
        requested_redraw 
    }

    fn on_key(&mut self, key: char) -> bool {
        if self.async_status == AsyncState::Idle {
            if key == '\x08' { 
                if !self.input_buffer.is_empty() { 
                    self.input_buffer.pop(); 
                    return true; 
                }
            } else if key != '?' && key != '\n' && key != '\r' {
                self.input_buffer.push(key);
                return true;
            }
        }
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 3. SYSTEM ENTRY POINT & HELPERS
// ─────────────────────────────────────────────────────────────────────────
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(256);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 256 * 4096); }

    nyx_gui::app::run(NetworkSuite::new());
}

fn draw_text_wrapped(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, text: &str, color: u32) {
    let mut cx = x; let mut cy = y;
    for c in text.chars() {
        if c == '\n' { cx = x; cy += 16; continue; }
        canvas.draw_char(cx, cy, c, color, 1);
        cx += 9;
        if cx > x + w - 9 { cx = x; cy += 16; }
        if cy > y + h - 16 { break; } 
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }