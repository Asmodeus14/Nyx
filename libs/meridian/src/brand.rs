//! The identity at run time — the mark, resampled for the one animation that needs it.
//!
//! ## Why this scales a bitmap when `icons::get` refuses to
//!
//! `icons::get` deliberately returns nothing for a size it was not drawn at, because a scaled icon
//! is a blurry icon and the atlas exists precisely so that every size is authored. That rule is
//! right for chrome, which is on screen for the life of the machine.
//!
//! Cold-start stage 3 is the exception, and it is the only one. The mark travels from the size the
//! kernel drew it at down to the Entity's 22px over 22 frames — twenty-two intermediate sizes, used
//! once, on the first second of the machine's life and never again. Authoring them would put
//! roughly 60 KB of coverage in the binary to be read once per power-on. Box-filtering the 96px
//! master is a handful of lines, and on a bitmap that is already an antialiased coverage field the
//! result is genuinely good rather than merely acceptable — averaging coverage is what a rasterizer
//! does anyway.
//!
//! The endpoints do not go through here. The mark starts at the kernel's native size and the
//! Entity's ring is an authored 22px atlas glyph, so the two frames anyone actually looks at are
//! both exact.

use alloc::vec;
use alloc::vec::Vec;

pub use crate::brand_gen::{Coverage, MARK, WORDMARK};

/// The mark's authored box, in pixels. Square by construction — the mark is drawn on the same 24
/// grid as every icon in the system.
pub const MARK_BOX: usize = 96;

/// Resample the mark to `px` square, returning `px * px` coverage.
///
/// Box filter over the mark's full BOX, not over its cropped ink: the crop offsets are margin, and
/// dropping them would silently enlarge the mark relative to everything positioned against the box.
///
/// Upscaling is allowed but pointless — the master is 96 and the animation only ever shrinks.
pub fn mark_at(px: usize) -> Vec<u8> {
    let px = px.max(1);
    let mut out = vec![0u8; px * px];
    let m = &MARK;
    for oy in 0..px {
        // Source rows this output row covers, in box coordinates.
        let y0 = oy * MARK_BOX / px;
        let y1 = (((oy + 1) * MARK_BOX + px - 1) / px).max(y0 + 1).min(MARK_BOX);
        for ox in 0..px {
            let x0 = ox * MARK_BOX / px;
            let x1 = (((ox + 1) * MARK_BOX + px - 1) / px).max(x0 + 1).min(MARK_BOX);
            let mut sum = 0u32;
            let mut n = 0u32;
            for by in y0..y1 {
                for bx in x0..x1 {
                    sum += box_cov(m, bx, by) as u32;
                    n += 1;
                }
            }
            if n > 0 {
                out[oy * px + ox] = (sum / n) as u8;
            }
        }
    }
    out
}

/// Coverage at a point in the asset's BOX, with the crop offsets undone. Outside the ink is 0.
#[inline]
fn box_cov(c: &Coverage, x: usize, y: usize) -> u8 {
    if x < c.off_x || y < c.off_y {
        return 0;
    }
    let (ix, iy) = (x - c.off_x, y - c.off_y);
    if ix >= c.w || iy >= c.h {
        return 0;
    }
    c.cov[iy * c.w + ix]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(cov: &[u8]) -> u32 {
        cov.iter().map(|&c| c as u32).sum()
    }

    #[test]
    fn the_master_is_the_size_the_kernel_draws() {
        // The shell picks the mark up at whatever size the kernel left it. If these ever disagree
        // the handoff is a jump-cut, which is the one thing this animation exists to avoid.
        assert_eq!(MARK.box_w, MARK_BOX);
        assert_eq!(MARK.box_h, MARK_BOX);
    }

    #[test]
    fn resampling_at_full_size_reproduces_the_box() {
        // 1:1 must be the identity, or every frame of the travel is filtered against a master that
        // is already slightly wrong.
        let out = mark_at(MARK_BOX);
        for y in 0..MARK_BOX {
            for x in 0..MARK_BOX {
                assert_eq!(out[y * MARK_BOX + x], box_cov(&MARK, x, y), "at {},{}", x, y);
            }
        }
    }

    #[test]
    fn ink_density_is_preserved_as_it_shrinks() {
        // A box filter conserves total coverage per unit area. If it did not — if it point-sampled,
        // say — the mark would visibly thin out and break up as it travelled, because it is made
        // almost entirely of diagonals a pixel or two wide.
        let full = ink(&mark_at(MARK_BOX)) as f64 / (MARK_BOX * MARK_BOX) as f64;
        for px in [64usize, 44, 32, 22] {
            let d = ink(&mark_at(px)) as f64 / (px * px) as f64;
            let ratio = d / full;
            assert!(
                (0.85..=1.15).contains(&ratio),
                "at {}px the ink density is {:.2}x the master's",
                px, ratio
            );
        }
    }

    #[test]
    fn every_size_in_the_travel_has_ink_everywhere_it_should() {
        // The failure this catches is a row or column of zeros from an off-by-one in the source
        // span — invisible in a still, a flicker in motion.
        for px in 22..=MARK_BOX {
            let c = mark_at(px);
            assert_eq!(c.len(), px * px);
            assert!(c.iter().any(|&v| v > 200), "{}px has no solid ink", px);
            // The mark spans x 4..20 of the 24 grid, so the middle 60% of every size must carry ink
            // on at least one row.
            let lo = px * 4 / 24;
            let hi = px * 20 / 24;
            let any = (lo..hi).any(|x| (0..px).any(|y| c[y * px + x] > 0));
            assert!(any, "{}px is empty across its own optical box", px);
        }
    }

    #[test]
    fn a_degenerate_size_does_not_panic_or_return_nothing() {
        assert_eq!(mark_at(0).len(), 1);
        assert_eq!(mark_at(1).len(), 1);
    }
}
