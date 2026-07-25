// Nyx Notepad — a proper multi-line text editor that saves to /mnt/nvme.
//
// A `target_os = "nyx"` std binary (same entry skeleton as apps/stdgui) so it can persist files with
// std::fs::write and load them with read_to_string. nyx-gui has no multi-line text widget, so the
// editor is hand-managed here: a Vec<String> of lines + a (row,col) caret, rendered with the
// proportional TTF font (caret x is the SUM of per-glyph advances — never col*8).
//
// Caret navigation uses the arrow/Home/End/Delete keys the kernel now delivers as Private-Use-Area
// chars (nyx_api::keys::*, wired in nyx-kernel/src/shell.rs). Save/Open/New are on-screen buttons
// (Ctrl chords never reach apps).
#![feature(restricted_std)]

core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, rsp",
    "mov rdi, rsp",
    "and rsp, -16",
    "call __nyx_setup_main_tls",
    "mov rdi, [rbx]",
    "lea rsi, [rbx + 8]",
    "and rsp, -16",
    "call main",
    "mov edi, eax",
    "mov eax, 231",
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

use nyx_api::keys;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};

const BG: u32 = Color::WARM_BG;
const SURFACE: u32 = Color::WARM_SURFACE;
const BORDER: u32 = Color::WARM_BORDER;
const TEXT: u32 = Color::TEXT_DARK;
const MUTED: u32 = Color::TEXT_MUTED;
const ACCENT: u32 = Color::ACCENT_PRIMARY;

const TOOLBAR_H: usize = 40;
const STATUS_H: usize = 22;
const TEXT_X: usize = 12;
const TEXT_TOP: usize = TOOLBAR_H + 8;

#[derive(PartialEq)]
enum Focus {
    Filename,
    Editor,
}

struct Notepad {
    lines: Vec<String>,
    cx: usize, // caret column (char index within the line)
    cy: usize, // caret row (line index)
    scroll: usize, // first visible line
    filename: String,
    focus: Focus,
    status: String,
    width: usize,
    height: usize,
}

impl Notepad {
    fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cx: 0,
            cy: 0,
            scroll: 0,
            filename: String::from("/mnt/nvme/untitled.txt"),
            focus: Focus::Editor,
            status: String::from("New file. Type, then click Save."),
            width: 700,
            height: 500,
        }
    }

    // --- geometry (recomputed from live width; no hardcoded sizes) ---
    fn btn_rects(&self) -> [(usize, usize, usize, usize); 3] {
        let w = 62;
        let h = 26;
        let y = 7;
        let save = (self.width.saturating_sub(w + 10), y, w, h);
        let open = (self.width.saturating_sub(2 * w + 18), y, w, h);
        let new = (self.width.saturating_sub(3 * w + 26), y, w, h);
        [new, open, save]
    }

    fn filename_rect(&self) -> (usize, usize, usize, usize) {
        let right = self.width.saturating_sub(3 * 62 + 34);
        (12, 7, right.saturating_sub(12), 26)
    }

    fn line_h(&self) -> usize {
        nyx_gui::font::line_height(1)
    }

    fn visible_rows(&self) -> usize {
        let editor_h = self.height.saturating_sub(TEXT_TOP + STATUS_H);
        (editor_h / self.line_h()).max(1)
    }

    // --- char-aware line edit helpers (lines are UTF-8 Strings; caret is a char index) ---
    fn line_chars(&self, row: usize) -> Vec<char> {
        self.lines[row].chars().collect()
    }

    fn set_line(&mut self, row: usize, chars: Vec<char>) {
        self.lines[row] = chars.into_iter().collect();
    }

    fn cur_len(&self) -> usize {
        self.lines[self.cy].chars().count()
    }

    fn ensure_visible(&mut self) {
        if self.cy < self.scroll {
            self.scroll = self.cy;
        }
        let rows = self.visible_rows();
        if self.cy >= self.scroll + rows {
            self.scroll = self.cy + 1 - rows;
        }
    }

    // --- actions ---
    fn do_save(&mut self) {
        let content = self.lines.join("\n");
        match std::fs::write(&self.filename, content.as_bytes()) {
            Ok(()) => self.status = format!("Saved {} ({} bytes)", self.filename, content.len()),
            Err(e) => self.status = format!("Save FAILED: {}", e),
        }
    }

    fn do_open(&mut self) {
        match std::fs::read_to_string(&self.filename) {
            Ok(s) => {
                self.lines = if s.is_empty() {
                    vec![String::new()]
                } else {
                    // Trailing newline shouldn't create a phantom blank line beyond one.
                    s.split('\n').map(String::from).collect()
                };
                self.cx = 0;
                self.cy = 0;
                self.scroll = 0;
                self.status = format!("Opened {} ({} bytes)", self.filename, s.len());
            }
            Err(e) => self.status = format!("Open FAILED: {}", e),
        }
    }

    fn do_new(&mut self) {
        self.lines = vec![String::new()];
        self.cx = 0;
        self.cy = 0;
        self.scroll = 0;
        self.status = String::from("New file.");
    }

    // --- editor key handling ---
    fn insert_char(&mut self, ch: char) {
        let mut chars = self.line_chars(self.cy);
        let col = self.cx.min(chars.len());
        chars.insert(col, ch);
        self.set_line(self.cy, chars);
        self.cx = col + 1;
    }

    fn insert_newline(&mut self) {
        let chars = self.line_chars(self.cy);
        let col = self.cx.min(chars.len());
        let (left, right) = chars.split_at(col);
        let left: String = left.iter().collect();
        let right: String = right.iter().collect();
        self.lines[self.cy] = left;
        self.lines.insert(self.cy + 1, right);
        self.cy += 1;
        self.cx = 0;
    }

    fn backspace(&mut self) {
        if self.cx > 0 {
            let mut chars = self.line_chars(self.cy);
            chars.remove(self.cx - 1);
            self.cx -= 1;
            self.set_line(self.cy, chars);
        } else if self.cy > 0 {
            // Merge this line onto the end of the previous one.
            let cur = self.lines.remove(self.cy);
            self.cy -= 1;
            self.cx = self.lines[self.cy].chars().count();
            self.lines[self.cy].push_str(&cur);
        }
    }

    fn delete_forward(&mut self) {
        let len = self.cur_len();
        if self.cx < len {
            let mut chars = self.line_chars(self.cy);
            chars.remove(self.cx);
            self.set_line(self.cy, chars);
        } else if self.cy + 1 < self.lines.len() {
            // Pull the next line up onto this one.
            let next = self.lines.remove(self.cy + 1);
            self.lines[self.cy].push_str(&next);
        }
    }

    fn move_left(&mut self) {
        if self.cx > 0 {
            self.cx -= 1;
        } else if self.cy > 0 {
            self.cy -= 1;
            self.cx = self.cur_len();
        }
    }

    fn move_right(&mut self) {
        let len = self.cur_len();
        if self.cx < len {
            self.cx += 1;
        } else if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cy > 0 {
            self.cy -= 1;
            self.cx = self.cx.min(self.cur_len());
        }
    }

    fn move_down(&mut self) {
        if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = self.cx.min(self.cur_len());
        }
    }
}

impl NyxApp for Notepad {
    fn icon_path(&self) -> &str { "/mnt/nvme/apps/Notepad.nyx/icon.png" }
    fn title(&self) -> &str {
        "Nyx Notepad"
    }
    fn initial_width(&self) -> usize {
        700
    }
    fn initial_height(&self) -> usize {
        500
    }

    fn content_height(&self) -> usize {
        self.lines.len() * self.line_h()
    }
    fn scroll_offset(&self) -> usize {
        self.scroll * self.line_h()
    }
    fn on_scroll(&mut self, new_offset: usize) -> bool {
        self.scroll = new_offset / self.line_h();
        true
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        self.width = canvas.width;
        self.height = canvas.height;
        let line_h = self.line_h();

        canvas.fill_rect(0, 0, self.width, self.height, BG);

        // Toolbar
        canvas.fill_rect(0, 0, self.width, TOOLBAR_H, SURFACE);
        canvas.fill_rect(0, TOOLBAR_H, self.width, 1, BORDER);

        // Filename field
        let (fx, fy, fw, fh) = self.filename_rect();
        canvas.fill_rect(fx, fy, fw, fh, Color::WHITE);
        let field_border = if self.focus == Focus::Filename { ACCENT } else { BORDER };
        canvas.fill_rect(fx, fy, fw, 1, field_border);
        canvas.fill_rect(fx, fy + fh - 1, fw, 1, field_border);
        canvas.print_str(fx + 5, fy + 5, &self.filename, TEXT, 1);

        // Buttons
        let labels = ["New", "Open", "Save"];
        for (i, (bx, by, bw, bh)) in self.btn_rects().iter().enumerate() {
            canvas.fill_rect(*bx, *by, *bw, *bh, BORDER);
            let tw = Canvas::text_width(labels[i], 1);
            let tx = bx + bw.saturating_sub(tw) / 2;
            canvas.print_str(tx, by + 5, labels[i], TEXT, 1);
        }

        // Editor text
        let rows = self.visible_rows();
        for i in 0..rows {
            let li = self.scroll + i;
            if li >= self.lines.len() {
                break;
            }
            let y = TEXT_TOP + i * line_h;
            let mut x = TEXT_X;
            for ch in self.lines[li].chars() {
                canvas.draw_char(x, y, ch, TEXT, 1);
                x += nyx_gui::font::advance(ch, 1);
            }
            // Caret on the active line (only when the editor is focused)
            if self.focus == Focus::Editor && li == self.cy {
                let mut caret_x = TEXT_X;
                for ch in self.lines[li].chars().take(self.cx) {
                    caret_x += nyx_gui::font::advance(ch, 1);
                }
                canvas.fill_rect(caret_x, y, 2, line_h, ACCENT);
            }
        }

        // Status bar
        let sy = self.height.saturating_sub(STATUS_H);
        canvas.fill_rect(0, sy, self.width, STATUS_H, SURFACE);
        canvas.fill_rect(0, sy, self.width, 1, BORDER);
        let info = format!(
            "Ln {}, Col {}   {} lines   {}",
            self.cy + 1,
            self.cx + 1,
            self.lines.len(),
            self.status
        );
        canvas.print_str(8, sy + 4, &info, MUTED, 1);
    }

    fn on_key(&mut self, key: char) -> bool {
        match self.focus {
            Focus::Filename => {
                if key == '\n' || key == '\r' {
                    self.do_open();
                    self.focus = Focus::Editor;
                } else if key == '\x08' {
                    self.filename.pop();
                } else if !key.is_control() && !is_pua(key) {
                    self.filename.push(key);
                } else {
                    return false;
                }
            }
            Focus::Editor => {
                match key {
                    '\n' | '\r' => self.insert_newline(),
                    '\x08' => self.backspace(),
                    '\t' => {
                        for _ in 0..4 {
                            self.insert_char(' ');
                        }
                    }
                    keys::LEFT => self.move_left(),
                    keys::RIGHT => self.move_right(),
                    keys::UP => self.move_up(),
                    keys::DOWN => self.move_down(),
                    keys::HOME => self.cx = 0,
                    keys::END => self.cx = self.cur_len(),
                    keys::DELETE => self.delete_forward(),
                    keys::PAGE_UP => {
                        let step = self.visible_rows();
                        self.cy = self.cy.saturating_sub(step);
                        self.cx = self.cx.min(self.cur_len());
                    }
                    keys::PAGE_DOWN => {
                        let step = self.visible_rows();
                        self.cy = (self.cy + step).min(self.lines.len() - 1);
                        self.cx = self.cx.min(self.cur_len());
                    }
                    c if !c.is_control() && !is_pua(c) => self.insert_char(c),
                    _ => return false,
                }
                self.ensure_visible();
            }
        }
        true
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        // Toolbar buttons
        let rects = self.btn_rects();
        for (i, (bx, by, bw, bh)) in rects.iter().enumerate() {
            if mx >= *bx && mx < bx + bw && my >= *by && my < by + bh {
                match i {
                    0 => self.do_new(),
                    1 => self.do_open(),
                    _ => self.do_save(),
                }
                return true;
            }
        }

        // Filename field
        let (fx, fy, fw, fh) = self.filename_rect();
        if mx >= fx && mx < fx + fw && my >= fy && my < fy + fh {
            self.focus = Focus::Filename;
            return true;
        }

        // Editor area → focus + place caret at the click
        if my >= TEXT_TOP {
            self.focus = Focus::Editor;
            let line_h = self.line_h();
            let row = self.scroll + (my - TEXT_TOP) / line_h;
            self.cy = row.min(self.lines.len() - 1);
            // Walk glyph advances to find the nearest column to mx.
            let mut x = TEXT_X;
            let mut col = 0;
            for ch in self.lines[self.cy].chars() {
                let adv = nyx_gui::font::advance(ch, 1);
                if mx < x + adv / 2 {
                    break;
                }
                x += adv;
                col += 1;
            }
            self.cx = col;
            return true;
        }
        false
    }
}

/// True for Unicode Private-Use-Area code points (our nav keys live at U+E010..). Prevents those
/// synthetic key chars from being inserted as literal text.
fn is_pua(c: char) -> bool {
    let u = c as u32;
    (0xE000..=0xF8FF).contains(&u)
}

fn main() {
    println!("notepad: multi-line editor, saves to /mnt/nvme (std::fs)");
    nyx_gui::app::run(Notepad::new());
}
