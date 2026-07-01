#![no_std]
#![no_main]
extern crate alloc;

use linked_list_allocator::LockedHeap;
use alloc::vec::Vec;
use alloc::vec;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::{draw_taskbar, draw_window_rounded, draw_cursor, Window, CursorType};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub struct WindowClient {
    pub win: Window,
    pub owner_pid: u64,
    pub shm_id: u64,
    pub buffer: *const u32, 
    pub buf_w: usize,
    pub buf_h: usize,
    pub gpu_gva: u32,
}

fn get_str_len(buf: &[u8; 64]) -> usize { buf.iter().position(|&c| c == 0).unwrap_or(64) }

pub struct CompositorState {
    pub clients: Vec<WindowClient>,
    pub next_win_id: usize,

    pub mx: usize, pub my: usize,
    pub prev_mx: usize, pub prev_my: usize,
    pub left_click: bool, pub prev_left: bool,

    pub dirty_min_x: usize, pub dirty_min_y: usize,
    pub dirty_max_x: usize, pub dirty_max_y: usize,
    pub needs_redraw: bool,

    pub dragging_win_idx: Option<usize>,
    pub drag_off_x: usize, pub drag_off_y: usize,
    
    pub is_resizing: bool,
    pub resizing_win_idx: Option<usize>,

    pub start_menu_open: bool,
    pub screen_w: usize, pub screen_h: usize, pub screen_stride: usize,
}

impl CompositorState {
    pub fn new(w: usize, h: usize, stride: usize) -> Self {
        Self {
            clients: Vec::new(), next_win_id: 0,
            mx: w / 2, my: h / 2, prev_mx: w / 2, prev_my: h / 2,
            left_click: false, prev_left: false,
            dirty_min_x: 0, dirty_min_y: 0, dirty_max_x: stride, dirty_max_y: h,
            needs_redraw: true,
            dragging_win_idx: None, drag_off_x: 0, drag_off_y: 0,
            is_resizing: false, resizing_win_idx: None,
            start_menu_open: false,
            screen_w: w, screen_h: h, screen_stride: stride,
        }
    }

    pub fn mark_dirty(&mut self, x: usize, y: usize, w: usize, h: usize) {
        self.dirty_min_x = self.dirty_min_x.min(x);
        self.dirty_min_y = self.dirty_min_y.min(y);
        self.dirty_max_x = self.dirty_max_x.max(x + w).min(self.screen_stride);
        self.dirty_max_y = self.dirty_max_y.max(y + h).min(self.screen_h);
        self.needs_redraw = true;
    }

    pub fn mark_full_redraw(&mut self) {
        self.dirty_min_x = 0; self.dirty_min_y = 0;
        self.dirty_max_x = self.screen_stride; self.dirty_max_y = self.screen_h;
        self.needs_redraw = true;
    }

    pub fn process_ipc(&mut self) {
        let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
        while sys_ipc_recv(&mut msg, false) {
            match msg.msg_type {
                MSG_REQ_WINDOW => {
                    let shm_id = msg.data1;
                    let vaddr = sys_map_shm(shm_id) as *mut u8;
                    let header = unsafe { &*(vaddr as *const WindowHeader) };
                    if header.magic == WIN_MAGIC {
                        let w = header.width as usize; let h = header.height as usize;
                        let x = if header.requested_x == -1 { 100 + (self.next_win_id * 30) } else { header.requested_x as usize };
                        let y = if header.requested_y == -1 { 100 + (self.next_win_id * 30) } else { header.requested_y as usize };
                        
                        let gpu_gva = 0x2000_0000 + (self.next_win_id * 0x0100_0000) as u32;
                        sys_gpu_map_shm(shm_id, gpu_gva);

                        self.clients.push(WindowClient {
                            win: Window { 
                                id: self.next_win_id, x, y, w, h, 
                                title: header.title, title_len: get_str_len(&header.title), 
                                active: true, exists: true, opacity: 0,
                                is_minimized: false, is_maximized: false,
                                saved_x: 0, saved_y: 0, saved_w: 0, saved_h: 0
                            },
                            owner_pid: msg.sender_pid, shm_id, buffer: unsafe { vaddr.add(core::mem::size_of::<WindowHeader>()) } as *const u32,
                            buf_w: w, buf_h: h,
                            gpu_gva,
                        });
                        self.next_win_id += 1;
                        self.mark_full_redraw();
                        sys_ipc_send(msg.sender_pid, MSG_WINDOW_CREATED, shm_id, 0);
                    }
                },
                MSG_WINDOW_UPDATE_SHM => {
                    let new_shm_id = msg.data1;
                    if let Some(client) = self.clients.iter_mut().find(|c| c.owner_pid == msg.sender_pid) {
                        let vaddr = sys_map_shm(new_shm_id) as *mut u8;
                        let header = unsafe { &*(vaddr as *const WindowHeader) };
                        client.shm_id = new_shm_id;
                        client.buffer = unsafe { vaddr.add(core::mem::size_of::<WindowHeader>()) } as *const u32;
                        client.buf_w = header.width as usize;
                        client.buf_h = header.height as usize;
                        
                        // Re-map the new SHM pages to the SAME GVA!
                        sys_gpu_map_shm(new_shm_id, client.gpu_gva);
                        self.mark_full_redraw();
                    }
                },
                MSG_FLUSH_WINDOW => {
                    let dirty_rect = self.clients.iter()
                        .find(|c| c.owner_pid == msg.sender_pid)
                        .map(|c| (c.win.x, c.win.y, c.win.w + 15, c.win.h + 45));
                    if let Some((x, y, w, h)) = dirty_rect { self.mark_dirty(x, y, w, h); }
                },
                _ => {}
            }
        }
    }

    pub fn process_input(&mut self) {
        if let Some(key) = sys_read_key() {
            if let Some(top_client) = self.clients.iter().rev().find(|c| c.win.exists && !c.win.is_minimized) {
                sys_ipc_send(top_client.owner_pid, MSG_KEY_EVENT, key as u64, 0);
            }
        }

        let (mx_raw, my_raw, left_click, _right) = sys_get_mouse();
        self.mx = mx_raw.clamp(0, self.screen_w - 1); 
        self.my = my_raw.clamp(0, self.screen_h - 1);
        self.left_click = left_click;

        if self.mx != self.prev_mx || self.my != self.prev_my {
            let pad = 20;
            self.mark_dirty(self.prev_mx.saturating_sub(pad), self.prev_my.saturating_sub(pad), pad * 2, pad * 2);
            self.mark_dirty(self.mx.saturating_sub(pad), self.my.saturating_sub(pad), pad * 2, pad * 2);
        }

        if self.left_click && !self.prev_left {
            let mut clicked_idx: Option<usize> = None;

            let btn_w = 70; let btn_x = (self.screen_stride / 2) - 35; let btn_y = self.screen_h - 36 + 6; 
            let net_x = self.screen_stride - 50; let net_w = 30;
            let menu_w = 180; let menu_h = 200;
            let menu_x = (self.screen_stride / 2) - (menu_w / 2); let menu_y = self.screen_h - 36 - menu_h - 10;

            if self.start_menu_open && self.mx >= menu_x && self.mx <= menu_x + menu_w && self.my >= menu_y && self.my <= menu_y + menu_h {
                let rel_y = self.my - menu_y;
                if rel_y < 40 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Terminal.nyx/run.bin\0"); sys_exit(1); } }
                else if rel_y < 80 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Settings.nyx/run.bin\0"); sys_exit(1); } }
                else if rel_y < 120 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Explorer.nyx/run.bin\0"); sys_exit(1); } }
                else if rel_y < 160 { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Network.nyx/run.bin\0"); sys_exit(1); } }
                else { if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/SystemMonitor.nyx/run.bin\0"); sys_exit(1); } }
                
                self.start_menu_open = false; 
                self.mark_full_redraw();
            } 
            else if self.mx >= btn_x && self.mx <= btn_x + btn_w && self.my >= btn_y && self.my <= btn_y + 24 {
                self.start_menu_open = !self.start_menu_open; 
                self.mark_full_redraw();
            }
            else if self.mx >= net_x && self.mx <= net_x + net_w && self.my >= btn_y && self.my <= btn_y + 24 {
                if sys_fork() == 0 { sys_execve("/bin/nyx-network\0"); sys_exit(1); }
                self.start_menu_open = false; 
                self.mark_full_redraw();
            } else {
                if self.start_menu_open { self.start_menu_open = false; self.mark_full_redraw(); }

                for (idx, client) in self.clients.iter_mut().enumerate().rev() {
                    if !client.win.exists { continue; }
                    let win_x = client.win.x; let win_y = client.win.y; let win_w = client.win.w; 
                    let win_h = if client.win.is_minimized { 30 } else { client.win.h + 30 };

                    if !client.win.is_minimized && !client.win.is_maximized && 
                       self.mx >= win_x + win_w - 15 && self.mx <= win_x + win_w && 
                       self.my >= win_y + win_h - 15 && self.my <= win_y + win_h {
                        self.is_resizing = true;
                        self.resizing_win_idx = Some(idx);
                        clicked_idx = Some(idx); break;
                    }

                    if self.mx >= win_x + 12 && self.mx <= win_x + 24 && self.my >= win_y + 10 && self.my <= win_y + 22 {
                        client.win.exists = false; 
                        sys_ipc_send(client.owner_pid, MSG_WINDOW_CLOSE, 0, 0); 
                        self.mark_dirty(win_x, win_y, win_w + 15, win_h + 15);
                        clicked_idx = Some(idx); break;
                    }

                    if self.mx >= win_x + 28 && self.mx <= win_x + 40 && self.my >= win_y + 10 && self.my <= win_y + 22 {
                        client.win.is_minimized = !client.win.is_minimized;
                        let (w, h) = (client.win.w, client.win.h);
                        self.mark_dirty(win_x, win_y, w + 15, h + 45); 
                        clicked_idx = Some(idx); break;
                    }

                    if self.mx >= win_x + 44 && self.mx <= win_x + 56 && self.my >= win_y + 10 && self.my <= win_y + 22 {
                        if client.win.is_maximized {
                            client.win.x = client.win.saved_x; client.win.y = client.win.saved_y;
                            client.win.w = client.win.saved_w; client.win.h = client.win.saved_h;
                            client.win.is_maximized = false;
                        } else {
                            client.win.saved_x = client.win.x; client.win.saved_y = client.win.y;
                            client.win.saved_w = client.win.w; client.win.saved_h = client.win.h;
                            client.win.x = 0; client.win.y = 0;
                            client.win.w = self.screen_w; client.win.h = self.screen_h - 36 - 30;
                            client.win.is_maximized = true;
                        }
                        sys_ipc_send(client.owner_pid, MSG_WINDOW_RESIZED, client.win.w as u64, client.win.h as u64);
                        self.mark_full_redraw();
                        clicked_idx = Some(idx); break;
                    }

                    if self.mx >= win_x && self.mx <= win_x + win_w && self.my >= win_y && self.my <= win_y + 30 {
                        if !client.win.is_maximized {
                            self.dragging_win_idx = Some(idx); 
                            self.drag_off_x = self.mx - win_x; 
                            self.drag_off_y = self.my - win_y; 
                        }
                        clicked_idx = Some(idx); break; 
                    }
                    
                    if !client.win.is_minimized && self.mx >= win_x && self.mx <= win_x + win_w && self.my > win_y + 30 && self.my <= win_y + win_h {
                        sys_ipc_send(client.owner_pid, MSG_MOUSE_EVENT, (self.mx - win_x) as u64, (self.my - (win_y + 30)) as u64);
                        clicked_idx = Some(idx); break; 
                    }
                }

                if let Some(idx) = clicked_idx {
                    if idx != self.clients.len() - 1 {
                        let moved_client = self.clients.remove(idx);
                        self.clients.push(moved_client);
                        if self.dragging_win_idx == Some(idx) { self.dragging_win_idx = Some(self.clients.len() - 1); }
                        if self.resizing_win_idx == Some(idx) { self.resizing_win_idx = Some(self.clients.len() - 1); }
                        self.mark_full_redraw();
                    }
                }
            }
        } else if self.left_click {
            if let Some(idx) = self.resizing_win_idx {
                let w = self.clients[idx].win.w + 15; let h = self.clients[idx].win.h + 45;
                self.mark_dirty(self.clients[idx].win.x, self.clients[idx].win.y, w, h);
                
                let new_w = self.mx.saturating_sub(self.clients[idx].win.x).max(200); 
                let new_h = self.my.saturating_sub(self.clients[idx].win.y + 30).max(100); 
                
                if new_w != self.clients[idx].win.w || new_h != self.clients[idx].win.h {
                    self.clients[idx].win.w = new_w;
                    self.clients[idx].win.h = new_h;
                    sys_ipc_send(self.clients[idx].owner_pid, MSG_WINDOW_RESIZED, new_w as u64, new_h as u64);
                }
                
                self.mark_dirty(self.clients[idx].win.x, self.clients[idx].win.y, new_w + 15, new_h + 45);
            } else if let Some(idx) = self.dragging_win_idx {
                let w = self.clients[idx].win.w + 15; let h = self.clients[idx].win.h + 45;
                self.mark_dirty(self.clients[idx].win.x, self.clients[idx].win.y, w, h);
                
                self.clients[idx].win.x = self.mx.saturating_sub(self.drag_off_x); 
                self.clients[idx].win.y = self.my.saturating_sub(self.drag_off_y);
                
                self.mark_dirty(self.clients[idx].win.x, self.clients[idx].win.y, w, h);
            }
        } else if !self.left_click { 
            self.dragging_win_idx = None; 
            self.resizing_win_idx = None;
            self.is_resizing = false;
        }
        
        self.prev_left = self.left_click;
    }

    pub fn update(&mut self) {
        for i in 0..self.clients.len() {
            if self.clients[i].win.exists && self.clients[i].win.opacity < 255 {
                self.clients[i].win.opacity = self.clients[i].win.opacity.saturating_add(15);
                let (x, y, w, h) = (
                    self.clients[i].win.x, self.clients[i].win.y, 
                    self.clients[i].win.w + 15, self.clients[i].win.h + 45
                );
                self.mark_dirty(x, y, w, h);
            }
        }
    }
}

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
    
    let mut state = CompositorState::new(screen_w, screen_h, screen_stride);

    let mut last_frame = sys_get_time();
    let ms_per_frame = 1000 / 60; 

    sys_print("[COMPOSITOR] Nyx Window Server Online. (Floating WM Restored)\n");

    loop {
        state.process_ipc();
        state.process_input();
        state.update();

        let now = sys_get_time();
        if !state.needs_redraw && now.wrapping_sub(last_frame) < ms_per_frame { 
            sys_sleep_ms(2); 
            continue; 
        }
        last_frame = now;

        if state.needs_redraw {
            state.mark_dirty(state.mx.saturating_sub(15), state.my.saturating_sub(15), 35, 35);

            // 1. Submit GPU background fill for the ENTIRE screen (Asynchronous)
            sys_gpu_fill_rect(0, 0, screen_stride, screen_h, Color::WARM_BG);

            // 2. Synchronize! Wait for GPU wallpaper clear to finish before CPU starts drawing
            sys_gpu_sync();

            // 3. Perform CPU drawing (Text, Window Borders, Windows, Taskbar, Cursor)
            let mut canvas = Canvas::new(hardware_fb, screen_stride, screen_h);

            // Draw window decorations and CPU client compositing sequentially in Z-order
            for client in state.clients.iter() {
                if client.win.exists {
                    // Draw window border, white background, and title bar
                    draw_window_rounded(canvas.buffer, screen_stride, screen_h, &client.win);
                    
                    if !client.win.is_minimized {
                        if client.buffer.is_null() || client.buffer as u64 == 0 { continue; }
                        
                        let expected_size = client.buf_w * client.buf_h;
                        let client_pixels = unsafe { core::slice::from_raw_parts(client.buffer, expected_size) };
                        canvas.composite_buffer(client.win.x, client.win.y + 30, client_pixels, client.buf_w, client.buf_h, client.win.opacity);
                    }
                }
            }

            // 5. Draw Taskbar on top of windows (CPU-based fills and text)
            let bar_h = 36;
            let start_y = screen_h - bar_h;
            canvas.fill_rect(0, start_y, screen_stride, bar_h, 0xFF_FFFFFF); // Opaque white taskbar
            canvas.fill_rect(0, start_y, screen_stride, 1, 0xFF_D1D1D1);     // Border
            
            // Draw Start Button
            let btn_x = (screen_stride / 2) - 35;
            canvas.fill_rect(btn_x, start_y + 6, 70, 24, Color::ACCENT_PRIMARY);

            // Draw taskbar text
            canvas.print_str(20, start_y + 14, "10:20 AM", Color::TEXT_DARK, 1);
            canvas.print_str(btn_x + 15, start_y + 8, "NYX", Color::WHITE, 1);
            
            let net_x = screen_stride - 50; let btn_y = screen_h - 36 + 6;
            canvas.print_str(net_x, btn_y + 4, "[WIFI]", Color::WHITE, 1);

            // Draw Start Menu on top of windows
            if state.start_menu_open {
                let menu_w = 180; let menu_h = 200;
                let menu_x = (screen_stride / 2) - (menu_w / 2); let menu_y = screen_h - 36 - menu_h - 10;
                
                canvas.fill_rect(menu_x, menu_y, menu_w, menu_h, 0xFF_111111);
                canvas.fill_rect(menu_x, menu_y, menu_w, 2, Color::NYX_ORANGE);
                
                canvas.print_str(menu_x + 20, menu_y + 12, "> Terminal", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 52, "> Settings", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 92, "> Explorer", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 132, "> Network Suite", Color::WHITE, 1);
                canvas.print_str(menu_x + 20, menu_y + 172, "> System Monitor", Color::WHITE, 1);
            }

            draw_cursor(canvas.buffer, screen_stride, screen_h, state.mx, state.my, CursorType::Arrow);

            sys_swap_buffers();
            sys_gpu_sync();

            state.prev_mx = state.mx; 
            state.prev_my = state.my;
            state.dirty_min_x = screen_stride; state.dirty_min_y = screen_h; 
            state.dirty_max_x = 0; state.dirty_max_y = 0;
            state.needs_redraw = false;
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(99); }