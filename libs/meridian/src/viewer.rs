//! The Image window — `.iv`, `.iv .frame`, `.iv-b`.
//!
//! ```text
//! .iv        { flex: 1; display: grid; place-items: center; position: relative }
//! .dark .iv  { background: var(--sunken) }
//! .iv .frame { border: 1px solid var(--line); border-radius: 4px; overflow: hidden }
//! .iv-b      { position: absolute; left: 0; right: 0; bottom: 0; height: 42px;
//!              display: flex; align-items: center; gap: 24px; padding: 0 20px;
//!              border-top: 1px solid var(--line); font-size: 12px; color: var(--fg-3);
//!              background: var(--chrome) }
//! .iv-b .z   { font-variant-numeric: tabular-nums; color: var(--fg-2) }
//! .iv-b .r   { margin-left: auto; display: flex; gap: 22px }
//! ```
//!
//! ## The bar floats; it does not take space
//!
//! `.iv-b` is `position: absolute`, so the image is centred in the **whole** surface and the bar is
//! drawn over it on a translucent ground. That is what makes the design's other claim work — *"its
//! controls are plain text on a hairline at the foot of the surface, and they appear on hover"* — a
//! bar that reserved 42px would make the image jump every time the pointer entered the window.
//!
//! ## Units
//!
//! Same rule as everywhere else in this crate: the `const`s are design units against the 1440x900
//! scene and are private; everything public returns real pixels through [`crate::scale::px`].

use crate::layout::Rect;
use crate::scale::px as sc;

/// `.iv-b { height: 42px }`
const BAR_H: i32 = 42;
/// `.iv-b { padding: 0 20px }`
const BAR_PAD_X: i32 = 20;
/// `.iv-b { gap: 24px }` — between the left-hand items.
const BAR_GAP_L: i32 = 24;
/// `.iv-b .r { gap: 22px }` — between the right-hand items.
const BAR_GAP_R: i32 = 22;
/// `.iv .frame { border-radius: 4px }`
const FRAME_RADIUS: i32 = 4;
/// The air between a fitted image and the window's edge.
///
/// The design's `.iv` declares no padding — its frame is simply the size the mockup's image happens
/// to be. A real viewer has to choose, and zero would put the frame's 1px border flush against the
/// window's own rounded corner, where it reads as a rendering fault rather than as a border.
const FIT_INSET: i32 = 40;

/// The bar across the foot of the surface. Drawn **over** the image, not beside it.
pub fn bar(surface: Rect) -> Rect {
    let h = sc(BAR_H).min(surface.h.max(0));
    Rect::new(surface.x, surface.bottom() - h, surface.w, h)
}

/// The rect of bar item `i`.
///
/// Items `[0, right_from)` are laid out from the left padding on `.iv-b`'s 24px gap; items
/// `[right_from, n)` are the `.r` group, laid out from the **right** padding on its 22px gap and in
/// left-to-right reading order. `widths` is each item's measured text width, so the caller measures
/// once and this cannot disagree with what gets drawn.
pub fn bar_item(surface: Rect, widths: &[i32], right_from: usize, i: usize) -> Option<Rect> {
    if i >= widths.len() {
        return None;
    }
    let b = bar(surface);
    if i < right_from {
        let mut x = b.x + sc(BAR_PAD_X);
        for w in &widths[..i] {
            x += w + sc(BAR_GAP_L);
        }
        return Some(Rect::new(x, b.y, widths[i], b.h));
    }
    // Right group: measure the whole run, then walk forward from its left edge, so the LAST item
    // ends exactly on the padding regardless of how many there are.
    let right: &[i32] = &widths[right_from..];
    let total: i32 = right.iter().sum::<i32>() + sc(BAR_GAP_R) * (right.len() as i32 - 1).max(0);
    let mut x = b.right() - sc(BAR_PAD_X) - total;
    for w in &right[..i - right_from] {
        x += w + sc(BAR_GAP_R);
    }
    Some(Rect::new(x, b.y, widths[i], b.h))
}

/// The bar item under `(mx, my)`, or None.
///
/// The hit band is the item's text width and the bar's **full height**, which is 42px against a 12px
/// line: these are plain words with no plate behind them, and a target the exact height of the ink
/// is a target nobody can hit.
pub fn bar_hit(surface: Rect, widths: &[i32], right_from: usize, mx: i32, my: i32) -> Option<usize> {
    if !bar(surface).contains(mx, my) {
        return None;
    }
    (0..widths.len()).find(|&i| bar_item(surface, widths, right_from, i).is_some_and(|r| r.contains(mx, my)))
}

/// `.iv .frame { border-radius: 4px }`, in real pixels.
pub fn frame_radius() -> usize {
    sc(FRAME_RADIUS).max(1) as usize
}

/// The largest `(w, h)` that fits `(max_w, max_h)` keeping `iw:ih`.
///
/// Integer maths on `u64` throughout: `iw * max_h` overflows `usize` on a 32-bit intermediate for a
/// large image, and an aspect ratio that wraps produces a frame the wrong shape rather than an
/// obviously broken one.
pub fn fit_dims(iw: i32, ih: i32, max_w: i32, max_h: i32) -> (i32, i32) {
    if iw <= 0 || ih <= 0 || max_w <= 0 || max_h <= 0 {
        return (0, 0);
    }
    let by_width = (ih as u64 * max_w as u64 / iw as u64) as i32;
    if by_width <= max_h {
        (max_w.max(1), by_width.max(1))
    } else {
        let by_height = (iw as u64 * max_h as u64 / ih as u64) as i32;
        (by_height.max(1), max_h.max(1))
    }
}

/// The size a `iw x ih` image is drawn at inside `surface`.
///
/// `fit` scales it down to sit inside the window with [`FIT_INSET`] of air; otherwise it is 1:1.
/// **Never scaled up**: enlarging a 32x32 icon to fill a 1400px window with nearest-neighbour
/// produces a mosaic, and "fit" is a request to see the whole image, not to magnify it.
pub fn drawn_size(surface: Rect, iw: i32, ih: i32, fit: bool) -> (i32, i32) {
    if iw <= 0 || ih <= 0 {
        return (0, 0);
    }
    if !fit {
        return (iw, ih);
    }
    let (max_w, max_h) = (surface.w - 2 * sc(FIT_INSET), surface.h - 2 * sc(FIT_INSET));
    if iw <= max_w && ih <= max_h {
        return (iw, ih);
    }
    fit_dims(iw, ih, max_w.max(1), max_h.max(1))
}

/// The frame's rect: the drawn image, centred in `surface` and offset by `pan`.
///
/// `place-items: center` — centred on the whole surface, because the bar floats over it rather than
/// taking space from it. An image larger than the window overhangs on both sides equally, which is
/// what `pan` then moves.
pub fn frame(surface: Rect, dw: i32, dh: i32, pan: (i32, i32)) -> Rect {
    Rect::new(
        surface.x + (surface.w - dw) / 2 + pan.0,
        surface.y + (surface.h - dh) / 2 + pan.1,
        dw,
        dh,
    )
}

/// How far `pan` may travel on each axis before the image's edge comes inside the window.
///
/// Zero on an axis where the image already fits — panning an image that is entirely visible moves it
/// away from the centre for no reason, and that reads as the window drifting.
pub fn pan_limit(surface: Rect, dw: i32, dh: i32) -> (i32, i32) {
    (((dw - surface.w) / 2).max(0), ((dh - surface.h) / 2).max(0))
}

/// The scale an image is being shown at, as a whole percentage — the design's `.z` readout.
///
/// Rounds to nearest rather than truncating: a 999-pixel-wide image drawn at 999 is `100%`, and a
/// viewer that says `99%` when nothing has been resampled is lying about the one thing this readout
/// exists to report.
pub fn zoom_pct(iw: i32, dw: i32) -> i32 {
    if iw <= 0 {
        return 0;
    }
    ((dw as i64 * 200 + iw as i64) / (iw as i64 * 2)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::register_text;
    use alloc::vec;
    use alloc::vec::Vec;

    fn surface() -> Rect {
        // The design's own Image window: 672 wide, 544 tall.
        Rect::new(0, 0, 672, 544)
    }

    /// The design's own metrics for the bar.
    #[test]
    fn the_bar_matches_the_design_metrics() {
        let b = bar(surface());
        assert_eq!(b.h, 42);
        assert_eq!(b.bottom(), 544, "the bar is not at the foot of the surface");
        assert_eq!(b.w, 672, "the bar spans the surface (left: 0; right: 0)");
    }

    /// ★ The bar FLOATS. If it took space from the viewport the image would jump by 42px every time
    /// the pointer entered the window, since the bar only appears on hover.
    #[test]
    fn the_bar_overlaps_the_image_rather_than_displacing_it() {
        let s = surface();
        // At 1:1 a large image really does run under the bar; that is the case the floating bar has
        // to be right about.
        let (dw, dh) = drawn_size(s, 1280, 720, false);
        let f = frame(s, dw, dh, (0, 0));
        // Centred on the WHOLE surface: equal air above and below, to the odd pixel that integer
        // centring cannot split.
        assert!(
            ((f.y - s.y) - (s.bottom() - f.bottom())).abs() <= 1,
            "the image was centred above the bar: {}px above, {}px below",
            f.y - s.y,
            s.bottom() - f.bottom()
        );
        assert!(f.bottom() > bar(s).y, "the fixture assumes the two actually overlap");
    }

    /// Left items walk from the left padding; the right group ends exactly on the right padding.
    #[test]
    fn bar_items_lay_out_from_both_ends() {
        let s = surface();
        let widths = vec![34, 22, 58, 30, 84];
        let first = bar_item(s, &widths, 2, 0).unwrap();
        assert_eq!(first.x, 20, ".iv-b padding-left");
        let second = bar_item(s, &widths, 2, 1).unwrap();
        assert_eq!(second.x, first.right() + 24, ".iv-b gap");

        let last = bar_item(s, &widths, 2, 4).unwrap();
        assert_eq!(last.right(), 672 - 20, ".iv-b padding-right");
        let middle = bar_item(s, &widths, 2, 3).unwrap();
        assert_eq!(middle.right() + 22, last.x, ".iv-b .r gap");
        assert!(bar_item(s, &widths, 2, 5).is_none());
    }

    /// Hit-testing must agree with drawing on every item, and decline everywhere else.
    #[test]
    fn every_bar_item_is_hit_within_itself_and_nowhere_outside_the_bar() {
        let s = surface();
        let widths = vec![34, 22, 58, 30, 84];
        for i in 0..widths.len() {
            let r = bar_item(s, &widths, 2, i).unwrap();
            assert_eq!(
                bar_hit(s, &widths, 2, r.x + r.w / 2, r.y + r.h / 2),
                Some(i),
                "item {} does not hit-test to itself",
                i
            );
        }
        assert_eq!(bar_hit(s, &widths, 2, 336, 10), None, "hit above the bar");
        assert_eq!(bar_hit(s, &widths, 2, 5, 530), None, "hit in the left padding");
        assert_eq!(bar_hit(s, &widths, 2, 668, 530), None, "hit in the right padding");
        // The gaps between items are dead, which is what makes two adjacent words two controls.
        let a = bar_item(s, &widths, 2, 0).unwrap();
        assert_eq!(bar_hit(s, &widths, 2, a.right() + 12, 530), None);
    }

    /// A single right-hand item must still end on the padding — the `len - 1` in the gap sum is
    /// where an off-by-one would put it 22px adrift.
    #[test]
    fn a_lone_right_hand_item_still_ends_on_the_padding() {
        let s = surface();
        let widths = vec![40, 90];
        assert_eq!(bar_item(s, &widths, 1, 1).unwrap().right(), 672 - 20);
        // ...and a bar with no right group at all must not panic.
        assert_eq!(bar_item(s, &widths, 2, 1).unwrap().x, 20 + 40 + 24);
    }

    /// ★ Fit never ENLARGES. Nearest-neighbour up-scaling a small image to fill a large window
    /// produces a mosaic, and "fit" means "show me all of it", not "magnify it".
    #[test]
    fn fitting_shrinks_a_large_image_and_leaves_a_small_one_alone() {
        let s = surface();
        assert_eq!(drawn_size(s, 32, 32, true), (32, 32), "a small image was enlarged");
        assert_eq!(drawn_size(s, 32, 32, false), (32, 32));

        let (w, h) = drawn_size(s, 4000, 3000, true);
        assert!(w <= 672 - 80 && h <= 544 - 80, "{}x{} does not fit with its inset", w, h);
        // Aspect preserved to within a pixel of rounding.
        assert!((w as f32 / h as f32 - 4.0 / 3.0).abs() < 0.02, "{}x{}", w, h);
        // 1:1 hands back the real size, however big.
        assert_eq!(drawn_size(s, 4000, 3000, false), (4000, 3000));
    }

    /// Degenerate images must not panic or divide by zero — a decoder can hand back anything.
    #[test]
    fn a_degenerate_image_is_answered_rather_than_dividing_by_zero() {
        let s = surface();
        assert_eq!(drawn_size(s, 0, 0, true), (0, 0));
        assert_eq!(drawn_size(s, -5, 10, true), (0, 0));
        assert_eq!(fit_dims(0, 0, 100, 100), (0, 0));
        assert_eq!(fit_dims(10, 10, 0, 0), (0, 0));
        assert_eq!(zoom_pct(0, 100), 0);
        // A window smaller than the inset must still produce something drawable.
        let tiny = Rect::new(0, 0, 30, 30);
        let (w, h) = drawn_size(tiny, 1000, 1000, true);
        assert!(w >= 1 && h >= 1, "{}x{}", w, h);
    }

    /// Panning is only possible on an axis that actually overhangs. Otherwise the image drifts off
    /// centre for no reason, which reads as the window sliding rather than the image moving.
    #[test]
    fn panning_is_limited_to_the_overhang_and_is_zero_when_the_image_fits() {
        let s = surface();
        assert_eq!(pan_limit(s, 100, 100), (0, 0), "a fitting image should not pan");
        let (px, py) = pan_limit(s, 1272, 1044);
        assert_eq!(px, (1272 - 672) / 2);
        assert_eq!(py, (1044 - 544) / 2);
        // At the limit the image's edge sits exactly on the window's edge.
        let f = frame(s, 1272, 1044, (px, 0));
        assert_eq!(f.x, s.x, "panning to the limit did not bring the edge in");
    }

    /// ★ The `.z` readout must say `100%` when nothing has been resampled. Truncating instead of
    /// rounding reports `99%` for an image drawn at its own size, which is a lie about the one thing
    /// this number exists to report.
    #[test]
    fn the_zoom_readout_rounds_so_unscaled_reads_as_one_hundred() {
        assert_eq!(zoom_pct(999, 999), 100);
        assert_eq!(zoom_pct(1280, 1280), 100);
        assert_eq!(zoom_pct(1280, 640), 50);
        assert_eq!(zoom_pct(1280, 320), 25);
        assert_eq!(zoom_pct(3, 1), 33);
        // Enlarged, which only 1:1 mode can reach — the number must still be honest.
        assert_eq!(zoom_pct(100, 200), 200);
    }

    /// The frame is centred, and `pan` moves it from there rather than from a corner.
    #[test]
    fn the_frame_is_centred_and_pan_displaces_it_from_the_centre() {
        let s = surface();
        let f = frame(s, 544, 306, (0, 0));
        assert_eq!(f.x, (672 - 544) / 2);
        assert_eq!(f.y, (544 - 306) / 2);
        assert_eq!(frame(s, 544, 306, (20, -10)).x, f.x + 20);
        assert_eq!(frame(s, 544, 306, (20, -10)).y, f.y - 10);
    }

    /// A window narrower than the bar's own padding must not produce negative-width items.
    #[test]
    fn a_window_narrower_than_the_bars_padding_does_not_go_negative() {
        register_text();
        let tiny = Rect::new(0, 0, 24, 20);
        let b = bar(tiny);
        assert!(b.h > 0 && b.h <= tiny.h, "{:?}", b);
        let widths: Vec<i32> = vec![30, 40];
        for i in 0..2 {
            let r = bar_item(tiny, &widths, 1, i).unwrap();
            assert!(r.w > 0 && r.h > 0, "item {} is {:?}", i, r);
        }
    }
}
