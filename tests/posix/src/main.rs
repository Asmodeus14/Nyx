// PosixProbe — the POSIX conformance harness for Nyx.
//
// Every effort estimate on the road to a libc (and from there to a real browser engine) is a guess
// until someone has actually asked this kernel, one syscall at a time, "do you have this, and does
// it do anything?" That is all this program is. It answers at two layers — raw `syscall` and the
// Rust std PAL — because they fail independently: a working kernel call sitting behind a PAL that
// still returns `Unsupported` is a different job from a syscall that was never written.
//
// The verdicts are deliberately harsher than a return code. `STUB` means the call returned success
// and demonstrably changed nothing, which is the most dangerous state a syscall can be in: every
// layer above it believes the operation happened.
//
// A `target_os = "nyx"` std binary; built standalone via Build-std.sh like the browser and notepad.
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

mod probes;
mod raw;
mod report;
mod sandbox;

use nyx_api::keys;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};

use report::{Report, Verdict};

const BG: u32 = Color::WARM_BG;
const SURFACE: u32 = Color::WARM_SURFACE;
const BORDER: u32 = Color::WARM_BORDER;
const TEXT: u32 = Color::TEXT_DARK;
const MUTED: u32 = Color::TEXT_MUTED;

const HEADER_H: usize = 62;
const ROW_H: usize = 34;
const PAD: usize = 12;

/// Column x-offsets, left to right. The verdict badge is placed from the right edge instead, so the
/// table stays readable when the window is resized.
const COL_NUM: usize = 12;
const COL_NAME: usize = 58;
const COL_RAW: usize = 200;
const COL_STD: usize = 386;
const BADGE_W: usize = 74;

struct Probe {
    report: Report,
    scroll: usize,
    /// Show only the rows that need action (FAIL / STUB / PAL-GAP).
    failures_only: bool,
    /// Two-step so the window can paint "re-running" before the run blocks the frame loop:
    /// 2 = requested, 1 = banner painted, run it now, 0 = idle.
    rerun: u8,
    height: usize,
}

impl Probe {
    fn new(report: Report) -> Probe {
        Probe { report, scroll: 0, failures_only: false, rerun: 0, height: 620 }
    }

    fn visible(&self) -> Vec<&report::Row> {
        self.report
            .rows
            .iter()
            .filter(|r| {
                !self.failures_only
                    || matches!(r.verdict, Verdict::Fail | Verdict::Stub | Verdict::PalGap)
            })
            .collect()
    }

    fn max_scroll(&self) -> usize {
        let content = self.visible().len() * ROW_H;
        let view = self.height.saturating_sub(HEADER_H);
        content.saturating_sub(view)
    }

    fn scroll_by(&mut self, delta: isize) -> bool {
        let max = self.max_scroll() as isize;
        let next = (self.scroll as isize + delta).clamp(0, max.max(0)) as usize;
        let moved = next != self.scroll;
        self.scroll = next;
        moved
    }
}

impl NyxApp for Probe {
    fn title(&self) -> &str {
        "POSIX Probe"
    }
    fn initial_width(&self) -> usize {
        820
    }
    fn initial_height(&self) -> usize {
        620
    }
    fn icon_path(&self) -> &str {
        "/mnt/nvme/apps/PosixProbe.nyx/icon.png"
    }

    fn update(&mut self) -> bool {
        match self.rerun {
            2 => {
                // One frame of "re-running" feedback before the probes block the loop.
                self.rerun = 1;
                true
            }
            1 => {
                self.report = probes::run_all();
                self.scroll = 0;
                self.rerun = 0;
                true
            }
            _ => false,
        }
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        self.height = canvas.height;
        let w = canvas.width;
        canvas.fill_rect(0, 0, w, canvas.height, BG);

        // ── header ──────────────────────────────────────────────────────────────────────────
        canvas.fill_rect(0, 0, w, HEADER_H, SURFACE);
        canvas.fill_rect(0, HEADER_H - 1, w, 1, BORDER);

        let counts = [
            (Verdict::Pass, self.report.count(Verdict::Pass)),
            (Verdict::Fail, self.report.count(Verdict::Fail)),
            (Verdict::Stub, self.report.count(Verdict::Stub)),
            (Verdict::Missing, self.report.count(Verdict::Missing)),
            (Verdict::PalGap, self.report.count(Verdict::PalGap)),
            (Verdict::Skip, self.report.count(Verdict::Skip)),
        ];
        let mut x = PAD;
        for (v, n) in counts {
            let label = format!("{} {}", v.label(), n as u32);
            let tw = Canvas::text_width(&label, 1) + 16;
            canvas.fill_rect(x, 10, tw, 22, v.color());
            canvas.print_str(x + 8, 13, &label, 0xFF_FFFFFF, 1);
            x += tw + 8;
        }

        // The baseline belongs in the header, not a log line: every MISSING verdict below is only
        // meaningful relative to it, and it also says which kernel you are looking at (-22 EINVAL
        // today, -38 ENOSYS once the dispatcher's default arm is fixed).
        let hint = format!(
            "unknown syscall returns {}   ·   j/k scroll  ·  f {}  ·  r re-run",
            report::render_ret(self.report.baseline.0),
            if self.failures_only { "all rows" } else { "needs-work only" },
        );
        canvas.print_str(PAD, 38, &hint, MUTED, 1);

        if self.rerun > 0 {
            canvas.fill_rect(0, HEADER_H, w, canvas.height - HEADER_H, BG);
            canvas.print_str(PAD, HEADER_H + 20, "Re-running probes...", TEXT, 1);
            return;
        }

        // ── rows ────────────────────────────────────────────────────────────────────────────
        let rows = self.visible();
        let badge_x = w.saturating_sub(PAD + BADGE_W);
        let first = self.scroll / ROW_H;
        let mut y = HEADER_H + (first * ROW_H).saturating_sub(self.scroll);

        for row in rows.iter().skip(first) {
            if y >= canvas.height {
                break;
            }
            // Zebra striping — a 4-column table of near-identical strings is unreadable without it.
            if (first + (y - HEADER_H) / ROW_H) % 2 == 1 {
                canvas.fill_rect(0, y, w, ROW_H, SURFACE);
            }

            if row.num > 0 {
                canvas.print_str(COL_NUM, y + 3, &format!("{}", row.num), MUTED, 1);
            }
            clipped(canvas, COL_NAME, y + 3, &row.name, COL_RAW - COL_NAME - 8, TEXT);
            clipped(canvas, COL_RAW, y + 3, &row.raw, COL_STD - COL_RAW - 8, TEXT);
            clipped(canvas, COL_STD, y + 3, &row.std_api, badge_x.saturating_sub(COL_STD + 8), MUTED);

            let v = row.verdict;
            canvas.fill_rect(badge_x, y + 3, BADGE_W, 18, v.color());
            let lw = Canvas::text_width(v.label(), 1);
            canvas.print_str(badge_x + (BADGE_W.saturating_sub(lw)) / 2, y + 4, v.label(), 0xFF_FFFFFF, 1);

            if !row.note.is_empty() {
                clipped(canvas, COL_NAME, y + 18, &row.note, w.saturating_sub(COL_NAME + PAD), MUTED);
            }
            y += ROW_H;
        }

        if rows.is_empty() {
            canvas.print_str(PAD, HEADER_H + 20, "Nothing needs work in this filter.", MUTED, 1);
        }
    }

    fn on_key(&mut self, key: char) -> bool {
        match key {
            'j' | keys::DOWN => self.scroll_by(ROW_H as isize),
            'k' | keys::UP => self.scroll_by(-(ROW_H as isize)),
            keys::PAGE_DOWN => self.scroll_by((self.height.saturating_sub(HEADER_H)) as isize),
            keys::PAGE_UP => self.scroll_by(-((self.height.saturating_sub(HEADER_H)) as isize)),
            keys::HOME => self.scroll_by(isize::MIN / 2),
            keys::END => self.scroll_by(isize::MAX / 2),
            'f' => {
                self.failures_only = !self.failures_only;
                self.scroll = 0;
                true
            }
            'r' => {
                self.rerun = 2;
                true
            }
            _ => false,
        }
    }

    fn content_height(&self) -> usize {
        self.visible().len() * ROW_H
    }
    fn scroll_offset(&self) -> usize {
        self.scroll
    }
    fn on_scroll(&mut self, new_offset: usize) -> bool {
        let clamped = new_offset.min(self.max_scroll());
        let moved = clamped != self.scroll;
        self.scroll = clamped;
        moved
    }
}

/// Draw `text` truncated to `max_w` pixels.
///
/// `Canvas::print_str` wraps at the window edge, which in a table turns one long note into a line
/// that overwrites the row below it. Measuring and cutting here keeps every row exactly one row tall.
fn clipped(canvas: &mut Canvas, x: usize, y: usize, text: &str, max_w: usize, color: u32) {
    if text.is_empty() || text == "-" {
        return;
    }
    if Canvas::text_width(text, 1) <= max_w {
        canvas.print_str(x, y, text, color, 1);
        return;
    }
    let mut cut = String::new();
    let ell = Canvas::text_width("..", 1);
    for c in text.chars() {
        let mut probe = cut.clone();
        probe.push(c);
        if Canvas::text_width(&probe, 1) + ell > max_w {
            break;
        }
        cut = probe;
    }
    cut.push_str("..");
    canvas.print_str(x, y, &cut, color, 1);
}

fn main() {
    // Probes run HERE, before the window exists. Two reasons, both learned the hard way on this
    // kernel: a probe that hangs must not hang inside the 16 ms frame loop (the compositor would
    // stop servicing everything else), and the serial log has to be complete even if the window
    // never opens — which is exactly the case where the log is the only evidence available.
    println!("posix: PosixProbe starting");
    let report = probes::run_all();
    nyx_gui::app::run(Probe::new(report));
}
