// nyx-user/src/apps/terminal.rs

// FIX: Import the drawing functions here
use crate::gfx::draw::{draw_char, draw_rect_simple};

pub const COLS: usize = 80;
pub const ROWS: usize = 25;
pub const CHAR_WIDTH: usize = 9;
pub const CHAR_HEIGHT: usize = 16;
const MAX_INPUT: usize = 128; 

#[derive(Copy, Clone)]
pub struct TerminalCell {
    pub char: char,
    pub fg: u32,
    pub bg: u32,
}

pub struct Terminal {
    pub buffer: [[TerminalCell; COLS]; ROWS],
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub default_fg: u32,
    pub default_bg: u32,
    input_buffer: [char; MAX_INPUT],
    input_len: usize,
}

impl Terminal {
    pub fn new() -> Self {
        let empty = TerminalCell { char: ' ', fg: 0xFFFFFF, bg: 0x1E1E1E };
        Self {
            buffer: [[empty; COLS]; ROWS],
            cursor_x: 0,
            cursor_y: 0,
            default_fg: 0xFFFFFF,
            default_bg: 0x1E1E1E,
            input_buffer: ['\0'; MAX_INPUT],
            input_len: 0,
        }
    }

    pub fn handle_key(&mut self, c: char) {
        if c == '\n' {
            self.process_command();
        } else if c == '\x08' {
            self.safe_backspace();
        } else {
            if self.input_len < MAX_INPUT && self.cursor_x < COLS - 1 {
                self.input_buffer[self.input_len] = c;
                self.input_len += 1;
                self.write_char_visual(c);
            }
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\n' { self.new_line(); } else { self.write_char_visual(c); }
        }
    }

    fn write_char_visual(&mut self, c: char) {
        if self.cursor_x >= COLS { self.new_line(); }
        self.buffer[self.cursor_y][self.cursor_x] = TerminalCell {
            char: c, fg: self.default_fg, bg: self.default_bg,
        };
        self.cursor_x += 1;
    }

    fn new_line(&mut self) {
        self.cursor_x = 0;
        if self.cursor_y < ROWS - 1 { self.cursor_y += 1; } else { self.scroll(); }
    }

    fn scroll(&mut self) {
        for y in 1..ROWS { self.buffer[y - 1] = self.buffer[y]; }
        let empty = TerminalCell { char: ' ', fg: self.default_fg, bg: self.default_bg };
        self.buffer[ROWS - 1] = [empty; COLS];
    }

    fn safe_backspace(&mut self) {
        if self.input_len > 0 {
            self.input_len -= 1;
            if self.cursor_x > 0 {
                self.cursor_x -= 1;
                self.buffer[self.cursor_y][self.cursor_x].char = ' ';
            }
        }
    }

    fn process_command(&mut self) {
        self.new_line(); 
        let cmd_len = self.input_len;
        
        if self.match_cmd(&self.input_buffer[..cmd_len], "help") {
            self.write_str("Available commands:\n");
            self.write_str("  help    - Show this message\n");
            self.write_str("  clear   - Clear the screen\n");
            self.write_str("  ver     - Show OS version\n");
            self.write_str("  whoami  - Show current user\n");
        } 
        else if self.match_cmd(&self.input_buffer[..cmd_len], "clear") {
            self.clear_screen();
        }
        else if self.match_cmd(&self.input_buffer[..cmd_len], "ver") {
            self.write_str("NyxOS User Shell v0.2.0 (Alpha)\n");
        }
        else if self.match_cmd(&self.input_buffer[..cmd_len], "whoami") {
            self.write_str("root\n");
        }
        else if cmd_len > 0 {
            self.write_str("Unknown command: ");
            for i in 0..cmd_len { 
                let c = self.input_buffer[i];
                self.write_char_visual(c); 
            }
            self.write_str("\n");
        }
        self.input_len = 0;
        self.write_str("> "); 
    }

    fn match_cmd(&self, buffer: &[char], cmd: &str) -> bool {
        if buffer.len() != cmd.len() { return false; }
        for (i, c) in cmd.chars().enumerate() { if buffer[i] != c { return false; } }
        true
    }

    fn clear_screen(&mut self) {
        let empty = TerminalCell { char: ' ', fg: self.default_fg, bg: self.default_bg };
        for y in 0..ROWS { self.buffer[y] = [empty; COLS]; }
        self.cursor_x = 0; self.cursor_y = 0;
    }

    pub fn draw(&self, fb: &mut [u32], screen_w: usize, screen_h: usize, win_x: usize, win_y: usize) {
        let offset_x = win_x + 8;
        let offset_y = win_y + 35;

        for y in 0..ROWS {
            for x in 0..COLS {
                let cell = self.buffer[y][x];
                let px = offset_x + (x * CHAR_WIDTH);
                let py = offset_y + (y * CHAR_HEIGHT);
                
                if px < screen_w && py < screen_h {
                    if cell.char != ' ' {
                        // FIX: Updated call to just draw_char()
                        draw_char(fb, screen_w, screen_h, px, py, cell.char, cell.fg);
                    }
                }
            }
        }
        // Draw Cursor
        let cx = offset_x + (self.cursor_x * CHAR_WIDTH);
        let cy = offset_y + (self.cursor_y * CHAR_HEIGHT) + 14; 
        // FIX: Updated call to just draw_rect_simple()
        draw_rect_simple(fb, screen_w, screen_h, cx, cy, CHAR_WIDTH, 2, 0xFFFFFFFF);
    }
}