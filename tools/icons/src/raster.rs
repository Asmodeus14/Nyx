//! Turns flattened contours into 8-bit coverage bitmaps.
//!
//! ## Why a distance field rather than a stroker
//!
//! The design draws icons as strokes: 1.25 units on the 24 grid, `stroke-linecap: round`,
//! `stroke-linejoin: round`. The obvious implementation is to convert each stroke into an outline
//! polygon and scanline-fill it — which means offsetting curves, building join geometry, and
//! resolving the self-intersections that offsetting produces at tight corners. Several hundred
//! lines, and the failure mode is a subtly wrong corner nobody notices.
//!
//! The distance field gets there directly: the set of points within `w/2` of a polyline *is* the
//! round-capped, round-joined stroke of that polyline, by definition. Round joins come from taking
//! the minimum distance over all segments; round caps come from the distance to a segment being
//! measured to its endpoints. No special cases at all.
//!
//! It costs more arithmetic, but this runs at build time on a development host, once, over 45 icons
//! at five sizes. The whole run is well under a second.
//!
//! ## Supersampling
//!
//! Coverage is sampled on an 8×8 grid inside each pixel. A single centre sample would alias a
//! one-pixel stroke into a dotted line wherever it runs diagonally, so supersampling is not
//! optional — but the grid also has to be *fine* enough. At 4×4 the coverage of a pixel quantizes
//! to steps of 1/16, which on a stroke that is only about one pixel wide is a 20% error in the
//! apparent weight: a horizontal hairline came out measurably heavier than a diagonal one. 8×8
//! puts the quantization step at 1/64, well under the eye's threshold at these sizes.
//!
//! The cost is four times the samples, all of it at build time, on a host, over 45 icons. It is
//! not close to being the slow part of anything.

use crate::svg::{Ink, Part, Poly, Symbol};

/// Samples per axis inside a pixel. See the module note on why this is 8 and not 4.
const SS: usize = 8;

/// One rasterized icon at one size.
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    pub cov: Vec<u8>,
}

/// Rasterize `sym` to `px` tall, stroking at `stroke_units` in the symbol's own grid units.
///
/// Width follows from the symbol's viewBox aspect, so the square icons come out square and the
/// wordmark is not squashed into a box it was never drawn for.
///
/// `stroke_units` varies by size and that is deliberate, not a rounding artefact: the design
/// specifies 1.6 at 12px down to 1.25 at 20/22px. Scaled into pixels those all land near 1.0, which
/// is the point — a hairline that stays a hairline. A single stroke width across all sizes would
/// give a 0.63px line at 12px, which a coverage rasterizer renders as a grey smudge.
pub fn rasterize(sym: &Symbol, px: usize, stroke_units: f64) -> Bitmap {
    rasterize_parts(sym, px, |part| {
        Some(match part.ink {
            Ink::Stroke => Paint { stroke: Some(stroke_units), ..Paint::default() },
            Ink::Fill => Paint { fill: true, ..Paint::default() },
        })
    })
}

/// How one part is painted, in the symbol's own grid units.
#[derive(Clone, Copy, Debug, Default)]
pub struct Paint {
    pub fill: bool,
    /// Stroke width, or `None` for `stroke: none`.
    pub stroke: Option<f64>,
    /// `stroke-linecap: butt` — the ink stops dead at an open contour's two endpoints instead of
    /// finishing in a half-disc. Only the text cursor asks for it, and it needs it: a 3.4-wide round
    /// cap on each end of a 16-unit stem turns an I-beam into a capsule.
    pub butt_caps: bool,
}

/// Rasterize only the parts `paint_of` returns a [`Paint`] for, each at its own width.
///
/// The icon set needs neither of those — every shape is stroked at the same width and every shape is
/// drawn. The cursors need both: each one is a dark edge at 3.4 and a light body at 1.5 over the
/// *same* geometry, and they have to come out as separate coverage planes so the runtime can
/// composite them in the right order with the right two colours. Selecting a subset of the parts is
/// what makes that one pass over one set of paths instead of two hand-kept copies.
pub fn rasterize_parts(
    sym: &Symbol,
    px: usize,
    paint_of: impl Fn(&Part) -> Option<Paint>,
) -> Bitmap {
    let scale = px as f64 / sym.vb_h;
    // Round rather than truncate: a 36.83-unit wordmark at scale 0.9167 is 33.76px, and truncating
    // would clip its last column.
    let w = (sym.vb_w * scale).round().max(1.0) as usize;
    let h = px;
    let mut cov = vec![0u8; w * h];

    // Pre-scale every contour into pixel space once, rather than per sample. The viewBox origin is
    // subtracted here, so a `-2 -2` box puts its (0,0) at pixel (2,2) and nothing downstream has to
    // know the box was offset.
    let scaled: Vec<(Paint, f64, Vec<Poly>)> = sym
        .parts
        .iter()
        .filter_map(|p: &Part| {
            let paint = paint_of(p)?;
            let polys = p
                .polys
                .iter()
                .map(|poly| Poly {
                    pts: poly
                        .pts
                        .iter()
                        .map(|&(x, y)| ((x - sym.vb_x) * scale, (y - sym.vb_y) * scale))
                        .collect(),
                    closed: poly.closed,
                })
                .collect();
            let half = paint.stroke.unwrap_or(0.0) * scale / 2.0;
            Some((paint, half, polys))
        })
        .collect();

    let step = 1.0 / SS as f64;
    let origin = step / 2.0;

    for iy in 0..h {
        for ix in 0..w {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = ix as f64 + origin + sx as f64 * step;
                    let y = iy as f64 + origin + sy as f64 * step;
                    if sample(&scaled, x, y) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                // Integer throughout — no float rounding to argue about later.
                cov[iy * w + ix] = ((hits * 255) / (SS * SS) as u32) as u8;
            }
        }
    }

    Bitmap { w, h, cov }
}

/// Is this sample point inked by any part of the symbol?
fn sample(parts: &[(Paint, f64, Vec<Poly>)], x: f64, y: f64) -> bool {
    for (paint, half, polys) in parts {
        for poly in polys {
            if paint.stroke.is_some() && dist_to_poly(poly, x, y, paint.butt_caps) <= *half {
                return true;
            }
            if paint.fill && point_in_poly(&poly.pts, x, y) {
                return true;
            }
        }
    }
    false
}

/// Shortest distance from a point to a contour. Closed contours include the closing segment, which
/// is what makes a `Z`-terminated path draw a corner there instead of leaving a gap.
///
/// `butt` suppresses the round cap at an *open* contour's two ends only. Interior joins keep it,
/// which is exactly what SVG means by `stroke-linecap: butt` with `stroke-linejoin: round`.
fn dist_to_poly(poly: &Poly, x: f64, y: f64, butt: bool) -> f64 {
    let pts = &poly.pts;
    if pts.is_empty() {
        return f64::MAX;
    }
    if pts.len() == 1 {
        return ((x - pts[0].0).powi(2) + (y - pts[0].1).powi(2)).sqrt();
    }
    let n = pts.len();
    let open_butt = butt && !poly.closed;
    let mut best = f64::MAX;
    for i in 0..n - 1 {
        let cap_start = open_butt && i == 0;
        let cap_end = open_butt && i == n - 2;
        best = best.min(dist_to_seg_ext(pts[i], pts[i + 1], x, y, !cap_start, !cap_end));
    }
    if poly.closed {
        best = best.min(dist_to_seg(pts[n - 1], pts[0], x, y));
    }
    best
}

/// Point-to-segment distance. Clamping `t` to [0,1] is what produces the round cap: beyond the
/// ends, the distance is measured to the endpoint, so the ink stops in a half-disc.
fn dist_to_seg(a: (f64, f64), b: (f64, f64), px: f64, py: f64) -> f64 {
    dist_to_seg_ext(a, b, px, py, true, true)
}

/// As above, but `extend_start`/`extend_end` choose per end whether the ink rounds past the endpoint
/// or stops at it. A rejected end returns `MAX` rather than a clamped distance, so the caller's
/// `min` over the other segments still finds the nearest ink.
fn dist_to_seg_ext(
    a: (f64, f64),
    b: (f64, f64),
    px: f64,
    py: f64,
    extend_start: bool,
    extend_end: bool,
) -> f64 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let (wx, wy) = (px - a.0, py - a.1);
    let len2 = vx * vx + vy * vy;
    if len2 <= f64::EPSILON {
        return ((px - a.0).powi(2) + (py - a.1).powi(2)).sqrt();
    }
    let t = (wx * vx + wy * vy) / len2;
    if (!extend_start && t < 0.0) || (!extend_end && t > 1.0) {
        return f64::MAX;
    }
    let t = t.clamp(0.0, 1.0);
    let (cx, cy) = (a.0 + t * vx, a.1 + t * vy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Even-odd crossing test, for the handful of solid shapes.
fn point_in_poly(pts: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Crop a bitmap to its inked bounds, returning the trimmed coverage plus the offset it was
/// trimmed by.
///
/// Icons are drawn inside a 24 grid with real margins — `#a-terminal` occupies barely half its box
/// — so storing the full square would put a large amount of nothing in the atlas and in the
/// binary. Cropping also lets the atlas shelf-pack tightly, exactly as it does for glyphs.
pub fn crop(bm: &Bitmap) -> (Bitmap, usize, usize) {
    let (mut x0, mut y0, mut x1, mut y1) = (bm.w, bm.h, 0usize, 0usize);
    for y in 0..bm.h {
        for x in 0..bm.w {
            if bm.cov[y * bm.w + x] != 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return (Bitmap { w: 0, h: 0, cov: Vec::new() }, 0, 0);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut cov = vec![0u8; w * h];
    for y in 0..h {
        cov[y * w..(y + 1) * w].copy_from_slice(&bm.cov[(y + y0) * bm.w + x0..(y + y0) * bm.w + x0 + w]);
    }
    (Bitmap { w, h, cov }, x0, y0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svg::parse_path;

    fn sym(id: &str, d: &str) -> Symbol {
        Symbol {
            id: id.into(),
            vb_x: 0.0,
            vb_y: 0.0,
            vb_w: 24.0,
            vb_h: 24.0,
            parts: vec![Part {
                ink: Ink::Stroke,
                class: String::new(),
                polys: parse_path(d).unwrap(),
            }],
        }
    }

    #[test]
    fn a_horizontal_stroke_lands_where_it_was_drawn() {
        // A 1.25-unit line centred exactly on y=12 straddles the boundary between rows 11 and 12,
        // so NEITHER row is solid — asserting that one of them is would be asserting a bug. What
        // must hold is that the ink is centred on the boundary and that the total coverage down a
        // column equals the stroke width.
        let s = sym("t", "M2 12H22");
        let bm = rasterize(&s, 24, 1.25);
        let col = 12;
        let total: u32 = (0..24).map(|y| bm.cov[y * 24 + col] as u32).sum();
        let width_px = total as f64 / 255.0;
        assert!(
            (width_px - 1.25).abs() < 0.15,
            "column ink should total the 1.25px stroke width, got {:.2}",
            width_px
        );
        assert_eq!(bm.cov[11 * 24 + col], bm.cov[12 * 24 + col], "ink should straddle evenly");
        assert_eq!(bm.cov[9 * 24 + col], 0, "three rows above the stroke should be clean");
        assert_eq!(bm.cov[0], 0, "top-left corner should be empty");
        assert_eq!(bm.cov[23 * 24 + 23], 0, "bottom-right corner should be empty");
    }

    #[test]
    fn a_stroke_on_a_pixel_centre_is_solid() {
        // The complement of the test above: centred on y=12.5, the same stroke sits wholly inside
        // row 12, and there the peak really should be near-solid. Between them these two pin down
        // both the position and the width.
        let s = sym("t", "M2 12.5H22");
        let bm = rasterize(&s, 24, 1.25);
        assert!(bm.cov[12 * 24 + 12] > 230, "got {}", bm.cov[12 * 24 + 12]);
    }

    #[test]
    fn round_caps_fall_off_past_the_endpoint() {
        let s = sym("t", "M12 12H12");
        let bm = rasterize(&s, 24, 4.0);
        // A zero-length segment with a round cap is a disc of radius 2 centred at (12,12).
        assert!(bm.cov[12 * 24 + 12] > 200, "centre of the cap");
        assert_eq!(bm.cov[12 * 24 + 20], 0, "well outside the cap radius");
    }

    #[test]
    fn coverage_is_antialiased_not_binary() {
        // The whole reason for supersampling: a diagonal must produce intermediate values. If this
        // ever comes back all-or-nothing the icons will look like a 1-bit font.
        let s = sym("t", "M3 3L21 21");
        let bm = rasterize(&s, 24, 1.25);
        let partial = bm.cov.iter().filter(|&&c| c > 0 && c < 255).count();
        assert!(partial > 10, "expected soft edges, found {} partial pixels", partial);
    }

    #[test]
    fn filled_shapes_are_solid_inside() {
        let s = Symbol {
            id: "dot".into(),
            vb_x: 0.0,
            vb_y: 0.0,
            vb_w: 24.0,
            vb_h: 24.0,
            parts: vec![Part {
                ink: Ink::Fill,
                class: String::new(),
                polys: vec![Poly {
                    pts: vec![(8.0, 8.0), (16.0, 8.0), (16.0, 16.0), (8.0, 16.0)],
                    closed: true,
                }],
            }],
        };
        let bm = rasterize(&s, 24, 1.25);
        assert_eq!(bm.cov[12 * 24 + 12], 255, "inside the square");
        assert_eq!(bm.cov[2 * 24 + 2], 0, "outside the square");
    }

    #[test]
    fn a_negative_viewbox_origin_shifts_the_drawing_into_the_box() {
        // The cursors are drawn on the 24 grid inside a `-2 -2 26 26` box, so grid (0,0) has to land
        // at pixel (2,2). If the origin were ignored the pointer's tip would sit in the corner and
        // its dark edge would be clipped off two sides — which looks like a slightly thin outline
        // rather than like a bug.
        let s = Symbol {
            id: "t".into(),
            vb_x: -2.0,
            vb_y: -2.0,
            vb_w: 26.0,
            vb_h: 26.0,
            parts: vec![Part {
                ink: Ink::Stroke,
                class: String::new(),
                polys: parse_path("M0 0H0").unwrap(),
            }],
        };
        let bm = rasterize(&s, 26, 4.0);
        assert_eq!(bm.w, 26);
        assert!(bm.cov[2 * 26 + 2] > 200, "grid (0,0) should be inked at pixel (2,2)");
        // The 2-unit cap radius reaches the box corner but does not fill it — which is the bleed
        // doing its job. If the origin were ignored the cap would be centred at (0,0) and this
        // pixel would be solid.
        assert!((1..255).contains(&bm.cov[0]), "corner coverage was {}", bm.cov[0]);
        assert_eq!(bm.cov[5 * 26 + 5], 0, "well outside the cap");
    }

    #[test]
    fn rasterize_parts_selects_only_what_it_is_asked_for() {
        // The property the cursor generator leans on: two paints over one set of paths, separated.
        let s = Symbol {
            id: "t".into(),
            vb_x: 0.0,
            vb_y: 0.0,
            vb_w: 24.0,
            vb_h: 24.0,
            parts: vec![
                Part { ink: Ink::Stroke, class: "e".into(), polys: parse_path("M4 12H20").unwrap() },
                Part { ink: Ink::Stroke, class: "b".into(), polys: parse_path("M4 12H20").unwrap() },
            ],
        };
        let edge = rasterize_parts(&s, 24, |p| {
            (p.class == "e").then_some(Paint { stroke: Some(6.0), ..Paint::default() })
        });
        let body = rasterize_parts(&s, 24, |p| {
            (p.class == "b").then_some(Paint { stroke: Some(2.0), ..Paint::default() })
        });
        let ink = |b: &Bitmap| b.cov.iter().map(|&c| c as u32).sum::<u32>();
        assert!(ink(&edge) > 2 * ink(&body), "the 6-unit edge should carry far more ink than the 2");
        assert_eq!(body.cov[9 * 24 + 12], 0, "the body must not pick up the edge's width");
        assert!(edge.cov[9 * 24 + 12] > 0, "the edge should reach 3 rows out");
    }

    #[test]
    fn crop_trims_to_the_ink() {
        let s = sym("t", "M11 11H13");
        let bm = rasterize(&s, 24, 1.25);
        let (c, x0, y0) = crop(&bm);
        assert!(c.w > 0 && c.h > 0);
        assert!(c.w < 24 && c.h < 24, "cropped {}x{} should be smaller than the box", c.w, c.h);
        assert!(x0 > 0 && y0 > 0, "offset should be inside the box");
        // Every edge of a cropped bitmap must carry ink, or the crop did not tighten.
        assert!((0..c.w).any(|x| c.cov[x] != 0), "top row is empty");
        assert!((0..c.h).any(|y| c.cov[y * c.w] != 0), "left column is empty");
    }
}
