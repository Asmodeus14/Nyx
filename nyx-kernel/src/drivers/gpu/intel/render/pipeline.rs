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
        let rs = state::setup_render_state(&mut surf_buf, &mut dyn_buf, rt_gva, width, height, pitch)?;

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

    /// Set up the persistent GPU resources for the cube ONCE (surface/dynamic/vertex state
    /// buffers, render state, a reserved vertex block, and the static index buffer). These
    /// are reused across frames by [`draw_cube_frame`], so an animation loop does not leak a
    /// GGTT mapping / contiguous frame per frame. The vertex block is left uninitialised —
    /// each frame rewrites it with fresh clip-space positions.
    unsafe fn setup_cube_gpu(
        &mut self,
        rt_gva: u32,
        width: u32,
        height: u32,
        pitch: u32,
    ) -> Result<CubeGpu, RenderError> {
        let (vs_off, ps_off) = match &self.kernels {
            Some(k) => (k.vs_off, k.ps_off),
            None => return Err(RenderError::NotInitialized),
        };
        let mmio = self.mmio_base;
        let mut surf_buf = StateBuffer::new(mmio, GVA_SURFACE_STATE, 1)?;
        let mut dyn_buf = StateBuffer::new(mmio, GVA_DYNAMIC_STATE, 1)?;
        let mut vbo_buf = StateBuffer::new(mmio, GVA_VERTEX_BUFFERS, 1)?;
        let rs = state::setup_render_state(&mut surf_buf, &mut dyn_buf, rt_gva, width, height, pitch)?;

        let (verts, colors, indices) = cube_geometry_colored();
        // Reserve the vertex block (24 verts * (pos vec4 + color vec4) = 192 dwords) at a
        // stable GVA; positions are rewritten every frame, colors are static. Indices are
        // static, so write them once after the block.
        let (vbo_cpu, vbo_gva) = vbo_buf.alloc(192, 64)?;
        let idx_gva = vbo_buf.write(&indices, 64)?;

        Ok(CubeGpu {
            surf_buf,
            dyn_buf,
            vbo_buf,
            rs,
            verts,
            colors,
            vbo_cpu,
            vbo_gva,
            vbo_dwords: 192,
            idx_gva,
            idx_count: indices.len(),
            vs_off,
            ps_off,
        })
    }

    /// Render ONE frame of the cube at rotation `angle` into `rt_gva`, using the pre-built
    /// `g`. Rewrites only the vertex block, then re-emits the full known-good Phase-5
    /// pipeline (index buffer + indexed 3DPRIMITIVE, back-face culled). `present` copies the
    /// result to scanout; `verbose` logs the decoded stream + pipeline stats + pixel
    /// readback (first frame only during animation, to avoid flooding serial).
    unsafe fn draw_cube_frame(
        &mut self,
        g: &mut CubeGpu,
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        angle: f32,
        present: bool,
        verbose: bool,
    ) -> Result<(), RenderError> {
        use super::math::{self, Mat4, Vec3, Vec4};

        let vs_off = g.vs_off;
        let ps_off = g.ps_off;
        let vbo_gva = g.vbo_gva;
        let idx_gva = g.idx_gva;
        let idx_count = g.idx_count;
        let vbo_dwords = g.vbo_dwords;

        // Clear the RT to DARK GRAY (0x30 in every byte -> 0x30303030 BGRA, ~RGB(48,48,48))
        // instead of black. DIAGNOSTIC (Boot A bring-up): the panel was "totally black, no
        // shape ever," which is ambiguous — a cube drawn with RGB=0 is invisible on a black
        // background, and so is a cube that never drew at all. A non-black clear disambiguates:
        //   * dark/black cube silhouette on gray -> geometry+present OK, bug is only the PS
        //     color read (attribute value from setup GRFs g2/g3);
        //   * colored cube on gray -> the whole attribute path works (we were fooled by
        //     black-on-black);
        //   * uniform gray, no shape -> geometry/present is broken (the 497-sample readback
        //     was stale/misleading), a different fix entirely.
        const CLEAR_BYTE: u8 = 0x30;
        let bytes = (pitch * height) as usize;
        core::ptr::write_bytes(bb_cpu as *mut u8, CLEAR_BYTE, bytes);
        let mut off = 0usize;
        while off < bytes {
            self.flush_line(bb_cpu as usize + off);
            off += 64;
        }

        // --- CPU transform: MVP * cube corners -> clip space, into the persistent VBO ---
        let aspect = width as f32 / height as f32;
        let proj = Mat4::perspective(math::radians(60.0), aspect, 0.1, 100.0);
        let view = Mat4::look_at(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let model = Mat4::rotate_y(angle).mul(&Mat4::rotate_x(angle * 0.6));
        let mvp = proj.mul(&view).mul(&model);

        // Interleaved layout per vertex: [pos.x pos.y pos.z pos.w  col.r col.g col.b col.a]
        // = 8 dwords (pitch 32B). Positions are MVP-transformed to clip space each frame;
        // colors are static (element 1, read by the VS as the color varying).
        for i in 0..g.verts.len() {
            let clip = mvp.mul_vec4(Vec4::from_point(g.verts[i]));
            let p = clip.to_bits();
            let c = g.colors[i];
            let base = i * 8;
            g.vbo_cpu.add(base).write_volatile(p[0]);
            g.vbo_cpu.add(base + 1).write_volatile(p[1]);
            g.vbo_cpu.add(base + 2).write_volatile(p[2]);
            g.vbo_cpu.add(base + 3).write_volatile(p[3]);
            g.vbo_cpu.add(base + 4).write_volatile(c[0].to_bits());
            g.vbo_cpu.add(base + 5).write_volatile(c[1].to_bits());
            g.vbo_cpu.add(base + 6).write_volatile(c[2].to_bits());
            g.vbo_cpu.add(base + 7).write_volatile(c[3].to_bits());
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
        cs.push(1);            // DW3 = Constant Interpolation Enable (attribute 0 -> flat)
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
        cs.push(h(so::WM, 2)).push(1 << 31); // BISECTION #7: barycentric OFF (no attrs)
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
        cs.push(h(so::SAMPLER_STATE_POINTERS_PS, 2)).push(0);
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
            crate::serial_println!("[CUBE] submitting {} dwords (angle={}.{:02})",
                cs.len(), angle as i32, ((angle - angle as i32 as f32) * 100.0) as i32);
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
            // Pixel readback: with the gray (0x30303030) diagnostic clear, "cube pixels" are
            // the ones that DIFFER from the background. Count those, and also (a) count how many
            // of them are pure-black-RGB (0x??000000 -> the "drawn but RGB=0" signature) and
            // (b) capture up to 4 distinct non-background pixel values so the boot log itself
            // shows whether the PS wrote real colors (e.g. 0xFFFF0000 red) or black.
            const CLEAR_U32: u32 = 0x3030_3030;
            let px = (pitch / 4) as usize;
            let mut o = 0usize;
            while o < bytes { self.flush_line(bb_cpu as usize + o); o += 64; }
            let mut non_bg = 0u32;
            let mut rgb_black = 0u32;
            let mut samples = [0u32; 4];
            let mut nsamp = 0usize;
            let mut yy = 0u32;
            while yy < height {
                let mut xx = 0u32;
                while xx < width {
                    let v = (bb_cpu as *const u32).add(yy as usize * px + xx as usize).read_volatile();
                    if v != CLEAR_U32 {
                        non_bg += 1;
                        if (v & 0x00FF_FFFF) == 0 { rgb_black += 1; } // RGB=0 (alpha-only)
                        if nsamp < 4 && !samples[..nsamp].contains(&v) {
                            samples[nsamp] = v; nsamp += 1;
                        }
                    }
                    xx += 13;
                }
                yy += 13;
            }
            crate::serial_println!(
                "[CUBE] readback: {} non-bg samples ({} of them RGB=0/black); distinct vals: {:#010x} {:#010x} {:#010x} {:#010x}",
                non_bg, rgb_black, samples[0], samples[1], samples[2], samples[3]);
        }

        if present {
            self.present_to_scanout(bb_cpu, bytes);
        }
        if hung { return Err(RenderError::EngineHang); }
        Ok(())
    }

    /// One-shot cube draw (Steps 1-2 entry point): allocate resources and render a single
    /// static frame at `angle`. Kept as a thin wrapper over the reusable frame path.
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
        let mut g = self.setup_cube_gpu(rt_gva, width, height, pitch)?;
        self.draw_cube_frame(&mut g, rt_gva, bb_cpu, width, height, pitch, angle, present, verbose)
    }

    /// Phase 6, Step 3: spin the cube. Allocate GPU resources ONCE, then render `frames`
    /// frames at increasing rotation and present each to the scanout. Only the first frame
    /// logs stats / decodes the stream, so serial isn't flooded. Reusing the buffers every
    /// frame is what makes an animation loop viable (no per-frame GGTT-mapping leak).
    pub unsafe fn spin_cube(
        &mut self,
        rt_gva: u32,
        bb_cpu: u64,
        width: u32,
        height: u32,
        pitch: u32,
        frames: u32,
    ) -> Result<(), RenderError> {
        let mut g = self.setup_cube_gpu(rt_gva, width, height, pitch)?;
        for f in 0..frames {
            let angle = f as f32 * 0.03; // ~0.03 rad/frame -> a smooth, steady tumble
            let verbose = f == 0; // stats/decode on the first frame only
            self.draw_cube_frame(&mut g, rt_gva, bb_cpu, width, height, pitch, angle, true, verbose)?;
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

    /// Copy the rendered backbuffer straight onto the live scanout framebuffer (same
    /// BGRA/stride layout). Shared by the Phase-5 proof-of-life present and the cube.
    unsafe fn present_to_scanout(&self, bb_cpu: u64, bytes: usize) {
        let sp = core::ptr::addr_of_mut!(crate::gui::SCREEN_PAINTER);
        if let Some(painter) = (*sp).as_mut() {
            let n = bytes.min(painter.buffer.len());
            crate::gui::turbo_copy(painter.buffer.as_mut_ptr(), bb_cpu as *const u8, n);
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
        cs.push(1 << 18); // DW3: Binding Table Entry Count = 1 [121:114] (blorp sets this)
        cs.push(0).push(0); // scratch base
        // DW6: 8 Pixel Dispatch Enable(bit0) | Max threads per PSD 63 (<<23).
        cs.push(1 | (63 << 23));
        // DW7: Dispatch GRF Start For Constant/Setup Data 0 = 2 (<<16).
        cs.push(2 << 16);
        cs.push(0).push(0); // Kernel Start Pointer 1
        cs.push(0).push(0); // Kernel Start Pointer 2
    }
}

/// Persistent GPU resources for an animated mesh: the state buffers + render state are
/// allocated ONCE and reused every frame (avoids leaking a GGTT mapping + contiguous frame
/// per frame). Only the vertex block is rewritten per frame; the index buffer and every
/// state object (surface/binding/blend/viewport) are static across the animation.
struct CubeGpu {
    surf_buf: StateBuffer,
    dyn_buf: StateBuffer,
    vbo_buf: StateBuffer,
    rs: state::RenderState,
    verts: [super::math::Vec3; 24],  // 24 face-vertices (model space), transformed per frame
    colors: [[f32; 4]; 24],          // per-vertex color (static), interleaved into the VBO
    vbo_cpu: *mut u32, // CPU ptr to the reserved vertex block (points into vbo_buf)
    vbo_gva: u32,      // GVA of the vertex block (stable across frames)
    vbo_dwords: usize, // size of the vertex block in dwords (192 = 24 verts * (pos4 + color4))
    idx_gva: u32,
    idx_count: usize,
    vs_off: u32,
    ps_off: u32,       // single per-vertex-color PS kernel (Phase 7 Boot A)
}

/// Unit cube centered at the origin (side 1.0): 8 corners + 36 indices (12 triangles).
/// Winding is CCW-front-facing for later back-face culling (Step 2); with cull NONE it
/// is irrelevant. Index order groups the 6 faces (front/back/left/right/top/bottom).
fn cube_geometry() -> ([super::math::Vec3; 8], [u32; 36]) {
    use super::math::Vec3;
    let v = [
        Vec3::new(-0.5, -0.5, -0.5), // 0
        Vec3::new(0.5, -0.5, -0.5),  // 1
        Vec3::new(0.5, 0.5, -0.5),   // 2
        Vec3::new(-0.5, 0.5, -0.5),  // 3
        Vec3::new(-0.5, -0.5, 0.5),  // 4
        Vec3::new(0.5, -0.5, 0.5),   // 5
        Vec3::new(0.5, 0.5, 0.5),    // 6
        Vec3::new(-0.5, 0.5, 0.5),   // 7
    ];
    let idx = [
        4, 5, 6, 4, 6, 7, // front  (+z)
        1, 0, 3, 1, 3, 2, // back   (-z)
        0, 4, 7, 0, 7, 3, // left   (-x)
        5, 1, 2, 5, 2, 6, // right  (+x)
        3, 7, 6, 3, 6, 2, // top    (+y)
        0, 1, 5, 0, 5, 4, // bottom (-y)
    ];
    (v, idx)
}

/// Phase 7 Boot A: a per-vertex COLORED cube. Unlike `cube_geometry` (8 shared corners),
/// this lays out 24 vertices = 6 faces x 4 unique corners, so each face's corners can carry
/// that face's color as a per-vertex attribute. Winding matches `cube_geometry`'s (the same
/// corner order per face), so back-face culling still removes the 3 hidden faces. Returns
/// (positions[24], colors[24], indices[36] as 6 quads: base+{0,1,2, 0,2,3}).
fn cube_geometry_colored() -> ([super::math::Vec3; 24], [[f32; 4]; 24], [u32; 36]) {
    use super::math::Vec3;
    // The 8 canonical corners (same coordinates as cube_geometry).
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
    // Each face: its 4 corners in the winding order cube_geometry() referenced, + a color.
    let faces: [([usize; 4], [f32; 4]); 6] = [
        ([4, 5, 6, 7], [1.0, 0.2, 0.2, 1.0]), // front (+z) red
        ([1, 0, 3, 2], [0.2, 1.0, 0.2, 1.0]), // back  (-z) green
        ([0, 4, 7, 3], [0.3, 0.5, 1.0, 1.0]), // left  (-x) blue
        ([5, 1, 2, 6], [1.0, 0.9, 0.2, 1.0]), // right (+x) yellow
        ([3, 7, 6, 2], [0.3, 1.0, 1.0, 1.0]), // top   (+y) cyan
        ([0, 1, 5, 4], [1.0, 0.4, 1.0, 1.0]), // bottom(-y) magenta
    ];
    let mut pos = [Vec3::new(0.0, 0.0, 0.0); 24];
    let mut col = [[0.0f32; 4]; 24];
    let mut idx = [0u32; 36];
    for (f, (corner_ids, color)) in faces.iter().enumerate() {
        let base = (f * 4) as u32;
        for (j, &cid) in corner_ids.iter().enumerate() {
            pos[f * 4 + j] = c[cid];
            col[f * 4 + j] = *color;
        }
        // Quad (base+0,1,2,3) -> two triangles preserving the original winding.
        let t = [base, base + 1, base + 2, base, base + 2, base + 3];
        idx[f * 6..f * 6 + 6].copy_from_slice(&t);
    }
    (pos, col, idx)
}
