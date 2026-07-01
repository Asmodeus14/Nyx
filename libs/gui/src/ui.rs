use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use crate::canvas::{Canvas, Color};
use crate::effects::{alpha_blend, apply_opacity};

// ─────────────────────────────────────────────────────────────────────────
// COMPOSITOR & KERNEL UI ELEMENTS (Used by nyx-user)
// ─────────────────────────────────────────────────────────────────────────
pub struct Window {
    pub id: usize,
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub title: [u8; 64],    
    pub title_len: usize,   
    pub active: bool, pub exists: bool, pub opacity: u8,
    pub is_minimized: bool, pub is_maximized: bool,
    pub saved_x: usize, pub saved_y: usize, pub saved_w: usize, pub saved_h: usize,
}

pub fn draw_taskbar(buffer: &mut [u32], stride: usize, screen_h: usize) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    let bar_h = 36; let start_y = screen_h - bar_h;
    
    canvas.fill_rect(0, start_y, stride, bar_h, 0xD8_FFFFFF); 
    canvas.fill_rect(0, start_y, stride, 1, 0xFF_D1D1D1);     
    
    canvas.print_str(20, start_y + 14, "10:20 AM", Color::TEXT_DARK, 1);
    
    let btn_x = (stride / 2) - 35;
    canvas.fill_rect(btn_x, start_y + 6, 70, 24, Color::ACCENT_PRIMARY);
    canvas.print_str(btn_x + 15, start_y + 8, "NYX", Color::WHITE, 1);
}

#[derive(PartialEq, Clone, Copy)]
pub enum CursorType {
    Arrow,
    IBeam,
    Hand,
}

const ARROW_BITMAP: [[u8; 11]; 16] = [
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

const IBEAM_BITMAP: [[u8; 5]; 16] = [
    [1,1,1,1,1],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [1,1,1,1,1],
];

const HAND_BITMAP: [[u8; 11]; 16] = [
    [0,0,0,1,1,0,0,0,0,0,0],
    [0,0,1,2,2,1,0,0,0,0,0],
    [0,0,1,2,2,1,0,0,0,0,0],
    [0,0,1,2,2,1,0,0,0,0,0],
    [0,0,1,2,2,1,1,1,1,0,0],
    [0,1,1,2,2,2,2,2,2,1,0],
    [1,2,2,1,2,2,2,2,2,2,1],
    [1,2,2,2,1,2,2,2,2,2,1],
    [1,2,2,2,2,2,2,2,2,2,1],
    [1,2,2,2,2,2,2,2,2,2,1],
    [0,1,2,2,2,2,2,2,2,1,0],
    [0,0,1,2,2,2,2,2,1,0,0],
    [0,0,1,2,2,2,2,2,1,0,0],
    [0,0,0,1,2,2,2,1,0,0,0],
    [0,0,0,1,2,2,2,1,0,0,0],
    [0,0,0,0,1,1,1,0,0,0,0],
];

pub fn draw_cursor(buffer: &mut [u32], stride: usize, screen_h: usize, mx: usize, my: usize, c_type: CursorType) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    
    match c_type {
        CursorType::Arrow => {
            for (row_idx, row) in ARROW_BITMAP.iter().enumerate() {
                for (col_idx, &pixel) in row.iter().enumerate() {
                    if pixel == 1 { canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK); } 
                    else if pixel == 2 { canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::WHITE); }
                }
            }
        },
        CursorType::IBeam => {
            let offset_x = mx.saturating_sub(2);
            for (row_idx, row) in IBEAM_BITMAP.iter().enumerate() {
                for (col_idx, &pixel) in row.iter().enumerate() {
                    if pixel == 1 { canvas.fill_rect(offset_x + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK); }
                }
            }
        },
        CursorType::Hand => {
            let offset_x = mx.saturating_sub(4);
            for (row_idx, row) in HAND_BITMAP.iter().enumerate() {
                for (col_idx, &pixel) in row.iter().enumerate() {
                    if pixel == 1 { canvas.fill_rect(offset_x + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK); } 
                    else if pixel == 2 { canvas.fill_rect(offset_x + col_idx, my + row_idx, 1, 1, Color::WHITE); }
                }
            }
        }
    }
}

pub fn draw_window_rounded(buffer: &mut [u32], stride: usize, screen_h: usize, win: &Window) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    let surface = apply_opacity(Color::WARM_SURFACE, win.opacity);
    let border = apply_opacity(Color::WARM_BORDER, win.opacity);
    let total_h = if win.is_minimized { 30 } else { win.h + 30 };
    canvas.fill_rect(win.x, win.y, win.w, total_h, surface);
    canvas.fill_rect(win.x, win.y, win.w, 1, border); 
    canvas.fill_rect(win.x, win.y + total_h, win.w, 1, border); 
    canvas.fill_rect(win.x, win.y, 1, total_h, border); 
    canvas.fill_rect(win.x + win.w, win.y, 1, total_h + 1, border); 

    // Header Controls (Now with symbols!)
    let icon_color = apply_opacity(0x60_000000, win.opacity); // Dark semi-transparent text

    canvas.fill_rect(win.x + 12, win.y + 10, 12, 12, apply_opacity(0xFF_FF5F56, win.opacity)); // Close
    canvas.print_str(win.x + 14, win.y + 12, "x", icon_color, 1);

    canvas.fill_rect(win.x + 28, win.y + 10, 12, 12, apply_opacity(0xFF_FFBD2E, win.opacity)); // Min
    canvas.print_str(win.x + 30, win.y + 11, "-", icon_color, 1);

    canvas.fill_rect(win.x + 44, win.y + 10, 12, 12, apply_opacity(0xFF_28C940, win.opacity)); // Max
    canvas.print_str(win.x + 46, win.y + 12, "+", icon_color, 1);
    
    let title_str = core::str::from_utf8(&win.title[..win.title_len]).unwrap_or("App");
    canvas.print_str(win.x + (win.w / 2) - ((title_str.len() * 8) / 2), win.y + 12, title_str, apply_opacity(Color::TEXT_DARK, win.opacity), 1);
}

// ─────────────────────────────────────────────────────────────────────────
// PHASE 3: POLYMORPHIC WIDGET TOOLKIT
// ─────────────────────────────────────────────────────────────────────────

/// Base trait for all UI components
pub trait Widget {
    fn draw(&mut self, canvas: &mut Canvas);
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool; // returns true if needs redraw
    fn on_key(&mut self, key: char) -> bool;
}

// --- 1. PANEL ---
pub struct Panel {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize, pub bg_color: u32,
    pub children: Vec<Box<dyn Widget>>,
}
impl Widget for Panel {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, self.bg_color);
        for child in &mut self.children { child.draw(canvas); }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_mouse(mx, my, clicked); }
        redraw
    }
    fn on_key(&mut self, key: char) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_key(key); }
        redraw
    }
}

// --- 2. LABEL ---
pub struct Label {
    pub x: usize, pub y: usize, pub text: String, pub color: u32,
}
impl Widget for Label {
    fn draw(&mut self, canvas: &mut Canvas) { canvas.print_str(self.x, self.y, &self.text, self.color, 1); }
    fn on_mouse(&mut self, _mx: usize, _my: usize, _clicked: bool) -> bool { false }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 3. BUTTON ---
pub struct Button {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub text: String,
    pub is_hovered: bool, pub is_pressed: bool,
}
impl Widget for Button {
    fn draw(&mut self, canvas: &mut Canvas) {
        let bg = if self.is_pressed { Color::ACCENT_HOVER } else if self.is_hovered { Color::ACCENT_PRIMARY } else { Color::WARM_BORDER };
        canvas.fill_rect(self.x, self.y, self.w, self.h, bg);
        canvas.print_str(self.x + 10, self.y + (self.h/2) - 4, &self.text, if self.is_hovered {Color::WHITE} else {Color::TEXT_DARK}, 1);
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let in_bounds = mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h;
        let old_hover = self.is_hovered; let old_pressed = self.is_pressed;
        self.is_hovered = in_bounds;
        self.is_pressed = in_bounds && clicked;
        old_hover != self.is_hovered || old_pressed != self.is_pressed
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 4. TEXTBOX ---
pub struct TextBox {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub text: String, pub is_focused: bool,
}
impl Widget for TextBox {
    fn draw(&mut self, canvas: &mut Canvas) {
        let border = if self.is_focused { Color::ACCENT_PRIMARY } else { Color::WARM_BORDER };
        canvas.fill_rect(self.x, self.y, self.w, self.h, Color::WHITE);
        canvas.fill_rect(self.x, self.y, self.w, 1, border);
        canvas.fill_rect(self.x, self.y + self.h, self.w, 1, border);
        canvas.fill_rect(self.x, self.y, 1, self.h, border);
        canvas.fill_rect(self.x + self.w, self.y, 1, self.h, border);
        canvas.print_str(self.x + 5, self.y + 8, &self.text, Color::TEXT_DARK, 1);
        if self.is_focused { canvas.fill_rect(self.x + 5 + (self.text.len() * 8), self.y + 6, 2, 12, Color::TEXT_DARK); }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked {
            let in_bounds = mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h;
            if self.is_focused != in_bounds { self.is_focused = in_bounds; return true; }
        }
        false
    }
    fn on_key(&mut self, key: char) -> bool {
        if self.is_focused {
            if key == '\x08' { self.text.pop(); } 
            else if key != '\n' && key != '\r' && key != '?' { self.text.push(key); }
            return true;
        }
        false
    }
}

// --- 5. CHECKBOX ---
pub struct CheckBox {
    pub x: usize, pub y: usize, pub text: String, pub is_checked: bool,
}
impl Widget for CheckBox {
    fn draw(&mut self, canvas: &mut Canvas) {
        let bg = if self.is_checked { Color::ACCENT_PRIMARY } else { Color::WHITE };
        canvas.fill_rect(self.x, self.y, 16, 16, bg);
        canvas.fill_rect(self.x, self.y, 16, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y+16, 16, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y, 1, 16, Color::WARM_BORDER);
        canvas.fill_rect(self.x+16, self.y, 1, 16, Color::WARM_BORDER);
        canvas.print_str(self.x + 25, self.y + 4, &self.text, Color::TEXT_DARK, 1);
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked && mx >= self.x && mx <= self.x + 16 && my >= self.y && my <= self.y + 16 {
            self.is_checked = !self.is_checked; return true;
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 6. LISTBOX ---
pub struct ListBox {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub items: Vec<String>, pub selected_idx: Option<usize>,
}
impl Widget for ListBox {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, Color::WHITE);
        for (i, item) in self.items.iter().enumerate() {
            let item_y = self.y + (i * 20);
            if item_y + 20 > self.y + self.h { break; } 
            if Some(i) == self.selected_idx {
                canvas.fill_rect(self.x, item_y, self.w, 20, Color::ACCENT_PRIMARY);
                canvas.print_str(self.x + 5, item_y + 6, item, Color::WHITE, 1);
            } else {
                canvas.print_str(self.x + 5, item_y + 6, item, Color::TEXT_DARK, 1);
            }
        }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked && mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h {
            let idx = (my - self.y) / 20;
            if idx < self.items.len() { self.selected_idx = Some(idx); return true; }
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 7. MENU (Dropdown) ---
pub struct Menu {
    pub x: usize, pub y: usize, pub w: usize,
    pub items: Vec<String>, pub is_open: bool, pub selected_idx: usize,
}
impl Widget for Menu {
    fn draw(&mut self, canvas: &mut Canvas) {
        // 🚨 FIX E0716: Treat it purely as a string slice (&str) rather than an allocated String reference
        let text = self.items.get(self.selected_idx).map(|s| s.as_str()).unwrap_or("Select");
        
        canvas.fill_rect(self.x, self.y, self.w, 25, Color::WARM_SURFACE);
        canvas.print_str(self.x + 5, self.y + 8, text, Color::TEXT_DARK, 1);
        canvas.print_str(self.x + self.w - 15, self.y + 8, "v", Color::TEXT_DARK, 1);
        
        if self.is_open {
            let drop_y = self.y + 25;
            canvas.fill_rect(self.x, drop_y, self.w, self.items.len() * 25, Color::WHITE);
            for (i, item) in self.items.iter().enumerate() {
                canvas.print_str(self.x + 5, drop_y + (i * 25) + 8, item, Color::TEXT_DARK, 1);
            }
        }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked {
            if mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + 25 {
                self.is_open = !self.is_open; return true;
            } else if self.is_open && mx >= self.x && mx <= self.x + self.w && my > self.y + 25 && my <= self.y + 25 + (self.items.len() * 25) {
                self.selected_idx = (my - (self.y + 25)) / 25;
                self.is_open = false; return true;
            } else if self.is_open {
                self.is_open = false; return true;
            }
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 8. SCROLLBAR ---
pub struct ScrollBar {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub value: usize, pub max_value: usize,
}
impl Widget for ScrollBar {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, 0xFF_E0E0E0);
        let thumb_h = core::cmp::max(20, self.h / core::cmp::max(1, self.max_value));
        let thumb_y = self.y + ((self.h - thumb_h) * self.value) / core::cmp::max(1, self.max_value);
        canvas.fill_rect(self.x + 2, thumb_y, self.w - 4, thumb_h, 0xFF_999999);
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked && mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h {
            self.value = ((my - self.y) * self.max_value) / self.h;
            return true;
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 9. IMAGEVIEW ---
pub struct ImageView {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub pixels: *const u32,
}
impl Widget for ImageView {
    fn draw(&mut self, canvas: &mut Canvas) {
        if self.pixels.is_null() { return; }
        let slice = unsafe { core::slice::from_raw_parts(self.pixels, self.w * self.h) };
        canvas.composite_buffer(self.x, self.y, slice, self.w, self.h, 255);
    }
    fn on_mouse(&mut self, _mx: usize, _my: usize, _clicked: bool) -> bool { false }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 10. DIALOG (Modal Box) ---
pub struct Dialog {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub title: String, pub children: Vec<Box<dyn Widget>>,
}
impl Widget for Dialog {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x + 5, self.y + 5, self.w, self.h, 0x40_000000); 
        canvas.fill_rect(self.x, self.y, self.w, self.h, Color::WARM_BG);
        canvas.fill_rect(self.x, self.y, self.w, 30, Color::WARM_SURFACE); 
        canvas.fill_rect(self.x, self.y, self.w, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y + self.h, self.w, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y, 1, self.h, Color::WARM_BORDER);
        canvas.fill_rect(self.x + self.w, self.y, 1, self.h, Color::WARM_BORDER);
        canvas.print_str(self.x + 10, self.y + 8, &self.title, Color::TEXT_DARK, 1);
        for child in &mut self.children { child.draw(canvas); }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_mouse(mx, my, clicked); }
        redraw
    }
    fn on_key(&mut self, key: char) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_key(key); }
        redraw
    }
}