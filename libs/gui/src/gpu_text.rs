// libs/gui/src/gpu_text.rs
//
// Phase U5: userspace font-atlas builder + text layout for GPU text (syscall 537). The kernel has no
// font (noto lives here in libs/gui), so we rasterize the printable ASCII glyphs ONCE into a packed
// B8G8R8A8 atlas, map that atlas to a GVA (like a window surface), and then every frame hand the kernel
// a list of `GlyphQuad`s (destination pixel rect + the glyph's atlas UV sub-rect). The kernel draws them
// as one textured-quad mesh sampling the atlas, blended over the backbuffer.
//
// Coverage/white text (this boot): each atlas texel is WHITE (R=G=B=255) with the glyph's antialiasing
// COVERAGE in the alpha byte. With the render engine's src-over blend (dst = a*src + (1-a)*dst) that
// yields crisp white antialiased text over whatever is behind it — through the existing textured PS, no
// new shader. (Colored text is a later boot: a PS that tints a per-quad color by the sampled coverage.)

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
const COLS: usize = 16; // atlas grid columns; rows = ceil(N_GLYPHS / COLS)

/// A prepared GPU font: the mapped atlas + the metrics needed to place glyphs and to compute each
/// glyph's atlas UV sub-rect. Build once with `GpuFont::prepare()`, reuse every frame via `layout_str`.
pub struct GpuFont {
    pub atlas_gva: u32,
    pub atlas_w: u32,
    pub atlas_h: u32,
    pub atlas_pitch: u32,
    cell_w: usize,
    cell_h: usize,
    atlas_cpu: *const u32, // CPU view of the atlas SHM (for the debug blit / inspection)
    ready: bool,
}

impl GpuFont {
    /// Build the atlas and map it. Returns None if a reference glyph can't be rasterized or the SHM
    /// can't be created/mapped (caller then keeps using CPU `print_str`).
    pub fn prepare() -> Option<GpuFont> {
        // F1: atlas is now sourced from the proportional TTF engine, but still packed into a MONOSPACE
        // grid (uniform cell) for this boot — the GPU banner is a T1 proof; a true per-glyph proportional
        // atlas (per-glyph UV/advance) is the T2 GPU-chrome-text boot. Cell height = the font line
        // height; cell width = the WIDEST printable-glyph advance (keeps columns aligned). Baseline sits
        // at `ascent` rows down inside each cell; glyphs are placed at their bearing.
        let px = crate::font::px_for(1);
        let cell_h = crate::font::line_height(1).max(1);
        let ascent = crate::font::ascent(1) as i32;
        let mut cell_w = 1usize;
        for i in 0..N_GLYPHS {
            let c = (FIRST + i as u8) as char;
            let a = crate::font::advance(c, 1);
            if a > cell_w { cell_w = a; }
        }
        let rows = (N_GLYPHS + COLS - 1) / COLS;

        // LINEAR sampled surface needs a 64-byte-aligned row pitch => atlas width (px) must be a
        // multiple of 16 (16 px * 4 B = 64 B). Pad the grid width up.
        let grid_w = COLS * cell_w;
        let atlas_w = (grid_w + 15) & !15;
        let atlas_h = rows * cell_h;
        let atlas_pitch = atlas_w * 4;
        let bytes = atlas_pitch * atlas_h;

        let shm_id = sys_create_shm(bytes);
        if shm_id == 0 { return None; }
        let vaddr = sys_map_shm(shm_id);
        if vaddr == 0 { return None; }

        // Zero the atlas (fully transparent), then stamp each glyph as OPAQUE WHITE where covered. We
        // THRESHOLD the antialiasing coverage (cov > 50) to a hard on/off so glyph pixels are fully
        // opaque (alpha 255). This is essential for the PLAIN textured PS: it RT-writes the SAMPLED alpha
        // and the src-over blender uses it, so a coverage-in-alpha glyph is semi-transparent and takes on
        // the background colour. Opaque glyphs are solid white on ANY background. (T2's colored PS will
        // instead consume real coverage from alpha and multiply a per-quad colour.)
        let pixels = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u32, atlas_w * atlas_h) };
        for p in pixels.iter_mut() { *p = 0x0000_0000; } // fully transparent

        for i in 0..N_GLYPHS {
            let c = (FIRST + i as u8) as char;
            let ox = (i % COLS) * cell_w;
            let oy = (i / COLS) * cell_h;
            crate::font::with_glyph(c, px, |g| {
                for row in 0..g.h {
                    let py_cell = ascent - g.bearing_y + row as i32;
                    if py_cell < 0 || py_cell as usize >= cell_h { continue; }
                    for col in 0..g.w {
                        let px_cell = g.bearing_x + col as i32;
                        if px_cell < 0 || px_cell as usize >= cell_w { continue; }
                        if g.cov[row * g.w + col] > 50 {
                            let ax = ox + px_cell as usize;
                            let ay = oy + py_cell as usize;
                            pixels[ay * atlas_w + ax] = 0xFFFF_FFFF; // opaque white
                        }
                    }
                }
            });
        }

        // Map the atlas SHM into the GPU's GGTT at ATLAS_GVA so the kernel can bind it as a texture.
        sys_gpu_map_shm(shm_id, ATLAS_GVA);

        Some(GpuFont {
            atlas_gva: ATLAS_GVA,
            atlas_w: atlas_w as u32,
            atlas_h: atlas_h as u32,
            atlas_pitch: atlas_pitch as u32,
            cell_w,
            cell_h,
            atlas_cpu: vaddr as *const u32,
            ready: true,
        })
    }

    pub fn is_ready(&self) -> bool { self.ready }
    pub fn cell_w(&self) -> usize { self.cell_w }
    pub fn cell_h(&self) -> usize { self.cell_h }

    /// DIAGNOSTIC: CPU-blit the RAW atlas to `canvas` at (x, y), `scale`×, so we can visually inspect
    /// the atlas contents independently of the GPU sample path. Each atlas texel is drawn opaque:
    /// non-zero (a glyph pixel) → white, zero (empty) → dark grey, so the glyph grid is visible. If this
    /// shows a clean grid of readable glyphs but the GPU-drawn text is garbage, the bug is in GPU
    /// sampling/UVs/mapping — not the atlas. If THIS is also garbled, the atlas builder is the culprit.
    pub fn debug_blit(&self, canvas: &mut crate::canvas::Canvas, x: usize, y: usize, scale: usize) {
        let aw = self.atlas_w as usize;
        let ah = self.atlas_h as usize;
        let src = unsafe { core::slice::from_raw_parts(self.atlas_cpu, aw * ah) };
        let cw = canvas.width;
        let ch = canvas.height;
        for ay in 0..ah {
            for ax in 0..aw {
                let col = if src[ay * aw + ax] != 0 { 0xFF_FFFFFFu32 } else { 0xFF_303030u32 };
                for sy in 0..scale {
                    let py = y + ay * scale + sy;
                    if py >= ch { continue; }
                    let base = py * cw;
                    for sx in 0..scale {
                        let px = x + ax * scale + sx;
                        if px < cw { canvas.buffer[base + px] = col; }
                    }
                }
            }
        }
    }

    /// Advance width (px) of one glyph at `scale`. Cells are drawn at their natural size; advancing by
    /// the same keeps the run clean (no overlap).
    pub fn advance(&self, scale: usize) -> usize { self.cell_w * scale }
    pub fn line_height(&self, scale: usize) -> usize { self.cell_h * scale }

    /// Lay out `text` at pixel (x, y), `scale`× the atlas cell, producing one GlyphQuad per visible
    /// glyph (whitespace is skipped — it only advances the pen). `color` is stored for the future
    /// colored-text path; the coverage/white path ignores it. Handles '\n'.
    pub fn layout_str(&self, x: i32, y: i32, text: &str, scale: usize, color: u32) -> Vec<GlyphQuad> {
        let mut out: Vec<GlyphQuad> = Vec::new();
        let adv = (self.cell_w * scale) as i32;
        let lh = (self.cell_h * scale) as i32;
        let dst_w = (self.cell_w * scale) as u32;
        let dst_h = (self.cell_h * scale) as u32;
        let aw = self.atlas_w as f32;
        let ah = self.atlas_h as f32;

        let mut cx = x;
        let mut cy = y;
        for ch in text.chars() {
            if ch == '\n' { cx = x; cy += lh; continue; }
            let b = ch as u32;
            if b < FIRST as u32 || b > LAST as u32 { cx += adv; continue; }
            if ch == ' ' { cx += adv; continue; }
            let i = (b as u8 - FIRST) as usize;
            let ox = (i % COLS) * self.cell_w;
            let oy = (i / COLS) * self.cell_h;
            out.push(GlyphQuad {
                dst_x: cx,
                dst_y: cy,
                dst_w,
                dst_h,
                u0: ox as f32 / aw,
                v0: oy as f32 / ah,
                u1: (ox + self.cell_w) as f32 / aw,
                v1: (oy + self.cell_h) as f32 / ah,
                color,
            });
            cx += adv;
        }
        out
    }
}
