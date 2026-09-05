//! The Meridian glyph and icon atlas.
//!
//! ## What this replaces
//!
//! `nyx_gui::gpu_text::GpuFont` builds ONE atlas: 14px, printable ASCII, one typeface, no icons. It
//! is what makes GPU text possible at all, and it is also the single thing blocking this design —
//! Meridian sets nine styles across five faces, and draws icons through the same path because the
//! GPU text shader samples one coverage channel and that is exactly what an icon is.
//!
//! So icons are not a separate system here. An icon is a cell in the same atlas, drawn by the same
//! `sys_gpu_draw_text` batch, tinted the same way. The design assumes this ("icons and glyphs share
//! the same coverage atlas") and it is what keeps the dock at eight quads.
//!
//! ## Why packing is separated from allocation
//!
//! [`plan`] computes the whole layout — every cell's position, every metric — as a pure function
//! over the style table. [`Atlas::build`] then allocates the SHM, stamps coverage into it and maps
//! it to a GVA.
//!
//! That split exists so the packing can be tested on the development host. A cell that overlaps its
//! neighbour, a row that runs past the surface, a pitch that loses its 64-byte alignment — these
//! are silent on hardware: the atlas builds, the GPU samples it, and text comes out with fragments
//! of other letters bleeding into it. Diagnosing that costs a power cycle. Proving it with
//! `cargo test` costs nothing, and this machine has no QEMU.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use nyx_api::{sys_create_shm, sys_gpu_map_shm, sys_map_shm, GlyphQuad};
use nyx_gui::font;

use crate::charset::Charset;
use crate::font::slot_of;
use crate::icons_gen;
use crate::tokens::{Style, ALL_STYLES, D1, D2, LB};

/// GVA the atlas is mapped to.
///
/// The same slot `nyx_gui::gpu_text` uses — clear of window GVAs (`0x2000_0000 + n*0x0100_0000`),
/// the SSAA render target, the GL backbuffer and the cursor. Only one shell runs at a time, so the
/// two never contend; the shell simply builds this atlas instead of that one.
pub const ATLAS_GVA: u32 = 0x6400_0000;

/// Atlas width in pixels. 1024 × 4 bytes = a 4096-byte pitch, which satisfies
/// `sys_gpu_draw_text`'s 64-byte alignment requirement with room to spare.
///
/// Wider than `gpu_text`'s 512 because the 40px display shelf alone is wider than 512 would pack
/// comfortably, and a shelf-packer wastes the tail of every row it cannot fill.
pub const ATLAS_W: usize = 1024;

/// Transparent gutter between cells, so sampling one cell can never pick up a neighbour's edge.
const GUTTER: usize = 1;

/// Where a planned cell's pixels come from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// A rasterized glyph: which face slot, which character, at what pixel height.
    Glyph { slot: u8, ch: char, px: u16 },
    /// An icon: its index in [`icons_gen::ICONS`] and in [`icons_gen::SIZES`].
    Icon { icon: u16, size: u8 },
}

/// One cell's placement and metrics.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub src: Source,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Left offset from the pen origin (glyphs) or from the icon box (icons).
    pub bx: i32,
    /// Rows the ink sits ABOVE the baseline (glyphs), or the top offset inside the box (icons).
    pub by: i32,
    /// How far to move the pen after drawing. Zero for icons.
    pub adv: u32,
}

/// A complete atlas layout, before any memory is touched.
pub struct Plan {
    pub w: usize,
    pub h: usize,
    pub pitch: usize,
    pub cells: Vec<Cell>,
    /// (style index in [`ALL_STYLES`], char) → index into `cells`.
    pub text: BTreeMap<(u8, char), u32>,
    /// (icon index, size index) → index into `cells`.
    pub icons: BTreeMap<(u16, u8), u32>,
}

impl Plan {
    /// Total bytes the atlas surface needs.
    pub fn bytes(&self) -> usize {
        self.pitch * self.h
    }

    /// The cell for (`style`, `ch`), or None if this style does not carry that character.
    pub fn cell(&self, style: Style, ch: char) -> Option<&Cell> {
        let si = style_index(style)?;
        self.cells.get(*self.text.get(&(si, ch))? as usize)
    }

    /// Pixel width of `text` in `style`. Identical arithmetic to [`Atlas::measure`] — both call
    /// [`measure_cells`] — so measuring against a plan is measuring the real thing.
    pub fn measure(&self, style: Style, text: &str) -> usize {
        measure_cells(style, text, |ch| self.cell(style, ch))
    }
}

/// Index of `style` in [`ALL_STYLES`], which is how the cell maps are keyed.
fn style_index(style: Style) -> Option<u8> {
    ALL_STYLES.iter().position(|s| *s == style).map(|i| i as u8)
}

/// Width of `text` in `style`, given a way to look up cells.
///
/// Shared by [`Plan::measure`] and [`Atlas::measure`] so the two can never disagree — which matters
/// because `measure` is what centres and right-aligns everything in the shell, and a measure that
/// drifts from the layout puts every centred label subtly off with nothing to point at.
///
/// Tracking is applied BETWEEN characters, not after the last one. At `lb`'s 0.115em the difference
/// is a visible pixel on every uppercase label.
pub fn measure_cells<'a>(
    style: Style,
    text: &str,
    lookup: impl Fn(char) -> Option<&'a Cell>,
) -> usize {
    let track = style.tracking_px();
    let mut w = 0i32;
    let mut n = 0i32;
    for ch in transform(style, text) {
        w += lookup(ch).map(|c| c.adv as i32).unwrap_or(0);
        n += 1;
    }
    if n > 1 {
        w += track * (n - 1);
    }
    w.max(0) as usize
}

/// Which charset a style packs.
///
/// Only `d1` is restricted, and only because of what it is actually used for: the System Monitor's
/// four numbers. The design puts the unit in a *separate 15px span* (`.mc .v .u`), so the 40px run
/// really is digits and punctuation.
///
/// `d2` is NOT restricted, despite being a display size. It sets section headings — "Applications",
/// "Documents" — which are Title Case and need lowercase. That was worth getting wrong once to
/// learn: "display size" and "digits only" are not the same claim, and the design says the latter
/// about the largest shelf specifically.
///
/// `lb` is uppercase-only by construction ([`Style::upper`]), so its lowercase cells would be
/// unreachable rather than merely unused.
fn charset_for(style: Style) -> Charset {
    if style == D1 {
        Charset::Display
    } else if style == LB {
        Charset::Label
    } else {
        Charset::Full
    }
}

/// Compute the atlas layout: every glyph of every style, then every icon at every size.
///
/// Two passes, and the split matters:
///
/// 1. **Collect** every cell with its metrics, in a canonical order, building the lookup maps as it
///    goes. The maps are produced here rather than re-derived later, because re-walking the style
///    table a second time to rebuild the same indices is exactly how a map and its vector drift
///    apart — and the symptom would be text rendering as the wrong letters.
///
/// 2. **Place**, shelf-packing in order of DESCENDING HEIGHT. Shelf packing wastes the difference
///    between the tallest cell on a shelf and every other cell on it, so feeding it a mixed-height
///    stream leaves a 40px shelf half-full of 11px glyphs. Sorting first makes each shelf nearly
///    uniform. Measured: occupancy 51% → 75%, and the surface 1028 KiB → 708 KiB. That is a third
///    of a megabyte of GGTT-mapped memory that was previously transparent padding.
pub fn plan() -> Plan {
    let mut cells: Vec<Cell> = Vec::new();
    let mut text = BTreeMap::new();
    let mut icons = BTreeMap::new();

    // --- pass 1: collect ---
    for (si, style) in ALL_STYLES.iter().enumerate() {
        let slot = slot_of(style.face);
        let px = style.size();
        charset_for(*style).for_each(|ch| {
            let m = font::with_glyph_face(slot, ch, px, |g| {
                (g.w, g.h, g.bearing_x, g.bearing_y, g.advance)
            });
            // None means even the DejaVu fallback is unavailable, so nothing can be drawn at all.
            // Recording an advance-only cell keeps the layout arithmetic sane rather than
            // collapsing every string to zero width.
            let (w, h, bx, by, adv) = m.unwrap_or((0, 0, 0, 0, px / 2));
            text.insert((si as u8, ch), cells.len() as u32);
            cells.push(Cell {
                src: Source::Glyph { slot: slot as u8, ch, px: px as u16 },
                // Whitespace and no-outline glyphs keep a cell — with an advance and no pixels — so
                // the layout path finds their advance without a second lookup.
                x: 0, y: 0, w: w as u32, h: h as u32, bx, by, adv: adv as u32,
            });
        });
    }
    // Only the sizes this panel's interface scale actually draws. The generated table carries every
    // size for every scale step, because it is `.rodata` and costs its bytes once; the atlas is a
    // GPU surface sampled every frame, and packing all sixteen would quadruple it to hold twelve
    // sizes nothing will ask for.
    let active = crate::scale::active_icon_sizes();
    for (ii, set) in icons_gen::ICONS.iter().enumerate() {
        for (szi, icon) in set.sizes.iter().enumerate() {
            if icon.w == 0 || icon.h == 0 || !active.contains(&icons_gen::SIZES[szi]) {
                continue;
            }
            icons.insert((ii as u16, szi as u8), cells.len() as u32);
            cells.push(Cell {
                src: Source::Icon { icon: ii as u16, size: szi as u8 },
                x: 0, y: 0, w: icon.w as u32, h: icon.h as u32,
                bx: icon.off_x as i32,
                by: icon.off_y as i32,
                adv: 0,
            });
        }
    }

    // --- pass 2: place, tallest first ---
    let mut order: Vec<u32> = (0..cells.len() as u32)
        .filter(|&i| cells[i as usize].w != 0 && cells[i as usize].h != 0)
        .collect();
    // Height first, then width, then index — the index breaks ties so the layout is reproducible
    // rather than depending on the sort's stability guarantees.
    order.sort_by_key(|&i| {
        let c = &cells[i as usize];
        (core::cmp::Reverse(c.h), core::cmp::Reverse(c.w), i)
    });

    let mut cx = GUTTER;
    let mut cy = GUTTER;
    let mut shelf_h = 0usize;
    for i in order {
        let (w, h) = {
            let c = &cells[i as usize];
            (c.w as usize, c.h as usize)
        };
        if cx + w + GUTTER > ATLAS_W {
            cx = GUTTER;
            cy += shelf_h + GUTTER;
            shelf_h = 0;
        }
        let c = &mut cells[i as usize];
        c.x = cx as u32;
        c.y = cy as u32;
        cx += w + GUTTER;
        if h > shelf_h {
            shelf_h = h;
        }
    }

    let h = cy + shelf_h + GUTTER;
    Plan { w: ATLAS_W, h, pitch: ATLAS_W * 4, cells, text, icons }
}

/// A built, GPU-mapped atlas.
pub struct Atlas {
    pub gva: u32,
    pub w: u32,
    pub h: u32,
    pub pitch: u32,
    /// (style index in `ALL_STYLES`, char) → index into `cells`.
    text: BTreeMap<(u8, char), u32>,
    /// (icon index, size index) → index into `cells`.
    icons: BTreeMap<(u16, u8), u32>,
    cells: Vec<Cell>,
    /// Characters the shell asked to draw that this atlas does not carry.
    ///
    /// A restricted charset is a real risk: a missing glyph leaves a hole in a word, which reads as
    /// a layout bug rather than a missing character, and on hardware it costs a boot to chase. This
    /// counter turns it into a number the shell can print.
    misses: AtomicU32,
    last_miss: AtomicU32,
}

impl Atlas {
    /// Plan, allocate, rasterize and GPU-map the atlas.
    ///
    /// Returns None if the SHM cannot be created or mapped, in which case the caller keeps using
    /// the CPU text path (`Canvas::print_str`) — slower, but a working desktop.
    ///
    /// Call [`crate::font::register_all`] first, or every glyph will be rasterized from the DejaVu
    /// fallback and the design's typography will be silently absent.
    pub fn build() -> Option<Atlas> {
        let p = plan();
        let shm = sys_create_shm(p.bytes());
        if shm == 0 {
            return None;
        }
        let vaddr = sys_map_shm(shm);
        if vaddr == 0 {
            return None;
        }

        let px_count = p.w * p.h;
        let pixels = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u32, px_count) };
        for v in pixels.iter_mut() {
            *v = 0;
        }

        // Stamp coverage into the alpha channel with white RGB — the format the kernel's text pixel
        // shader expects. It samples alpha as coverage and tints RGB to the per-quad luminance.
        for cell in &p.cells {
            if cell.w == 0 {
                continue;
            }
            match cell.src {
                Source::Glyph { slot, ch, px } => {
                    font::with_glyph_face(slot as usize, ch, px as usize, |g| {
                        blit(pixels, p.w, cell, &g.cov, g.w);
                    });
                }
                Source::Icon { icon, size } => {
                    let ic = &icons_gen::ICONS[icon as usize].sizes[size as usize];
                    blit(pixels, p.w, cell, ic.cov, ic.w as usize);
                }
            }
        }

        sys_gpu_map_shm(shm, ATLAS_GVA);

        Some(Atlas {
            gva: ATLAS_GVA,
            w: p.w as u32,
            h: p.h as u32,
            pitch: p.pitch as u32,
            text: p.text,
            icons: p.icons,
            cells: p.cells,
            misses: AtomicU32::new(0),
            last_miss: AtomicU32::new(0),
        })
    }

    fn cell_for(&self, style: Style, ch: char) -> Option<&Cell> {
        let si = style_index(style)?;
        let i = *self.text.get(&(si, ch))?;
        self.cells.get(i as usize)
    }

    /// Record a character this atlas cannot draw.
    fn miss(&self, ch: char) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.last_miss.store(ch as u32, Ordering::Relaxed);
    }

    /// How many characters have been asked for and not drawn, and the most recent one.
    ///
    /// Worth printing at startup and after the first frame. A non-zero count means a charset needs
    /// widening — and says so in one line instead of as a hole in a word nobody can account for.
    pub fn misses(&self) -> (u32, Option<char>) {
        let n = self.misses.load(Ordering::Relaxed);
        let c = char::from_u32(self.last_miss.load(Ordering::Relaxed));
        (n, if n == 0 { None } else { c })
    }

    /// Pixel width of `text` in `style`, including the style's letter-spacing.
    pub fn measure(&self, style: Style, text: &str) -> usize {
        measure_cells(style, text, |ch| self.cell_for(style, ch))
    }

    /// Lay `text` out at `(x, y)` — where `y` is the TOP of the line box — appending one
    /// [`GlyphQuad`] per visible glyph to `out`.
    ///
    /// The baseline is derived from the face's own ascent rather than the style's line height,
    /// because the two differ: the design's line heights are tighter than the fonts' natural
    /// metrics, and positioning text by the design value would sit it low in its box.
    pub fn layout(&self, style: Style, x: i32, y: i32, text: &str, color: u32, out: &mut Vec<GlyphQuad>) {
        let aw = self.w as f32;
        let ah = self.h as f32;
        let track = style.tracking_px();
        let baseline = y + font::ascent_px_face(slot_of(style.face), style.size()) as i32;
        let mut pen = x;
        for ch in transform(style, text) {
            let cell = match self.cell_for(style, ch) {
                Some(c) => c,
                None => {
                    self.miss(ch);
                    pen += track;
                    continue;
                }
            };
            if cell.w != 0 {
                out.push(GlyphQuad {
                    dst_x: pen + cell.bx,
                    dst_y: baseline - cell.by,
                    dst_w: cell.w,
                    dst_h: cell.h,
                    u0: cell.x as f32 / aw,
                    v0: cell.y as f32 / ah,
                    u1: (cell.x + cell.w) as f32 / aw,
                    v1: (cell.y + cell.h) as f32 / ah,
                    color,
                });
            }
            pen += cell.adv as i32 + track;
        }
    }

    /// One icon quad, positioned by the TOP-LEFT of its `px`-square box.
    ///
    /// The caller positions by the box, not by the ink: the generator crops each icon to its
    /// coverage and records the offset, so a caller never has to know an icon was trimmed. Returns
    /// None for an unknown id or a size the set is not rasterized at — see
    /// [`crate::icons::get`] for why that is not silently rounded to a nearby size.
    pub fn icon(&self, id: &str, px: usize, x: i32, y: i32, color: u32) -> Option<GlyphQuad> {
        let ii = icons_gen::ICONS.iter().position(|s| s.id == id)? as u16;
        let szi = icons_gen::size_index(px)? as u8;
        let cell = self.cells.get(*self.icons.get(&(ii, szi))? as usize)?;
        Some(GlyphQuad {
            dst_x: x + cell.bx,
            dst_y: y + cell.by,
            dst_w: cell.w,
            dst_h: cell.h,
            u0: cell.x as f32 / self.w as f32,
            v0: cell.y as f32 / self.h as f32,
            u1: (cell.x + cell.w) as f32 / self.w as f32,
            v1: (cell.y + cell.h) as f32 / self.h as f32,
            color,
        })
    }
}

/// Break `text` into lines no wider than `max_w`, using `measure` to size a candidate.
///
/// The Entity's detail is the only paragraph in Meridian — everywhere else, text that does not fit
/// is truncated, because a wrapping label is a label that changes the height of its container. Here
/// the surface is *designed* to grow with it, which is why `entity_layout` takes a line count.
///
/// Breaks on spaces only. A single word longer than `max_w` is emitted on its own over-long line
/// rather than split mid-word: hyphenating "recalibrating" at a pixel boundary looks like a bug, and
/// the strings here are English sentences, not paths.
///
/// Takes a closure rather than an `&Atlas` so it can be tested without a GPU mapping — the wrapping
/// logic is the part that goes wrong, and it does not need real font metrics to prove.
pub fn wrap<'a>(text: &'a str, max_w: usize, measure: impl Fn(&str) -> usize) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    if text.is_empty() {
        return out;
    }
    // Byte offsets into `text`: `start` of the current line, `end` of the last word that fit.
    let mut start = 0usize;
    let mut end = 0usize;
    let mut i = 0usize;
    let bytes = text.as_bytes();
    while i <= bytes.len() {
        // Advance to the end of the next word.
        let word_end = match bytes[i..].iter().position(|&b| b == b' ') {
            Some(p) => i + p,
            None => bytes.len(),
        };
        if measure(&text[start..word_end]) <= max_w || end == start {
            // It fits — or it is the first word on the line, which must be emitted regardless or the
            // loop cannot make progress.
            end = word_end;
        } else {
            out.push(&text[start..end]);
            start = end + 1; // skip the space we broke on
            i = start;
            end = start;
            continue;
        }
        if word_end >= bytes.len() {
            break;
        }
        i = word_end + 1;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Apply a style's case transform. `lb` is uppercase by design, and doing it here means no caller
/// has to remember — and means the label shelf never needs lowercase cells.
fn transform(style: Style, text: &str) -> impl Iterator<Item = char> + '_ {
    let upper = style.upper;
    text.chars().flat_map(move |c| {
        // `to_uppercase` yields an iterator because one character can uppercase to several.
        let mut buf = ['\0'; 3];
        let mut n = 0;
        if upper {
            for u in c.to_uppercase() {
                if n < 3 {
                    buf[n] = u;
                    n += 1;
                }
            }
        } else {
            buf[0] = c;
            n = 1;
        }
        (0..n).map(move |i| buf[i])
    })
}

/// Copy one coverage bitmap into the atlas: coverage in alpha, white RGB.
fn blit(dst: &mut [u32], dst_w: usize, cell: &Cell, cov: &[u8], src_w: usize) {
    for row in 0..cell.h as usize {
        let dy = cell.y as usize + row;
        for col in 0..cell.w as usize {
            let c = cov[row * src_w + col];
            if c == 0 {
                continue;
            }
            let dx = cell.x as usize + col;
            let i = dy * dst_w + dx;
            if i < dst.len() {
                dst[i] = ((c as u32) << 24) | 0x00FF_FFFF;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::register_all;
    use crate::tokens::{B1, B2, B3, MONO};

    fn planned() -> Plan {
        register_all();
        plan()
    }

    /// `sys_gpu_draw_text` requires a 64-byte-aligned pitch. A violation does not error — the GPU
    /// samples garbage and the text comes out as noise.
    #[test]
    fn pitch_is_64_byte_aligned() {
        let p = planned();
        assert_eq!(p.pitch % 64, 0, "pitch {} is not 64-byte aligned", p.pitch);
        assert_eq!(p.pitch, p.w * 4);
    }

    /// Every cell must sit inside the surface. A cell that runs past the right edge wraps onto the
    /// next row's start and corrupts whatever is there.
    #[test]
    fn every_cell_is_inside_the_atlas() {
        let p = planned();
        for c in &p.cells {
            if c.w == 0 {
                continue;
            }
            assert!(
                c.x as usize + c.w as usize <= p.w,
                "{:?} runs past the right edge: {} + {} > {}",
                c.src, c.x, c.w, p.w
            );
            assert!(
                c.y as usize + c.h as usize <= p.h,
                "{:?} runs past the bottom: {} + {} > {}",
                c.src, c.y, c.h, p.h
            );
        }
    }

    /// No two cells may share a pixel. This is the failure that puts fragments of one letter inside
    /// another — visible, baffling, and a power cycle to find on hardware.
    ///
    /// Done by claiming every pixel in a scratch buffer rather than by comparing cells pairwise.
    /// An earlier version bucketed cells by their `y` and only compared within a bucket, which
    /// checks that a shelf packs correctly but is blind to the more dangerous bug: two *shelves*
    /// overlapping because a shelf's height was tracked wrong. The atlas is a megabyte; claiming
    /// every pixel costs a millisecond and checks the thing that actually matters.
    #[test]
    fn no_two_cells_share_a_pixel() {
        let p = planned();
        let mut owner: Vec<i32> = vec![-1; p.w * p.h];
        for (i, c) in p.cells.iter().enumerate() {
            if c.w == 0 || c.h == 0 {
                continue;
            }
            for y in c.y as usize..(c.y + c.h) as usize {
                for x in c.x as usize..(c.x + c.w) as usize {
                    let at = y * p.w + x;
                    let prev = owner[at];
                    assert!(
                        prev < 0,
                        "pixel ({}, {}) claimed by both {:?} and {:?}",
                        x, y, p.cells[prev as usize].src, c.src
                    );
                    owner[at] = i as i32;
                }
            }
        }
    }

    /// Shelf packing should not waste most of the surface.
    ///
    /// The threshold is set just under what height-sorted packing achieves, so that removing the
    /// sort — or reintroducing a mixed-height stream some other way — fails here rather than
    /// quietly costing a third of a megabyte of GGTT-mapped surface. Unsorted packing scores 51%.
    #[test]
    fn packing_is_reasonably_dense() {
        let p = planned();
        let ink: usize = p.cells.iter().map(|c| c.w as usize * c.h as usize).sum();
        let pct = ink * 100 / (p.w * p.h);
        std::println!("atlas occupancy: {}%", pct);
        assert!(pct > 70, "atlas is only {}% occupied; height-sorted packing regressed", pct);
    }

    /// Every style must contribute a cell for every character in its charset, or a string will lay
    /// out with holes in it.
    #[test]
    fn every_style_packs_its_whole_charset() {
        let p = planned();
        for (si, style) in ALL_STYLES.iter().enumerate() {
            let mut want = 0usize;
            charset_for(*style).for_each(|_| want += 1);
            let got = p
                .cells
                .iter()
                .filter(|c| matches!(c.src, Source::Glyph { px, slot, .. }
                    if px as usize == style.size() && slot as usize == slot_of(style.face)))
                .count();
            assert!(
                got >= want,
                "style {} ({:?} {}px) packed {} cells for a {}-character set",
                si, style.face, style.size(), got, want
            );
        }
    }

    /// Every icon must reach the atlas at every size THIS SCALE draws — the dock and the Entity are
    /// icons, and a missing one is a blank space where the design's permanent chrome should be.
    ///
    /// Not every size in the generated table: that carries all sixteen, one for each design size at
    /// each interface-scale step, and only five of them are live on any given panel.
    #[test]
    fn every_icon_is_packed_at_every_size_in_use() {
        let p = planned();
        let n = p.cells.iter().filter(|c| matches!(c.src, Source::Icon { .. })).count();
        let active = crate::scale::active_icon_sizes();
        assert_eq!(n, icons_gen::ICONS.len() * active.len());
        for &px in active.iter() {
            assert!(
                icons_gen::size_index(px).is_some(),
                "the atlas wants icons at {}px and the generated table has no such size — re-run \
                 `cargo run --release -p nyx-icons-gen -- ../Nyx-ui`",
                px
            );
        }
    }

    /// ★ The contract between `tools/icons` and `scale`: every size the shell can EVER ask for, at
    /// any interface scale, has to exist in the generated table.
    ///
    /// `icons::get` refuses a size it was not drawn at rather than scaling a bitmap, so a gap here
    /// is a piece of permanent chrome that draws nothing on one class of panel and is fine on every
    /// other — the kind of bug that only shows up on someone else's screen.
    #[test]
    fn the_generated_table_covers_every_scale_step() {
        for px in crate::scale::required_icon_sizes() {
            assert!(
                icons_gen::size_index(px).is_some(),
                "no icons rasterized at {}px; generated sizes are {:?}",
                px, icons_gen::SIZES
            );
        }
    }

    /// Whitespace still needs a cell — with an advance and no pixels — or a space would collapse to
    /// nothing and every word would run together.
    #[test]
    fn space_has_an_advance_but_no_pixels() {
        let p = planned();
        let space = p
            .cells
            .iter()
            .find(|c| matches!(c.src, Source::Glyph { ch: ' ', px, .. } if px as usize == B1.size()))
            .expect("no space cell for b1");
        assert_eq!(space.w, 0, "space should have no ink");
        assert!(space.adv > 0, "space must still advance the pen");
    }

    /// Guards the atlas's size, and prints the actual numbers so the charset restriction can be
    /// judged on evidence rather than on the assumption that it must be helping.
    ///
    /// Run with `--nocapture` to see the breakdown. The restriction is narrow now — `d1` and `lb`
    /// only — so if the saving ever stops justifying the machinery, this is where that shows up.
    #[test]
    fn the_atlas_is_a_reasonable_size() {
        register_all();
        let p = plan();
        let bytes = p.bytes();

        let mut saved = 0usize;
        for style in [D1, LB] {
            let slot = slot_of(style.face);
            Charset::Full.for_each(|ch| {
                if charset_for(style).contains(ch) {
                    return;
                }
                if let Some((w, h)) = font::with_glyph_face(slot, ch, style.size(), |g| (g.w, g.h)) {
                    saved += (w + GUTTER) * (h + GUTTER) * 4;
                }
            });
        }

        let glyphs = p.cells.iter().filter(|c| matches!(c.src, Source::Glyph { .. })).count();
        let icons = p.cells.iter().filter(|c| matches!(c.src, Source::Icon { .. })).count();
        std::println!(
            "atlas {}x{} = {} KiB ({} glyph cells, {} icon cells); charset restriction saves ~{} KiB",
            p.w, p.h, bytes / 1024, glyphs, icons, saved / 1024
        );

        // The atlas is a GGTT-mapped GPU surface. Several megabytes would still work but would be a
        // sign the style table had grown without anyone noticing.
        assert!(bytes < 4 * 1024 * 1024, "atlas is {} bytes, which is a lot to map", bytes);
        assert!(p.h > 0 && !p.cells.is_empty());
    }

    /// Tracking goes BETWEEN characters. A single character must measure exactly its advance, or
    /// every centred label drifts by a pixel per character.
    ///
    /// Runs against `Plan::measure`, which shares `measure_cells` with `Atlas::measure` — so this
    /// exercises the arithmetic the shell will actually use, with no SHM and no hardware.
    #[test]
    fn tracking_is_applied_between_characters_only() {
        let p = planned();
        assert!(LB.tracking_px() > 0, "lb should have positive tracking");
        let one = p.measure(LB, "A") as i32;
        let two = p.measure(LB, "AA") as i32;
        let three = p.measure(LB, "AAA") as i32;
        assert_eq!(two - one, one + LB.tracking_px(), "one gap between two characters");
        assert_eq!(three - two, one + LB.tracking_px(), "each further character adds one gap");
        assert_eq!(p.measure(LB, ""), 0);
    }

    /// Measuring must be additive and monotonic — a longer string is never narrower.
    #[test]
    fn measure_grows_with_the_string() {
        let p = planned();
        let mut last = 0;
        for s in ["", "A", "Ap", "App", "Appl", "Appli", "Applications"] {
            let w = p.measure(B1, s);
            assert!(w >= last, "{:?} measured {} after {}", s, w, last);
            last = w;
        }
        assert!(last > 40, "'Applications' at 15px should be wider than {}px", last);
    }

    /// `measure` must agree with what `layout` would actually place. They share `measure_cells`, but
    /// the pen arithmetic in `layout` is written separately, and a drift between them is what puts a
    /// centred string off-centre.
    #[test]
    fn measure_matches_the_layout_pen() {
        let p = planned();
        for (style, text) in [
            (B1, "Applications"),
            (B2, "58.2 GB free of 64 GB"),
            (LB, "Locations"),
            (MONO, "qcc build bell.qc"),
            (D1, "82"),
        ] {
            // Walk the pen exactly as `Atlas::layout` does.
            let track = style.tracking_px();
            let mut pen = 0i32;
            let mut n = 0;
            for ch in transform(style, text) {
                if let Some(c) = p.cell(style, ch) {
                    pen += c.adv as i32 + track;
                    n += 1;
                }
            }
            if n > 0 {
                pen -= track; // no trailing gap
            }
            assert_eq!(
                pen.max(0) as usize,
                p.measure(style, text),
                "pen and measure disagree for {:?} in {:?}",
                text, style.face
            );
        }
    }

    /// Every character of the strings the design actually sets must be present in the style that
    /// sets it. This is the concrete version of the charset guarantee — a hole here is a hole in a
    /// word on screen.
    #[test]
    fn the_strings_the_design_sets_are_fully_covered() {
        let p = planned();
        let cases: &[(Style, &str)] = &[
            (B1, "Applications"),
            (B1, "QCLang Studio"),
            (B1, "manifest.toml"),
            (B2, "15 items \u{00B7} 58.2 GB free of 64 GB"),
            (B2, "mnt / nvme / apps"),
            (B3, "86% \u{00B7} 4 h 20 m"),
            (B3, "Package reached 82 \u{00B0}C during the last build."),
            (LB, "Locations"),
            (LB, "Devices"),
            (D2, "Applications"),
            (D1, "82"),
            (MONO, "~/src/bell \u{203A} qcc build bell.qc"),
            (MONO, "|00\u{27E9}  512"),
        ];
        for (style, text) in cases {
            for ch in transform(*style, text) {
                assert!(
                    p.cell(*style, ch).is_some(),
                    "{:?} at {}px cannot draw {:?} (from {:?})",
                    style.face, style.size(), ch, text
                );
            }
        }
    }

    /// The mono shelf must stay monospaced through the atlas, or the terminal's columns drift.
    #[test]
    fn the_mono_shelf_keeps_a_uniform_advance() {
        let p = planned();
        let advs: Vec<u32> = p
            .cells
            .iter()
            .filter(|c| matches!(c.src, Source::Glyph { slot, px, ch }
                if slot as usize == slot_of(MONO.face) && px as usize == MONO.size()
                   && ch.is_ascii_graphic()))
            .map(|c| c.adv)
            .collect();
        assert!(!advs.is_empty());
        assert!(advs.iter().all(|a| *a == advs[0]), "mono advances vary: {:?}", &advs[..8.min(advs.len())]);
    }

    /// Styles at the same pixel size but different faces must not collide in the cell maps — `b3`
    /// and `caption` are both 12px, and if they shared cells the caption would lose its weight.
    #[test]
    fn same_size_different_face_are_distinct_shelves() {
        use crate::tokens::{B3, CAPTION};
        assert_eq!(B3.size(), CAPTION.size(), "this test is only meaningful if the sizes match");
        assert_ne!(slot_of(B3.face), slot_of(CAPTION.face));
        let p = planned();
        let count = |slot: usize| {
            p.cells.iter().filter(|c| matches!(c.src, Source::Glyph { slot: s, px, .. }
                if s as usize == slot && px as usize == B3.size())).count()
        };
        assert!(count(slot_of(B3.face)) > 0);
        assert!(count(slot_of(CAPTION.face)) > 0);
    }

    #[test]
    fn uppercase_transform_applies_only_to_label_styles() {
        let up: alloc::string::String = transform(LB, "Locations").collect();
        assert_eq!(up, "LOCATIONS");
        let same: alloc::string::String = transform(B2, "Locations").collect();
        assert_eq!(same, "Locations");
    }

    /// ★ Every design symbol must actually RASTERIZE, in every face that packs it.
    ///
    /// A character the font does not contain is not a miss — it is packed, found, and drawn as
    /// nothing, so [`Atlas::misses`] stays at zero and the only symptom is a gap on screen. That is
    /// how the Command's footer would ship reading "  navigate" with no key cap, and it would take a
    /// power cycle to notice.
    ///
    /// The risk is concentrated in JetBrains Mono, which packs the same `Full` charset as the body
    /// faces but is a much smaller font: `\u{21E5}` (⇥) and `\u{27E9}` (⟩) are the kind of thing a
    /// monospace face omits. If this fails, set that string in Inter instead of adding a glyph.
    #[test]
    fn every_design_symbol_rasterizes_in_every_shelf_that_packs_it() {
        let p = planned();
        let mut blank: alloc::vec::Vec<(usize, usize, char)> = alloc::vec::Vec::new();
        for cell in &p.cells {
            if let Source::Glyph { slot, ch, px } = cell.src {
                if crate::charset::SYMBOLS.contains(&ch) && cell.w == 0 {
                    blank.push((slot as usize, px as usize, ch));
                }
            }
        }
        assert!(
            blank.is_empty(),
            "these symbols packed a zero-size cell — the face has no glyph for them, and they will \
             draw as nothing without registering a miss: {:?}",
            blank
        );
    }

    /// The same trap for ordinary text: a shelf whose glyphs are all empty means the face failed to
    /// register and every string on it is invisible.
    #[test]
    fn every_shelf_has_ink() {
        let p = planned();
        for (si, style) in ALL_STYLES.iter().enumerate() {
            let inked = p
                .cells
                .iter()
                .filter(|c| matches!(c.src, Source::Glyph { slot, px, .. }
                    if slot as usize == slot_of(style.face) && px as usize == style.size()))
                .filter(|c| c.w > 0)
                .count();
            assert!(inked > 20, "style {} ({}px) packed only {} inked cells", si, style.size(), inked);
        }
    }

    // ---- wrap ----
    //
    // A fixed 10px-per-character measure, so the expected breaks can be read off the string by
    // counting letters. The wrapping logic is what goes wrong; real font metrics would only make
    // the failures harder to read.
    fn w10(s: &str) -> usize {
        s.len() * 10
    }

    #[test]
    fn wrapping_breaks_on_spaces_and_never_loses_a_word() {
        let text = "Package reached 82 C during the last build";
        for max in [60, 100, 150, 240, 1000] {
            let lines = wrap(text, max, w10);
            let rejoined = lines.join(" ");
            assert_eq!(rejoined, text, "at max_w={} the text came back different", max);
        }
    }

    #[test]
    fn every_line_fits_the_width() {
        let text = "Clocks reduced to hold the envelope; no action needed.";
        let lines = wrap(text, 200, w10);
        assert!(lines.len() > 1, "this text should need more than one line at 200px");
        for l in &lines {
            assert!(w10(l) <= 200, "line {:?} is {}px, over the 200px limit", l, w10(l));
        }
    }

    /// A word longer than the whole line must be emitted anyway. Returning early, looping forever,
    /// or hyphenating are all worse than one over-long line — and the loop not terminating would
    /// hang the shell, which is the desktop.
    #[test]
    fn an_unbreakable_word_is_emitted_rather_than_dropped() {
        let lines = wrap("a supercalifragilistic b", 50, w10);
        assert!(lines.iter().any(|l| l.contains("supercalifragilistic")));
        assert_eq!(lines.join(" "), "a supercalifragilistic b");
    }

    #[test]
    fn text_that_already_fits_is_one_line() {
        assert_eq!(wrap("System nominal", 500, w10), ["System nominal"]);
        assert_eq!(wrap("", 500, w10).len(), 0);
        assert_eq!(wrap("one", 5, w10), ["one"], "a single over-long word is still one line");
    }

    /// The line count is what `layout::entity_layout` sizes the surface from, so it has to be
    /// monotonic: a narrower surface can never need FEWER lines.
    #[test]
    fn narrower_never_means_fewer_lines() {
        let text = "Package reached 82 C during the last build. Clocks reduced to hold the envelope.";
        let mut prev = 0;
        for max in [1000, 600, 400, 300, 200, 120] {
            let n = wrap(text, max, w10).len();
            assert!(n >= prev, "wrapping at {}px gave {} lines, fewer than the wider {}", max, n, prev);
            prev = n;
        }
    }
}
