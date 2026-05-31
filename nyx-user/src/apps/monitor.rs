use crate::gfx::draw;
use crate::syscalls::{sys_get_system_info, SystemInfo};
use core::fmt::Write;

// --- A lightweight, heap-free string buffer ---
struct StackString<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackString<N> {
    fn new() -> Self { Self { buf: [0; N], len: 0 } }
    fn as_str(&self) -> &str { core::str::from_utf8(&self.buf[..self.len]).unwrap_or("") }
}

impl<const N: usize> core::fmt::Write for StackString<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            if self.len < N {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}
// ----------------------------------------------

pub struct SysMonitor {
    info: SystemInfo,
}

impl SysMonitor {
    pub fn new() -> Self {
        let mut info = unsafe { core::mem::zeroed() };
        sys_get_system_info(&mut info);
        Self { info }
    }

    pub fn update_stats(&mut self) {
        sys_get_system_info(&mut self.info);
    }

    pub fn draw(&self, fb: &mut [u32], screen_w: usize, screen_h: usize, x: usize, y: usize) {
        // Draw Window Background (Fixed bounds to match main.rs 300x200)
        draw::draw_rect(fb, screen_w, screen_h, x, y, 300, 200, 0xFF181818); 
        draw::draw_rect(fb, screen_w, screen_h, x, y, 300, 25, 0xFF2A2A2A);
        draw::draw_text(fb, screen_w, screen_h, x + 10, y + 5, "NyxOS Live Telemetry", 0xFFFFFFFF);

        let mut cy = y + 40;
        
        // 1. SILICON THERMALS
        let temp_color = if self.info.current_temp >= 80 { 0xFFFF3333 } 
                         else if self.info.current_temp > 60 { 0xFFFFFF33 } 
                         else { 0xFF33FF33 };
                         
        let mut buf = StackString::<64>::new();
        let _ = write!(&mut buf, "Silicon Temp : {} C", self.info.current_temp);
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, buf.as_str(), temp_color);
        cy += 25;
        
        // 2. LIVE SMM FAN TACHOMETER
        let mut buf = StackString::<64>::new();
        let _ = write!(&mut buf, "CPU Fan Speed: {} RPM", self.info.cpu_fan_rpm);
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, buf.as_str(), 0xFF00AAFF);
        cy += 20;
        
        let mut buf = StackString::<64>::new();
        let _ = write!(&mut buf, "GPU Fan Speed: {} RPM", self.info.gpu_fan_rpm);
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, buf.as_str(), 0xFF00AAFF);
        cy += 25;

        // 3. TASK SCHEDULER
        let mut buf = StackString::<64>::new();
        let _ = write!(&mut buf, "Total Tasks  : {}", self.info.task_count);
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, buf.as_str(), 0xFFAAAAAA);
        cy += 20;
        
        draw::draw_rect(fb, screen_w, screen_h, x + 15, cy, 270, 1, 0xFF444444);
        cy += 10;
        
        // Render top active tasks
        let limit = core::cmp::min(self.info.task_count as usize, 4); // Max 4 to fit in 200px height
        for i in 0..limit {
            let t = &self.info.tasks[i];
            let name = if let Ok(s) = core::str::from_utf8(&t.name) { s.trim_matches(char::from(0)) } else { "Unknown" };
            
            let mut buf = StackString::<64>::new();
            let _ = write!(&mut buf, "PID {:02} | {} | {} Ticks", t.pid, name, t.cpu_ticks);
            draw::draw_text(fb, screen_w, screen_h, x + 15, cy, buf.as_str(), 0xFF888888);
            cy += 16;
        }
    }
}