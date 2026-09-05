//! The wallpaper.
//!
//! ## Why the wallpaper is load-bearing
//!
//! Meridian has no backdrop blur. It cannot: there is no GPU blur pass, and the CPU `box_blur` in
//! `libs/gui` is capped at 300×300 and would eat the frame budget at 1080p. But the design leans
//! heavily on translucency — `--chrome` at 0.80 for the Command and the Entity surface, `--line` at
//! 0.07 for every hairline, the washes at 0.035.
//!
//! Flat alpha over a *busy* background reads as a smear. Flat alpha over a **low-frequency** one
//! reads as material. So the wallpaper is not decoration here; it is what makes the whole
//! translucency scheme work without a blur pass, and "low frequency" is a hard requirement rather
//! than a stylistic preference. The design says so directly: *"Abstract, matte, low-frequency: light
//! falling across an anodised surface. Low frequency is also a requirement — it is what lets flat
//! alpha read as translucency with no blur pass."*
//!
//! ## How it is drawn
//!
//! A **256×256 texture, stretched fullscreen** as the bottom `WindowQuad`. Not a full-resolution
//! buffer: at 1920×1080 that would be 8 MB to generate and re-upload, and the whole point of the
//! gradient is that it has no detail worth preserving. Stretching 256² costs one quad and the
//! bilinear smoothing is free — it is, if anything, an improvement.
//!
//! The CSS uses three radial gradients over a linear one. Radial gradients are reproduced here
//! directly; `sys_gpu_fill_rect` is a solid BLT and could not do it, but this runs once at startup
//! on the CPU into a texture, not per frame.

use alloc::vec;
use alloc::vec::Vec;

use crate::tokens::Theme;

/// Edge length of the wallpaper texture. Stretched to the full screen by the compositor.
///
/// 256 is chosen because the gradient's highest spatial frequency is roughly one stop per screen
/// quarter; sampling that at 256 across is already several times finer than the content. Halving it
/// to 128 is visible as banding on the diagonal; doubling it changes nothing.
pub const PAPER: usize = 256;

/// One radial stop: centre and radii as fractions of the texture, an RGB colour, and the alpha at
/// the centre falling to zero at `fade`.
struct Radial {
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    r: f32,
    g: f32,
    b: f32,
    a: f32,
    /// Where the gradient reaches zero, as a fraction of its radius. Matches the CSS `transparent N%`
    /// stop; without it every wash would reach the far corner and the result would be flat.
    fade: f32,
}

/// Smoothstep, so a radial stop's falloff is not a hard cone.
///
/// CSS radial gradients interpolate linearly, but a linear falloff on a very wide, very low-alpha
/// wash produces a faint but visible ring at the stop — precisely the kind of artefact that reads as
/// banding on a near-black desktop. Smoothstep removes it and is indistinguishable elsewhere.
fn smoothstep(t: f32) -> f32 {
    let t = if t < 0.0 { 0.0 } else if t > 1.0 { 1.0 } else { t };
    t * t * (3.0 - 2.0 * t)
}

/// Generate the wallpaper texture for `theme` as `PAPER × PAPER` `0xAARRGGBB` pixels, fully opaque.
///
/// Transcribed from `--paper` in `design/ver3.0/parts/01-head.html`.
pub fn generate(theme: &Theme) -> Vec<u32> {
    let dark = theme.is_dark;

    // The linear base: `linear-gradient(162deg, A 0%, B 58%, C 100%)`.
    //
    // 162° in CSS is measured clockwise from "to top", so the axis runs mostly downward and slightly
    // left-to-right. Projecting each pixel onto that axis reproduces it.
    let (a, b, c) = if dark {
        ((0x0C, 0x0E, 0x10), (0x08, 0x09, 0x0A), (0x0A, 0x0B, 0x0D))
    } else {
        ((0xEF, 0xF1, 0xF4), (0xE8, 0xEB, 0xEF), (0xEC, 0xEE, 0xF1))
    };

    let stops: [Radial; 3] = if dark {
        [
            // radial-gradient(88% 68% at 20% 14%, rgba(62,71,84,0.50), transparent 58%)
            Radial { cx: 0.20, cy: 0.14, rx: 0.88, ry: 0.68, r: 62.0, g: 71.0, b: 84.0, a: 0.50, fade: 0.58 },
            // radial-gradient(64% 56% at 90% 94%, rgba(34,42,54,0.46), transparent 62%)
            Radial { cx: 0.90, cy: 0.94, rx: 0.64, ry: 0.56, r: 34.0, g: 42.0, b: 54.0, a: 0.46, fade: 0.62 },
            // radial-gradient(50% 40% at 62% 44%, rgba(70,118,240,0.045), transparent 70%)
            // The single trace of accent in the entire wallpaper, at 4.5% — present, never nameable.
            Radial { cx: 0.62, cy: 0.44, rx: 0.50, ry: 0.40, r: 70.0, g: 118.0, b: 240.0, a: 0.045, fade: 0.70 },
        ]
    } else {
        [
            // radial-gradient(88% 68% at 20% 14%, rgba(255,255,255,0.95), transparent 60%)
            Radial { cx: 0.20, cy: 0.14, rx: 0.88, ry: 0.68, r: 255.0, g: 255.0, b: 255.0, a: 0.95, fade: 0.60 },
            // radial-gradient(64% 56% at 90% 94%, rgba(222,228,236,0.85), transparent 62%)
            Radial { cx: 0.90, cy: 0.94, rx: 0.64, ry: 0.56, r: 222.0, g: 228.0, b: 236.0, a: 0.85, fade: 0.62 },
            // The light theme has no accent wash — a blue tint on white reads as a colour cast.
            Radial { cx: 0.5, cy: 0.5, rx: 0.1, ry: 0.1, r: 0.0, g: 0.0, b: 0.0, a: 0.0, fade: 1.0 },
        ]
    };

    // 162° clockwise from "to top" → the unit vector the gradient advances along, in texture space
    // where y grows downward.
    let ang = 162.0f32.to_radians();
    let (dx, dy) = (libm::sinf(ang), -libm::cosf(ang));
    // Length of the projection of the unit square onto that axis, so t spans 0..1 corner to corner.
    let span = libm::fabsf(dx) + libm::fabsf(dy);

    let mut out = vec![0u32; PAPER * PAPER];
    for py in 0..PAPER {
        for px in 0..PAPER {
            let u = (px as f32 + 0.5) / PAPER as f32;
            let v = (py as f32 + 0.5) / PAPER as f32;

            // Base: project onto the gradient axis, then interpolate A→B over 0..0.58 and B→C over
            // 0.58..1, matching the CSS stop positions.
            let t = ((u - 0.5) * dx + (v - 0.5) * dy) / span + 0.5;
            let (mut r, mut g, mut b) = if t < 0.58 {
                let k = t / 0.58;
                (
                    a.0 as f32 + (b.0 - a.0) as f32 * k,
                    a.1 as f32 + (b.1 - a.1) as f32 * k,
                    a.2 as f32 + (b.2 - a.2) as f32 * k,
                )
            } else {
                let k = (t - 0.58) / 0.42;
                (
                    b.0 as f32 + (c.0 - b.0) as f32 * k,
                    b.1 as f32 + (c.1 - b.1) as f32 * k,
                    b.2 as f32 + (c.2 - b.2) as f32 * k,
                )
            };

            // The washes, painted over the base in order, exactly as the CSS stacks them.
            for s in &stops {
                if s.a == 0.0 {
                    continue;
                }
                let ndx = (u - s.cx) / s.rx;
                let ndy = (v - s.cy) / s.ry;
                let d = libm::sqrtf(ndx * ndx + ndy * ndy);
                if d >= s.fade {
                    continue;
                }
                let alpha = s.a * smoothstep(1.0 - d / s.fade);
                r += (s.r - r) * alpha;
                g += (s.g - g) * alpha;
                b += (s.b - b) * alpha;
            }

            let q = |v: f32| -> u32 {
                let v = if v < 0.0 { 0.0 } else if v > 255.0 { 255.0 } else { v };
                (v + 0.5) as u32
            };
            out[py * PAPER + px] = 0xFF00_0000 | (q(r) << 16) | (q(g) << 8) | q(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(p: u32) -> (i32, i32, i32) {
        (((p >> 16) & 0xFF) as i32, ((p >> 8) & 0xFF) as i32, (p & 0xFF) as i32)
    }

    #[test]
    fn the_texture_is_the_expected_size_and_opaque() {
        let p = generate(&Theme::dark());
        assert_eq!(p.len(), PAPER * PAPER);
        assert!(p.iter().all(|&v| (v >> 24) == 0xFF), "the wallpaper must be fully opaque");
    }

    /// **The load-bearing property.** Flat alpha only reads as translucency over a low-frequency
    /// ground; if the wallpaper ever gains detail, every hairline and every `--chrome` surface in
    /// the design starts to look like a smear. So: no neighbouring pixels may differ much.
    ///
    /// This is the one test here that is about the design working at all, rather than about the
    /// gradient being correct.
    #[test]
    fn the_wallpaper_is_low_frequency() {
        for theme in [Theme::dark(), Theme::light()] {
            let p = generate(&theme);
            let mut worst = 0i32;
            for y in 0..PAPER {
                for x in 1..PAPER {
                    let (r0, g0, b0) = rgb(p[y * PAPER + x - 1]);
                    let (r1, g1, b1) = rgb(p[y * PAPER + x]);
                    worst = worst.max((r1 - r0).abs()).max((g1 - g0).abs()).max((b1 - b0).abs());
                }
            }
            assert!(
                worst <= 2,
                "neighbouring pixels differ by {} — too much detail for flat alpha to read as \
                 translucency (dark={})",
                worst, theme.is_dark
            );
        }
    }

    /// It must still be a gradient, not a flat fill — the design describes light falling across an
    /// anodised surface, and a constant colour would make every translucent surface invisible.
    #[test]
    fn the_wallpaper_actually_varies() {
        let p = generate(&Theme::dark());
        let lum = |v: u32| { let (r, g, b) = rgb(v); r + g + b };
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for &v in &p {
            lo = lo.min(lum(v));
            hi = hi.max(lum(v));
        }
        assert!(hi - lo > 30, "the wallpaper is nearly flat (range {})", hi - lo);
    }

    /// The bright wash is at 20%/14% and the dark one at 90%/94%, so the top-left must be lighter
    /// than the bottom-right. A sign error in the gradient axis would flip this and nothing else
    /// would complain.
    #[test]
    fn the_light_falls_from_the_top_left() {
        let p = generate(&Theme::dark());
        let at = |fx: f32, fy: f32| {
            let x = (fx * PAPER as f32) as usize;
            let y = (fy * PAPER as f32) as usize;
            let (r, g, b) = rgb(p[y.min(PAPER - 1) * PAPER + x.min(PAPER - 1)]);
            r + g + b
        };
        assert!(
            at(0.20, 0.14) > at(0.90, 0.94),
            "the top-left wash should be brighter than the bottom-right one ({} vs {})",
            at(0.20, 0.14), at(0.90, 0.94)
        );
    }

    /// Dark stays dark and light stays light. Both themes ship, and a theme that came out inverted
    /// would make every text token unreadable.
    #[test]
    fn both_themes_stay_on_their_side_of_the_range() {
        let mean = |p: &[u32]| -> i32 {
            let s: i64 = p.iter().map(|&v| { let (r, g, b) = rgb(v); (r + g + b) as i64 }).sum();
            (s / p.len() as i64) as i32
        };
        let d = mean(&generate(&Theme::dark()));
        let l = mean(&generate(&Theme::light()));
        assert!(d < 150, "the dark wallpaper averages {} of 765 — too bright", d);
        assert!(l > 600, "the light wallpaper averages {} of 765 — too dark", l);
    }

    /// Generating it twice must give the same texture. It is produced once at startup, but a
    /// theme switch regenerates it, and a wallpaper that shifted on every switch would be visible.
    #[test]
    fn generation_is_deterministic() {
        assert_eq!(generate(&Theme::dark()), generate(&Theme::dark()));
    }

    /// The accent trace must be present but unnameable — 4.5% blue. If it ever became obvious the
    /// design's "one blue, three times per screen" rule would be broken by the wallpaper itself.
    #[test]
    fn the_accent_trace_is_barely_there() {
        let p = generate(&Theme::dark());
        let x = (0.62 * PAPER as f32) as usize;
        let y = (0.44 * PAPER as f32) as usize;
        let (r, g, b) = rgb(p[y * PAPER + x]);
        assert!(b > r, "the accent wash should push blue above red");
        assert!(b - r < 24, "the accent is nameable as blue (b-r = {}), which breaks the colour rule", b - r);
    }
}
