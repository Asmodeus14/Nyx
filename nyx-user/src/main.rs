#![no_std]
#![no_main]

mod syscalls;
mod console;

use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
use syscalls::*;

const MAX_WINDOWS: usize = 3;

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    if screen_w == 0 || screen_h == 0 { sys_exit(1); }

    let fb_ptr = sys_map_framebuffer();
    if fb_ptr == 0 { sys_exit(1); }
    
    let buffer_size_bytes = (screen_w * screen_h * 4) + 4096;
    let back_ptr = sys_alloc(buffer_size_bytes);
    
    // Safety: If allocation failed or syscall isn't handled (returns 9), stop.
    if back_ptr == 0 || back_ptr == 9 { loop {} }

    let front_buffer = unsafe { 
        core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) 
    };
    let back_buffer = unsafe { 
        core::slice::from_raw_parts_mut(back_ptr as *mut u32, screen_w * screen_h) 
    };

    // 1. Draw Wallpaper
    restore_wallpaper_rect(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h);

    // 2. Define Windows (MOVED UP)
    let mut windows = [
        Window { id: 0, x: 100, y: 100, w: 500, h: 350, title: "Nyx Terminal", active: true },
        Window { id: 1, x: 650, y: 150, w: 400, h: 300, title: "System Monitor", active: false },
        Window { id: 2, x: 300, y: 500, w: 400, h: 250, title: "Help Info", active: false },
    ];
    let mut z_order = [0, 1, 2];

    // 3. INITIAL DRAW (The Fix)
    // We must draw the windows onto the wallpaper BEFORE the first frame is shown.
    for &idx in z_order.iter() {
        draw_window_rounded(back_buffer, screen_w, screen_h, &windows[idx]);
    }

    // 4. Initial Copy to Screen
    for y in 0..screen_h {
        let src = y * screen_w;
        let dst = y * screen_stride;
        if dst + screen_w <= front_buffer.len() && src + screen_w <= back_buffer.len() {
            front_buffer[dst..dst+screen_w].copy_from_slice(&back_buffer[src..src+screen_w]);
        }
    }

    // --- SETUP VARIABLES ---
    let mut is_dragging = false;
    let mut drag_target_idx = 0; 
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
        if now.wrapping_sub(last_frame) < ms_per_frame {
            unsafe { core::arch::asm!("nop"); }
            continue;
        }
        last_frame = now;

        let (mx_raw, my_raw, left, _right) = sys_get_mouse();
        let mx = mx_raw.clamp(0, screen_w - 1);
        let my = my_raw.clamp(0, screen_h - 1);
        let mut needs_redraw = false;
        
        let mut dirty_min_x = screen_w;
        let mut dirty_min_y = screen_h;
        let mut dirty_max_x = 0;
        let mut dirty_max_y = 0;

        let mut mark_dirty = |x: usize, y: usize, w: usize, h: usize| {
            let pad = 10;
            let sx = x.saturating_sub(pad);
            let sy = y.saturating_sub(pad);
            let ex = (x + w + pad).min(screen_w);
            let ey = (y + h + pad).min(screen_h);
            dirty_min_x = dirty_min_x.min(sx);
            dirty_min_y = dirty_min_y.min(sy);
            dirty_max_x = dirty_max_x.max(ex);
            dirty_max_y = dirty_max_y.max(ey);
        };

        // Clock Check
        let current_sec = now / 1000;
        if current_sec != last_second_draw {
            mark_dirty(screen_w.saturating_sub(160), 0, 160, 30);
            needs_redraw = true;
            last_second_draw = current_sec;
        }

        // Mouse Logic
        if left && !prev_left {
            let mut hit_z_index = None;
            for i in (0..MAX_WINDOWS).rev() {
                let idx = z_order[i];
                let w = &windows[idx];
                if mx >= w.x && mx < w.x + w.w && my >= w.y && my < w.y + 30 {
                    hit_z_index = Some(i);
                    break;
                }
            }
            if let Some(i) = hit_z_index {
                let idx = z_order[i];
                let old_w = windows[prev_active_idx];
                mark_dirty(old_w.x, old_w.y, old_w.w, old_w.h);
                let new_w = windows[idx];
                mark_dirty(new_w.x, new_w.y, new_w.w, new_w.h);
                
                is_dragging = true;
                drag_target_idx = idx;
                drag_off_x = mx as isize - new_w.x as isize;
                drag_off_y = my as isize - new_w.y as isize;
                
                for j in i..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
                z_order[MAX_WINDOWS-1] = idx;
                
                for win in windows.iter_mut() { win.active = false; }
                windows[idx].active = true;
                prev_active_idx = idx;
                needs_redraw = true;
            }
        } else if !left && is_dragging {
            is_dragging = false;
        }

        if is_dragging {
            let win = &mut windows[drag_target_idx];
            let new_x = (mx as isize - drag_off_x).clamp(0, (screen_w - win.w) as isize) as usize;
            let new_y = (my as isize - drag_off_y).clamp(0, (screen_h - win.h) as isize) as usize;
            if new_x != win.x || new_y != win.y {
                mark_dirty(win.x, win.y, win.w, win.h);
                win.x = new_x; win.y = new_y;
                mark_dirty(win.x, win.y, win.w, win.h);
                needs_redraw = true;
            }
        }
        
        if mx != prev_mx || my != prev_my { 
            mark_dirty(prev_mx, prev_my, 15, 15); 
            mark_dirty(mx, my, 15, 15);
            needs_redraw = true; 
        }
        prev_left = left;

        // Render Frame
        if needs_redraw {
            if dirty_max_x > dirty_min_x && dirty_max_y > dirty_min_y {
                let dx = dirty_min_x.clamp(0, screen_w);
                let dy = dirty_min_y.clamp(0, screen_h);
                let dw = (dirty_max_x - dx).clamp(0, screen_w - dx);
                let dh = (dirty_max_y - dy).clamp(0, screen_h - dy);

                if dw > 0 && dh > 0 {
                    restore_wallpaper_rect(back_buffer, screen_w, screen_h, dx, dy, dw, dh);

                    for &idx in z_order.iter() {
                        let w = &windows[idx];
                        if w.x < dx + dw && w.x + w.w > dx && w.y < dy + dh && w.y + w.h > dy {
                            draw_window_rounded(back_buffer, screen_w, screen_h, w);
                        }
                    }

                    if dx + dw > screen_w - 160 && dy < 30 {
                        draw_clock(back_buffer, screen_w, screen_h, now);
                    }
                    
                    draw_cursor(back_buffer, screen_w, screen_h, mx, my);
                    present_rect(front_buffer, back_buffer, screen_w, screen_stride, screen_h, dx, dy, dw, dh);
                }
            }
            prev_mx = mx; prev_my = my;
        }
    }
}

// --- HELPERS ---

fn draw_clock(fb: &mut [u32], w: usize, h: usize, ticks: u64) {
    let seconds = ticks / 1000;
    let mins = (seconds / 60) % 60;
    let hrs = (seconds / 3600) % 24;
    let sec_disp = seconds % 60;

    let start_x = w.saturating_sub(160);
    for y in 0..30 {
        if y >= h { break; }
        for x in start_x..w {
            if x < w { fb[y * w + x] = 0xFF202020; }
        }
    }
    
    let tx = w.saturating_sub(150);
    let ty = 8;
    
    draw_digit(fb, w, h, tx, ty, (hrs / 10) as u8);
    draw_digit(fb, w, h, tx + 10, ty, (hrs % 10) as u8);
    draw_rect_simple(fb, w, h, tx + 20, ty + 5, 2, 2, 0xFFFFFFFF);
    draw_rect_simple(fb, w, h, tx + 20, ty + 12, 2, 2, 0xFFFFFFFF);
    draw_digit(fb, w, h, tx + 25, ty, (mins / 10) as u8);
    draw_digit(fb, w, h, tx + 35, ty, (mins % 10) as u8);
    draw_rect_simple(fb, w, h, tx + 45, ty + 5, 2, 2, 0xFFFFFFFF);
    draw_rect_simple(fb, w, h, tx + 45, ty + 12, 2, 2, 0xFFFFFFFF);
    draw_digit(fb, w, h, tx + 50, ty, (sec_disp / 10) as u8);
    draw_digit(fb, w, h, tx + 60, ty, (sec_disp % 10) as u8);
}

fn draw_digit(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, digit: u8) {
    let c = (digit + 48) as char;
    draw_char(fb, w, h, x, y, c, 0xFFFFFFFF);
}

fn draw_char(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, c: char, color: u32) {
    if let Some(raster) = get_raster(c, FontWeight::Regular, RasterHeight::Size16) {
        for (ri, row) in raster.raster().iter().enumerate() {
            for (ci, val) in row.iter().enumerate() {
                if *val > 10 {
                    let px = x + ci;
                    let py = y + ri;
                    if px < w && py < h { fb[py * w + px] = color; }
                }
            }
        }
    }
}

fn present_rect(front: &mut [u32], back: &[u32], w: usize, stride: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let screen_y = y + row;
        if screen_y >= h { break; }
        let src_idx = screen_y * w + x;
        let dst_idx = screen_y * stride + x;
        if dst_idx + dw <= front.len() && src_idx + dw <= back.len() {
            front[dst_idx..dst_idx + dw].copy_from_slice(&back[src_idx..src_idx + dw]);
        }
    }
}

fn restore_wallpaper_rect(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, dw: usize, dh: usize) {
    for row in 0..dh {
        let sy = y + row;
        if sy >= h { break; }
        for col in 0..dw {
            let sx = x + col;
            if sx >= w { break; }
            let b = (sy * 255 / h) as u32;
            let g = (sx * 100 / w) as u32;
            fb[sy * w + sx] = 0xFF000000 | (g << 8) | b;
        }
    }
}

#[derive(Clone, Copy)]
struct Window { id: usize, x: usize, y: usize, w: usize, h: usize, title: &'static str, active: bool }

fn draw_window_rounded(fb: &mut [u32], w: usize, h: usize, win: &Window) {
    let color = if win.active { 0xFF303030 } else { 0xFF202020 };
    let border = if win.active { 0xFF00AAFF } else { 0xFF444444 };
    let radius: isize = 12; 
    let radius_sq = radius * radius;

    for row in 0..win.h {
        let sy = win.y + row;
        if sy >= h { break; }
        
        let mut is_header = false;
        if row < 30 { is_header = true; }

        for col in 0..win.w {
            let sx = win.x + col;
            if sx >= w { break; }

            // --- CORNER CHECK ---
            let mut in_corner = false;
            let cx: isize = col as isize;
            let cy: isize = row as isize;
            let width: isize = win.w as isize;
            let height: isize = win.h as isize;

            // Top-Left
            if cx < radius && cy < radius {
                if (radius - cx - 1).pow(2) + (radius - cy - 1).pow(2) > radius_sq { in_corner = true; }
            }
            // Top-Right
            else if cx >= width - radius && cy < radius {
                if (cx - (width - radius)).pow(2) + (radius - cy - 1).pow(2) > radius_sq { in_corner = true; }
            }
            // Bottom-Left
            else if cx < radius && cy >= height - radius {
                if (radius - cx - 1).pow(2) + (cy - (height - radius)).pow(2) > radius_sq { in_corner = true; }
            }
            // Bottom-Right
            else if cx >= width - radius && cy >= height - radius {
                if (cx - (width - radius)).pow(2) + (cy - (height - radius)).pow(2) > radius_sq { in_corner = true; }
            }

            if in_corner { continue; } 

            let mut c = if is_header { 0xFF404040 } else { color };
            
            if col < 2 || col >= win.w-2 || row >= win.h-2 || row < 2 || (row >= 28 && row < 30) { 
                c = border; 
            }
            
            fb[sy * w + sx] = c;
        }
    }
    draw_text(fb, w, h, win.x + 15, win.y + 7, win.title, 0xFFFFFFFF);
}

fn draw_cursor(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
    let color = 0xFFFFFFFF;
    for i in 0..10 {
        if x + i < w && y + i < h { fb[(y+i)*w + (x+i)] = color; }
        if x + i < w && y < h { fb[y*w + (x+i)] = color; }
        if x < w && y + i < h { fb[(y+i)*w + x] = color; }
    }
}

fn draw_text(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, text: &str, color: u32) {
    let mut cx = x;
    for c in text.chars() {
        draw_char(fb, w, h, cx, y, c, color);
        cx += 9;
    }
}

fn draw_rect_simple(fb: &mut [u32], w: usize, h: usize, x: usize, y: usize, rw: usize, rh: usize, color: u32) {
    for row in 0..rh {
        let sy = y + row;
        if sy >= h { break; }
        for col in 0..rw {
            let sx = x + col;
            if sx >= w { break; }
            let idx = sy * w + sx;
            if idx < fb.len() { fb[idx] = color; }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { sys_exit(1); }