// libs/gui/src/gpu_text.rs
//
// U5 GPU text atlas builder + layout. Userspace rasterizes the printable ASCII glyphs ONCE (via the F1
// TTF font engine) into a packed B8G8R8A8 atlas, maps it to a GVA (like a window surface), then each
// frame hands the kernel a list of `GlyphQuad`s (dst pixel rect + the glyph's atlas UV sub-rect + colour).
// The kernel draws them as one textured-quad mesh sampling the atlas, blended over the backbuffer.
//
// T2: the atlas is now PROPORTIONAL (per-glyph cell sized to the glyph, packed into a shelf layout) and
// stores REAL antialiasing COVERAGE in the alpha channel (RGB = white). The kernel's TEXT pixel shader
// (`build_ps_text`) samples that coverage and tints it to a per-quad LUMINANCE (derived from GlyphQuad
// .color), so src-over blend draws smooth grayscale text in the requested colour. `layout_str` places
// each glyph by its own advance/bearing (no monospace cell) and emits a tight dst rect + UV sub-rect.

extern crate alloc;
use alloc::vec::Vec;

use nyx_api::{sys_create_shm, sys_map_shm, sys_gpu_map_shm, GlyphQuad};

/// GVA the atlas SHM is mapped to. Matches the kernel's reserved GVA_TEXT_ATLAS slot (0x6400_0000),
/// clear of window GVAs (0x2000_0000 + n*0x0100_0000), the SSAA RT, GL window backbuffer, and cursor.
const ATLAS_GVA: u32 = 0x6400_0000;

/// Printable ASCII range packed into the atlas.
const FIRST: u8 = 0x20; // space
const LAST: u8 = 0x7E;  // '~'
const N_GLYPHS: usize = (LAST - FIRST + 1) as usize; // 95

/// Per-glyph metrics + atlas placement (proportional). `ax,ay,gw,gh` locate the glyph's coverage
/// bitmap inside the atlas; `bearing_x/_y` and `advance` position it against the pen at layout time.
#[derive(Clone, Copy, Default)]
struct GlyphMeta {
    ax: u32,
    ay: u32,
    gw: u32,
    gh: u32,
    bearing_x: i32,
    bearing_y: i32,
    advance: u32,
}

/// A prepared GPU font: the mapped atlas + a per-glyph metrics table. Build once with
/// `GpuFont::prepare()`, reuse every frame via `layout_str`.
pub struct GpuFont {
    pub atlas_gva: u32,
    pub atlas_w: u32,
    pub atlas_h: u32,
    pub atlas_pitch: u32,
    ascent: i32,
    line_h: usize,
    space_adv: usize,
    glyphs: [GlyphMeta; N_GLYPHS],
    atlas_cpu: *const u32, // CPU view of the atlas SHM (inspection)
    ready: bool,
}

impl GpuFont {
    /// Build the proportional coverage atlas and map it. Returns None if the SHM can't be
    /// created/mapped (caller then keeps using CPU `print_str`).
    pub fn prepare() -> Option<GpuFont> {
        let px = crate::font::px_for(1);
        let line_h = crate::font::line_height(1).max(1);
        let ascent = crate::font::ascent(1) as i32;
        let space_adv = crate::font::advance(' ', 1).max(1);

        // First pass: gather each glyph's bitmap size + metrics, and shelf-pack them into rows of the
        // atlas. Shelf packing: lay glyphs left-to-right; when a row overflows `atlas_w`, drop to the
        // next shelf. 1px gutter between glyphs so bilinear-ish sampling of one cell never bleeds a
        // neighbour. Atlas width is fixed generously; height grows by shelves.
        const ATLAS_W: usize = 512; // 512*4 = 2048B pitch (64B-aligned) — ample for 95 small glyphs
        const GUT: usize = 1;

        // Measure all glyphs and compute placement (without touching pixels yet).
        let mut metas = [GlyphMeta::default(); N_GLYPHS];
        let mut sizes: [(usize, usize, i32, i32, usize); N_GLYPHS] = [(0, 0, 0, 0, 0); N_GLYPHS];
        for i in 0..N_GLYPHS {
            let c = (FIRST + i as u8) as char;
            crate::font::with_glyph(c, px, |g| {
                sizes[i] = (g.w, g.h, g.bearing_x, g.bearing_y, g.advance);
            });
        }

        let mut cx = GUT;
        let mut cy = GUT;
        let mut shelf_h = 0usize;
        for i in 0..N_GLYPHS {
            let (gw, gh, bx, by, adv) = sizes[i];
            if gw == 0 || gh == 0 {
                // whitespace / no outline: advance-only glyph, no atlas cell.
                metas[i] = GlyphMeta { ax: 0, ay: 0, gw: 0, gh: 0, bearing_x: bx, bearing_y: by, advance: adv as u32 };
                continue;
            }
            if cx + gw + GUT > ATLAS_W {
                cx = GUT;
                cy += shelf_h + GUT;
                shelf_h = 0;
            }
            metas[i] = GlyphMeta {
                ax: cx as u32, ay: cy as u32, gw: gw as u32, gh: gh as u32,
                bearing_x: bx, bearing_y: by, advance: adv as u32,
            };
            cx += gw + GUT;
            if gh > shelf_h { shelf_h = gh; }
        }
        let atlas_w = ATLAS_W;
        let atlas_h = cy + shelf_h + GUT;
        let atlas_pitch = atlas_w * 4;
        let bytes = atlas_pitch * atlas_h;

        let shm_id = sys_create_shm(bytes);
        if shm_id == 0 { return None; }
        let vaddr = sys_map_shm(shm_id);
        if vaddr == 0 { return None; }

        // Second pass: stamp each glyph's REAL coverage into the alpha channel, RGB = white. The kernel
        // text PS reads this coverage as the output alpha and tints RGB to the per-quad luminance.
        let pixels = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u32, atlas_w * atlas_h) };
        for p in pixels.iter_mut() { *p = 0x0000_0000; } // transparent

        for i in 0..N_GLYPHS {
            let m = metas[i];
            if m.gw == 0 { continue; }
            let c = (FIRST + i as u8) as char;
            crate::font::with_glyph(c, px, |g| {
                for row in 0..g.h {
                    let ay = m.ay as usize + row;
                    if ay >= atlas_h { break; }
                    for col in 0..g.w {
                        let cov = g.cov[row * g.w + col];
                        if cov == 0 { continue; }
                        let ax = m.ax as usize + col;
                        if ax >= atlas_w { continue; }
                        // A8 coverage in the top byte, white RGB.
                        pixels[ay * atlas_w + ax] = ((cov as u32) << 24) | 0x00FF_FFFF;
                    }
                }
            });
        }

        sys_gpu_map_shm(shm_id, ATLAS_GVA);

        Some(GpuFont {
            atlas_gva: ATLAS_GVA,
            atlas_w: atlas_w as u32,
            atlas_h: atlas_h as u32,
            atlas_pitch: atlas_pitch as u32,
            ascent,
            line_h,
            space_adv,
            glyphs: metas,
            atlas_cpu: vaddr as *const u32,
            ready: true,
        })
    }

    pub fn is_ready(&self) -> bool { self.ready }
    pub fn line_height(&self, scale: usize) -> usize { self.line_h * scale }

    /// Pixel width of `text` at `scale` (sum of per-glyph advances; widest line for multi-line).
    pub fn text_width(&self, text: &str, scale: usize) -> usize {
        let mut w = 0usize;
        let mut max = 0usize;
        for ch in text.chars() {
            if ch == '\n' { if w > max { max = w; } w = 0; continue; }
            w += self.advance_of(ch) * scale;
        }
        if w > max { max = w; }
        max
    }

    /// Advance (px, scale 1) of char `ch` — from the metrics table, falling back to the space advance.
    fn advance_of(&self, ch: char) -> usize {
        let b = ch as u32;
        if b < FIRST as u32 || b > LAST as u32 { return self.space_adv; }
        let a = self.glyphs[(b as u8 - FIRST) as usize].advance as usize;
        if a == 0 { self.space_adv } else { a }
    }

    /// Lay out `text` at pixel (x, y=line TOP), `scale`×, producing one GlyphQuad per VISIBLE glyph.
    /// Each glyph is placed at its bearing relative to the baseline (`y + ascent`), with a tight dst
    /// rect and its atlas UV sub-rect. `color` (0xAARRGGBB) is carried per glyph; the kernel text PS
    /// tints coverage to that colour's luminance. Whitespace only advances the pen. Handles '\n'.
    pub fn layout_str(&self, x: i32, y: i32, text: &str, scale: usize, color: u32) -> Vec<GlyphQuad> {
        let mut out: Vec<GlyphQuad> = Vec::new();
        let s = scale as i32;
        let aw = self.atlas_w as f32;
        let ah = self.atlas_h as f32;
        let baseline = y + self.ascent * s;
        let mut cx = x;
        let mut cy_baseline = baseline;
        for ch in text.chars() {
            if ch == '\n' {
                cx = x;
                cy_baseline += (self.line_h * scale) as i32;
                continue;
            }
            let adv = (self.advance_of(ch) * scale) as i32;
            let b = ch as u32;
            if ch == ' ' || b < FIRST as u32 || b > LAST as u32 {
                cx += adv;
                continue;
            }
            let m = self.glyphs[(b as u8 - FIRST) as usize];
            if m.gw == 0 {
                cx += adv;
                continue;
            }
            let dx = cx + m.bearing_x * s;
            let dy = cy_baseline - m.bearing_y * s;
            out.push(GlyphQuad {
                dst_x: dx,
                dst_y: dy,
                dst_w: m.gw * scale as u32,
                dst_h: m.gh * scale as u32,
                u0: m.ax as f32 / aw,
                v0: m.ay as f32 / ah,
                u1: (m.ax + m.gw) as f32 / aw,
                v1: (m.ay + m.gh) as f32 / ah,
                color,
            });
            cx += adv;
        }
        out
    }
}
