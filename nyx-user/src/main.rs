#![no_std]
#![no_main]
extern crate alloc;

use linked_list_allocator::LockedHeap;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;

mod syscalls;
mod console;
mod apps;
mod gfx;

use syscalls::*;
use apps::terminal::Terminal;
use apps::clock::Clock;
use apps::editor::Editor;
use apps::explorer::Explorer;
use apps::monitor::SysMonitor;
use apps::sysinfo::SysInfoApp;
use apps::bootlog::BootlogApp;
use apps::netchat::NetChatApp;
use apps::browser::BrowserApp;
use gfx::draw;
use gfx::ui::{draw_taskbar, draw_window_rounded, draw_cursor, Window, TASKBAR_H};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();
const MAX_WINDOWS: usize = 9;

// --- HARDWARE ACCELERATION SYSCALL WRAPPERS ---
pub fn sys_fill_rect(x: usize, y: usize, w: usize, h: usize, color_idx: usize) {
    syscall(501, x as u64, y as u64, w as u64, h as u64, color_idx as u64, 0);
}

pub fn sys_swap_buffers() {
    syscall(502, 0, 0, 0, 0, 0, 0);
}

pub fn sys_gpu_sync() {
    syscall(503, 0, 0, 0, 0, 0, 0);
}

pub fn sys_map_gpu_backbuffer() -> u64 {
    syscall(509, 0, 0, 0, 0, 0, 0)
}

pub fn sys_wait_vsync() {
    syscall(513, 0, 0, 0, 0, 0, 0);
}

// --- NEW: POWER MANAGEMENT YIELD ---
pub fn sys_sleep_ms(ms: u64) {
    syscall(525, ms, 0, 0, 0, 0, 0);
}
// ----------------------------------------------

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    const HEAP_PAGES: usize = 4096;
    let heap_start = sys_alloc_pages(HEAP_PAGES);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as usize, HEAP_PAGES * 4096); }

    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    if screen_w == 0 || screen_h == 0 { sys_exit(2); }
    
    let fb_ptr = sys_map_framebuffer();
    if fb_ptr == 0 { sys_exit(3); }

    // Clear the physical screen to dark gray on boot to erase kernel logs
    let hardware_fb = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
    hardware_fb.fill(0xFF1E1E1E);

    let mut back_buffer: Vec<u32> = vec![0xFF000000; screen_stride * screen_h];
    
    let mut windows = [
        Window { id: 0, x: 50, y: 50, w: 760, h: 480, title: "Nyx Terminal", active: true, exists: true },
        Window { id: 1, x: 880, y: 50, w: 300, h: 200, title: "Sys Monitor", active: false, exists: true },
        Window { id: 2, x: 150, y: 150, w: 300, h: 200, title: "Help", active: false, exists: false },
        Window { id: 3, x: 200, y: 100, w: 600, h: 450, title: "NyxPad", active: false, exists: false },
        Window { id: 4, x: 150, y: 150, w: 500, h: 400, title: "File Explorer", active: false, exists: false },
        Window { id: 5, x: 250, y: 150, w: 400, h: 250, title: "Hardware Profile", active: false, exists: false },
        Window { id: 6, x: 100, y: 100, w: 800, h: 800, title: "Kernel Boot Logs", active: false, exists: true },
        Window { id: 7, x: 300, y: 150, w: 450, h: 350, title: "NetChat", active: false, exists: false },
        Window { id: 8, x: 150, y: 100, w: 850, h: 650, title: "NyxBrowser", active: false, exists: false },
    ];
    let mut z_order = [0, 1, 2, 3, 4, 5, 6, 7, 8];
    
    let mut my_terminal = Terminal::new();
    my_terminal.write_str("NyxOS Shell v0.6\nType 'ls' to list files.\n> ");
    let mut my_editor = Editor::new();
    let mut my_explorer = Explorer::new();
    let mut my_monitor = SysMonitor::new();
    let mut my_sysinfo = SysInfoApp::new();
    let mut my_bootlog = BootlogApp::new();
    let mut my_netchat = NetChatApp::new();
    my_netchat.init();
    let mut my_browser = BrowserApp::new(); 

    let mut show_start_menu = false;
    let mut is_dragging = false; let mut is_resizing = false;
    let mut target_idx = 0;
    let mut drag_off_x = 0; let mut drag_off_y = 0;
    let mut prev_left = false; 
    let mut prev_mx = 0; let mut prev_my = 0;
    
    let mut last_frame = sys_get_time();
    let mut last_second = sys_get_time() / 1000;
    let ms_per_frame = 1000 / 60; // 60 FPS Target
    
    let mut dirty_min_x = 0; let mut dirty_min_y = 0;
    let mut dirty_max_x = screen_stride; let mut dirty_max_y = screen_h;
    let mut needs_redraw = true;

    loop {
        let now = sys_get_time();
        let elapsed = now.wrapping_sub(last_frame);
        
        // ==========================================
        // THE HARDWARE POWER MANAGEMENT FIX
        // ==========================================
        if elapsed < ms_per_frame { 
            let time_to_sleep = ms_per_frame - elapsed;
            // INSTANTLY yields the CPU back to the kernel, allowing the Idle Task to fire `hlt`!
            sys_sleep_ms(time_to_sleep as u64); 
            continue; 
        }
        
        last_frame = now;
        
        let (mx_raw, my_raw, left, _) = sys_get_mouse();
        let mx = mx_raw.clamp(0, screen_w - 1); let my = my_raw.clamp(0, screen_h - 1);
        
        let mut mark_dirty = |x: usize, y: usize, w: usize, h: usize| {
            let pad = 20;
            let sx = x.saturating_sub(pad); let sy = y.saturating_sub(pad);
            let ex = (x + w + pad).min(screen_stride); let ey = (y + h + pad).min(screen_h);
            dirty_min_x = dirty_min_x.min(sx); dirty_min_y = dirty_min_y.min(sy);
            dirty_max_x = dirty_max_x.max(ex); dirty_max_y = dirty_max_y.max(ey);
        };

        my_terminal.pump_pipe();
        if my_browser.pump_pipe() { needs_redraw = true; }
        
        if now / 1000 != last_second {
            last_second = now / 1000;
            mark_dirty(0, 0, screen_stride, screen_h);
            my_monitor.update_stats();
            my_bootlog.refresh();
            my_netchat.update();
            needs_redraw = true;
        }
        
        if let Some(c) = sys_read_key() {
            if windows[0].active && windows[0].exists { my_terminal.handle_key(c); }
            else if windows[3].active && windows[3].exists { my_editor.handle_key(c); }
            else if windows[7].active && windows[7].exists { my_netchat.handle_key(c); }
            else if windows[8].active && windows[8].exists { my_browser.handle_key(c); }
            mark_dirty(0, 0, screen_stride, screen_h);
            needs_redraw = true;
        }
        
        if left && !prev_left {
            mark_dirty(0, 0, screen_stride, screen_h);
            needs_redraw = true;

            let mut handled = false;
            if show_start_menu {
                let menu_w = 150; let menu_h = 9 * 40; let menu_y = screen_h - TASKBAR_H - menu_h;
                if mx <= menu_w && my >= menu_y {
                    let item_idx = (my - menu_y) / 40;
                    match item_idx {
                        0 => launch_app(&mut windows, &mut z_order, 0),
                        1 => launch_app(&mut windows, &mut z_order, 4),
                        2 => launch_app(&mut windows, &mut z_order, 3),
                        3 => launch_app(&mut windows, &mut z_order, 1),
                        4 => launch_app(&mut windows, &mut z_order, 2),
                        5 => launch_app(&mut windows, &mut z_order, 5),
                        6 => launch_app(&mut windows, &mut z_order, 6),
                        7 => launch_app(&mut windows, &mut z_order, 7),
                        8 => launch_app(&mut windows, &mut z_order, 8),
                        _ => {}
                    }
                    handled = true;
                }
                show_start_menu = false;
            }
            
            if !handled && my >= screen_h - TASKBAR_H && mx < 100 {
                show_start_menu = !show_start_menu;
                handled = true;
            }
            
            if !handled {
                let mut hit_z_index = None;
                for i in (0..MAX_WINDOWS).rev() {
                    let idx = z_order[i];
                    let w = &windows[idx];
                    if w.exists && mx >= w.x && mx < w.x + w.w && my >= w.y && my < w.y + w.h {
                        hit_z_index = Some(i); break;
                    }
                }
                if let Some(i) = hit_z_index {
                    let idx = z_order[i];
                    for j in i..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
                    z_order[MAX_WINDOWS-1] = idx;
                    for win in windows.iter_mut() { win.active = false; }
                    windows[idx].active = true; target_idx = idx;
                    
                    let wx = windows[idx].x; let wy = windows[idx].y;
                    let ww = windows[idx].w; let wh = windows[idx].h;
                    
                    if mx >= wx + ww - 35 && mx <= wx + ww - 5 && my >= wy + 5 && my <= wy + 25 {
                        windows[idx].exists = false;
                    } else if mx >= wx + ww - 25 && mx <= wx + ww && my >= wy + wh - 25 && my <= wy + wh {
                        is_resizing = true; drag_off_x = (wx + ww) as isize - mx as isize; drag_off_y = (wy + wh) as isize - my as isize;
                    } else if my < wy + 30 {
                        is_dragging = true; drag_off_x = mx as isize - wx as isize; drag_off_y = my as isize - wy as isize;
                    } else {
                        let rx = mx.saturating_sub(wx); let ry = my.saturating_sub(wy);
                        if windows[idx].id == 3 { my_editor.handle_click(rx, ry, ww); }
                        if windows[idx].id == 4 { my_explorer.handle_click(rx, ry, ww); }
                        if windows[idx].id == 6 { my_bootlog.handle_click(mx, my, wx, wy, ww, wh); }
                        if windows[idx].id == 8 { my_browser.handle_click(rx, ry, ww, wh); }
                    }
                }
            }
        } else if !left { is_dragging = false; is_resizing = false; }
        
        if is_dragging {
            let win = &mut windows[target_idx];
            let old_x = win.x; let old_y = win.y; let old_w = win.w; let old_h = win.h;
            win.x = (mx as isize - drag_off_x).clamp(0, (screen_w - win.w) as isize) as usize;
            win.y = (my as isize - drag_off_y).clamp(0, (screen_h - TASKBAR_H - win.h) as isize) as usize;
            mark_dirty(old_x, old_y, old_w, old_h);
            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        }
        if is_resizing {
            let win = &mut windows[target_idx];
            let old_x = win.x; let old_y = win.y; let old_w = win.w; let old_h = win.h;
            let new_right = mx as isize + drag_off_x; let new_bottom = my as isize + drag_off_y;
            win.w = (new_right - win.x as isize).max(300).min((screen_w - win.x) as isize) as usize;
            win.h = (new_bottom - win.y as isize).max(200).min((screen_h - TASKBAR_H - win.y) as isize) as usize;
            mark_dirty(old_x, old_y, old_w, old_h);
            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        }
        
        if mx != prev_mx || my != prev_my { 
            mark_dirty(prev_mx, prev_my, 15, 15);
            mark_dirty(mx, my, 15, 15);
            needs_redraw = true; 
        }
        prev_left = left;
        
        if needs_redraw {
            dirty_min_x = dirty_min_x.clamp(0, screen_stride);
            dirty_min_y = dirty_min_y.clamp(0, screen_h);
            dirty_max_x = dirty_max_x.clamp(0, screen_stride);
            dirty_max_y = dirty_max_y.clamp(0, screen_h);
            
            let d_w = dirty_max_x.saturating_sub(dirty_min_x);
            let d_h = dirty_max_y.saturating_sub(dirty_min_y);

            if d_w > 0 && d_h > 0 {
                draw::restore_wallpaper_rect(&mut back_buffer, screen_stride, screen_h, dirty_min_x, dirty_min_y, d_w, d_h);
                
                draw_desktop_icons(&mut back_buffer, screen_stride, screen_h);
                for &idx in z_order.iter() {
                    if windows[idx].exists {
                        draw_window_rounded(&mut back_buffer, screen_stride, screen_h, &windows[idx]);
                        match windows[idx].id {
                            0 => my_terminal.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y),
                            1 => my_monitor.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y),
                            2 => draw::draw_text(&mut back_buffer, screen_stride, screen_h, windows[idx].x + 20, windows[idx].y + 50, "NyxOS Help", 0xFFFFFFFF),
                            3 => my_editor.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h),
                            4 => my_explorer.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h),
                            5 => my_sysinfo.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y),
                            6 => my_bootlog.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h),
                            7 => my_netchat.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h),
                            8 => my_browser.draw(&mut back_buffer, screen_stride, screen_h, windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h),
                            _ => {}
                        }
                    }
                }
                
                draw_taskbar(&mut back_buffer, screen_stride, screen_h);
                Clock::draw(&mut back_buffer, screen_stride, screen_h);
                if show_start_menu { draw_start_menu(&mut back_buffer, screen_stride, screen_h); }
                draw_cursor(&mut back_buffer, screen_stride, screen_h, mx, my);

                let hardware_fb = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
                for y in dirty_min_y..dirty_max_y {
                    let start_idx = y * screen_stride + dirty_min_x;
                    let end_idx = y * screen_stride + dirty_max_x;
                    hardware_fb[start_idx..end_idx].copy_from_slice(&back_buffer[start_idx..end_idx]);
                }
            }

            prev_mx = mx; prev_my = my;
            dirty_min_x = screen_stride; dirty_min_y = screen_h; dirty_max_x = 0; dirty_max_y = 0;
            needs_redraw = false;
        }
    }
}

fn draw_start_menu(fb: &mut [u32], w: usize, h: usize) {
    let menu_w = 150;
    let items = ["Terminal", "File Explorer", "Text Editor", "Sys Monitor", "Help", "Hardware Info", "Boot Logs", "NetChat", "Browser"];
    let item_h = 40; let menu_h = items.len() * item_h;
    let x = 10; let y = h - TASKBAR_H - menu_h;
    
    draw::draw_rect_simple(fb, w, h, x, y, menu_w, menu_h, 0xFF2D2D30);
    draw::draw_rect_simple(fb, w, h, x, y, 1, menu_h, 0xFF00AAFF);
    draw::draw_rect_simple(fb, w, h, x + menu_w - 1, y, 1, menu_h, 0xFF555555);
    draw::draw_rect_simple(fb, w, h, x, y, menu_w, 1, 0xFF555555);
    
    for (i, item) in items.iter().enumerate() {
        let item_y = y + (i * item_h);
        if i > 0 { draw::draw_rect_simple(fb, w, h, x + 5, item_y, menu_w - 10, 1, 0xFF3E3E42); }
        draw::draw_text(fb, w, h, x + 15, item_y + 12, item, 0xFFFFFFFF);
    }
}

fn launch_app(windows: &mut [Window], z_order: &mut [usize; MAX_WINDOWS], id_to_launch: usize) {
    let mut arr_idx = 0;
    for (i, w) in windows.iter().enumerate() { if w.id == id_to_launch { arr_idx = i; break; } }
    windows[arr_idx].exists = true;
    let mut z_pos = 0;
    for (i, &idx) in z_order.iter().enumerate() { if idx == arr_idx { z_pos = i; break; } }
    for j in z_pos..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
    z_order[MAX_WINDOWS-1] = arr_idx;
    for w in windows.iter_mut() { w.active = w.id == id_to_launch; }
}

fn draw_desktop_icons(fb: &mut [u32], w: usize, h: usize) {
    use crate::syscalls::{sys_fs_count, sys_fs_get_name};
    let count = sys_fs_count("/");
    let mut icon_x = 20; let mut icon_y = 20; let grid_h = 100;
    for i in 0..count {
        let mut name_buf = [0u8; 32];
        let len = sys_fs_get_name("/", i, &mut name_buf);
        if len > 0 {
            if let Ok(name) = core::str::from_utf8(&name_buf[..len]) {
                crate::gfx::ui::draw_file_icon(fb, w, h, icon_x, icon_y, name);
                icon_y += grid_h;
                if icon_y > h - 100 { icon_y = 20; icon_x += 80; }
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_write(2, b"\n[USERSPACE PANIC] Rust panicked in Ring 3!\n");
    sys_exit(99);
    loop {}
}