use alloc::vec::Vec;
use alloc::string::String;
use crate::gui::{Painter, Rect, Color};
use crate::mouse::MouseState;

pub struct Window {
    pub x: usize, pub y: usize,
    pub w: usize, pub h: usize,
    pub title: String,
    pub is_dragging: bool,
    pub drag_offset_x: usize, pub drag_offset_y: usize,
    pub color: Color,
}

impl Window {
    pub fn new(x: usize, y: usize, w: usize, h: usize, title: &str) -> Self {
        Self {
            x, y, w, h,
            title: String::from(title),
            is_dragging: false,
            drag_offset_x: 0, drag_offset_y: 0,
            color: Color::DARK_GRAY,
        }
    }

    pub fn draw(&self, painter: &mut impl Painter) {
        // Shadow
        painter.draw_rect(Rect::new(self.x + 8, self.y + 8, self.w, self.h), Color::new(20, 20, 20));
        // Body
        painter.draw_rect(Rect::new(self.x, self.y, self.w, self.h), self.color);
        // Header
        let title_height = 40; 
        painter.draw_rect(Rect::new(self.x, self.y, self.w, title_height), Color::DARK_BLUE);
        // Text
        painter.draw_string(self.x + 12, self.y + 4, &self.title, Color::WHITE);
        // Close Button
        painter.draw_rect(Rect::new(self.x + self.w - 35, self.y + 5, 30, 30), Color::RED);
    }

    pub fn is_title_bar_hit(&self, mx: usize, my: usize) -> bool {
        mx >= self.x && mx <= self.x + self.w &&
        my >= self.y && my <= self.y + 40
    }
}

pub struct WindowManager {
    windows: Vec<Window>,
}

impl WindowManager {
    pub fn new() -> Self { Self { windows: Vec::new() } }
    pub fn add(&mut self, window: Window) { self.windows.push(window); }

    pub fn update(&mut self, mouse: &MouseState) {
        for window in self.windows.iter_mut() {
            if window.is_dragging {
                if mouse.left_click {
                    if mouse.x >= window.drag_offset_x { window.x = mouse.x - window.drag_offset_x; }
                    if mouse.y >= window.drag_offset_y { window.y = mouse.y - window.drag_offset_y; }
                } else {
                    window.is_dragging = false;
                }
                return;
            }
        }
        
        if mouse.left_click {
            for window in self.windows.iter_mut().rev() {
                if window.is_title_bar_hit(mouse.x, mouse.y) {
                    window.is_dragging = true;
                    window.drag_offset_x = mouse.x.saturating_sub(window.x);
                    window.drag_offset_y = mouse.y.saturating_sub(window.y);
                    return; 
                }
            }
        }
    }

    pub fn draw(&mut self, painter: &mut impl Painter) {
        for window in &self.windows {
            window.draw(painter);
        }
    }
}