#![no_std]
#![no_main]

extern crate alloc;
use core::panic::PanicInfo;
use linked_list_allocator::LockedHeap;
use alloc::vec::Vec;
use alloc::vec;

mod syscalls;
mod console;
mod apps;
mod gfx;

use syscalls::*;
use apps::terminal::Terminal;
use apps::clock::Clock;
use apps::editor::Editor;
use apps::explorer::Explorer;
use gfx::draw;
use gfx::ui::{draw_taskbar, draw_window_rounded, draw_cursor, Window, TASKBAR_H};

const HEAP_SIZE: usize = 8 * 1024 * 1024; 
static mut HEAP_MEM: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// 5 Apps: Terminal, SysMon, Help, Editor, Explorer
const MAX_WINDOWS: usize = 5; 

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    unsafe { ALLOCATOR.lock().init(HEAP_MEM.as_mut_ptr() as usize, HEAP_SIZE); }

    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    if screen_w == 0 || screen_h == 0 { sys_exit(1); }

    let fb_ptr = sys_map_framebuffer();
    if fb_ptr == 0 { sys_exit(1); }
    
    let front_buffer = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
    let mut back_buffer: Vec<u32> = vec![0xFF008080; screen_stride * screen_h];

    // --- PHASE 1: BOOT SPLASH ---
    draw::draw_rect_simple(&mut back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h, 0xFF000000);
    draw::draw_text(&mut back_buffer, screen_w, screen_h, (screen_w / 2) - 60, screen_h / 2, "NyxOS User Mode", 0xFFFFFFFF);
    
    for y in 0..screen_h {
        let src = y * screen_w; let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() { 
            front_buffer[dst..dst+screen_w].copy_from_slice(&back_buffer[src..src+screen_w]); 
        }
    }

    let start_time = sys_get_time();
    while sys_get_time() < start_time + 1000 { unsafe { core::arch::asm!("nop"); } }

    // --- PHASE 2: DESKTOP INIT ---
    draw::restore_wallpaper_rect(&mut back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h);

    // --- WINDOW DEFINITIONS ---
    let mut windows = [
        Window { id: 0, x: 50, y: 50, w: 760, h: 480, title: "Nyx Terminal", active: true, exists: true },
        Window { id: 1, x: 100, y: 100, w: 300, h: 200, title: "Sys Monitor", active: false, exists: false },
        Window { id: 2, x: 150, y: 150, w: 300, h: 200, title: "Help", active: false, exists: false },
        Window { id: 3, x: 200, y: 100, w: 600, h: 450, title: "NyxPad", active: false, exists: false },
        Window { id: 4, x: 150, y: 150, w: 500, h: 400, title: "File Explorer", active: false, exists: true }, 
    ];
    let mut z_order = [0, 1, 2, 3, 4];

    let mut my_terminal = Terminal::new();
    my_terminal.write_str("NyxOS Shell v0.6\nType 'ls' to list files.\n> ");
    let mut my_editor = Editor::new();
    let mut my_explorer = Explorer::new(); 

    let mut show_start_menu = false;
    let mut is_dragging = false;
    let mut is_resizing = false;
    let mut target_idx = 0; 
    let mut drag_off_x = 0;
    let mut drag_off_y = 0;
    let mut prev_left = false;
    let mut prev_mx = 0;
    let mut prev_my = 0;
    let mut prev_active_idx = 0;
    
    let mut last_frame = sys_get_time();
    let mut last_second = sys_get_time() / 1000;
    let ms_per_frame = 1000 / 60; 

    let mut dirty_min_x = 0; let mut dirty_min_y = 0;
    let mut dirty_max_x = screen_w; let mut dirty_max_y = screen_h;
    let mut needs_redraw = true;

    loop {
        let now = sys_get_time();
        if now.wrapping_sub(last_frame) < ms_per_frame { unsafe { core::arch::asm!("nop"); } continue; }
        last_frame = now;

        let (mx_raw, my_raw, left, _right) = sys_get_mouse();
        let mx = mx_raw.clamp(0, screen_w - 1);
        let my = my_raw.clamp(0, screen_h - 1);
        
        let mut mark_dirty = |x: usize, y: usize, w: usize, h: usize| {
            let pad = 20;
            let sx = x.saturating_sub(pad); let sy = y.saturating_sub(pad);
            let ex = (x + w + pad).min(screen_w); let ey = (y + h + pad).min(screen_h);
            dirty_min_x = dirty_min_x.min(sx); dirty_min_y = dirty_min_y.min(sy);
            dirty_max_x = dirty_max_x.max(ex); dirty_max_y = dirty_max_y.max(ey);
        };

        if now / 1000 != last_second {
            last_second = now / 1000;
            let clock_w = 220; let clock_h = 80;
            let clock_x = (screen_w / 2).saturating_sub(clock_w / 2);
            mark_dirty(clock_x, 60, clock_w, clock_h);
            needs_redraw = true;
        }

        if let Some(c) = sys_read_key() {
            if windows[0].active && windows[0].exists {
                my_terminal.handle_key(c);
                mark_dirty(windows[0].x, windows[0].y, windows[0].w, windows[0].h);
                if c == '\n' { 
                    mark_dirty(0, 0, 300, screen_h - TASKBAR_H); 
                    my_explorer.refresh(); 
                }
                needs_redraw = true;
            } 
            else if windows[3].active && windows[3].exists {
                my_editor.handle_key(c);
                mark_dirty(windows[3].x, windows[3].y, windows[3].w, windows[3].h);
                needs_redraw = true;
            }
        }

        if left && !prev_left {
            let mut handled = false;

            if show_start_menu {
                let menu_w = 150;
                let menu_h = 200; 
                let menu_x = 10;
                let menu_y = screen_h - TASKBAR_H - menu_h;

                if mx >= menu_x && mx <= menu_x + menu_w && my >= menu_y && my <= menu_y + menu_h {
                    let item_idx = (my - menu_y) / 40;
                    match item_idx {
                        0 => launch_app(&mut windows, &mut z_order, 0, &mut mark_dirty), 
                        1 => launch_app(&mut windows, &mut z_order, 4, &mut mark_dirty), 
                        2 => launch_app(&mut windows, &mut z_order, 3, &mut mark_dirty), 
                        3 => launch_app(&mut windows, &mut z_order, 1, &mut mark_dirty), 
                        4 => launch_app(&mut windows, &mut z_order, 2, &mut mark_dirty), 
                        _ => {}
                    }
                    show_start_menu = false;
                    mark_dirty(menu_x, menu_y, menu_w, menu_h);
                    needs_redraw = true;
                    handled = true;
                } else {
                    show_start_menu = false;
                    mark_dirty(menu_x, menu_y, menu_w, menu_h);
                    needs_redraw = true;
                }
            }

            if !handled && my >= screen_h - TASKBAR_H {
                if mx < 100 { 
                    show_start_menu = !show_start_menu;
                    let menu_h = 200; 
                    mark_dirty(10, screen_h - TASKBAR_H - menu_h, 150, menu_h + TASKBAR_H);
                    needs_redraw = true;
                    handled = true;
                }
            }

            if !handled {
                let mut hit_z_index = None;
                for i in (0..MAX_WINDOWS).rev() {
                    let idx = z_order[i];
                    let w = &windows[idx];
                    if !w.exists { continue; }
                    
                    if mx >= w.x && mx < w.x + w.w && my >= w.y && my < w.y + w.h {
                        hit_z_index = Some(i);
                        break; 
                    }
                }

                if let Some(i) = hit_z_index {
                    let idx = z_order[i];
                    
                    // FIX: Temporarily copy needed values to avoid conflicting borrows
                    let old_w_idx = prev_active_idx;
                    let old_x = windows[old_w_idx].x;
                    let old_y = windows[old_w_idx].y;
                    let old_w_val = windows[old_w_idx].w;
                    let old_h_val = windows[old_w_idx].h;
                    mark_dirty(old_x, old_y, old_w_val, old_h_val);

                    for j in i..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
                    z_order[MAX_WINDOWS-1] = idx;
                    
                    for win in windows.iter_mut() { win.active = false; }
                    windows[idx].active = true;
                    prev_active_idx = idx; target_idx = idx;

                    // FIX: Re-borrow purely for read access values
                    let wx = windows[idx].x;
                    let wy = windows[idx].y;
                    let ww = windows[idx].w;
                    let wh = windows[idx].h;
                    let wid = windows[idx].id;

                    if mx >= wx + ww - 35 && mx <= wx + ww - 5 && my >= wy + 5 && my <= wy + 25 {
                        windows[idx].exists = false;
                        mark_dirty(wx, wy, ww, wh);
                    } 
                    else if mx >= wx + ww - 25 && mx <= wx + ww && my >= wy + wh - 25 && my <= wy + wh {
                        is_resizing = true;
                        drag_off_x = (wx + ww) as isize - mx as isize;
                        drag_off_y = (wy + wh) as isize - my as isize;
                    }
                    else if my < wy + 30 {
                        is_dragging = true;
                        drag_off_x = mx as isize - wx as isize;
                        drag_off_y = my as isize - wy as isize;
                    }
                    else {
                        // Pass click to App Logic
                        let rx = mx.saturating_sub(wx);
                        let ry = my.saturating_sub(wy);
                        
                        // UPDATED: Pass 'ww' (window width) to handle_click
                        if wid == 3 { my_editor.handle_click(rx, ry, ww); }
                        
                        if wid == 4 { my_explorer.handle_click(rx, ry, ww); }
                    }
                    mark_dirty(wx, wy, ww, wh);
                    needs_redraw = true;
                }
            }
        } else if !left {
            is_dragging = false; is_resizing = false;
        }

        if is_dragging {
            let win = &mut windows[target_idx];
            mark_dirty(win.x, win.y, win.w, win.h);
            win.x = (mx as isize - drag_off_x).clamp(0, (screen_w - win.w) as isize) as usize;
            win.y = (my as isize - drag_off_y).clamp(0, (screen_h - TASKBAR_H - win.h) as isize) as usize;
            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        }
        
        if is_resizing {
            let win = &mut windows[target_idx];
            mark_dirty(win.x, win.y, win.w, win.h);
            let new_right = mx as isize + drag_off_x;
            let new_bottom = my as isize + drag_off_y;
            win.w = (new_right - win.x as isize).max(300).min((screen_w - win.x) as isize) as usize;
            win.h = (new_bottom - win.y as isize).max(200).min((screen_h - TASKBAR_H - win.y) as isize) as usize;
            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        }

        if mx != prev_mx || my != prev_my { 
            mark_dirty(prev_mx, prev_my, 15, 15); 
            mark_dirty(mx, my, 15, 15); 
            needs_redraw = true; 
        }
        prev_left = left;

        if needs_redraw && dirty_max_x > dirty_min_x {
            let dx = dirty_min_x; let dy = dirty_min_y;
            let dw = dirty_max_x - dx; let dh = dirty_max_y - dy;
            
            draw::restore_wallpaper_rect(&mut back_buffer, screen_w, screen_h, dx, dy, dw, dh);
            draw_desktop_icons(&mut back_buffer, screen_w, screen_h);
            
            for &idx in z_order.iter() {
                if windows[idx].exists { 
                    draw_window_rounded(&mut back_buffer, screen_w, screen_h, &windows[idx]);
                    if windows[idx].id == 0 { 
                         my_terminal.draw(&mut back_buffer, screen_w, screen_h, windows[idx].x, windows[idx].y);
                    } else if windows[idx].id == 3 {
                        my_editor.draw(&mut back_buffer, screen_w, screen_h, windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h);
                    } else if windows[idx].id == 4 {
                        my_explorer.draw(&mut back_buffer, screen_w, screen_h, windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h);
                    } else if windows[idx].id == 1 {
                        draw::draw_text(&mut back_buffer, screen_w, screen_h, windows[idx].x + 20, windows[idx].y + 50, "CPU: <1%", 0xFFFFFFFF);
                        draw::draw_text(&mut back_buffer, screen_w, screen_h, windows[idx].x + 20, windows[idx].y + 70, "RAM: 32MB", 0xFFFFFFFF);
                    } else if windows[idx].id == 2 {
                        draw::draw_text(&mut back_buffer, screen_w, screen_h, windows[idx].x + 20, windows[idx].y + 50, "NyxOS Help", 0xFFFFFFFF);
                        draw::draw_text(&mut back_buffer, screen_w, screen_h, windows[idx].x + 20, windows[idx].y + 70, "v0.6 Beta", 0xFFAAAAAA);
                    }
                }
            }
            
            draw_taskbar(&mut back_buffer, screen_w, screen_h);
            Clock::draw(&mut back_buffer, screen_w, screen_h);
            
            if show_start_menu {
                draw_start_menu(&mut back_buffer, screen_w, screen_h);
            }

            draw_cursor(&mut back_buffer, screen_w, screen_h, mx, my);
            
            present_rect(front_buffer, &back_buffer, screen_w, screen_stride, screen_h, dx, dy, dw, dh);
            prev_mx = mx; prev_my = my;
            
            dirty_min_x = screen_w; dirty_min_y = screen_h;
            dirty_max_x = 0; dirty_max_y = 0;
        }
    }
}

fn draw_start_menu(fb: &mut [u32], w: usize, h: usize) {
    let menu_w = 150;
    let items = ["Terminal", "File Explorer", "Text Editor", "Sys Monitor", "Help"];
    let item_h = 40;
    let menu_h = items.len() * item_h;
    let x = 10;
    let y = h - TASKBAR_H - menu_h;

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

fn launch_app(
    windows: &mut [Window], 
    z_order: &mut [usize; MAX_WINDOWS], 
    id_to_launch: usize, 
    mark_dirty: &mut impl FnMut(usize, usize, usize, usize)
) {
    let mut arr_idx = 0;
    for (i, w) in windows.iter().enumerate() {
        if w.id == id_to_launch { arr_idx = i; break; }
    }
    
    // Copy values to avoid borrow issues
    let (wx, wy, ww, wh) = {
        let w = &windows[arr_idx];
        (w.x, w.y, w.w, w.h)
    };
    mark_dirty(wx, wy, ww, wh);

    windows[arr_idx].exists = true; 
    
    let mut z_pos = 0;
    for (i, &idx) in z_order.iter().enumerate() {
        if idx == arr_idx { z_pos = i; break; }
    }
    for j in z_pos..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
    z_order[MAX_WINDOWS-1] = arr_idx;

    for w in windows.iter_mut() { w.active = (w.id == id_to_launch); }
}

fn draw_desktop_icons(fb: &mut [u32], w: usize, h: usize) {
    use crate::syscalls::{sys_fs_count, sys_fs_get_name};
    let count = sys_fs_count("/"); // Pass root path
    let mut icon_x = 20;
    let mut icon_y = 20;
    let grid_h = 100; 

    for i in 0..count {
        let mut name_buf = [0u8; 32];
        let len = sys_fs_get_name("/", i, &mut name_buf); // Pass root path
        if len > 0 {
            if let Ok(name) = core::str::from_utf8(&name_buf[..len]) {
                crate::gfx::ui::draw_file_icon(fb, w, h, icon_x, icon_y, name);
                icon_y += grid_h;
                if icon_y > h - 100 { icon_y = 20; icon_x += 80; }
            }
        }
    }
}

fn present_rect(front: &mut [u32], back: &[u32], w: usize, stride: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let sy = y + row; if sy >= h { break; }
        let src = sy * w + x; 
        let dst = sy * stride + x;
        if dst + dw <= front.len() && src + dw <= back.len() { 
            front[dst..dst + dw].copy_from_slice(&back[src..src + dw]); 
        }
    }
}

#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }