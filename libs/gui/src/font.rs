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

/// How many typefaces can be live at once.
///
/// Slot 0 is the embedded DejaVu Sans and is what every existing caller gets. Meridian registers
/// Inter at four weights plus JetBrains Mono into slots 1..=5 — see `nyx_meridian::font`.
///
/// ★ The font BYTES deliberately do not live in this crate. Eight applications link `nyx-gui`, and
/// `include_bytes!`-ing two megabytes of Inter here would add it to every one of them for the
/// benefit of the one binary that draws Meridian. So this is a registry: the engine lives here, the
/// bytes are supplied by whoever needs them.
pub const MAX_FACES: usize = 6;

/// Slot 0: the original embedded DejaVu Sans. Unported apps never name a slot and get this.
pub const FACE_DEFAULT: usize = 0;

static FONTS: [spin::Mutex<Option<FontState>>; MAX_FACES] = [
    spin::Mutex::new(None),
    spin::Mutex::new(None),
    spin::Mutex::new(None),
    spin::Mutex::new(None),
    spin::Mutex::new(None),
    spin::Mutex::new(None),
];

/// Install a typeface into `slot`. Returns false if the slot is out of range or the bytes do not
/// parse as a TrueType face — in which case nothing is installed and the slot keeps whatever was
/// there, so a bad font degrades to the previous one rather than to no text at all.
///
/// Registering the same slot twice replaces the face and drops its glyph cache.
pub fn register_face(slot: usize, ttf: &'static [u8]) -> bool {
    if slot >= MAX_FACES {
        return false;
    }
    match ttf_parser::Face::parse(ttf, 0) {
        Ok(face) => {
            let upem = face.units_per_em() as f32;
            *FONTS[slot].lock() = Some(FontState { face, upem, cache: BTreeMap::new() });
            true
        }
        Err(_) => false,
    }
}

/// True once `slot` holds a usable face. Slot 0 reports true only after something has drawn with
/// it, since DejaVu is loaded lazily on first use.
pub fn face_ready(slot: usize) -> bool {
    slot < MAX_FACES && FONTS[slot].lock().is_some()
}

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

/// Lazily parse slot 0's embedded face on first use. Returns false if the embedded TTF fails to
/// parse (should never happen — it's a known-good file — but keeps callers on a graceful path).
///
/// Only slot 0 self-loads. Every other slot must be filled by `register_face`, because this crate
/// does not know what is supposed to be in them.
fn ensure_loaded(slot: usize, cell: &mut Option<FontState>) -> bool {
    if cell.is_some() {
        return true;
    }
    if slot != FACE_DEFAULT {
        return false;
    }
    match ttf_parser::Face::parse(FONT_TTF, 0) {
        Ok(face) => {
            let upem = face.units_per_em() as f32;
            *cell = Some(FontState { face, upem, cache: BTreeMap::new() });
            true
        }
        Err(_) => false,
    }
}

/// Run `f` with the cached glyph for (`c`, `px`) from slot 0. The original entry point; every
/// existing caller keeps drawing in DejaVu with no change.
pub fn with_glyph<R>(c: char, px: usize, f: impl FnOnce(&Glyph) -> R) -> Option<R> {
    with_glyph_face(FACE_DEFAULT, c, px, f)
}

/// Run `f` with the cached glyph for (`slot`, `c`, `px`), rasterizing on miss. This is the hot path
/// (`canvas.draw_char` / `text_width`) — no per-call clone.
///
/// An unregistered or out-of-range slot falls back to slot 0 rather than returning None. Drawing
/// the right words in the wrong typeface is a cosmetic problem; drawing nothing is a blank desktop,
/// and on a machine with no QEMU that is a power cycle to diagnose.
pub fn with_glyph_face<R>(slot: usize, c: char, px: usize, f: impl FnOnce(&Glyph) -> R) -> Option<R> {
    let slot = if slot < MAX_FACES { slot } else { FACE_DEFAULT };
    let mut guard = FONTS[slot].lock();
    if !ensure_loaded(slot, &mut guard) {
        // ★ Release the lock BEFORE recursing. Holding slot N's guard while taking slot 0's is a
        // lock ordering waiting to bite, and this path is reachable from the compositor.
        drop(guard);
        return if slot == FACE_DEFAULT {
            None
        } else {
            with_glyph_face(FACE_DEFAULT, c, px, f)
        };
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
    advance_px(c, px_for(scale))
}

/// Ascent (pixels above the baseline) at `scale`.
pub fn ascent(scale: usize) -> usize {
    ascent_px(px_for(scale))
}

/// Full line height (pixels) at `scale` = (ascender - descender + line_gap) * scale.
pub fn line_height(scale: usize) -> usize {
    line_height_px(px_for(scale))
}

// --- Pixel-exact variants -------------------------------------------------------------------
//
// The `scale` API can only express integer multiples of BASE_PX (14), which is fine for UI chrome
// and wrong for a browser: CSS asks for 32px h1 and 24px h2, and both would round to scale 2 and
// render identically. The rasterizer has always worked in pixels (`with_glyph` takes px) — these
// just expose that, and the scale functions above are now thin wrappers so there is one code path.

/// Horizontal advance (pixels) of a single glyph rasterized at `px` height.
pub fn advance_px(c: char, px: usize) -> usize {
    advance_px_face(FACE_DEFAULT, c, px)
}

/// Horizontal advance (pixels) of a glyph from `slot` at `px` height.
pub fn advance_px_face(slot: usize, c: char, px: usize) -> usize {
    let px = px.max(1);
    with_glyph_face(slot, c, px, |g| g.advance).unwrap_or(px / 2)
}

/// Ascent (pixels above the baseline) at `px` height.
pub fn ascent_px(px: usize) -> usize {
    ascent_px_face(FACE_DEFAULT, px)
}

/// Ascent for `slot`. Faces disagree: Inter and DejaVu have different ascenders at the same pixel
/// size, so a caller that mixes faces on one line must position by the face it is drawing, not by
/// the default.
pub fn ascent_px_face(slot: usize, px: usize) -> usize {
    let px = px.max(1);
    let slot = if slot < MAX_FACES { slot } else { FACE_DEFAULT };
    let mut guard = FONTS[slot].lock();
    if !ensure_loaded(slot, &mut guard) {
        drop(guard);
        return if slot == FACE_DEFAULT { (px * 3) / 4 } else { ascent_px_face(FACE_DEFAULT, px) };
    }
    let st = guard.as_ref().unwrap();
    let s = px as f32 / st.upem;
    libm::ceilf(st.face.ascender() as f32 * s) as usize
}

/// Full line height (pixels) at `px` height = (ascender - descender + line_gap) scaled.
pub fn line_height_px(px: usize) -> usize {
    line_height_px_face(FACE_DEFAULT, px)
}

/// Line height for `slot`.
///
/// ⚠️ Meridian does NOT use this for layout. The design specifies its own line height per style
/// (40/44, 28/34, 15/22 …), and those are tighter than the font's natural metric — which is what
/// makes the type read as designed rather than as a default. This remains for callers that want the
/// face's own rhythm.
pub fn line_height_px_face(slot: usize, px: usize) -> usize {
    let px = px.max(1);
    let slot = if slot < MAX_FACES { slot } else { FACE_DEFAULT };
    let mut guard = FONTS[slot].lock();
    if !ensure_loaded(slot, &mut guard) {
        drop(guard);
        return if slot == FACE_DEFAULT { px } else { line_height_px_face(FACE_DEFAULT, px) };
    }
    let st = guard.as_ref().unwrap();
    let s = px as f32 / st.upem;
    let lh = (st.face.ascender() as i32 - st.face.descender() as i32 + st.face.line_gap() as i32) as f32 * s;
    libm::ceilf(lh) as usize
}

/// Advance of a whole string at `px` height. Newlines are not handled — callers doing their own
/// line breaking (the browser) never pass them.
pub fn text_width_px(text: &str, px: usize) -> usize {
    text.chars().map(|c| advance_px(c, px)).sum()
}

/// Advance of a whole string in `slot` at `px` height.
pub fn text_width_px_face(slot: usize, text: &str, px: usize) -> usize {
    text.chars().map(|c| advance_px_face(slot, c, px)).sum()
}
