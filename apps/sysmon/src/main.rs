//! # System Monitor — Meridian build-order step 12
//!
//! The design's lede is the specification: *"Four numbers, set large and light, with room around
//! them. No dashboard, no grid of cards, no permanent graphs. A sparkline appears on the one metric
//! under the pointer and nowhere else; process detail exists but has to be asked for. The window is
//! the argument that a monitoring tool does not have to look like an administration console."*
//!
//! What that deletes from the old app is nearly all of it: the 150px tab rail down the left, the
//! three named views (Entity Vitals / Task Scheduler / Kernel Bootlog), the four coloured progress
//! bars, and the raised panel behind the log. Those were the administration console. What replaces
//! them is one 2x2 grid of `.mc` cells and a `Processes` row that expands when asked.
//!
//! ## The four numbers had to be earned first
//!
//! Three of the four did not exist as data. This app is the one surface whose entire job is
//! reporting the machine's state, so inventing any of it would be the worst possible place to do it
//! — hence `SYS_SYS_METRICS` (567) and the two producers behind it:
//!
//! - **Processor** — `thermal.rs` already computed a busy percentage with the "sum EVERY idle task"
//!   fix in it, but only *inside* the log printer, which runs at `temp >= 70` or once a minute. The
//!   arithmetic is now split out and published every cycle. One producer, no second implementation.
//! - **Memory** — the frame allocator had no accounting at all. It now keeps two atomics.
//!   Deliberately atomics: `MEMORY_MANAGER` is the bare spin lock the page-fault handler takes, and
//!   an app polling it once a second through a syscall at IF=0 would be a standing deadlock
//!   invitation.
//! - **Thermal** — `SystemInfo::current_temp`, which was always honest.
//!
//! ## The one metric the design names that this does not draw
//!
//! **Graphics, "31%".** There is no GPU utilisation counter anywhere in this system to read. The
//! Intel driver is pure BLT on a legacy ring; it reads no performance counters, and Gen9's are not
//! set up. The only GPU state the kernel actually knows is whether forcewake is held — a boolean,
//! and not a percentage. A plausible-looking number in the one window whose entire purpose is
//! reporting true numbers is the worst lie available here.
//!
//! **Storage** takes the slot instead, because it is a real measurement (`sys_statfs`, the same call
//! Files uses for the nvme0 capacity) and it has exactly the shape the design's `.mc` wants: a
//! percentage, and a "x of y" sub-line. The card is a substitution, not an invention.
//!
//! ## The one thing the design has no room for, kept anyway
//!
//! The old app's third tab was **Kernel Bootlog**, and `grep` says this binary was the *only*
//! consumer of `sys_get_boot_logs` in the entire tree. **This laptop has no serial console** —
//! `serial_println!` output is written and never read — so that view was one of the few ways to see
//! what the kernel said. Deleting it along with the tab rail would have quietly removed a
//! diagnostic channel to make a window prettier.
//!
//! It comes back as a second disclosure row beside Processes, in the design's own idiom rather than
//! as a tab: detail that exists but has to be asked for. That is also why this app calls
//! `register_all()` — it is the one window that needs `d1` for the numbers *and* `mono` for a log,
//! which between them is every face.
//!
//! ## Where the geometry lives
//!
//! `nyx_meridian::panes` — `metric_cell` / `metric_hit` / `spark_bar`. Same argument as Files: this
//! is a `no_std` binary whose tests cannot run on the host, on a machine with no QEMU, so a
//! hit-test that disagrees with a draw by four pixels would be found by booting and squinting.

#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
use nyx_meridian::layout;
use nyx_meridian::panes::{self, SPARK_BARS};
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B1, B3, D1, MONO};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// The shell's theme, kept in step by `MSG_THEME_CHANGED` (see `NyxApp::on_theme` below). Same
/// static-plus-atomic shape as Files, and for the same reason: every draw helper here reads it.
static DARK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

fn theme() -> Theme {
    if DARK.load(core::sync::atomic::Ordering::Relaxed) { Theme::dark() } else { Theme::light() }
}

/// Which of the four cards. The order is the design's reading order, left to right, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Processor,
    Memory,
    Storage,
    Thermal,
}

const METRICS: [Metric; 4] = [Metric::Processor, Metric::Memory, Metric::Storage, Metric::Thermal];

impl Metric {
    fn label(self) -> &'static str {
        match self {
            Metric::Processor => "Processor",
            Metric::Memory => "Memory",
            Metric::Storage => "Storage",
            Metric::Thermal => "Thermal",
        }
    }
}

// `.mg` sits directly under the surface top; the Processes row is `padding: 26px 34px`.
const PROC_PAD_X: i32 = 34;
const PROC_PAD_Y: i32 = 26;
/// A process row in the expanded list. Not an `.fr` — those are 44px with an icon, and a process has
/// no icon. This is the design's plain row-on-a-hairline at `.fr`'s rhythm minus the artwork.
const ROW_H: i32 = 30;
/// A kernel-log line. `MONO`'s own leading; a log is dense by nature and the `.fr` rhythm would
/// stretch a 400-line boot into something nobody scrolls through.
const LOG_H: i32 = 18;

/// Which detail section is open. One at a time: "process detail exists but has to be asked for", and
/// asking for one thing should not also open the other.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Detail {
    None,
    Processes,
    Log,
}

struct Monitor {
    width: i32,
    height: i32,

    // Live data, refreshed on a timer rather than per frame.
    metrics: SysMetrics,
    info: SystemInfo,
    disk: Option<StatFs>,
    have_metrics: bool,
    last_poll: usize,

    /// A ring of recent samples per metric, newest first — index 0 is the most recent. The design
    /// says the sparkline appears on "the one metric under the pointer", not on the processor, so
    /// every card keeps its own history. Four series of eighteen floats is 288 bytes; keeping only
    /// the CPU's would make three of the four cards silently inert on hover, which reads as a bug.
    history: [[f32; SPARK_BARS]; METRICS.len()],
    samples: usize,

    /// The card under the pointer, or None. The sparkline appears here and nowhere else.
    hover: Option<usize>,
    /// "process detail exists but has to be asked for."
    detail: Detail,
    scroll: i32,

    /// The kernel log. Only read while the section is open — it is up to 128 KB and re-splitting it
    /// once a second behind a closed disclosure would be work nobody asked for.
    log_buf: Vec<u8>,
    log_lines: Vec<String>,
    log_len: usize,
}

impl Monitor {
    fn new() -> Self {
        Self {
            width: 640,
            height: 480,
            metrics: SysMetrics::default(),
            info: unsafe { core::mem::zeroed() },
            disk: None,
            have_metrics: false,
            last_poll: 0,
            history: [[0.0; SPARK_BARS]; METRICS.len()],
            samples: 0,
            hover: None,
            detail: Detail::None,
            scroll: 0,
            log_buf: Vec::new(),
            log_lines: Vec::new(),
            log_len: 0,
        }
    }

    /// The value and unit for a card: the two runs that share a baseline in `.mc .v`.
    ///
    /// Returns None when the number is not known. A card with no number draws a dash rather than a
    /// zero — "0%" and "not measured" are different statements and only one of them is true.
    fn value(&self, m: Metric) -> Option<(String, &'static str)> {
        match m {
            Metric::Processor => self
                .have_metrics
                .then(|| (alloc::format!("{}", self.metrics.cpu_pct.min(100)), "%")),
            Metric::Memory => {
                let t = self.metrics.mem_total_bytes;
                (self.have_metrics && t > 0)
                    .then(|| (alloc::format!("{}", self.metrics.mem_used_bytes * 100 / t), "%"))
            }
            Metric::Storage => self.disk.as_ref().and_then(|d| {
                (d.total_bytes > 0).then(|| {
                    let used = d.total_bytes.saturating_sub(d.free_bytes);
                    (alloc::format!("{}", used * 100 / d.total_bytes), "%")
                })
            }),
            // A zero temperature is the "no reading" value here, not a cold CPU — the MSR read
            // returns 0 when it fails, and this laptop idles near 50 C.
            Metric::Thermal => (self.info.current_temp > 0)
                .then(|| (alloc::format!("{}", self.info.current_temp), "\u{00B0}C")),
        }
    }

    /// The `.mc .sub` line: the measurement behind the percentage.
    fn sub(&self, m: Metric) -> String {
        match m {
            Metric::Processor => {
                let cores = self.info.task_count;
                if cores == 0 {
                    String::new()
                } else {
                    alloc::format!("{} kernel tasks", cores)
                }
            }
            Metric::Memory => {
                if !self.have_metrics || self.metrics.mem_total_bytes == 0 {
                    String::new()
                } else {
                    alloc::format!(
                        "{} of {}",
                        human_size(self.metrics.mem_used_bytes),
                        human_size(self.metrics.mem_total_bytes)
                    )
                }
            }
            Metric::Storage => match &self.disk {
                Some(d) if d.total_bytes > 0 => alloc::format!(
                    "{} of {}",
                    human_size(d.total_bytes.saturating_sub(d.free_bytes)),
                    human_size(d.total_bytes)
                ),
                _ => String::new(),
            },
            // The thresholds are the governor's own, not invented for the display: it throttles at
            // 76 and halts the machine at 95 (thermal.rs). Saying "within envelope" at 80 C would be
            // this window disagreeing with the kernel about the machine it is describing.
            Metric::Thermal => {
                let t = self.info.current_temp;
                if t == 0 {
                    String::new()
                } else if t >= 95 {
                    String::from("Critical \u{00B7} emergency halt")
                } else if t >= 76 {
                    String::from("Throttling")
                } else {
                    String::from("Within envelope")
                }
            }
        }
    }

    /// How many processes the expanded list has rows for.
    fn proc_count(&self) -> usize {
        (self.info.task_count as usize).min(self.info.tasks.len())
    }

    fn grid_h(&self) -> i32 {
        panes::metric_grid_height(METRICS.len())
    }

    /// The height of a disclosure row (`Processes`, `Kernel log`).
    fn disclosure_h(&self) -> i32 {
        sc_line() + sc(PROC_PAD_Y) * 2
    }

    /// Where the two disclosure rows sit and where their lists start, walked top to bottom because
    /// the second row's position depends on whether the first is open. Returns
    /// `(processes_row, log_row, content_bottom)`.
    ///
    /// One walk that draw, hit-test and `content_height` all call, for the reason `panes` exists at
    /// all: an accordion whose click target and whose plate are computed separately drifts the
    /// moment one of them is edited.
    fn sections(&self) -> (layout::Rect, layout::Rect, i32) {
        let dh = self.disclosure_h();
        let mut y = self.grid_h();

        let proc_row = layout::Rect::new(0, y, self.width, dh);
        y += dh;
        if self.detail == Detail::Processes {
            y += self.proc_count() as i32 * sc(ROW_H);
        }

        let log_row = layout::Rect::new(0, y, self.width, dh);
        y += dh;
        if self.detail == Detail::Log {
            y += self.log_lines.len() as i32 * sc(LOG_H);
        }

        (proc_row, log_row, y)
    }

    /// Everything this window has to show, in pixels. See `panes::reported_content_height` for why
    /// this must be measured from the top of the surface and not from the top of a list.
    fn content_h(&self) -> i32 {
        self.sections().2
    }

    /// Read and split the kernel log, but only when it has actually changed. `sys_get_boot_logs`
    /// copies up to 128 KB and the split allocates a `String` per line; doing that once a second
    /// for a log that appends a line a minute is the kind of idle work that shows up as heat.
    fn refresh_log(&mut self) {
        if self.log_buf.is_empty() {
            self.log_buf = alloc::vec![0u8; 131072];
        }
        let len = sys_get_boot_logs(&mut self.log_buf);
        if len == self.log_len && !self.log_lines.is_empty() {
            return;
        }
        self.log_len = len;
        self.log_lines.clear();
        if let Ok(text) = core::str::from_utf8(&self.log_buf[..len]) {
            for line in text.split('\n') {
                if !line.trim().is_empty() {
                    self.log_lines.push(String::from(line));
                }
            }
        }
    }

    fn max_scroll(&self) -> i32 {
        (self.content_h() - self.height).max(0)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0, self.max_scroll());
    }

    fn poll(&mut self) {
        if let Some(m) = sys_get_metrics() {
            self.metrics = m;
            self.have_metrics = true;
        }
        sys_get_system_info(&mut self.info);
        self.disk = sys_statfs();

        // Push a sample per metric onto the front of its ring. A metric with no reading this tick
        // carries its previous value forward rather than recording a zero: a failed read is not a
        // measurement of nothing, and a zero would draw a dip in the sparkline that never happened.
        let fresh: Vec<Option<f32>> = METRICS.iter().map(|&m| self.fraction(m)).collect();
        let mut advanced = false;
        for (i, v) in fresh.iter().enumerate() {
            let carry = self.history[i][0];
            self.history[i].rotate_right(1);
            self.history[i][0] = v.unwrap_or(carry);
            advanced = true;
        }
        if advanced {
            self.samples = (self.samples + 1).min(SPARK_BARS);
        }
    }

    /// A metric as a 0.0..=1.0 fraction, for the sparkline. `None` when there is no reading.
    fn fraction(&self, m: Metric) -> Option<f32> {
        let f = match m {
            Metric::Processor => {
                if !self.have_metrics {
                    return None;
                }
                self.metrics.cpu_pct.min(100) as f32 / 100.0
            }
            Metric::Memory => {
                let t = self.metrics.mem_total_bytes;
                if !self.have_metrics || t == 0 {
                    return None;
                }
                self.metrics.mem_used_bytes as f32 / t as f32
            }
            Metric::Storage => {
                let d = self.disk.as_ref()?;
                if d.total_bytes == 0 {
                    return None;
                }
                d.total_bytes.saturating_sub(d.free_bytes) as f32 / d.total_bytes as f32
            }
            // Scaled against the governor's own trip point, not against 100 C. The sparkline is a
            // shape, and "how close is this to the temperature that halts the machine" is the only
            // scale for it that means anything here.
            Metric::Thermal => {
                if self.info.current_temp == 0 {
                    return None;
                }
                self.info.current_temp as f32 / 95.0
            }
        };
        Some(f.clamp(0.0, 1.0))
    }

    // ── drawing ──────────────────────────────────────────────────────────────────────────────────

    fn draw_card(&self, canvas: &mut Canvas, t: &Theme, i: usize, m: Metric, sy: i32) {
        let Some(c) = panes::metric_cell(self.width, METRICS.len(), i) else { return };

        // `.mc { border-right, border-bottom }`. Hairlines only — the cells share one ground, and a
        // filled panel per metric is the card grid the design explicitly rejects.
        if c.rule_right {
            fill(canvas, c.rect.right() - 1, c.rect.y - sy, 1, c.rect.h, t.line);
        }
        fill(canvas, c.rect.x, c.rect.bottom() - 1 - sy, c.rect.w, 1, t.line);

        text::draw(canvas, c.label.x, c.label.y - sy, B3, t.fg_3, m.label());

        // `.mc .v { display: flex; align-items: baseline; gap: 6px }` — a 40px number and a 15px
        // unit on ONE baseline. See `text::baseline_with`: two runs drawn at the same `y` do not
        // share a baseline, because each is placed from its own face's ascent.
        match self.value(m) {
            Some((v, unit)) => {
                let top = c.value.y - sy;
                let w = text::draw(canvas, c.value.x, top, D1, t.fg, &v);
                let uy = text::baseline_with(B1, D1, top);
                text::draw(canvas, c.value.x + w + sc(6), uy, B1, t.fg_3, unit);
            }
            // An em dash at the number's own size, so a card with no reading still holds its place
            // in the grid instead of collapsing.
            None => {
                text::draw(canvas, c.value.x, c.value.y - sy, D1, t.fg_4, "\u{2014}");
            }
        }

        // The one graph in the app, and only on the metric under the pointer.
        if self.hover == Some(i) && self.fraction(m).is_some() {
            self.draw_spark(canvas, t, c.detail, i, sy);
        } else {
            let s = self.sub(m);
            if !s.is_empty() {
                // The sub-line sits on the text baseline of the detail box, not centred in it: the
                // box is 26px tall to hold a sparkline, and centring a 12px line in it would drop
                // the sub-lines 7px below where the design puts them.
                text::draw(canvas, c.detail.x, c.detail.y - sy, B3, t.fg_4, &s);
            }
        }
    }

    fn draw_spark(&self, canvas: &mut Canvas, t: &Theme, detail: layout::Rect, card: usize, sy: i32) {
        for i in 0..self.samples {
            let Some(b) = panes::spark_bar(detail, i, self.history[card][i]) else { continue };
            // `.spark i.hi { background: var(--accent) }` — the newest sample is the accent, the
            // history is `fg-4`. One accent mark, which is the design's budget for the whole card.
            let col = if i == 0 { t.accent } else { t.fg_4 };
            fill(canvas, b.x, b.y - sy, b.w, b.h, col);
        }
    }

    /// One disclosure row: a label, a count, and the chevron that is the only thing saying the row
    /// can be asked for more.
    fn draw_disclosure(&self, canvas: &mut Canvas, t: &Theme, r: layout::Rect, sy: i32,
                       label: &str, count: &str, open: bool) {
        let top = r.y - sy;
        if top >= self.height || top + r.h < 0 {
            return;
        }
        let y = text::centre_y(top, r.h, B2_LIKE);
        text::draw(canvas, sc(PROC_PAD_X), y, B2_LIKE, t.fg_3, label);

        let chev_x = self.width - sc(PROC_PAD_X) - sc(14);
        text::draw_right(canvas, chev_x - sc(12), y, B3, t.fg_4, count);

        // The set has no up chevron, so the open state mirrors the down one — see
        // `text::icon_flipped` for why that is better than reusing the right-pointing `u-chev`.
        let iy = top + (r.h - sc(14)) / 2;
        text::icon_flipped(canvas, chev_x, iy, "u-chev-d", sc(14) as usize, t.fg_4, open);
    }

    fn draw_processes(&self, canvas: &mut Canvas, t: &Theme, from: i32, sy: i32) {
        // The task array is what `SystemInfo` carries; it is already capped at 64 by the kernel.
        let mut ry = from;
        for i in 0..self.proc_count() {
            let task = &self.info.tasks[i];
            let top = ry - sy;
            ry += sc(ROW_H);
            // Cheap vertical cull. The list is short, but a row drawn far off screen still walks
            // every glyph, and the log below can be four hundred lines.
            if top + sc(ROW_H) < 0 || top >= self.height {
                continue;
            }
            fill(canvas, sc(PROC_PAD_X), top, self.width - sc(PROC_PAD_X) * 2, 1, t.line);
            let ty = text::centre_y(top, sc(ROW_H), B1);
            let name = core::str::from_utf8(&task.name)
                .unwrap_or("")
                .trim_matches(char::from(0));
            let name = if name.is_empty() { "\u{2014}" } else { name };
            text::draw(canvas, sc(PROC_PAD_X), ty, B1, t.fg_2, name);
            text::draw_right(
                canvas,
                self.width - sc(PROC_PAD_X) - layout::scrollbar_reserve(),
                ty,
                B3,
                t.fg_4,
                &alloc::format!("pid {}", task.pid),
            );
        }
    }

    fn draw_log(&self, canvas: &mut Canvas, t: &Theme, from: i32, sy: i32) {
        let mut ry = from;
        for line in self.log_lines.iter() {
            let top = ry - sy;
            ry += sc(LOG_H);
            if top + sc(LOG_H) < 0 || top >= self.height {
                continue;
            }
            // Mono, because a kernel log is columnar output — `[Thermal] 72C | cpu 4% busy` only
            // reads as a table if the digits line up. No plate behind it: the design has no sunken
            // console panel, and a log on the same ground as everything else is the point.
            text::draw(canvas, sc(PROC_PAD_X), top, MONO, t.fg_2, line);
        }
    }
}

/// `.mc .k` and the Processes row share 12px, but the row's label is `13px/var(--fg-3)` in the
/// design's inline style. One named constant so the two cannot drift.
const B2_LIKE: nyx_meridian::tokens::Style = nyx_meridian::tokens::B2;

fn sc(v: i32) -> i32 {
    nyx_meridian::scale::px(v)
}

fn sc_line() -> i32 {
    text::line_box(B2_LIKE)
}

/// `Canvas::fill_rect` takes usize and has no notion of a negative origin, so every fill in this
/// file goes through here. A scrolled row has a negative `y` on the way out of view, and passing
/// that in as `usize` wraps it to an enormous coordinate — a fill that silently paints nothing, or
/// worse, paints somewhere else.
fn fill(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(cw);
    let y1 = (y + h).min(ch);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    canvas.fill_rect(x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, color);
}

fn human_size(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if b < KB {
        alloc::format!("{} B", b)
    } else if b < MB {
        alloc::format!("{}.{} KB", b / KB, (b % KB) * 10 / KB)
    } else if b < GB {
        alloc::format!("{}.{} MB", b / MB, (b % MB) * 10 / MB)
    } else {
        alloc::format!("{}.{} GB", b / GB, (b % GB) * 10 / GB)
    }
}

impl NyxApp for Monitor {
    fn icon_path(&self) -> &str {
        "/mnt/nvme/apps/SystemMonitor.nyx/icon.png"
    }
    fn title(&self) -> &str {
        "System Monitor"
    }
    /// The design's own window: 640 wide, quantised to 16 like every width in the system.
    fn initial_width(&self) -> usize {
        640
    }
    fn initial_height(&self) -> usize {
        480
    }

    fn update(&mut self) -> bool {
        let now = sys_get_time();
        // 1 Hz, matching the producer. `CPU_BUSY_PCT` is republished once a second by the thermal
        // governor, so polling faster would redraw the window to show the same number.
        if now.wrapping_sub(self.last_poll) < 1000 {
            return false;
        }
        self.last_poll = now;
        self.poll();
        // The log grows while it is open — the thermal governor prints into it once a minute, and
        // far more often when the machine is in trouble, which is exactly when someone is watching.
        if self.detail == Detail::Log {
            self.refresh_log();
        }
        true
    }

    fn content_height(&self) -> usize {
        let h = self.content_h();
        if h > self.height { h as usize } else { 0 }
    }
    fn scroll_offset(&self) -> usize {
        self.scroll.max(0) as usize
    }
    fn on_scroll(&mut self, off: usize) -> bool {
        let new = (off as i32).clamp(0, self.max_scroll());
        if new != self.scroll {
            self.scroll = new;
            true
        } else {
            false
        }
    }

    fn on_theme(&mut self, dark: bool) -> bool {
        use core::sync::atomic::Ordering;
        DARK.swap(dark, Ordering::Relaxed) != dark
    }

    fn on_mouse_move(&mut self, mx: usize, my: usize) -> bool {
        // The grid does not scroll out from under the pointer independently of the rest of the
        // surface, so the hit-test is in content space.
        let new = panes::metric_hit(self.width, METRICS.len(), mx as i32, my as i32 + self.scroll);
        if new != self.hover {
            self.hover = new;
            return true;
        }
        false
    }

    fn on_mouse_leave(&mut self) -> bool {
        if self.hover.is_some() {
            self.hover = None;
            return true;
        }
        false
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        let (proc_row, log_row, _) = self.sections();
        let (x, y) = (mx as i32, my as i32 + self.scroll);

        let want = if proc_row.contains(x, y) {
            Some(Detail::Processes)
        } else if log_row.contains(x, y) {
            Some(Detail::Log)
        } else {
            None
        };
        let Some(want) = want else { return false };

        // Clicking the open section closes it; clicking the other switches.
        self.detail = if self.detail == want { Detail::None } else { want };
        if self.detail == Detail::Log {
            self.refresh_log();
        }
        self.clamp_scroll();
        true
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        self.width = canvas.width as i32;
        self.height = canvas.height as i32;
        self.clamp_scroll();
        let t = theme();
        let sy = self.scroll;

        // One ground for the whole window. The buffer is never cleared between frames, so this fill
        // is also what erases the last one.
        canvas.fill_rect(0, 0, self.width as usize, self.height as usize, t.surface);

        for (i, m) in METRICS.iter().enumerate() {
            self.draw_card(canvas, &t, i, *m, sy);
        }

        let (proc_row, log_row, _) = self.sections();
        let procs = self.proc_count();
        self.draw_disclosure(canvas, &t, proc_row, sy, "Processes",
            &alloc::format!("{} running", procs), self.detail == Detail::Processes);
        if self.detail == Detail::Processes {
            self.draw_processes(canvas, &t, proc_row.bottom(), sy);
        }

        // The count is only known once the log has been read, and it is only read when asked for —
        // so before the first open this says "kernel log" and nothing more. Better than a zero,
        // which would read as an empty log rather than an unread one.
        let lines = if self.log_lines.is_empty() {
            String::new()
        } else {
            alloc::format!("{} lines", self.log_lines.len())
        };
        self.draw_disclosure(canvas, &t, log_row, sy, "Kernel log", &lines,
            self.detail == Detail::Log);
        if self.detail == Detail::Log {
            self.draw_log(canvas, &t, log_row.bottom(), sy);
        }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // ★ 2026-09-08: the typefaces are no longer `include_bytes!`d into this binary — they are
    // read out of the shared bundle at /mnt/nvme/fonts/ (see `nyx_meridian::font`). That took
    // 1.93 MB off the boot image and put it on THIS HEAP instead, so the heap had to grow to
    // hold it. A heap too small to take the fonts is not a crash: the read fails, every face
    // falls back to DejaVu, and the window renders in the wrong typeface at the right size.
    let heap_start = sys_alloc_pages(1024);
    if heap_start == 0 {
        sys_exit(1);
    }
    unsafe {
        ALLOCATOR.lock().init(heap_start as *mut u8, 1024 * 4096);
    }

    // This is the one window that needs every face: `d1` (ExtraLight) for the four large numbers,
    // `mono` for the kernel log, and the usual three for everything else. `register_text()` +
    // `register_display()` + `register_mono()` would install exactly the same five statics at
    // exactly the same cost, so the honest call is the single one.
    nyx_meridian::font::register_all();

    nyx_gui::app::run(Monitor::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(111);
}
