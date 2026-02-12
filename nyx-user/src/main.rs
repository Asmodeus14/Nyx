#![no_std]
#![no_main]

mod syscalls;
mod console;
mod terminal; // Connects to terminal.rs
mod clock;    // Connects to clock.rs

use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
use syscalls::*;
use terminal::Terminal;
use clock::Clock;

const MAX_WINDOWS: usize = 3;
const TASKBAR_H: usize = 50; 

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    // 1. SYSTEM SETUP
    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    if screen_w == 0 || screen_h == 0 { sys_exit(1); }

    let fb_ptr = sys_map_framebuffer();
    if fb_ptr == 0 { sys_exit(1); }
    
    // Allocate Back Buffer (Double Buffering)
    let buffer_size_bytes = (screen_w * screen_h * 4) + 4096;
    let back_ptr = sys_alloc(buffer_size_bytes);
    if back_ptr == 0 || back_ptr == 9 { loop {} }

    // Create safe slices for buffers
    let front_buffer = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
    let back_buffer = unsafe { core::slice::from_raw_parts_mut(back_ptr as *mut u32, screen_w * screen_h) };

    // --- PHASE 1: BOOT SPLASH ---
    draw_rect_simple(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h, 0xFF000000);
    draw_text(back_buffer, screen_w, screen_h, (screen_w / 2) - 60, screen_h / 2, "NyxOS User Mode", 0xFFFFFFFF);
    
    // Present Splash immediately
    for y in 0..screen_h {
        let src = y * screen_w; let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() { front_buffer[dst..dst+screen_w].copy_from_slice(&back_buffer[src..src+screen_w]); }
    }

    // --- PHASE 2: DESKTOP INITIALIZATION ---
    restore_wallpaper_rect(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h);

    // Define Windows
    // ID 0 = Terminal, ID 1 = Sys Monitor, ID 2 = Help
    let mut windows = [
        Window { id: 0, x: 50, y: 50, w: 760, h: 480, title: "Nyx Terminal", active: true, exists: true },
        Window { id: 1, x: 850, y: 100, w: 300, h: 200, title: "Sys Monitor", active: false, exists: true },
        Window { id: 2, x: 200, y: 300, w: 300, h: 200, title: "Help", active: false, exists: true },
    ];
    let mut z_order = [0, 1, 2];

    // Initialize Terminal Application
    let mut my_terminal = Terminal::new();
    my_terminal.write_str("NyxOS Shell v0.2\nType 'help' for commands.\n> ");

    // Initial Full Draw
    for &idx in z_order.iter() {
        if windows[idx].exists { 
            draw_window_rounded(back_buffer, screen_w, screen_h, &windows[idx]);
            // If this is the terminal window, draw the terminal contents inside it
            if windows[idx].id == 0 { 
                my_terminal.draw(back_buffer, screen_w, screen_h, windows[idx].x, windows[idx].y); 
            }
        }
    }
    draw_taskbar(back_buffer, screen_w, screen_h);
    Clock::draw(back_buffer, screen_w, screen_h); 

    // Present Desktop
    for y in 0..screen_h {
        let src = y * screen_w; let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() { front_buffer[dst..dst+screen_w].copy_from_slice(&back_buffer[src..src+screen_w]); }
    }

    // --- VARIABLES FOR EVENT LOOP ---
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
        
        // Dirty Rect Setup (Optimized Drawing)
        let mut dirty_min_x = screen_w; let mut dirty_min_y = screen_h;
        let mut dirty_max_x = 0; let mut dirty_max_y = 0;

        let mut mark_dirty = |x: usize, y: usize, w: usize, h: usize| {
            let pad = 20;
            let sx = x.saturating_sub(pad); let sy = y.saturating_sub(pad);
            let ex = (x + w + pad).min(screen_w); let ey = (y + h + pad).min(screen_h);
            dirty_min_x = dirty_min_x.min(sx); dirty_min_y = dirty_min_y.min(sy);
            dirty_max_x = dirty_max_x.max(ex); dirty_max_y = dirty_max_y.max(ey);
        };

        // 1. CLOCK UPDATE (Every Second)
        if now / 1000 != last_second {
            last_second = now / 1000;
            mark_dirty(screen_w - 120, screen_h - TASKBAR_H, 120, TASKBAR_H);
            needs_redraw = true;
        }

        // 2. KEYBOARD INPUT
        // Only process keys if the Terminal Window (ID 0) is active and exists
        if windows[0].active && windows[0].exists {
            if let Some(c) = sys_read_key() {
                my_terminal.handle_key(c); // Use the new handle_key with command buffer
                mark_dirty(windows[0].x, windows[0].y, windows[0].w, windows[0].h);
                needs_redraw = true;
            }
        } else {
             let _ = sys_read_key(); // Consume key to prevent buffer overflow
        }

        // 3. MOUSE INPUT
        if left && !prev_left {
            let mut hit_z_index = None;
            let mut hit_resize = false;
            let mut hit_close = false;

            // Hit testing (Check top-most window first)
            for i in (0..MAX_WINDOWS).rev() {
                let idx = z_order[i];
                let w = &windows[idx];
                if !w.exists { continue; }
                
                // [X] Close Button
                if mx >= w.x + w.w - 35 && mx <= w.x + w.w - 5 && my >= w.y + 5 && my <= w.y + 25 {
                    hit_z_index = Some(i); hit_close = true; break;
                }
                // Resize Handle (Bottom Right)
                if mx >= w.x + w.w - 25 && mx <= w.x + w.w && my >= w.y + w.h - 25 && my <= w.y + w.h {
                    hit_z_index = Some(i); hit_resize = true; break;
                }
                // Header (Move)
                if mx >= w.x && mx < w.x + w.w && my >= w.y && my < w.y + 30 {
                    hit_z_index = Some(i); break;
                }
                // Body (Focus only)
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
                    
                    // Reorder Z-Index (Bring to front)
                    for j in i..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
                    z_order[MAX_WINDOWS-1] = idx;
                    
                    // Activate Window
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
            // Move window
            win.x = (mx as isize - drag_off_x).clamp(0, (screen_w - win.w) as isize) as usize;
            win.y = (my as isize - drag_off_y).clamp(0, (screen_h - TASKBAR_H - win.h) as isize) as usize;
            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        } else if is_resizing {
            let win = &mut windows[target_idx];
            mark_dirty(win.x, win.y, win.w, win.h);
            // Resize window
            let new_right = mx as isize + drag_off_x;
            let new_bottom = my as isize + drag_off_y;
            win.w = (new_right - win.x as isize).max(300).min((screen_w - win.x) as isize) as usize;
            win.h = (new_bottom - win.y as isize).max(200).min((screen_h - TASKBAR_H - win.y) as isize) as usize;
            mark_dirty(win.x, win.y, win.w, win.h);
            needs_redraw = true;
        }
        
        // Mouse movement always causes redraw of cursor area
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
            
            // 1. Restore Wallpaper (Clear only dirty area)
            restore_wallpaper_rect(back_buffer, screen_w, screen_h, dx, dy, dw, dh);
            
            // 2. Draw Windows (Back to Front)
            for &idx in z_order.iter() {
                if windows[idx].exists { 
                    draw_window_rounded(back_buffer, screen_w, screen_h, &windows[idx]);
                    
                    // Draw Terminal Content if applicable
                    if windows[idx].id == 0 { 
                         my_terminal.draw(back_buffer, screen_w, screen_h, windows[idx].x, windows[idx].y);
                    }
                }
            }
            
            // 3. Draw UI Overlays
            draw_taskbar(back_buffer, screen_w, screen_h);
            Clock::draw(back_buffer, screen_w, screen_h);
            draw_cursor(back_buffer, screen_w, screen_h, mx, my);
            
            // 4. Present (Blit to Hardware Buffer)
            present_rect(front_buffer, back_buffer, screen_w, screen_stride, screen_h, dx, dy, dw, dh);
            
            prev_mx = mx; prev_my = my;
        }
    }
}

// --- PUBLIC HELPERS (Used by main, clock, terminal) ---

pub fn draw_rect_simple(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, rw: usize, rh: usize, color: u32) {
    for row in 0..rh {
        let sy = y + row; if sy >= h { break; }
        for col in 0..rw { 
            let sx = x + col; 
            if sx >= w { break; } 
            fb[sy * w + sx] = color; 
        }
    }
}

pub fn draw_char(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, c: char, color: u32) {
    if let Some(raster) = get_raster(c, FontWeight::Regular, RasterHeight::Size16) {
        for (ri, row) in raster.raster().iter().enumerate() {
            for (ci, val) in row.iter().enumerate() {
                if *val > 10 { 
                    let px = x+ci; let py = y+ri; 
                    if px < w && py < h { fb[py*w+px] = color; } 
                }
            }
        }
    }
}

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

fn draw_window_rounded(fb: &mut [u32], w: usize, h: usize, win: &Window) {
    let color = if win.active { 0xFF2A2A2A } else { 0xFF1A1A1A };
    let border = if win.active { 0xFF00AAFF } else { 0xFF444444 };
    let radius: isize = 12; let radius_sq = radius * radius;

    for row in 0..win.h {
        let sy = win.y + row; if sy >= h { break; }
        for col in 0..win.w {
            let sx = win.x + col; if sx >= w { break; }
            let cx = col as isize; let cy = row as isize;
            
            // Rounded Corners Logic
            let mut in_corner = false;
            if cx < radius && cy < radius { if (radius-cx-1).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy < radius { if (cx-(win.w as isize-radius)).pow(2)+(radius-cy-1).pow(2) > radius_sq { in_corner = true; } }
            else if cx < radius && cy >= (win.h as isize)-radius { if (radius-cx-1).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            else if cx >= (win.w as isize)-radius && cy >= (win.h as isize)-radius { if (cx-(win.w as isize-radius)).pow(2)+(cy-((win.h as isize)-radius)).pow(2) > radius_sq { in_corner = true; } }
            if in_corner { continue; }

            let mut c = if row < 30 { 0xFF3A3A3A } else { color }; // Title bar vs Body
            if col < 2 || col >= win.w-2 || row >= win.h-2 || row < 2 || (row >= 28 && row < 30) { c = border; }
            // Resize Handle Indicators
            if row > win.h - 15 && col > win.w - 15 && (row + col) % 4 == 0 { c = 0xFF888888; }
            fb[sy * w + sx] = c;
        }
    }
    // Close Button
    draw_rect_simple(fb, w, h, win.x + win.w - 35, win.y + 5, 30, 20, 0xFFFF4444);
    draw_text(fb, w, h, win.x + win.w - 25, win.y + 7, "X", 0xFFFFFFFF);
    // Title
    draw_text(fb, w, h, win.x + 15, win.y + 7, win.title, 0xFFFFFFFF);
}

fn draw_text(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, text: &str, color: u32) {
    let mut cx = x; for c in text.chars() { draw_char(fb, w, h, cx, y, c, color); cx += 9; }
}

fn draw_cursor(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
    for i in 0..10 {
        if x+i < w && y+i < h { fb[(y+i)*w+(x+i)] = 0xFFFFFFFF; }
        if x+i < w && y < h { fb[y*w+(x+i)] = 0xFFFFFFFF; }
        if x < w && y+i < h { fb[(y+i)*w+x] = 0xFFFFFFFF; }
    }
}

fn restore_wallpaper_rect(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let sy = y + row; if sy >= h { break; }
        for col in 0..dw {
            let sx = x + col; if sx >= w { break; }
            // Simple Gradient Wallpaper
            let b = (sy * 255 / h) as u32; 
            let g = (sx * 100 / w) as u32;
            fb[sy * w + sx] = 0xFF000000 | (g << 8) | b;
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

#[derive(Clone, Copy)]
struct Window { id: usize, x: usize, y: usize, w: usize, h: usize, title: &'static str, active: bool, exists: bool }

#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { sys_exit(1); }