use alloc::format;
use crate::gfx::draw;
use crate::syscalls;

pub struct SysInfoApp {
    last_update: usize,
    stats: [f32; 4], // [Energy, Entropy, Stability, Curiosity]
    active_cores: usize, 
}

impl SysInfoApp {
    pub fn new() -> Self {
        Self {
            last_update: 0,
            stats: [0.0; 4],
            // Grab the core count immediately on app launch
            active_cores: syscalls::sys_get_active_cores(),
        }
    }

    pub fn draw(&mut self, fb: &mut [u32], w: usize, h: usize, x: usize, y: usize) {
        let now = syscalls::sys_get_time();
        
        // Only ping the kernel for new stats every 100ms so we don't spam Syscalls
        if now.wrapping_sub(self.last_update) > 100 {
            syscalls::sys_get_entity_stats(&mut self.stats);
            // Refresh the core count dynamically just in case one wakes up late!
            self.active_cores = syscalls::sys_get_active_cores(); 
            self.last_update = now;
        }

        let start_y = y + 40;
        let pad_x = x + 20;

        // Static System Info
        draw::draw_text(fb, w, h, pad_x, start_y, "NyxOS v0.6 [Kernel Ring-0]", 0xFF00AAFF);
        draw::draw_text(fb, w, h, pad_x, start_y + 20, "Architecture: x86_64 SMP", 0xFFDDDDDD);
        
        // --- NEW: Dynamic Hardware Core Display ---
        let core_text = format!("Active Hardware Cores: {}", self.active_cores);
        draw::draw_text(fb, w, h, pad_x, start_y + 40, &core_text, 0xFF00FFFF); // Bright Cyan
        // ------------------------------------------

        draw::draw_text(fb, w, h, pad_x, start_y + 60, "NVMe Lossless Compression: ACTIVE", 0xFF00FF00);

        // Entity Vitals Header (Shifted down slightly to make room)
        draw::draw_text(fb, w, h, pad_x, start_y + 95, "ENTITY LIVE TELEMETRY", 0xFFFF00FF);
        draw::draw_rect_simple(fb, w, h, pad_x, start_y + 113, 300, 1, 0xFF555555);

        // Helper function to draw a data bar
        let mut draw_bar = |label: &str, value: f32, y_offset: usize, color: u32| {
            let bar_y = start_y + y_offset;
            let val_str = format!("{}: {:.2}", label, value);
            draw::draw_text(fb, w, h, pad_x, bar_y, &val_str, 0xFFFFFFFF);
            
            // Bar Background
            draw::draw_rect_simple(fb, w, h, pad_x, bar_y + 18, 200, 8, 0xFF222222);
            
            // Filled Bar (clamp value between 0 and 100 for width calc)
            let fill_width = ((value.clamp(0.0, 100.0) / 100.0) * 200.0) as usize;
            if fill_width > 0 {
                draw::draw_rect_simple(fb, w, h, pad_x, bar_y + 18, fill_width, 8, color);
            }
        };

        // Render the 4 states! (Y-offsets pushed down to accommodate the new core text)
        draw_bar("Energy", self.stats[0], 125, 0xFFFF3333);
        draw_bar("Entropy", self.stats[1], 165, 0xFF00FFFF);
        draw_bar("Stability", self.stats[2], 205, 0xFF33FF33);
        draw_bar("Curiosity", self.stats[3], 245, 0xFFFFFF00);
    }
}