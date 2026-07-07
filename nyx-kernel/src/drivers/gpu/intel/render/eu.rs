// src/drivers/gpu/intel/render/eu.rs
//
// GEN9 Execution Unit (EU) shader kernels (Phase 3).
//
// Gen9 has NO fixed-function color-fill path: rasterizing a triangle requires two real
// EU machine-code kernels:
//   - VS (pass-through / MVP): reads vertex attributes from its URB-sourced GRF payload
//     and issues a URB Write message, terminating with EOT.
//   - PS (constant / vertex-color): loads color into a GRF and issues a Render Target
//     Write message ("Replicate Data" form for constant color), terminating with EOT.
//     Carries >=1 dummy push constant so the dispatch is never header-only
//     (WaSendDummyConstantsForPS).
//
// This file has two layers:
//   1. KernelStore — allocates/GGTT-maps the Instruction Base buffer, places kernels
//      64-byte-aligned, and hex-dumps them over serial for the Mesa byte-diff check.
//      (Independent of the exact instruction encoding — implemented now.)
//   2. The GEN9 128-bit instruction encoder + the two kernels themselves.
//      (Encoding filled from the authoritative Gen9 EU ISA field layout; validated by
//      byte-diff against Mesa's assembler — the only safe way without a GPU debugger.)

#![allow(dead_code)]

use super::{RenderError, GVA_INSTRUCTION_BASE};

/// One EU instruction is 16 bytes (native, uncompacted). Kernels must be 64B-aligned
/// (the Kernel Start Pointer fields are 64-byte-granular offsets from Instruction Base).
pub const EU_INSN_BYTES: usize = 16;
pub const KERNEL_ALIGN: usize = 64;

/// Owns the Instruction Base buffer that holds all shader kernels. Kernel Start Pointers
/// programmed into 3DSTATE_VS / 3DSTATE_PS are 64B-granular offsets into this buffer.
pub struct KernelStore {
    /// CPU-visible virtual address of the instruction buffer.
    pub virt: u64,
    /// GGTT base (== GVA_INSTRUCTION_BASE); also the STATE_BASE_ADDRESS Instruction Base.
    pub gva: u32,
    /// Bytes used so far (bump allocator, kept 64B-aligned).
    pub used: usize,
    /// Total capacity in bytes.
    pub capacity: usize,
    /// Kernel Start Pointer offsets (from instruction base) for the placed kernels.
    pub vs_off: u32,
    pub ps_off: u32,
    /// Six constant-color PS kernels, one per cube face (Phase 6 Step 4 per-face color).
    /// `ps_off` aliases `ps_offs[0]` for the single-color callers.
    pub ps_offs: [u32; 6],
}

impl KernelStore {
    /// Allocate `pages` contiguous 4KB pages, zero them, and GGTT-map them starting at
    /// GVA_INSTRUCTION_BASE.
    pub unsafe fn new(mmio_base: u64, pages: usize) -> Result<Self, RenderError> {
        let frame = crate::memory::allocate_contiguous(pages, 4096, true)
            .ok_or(RenderError::NotInitialized)?;
        let phys = frame.start_address().as_u64();
        let virt = crate::memory::phys_to_virt(phys).ok_or(RenderError::NotInitialized)?;
        core::ptr::write_bytes(virt as *mut u8, 0, pages * 4096);

        // GGTT-map page by page (map_ggtt_page lives on RenderEngine; replicate the PTE
        // write here so KernelStore is self-contained).
        for i in 0..pages {
            let gtt_offset = 0x800000 + (((GVA_INSTRUCTION_BASE / 4096) as u64 + i as u64) * 8);
            let gtt_ptr = (mmio_base + gtt_offset) as *mut u64;
            let pte = ((phys + (i as u64 * 4096)) & 0xFFFF_FFFF_FFFF_F000) | 0x07;
            core::ptr::write_volatile(gtt_ptr, pte);
            let _ = core::ptr::read_volatile(gtt_ptr);
        }

        Ok(Self {
            virt,
            gva: GVA_INSTRUCTION_BASE,
            used: 0,
            capacity: pages * 4096,
            vs_off: 0,
            ps_off: 0,
            ps_offs: [0; 6],
        })
    }

    /// Copy an assembled kernel (raw EU bytes) into the store at a 64B-aligned offset.
    /// Returns the byte offset from Instruction Base (== the Kernel Start Pointer value,
    /// which the 3DSTATE_* commands then shift right by 6).
    pub unsafe fn place(&mut self, kernel: &[u8]) -> Result<u32, RenderError> {
        let off = (self.used + KERNEL_ALIGN - 1) & !(KERNEL_ALIGN - 1);
        if off + kernel.len() > self.capacity {
            return Err(RenderError::NotInitialized);
        }
        let dst = (self.virt as *mut u8).add(off);
        for (i, &b) in kernel.iter().enumerate() {
            dst.add(i).write_volatile(b);
        }
        // Flush the written range so the GPU instruction cache fetch (post SBA
        // instruction-cache-invalidate) sees it.
        let mut p = off & !63;
        while p < off + kernel.len() {
            core::arch::asm!("clflush [{}]", in(reg) (self.virt as usize + p),
                options(nostack, preserves_flags));
            p += 64;
        }
        core::arch::asm!("mfence", options(nostack, preserves_flags));

        self.used = off + kernel.len();
        Ok(off as u32)
    }

    /// Hex-dump a placed kernel over serial so it can be byte-diffed against Mesa's
    /// `brw_disasm` / assembler output — the Phase 3 validation milestone.
    pub unsafe fn dump(&self, label: &str, off: u32, len: usize) {
        crate::serial_println!("[EU] kernel '{}' @ instr+{:#x} ({} bytes):", label, off, len);
        let base = (self.virt as *const u8).add(off as usize);
        let mut i = 0;
        while i < len {
            // 16 bytes (one native EU instruction) per line.
            let a = base.add(i);
            crate::serial_println!(
                "[EU]   {:#06x}: {:08x} {:08x} {:08x} {:08x}",
                i,
                u32::from_le_bytes([*a, *a.add(1), *a.add(2), *a.add(3)]),
                u32::from_le_bytes([*a.add(4), *a.add(5), *a.add(6), *a.add(7)]),
                u32::from_le_bytes([*a.add(8), *a.add(9), *a.add(10), *a.add(11)]),
                u32::from_le_bytes([*a.add(12), *a.add(13), *a.add(14), *a.add(15)]),
            );
            i += EU_INSN_BYTES;
        }
    }
}

// ===========================================================================
// GEN9 128-bit instruction encoder.
//
// Bit positions are absolute within the 128-bit instruction (bit 0 = LSB of DW0,
// bit 127 = MSB of DW3), from Mesa `brw_inst.h` (gen8 column, identical on gfx9).
// Memory layout is little-endian: DW0=bits[31:0] .. DW3=bits[127:96]. Every field
// used here lies within a single dword, so `set` operates on one dword.
//
// Validated at runtime by `encoder_selftest()` against the PRM/Mesa worked example
// `mov(8) g2<1>:F g1<8;8,1>:F` = 00600002 20403AE8 008D0020 00000000, and ultimately
// by byte-diffing the emitted kernels against Mesa's assembler (`i965_asm -g 9`).
// ===========================================================================

use alloc::vec::Vec;

/// Set bits [hi:lo] of the 128-bit instruction to `val` (range must be within one dword).
#[inline]
fn set(inst: &mut [u32; 4], hi: u32, lo: u32, val: u32) {
    let dw = (lo / 32) as usize;
    debug_assert_eq!(dw, (hi / 32) as usize, "field crosses a dword boundary");
    let width = hi - lo + 1;
    let shift = lo % 32;
    let mask: u32 = if width >= 32 { 0xFFFF_FFFF } else { (1u32 << width) - 1 };
    inst[dw] = (inst[dw] & !(mask << shift)) | ((val & mask) << shift);
}

// Register files, hw types, exec sizes, strides, opcodes (Mesa brw_eu_defines.h).
mod enc {
    pub const RF_ARF: u32 = 0;
    pub const RF_GRF: u32 = 1;
    pub const RF_IMM: u32 = 3;

    pub const TY_UD: u32 = 0;
    pub const TY_D: u32 = 1;
    pub const TY_F: u32 = 7;

    pub const EXEC_SIMD8: u32 = 3;
    pub const EXEC_SIMD16: u32 = 4;

    pub const HS_1: u32 = 1; // horizontal stride 1
    pub const W_8: u32 = 3; // width 8
    pub const VS_8: u32 = 4; // vertical stride 8

    // GEN hardware opcodes (7-bit field [6:0]). Authoritative values from Mesa
    // src/intel/compiler/gen/gen_opcodes.py "pre_xe" column (pre_xe == Gen4..Gen11,
    // covers our Gen9). These are the HARDWARE opcodes written to the instruction
    // word — NOT Mesa's internal sequential `enum opcode` indices.
    //   mov=1, sel=2, movi=3, send=49, sendc=50, sends=51, nop=126.
    // Earlier this file used mov=2 (=sel!) and send=51 (=sends, split-send), which
    // silently encoded every `mov` as `sel` and every `send` as a split-send whose
    // message descriptor the HW reads from different fields -> URB write never lands
    // -> 3DPRIMITIVE deadlock (VS_inv>0, CL_inv=0). See gen9-eu-opcode-bug memory.
    pub const OP_MOV: u32 = 1;
    pub const OP_SEND: u32 = 49; // legacy single-descriptor send (matches our encoding)
    pub const OP_NOP: u32 = 126;

    // SFIDs
    pub const SFID_URB: u32 = 6;
    pub const SFID_RENDER_CACHE: u32 = 5;
}
use enc::*;

/// A base instruction with opcode + exec size (Align1, WE_normal, uncompacted).
fn base(opcode: u32, exec: u32) -> [u32; 4] {
    let mut i = [0u32; 4];
    set(&mut i, 6, 0, opcode); // opcode [6:0]
    set(&mut i, 23, 21, exec); // exec size [23:21]
    i // access mode (8)=Align1, cmpt(29)=0, mask ctrl(34)=WE_normal all left 0
}

/// Program the destination register region (Align1 direct).
fn set_dst(i: &mut [u32; 4], file: u32, ty: u32, regnum: u32, subreg: u32, hstride: u32) {
    set(i, 36, 35, file);
    set(i, 40, 37, ty);
    set(i, 52, 48, subreg);
    set(i, 60, 53, regnum);
    set(i, 62, 61, hstride);
    // addr mode [63]=0 Direct
}

/// Program src0 as a GRF region <vstride;width,hstride>.
fn set_src0_grf(i: &mut [u32; 4], ty: u32, regnum: u32, subreg: u32, vs: u32, w: u32, hs: u32) {
    set(i, 42, 41, RF_GRF);
    set(i, 46, 43, ty);
    set(i, 68, 64, subreg);
    set(i, 76, 69, regnum);
    set(i, 81, 80, hs);
    set(i, 84, 82, w);
    set(i, 88, 85, vs);
}

/// Program src0 as a 32-bit immediate (value lands in DW3 / bits [127:96]).
fn set_src0_imm(i: &mut [u32; 4], ty: u32, imm: u32) {
    set(i, 42, 41, RF_IMM);
    set(i, 46, 43, ty);
    i[3] = imm;
}

/// `mov(exec) gDst<1>:ty  gSrc<vs;w,hs>:ty`
fn mov_grf(exec: u32, ty: u32, dst: u32, src: u32) -> [u32; 4] {
    let mut i = base(OP_MOV, exec);
    set_dst(&mut i, RF_GRF, ty, dst, 0, HS_1);
    set_src0_grf(&mut i, ty, src, 0, VS_8, W_8, HS_1);
    i
}

/// `mov(exec) gDst<1>:F  imm:F` (broadcast a scalar immediate to all lanes).
fn mov_imm_f(exec: u32, dst: u32, imm: f32) -> [u32; 4] {
    let mut i = base(OP_MOV, exec);
    set_dst(&mut i, RF_GRF, TY_F, dst, 0, HS_1);
    set_src0_imm(&mut i, TY_F, imm.to_bits());
    i
}

/// `send(exec) null  gBase<8;8,1>:UD  desc` with SFID and an immediate 32-bit message
/// descriptor (carries header-present, rlen, mlen and EOT in DW3). SFID goes in [27:24].
fn send_eot(exec: u32, sfid: u32, base_grf: u32, desc: u32) -> [u32; 4] {
    let mut i = base(OP_SEND, exec);
    set(&mut i, 27, 24, sfid); // SFID overlays the cond-modifier field
    // dst = ARF null (rlen carried in desc).
    set_dst(&mut i, RF_ARF, TY_UD, 0, 0, HS_1);
    // src0 = message payload base GRF.
    set_src0_grf(&mut i, TY_UD, base_grf, 0, VS_8, W_8, HS_1);
    // src1 file = IMM signals an immediate descriptor (bits [90:89]); desc in DW3.
    set(&mut i, 90, 89, RF_IMM);
    i[3] = desc; // full 32-bit descriptor incl. EOT(31), mlen[28:25], rlen[24:20], header(19)
    i
}

/// Push a 16-byte instruction (little-endian) onto a kernel byte buffer.
fn emit(buf: &mut Vec<u8>, inst: [u32; 4]) {
    for dw in inst {
        buf.extend_from_slice(&dw.to_le_bytes());
    }
}

// ---------------------------------------------------------------------------
// Message descriptors (from the Gen9 encoding reference).
// ---------------------------------------------------------------------------

/// URB SIMD8 write: opcode 7, global offset 0, header present, rlen 0.
/// `mlen` = 1 (header) + number of data GRFs.
fn urb_write_desc(mlen: u32) -> u32 {
    let opcode = 7u32; // GEN8_URB_OPCODE_SIMD8_WRITE, desc[3:0]
    (1 << 31)            // EOT
        | (mlen << 25)   // message length [28:25]
        | (1 << 19)      // header present
        | opcode
}

/// Render Target Write, SIMD8 single source (mode 4), last RT, HEADERLESS, BTI 0.
/// `mlen` = color GRFs only (4). rlen 0.
///
/// HEADERLESS is the correct Gen9 path for a trivial fragment shader, confirmed against
/// Mesa's ELK `lower_fb_write_logical_send`: for gfx9 (verx10=90) with a single RT, no
/// dual-source and no kill, `header_size == 0`. The render cache derives each pixel's
/// destination from the fixed-function PS dispatch (not from a message header), and the
/// end-of-thread pixel mask comes from the execution mask — so no header is needed.
/// Mesa only builds a 2-GRF header for dual-source / multi-color-region writes, and even
/// then it is a *constructed* g0-derived header (RT index at .2, oMask at .15) — NEVER a
/// raw R0/R1 forward. Our previous "header present + forward R0/R1" put the PS payload
/// (which is NOT in RT-write-header format) where the render cache reads header fields,
/// producing per-thread GARBAGE destination addresses -> the wild, per-boot-varying GGTT
/// page faults (FAULT_VA=0xfceca000, codes 0xd1/0x8d9).
///
/// Descriptor field bits verified vs Mesa `elk_fb_desc`/`elk_fb_write_desc` (gfx7+):
///   BTI [7:0], msg_control [13:8], Last RT [12], msg_type [17:14]=12, Header Present [19].
fn rt_write_desc(mlen: u32, bti: u32) -> u32 {
    let msgtype = 12u32; // GFX6_DATAPORT_WRITE_MESSAGE_RENDER_TARGET_WRITE, desc[17:14]
    let mode = 4u32; // SIMD8_SINGLE_SOURCE_SUBSPAN01, msg_control desc[13:8]
    (1 << 31)                 // EOT (instruction bit 127)
        | (mlen << 25)        // message length [28:25]
        // header-present bit [19] deliberately CLEAR -> headerless
        | (msgtype << 14)     // message type [17:14]
        | (1 << 12)           // Last Render Target Select [12]
        | (mode << 8)         // RT write mode / msg_control [13:8]
        | (bti & 0xFF)        // binding table index [7:0]
}

// ---------------------------------------------------------------------------
// The two kernels.
//
// NOTE: the input-GRF choices (VS vertex data, PS payload) are the payload layout
// Phase 5 finalizes via 3DSTATE_VS URB-read config + 3DSTATE_VERTEX_ELEMENTS. The
// encoding is what Phase 3 validates; the exact source registers are refined then.
// ---------------------------------------------------------------------------

/// Pass-through vertex shader (SIMD8): forward the 8 URB return handles from R1 into
/// the URB message header, copy the vec4 position from the input GRFs into the URB
/// data phases, and emit a URB write with EOT.
pub fn build_vs() -> Vec<u8> {
    let mut k = Vec::new();
    // The output VUE is [DW0..3 = VUE header][DW4..7 = clip-space position]; the SF/CLIP
    // read position from DW4..7. Input position (from the VF) is preloaded at g2..g5
    // (Dispatch GRF Start = 2), one component per GRF across the 8 SIMD lanes.
    //
    // Message layout: g6 = URB handles header; g7..g10 = VUE header (zeros);
    //                 g11..g14 = position x,y,z,w.
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_UD, 6, 1)); // handles from R1
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 7, 0.0)); // VUE header dw0
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 8, 0.0)); // dw1
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 9, 0.0)); // dw2
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 10, 0.0)); // dw3
    // Full vec4 pass-through: the VF deinterleaves the R32G32B32A32 position into
    // g2=x g3=y g4=z g5=w (scalar SIMD8 = 4 GRFs per attribute slot, urb_read_length=1
    // covers all four — verified vs Mesa ELK elk_fs.cpp:1575 / elk_vec4.cpp:2608). We
    // now forward z and w too, so a CPU-computed clip-space vertex (w = -z_eye) survives
    // the clipper's perspective divide. Backward compatible with the Phase-5 triangle,
    // whose verts are already [x,y,0,1] -> forwarding g4=0,g5=1 reproduces it exactly.
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 11, 2)); // pos x <- input g2
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 12, 3)); // pos y <- input g3
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 13, 4)); // pos z <- input g4
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 14, 5)); // pos w <- input g5
    // URB write: base = g6, mlen = 1 handle-header + 8 data (VUE header + position) = 9, EOT.
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_URB, 6, urb_write_desc(9)));
    k
}

/// Constant-color pixel shader (SIMD8 single source): forward the R0/R1 header, load
/// the RGBA color into the message color GRFs, and emit a Render Target write with EOT.
pub fn build_ps(color: [f32; 4]) -> Vec<u8> {
    let mut k = Vec::new();
    // HEADERLESS SIMD8 single-source RT write (Mesa ELK simple-FS path). No R0/R1 header
    // forward: the render cache derives per-pixel destination from the fixed-function PS
    // dispatch, and the EOT pixel mask comes from the execution mask. The message payload
    // is just the 4 color GRFs (R,G,B,A), each broadcast to all 8 lanes.
    //
    // Colors go at g2..g5 (the send base). g0/g1 remain the PS thread header (R0/R1) but
    // are not part of the message. Output order is canonical RGBA; the B8G8R8A8_UNORM
    // surface format handles the channel/byte swizzle on store.
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 2, color[0])); // R
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 3, color[1])); // G
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 4, color[2])); // B
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 5, color[3])); // A
    // RT write: base = g2, mlen = 4 color GRFs (no header), BTI 0, Last RT, EOT.
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_RENDER_CACHE, 2, rt_write_desc(4, 0)));
    k
}

/// Validate the encoder core against a `mov(8) g2<1>:F g1<8;8,1>:F`. DW0 must carry
/// hardware opcode 1 (mov) in [6:0] and SIMD8 (3) in [23:21] => 0x0060_0001. (The old
/// reference used 0x0060_0002, i.e. opcode 2 = `sel` — it validated the bug. Fixed to
/// the real mov opcode; see gen_opcodes.py pre_xe.)
pub fn encoder_selftest() -> bool {
    let got = mov_grf(EXEC_SIMD8, TY_F, 2, 1);
    let want = [0x0060_0001u32, 0x2040_3AE8, 0x008D_0020, 0x0000_0000];
    if got == want {
        crate::serial_println!("[EU] encoder self-test PASS (mov(8) g2:F,g1:F encodes correctly)");
        true
    } else {
        crate::serial_println!(
            "[EU] encoder self-test FAIL: got {:08x} {:08x} {:08x} {:08x} want {:08x} {:08x} {:08x} {:08x}",
            got[0], got[1], got[2], got[3], want[0], want[1], want[2], want[3]
        );
        false
    }
}

/// Phase 3 milestone entry: verify the encoder, then build both kernels, place them in
/// a fresh instruction-base store, and hex-dump them over serial for byte-diff against
/// Mesa. Returns the store (kernels remain mapped at GVA_INSTRUCTION_BASE for Phase 5).
pub unsafe fn build_and_dump_kernels(mmio_base: u64) -> Result<KernelStore, RenderError> {
    encoder_selftest();

    let mut store = KernelStore::new(mmio_base, 1)?;

    let vs = build_vs();
    let vs_off = store.place(&vs)?;
    store.vs_off = vs_off;
    store.dump("VS pass-through", vs_off, vs.len());

    // Six constant-color PS kernels, one per cube face. Distinct, bright colors so the
    // three visible faces of the back-face-culled cube read as clearly different planes.
    // Order matches cube_geometry()'s face grouping: front,back,left,right,top,bottom.
    const FACE_COLORS: [[f32; 4]; 6] = [
        [1.0, 0.2, 0.2, 1.0], // front (+z) red
        [0.2, 1.0, 0.2, 1.0], // back  (-z) green
        [0.3, 0.5, 1.0, 1.0], // left  (-x) blue
        [1.0, 0.9, 0.2, 1.0], // right (+x) yellow
        [0.3, 1.0, 1.0, 1.0], // top   (+y) cyan
        [1.0, 0.4, 1.0, 1.0], // bottom(-y) magenta
    ];
    for (i, color) in FACE_COLORS.iter().enumerate() {
        let ps = build_ps(*color);
        let off = store.place(&ps)?;
        store.ps_offs[i] = off;
    }
    store.ps_off = store.ps_offs[0]; // back-compat alias for single-color callers
    store.dump("PS face[0] red", store.ps_offs[0], build_ps(FACE_COLORS[0]).len());

    crate::serial_println!(
        "[EU] kernels placed: VS={:#x}, PS faces={:#x?} (offsets from instruction base)",
        vs_off, store.ps_offs
    );
    Ok(store)
}
