//! Genome + stage + influence -> a 24x24 field of palette indices, then a
//! run-length pass into `FillRect` draw calls.
//!
//! Integer only. No float, no trig, no `alloc`. This is a direct port of the
//! JavaScript reference generator and must stay in step with it — the two are
//! verified against each other by rendering the same seeds.

use crate::genome::{Archetype, Genome, Stage};
use crate::Influence;

pub const G: usize = 24;

// Palette indices.
pub const EMPTY: u8 = 0;
pub const OUT: u8 = 1;
pub const SHADOW: u8 = 2;
pub const BODY: u8 = 3;
pub const LIGHT: u8 = 4;
pub const ACC: u8 = 5;
pub const LITE: u8 = 6;

/// 0xAARRGGBB, straight into the SHM surface.
pub const PALETTE: [u32; 7] = [
    0x0000_0000, 0xFF08_090A, 0xFF16_1A1E, 0xFF22_262B,
    0xFF34_3A41, 0xFF46_76F0, 0xFF8F_ADFF,
];

/// Authored half-width profiles, 18 rows, 0-8 cells. Repeated values are flat
/// facets; four of the six carry a pinch, which is what separates "head" from
/// "mass" without anything being drawn to say so.
const PROFILE: [[u8; 18]; 6] = [
    [2, 4, 5, 5, 3, 3, 5, 6, 7, 7, 7, 7, 7, 6, 5, 4, 3, 2], // Wanderer
    [3, 4, 4, 4, 3, 4, 5, 5, 4, 4, 3, 3, 2, 2, 1, 1, 0, 0], // Specter
    [1, 2, 3, 4, 5, 6, 7, 8, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0], // Shard
    [2, 4, 6, 7, 7, 6, 5, 4, 3, 2, 2, 1, 1, 0, 0, 0, 0, 0], // Ember
    [1, 2, 3, 3, 3, 4, 4, 4, 4, 4, 3, 3, 3, 2, 2, 1, 0, 0], // Weaver
    [2, 4, 6, 7, 8, 8, 8, 8, 8, 8, 8, 8, 7, 6, 4, 2, 0, 0], // Void
];

pub struct Field {
    pub cells: [u8; G * G],
    pub eye_row: usize,
    pub top: usize,
    pub rows: usize,
}

/// JavaScript `x || d` is substitute-if-zero — NOT `max(d)`. The two differ
/// whenever a half-width is 1, which is exactly where thin tapers live.
#[inline]
fn nz(v: u8, d: isize) -> isize { if v == 0 { d } else { v as isize } }

/// `hw[i] || d`, including JavaScript's behaviour for an index off the end of
/// the profile: `undefined || d` is `d`, so an out-of-range row yields the
/// DEFAULT and not the last row's half-width.
///
/// This is the one place the port had drifted. The marking-1 notch walks down
/// from the eye row in steps of three and runs off the bottom of the body at
/// stages 4 and 5; the Rust reached for `.min(rows - 1)` there, which clamps to
/// the last row instead of falling back to 2.
///
/// It is a LATENT difference, not a live one — 20,000 random seeds were
/// rendered under both rules and not one produced a different field, because an
/// index past the end means a row below the creature, where there is nothing
/// for `mark` to recolour. Corrected anyway. A port that agrees with its
/// specification only because the disagreement cannot currently escape is one
/// refactor away from being wrong, and the failure here is silent: a creature
/// that is valid, plausible, and not the one the seed describes.
#[inline]
fn hwv(hw: &[u8; 20], rows: usize, i: isize, d: isize) -> isize {
    if i < 0 || i as usize >= rows { d } else { nz(hw[i as usize], d) }
}

#[inline]
fn rnd_div(num: u32, den: u32) -> u32 { (2 * num + den) / (2 * den) }

impl Field {
    #[inline]
    fn at(&self, x: isize, y: isize) -> u8 {
        if x < 0 || y < 0 || x >= G as isize || y >= G as isize { EMPTY }
        else { self.cells[y as usize * G + x as usize] }
    }
    #[inline]
    fn set(&mut self, x: isize, y: isize, v: u8) {
        if x >= 0 && y >= 0 && x < G as isize && y < G as isize {
            self.cells[y as usize * G + x as usize] = v;
        }
    }
}

pub fn build(g: &Genome, stage: Stage, inf: &Influence) -> Field {
    let s = stage.index() as u32;
    let prof = &PROFILE[g.archetype as usize];
    let nrows = g.height_rows as usize;

    // Resample the 18-row profile to the genome's row count, then scale width.
    let mut raw = [0u8; 20];
    for i in 0..nrows {
        let src = rnd_div((i * 17) as u32, (nrows - 1) as u32) as usize;
        let v = (prof[src] as u32 * g.width_scale as u32 + 50) / 100;
        raw[i] = if v > 8 { 8 } else { v as u8 };
    }
    // Trim zero-width rows. Without this a profile that tapers to nothing
    // leaves phantom rows and the crest and mantle detach from the body.
    let mut f0 = 0usize;
    let mut l0 = nrows - 1;
    while f0 < l0 && raw[f0] == 0 { f0 += 1; }
    while l0 > f0 && raw[l0] == 0 { l0 -= 1; }
    let rows = l0 - f0 + 1;
    let mut hw = [0u8; 20];
    hw[..rows].copy_from_slice(&raw[f0..=l0]);

    let top = 3 + (20usize.saturating_sub(rows)) / 2;
    let bot = top + rows - 1;
    let mut f = Field { cells: [EMPTY; G * G], eye_row: top, top, rows };

    // 1 — mass, mirrored about the 11/12 axis.
    for i in 0..rows {
        let y = (top + i) as isize;
        for d in 0..hw[i] as isize { f.set(11 - d, y, BODY); f.set(12 + d, y, BODY); }
    }

    // Void is hollow — carved before shading so the cavity gets its own rim.
    if g.archetype == Archetype::Void && rows > 7 {
        for i in 3..rows - 4 {
            let y = (top + i) as isize;
            for d in 0..hw[i] as isize - 3 { f.set(11 - d, y, EMPTY); f.set(12 + d, y, EMPTY); }
        }
    }

    // 2 — shading, two passes. Outline, then a one-cell rim inside it: light
    // on the top-left faces, shadow on the bottom-right. Same light direction
    // as the desktop wallpaper.
    for y in 0..G as isize { for x in 0..G as isize {
        if f.at(x, y) != BODY { continue; }
        if f.at(x - 1, y) == EMPTY || f.at(x + 1, y) == EMPTY
            || f.at(x, y - 1) == EMPTY || f.at(x, y + 1) == EMPTY { f.set(x, y, OUT); }
    }}
    for y in 0..G as isize { for x in 0..G as isize {
        if f.at(x, y) != BODY { continue; }
        if f.at(x - 1, y) == OUT || f.at(x, y - 1) == OUT || f.at(x - 1, y - 1) == OUT {
            f.set(x, y, LIGHT);
        } else if f.at(x + 1, y) == OUT || f.at(x, y + 1) == OUT || f.at(x + 1, y + 1) == OUT {
            f.set(x, y, SHADOW);
        }
    }}

    // Weaver filaments. Drawn AFTER shading and in a body tone, never in
    // outline: a one-cell-thick feature has no interior, so if it were shaded
    // it would come out entirely as #08090A and vanish against the ground.
    if g.archetype == Archetype::Weaver && s >= 3 {
        let reach: isize = if s >= 5 { 5 } else { 3 };
        for k in 0..3usize {
            let y = top + 3 + k * 4;
            if y > bot { break; }
            let x = 11 - hwv(&hw, rows, (y - top) as isize, 2);
            for d in 1..=reach {
                let fy = (y as isize) + (d >> 1);
                let tone = if d == reach { SHADOW } else { BODY };
                f.set(x - d, fy, tone);
                f.set(23 - (x - d), fy, tone);
            }
            if s >= 5 {
                let fy = (y as isize) + (reach >> 1);
                f.set(x - reach, fy, ACC);
                f.set(23 - (x - reach), fy, ACC);
            }
        }
    }

    // 3 — eye row: the WIDEST row in the upper body, wherever the profile puts
    // the head. A fixed fraction lands the eyes on the neck of a pinched
    // profile, where they overwrite the outline and break the silhouette.
    let lim = ((rows + 1) / 2).max(2);
    let mut eyi = 1usize;
    for i in 1..lim.min(rows) { if hw[i] > hw[eyi] { eyi = i; } }
    let ey = (top + eyi) as isize;
    let ehw = hw[eyi] as isize;
    f.eye_row = ey as usize;
    let wide = ehw >= 5;
    let ew: isize = if wide { 2 } else { 1 };
    let off = ew.max((ehw - 1 - ew).min(5));
    let (eyl, eyr) = (11 - off - 1, 12 + off + 1);

    // 4 — markings. Body tones only, below the eye line, never in the eye box,
    // always mirrored.
    if s >= 2 {
        let n = (s - 1) as isize;
        let mut r2 = crate::genome::Rng::new(g.detail ^ (g.marking as u32).wrapping_mul(2654435761));
        let my = (top + rnd_div(rows as u32 * 62, 100) as usize) as isize;

        let mark = |f: &mut Field, px: isize, py: isize| {
            if py <= ey + 1 { return; }
            if py >= ey - 1 && py <= ey + 2 && px >= eyl && px <= eyr { return; }
            let a = f.at(px, py);
            if a == BODY || a == LIGHT || a == SHADOW { f.set(px, py, ACC); }
            let b = f.at(23 - px, py);
            if b == BODY || b == LIGHT || b == SHADOW { f.set(23 - px, py, ACC); }
        };

        match g.marking {
            0 => { let w = (hwv(&hw, rows, my - top as isize, 3) - 2).max(1);
                   for d in 0..w { mark(&mut f, 11 - d, my); if s >= 4 { mark(&mut f, 11 - d, my + 1); } } }
            1 => { for k in 0..n { let y = ey + 2 + k * 3;
                       let w = hwv(&hw, rows, y - top as isize, 2);
                       mark(&mut f, 12 - w, y); mark(&mut f, 12 - w, y + 1); } }
            2 => { for k in 0..n { let y = ey + 2 + k * 2;
                       let mut d = 1isize; while d < 7 { mark(&mut f, 11 - d, y); d += 2; } } }
            3 => { for k in 0..n + 2 { mark(&mut f, 11, my - k); } }
            _ => { let span = ((bot as isize) - ey - 1).max(1) as u32;
                   for _ in 0..n * 2 {
                       let x = 6 + (r2.next() % 6) as isize;
                       let y = ey + 2 + (r2.next() % span) as isize;
                       mark(&mut f, x, y); mark(&mut f, x, y + 1); } }
        }
    }

    // 5 — crest, stage 4+. Anchored to the real top row.
    if s >= 4 && g.crest > 0 {
        let cy = top as isize - 1;
        let cw = hw[0] as isize;
        match g.crest {
            1 => { f.set(11, cy, ACC); f.set(12, cy, ACC);
                   f.set(11, cy - 1, BODY); f.set(12, cy - 1, BODY); }
            2 => { f.set(11 - cw, cy, BODY); f.set(12 + cw, cy, BODY);
                   f.set(11 - cw - 1, cy - 1, ACC); f.set(12 + cw + 1, cy - 1, ACC); }
            _ => { f.set(11, cy, ACC); f.set(12, cy, ACC);
                   f.set(8, cy, BODY); f.set(15, cy, BODY);
                   f.set(6, cy + 1, SHADOW); f.set(17, cy + 1, SHADOW); }
        }
    }

    // 6 — mantle, stage 3+. Anchored to the real bottom row and ramped
    // BODY -> SHADOW so it fades into the ground instead of being drawn in an
    // outline tone that is invisible on it.
    if s >= 3 && g.mantle > 0 {
        let by = bot as isize + 1;
        match g.mantle {
            1 => for d in 0..3 { let t = if d < 1 { BODY } else { SHADOW };
                                 f.set(11, by + d, t); f.set(12, by + d, t); },
            2 => for d in 0..3 { f.set(11 - d * 2, by, BODY); f.set(12 + d * 2, by, BODY);
                                 if d < 2 { f.set(11 - d * 2, by + 1, SHADOW);
                                            f.set(12 + d * 2, by + 1, SHADOW); } },
            _ => for d in 0..4 { let t = if d < 2 { BODY } else { SHADOW };
                                 f.set(11 - d, by + d, t); f.set(12 + d, by + d, t); },
        }
    }

    // 7 — eyes, drawn last and on top of everything.
    let lit = s >= 2;
    let eye = |f: &mut Field, cx: isize, w: isize, h: isize| {
        for dy in 0..h { for dx in 0..w { f.set(cx + dx, ey + dy, ACC); } }
        if lit { f.set(cx, ey, LITE); }
    };
    if s <= 1 { f.set(11 - off, ey, ACC); f.set(12 + off, ey, ACC); }
    else { match g.eye {
        0 => { eye(&mut f, 11 - off, ew, 2); eye(&mut f, 12 + off - ew + 1, ew, 2); }
        1 => { eye(&mut f, 11 - off, ew, 1); eye(&mut f, 12 + off - ew + 1, ew, 1); }
        2 => { eye(&mut f, 11 - off, 1, 2); eye(&mut f, 12 + off, 1, 2); }
        _ => { eye(&mut f, 11 - off, 1, 1); eye(&mut f, 12 + off, 1, 1);
               if s >= 4 { f.set(11, ey + 2, ACC); f.set(12, ey + 2, ACC); } }
    }}

    // 8 — accumulated system influence. Earned and stored, never live jitter.
    if inf.luminance {
        for y in 0..G as isize { for x in 0..12isize {
            if f.at(x, y) == ACC && f.at(x, y - 1) == LIGHT && (x + y) % 2 == 0 {
                f.set(x, y - 1, LITE); f.set(23 - x, y - 1, LITE);
            }
        }}
    }
    if inf.crystal {
        let mut y = top;
        while y < top + rows { let w = hwv(&hw, rows, (y - top) as isize, 2);
            if f.at(12 - w + 1, y as isize) != EMPTY {
                f.set(12 - w + 1, y as isize, LIGHT); f.set(11 + w - 1, y as isize, LIGHT); }
            y += 3; }
    }
    if inf.flow {
        let by2 = bot as isize - 1;
        for d in 0..3isize { if f.at(11 - d - 1, by2 - d) != EMPTY {
            f.set(11 - d - 1, by2 - d, ACC); f.set(12 + d + 1, by2 - d, ACC); } }
    }
    if inf.structure {
        let mut y = top + 2;
        while y + 2 < top + rows {
            let mut d = 2isize;
            while d < 6 { if f.at(11 - d, y as isize) != EMPTY { f.set(11 - d, y as isize, LIGHT); }
                          if f.at(12 + d, y as isize) != EMPTY { f.set(12 + d, y as isize, LIGHT); }
                          d += 3; }
            y += 4; }
    }
    if inf.maturity && f.at(11, top as isize + 1) != EMPTY {
        f.set(11, top as isize + 1, LITE); f.set(12, top as isize + 1, LITE);
    }

    f
}

// ─────────────────────────────────────────────────────────────────────────
// DRAW CALLS
// ─────────────────────────────────────────────────────────────────────────

/// One run of identical cells in a row — a single `sys_gpu_fill_rect`, or one
/// span in the CPU blit into the Entity surface's ARGB buffer.
#[derive(Clone, Copy)]
pub struct FillRect { pub x: u16, pub y: u16, pub w: u16, pub argb: u32 }

/// Horizontal run-length pass. This is the whole renderer: a pixel creature is
/// a list of filled rectangles, and `sys_gpu_fill_rect` is a solid BLT. A
/// mature Entity comes to roughly 80-120 runs.
pub fn runs(f: &Field, cell: u16, out: &mut [FillRect]) -> usize {
    let mut n = 0usize;
    for y in 0..G {
        let mut x = 0usize;
        while x < G {
            let v = f.cells[y * G + x];
            if v == EMPTY { x += 1; continue; }
            let mut w = 1usize;
            while x + w < G && f.cells[y * G + x + w] == v { w += 1; }
            if n < out.len() {
                out[n] = FillRect {
                    x: (x as u16) * cell, y: (y as u16) * cell,
                    w: (w as u16) * cell, argb: PALETTE[v as usize],
                };
                n += 1;
            }
            x += w;
        }
    }
    n
}

/// Rasterise straight into a 32-bit ARGB surface. `pitch_px` must satisfy
/// `pitch_px * 4 % 64 == 0` for `sys_gpu_composite` to take the buffer without
/// falling back to CPU compositing — i.e. a multiple of 16. At cell=4 the
/// creature is 96 px wide, and 96 * 4 = 384 = 6 * 64.
pub fn blit(f: &Field, cell: usize, dst: &mut [u32], pitch_px: usize) {
    for y in 0..G { for x in 0..G {
        let v = f.cells[y * G + x];
        if v == EMPTY { continue; }
        let argb = PALETTE[v as usize];
        for dy in 0..cell {
            let row = (y * cell + dy) * pitch_px;
            for dx in 0..cell {
                let i = row + x * cell + dx;
                if i < dst.len() { dst[i] = argb; }
            }
        }
    }}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two JavaScript idioms this file leans on, pinned directly.
    ///
    /// Both are easy to "tidy" into something that reads better and means something else, and
    /// neither failure would be visible: the creature stays a creature, it is just no longer the one
    /// the seed describes, and nobody has a previous copy of their own Entity to compare against.
    #[test]
    fn hw_lookup_matches_javascript_truthiness_and_bounds() {
        let mut hw = [0u8; 20];
        hw[..4].copy_from_slice(&[3, 0, 5, 1]);
        let rows = 4;

        // `hw[i] || d` — a present, non-zero value wins.
        assert_eq!(hwv(&hw, rows, 0, 2), 3);
        assert_eq!(hwv(&hw, rows, 2, 2), 5);
        assert_eq!(hwv(&hw, rows, 3, 2), 1, "1 is truthy in JS; only 0 is not");

        // A stored ZERO is falsey, so the default substitutes. `max(d)` would give 2 here too —
        // but would give 2 for index 3 as well, which is why `nz` is not `max`.
        assert_eq!(hwv(&hw, rows, 1, 2), 2);

        // Off either end is `undefined || d`. NOT the nearest row: index 9 must not read as 1.
        assert_eq!(hwv(&hw, rows, 9, 2), 2, "past the end must fall back, not clamp");
        assert_eq!(hwv(&hw, rows, -1, 2), 2);
        // And the row count is the bound, not the array length — `hw` is a fixed 20 with slack.
        assert_eq!(hwv(&hw, rows, 4, 7), 7);
    }

    /// `Math.round(a/b)` for positives, in integers.
    #[test]
    fn rounding_is_half_up_and_never_truncates() {
        assert_eq!(rnd_div(1, 2), 1, "exactly a half rounds up");
        assert_eq!(rnd_div(1, 3), 0);
        assert_eq!(rnd_div(2, 3), 1);
        // The profile resample: row 7 of 18 over a 17-step source.
        assert_eq!(rnd_div(7 * 17, 17), 7);
        assert_eq!(rnd_div(9 * 17, 14), 11);
    }
}
