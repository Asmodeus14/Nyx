// src/drivers/gpu/intel/render/engine.rs
//
// Phase 8: the engine API layer.
//
// Phases 5-7 proved the raw Gen9 3D pipeline on hardware, but the code was welded to a
// single hardcoded cube: fixed 24-vertex / 36-index counts, a baked-in camera and spin,
// and one procedural checkerboard. This module lifts the *reusable* rendering primitive
// out of the cube so an ARBITRARY mesh + transform + texture can be drawn through one call:
//
//     let mut gm = engine.upload_mesh(&mesh, Some(&texture))?;  // once
//     for frame in .. {
//         let mvp = camera.mul(&spin(frame));                   // caller owns the transform
//         engine.draw(&mut gm, &mvp, present, verbose)?;        // per frame
//     }
//
// This is the exact primitive Phase 9's mini-GL syscalls will wrap. The command stream this
// path emits is byte-identical to the Phase 7 Boot-C cube (a behavior-preserving refactor):
// the only thing that changed is that geometry, texture, and the MVP are now *inputs* rather
// than constants. See [[3d-engine-plan]] Phase 8.

#![allow(dead_code)]

use alloc::vec::Vec;

use super::math::{Vec3, Vec4};
use super::state::{self, StateBuffer};
use super::{GVA_DYNAMIC_STATE, GVA_SURFACE_STATE, GVA_TEXTURES, GVA_VERTEX_BUFFERS};
use super::{RenderEngine, RenderError};

/// One interleaved vertex: position (model space) + a generic 4-component varying.
///
/// The pipeline delivers `varying` to the pixel shader as a perspective-correct attribute
/// (Boot B's `pln` path). Phase 7 used it to carry UV as `(u, v, 0, 1)`; a flat-colored
/// mesh would carry RGBA. The GPU vertex layout is two R32G32B32A32_FLOAT elements
/// (pos promoted to vec4 with w=1, then varying) => 8 dwords / 32-byte pitch, matching
/// `emit_vertex_elements`.
#[derive(Clone, Copy)]
pub struct Vertex {
    pub pos: Vec3,
    pub varying: Vec4,
}

impl Vertex {
    pub const fn new(pos: Vec3, varying: Vec4) -> Self {
        Self { pos, varying }
    }
}

/// Dwords the interleaved vertex layout occupies (pos vec4 + varying vec4). The GPU vertex
/// buffer pitch is `VERTEX_DWORDS * 4` = 32 bytes.
pub const VERTEX_DWORDS: usize = 8;

/// CPU-side geometry: an indexed triangle list. Sizes are arbitrary (no baked cube counts).
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
    }
}

/// A CPU-side 2D texture to be uploaded as a LINEAR B8G8R8A8_UNORM sampled surface.
/// `pitch` (row stride, bytes) must be a 64-byte multiple for a linear surface (PRM Vol2a).
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub texels: Vec<u32>, // width*height B8G8R8A8, one texel per u32 (A<<24|R<<16|G<<8|B)
}

impl Texture {
    /// A `size`x`size` white/blue checkerboard with `square`-texel cells (the Phase-7 texture,
    /// now expressed as a first-class `Texture` the cube passes to `upload_mesh`).
    pub fn checkerboard(size: u32, square: u32) -> Self {
        const WHITE: u32 = 0xFFFF_FFFF;
        const BLUE: u32 = 0xFF00_00FF;
        let mut texels = Vec::with_capacity((size * size) as usize);
        let mut y = 0u32;
        while y < size {
            let mut x = 0u32;
            while x < size {
                let cell = ((x / square) + (y / square)) & 1;
                texels.push(if cell == 0 { WHITE } else { BLUE });
                x += 1;
            }
            y += 1;
        }
        Self { width: size, height: size, pitch: size * 4, texels }
    }

    /// A `size`x`size` texture filled with one B8G8R8A8 color (A<<24|R<<16|G<<8|B). Used by the U2
    /// blend gate: a half-alpha panel that must show the meshes behind it when src-over is on.
    pub fn solid(size: u32, argb: u32) -> Self {
        let texels = alloc::vec![argb; (size * size) as usize];
        Self { width: size, height: size, pitch: size * 4, texels }
    }
}

/// Persistent per-mesh GPU resources: state buffers + render state allocated ONCE and reused
/// every frame. Only the vertex block is rewritten per `draw` (with the caller's MVP applied);
/// the index buffer, texture, sampler, and every state object are static across frames.
///
/// This is the generalization of Phase 7's `CubeGpu`: the fixed 24/36/192 constants become
/// `vert_count` / `idx_count` / `vbo_dwords` derived from the uploaded mesh.
pub struct GpuMesh {
    // Retained GGTT-mapped buffers (dropping these would unmap the pages the GPU reads).
    pub(super) surf_buf: StateBuffer,
    pub(super) dyn_buf: StateBuffer,
    pub(super) vbo_buf: StateBuffer,
    pub(super) tex_buf: Option<StateBuffer>,
    pub(super) rs: state::RenderState,

    // Model-space vertices (transformed to clip space every frame) + their static varyings.
    pub(super) verts: Vec<Vertex>,

    pub(super) vbo_cpu: *mut u32, // CPU ptr to the reserved vertex block
    pub(super) vbo_gva: u32,      // GVA of the vertex block (stable across frames)
    pub(super) vbo_dwords: usize, // vertex block size in dwords = vert_count * VERTEX_DWORDS
    pub(super) idx_gva: u32,
    pub(super) idx_count: usize,
    pub(super) vs_off: u32,
    pub(super) ps_off: u32,
    pub(super) sampler_off: u32, // SAMPLER_STATE offset from dynamic base (0 if untextured)
}

impl RenderEngine {
    /// Upload a mesh (and optional texture) into freshly-allocated, reused GPU buffers, and
    /// build every static state object. The returned `GpuMesh` is drawn each frame via
    /// [`RenderEngine::draw`]. Generalizes Phase 7's `setup_cube_gpu`.
    pub unsafe fn upload_mesh(
        &mut self,
        mesh: &Mesh,
        texture: Option<&Texture>,
        rt_gva: u32,
        width: u32,
        height: u32,
        pitch: u32,
    ) -> Result<GpuMesh, RenderError> {
        let (vs_off, ps_off) = match &self.kernels {
            Some(k) => (k.vs_off, k.ps_off),
            None => return Err(RenderError::NotInitialized),
        };
        let mmio = self.mmio_base;
        let mut surf_buf = StateBuffer::new(mmio, GVA_SURFACE_STATE, 1)?;
        let mut dyn_buf = StateBuffer::new(mmio, GVA_DYNAMIC_STATE, 1)?;
        let mut vbo_buf = StateBuffer::new(mmio, GVA_VERTEX_BUFFERS, 1)?;
        let mut rs =
            state::setup_render_state(&mut surf_buf, &mut dyn_buf, rt_gva, width, height, pitch, state::TILEMODE_LINEAR)?;

        // --- Optional texture: upload texels, build a LINEAR 2D surface, extend the binding
        // table to [0]=RT, [1]=texture, and place a NEAREST/CLAMP sampler. (Phase 7 Boot C,
        // now driven by the caller-supplied `Texture` instead of a hardcoded checkerboard.) ---
        let mut tex_buf_opt: Option<StateBuffer> = None;
        let mut sampler_off = 0u32;
        if let Some(tex) = texture {
            let tex_bytes = (tex.height * tex.pitch) as usize;
            let pages = (tex_bytes + 4095) / 4096;
            let mut tex_buf = StateBuffer::new(mmio, GVA_TEXTURES, pages)?;
            let (tex_cpu, tex_gva) = tex_buf.alloc(tex.texels.len(), 4096)?;
            for (i, &t) in tex.texels.iter().enumerate() {
                tex_cpu.add(i).write_volatile(t);
            }
            tex_buf.flush_range(tex_gva, tex.texels.len() * 4);
            // LINEAR 2D sampled surface (same builder as the RT; Gen9 samples linear surfaces).
            let ts = state::render_surface_state_2d(
                state::SURFACE_FORMAT_B8G8R8A8_UNORM,
                tex.width,
                tex.height,
                tex.pitch,
                tex_gva,
                state::TILEMODE_LINEAR,
            );
            let tex_surf_gva = surf_buf.write(&ts, 64)?;
            let tex_surf_off = tex_surf_gva - surf_buf.gva;
            // 2-entry binding table overrides the 1-entry table setup_render_state wrote.
            let bt2 = state::binding_table_2(rs.rt_surface_off, tex_surf_off);
            let bt2_gva = surf_buf.write(&bt2, 64)?;
            rs.binding_table_off = bt2_gva - surf_buf.gva;
            // SAMPLER_STATE (NEAREST/CLAMP) into dynamic state; 32B-aligned per the SSP field.
            let samp_gva = dyn_buf.write(&state::sampler_state_nearest_clamp(), 32)?;
            sampler_off = samp_gva - dyn_buf.gva;
            crate::serial_println!(
                "[STATE] tex_gva={:#x} tex_surf_off={:#x} bt2_off={:#x} sampler_off={:#x}",
                tex_gva,
                tex_surf_off,
                rs.binding_table_off,
                sampler_off
            );
            tex_buf_opt = Some(tex_buf);
        }

        // Reserve the per-frame vertex block (vert_count * 8 dwords) at a stable GVA, then
        // write the static index buffer after it. Positions are rewritten every frame; the
        // varyings are static but rewritten alongside them (cheap, keeps the loop simple).
        let vbo_dwords = mesh.vertices.len() * VERTEX_DWORDS;
        let (vbo_cpu, vbo_gva) = vbo_buf.alloc(vbo_dwords, 64)?;
        let idx_gva = vbo_buf.write(&mesh.indices, 64)?;

        Ok(GpuMesh {
            surf_buf,
            dyn_buf,
            vbo_buf,
            tex_buf: tex_buf_opt,
            rs,
            verts: mesh.vertices.clone(),
            vbo_cpu,
            vbo_gva,
            vbo_dwords,
            idx_gva,
            idx_count: mesh.indices.len(),
            vs_off,
            ps_off,
            sampler_off,
        })
    }
}

// ===========================================================================
// Multi-mesh scene (Phase 8b): several textured meshes sharing ONE set of state
// buffers under ONE STATE_BASE_ADDRESS, drawn in ONE fenced command stream.
//
// GpuMesh (above) maps its own StateBuffers at the FIXED GVAs, so two of them would
// collide in the GGTT. A Scene instead owns ONE surf/dyn/vbo/tex buffer and sub-allocates
// each mesh's vertex/index/texture/binding-table out of them via the StateBuffer bump
// allocator. Every mesh shares the RT surface (BTI 0), blend/cc/viewport state, and one
// NEAREST/CLAMP sampler; each mesh has its OWN vertex block, index block, texture, binding
// table (BTI 1 -> its texture) and PS kernel offset. This is the kernel-side shape a Phase-9
// userspace GL context maps onto.
// ===========================================================================

/// One mesh inside a [`Scene`]: offsets into the scene's shared buffers, no owned allocation.
pub struct SceneMesh {
    pub(super) verts: Vec<Vertex>,   // model-space verts, transformed per frame
    pub(super) vbo_cpu: *mut u32,    // CPU ptr to this mesh's vertex block in the shared vbo_buf
    pub(super) vbo_gva: u32,         // GVA of this mesh's vertex block
    pub(super) vbo_dwords: usize,    // vert_count * VERTEX_DWORDS
    pub(super) idx_gva: u32,         // GVA of this mesh's static index block
    pub(super) idx_count: usize,
    pub(super) binding_table_off: u32, // per-mesh BINDING_TABLE offset (shared RT + this texture)
    pub(super) ps_off: u32,          // PS kernel offset (shared textured PS for now)
}

/// A collection of meshes sharing one set of GGTT-mapped state buffers, drawn together each
/// frame. Built by [`RenderEngine::create_scene`], populated by [`Scene::add_mesh`], drawn by
/// [`RenderEngine::draw_scene`].
pub struct Scene {
    pub(super) surf_buf: StateBuffer, // shared: RT surface, per-mesh texture surfaces + binding tables
    pub(super) dyn_buf: StateBuffer,  // shared: blend/cc/viewports + sampler
    pub(super) vbo_buf: StateBuffer,  // shared: per-mesh vertex + index blocks
    pub(super) tex_buf: StateBuffer,  // shared: per-mesh texture texels (sub-allocated by page)
    pub(super) rs: state::RenderState, // shared render state (RT surface off, blend/cc/viewport offs)
    pub(super) sampler_off: u32,      // shared NEAREST/CLAMP sampler (offset from dynamic base)
    // MSAA-track Boot 3 (SSAA): the mesh pass renders into a 2x-oversized scene RT using the shared
    // NEAREST sampler + rs.sf_clip_viewport_off (baked at SUPERSAMPLED dims). The resolve pass writes
    // the display-size backbuffer, so it needs its OWN display-dims viewport and a BILINEAR sampler
    // (to box-average each 2x2 block). Both are allocated by add_resolve_quad; 0 until then.
    pub(super) resolve_sampler_off: u32,  // BILINEAR/CLAMP sampler for the SSAA downsample
    pub(super) resolve_sf_clip_off: u32,  // SF_CLIP_VIEWPORT at DISPLAY dims (resolve output)
    pub(super) vs_off: u32,
    pub(super) ps_off: u32,
    pub(super) meshes: Vec<SceneMesh>,
    /// MSAA-track Boot 2: the full-screen resolve quad. When present, the scene is rendered to an
    /// offscreen (tiled) RT, then this quad samples that RT as a texture and writes it to the
    /// linear backbuffer (BTI 0 = backbuffer, BTI 1 = the offscreen scene RT). This is the
    /// render-to-texture "resolve" pass; Boot 3 makes the scene RT multisampled + averages samples.
    pub(super) resolve: Option<SceneMesh>,
}

impl Scene {
    /// Sub-allocate `mesh` + `texture` out of the shared buffers and register it for drawing.
    /// Uploads the texels, builds a LINEAR 2D texture surface, writes a per-mesh 2-entry binding
    /// table (shared RT at BTI 0, this texture at BTI 1), and reserves the per-frame vertex block
    /// + static index block. Every mesh reuses the scene's single sampler.
    pub unsafe fn add_mesh(&mut self, mesh: &Mesh, texture: &Texture) -> Result<(), RenderError> {
        // --- Texture: upload texels into a fresh page-aligned region of the shared tex_buf. ---
        let (tex_cpu, tex_gva) = self.tex_buf.alloc(texture.texels.len(), 4096)?;
        for (i, &t) in texture.texels.iter().enumerate() {
            tex_cpu.add(i).write_volatile(t);
        }
        self.tex_buf.flush_range(tex_gva, texture.texels.len() * 4);
        let ts = state::render_surface_state_2d(
            state::SURFACE_FORMAT_B8G8R8A8_UNORM,
            texture.width,
            texture.height,
            texture.pitch,
            tex_gva,
            state::TILEMODE_LINEAR,
        );
        let tex_surf_gva = self.surf_buf.write(&ts, 64)?;
        let tex_surf_off = tex_surf_gva - self.surf_buf.gva;

        // --- Per-mesh binding table: [0]=shared RT, [1]=this texture. ---
        let bt = state::binding_table_2(self.rs.rt_surface_off, tex_surf_off);
        let bt_gva = self.surf_buf.write(&bt, 64)?;
        let binding_table_off = bt_gva - self.surf_buf.gva;

        // --- Vertex block (rewritten per frame) + static index block, in the shared vbo_buf. ---
        let vbo_dwords = mesh.vertices.len() * VERTEX_DWORDS;
        let (vbo_cpu, vbo_gva) = self.vbo_buf.alloc(vbo_dwords, 64)?;
        let idx_gva = self.vbo_buf.write(&mesh.indices, 64)?;

        crate::serial_println!(
            "[SCENE] mesh {}: tex_gva={:#x} tex_surf_off={:#x} bt_off={:#x} vbo_gva={:#x} idx_gva={:#x} ({} verts, {} idx)",
            self.meshes.len(), tex_gva, tex_surf_off, binding_table_off, vbo_gva, idx_gva,
            mesh.vertices.len(), mesh.indices.len()
        );

        self.meshes.push(SceneMesh {
            verts: mesh.vertices.clone(),
            vbo_cpu,
            vbo_gva,
            vbo_dwords,
            idx_gva,
            idx_count: mesh.indices.len(),
            binding_table_off,
            ps_off: self.ps_off,
        });
        Ok(())
    }

    /// MSAA-track Boot 2: register the full-screen resolve quad. `scene_rt_gva` is the offscreen
    /// (Y-tiled) render target the meshes draw into; `scene_rt_pitch` its tiled row pitch;
    /// `bb_gva` the LINEAR backbuffer the resolve writes to. Builds:
    ///   - a LINEAR backbuffer RENDER_SURFACE_STATE (the resolve pass's RT, BTI 0),
    ///   - a Y-tiled surface over the EXISTING scene RT to SAMPLE it as a texture (BTI 1; no upload
    ///     — it's an already-GPU-resident surface), and
    ///   - a resolve binding table [0]=backbuffer, [1]=scene RT, plus the full-screen quad VBO/IB.
    /// The quad's clip-space NDC positions are static (identity MVP), so draw_resolve writes them
    /// once rather than per frame.
    pub unsafe fn add_resolve_quad(
        &mut self,
        scene_rt_gva: u32,
        scene_rt_pitch: u32,
        scene_w: u32,
        scene_h: u32,
        bb_gva: u32,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        // Backbuffer as a LINEAR render target at DISPLAY dims (BTI 0 for the resolve's RT write).
        let bb_ss = state::render_surface_state_2d(
            state::SURFACE_FORMAT_B8G8R8A8_UNORM, width, height, width * 4, bb_gva, state::TILEMODE_LINEAR);
        let bb_surf_gva = self.surf_buf.write(&bb_ss, 64)?;
        let bb_surf_off = bb_surf_gva - self.surf_buf.gva;

        // The offscreen scene RT as a Y-tiled SAMPLED texture (BTI 1), at its SUPERSAMPLED dims
        // (scene_w x scene_h = 2x display for SSAA; == display for the 1:1 Boot-2 case). Same bytes
        // the meshes just rendered; the sampler reads them via the tiled surface layout. The width/
        // height here set the sampler's unnormalization scale, so a display-pixel UV (px+0.5)/W maps
        // to texel coord 2*px+1 in a 2x RT — exactly a 2x2 block center (the SSAA box-filter point).
        let scene_ss = state::render_surface_state_2d(
            state::SURFACE_FORMAT_B8G8R8A8_UNORM, scene_w, scene_h, scene_rt_pitch, scene_rt_gva, state::TILEMODE_YMAJOR);
        let scene_surf_gva = self.surf_buf.write(&scene_ss, 64)?;
        let scene_surf_off = scene_surf_gva - self.surf_buf.gva;

        // Resolve-only dynamic state: a BILINEAR sampler (box-averages each 2x2 block on downsample)
        // and a DISPLAY-dims viewport (the mesh pass baked rs.sf_clip at supersampled dims). Both live
        // in the shared dyn_buf; the prologue picks sampler_off / sf_clip_off per pass.
        let rsamp_gva = self.dyn_buf.write(&state::sampler_state_linear_clamp(), 32)?;
        self.resolve_sampler_off = rsamp_gva - self.dyn_buf.gva;
        let rsfv_gva = self.dyn_buf.write(&state::sf_clip_viewport(width, height), 64)?;
        self.resolve_sf_clip_off = rsfv_gva - self.dyn_buf.gva;

        // Resolve binding table: [0] = backbuffer RT, [1] = scene RT texture.
        let bt = state::binding_table_2(bb_surf_off, scene_surf_off);
        let bt_gva = self.surf_buf.write(&bt, 64)?;
        let binding_table_off = bt_gva - self.surf_buf.gva;

        // Full-screen quad: 4 verts (NDC pos + UV), 6 indices. Positions are already in clip
        // space (w=1), so draw_resolve applies an identity transform.
        let quad = fullscreen_quad_mesh();
        let vbo_dwords = quad.vertices.len() * VERTEX_DWORDS;
        let (vbo_cpu, vbo_gva) = self.vbo_buf.alloc(vbo_dwords, 64)?;
        let idx_gva = self.vbo_buf.write(&quad.indices, 64)?;

        crate::serial_println!(
            "[SCENE] resolve quad: bb_surf_off={:#x} scene_surf_off={:#x} bt_off={:#x} vbo_gva={:#x} rsamp_off={:#x} rsfv_off={:#x} scene={}x{} bb={}x{}",
            bb_surf_off, scene_surf_off, binding_table_off, vbo_gva,
            self.resolve_sampler_off, self.resolve_sf_clip_off, scene_w, scene_h, width, height
        );

        self.resolve = Some(SceneMesh {
            verts: quad.vertices.clone(),
            vbo_cpu,
            vbo_gva,
            vbo_dwords,
            idx_gva,
            idx_count: quad.indices.len(),
            binding_table_off,
            ps_off: self.ps_off,
        });
        Ok(())
    }
}

// Vertex is trivially cloneable so Mesh::vertices can be moved into GpuMesh::verts.
impl Clone for Mesh {
    fn clone(&self) -> Self {
        Self { vertices: self.vertices.clone(), indices: self.indices.clone() }
    }
}

/// The Phase-7 unit cube as a first-class `Mesh`: 24 face-vertices (4 per face x 6 faces),
/// each carrying UV `(u, v, 0, 1)` so the whole texture tiles across every face, + 36 indices
/// (two triangles per quad). This is the cube_geometry_colored() geometry, unchanged — the
/// cube is now just the first client of the generic mesh API.
pub fn cube_mesh() -> Mesh {
    // The 8 canonical corners of a unit cube centered at the origin.
    let c = [
        Vec3::new(-0.5, -0.5, -0.5), // 0
        Vec3::new(0.5, -0.5, -0.5),  // 1
        Vec3::new(0.5, 0.5, -0.5),   // 2
        Vec3::new(-0.5, 0.5, -0.5),  // 3
        Vec3::new(-0.5, -0.5, 0.5),  // 4
        Vec3::new(0.5, -0.5, 0.5),   // 5
        Vec3::new(0.5, 0.5, 0.5),    // 6
        Vec3::new(-0.5, 0.5, 0.5),   // 7
    ];
    // Each face lists its 4 corners in the winding order used for back-face culling.
    let faces: [[usize; 4]; 6] = [
        [4, 5, 6, 7], // front  (+z)
        [1, 0, 3, 2], // back   (-z)
        [0, 4, 7, 3], // left   (-x)
        [5, 1, 2, 6], // right  (+x)
        [3, 7, 6, 2], // top    (+y)
        [0, 1, 5, 4], // bottom (-y)
    ];
    // Corner -> UV: map each face's 4 corners to the unit square (0,0)-(1,1).
    let corner_uv: [Vec4; 4] = [
        Vec4::new(0.0, 0.0, 0.0, 1.0), // corner 0 -> (0,0)
        Vec4::new(1.0, 0.0, 0.0, 1.0), // corner 1 -> (1,0)
        Vec4::new(1.0, 1.0, 0.0, 1.0), // corner 2 -> (1,1)
        Vec4::new(0.0, 1.0, 0.0, 1.0), // corner 3 -> (0,1)
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (f, corner_ids) in faces.iter().enumerate() {
        let base = (f * 4) as u32;
        for (j, &cid) in corner_ids.iter().enumerate() {
            vertices.push(Vertex::new(c[cid], corner_uv[j]));
        }
        // Quad (base+0,1,2,3) -> two triangles preserving the original winding.
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(vertices, indices)
}

/// A square pyramid (base side 1.0, apex above the base center): 4 triangular side faces + a
/// square base = 6 triangles, laid out with UNIQUE vertices per face-corner (16 verts) so each
/// face can carry its own UV. This proves the mesh API is not cube-shaped.
///
/// Winding is computed to MATCH the cube's back-face-cull convention (outward faces CCW in model
/// space; sf_clip_viewport's Y-flip makes them CW in screen space, so Front Winding=CW + Cull
/// BACK removes the away faces): each side is base-left -> base-right -> apex (CCW seen from
/// outside), and the base (-y, facing down) is wound so its normal points -y. If HW shows it
/// inside-out (sides culled / base-through), the fix is to reverse the base + side index order
/// (or give the pyramid its own RASTER Front Winding); expectation is this order is correct.
pub fn pyramid_mesh() -> Mesh {
    // Base square in the y = -0.5 plane (CCW when viewed from below/outside), apex at +y.
    let b0 = Vec3::new(-0.5, -0.5, 0.5);  // front-left
    let b1 = Vec3::new(0.5, -0.5, 0.5);   // front-right
    let b2 = Vec3::new(0.5, -0.5, -0.5);  // back-right
    let b3 = Vec3::new(-0.5, -0.5, -0.5); // back-left
    let apex = Vec3::new(0.0, 0.5, 0.0);

    // UVs: each side maps base-left->(0,0), base-right->(1,0), apex->(0.5,1) so the checker
    // fans up to the tip; the base maps to the full unit square.
    let uv_l = Vec4::new(0.0, 0.0, 0.0, 1.0);
    let uv_r = Vec4::new(1.0, 0.0, 0.0, 1.0);
    let uv_apex = Vec4::new(0.5, 1.0, 0.0, 1.0);

    let mut vertices = Vec::with_capacity(16);
    let mut indices = Vec::with_capacity(18);

    // 4 side faces: each base-left -> base-right -> apex (outward CCW). Emit 3 fresh verts and
    // a straight 0,1,2 triangle per face.
    let sides = [(b0, b1), (b1, b2), (b2, b3), (b3, b0)];
    for (l, r) in sides.iter() {
        let base = vertices.len() as u32;
        vertices.push(Vertex::new(*l, uv_l));
        vertices.push(Vertex::new(*r, uv_r));
        vertices.push(Vertex::new(apex, uv_apex));
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    // Base quad (-y): four fresh verts b0,b1,b2,b3 with unit-square UVs; wound (0,2,1)+(0,3,2)
    // so both triangles' normals point -y (outward/down), matching the cull convention.
    let base = vertices.len() as u32;
    vertices.push(Vertex::new(b0, Vec4::new(0.0, 0.0, 0.0, 1.0)));
    vertices.push(Vertex::new(b1, Vec4::new(1.0, 0.0, 0.0, 1.0)));
    vertices.push(Vertex::new(b2, Vec4::new(1.0, 1.0, 0.0, 1.0)));
    vertices.push(Vertex::new(b3, Vec4::new(0.0, 1.0, 0.0, 1.0)));
    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);

    Mesh::new(vertices, indices)
}

/// A unit quad centered at the origin in the z=0 plane, facing +z (toward the camera), wound to
/// match the cube's front (+z) face so the shared BACK-cull keeps it. Model coords span [-0.5,0.5]
/// in x/y; UVs map the unit square. Used by the U2 blend gate as a translucent panel: placed in
/// front of the opaque meshes and drawn last so src-over composites it over them.
pub fn panel_mesh() -> Mesh {
    let v = [
        Vec3::new(-0.5, -0.5, 0.0),
        Vec3::new(0.5, -0.5, 0.0),
        Vec3::new(0.5, 0.5, 0.0),
        Vec3::new(-0.5, 0.5, 0.0),
    ];
    let uv = [
        Vec4::new(0.0, 0.0, 0.0, 1.0),
        Vec4::new(1.0, 0.0, 0.0, 1.0),
        Vec4::new(1.0, 1.0, 0.0, 1.0),
        Vec4::new(0.0, 1.0, 0.0, 1.0),
    ];
    let vertices = alloc::vec![
        Vertex::new(v[0], uv[0]),
        Vertex::new(v[1], uv[1]),
        Vertex::new(v[2], uv[2]),
        Vertex::new(v[3], uv[3]),
    ];
    // Clockwise winding (viewed from +z) so BACK-cull keeps it front-facing.
    let indices = alloc::vec![0u32, 2, 1, 0, 3, 2];
    Mesh::new(vertices, indices)
}

/// A unit quad spanning [0,1]x[0,1] in the z=0 plane (UVs = position), for U3 screen-space drawing:
/// `MVP = Mat4::ortho_screen(W,H) * translate(x,y,0) * scale(w,h,1)` places a pixel-space rectangle.
/// Winding: CCW `[0,1,2, 0,2,3]`. The ortho matrix's Y-flip plus the sf_clip viewport's Y-flip compose
/// to TWO flips (vs. the perspective panel's one), so the front-facing order is the OPPOSITE of
/// `panel_mesh` — if it renders invisible (back-face culled), swap to `[0,2,1, 0,3,2]`.
pub fn ui_quad_mesh() -> Mesh {
    let vertices = alloc::vec![
        Vertex::new(Vec3::new(0.0, 0.0, 0.0), Vec4::new(0.0, 0.0, 0.0, 1.0)),
        Vertex::new(Vec3::new(1.0, 0.0, 0.0), Vec4::new(1.0, 0.0, 0.0, 1.0)),
        Vertex::new(Vec3::new(1.0, 1.0, 0.0), Vec4::new(1.0, 1.0, 0.0, 1.0)),
        Vertex::new(Vec3::new(0.0, 1.0, 0.0), Vec4::new(0.0, 1.0, 0.0, 1.0)),
    ];
    let indices = alloc::vec![0u32, 1, 2, 0, 2, 3];
    Mesh::new(vertices, indices)
}

/// A full-screen quad in CLIP/NDC space (positions already have w=1 after Vec4::from_point, so
/// the resolve pass uses an identity MVP), textured with a render-to-texture source. Used by the
/// MSAA-track resolve pass to blit the offscreen scene RT onto the backbuffer.
///
/// Winding: vertex order BL, BR, TR, TL with the cube's index pattern (0,1,2, 0,2,3). After the
/// SF_CLIP_VIEWPORT Y-flip, the first triangle (BL,BR,TR) lands in screen space with the IDENTICAL
/// orientation as the proven cube front face, so it survives the shared RASTER cull (BACK, front=CW).
/// UVs are V-FLIPPED relative to position: screen-top-left NDC(-1,+1) must sample texture-top
/// UV(0,0) so the render-to-texture image is upright. If the blit comes out upside down, swap V.
pub fn fullscreen_quad_mesh() -> Mesh {
    let vertices = alloc::vec![
        Vertex::new(Vec3::new(-1.0, -1.0, 0.0), Vec4::new(0.0, 1.0, 0.0, 1.0)), // BL -> UV(0,1)
        Vertex::new(Vec3::new(1.0, -1.0, 0.0), Vec4::new(1.0, 1.0, 0.0, 1.0)),  // BR -> UV(1,1)
        Vertex::new(Vec3::new(1.0, 1.0, 0.0), Vec4::new(1.0, 0.0, 0.0, 1.0)),   // TR -> UV(1,0)
        Vertex::new(Vec3::new(-1.0, 1.0, 0.0), Vec4::new(0.0, 0.0, 0.0, 1.0)),  // TL -> UV(0,0)
    ];
    let indices = alloc::vec![0u32, 1, 2, 0, 2, 3];
    Mesh::new(vertices, indices)
}
