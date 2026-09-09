//! Drawing Meridian type and icons onto a CPU [`Canvas`] — the path a **ported application**
//! interior uses.
//!
//! The shell does not draw this way. It batches every glyph in a frame into one
//! `sys_gpu_draw_text` call against the prebuilt [`crate::atlas`], because it is redrawing a whole
//! screen. An application redraws a few hundred glyphs inside its own SHM surface and has no atlas
//! of its own, so it goes through `nyx_gui`'s CPU rasterizer — the same rasterizer the atlas is
//! built from, so the two agree on metrics.
//!
//! ## Why this is not five lines in each app
//!
//! A `Style` is not just a pixel size. It carries a **face** (which registry slot to rasterize
//! from), **tracking** (letter-spacing, which nothing in `nyx_gui` applies) and an **uppercase**
//! flag (`lb` transforms its text). An app that reached for `draw_char_px_face` directly would get
//! the size right and silently lose the other three, and the result — 11px mixed-case Inter where
//! the design has a tracked uppercase label — reads as a design disagreement rather than a bug.
//!
//! ## One walk, so measurement and drawing cannot disagree
//!
//! [`width`] and [`draw`] are the same function with a different visitor. This is the
//! `ctl_btn_rect` lesson from `layout.rs` applied to type: a right-aligned column whose measured
//! width and drawn width differ by the tracking of one glyph is off by a pixel per character, and
//! the symptom is a number that drifts out of its column as it gets longer.

use alloc::string::String;

use nyx_gui::canvas::Canvas;
use nyx_gui::effects::blend_color;
use nyx_gui::font as raster;

use crate::font::slot_of;
use crate::icons;
use crate::tokens::Style;

/// Walk `s` in `style`, calling `emit(char, pen_x)` for each glyph actually drawn, and return the
/// total advance.
///
/// The uppercase transform happens **here**, not at the call site, so a caller cannot measure the
/// original and draw the transform. `char::to_uppercase` yields an iterator because a few
/// codepoints uppercase to more than one glyph; those are laid out as the separate glyphs they are.
///
/// Tracking is added after **every** glyph including the last, which is what CSS `letter-spacing`
/// does. That leaves one glyph's worth of space on the right of a run at `lb`'s +1px, which is
/// invisible and is the price of `width` and `draw` being the same walk.
fn run(style: Style, s: &str, mut emit: impl FnMut(char, i32)) -> i32 {
    let slot = slot_of(style.face);
    let px = style.size();
    let track = style.tracking_px();
    let mut pen = 0i32;
    let mut step = |c: char, pen: &mut i32| {
        emit(c, *pen);
        *pen += raster::advance_px_face(slot, c, px) as i32 + track;
    };
    for c in s.chars() {
        if style.upper {
            for u in c.to_uppercase() {
                step(u, &mut pen);
            }
        } else {
            step(c, &mut pen);
        }
    }
    pen.max(0)
}

/// Pixel width of `s` set in `style`, tracking and uppercasing included.
pub fn width(style: Style, s: &str) -> i32 {
    run(style, s, |_, _| {})
}

/// The height of `style`'s real line box, from the metrics of the face it draws with.
///
/// ⚠️ Deliberately **not** `Style::line()`. `line_h` in the token table is the design's CSS
/// line-height (22px for 15px body), which is a leading value chosen for a web document; the face's
/// own ascent + descent at that size is around 18px. [`draw`] places the baseline from the face's
/// ascent, so centring against anything other than the face's own box puts the ink off-centre by
/// half the difference. Use `Style::line()` when stacking lines of a paragraph, and this when
/// centring one line inside a box.
pub fn line_box(style: Style) -> i32 {
    raster::line_height_px_face(slot_of(style.face), style.size()) as i32
}

/// The `y` at which to [`draw`] so one line of `style` sits centred in an `h`-tall box at `top`.
pub fn centre_y(top: i32, h: i32, style: Style) -> i32 {
    top + (h - line_box(style)) / 2
}

/// The distance from a line's top (what [`draw`] takes as `y`) down to its baseline.
pub fn ascent(style: Style) -> i32 {
    raster::ascent_px_face(slot_of(style.face), style.size()) as i32
}

/// The `y` at which to [`draw`] `style` so it shares a baseline with a line of `on` drawn at `y`.
///
/// `.mc .v { display: flex; align-items: baseline }` — System Monitor's 40px number and its 15px
/// unit sit on one baseline. Two runs drawn at the same `y` do **not**: `draw` places each from its
/// own face's ascent, so the smaller run hangs near the top of the larger one's line box, well above
/// the digits it belongs to. Aligning by line-box difference instead is wrong by the difference in
/// *descent*, which is a couple of pixels at these sizes and reads as a unit sagging below its
/// number.
pub fn baseline_with(style: Style, on: Style, y: i32) -> i32 {
    y + ascent(on) - ascent(style)
}

/// Draw `s` at `(x, y)`, where `y` is the top of the line box. Returns the advance width, so a
/// caller can set a second run immediately after the first.
///
/// Off-canvas glyphs are skipped rather than clamped: clamping would pile the leading characters of
/// an overhanging string on top of each other at x=0, which looks like corruption. Callers that
/// must not overflow should [`ellipsize`] first.
pub fn draw(canvas: &mut Canvas, x: i32, y: i32, style: Style, color: u32, s: &str) -> i32 {
    let slot = slot_of(style.face);
    let px = style.size();
    let w = canvas.width as i32;
    run(style, s, |c, pen| {
        let gx = x + pen;
        // A glyph starting past either edge cannot contribute ink. `draw_char_px_face` clips
        // per-texel, so this is an early-out and not the correctness boundary.
        if gx >= w || y < 0 {
            return;
        }
        if gx >= 0 {
            canvas.draw_char_px_face(gx as usize, y as usize, c, color, slot, px);
        }
    })
}

/// Draw `s` so its right edge lands on `right`. For the tabular metadata columns, which the design
/// right-aligns (`.fr .mt { text-align: right }`).
pub fn draw_right(canvas: &mut Canvas, right: i32, y: i32, style: Style, color: u32, s: &str) -> i32 {
    let w = width(style, s);
    draw(canvas, right - w, y, style, color, s);
    w
}

/// `s` trimmed with a trailing `…` until it fits in `max_w`, or `s` unchanged if it already does.
///
/// The design ellipsizes file names (`.fr .nm { text-overflow: ellipsis }`) and there is no other
/// way to keep a long name out of the size column. Every candidate is measured with [`width`], the
/// same function [`draw`] walks, so "it fits" here means it fits on screen.
///
/// A budget too narrow for even the ellipsis returns **empty**, not a lone `…`. That case is real —
/// a window dragged narrow enough leaves a name column of a few pixels — and one character
/// overflowing its column into the size column beside it looks like a layout bug, where nothing
/// looks like the column being too small, which is what has actually happened.
pub fn ellipsize(style: Style, s: &str, max_w: i32) -> String {
    if width(style, s) <= max_w {
        return String::from(s);
    }
    let ell = width(style, "…");
    if ell > max_w {
        return String::new();
    }
    let mut end = s.len();
    while end > 0 {
        end -= 1;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        if width(style, &s[..end]) + ell <= max_w {
            break;
        }
    }
    let mut out = String::from(&s[..end]);
    out.push('…');
    out
}

/// Blit the icon `id` at exactly `px`, tinted `color`, with `(x, y)` the top-left of its **box**.
///
/// Returns false when the table has no such icon at that size — [`icons::get`] refuses to
/// substitute a nearby size, and a caller that ignores this draws nothing and says nothing, which
/// is the failure the design's own note warns about. Callers should treat false as a bug, not as a
/// blank.
///
/// Coverage is blended straight into the buffer rather than expanded into a temporary RGBA `Vec`
/// (which is what `apps/shell` does for its two per-frame accent glyphs). A file list draws one
/// icon per row on every hover frame, and an allocation per icon per frame is a heap churn an app
/// with a 256-page heap does not need.
pub fn icon(canvas: &mut Canvas, x: i32, y: i32, id: &str, px: usize, color: u32) -> bool {
    icon_flipped(canvas, x, y, id, px, color, false)
}

/// [`icon`], optionally mirrored top-to-bottom inside its own `px` box.
///
/// This exists for one real gap: **the set has no up chevron.** `u-chev-d` and `u-chev` (right) are
/// the only two, so a disclosure control has nothing to turn into when it opens. Reusing `u-chev`
/// for the open state would be actively misleading — a right-pointing chevron is the conventional
/// mark for *collapsed*, so the row would read as closed exactly when it is open.
///
/// A chevron is symmetric about its vertical axis, so mirroring the down one is not an approximation
/// of an up chevron; it is the same shape, and it costs no atlas. The flip is about the icon's
/// **box**, not its cropped bitmap, so `off_y` has to be reflected too — flipping the bitmap in
/// place would shift the mark by the difference between the crop and the box.
pub fn icon_flipped(
    canvas: &mut Canvas,
    x: i32,
    y: i32,
    id: &str,
    px: usize,
    color: u32,
    flip_y: bool,
) -> bool {
    let ic = match icons::get(id, px) {
        Some(i) => i,
        None => return false,
    };
    // `off_x`/`off_y` are where the tightly-cropped bitmap sits inside the icon's box, so the
    // caller positions by the box and never has to know the icon was trimmed.
    let off_y = if flip_y {
        px as i32 - ic.off_y as i32 - ic.h as i32
    } else {
        ic.off_y as i32
    };
    let (ox, oy) = (x + ic.off_x as i32, y + off_y);
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let rgb = color & 0x00FF_FFFF;
    let tint_a = (color >> 24) & 0xFF;
    let (iw, ih) = (ic.w as i32, ic.h as i32);
    for row in 0..ih {
        let py = oy + row;
        if py < 0 || py >= ch {
            continue;
        }
        let src_row = if flip_y { ih - 1 - row } else { row };
        let cov_row = (src_row * iw) as usize;
        let dst_row = (py * cw) as usize;
        for col in 0..iw {
            let pxx = ox + col;
            if pxx < 0 || pxx >= cw {
                continue;
            }
            let cov = ic.cov[cov_row + col as usize] as u32;
            if cov == 0 {
                continue;
            }
            let a = (cov * tint_a / 255) as u8;
            let idx = dst_row + pxx as usize;
            canvas.buffer[idx] = 0xFF00_0000 | blend_color(rgb, canvas.buffer[idx], a);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::register_text;
    use crate::icons::Icons;
    use crate::tokens::{B1, B3, LB};
    use alloc::vec;
    use alloc::vec::Vec;

    /// A canvas backed by a plain buffer, so the drawing paths can be exercised on the host.
    fn surface(w: usize, h: usize) -> Vec<u32> {
        vec![0xFF00_0000u32; w * h]
    }

    fn ink(buf: &[u32]) -> usize {
        buf.iter().filter(|&&p| p & 0x00FF_FFFF != 0).count()
    }

    /// The whole reason this module exists: `lb` transforms its text, and a caller measuring the
    /// original string would right-align against a width that is not what appears.
    #[test]
    fn a_label_style_measures_and_draws_uppercase() {
        register_text();
        assert_eq!(width(LB, "locations"), width(LB, "LOCATIONS"));
        assert!(width(LB, "locations") > width(LB, "location"));
    }

    /// Tracking has to reach the pen, or 11px labels set solid where the design sets them loose.
    #[test]
    fn tracking_widens_a_run_by_one_step_per_glyph() {
        register_text();
        let track = LB.tracking_px();
        assert_eq!(track, 1, "the fixture assumes lb tracks +1px at scale 1.0");
        let n = "ABCD".chars().count() as i32;
        let tracked = width(LB, "ABCD");
        let bare: i32 = "ABCD"
            .chars()
            .map(|c| raster::advance_px_face(slot_of(LB.face), c, LB.size()) as i32)
            .sum();
        assert_eq!(tracked - bare, n * track);
    }

    /// `width` and `draw` are one walk. If they ever diverge, a right-aligned column drifts — so
    /// the returned advance is asserted against the measurement rather than assumed.
    #[test]
    fn draw_returns_exactly_what_width_measured() {
        register_text();
        let mut buf = surface(400, 40);
        let mut c = Canvas::new(&mut buf, 400, 40);
        for s in ["Applications", "12.4 KB", "manifest.toml", ""] {
            for style in [B1, B3, LB] {
                assert_eq!(draw(&mut c, 4, 4, style, 0xFF_FFFFFF, s), width(style, s), "{:?} {:?}", style, s);
            }
        }
    }

    #[test]
    fn right_aligned_text_ends_where_it_was_told_to() {
        register_text();
        let mut buf = surface(200, 30);
        let mut c = Canvas::new(&mut buf, 200, 30);
        let w = draw_right(&mut c, 180, 6, B3, 0xFF_FFFFFF, "1.8 MB");
        assert_eq!(w, width(B3, "1.8 MB"));
        // Nothing may be painted to the right of the alignment edge.
        for y in 0..30 {
            for x in 180..200 {
                assert_eq!(buf[y * 200 + x] & 0x00FF_FFFF, 0, "ink at ({}, {})", x, y);
            }
        }
    }

    #[test]
    fn ellipsize_leaves_a_fitting_string_alone() {
        register_text();
        let w = width(B1, "icon.png");
        assert_eq!(ellipsize(B1, "icon.png", w), "icon.png");
        assert_eq!(ellipsize(B1, "icon.png", w + 50), "icon.png");
    }

    /// The property that matters is not which characters survive — it is that the result FITS.
    #[test]
    fn an_ellipsized_string_always_fits_its_budget() {
        register_text();
        let long = "a-very-long-application-bundle-name.nyx";
        let ell = width(B1, "…");
        for budget in [0, 1, 10, 24, 40, 61, 97, 140] {
            let cut = ellipsize(B1, long, budget);
            assert!(
                width(B1, &cut) <= budget,
                "{:?} is {}px in a {}px budget",
                cut, width(B1, &cut), budget
            );
            if budget >= ell {
                assert!(cut.ends_with('…'), "{:?} lost its ellipsis", cut);
            } else {
                assert!(cut.is_empty(), "{:?} does not fit at all and should be empty", cut);
            }
        }
    }

    /// A UTF-8 name must not be cut through a codepoint. The loop walks back over continuation
    /// bytes for exactly this; without it the slice panics rather than rendering short.
    #[test]
    fn ellipsize_respects_char_boundaries() {
        register_text();
        let s = "рабочий-стол-документы.txt";
        for budget in 1..90 {
            let cut = ellipsize(B1, s, budget);
            assert!(width(B1, &cut) <= budget.max(width(B1, "…")));
        }
    }

    #[test]
    fn an_icon_lands_in_its_box_and_puts_ink_there() {
        let mut buf = surface(40, 40);
        let mut c = Canvas::new(&mut buf, 40, 40);
        assert!(icon(&mut c, 8, 8, Icons::FOLDER, 16, 0xFF_FFFFFF));
        assert!(ink(&buf) > 0, "the folder glyph rasterized to nothing");
        // Positioned by the BOX: nothing may land outside the 16x16 the caller reserved.
        for y in 0..40 {
            for x in 0..40 {
                if (8..24).contains(&x) && (8..24).contains(&y) {
                    continue;
                }
                assert_eq!(buf[y * 40 + x] & 0x00FF_FFFF, 0, "ink outside the box at ({}, {})", x, y);
            }
        }
    }

    /// A size the table does not carry must be reported, not silently skipped. This is the one
    /// error an app can actually act on.
    #[test]
    /// A flipped icon must stay inside the same box and be the exact mirror of the original. If
    /// `off_y` is not reflected along with the bitmap, the mark shifts by the crop margin — which is
    /// a couple of pixels, invisible in isolation, and enough to make a disclosure chevron twitch as
    /// it opens.
    #[test]
    fn a_flipped_icon_is_the_exact_mirror_of_the_original_in_the_same_box() {
        const N: usize = 32;
        const BOX: usize = 16;
        let mut a = surface(N, N);
        let mut b = surface(N, N);
        assert!(icon_flipped(&mut Canvas::new(&mut a, N, N), 8, 8, "u-chev-d", BOX, 0xFF_FFFFFF, false));
        assert!(icon_flipped(&mut Canvas::new(&mut b, N, N), 8, 8, "u-chev-d", BOX, 0xFF_FFFFFF, true));
        assert!(ink(&a) > 0 && ink(&b) > 0, "the chevron rasterized to nothing");
        assert_eq!(ink(&a), ink(&b), "the mirror lost or gained ink");

        // The flip is about the BOX, so row `8 + r` of one is row `8 + BOX - 1 - r` of the other.
        for r in 0..BOX {
            for x in 0..N {
                let top = a[(8 + r) * N + x] & 0x00FF_FFFF;
                let bot = b[(8 + BOX - 1 - r) * N + x] & 0x00FF_FFFF;
                assert_eq!(top, bot, "mirror differs at column {}, row {}", x, r);
            }
        }
        // And it must not have escaped the box the caller reserved.
        for y in 0..N {
            for x in 0..N {
                if (8..8 + BOX).contains(&x) && (8..8 + BOX).contains(&y) {
                    continue;
                }
                assert_eq!(b[y * N + x] & 0x00FF_FFFF, 0, "flipped ink escaped at ({}, {})", x, y);
            }
        }
        // A down chevron and an up chevron are not the same picture — if they were, the flip would
        // be a no-op and this whole path pointless.
        assert_ne!(a, b, "flipping produced an identical bitmap");
    }

    #[test]
    fn an_unavailable_icon_size_is_reported_rather_than_drawn_blank() {
        let mut buf = surface(40, 40);
        let mut c = Canvas::new(&mut buf, 40, 40);
        assert!(!icon(&mut c, 8, 8, Icons::FOLDER, 17, 0xFF_FFFFFF));
        assert!(!icon(&mut c, 8, 8, "u-no-such-thing", 16, 0xFF_FFFFFF));
        assert_eq!(ink(&buf), 0);
    }

    /// Drawing off the right edge must clip, not wrap onto the next row. An app's canvas changes
    /// size under it on every resize, and the first frame after a shrink is drawn against geometry
    /// computed for the old width — so overhanging text is a normal event, not a bug to assert
    /// against. What must never happen is a run reappearing at x=0 one row down, which reads as
    /// memory corruption rather than as a too-narrow column.
    ///
    /// The tell is ink to the LEFT of where drawing started. Each case gets its own surface so that
    /// is unambiguous.
    #[test]
    fn drawing_past_the_right_edge_clips_instead_of_wrapping() {
        register_text();
        let (w, h) = (60usize, 24usize);
        let left_of = |buf: &[u32], edge: usize| -> usize {
            (0..h).flat_map(|y| (0..edge).map(move |x| (x, y)))
                .filter(|&(x, y)| buf[y * w + x] & 0x00FF_FFFF != 0)
                .count()
        };

        let mut buf = surface(w, h);
        draw(&mut Canvas::new(&mut buf, w, h), 40, 4, B1, 0xFF_FFFFFF, "a long overflowing name");
        assert!(ink(&buf) > 0, "nothing was drawn, so the test proves nothing");
        assert_eq!(left_of(&buf, 40), 0, "the run wrapped back to the left edge");

        let mut buf = surface(w, h);
        icon(&mut Canvas::new(&mut buf, w, h), 54, 4, Icons::DOC, 16, 0xFF_FFFFFF);
        assert!(ink(&buf) > 0);
        assert_eq!(left_of(&buf, 54), 0, "the icon wrapped back to the left edge");
    }

    /// The other three edges: drawing wholly or partly outside must not panic. Nothing here
    /// asserts on the result — the point is that an app whose window was just dragged to 1px does
    /// not take the desktop down with it.
    #[test]
    fn drawing_past_the_other_edges_does_not_panic() {
        register_text();
        let mut buf = surface(60, 24);
        let mut c = Canvas::new(&mut buf, 60, 24);
        draw(&mut c, -30, 4, B1, 0xFF_FFFFFF, "off the left");
        draw(&mut c, 4, -20, B1, 0xFF_FFFFFF, "off the top");
        draw(&mut c, 4, 60, B1, 0xFF_FFFFFF, "below the bottom");
        icon(&mut c, -8, -8, Icons::DOC, 16, 0xFF_FFFFFF);
        icon(&mut c, 4, 20, Icons::DOC, 16, 0xFF_FFFFFF);
        icon(&mut c, 4, 900, Icons::DOC, 16, 0xFF_FFFFFF);
    }

    /// One line centred in a row has to use the FACE's box. Against `Style::line()` — the design's
    /// CSS leading — 15px body in a 44px row sits about 2px high, which is visible next to a 16px
    /// icon centred correctly beside it.
    #[test]
    fn centring_uses_the_faces_own_line_box() {
        register_text();
        let row_h = 44;
        let y = centre_y(0, row_h, B1);
        let above = y;
        let below = row_h - (y + line_box(B1));
        assert!((above - below).abs() <= 1, "{}px above, {}px below", above, below);
        assert_ne!(line_box(B1), B1.line() as i32, "the fixture assumes the two differ");
    }
}
