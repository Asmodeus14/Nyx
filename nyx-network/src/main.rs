#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::Button;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const WIDTH: usize = 700;
const HEIGHT: usize = 480;

#[derive(PartialEq,Clone, Copy)]
enum NetState { Dns, Fetch, Chat, Https, Browser }

#[derive(PartialEq)]
enum AsyncState { Idle, WaitingForData }

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(256);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 256 * 4096); }

    const COMPOSITOR_PID: u64 = 4;
    let total_size = core::mem::size_of::<WindowHeader>() + (WIDTH * HEIGHT * 4);
    let shm_id = sys_create_shm(total_size);
    if shm_id == 0 { sys_exit(1); }

    let buffer_ptr = sys_map_shm(shm_id) as *mut u8;
    let header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };

    header.magic = WIN_MAGIC;
    header.requested_x = -1; // Cascades smoothly now
    header.requested_y = -1;
    header.width = WIDTH as u32;
    header.height = HEIGHT as u32;
    header.flags = WIN_FLAG_NONE;

    let title = b"Nyx Network Suite";
    header.title.fill(0);
    header.title[..title.len()].copy_from_slice(title);

    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) { sys_exit(1); }
    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop { if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED { break; } }

    let pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
    let screen = unsafe { core::slice::from_raw_parts_mut(pixels_ptr, WIDTH * HEIGHT) };
    let mut canvas = Canvas::new(screen, WIDTH, HEIGHT);

    let mut state = NetState::Dns;
    let mut needs_redraw = true;

    // --- State Machines & Containers ---
    let mut input_buffer = String::from("nyxos.org");
    let mut log_buffer = String::from("Welcome to the Nyx Network Suite.\nSystems nominal. Select a module.");
    
    let mut async_status = AsyncState::Idle;
    let mut active_fd: i64 = -1;
    let mut request_start_time: usize = 0;
    let mut receive_buffer = [0u8; 1024];

    loop {
        // 1. Asynchronous Non-Blocking Network Driver Polling Hook
        if async_status == AsyncState::WaitingForData && active_fd >= 0 {
            let res = sys_read(active_fd, &mut receive_buffer);
            
            if res > 0 {
                sys_close(active_fd);
                active_fd = -1;
                async_status = AsyncState::Idle;
                
                let len = res as usize;
                if state == NetState::Dns {
                    // Extract safe payload data out of the raw response
                    if len > 12 && receive_buffer[3] & 0x0F == 0 {
                        let ip1 = receive_buffer[len - 4]; let ip2 = receive_buffer[len - 3];
                        let ip3 = receive_buffer[len - 2]; let ip4 = receive_buffer[len - 1];
                        log_buffer = alloc::format!("Resolved Target Address:\n{}.{}.{}.{}", ip1, ip2, ip3, ip4);
                    } else {
                        log_buffer = String::from("DNS Query Failed: Name Server returned error configuration.");
                    }
                } else if state == NetState::Fetch || state == NetState::Browser {
                    log_buffer = String::from_utf8_lossy(&receive_buffer[..len]).into_owned();
                }
                needs_redraw = true;
            } 
            else if res < 0 && res != -11 {
                sys_close(active_fd);
                active_fd = -1;
                async_status = AsyncState::Idle;
                log_buffer = alloc::format!("Hardware Socket Fault: Interrupt channel returned error code {}", res);
                needs_redraw = true;
            } 
            else if sys_get_time().wrapping_sub(request_start_time) > 2500 {
                sys_close(active_fd);
                active_fd = -1;
                async_status = AsyncState::Idle;
                log_buffer = String::from("Network Error: Connection timed out (Remote host dropped packet).");
                needs_redraw = true;
            }
        }

        // 2. GUI UI Rendering Pass
        if needs_redraw {
            canvas.fill_rect(0, 0, WIDTH, HEIGHT, Color::WARM_BG);
            canvas.fill_rect(0, 0, 150, HEIGHT, Color::WARM_SURFACE);
            canvas.fill_rect(150, 0, 1, HEIGHT, Color::WARM_BORDER); 

            canvas.print_str(15, 20, "NET SUITE", Color::ACCENT_PRIMARY, 2);

            let tabs = [
                (NetState::Dns, "DNS Lookup", 80),
                (NetState::Fetch, "HTTP Fetch", 120),
                (NetState::Chat, "NetChat", 160),
                (NetState::Https, "HTTPS Info", 200),
                (NetState::Browser, "Web Browser", 240),
            ];

            for (s, text, y) in tabs.iter() {
                let is_active = state == *s;
                if is_active { canvas.fill_rect(10, *y - 5, 130, 30, Color::ACCENT_PRIMARY); }
                let text_color = if is_active { Color::WHITE } else { Color::TEXT_MUTED };
                canvas.print_str(20, *y + 2, text, text_color, 1);
            }

            let cx = 170; let cw = WIDTH - cx - 20;

            // Draw Sub-Module Views dynamically
            match state {
                NetState::Dns | NetState::Fetch | NetState::Browser => {
                    let title_text = match state {
                        NetState::Dns => "Asynchronous DNS Domain Resolver",
                        NetState::Fetch => "HTTP Raw Resource Fetch Engine",
                        _ => "NyxOS Vector Web Browser",
                    };
                    canvas.print_str(cx, 20, title_text, Color::TEXT_DARK, 1);
                    
                    canvas.fill_rect(cx, 50, cw - 80, 30, Color::WHITE);
                    canvas.fill_rect(cx, 50, cw - 80, 1, Color::WARM_BORDER);
                    canvas.print_str(cx + 10, 57, &input_buffer, Color::TEXT_DARK, 1);

                    let btn_label = if async_status == AsyncState::Idle { "EXEC" } else { "WAIT" };
                    let go_btn = Button { x: cx + cw - 70, y: 50, w: 70, h: 30, text: btn_label, is_hovered: false, is_pressed: false };
                    go_btn.draw(&mut canvas);

                    canvas.fill_rect(cx, 100, cw, HEIGHT - 120, 0xFF_171717); 
                    draw_text_wrapped(&mut canvas, cx + 15, 115, cw - 30, HEIGHT - 140, &log_buffer, Color::ACCENT_GREEN);
                },
                _ => {
                    canvas.print_str(cx, 20, "Module Standby", Color::TEXT_DARK, 2);
                    draw_text_wrapped(&mut canvas, cx, 80, cw, HEIGHT - 100, "This service is initializing or requires structural loop adjustments.", Color::TEXT_MUTED);
                }
            }

            sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            needs_redraw = false;
        }

        // 3. True Non-blocking Event Polling Loop
        let wait_for_ipc = async_status == AsyncState::Idle;
        if sys_ipc_recv(&mut msg, wait_for_ipc) {
            if msg.msg_type == MSG_WINDOW_CLOSE { sys_exit(0); }
            
            else if msg.msg_type == MSG_MOUSE_EVENT {
                let mx = msg.data1 as usize; let my = msg.data2 as usize;

                if mx < 150 {
                    let old_state = state;
                    if my >= 75 && my <= 105 { state = NetState::Dns; }
                    else if my >= 115 && my <= 145 { state = NetState::Fetch; }
                    else if my >= 155 && my <= 185 { state = NetState::Chat; }
                    else if my >= 195 && my <= 225 { state = NetState::Https; }
                    else if my >= 235 && my <= 265 { state = NetState::Browser; }
                    
                    if state != old_state {
                        log_buffer = String::from("Module safe mode switched. Ready.");
                        needs_redraw = true;
                    }
                }

                // Handle Execution Clicks (GO / EXEC)
                let cx = 170; let cw = WIDTH - cx - 20;
                if mx >= cx + cw - 70 && mx <= cx + cw && my >= 50 && my <= 80 {
                    if async_status == AsyncState::Idle && !input_buffer.is_empty() {
                        
                        if state == NetState::Dns {
                            log_buffer = alloc::format!("Resolving address vector for {}...", input_buffer);
                            active_fd = sys_socket(2, 2, 0); // UDP
                            if active_fd >= 0 {
                                let addr = sockaddr_in { sin_family: 2, sin_port: 53u16.to_be(), sin_addr: [1, 1, 1, 1], sin_zero: [0; 8] };
                                sys_connect(active_fd, &addr as *const _ as *const u8, core::mem::size_of::<sockaddr_in>());
                                
                                let mut packet = Vec::new();
                                packet.extend_from_slice(&[0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
                                for part in input_buffer.split('.') {
                                    packet.push(part.len() as u8); packet.extend_from_slice(part.as_bytes());
                                }
                                packet.push(0); packet.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
                                
                                sys_write(active_fd, &packet);
                                request_start_time = sys_get_time();
                                async_status = AsyncState::WaitingForData;
                            }
                        } 
                        else if state == NetState::Fetch || state == NetState::Browser {
                            log_buffer = alloc::format!("Opening TCP channel stream to {}...", input_buffer);
                            active_fd = sys_socket(2, 1, 0); // TCP
                            if active_fd >= 0 {
                                let addr = sockaddr_in { sin_family: 2, sin_port: 80u16.to_be(), sin_addr: [1, 1, 1, 1], sin_zero: [0; 8] };
                                sys_connect(active_fd, &addr as *const _ as *const u8, core::mem::size_of::<sockaddr_in>());
                                
                                let req = alloc::format!("GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", input_buffer);
                                sys_write(active_fd, req.as_bytes());
                                request_start_time = sys_get_time();
                                async_status = AsyncState::WaitingForData;
                            }
                        }
                        needs_redraw = true;
                    }
                }
            }
            
            else if msg.msg_type == MSG_KEY_EVENT && async_status == AsyncState::Idle {
                let key = core::char::from_u32(msg.data1 as u32).unwrap_or('?');
                if key == '\x08' { 
                    if !input_buffer.is_empty() { input_buffer.pop(); needs_redraw = true; }
                } else if key != '?' && key != '\n' && key != '\r' {
                    input_buffer.push(key);
                    needs_redraw = true;
                }
            }
        }
    }
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