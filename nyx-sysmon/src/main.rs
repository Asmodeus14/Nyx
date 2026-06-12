#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

#[derive(PartialEq, Clone, Copy)]
enum SysMonState { Vitals, Tasks, Bootlog }

struct SysMonApp {
    state: SysMonState,
    last_update_time: usize,
    entity_stats: [f32; 4],
    active_cores: usize,
    sys_info: SystemInfo,
    bootlog_buf: Vec<u8>,
    bootlog_lines: Vec<String>,
    bootlog_last_len: usize,
    bootlog_scroll: usize,
}

impl SysMonApp {
    fn new() -> Self {
        Self {
            state: SysMonState::Vitals,
            last_update_time: 0,
            entity_stats: [0.0; 4],
            active_cores: 0,
            sys_info: unsafe { core::mem::zeroed() },
            bootlog_buf: alloc::vec![0u8; 16384],
            bootlog_lines: Vec::new(),
            bootlog_last_len: 0,
            bootlog_scroll: 0,
        }
    }
}

impl NyxApp for SysMonApp {
    fn title(&self) -> &str { "Nyx System Monitor" }
    fn initial_width(&self) -> usize { 700 }
    fn initial_height(&self) -> usize { 480 }

    fn update(&mut self) -> bool {
        let now = sys_get_time();
        
        // 1. NON-BLOCKING DATA REFRESH (Every 500ms)
        if now.wrapping_sub(self.last_update_time) > 500 {
            sys_get_entity_stats(&mut self.entity_stats);
            self.active_cores = sys_get_active_cores();
            sys_get_system_info(&mut self.sys_info);

            let len = sys_get_boot_logs(&mut self.bootlog_buf);
            if len != self.bootlog_last_len {
                self.bootlog_last_len = len;
                self.bootlog_lines.clear();
                if let Ok(text) = core::str::from_utf8(&self.bootlog_buf[..len]) {
                    for line in text.split('\n') {
                        if !line.trim().is_empty() {
                            self.bootlog_lines.push(String::from(line));
                        }
                    }
                }
                let max_lines = 24;
                self.bootlog_scroll = self.bootlog_lines.len().saturating_sub(max_lines);
            }
            self.last_update_time = now;
            return true; // Data refreshed, force UI redraw
        }
        false
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let width = canvas.width;
        let height = canvas.height;

        canvas.fill_rect(0, 0, width, height, Color::WARM_BG);
        canvas.fill_rect(0, 0, 150, height, Color::WARM_SURFACE);
        canvas.fill_rect(150, 0, 1, height, Color::WARM_BORDER); 

        canvas.print_str(15, 20, "SYS MON", Color::ACCENT_PRIMARY, 2);

        let tabs = [
            (SysMonState::Vitals, "Entity Vitals", 80),
            (SysMonState::Tasks, "Task Scheduler", 120),
            (SysMonState::Bootlog, "Kernel Bootlog", 160),
        ];

        for (s, text, y) in tabs.iter() {
            let is_active = self.state == *s;
            if is_active { canvas.fill_rect(10, *y - 5, 130, 30, Color::ACCENT_PRIMARY); }
            let text_color = if is_active { Color::WHITE } else { Color::TEXT_MUTED };
            canvas.print_str(20, *y + 2, text, text_color, 1);
        }

        let cx = 170; let cw = width.saturating_sub(cx + 20);

        match self.state {
            SysMonState::Vitals => {
                canvas.print_str(cx, 20, "Entity Live Telemetry", Color::TEXT_DARK, 2);
                
                let core_text = alloc::format!("Architecture: x86_64 SMP | Active Hardware Cores: {}", self.active_cores);
                canvas.print_str(cx, 60, &core_text, Color::TEXT_MUTED, 1);
                canvas.print_str(cx, 80, "NVMe Lossless Compression: ACTIVE", Color::ACCENT_GREEN, 1);

                let bars = [
                    ("Energy", self.entity_stats[0], 130, 0xFF_E74C3C),
                    ("Entropy", self.entity_stats[1], 180, 0xFF_3498DB),
                    ("Stability", self.entity_stats[2], 230, 0xFF_2ECC71),
                    ("Curiosity", self.entity_stats[3], 280, 0xFF_F1C40F),
                ];

                for (label, val, y, color) in bars.iter() {
                    let text = alloc::format!("{}: {:.2}", label, val);
                    canvas.print_str(cx, *y, &text, Color::TEXT_DARK, 1);
                    
                    canvas.fill_rect(cx, *y + 20, cw, 12, Color::WARM_BORDER); // Scaled dynamically
                    let fill_w = ((val.clamp(0.0, 100.0) / 100.0) * cw as f32) as usize;
                    if fill_w > 0 {
                        canvas.fill_rect(cx, *y + 20, fill_w, 12, *color);
                    }
                }
            },
            SysMonState::Tasks => {
                canvas.print_str(cx, 20, "Hardware & Scheduler", Color::TEXT_DARK, 2);
                
                let temp_color = if self.sys_info.current_temp >= 80 { 0xFF_E74C3C } else { Color::ACCENT_GREEN };
                canvas.print_str(cx, 70, &alloc::format!("Silicon Temp: {} C", self.sys_info.current_temp), temp_color, 1);
                canvas.print_str(cx, 90, &alloc::format!("CPU Fan Speed: {} RPM", self.sys_info.cpu_fan_rpm), Color::TEXT_DARK, 1);
                canvas.print_str(cx, 110, &alloc::format!("GPU Fan Speed: {} RPM", self.sys_info.gpu_fan_rpm), Color::TEXT_DARK, 1);

                canvas.fill_rect(cx, 140, cw, 1, Color::WARM_BORDER);
                canvas.print_str(cx, 155, &alloc::format!("Total Kernel Tasks: {}", self.sys_info.task_count), Color::TEXT_DARK, 1);
                
                let mut ty = 185;
                let limit = core::cmp::min(self.sys_info.task_count as usize, 10);
                for i in 0..limit {
                    let t = &self.sys_info.tasks[i];
                    let name = core::str::from_utf8(&t.name).unwrap_or("Unknown").trim_matches(char::from(0));
                    let t_str = alloc::format!("PID {:02} | {} | {} Ticks", t.pid, name, t.cpu_ticks);
                    canvas.print_str(cx, ty, &t_str, Color::TEXT_MUTED, 1);
                    ty += 20;
                }
            },
            SysMonState::Bootlog => {
                canvas.print_str(cx, 20, "Kernel Ring Buffer (dmesg)", Color::TEXT_DARK, 2);
                
                let log_y = 60; let log_h = height.saturating_sub(80);
                canvas.fill_rect(cx, log_y, cw, log_h, 0xFF_1E1E1E); 
                
                // Dynamic lines based on window height
                let max_lines = log_h / 16;
                let total = self.bootlog_lines.len();
                let start_idx = self.bootlog_scroll;
                let end_idx = core::cmp::min(start_idx + max_lines, total);

                let mut draw_y = log_y + 10;
                for i in start_idx..end_idx {
                    canvas.print_str(cx + 10, draw_y, &self.bootlog_lines[i], 0xFF_CCCCCC, 1);
                    draw_y += 16;
                }

                if total > max_lines {
                    let track_x = cx + cw - 15;
                    canvas.fill_rect(track_x, log_y, 15, log_h, 0xFF_2A2A2A);
                    
                    let thumb_h = core::cmp::max(20, (max_lines * log_h) / total);
                    let max_scroll = total.saturating_sub(max_lines);
                    let scroll_pct = if max_scroll > 0 { (self.bootlog_scroll * 100) / max_scroll } else { 0 };
                    
                    let thumb_y = log_y + ((log_h.saturating_sub(thumb_h)) * scroll_pct) / 100;
                    canvas.fill_rect(track_x + 2, thumb_y, 11, thumb_h, 0xFF_666666);
                }
            }
        }
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        let mut needs_redraw = false;

        if mx < 150 {
            let old_state = self.state;
            if my >= 75 && my <= 105 { self.state = SysMonState::Vitals; }
            else if my >= 115 && my <= 145 { self.state = SysMonState::Tasks; }
            else if my >= 155 && my <= 185 { self.state = SysMonState::Bootlog; }
            if self.state != old_state { needs_redraw = true; }
        }

        if self.state == SysMonState::Bootlog {
            let cx = 170; // We assume default layout width for hit testing
            // Because width is dynamic, we need to know the canvas width for perfect hit testing.
            // A simple approximation is checking the far right of the screen.
            if mx > 500 {
                if my < 200 { 
                    self.bootlog_scroll = self.bootlog_scroll.saturating_sub(5); 
                } else { 
                    self.bootlog_scroll = core::cmp::min(
                        self.bootlog_scroll.saturating_add(5), 
                        self.bootlog_lines.len().saturating_sub(24) // Approx max lines
                    ); 
                }
                needs_redraw = true;
            }
        }
        needs_redraw
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(512);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 512 * 4096); }

    nyx_gui::app::run(SysMonApp::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }