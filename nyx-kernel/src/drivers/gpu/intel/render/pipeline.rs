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
        cs.push(h(so::VERTEX_ELEMENTS, 3)); // header + 1 VERTEX_ELEMENT_STATE (2 dw)
        // DW0: format R32G32B32A32_FLOAT (0), Valid (bit25), VB index 0, offset 0.
        cs.push(1 << 25);
        // DW1: components 0..3 = STORE_SRC (1) at [30:28],[26:24],[22:20],[18:16].
        cs.push((1 << 28) | (1 << 24) | (1 << 20) | (1 << 16));
    }

    unsafe fn emit_vs(&self, cs: &mut CmdStream, kernel_off: u32) {
        cs.push(h(so::VS, 9));
        cs.push(kernel_off).push(0); // Kernel Start Pointer (64B granular at bit 6; off is 64B aligned)
        cs.push(0); // scratch
        cs.push(0).push(0); // scratch base
        // DW6: URB read offset 0, read length 1 (<<11), dispatch GRF start 2 (<<20).
        cs.push((1 << 11) | (2 << 20));
        // DW7: Enable(bit0) | SIMD8 Dispatch(bit2) | StatisticsEnable(bit10) | Max threads 63 (<<23).
        cs.push(1 | (1 << 2) | (1 << 10) | (63 << 23));
        // DW8: Vertex URB Entry Output Length = 1 (<<16). My VUE is exactly the 256-bit
        // header (position in DW4-7), so 1 unit of output. Output length 2 exceeded the
        // URB_VS allocation (size 1) -> VUE write overflowed -> no primitives to clip.
        cs.push(1 << 16);
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
