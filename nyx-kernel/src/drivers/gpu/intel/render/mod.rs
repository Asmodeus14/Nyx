// src/drivers/gpu/intel/render/mod.rs
//
// Intel Gen9.5 (Comet/Kaby Lake) RENDER engine (RCS) + 3D pipeline.
//
// This module is the counterpart to the BLT driver in `intel/mod.rs`. Where that
// module drives the blitter command streamer (base 0x22000) for 2D compositing,
// this one brings up the RENDER command streamer (RCS, base 0x2000) in legacy
// ring-buffer mode and, on top of it, the Gen9 3D pipeline.
//
// Submission model: PRM Vol 6 "Render Engine Command Streamer (RCS)" confirms the
// RCS claims MMIO 0x2000..0x27FF and can be driven in the same legacy ring-buffer
// mode as the BLT engine (RING_START/HEAD/TAIL/CTL) — no execlists/GuC required for
// bring-up. We deliberately leave the execlist-enable bit in GFX_MODE clear.
//
// Build-up order (see agile-churning-parrot.md):
//   Phase 1  ring bring-up + reset          -> this file + ring.rs
//   Phase 2  batch buffers + PIPE_CONTROL   -> cmd.rs
//   Phase 3  EU shader kernels              -> eu.rs
//   Phase 4  state objects                  -> state.rs
//   Phase 5  full 3D pipeline / draw        -> pipeline.rs + urb.rs

#![allow(dead_code)] // scaffolding: many constants/fields land in later phases

pub mod cmd;
pub mod ring;
pub mod eu;
pub mod state;
pub mod urb;
pub mod pipeline;
pub mod decode;

// ---------------------------------------------------------------------------
// RCS MMIO register map (offsets from BAR0 / mmio_base). PRM Vol 6 RINGBUF.
// The RCS ring registers follow the same base+0x30/0x34/0x38/0x3C layout as the
// blitter, but at engine base 0x2000 (BLT is at 0x22000).
// ---------------------------------------------------------------------------
pub const RCS_BASE: u32 = 0x2000;

pub const RENDER_RING_TAIL: u32 = RCS_BASE + 0x30; // 0x2030
pub const RENDER_RING_HEAD: u32 = RCS_BASE + 0x34; // 0x2034
pub const RENDER_RING_START: u32 = RCS_BASE + 0x38; // 0x2038
pub const RENDER_RING_CTL: u32 = RCS_BASE + 0x3C; // 0x203C

// Per-engine mode/reset. GFX_MODE (RCS) controls execlist-enable; we keep it clear.
// RESET_CTL (a.k.a. RING_RESET_CTL) is used for the graceful per-engine reset path.
// NOTE: exact offsets to be confirmed on hardware during Phase 1 bring-up.
pub const RCS_GFX_MODE: u32 = RCS_BASE + 0x29C; // 0x229C (verify)
pub const RCS_RESET_CTL: u32 = RCS_BASE + 0xD0; // 0x20D0 (verify)

// Forcewake — RENDER domain (Gen9). The BLT driver's FORCEWAKE_BLT=0xA188 is the
// *blitter* domain; the render domain is a different register pair (i915:
// FORCEWAKE_RENDER_GEN9 / FORCEWAKE_ACK_RENDER_GEN9). Verify ack bit on hardware.
pub const FORCEWAKE_RENDER: u32 = 0xA278;
pub const FORCEWAKE_ACK_RENDER: u32 = 0x0D84;

// Full-GPU soft reset (GEN6_GDRST). Render reset domain = bit 1.
pub const GDRST: u32 = 0x941C;
pub const GRDOM_RENDER: u32 = 1 << 1;
pub const GRDOM_FULL: u32 = 1 << 0;

// Global fault register (already probed by the BLT self-test at 0x4094).
pub const RENDER_FAULT_REG: u32 = 0x4094;

// ---------------------------------------------------------------------------
// GVA (GGTT) layout owned by the 3D engine. Chosen to avoid the BLT driver's
// existing allocations (ring 0x1000_0000, backbuffer 0x1400_0000, fence
// 0x1500_0000, windows 0x2000_0000+, test 0x3000_0000).
// ---------------------------------------------------------------------------
pub const GVA_RCS_RING: u32 = 0x1600_0000;
pub const GVA_BATCH: u32 = 0x1700_0000;
pub const GVA_INSTRUCTION_BASE: u32 = 0x1800_0000; // shader kernels
pub const GVA_DYNAMIC_STATE: u32 = 0x1900_0000; // viewport/CC/blend/sampler
pub const GVA_SURFACE_STATE: u32 = 0x1A00_0000; // surface state + binding tables
pub const GVA_VERTEX_BUFFERS: u32 = 0x1B00_0000;
pub const GVA_DEPTH_BUFFER: u32 = 0x1C00_0000;
pub const GVA_TEXTURES: u32 = 0x1D00_0000;
pub const GVA_RCS_FENCE: u32 = 0x1E00_0000; // fence / scratch

/// The RCS ring is one 4 KiB page = 1024 dwords, matching the BLT ring.
pub const RCS_RING_DWORDS: u32 = 1024;

#[derive(Debug)]
pub enum RenderError {
    RingFull,
    EngineHang,
    ResetFailed,
    NotInitialized,
}

/// RENDER command streamer + 3D pipeline state. Populated across Phases 1-5.
/// Holds its own copy of `mmio_base` and reimplements the 3-line volatile MMIO /
/// GGTT helpers (rather than cross-locking `INTEL_GPU`) to avoid lock-ordering
/// hazards between the two engine drivers. Memory allocation goes through the
/// shared `crate::memory` free functions.
pub struct RenderEngine {
    /// BAR0 MMIO virtual base (same value `IntelGpuDriver.mmio_base` holds).
    pub mmio_base: u64,
    /// GGTT GVA of the RCS ring buffer (GVA_RCS_RING once mapped).
    pub ring_gva: u32,
    /// CPU-visible virtual address of the ring page.
    pub ring_virt: Option<u64>,
    /// CPU-visible virtual address of the batch-buffer page (mapped at GVA_BATCH).
    pub batch_virt: Option<u64>,
    /// Fence/scratch page: phys, CPU-virt, and running fence value.
    pub fence_phys: u64,
    pub fence_virt: *mut u32,
    pub next_fence: u32,
    pub initialized: bool,
    /// Instruction-base store holding the compiled shader kernels (Phase 3+).
    pub kernels: Option<eu::KernelStore>,
}

unsafe impl Send for RenderEngine {}
unsafe impl Sync for RenderEngine {}

impl RenderEngine {
    pub const fn new() -> Self {
        Self {
            mmio_base: 0,
            ring_gva: 0,
            ring_virt: None,
            batch_virt: None,
            fence_phys: 0,
            fence_virt: core::ptr::null_mut(),
            next_fence: 0,
            initialized: false,
            kernels: None,
        }
    }

    // --- MMIO / GGTT primitives (mirror the BLT driver's helpers) ---

    #[inline]
    pub unsafe fn read_reg(&self, offset: u32) -> u32 {
        core::ptr::read_volatile((self.mmio_base + offset as u64) as *const u32)
    }

    #[inline]
    pub unsafe fn write_reg(&self, offset: u32, value: u32) {
        core::ptr::write_volatile((self.mmio_base + offset as u64) as *mut u32, value);
    }

    /// Map one physical RAM page into the GGTT at `gpu_page` (Gen8+ 8-byte PTEs at
    /// BAR0 + 8 MB). Identical layout to `IntelGpuDriver::map_ggtt_page`.
    pub unsafe fn map_ggtt_page(&self, gpu_page: u32, phys: u64, coherent: bool) {
        let gtt_offset = 0x800000 + (gpu_page as u64 * 8);
        let gtt_ptr = (self.mmio_base + gtt_offset) as *mut u64;
        let flags = if coherent { 0x07 } else { 0x03 };
        let pte = (phys & 0xFFFF_FFFF_FFFF_F000) | flags;
        core::ptr::write_volatile(gtt_ptr, pte);
        let _ = core::ptr::read_volatile(gtt_ptr); // posting read
    }

    /// clflush + mfence a cache line so CPU/GPU see coherent data on the ring/fence.
    #[inline]
    pub unsafe fn flush_line(&self, addr: usize) {
        core::arch::asm!("clflush [{}]", in(reg) addr, options(nostack, preserves_flags));
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }

    /// Program the Gen9 MOCS (Memory Object Control State) table. On Gen9 the KERNEL
    /// must set up these control registers or render-cache accesses (the PS render-target
    /// writes) have undefined caching and may never reach memory — the classic "pipeline
    /// runs, shaders execute, but no pixels" symptom. We set every entry to LLC write-back
    /// cached (coherent with the CPU). Values from i915 skl_mocs_table.
    pub unsafe fn program_mocs(&self) {
        // GFX (render) MOCS control: __GEN9_RCS0_MOCS0 = 0xC800 + i*4, 64 entries.
        // control = LE_3_WB(3) | LE_TC_2_LLC_ELLC(2<<2) | LE_LRUM(3<<4) = 0x3B.
        for i in 0..64u32 {
            self.write_reg(0xC800 + i * 4, 0x0000_003B);
        }
        // L3 (LNCF) MOCS: GEN9_LNCFCMOCS = 0xB020 + i*4, 32 regs, two 16-bit entries each.
        // l3cc = L3_3_WB = 3<<4 = 0x30, packed low+high.
        for i in 0..32u32 {
            self.write_reg(0xB020 + i * 4, 0x0030_0030);
        }
        crate::serial_println!("[RCS] MOCS table programmed (all entries -> LLC WB cached).");
    }

    // --- Power / reset ---

    /// Wake the RENDER power domain. Distinct register pair from the blitter
    /// (0xA188) — the BLT forcewake does NOT wake the render domain, which is why a
    /// prior attempt saw RCS MMIO writes silently dropped.
    pub unsafe fn forcewake_render(&self) {
        // Masked write: enable bit 0 (mask bit 16 set).
        self.write_reg(FORCEWAKE_RENDER, 0x0001_0001);
        let mut t = 0u32;
        while (self.read_reg(FORCEWAKE_ACK_RENDER) & 1) == 0 {
            core::hint::spin_loop();
            t += 1;
            if t > 1_000_000 {
                crate::serial_println!("[RCS] WARN: render forcewake ack timeout (ack={:#x})",
                    self.read_reg(FORCEWAKE_ACK_RENDER));
                break;
            }
        }
    }

    /// Per-engine render reset (Gen8 sequence): request via RESET_CTL, wait
    /// ready-to-reset, pulse GDRST render domain, then release the request. Lets a
    /// hung experiment recover without rebooting the machine. Bits to verify on HW.
    pub unsafe fn reset_render(&self) -> Result<(), RenderError> {
        // 1. Request reset (masked reg: set REQUEST_RESET bit 0).
        self.write_reg(RCS_RESET_CTL, (1 << 16) | (1 << 0));
        // 2. Wait until the engine reports READY_FOR_RESET (bit 1).
        let mut t = 0u32;
        while (self.read_reg(RCS_RESET_CTL) & (1 << 1)) == 0 {
            core::hint::spin_loop();
            t += 1;
            if t > 2_000_000 {
                crate::serial_println!("[RCS] WARN: engine not ready-for-reset, forcing anyway");
                break;
            }
        }
        // 3. Pulse the render reset domain via GEN6_GDRST.
        self.write_reg(GDRST, GRDOM_RENDER);
        let mut t = 0u32;
        loop {
            if (self.read_reg(GDRST) & GRDOM_RENDER) == 0 {
                break;
            }
            core::hint::spin_loop();
            t += 1;
            if t > 5_000_000 {
                crate::serial_println!("[RCS] ERROR: GDRST render reset timeout");
                self.write_reg(RCS_RESET_CTL, 1 << 16); // release request
                return Err(RenderError::ResetFailed);
            }
        }
        // 4. Release the reset request (masked disable of bit 0).
        self.write_reg(RCS_RESET_CTL, 1 << 16);
        Ok(())
    }

    // --- Bring-up + self-test (Phase 1) ---

    /// Full RCS bring-up in legacy ring-buffer mode. Called once during boot.
    pub unsafe fn bring_up(&mut self, mmio_base: u64) -> Result<(), RenderError> {
        self.mmio_base = mmio_base;
        crate::serial_println!("[RCS] Bringing up RENDER command streamer (base {:#06x})...", RCS_BASE);

        self.forcewake_render();

        // Start from a known-idle engine (recover from any prior hang).
        if self.reset_render().is_err() {
            crate::serial_println!("[RCS] WARN: render reset failed; continuing");
        }
        self.forcewake_render(); // re-assert after reset

        // Program the MOCS table (REQUIRED on Gen9 for render-cache writes to reach memory).
        self.program_mocs();

        // Ensure LEGACY ring-buffer mode: clear the execlist-enable bit in GFX_MODE
        // (masked reg: mask bit 15 set in [31:16], data bit 15 = 0). Firmware may have
        // left the engine in execlist mode, which would ignore our ring registers.
        self.write_reg(RCS_GFX_MODE, 1 << (15 + 16));

        // Allocate + GGTT-map the RCS ring (one 4 KiB page = 1024 dwords).
        let ring_frame = crate::memory::allocate_frame().ok_or(RenderError::NotInitialized)?;
        let ring_phys = ring_frame.start_address().as_u64();
        let ring_virt = crate::memory::phys_to_virt(ring_phys).ok_or(RenderError::NotInitialized)?;
        self.ring_virt = Some(ring_virt);
        self.ring_gva = GVA_RCS_RING;
        self.map_ggtt_page(GVA_RCS_RING / 4096, ring_phys, true);
        core::ptr::write_bytes(ring_virt as *mut u8, 0, 4096);

        // Program the ring in legacy mode: disable, set base/head/tail, enable.
        // RING_CTL = 0x1 => enable + length field 0 (one 4 KiB page).
        self.write_reg(RENDER_RING_CTL, 0);
        self.write_reg(RENDER_RING_HEAD, 0);
        self.write_reg(RENDER_RING_TAIL, 0);
        self.write_reg(RENDER_RING_START, GVA_RCS_RING);
        self.write_reg(RENDER_RING_CTL, 0x1);

        let ctl = self.read_reg(RENDER_RING_CTL);
        let start = self.read_reg(RENDER_RING_START);
        crate::serial_println!("[RCS] Ring CTL readback={:#010x} START readback={:#010x}", ctl, start);

        // Allocate + GGTT-map the fence / scratch page.
        let fence_frame =
            crate::memory::allocate_contiguous(1, 4096, true).ok_or(RenderError::NotInitialized)?;
        self.fence_phys = fence_frame.start_address().as_u64();
        self.fence_virt =
            crate::memory::phys_to_virt(self.fence_phys).ok_or(RenderError::NotInitialized)? as *mut u32;
        self.fence_virt.write_volatile(0);
        self.map_ggtt_page(GVA_RCS_FENCE / 4096, self.fence_phys, true);

        // Allocate + GGTT-map a batch-buffer page (Phase 2: MI_BATCH_BUFFER_START target).
        let batch_frame =
            crate::memory::allocate_frame().ok_or(RenderError::NotInitialized)?;
        let batch_phys = batch_frame.start_address().as_u64();
        let batch_virt = crate::memory::phys_to_virt(batch_phys).ok_or(RenderError::NotInitialized)?;
        core::ptr::write_bytes(batch_virt as *mut u8, 0, 4096);
        self.batch_virt = Some(batch_virt);
        self.map_ggtt_page(GVA_BATCH / 4096, batch_phys, true);

        self.next_fence = 0;
        self.initialized = true;
        crate::serial_println!(
            "[RCS] Render engine ready. Ring GVA {:#x}, Fence GVA {:#x}",
            self.ring_gva, GVA_RCS_FENCE
        );
        Ok(())
    }

    /// Phase 1 milestone: prove the RCS ring executes commands by having the engine
    /// write a magic value to the fence page via MI_STORE_DATA_IMM, then reading it
    /// back on the CPU. Mirrors the BLT `test_blitter` proof.
    pub unsafe fn rcs_selftest(&mut self) -> Result<(), RenderError> {
        if !self.initialized {
            return Err(RenderError::NotInitialized);
        }
        crate::serial_println!("[RCS] Self-test: MI_STORE_DATA_IMM via render ring...");

        self.fence_virt.write_volatile(0);
        self.flush_line(self.fence_virt as usize);

        const MAGIC: u32 = 0x900D_1234;
        // MI_STORE_DATA_IMM (Use GTT), 32-bit address form: header, gva_lo, gva_hi, data.
        // Length field 0x02 => 4 dwords total (matches the BLT fence encoding).
        let cmd = [
            cmd::mi_store_data_imm(true, 0x02),
            GVA_RCS_FENCE,
            0x0,
            MAGIC,
        ];
        self.rcs_submit(&cmd)?;

        let mut t = 0u32;
        loop {
            self.flush_line(self.fence_virt as usize);
            let v = self.fence_virt.read_volatile();
            if v == MAGIC {
                break;
            }
            core::hint::spin_loop();
            t += 1;
            if t > 20_000_000 {
                let head = self.read_reg(RENDER_RING_HEAD);
                let tail = self.read_reg(RENDER_RING_TAIL);
                let fault = self.read_reg(RENDER_FAULT_REG);
                crate::serial_println!(
                    "[RCS] SELFTEST FAIL: fence={:#010x} HEAD={:#x} TAIL={:#x} FAULT={:#010x}",
                    v, head, tail, fault
                );
                return Err(RenderError::EngineHang);
            }
        }
        crate::serial_println!("[RCS] SELFTEST PASS: render ring executed a command.");
        Ok(())
    }

    // --- Batch buffers (Phase 2) ---

    /// Copy a command list into the batch page, terminate it with
    /// MI_BATCH_BUFFER_END, flush it to RAM, then jump to it from the ring via
    /// MI_BATCH_BUFFER_START (GGTT address space). The batch must fit in one page.
    pub unsafe fn exec_batch(&mut self, batch: &[u32]) -> Result<(), RenderError> {
        let batch_virt = self.batch_virt.ok_or(RenderError::NotInitialized)?;
        if (batch.len() + 1) * 4 > 4096 {
            return Err(RenderError::RingFull); // batch too large for one page
        }
        let dst = batch_virt as *mut u32;
        for (i, &dw) in batch.iter().enumerate() {
            dst.add(i).write_volatile(dw);
        }
        dst.add(batch.len()).write_volatile(cmd::MI_BATCH_BUFFER_END);

        // Flush every touched cache line so the GPU's command fetch sees the batch.
        let bytes = (batch.len() + 1) * 4;
        let mut off = 0usize;
        while off < bytes {
            self.flush_line(batch_virt as usize + off);
            off += 64;
        }

        // MI_BATCH_BUFFER_START (Gen8+, 3 dwords, GGTT): header, addr_lo, addr_hi.
        let start = [
            cmd::mi_batch_buffer_start(false, 1),
            GVA_BATCH,
            0x0,
        ];
        self.rcs_submit(&start)
    }

    /// The Phase 5 workhorse: run a command list as a batch buffer, append a
    /// PIPE_CONTROL end-of-pipe post-sync fence (the mechanism proven in Phase 2b —
    /// note MI_STORE_DATA_IMM from a batch is unreliable on this HW, PIPE_CONTROL is
    /// not), and block until the GPU signals completion. `flush_rt` adds a render-target
    /// cache flush so drawn pixels are visible in memory before we present.
    pub unsafe fn run_batch(&mut self, cmds: &[u32], flush_rt: bool) -> Result<(), RenderError> {
        const DONE: u32 = 0x5EED_D01E;
        self.fence_virt.write_volatile(0);
        self.flush_line(self.fence_virt as usize);

        let mut cs = cmd::CmdStream::new();
        for &d in cmds {
            cs.push(d);
        }
        let mut flags = cmd::pc::CS_STALL | cmd::pc::POST_SYNC_WRITE_IMM | cmd::pc::DEST_ADDRESS_GTT;
        if flush_rt {
            flags |= cmd::pc::RENDER_TARGET_CACHE_FLUSH | cmd::pc::DC_FLUSH_ENABLE;
        }
        cmd::pipe_control(&mut cs, flags, GVA_RCS_FENCE, DONE as u64);

        self.exec_batch(cs.as_slice())?;
        self.wait_fence_value(DONE)
    }

    /// Spin until the fence page reads `expected` (or a hang timeout trips).
    unsafe fn wait_fence_value(&self, expected: u32) -> Result<(), RenderError> {
        let mut t = 0u32;
        loop {
            self.flush_line(self.fence_virt as usize);
            if self.fence_virt.read_volatile() == expected {
                return Ok(());
            }
            core::hint::spin_loop();
            t += 1;
            if t > 20_000_000 {
                let head = self.read_reg(RENDER_RING_HEAD);
                let tail = self.read_reg(RENDER_RING_TAIL);
                let fault = self.read_reg(RENDER_FAULT_REG);
                crate::serial_println!(
                    "[RCS] fence wait timeout: got {:#010x} want {:#010x} HEAD={:#x} TAIL={:#x} FAULT={:#010x}",
                    self.fence_virt.read_volatile(), expected, head, tail, fault
                );
                return Err(RenderError::EngineHang);
            }
        }
    }

    /// Phase 2a: isolate MI_BATCH_BUFFER_START by running the *proven* MI_STORE_DATA_IMM
    /// inside a batch buffer (no PIPE_CONTROL). If Phase 1 passes but this fails, the
    /// batch-jump mechanism is at fault; if this passes but the PIPE_CONTROL test fails,
    /// PIPE_CONTROL is at fault.
    pub unsafe fn batch_mistore_test(&mut self) -> Result<(), RenderError> {
        if !self.initialized {
            return Err(RenderError::NotInitialized);
        }
        crate::serial_println!("[RCS] Self-test 2a: MI_STORE_DATA_IMM inside a batch buffer...");

        const MAGIC: u32 = 0xBA7C_0001;
        self.fence_virt.write_volatile(0);
        self.flush_line(self.fence_virt as usize);

        let batch = [
            cmd::mi_store_data_imm(true, 0x02),
            GVA_RCS_FENCE,
            0x0,
            MAGIC,
        ];
        self.exec_batch(&batch)?;
        self.wait_fence_value(MAGIC)?;

        crate::serial_println!("[RCS] SELFTEST PASS: batch buffer (MI_BATCH_BUFFER_START) works.");
        Ok(())
    }

    /// Phase 2 gate: PIPE_CONTROL post-sync write submitted DIRECTLY to the ring (not a
    /// batch buffer). Batch buffers are avoided entirely — MI_BATCH_BUFFER_END does not
    /// return control to the ring on this HW, which leaves the engine stuck in the batch
    /// page and starves all subsequent ring work. The ring path is proven solid.
    pub unsafe fn pipecontrol_ring_test(&mut self) -> Result<(), RenderError> {
        if !self.initialized {
            return Err(RenderError::NotInitialized);
        }
        crate::serial_println!("[RCS] Phase 2: PIPE_CONTROL post-sync via RING...");

        const MAGIC: u32 = 0xC0FF_EE01;
        self.fence_virt.write_volatile(0);
        self.flush_line(self.fence_virt as usize);

        let mut cs = cmd::CmdStream::new();
        cmd::pipe_control(
            &mut cs,
            cmd::pc::CS_STALL | cmd::pc::POST_SYNC_WRITE_IMM | cmd::pc::DEST_ADDRESS_GTT,
            GVA_RCS_FENCE,
            MAGIC as u64,
        );

        self.rcs_submit(cs.as_slice())?;
        self.wait_fence_value(MAGIC)?;

        crate::serial_println!("[RCS] SELFTEST PASS: ring PIPE_CONTROL post-sync observed.");
        Ok(())
    }
}

/// Global RENDER engine instance (mirrors `INTEL_GPU`).
pub static RENDER_ENGINE: spin::Mutex<RenderEngine> = spin::Mutex::new(RenderEngine::new());

/// Boot entry point: bring up the render engine and run the Phase 1 self-test.
/// Called from the boot flow with the GPU's MMIO base. Returns true on self-test pass.
pub fn init_render_engine(mmio_base: u64) -> bool {
    let mut eng = RENDER_ENGINE.lock();
    unsafe {
        if let Err(e) = eng.bring_up(mmio_base) {
            crate::serial_println!("[RCS] bring-up failed: {:?}", e);
            return false;
        }
        if let Err(e) = eng.rcs_selftest() {
            crate::serial_println!("[RCS] ring self-test failed: {:?}", e);
            return false;
        }
        // Phase 2: PIPE_CONTROL post-sync fence via the RING. We deliberately do NOT run
        // any batch-buffer test here — batches leave the engine stuck (BB_END doesn't
        // return on this HW) and would starve the subsequent pipeline submission.
        let pc_ok = eng.pipecontrol_ring_test().is_ok();

        // Phase 3: encode the VS/PS kernels, place them in the instruction base, and
        // hex-dump for byte-diff against Mesa. Non-fatal (encoding validation step).
        let mmio = eng.mmio_base;
        match eu::build_and_dump_kernels(mmio) {
            Ok(store) => eng.kernels = Some(store),
            Err(e) => crate::serial_println!("[EU] kernel build failed: {:?}", e),
        }

        // Phase 5: draw a triangle into the backbuffer (GVA 0x1400_0000) and verify via
        // CPU pixel readback. Only if the fence path (Phase 2b) and kernels are ready.
        if pc_ok && eng.kernels.is_some() {
            let bb_phys = {
                let g = crate::drivers::gpu::intel::INTEL_GPU.lock();
                g.as_ref().map(|d| d.backbuffer_phys).unwrap_or(0)
            };
            let bb_cpu = crate::memory::phys_to_virt(bb_phys).unwrap_or(0);
            let (w, h, stride, bpp) = if let Some(p) = &crate::gui::SCREEN_PAINTER {
                (
                    p.info.width as u32,
                    p.info.height as u32,
                    p.info.stride as u32,
                    p.info.bytes_per_pixel as u32,
                )
            } else {
                (1920, 1080, 1920, 4)
            };
            if bb_cpu != 0 {
                match eng.draw_triangle(0x1400_0000, bb_cpu, w, h, stride * bpp) {
                    Ok(()) => {
                        // PROOF-OF-LIFE PRESENT: the GPU rendered the triangle into the
                        // backbuffer (GVA 0x1400_0000) but nothing presents it, and the
                        // window server is about to clear+repaint that same buffer. Copy
                        // the rendered backbuffer straight onto the live scanout
                        // framebuffer (same BGRA/stride layout) and hold a few seconds so
                        // the red triangle is actually visible before boot continues.
                        let bb_len = (stride * bpp * h) as usize;
                        let sp = core::ptr::addr_of_mut!(crate::gui::SCREEN_PAINTER);
                        if let Some(painter) = (*sp).as_mut() {
                            let n = bb_len.min(painter.buffer.len());
                            crate::gui::turbo_copy(
                                painter.buffer.as_mut_ptr(),
                                bb_cpu as *const u8,
                                n,
                            );
                            crate::serial_println!(
                                "[TRI] presented GPU backbuffer to scanout ({} bytes); holding 5s so it's visible.",
                                n
                            );
                        }
                        crate::time::sleep_ms(5000);
                    }
                    Err(e) => crate::serial_println!("[TRI] draw_triangle failed: {:?}", e),
                }
            }
        }

        pc_ok // the meaningful gate: ring PIPE_CONTROL fence works
    }
}
