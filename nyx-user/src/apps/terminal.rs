use crate::gfx::draw::{draw_char, draw_rect_simple};
use crate::gfx::font::{CHAR_WIDTH, CHAR_HEIGHT};

pub const COLS: usize = 80;
pub const ROWS: usize = 25;
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

    // --- COMMAND PROCESSOR ---
    fn process_command(&mut self) {
        self.new_line(); 
        
        // 1. Convert input buffer to string for easier parsing
        // We create a temporary buffer on the stack
        let mut cmd_buf = [0u8; MAX_INPUT];
        let mut cmd_len = 0;
        for i in 0..self.input_len {
            // Simple cast to u8 (Assuming ASCII for commands)
            cmd_buf[i] = self.input_buffer[i] as u8;
            cmd_len += 1;
        }

        if let Ok(cmd_str) = core::str::from_utf8(&cmd_buf[..cmd_len]) {
            let mut parts = cmd_str.split_whitespace();
            let command = parts.next().unwrap_or("");

            match command {
                "help" => {
                    self.write_str("NyxOS Commands:\n");
                    self.write_str("  ls            - List files\n");
                    self.write_str("  cat <file>    - Read file\n");
                    self.write_str("  touch <file>  - Create empty file\n");
                    self.write_str("  write <f> <t> - Write text to file\n");
                    self.write_str("  clear         - Clear screen\n");
                },
                "clear" => self.clear_screen(),
                "ls" => {
                    use crate::syscalls::{sys_fs_count, sys_fs_get_name};
                    let count = sys_fs_count();
                    self.write_str("Files:\n");
                    let mut name_buf = [0u8; 32];
                    for i in 0..count {
                        for b in name_buf.iter_mut() { *b = 0; }
                        let len = sys_fs_get_name(i, &mut name_buf);
                        if len > 0 {
                            if let Ok(s) = core::str::from_utf8(&name_buf[..len]) {
                                self.write_str("  - ");
                                self.write_str(s);
                                self.write_str("\n");
                            }
                        }
                    }
                },
                "touch" => {
                    if let Some(filename) = parts.next() {
                        use crate::syscalls::sys_fs_write;
                        // Create empty file (or default content)
                        sys_fs_write(filename, b"Created by touch.");
                        self.write_str("File created: ");
                        self.write_str(filename);
                        self.write_str("\n");
                    } else {
                        self.write_str("Usage: touch <filename>\n");
                    }
                },
                "cat" => {
                    if let Some(filename) = parts.next() {
                        use crate::syscalls::{sys_fs_open, sys_fs_read};
                        let id = sys_fs_open(filename);
                        if id > 0 {
                            let mut file_buf = [0u8; 256];
                            let read_len = sys_fs_read(id, &mut file_buf);
                            if read_len > 0 {
                                if let Ok(content) = core::str::from_utf8(&file_buf[..read_len]) {
                                    self.write_str(content);
                                    self.write_str("\n");
                                } else {
                                    self.write_str("Error: Not valid text file.\n");
                                }
                            } else {
                                self.write_str("(File is empty)\n");
                            }
                        } else {
                            self.write_str("Error: File not found: ");
                            self.write_str(filename);
                            self.write_str("\n");
                        }
                    } else {
                        self.write_str("Usage: cat <filename>\n");
                    }
                },
                "write" => {
                    if let Some(filename) = parts.next() {
                        // Reconstruct content from the rest of the parts
                        // This is a bit manual because split_whitespace consumes the string
                        
                        // Find where content starts in original string
                        // "write filename content..."
                        // 1. Find end of filename
                        if let Some(start_idx) = cmd_str.find(filename) {
                            let content_start = start_idx + filename.len();
                            if content_start < cmd_str.len() {
                                let content = &cmd_str[content_start..].trim_start();
                                if content.len() > 0 {
                                    use crate::syscalls::sys_fs_write;
                                    sys_fs_write(filename, content.as_bytes());
                                    self.write_str("Wrote to ");
                                    self.write_str(filename);
                                    self.write_str("\n");
                                } else {
                                    self.write_str("Error: No content to write.\n");
                                }
                            } else {
                                self.write_str("Usage: write <filename> <content>\n");
                            }
                        }
                    } else {
                        self.write_str("Usage: write <filename> <content>\n");
                    }
                },
                "" => {}, // Empty Enter
                _ => {
                    self.write_str("Unknown command: ");
                    self.write_str(command);
                    self.write_str("\n");
                }
            }
        }

        self.input_len = 0;
        self.write_str("> "); 
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
                        draw_char(fb, screen_w, screen_h, px, py, cell.char, cell.fg);
                    }
                }
            }
        }
        let cx = offset_x + (self.cursor_x * CHAR_WIDTH);
        let cy = offset_y + (self.cursor_y * CHAR_HEIGHT) + 14; 
        draw_rect_simple(fb, screen_w, screen_h, cx, cy, CHAR_WIDTH, 2, 0xFFFFFFFF);
    }
}