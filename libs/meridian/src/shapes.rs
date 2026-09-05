//! Antialiased rounded rectangles.
//!
//! ## The bug this fixes
//!
//! `nyx_gui::ui::round_rect_corners` carves a window's square corners into a rounded outline with a
//! hard distance test: a pixel is outside, on the arc, or inside, with nothing in between. At the
//! 12px radius the current desktop uses, on a light surface, that is tolerable. Meridian uses a
//! **6px radius on a near-black wallpaper**, where the same code visibly stair-steps — the design
//! calls it out by name as "a real bug, not a constraint", and notes that supersampling the four
//! corner arcs is "the cheapest quality win available".
//!
//! So that is what this does: 4×4 coverage sampling on the corner arcs only. The straight edges are
//! axis-aligned and need no antialiasing at all, and the interior is untouched, so the cost is
//! bounded by four `r × r` boxes — about 150 pixels per window at r=6, against a frame budget of
//! roughly a million.
//!
//! ## What this is not
//!
//! Not a general shape rasterizer, and not a replacement for the GPU path. Windows are composited
//! as GPU quads; this draws the *shell's own* surfaces — the Command, the Entity surface, folded
//! captions — and carves window corners after compositing. The design's long-term answer is an SDF
//! pixel shader, which is a dedicated EU-assembly boot and not this one.

use nyx_gui::canvas::Canvas;

/// Samples per axis inside a corner pixel. Sixteen samples resolves a 6px arc cleanly; more is
/// indistinguishable and the corner loop is the only place this runs.
const SS: i32 = 4;

/// Coverage of pixel `(px, py)` against a circle of radius `r` centred at `(cx, cy)`, as 0..=255.
///
/// Supersampled rather than computed from the exact area, because the exact coverage of a circle
/// crossing a square is a genuinely awkward integral and this is a corner of a window.
fn arc_coverage(px: i32, py: i32, cx: f32, cy: f32, r: f32) -> u8 {
    let r2 = r * r;
    let mut hits = 0i32;
    for sy in 0..SS {
        for sx in 0..SS {
            let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
            let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                hits += 1;
            }
        }
    }
    ((hits * 255) / (SS * SS)) as u8
}

/// Fill a rounded rectangle with `color`, antialiased on the four corner arcs.
///
/// `color` is `0xAARRGGBB` and is alpha-blended, so this draws Meridian's translucent surfaces
/// (`--chrome` at 0.80, the washes at 0.035) correctly over whatever is beneath.
pub fn fill_round_rect(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
    color: u32,
) {
    if w == 0 || h == 0 || (color >> 24) == 0 {
        return;
    }
    let r = radius.min(w / 2).min(h / 2);
    if r == 0 {
        canvas.fill_rect(x, y, w, h, color);
        return;
    }

    // Middle band: full width, no corners involved.
    canvas.fill_rect(x, y + r, w, h - 2 * r, color);
    // Top and bottom bands, inset by the radius so only the arcs remain.
    canvas.fill_rect(x + r, y, w - 2 * r, r, color);
    canvas.fill_rect(x + r, y + h - r, w - 2 * r, r, color);

    let rf = r as f32;
    let alpha = (color >> 24) & 0xFF;
    let rgb = color & 0x00FF_FFFF;
    for dy in 0..r {
        for dx in 0..r {
            // One coverage value serves all four corners: the arc is the same shape mirrored, so
            // sampling it once and reusing it is exact, not an approximation.
            let cov = arc_coverage(dx as i32, dy as i32, rf, rf, rf);
            if cov == 0 {
                continue;
            }
            let a = (alpha * cov as u32) / 255;
            if a == 0 {
                continue;
            }
            let c = (a << 24) | rgb;
            for (px, py) in [
                (x + dx, y + dy),
                (x + w - 1 - dx, y + dy),
                (x + dx, y + h - 1 - dy),
                (x + w - 1 - dx, y + h - 1 - dy),
            ] {
                canvas.fill_rect(px, py, 1, 1, c);
            }
        }
    }
}

/// Stroke a 1px rounded-rectangle outline in `color`, antialiased on the corners.
///
/// Meridian leans on hairlines for hierarchy — they are what carries structure in place of the
/// cards, panels and shadows the design does without — so a jagged one is not a small defect.
pub fn stroke_round_rect(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
    color: u32,
) {
    if w < 2 || h < 2 || (color >> 24) == 0 {
        return;
    }
    let r = radius.min(w / 2).min(h / 2);
    let alpha = (color >> 24) & 0xFF;
    let rgb = color & 0x00FF_FFFF;

    // Straight runs, stopping short of the corner boxes.
    canvas.fill_rect(x + r, y, w - 2 * r, 1, color);
    canvas.fill_rect(x + r, y + h - 1, w - 2 * r, 1, color);
    canvas.fill_rect(x, y + r, 1, h - 2 * r, color);
    canvas.fill_rect(x + w - 1, y + r, 1, h - 2 * r, color);
    if r == 0 {
        return;
    }

    // The arc band: coverage inside radius r minus coverage inside radius r-1, which is a 1px ring
    // whose antialiasing matches the straight edges it joins.
    let rf = r as f32;
    let inner = (r as f32 - 1.0).max(0.0);
    for dy in 0..r {
        for dx in 0..r {
            let outer_cov = arc_coverage(dx as i32, dy as i32, rf, rf, rf) as i32;
            let inner_cov = arc_coverage(dx as i32, dy as i32, rf, rf, inner) as i32;
            let ring = (outer_cov - inner_cov).clamp(0, 255) as u32;
            if ring == 0 {
                continue;
            }
            let a = (alpha * ring) / 255;
            if a == 0 {
                continue;
            }
            let c = (a << 24) | rgb;
            for (px, py) in [
                (x + dx, y + dy),
                (x + w - 1 - dx, y + dy),
                (x + dx, y + h - 1 - dy),
                (x + w - 1 - dx, y + h - 1 - dy),
            ] {
                canvas.fill_rect(px, py, 1, 1, c);
            }
        }
    }
}

/// Draw a 1px hairline. Meridian's `--line` tokens are translucent, so this is an alpha blend and
/// not a write — the same hairline has to read correctly over a window surface and over the
/// wallpaper, which is exactly why the token is `rgba(255,255,255,0.070)` and not a flat grey.
pub fn hairline_h(canvas: &mut Canvas, x: usize, y: usize, w: usize, color: u32) {
    canvas.fill_rect(x, y, w, 1, color);
}

/// Vertical companion to [`hairline_h`].
pub fn hairline_v(canvas: &mut Canvas, x: usize, y: usize, h: usize, color: u32) {
    canvas.fill_rect(x, y, 1, h, color);
}

/// Round off the corners of a rectangle that has **already been drawn** — the GPU composited a
/// window's content as a hard-edged quad, and this is what makes it a rounded surface.
///
/// `bg(x, y)` supplies what should show through: the wallpaper where the window sits on the desktop,
/// or the surface colour of whatever window is underneath. The caller knows that and this cannot, so
/// it is a callback rather than a colour — passing a single flat colour is exactly how the current
/// desktop's carve leaves a visible square patch at each corner on a gradient ground.
///
/// Both the carved pixel and `bg` are treated as opaque; the result is a straight lerp by coverage,
/// which is why this reads the framebuffer directly instead of going through `Canvas::fill_rect`
/// (that path alpha-blends *onto* the pixel, and cannot subtract the content that is already there).
pub fn carve_round_corners<F>(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
    bg: F,
) where
    F: Fn(usize, usize) -> u32,
{
    if w < 2 || h < 2 {
        return;
    }
    let r = radius.min(w / 2).min(h / 2);
    if r == 0 {
        return;
    }
    let rf = r as f32;
    let stride = canvas.width;
    let height = canvas.height;
    for dy in 0..r {
        for dx in 0..r {
            let cov = arc_coverage(dx as i32, dy as i32, rf, rf, rf) as u32;
            if cov == 255 {
                continue;
            }
            for (px, py) in [
                (x + dx, y + dy),
                (x + w - 1 - dx, y + dy),
                (x + dx, y + h - 1 - dy),
                (x + w - 1 - dx, y + h - 1 - dy),
            ] {
                if px >= stride || py >= height {
                    continue;
                }
                let idx = py * stride + px;
                let under = bg(px, py);
                canvas.buffer[idx] = if cov == 0 {
                    0xFF00_0000 | (under & 0x00FF_FFFF)
                } else {
                    lerp_rgb(under, canvas.buffer[idx], cov)
                };
            }
        }
    }
}

/// `t`/255 of the way from `a` to `b`, per channel. Always returns an opaque pixel — the
/// framebuffer is opaque and `Canvas` keeps it that way.
/// Mix two 0xAARRGGBB colours, `t` in 0..=255. Opaque out — every caller here is compositing
/// onto something already opaque, and carrying a source alpha through would only invite a
/// caller to pass one and expect it to mean something.
pub fn lerp_rgb(a: u32, b: u32, t: u32) -> u32 {
    let mut out = 0xFF00_0000u32;
    let mut shift = 0;
    while shift <= 16 {
        let ca = (a >> shift) & 0xFF;
        let cb = (b >> shift) & 0xFF;
        out |= ((ca * (255 - t) + cb * t) / 255) << shift;
        shift += 8;
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// SHADOW
// ─────────────────────────────────────────────────────────────────────────────

/// Newton-Raphson sqrt off a bit-twiddle seed, copied from `nyx_gui::ui`. `libm` is available here,
/// but this runs per shadow pixel and the exact value is being fed into a linear falloff — two
/// iterations are already finer than the 1/255 the result is quantised to.
fn fsqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let i = x.to_bits();
    let i = 0x5f37_5a86u32.wrapping_sub(i >> 1);
    let mut y = f32::from_bits(i);
    y = y * (1.5 - 0.5 * x * y * y);
    y = y * (1.5 - 0.5 * x * y * y);
    x * y
}

/// Signed distance from `(px, py)` to a rounded rect centred at `(cx, cy)` with half-extents
/// `(hx, hy)` and corner radius `r`. Negative inside.
fn round_rect_sdf(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hx - r);
    let qy = (py - cy).abs() - (hy - r);
    let mx = qx.max(0.0);
    let my = qy.max(0.0);
    fsqrt(mx * mx + my * my) + qx.max(qy).min(0.0) - r
}

/// The one shadow in the system: the focused window, and the floating surfaces (the Command, the
/// Entity). Drawn AFTER all content, so it falls on the windows below and on the wallpaper.
///
/// This is `nyx_gui::ui::draw_window_shadow`'s method and its tuning — see
/// [`crate::tokens::SHADOW_BLUR`] for why the shipping numbers win over the CSS token — but it takes
/// an arbitrary rect and radius. That function bakes in a 30px title bar and a 12px radius, and
/// Meridian has neither: the caption lives *outside* the surface and casts no shadow of its own,
/// because it is text on the wallpaper rather than a piece of the window.
///
/// `above` is the on-screen rects of anything stacked higher, which are skipped so a lower window's
/// shadow never darkens an upper one.
pub fn drop_shadow(
    canvas: &mut Canvas,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: usize,
    above: &[(i32, i32, i32, i32)],
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let blur = crate::tokens::SHADOW_BLUR as i32;
    let off_y = crate::tokens::SHADOW_OFFSET_Y;
    let max_a = crate::tokens::SHADOW_PEAK_ALPHA as f32;

    let stride = canvas.width as i32;
    let screen_h = canvas.height as i32;

    // The shadow is the window's outline dropped by `off_y`; the window itself does not move, so the
    // two SDFs share half-extents but not centres.
    let hx = w as f32 * 0.5;
    let hy = h as f32 * 0.5;
    let r = (radius as f32).min(hx).min(hy);
    let wcx = x as f32 + hx;
    let wcy = y as f32 + hy;
    let scy = wcy + off_y as f32;

    let (wx0, wy0, wx1, wy1) = (x, y, x + w, y + h);
    let ri = r as i32;

    let y0 = (y + off_y - blur).max(0);
    let y1 = (y + off_y + h + blur).min(screen_h);
    let x0 = (x - blur).max(0);
    let x1 = (x + w + blur).min(stride);

    let mut py = y0;
    while py < y1 {
        // A row in the window's straight vertical span is fully covered by the window across its
        // whole width — jump the interior in one step rather than testing a million pixels.
        let straight_row = py >= wy0 + ri && py < wy1 - ri;
        let mut px = x0;
        while px < x1 {
            if px >= wx0 && px < wx1 && py >= wy0 && py < wy1 {
                if straight_row {
                    px = wx1;
                    continue;
                }
                // In a corner band: only skip what is actually inside the ROUNDED window. A carved
                // corner falls through and takes shadow, so the shadow curves with the corner
                // instead of leaving a square notch.
                if round_rect_sdf(px as f32, py as f32, wcx, wcy, hx, hy, r) <= 0.0 {
                    px += 1;
                    continue;
                }
            }
            let mut occluded = false;
            for &(ox0, oy0, ox1, oy1) in above {
                if px >= ox0 && px < ox1 && py >= oy0 && py < oy1 {
                    occluded = true;
                    break;
                }
            }
            if occluded {
                px += 1;
                continue;
            }
            let d = round_rect_sdf(px as f32, py as f32, wcx, scy, hx, hy, r);
            let a = if d <= 0.0 {
                max_a
            } else if d < blur as f32 {
                max_a * (1.0 - d / blur as f32)
            } else {
                px += 1;
                continue;
            };
            let ai = a as u32;
            if ai > 0 {
                canvas.fill_rect(px as usize, py as usize, 1, 1, ai << 24);
            }
            px += 1;
        }
        py += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn canvas_of(w: usize, h: usize, buf: &mut vec::Vec<u32>) -> Canvas<'_> {
        Canvas::new(buf, w, h)
    }

    /// ★ Read the BLUE channel, not the alpha channel.
    ///
    /// `Canvas` is a compositing target: `effects::alpha_blend` returns `0xFF << 24 | rgb`, so every
    /// pixel it touches ends up opaque regardless of the coverage that produced it. Asserting on the
    /// alpha channel therefore measures nothing — it reads 255 for a 6%-covered pixel and 0 only
    /// where the buffer was never written. An earlier version of these tests did exactly that, and
    /// two of them passed by coincidence.
    ///
    /// Drawing white on black makes the blue channel equal the coverage, which is the quantity
    /// actually under test.
    fn lum(buf: &[u32], w: usize, x: usize, y: usize) -> u32 {
        buf[y * w + x] & 0xFF
    }

    /// An opaque black canvas, so `lum` reads back as coverage.
    fn black(w: usize, h: usize) -> vec::Vec<u32> {
        vec![0xFF_000000u32; w * h]
    }

    /// The bug, stated as a test: a corner must contain intermediate coverage. The old hard
    /// distance test produces only fully-on and fully-off pixels, which is what stair-steps.
    #[test]
    fn corners_are_antialiased_not_stepped() {
        let (w, h) = (40usize, 40usize);
        let mut buf = black(w, h);
        let mut c = canvas_of(w, h, &mut buf);
        fill_round_rect(&mut c, 0, 0, w, h, 6, 0xFF_FFFFFF);

        // Look only at the top-left 6x6 corner box.
        let mut partial = 0;
        for y in 0..6 {
            for x in 0..6 {
                let v = lum(&buf, w, x, y);
                if v > 0 && v < 255 {
                    partial += 1;
                }
            }
        }
        assert!(
            partial >= 4,
            "only {} partially-covered pixels in a 6px corner — the arc is being hard-tested",
            partial
        );
    }

    /// The corner must actually be cut away, or the "rounded" rect is square.
    #[test]
    fn the_extreme_corner_pixel_is_empty() {
        let (w, h) = (40usize, 40usize);
        let mut buf = black(w, h);
        let mut c = canvas_of(w, h, &mut buf);
        fill_round_rect(&mut c, 0, 0, w, h, 8, 0xFF_FFFFFF);
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert_eq!(lum(&buf, w, x, y), 0, "corner ({}, {}) should be outside the arc", x, y);
        }
    }

    /// The interior must be solid — antialiasing the corners must not leak into the fill.
    #[test]
    fn the_interior_is_fully_covered() {
        let (w, h) = (40usize, 40usize);
        let mut buf = black(w, h);
        let mut c = canvas_of(w, h, &mut buf);
        fill_round_rect(&mut c, 0, 0, w, h, 6, 0xFF_FFFFFF);
        for (x, y) in [(20, 20), (20, 1), (1, 20), (20, h - 2), (w - 2, 20)] {
            assert_eq!(lum(&buf, w, x, y), 255, "({}, {}) should be solid", x, y);
        }
    }

    /// All four corners must be identical. Mirroring one coverage value is only correct if it is
    /// applied symmetrically, and a transposed index would tilt the shape in a way that is subtle
    /// on screen and obvious here.
    #[test]
    fn all_four_corners_match() {
        let (w, h) = (40usize, 40usize);
        let mut buf = black(w, h);
        let mut c = canvas_of(w, h, &mut buf);
        fill_round_rect(&mut c, 0, 0, w, h, 7, 0xFF_FFFFFF);
        for dy in 0..7 {
            for dx in 0..7 {
                let tl = lum(&buf, w, dx, dy);
                assert_eq!(tl, lum(&buf, w, w - 1 - dx, dy), "TL vs TR at ({}, {})", dx, dy);
                assert_eq!(tl, lum(&buf, w, dx, h - 1 - dy), "TL vs BL at ({}, {})", dx, dy);
                assert_eq!(tl, lum(&buf, w, w - 1 - dx, h - 1 - dy), "TL vs BR at ({}, {})", dx, dy);
            }
        }
    }

    /// Coverage must increase monotonically as you move diagonally inward from the corner. A shape
    /// can be antialiased and still be the wrong shape; this pins the gradient's direction.
    #[test]
    fn coverage_increases_inward_along_the_diagonal() {
        let (w, h) = (40usize, 40usize);
        let mut buf = black(w, h);
        let mut c = canvas_of(w, h, &mut buf);
        fill_round_rect(&mut c, 0, 0, w, h, 8, 0xFF_FFFFFF);
        let mut prev = 0u32;
        for d in 0..8 {
            let v = lum(&buf, w, d, d);
            assert!(v >= prev, "coverage dropped moving inward at d={} ({} < {})", d, v, prev);
            prev = v;
        }
        assert_eq!(prev, 255, "the diagonal should reach solid by the radius");
    }

    /// A translucent fill must blend, not overwrite — Meridian's floating surfaces are `--chrome`
    /// at 0.80 and are supposed to read as material over the wallpaper.
    #[test]
    fn translucent_fills_blend_with_the_background() {
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0xFF_000000u32; w * h];
        let mut c = canvas_of(w, h, &mut buf);
        // 50% white over black should land near mid grey.
        fill_round_rect(&mut c, 0, 0, w, h, 4, 0x80_FFFFFF);
        let mid = buf[10 * w + 10] & 0xFF;
        assert!((100..=160).contains(&mid), "expected a mid grey, got {:#04x}", mid);
    }

    /// A radius larger than the box must clamp rather than underflow. `w - 2*r` on `usize` panics
    /// if `r` is not clamped, and a shell that panics is a desktop that vanishes.
    #[test]
    fn an_oversized_radius_clamps() {
        let (w, h) = (10usize, 10usize);
        let mut buf = black(w, h);
        {
            let mut c = canvas_of(w, h, &mut buf);
            fill_round_rect(&mut c, 0, 0, w, h, 999, 0xFF_FFFFFF);
            // Must not panic on the `w - 2*r` arithmetic either.
            stroke_round_rect(&mut c, 0, 0, w, h, 999, 0xFF_FFFFFF);
        }
        // A radius of half the box is a circle; the centre must still be filled.
        assert_eq!(lum(&buf, w, 5, 5), 255);
    }

    /// Degenerate sizes must be no-ops, not panics.
    #[test]
    fn zero_sized_shapes_are_ignored() {
        let mut buf = vec![0xFF_000000u32; 16];
        let mut c = canvas_of(4, 4, &mut buf);
        fill_round_rect(&mut c, 0, 0, 0, 4, 2, 0xFF_FFFFFF);
        fill_round_rect(&mut c, 0, 0, 4, 0, 2, 0xFF_FFFFFF);
        stroke_round_rect(&mut c, 0, 0, 1, 1, 2, 0xFF_FFFFFF);
        assert!(buf.iter().all(|&p| p == 0xFF_000000), "nothing should have been drawn");
    }

    /// A fully transparent colour must draw nothing at all, rather than blending a no-op over every
    /// pixel in the rect.
    #[test]
    fn a_transparent_colour_draws_nothing() {
        let mut buf = vec![0xFF_123456u32; 400];
        let mut c = canvas_of(20, 20, &mut buf);
        fill_round_rect(&mut c, 0, 0, 20, 20, 4, 0x00_FFFFFF);
        assert!(buf.iter().all(|&p| p == 0xFF_123456));
    }

    /// The stroke must be a ring: edges inked, centre untouched.
    #[test]
    fn the_stroke_is_hollow() {
        let (w, h) = (30usize, 30usize);
        let mut buf = black(w, h);
        let mut c = canvas_of(w, h, &mut buf);
        stroke_round_rect(&mut c, 0, 0, w, h, 6, 0xFF_FFFFFF);
        assert_eq!(lum(&buf, w, 15, 0), 255, "top edge should be inked");
        assert_eq!(lum(&buf, w, 0, 15), 255, "left edge should be inked");
        assert_eq!(lum(&buf, w, 15, h - 1), 255, "bottom edge should be inked");
        assert_eq!(lum(&buf, w, w - 1, 15), 255, "right edge should be inked");
        assert_eq!(lum(&buf, w, 15, 15), 0, "the middle must stay empty");
    }

    /// The stroke's corner must join its straight edges rather than leaving a gap or a blob. Walking
    /// the arc, every step should carry some ink.
    #[test]
    fn the_stroke_corner_is_continuous() {
        let (w, h) = (40usize, 40usize);
        let mut buf = black(w, h);
        let mut c = canvas_of(w, h, &mut buf);
        let r = 8usize;
        stroke_round_rect(&mut c, 0, 0, w, h, r, 0xFF_FFFFFF);
        // Sample the arc at 45-degree-ish steps between the two tangent points.
        for i in 0..=8 {
            let t = core::f32::consts::FRAC_PI_2 * (i as f32 / 8.0);
            let x = (r as f32 - r as f32 * t.sin()).round() as usize;
            let y = (r as f32 - r as f32 * t.cos()).round() as usize;
            // Accept any of the 3x3 around the ideal point — the ring is 1px and rounding can land
            // us just off it.
            let mut found = 0u32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        found = found.max(lum(&buf, w, px as usize, py as usize));
                    }
                }
            }
            assert!(found > 40, "gap in the stroke arc near ({}, {}): max ink {}", x, y, found);
        }
    }

    // ── carve_round_corners ──────────────────────────────────────────────────

    /// The whole point: a composited window quad is square, and after the carve its corners must
    /// show what is behind it. Content is white, ground is black.
    #[test]
    fn carving_replaces_the_corner_with_the_ground() {
        let (w, h) = (40usize, 40usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        {
            let mut c = canvas_of(w, h, &mut buf);
            carve_round_corners(&mut c, 0, 0, w, h, 6, |_, _| 0xFF_000000);
        }
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert_eq!(lum(&buf, w, x, y), 0, "corner ({}, {}) was not carved", x, y);
        }
        assert_eq!(lum(&buf, w, 20, 20), 255, "the interior must be untouched");
        assert_eq!(lum(&buf, w, 20, 0), 255, "the straight top edge must be untouched");
    }

    /// The carve must be antialiased for the same reason the fill is — it is the *same arc*, and a
    /// hard-tested carve would stair-step the window outline the fill just smoothed.
    #[test]
    fn the_carve_is_antialiased() {
        let (w, h) = (40usize, 40usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        {
            let mut c = canvas_of(w, h, &mut buf);
            carve_round_corners(&mut c, 0, 0, w, h, 6, |_, _| 0xFF_000000);
        }
        let partial = (0..6)
            .flat_map(|y| (0..6).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let v = lum(&buf, w, x, y);
                v > 0 && v < 255
            })
            .count();
        assert!(partial >= 4, "only {} partial pixels — the carve is hard-testing the arc", partial);
    }

    /// ★ The reason `bg` is a callback. The current desktop carves to one flat colour, which on a
    /// gradient wallpaper paints four visible patches of the wrong shade. Each carved pixel must take
    /// the ground colour *at its own position*.
    #[test]
    fn each_carved_pixel_takes_the_ground_beneath_it() {
        let (w, h) = (40usize, 40usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        {
            let mut c = canvas_of(w, h, &mut buf);
            // A horizontal ramp: blue channel == x. Nothing about a corner is uniform under this.
            carve_round_corners(&mut c, 0, 0, w, h, 8, |x, _| 0xFF_000000 | x as u32);
        }
        assert_eq!(lum(&buf, w, 0, 0), 0, "top-left corner should take x=0");
        assert_eq!(lum(&buf, w, w - 1, 0), (w - 1) as u32, "top-right corner should take x=39");
        assert_eq!(lum(&buf, w, 0, h - 1), 0);
        assert_eq!(lum(&buf, w, w - 1, h - 1), (w - 1) as u32);
    }

    /// A carve that runs off the framebuffer must clip, not index out of bounds — the shell is the
    /// desktop, and a window dragged to the screen edge is the ordinary case.
    #[test]
    fn carving_past_the_edge_clips() {
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        let mut c = canvas_of(w, h, &mut buf);
        carve_round_corners(&mut c, 10, 10, 40, 40, 6, |_, _| 0xFF_000000);
    }

    // ── drop_shadow ──────────────────────────────────────────────────────────

    /// The shadow must fall BELOW the surface, not around it: `SHADOW_OFFSET_Y` is what makes a
    /// window read as lifted rather than outlined.
    #[test]
    fn the_shadow_falls_downward() {
        let (w, h) = (80usize, 80usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        {
            let mut c = canvas_of(w, h, &mut buf);
            drop_shadow(&mut c, 20, 20, 40, 40, 6, &[]);
        }
        let below = lum(&buf, w, 40, 62);
        let above = lum(&buf, w, 40, 18);
        assert!(below < 255, "no shadow below the window");
        assert!(below < above, "the shadow is not heavier below ({} vs {})", below, above);
    }

    /// The surface's own footprint must be left alone — the window's content covers it, and
    /// darkening it would tint every window by its own shadow.
    #[test]
    fn the_shadow_does_not_touch_the_surface_it_casts_from() {
        let (w, h) = (80usize, 80usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        {
            let mut c = canvas_of(w, h, &mut buf);
            drop_shadow(&mut c, 20, 20, 40, 40, 6, &[]);
        }
        for (x, y) in [(40, 40), (22, 40), (57, 40), (40, 22), (40, 57)] {
            assert_eq!(lum(&buf, w, x, y), 255, "({}, {}) is inside the surface", x, y);
        }
    }

    /// It must fade out, not stop at an edge. A hard-edged shadow reads as a grey rectangle.
    #[test]
    fn the_shadow_feathers() {
        let (w, h) = (80usize, 80usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        {
            let mut c = canvas_of(w, h, &mut buf);
            drop_shadow(&mut c, 20, 20, 40, 40, 6, &[]);
        }
        // Walking away from the bottom edge, darkness must strictly recover toward the ground.
        let mut prev = 0u32;
        // The shadow's bottom edge is at 20+40+SHADOW_OFFSET_Y = 64 and it feathers over
        // SHADOW_BLUR = 9 px, so 73 is the first row it must not reach at all.
        for y in 64..=73 {
            let v = lum(&buf, w, 40, y);
            assert!(v >= prev, "shadow got darker further out at y={} ({} < {})", y, v, prev);
            prev = v;
        }
        assert_eq!(prev, 255, "the shadow should have faded to nothing by the blur radius");
    }

    /// A window stacked above must never be darkened by a lower window's shadow. This is the
    /// painter-order rule the compositor's pre-pass got wrong once already.
    #[test]
    fn occluders_are_skipped() {
        let (w, h) = (80usize, 80usize);
        let mut buf = vec![0xFF_FFFFFFu32; w * h];
        {
            let mut c = canvas_of(w, h, &mut buf);
            drop_shadow(&mut c, 20, 20, 40, 40, 6, &[(30, 60, 50, 75)]);
        }
        assert_eq!(lum(&buf, w, 40, 65), 255, "an occluding window was darkened");
    }

    #[test]
    fn a_degenerate_shadow_is_a_no_op() {
        let mut buf = vec![0xFF_FFFFFFu32; 400];
        let mut c = canvas_of(20, 20, &mut buf);
        drop_shadow(&mut c, 5, 5, 0, 10, 6, &[]);
        drop_shadow(&mut c, 5, 5, 10, -3, 6, &[]);
        assert!(buf.iter().all(|&p| p == 0xFF_FFFFFF));
    }
}
