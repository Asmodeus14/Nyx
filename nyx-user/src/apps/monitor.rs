use crate::syscalls::sys_get_context_switches;
use crate::gfx::draw;
use alloc::format;

pub struct SysMonitor {
    last_switches: u64,
    switches_per_sec: u64,
}

impl SysMonitor {
    pub fn new() -> Self {
        Self {
            last_switches: 0,
            switches_per_sec: 0,
        }
    }

    /// Called once per second by the main loop to calculate delta
    pub fn update_stats(&mut self) {
        let current = sys_get_context_switches();
        self.switches_per_sec = current.saturating_sub(self.last_switches);
        self.last_switches = current;
    }

    /// Renders the monitor content into the window bounds
    pub fn draw(&mut self, fb: &mut [u32], screen_w: usize, screen_h: usize, window_x: usize, window_y: usize) {
        let content_x = window_x + 20;
        let content_y = window_y + 40;
        
        let total_str = format!("Total Switches: {}", self.last_switches);
        let speed_str = format!("Switches / Sec: {}", self.switches_per_sec);
        let threads_str = "Active Threads: 3"; 
        
        // Draw the telemetry text
        draw::draw_text(fb, screen_w, screen_h, content_x, content_y, &total_str, 0xFFFFFFFF); // White
        draw::draw_text(fb, screen_w, screen_h, content_x, content_y + 25, &speed_str, 0xFFFFFF00); // Yellow
        draw::draw_text(fb, screen_w, screen_h, content_x, content_y + 50, threads_str, 0xFF00FFFF); // Cyan
    }
}