#![no_std]
#![no_main]
extern crate alloc;

use linked_list_allocator::LockedHeap;
use alloc::vec::Vec;
use alloc::vec;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::{draw_taskbar, draw_window_rounded, draw_cursor, Window};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub struct WindowClient {
    pub win: Window,
    pub owner_pid: u64,
    pub shm_id: u64,
    pub buffer: *const u32, 
}

fn get_str_len(buf: &[u8; 64]) -> usize { buf.iter().position(|&c| c == 0).unwrap_or(64) }

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    const HEAP_PAGES: usize = 4096; 
    let heap_start = sys_alloc_pages(HEAP_PAGES);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, HEAP_PAGES * 4096); }

    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    let fb_ptr = sys_map_framebuffer();
    let hardware_fb = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
    
    let mut back_buffer: Vec<u32> = vec![Color::WARM_BG; screen_stride * screen_h];
    let mut clients: Vec<WindowClient> = Vec::new();
    let mut next_win_id = 0;

    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    let mut last_frame = sys_get_time();
    let ms_per_frame = 1000 / 60; 

    let mut prev_mx = screen_w / 2; let mut prev_my = screen_h / 2;
    let mut dirty_min_x = 0; let mut dirty_min_y = 0;
    let mut dirty_max_x = screen_stride; let mut dirty_max_y = screen_h;
    let mut needs_redraw = true; 

    let mut prev_left = false;
    let mut dragging_win_idx: Option<usize> = None;
    let mut drag_off_x = 0; let mut drag_off_y = 0;

    // 🚨 NEW: Start Menu State
    let mut start_menu_open = false;

    sys_print("[COMPOSITOR] Nyx Window Server Online. (Start Menu Active)\n");

    loop {
        // 1. Process IPC Messages (Unchanged)
        while sys_ipc_recv(&mut msg, false) {
            match msg.msg_type {
                MSG_REQ_WINDOW => {
                    let shm_id = msg.data1;
                    let vaddr = sys_map_shm(shm_id) as *mut u8;
                    let header = unsafe { &*(vaddr as *const WindowHeader) };
                    if header.magic == WIN_MAGIC {
                        let w = header.width as usize; let h = header.height as usize;
                        let x = if header.requested_x == -1 { 100 + (next_win_id as usize * 30) } else { header.requested_x as usize };
                        let y = if header.requested_y == -1 { 100 + (next_win_id as usize * 30) } else { header.requested_y as usize };
                        
                        clients.push(WindowClient {
                            win: Window { id: next_win_id, x, y, w, h, title: header.title, title_len: get_str_len(&header.title), active: true, exists: true, opacity: 0 },
                            owner_pid: msg.sender_pid, shm_id, buffer: unsafe { vaddr.add(core::mem::size_of::<WindowHeader>()) } as *const u32,
                        });
                        next_win_id += 1;
                        
                        // 🚨 THE FIX: Force compositor to redraw the new region instantly!
                        dirty_min_x = 0; dirty_min_y = 0;
                        dirty_max_x = screen_w; dirty_max_y = screen_h;
                        needs_redraw = true;

                        sys_ipc_send(msg.sender_pid, MSG_WINDOW_CREATED, shm_id, 0);
                    }
                },
                MSG_FLUSH_WINDOW => {
                    if let Some(client) = clients.iter_mut().find(|c| c.owner_pid == msg.sender_pid) {
                        dirty_min_x = dirty_min_x.min(client.win.x); dirty_min_y = dirty_min_y.min(client.win.y);
                        dirty_max_x = dirty_max_x.max(client.win.x + client.win.w + 10); dirty_max_y = dirty_max_y.max(client.win.y + client.win.h + 40); 
                        needs_redraw = true;
                    }
                },
                _ => {}
            }
        }

        // 2. Keyboard Routing (Removed 's' and 't' hotkeys per your request!)
        if let Some(key) = sys_read_key() {
            if let Some(top_client) = clients.iter().rev().find(|c| c.win.exists) {
                sys_ipc_send(top_client.owner_pid, MSG_KEY_EVENT, key as u64, 0);
            }
        }

        // 3. Mouse Engine
        let (mx_raw, my_raw, left_click, _right) = sys_get_mouse();
        let mx = mx_raw.clamp(0, screen_w - 1); let my = my_raw.clamp(0, screen_h - 1);

        if mx != prev_mx || my != prev_my {
            let pad = 20;
            dirty_min_x = dirty_min_x.min(prev_mx.saturating_sub(pad)).min(mx.saturating_sub(pad));
            dirty_min_y = dirty_min_y.min(prev_my.saturating_sub(pad)).min(my.saturating_sub(pad));
            dirty_max_x = dirty_max_x.max(prev_mx + pad).max(mx + pad).min(screen_stride);
            dirty_max_y = dirty_max_y.max(prev_my + pad).max(my + pad).min(screen_h);
            needs_redraw = true; 
        }

        if left_click && !prev_left {
            let mut clicked_idx: Option<usize> = None;

            // --- TASKBAR DIMENSIONS ---
            let btn_w = 70; let btn_x = (screen_stride / 2) - (btn_w / 2); let btn_y = screen_h - 36 + 6; 
            let net_x = screen_stride - 50; let net_w = 30;

            let menu_w = 180; let menu_h = 200;
            let menu_x = (screen_stride / 2) - (menu_w / 2); let menu_y = screen_h - 36 - menu_h - 10;

            // 3a. Start Menu Item Click
            if start_menu_open && mx >= menu_x && mx <= menu_x + menu_w && my >= menu_y && my <= menu_y + menu_h {
                let rel_y = my - menu_y;
                
                //  THE FIX: Map clicks to the physical NVMe app bundles!
                if rel_y < 40 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Terminal.nyx/run.bin\0"); sys_exit(1); } }
                else if rel_y < 80 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Settings.nyx/run.bin\0"); sys_exit(1); } }
                else if rel_y < 120 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Explorer.nyx/run.bin\0"); sys_exit(1); } }
                else if rel_y < 160 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Network.nyx/run.bin\0"); sys_exit(1); } }
                else { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/SystemMonitor.nyx/run.bin\0"); sys_exit(1); } }
                
                start_menu_open = false; prev_left = left_click; needs_redraw = true;
                dirty_min_x = 0; dirty_min_y = 0; dirty_max_x = screen_stride; dirty_max_y = screen_h;
                continue;
            }

            // 3b. NYX Button Click (Toggle Menu)
            if mx >= btn_x && mx <= btn_x + btn_w && my >= btn_y && my <= btn_y + 24 {
                start_menu_open = !start_menu_open; prev_left = left_click; needs_redraw = true;
                dirty_min_x = 0; dirty_min_y = 0; dirty_max_x = screen_stride; dirty_max_y = screen_h;
                continue; 
            }

            // 3c. Network Icon Click
            if mx >= net_x && mx <= net_x + net_w && my >= btn_y && my <= btn_y + 24 {
                if sys_fork() == 0 { sys_execve("/bin/nyx-network\0"); sys_exit(1); }
                start_menu_open = false; prev_left = left_click; needs_redraw = true;
                continue; 
            }

            // 3d. Close menu if clicked anywhere else
            if start_menu_open {
                start_menu_open = false; needs_redraw = true;
                dirty_min_x = 0; dirty_min_y = 0; dirty_max_x = screen_stride; dirty_max_y = screen_h;
            }

            // 3e. Standard Window Click Routing
            for (idx, client) in clients.iter_mut().enumerate().rev() {
                if !client.win.exists { continue; }
                let win_x = client.win.x; let win_y = client.win.y; let win_w = client.win.w; let win_h = client.win.h + 30;
                
                if mx >= win_x + 10 && mx <= win_x + 24 && my >= win_y + 10 && my <= win_y + 22 {
                    client.win.exists = false; 
                    sys_ipc_send(client.owner_pid, MSG_WINDOW_CLOSE, 0, 0); 
                    dirty_min_x = dirty_min_x.min(win_x); dirty_min_y = dirty_min_y.min(win_y);
                    dirty_max_x = dirty_max_x.max(win_x + win_w + 10); dirty_max_y = dirty_max_y.max(win_y + win_h + 10); 
                    needs_redraw = true; clicked_idx = Some(idx); break;
                }
                if mx >= win_x && mx <= win_x + win_w && my >= win_y && my <= win_y + 30 {
                    dragging_win_idx = Some(idx); drag_off_x = mx - win_x; drag_off_y = my - win_y; 
                    clicked_idx = Some(idx); break; 
                }
                if mx >= win_x && mx <= win_x + win_w && my > win_y + 30 && my <= win_y + win_h {
                    sys_ipc_send(client.owner_pid, MSG_MOUSE_EVENT, (mx - win_x) as u64, (my - (win_y + 30)) as u64);
                    clicked_idx = Some(idx); break; 
                }
            }

            if let Some(idx) = clicked_idx {
                if idx != clients.len() - 1 {
                    let moved_client = clients.remove(idx);
                    clients.push(moved_client);
                    if dragging_win_idx == Some(idx) { dragging_win_idx = Some(clients.len() - 1); }
                    dirty_min_x = 0; dirty_min_y = 0; dirty_max_x = screen_stride; dirty_max_y = screen_h;
                    needs_redraw = true;
                }
            }
        } else if left_click && dragging_win_idx.is_some() {
            let idx = dragging_win_idx.unwrap();
            let w = clients[idx].win.w + 10; let h = clients[idx].win.h + 40;
            dirty_min_x = dirty_min_x.min(clients[idx].win.x); dirty_min_y = dirty_min_y.min(clients[idx].win.y);
            dirty_max_x = dirty_max_x.max(clients[idx].win.x + w); dirty_max_y = dirty_max_y.max(clients[idx].win.y + h);
            clients[idx].win.x = mx.saturating_sub(drag_off_x); clients[idx].win.y = my.saturating_sub(drag_off_y);
            dirty_min_x = dirty_min_x.min(clients[idx].win.x); dirty_min_y = dirty_min_y.min(clients[idx].win.y);
            dirty_max_x = dirty_max_x.max(clients[idx].win.x + w); dirty_max_y = dirty_max_y.max(clients[idx].win.y + h);
            needs_redraw = true;
        } else if !left_click { dragging_win_idx = None; }
        prev_left = left_click;

        for client in clients.iter_mut() {
            if client.win.exists && client.win.opacity < 255 {
                client.win.opacity = client.win.opacity.saturating_add(15);
                dirty_min_x = dirty_min_x.min(client.win.x); dirty_min_y = dirty_min_y.min(client.win.y);
                dirty_max_x = dirty_max_x.max(client.win.x + client.win.w + 10); dirty_max_y = dirty_max_y.max(client.win.y + client.win.h + 40);
                needs_redraw = true; 
            }
        }

        let now = sys_get_time();
        if !needs_redraw && now.wrapping_sub(last_frame) < ms_per_frame { sys_sleep_ms(2); continue; }
        last_frame = now;

        if needs_redraw {
            dirty_min_x = dirty_min_x.min(mx.saturating_sub(15)); dirty_min_y = dirty_min_y.min(my.saturating_sub(15));
            dirty_max_x = dirty_max_x.max(mx + 20).min(screen_stride); dirty_max_y = dirty_max_y.max(my + 20).min(screen_h);

            for y in dirty_min_y..dirty_max_y {
                for x in dirty_min_x..dirty_max_x { back_buffer[y * screen_stride + x] = Color::WARM_BG; }
            }

            let mut canvas = Canvas::new(&mut back_buffer, screen_stride, screen_h);

            for client in clients.iter() {
                if client.win.exists {
                    draw_window_rounded(canvas.buffer, screen_stride, screen_h, &client.win);
                    if client.buffer.is_null() || client.buffer as u64 == 0 { continue; }
                    let client_pixels = unsafe { core::slice::from_raw_parts(client.buffer, client.win.w * client.win.h) };
                    canvas.composite_buffer(client.win.x, client.win.y + 30, client_pixels, client.win.w, client.win.h, client.win.opacity);
                }
            }

            draw_taskbar(canvas.buffer, screen_stride, screen_h);
            
            // 🚨 NEW: Draw the Taskbar Network Icon
            let net_x = screen_stride - 50; let btn_y = screen_h - 36 + 6;
            canvas.print_str(net_x, btn_y + 4, "[WIFI]", Color::WHITE, 1);

            // 🚨 NEW: Draw the Start Menu Over everything else!
            if start_menu_open {
                let menu_w = 180; let menu_h = 200;
                let menu_x = (screen_stride / 2) - (menu_w / 2); let menu_y = screen_h - 36 - menu_h - 10;
                
                canvas.fill_rect(menu_x, menu_y, menu_w, menu_h, 0xFF_111111); // Dark Menu BG
                canvas.fill_rect(menu_x, menu_y, menu_w, 2, Color::NYX_ORANGE); // Top Accent
                
                canvas.print_str(menu_x + 20, menu_y + 12, "> Terminal", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 52, "> Settings", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 92, "> Explorer", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 132, "> Network Suite", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 172, "> System Monitor", Color::WHITE, 1);
            }

            draw_cursor(canvas.buffer, screen_stride, screen_h, mx, my);

            for y in dirty_min_y..dirty_max_y {
                let start_idx = y * screen_stride + dirty_min_x;
                let end_idx = y * screen_stride + dirty_max_x;
                hardware_fb[start_idx..end_idx].copy_from_slice(&canvas.buffer[start_idx..end_idx]);
            }

            prev_mx = mx; prev_my = my;
            dirty_min_x = screen_stride; dirty_min_y = screen_h; dirty_max_x = 0; dirty_max_y = 0;
            needs_redraw = false;
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(99); }