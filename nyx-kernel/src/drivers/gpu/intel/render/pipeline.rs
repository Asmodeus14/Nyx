// src/drivers/gpu/intel/render/pipeline.rs
//
// Full Gen9 3D pipeline assembly + first triangle (Phase 5).
//
// Command layouts, sub-opcodes and struct field positions are all taken from the local
// Mesa gen9.xml (Nyx-Firmwares/Intel_Docs/mesa-gen9.xml) and PRM Vol 2a. Because there
// is no GPU debugger, the batch is instrumented with staged PIPE_CONTROL "progress"
// writes: if the GPU hangs, the fence page holds the last stage it completed. Final
// verification reads back the rendered pixels on the CPU (serial-logged) rather than
// depending on display timing.

#![allow(dead_code)]

use super::cmd::{self, gfxpipe_header, CmdStream, PIPELINE_3D};
use super::state::{self, StateBuffer};
use super::{
    RenderEngine, RenderError, GVA_DYNAMIC_STATE, GVA_RCS_FENCE, GVA_SURFACE_STATE,
    GVA_VERTEX_BUFFERS,
};

// --- 3D command sub-opcodes (verified against gen9.xml) ---
mod so {
    pub const VS: u32 = 16;
    pub const HS: u32 = 27;
    pub const DS: u32 = 29;
    pub const GS: u32 = 17;
    pub const TE: u32 = 28;
    pub const STREAMOUT: u32 = 30;
    pub const PS: u32 = 32;
    pub const PS_EXTRA: u32 = 79;
    pub const PS_BLEND: u32 = 77;
    pub const SBE: u32 = 31;
    pub const SBE_SWIZ: u32 = 81; // 3DSTATE_SBE_SWIZ (per-attribute source/swizzle table)
    pub const SF: u32 = 19;
    pub const RASTER: u32 = 80;
    pub const WM: u32 = 20;
    pub const WM_DEPTH_STENCIL: u32 = 78;
    pub const CLIP: u32 = 18;
    pub const VF: u32 = 12;
    pub const VF_INSTANCING: u32 = 73;
    pub const VF_TOPOLOGY: u32 = 75;
    pub const MULTISAMPLE: u32 = 13;
    pub const SAMPLE_MASK: u32 = 24;
    pub const DEPTH_BUFFER: u32 = 5;
    pub const URB_VS: u32 = 48;
    pub const URB_HS: u32 = 49;
    pub const URB_DS: u32 = 50;
    pub const URB_GS: u32 = 51;
    pub const VERTEX_BUFFERS: u32 = 8;
    pub const VERTEX_ELEMENTS: u32 = 9;
    pub const INDEX_BUFFER: u32 = 10;
    pub const BINDING_TABLE_POINTERS_PS: u32 = 42;
    pub const SAMPLER_STATE_POINTERS_PS: u32 = 47;
    pub const VIEWPORT_SF_CLIP: u32 = 33;
    pub const VIEWPORT_CC: u32 = 35;
    pub const CC_STATE_POINTERS: u32 = 14;
    pub const BLEND_STATE_POINTERS: u32 = 36;
    // opcode-1 commands
    pub const DRAWING_RECTANGLE: u32 = 0;
    pub const PUSH_CONSTANT_ALLOC_VS: u32 = 18;
    pub const PUSH_CONSTANT_ALLOC_PS: u32 = 22;
}

/// 3DSTATE header, opcode 0 (most 3DSTATE commands). `total` = full dword count.
#[inline]
fn h(subop: u32, total: u32) -> u32 {
    gfxpipe_header(PIPELINE_3D, 0, subop, total - 2)
}
/// 3DSTATE header, opcode 1 (DRAWING_RECTANGLE, PUSH_CONSTANT_ALLOC_*).
#[inline]
fn h1(subop: u32, total: u32) -> u32 {
    gfxpipe_header(PIPELINE_3D, 1, subop, total - 2)
}

impl RenderEngine {
    /// Phase 5: render one triangle into the linear B8G8R8A8 render target at `rt_gva`,
    /// then read back pixels on the CPU (via `bb_cpu` = the backbuffer's CPU-visible
    /// address) to confirm it drew. Returns Ok if the pipeline completed (fence reached
    /// the success marker).
    pub unsafe fn draw_triangle(
        &mut self,
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
    ) -> Result<(), RenderError> {
        crate::serial_println!(
            "[TRI] draw_triangle rt_gva={:#x} {}x{} pitch={} bb_cpu={:#x}",
            rt_gva, width, height, pitch, bb_cpu
        );

        // --- ISOLATION TEST: prove the render engine can write to the RT's GGTT page.
        // MI_STORE_DATA_IMM to rt_gva, then read back via bb_cpu. If this fails, the
        // backbuffer GGTT mapping/address is wrong and no PS write could ever land. ---
        {
            let probe_cpu = bb_cpu as *mut u32;
            probe_cpu.write_volatile(0);
            self.flush_line(bb_cpu as usize);
            let probe = [cmd::mi_store_data_imm(true, 0x02), rt_gva, 0x0, 0xABCD_1234u32];
            self.rcs_submit(&probe).ok();
            // small settle via a ring fence
            let _ = self.pipecontrol_ring_test();
            self.flush_line(bb_cpu as usize);
            let got = probe_cpu.read_volatile();
            crate::serial_println!(
                "[TRI] RT-GGTT probe: wrote 0xABCD1234 to gva {:#x}, read back {:#010x} via bb_cpu -> {}",
                rt_gva, got, if got == 0xABCD_1234 { "GGTT OK" } else { "GGTT MISMATCH!" }
            );
        }

        // --- Clear the render target to black on the CPU (so readback is unambiguous) ---
        let px = (pitch / 4) as usize;
        core::ptr::write_bytes(bb_cpu as *mut u8, 0x00, (pitch * height) as usize);
        let mut off = 0usize;
        let bytes = (pitch * height) as usize;
        while off < bytes {
            self.flush_line(bb_cpu as usize + off);
            off += 64;
        }

        // --- Kernel offsets (from the instruction-base store built in Phase 3) ---
        let (vs_off, ps_off) = match &self.kernels {
            Some(k) => (k.vs_off, k.ps_off),
            None => return Err(RenderError::NotInitialized),
        };
        crate::serial_println!("[TRI] kernel offsets vs={:#x} ps={:#x}", vs_off, ps_off);

        // --- Allocate state buffers + build all state objects (Phase 4) ---
        let mmio = self.mmio_base;
        let mut surf_buf = StateBuffer::new(mmio, GVA_SURFACE_STATE, 1)?;
        let mut dyn_buf = StateBuffer::new(mmio, GVA_DYNAMIC_STATE, 1)?;
        let mut vbo_buf = StateBuffer::new(mmio, GVA_VERTEX_BUFFERS, 1)?;
        let rs = state::setup_render_state(&mut surf_buf, &mut dyn_buf, rt_gva, width, height, pitch, state::TILEMODE_LINEAR)?;

        // Dump the RENDER_SURFACE_STATE + binding table to cross-check against isl.
        {
            let ss = (surf_buf.virt as *const u32).add((rs.rt_surface_off / 4) as usize);
            crate::serial_println!(
                "[TRI] SS[0..4]={:#010x} {:#010x} {:#010x} {:#010x}",
                ss.read_volatile(), ss.add(1).read_volatile(),
                ss.add(2).read_volatile(), ss.add(3).read_volatile()
            );
            crate::serial_println!(
                "[TRI] SS[4..9]={:#010x} {:#010x} {:#010x} {:#010x} {:#010x}",
                ss.add(4).read_volatile(), ss.add(5).read_volatile(),
                ss.add(6).read_volatile(), ss.add(7).read_volatile(), ss.add(8).read_volatile()
            );
            let bt = (surf_buf.virt as *const u32)
                .add((rs.binding_table_off / 4) as usize)
                .read_volatile();
            crate::serial_println!(
                "[TRI] BT[0]={:#010x} (expect (surf_off {:#x})>>0 in [31:6]); SS base={:#x}",
                bt, rs.rt_surface_off, rs.surface_state_base
            );
        }

        // --- Vertex buffer: NDC coordinates (standard path: viewport transform maps
        // NDC [-1,1] -> screen; clipper works normally since coords are in the clip
        // volume). Large triangle so coverage is guaranteed. w=1. ---
        let verts: [f32; 12] = [
            0.0, 0.8, 0.0, 1.0, // top
            -0.8, -0.8, 0.0, 1.0, // bottom-left
            0.8, -0.8, 0.0, 1.0, // bottom-right
        ];
        let mut vdata = [0u32; 12];
        for i in 0..12 {
            vdata[i] = verts[i].to_bits();
        }
        let vbo_gva = vbo_buf.write(&vdata, 64)?;

        // --- Assemble the pipeline command stream ---
        let mut cs = CmdStream::new();
        self.progress(&mut cs, 0x10); // entered

        // The engine was just reset and is idle, so there are no dirty render/depth
        // caches to flush — and a RENDER_TARGET_CACHE_FLUSH before the pipeline is up
        // HANGS the CS (observed in Phase 2). Do only a cheap read-only cache invalidate
        // (safe on an idle engine) before switching to the 3D pipeline.
        cmd::pipe_control(
            &mut cs,
            cmd::pc::CS_STALL
                | cmd::pc::STATE_CACHE_INVALIDATE
                | cmd::pc::CONSTANT_CACHE_INVALIDATE
                | cmd::pc::INSTRUCTION_CACHE_INVALIDATE,
            0,
            0,
        );
        cs.push(cmd::pipeline_select(cmd::PIPELINE_SELECT_3D));
        self.progress(&mut cs, 1);

        self.emit_state_base_address(&mut cs);
        self.progress(&mut cs, 2);

        // Push-constant alloc (VS then PS) — carve regions before URB.
        cs.push(h1(so::PUSH_CONSTANT_ALLOC_VS, 2)).push(2); // offset 0, size 2
        cs.push(h1(so::PUSH_CONSTANT_ALLOC_PS, 2)).push((2 << 16) | 2); // offset 2, size 2

        // URB (all four stages, programmed together). DW1 layout:
        //   [15:0]  VS Number of URB Entries
        //   [24:16] VS URB Entry Allocation Size (in 512-bit units, minus 1)
        //   [31:25] VS URB Starting Address (in 8KB units)
        // VS: 64 entries, alloc size 0 (=1 unit=512b, holds our 256b VUE header),
        // start 4 (past the push-constant region). HS/DS/GS: 0 entries, start 4 (empty),
        // matching blorp's "assign entire URB to VS" scheme.
        let vs_entries = 64u32;
        let vs_alloc = 0u32; // (size-1); entry size = 1 * 64B
        let vs_start = 4u32;
        // Disabled stages (HS/DS/GS): 0 entries, size 1, but their URB Starting Address
        // MUST be PAST the VS region — never equal to vs_start. Overlapping URB start
        // addresses corrupt VUE handle allocation and DEADLOCK the 3DPRIMITIVE (the VS
        // writes VUEs the clipper can never retrieve: VS_inv>0 but CL_inv=0). This is the
        // exact PRM hazard blorp's emit_urb_config guards against ("URB corruption caused
        // by allocating a previous GS unit URB entry to the VS unit"); blorp stacks the
        // stages monotonically. VS occupies vs_entries*(vs_alloc+1)*64 bytes; the start
        // field is in 8KB units, so the empty stages start one 8KB slot past VS.
        let vs_bytes = vs_entries * (vs_alloc + 1) * 64;
        let empty_start = vs_start + (vs_bytes + 8191) / 8192; // = 5
        cs.push(h(so::URB_VS, 2))
            .push(vs_entries | (vs_alloc << 16) | (vs_start << 25));
        cs.push(h(so::URB_HS, 2)).push(empty_start << 25); // 0 entries, past VS
        cs.push(h(so::URB_DS, 2)).push(empty_start << 25);
        cs.push(h(so::URB_GS, 2)).push(empty_start << 25);
        self.progress(&mut cs, 3);

        // Vertex input.
        self.emit_vertex_buffers(&mut cs, vbo_gva, 16, 48);
        self.emit_vertex_elements(&mut cs);
        cs.push(h(so::VF, 2)).push(0);
        // 3DSTATE_VF_INSTANCING for element 0: instancing OFF.
        cs.push(h(so::VF_INSTANCING, 3)).push(0).push(0);
        // 3DSTATE_VF_TOPOLOGY: on Gen8+ THIS sets the primitive topology (the 3DPRIMITIVE
        // field is legacy/ignored). Without it, zero vertices are fetched. TRILIST = 4.
        cs.push(h(so::VF_TOPOLOGY, 2)).push(4);

        // Shader stages: VS enabled, the rest disabled.
        self.emit_vs(&mut cs, vs_off);
        cs.push(h(so::HS, 9)); for _ in 0..8 { cs.push(0); }
        cs.push(h(so::DS, 11)); for _ in 0..10 { cs.push(0); }
        cs.push(h(so::GS, 10)); for _ in 0..9 { cs.push(0); }
        cs.push(h(so::TE, 4)); for _ in 0..3 { cs.push(0); }
        cs.push(h(so::STREAMOUT, 5)); for _ in 0..4 { cs.push(0); }
        self.progress(&mut cs, 4);

        // CLIP: enabled, NORMAL mode, perspective divide ENABLED (w=1 -> no-op), viewport
        // XY + guardband clip tests on. DW1 StatisticsEnable[42]=bit10;
        // DW2 ClipEnable[95]=bit31 | ViewportXYClipTest[92]=bit28 | GuardbandClipTest[90]=bit26.
        cs.push(h(so::CLIP, 4)).push(1 << 10).push((1 << 31) | (1 << 28) | (1 << 26)).push(0);
        // SF: ViewportTransformEnable [33]=DW1 bit1; StatisticsEnable[42]=bit10.
        cs.push(h(so::SF, 4)).push((1 << 1) | (1 << 10)).push(0).push(0);
        // RASTER: Cull Mode [49:48] = CULLMODE_NONE(1) -> DW1 bit16. Value 0 is
        // CULLMODE_BOTH (cull EVERY triangle) per genxml — the all-zero DW1 was the
        // "CL_prims=1 but PS_inv=0, black screen" bug (SF culled the triangle after clip).
        cs.push(h(so::RASTER, 5)).push(1 << 16).push(0).push(0).push(0); // cull NONE, solid fill
        // SBE (matches blorp for a constant shader): 0 output attrs, read offset 1
        // (skip VUE header), read length 1, and FORCE both so the SF uses our values.
        cs.push(h(so::SBE, 6))
            .push((1 << 5) | (1 << 11) | (1 << 28) | (1 << 29));
        for _ in 0..4 { cs.push(0); }
        // 3DSTATE_WM: empty (all zeros), exactly like blorp's constant-color path.
        cs.push(h(so::WM, 2)).push(1 << 31); // WM StatisticsEnable [63] (else all 0, like blorp)

        // Viewport pointers + drawing rectangle.
        cs.push(h(so::VIEWPORT_SF_CLIP, 2)).push(rs.sf_clip_viewport_off);
        cs.push(h(so::VIEWPORT_CC, 2)).push(rs.cc_viewport_off);
        cs.push(h1(so::DRAWING_RECTANGLE, 4)).push(0).push(((height - 1) << 16) | (width - 1)).push(0);

        // Pixel back-end.
        self.emit_ps(&mut cs, ps_off);
        cs.push(h(so::PS_EXTRA, 2)).push(1u32 << 31); // Pixel Shader Valid
        cs.push(h(so::PS_BLEND, 2)).push(1u32 << 30); // Has Writeable RT
        cs.push(h(so::CC_STATE_POINTERS, 2)).push(rs.cc_off | 1);
        cs.push(h(so::BLEND_STATE_POINTERS, 2)).push(rs.blend_off | 1);
        cs.push(h(so::WM_DEPTH_STENCIL, 4)).push(0).push(0).push(0); // depth/stencil disabled
        cs.push(h(so::MULTISAMPLE, 2)).push(0); // 1 sample
        cs.push(h(so::SAMPLE_MASK, 2)).push(1); // sample mask 0x1
        cs.push(h(so::DEPTH_BUFFER, 8)).push(7u32 << 29); for _ in 0..6 { cs.push(0); } // SURFTYPE_NULL

        // Binding / sampler pointers.
        cs.push(h(so::BINDING_TABLE_POINTERS_PS, 2)).push(rs.binding_table_off);
        cs.push(h(so::SAMPLER_STATE_POINTERS_PS, 2)).push(0);
        self.progress(&mut cs, 5);

        // Draw!
        cs.push(super::cmd::gfxpipe_header(PIPELINE_3D, 3, 0, 7 - 2)); // 3DPRIMITIVE header
        cs.push(4); // Primitive Topology = TRILIST (SEQUENTIAL access)
        cs.push(3); // Vertex Count Per Instance
        cs.push(0); // Start Vertex
        cs.push(1); // Instance Count
        cs.push(0); // Start Instance
        cs.push(0); // Base Vertex

        // progress 6: after the draw drained (VS+PS+raster completed).
        self.progress(&mut cs, 6);
        // Separate RT-cache flush (no post-sync). If this stalls, the PS's RT writes are
        // stuck (bad render-target surface).
        cmd::pipe_control(
            &mut cs,
            cmd::pc::CS_STALL | cmd::pc::RENDER_TARGET_CACHE_FLUSH | cmd::pc::DC_FLUSH_ENABLE,
            0,
            0,
        );
        // progress 7: RT flush drained -> pixels are in memory, readback is valid.
        self.progress(&mut cs, 7);

        // Success marker (minimal post-sync, proven to work).
        const DONE: u32 = 0x00D0_5EED;
        cmd::pipe_control(
            &mut cs,
            cmd::pc::CS_STALL | cmd::pc::POST_SYNC_WRITE_IMM | cmd::pc::DEST_ADDRESS_GTT,
            GVA_RCS_FENCE,
            DONE as u64,
        );
        // A post-sync on the LAST ring command doesn't retire until something follows it
        // is fetched; pad with NOOPs so DONE actually lands.
        for _ in 0..16 {
            cs.push(cmd::MI_NOOP);
        }

        // --- Submit DIRECTLY to the RCS ring (not via a batch buffer) ---
        // The batch-buffer path is unreliable on this HW (test 2a: MI_STORE-in-batch
        // silently fails), but the ring is proven solid (Phase 1). The pipeline is
        // ~240 dwords, well within the 1024-dword ring.
        crate::serial_println!("[TRI] submitting {} dwords to RCS ring", cs.len());
        // GPU X-ray: decode the stream on-device into readable lines so the whole
        // pipeline can be captured in a single photo of the screen (no OCR of raw hex).
        super::decode::decode_and_print(cs.as_slice());
        self.fence_virt.write_volatile(0);
        self.flush_line(self.fence_virt as usize);
        self.rcs_submit(cs.as_slice())?;

        let mut t = 0u32;
        let mut last = 0u32;
        let mut hung = false;
        loop {
            self.flush_line(self.fence_virt as usize);
            let v = self.fence_virt.read_volatile();
            if v != last {
                last = v;
            }
            if v == DONE {
                break;
            }
            core::hint::spin_loop();
            t += 1;
            if t > 30_000_000 {
                let fault = self.read_reg(super::RENDER_FAULT_REG);
                crate::serial_println!(
                    "[TRI] pipeline HUNG. last_progress={:#x} FAULT={:#010x} HEAD={:#x}",
                    last, fault, self.read_reg(super::RENDER_RING_HEAD)
                );
                // Decode the faulting GPU virtual address (GEN8_FAULT_TLB_DATA0/1).
                // DATA0[31:0] = VA[43:12]; DATA1[3:0] = VA[47:44]; DATA1 bit4 = GTT sel.
                // This tells us EXACTLY where the PS render-target write is landing.
                if fault & 1 != 0 {
                    let d0 = self.read_reg(0x4b10);
                    let d1 = self.read_reg(0x4b14);
                    let va = ((d0 as u64) << 12) | (((d1 & 0xF) as u64) << 44);
                    crate::serial_println!(
                        "[TRI]  FAULT_VA={:#x} (TLB_DATA0={:#010x} DATA1={:#010x} gtt_sel={} srcid[16:12]={:#x} type_bits={:#x})",
                        va, d0, d1, (d1 >> 4) & 1, (fault >> 12) & 0x1F, (fault >> 1) & 0x7FF
                    );
                }
                if last >= 7 {
                    crate::serial_println!("[TRI]  -> draw + RT flush OK; only the final post-sync stalled (odd).");
                } else if last == 6 {
                    crate::serial_println!("[TRI]  -> RT-cache flush stalls: PS render-target writes stuck (bad RT surface).");
                } else if last == 5 {
                    crate::serial_println!("[TRI]  -> hang is in the 3DPRIMITIVE (VS/PS/raster) itself.");
                }
                hung = true;
                break;
            }
        }
        if !hung {
            crate::serial_println!("[TRI] pipeline completed (fence reached DONE).");
        }

        // Pipeline statistics — pinpoint exactly where the draw dies (Gen9 MMIO counters).
        let ia_verts = self.read_reg(0x2310); // IA_VERTICES_COUNT (vertices fetched by VF)
        let vs_inv = self.read_reg(0x2320); // VS_INVOCATION_COUNT
        let cl_inv = self.read_reg(0x2338); // CL_INVOCATION_COUNT
        let cl_prims = self.read_reg(0x2340); // CL_PRIMITIVES_COUNT (survive clipping)
        let ps_inv = self.read_reg(0x2348); // PS_INVOCATION_COUNT (pixels shaded)
        crate::serial_println!(
            "[TRI] STATS: IA_verts={} VS_inv={} CL_inv={} CL_prims={} PS_inv={}",
            ia_verts, vs_inv, cl_inv, cl_prims, ps_inv
        );

        // --- CPU pixel readback (done even on hang: the draw may have written pixels
        // before a downstream stall). center should be reddish, a corner should be 0. ---
        let mut o = 0usize;
        while o < bytes {
            self.flush_line(bb_cpu as usize + o);
            o += 64;
        }
        let center = (bb_cpu as *const u32)
            .add((height / 2) as usize * px + (width / 2) as usize)
            .read_volatile();
        // Grid scan: did the triangle draw ANYWHERE? Report count + first non-black pixel.
        let mut nonblack = 0u32;
        let (mut fx, mut fy, mut fc) = (0u32, 0u32, 0u32);
        let mut yy = 0u32;
        while yy < height {
            let mut xx = 0u32;
            while xx < width {
                let p = (bb_cpu as *const u32).add(yy as usize * px + xx as usize).read_volatile();
                if p != 0 {
                    if nonblack == 0 {
                        fx = xx;
                        fy = yy;
                        fc = p;
                    }
                    nonblack += 1;
                }
                xx += 13;
            }
            yy += 13;
        }
        crate::serial_println!(
            "[TRI] readback: center_px={:#010x} | grid: {} non-black samples; first at ({},{})={:#010x}",
            center, nonblack, fx, fy, fc
        );
        if nonblack != 0 {
            crate::serial_println!("[TRI] SUCCESS: triangle wrote pixels somewhere!");
        } else {
            crate::serial_println!("[TRI] nothing drawn anywhere — PS not writing / no raster coverage.");
        }
        if hung {
            return Err(RenderError::EngineHang);
        }
        Ok(())
    }

    /// Render ONE frame of `g` transformed by `mvp` into `rt_gva`. Rewrites only the vertex
    /// block (CPU-applying `mvp` to each model-space position -> clip space), then re-emits the
    /// full known-good Phase-5..7 pipeline (index buffer + indexed 3DPRIMITIVE, back-face
    /// culled, textured/sampled). The caller owns the camera + animation: `mvp` is a finished
    /// projection*view*model matrix. `present` copies the result to scanout; `verbose` logs the
    /// decoded stream + pipeline stats (first frame only during animation).
    ///
    /// This is the generic Phase-8 draw primitive (Phase 9's syscalls wrap it). The emitted
    /// command stream is identical to the Phase-7 cube; only geometry/texture/MVP are inputs.
    pub unsafe fn draw(
        &mut self,
        g: &mut super::engine::GpuMesh,
        mvp: &super::math::Mat4,
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        present: bool,
        verbose: bool,
    ) -> Result<(), RenderError> {
        use super::math::{Vec4};

        let vs_off = g.vs_off;
        let ps_off = g.ps_off;
        let vbo_gva = g.vbo_gva;
        let idx_gva = g.idx_gva;
        let idx_count = g.idx_count;
        let vbo_dwords = g.vbo_dwords;
        let sampler_off = g.sampler_off; // SAMPLER_STATE offset from dynamic base (0 = untextured)

        // Clear the render target to black. (Boot A/C bring-up used a 0x30 gray diagnostic
        // clear to disambiguate "black-on-black" from "never drew"; no longer needed now that
        // the textured cube renders correctly.)
        const CLEAR_BYTE: u8 = 0x00;
        let bytes = (pitch * height) as usize;
        core::ptr::write_bytes(bb_cpu as *mut u8, CLEAR_BYTE, bytes);
        let mut off = 0usize;
        while off < bytes {
            self.flush_line(bb_cpu as usize + off);
            off += 64;
        }

        // --- CPU transform: MVP * model-space positions -> clip space, into the persistent
        // VBO. Interleaved layout per vertex: [pos.x pos.y pos.z pos.w  var.x var.y var.z var.w]
        // = 8 dwords (pitch 32B). Positions are MVP-transformed each frame; the varying (UV,
        // element 1) is static but rewritten alongside to keep the loop uniform. ---
        for (i, v) in g.verts.iter().enumerate() {
            let clip = mvp.mul_vec4(Vec4::from_point(v.pos));
            let p = clip.to_bits();
            let c = v.varying;
            let base = i * 8;
            g.vbo_cpu.add(base).write_volatile(p[0]);
            g.vbo_cpu.add(base + 1).write_volatile(p[1]);
            g.vbo_cpu.add(base + 2).write_volatile(p[2]);
            g.vbo_cpu.add(base + 3).write_volatile(p[3]);
            g.vbo_cpu.add(base + 4).write_volatile(c.x.to_bits());
            g.vbo_cpu.add(base + 5).write_volatile(c.y.to_bits());
            g.vbo_cpu.add(base + 6).write_volatile(c.z.to_bits());
            g.vbo_cpu.add(base + 7).write_volatile(c.w.to_bits());
        }
        g.vbo_buf.flush_range(vbo_gva, vbo_dwords * 4);
        let rs = &g.rs;

        // --- Assemble the pipeline (identical to draw_triangle except geometry/draw) ---
        let mut cs = CmdStream::new();
        // The progress() PIPE_CONTROL below is a CS-stall + post-sync write; it also serves
        // as the mandatory preceding stall for WaPipeControlBeforeVFCacheInvalidationEnable
        // (Gen9: a VF-cache-invalidate PIPE_CONTROL must follow a CS-stall/post-sync one).
        self.progress(&mut cs, 0x10);
        cmd::pipe_control(
            &mut cs,
            cmd::pc::CS_STALL
                // VF_CACHE_INVALIDATE is the load-bearing fix for animation: the VBO lives at
                // a FIXED GVA and we rewrite its contents every frame. Without invalidating the
                // vertex-fetch cache the GPU keeps re-fetching frame 0's vertices, so the cube
                // renders frozen (looked static / "no rotation") despite the CPU-side rewrite.
                | cmd::pc::VF_CACHE_INVALIDATE
                | cmd::pc::STATE_CACHE_INVALIDATE
                | cmd::pc::CONSTANT_CACHE_INVALIDATE
                | cmd::pc::INSTRUCTION_CACHE_INVALIDATE,
            0,
            0,
        );
        cs.push(cmd::pipeline_select(cmd::PIPELINE_SELECT_3D));
        self.progress(&mut cs, 1);
        self.emit_state_base_address(&mut cs);
        self.progress(&mut cs, 2);
        cs.push(h1(so::PUSH_CONSTANT_ALLOC_VS, 2)).push(2);
        cs.push(h1(so::PUSH_CONSTANT_ALLOC_PS, 2)).push((2 << 16) | 2);
        let vs_entries = 64u32;
        // BISECTION #9: URB_VS alloc reverted 1 -> 0 (Phase-6 512-bit entry) to pair with the
        // 1-slot VUE (VS output length 1, position-only build_vs). Tests the 2-slot-VUE hang.
        let vs_alloc = 0u32;
        let vs_start = 4u32;
        let vs_bytes = vs_entries * (vs_alloc + 1) * 64;
        let empty_start = vs_start + (vs_bytes + 8191) / 8192;
        cs.push(h(so::URB_VS, 2)).push(vs_entries | (vs_alloc << 16) | (vs_start << 25));
        cs.push(h(so::URB_HS, 2)).push(empty_start << 25);
        cs.push(h(so::URB_DS, 2)).push(empty_start << 25);
        cs.push(h(so::URB_GS, 2)).push(empty_start << 25);
        self.progress(&mut cs, 3);
        // Vertex input: 24 verts, pitch 32 B (pos vec4 + color vec4), size = whole VBO.
        self.emit_vertex_buffers(&mut cs, vbo_gva, 32, (vbo_dwords * 4) as u32);
        self.emit_vertex_elements(&mut cs);
        cs.push(h(so::VF, 2)).push(0);
        cs.push(h(so::VF_INSTANCING, 3)).push(0).push(0);
        cs.push(h(so::VF_TOPOLOGY, 2)).push(4); // TRILIST
        // Index buffer: DWORD (u32) format = 2 at bits [41:40] (DW1 [9:8]); addr; size B.
        cs.push(h(so::INDEX_BUFFER, 5))
            .push(2 << 8)
            .push(idx_gva)
            .push(0)
            .push((idx_count * 4) as u32);
        // Shader stages: VS enabled, rest disabled (same as triangle).
        self.emit_vs(&mut cs, vs_off);
        cs.push(h(so::HS, 9));
        for _ in 0..8 { cs.push(0); }
        cs.push(h(so::DS, 11));
        for _ in 0..10 { cs.push(0); }
        cs.push(h(so::GS, 10));
        for _ in 0..9 { cs.push(0); }
        cs.push(h(so::TE, 4));
        for _ in 0..3 { cs.push(0); }
        cs.push(h(so::STREAMOUT, 5));
        for _ in 0..4 { cs.push(0); }
        self.progress(&mut cs, 4);
        cs.push(h(so::CLIP, 4)).push(1 << 10).push((1 << 31) | (1 << 28) | (1 << 26)).push(0);
        cs.push(h(so::SF, 4)).push((1 << 1) | (1 << 10)).push(0).push(0);
        // Step 2 — back-face culling. Cull Mode[49:48] -> DW1[17:16]: BACK=3 (was NONE=1).
        // Front Winding[53] -> DW1[21]: Clockwise=0. Our cube's outward faces are CCW in
        // MODEL space, but sf_clip_viewport flips Y (m11=-h/2) so they become CW in SCREEN
        // space, and Gen9 classifies front/back from post-viewport screen-space area. Hence
        // Front Winding=CW(0) marks the outward faces as front; Cull BACK removes the 6
        // faces pointing away. Expected: CL_prims 12 -> 6. If the cube shows HOLES instead
        // (outward faces culled), flip to Front Winding=CCW: .push((3<<16)|(1<<21)).
        cs.push(h(so::RASTER, 5)).push(3 << 16).push(0).push(0).push(0); // cull BACK, front=CW
        // SBE (Boot A): route ONE SF output attribute (the color varying). DW1 bits:
        //   [10:5]=Read Offset 1 (skip the 256-bit VUE header+position -> attr at VUE DW8),
        //   [15:11]=Read Length 1, [27:22]=Number of SF Output Attributes = 1 (1<<22),
        //   [28]/[29]=Force Read Length/Offset. DW3 (bits[127:96]) = Constant Interpolation
        //   Enable, attr-0 bit set (flat: a0=provoking-vertex value, a1=a2=0 -> no PLN).
        //   [53]=Attribute Swizzle Enable (DW1 bit 21) -> must be ON so the paired
        //   3DSTATE_SBE_SWIZ table below actually routes SF output attr 0 <- URB source attr 0.
        // BOOT #13: hang is fully fixed (split 2-slot URB write). Re-enable the FLAT attribute
        // path. DW1: Read Offset 1 (1<<5) + Read Length 1 (1<<11) + Attribute Swizzle Enable
        // (1<<21) + Number of SF Output Attributes = 1 (1<<22) + Force Read Length/Offset
        // (1<<28|1<<29). Barycentric stays OFF (flat, a0 only) so PS setup lands at g2/g3.
        cs.push(h(so::SBE, 6)).push((1 << 5) | (1 << 11) | (1 << 21) | (1 << 22) | (1 << 28) | (1 << 29));
        cs.push(0);            // DW2 = Point Sprite Texture Coordinate Enable (none)
        cs.push(0);            // DW3 = Constant Interpolation Enable OFF (Boot B: smooth/pln)
        // DW4 = "Attribute Active Component Format" group (genxml: 32 attrs x 2 bits, starting
        // at bit 128). Attribute 0 = DW4[1:0]. LOAD-BEARING: this MUST be XYZW(3), not the
        // DISABLED(0) default. Number-of-SF-Output-Attributes=1 told the SF to emit one
        // attribute, but leaving its Active Component Format = DISABLED contradicts that: the
        // SF then materializes ZERO components, so (a) the PS setup GRFs g2/g3 are never
        // populated -> build_ps_attr reads per-subspan garbage (the "strided noise"), and
        // (b) the inconsistent SF state wedges the pipeline on the following draw (the
        // no-fault 3DPRIMITIVE hang at progress=0x5). Mesa/iris genX(upload_sbe) always sets
        // ACTIVE_COMPONENT_XYZW for every enabled attribute. Phases 5-6 used num_attr=0 so
        // this group was don't-care -> the gap only surfaced now (Boot A's first nonzero attr).
        cs.push(3).push(0);   // DW4 = attr0 Active Component Format XYZW(3); DW5 attrs16-31 off
        // 3DSTATE_SBE_SWIZ (11 dw): the per-attribute source/swizzle table Mesa ALWAYS emits
        // next to 3DSTATE_SBE but we never had. genxml: DW1..DW8 = 16 x SF_OUTPUT_ATTRIBUTE_DETAIL
        // (16 bits each, 2/dword); DW9..DW10 = 16 x 4-bit Attribute Wrap. Attribute 0 detail = 0
        // means Source Attribute 0, Swizzle Select INPUTATTR, no Constant Source, no Component
        // Override -> SF output attr 0 passes through URB source attr 0 (our color at Read
        // Offset 1 = VUE DW8) with all 4 components. This is the prime suspect for BOTH the
        // "a0 reads zero regardless of VUE contents" AND the persistent no-fault 3DPRIMITIVE
        // hang: with num_attr=1 but the swizzle table never programmed, the SF's source routing
        // for output attr 0 is undefined -> it materializes zero AND wedges the pipeline.
        // BISECTION #7: SBE_SWIZ still emitted (all-zero table is harmless with num_attr=0).
        cs.push(h(so::SBE_SWIZ, 11));
        for _ in 0..10 { cs.push(0); }
        // 3DSTATE_WM DW1: enable BIM_PERSPECTIVE_PIXEL barycentric (genxml Barycentric
        // Interpolation Mode [48:43], value 1 -> DW1 bit 11) IN ADDITION to bit 31.
        // LOAD-BEARING for the HANG: PS_EXTRA Attribute Enable tells the PSD the pixel shader
        // consumes interpolated attributes, but with Barycentric Interpolation Mode = 0 the SF
        // delivers no barycentric setup -> the PS-dispatch handshake never completes and the
        // post-draw CS-stall never drains (hang at progress 5->6, FAULT=0). Mesa never enables
        // attribute input without >=1 barycentric mode. Tested with the PS hardcoded to orange
        // so this isolates the hang (color is unaffected). NOTE: enabling a barycentric mode
        // inserts the R3-R4 perspective-pixel phase into the PS payload, which SHIFTS the
        // attribute setup a0/a1/a2 to a higher GRF -> when the real attribute read is restored,
        // its source register must move down from g2/g3 accordingly (that's Boot B's job).
        // Boot B: enable BIM_PERSPECTIVE_PIXEL barycentric (genxml Barycentric Interpolation Mode
        // [48:43]=1 -> DW1 bit 11) so the SF delivers per-pixel b1/b2 at PS payload g2/g3 for pln.
        cs.push(h(so::WM, 2)).push((1 << 31) | (1 << 11));
        cs.push(h(so::VIEWPORT_SF_CLIP, 2)).push(rs.sf_clip_viewport_off);
        cs.push(h(so::VIEWPORT_CC, 2)).push(rs.cc_viewport_off);
        cs.push(h1(so::DRAWING_RECTANGLE, 4)).push(0).push(((height - 1) << 16) | (width - 1)).push(0);
        // PS-adjacent state. Boot A: set PS_EXTRA Attribute Enable (bit 40 -> DW1 bit 8) in
        // addition to Pixel Shader Valid (bit 63 -> DW1 bit 31), so the PS is told it consumes
        // per-vertex attributes (the color varying delivered as plane setup at g2/g3).
        cs.push(h(so::PS_EXTRA, 2)).push((1u32 << 31) | (1 << 8)); // PS Valid + Attribute Enable
        cs.push(h(so::PS_BLEND, 2)).push(1u32 << 30);
        cs.push(h(so::CC_STATE_POINTERS, 2)).push(rs.cc_off | 1);
        cs.push(h(so::BLEND_STATE_POINTERS, 2)).push(rs.blend_off | 1);
        cs.push(h(so::WM_DEPTH_STENCIL, 4)).push(0).push(0).push(0); // depth off (convex + cull)
        cs.push(h(so::MULTISAMPLE, 2)).push(0);
        cs.push(h(so::SAMPLE_MASK, 2)).push(1);
        cs.push(h(so::DEPTH_BUFFER, 8)).push(7u32 << 29);
        for _ in 0..6 { cs.push(0); } // SURFTYPE_NULL
        cs.push(h(so::BINDING_TABLE_POINTERS_PS, 2)).push(rs.binding_table_off);
        // Boot C: point the PS sampler-state table at our NEAREST/CLAMP SAMPLER_STATE. The
        // field is Pointer to PS Sampler State [63:37] = DW1[31:5]; sampler_off is 32B-aligned
        // so its low 5 bits are already 0 and it drops straight into the field.
        cs.push(h(so::SAMPLER_STATE_POINTERS_PS, 2)).push(sampler_off);
        self.progress(&mut cs, 5);
        // Boot #8 showed 6 draws STILL hang -> draw count is not the cause. Back to ONE indexed
        // 36-draw (clean geometry). Boot #9 tests the LAST Set-B suspect: the 2-slot VUE, by
        // reverting the VUE to Phase-6 1-slot (position-only VS, output length 1, URB alloc 0).
        self.emit_ps(&mut cs, ps_off);
        cs.push(super::cmd::gfxpipe_header(PIPELINE_3D, 3, 0, 7 - 2));
        cs.push(4 | (1 << 8)); // TRILIST topology + Vertex Access Type = RANDOM
        cs.push(idx_count as u32); // Vertex Count Per Instance (all 36 indices)
        cs.push(0); // Start Index Location
        cs.push(1); // Instance Count
        cs.push(0); // Start Instance
        cs.push(0); // Base Vertex
        self.progress(&mut cs, 6);
        cmd::pipe_control(
            &mut cs,
            cmd::pc::CS_STALL | cmd::pc::RENDER_TARGET_CACHE_FLUSH | cmd::pc::DC_FLUSH_ENABLE,
            0,
            0,
        );
        self.progress(&mut cs, 7);
        const DONE: u32 = 0x00D0_5EED;
        cmd::pipe_control(
            &mut cs,
            cmd::pc::CS_STALL | cmd::pc::POST_SYNC_WRITE_IMM | cmd::pc::DEST_ADDRESS_GTT,
            GVA_RCS_FENCE,
            DONE as u64,
        );
        for _ in 0..16 { cs.push(cmd::MI_NOOP); }

        if verbose {
            crate::serial_println!("[DRAW] submitting {} dwords ({} verts, {} indices)",
                cs.len(), g.verts.len(), idx_count);
            super::decode::decode_and_print(cs.as_slice());
        }
        self.fence_virt.write_volatile(0);
        self.flush_line(self.fence_virt as usize);
        self.rcs_submit(cs.as_slice())?;

        // Wait for completion / detect hang (reuses the triangle's fault-dump logic).
        let mut t = 0u32;
        let mut last = 0u32;
        let mut hung = false;
        loop {
            self.flush_line(self.fence_virt as usize);
            let v = self.fence_virt.read_volatile();
            if v != last { last = v; }
            if v == DONE { break; }
            core::hint::spin_loop();
            t += 1;
            if t > 30_000_000 {
                let fault = self.read_reg(super::RENDER_FAULT_REG);
                crate::serial_println!(
                    "[CUBE] HUNG. last_progress={:#x} FAULT={:#010x} HEAD={:#x}",
                    last, fault, self.read_reg(super::RENDER_RING_HEAD)
                );
                if fault & 1 != 0 {
                    let d0 = self.read_reg(0x4b10);
                    let d1 = self.read_reg(0x4b14);
                    let va = ((d0 as u64) << 12) | (((d1 & 0xF) as u64) << 44);
                    crate::serial_println!("[CUBE]  FAULT_VA={:#x} (D0={:#010x} D1={:#010x})", va, d0, d1);
                }
                hung = true;
                break;
            }
        }

        if verbose {
            let ia = self.read_reg(0x2310);
            let vs = self.read_reg(0x2320);
            let cl = self.read_reg(0x2338);
            let clp = self.read_reg(0x2340);
            let ps = self.read_reg(0x2348);
            crate::serial_println!(
                "[CUBE] STATS: IA_verts={} VS_inv={} CL_inv={} CL_prims={} PS_inv={}",
                ia, vs, cl, clp, ps
            );
            // Step 2 gate: back-face cull is applied in SF/setup, AFTER the clipper, so
            // CL_PRIMITIVES(0x2340) stays 12 (clipper sees all tris). The counter that
            // reflects culling is PS_inv: culled tris never reach the PS. Baseline (cull
            // off) was 329108; with CULLMODE_BACK the 6 back faces drop it to ~half.
            // ~half => cull OK; ~full(329k) => cull not applied; 0 => winding inverted.
            crate::serial_println!(
                "[CUBE] cull check: PS_inv={} (expect ~half of 329108; full=cull off, 0=winding inverted)",
                ps
            );
        }

        if present {
            self.present_to_scanout(bb_cpu, bytes);
        }
        if hung { return Err(RenderError::EngineHang); }
        Ok(())
    }

    /// The perspective * view (camera) matrix used by the demo cube: eye at z=3 looking at
    /// the origin, 60-degree vertical FOV. Split out so both draw_mesh and spin_cube share it
    /// and it reads as the caller-owned half of the MVP (Phase 8: transforms are inputs).
    fn demo_camera(width: u32, height: u32) -> super::math::Mat4 {
        use super::math::{self, Mat4, Vec3};
        let aspect = width as f32 / height as f32;
        let proj = Mat4::perspective(math::radians(60.0), aspect, 0.1, 100.0);
        let view = Mat4::look_at(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        proj.mul(&view)
    }

    /// The cube's model (spin) matrix at rotation `angle`: tumble about Y and, more slowly, X.
    fn demo_spin(angle: f32) -> super::math::Mat4 {
        use super::math::Mat4;
        Mat4::rotate_y(angle).mul(&Mat4::rotate_x(angle * 0.6))
    }

    /// One-shot textured-cube draw: upload the cube mesh + checkerboard once and render a
    /// single static frame at `angle`. Thin wrapper over the generic upload_mesh + draw API.
    pub unsafe fn draw_mesh(
        &mut self,
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        angle: f32,
        present: bool,
        verbose: bool,
    ) -> Result<(), RenderError> {
        use super::engine::{cube_mesh, Texture};
        let mesh = cube_mesh();
        let tex = Texture::checkerboard(64, 8);
        let mut g = self.upload_mesh(&mesh, Some(&tex), rt_gva, width, height, pitch)?;
        let mvp = Self::demo_camera(width, height).mul(&Self::demo_spin(angle));
        self.draw(&mut g, &mvp, rt_gva, bb_cpu, width, height, pitch, present, verbose)
    }

    /// Phase 6/7 demo: spin the textured cube. Uploads GPU resources ONCE via the generic
    /// engine API, then renders `frames` frames at increasing rotation, computing a fresh MVP
    /// per frame and presenting each to the scanout. Only the first frame logs stats / decodes
    /// the stream. Reusing the buffers every frame is what makes the animation loop viable.
    pub unsafe fn spin_cube(
        &mut self,
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        frames: u32,
    ) -> Result<(), RenderError> {
        use super::engine::{cube_mesh, Texture};
        let mesh = cube_mesh();
        let tex = Texture::checkerboard(64, 8);
        let mut g = self.upload_mesh(&mesh, Some(&tex), rt_gva, width, height, pitch)?;
        let camera = Self::demo_camera(width, height);
        for f in 0..frames {
            let angle = f as f32 * 0.03; // ~0.03 rad/frame -> a smooth, steady tumble
            let verbose = f == 0; // stats/decode on the first frame only
            let mvp = camera.mul(&Self::demo_spin(angle));
            self.draw(&mut g, &mvp, rt_gva, bb_cpu, width, height, pitch, true, verbose)?;
            // Heartbeat: prove the loop advances the angle (and at what cadence) in the boot
            // log, independent of what the panel shows. With the VF-cache invalidate in place
            // each logged angle should correspond to a visibly different cube pose.
            if f % 60 == 0 {
                crate::serial_println!("[CUBE] frame {}/{} angle={}.{:02} rad",
                    f, frames, angle as i32, ((angle - angle as i32 as f32) * 100.0) as i32);
            }
            crate::time::sleep_ms(12); // pace the spin so it reads as motion, not a blur
        }
        crate::serial_println!("[CUBE] spin complete: {} frames rendered + presented", frames);
        Ok(())
    }

    /// Phase 8b: allocate the SHARED state buffers for a multi-mesh scene and build the shared
    /// render state (RT surface for BTI 0, blend/cc/viewports) + one NEAREST/CLAMP sampler.
    /// Meshes are added with [`Scene::add_mesh`] and drawn with [`RenderEngine::draw_scene`].
    /// Unlike upload_mesh (one buffer set per mesh, colliding GVAs), a Scene sub-allocates every
    /// mesh out of ONE set of buffers under ONE STATE_BASE_ADDRESS.
    pub unsafe fn create_scene(
        &mut self,
        rt_gva: u32,
        width: u32,
        height: u32,
        pitch: u32,
        rt_tile_mode: u32,
    ) -> Result<super::engine::Scene, RenderError> {
        use super::engine::Scene;
        let (vs_off, ps_off) = match &self.kernels {
            Some(k) => (k.vs_off, k.ps_off),
            None => return Err(RenderError::NotInitialized),
        };
        let mmio = self.mmio_base;
        // Shared buffers: surf (RT surf + per-mesh tex surfaces + binding tables), dyn (blend/cc/
        // viewport + sampler), vbo (per-mesh vertex + index blocks), tex (8 pages = two 64x64
        // textures). Sized generously; the bump allocator packs meshes in order.
        let mut surf_buf = StateBuffer::new(mmio, GVA_SURFACE_STATE, 1)?;
        let mut dyn_buf = StateBuffer::new(mmio, GVA_DYNAMIC_STATE, 1)?;
        let vbo_buf = StateBuffer::new(mmio, GVA_VERTEX_BUFFERS, 2)?;
        let tex_buf = StateBuffer::new(mmio, super::GVA_TEXTURES, 8)?;
        // `pitch` here is the RT ROW pitch (bytes): the Y-tiled pitch when rt_tile_mode=YMAJOR
        // (a 128B multiple), else the linear scanout pitch. The RT surface points BTI 0 at it.
        let rs = state::setup_render_state(&mut surf_buf, &mut dyn_buf, rt_gva, width, height, pitch, rt_tile_mode)?;
        // One shared sampler for every mesh (NEAREST/CLAMP), 32B-aligned per the SSP field.
        let samp_gva = dyn_buf.write(&state::sampler_state_nearest_clamp(), 32)?;
        let sampler_off = samp_gva - dyn_buf.gva;
        crate::serial_println!(
            "[SCENE] created: surf_base={:#x} dyn_base={:#x} sampler_off={:#x}",
            surf_buf.gva, dyn_buf.gva, sampler_off
        );
        Ok(Scene {
            surf_buf,
            dyn_buf,
            vbo_buf,
            tex_buf,
            rs,
            sampler_off,
            resolve_sampler_off: 0, // set by add_resolve_quad (Boot-3 SSAA bilinear downsample)
            resolve_sf_clip_off: 0, // set by add_resolve_quad (display-dims resolve viewport)
            vs_off,
            ps_off,
            meshes: alloc::vec::Vec::new(),
            resolve: None,
        })
    }

    /// Phase 8b: render ALL meshes in `scene` (each transformed by its matching entry in `mvps`)
    /// into `rt_gva` as ONE fenced command stream. The full fixed-function pipeline state is
    /// emitted ONCE (shared), then a per-mesh tail swaps VERTEX_BUFFERS / INDEX_BUFFER /
    /// BINDING_TABLE_POINTERS_PS / 3DSTATE_PS and issues a 3DPRIMITIVE — the same pipelined-
    /// state-swap pattern Phase 6 Step 4 proved for 6 per-face PS kernels. `mvps.len()` must
    /// equal `scene.meshes.len()`.
    /// `bb_cpu` is the CPU-visible base of the render-target backing store (linear OR Y-tiled).
    /// When `tiled_pitch != 0` the RT is Y-tiled: the whole `rt_buf_bytes` region is cleared and
    /// the present path DE-TILES into the linear scanout (`pitch` = scanout row pitch). When
    /// `tiled_pitch == 0` the RT is linear (`pitch` is its row pitch, `rt_buf_bytes` ignored).
    pub unsafe fn draw_scene(
        &mut self,
        scene: &mut super::engine::Scene,
        mvps: &[super::math::Mat4],
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        tiled_pitch: u32,
        rt_buf_bytes: usize,
        present: bool,
        verbose: bool,
    ) -> Result<(), RenderError> {
        use super::math::Vec4;
        let _ = rt_gva;

        // Clear the render target to black. Black = all-zero bytes, which is layout-independent,
        // so a Y-tiled RT is cleared by simply zeroing its whole backing store.
        const CLEAR_BYTE: u8 = 0x00;
        let clear_bytes = if tiled_pitch != 0 { rt_buf_bytes } else { (pitch * height) as usize };
        core::ptr::write_bytes(bb_cpu as *mut u8, CLEAR_BYTE, clear_bytes);
        let mut off = 0usize;
        while off < clear_bytes {
            self.flush_line(bb_cpu as usize + off);
            off += 64;
        }

        // CPU-transform each mesh's model-space verts -> clip space into its own vbo block.
        for (mi, mesh) in scene.meshes.iter().enumerate() {
            let mvp = &mvps[mi];
            for (i, v) in mesh.verts.iter().enumerate() {
                let clip = mvp.mul_vec4(Vec4::from_point(v.pos));
                if verbose && mi == 0 && i == 0 {
                    // Actual clip coords fed to the GPU (x1000, integer-formatted to dodge no_std
                    // float fmt). Sane cube v0 ~ (±small, ±small, 0..w, w>0); if w<=0 or wild, the
                    // MVP/vertex data is bad; if sane but ps=0, the failure is rasterizer state.
                    crate::serial_println!(
                        "[DS] mesh0 v0 clipx1e3=({},{},{},{})",
                        (clip.x * 1000.0) as i32, (clip.y * 1000.0) as i32,
                        (clip.z * 1000.0) as i32, (clip.w * 1000.0) as i32
                    );
                }
                let p = clip.to_bits();
                let c = v.varying;
                let base = i * 8;
                mesh.vbo_cpu.add(base).write_volatile(p[0]);
                mesh.vbo_cpu.add(base + 1).write_volatile(p[1]);
                mesh.vbo_cpu.add(base + 2).write_volatile(p[2]);
                mesh.vbo_cpu.add(base + 3).write_volatile(p[3]);
                mesh.vbo_cpu.add(base + 4).write_volatile(c.x.to_bits());
                mesh.vbo_cpu.add(base + 5).write_volatile(c.y.to_bits());
                mesh.vbo_cpu.add(base + 6).write_volatile(c.z.to_bits());
                mesh.vbo_cpu.add(base + 7).write_volatile(c.w.to_bits());
            }
            scene.vbo_buf.flush_range(mesh.vbo_gva, mesh.vbo_dwords * 4);
        }

        let rs = &scene.rs;
        let vs_off = scene.vs_off;
        let sampler_off = scene.sampler_off;

        // Shared fixed-function prologue (extracted so the resolve pass reuses the exact bytes).
        let mut cs = CmdStream::new();
        // Mesh pass: shared NEAREST sampler + rs viewport (baked at the RT's — supersampled — dims).
        self.emit_pipeline_prologue(&mut cs, rs, vs_off, sampler_off, rs.sf_clip_viewport_off, width, height, 3 << 16); // cull BACK

        // Per-mesh tail: swap VB / IB / binding table / PS and draw. Pipelined 3DSTATE, legal to
        // change between draws in one stream (proven: Phase 6 Step 4).
        for (mi, mesh) in scene.meshes.iter().enumerate() {
            self.emit_draw_one(&mut cs, mesh, 0x20 + mi as u32);
        }

        let hung = self.finish_submit_and_wait(&mut cs, verbose, scene.meshes.len());

        if present {
            if tiled_pitch != 0 {
                // The RT is Y-tiled: de-swizzle it into the linear scanout as we copy.
                self.present_to_scanout_detiled(bb_cpu, width, height, pitch, tiled_pitch);
            } else {
                self.present_to_scanout(bb_cpu, (pitch * height) as usize);
            }
        }
        if hung { return Err(RenderError::EngineHang); }
        Ok(())
    }

    /// Emit the shared fixed-function pipeline prologue (everything up to and including the
    /// sampler pointer, but NO per-draw VB/IB/BT/PS/3DPRIMITIVE). Byte-identical to the proven
    /// Boot-1 scene stream; shared by draw_scene and draw_resolve.
    unsafe fn emit_pipeline_prologue(
        &self,
        cs: &mut CmdStream,
        rs: &state::RenderState,
        vs_off: u32,
        sampler_off: u32,
        sf_clip_off: u32,
        width: u32,
        height: u32,
        raster_cull_dw1: u32,
    ) {
        self.progress(cs, 0x10);
        cmd::pipe_control(
            cs,
            cmd::pc::CS_STALL
                | cmd::pc::VF_CACHE_INVALIDATE
                | cmd::pc::STATE_CACHE_INVALIDATE
                | cmd::pc::CONSTANT_CACHE_INVALIDATE
                // TEXTURE_CACHE_INVALIDATE is load-bearing for the resolve pass: it samples the
                // offscreen scene RT, which is re-rendered every frame at a FIXED GVA. Without it
                // the sampler returns stale texels from a prior frame -> a frozen resolve image
                // (same failure class as the Phase-6 VF-cache freeze). Harmless for the mesh pass
                // (its checkerboard textures are static, just re-fetched once).
                | cmd::pc::TEXTURE_CACHE_INVALIDATE
                | cmd::pc::INSTRUCTION_CACHE_INVALIDATE
                // TLB_INVALIDATE flushes the RCS engine's INTERNAL GGTT TLB in-stream. The chipset
                // GTT flush (GFX_FLSH_CNTL, invalidate_ggtt_tlb) does NOT cover the per-engine TLB, so
                // when the GL path re-maps the fixed state GVAs the boot self-test first populated, the
                // engine keeps serving stale translations -> garbage viewport/VBO reads -> zero
                // coverage (CL>0, PS_inv=0, black). Legal here: this PIPE_CONTROL has CS_STALL and no
                // post-sync write, and does not flush RT/depth. The boot self-test uses the same
                // prologue, so if this were unsafe its ring-0 cube would break too.
                | cmd::pc::TLB_INVALIDATE,
            0,
            0,
        );
        cs.push(cmd::pipeline_select(cmd::PIPELINE_SELECT_3D));
        self.progress(cs, 1);
        self.emit_state_base_address(cs);
        self.progress(cs, 2);
        cs.push(h1(so::PUSH_CONSTANT_ALLOC_VS, 2)).push(2);
        cs.push(h1(so::PUSH_CONSTANT_ALLOC_PS, 2)).push((2 << 16) | 2);
        let vs_entries = 64u32;
        let vs_alloc = 0u32;
        let vs_start = 4u32;
        let vs_bytes = vs_entries * (vs_alloc + 1) * 64;
        let empty_start = vs_start + (vs_bytes + 8191) / 8192;
        cs.push(h(so::URB_VS, 2)).push(vs_entries | (vs_alloc << 16) | (vs_start << 25));
        cs.push(h(so::URB_HS, 2)).push(empty_start << 25);
        cs.push(h(so::URB_DS, 2)).push(empty_start << 25);
        cs.push(h(so::URB_GS, 2)).push(empty_start << 25);
        self.progress(cs, 3);
        self.emit_vertex_elements(cs);
        cs.push(h(so::VF, 2)).push(0);
        cs.push(h(so::VF_INSTANCING, 3)).push(0).push(0);
        cs.push(h(so::VF_TOPOLOGY, 2)).push(4); // TRILIST
        self.emit_vs(cs, vs_off);
        cs.push(h(so::HS, 9));
        for _ in 0..8 { cs.push(0); }
        cs.push(h(so::DS, 11));
        for _ in 0..10 { cs.push(0); }
        cs.push(h(so::GS, 10));
        for _ in 0..9 { cs.push(0); }
        cs.push(h(so::TE, 4));
        for _ in 0..3 { cs.push(0); }
        cs.push(h(so::STREAMOUT, 5));
        for _ in 0..4 { cs.push(0); }
        self.progress(cs, 4);
        cs.push(h(so::CLIP, 4)).push(1 << 10).push((1 << 31) | (1 << 28) | (1 << 26)).push(0);
        cs.push(h(so::SF, 4)).push((1 << 1) | (1 << 10)).push(0).push(0);
        // Cull mode is caller-supplied: meshes use BACK (3<<16); the full-screen resolve quad
        // uses NONE (1<<16) because its identity transform inverts screen winding vs the meshes'
        // winding-flipping MVP, and a blit quad must never be culled.
        cs.push(h(so::RASTER, 5)).push(raster_cull_dw1).push(0).push(0).push(0);
        cs.push(h(so::SBE, 6)).push((1 << 5) | (1 << 11) | (1 << 21) | (1 << 22) | (1 << 28) | (1 << 29));
        cs.push(0);
        cs.push(0);
        cs.push(3).push(0);
        cs.push(h(so::SBE_SWIZ, 11));
        for _ in 0..10 { cs.push(0); }
        cs.push(h(so::WM, 2)).push((1 << 31) | (1 << 11));
        // Viewport is caller-supplied: the mesh pass uses rs.sf_clip_viewport_off (baked at the
        // supersampled RT dims); the resolve pass passes its own display-dims viewport so the blit
        // covers the whole backbuffer. `width`/`height` (DRAWING_RECTANGLE below) match accordingly.
        cs.push(h(so::VIEWPORT_SF_CLIP, 2)).push(sf_clip_off);
        cs.push(h(so::VIEWPORT_CC, 2)).push(rs.cc_viewport_off);
        cs.push(h1(so::DRAWING_RECTANGLE, 4)).push(0).push(((height - 1) << 16) | (width - 1)).push(0);
        cs.push(h(so::PS_EXTRA, 2)).push((1u32 << 31) | (1 << 8));
        cs.push(h(so::PS_BLEND, 2)).push(1u32 << 30);
        cs.push(h(so::CC_STATE_POINTERS, 2)).push(rs.cc_off | 1);
        cs.push(h(so::BLEND_STATE_POINTERS, 2)).push(rs.blend_off | 1);
        cs.push(h(so::WM_DEPTH_STENCIL, 4)).push(0).push(0).push(0);
        cs.push(h(so::MULTISAMPLE, 2)).push(0);
        cs.push(h(so::SAMPLE_MASK, 2)).push(1);
        cs.push(h(so::DEPTH_BUFFER, 8)).push(7u32 << 29);
        for _ in 0..6 { cs.push(0); }
        cs.push(h(so::SAMPLER_STATE_POINTERS_PS, 2)).push(sampler_off);
    }

    /// Emit one indexed draw (VB + IB + binding table + PS + 3DPRIMITIVE) for `mesh`, tagged with
    /// `progress_marker` so a mid-stream hang localizes the draw. Byte-identical to the proven
    /// Boot-1 per-mesh tail; shared by draw_scene and draw_resolve.
    unsafe fn emit_draw_one(&self, cs: &mut CmdStream, mesh: &super::engine::SceneMesh, progress_marker: u32) {
        self.emit_vertex_buffers(cs, mesh.vbo_gva, 32, (mesh.vbo_dwords * 4) as u32);
        cs.push(h(so::INDEX_BUFFER, 5))
            .push(2 << 8)
            .push(mesh.idx_gva)
            .push(0)
            .push((mesh.idx_count * 4) as u32);
        cs.push(h(so::BINDING_TABLE_POINTERS_PS, 2)).push(mesh.binding_table_off);
        self.emit_ps(cs, mesh.ps_off);
        self.progress(cs, progress_marker);
        cs.push(super::cmd::gfxpipe_header(PIPELINE_3D, 3, 0, 7 - 2));
        cs.push(4 | (1 << 8)); // TRILIST + Vertex Access Type = RANDOM
        cs.push(mesh.idx_count as u32);
        cs.push(0); // Start Index Location
        cs.push(1); // Instance Count
        cs.push(0); // Start Instance
        cs.push(0); // Base Vertex
    }

    /// Emit the shared epilogue (RT flush + fence), submit the stream, and spin-wait for the DONE
    /// fence (with the fault dump on hang). Returns true if it hung. Shared by draw_scene and
    /// draw_resolve. `count` is logged as the draw/mesh count.
    unsafe fn finish_submit_and_wait(&mut self, cs: &mut CmdStream, verbose: bool, count: usize) -> bool {
        self.progress(cs, 6);
        cmd::pipe_control(
            cs,
            cmd::pc::CS_STALL | cmd::pc::RENDER_TARGET_CACHE_FLUSH | cmd::pc::DC_FLUSH_ENABLE,
            0,
            0,
        );
        self.progress(cs, 7);
        const DONE: u32 = 0x00D0_5EED;
        cmd::pipe_control(
            cs,
            cmd::pc::CS_STALL | cmd::pc::POST_SYNC_WRITE_IMM | cmd::pc::DEST_ADDRESS_GTT,
            GVA_RCS_FENCE,
            DONE as u64,
        );
        for _ in 0..16 { cs.push(cmd::MI_NOOP); }

        if verbose {
            crate::serial_println!("[SCENE] submitting {} dwords, {} draws", cs.len(), count);
            super::decode::decode_and_print(cs.as_slice());
        }
        self.fence_virt.write_volatile(0);
        self.flush_line(self.fence_virt as usize);
        if self.rcs_submit(cs.as_slice()).is_err() {
            return true;
        }

        let mut t = 0u32;
        let mut last = 0u32;
        loop {
            self.flush_line(self.fence_virt as usize);
            let v = self.fence_virt.read_volatile();
            if v != last { last = v; }
            if v == DONE { break; }
            core::hint::spin_loop();
            t += 1;
            if t > 30_000_000 {
                let fault = self.read_reg(super::RENDER_FAULT_REG);
                crate::serial_println!(
                    "[SCENE] HUNG. last_progress={:#x} FAULT={:#010x} HEAD={:#x}",
                    last, fault, self.read_reg(super::RENDER_RING_HEAD)
                );
                if fault & 1 != 0 {
                    let d0 = self.read_reg(0x4b10);
                    let d1 = self.read_reg(0x4b14);
                    let va = ((d0 as u64) << 12) | (((d1 & 0xF) as u64) << 44);
                    crate::serial_println!("[SCENE]  FAULT_VA={:#x} (D0={:#010x} D1={:#010x})", va, d0, d1);
                }
                // Reset the render domain so the engine doesn't stay wedged for every following frame
                // (a hung RCS with a half-consumed ring never drains, so the next submit hangs too —
                // that repetition is what pins the CPU and overheats the box). ensure_ready() re-arms
                // the ring on the next call (CTL reads 0 after GDRST).
                if self.reset_render().is_err() {
                    crate::serial_println!("[SCENE]  reset_render after hang FAILED");
                }
                return true;
            }
        }
        if verbose {
            let clp = self.read_reg(0x2340);
            let ps = self.read_reg(0x2348);
            crate::serial_println!("[SCENE] STATS: CL_prims={} PS_inv={}", clp, ps);
        }
        false
    }

    /// MSAA-track Boot 2: the resolve pass. Draws the full-screen quad (bound to the offscreen
    /// scene RT as a texture, writing the LINEAR backbuffer via its binding table), then presents
    /// the backbuffer. This boot the quad's PS is the plain textured PS (1:1 copy — no visual
    /// change, jaggies remain); Boot 3 makes the scene RT multisampled and averages samples here.
    pub unsafe fn draw_resolve(
        &mut self,
        scene: &mut super::engine::Scene,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        present: bool,
        verbose: bool,
    ) -> Result<(), RenderError> {
        use super::math::Vec4;
        let resolve = match &scene.resolve {
            Some(r) => r,
            None => return Err(RenderError::NotInitialized),
        };

        // The quad's positions are already clip-space (w=1); write them once with an identity
        // transform. (Cheap to redo each frame; keeps the vbo warm for the VF cache invalidate.)
        for (i, v) in resolve.verts.iter().enumerate() {
            let pv = Vec4::from_point(v.pos);
            if verbose && i == 0 {
                // Static full-screen quad v0 must be ~(-1000,-1000,0,1000) (NDC corner, w=1). If it's
                // anything else, the resolve VBO/geometry is corrupt; if it's correct but res_ps=0,
                // the resolve rasterizer state (viewport/drawing-rect ptr) is at fault.
                crate::serial_println!(
                    "[DR] resolve v0 posx1e3=({},{},{},{}) vbo_gva={:#x}",
                    (pv.x * 1000.0) as i32, (pv.y * 1000.0) as i32,
                    (pv.z * 1000.0) as i32, (pv.w * 1000.0) as i32, resolve.vbo_gva
                );
            }
            let p = pv.to_bits();
            let c = v.varying;
            let base = i * 8;
            resolve.vbo_cpu.add(base).write_volatile(p[0]);
            resolve.vbo_cpu.add(base + 1).write_volatile(p[1]);
            resolve.vbo_cpu.add(base + 2).write_volatile(p[2]);
            resolve.vbo_cpu.add(base + 3).write_volatile(p[3]);
            resolve.vbo_cpu.add(base + 4).write_volatile(c.x.to_bits());
            resolve.vbo_cpu.add(base + 5).write_volatile(c.y.to_bits());
            resolve.vbo_cpu.add(base + 6).write_volatile(c.z.to_bits());
            resolve.vbo_cpu.add(base + 7).write_volatile(c.w.to_bits());
        }
        scene.vbo_buf.flush_range(resolve.vbo_gva, resolve.vbo_dwords * 4);

        // Clear the backbuffer to black first: the full-screen quad should overwrite every pixel,
        // but clearing makes any coverage gap (e.g. the quad culled/mis-sized) show as BLACK rather
        // than stale garbage — an unambiguous failure signal.
        let bytes = (pitch * height) as usize;
        core::ptr::write_bytes(bb_cpu as *mut u8, 0u8, bytes);
        let mut off = 0usize;
        while off < bytes { self.flush_line(bb_cpu as usize + off); off += 64; }

        let mut cs = CmdStream::new();
        // Cull NONE for the full-screen blit (1<<16); its identity transform inverts screen
        // winding relative to the meshes' MVP, so BACK cull would drop it. The resolve uses the
        // BILINEAR sampler (box-averages each 2x2 supersample block) and a DISPLAY-dims viewport
        // (scene.rs viewport was baked at the larger supersampled RT dims). Fall back to the shared
        // NEAREST sampler / rs viewport if add_resolve_quad hasn't populated them (defensive; the
        // resolve path always sets both).
        let resolve_sampler = if scene.resolve_sampler_off != 0 { scene.resolve_sampler_off } else { scene.sampler_off };
        let resolve_sf_clip = if scene.resolve_sf_clip_off != 0 { scene.resolve_sf_clip_off } else { scene.rs.sf_clip_viewport_off };
        self.emit_pipeline_prologue(&mut cs, &scene.rs, scene.vs_off, resolve_sampler, resolve_sf_clip, width, height, 1 << 16);
        self.emit_draw_one(&mut cs, resolve, 0x30);
        let hung = self.finish_submit_and_wait(&mut cs, verbose, 1);

        // Present ONLY on success. draw_resolve CPU-clears bb to black up front, so presenting after
        // a hung/failed resolve would blit a solid-black frame over the whole live scanout — which is
        // precisely how a GL render failure nukes the desktop AND the on-screen sysmon boot-log we
        // rely on to diagnose it (no serial on this HW). On failure, leave the last good frame up.
        if hung { return Err(RenderError::EngineHang); }
        // The resolve wrote the LINEAR backbuffer. `present` chooses the destination: the boot
        // self-test (spin_scene) blits straight to the scanout; the windowed userspace GL path passes
        // present=false and instead has gl_render copy bb_cpu into the app's SHM window buffer, so the
        // compositor composites it (no fighting over the scanout).
        if present {
            self.present_to_scanout(bb_cpu, (pitch * height) as usize);
        }
        Ok(())
    }

    /// Phase 8b demo/boot entry: spin a cube AND a pyramid side by side in one scene. Uploads
    /// both meshes (each with its own checkerboard) into one shared buffer set, then renders
    /// `frames` frames — each mesh at its own transform, tumbling in opposite directions so they
    /// read as independent objects. Proves the mesh API is not cube-shaped and that multiple
    /// GPU-resident meshes coexist and draw in one command stream.
    pub unsafe fn spin_scene(
        &mut self,
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        frames: u32,
    ) -> Result<(), RenderError> {
        use super::engine::{cube_mesh, pyramid_mesh, Texture};
        use super::math::{Mat4, Vec3};

        // --- MSAA track, Boot 3: SSAA (2x2 supersampling). Render the scene into a Y-TILED RT that
        // is 2x the display in each axis (4x the pixels), then downsample to the display backbuffer
        // in the resolve pass. The resolve uses a BILINEAR sampler whose UVs land on 2x2 block
        // centers, so hardware box-averages the 4 supersamples per output pixel — free antialiasing
        // with no shader arithmetic (which the EU encoder can't emit). Reuses Boot-1 Y-tiling and
        // the Boot-2 render-to-texture resolve pass unchanged; only the RT is bigger + one sampler
        // flips to LINEAR. The 2x RT (~32 MB at 1080p) lives at GVA_SSAA_RT (16 MB DEPTH slot too
        // small). Tiled row pitch must be a 128B multiple; height rounds up to 32-row tiles. ---
        const SS: u32 = 2; // supersample factor per axis
        let ss_w = width * SS;
        let ss_h = height * SS;
        let tiled_pitch = (ss_w * 4 + 127) & !127;       // 128B-aligned row pitch (supersampled)
        let tiled_rows = (ss_h + 31) & !31;              // 32-row tile alignment (supersampled)
        let rt_buf_bytes = (tiled_pitch * tiled_rows) as usize;
        let pages = (rt_buf_bytes + 4095) / 4096;
        let rt_buf = StateBuffer::new(self.mmio_base, super::GVA_SSAA_RT, pages)?;
        let rt_tiled_gva = rt_buf.gva;
        let rt_tiled_cpu = rt_buf.virt;
        crate::serial_println!(
            "[SCENE] SSAA {}x Y-tiled RT: gva={:#x} {}x{} pitch={} rows={} bytes={} pages={}",
            SS, rt_tiled_gva, ss_w, ss_h, tiled_pitch, tiled_rows, rt_buf_bytes, pages
        );

        // Scene (mesh pass) is built at SUPERSAMPLED dims: RT surface, viewport, DRAWING_RECTANGLE
        // all use ss_w/ss_h so the meshes rasterize at 2x resolution into the big RT.
        let mut scene = self.create_scene(rt_tiled_gva, ss_w, ss_h, tiled_pitch, state::TILEMODE_YMAJOR)?;
        // Cube: coarse (8-texel) checker; pyramid: finer (4-texel) checker so it's obvious each
        // mesh samples its OWN texture.
        scene.add_mesh(&cube_mesh(), &Texture::checkerboard(64, 8))?;
        scene.add_mesh(&pyramid_mesh(), &Texture::checkerboard(64, 4))?;
        // Resolve quad: samples the SUPERSAMPLED scene RT (ss_w x ss_h) and writes the DISPLAY-size
        // linear backbuffer (width x height, GVA 0x1400_0000 still mapped to bb_cpu). The BILINEAR
        // sampler averages each 2x2 block -> antialiased downsample.
        let bb_gva = rt_gva; // the original linear backbuffer GVA passed into spin_scene
        scene.add_resolve_quad(rt_tiled_gva, tiled_pitch, ss_w, ss_h, bb_gva, width, height)?;

        // Camera pulled back to z=4 so both objects (centers 1.8 apart) fit in view. Aspect is the
        // display aspect (SSAA scales both axes equally, so ss_w/ss_h == width/height).
        let aspect = width as f32 / height as f32;
        let proj = Mat4::perspective(super::math::radians(60.0), aspect, 0.1, 100.0);
        let view = Mat4::look_at(
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let camera = proj.mul(&view);
        // Static placement: cube left, pyramid right; both scaled to 0.65 (half-diagonal ~0.46,
        // centers 1.8 apart -> never overlap, so back-face culling alone suffices, no depth).
        let place_cube = Mat4::translate(Vec3::new(-0.9, 0.0, 0.0)).mul(&Mat4::scale(Vec3::new(0.65, 0.65, 0.65)));
        let place_pyr = Mat4::translate(Vec3::new(0.9, 0.0, 0.0)).mul(&Mat4::scale(Vec3::new(0.65, 0.65, 0.65)));

        for f in 0..frames {
            let a = f as f32 * 0.03;
            let verbose = f == 0;
            // Cube tumbles about Y+X; pyramid spins the opposite way about Y so they look independent.
            let mvp_cube = camera.mul(&place_cube).mul(&Mat4::rotate_y(a).mul(&Mat4::rotate_x(a * 0.6)));
            let mvp_pyr = camera.mul(&place_pyr).mul(&Mat4::rotate_y(-a * 0.8));
            let mvps = [mvp_cube, mvp_pyr];
            // Pass 1: render the meshes into the Y-tiled offscreen scene RT at SUPERSAMPLED dims
            // (ss_w x ss_h; no present). `pitch` here is unused for a tiled RT (clear uses
            // rt_buf_bytes), so the display scanout pitch is fine to pass through.
            self.draw_scene(&mut scene, &mvps, rt_tiled_gva, rt_tiled_cpu, ss_w, ss_h,
                pitch, tiled_pitch, rt_buf_bytes, false, verbose)?;
            // Pass 2: resolve — box-downsample the supersampled scene RT with the full-screen quad,
            // write the DISPLAY-size linear backbuffer, and present it (boot self-test: straight to scanout).
            self.draw_resolve(&mut scene, bb_cpu, width, height, pitch, true, verbose)?;
            if f % 60 == 0 {
                crate::serial_println!("[SCENE] frame {}/{} angle={}.{:02} rad",
                    f, frames, a as i32, ((a - a as i32 as f32) * 100.0) as i32);
            }
            crate::time::sleep_ms(12);
        }
        crate::serial_println!("[SCENE] spin complete: {} frames ({} meshes each)", frames, scene.meshes.len());
        Ok(())
    }

    /// Copy the rendered backbuffer straight onto the live scanout framebuffer (same
    /// BGRA/stride layout). Shared by the Phase-5 proof-of-life present and the cube.
    unsafe fn present_to_scanout(&self, bb_cpu: u64, bytes: usize) {
        let sp = core::ptr::addr_of_mut!(crate::gui::SCREEN_PAINTER);
        if let Some(painter) = (*sp).as_mut() {
            let n = bytes.min(painter.buffer.len());
            crate::gui::turbo_copy(painter.buffer.as_mut_ptr(), bb_cpu as *const u8, n);
        }
    }

    /// Present a Y-TILED render target: de-swizzle each pixel from the Gen9 Tile-Y layout in
    /// `rt_cpu` into the LINEAR scanout framebuffer. `scanout_pitch`/`tiled_pitch` are byte row
    /// pitches. See [[gen9-tiley-layout]] — a tile is 128B x 32 rows; within a tile the byte at
    /// (u = X%128, v = Y%32) sits at ((u>>4)<<9) | (v<<4) | (u&0xF); the tile itself is at
    /// (Y/32)*(32*pitch) + (X/128)*4096. Bit-6 channel swizzle is disabled on Gen8+, so there is
    /// no XOR term. A BGRA pixel (4 bytes) never straddles a 16B column, so we gather it as one
    /// u32. This is a per-pixel CPU gather (~2M/frame at 1080p) — fine for the demo; a tiled BLT
    /// or GPU resolve would replace it later.
    unsafe fn present_to_scanout_detiled(
        &self,
        rt_cpu: u64,
        width: u32,
        height: u32,
        scanout_pitch: u32,
        tiled_pitch: u32,
    ) {
        let sp = core::ptr::addr_of_mut!(crate::gui::SCREEN_PAINTER);
        let painter = match (*sp).as_mut() { Some(p) => p, None => return };
        let dst = painter.buffer.as_mut_ptr();
        let dst_len = painter.buffer.len();
        let tile_row_bytes = 32u32 * tiled_pitch; // bytes spanned by one row of tiles (32 rows tall)
        let mut py = 0u32;
        while py < height {
            let tile_y = py / 32;
            let v = py % 32;
            let row_tile_base = tile_y * tile_row_bytes;
            let dst_row = (py * scanout_pitch) as usize;
            let mut px = 0u32;
            while px < width {
                let x = px * 4; // byte column (BGRA)
                let u = x % 128;
                let src = (row_tile_base + (x / 128) * 4096
                    + ((u >> 4) << 9) + (v << 4) + (u & 0xF)) as usize;
                let doff = dst_row + (px * 4) as usize;
                if doff + 4 <= dst_len {
                    let texel = (rt_cpu as *const u32).byte_add(src).read_volatile();
                    (dst.add(doff) as *mut u32).write_volatile(texel);
                }
                px += 1;
            }
            py += 1;
        }
    }

    /// Emit a PIPE_CONTROL that writes `val` to the fence page (CS-stall, no cache flush).
    /// Used as a progress checkpoint; the last value written survives a hang.
    unsafe fn progress(&self, cs: &mut CmdStream, val: u32) {
        cmd::pipe_control(
            cs,
            cmd::pc::CS_STALL | cmd::pc::POST_SYNC_WRITE_IMM | cmd::pc::DEST_ADDRESS_GTT,
            GVA_RCS_FENCE,
            val as u64,
        );
    }

    /// STATE_BASE_ADDRESS: general/surface/dynamic/instruction bases + buffer sizes.
    /// Length 18 (bindless-size dword dropped per WaBindlessSurfaceStateModifyEnable).
    unsafe fn emit_state_base_address(&self, cs: &mut CmdStream) {
        let instr_base = super::GVA_INSTRUCTION_BASE;
        // header: subtype COMMON(0), opcode 1, subop 1, length 0x10 (18 dwords).
        cs.push(gfxpipe_header(0, 1, 1, 0x10));
        let m = 1u32; // modify-enable bit 0; MOCS 0
        cs.push((0 & 0xFFFFF000) | m).push(0); // General State
        cs.push(0); // Stateless DataPort MOCS
        cs.push((GVA_SURFACE_STATE & 0xFFFFF000) | m).push(0); // Surface State
        cs.push((GVA_DYNAMIC_STATE & 0xFFFFF000) | m).push(0); // Dynamic State
        cs.push(0).push(0); // Indirect Object (disabled)
        cs.push((instr_base & 0xFFFFF000) | m).push(0); // Instruction
        cs.push((0xFFFFF << 12) | 1); // General State buffer size
        cs.push((0xFFFFF << 12) | 1); // Dynamic State buffer size
        cs.push((0xFFFFF << 12) | 1); // Indirect Object buffer size
        cs.push((0xFFFFF << 12) | 1); // Instruction buffer size
        cs.push(0).push(0); // Bindless surface state base (modify disabled)
    }

    unsafe fn emit_vertex_buffers(&self, cs: &mut CmdStream, addr: u32, pitch: u32, size: u32) {
        cs.push(h(so::VERTEX_BUFFERS, 5)); // header + 1 VERTEX_BUFFER_STATE (4 dw)
        cs.push((pitch & 0xFFF) | (1 << 14)); // pitch, Address Modify Enable, index 0, MOCS 0
        cs.push(addr).push(0); // buffer starting address (64-bit)
        cs.push(size); // buffer size
    }

    unsafe fn emit_vertex_elements(&self, cs: &mut CmdStream) {
        cs.push(h(so::VERTEX_ELEMENTS, 5)); // header + 2 VERTEX_ELEMENT_STATE (2 dw each)
        // Element 0 = position: R32G32B32A32_FLOAT (0), Valid (bit25), VB 0, Source Offset 0.
        cs.push(1 << 25);
        // DW1: components 0..3 = STORE_SRC (1) at [30:28],[26:24],[22:20],[18:16].
        cs.push((1 << 28) | (1 << 24) | (1 << 20) | (1 << 16));
        // Element 1 = color: same format, Valid, VB 0, Source Element Offset = 16 bytes [11:0].
        cs.push((1 << 25) | 16);
        cs.push((1 << 28) | (1 << 24) | (1 << 20) | (1 << 16));
    }

    unsafe fn emit_vs(&self, cs: &mut CmdStream, kernel_off: u32) {
        cs.push(h(so::VS, 9));
        cs.push(kernel_off).push(0); // Kernel Start Pointer (64B granular at bit 6; off is 64B aligned)
        cs.push(0); // scratch
        cs.push(0).push(0); // scratch base
        // DW6: URB read offset 0, read length 2 (<<11), dispatch GRF start 2 (<<20).
        // Boot A: read length 2 so the VF delivers BOTH input attributes (position g2-g5 and
        // color g6-g9). 2 covers both under either read-length unit convention (per-attribute
        // or per-pair); any over-read lands in unused GRFs, so it is safe.
        cs.push((2 << 11) | (2 << 20));
        // DW7: Enable(bit0) | SIMD8 Dispatch(bit2) | StatisticsEnable(bit10) | Max threads 63 (<<23).
        cs.push(1 | (1 << 2) | (1 << 10) | (63 << 23));
        // DW8: Vertex URB Entry Output Length = 2 (Boot #10). 2-slot VUE = header(DW0-3) +
        // position(DW4-7) + color(DW8-11) = 48B -> rounds to 2 x 256-bit units (64B). Pairs with
        // build_vs writing the color slot. URB_VS alloc STAYS 0 (a 64B VUE fits a 64B entry) --
        // the earlier alloc=1 bump is what overflowed the URB and hung the drain (Boot #9).
        cs.push(2 << 16);
    }

    unsafe fn emit_ps(&self, cs: &mut CmdStream, kernel_off: u32) {
        cs.push(h(so::PS, 12));
        cs.push(kernel_off).push(0); // Kernel Start Pointer 0
        // DW3: Binding Table Entry Count [121:114]=DW3[25:18], Sampler Count [125:123]=DW3[29:27].
        // Boot C: 2 binding-table entries (RT + texture) and 1 sampler (1-4 Samplers=1) so the
        // PSD prefetches the sampler state. (Only the cube path runs at boot; the Phase-5
        // triangle path shares this and would over-prefetch harmlessly.)
        cs.push((2 << 18) | (1 << 27));
        cs.push(0).push(0); // scratch base
        // DW6: 8 Pixel Dispatch Enable(bit0) | Max threads per PSD 63 (<<23).
        cs.push(1 | (63 << 23));
        // DW7: Dispatch GRF Start For Constant/Setup Data 0. Boot B = 4: with BIM_PERSPECTIVE_PIXEL
        // enabled the payload is g0/g1 header + g2/g3 barycentric, so attribute setup starts at g4.
        // (Boot A flat path used 2 = setup right after header with no barycentric.)
        cs.push(4 << 16);
        cs.push(0).push(0); // Kernel Start Pointer 1
        cs.push(0).push(0); // Kernel Start Pointer 2
    }
}

// Persistent per-mesh GPU resources (`GpuMesh`) and the mesh/texture builders now live in
// `engine.rs`; the cube is just the first client of that generic API (engine::cube_mesh +
// Texture::checkerboard). The 8-corner cube_geometry / 24-vertex cube_geometry_colored
// helpers and the CubeGpu struct were removed in the Phase 8 refactor.
