#![no_std]
#![no_main]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::Button;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const WIDTH: usize = 650;
const HEIGHT: usize = 450;

// --- NATIVE SYSCALL WRAPPERS FOR VFS ---
#[inline]
unsafe fn syscall_2(id: u64, a: u64, b: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

#[inline]
unsafe fn syscall_3(id: u64, a: u64, b: u64, c: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, in("rdx") c, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

#[inline]
unsafe fn syscall_4(id: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

// --- FILE SYSTEM API ---
fn get_directory_contents(path: &str) -> Vec<String> {
    let mut files = Vec::new();
    let count = unsafe { syscall_2(510, path.as_ptr() as u64, path.len() as u64) };
    for i in 0..count {
        let mut buf = [0u8; 256];
        let len = unsafe { syscall_4(511, i, buf.as_mut_ptr() as u64, path.as_ptr() as u64, path.len() as u64) };
        if len > 0 {
            if let Ok(s) = core::str::from_utf8(&buf[..len as usize]) { files.push(String::from(s)); }
        }
    }
    files
}

fn read_file(path: &str) -> String {
    let fd = unsafe { syscall_2(2, path.as_ptr() as u64, path.len() as u64) } as i64;
    if fd < 0 { return String::from("Error: Could not open file (Directory or Not Found)."); }
    
    let mut buf = vec![0u8; 8192];
    let bytes_read = unsafe { syscall_3(0, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) } as i64;
    unsafe { syscall_2(3, fd as u64, 0); } // Close FD
    
    if bytes_read > 0 {
        String::from_utf8_lossy(&buf[..bytes_read as usize]).into_owned()
    } else {
        String::from("[Empty File]")
    }
}

// --- APP STATE ---
#[derive(PartialEq)]
enum AppState { Explorer, Editor }

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
    header.requested_x = -1; header.requested_y = -1;
    header.width = WIDTH as u32; header.height = HEIGHT as u32;
    header.flags = WIN_FLAG_NONE;
    
    let title = b"Nyx Explorer Suite";
    header.title.fill(0);
    header.title[..title.len()].copy_from_slice(title);

    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) { sys_exit(1); }
    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop { if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED { break; } }

    let pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
    let screen = unsafe { core::slice::from_raw_parts_mut(pixels_ptr, WIDTH * HEIGHT) };
    let mut canvas = Canvas::new(screen, WIDTH, HEIGHT);

    let mut state = AppState::Explorer;
    let mut current_path = String::from("/mnt/nvme/apps"); 
    let mut files = get_directory_contents(&current_path);
    
    // 🚨 NEW: Pagination State
    let mut current_page = 0;
    const ITEMS_PER_PAGE: usize = 24; 
    
    let mut active_file = String::new();
    let mut editor_content = String::new();
    let mut needs_redraw = true;

    loop {
        if needs_redraw {
            canvas.fill_rect(0, 0, WIDTH, HEIGHT, Color::WARM_BG); 
            canvas.fill_rect(0, 0, WIDTH, 50, Color::WARM_SURFACE); 
            canvas.fill_rect(0, 50, WIDTH, 1, Color::WARM_BORDER);

            if state == AppState::Explorer {
                let up_btn = Button { x: 10, y: 10, w: 60, h: 30, text: "Up", is_hovered: false, is_pressed: false };
                up_btn.draw(&mut canvas);

                canvas.fill_rect(80, 10, WIDTH - 340, 30, Color::WHITE);
                canvas.fill_rect(80, 10, WIDTH - 340, 1, Color::WARM_BORDER);
                canvas.print_str(90, 17, &current_path, Color::TEXT_DARK, 1);

                // 🚨 NEW: Draw Pagination Controls
                let total_pages = if files.is_empty() { 1 } else { (files.len() + ITEMS_PER_PAGE - 1) / ITEMS_PER_PAGE };
                if total_pages > 1 {
                    let prev_btn = Button { x: WIDTH - 250, y: 10, w: 30, h: 30, text: "<", is_hovered: false, is_pressed: false };
                    let next_btn = Button { x: WIDTH - 130, y: 10, w: 30, h: 30, text: ">", is_hovered: false, is_pressed: false };
                    prev_btn.draw(&mut canvas);
                    next_btn.draw(&mut canvas);
                    
                    let page_text = alloc::format!("{} / {}", current_page + 1, total_pages);
                    canvas.print_str(WIDTH - 210, 17, &page_text, Color::TEXT_DARK, 1);
                }

                let refresh_btn = Button { x: WIDTH - 90, y: 10, w: 80, h: 30, text: "Refresh", is_hovered: false, is_pressed: false };
                refresh_btn.draw(&mut canvas);

                // 🚨 NEW: Only iterate over the files meant for the current page!
                let start_idx = current_page * ITEMS_PER_PAGE;
                let end_idx = core::cmp::min(start_idx + ITEMS_PER_PAGE, files.len());
                let visible_files = &files[start_idx..end_idx];

                let mut fx = 20; let mut fy = 70;
                if files.is_empty() {
                    canvas.print_str(WIDTH/2 - 50, HEIGHT/2, "Folder is Empty", Color::TEXT_MUTED, 1);
                } else {
                    for file in visible_files.iter() {
                        canvas.fill_rect(fx, fy, 130, 40, Color::WARM_SURFACE); 
                        canvas.fill_rect(fx, fy, 5, 40, Color::ACCENT_PRIMARY); 
                        
                        // Truncate long names so they don't draw outside their box
                        let display_name = if file.len() > 14 { alloc::format!("{}...", &file[..11]) } else { file.clone() };
                        canvas.print_str(fx + 15, fy + 12, &display_name, Color::TEXT_DARK, 1);
                        
                        fx += 150;
                        if fx > WIDTH - 150 { fx = 20; fy += 60; }
                    }
                }
            } 
            else if state == AppState::Editor {
                let back_btn = Button { x: 10, y: 10, w: 70, h: 30, text: "Back", is_hovered: false, is_pressed: false };
                back_btn.draw(&mut canvas);
                let title_str = alloc::format!("Reading: {}{}{}", current_path, if current_path.ends_with('/') {""} else {"/"}, active_file);
                canvas.print_str(95, 17, &title_str, Color::TEXT_DARK, 1);

                canvas.fill_rect(10, 60, WIDTH - 20, HEIGHT - 70, 0xFF_1E1E1E); 
                draw_text_wrapped(&mut canvas, 15, 65, WIDTH - 30, HEIGHT - 80, &editor_content, 0xFF_CCCCCC);
            }

            sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            needs_redraw = false;
        }

        if sys_ipc_recv(&mut msg, true) {
            if msg.msg_type == MSG_WINDOW_CLOSE { sys_exit(0); }
            else if msg.msg_type == MSG_MOUSE_EVENT {
                let mx = msg.data1 as usize; let my = msg.data2 as usize;

                if state == AppState::Explorer {
                    let total_pages = if files.is_empty() { 1 } else { (files.len() + ITEMS_PER_PAGE - 1) / ITEMS_PER_PAGE };

                    if mx >= 10 && mx <= 70 && my >= 10 && my <= 40 {
                        if current_path != "/" {
                            let mut parts: Vec<&str> = current_path.split('/').filter(|s| !s.is_empty()).collect();
                            parts.pop();
                            current_path = if parts.is_empty() { String::from("/") } else { alloc::format!("/{}", parts.join("/")) };
                            files = get_directory_contents(&current_path);
                            current_page = 0; // 🚨 Reset to page 1 on folder change
                            needs_redraw = true;
                        }
                    }
                    else if mx >= WIDTH - 90 && mx <= WIDTH - 10 && my >= 10 && my <= 40 {
                        files = get_directory_contents(&current_path);
                        current_page = 0;
                        needs_redraw = true;
                    } 
                    // 🚨 NEW: Handle Page Navigation Clicks
                    else if total_pages > 1 && mx >= WIDTH - 250 && mx <= WIDTH - 220 && my >= 10 && my <= 40 {
                        if current_page > 0 { current_page -= 1; needs_redraw = true; }
                    }
                    else if total_pages > 1 && mx >= WIDTH - 130 && mx <= WIDTH - 100 && my >= 10 && my <= 40 {
                        if current_page < total_pages - 1 { current_page += 1; needs_redraw = true; }
                    }
                    else {
                        // 🚨 NEW: Map clicks only to the visible files
                        let start_idx = current_page * ITEMS_PER_PAGE;
                        let end_idx = core::cmp::min(start_idx + ITEMS_PER_PAGE, files.len());
                        let visible_files = &files[start_idx..end_idx];

                        let mut fx = 20; let mut fy = 70;
                        for file in visible_files.iter() {
                            if mx >= fx && mx <= fx + 130 && my >= fy && my <= fy + 40 {
                                let target_path = alloc::format!("{}{}{}", current_path, if current_path.ends_with('/') {""} else {"/"}, file);
                                let dir_contents = get_directory_contents(&target_path);
                                
                                if !dir_contents.is_empty() {
                                    current_path = target_path;
                                    files = dir_contents;
                                    current_page = 0; // 🚨 Reset to page 1 on folder change
                                } else {
                                    active_file = file.clone();
                                    editor_content = read_file(&target_path);
                                    state = AppState::Editor;
                                }
                                needs_redraw = true;
                                break;
                            }
                            fx += 150; if fx > WIDTH - 150 { fx = 20; fy += 60; }
                        }
                    }
                } 
                else if state == AppState::Editor {
                    if mx >= 10 && mx <= 80 && my >= 10 && my <= 40 {
                        state = AppState::Explorer;
                        files = get_directory_contents(&current_path); 
                        needs_redraw = true;
                    }
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
        cx += 9; // Noto Sans Width
        if cx > x + w - 9 { cx = x; cy += 16; }
        if cy > y + h - 16 { break; } 
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }