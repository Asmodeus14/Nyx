use crate::syscalls::sys_get_context_switches;
use crate::gfx::draw;

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

    /// Renders the monitor content into the window bounds (ZERO ALLOCATIONS)
    pub fn draw(&mut self, fb: &mut [u32], screen_w: usize, screen_h: usize, window_x: usize, window_y: usize) {
        let content_x = window_x + 20;
        let content_y = window_y + 40;
        
        // We set a fixed "Right Edge" for our numbers so they align perfectly.
        // 16 chars for the label * 8px = 128px offset. We add 100px for the number box width.
        let number_right_edge = content_x + 228; 

        // Row 1: Total Switches
        draw::draw_text(fb, screen_w, screen_h, content_x, content_y, "Total Switches: ", 0xFFFFFFFF); 
        Self::draw_number_right_aligned(fb, screen_w, screen_h, number_right_edge, content_y, self.last_switches, 0xFFFFFFFF);

        // Row 2: Switches per Second
        draw::draw_text(fb, screen_w, screen_h, content_x, content_y + 25, "Switches / Sec: ", 0xFFFFFF00); 
        Self::draw_number_right_aligned(fb, screen_w, screen_h, number_right_edge, content_y + 25, self.switches_per_sec, 0xFFFFFF00);
        
        // Row 3: Active Threads
        draw::draw_text(fb, screen_w, screen_h, content_x, content_y + 50, "Active Threads: 3", 0xFF00FFFF); 
    }

    // --- MICRO-OPTIMIZATION: Right-Aligned Stack Rendering ---
    fn draw_number_right_aligned(fb: &mut [u32], w: usize, h: usize, right_x: usize, y: usize, mut val: u64, color: u32) {
        // Edge case for zero
        if val == 0 {
            // '0' is 1 char = 8px wide
            draw::draw_text(fb, w, h, right_x - 8, y, "0", color);
            return;
        }

        // A u64 maxes out at 20 digits, so a 20-byte stack array is perfect.
        let mut buf = [0u8; 20];
        let mut i = 20;

        // Extract digits mathematically from right to left
        while val > 0 {
            i -= 1;
            buf[i] = b'0' + (val % 10) as u8;
            val /= 10;
        }

        // Calculate how many characters we actually generated
        let char_count = 20 - i;
        let text_width = char_count * 8; // Assumes your font is 8 pixels wide
        
        // Shift the starting X coordinate left based on the width of the number!
        let start_x = right_x.saturating_sub(text_width);

        // Unchecked conversion because we mathematically guarantee 0-9 ASCII bounds
        let s = unsafe { core::str::from_utf8_unchecked(&buf[i..]) };
        
        // Draw the cleanly formatted string
        draw::draw_text(fb, w, h, start_x, y, s, color);
    }
}