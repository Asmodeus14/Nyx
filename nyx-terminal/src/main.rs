#![no_std]
#![no_main]

extern crate alloc;
use alloc::string::String;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::canvas::Canvas; 

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const BG_COLOR: u32 = 0xFF0D0D0D; 
const FG_COLOR: u32 = 0xFF00FF66; 
const SCALE: usize = 1;             
const FONT_W: usize = 8 * SCALE;
const FONT_H: usize = 8 * SCALE;
const WIDTH: usize = 640;
const HEIGHT: usize = 400;

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let heap_pages = 256; 
    let heap_start = sys_alloc_pages(heap_pages);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, heap_pages * 4096); }

    // 🚨 FIX 1: Compositor is now PID 3 (0: Idle, 1: Init, 2: Thermal, 3: Compositor)
    const COMPOSITOR_PID: u64 = 4;
    
    // 🚨 FIX 2: Phase 3 Client-Side Allocation & WindowHeader Setup
    let total_size = core::mem::size_of::<WindowHeader>() + (WIDTH * HEIGHT * 4);
    let shm_id = sys_create_shm(total_size);
    if shm_id == 0 { sys_exit(1); }

    let buffer_ptr = sys_map_shm(shm_id) as *mut u8;
    let header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };
    
    header.magic = WIN_MAGIC;
    header.requested_x = -1; // Let Compositor choose
    header.requested_y = -1;
    header.width = WIDTH as u32;
    header.height = HEIGHT as u32;
    header.flags = WIN_FLAG_NONE;
    
    // Set dynamic title
    let title = b"Nyx Matrix Terminal";
    header.title.fill(0);
    header.title[..title.len()].copy_from_slice(title);

    // Send the SHM ID to the Compositor, NOT the Width/Height!
    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) { sys_exit(1); }

    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop { 
        if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED { break; } 
    }

    // 🚨 FIX 3: Shift the pixel pointer safely PAST the WindowHeader!
    let pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
    let screen = unsafe { core::slice::from_raw_parts_mut(pixels_ptr, WIDTH * HEIGHT) };
    let mut canvas = Canvas::new(screen, WIDTH, HEIGHT);

    let mut cursor_x = 10; 
    let mut cursor_y = 10;
    let mut input_buffer = String::new();
    let mut blink_timer = 0;
    let mut cursor_visible = true;

    // Boot Sequence
    canvas.fill_rect(0, 0, WIDTH, HEIGHT, BG_COLOR);
    terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, "NyxOS v0.1 Shell\nN> ");
    canvas.fill_rect(cursor_x, cursor_y, FONT_W, FONT_H, FG_COLOR); 
    sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);

    // The Main Interactive Loop (Non-Blocking for Cursor Blink)
    loop {
        if sys_ipc_recv(&mut msg, false) {
            if msg.msg_type == MSG_WINDOW_CLOSE {
                sys_exit(0);
            }
            if msg.msg_type == MSG_KEY_EVENT {
                let key = core::char::from_u32(msg.data1 as u32).unwrap_or('?');

                cursor_visible = true;
                blink_timer = 0;
                canvas.fill_rect(cursor_x, cursor_y, FONT_W, FONT_H, BG_COLOR); 

                if key == '\n' || key == '\r' {
                    let cmd = input_buffer.trim();
                    terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, "\n");
                    
                    if cmd == "help" {
                        terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, "Commands: help, clear, echo <text>\n");
                    } else if cmd == "clear" {
                        canvas.fill_rect(0, 0, WIDTH, HEIGHT, BG_COLOR);
                        cursor_x = 10;
                        cursor_y = 10;
                    } else if cmd.starts_with("echo ") {
                        let text = &cmd[5..];
                        terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, text);
                        terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, "\n");
                    } else if !cmd.is_empty() {
                        terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, "Unknown command. Type 'help'.\n");
                    }
                    
                    input_buffer.clear();
                    terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, "N> ");

                } else if key == '\x08' { 
                    if !input_buffer.is_empty() {
                        input_buffer.pop();
                        if cursor_x > 10 {
                            cursor_x -= FONT_W;
                            canvas.fill_rect(cursor_x, cursor_y, FONT_W, FONT_H, BG_COLOR);
                        } else if cursor_y > 10 {
                            cursor_y -= FONT_H;
                            cursor_x = WIDTH - 10 - FONT_W;
                            canvas.fill_rect(cursor_x, cursor_y, FONT_W, FONT_H, BG_COLOR);
                        }
                    }
                } else {
                    input_buffer.push(key);
                    let mut tmp_buf = [0; 4];
                    let char_str = key.encode_utf8(&mut tmp_buf);
                    terminal_print(&mut canvas, &mut cursor_x, &mut cursor_y, char_str);
                }

                canvas.fill_rect(cursor_x, cursor_y, FONT_W, FONT_H, FG_COLOR);
                sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            }
        } else {
            // THE BLINK ENGINE
            blink_timer += 1;
            if blink_timer > 30 {
                blink_timer = 0;
                cursor_visible = !cursor_visible;
                
                if cursor_visible {
                    canvas.fill_rect(cursor_x, cursor_y, FONT_W, FONT_H, FG_COLOR);
                } else {
                    canvas.fill_rect(cursor_x, cursor_y, FONT_W, FONT_H, BG_COLOR); 
                }
                
                sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            }
            
            sys_sleep_ms(16);
        }
    }
}

fn terminal_print(canvas: &mut Canvas, cx: &mut usize, cy: &mut usize, text: &str) {
    for c in text.chars() {
        if c == '\n' {
            *cx = 10;
            *cy += FONT_H;
        } else {
            canvas.draw_char(*cx, *cy, c, FG_COLOR, SCALE);
            *cx += FONT_W;
        }
        
        if *cx + FONT_W >= WIDTH - 10 {
            *cx = 10;
            *cy += FONT_H;
        }
        
        if *cy + FONT_H >= HEIGHT - 10 {
            let shift = FONT_H * WIDTH;
            canvas.buffer.copy_within(shift.., 0);
            canvas.fill_rect(0, HEIGHT - FONT_H - 10, WIDTH, FONT_H + 10, BG_COLOR);
            *cy -= FONT_H;
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }