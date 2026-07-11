// libs/gui/src/font.rs
//
// F1: proportional, antialiased TrueType font. Replaces the fixed 9x16 `noto-sans-mono-bitmap`.
// DejaVuSans.ttf is embedded and parsed once by `ttf-parser`; each glyph's outline is filled to a
// grayscale COVERAGE bitmap (0..255) by `ab_glyph_rasterizer`, cached by (char, pixel-height). The
// renderer (canvas.rs) blends by coverage for real AA, and advances the pen by each glyph's own
// advance (proportional). Metrics (ascent/descent/line height, per-glyph advance/bearing) come from
// the face, so layout is no longer monospace.
//
// no_std + alloc. A single global `FONT` (spin::Mutex) holds the parsed face + a glyph cache; the hot
// path is `with_glyph`, which rasterizes-on-miss then hands a borrow to a closure (no per-draw clone).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use ab_glyph_rasterizer::{point, Point, Rasterizer};

/// Embedded proportional typeface (DejaVu Sans, OFL). ~740 KB — smaller than assets the kernel already
/// embeds (iwlwifi.ucode is 1.3 MB). Same `include_bytes!` convention.
static FONT_TTF: &[u8] = include_bytes!("../DejaVuSans.ttf");

/// Back-compat fallback constants. New code uses the metric fns below; these remain only so any
/// not-yet-migrated caller referencing `font::CHAR_WIDTH`/`CHAR_HEIGHT` still compiles. They are the
/// APPROXIMATE monospace cell of the OLD font — do NOT use for proportional layout.
pub const CHAR_WIDTH: usize = 9;
pub const CHAR_HEIGHT: usize = 16;

/// Target glyph pixel height for `scale == 1`. DejaVu upem=2048, ascender≈1901, descender≈-483 →
/// line_height ≈ 1.16*px, so px=14 gives ≈16px lines (matches the old 16px rhythm sysmon/terminal use).
/// Tune on HW if body text reads too small/large.
pub const BASE_PX: usize = 14;

/// Pixel height for a given integer `scale` (the canvas/print_str scale arg).
#[inline]
pub fn px_for(scale: usize) -> usize {
    BASE_PX * scale.max(1)
}

/// One rasterized glyph: `cov` is a `w*h` grayscale coverage bitmap (0..255, row-major). `bearing_x`
/// is the left offset from the pen origin; `bearing_y` is how many rows the glyph top sits ABOVE the
/// baseline. `advance` is how far to move the pen after drawing. Whitespace glyphs have empty `cov`
/// (w=h=0) and only an advance.
pub struct Glyph {
    pub cov: Vec<u8>,
    pub w: usize,
    pub h: usize,
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub advance: usize,
}

struct FontState {
    face: ttf_parser::Face<'static>,
    upem: f32,
    /// key = (char, pixel-height). Glyphs are cached across frames; UI text reuses a small set.
    cache: BTreeMap<(char, u32), Glyph>,
}

// The face borrows only 'static font bytes and parsed tables (plain slices) — safe to share.
unsafe impl Send for FontState {}

static FONT: spin::Mutex<Option<FontState>> = spin::Mutex::new(None);

/// Fills a `ttf-parser` glyph outline into an `ab_glyph_rasterizer`, transforming font units (y-up,
/// origin at the glyph's bbox) into bitmap pixels (y-down, origin at the top-left of the `w*h` box).
struct Outliner {
    rast: Rasterizer,
    scale: f32,
    x_min: f32,
    y_max: f32,
    last: Point,
    start: Point,
}

impl Outliner {
    #[inline]
    fn tx(&self, x: f32, y: f32) -> Point {
        // x grows right; y flips so the glyph top (y_max) maps to row 0.
        point((x - self.x_min) * self.scale, (self.y_max - y) * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for Outliner {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.tx(x, y);
        self.last = p;
        self.start = p;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.tx(x, y);
        self.rast.draw_line(self.last, p);
        self.last = p;
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let c = self.tx(cx, cy);
        let p = self.tx(x, y);
        self.rast.draw_quad(self.last, c, p);
        self.last = p;
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        let c1 = self.tx(c1x, c1y);
        let c2 = self.tx(c2x, c2y);
        let p = self.tx(x, y);
        self.rast.draw_cubic(self.last, c1, c2, p);
        self.last = p;
    }
    fn close(&mut self) {
        self.rast.draw_line(self.last, self.start);
        self.last = self.start;
    }
}

/// Rasterize glyph `c` at pixel height `px` (no cache). Returns a `Glyph`; whitespace / no-outline
/// glyphs yield an empty bitmap with only an advance. Returns None only if the char has no glyph AND
/// no advance (never for normal text).
fn rasterize(face: &ttf_parser::Face<'static>, upem: f32, c: char, px: f32) -> Option<Glyph> {
    let gid = face.glyph_index(c).or_else(|| face.glyph_index('?'))?;
    let scale = px / upem;
    let advance = libm::roundf(face.glyph_hor_advance(gid).unwrap_or(0) as f32 * scale) as usize;

    // Outline bbox (font units). Absent for whitespace → advance-only glyph.
    let mut probe = BBox::default();
    let bbox = face.outline_glyph(gid, &mut probe);
    let (fx_min, fy_min, fx_max, fy_max) = match bbox {
        Some(r) => (r.x_min as f32, r.y_min as f32, r.x_max as f32, r.y_max as f32),
        None => {
            return Some(Glyph { cov: Vec::new(), w: 0, h: 0, bearing_x: 0, bearing_y: 0, advance });
        }
    };
    let _ = fy_min;

    // Bitmap size (+1px pad so AA at the far edge isn't clipped).
    let w = (libm::ceilf((fx_max - fx_min) * scale) as usize) + 1;
    let h = (libm::ceilf((fy_max - fy_min) * scale) as usize) + 1;
    let bearing_x = libm::floorf(fx_min * scale) as i32;
    let bearing_y = libm::ceilf(fy_max * scale) as i32;

    let mut outliner = Outliner {
        rast: Rasterizer::new(w, h),
        scale,
        x_min: fx_min,
        y_max: fy_max,
        last: point(0.0, 0.0),
        start: point(0.0, 0.0),
    };
    // Second pass: actually feed the outline into the rasterizer.
    face.outline_glyph(gid, &mut outliner);

    let mut cov = vec![0u8; w * h];
    outliner.rast.for_each_pixel(|idx, alpha| {
        // alpha is 0.0..1.0 coverage.
        let a = (alpha * 255.0 + 0.5) as u32;
        if idx < cov.len() {
            cov[idx] = if a > 255 { 255 } else { a as u8 };
        }
    });

    Some(Glyph { cov, w, h, bearing_x, bearing_y, advance })
}

/// A no-op OutlineBuilder used only to trigger `outline_glyph`'s bbox return (first pass).
#[derive(Default)]
struct BBox;
impl ttf_parser::OutlineBuilder for BBox {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

/// Lazily parse the face on first use. Returns false if the embedded TTF fails to parse (should
/// never happen — it's a known-good file — but keeps callers on a graceful path).
fn ensure_loaded(slot: &mut Option<FontState>) -> bool {
    if slot.is_some() {
        return true;
    }
    match ttf_parser::Face::parse(FONT_TTF, 0) {
        Ok(face) => {
            let upem = face.units_per_em() as f32;
            *slot = Some(FontState { face, upem, cache: BTreeMap::new() });
            true
        }
        Err(_) => false,
    }
}

/// Run `f` with the cached glyph for (`c`, `px`), rasterizing on miss. Returns None if the font
/// failed to load. This is the hot path (canvas.draw_char / text_width) — no per-call clone.
pub fn with_glyph<R>(c: char, px: usize, f: impl FnOnce(&Glyph) -> R) -> Option<R> {
    let mut guard = FONT.lock();
    if !ensure_loaded(&mut guard) {
        return None;
    }
    let st = guard.as_mut().unwrap();
    let key = (c, px as u32);
    if !st.cache.contains_key(&key) {
        // Split borrows: read face/upem, then insert into cache.
        let g = rasterize(&st.face, st.upem, c, px as f32);
        if let Some(g) = g {
            st.cache.insert(key, g);
        } else {
            // No glyph and no advance: cache an empty zero-advance glyph so we don't retry every frame.
            st.cache.insert(key, Glyph { cov: Vec::new(), w: 0, h: 0, bearing_x: 0, bearing_y: 0, advance: 0 });
        }
    }
    let g = st.cache.get(&key).unwrap();
    Some(f(g))
}

/// Horizontal advance (pixels) of a single glyph at `scale`.
pub fn advance(c: char, scale: usize) -> usize {
    let px = px_for(scale);
    with_glyph(c, px, |g| g.advance).unwrap_or(CHAR_WIDTH * scale)
}

/// Ascent (pixels above the baseline) at `scale`.
pub fn ascent(scale: usize) -> usize {
    let mut guard = FONT.lock();
    if !ensure_loaded(&mut guard) {
        return (CHAR_HEIGHT * scale * 3) / 4;
    }
    let st = guard.as_ref().unwrap();
    let s = px_for(scale) as f32 / st.upem;
    libm::ceilf(st.face.ascender() as f32 * s) as usize
}

/// Full line height (pixels) at `scale` = (ascender - descender + line_gap) * scale.
pub fn line_height(scale: usize) -> usize {
    let mut guard = FONT.lock();
    if !ensure_loaded(&mut guard) {
        return CHAR_HEIGHT * scale;
    }
    let st = guard.as_ref().unwrap();
    let s = px_for(scale) as f32 / st.upem;
    let lh = (st.face.ascender() as i32 - st.face.descender() as i32 + st.face.line_gap() as i32) as f32 * s;
    libm::ceilf(lh) as usize
}
