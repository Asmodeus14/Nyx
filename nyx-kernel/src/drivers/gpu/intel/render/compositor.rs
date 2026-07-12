// src/drivers/gpu/intel/render/compositor.rs
//
// Phase U4: the GPU desktop compositor. The userspace window server (apps/compositor) used to
// CPU-copy every window's pixels into the backbuffer each frame. Here the kernel instead draws ONE
// textured orthographic quad per window — sampling the window's already-GGTT-mapped SHM surface —
// so the GPU composites all windows into the backbuffer in a single pass. This is the pixel-space
// quad (U3) + src-over blend (U2) path applied to real windows; no new GPU code, only orchestration.
//
// SAFETY / threading: a single global COMPOSITOR_CTX (spin::Mutex), mirroring GL_CONTEXT. The scene
// is rebuilt only when the window SET changes (count / GVA / size); dragging a window changes only the
// per-frame ortho MVP, so it reuses the scene (no rebuild, no leak). The compositor scene shares the
// render engine's fixed state GVAs, so it must not run concurrently with the mini-GL context
// (glcube) — giving each its own GVA range is a Boot-2 item.

#![allow(dead_code)]

use alloc::vec::Vec;

use super::engine::Scene;
use super::math::{Mat4, Vec3};
use super::{RENDER_ENGINE};

/// One window to composite, matching the userspace `WindowQuad` (repr(C), 9 x u32/i32 = 36 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WindowQuad {
    pub tex_gva: u32,   // GVA of the window's page-aligned pixel data (already GGTT-mapped)
    pub src_w: u32,
    pub src_h: u32,
    pub src_pitch: u32, // bytes; must be 64-byte aligned (width % 16 == 0) for a LINEAR surface
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: u32,
    pub dst_h: u32,
    pub opacity: u32,   // U4 Boot 2: 0..=255 per-window alpha; 255 = opaque (byte-identical to Boot 1)
}

struct CompositorCtx {
    scene: Option<Scene>,
    /// Signature of the window set the current scene was built for: (tex_gva, src_w, src_h) per window.
    /// A change (add/remove/resize/reopen) triggers a rebuild; a pure move does not.
    sig: Vec<(u32, u32, u32)>,
}

impl CompositorCtx {
    const fn new() -> Self {
        Self { scene: None, sig: Vec::new() }
    }
}

unsafe impl Send for CompositorCtx {}

static COMPOSITOR_CTX: spin::Mutex<CompositorCtx> = spin::Mutex::new(CompositorCtx::new());

/// Read the scanout dimensions (width, height, stride) from the screen painter.
fn screen_dims() -> Option<(u32, u32, u32)> {
    let sp = core::ptr::addr_of!(crate::gui::SCREEN_PAINTER);
    let painter = unsafe { (*sp).as_ref()? };
    let stride = if painter.info.stride > 0 { painter.info.stride } else { painter.info.width };
    Some((painter.info.width as u32, painter.info.height as u32, stride as u32))
}

/// GPU-composite `quads` (back-to-front) into the backbuffer (GVA 0x1400_0000) OVER the wallpaper the
/// BLT engine already filled there. Returns true on success; false tells the caller to fall back to
/// CPU compositing. A LINEAR-pitch violation (window width not 16-aligned) fails the whole call so the
/// compositor CPU-composites that frame — correct, just not accelerated.
pub fn composite(quads: &[WindowQuad]) -> bool {
    if quads.is_empty() {
        return true; // nothing to composite; wallpaper alone is correct
    }
    // A LINEAR sampled surface needs a 64-byte-aligned row pitch. Bail (CPU fallback) if any window
    // violates it, rather than sampling garbage.
    for q in quads {
        if q.src_pitch % 64 != 0 || q.tex_gva == 0 {
            return false;
        }
    }

    let (sw, sh, stride) = match screen_dims() {
        Some(d) => d,
        None => return false,
    };
    let pitch = stride * 4;

    let mut ctx = COMPOSITOR_CTX.lock();
    let mut eng = RENDER_ENGINE.lock();
    if !eng.initialized || eng.kernels.is_none() {
        return false;
    }

    // Rebuild the scene only when the window SET changed (not on a pure move).
    let sig: Vec<(u32, u32, u32)> = quads.iter().map(|q| (q.tex_gva, q.src_w, q.src_h)).collect();
    if ctx.scene.is_none() || ctx.sig != sig {
        let mut scene = match unsafe {
            eng.create_compositor_scene(0x1400_0000, sw, sh, pitch)
        } {
            Ok(s) => s,
            Err(_) => return false,
        };
        // U4 Boot 3: corner rounding is done CPU-side on the TRUE window rectangle (constant-radius,
        // crisp) — see libs/gui draw_window_chrome. The GPU stretched-mask approach produced a
        // size-dependent ELLIPSE on the inner content (the shader only has UV 0..1, not the window's
        // pixel size, so it can't hold a constant radius), so it's disabled here: with no mask
        // uploaded, add_window_quad falls back to the Boot-2 opacity PS = clean square content, and the
        // CPU rounds the real window outline on top. (A GPU signed-distance-field PS is the premium
        // upgrade — constant radius + correct over overlaps — deferred as a dedicated EU boot.)
        for q in quads {
            if unsafe { scene.add_window_quad(q.tex_gva, q.src_pitch, q.src_w, q.src_h) }.is_err() {
                return false;
            }
        }
        ctx.scene = Some(scene);
        ctx.sig = sig;
        // (Silenced: this fired on every window layout change — drag/resize/open/close — and spammed
        //  the serial log during normal UI use. Re-enable locally when debugging scene rebuilds.)
    }

    // Per-frame ortho MVPs from the destination rects (pixel space, top-left origin).
    let ortho = Mat4::ortho_screen(sw as f32, sh as f32);
    let mvps: Vec<Mat4> = quads
        .iter()
        .map(|q| {
            ortho
                .mul(&Mat4::translate(Vec3::new(q.dst_x as f32, q.dst_y as f32, 0.0)))
                .mul(&Mat4::scale(Vec3::new(q.dst_w as f32, q.dst_h as f32, 1.0)))
        })
        .collect();

    let scene = ctx.scene.as_mut().unwrap();
    // Per-window opacity (U4 Boot 2): write each window's alpha into its quad every frame. Opacity is
    // deliberately NOT part of `sig`, so a fade reuses the cached scene (no rebuild) — this only
    // mutates 4 floats per window before the draw. 255 -> 1.0 renders byte-identical to Boot 1.
    for (i, q) in quads.iter().enumerate() {
        scene.set_quad_opacity(i, q.opacity as f32 / 255.0);
    }
    // Composite into the backbuffer: linear RT (tiled_pitch=0), no clear (wallpaper is already there),
    // no present (the compositor's sys_swap_buffers does that), blend=true (src-over per-window alpha).
    // bb_cpu unused when clear=false && present=false, so 0 is safe.
    let ok = unsafe {
        eng.draw_scene(scene, &mvps, 0x1400_0000, 0, sw, sh, pitch, 0, 0, false, true, false, false)
    };
    if ok.is_err() {
        // On a GPU hang the scene may be wedged; drop it so the next call rebuilds, and fall back.
        ctx.scene = None;
        ctx.sig.clear();
        return false;
    }
    true
}
