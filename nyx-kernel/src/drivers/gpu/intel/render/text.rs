// src/drivers/gpu/intel/render/text.rs
//
// Phase U5: GPU-accelerated text. Userspace builds a font atlas (a packed grid of rasterized glyphs,
// coverage in the alpha channel) once, maps it to a GVA via sys_gpu_map_shm, and calls sys_gpu_draw_text
// (syscall 537) with a list of GlyphQuads — each = a destination pixel rect + the glyph's atlas UV
// sub-rect. The kernel draws them as ONE batched textured-quad mesh sampling the atlas, blended over
// whatever is already in the backbuffer. No new EU shader: this reuses the proven textured PS
// (build_ps_tex) + src-over blend (U2). A coverage-in-alpha atlas whose RGB is white therefore blends as
// antialiased WHITE text; colored text is a later boot (a PS that tints by coverage).
//
// SAFETY / threading: a single global TEXT_CTX (spin::Mutex), mirroring COMPOSITOR_CTX. The text scene
// lives on its OWN state GVAs (GVA_TEXT_*), so it coexists with the cached compositor scene without
// forcing a rebuild. The scene is cached and reused; only the glyph run (vertex buffer) is rebuilt each
// call (the text scene's vbo holds nothing else, so its bump cursor is reset per frame — no leak).

#![allow(dead_code)]

use alloc::vec::Vec;

use super::engine::{Scene, Vertex};
use super::math::{Mat4, Vec3, Vec4};
use super::RENDER_ENGINE;

/// One glyph to draw, matching the userspace `GlyphQuad` (repr(C), 9 x 4 bytes = 36 bytes). `dst_*`
/// is the destination pixel rect in the backbuffer; `u0,v0..u1,v1` is the glyph's normalized sub-rect
/// in the atlas (0..1). `color` is reserved for the colored-text boot (ignored here — white/coverage).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GlyphQuad {
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: u32,
    pub dst_h: u32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub color: u32,
}

struct TextCtx {
    scene: Option<Scene>,
    /// GVA the current scene's atlas surface was built over. A change (atlas re-mapped) rebuilds.
    atlas_gva: u32,
    /// Binding-table offset for the atlas (BTI 1), reused by every glyph run against this atlas.
    atlas_bt_off: u32,
}

impl TextCtx {
    const fn new() -> Self {
        Self { scene: None, atlas_gva: 0, atlas_bt_off: 0 }
    }
}

unsafe impl Send for TextCtx {}

static TEXT_CTX: spin::Mutex<TextCtx> = spin::Mutex::new(TextCtx::new());

/// Read the scanout dimensions (width, height, stride) from the screen painter.
fn screen_dims() -> Option<(u32, u32, u32)> {
    let sp = core::ptr::addr_of!(crate::gui::SCREEN_PAINTER);
    let painter = unsafe { (*sp).as_ref()? };
    let stride = if painter.info.stride > 0 { painter.info.stride } else { painter.info.width };
    Some((painter.info.width as u32, painter.info.height as u32, stride as u32))
}

/// Build the batched glyph-run geometry: 4 verts + 6 indices per glyph. Vertex positions are in PIXEL
/// space (the ortho MVP maps them to NDC); `varying.xy` carries the glyph's atlas UV so the textured PS
/// samples the right cell. Winding matches `ui_quad_mesh` (CCW after the ortho + viewport Y-flips).
fn build_run(glyphs: &[GlyphQuad]) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts: Vec<Vertex> = Vec::with_capacity(glyphs.len() * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(glyphs.len() * 6);
    for g in glyphs {
        let x0 = g.dst_x as f32;
        let y0 = g.dst_y as f32;
        let x1 = (g.dst_x + g.dst_w as i32) as f32;
        let y1 = (g.dst_y + g.dst_h as i32) as f32;
        let base = verts.len() as u32;
        // top-left, top-right, bottom-right, bottom-left (UV follows position)
        verts.push(Vertex::new(Vec3::new(x0, y0, 0.0), Vec4::new(g.u0, g.v0, 0.0, 1.0)));
        verts.push(Vertex::new(Vec3::new(x1, y0, 0.0), Vec4::new(g.u1, g.v0, 0.0, 1.0)));
        verts.push(Vertex::new(Vec3::new(x1, y1, 0.0), Vec4::new(g.u1, g.v1, 0.0, 1.0)));
        verts.push(Vertex::new(Vec3::new(x0, y1, 0.0), Vec4::new(g.u0, g.v1, 0.0, 1.0)));
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (verts, indices)
}

/// Draw `glyphs` sampling the font atlas at `atlas_gva` (already GGTT-mapped, LINEAR B8G8R8A8,
/// `atlas_pitch` bytes/row, `atlas_w` x `atlas_h`) into the shared backbuffer (0x1400_0000), blended
/// over what's already there. Returns true on success; false tells the caller to fall back to CPU text.
/// Empty list is a no-op success. The atlas pitch must be 64-byte aligned (LINEAR surface requirement).
pub fn draw_text(atlas_gva: u32, atlas_w: u32, atlas_h: u32, atlas_pitch: u32, glyphs: &[GlyphQuad]) -> bool {
    if glyphs.is_empty() {
        return true;
    }
    if atlas_gva == 0 || atlas_pitch % 64 != 0 || atlas_w == 0 || atlas_h == 0 {
        return false;
    }

    let (sw, sh, stride) = match screen_dims() {
        Some(d) => d,
        None => return false,
    };
    let pitch = stride * 4;

    let mut ctx = TEXT_CTX.lock();
    let mut eng = RENDER_ENGINE.lock();
    if !eng.initialized || eng.kernels.is_none() {
        return false;
    }

    // Build (or rebuild) the cached text scene + atlas binding when the atlas GVA changes.
    if ctx.scene.is_none() || ctx.atlas_gva != atlas_gva {
        let mut scene = match unsafe { eng.create_text_scene(0x1400_0000, sw, sh, pitch) } {
            Ok(s) => s,
            Err(_) => return false,
        };
        let bt = match unsafe { scene.add_text_atlas(atlas_gva, atlas_pitch, atlas_w, atlas_h) } {
            Ok(off) => off,
            Err(_) => return false,
        };
        ctx.scene = Some(scene);
        ctx.atlas_gva = atlas_gva;
        ctx.atlas_bt_off = bt;
        crate::serial_println!(
            "[TEXT] atlas bound: gva={:#x} {}x{} pitch={} bt_off={:#x}",
            atlas_gva, atlas_w, atlas_h, atlas_pitch, bt
        );
    }

    let bt_off = ctx.atlas_bt_off;
    let scene = ctx.scene.as_mut().unwrap();

    // Rebuild the batched glyph run for this frame (resets the text vbo — no leak).
    let (verts, indices) = build_run(glyphs);
    if unsafe { scene.set_text_mesh(bt_off, verts, indices) }.is_err() {
        ctx.scene = None; // vbo overflow (run too long) — drop so the next call rebuilds clean
        return false;
    }

    // One ortho MVP for the whole run (positions are already in pixel space).
    let mvps = [Mat4::ortho_screen(sw as f32, sh as f32)];
    let ok = unsafe {
        eng.draw_scene(scene, &mvps, 0x1400_0000, 0, sw, sh, pitch, 0, 0, false, true, false, false)
    };
    if ok.is_err() {
        ctx.scene = None;
        ctx.atlas_gva = 0;
        return false;
    }
    true
}
