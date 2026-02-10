#![no_std]
#![no_main]

mod syscalls;
mod console;

use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
const MAX_WINDOWS: usize = 3;

use core::fmt::Write;
struct ConsoleWriter;
impl core::fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() { syscalls::sys_print(c); }
        Ok(())
    }
}

// --- STRUCTURES ---
#[derive(Clone, Copy)]
struct Window {
    id: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    title: &'static str,
    active: bool,
}

// --- MATH & COLORS ---
fn blend_color(bg: u32, fg: u32, alpha: u32) -> u32 {
    let inv = 256 - alpha; 
    let r = (((fg >> 16) & 0xFF) * alpha + ((bg >> 16) & 0xFF) * inv) >> 8;
    let g = (((fg >> 8) & 0xFF) * alpha + ((bg >> 8) & 0xFF) * inv) >> 8;
    let b = ((fg & 0xFF) * alpha + (bg & 0xFF) * inv) >> 8;
    (r << 16) | (g << 8) | b
}

// --- GRAPHICS ENGINE ---

// 1. Wallpaper Generator (Matrix Grid)
fn restore_wallpaper_rect(fb: &mut [u32], fb_w: usize, fb_h: usize, x: usize, y: usize, w: usize, h: usize) {
    for row in 0..h {
        let screen_y = y + row;
        if screen_y >= fb_h { break; }
        let offset = screen_y * fb_w + x;
        for col in 0..w {
            if x + col >= fb_w { break; }
            let grid_x = x + col;
            
            // Gradient Background
            let mut r = 0; 
            let mut g = 0; 
            let mut b = (screen_y * 60 / fb_h) as u32 + 20;
            
            // Grid Lines
            if grid_x % 40 == 0 || screen_y % 40 == 0 { r = 0; g = 30; b = 80; }
            
            fb[offset + col] = (r << 16) | (g << 8) | b;
        }
    }
}

// 2. Drop Shadow (Alpha Blended)
fn draw_shadow(fb: &mut [u32], fb_w: usize, fb_h: usize, x: usize, y: usize, w: usize, h: usize) {
    let shadow_color = 0x000000;
    let alpha = 90; 
    for row in 0..h {
        let screen_y = y + row;
        if screen_y >= fb_h { break; }
        let offset = screen_y * fb_w + x;
        for col in 0..w {
            if x + col >= fb_w { break; }
            let bg = fb[offset + col];
            fb[offset + col] = blend_color(bg, shadow_color, alpha);
        }
    }
}

// 3. Hardware Cursor (Drawn directly to Front Buffer)
fn draw_cursor_front(front: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
    let color = 0x0000FFFF;
    if x < w && y < h { front[y * w + x] = color; }
    for i in 1..5 {
        if x + i < w { front[y * w + x + i] = color; }
        if x >= i { front[y * w + x - i] = color; }
        if y + i < h { front[(y + i) * w + x] = color; }
        if y >= i { front[(y - i) * w + x] = color; }
    }
}

// 4. Buffer Swap (Dirty Rect Optimization)
fn present_rect(
    front: &mut [u32], back: &[u32], stride: usize, height: usize,
    x: usize, y: usize, w: usize, h: usize
) {
    // VSYNC / TURBO: We now assume 'sys_blit' handles the heavy lifting
    // But in userspace, we simply copy.
    for row in 0..h {
        let screen_y = y + row;
        if screen_y >= height { break; }
        let offset = screen_y * stride + x;
        let count = w.min(stride - x);
        if offset + count > front.len() { break; }
        front[offset..offset + count].copy_from_slice(&back[offset..offset + count]);
    }
}

// --- FONT RENDERING ---
fn draw_char(fb: &mut [u32], fb_w: usize, x: usize, y: usize, c: char, color: u32) {
    if let Some(char_raster) = get_raster(c, FontWeight::Regular, RasterHeight::Size16) {
        for (row_i, row) in char_raster.raster().iter().enumerate() {
            for (col_i, intensity) in row.iter().enumerate() {
                if *intensity > 0 {
                    let px = x + col_i;
                    let py = y + row_i;
                    if px < fb_w { fb[py * fb_w + px] = color; }
                }
            }
        }
    }
}

fn draw_text(fb: &mut [u32], fb_w: usize, x: usize, y: usize, text: &str, color: u32) {
    let mut curr_x = x;
    for c in text.chars() {
        draw_char(fb, fb_w, curr_x, y, c, color);
        curr_x += 10;
    }
}

// --- WINDOW PAINTER ---
fn draw_window(fb: &mut [u32], fb_w: usize, fb_h: usize, win: &Window) {
    let title_bg = if win.active { 0x00000080 } else { 0x00202020 };
    let body_bg = 0x00E0E0E0; 
    let border = if win.active { 0x0000AA00 } else { 0x00555555 };
    let text_col = if win.active { 0x00FFFFFF } else { 0x00AAAAAA };

    let radius = 12;
    let r_sq = radius * radius;

    for row in 0..win.h {
        let screen_y = win.y + row;
        if screen_y >= fb_h { break; }
        let offset = screen_y * fb_w + win.x;
        
        for col in 0..win.w {
            if win.x + col >= fb_w { break; }
            
            let mut color = body_bg;
            if row < 30 { color = title_bg; }
            if col < 1 || row < 1 || col >= win.w-1 || row >= win.h-1 { color = border; }

            // Rounded Corners (Transparent via Skip)
            if row < radius {
                if col < radius {
                    if (radius - col)*(radius - col) + (radius - row)*(radius - row) > r_sq { continue; }
                } else if col >= win.w - radius {
                    if (col - (win.w - radius))*(col - (win.w - radius)) + (radius - row)*(radius - row) > r_sq { continue; }
                }
            } else if row >= win.h - radius {
                if col < radius {
                    if (radius - col)*(radius - col) + (row - (win.h - radius))*(row - (win.h - radius)) > r_sq { continue; }
                } else if col >= win.w - radius {
                    if (col - (win.w - radius))*(col - (win.w - radius)) + (row - (win.h - radius))*(row - (win.h - radius)) > r_sq { continue; }
                }
            }

            // Resize Grip (Bottom Right)
            if row >= win.h - 15 && col >= win.w - 15 {
                if (col + row) % 4 == 0 { color = 0x00888888; } 
            }
            
            fb[offset + col] = color;
        }
    }

    draw_text(fb, fb_w, win.x + 15, win.y + 8, win.title, text_col);
    
    // Draw Window Content
    if win.id == 0 {
        draw_text(fb, fb_w, win.x + 15, win.y + 50, "$ neofetch", 0x0);
        draw_text(fb, fb_w, win.x + 15, win.y + 70, "NyxOS v0.3", 0x0);
    } else if win.id == 1 {
        draw_text(fb, fb_w, win.x + 15, win.y + 50, "CPU: 2% Used", 0x0);
        draw_text(fb, fb_w, win.x + 15, win.y + 70, "RAM: 32MB", 0x0);
    } else {
        draw_text(fb, fb_w, win.x + 15, win.y + 50, "Status: OK", 0x0);
        draw_text(fb, fb_w, win.x + 15, win.y + 70, "FPS: 60 Locked", 0x00008800);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut _console = ConsoleWriter;
    
    let (mut screen_w, mut screen_h) = syscalls::sys_get_screen_size();
    if screen_w == 0 { screen_w = 1024; screen_h = 768; }
    if screen_w > MAX_W { screen_w = MAX_W; }
    if screen_h > MAX_H { screen_h = MAX_H; }

    let fb_ptr = syscalls::sys_map_framebuffer();
    if fb_ptr == 0 { loop {} }
    let front_buffer = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_w * screen_h) };

    static mut BACK_BUFFER_STORE: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];
    let back_buffer = unsafe { &mut BACK_BUFFER_STORE[..screen_w * screen_h] };

    restore_wallpaper_rect(back_buffer, screen_w, screen_h, 0, 0, screen_w, screen_h);
    front_buffer.copy_from_slice(back_buffer);

    let mut windows = [
        Window { id: 0, x: 50,  y: 50,  w: 300, h: 200, title: "Terminal", active: true },
        Window { id: 1, x: 400, y: 100, w: 300, h: 200, title: "System Monitor", active: false },
        Window { id: 2, x: 200, y: 350, w: 300, h: 200, title: "Help Info", active: false },
    ];
    let mut z_order = [0, 1, 2];

    let mut is_dragging = false;
    let mut is_resizing = false;
    let mut drag_target_idx = 0; 
    
    // Drag/Resize State
    let mut drag_offset_x: isize = 0;
    let mut drag_offset_y: isize = 0;
    let mut resize_start_w: usize = 0;
    let mut resize_start_h: usize = 0;
    let mut resize_start_mx: usize = 0;
    let mut resize_start_my: usize = 0;

    let mut prev_left = false;
    let mut prev_mx = 0;
    let mut prev_my = 0;

    let mut last_frame_time = syscalls::sys_get_time();
    let ms_per_frame = 1000 / 60; // Target 60 FPS
    let mut force_redraw = true;

    // Track previous active rect for dirty calculations
    let mut prev_win_rect = (0, 0, 0, 0);

    loop {
        // --- VSYNC / FPS LOCK ---
        // Busy-wait until enough time has passed for the next frame
        // This ensures smooth animation and prevents tearing by not drawing faster than refresh
        let current_time = syscalls::sys_get_time();
        let elapsed = current_time.wrapping_sub(last_frame_time);

        if elapsed < ms_per_frame {
            // Optional: Yield CPU here if we had a sys_yield syscall
             unsafe { core::arch::asm!("nop"); } 
            continue;
        }
        last_frame_time = current_time;

        let (mx, my, left, _right) = syscalls::sys_get_mouse();
        let mut input_changed = false;

        // --- CLICK HANDLING ---
        if left && !prev_left {
            if !is_dragging && !is_resizing {
                for i in (0..MAX_WINDOWS).rev() {
                    let win_idx = z_order[i];
                    
                    // Copy data to avoid borrowing conflict
                    let (wx, wy, ww, wh) = {
                        let w = &windows[win_idx];
                        (w.x, w.y, w.w, w.h)
                    };
                    
                    if mx >= wx && mx < wx + ww && my >= wy && my < wy + wh {
                        input_changed = true;
                        force_redraw = true; // FORCE FULL REDRAW (Fixes Sticky Blue Window)

                        // 1. Bring to Front
                        for j in i..(MAX_WINDOWS-1) { z_order[j] = z_order[j+1]; }
                        z_order[MAX_WINDOWS-1] = win_idx;
                        
                        // 2. Set Active
                        for w in windows.iter_mut() { w.active = false; }
                        windows[win_idx].active = true;

                        // 3. Interaction Check
                        if mx >= wx + ww - 20 && my >= wy + wh - 20 {
                            is_resizing = true;
                            drag_target_idx = win_idx;
                            resize_start_w = ww;
                            resize_start_h = wh;
                            resize_start_mx = mx;
                            resize_start_my = my;
                            prev_win_rect = (wx, wy, ww, wh);
                        } else if my < wy + 30 {
                            is_dragging = true;
                            drag_target_idx = win_idx;
                            drag_offset_x = (mx as isize) - (wx as isize);
                            drag_offset_y = (my as isize) - (wy as isize);
                            prev_win_rect = (wx, wy, ww, wh);
                        }
                        break; 
                    }
                }
            }
        }
        
        if !left && (is_dragging || is_resizing) {
            is_dragging = false;
            is_resizing = false;
            input_changed = true;
        }
        prev_left = left;

        // --- LOGIC UPDATE ---
        if is_dragging {
            let win = &mut windows[drag_target_idx];
            let raw_x = (mx as isize) - drag_offset_x;
            let raw_y = (my as isize) - drag_offset_y;
            
            let new_x = raw_x.clamp(0, (screen_w - win.w) as isize) as usize;
            let new_y = raw_y.clamp(0, (screen_h - win.h) as isize) as usize;

            if new_x != win.x || new_y != win.y {
                win.x = new_x;
                win.y = new_y;
            }
        }

        if is_resizing {
            let win = &mut windows[drag_target_idx];
            let delta_w = (mx as isize) - (resize_start_mx as isize);
            let delta_h = (my as isize) - (resize_start_my as isize);

            let new_w = (resize_start_w as isize + delta_w).max(150) as usize;
            let new_h = (resize_start_h as isize + delta_h).max(100) as usize;
            
            let new_w = new_w.min(screen_w - win.x);
            let new_h = new_h.min(screen_h - win.y);

            if new_w != win.w || new_h != win.h {
                win.w = new_w;
                win.h = new_h;
            }
        }

        // --- RENDER ---
        let visual_change = is_dragging || is_resizing || input_changed || mx != prev_mx || my != prev_my || force_redraw;

        if visual_change {
            
            // 1. Erase Cursor
            restore_wallpaper_rect(back_buffer, screen_w, screen_h,
                prev_mx.saturating_sub(12), prev_my.saturating_sub(12), 24, 24);
            
            // 2. Erase Active Window Trail
            if is_dragging || is_resizing || input_changed || force_redraw {
                let (ox, oy, ow, oh) = prev_win_rect;
                restore_wallpaper_rect(back_buffer, screen_w, screen_h, ox, oy, ow + 25, oh + 25);
            }

            // 3. Draw All Windows (Bottom to Top)
            for &idx in z_order.iter() {
                let win = &windows[idx];
                draw_shadow(back_buffer, screen_w, screen_h, win.x + 8, win.y + 8, win.w, win.h);
                draw_window(back_buffer, screen_w, screen_h, win);
            }

            // 4. Dirty Rect Calc
            let win = &windows[drag_target_idx];
            let (ox, oy, ow, oh) = prev_win_rect;
            
            let mut min_x = ox.min(win.x).min(prev_mx.saturating_sub(20)).min(mx.saturating_sub(20));
            let mut min_y = oy.min(win.y).min(prev_my.saturating_sub(20)).min(my.saturating_sub(20));
            let mut max_x = (ox + ow + 25).max(win.x + win.w + 25).max(prev_mx + 20).max(mx + 20);
            let mut max_y = (oy + oh + 25).max(win.y + win.h + 25).max(prev_my + 20).max(my + 20);

            // SPECIAL CASE: Focus Switch = Full Screen Refresh
            if force_redraw {
                min_x = 0; min_y = 0;
                max_x = screen_w; max_y = screen_h;
            }

            let dirty_x = min_x.clamp(0, screen_w);
            let dirty_y = min_y.clamp(0, screen_h);
            let dirty_w = (max_x - dirty_x).clamp(0, screen_w - dirty_x);
            let dirty_h = (max_y - dirty_y).clamp(0, screen_h - dirty_y);

            present_rect(front_buffer, back_buffer, screen_w, screen_h, dirty_x, dirty_y, dirty_w, dirty_h);
            draw_cursor_front(front_buffer, screen_w, screen_h, mx, my);

            // Update State
            if is_dragging || is_resizing {
                 prev_win_rect = (win.x, win.y, win.w, win.h);
            } else {
                 let focus = &windows[z_order[MAX_WINDOWS-1]];
                 prev_win_rect = (focus.x, focus.y, focus.w, focus.h);
            }
            prev_mx = mx;
            prev_my = my;
            force_redraw = false;
        }

        for _ in 0..100 { unsafe { core::arch::asm!("nop"); } }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscalls::sys_exit(1);
}