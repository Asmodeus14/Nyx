#![no_std]
#![no_main]

mod syscalls;
mod console;

use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
use syscalls::*;

const MAX_WINDOWS: usize = 3;
const TASKBAR_H: usize = 50; // Thicker Taskbar

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    if screen_w == 0 || screen_h == 0 { sys_exit(1); }

    let fb_ptr = sys_map_framebuffer();
    if fb_ptr == 0 { sys_exit(1); }
    
    // Allocate Back Buffer
    let buffer_size_bytes = (screen_w * screen_h * 4) + 4096;
    let back_ptr = sys_alloc(buffer_size_bytes);
    if back_ptr == 0 || back_ptr == 9 { loop {} }

    let front_buffer = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
    let back_buffer = unsafe { core::slice::from_raw_parts_mut(back_ptr as *mut u32, screen_w * screen_h) };

    // --- PHASE 1: THE LOADER (Boot Splash) ---
    // This wipes the Kernel's text and ensures the screen is clean.
    
    // 1. Fill screen with black
    draw_rect_simple(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h, 0xFF000000);
    
    // 2. Draw Loading Text
    let msg = "Starting NyxOS...";
    let msg_len = msg.len() * 9;
    draw_text(back_buffer, screen_w, screen_h, (screen_w / 2) - (msg_len / 2), screen_h / 2, msg, 0xFFFFFFFF);
    
    // 3. Draw Loading Bar
    draw_rect_simple(back_buffer, screen_w, screen_h, (screen_w/2) - 100, (screen_h/2) + 20, 200, 2, 0xFF555555);
    draw_rect_simple(back_buffer, screen_w, screen_h, (screen_w/2) - 100, (screen_h/2) + 20, 100, 2, 0xFF00AAFF);

    // 4. Present the Loader to the screen immediately
    for y in 0..screen_h {
        let src = y * screen_w;
        let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() && src + screen_w <= back_buffer.len() {
            front_buffer[dst..dst+screen_w].copy_from_slice(&back_buffer[src..src+screen_w]);
        }
    }

    // Optional: Small delay to simulate loading (and let user see the splash)
    let start_load = sys_get_time();
    while sys_get_time() - start_load < 1000 { unsafe { core::arch::asm!("nop"); } }

    // --- PHASE 2: DESKTOP INITIALIZATION ---

    // 1. Draw Wallpaper (Wipe the loading screen)
    restore_wallpaper_rect(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h);

    let mut windows = [
        Window { id: 0, x: 100, y: 100, w: 500, h: 350, title: "Nyx Terminal", active: true, exists: true },
        Window { id: 1, x: 650, y: 150, w: 400, h: 300, title: "System Monitor", active: false, exists: true },
        Window { id: 2, x: 300, y: 500, w: 400, h: 250, title: "Help Info", active: false, exists: true },
    ];
    let mut z_order = [0, 1, 2];

    // 2. Draw Initial Windows
    for &idx in z_order.iter() {
        if windows[idx].exists { draw_window_rounded(back_buffer, screen_w, screen_h, &windows[idx]); }
    }
    draw_taskbar(back_buffer, screen_w, screen_h);

    // 3. Present Desktop (Full Refresh)
    for y in 0..screen_h {
        let src = y * screen_w;
        let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() && src + screen_w <= back_buffer.len() {
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
    let mut last_second_draw = 0;
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

        if now / 1000 != last_second_draw {
            mark_dirty(screen_w.saturating_sub(120), screen_h - TASKBAR_H + 15, 110, 25);
            needs_redraw = true; last_second_draw = now / 1000;
        }

        if left && !prev_left {
            let mut hit_z_index = None;
            let mut hit_resize = false;
            let mut hit_close = false;

            for i in (0..MAX_WINDOWS).rev() {
                let idx = z_order[i];
                let w = &windows[idx];
                if !w.exists { continue; }
                
                // Close [X] (Top Right)
                if mx >= w.x + w.w - 35 && mx <= w.x + w.w - 5 && my >= w.y + 5 && my <= w.y + 25 {
                    hit_z_index = Some(i); hit_close = true; break;
                }
                // Resize Handle (Bottom Right 25x25)
                if mx >= w.x + w.w - 25 && mx <= w.x + w.w && my >= w.y + w.h - 25 && my <= w.y + w.h {
                    hit_z_index = Some(i); hit_resize = true; break;
                }
                // Title Bar (Move)
                if mx >= w.x && mx < w.x + w.w && my >= w.y && my < w.y + 30 {
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
                        // FIX: Calculate offset from the RIGHT/BOTTOM edge, not top-left
                        drag_off_x = (windows[idx].x + windows[idx].w) as isize - mx as isize;
                        drag_off_y = (windows[idx].y + windows[idx].h) as isize - my as isize;
                    } else {
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
            
            // FIX: Calculate new width based on absolute mouse pos + offset - start pos
            // New Right Edge = mx + drag_off_x
            // Width = Right Edge - Left Edge (win.x)
            let new_right = mx as isize + drag_off_x;
            let new_bottom = my as isize + drag_off_y;
            
            win.w = (new_right - win.x as isize).max(150).min((screen_w - win.x) as isize) as usize;
            win.h = (new_bottom - win.y as isize).max(100).min((screen_h - TASKBAR_H - win.y) as isize) as usize;

            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        }
        
        if mx != prev_mx || my != prev_my { mark_dirty(prev_mx, prev_my, 15, 15); mark_dirty(mx, my, 15, 15); needs_redraw = true; }
        prev_left = left;

        if needs_redraw && dirty_max_x > dirty_min_x {
            let dx = dirty_min_x; let dy = dirty_min_y;
            let dw = dirty_max_x - dx; let dh = dirty_max_y - dy;
            restore_wallpaper_rect(back_buffer, screen_w, screen_h, dx, dy, dw, dh);
            for &idx in z_order.iter() {
                if windows[idx].exists { draw_window_rounded(back_buffer, screen_w, screen_h, &windows[idx]); }
            }
            draw_taskbar(back_buffer, screen_w, screen_h);
            draw_clock_taskbar(back_buffer, screen_w, screen_h, now);
            draw_cursor(back_buffer, screen_w, screen_h, mx, my);
            present_rect(front_buffer, back_buffer, screen_w, screen_stride, screen_h, dx, dy, dw, dh);
            prev_mx = mx; prev_my = my;
        }
    }
}

// --- DRAWING FUNCTIONS ---

fn draw_taskbar(fb: &mut [u32], w: usize, h: usize) {
    let y_start = h - TASKBAR_H;
    for y in y_start..h {
        let color = if y == y_start { 0xFF555555 } else { 0xFF1A1A1A };
        for x in 0..w { fb[y * w + x] = color; }
    }
    // Start Button
    draw_rect_simple(fb, w, h, 10, y_start + 10, 30, 30, 0xFF00AAFF);
    draw_text(fb, w, h, 50, y_start + 17, "NYX", 0xFFFFFFFF);
}

fn draw_clock_taskbar(fb: &mut [u32], w: usize, h: usize, ticks: u64) {
    let seconds = ticks / 1000;
    let mins = (seconds / 60) % 60;
    let hrs = (seconds / 3600) % 24;
    let tx = w - 100; let ty = h - TASKBAR_H + 17;
    draw_digit(fb, w, h, tx, ty, (hrs / 10) as u8); draw_digit(fb, w, h, tx + 10, ty, (hrs % 10) as u8);
    draw_rect_simple(fb, w, h, tx + 20, ty + 5, 2, 2, 0xFFFFFFFF); draw_rect_simple(fb, w, h, tx + 20, ty + 12, 2, 2, 0xFFFFFFFF);
    draw_digit(fb, w, h, tx + 25, ty, (mins / 10) as u8); draw_digit(fb, w, h, tx + 35, ty, (mins % 10) as u8);
}

fn draw_window_rounded(fb: &mut [u32], w: usize, h: usize, win: &Window) {
    let color = if win.active { 0xFF2A2A2A } else { 0xFF1A1A1A };
    let border = if win.active { 0xFF00AAFF } else { 0xFF444444 };
    let radius: isize = 12; let radius_sq = radius * radius;

    for row in 0..win.h {
        let sy = win.y + row; if sy >= h { break; }
        for col in 0..win.w {
            let sx = win.x + col; if sx >= w { break; }
            let cx = col as isize; let cy = row as isize;
            let mut in_corner = false;
            if cx < radius && cy < radius { if (radius-cx-1).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy < radius { if (cx-(win.w as isize-radius)).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx < radius && cy >= (win.h as isize)-radius { if (radius-cx-1).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy >= (win.h as isize)-radius { if (cx-(win.w as isize-radius)).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            if in_corner { continue; }

            let mut c = if row < 30 { 0xFF3A3A3A } else { color };
            if col < 2 || col >= win.w-2 || row >= win.h-2 || row < 2 || (row >= 28 && row < 30) { c = border; }
            // Resize Handle Indicators
            if row > win.h - 15 && col > win.w - 15 && (row + col) % 4 == 0 { c = 0xFF888888; }
            fb[sy * w + sx] = c;
        }
    }
    // Close Button [X]
    draw_rect_simple(fb, w, h, win.x + win.w - 35, win.y + 5, 30, 20, 0xFFFF4444);
    draw_text(fb, w, h, win.x + win.w - 25, win.y + 7, "X", 0xFFFFFFFF);
    draw_text(fb, w, h, win.x + 15, win.y + 7, win.title, 0xFFFFFFFF);
}

fn draw_digit(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, digit: u8) { draw_char(fb, w, h, x, y, (digit + 48) as char, 0xFFFFFFFF); }
fn draw_char(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, c: char, color: u32) {
    if let Some(raster) = get_raster(c, FontWeight::Regular, RasterHeight::Size16) {
        for (ri, row) in raster.raster().iter().enumerate() {
            for (ci, val) in row.iter().enumerate() {
                if *val > 10 { let px = x+ci; let py = y+ri; if px < w && py < h { fb[py*w+px] = color; } }
            }
        }
    }
}
fn restore_wallpaper_rect(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let sy = y + row; if sy >= h { break; }
        for col in 0..dw {
            let sx = x + col; if sx >= w { break; }
            let b = (sy * 255 / h) as u32; let g = (sx * 100 / w) as u32;
            fb[sy * w + sx] = 0xFF000000 | (g << 8) | b;
        }
    }
}
fn draw_cursor(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
    for i in 0..10 {
        if x+i < w && y+i < h { fb[(y+i)*w+(x+i)] = 0xFFFFFFFF; }
        if x+i < w && y < h { fb[y*w+(x+i)] = 0xFFFFFFFF; }
        if x < w && y+i < h { fb[(y+i)*w+x] = 0xFFFFFFFF; }
    }
}
fn draw_text(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, text: &str, color: u32) {
    let mut cx = x; for c in text.chars() { draw_char(fb, w, h, cx, y, c, color); cx += 9; }
}
fn draw_rect_simple(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, rw: usize, rh: usize, color: u32) {
    for row in 0..rh {
        let sy = y + row; if sy >= h { break; }
        for col in 0..rw { let sx = x + col; if sx >= w { break; } fb[sy * w + sx] = color; }
    }
}
fn present_rect(front: &mut [u32], back: &[u32], w: usize, stride: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let sy = y + row; if sy >= h { break; }
        let src = sy * w + x; let dst = sy * stride + x;
        if dst + dw <= front.len() && src + dw <= back.len() { front[dst..dst + dw].copy_from_slice(&back[src..src + dw]); }
    }
}

// FIX: Added Clone and Copy
#[derive(Clone, Copy)]
struct Window { id: usize, x: usize, y: usize, w: usize, h: usize, title: &'static str, active: bool, exists: bool }

#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { sys_exit(1); }