use alloc::format;
use crate::gfx::draw;
use crate::syscalls::{sys_get_system_info, SystemInfo, TaskInfo};

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
        // Draw Window Background
        draw::draw_rect(fb, screen_w, screen_h, x, y, 320, 260, 0xFF181818); 
        draw::draw_rect(fb, screen_w, screen_h, x, y, 320, 25, 0xFF2A2A2A);
        draw::draw_text(fb, screen_w, screen_h, x + 10, y + 5, "NyxOS Live Telemetry", 0xFFFFFFFF);

        let mut cy = y + 40;
        
        // 1. SILICON THERMALS
        let temp_color = if self.info.current_temp >= 80 { 0xFFFF3333 } 
                         else if self.info.current_temp > 60 { 0xFFFFFF33 } 
                         else { 0xFF33FF33 };
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, &format!("Silicon Temp : {} C", self.info.current_temp), temp_color);
        cy += 25;
        
        // 2. LIVE SMM FAN TACHOMETER
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, &format!("CPU Fan Speed: {} RPM", self.info.cpu_fan_rpm), 0xFF00AAFF);
        cy += 20;
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, &format!("GPU Fan Speed: {} RPM", self.info.gpu_fan_rpm), 0xFF00AAFF);
        cy += 25;

        // 3. TASK SCHEDULER
        draw::draw_text(fb, screen_w, screen_h, x + 15, cy, &format!("Total Tasks  : {}", self.info.task_count), 0xFFAAAAAA);
        cy += 20;
        
        draw::draw_rect(fb, screen_w, screen_h, x + 15, cy, 290, 1, 0xFF444444);
        cy += 10;
        
        // Render top 5 active tasks
        let limit = core::cmp::min(self.info.task_count as usize, 5);
        for i in 0..limit {
            let t = &self.info.tasks[i];
            let name = if let Ok(s) = core::str::from_utf8(&t.name) { s.trim_matches(char::from(0)) } else { "Unknown" };
            draw::draw_text(fb, screen_w, screen_h, x + 15, cy, &format!("PID {:02} | {} | {} Ticks", t.pid, name, t.cpu_ticks), 0xFF888888);
            cy += 16;
        }
    }
}