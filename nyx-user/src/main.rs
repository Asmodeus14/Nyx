#![no_std]
#![no_main]

mod syscalls;
mod console;
mod apps;
mod gfx;

use syscalls::*;
use apps::terminal::Terminal;
use apps::clock::Clock;

use gfx::draw::{self, restore_wallpaper_rect};
use gfx::ui::{draw_taskbar, draw_window_rounded, draw_cursor, draw_file_icon, Window, TASKBAR_H};

const MAX_WINDOWS: usize = 3;

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    // 1. SYSTEM SETUP
    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    if screen_w == 0 || screen_h == 0 { sys_exit(1); }

    let fb_ptr = sys_map_framebuffer();
    if fb_ptr == 0 { sys_exit(1); }
    
    let buffer_size_bytes = (screen_w * screen_h * 4) + 4096;
    let back_ptr = sys_alloc(buffer_size_bytes);
    if back_ptr == 0 || back_ptr == 9 { loop {} }

    let front_buffer = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
    let back_buffer = unsafe { core::slice::from_raw_parts_mut(back_ptr as *mut u32, screen_w * screen_h) };

    // --- PHASE 1: BOOT SPLASH ---
    draw::draw_rect_simple(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h, 0xFF000000);
    draw::draw_text(back_buffer, screen_w, screen_h, (screen_w / 2) - 60, screen_h / 2, "NyxOS User Mode", 0xFFFFFFFF);
    
    for y in 0..screen_h {
        let src = y * screen_w; let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() { 
            front_buffer[dst..dst+screen_w].copy_from_slice(&back_buffer[src..src+screen_w]); 
        }
    }

    // --- PHASE 2: DESKTOP INIT ---
    restore_wallpaper_rect(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h);

    let mut windows = [
        Window { id: 0, x: 50, y: 150, w: 760, h: 480, title: "Nyx Terminal", active: true, exists: true },
        Window { id: 1, x: 850, y: 200, w: 300, h: 200, title: "Sys Monitor", active: false, exists: true },
        Window { id: 2, x: 200, y: 400, w: 300, h: 200, title: "Help", active: false, exists: true },
    ];
    let mut z_order = [0, 1, 2];

    let mut my_terminal = Terminal::new();
    my_terminal.write_str("NyxOS Shell v0.3\n> ");

    // Initial Draw
    draw_desktop_icons(back_buffer, screen_w, screen_h);
    for &idx in z_order.iter() {
        if windows[idx].exists { 
            draw_window_rounded(back_buffer, screen_w, screen_h, &windows[idx]);
            if windows[idx].id == 0 { 
                my_terminal.draw(back_buffer, screen_w, screen_h, windows[idx].x, windows[idx].y); 
            }
        }
    }
    draw_taskbar(back_buffer, screen_w, screen_h);
    Clock::draw(back_buffer, screen_w, screen_h); 

    for y in 0..screen_h {
        let src = y * screen_w; let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() { 
            front_buffer[dst..dst+screen_w].copy_from_slice(&back_buffer[src..src+screen_w]); 
        }
    }

    // --- VARIABLES ---
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

    // --- EVENT LOOP ---
    loop {
        let now = sys_get_time();
        if now.wrapping_sub(last_frame) < ms_per_frame { unsafe { core::arch::asm!("nop"); } continue; }
        last_frame = now;

        let (mx_raw, my_raw, left, _right) = sys_get_mouse();
        let mx = mx_raw.clamp(0, screen_w - 1);
        let my = my_raw.clamp(0, screen_h - 1);
        
        let mut needs_redraw = false;
        
        let mut dirty_min_x = screen_w; let mut dirty_min_y = screen_h;
        let mut dirty_max_x = 0; let mut dirty_max_y = 0;

        let mut mark_dirty = |x: usize, y: usize, w: usize, h: usize| {
            let pad = 20;
            let sx = x.saturating_sub(pad); let sy = y.saturating_sub(pad);
            let ex = (x + w + pad).min(screen_w); let ey = (y + h + pad).min(screen_h);
            dirty_min_x = dirty_min_x.min(sx); dirty_min_y = dirty_min_y.min(sy);
            dirty_max_x = dirty_max_x.max(ex); dirty_max_y = dirty_max_y.max(ey);
        };

        // 1. CLOCK UPDATE
        if now / 1000 != last_second {
            last_second = now / 1000;
            let clock_w = 220;
            let clock_h = 80;
            let clock_x = (screen_w / 2).saturating_sub(clock_w / 2);
            let clock_y = 60;
            mark_dirty(clock_x, clock_y, clock_w, clock_h);
            needs_redraw = true;
        }

        // 2. KEYBOARD
        if windows[0].active && windows[0].exists {
            if let Some(c) = sys_read_key() {
                my_terminal.handle_key(c);
                mark_dirty(windows[0].x, windows[0].y, windows[0].w, windows[0].h);
                
                // If user pressed Enter, update desktop icons (in case file was created)
                if c == '\n' { 
                    mark_dirty(0, 0, 300, screen_h - TASKBAR_H); // Repaint icon area
                }
                needs_redraw = true;
            }
        } else {
             let _ = sys_read_key(); 
        }

        // 3. MOUSE
        if left && !prev_left {
            let mut hit_z_index = None;
            let mut hit_resize = false;
            let mut hit_close = false;

            for i in (0..MAX_WINDOWS).rev() {
                let idx = z_order[i];
                let w = &windows[idx];
                if !w.exists { continue; }
                
                if mx >= w.x + w.w - 35 && mx <= w.x + w.w - 5 && my >= w.y + 5 && my <= w.y + 25 {
                    hit_z_index = Some(i); hit_close = true; break;
                }
                if mx >= w.x + w.w - 25 && mx <= w.x + w.w && my >= w.y + w.h - 25 && my <= w.y + w.h {
                    hit_z_index = Some(i); hit_resize = true; break;
                }
                if mx >= w.x && mx < w.x + w.w && my >= w.y && my < w.y + 30 {
                    hit_z_index = Some(i); break;
                }
                if mx >= w.x && mx < w.x + w.w && my >= w.y && my < w.y + w.h {
                    hit_z_index = Some(i); break;
                }
            }

            if let Some(i) = hit_z_index {
                let idx = z_order[i];
                if hit_close {
                    mark_dirty(windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h);
                    windows[idx].exists = false;
                    needs_redraw = true;
                } else {
                    let old_w = windows[prev_active_idx];
                    mark_dirty(old_w.x, old_w.y, old_w.w, old_w.h);
                    for j in i..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
                    z_order[MAX_WINDOWS-1] = idx;
                    for win in windows.iter_mut() { win.active = false; }
                    windows[idx].active = true;
                    prev_active_idx = idx; target_idx = idx;
                    
                    if hit_resize {
                        is_resizing = true;
                        drag_off_x = (windows[idx].x + windows[idx].w) as isize - mx as isize;
                        drag_off_y = (windows[idx].y + windows[idx].h) as isize - my as isize;
                    } else if !hit_close {
                        is_dragging = true;
                        drag_off_x = mx as isize - windows[idx].x as isize;
                        drag_off_y = my as isize - windows[idx].y as isize;
                    }
                    mark_dirty(windows[idx].x, windows[idx].y, windows[idx].w, windows[idx].h);
                    needs_redraw = true;
                }
            }
        } else if !left { is_dragging = false; is_resizing = false; }

        if is_dragging {
            let win = &mut windows[target_idx];
            mark_dirty(win.x, win.y, win.w, win.h);
            win.x = (mx as isize - drag_off_x).clamp(0, (screen_w - win.w) as isize) as usize;
            win.y = (my as isize - drag_off_y).clamp(0, (screen_h - TASKBAR_H - win.h) as isize) as usize;
            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        } else if is_resizing {
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

        // 4. RENDER
        if needs_redraw && dirty_max_x > dirty_min_x {
            let dx = dirty_min_x; let dy = dirty_min_y;
            let dw = dirty_max_x - dx; let dh = dirty_max_y - dy;
            
            restore_wallpaper_rect(back_buffer, screen_w, screen_h, dx, dy, dw, dh);
            
            // --- DRAW ICONS ---
            draw_desktop_icons(back_buffer, screen_w, screen_h);
            
            for &idx in z_order.iter() {
                if windows[idx].exists { 
                    draw_window_rounded(back_buffer, screen_w, screen_h, &windows[idx]);
                    if windows[idx].id == 0 { 
                         my_terminal.draw(back_buffer, screen_w, screen_h, windows[idx].x, windows[idx].y);
                    }
                }
            }
            
            draw_taskbar(back_buffer, screen_w, screen_h);
            Clock::draw(back_buffer, screen_w, screen_h);
            draw_cursor(back_buffer, screen_w, screen_h, mx, my);
            
            present_rect(front_buffer, back_buffer, screen_w, screen_stride, screen_h, dx, dy, dw, dh);
            
            prev_mx = mx; prev_my = my;
        }
    }
}

// Helper to draw icons based on FS content
fn draw_desktop_icons(fb: &mut [u32], w: usize, h: usize) {
    use crate::syscalls::{sys_fs_count, sys_fs_get_name};
    
    let count = sys_fs_count();
    let mut icon_x = 20;
    let mut icon_y = 20;
    let grid_h = 100; 

    for i in 0..count {
        let mut name_buf = [0u8; 32];
        let len = sys_fs_get_name(i, &mut name_buf);
        
        if len > 0 {
            if let Ok(name) = core::str::from_utf8(&name_buf[..len]) {
                // Now we call the function from gfx::ui
                crate::gfx::ui::draw_file_icon(fb, w, h, icon_x, icon_y, name);
                
                icon_y += grid_h;
                if icon_y > h - 100 {
                    icon_y = 20;
                    icon_x += 80; 
                }
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

#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { sys_exit(1); }