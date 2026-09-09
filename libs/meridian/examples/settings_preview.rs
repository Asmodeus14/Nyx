//! Render the Settings interior to a PNG on the development host, without booting.
//!
//! ```text
//! cargo run --release -p nyx-meridian --example settings_preview -- /tmp/set.png [--light] [--size 896x540]
//! ```
//!
//! Settings is the window the design shows on the **light** ramp, so `--light` is the one that
//! matches the document. Both are worth looking at: the claim being checked is that they are one
//! design with eight token values swapped, and the previews are where that is cheapest to falsify.

#[path = "common/png.rs"]
mod png;
use png::write_png;

use nyx_gui::canvas::Canvas;
use nyx_meridian::panes::{self, NavKind, RowKind};
use nyx_meridian::shapes;
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B1, B2, B3, D2, LB};

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}
fn flag(name: &str) -> bool {
    std::env::args().any(|x| x == name)
}
fn px(v: i32) -> i32 {
    nyx_meridian::scale::px(v)
}

const NAV_W: i32 = 198;
const MAIN_PAD_TOP: i32 = 30;
const MAIN_PAD_X: i32 = 36;
const HEAD_GAP_BELOW: i32 = 26;
const SUB_GAP_ABOVE: i32 = 5;

const NAV: [(&str, bool); 8] = [
    ("System", false),
    ("Appearance", true),
    ("Display", true),
    ("Network", true),
    ("Power", true),
    ("Machine", false),
    ("Storage", true),
    ("About", true),
];
/// Which nav entry is current (index into NAV).
const CURRENT: usize = 1;

/// A row's right-hand side, mirroring `apps/settings`' `Control`.
enum Ctl {
    Text(&'static str),
    Segments(&'static [&'static str], usize),
    Swatch(&'static str),
}

const ROWS: [(&str, &str, Ctl); 2] = [
    ("Mode", "Applies to the shell and every Nyx application",
     Ctl::Segments(&["Light", "Dark"], 1)),
    ("Accent", "Used for focus, selection and measurement only",
     Ctl::Swatch("#2B55C8")),
];

#[allow(non_snake_case)]
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "settings.png".into());
    let t = if flag("--light") { Theme::light() } else { Theme::dark() };
    let (W, H) = match arg("--size").and_then(|s| {
        let (a, b) = s.split_once('x')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }) {
        Some(v) => v,
        None => (896usize, 540usize),
    };

    nyx_meridian::font::register_text();

    let mut buf = vec![0u32; W * H];
    let mut canvas = Canvas::new(&mut buf, W, H);
    let w = W as i32;
    canvas.fill_rect(0, 0, W, H, t.surface);

    // ── the nav pane, the SAME component Files uses ──
    let kinds: Vec<NavKind> = NAV
        .iter()
        .map(|(_, item)| if *item { NavKind::Item } else { NavKind::Group })
        .collect();
    for (i, (label, item)) in NAV.iter().enumerate() {
        let Some(e) = panes::nav_entry(&kinds, i) else { continue };
        if !*item {
            text::draw(&mut canvas, e.x, e.y, LB, t.fg_4, label);
            continue;
        }
        let on = i == CURRENT;
        if on {
            fill(&mut canvas, 0, e.y, px(NAV_W), e.h, t.wash_2);
            let b = panes::nav_bar(e);
            fill(&mut canvas, b.x, b.y, b.w, b.h, t.accent);
        }
        let col = if on { t.fg } else { t.fg_2 };
        text::draw(&mut canvas, panes::nav_label_x(e), text::centre_y(e.y, e.h, B1), B1, col, label);
    }

    // ── the heading block ──
    let cx = px(NAV_W) + px(MAIN_PAD_X);
    let cw = w - cx - px(MAIN_PAD_X) - nyx_meridian::layout::scrollbar_reserve();
    text::draw(&mut canvas, cx, px(MAIN_PAD_TOP), D2, t.fg, "Appearance");
    let sy = px(MAIN_PAD_TOP) + text::line_box(D2) + px(SUB_GAP_ABOVE);
    text::draw(&mut canvas, cx, sy, B3, t.fg_3, "How Nyx renders surfaces, type and colour.");

    // ── the settings column ──
    let top = px(MAIN_PAD_TOP)
        + text::line_box(D2)
        + px(SUB_GAP_ABOVE)
        + text::line_box(B3)
        + px(HEAD_GAP_BELOW);

    let mut row_kinds = vec![RowKind::Heading];
    for (_, d, _) in ROWS.iter() {
        row_kinds.push(if d.is_empty() { RowKind::Row } else { RowKind::RowWithDesc });
    }

    if let Some(h) = panes::setting_row(&row_kinds, cw, cx, 0) {
        text::draw(&mut canvas, h.label.x, h.label.y + top, LB, t.fg_4, "Theme");
    }

    for (i, (label, desc, ctl)) in ROWS.iter().enumerate() {
        let Some(mut r) = panes::setting_row(&row_kinds, cw, cx, i + 1) else { continue };
        r.rect.y += top;
        r.label.y += top;
        r.desc.y += top;

        text::draw(&mut canvas, r.label.x, r.label.y, B1, t.fg, label);
        if !desc.is_empty() {
            text::draw(&mut canvas, r.desc.x, r.desc.y, B3, t.fg_3, desc);
        }
        if r.rule {
            fill(&mut canvas, r.rect.x, r.rect.bottom() - 1, r.rect.w, 1, t.line);
        }

        match ctl {
            Ctl::Text(s) => {
                let y = text::centre_y(r.rect.y, r.rect.h, B2);
                text::draw_right(&mut canvas, r.value_right, y, B2, t.fg_2, s);
            }
            Ctl::Swatch(s) => {
                let b = px(18);
                let bx = r.value_right - b;
                let by = r.rect.y + (r.rect.h - b) / 2;
                shapes::fill_round_rect(
                    &mut canvas, bx as usize, by as usize, b as usize, b as usize,
                    px(4) as usize, t.accent,
                );
                let y = text::centre_y(r.rect.y, r.rect.h, B3);
                text::draw_right(&mut canvas, bx - px(12), y, B3, t.fg_3, s);
            }
            Ctl::Segments(labels, on) => {
                let widths: Vec<i32> = labels.iter().map(|s| text::width(B2, s)).collect();
                for (j, label) in labels.iter().enumerate() {
                    let Some(s) = panes::segment(r.rect, r.value_right, &widths, j) else { continue };
                    if j == *on {
                        shapes::fill_round_rect(
                            &mut canvas, s.x as usize, s.y as usize, s.w as usize, s.h as usize,
                            panes::segment_radius(), t.wash_2,
                        );
                    }
                    let col = if j == *on { t.fg } else { t.fg_3 };
                    let ty = text::centre_y(s.y, s.h, B2);
                    text::draw(&mut canvas, s.x + (s.w - widths[j]) / 2, ty, B2, col, label);
                }
            }
        }
    }

    // ── a brightness row, so the `.trk` control is in the proof sheet too ──
    let trk_kinds = [RowKind::RowWithDesc];
    if let Some(mut r) = panes::setting_row(&trk_kinds, cw, cx, 0) {
        let base = top + panes::settings_height(&row_kinds) + px(40);
        r.rect.y += base;
        r.label.y += base;
        r.desc.y += base;
        text::draw(&mut canvas, r.label.x, r.label.y, B1, t.fg, "Brightness");
        text::draw(&mut canvas, r.desc.x, r.desc.y, B3, t.fg_3, "Drives the panel's PWM directly");
        let y = text::centre_y(r.rect.y, r.rect.h, B3);
        text::draw_right(&mut canvas, r.value_right, y, B3, t.fg_3, "72%");
        let trk = panes::track(r.rect, r.value_right - px(46));
        fill(&mut canvas, trk.x, trk.y, trk.w, trk.h, t.wash_2);
        let f = panes::track_fill(trk, 72);
        fill(&mut canvas, f.x, f.y, f.w, f.h, t.accent);
    }

    write_png(&out, &buf, W, H).expect("write png");
    println!("wrote {} ({}x{}, {})", out, W, H, if t.is_dark { "dark" } else { "light" });
}

fn fill(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(cw), (y + h).min(ch));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    canvas.fill_rect(x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, color);
}
