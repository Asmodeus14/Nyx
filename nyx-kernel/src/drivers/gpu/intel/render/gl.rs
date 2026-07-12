// src/drivers/gpu/intel/render/gl.rs
//
// Phase 9: the mini-GL layer exposed to userspace via syscalls.
//
// Phases 5-8 built and HW-proved the whole Gen9.5 RCS 3D pipeline behind an in-kernel API
// (`Scene` / `add_mesh` / `draw_scene` / `draw_resolve`, plus the Boot-3 SSAA antialiasing).
// This module is the thin, stateful bridge that lets a *userspace* process drive that pipeline
// through syscalls (514 gl_init / 515 gl_upload_mesh / 516 gl_render / 527 gl_reset). It owns a
// single persistent GL context — the same shared-buffer `Scene` + supersampled Y-tiled RT that
// `spin_scene` builds — so userspace just:
//
//     sys_gl_init(w, h);                                   // once
//     let h0 = sys_gl_upload_mesh(verts, idx, texels, tw, th);   // per mesh
//     loop { sys_gl_render(&[mvp0, mvp1, ..]); }           // per frame (one MVP per mesh)
//
// The heavy pixels never cross the syscall boundary: geometry/texels are copied into kernel Vecs
// once at upload, transforms are 16 floats/mesh/frame. Rendering reuses the exact proven
// draw_scene (SSAA pass 1) + draw_resolve (bilinear box-downsample + present) path — no new GPU
// code, only orchestration. See [[3d-engine-plan]] Phase 9.
//
// SAFETY / threading: a single global GL_CONTEXT under a spin::Mutex, mirroring RENDER_ENGINE.
// The context is deliberately simple (one scene, meshes appended, finalized on first render); a
// gl_reset tears it down so a client can rebuild. This is the minimum that makes the 3D engine
// programmable from userspace; a multi-context / handle-registry design is a later phase.

#![allow(dead_code)]

use alloc::vec::Vec;

use super::engine::{Mesh, Scene, Texture, Vertex};
use super::math::{Mat4, Vec3, Vec4};
use super::{state, RenderError, RENDER_ENGINE};

/// The interleaved vertex the syscall ABI accepts: position (x,y,z) + a 4-component varying
/// (u,v,0,1 for a textured mesh, or r,g,b,a for a flat one). `#[repr(C)]` so the userspace
/// struct in `libs/api` maps byte-for-byte. 7 f32 = 28 bytes; the array is tightly packed.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GlVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub vx: f32,
    pub vy: f32,
    pub vz: f32,
    pub vw: f32,
}

/// A CPU-side mesh staged before the scene is finalized. Held in the context until the first
/// `gl_render`, which builds the `Scene` (meshes must all be known before the resolve quad is
/// added, because the shared bump-allocated buffers are sized/laid out in one pass).
struct StagedMesh {
    mesh: Mesh,
    texture: Texture,
}

/// The single persistent GL context. `scene` is None until the first render finalizes it from the
/// staged meshes; after that, meshes are fixed (re-upload requires gl_reset). `bb_cpu`/`bb_gva`
/// are the linear backbuffer the resolve presents into (the same GVA 0x1400_0000 the BLT path and
/// spin_scene use). `ss_*` are the supersampled scene-RT dims.
pub struct GlContext {
    initialized: bool,
    width: u32,
    height: u32,
    pitch: u32,       // window backbuffer row pitch (bytes) == width*4 (tightly packed)
    bb_cpu: u64,      // CPU-visible KERNEL window-backbuffer base (resolve writes here)
    bb_gva: u32,      // window-backbuffer GVA (GVA_GL_WIN_BB) the resolve quad targets
    dst_cpu: u64,     // USERSPACE SHM window pixel buffer; gl_render copies bb_cpu -> here each frame
    dst_pitch: u32,   // SHM window row pitch (bytes) == width*4
    ss_w: u32,        // supersampled RT width
    ss_h: u32,        // supersampled RT height
    tiled_pitch: u32, // supersampled Y-tiled row pitch
    rt_buf_bytes: usize,
    rt_tiled_gva: u32,
    rt_tiled_cpu: u64,
    staged: Vec<StagedMesh>,
    scene: Option<Scene>,
    frame: u64, // rendered-frame counter; first few frames log pipeline stats for diagnosis
}

impl GlContext {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            width: 0,
            height: 0,
            pitch: 0,
            bb_cpu: 0,
            bb_gva: 0,
            dst_cpu: 0,
            dst_pitch: 0,
            ss_w: 0,
            ss_h: 0,
            tiled_pitch: 0,
            rt_buf_bytes: 0,
            rt_tiled_gva: 0,
            rt_tiled_cpu: 0,
            staged: Vec::new(),
            scene: None,
            frame: 0,
        }
    }
}

unsafe impl Send for GlContext {}
unsafe impl Sync for GlContext {}

/// Global GL context (mirrors RENDER_ENGINE / INTEL_GPU).
pub static GL_CONTEXT: spin::Mutex<GlContext> = spin::Mutex::new(GlContext::new());

/// Supersample factor per axis (2 => 2x2 SSAA, matching Boot 3).
const SS: u32 = 2;

/// syscall 514: initialize the GL context for a `width`x`height` WINDOW. `dst_cpu` is the userspace
/// SHM window pixel buffer (validated by the syscall) into which each frame's resolved image is copied
/// so the compositor composites glcube as a normal window — the GL path never touches the scanout.
/// Allocates the supersampled Y-tiled scene RT at GVA_SSAA_RT and a private linear resolve backbuffer
/// at GVA_GL_WIN_BB (both tightly packed at the window dims). Idempotent-ish: a second init after
/// meshes are staged is rejected (call gl_reset first).
///
/// The RENDER_ENGINE must already be brought up (init_render_engine ran at boot); we only borrow its
/// kernels/mmio, we don't re-bring-up here.
pub fn gl_init(width: u32, height: u32, dst_cpu: u64) -> Result<(), RenderError> {
    let mut ctx = GL_CONTEXT.lock();
    if ctx.initialized {
        // Re-init with the same geometry pending is fine (client just re-declaring dims); but if a
        // scene is already finalized, require an explicit reset so we don't leak GGTT buffers.
        if ctx.scene.is_some() {
            return Err(RenderError::NotInitialized);
        }
    }

    // Ensure the render engine is up and has kernels (brought up at boot). Without it there is no
    // VS/PS to draw with.
    {
        let eng = RENDER_ENGINE.lock();
        if !eng.initialized || eng.kernels.is_none() {
            return Err(RenderError::NotInitialized);
        }
    }

    let ss_w = width * SS;
    let ss_h = height * SS;
    let tiled_pitch = (ss_w * 4 + 127) & !127;
    let tiled_rows = (ss_h + 31) & !31;
    let rt_buf_bytes = (tiled_pitch * tiled_rows) as usize;
    let pages = (rt_buf_bytes + 4095) / 4096;

    // Window backbuffer: tightly-packed linear W*H*4 (pitch = width*4). The resolve quad writes here
    // (GVA_GL_WIN_BB); gl_render then CPU-copies it into the app's SHM window buffer (dst_cpu).
    let win_pitch = width * 4;
    let win_bytes = (win_pitch * height) as usize;
    let win_pages = (win_bytes + 4095) / 4096;

    // Allocate the supersampled Y-tiled scene RT (same as spin_scene Boot 3) + the window backbuffer.
    let (rt_tiled_gva, rt_tiled_cpu, bb_gva, bb_cpu) = {
        let eng = RENDER_ENGINE.lock();
        let rt_buf = unsafe { state::StateBuffer::new(eng.mmio_base, super::GVA_SSAA_RT, pages)? };
        let bb_buf = unsafe { state::StateBuffer::new(eng.mmio_base, super::GVA_GL_WIN_BB, win_pages)? };
        (rt_buf.gva, rt_buf.virt, bb_buf.gva, bb_buf.virt)
    };
    // NOTE: both buffers are intentionally leaked here (pages stay GGTT-mapped for the context's life).
    // gl_reset re-allocates from the same GVAs, overwriting the PTEs; the physical frames are not
    // reclaimed (acceptable for a single long-lived context). A future gl_reset can free.

    ctx.initialized = true;
    ctx.width = width;
    ctx.height = height;
    ctx.pitch = win_pitch;
    ctx.bb_cpu = bb_cpu;
    ctx.bb_gva = bb_gva;
    ctx.dst_cpu = dst_cpu;
    ctx.dst_pitch = win_pitch;
    ctx.ss_w = ss_w;
    ctx.ss_h = ss_h;
    ctx.tiled_pitch = tiled_pitch;
    ctx.rt_buf_bytes = rt_buf_bytes;
    ctx.rt_tiled_gva = rt_tiled_gva;
    ctx.rt_tiled_cpu = rt_tiled_cpu;
    ctx.staged.clear();
    ctx.scene = None;

    Ok(())
}

/// syscall 515: stage a mesh (vertices + indices + a B8G8R8A8 texture) into the context. Returns
/// the mesh handle (its index, 0-based) on success. Must be called before the first gl_render;
/// after the scene is finalized, returns Err (call gl_reset to rebuild).
pub fn gl_upload_mesh(
    verts: &[GlVertex],
    indices: &[u32],
    texels: &[u32],
    tex_w: u32,
    tex_h: u32,
) -> Result<usize, RenderError> {
    let mut ctx = GL_CONTEXT.lock();
    if !ctx.initialized || ctx.scene.is_some() {
        return Err(RenderError::NotInitialized);
    }
    if verts.is_empty() || indices.is_empty() || texels.len() < (tex_w * tex_h) as usize {
        return Err(RenderError::NotInitialized);
    }

    // Convert the flat ABI vertices into engine Vertex structs.
    let mut vertices = Vec::with_capacity(verts.len());
    for v in verts {
        vertices.push(Vertex::new(
            Vec3::new(v.x, v.y, v.z),
            Vec4::new(v.vx, v.vy, v.vz, v.vw),
        ));
    }
    let mesh = Mesh::new(vertices, indices.to_vec());

    let texture = Texture {
        width: tex_w,
        height: tex_h,
        pitch: tex_w * 4,
        texels: texels[..(tex_w * tex_h) as usize].to_vec(),
    };

    let handle = ctx.staged.len();
    ctx.staged.push(StagedMesh { mesh, texture });
    Ok(handle)
}

/// syscall 516 entry: render from a FLAT `count * 16` f32 array (column-major MVPs, one per mesh).
/// Builds `Mat4`s (avoids relying on Mat4's memory layout across the ABI) and calls `gl_render`.
pub fn gl_render_flat(flat: &[f32], count: usize) -> Result<(), RenderError> {
    if flat.len() < count * 16 {
        return Err(RenderError::NotInitialized);
    }
    let mut mvps = Vec::with_capacity(count);
    for i in 0..count {
        let mut m = [0.0f32; 16];
        m.copy_from_slice(&flat[i * 16..i * 16 + 16]);
        mvps.push(Mat4 { m });
    }
    gl_render(&mvps)
}

/// Render one frame. `mvps` supplies one 4x4 MVP matrix per staged mesh (in upload order). On the
/// first call, the `Scene` is finalized from the staged meshes (+ resolve quad). Then: pass 1 draws
/// every mesh into the supersampled Y-tiled RT (SSAA), pass 2 box-downsamples to the backbuffer and
/// presents. Reuses draw_scene + draw_resolve unchanged.
pub fn gl_render(mvps: &[Mat4]) -> Result<(), RenderError> {
    let mut ctx = GL_CONTEXT.lock();
    if !ctx.initialized {
        return Err(RenderError::NotInitialized);
    }

    // Finalize the scene on first render (all meshes now known -> shared buffers can be laid out).
    if ctx.scene.is_none() {
        if ctx.staged.is_empty() {
            return Err(RenderError::NotInitialized);
        }
        let ss_w = ctx.ss_w;
        let ss_h = ctx.ss_h;
        let tiled_pitch = ctx.tiled_pitch;
        let rt_tiled_gva = ctx.rt_tiled_gva;
        let bb_gva = ctx.bb_gva;
        let width = ctx.width;
        let height = ctx.height;

        let mut eng = RENDER_ENGINE.lock();
        let mut scene = unsafe {
            eng.create_scene(rt_tiled_gva, ss_w, ss_h, tiled_pitch, state::TILEMODE_YMAJOR)?
        };
        for sm in ctx.staged.iter() {
            unsafe { scene.add_mesh(&sm.mesh, &sm.texture)? };
        }
        unsafe {
            scene.add_resolve_quad(rt_tiled_gva, tiled_pitch, ss_w, ss_h, bb_gva, width, height)?
        };
        drop(eng);
        ctx.scene = Some(scene);
    }

    let mesh_count = ctx.staged.len();
    if mvps.len() < mesh_count {
        return Err(RenderError::NotInitialized);
    }

    // Snapshot the fields draw_scene/draw_resolve need (avoid holding ctx borrows across the &mut).
    let ss_w = ctx.ss_w;
    let ss_h = ctx.ss_h;
    let pitch = ctx.pitch;
    let tiled_pitch = ctx.tiled_pitch;
    let rt_buf_bytes = ctx.rt_buf_bytes;
    let rt_tiled_gva = ctx.rt_tiled_gva;
    let rt_tiled_cpu = ctx.rt_tiled_cpu;
    let bb_cpu = ctx.bb_cpu;
    let dst_cpu = ctx.dst_cpu;
    let width = ctx.width;
    let height = ctx.height;
    let mvps_owned: Vec<Mat4> = mvps[..mesh_count].to_vec();

    // Log pipeline stats (IA/VS/CL/PS counters) for the first few frames so a HW boot can be
    // diagnosed from the sysmon boot-log without a serial cable: CL_prims==0 => geometry/transform,
    // CL_prims>0 & PS_inv==0 => cull/winding, PS_inv>0 but black => present/scanout contention.
    let frame = ctx.frame;
    // Per-frame pipeline diagnostics ([DS]/[SCENE] submitting+STATS/[DR]/[GL] frame …) are OFF now
    // that the GL path is proven. Flip to `frame < 3` to bring back the first-few-frames X-ray when
    // diagnosing a black/garbled cube on new hardware.
    let verbose = false;
    ctx.frame = frame.wrapping_add(1);

    let scene = ctx.scene.as_mut().unwrap();
    let mut eng = RENDER_ENGINE.lock();
    // Split the CL_prims / PS_inv deltas across the two passes so a black frame is pinned exactly:
    //   mesh_ps==0 & res_ps>0  => the CUBE covers no pixels (cull/winding/degenerate) — resolve is fine
    //   mesh_ps==0 & res_ps==0 => GLOBAL no-coverage (viewport/DRAWING_RECTANGLE/scissor empty)
    //   mesh_ps>0  & winbb 0   => cube shaded but wrote nothing/black (RT write / texture)
    // (draw_scene/draw_resolve each wait on their fence, so the counters are settled between reads.)
    let cl0 = unsafe { eng.read_reg(0x2340) };
    let ps0 = unsafe { eng.read_reg(0x2348) };
    unsafe {
        // Wake + re-arm the RENDER engine: it may have parked (RC6) since the boot self-test, and
        // its forcewake is separate from the BLT domain the compositor holds. Without this, RCS
        // MMIO writes are dropped and nothing rasterizes (black RT). Cheap no-op when already awake.
        eng.ensure_ready();
        // NOTE: the register-based GGTT flush (invalidate_ggtt_tlb / GFX_FLSH_CNTL 0x101008) used to
        // be called here, but it is GL-path-ONLY (ring-0 spin_scene never touches it) and correlated
        // exactly with the fence-never-signals hang (last_progress=0 FAULT=0 HEAD advanced — post-sync
        // writes stopped becoming CPU-visible). The in-stream prologue TLB_INVALIDATE PIPE_CONTROL
        // (proven safe: ring-0 passes 300 frames with it) does the real engine-TLB flush, so the
        // chipset register poke is both redundant and the suspected wedge. Removed.
        // Pass 1: render all meshes into the supersampled Y-tiled scene RT (no present). blend=false:
        // glcube's cube is opaque, so keep the GL mesh pass byte-identical to its HW-proven stream.
        // (Per-mesh GL blending is a later addition when the GPU compositor path needs translucency.)
        eng.draw_scene(
            scene, &mvps_owned, rt_tiled_gva, rt_tiled_cpu, ss_w, ss_h, pitch, tiled_pitch,
            rt_buf_bytes, false, false, true, verbose,
        )?;
    }
    let cl1 = unsafe { eng.read_reg(0x2340) };
    let ps1 = unsafe { eng.read_reg(0x2348) };
    unsafe {
        // Pass 2: bilinear box-downsample the supersampled RT to the private window backbuffer.
        // present=false: we do NOT blit the scanout — the compositor composites our window instead.
        eng.draw_resolve(scene, bb_cpu, width, height, pitch, false, verbose)?;
    }
    let cl2 = unsafe { eng.read_reg(0x2340) };
    let ps2 = unsafe { eng.read_reg(0x2348) };
    let mesh_cl = cl1.wrapping_sub(cl0);
    let mesh_ps = ps1.wrapping_sub(ps0);
    let res_cl = cl2.wrapping_sub(cl1);
    let res_ps = ps2.wrapping_sub(ps1);
    drop(eng);

    // Publish the finished frame into the app's SHM window buffer (userspace vaddr, valid in this
    // syscall's address space; validated at gl_init). The compositor reads this and composites the
    // window. Only reached when both passes succeeded (`?` above), so a failed/hung frame leaves the
    // window showing its previous good content instead of garbage.
    unsafe {
        core::ptr::copy_nonoverlapping(bb_cpu as *const u8, dst_cpu as *mut u8, (pitch * height) as usize);
    }

    // Diagnostic (first few frames): split the black window across the three stages.
    //  - `ssaa_nz`  : non-black pixels in the SSAA RT (MESH-pass output). >0 => the cube rasterized
    //                 into the scene RT, so a black window is the RESOLVE pass's fault. ==0 => the mesh
    //                 pass wrote black (texture sampled black, or RT write not landing).
    //  - `winbb`    : the resolved window backbuffer (RESOLVE-pass output).
    //  - `shm`      : the copy the compositor composites.
    // The SSAA RT is Y-tiled, so we scan raw bytes (clflush first for a coherent read) rather than
    // indexing a pixel — any non-zero byte means the mesh pass produced coverage.
    if verbose {
        unsafe {
            let ss = rt_tiled_cpu as *const u32;
            let words = rt_buf_bytes / 4;
            let mut i = 0usize;
            while i < words {
                core::arch::asm!("clflush [{}]", in(reg) ss.add(i), options(nostack, preserves_flags));
                i += 256; // ~1 KB stride
            }
            core::arch::asm!("mfence", options(nostack, preserves_flags));
            let mut ss_nz = 0u32;
            let mut ss_sample = 0u32;
            i = 0;
            while i < words {
                let v = *ss.add(i);
                if v != 0 { ss_nz += 1; if ss_sample == 0 { ss_sample = v; } }
                i += 256;
            }

            let stride = (pitch / 4) as usize;
            let ci = (height as usize / 2) * stride + (width as usize / 2);
            let src = bb_cpu as *const u32;
            let dst = dst_cpu as *const u32;
            crate::serial_println!(
                "[GL] frame {} mesh(cl={} ps={}) resolve(cl={} ps={}) | ssaa_nz={} ss_sample={:#010x} | winbb center={:#010x} tl={:#010x} | shm={:#010x}",
                frame, mesh_cl, mesh_ps, res_cl, res_ps, ss_nz, ss_sample, *src.add(ci), *src.add(0), *dst.add(ci)
            );
        }
    }
    Ok(())
}

/// syscall 527: tear down the context so a client can rebuild (re-init + re-upload). Drops the
/// scene (unmapping its shared buffers) and clears staged meshes. The supersampled RT GVA is left
/// mapped; a following gl_init reuses/overwrites it.
pub fn gl_reset() {
    let mut ctx = GL_CONTEXT.lock();
    ctx.scene = None;
    ctx.staged.clear();
    ctx.initialized = false;
}
