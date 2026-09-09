//! Render the Files interior to a PNG on the development host, without booting.
//!
//! ```text
//! cargo run --release -p nyx-meridian --example files_preview -- /tmp/files.png
//! ```
//!
//! ## Why this exists
//!
//! The same argument `tools/icons --preview` makes, one level up. A mis-parsed icon curve produces
//! a valid bitmap of the wrong picture and no assertion catches it; a layout whose columns collide,
//! whose type is the wrong weight, or whose contrast collapses is a valid frame of the wrong
//! picture, and the unit tests in `panes` cannot see it — they prove the boxes do not overlap, not
//! that the result looks like the design.
//!
//! This machine has no QEMU. Looking at the running system costs a power cycle, so the frame is
//! composed here first, out of the *same* `panes` geometry, the same `text` drawing path and the
//! same `tokens` palette that `apps/explorer` uses. What it cannot check is the app's behaviour —
//! only its appearance at one moment. It is a proof sheet, not a test.
//!
//! The PNG is written by hand (`examples/common/png.rs`) with stored (uncompressed) DEFLATE blocks
//! so this needs no image dependency in a crate that ships to a kernel target.

#[path = "common/png.rs"]
mod png;
use png::write_png;

use nyx_gui::canvas::Canvas;
use nyx_meridian::panes::{self, CrumbKind, NavKind};
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B1, B2, B3, D2, HINT, LB};
use nyx_meridian::Icons;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

struct Row(&'static str, bool, u64);

/// The hero scene's folder, as the real one on this machine reads.
const ROWS: &[Row] = &[
    Row("Explorer.nyx", true, 0),
    Row("GlCube.nyx", true, 0),
    Row("ImageViewer.nyx", true, 0),
    Row("Init.nyx", true, 0),
    Row("Notepad.nyx", true, 0),
    Row("QcStudio.nyx", true, 0),
    Row("Settings.nyx", true, 0),
    Row("SystemMonitor.nyx", true, 0),
    Row("Terminal.nyx", true, 0),
    Row("WindowServer.nyx", true, 0),
    Row("a-very-long-application-bundle-name-that-must-ellipsize.nyx", false, 4_312_665),
    Row("manifest.toml", false, 2_148),
    Row("meridian.conf", false, 61),
];

struct Nav(&'static str, Option<&'static str>, &'static str);

const NAV: &[Nav] = &[
    Nav("Locations", None, ""),
    Nav("Applications", Some("/mnt/nvme/apps"), "13"),
    Nav("Temp", Some("/tmp"), ""),
    Nav("Root", Some("/"), "2"),
    Nav("Devices", None, ""),
    Nav("nvme0", Some("/mnt/nvme"), "476 G"),
];

const PATH: &str = "/mnt/nvme/apps";

fn human(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if b < KB {
        format!("{} B", b)
    } else if b < MB {
        format!("{}.{} KB", b / KB, (b % KB) * 10 / KB)
    } else {
        format!("{}.{} MB", b / MB, (b % MB) * 10 / MB)
    }
}

// `W`/`H` are the surface dimensions and read better capitalised against the design's own scene
// notation; they are locals only because the preview takes a `--size`.
#[allow(non_snake_case)]
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "files_preview.png".into());
    let dark = !std::env::args().any(|a| a == "--light");
    let t = if dark { Theme::dark() } else { Theme::light() };
    let (w, h) = match arg("--size") {
        Some(s) => {
            let (a, b) = s.split_once('x').expect("--size WxH");
            (a.parse().unwrap(), b.parse().unwrap())
        }
        None => (880usize, 512usize),
    };
    let (W, H) = (w, h);
    let scroll: i32 = arg("--scroll").map(|s| s.parse().unwrap()).unwrap_or(0);

    if nyx_meridian::font::register_text() != 0 {
        eprintln!("warning: an Inter face failed to parse — this preview is in DejaVu");
    }

    let mut buf = vec![0u32; W * H];
    let mut c = Canvas::new(&mut buf, W, H);

    // Two states worth looking at at once: a hovered row and a selected one.
    let hover = Some(3usize);
    let selected = Some(10usize);

    c.fill_rect(0, 0, W, H, t.surface);

    // ── nav ──
    let kinds: Vec<NavKind> = NAV
        .iter()
        .map(|n| if n.1.is_some() { NavKind::Item } else { NavKind::Group })
        .collect();
    for (i, n) in NAV.iter().enumerate() {
        let r = panes::nav_entry(&kinds, i).unwrap();
        match n.1 {
            None => {
                text::draw(&mut c, r.x, text::centre_y(r.y, r.h, LB), LB, t.fg_4, n.0);
            }
            Some(p) => {
                let active = p == PATH;
                if active {
                    let b = panes::nav_bar(r);
                    c.fill_rect(b.x as usize, b.y as usize, b.w as usize, b.h as usize, t.accent);
                }
                let fg = if active { t.fg } else { t.fg_3 };
                text::draw(&mut c, panes::nav_label_x(r), text::centre_y(r.y, r.h, B2), B2, fg, n.0);
                if !n.2.is_empty() {
                    let cy = text::centre_y(r.y, r.h, HINT);
                    text::draw_right(&mut c, panes::nav_count_right(r), cy, HINT, t.fg_4, n.2);
                }
            }
        }
    }

    let scrollbar = panes::needs_scrollbar(W as i32, H as i32, ROWS.len());
    let l = panes::list(W as i32, H as i32, scrollbar);

    draw_rows(&mut c, &t, &l, scroll, hover, selected);

    // ★ Same order the app uses: rows, then the mask, then the heading. The band does not clip.
    let m = panes::header_mask(&l, W as i32);
    c.fill_rect(m.x as usize, m.y as usize, m.w as usize, m.h as usize, t.surface);

    // ── heading ──
    let y = text::centre_y(l.title.y, l.title.h, D2);
    let last = PATH.len();
    let label = |target: usize, raw: &str| {
        if target >= last { String::from("Applications") } else { String::from(raw) }
    };
    panes::breadcrumb(PATH, D2, label, l.title.w, |kind, s, x, _| {
        let color = match kind {
            CrumbKind::Segment(tg) if tg >= last => t.fg,
            CrumbKind::Segment(_) => t.fg_3,
            CrumbKind::Separator => t.fg_4,
        };
        text::draw(&mut c, l.title.x + x, y, D2, color, s);
    });
    let sub = format!("{} items · 58.2 GB free of 64.0 GB", ROWS.len());
    text::draw(&mut c, l.sub.x, text::centre_y(l.sub.y, l.sub.h, B3), B3, t.fg_3, &sub);

    // The window server's scrollbar. Drawn here — even though the shell owns it, not the app —
    // because the whole point of `panes::list`'s `scrollbar` argument is that the size column
    // clears it, and a preview that omits it cannot show whether it does.
    let surface = nyx_meridian::layout::Rect::new(0, 0, W as i32, H as i32);
    let track = nyx_meridian::layout::scrollbar_track(surface);
    if let Some(thumb) = nyx_meridian::layout::scrollbar_thumb(
        track,
        panes::reported_content_height(&l, ROWS.len()),
        H as i32,
        scroll,
    ) {
        let radius = (track.w / 2).max(1) as usize;
        nyx_meridian::shapes::fill_round_rect(
            &mut c, track.x as usize, track.y as usize,
            track.w as usize, track.h as usize, radius, t.line,
        );
        nyx_meridian::shapes::fill_round_rect(
            &mut c, thumb.x as usize, thumb.y as usize,
            thumb.w as usize, thumb.h as usize, radius, t.fg_4,
        );
    }

    write_png(&out, &buf, W, H).expect("write png");
    println!("wrote {} ({}x{}, {} theme)", out, W, H, if dark { "dark" } else { "light" });
}

/// The row list, drawn exactly the way `apps/explorer::draw_rows` draws it — full-height wash,
/// no clipping, relying on the header mask painted over it afterwards.
fn draw_rows(
    c: &mut Canvas,
    t: &Theme,
    l: &panes::List,
    scroll: i32,
    hover: Option<usize>,
    selected: Option<usize>,
) {
    let icon_px = panes::fr_icon_px();
    let (name_x, name_w) = panes::row_name(l);
    let meta_right = panes::row_meta_right(l);
    let (first, lastr) = panes::visible_rows(l, scroll, ROWS.len());
    for i in first..lastr {
        let e = &ROWS[i];
        let plate = panes::row_plate(l, i, scroll);
        if plate.bottom() <= 0 || plate.y >= l.band.bottom() {
            continue;
        }
        let on = selected == Some(i);
        let hv = hover == Some(i);
        if on || hv {
            let top = plate.y.max(0);
            nyx_meridian::shapes::fill_round_rect(
                c,
                plate.x.max(0) as usize,
                top as usize,
                plate.w as usize,
                (plate.bottom().min(l.band.bottom()) - top).max(0) as usize,
                panes::fr_radius(),
                if on { t.wash_2 } else { t.wash },
            );
        }
        let ic = panes::row_icon(l, plate);
        let id = if e.1 { Icons::FOLDER } else { Icons::DOC };
        let icon_fg = if on || hv { t.fg_3 } else { t.fg_4 };
        if !text::icon(c, ic.x, ic.y, id, icon_px, icon_fg) {
            eprintln!("warning: no '{}' at {}px", id, icon_px);
        }
        let ty = text::centre_y(plate.y, plate.h, B1);
        let name = text::ellipsize(B1, e.0, name_w);
        text::draw(c, name_x, ty, B1, t.fg, &name);
        if !e.1 {
            let my = text::centre_y(plate.y, plate.h, B3);
            text::draw_right(c, meta_right, my, B3, t.fg_3, &human(e.2));
        }
    }
}
