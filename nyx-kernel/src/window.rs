use alloc::vec::Vec;
use alloc::string::String;
use crate::gui::{Painter, Rect, Color};
use crate::mouse::MouseState;

pub struct Window {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub title: String,
    // Dragging State
    pub is_dragging: bool,
    pub drag_offset_x: usize,
    pub drag_offset_y: usize,
    // Visual Style
    pub color: Color,
}

impl Window {
    pub fn new(x: usize, y: usize, w: usize, h: usize, title: &str) -> Self {
        Self {
            x, y, w, h,
            title: String::from(title),
            is_dragging: false,
            drag_offset_x: 0,
            drag_offset_y: 0,
            color: Color::GRAY,
        }
    }

    pub fn draw(&self, painter: &mut impl Painter) {
        // 1. Draw Shadow (Simple visual depth)
        painter.draw_rect(Rect::new(self.x + 5, self.y + 5, self.w, self.h), Color::new(50, 50, 50));

        // 2. Draw Main Body
        painter.draw_rect(Rect::new(self.x, self.y, self.w, self.h), self.color);

        // 3. Draw Title Bar (Dark Blue)
        let title_bar_height = 25;
        painter.draw_rect(Rect::new(self.x, self.y, self.w, title_bar_height), Color::new(0, 0, 150));

        // 4. Draw Title Text
        // Center the text vertically in the bar
        painter.draw_string(self.x + 8, self.y + 5, &self.title, Color::WHITE);

        // 5. Draw Close Button [X]
        let close_btn_x = self.x + self.w - 20;
        painter.draw_string(close_btn_x, self.y + 5, "X", Color::RED);
    }

    // Helper: Did the mouse click the title bar?
    pub fn is_title_bar_hit(&self, mx: usize, my: usize) -> bool {
        mx >= self.x && mx <= self.x + self.w &&
        my >= self.y && my <= self.y + 25
    }
}

pub struct WindowManager {
    windows: Vec<Window>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self { windows: Vec::new() }
    }

    pub fn add(&mut self, window: Window) {
        self.windows.push(window);
    }

    pub fn update(&mut self, mouse: &MouseState) {
        // --- LOGIC: DRAGGING WINDOWS ---
        
        // 1. Check if we are currently dragging a window
        for window in self.windows.iter_mut() {
            if window.is_dragging {
                if mouse.left_click {
                    // Update Position based on Mouse - Offset
                    if mouse.x >= window.drag_offset_x {
                        window.x = mouse.x - window.drag_offset_x;
                    }
                    if mouse.y >= window.drag_offset_y {
                        window.y = mouse.y - window.drag_offset_y;
                    }
                } else {
                    // Mouse released: Stop dragging
                    window.is_dragging = false;
                }
                return; // Focus on one window at a time
            }
        }

        // 2. Check for New Clicks (Hit Testing)
        if mouse.left_click {
            // Iterate in Reverse (Top-most windows first) to grab the one on top
            for window in self.windows.iter_mut().rev() {
                if window.is_title_bar_hit(mouse.x, mouse.y) {
                    // Start Dragging
                    window.is_dragging = true;
                    // Calculate offset so the window doesn't "snap" to top-left
                    window.drag_offset_x = mouse.x.saturating_sub(window.x);
                    window.drag_offset_y = mouse.y.saturating_sub(window.y);
                    
                    // TODO: Move this window to the end of the vector (bring to front)
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