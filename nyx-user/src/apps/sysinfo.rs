use crate::gfx::draw;
use crate::syscalls;
use alloc::string::String;

pub struct SysInfoApp {
    info_text: String,
}

impl SysInfoApp {
    pub fn new() -> Self {
        let mut buf = [0u8; 512];
        let len = syscalls::sys_get_hw_info(&mut buf);
        let info = if len > 0 {
            core::str::from_utf8(&buf[..len]).unwrap_or("Encoding Error").into()
        } else {
            "Kernel failed to provide hardware info.".into()
        };
        
        Self { info_text: info }
    }

    pub fn draw(&mut self, fb: &mut [u32], screen_w: usize, screen_h: usize, window_x: usize, window_y: usize) {
        let content_x = window_x + 20;
        let mut content_y = window_y + 40;
        
        for line in self.info_text.split('\n') {
            // Draw in "Hacker Green" to fit the aesthetic!
            draw::draw_text(fb, screen_w, screen_h, content_x, content_y, line, 0xFF00FF00); 
            content_y += 20;
        }
    }
}