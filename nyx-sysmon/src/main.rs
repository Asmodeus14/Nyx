#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const WIDTH: usize = 700;
const HEIGHT: usize = 480;

#[derive(PartialEq, Clone, Copy)]
enum SysMonState { Vitals, Tasks, Bootlog }

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(512); // Needs a bit more RAM for bootlogs
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 512 * 4096); }

    const COMPOSITOR_PID: u64 = 4;
    let total_size = core::mem::size_of::<WindowHeader>() + (WIDTH * HEIGHT * 4);
    let shm_id = sys_create_shm(total_size);
    if shm_id == 0 { sys_exit(1); }

    let buffer_ptr = sys_map_shm(shm_id) as *mut u8;
    let header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };

    header.magic = WIN_MAGIC;
    header.requested_x = -1; // Let Compositor Cascade it!
    header.requested_y = -1;
    header.width = WIDTH as u32;
    header.height = HEIGHT as u32;
    header.flags = WIN_FLAG_NONE;

    let title = b"Nyx System Monitor";
    header.title.fill(0);
    header.title[..title.len()].copy_from_slice(title);

    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) { sys_exit(1); }
    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop { if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED { break; } }

    let pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
    let screen = unsafe { core::slice::from_raw_parts_mut(pixels_ptr, WIDTH * HEIGHT) };
    let mut canvas = Canvas::new(screen, WIDTH, HEIGHT);

    let mut state = SysMonState::Vitals;
    let mut needs_redraw = true;

    // --- SYSTEM DATA CACHE ---
    let mut last_update_time = 0;
    
    // Vitals Data
    let mut entity_stats = [0.0f32; 4];
    let mut active_cores = 0;

    // Task & Hardware Data
    let mut sys_info: SystemInfo = unsafe { core::mem::zeroed() };

    // Bootlog Data
    let mut bootlog_buf = alloc::vec![0u8; 16384];
    let mut bootlog_lines: Vec<String> = Vec::new();
    let mut bootlog_last_len = 0;
    let mut bootlog_scroll = 0;

    loop {
        let now = sys_get_time();

        // 1. NON-BLOCKING DATA REFRESH (Every 500ms)
        if now.wrapping_sub(last_update_time) > 500 {
            // Update Vitals
            sys_get_entity_stats(&mut entity_stats);
            active_cores = sys_get_active_cores();
            
            // Update Hardware & Tasks
            sys_get_system_info(&mut sys_info);

            // Update Bootlog
            let len = sys_get_boot_logs(&mut bootlog_buf);
            if len != bootlog_last_len {
                bootlog_last_len = len;
                bootlog_lines.clear();
                if let Ok(text) = core::str::from_utf8(&bootlog_buf[..len]) {
                    for line in text.split('\n') {
                        if !line.trim().is_empty() {
                            bootlog_lines.push(String::from(line));
                        }
                    }
                }
                // Auto-scroll to bottom on new logs
                let max_lines = 24;
                bootlog_scroll = bootlog_lines.len().saturating_sub(max_lines);
            }

            last_update_time = now;
            needs_redraw = true; // Force UI to visually update!
        }

        // 2. RENDER THE UI
        if needs_redraw {
            // Background & Sidebar
            canvas.fill_rect(0, 0, WIDTH, HEIGHT, Color::WARM_BG);
            canvas.fill_rect(0, 0, 150, HEIGHT, Color::WARM_SURFACE);
            canvas.fill_rect(150, 0, 1, HEIGHT, Color::WARM_BORDER); 

            canvas.print_str(15, 20, "SYS MON", Color::ACCENT_PRIMARY, 2);

            // Draw Tabs
            let tabs = [
                (SysMonState::Vitals, "Entity Vitals", 80),
                (SysMonState::Tasks, "Task Scheduler", 120),
                (SysMonState::Bootlog, "Kernel Bootlog", 160),
            ];

            for (s, text, y) in tabs.iter() {
                let is_active = state == *s;
                if is_active { canvas.fill_rect(10, *y - 5, 130, 30, Color::ACCENT_PRIMARY); }
                let text_color = if is_active { Color::WHITE } else { Color::TEXT_MUTED };
                canvas.print_str(20, *y + 2, text, text_color, 1);
            }

            let cx = 170; let cw = WIDTH - cx - 20;

            match state {
                SysMonState::Vitals => {
                    canvas.print_str(cx, 20, "Entity Live Telemetry", Color::TEXT_DARK, 2);
                    
                    let core_text = alloc::format!("Architecture: x86_64 SMP | Active Hardware Cores: {}", active_cores);
                    canvas.print_str(cx, 60, &core_text, Color::TEXT_MUTED, 1);
                    canvas.print_str(cx, 80, "NVMe Lossless Compression: ACTIVE", Color::ACCENT_GREEN, 1);

                    // Draw Data Bars
                    let bars = [
                        ("Energy", entity_stats[0], 130, 0xFF_E74C3C),
                        ("Entropy", entity_stats[1], 180, 0xFF_3498DB),
                        ("Stability", entity_stats[2], 230, 0xFF_2ECC71),
                        ("Curiosity", entity_stats[3], 280, 0xFF_F1C40F),
                    ];

                    for (label, val, y, color) in bars.iter() {
                        let text = alloc::format!("{}: {:.2}", label, val);
                        canvas.print_str(cx, *y, &text, Color::TEXT_DARK, 1);
                        
                        // Background Track
                        canvas.fill_rect(cx, *y + 20, 300, 12, Color::WARM_BORDER);
                        // Filled Bar
                        let fill_w = ((val.clamp(0.0, 100.0) / 100.0) * 300.0) as usize;
                        if fill_w > 0 {
                            canvas.fill_rect(cx, *y + 20, fill_w, 12, *color);
                        }
                    }
                },
                SysMonState::Tasks => {
                    canvas.print_str(cx, 20, "Hardware & Scheduler", Color::TEXT_DARK, 2);
                    
                    // Hardware Thermals
                    let temp_color = if sys_info.current_temp >= 80 { 0xFF_E74C3C } else { Color::ACCENT_GREEN };
                    canvas.print_str(cx, 70, &alloc::format!("Silicon Temp: {} C", sys_info.current_temp), temp_color, 1);
                    canvas.print_str(cx, 90, &alloc::format!("CPU Fan Speed: {} RPM", sys_info.cpu_fan_rpm), Color::TEXT_DARK, 1);
                    canvas.print_str(cx, 110, &alloc::format!("GPU Fan Speed: {} RPM", sys_info.gpu_fan_rpm), Color::TEXT_DARK, 1);

                    // Tasks
                    canvas.fill_rect(cx, 140, cw, 1, Color::WARM_BORDER);
                    canvas.print_str(cx, 155, &alloc::format!("Total Kernel Tasks: {}", sys_info.task_count), Color::TEXT_DARK, 1);
                    
                    let mut ty = 185;
                    let limit = core::cmp::min(sys_info.task_count as usize, 10);
                    for i in 0..limit {
                        let t = &sys_info.tasks[i];
                        let name = core::str::from_utf8(&t.name).unwrap_or("Unknown").trim_matches(char::from(0));
                        let t_str = alloc::format!("PID {:02} | {} | {} Ticks", t.pid, name, t.cpu_ticks);
                        canvas.print_str(cx, ty, &t_str, Color::TEXT_MUTED, 1);
                        ty += 20;
                    }
                },
                SysMonState::Bootlog => {
                    canvas.print_str(cx, 20, "Kernel Ring Buffer (dmesg)", Color::TEXT_DARK, 2);
                    
                    let log_y = 60; let log_h = HEIGHT - 80;
                    canvas.fill_rect(cx, log_y, cw, log_h, 0xFF_1E1E1E); // Dark Terminal Box
                    
                    let max_lines = 24;
                    let total = bootlog_lines.len();
                    let start_idx = bootlog_scroll;
                    let end_idx = core::cmp::min(start_idx + max_lines, total);

                    let mut draw_y = log_y + 10;
                    for i in start_idx..end_idx {
                        canvas.print_str(cx + 10, draw_y, &bootlog_lines[i], 0xFF_CCCCCC, 1);
                        draw_y += 16;
                    }

                    // The Interactive Scrollbar!
                    if total > max_lines {
                        let track_x = cx + cw - 15;
                        canvas.fill_rect(track_x, log_y, 15, log_h, 0xFF_2A2A2A);
                        
                        let thumb_h = core::cmp::max(20, (max_lines * log_h) / total);
                        let max_scroll = total.saturating_sub(max_lines);
                        let scroll_pct = if max_scroll > 0 { (bootlog_scroll * 100) / max_scroll } else { 0 };
                        
                        let thumb_y = log_y + ((log_h - thumb_h) * scroll_pct) / 100;
                        canvas.fill_rect(track_x + 2, thumb_y, 11, thumb_h, 0xFF_666666);
                    }
                }
            }

            sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            needs_redraw = false;
        }

        // 3. NON-BLOCKING EVENT LOOP
        // Passing 'false' to block means it instantly returns if there's no mouse input!
        if sys_ipc_recv(&mut msg, false) {
            if msg.msg_type == MSG_WINDOW_CLOSE { sys_exit(0); }
            
            else if msg.msg_type == MSG_MOUSE_EVENT {
                let mx = msg.data1 as usize; let my = msg.data2 as usize;

                // Tab Switching
                if mx < 150 {
                    let old_state = state;
                    if my >= 75 && my <= 105 { state = SysMonState::Vitals; }
                    else if my >= 115 && my <= 145 { state = SysMonState::Tasks; }
                    else if my >= 155 && my <= 185 { state = SysMonState::Bootlog; }
                    
                    if state != old_state { needs_redraw = true; }
                }

                // Handle Bootlog Scrollbar Clicks
                if state == SysMonState::Bootlog {
                    let cx = 170; let cw = WIDTH - cx - 20;
                    let track_x = cx + cw - 15;
                    
                    if mx >= track_x {
                        let mid_y = HEIGHT / 2;
                        if my < mid_y { 
                            bootlog_scroll = bootlog_scroll.saturating_sub(5); 
                        } else { 
                            bootlog_scroll = core::cmp::min(
                                bootlog_scroll.saturating_add(5), 
                                bootlog_lines.len().saturating_sub(24)
                            ); 
                        }
                        needs_redraw = true;
                    }
                }
            }
        } else {
            // Sleep for 10ms to save CPU power while waiting for the 500ms data refresh tick
            sys_sleep_ms(10);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }