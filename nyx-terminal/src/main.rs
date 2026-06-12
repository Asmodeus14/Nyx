#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const BG_COLOR: u32 = 0xFF0D0D0D; 
const FG_COLOR: u32 = 0xFF00FF66; 
const FONT_W: usize = 8;
const FONT_H: usize = 8;

struct TerminalApp {
    input_buffer: String,
    output_history: String,
    blink_timer: usize,
    cursor_visible: bool,
}

impl TerminalApp {
    fn new() -> Self {
        Self {
            input_buffer: String::new(),
            output_history: String::from("NyxOS v0.1 Shell\nType 'help' for commands.\n"),
            blink_timer: 0,
            cursor_visible: true,
        }
    }
}

impl NyxApp for TerminalApp {
    fn title(&self) -> &str { "Nyx Matrix Terminal" }
    fn initial_width(&self) -> usize { 640 }
    fn initial_height(&self) -> usize { 400 }

    fn update(&mut self) -> bool {
        self.blink_timer += 1;
        if self.blink_timer > 30 {
            self.blink_timer = 0;
            self.cursor_visible = !self.cursor_visible;
            return true; // Force redraw to show/hide cursor
        }
        false
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(0, 0, canvas.width, canvas.height, BG_COLOR);
        
        let mut cx = 10;
        let mut cy = 10;
        
        // Draw History
        for c in self.output_history.chars() {
            if c == '\n' { cx = 10; cy += FONT_H + 4; continue; }
            canvas.draw_char(cx, cy, c, FG_COLOR, 1);
            cx += FONT_W;
            if cx >= canvas.width - 15 { cx = 10; cy += FONT_H + 4; }
        }

        // Draw Prompt
        let prompt = "N> ";
        for c in prompt.chars() {
            canvas.draw_char(cx, cy, c, FG_COLOR, 1);
            cx += FONT_W;
        }

        // Draw Input Buffer
        for c in self.input_buffer.chars() {
            canvas.draw_char(cx, cy, c, FG_COLOR, 1);
            cx += FONT_W;
            if cx >= canvas.width - 15 { cx = 10; cy += FONT_H + 4; }
        }

        // Draw Cursor
        if self.cursor_visible {
            canvas.fill_rect(cx, cy, FONT_W, FONT_H, FG_COLOR);
        }
    }

    fn on_key(&mut self, key: char) -> bool {
        self.cursor_visible = true;
        self.blink_timer = 0;

        if key == '\n' || key == '\r' {
            let cmd = self.input_buffer.trim();
            self.output_history.push_str("N> ");
            self.output_history.push_str(cmd);
            self.output_history.push('\n');

            if cmd == "help" {
                self.output_history.push_str("Commands: help, clear, echo <text>\n");
            } else if cmd == "clear" {
                self.output_history.clear();
            } else if cmd.starts_with("echo ") {
                self.output_history.push_str(&cmd[5..]);
                self.output_history.push('\n');
            } else if !cmd.is_empty() {
                self.output_history.push_str("Unknown command. Type 'help'.\n");
            }
            self.input_buffer.clear();
        } else if key == '\x08' { 
            self.input_buffer.pop();
        } else {
            self.input_buffer.push(key);
        }
        true // Redraw instantly on keypress
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(256);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 256 * 4096); }

    nyx_gui::app::run(TerminalApp::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }