use crate::canvas::{Canvas, Color};
use crate::effects::{alpha_blend, apply_opacity};

pub struct Window {
    pub id: usize,
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub title: [u8; 64],    
    pub title_len: usize,   
    pub active: bool,
    pub exists: bool,
    pub opacity: u8,
}

// ─────────────────────────────────────────────────────────────────────────
// 1. TASKBAR & CURSOR
// ─────────────────────────────────────────────────────────────────────────
pub fn draw_taskbar(buffer: &mut [u32], stride: usize, screen_h: usize) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    let bar_h = 36;
    let start_y = screen_h - bar_h;

    canvas.fill_rect(0, start_y, stride, bar_h, 0xD8_FFFFFF); 
    canvas.fill_rect(0, start_y, stride, 1, 0xFF_D1D1D1);     

    let clock_y = start_y + 14;
    canvas.print_str(20, clock_y, "10:20 AM", Color::TEXT_DARK, 1);

    let btn_w = 70;
    let btn_x = (stride / 2) - (btn_w / 2);
    let btn_y = start_y + 6;
    canvas.fill_rect(btn_x, btn_y, btn_w, 24, Color::ACCENT_PRIMARY);
    canvas.print_str(btn_x + 15, btn_y + 8, "NYX", Color::WHITE, 1);
}

const CURSOR_BITMAP: [[u8; 11]; 16] = [
    [1,1,0,0,0,0,0,0,0,0,0],
    [1,2,1,0,0,0,0,0,0,0,0],
    [1,2,2,1,0,0,0,0,0,0,0],
    [1,2,2,2,1,0,0,0,0,0,0],
    [1,2,2,2,2,1,0,0,0,0,0],
    [1,2,2,2,2,2,1,0,0,0,0],
    [1,2,2,2,2,2,2,1,0,0,0],
    [1,2,2,2,2,2,2,2,1,0,0],
    [1,2,2,2,2,2,2,2,2,1,0],
    [1,2,2,2,2,2,2,2,2,2,1],
    [1,2,2,2,2,2,2,1,1,1,1],
    [1,2,2,1,2,2,1,0,0,0,0],
    [1,2,1,0,1,2,2,1,0,0,0],
    [1,1,0,0,1,2,2,1,0,0,0],
    [1,0,0,0,0,1,2,2,1,0,0],
    [0,0,0,0,0,0,1,1,0,0,0],
];

pub fn draw_cursor(buffer: &mut [u32], stride: usize, screen_h: usize, mx: usize, my: usize) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    for (row_idx, row) in CURSOR_BITMAP.iter().enumerate() {
        for (col_idx, &pixel) in row.iter().enumerate() {
            if pixel == 1 {
                canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK);
            } else if pixel == 2 {
                canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::WHITE);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 2. WINDOW RENDERING
// ─────────────────────────────────────────────────────────────────────────
pub fn draw_window_rounded(buffer: &mut [u32], stride: usize, screen_h: usize, win: &Window) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    
    let shadow_color = apply_opacity(0x40_000000, win.opacity);
    let surface_color = apply_opacity(Color::WARM_SURFACE, win.opacity);
    let border_color = apply_opacity(Color::WARM_BORDER, win.opacity);
    let text_color = apply_opacity(Color::TEXT_DARK, win.opacity);
    let close_color = apply_opacity(0xFF_FF5F56, win.opacity);

    canvas.fill_rect(win.x + win.w, win.y + 4, 4, win.h + 26, shadow_color); 
    canvas.fill_rect(win.x + 4, win.y + win.h + 30, win.w, 4, shadow_color); 

    canvas.fill_rect(win.x, win.y, win.w, win.h + 30, surface_color);
    
    canvas.fill_rect(win.x, win.y + 30, win.w, 1, border_color);
    canvas.fill_rect(win.x, win.y, win.w, 1, border_color); 
    canvas.fill_rect(win.x, win.y + win.h + 30, win.w, 1, border_color); 
    canvas.fill_rect(win.x, win.y, 1, win.h + 30, border_color); 
    canvas.fill_rect(win.x + win.w, win.y, 1, win.h + 30, border_color); 

    canvas.fill_rect(win.x + 12, win.y + 10, 12, 12, close_color);
    
    let title_str = core::str::from_utf8(&win.title[..win.title_len]).unwrap_or("App");
    let title_x = win.x + (win.w / 2) - ((title_str.len() * 8) / 2);
    canvas.print_str(title_x, win.y + 12, title_str, text_color, 1);
}

// ─────────────────────────────────────────────────────────────────────────
// 3. INTERACTIVE UI COMPONENTS
// ─────────────────────────────────────────────────────────────────────────

// --- BUTTON ---
pub struct Button {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub text: &'static str,
    pub is_hovered: bool, pub is_pressed: bool,
}

impl Button {
    pub fn draw(&self, canvas: &mut Canvas) {
        let bg_color = if self.is_pressed { Color::ACCENT_HOVER } 
                       else if self.is_hovered { Color::ACCENT_PRIMARY } 
                       else { Color::WARM_BORDER };
                       
        let text_color = if self.is_hovered || self.is_pressed { Color::WHITE } else { Color::TEXT_DARK };

        if !self.is_pressed { canvas.fill_rect(self.x, self.y + self.h, self.w, 2, 0x40_000000); }
        
        canvas.fill_rect(self.x, self.y, self.w, self.h, bg_color);
        
        let text_x = self.x + (self.w / 2) - ((self.text.len() * 8) / 2);
        let text_y = self.y + (self.h / 2) - 4;
        canvas.print_str(text_x, text_y, self.text, text_color, 1);
    }
}

// --- TOGGLE SWITCH ---
pub struct ToggleSwitch {
    pub x: usize, pub y: usize, pub is_on: bool,
}

impl ToggleSwitch {
    pub fn draw(&self, canvas: &mut Canvas) {
        let w = 40; let h = 20;
        let bg = if self.is_on { Color::ACCENT_GREEN } else { Color::WARM_BORDER };
        
        canvas.fill_rect(self.x, self.y, w, h, bg);
        
        let knob_x = if self.is_on { self.x + w - 18 } else { self.x + 2 };
        canvas.fill_rect(knob_x, self.y + 2, 16, 16, Color::WHITE);
    }
}

// --- DROPDOWN MENU ---
pub struct DropdownMenu<'a> {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub options: &'a [&'a str],
    pub selected_idx: usize,
    pub is_open: bool,
    pub hover_idx: Option<usize>,
}

impl<'a> DropdownMenu<'a> {
    pub fn draw(&self, canvas: &mut Canvas) {
        // Draw Main Selector Box
        canvas.fill_rect(self.x, self.y, self.w, self.h, Color::WARM_SURFACE);
        canvas.fill_rect(self.x, self.y, self.w, 1, Color::WARM_BORDER); // Top
        canvas.fill_rect(self.x, self.y + self.h, self.w, 1, Color::WARM_BORDER); // Bottom
        canvas.fill_rect(self.x, self.y, 1, self.h, Color::WARM_BORDER); // Left
        canvas.fill_rect(self.x + self.w, self.y, 1, self.h, Color::WARM_BORDER); // Right

        let text = self.options.get(self.selected_idx).unwrap_or(&"Select...");
        canvas.print_str(self.x + 10, self.y + (self.h / 2) - 4, text, Color::TEXT_DARK, 1);
        
        // Dropdown Arrow Indicator
        canvas.print_str(self.x + self.w - 15, self.y + (self.h / 2) - 4, "v", Color::TEXT_DARK, 1);

        // Draw the Menu Items if open
        if self.is_open {
            let drop_y = self.y + self.h;
            let drop_h = self.options.len() * self.h;
            
            // Drop Shadow
            canvas.fill_rect(self.x + 4, drop_y + 4, self.w, drop_h, 0x40_000000);
            
            // Menu Background
            canvas.fill_rect(self.x, drop_y, self.w, drop_h, Color::WARM_SURFACE);

            for (i, opt) in self.options.iter().enumerate() {
                let item_y = drop_y + (i * self.h);
                if Some(i) == self.hover_idx {
                    canvas.fill_rect(self.x, item_y, self.w, self.h, Color::ACCENT_PRIMARY);
                    canvas.print_str(self.x + 10, item_y + (self.h / 2) - 4, opt, Color::WHITE, 1);
                } else {
                    canvas.print_str(self.x + 10, item_y + (self.h / 2) - 4, opt, Color::TEXT_DARK, 1);
                }
            }
            
            // Menu Outline Border
            canvas.fill_rect(self.x, drop_y, self.w, 1, Color::WARM_BORDER); 
            canvas.fill_rect(self.x, drop_y + drop_h, self.w, 1, Color::WARM_BORDER); 
            canvas.fill_rect(self.x, drop_y, 1, drop_h, Color::WARM_BORDER); 
            canvas.fill_rect(self.x + self.w, drop_y, 1, drop_h, Color::WARM_BORDER); 
        }
    }
}