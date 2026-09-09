//! # Text — Meridian build-order step 15
//!
//! The design's lede is the specification: *"Text has no toolbar and no format bar — the measure is
//! capped, the column is centred, and the word count lives on the caption line with every other
//! piece of window context."* And its note: *"The application ships one column of type and nothing
//! else."*
//!
//! What that deletes from the old `notepad` is everything except the editing: the 40px toolbar, the
//! New/Open/Save buttons, the editable filename field, and the 22px status bar that said
//! `Ln 3, Col 12   47 lines   Saved …`. Between them those were two horizontal strips of chrome on a
//! window whose entire content is one column of prose.
//!
//! ## ★ This step had to add a protocol first
//!
//! Everything the strips carried has to go **somewhere**, and the design is explicit about where:
//! *"Every text editor spends a strip of its own window telling you the filename, the word count and
//! whether there are unsaved changes. Here all three are already outside the surface."* But until
//! this step the window protocol carried exactly **one** string per window, copied out of
//! `WindowHeader::title` once at creation and never read again. There was no way for a window to say
//! anything that changes.
//!
//! So `WindowHeader` gained `context: [u8; 64]` and `dirty: u32`, `NyxApp` gained
//! [`NyxApp::context`] and [`NyxApp::is_dirty`], and `apps/shell` reads both **live from the mapped
//! header every frame** — the same way it already read `content_h`/`scroll_off`. No kernel change and
//! no new IPC message; the header was already mapped in both processes and had 3.9 KB of padding.
//! QCLang's `sample.ql · 2 qubits · depth 3` moved onto the caption line at the same time, where the
//! design always had it.
//!
//! ## Structure, and why the markers stay visible
//!
//! The design's scene renders a Markdown document: an `h1`, an `h2`, paragraphs, and two emphasised
//! spans — with **no `#` or `**` anywhere**. Hiding the markers is the one thing this deliberately
//! does not copy, and it is worth being exact about why.
//!
//! A caret is a position in the **source**. If a marker is hidden, every caret movement, click and
//! backspace needs a source↔display map, and every place that map is slightly wrong is a caret that
//! lies about where the next character will land. In an editor that is not a cosmetic bug. So the
//! markers are drawn, in `--fg-4` — the same treatment the QCLang code pane gives a comment — and
//! the mapping stays exactly 1:1. The document still *looks* like the design: a `#` line is set at
//! 28px Light, a `##` line at 15px Medium, and an emphasised span at `--fg`, which is precisely the
//! design's `.ul` tone.
//!
//! (The other option — hide markers except on the line holding the caret — is what Obsidian and Bear
//! do. It keeps the caret honest and matches the design more closely, at the cost of the line
//! visibly jumping as the caret enters it. Worth revisiting; it is a bigger change than it looks.)
//!
//! ## Saving, with no toolbar to put a button on
//!
//! The design supplies no save affordance at all, and this machine has no modifier chords — keys
//! arrive as bare characters, so `Ctrl+S` does not exist and cannot be made to. **F2 saves.** F-keys
//! reach applications now (the shell claims only the two bound to brightness), and the caption
//! context says so whenever there is anything to save, which is the only moment the hint is useful.
//!
//! ## Where the geometry lives
//!
//! `nyx_meridian::prose` — the measure, the block stack, the wrap and the caret, all host-tested
//! because this machine has no QEMU and a caret drawn four pixels from where the click landed would
//! otherwise be found by booting and squinting.
//!
//! `restricted_std` is rustc's required opt-in for a std built for an out-of-tree target it doesn't
//! recognise; the nyx PAL is injected into rust-src by vendor/nyx-std/apply.py.
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

use nyx_api::{keys, sys_launch_arg};
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
use nyx_meridian::layout::Rect;
use nyx_meridian::prose::{self, Block, Row};
use nyx_meridian::text;
use nyx_meridian::tokens::Theme;

/// Where an unnamed document goes. `/mnt/nvme` is the only writable filesystem that survives a
/// power cycle, and `.md` because the structure this window understands is Markdown's.
const DEFAULT_PATH: &str = "/mnt/nvme/untitled.md";

/// The shell's theme, kept in step by `MSG_THEME_CHANGED`. Same static-plus-atomic shape as every
/// other ported interior.
static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn theme() -> Theme {
    if DARK.load(std::sync::atomic::Ordering::Relaxed) { Theme::dark() } else { Theme::light() }
}

/// One visual row: which source line it came from, and which slice of it.
///
/// Byte ranges, not char indices. The caret is a byte offset for the same reason — slicing is then
/// free, and every movement helper steps by characters explicitly, which is the honest place for
/// that cost to be paid.
struct Line {
    src: usize,
    range: core::ops::Range<usize>,
    row: Row,
}

struct Text {
    width: i32,
    height: i32,

    path: String,
    name: String,
    /// The document, one `String` per source line. Never empty — an empty document is one empty
    /// line, so `lines[cy]` is always valid and the caret always has somewhere to be.
    lines: Vec<String>,
    /// Caret: source line, and a **byte** offset into it, always on a char boundary.
    cy: usize,
    cx: usize,
    /// The x the caret is trying to keep while moving vertically. Without it, moving down through a
    /// short line and back up lands in the wrong column — the classic "the caret walked left".
    goal_x: Option<i32>,

    scroll: i32,
    dirty: bool,
    /// Set after a save or a failure, and shown on the caption until the next edit. The design has
    /// nowhere else to put it, and a save that reports nothing is a save you cannot trust.
    note: Option<String>,
    /// The caption context, rebuilt in `draw`. Held as a field because `NyxApp::context` returns a
    /// borrow — the message pump reads it every frame and must not be handed a temporary.
    ctx: String,

    /// The laid-out rows, rebuilt whenever the text or the width changes. Cached because a wrap
    /// measures every glyph of every line, and this window repaints on every keystroke.
    rows: Vec<Line>,
    laid_out_for: (i32, usize),
}

impl Text {
    fn new() -> Self {
        // Files can hand this app a path — the same `SYS_LAUNCH_ARG` route that opens a .png in the
        // Image Viewer. With nothing to open, start on the scratch document.
        let mut buf = [0u8; 256];
        let n = sys_launch_arg(&mut buf);
        let arg = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();
        let path = if arg.is_empty() { DEFAULT_PATH.to_string() } else { arg.to_string() };

        // A file that does not exist yet is a NEW document, not an error: the default path names a
        // scratch file that will not exist on a fresh machine, and opening it must not read as a
        // failure.
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<String> = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(String::from).collect()
        };
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();

        Self {
            width: 592,
            height: 544,
            path,
            name,
            lines,
            cy: 0,
            cx: 0,
            goal_x: None,
            scroll: 0,
            dirty: false,
            note: None,
            ctx: String::new(),
            rows: Vec::new(),
            laid_out_for: (-1, usize::MAX),
        }
    }

    // ── structure ────────────────────────────────────────────────────────────────────────────────

    /// The block a source line is, and how many bytes of it are the marker.
    ///
    /// Markdown's ATX headings, and nothing else. `###` and deeper collapse to `h2`: the design's
    /// type scale has two heading sizes for this window and inventing a third would be inventing a
    /// token.
    fn classify(line: &str) -> (Block, usize) {
        let hashes = line.bytes().take_while(|&b| b == b'#').count();
        if hashes == 0 || line.as_bytes().get(hashes) != Some(&b' ') {
            return (Block::Body, 0);
        }
        let block = if hashes == 1 { Block::H1 } else { Block::H2 };
        (block, hashes + 1)
    }

    fn column(&self) -> Rect {
        prose::column(self.width, self.height)
    }

    /// Rebuild the row list if the width or the document changed under it.
    ///
    /// Keyed on `(column width, total bytes)`. Not a hash: the point is to skip the wrap on the
    /// frames where nothing happened (a hover, a theme change, a scroll), and any edit at all
    /// changes the byte count except a same-length replacement — which cannot happen, because this
    /// editor has no replace.
    fn relayout(&mut self) {
        let col = self.column();
        let bytes: usize = self.lines.iter().map(|l| l.len() + 1).sum();
        if self.laid_out_for == (col.w, bytes) {
            return;
        }
        self.laid_out_for = (col.w, bytes);
        self.rows.clear();

        // Wrap first, then let `prose::rows` decide which rows carry margins — it is the one place
        // that knows a blank source line and a CSS margin are two ways of saying the same thing.
        let mut ranges: Vec<Vec<core::ops::Range<usize>>> = Vec::with_capacity(self.lines.len());
        let mut src: Vec<prose::Source> = Vec::with_capacity(self.lines.len());
        for line in &self.lines {
            let (block, _) = Self::classify(line);
            let r = prose::wrap(block.style(), line, col.w);
            src.push(prose::Source {
                block,
                wrapped: r.len(),
                blank: line.trim().is_empty(),
            });
            ranges.push(r);
        }

        let mut flat = prose::rows(&src).into_iter();
        for (i, rs) in ranges.into_iter().enumerate() {
            for r in rs {
                let Some(row) = flat.next() else { return };
                self.rows.push(Line { src: i, range: r, row });
            }
        }
    }

    fn row_kinds(&self) -> Vec<Row> {
        self.rows.iter().map(|l| l.row).collect()
    }

    /// The visual row the caret is on, and the byte offset within that row's slice.
    fn caret_row(&self) -> usize {
        let mut fallback = 0;
        for (i, l) in self.rows.iter().enumerate() {
            if l.src != self.cy {
                continue;
            }
            fallback = i;
            if self.cx >= l.range.start && self.cx <= l.range.end {
                // A caret exactly on a wrap point belongs to the row it can be SEEN on — the end of
                // the row it terminates, not the start of the next. Except at a real line end, where
                // there is no next row.
                return i;
            }
        }
        fallback
    }

    fn line(&self, i: usize) -> &str {
        &self.lines[i]
    }

    /// The pen x for byte offset `at` on visual row `i`.
    fn caret_x(&self, col: Rect, i: usize) -> i32 {
        let Some(l) = self.rows.get(i) else { return col.x };
        let src = self.line(l.src);
        let start = l.range.start.max(0);
        let at = self.cx.clamp(l.range.start, l.range.end);
        col.x + text::width(l.row.block.style(), &src[start..at])
    }

    // ── editing ──────────────────────────────────────────────────────────────────────────────────

    fn touch(&mut self) {
        self.dirty = true;
        self.note = None;
        self.goal_x = None;
    }

    fn insert(&mut self, ch: char) {
        let cx = self.cx.min(self.lines[self.cy].len());
        self.lines[self.cy].insert(cx, ch);
        self.cx = cx + ch.len_utf8();
        self.touch();
    }

    fn newline(&mut self) {
        let cx = self.cx.min(self.lines[self.cy].len());
        let rest = self.lines[self.cy].split_off(cx);
        self.lines.insert(self.cy + 1, rest);
        self.cy += 1;
        self.cx = 0;
        self.touch();
    }

    fn backspace(&mut self) {
        if self.cx > 0 {
            // Back over one CHARACTER, not one byte.
            let prev = self.lines[self.cy][..self.cx]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.lines[self.cy].replace_range(prev..self.cx, "");
            self.cx = prev;
        } else if self.cy > 0 {
            let cur = self.lines.remove(self.cy);
            self.cy -= 1;
            self.cx = self.lines[self.cy].len();
            self.lines[self.cy].push_str(&cur);
        } else {
            return;
        }
        self.touch();
    }

    fn delete_forward(&mut self) {
        if self.cx < self.lines[self.cy].len() {
            let next = self.lines[self.cy][self.cx..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cx + i)
                .unwrap_or(self.lines[self.cy].len());
            self.lines[self.cy].replace_range(self.cx..next, "");
        } else if self.cy + 1 < self.lines.len() {
            let next = self.lines.remove(self.cy + 1);
            self.lines[self.cy].push_str(&next);
        } else {
            return;
        }
        self.touch();
    }

    fn left(&mut self) {
        self.goal_x = None;
        if self.cx > 0 {
            self.cx = self.lines[self.cy][..self.cx]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        } else if self.cy > 0 {
            self.cy -= 1;
            self.cx = self.lines[self.cy].len();
        }
    }

    fn right(&mut self) {
        self.goal_x = None;
        if self.cx < self.lines[self.cy].len() {
            self.cx = self.lines[self.cy][self.cx..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cx + i)
                .unwrap_or(self.lines[self.cy].len());
        } else if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = 0;
        }
    }

    /// Move the caret by `delta` VISUAL rows, keeping its x.
    ///
    /// Visual, not source: in a wrapped paragraph, Down must go to the next line on screen. Moving
    /// by source line would skip a whole paragraph in one press, which is what a text editor that
    /// wraps but navigates by source feels like — and it feels broken.
    fn vertical(&mut self, delta: i32) {
        let col = self.column();
        let here = self.caret_row();
        let want = self.goal_x.unwrap_or_else(|| self.caret_x(col, here));
        let target = (here as i32 + delta).clamp(0, self.rows.len().saturating_sub(1) as i32) as usize;
        if target == here && delta != 0 {
            // Already at the end: park the caret at the document's start or end, which is what every
            // editor does and what makes Ctrl-less navigation bearable.
            if delta < 0 {
                self.cy = 0;
                self.cx = 0;
            } else {
                self.cy = self.lines.len() - 1;
                self.cx = self.lines[self.cy].len();
            }
            self.goal_x = None;
            return;
        }
        self.place_on_row(target, want);
        self.goal_x = Some(want);
    }

    /// Put the caret on visual row `i` at the character nearest pen position `x`.
    fn place_on_row(&mut self, i: usize, x: i32) {
        let col = self.column();
        let Some(l) = self.rows.get(i) else { return };
        let (src, range, style) = (l.src, l.range.clone(), l.row.block.style());
        let s = self.lines[src][range.clone()].to_string();

        let mut best = range.start;
        let mut pen = col.x;
        for (off, ch) in s.char_indices() {
            let adv = text::width(style, &s[off..off + ch.len_utf8()]);
            if x < pen + adv / 2 {
                best = range.start + off;
                self.cy = src;
                self.cx = best;
                return;
            }
            pen += adv;
            best = range.start + off + ch.len_utf8();
        }
        self.cy = src;
        self.cx = best;
    }

    fn save(&mut self) {
        let content = self.lines.join("\n");
        match std::fs::write(&self.path, content.as_bytes()) {
            Ok(()) => {
                self.dirty = false;
                self.note = Some(format!("saved {} bytes", content.len()));
            }
            // A failed save must be LOUD. It is the one action in this window that can lose work,
            // and the caption is the only surface left to say so on.
            Err(e) => self.note = Some(format!("save failed: {}", e)),
        }
    }

    /// `atlas-sizes.md · 412 words`, plus whatever the window most needs to say.
    ///
    /// Order matters: a save failure displaces the word count, because a count is ambient and a
    /// failed write is the one thing here that can lose work.
    fn refresh_context(&mut self) {
        let words: usize = self.lines.iter().map(|l| prose::word_count(l)).sum();
        let mut s = format!("{} \u{00B7} {} word{}", self.name, words, if words == 1 { "" } else { "s" });
        match (&self.note, self.dirty) {
            (Some(n), _) => {
                s.push_str(" \u{00B7} ");
                s.push_str(n);
            }
            // The hint appears only while there is something to lose. There is no toolbar to put a
            // save button on and no Ctrl chord on this machine, so without it F2 is undiscoverable.
            (None, true) => s.push_str(" \u{00B7} unsaved, F2 to save"),
            (None, false) => {}
        }
        if s != self.ctx {
            self.ctx = s;
        }
    }

    fn max_scroll(&self) -> i32 {
        (prose::height(&self.row_kinds()) - self.height).max(0)
    }

    /// Scroll so the caret is on screen. Called after every edit and every movement — a caret that
    /// walks off the bottom while you type is the single most common way a hand-rolled editor
    /// breaks.
    fn reveal_caret(&mut self) {
        let kinds = self.row_kinds();
        let i = self.caret_row();
        let Some(top) = prose::row_top(&kinds, i) else { return };
        let col = self.column();
        let (top, h) = (col.y + top, kinds[i].block.line_h());
        if top < self.scroll {
            self.scroll = top;
        } else if top + h > self.scroll + self.height {
            self.scroll = top + h - self.height;
        }
        self.scroll = self.scroll.clamp(0, self.max_scroll());
    }
}

/// True for Unicode Private-Use-Area code points — the nav keys live at U+E010 and must never be
/// inserted as literal text.
fn is_pua(c: char) -> bool {
    (0xE000..=0xF8FF).contains(&(c as u32))
}

/// Draw one row's text, `**emphasis**` at `--fg` and its markers at `--fg-4`.
///
/// Inline structure is per-row rather than per-line, so an emphasis that spans a wrap point is set
/// correctly on both rows. The state that needs carrying across is one boolean, which is cheap
/// enough to recompute from the start of the source line.
fn draw_runs(canvas: &mut Canvas, x: i32, y: i32, t: &Theme, style: nyx_meridian::tokens::Style,
             src: &str, range: core::ops::Range<usize>, marker: usize) -> i32 {
    let mut pen = x;
    let mut emph = emphasis_at(src, range.start);
    let mut i = range.start;
    while i < range.end {
        // The heading's `# ` prefix: drawn, but quiet. See the module docs for why it is not hidden.
        if i < marker {
            let end = marker.min(range.end);
            pen += text::draw(canvas, pen, y, style, t.fg_4, &src[i..end]);
            i = end;
            continue;
        }
        if src[i..].starts_with("**") && i + 2 <= range.end {
            pen += text::draw(canvas, pen, y, style, t.fg_4, "**");
            emph = !emph;
            i += 2;
            continue;
        }
        // Up to the next marker, or the end of the row.
        let next = src[i..range.end].find("**").map(|o| i + o).unwrap_or(range.end);
        let next = if next == i { range.end } else { next };
        pen += text::draw(canvas, pen, y, style, if emph { t.fg } else { t.fg_2 }, &src[i..next]);
        i = next;
    }
    pen
}

/// Whether byte `at` of `src` is inside a `**…**` span, counted from the start of the line.
fn emphasis_at(src: &str, at: usize) -> bool {
    let mut count = 0;
    let mut i = 0;
    while i + 2 <= at {
        if src[i..].starts_with("**") {
            count += 1;
            i += 2;
        } else {
            i += src[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    count % 2 == 1
}

impl NyxApp for Text {
    fn title(&self) -> &str {
        "Text"
    }
    fn icon_path(&self) -> &str {
        "/mnt/nvme/apps/Notepad.nyx/icon.png"
    }
    /// The design's own window: 592 wide, quantised to 16 like every width in the system.
    fn initial_width(&self) -> usize {
        592
    }
    fn initial_height(&self) -> usize {
        544
    }

    /// `notes/atlas-sizes.md · 412 words` — and, when there is something to lose, how to save it.
    ///
    /// The save hint lives here rather than anywhere in the window because the design gives this app
    /// no toolbar to put a button on and this machine has no modifier chords. It appears only while
    /// the document is dirty, which is the only moment it is worth screen space.
    fn context(&self) -> &str {
        &self.ctx
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn update(&mut self) -> bool {
        false
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let t = theme();
        self.width = canvas.width as i32;
        self.height = canvas.height as i32;
        self.relayout();
        self.refresh_context();
        self.scroll = self.scroll.clamp(0, self.max_scroll());

        canvas.fill_rect(0, 0, canvas.width, canvas.height, t.surface);

        let col = self.column();
        let kinds = self.row_kinds();
        let caret_row = self.caret_row();

        for i in 0..self.rows.len() {
            let Some(r) = prose::row(col, &kinds, i) else { continue };
            let y = r.y - self.scroll;
            // `.tx { overflow: hidden }` — cull rather than clip. Nothing in `Canvas` clips to a
            // rect, and a row drawn far off screen still walks every glyph.
            if y + r.h < 0 || y >= self.height {
                continue;
            }
            let l = &self.rows[i];
            let (_, marker) = Text::classify(&self.lines[l.src]);
            draw_runs(
                canvas, col.x, y, &t, l.row.block.style(),
                &self.lines[l.src], l.range.clone(),
                if l.row.first { marker } else { 0 },
            );
            if i == caret_row {
                let c = prose::caret(Rect::new(r.x, y, r.w, r.h), self.caret_x(col, i));
                canvas.fill_rect(
                    c.x.max(0) as usize, c.y.max(0) as usize,
                    c.w as usize, c.h as usize, t.accent,
                );
            }
        }
    }

    fn content_height(&self) -> usize {
        let h = prose::height(&self.row_kinds());
        if h > self.height { h as usize } else { 0 }
    }
    fn scroll_offset(&self) -> usize {
        self.scroll.max(0) as usize
    }
    fn on_scroll(&mut self, off: usize) -> bool {
        let new = (off as i32).clamp(0, self.max_scroll());
        if new != self.scroll {
            self.scroll = new;
            return true;
        }
        false
    }

    fn on_theme(&mut self, dark: bool) -> bool {
        DARK.swap(dark, std::sync::atomic::Ordering::Relaxed) != dark
    }

    fn on_key(&mut self, key: char) -> bool {
        self.relayout();
        match key {
            // F2 saves. There is no toolbar to put a button on and no Ctrl chord to bind — see the
            // module docs. Tested before everything else because an F-key is a PUA codepoint and
            // would otherwise fall through to the "not a control character" arm's rejection.
            c if keys::fkey(c) == Some(2) => self.save(),
            '\n' | '\r' => self.newline(),
            '\x08' => self.backspace(),
            '\t' => {
                for _ in 0..4 {
                    self.insert(' ');
                }
            }
            keys::LEFT => self.left(),
            keys::RIGHT => self.right(),
            keys::UP => self.vertical(-1),
            keys::DOWN => self.vertical(1),
            keys::HOME => {
                // The start of the VISUAL row, which in a wrapped paragraph is not the start of the
                // source line. Home going to somewhere off screen would be surprising.
                let i = self.caret_row();
                self.cx = self.rows.get(i).map(|l| l.range.start).unwrap_or(0);
                self.goal_x = None;
            }
            keys::END => {
                let i = self.caret_row();
                self.cx = self.rows.get(i).map(|l| l.range.end).unwrap_or(0);
                self.goal_x = None;
            }
            keys::DELETE => self.delete_forward(),
            keys::PAGE_UP => {
                let step = (self.height / Block::Body.line_h()).max(1);
                self.vertical(-step);
            }
            keys::PAGE_DOWN => {
                let step = (self.height / Block::Body.line_h()).max(1);
                self.vertical(step);
            }
            c if !c.is_control() && !is_pua(c) => self.insert(c),
            _ => return false,
        }
        self.relayout();
        self.reveal_caret();
        true
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        self.relayout();
        let col = self.column();
        let kinds = self.row_kinds();
        let y = my as i32 + self.scroll - col.y;
        let Some(i) = prose::row_at(&kinds, y) else { return false };
        self.place_on_row(i, mx as i32);
        self.goal_x = None;
        true
    }
}

fn main() {
    // The three faces a ported interior sets type in. Not `register_all`: this window draws no `d1`
    // and no monospace, and those two are ~690 KB of font it would never render a glyph of.
    nyx_meridian::font::register_text();
    nyx_gui::app::run(Text::new());
}
