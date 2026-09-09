//! Render the System Monitor interior to a PNG on the development host, without booting.
//!
//! ```text
//! cargo run --release -p nyx-meridian --example monitor_preview -- /tmp/mon.png
//!     [--light] [--size 640x480] [--hover 0] [--expand] [--scroll 0]
//! ```
//!
//! Same argument as `files_preview`, and it earned its keep there twice: the `panes` unit tests
//! prove the boxes do not overlap, not that the result looks like the design. A number sitting on
//! the wrong baseline, a unit hanging above its digits, or a sparkline drawn upside down are all
//! valid frames of the wrong picture.
//!
//! `--hover N` is the one that matters most here. The sparkline only exists under the pointer, so it
//! is the part of this window least likely to be looked at on hardware and most likely to be wrong.

#[path = "common/png.rs"]
mod png;
use png::write_png;

use nyx_gui::canvas::Canvas;
use nyx_meridian::panes::{self, SPARK_BARS};
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B1, B2, B3, D1, MONO};

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}
fn flag(name: &str) -> bool {
    std::env::args().any(|x| x == name)
}

/// A metric, exactly as the app models one: label, value, unit, sub-line.
struct Card(&'static str, Option<&'static str>, &'static str, &'static str);

/// Plausible readings for this laptop — 16 GB installed, a 512 GB NVMe, idling warm. These are the
/// shapes the real numbers take (two digits, a "x of y" sub-line), which is what the layout is being
/// checked against.
const CARDS: [Card; 4] = [
    Card("Processor", Some("3"), "%", "34 kernel tasks"),
    Card("Memory", Some("42"), "%", "6.7 GB of 15.6 GB"),
    Card("Storage", Some("18"), "%", "84.2 GB of 476.9 GB"),
    Card("Thermal", Some("52"), "\u{00B0}C", "Within envelope"),
];

/// The Processor history the app keeps, newest first.
const HISTORY: [f32; SPARK_BARS] = [
    0.36, 0.48, 0.35, 0.22, 0.29, 0.40, 0.58, 0.44, 0.31, 0.26, 0.34, 0.46, 0.20, 0.28, 0.38, 0.24,
    0.30, 0.22,
];

const PROC_PAD_X: i32 = 34;
const PROC_PAD_Y: i32 = 26;
const ROW_H: i32 = 30;
const LOG_H: i32 = 18;

/// Real lines from this kernel's boot log — the columnar shape is the thing being checked, since
/// it is why the log is set in mono rather than in the body face.
const LOG: [&str; 6] = [
    "[Thermal] NyxOS Mobile Thermal Governor Online.",
    "[Thermal] 52C | cpu 3% busy",
    "[ACPI]    DSDT + 12 SSDTs loaded",
    "[WiFi]    9462/JF associated, DHCP 192.168.1.42",
    "[GPU]     BLT ring armed, RC6 park after 1000ms idle",
    "[Shell]   WindowServer = Meridian",
];

const PROCS: [(&str, u64); 8] = [
    ("shell", 4),
    ("Explorer", 11),
    ("SystemMonitor", 14),
    ("Terminal", 9),
    ("idle", 2),
    ("net", 6),
    ("thermal", 3),
    ("init", 1),
];

#[allow(non_snake_case)]
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "monitor.png".into());
    let t = if flag("--light") { Theme::light() } else { Theme::dark() };
    let (W, H) = match arg("--size").and_then(|s| {
        let (a, b) = s.split_once('x')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }) {
        Some(v) => v,
        None => (640usize, 480usize),
    };
    let hover: Option<usize> = arg("--hover").and_then(|s| s.parse().ok());
    let expand = flag("--expand");
    let sy: i32 = arg("--scroll").and_then(|s| s.parse().ok()).unwrap_or(0);

    // The same call the app makes — this window needs every face (see `apps/sysmon`).
    nyx_meridian::font::register_all();

    let mut buf = vec![0u32; W * H];
    let mut canvas = Canvas::new(&mut buf, W, H);
    let w = W as i32;
    canvas.fill_rect(0, 0, W, H, t.surface);

    // ── the metric grid ──
    for (i, card) in CARDS.iter().enumerate() {
        let Some(c) = panes::metric_cell(w, CARDS.len(), i) else { continue };

        if c.rule_right {
            fill(&mut canvas, c.rect.right() - 1, c.rect.y - sy, 1, c.rect.h, t.line);
        }
        fill(&mut canvas, c.rect.x, c.rect.bottom() - 1 - sy, c.rect.w, 1, t.line);

        text::draw(&mut canvas, c.label.x, c.label.y - sy, B3, t.fg_3, card.0);

        match card.1 {
            Some(v) => {
                let top = c.value.y - sy;
                let vw = text::draw(&mut canvas, c.value.x, top, D1, t.fg, v);
                let x = c.value.x + vw + nyx_meridian::scale::px(6);
                text::draw(&mut canvas, x, text::baseline_with(B1, D1, top), B1, t.fg_3, card.2);
            }
            None => {
                text::draw(&mut canvas, c.value.x, c.value.y - sy, D1, t.fg_4, "\u{2014}");
            }
        }

        // The sparkline replaces the sub-line, on the hovered card only.
        if hover == Some(i) {
            for b in 0..SPARK_BARS {
                let Some(r) = panes::spark_bar(c.detail, b, HISTORY[b]) else { continue };
                let col = if b == 0 { t.accent } else { t.fg_4 };
                fill(&mut canvas, r.x, r.y - sy, r.w, r.h, col);
            }
        } else {
            text::draw(&mut canvas, c.detail.x, c.detail.y - sy, B3, t.fg_4, card.3);
        }
    }

    // ── the two disclosure rows ──
    let grid_h = panes::metric_grid_height(CARDS.len());
    let row_h = text::line_box(B2) + px(PROC_PAD_Y) * 2;

    let disclosure = |canvas: &mut Canvas, y: i32, label: &str, count: &str, open: bool| {
        let ry = text::centre_y(y - sy, row_h, B2);
        text::draw(canvas, px(PROC_PAD_X), ry, B2, t.fg_3, label);
        let chev_x = w - px(PROC_PAD_X) - px(14);
        text::draw_right(canvas, chev_x - px(12), ry, B3, t.fg_4, count);
        let iy = y - sy + (row_h - px(14)) / 2;
        text::icon_flipped(canvas, chev_x, iy, "u-chev-d", px(14) as usize, t.fg_4, open);
    };

    let mut y = grid_h;
    disclosure(&mut canvas, y, "Processes", &format!("{} running", PROCS.len()), expand);
    y += row_h;

    if expand {
        for (name, pid) in PROCS.iter() {
            let top = y - sy;
            fill(&mut canvas, px(PROC_PAD_X), top, w - px(PROC_PAD_X) * 2, 1, t.line);
            let ty = text::centre_y(top, px(ROW_H), B1);
            text::draw(&mut canvas, px(PROC_PAD_X), ty, B1, t.fg_2, name);
            let right = w - px(PROC_PAD_X) - nyx_meridian::layout::scrollbar_reserve();
            text::draw_right(&mut canvas, right, ty, B3, t.fg_4, &format!("pid {}", pid));
            y += px(ROW_H);
        }
    }

    let show_log = flag("--log");
    let log_count = if show_log { format!("{} lines", LOG.len()) } else { String::new() };
    disclosure(&mut canvas, y, "Kernel log", &log_count, show_log);
    y += row_h;
    if show_log {
        for line in LOG.iter() {
            text::draw(&mut canvas, px(PROC_PAD_X), y - sy, MONO, t.fg_2, line);
            y += px(LOG_H);
        }
    }

    // ── the shell's scrollbar, so the metadata column can be checked against it ──
    let content_h = y;
    let surface = nyx_meridian::layout::Rect::new(0, 0, w, H as i32);
    let track = nyx_meridian::layout::scrollbar_track(surface);
    if let Some(thumb) = nyx_meridian::layout::scrollbar_thumb(track, content_h, H as i32, sy) {
        let radius = (track.w / 2).max(1) as usize;
        nyx_meridian::shapes::fill_round_rect(
            &mut canvas, track.x as usize, track.y as usize,
            track.w as usize, track.h as usize, radius, t.line,
        );
        nyx_meridian::shapes::fill_round_rect(
            &mut canvas, thumb.x.max(0) as usize, thumb.y.max(0) as usize,
            thumb.w as usize, thumb.h as usize, radius, t.fg_4,
        );
    }

    write_png(&out, &buf, W, H).expect("write png");
    println!(
        "wrote {} ({}x{}, {}, hover={:?}, expanded={}, scroll={}, content_h={})",
        out, W, H, if t.is_dark { "dark" } else { "light" }, hover, expand, sy, content_h
    );
}

fn px(v: i32) -> i32 {
    nyx_meridian::scale::px(v)
}

/// The app's clipping fill, mirrored — see `apps/sysmon`'s `fill` for why a negative origin cannot
/// be handed to `Canvas::fill_rect`.
fn fill(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(cw), (y + h).min(ch));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    canvas.fill_rect(x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, color);
}
