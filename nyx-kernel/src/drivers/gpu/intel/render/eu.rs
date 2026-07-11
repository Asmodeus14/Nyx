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
    /// U4 Boot 2: textured PS that OVERWRITES the sampled alpha with a per-window opacity
    /// interpolated from the varying's channel 2 (`varying.z`). Used ONLY by window quads
    /// (`Scene::add_window_quad`); the cube/pyramid/panel/glcube keep the byte-identical
    /// `ps_off` shader, so this adds zero regression surface to the proven textured path.
    pub ps_opacity_off: u32,
    /// U4 Boot 3: opacity PS + corner-mask multiply (rounded window corners). Used by window quads
    /// when a corner mask is bound (BTI 2); samples the mask (sampler 1, bilinear) and folds its
    /// alpha into the window alpha. 0 until placed → add_window_quad falls back to ps_opacity_off.
    pub ps_rounded_off: u32,
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
            ps_opacity_off: 0,
            ps_rounded_off: 0,
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
    pub const OP_PLN: u32 = 90;  // plane: dst = p*u + q*v + r (2-source, perspective interp)
    // add=0x40, mul=0x41 — the ALU-arithmetic opcode block (gen4..gen11 "pre_xe", same table as the
    // HW-verified mov/send/pln/nop above). Cross-checked against the Intel Gen ISA PRM instruction-
    // encoding table (ADD=0x40, MUL=0x41). Used by the U4 rounded-corner PS to multiply the corner
    // mask's alpha into the window alpha. The encoder core is already validated by encoder_selftest().
    pub const OP_MUL: u32 = 0x41; // 65
    pub const OP_NOP: u32 = 126;

    // SFIDs
    pub const SFID_URB: u32 = 6;
    pub const SFID_RENDER_CACHE: u32 = 5;
    pub const SFID_SAMPLER: u32 = 2; // 3D sampler / sampling engine
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

/// Program src1 as a GRF region <vstride;width,hstride>. Field bits mirror src0 shifted +32
/// into DW2-top/DW3 (PRM Vol2a: Src1.RegFile [90:89], Src1.SrcType [94:91]; region [120:96]).
fn set_src1_grf(i: &mut [u32; 4], ty: u32, regnum: u32, subreg: u32, vs: u32, w: u32, hs: u32) {
    set(i, 90, 89, RF_GRF);
    set(i, 94, 91, ty);
    set(i, 100, 96, subreg);
    set(i, 108, 101, regnum);
    set(i, 113, 112, hs);
    set(i, 116, 114, w);
    set(i, 120, 117, vs);
}

/// `pln(exec) gDst<1>:F  gSetup.<sub><0;1,0>:F  gBary<8;8,1>:F` — perspective-correct plane
/// interpolation of ONE attribute channel. Computes dst = p*u + q*v + r where src0 (setup) is a
/// replicated scalar whose 4-tuple holds p (a1) at sub .0, q (a2) at .1, r (a0) at .3, and src1
/// (barycentric) supplies u = gBary (b1) and v = gBary+1 (b2, implied by HW). `setup_subreg` is
/// a BYTE offset and MUST be 0 or 16 (PRM restriction: src0 sub-register .0 or .4).
fn pln(exec: u32, dst: u32, setup_reg: u32, setup_subreg: u32, bary_reg: u32) -> [u32; 4] {
    let mut i = base(OP_PLN, exec);
    set_dst(&mut i, RF_GRF, TY_F, dst, 0, HS_1);
    // src0 = setup: replicated scalar region <0;1,0> (VertStride 0, Width 1(enc 0), HorzStride 0).
    set_src0_grf(&mut i, TY_F, setup_reg, setup_subreg, 0, 0, 0);
    // src1 = barycentric u vector <8;8,1>; HW auto-reads bary_reg+1 as v.
    set_src1_grf(&mut i, TY_F, bary_reg, 0, VS_8, W_8, HS_1);
    i
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

/// `mul(exec) gDst<1>:F  gSrc0<8;8,1>:F  gSrc1<8;8,1>:F` — per-lane float multiply (2-source ALU).
/// Reuses the same region encoders as every other instruction here; src0/src1 are full SIMD8 GRF
/// vectors. Used by the U4 rounded-corner PS to fold the corner mask's alpha into the window alpha
/// (`alpha = opacity * mask_alpha`). dst may alias src0 (read-modify-write of the same GRF is legal;
/// the HW scoreboard serializes the in-place update).
fn mul_grf(exec: u32, dst: u32, src0: u32, src1: u32) -> [u32; 4] {
    let mut i = base(OP_MUL, exec);
    set_dst(&mut i, RF_GRF, TY_F, dst, 0, HS_1);
    set_src0_grf(&mut i, TY_F, src0, 0, VS_8, W_8, HS_1);
    set_src1_grf(&mut i, TY_F, src1, 0, VS_8, W_8, HS_1);
    i
}

/// `mov(exec) gDst<1>:F  gSrc.<subreg_bytes><0;1,0>:F` — broadcast ONE scalar float from a
/// source sub-register to all `exec` lanes. Used to pull a flat (constant-interpolated) PS
/// attribute out of its plane-setup GRF: the attribute's a0/constant term lives at the `r`
/// sub-register of the PLN setup tuple (Vol2a "pln": r = 4th component, sub-reg .3 within a
/// .0-based tuple or .7 within a .4-based tuple). `src_subreg` is a BYTE offset in [0,31].
fn mov_scalar_f(exec: u32, dst: u32, src: u32, src_subreg: u32) -> [u32; 4] {
    let mut i = base(OP_MOV, exec);
    set_dst(&mut i, RF_GRF, TY_F, dst, 0, HS_1);
    // src0 = replicated scalar g{src}.{subreg}<0;1,0>:F (VertStride=0, Width=1, HorzStride=0).
    set(&mut i, 42, 41, RF_GRF);
    set(&mut i, 46, 43, TY_F);
    set(&mut i, 68, 64, src_subreg); // sub-register number (bytes)
    set(&mut i, 76, 69, src);        // register number
    set(&mut i, 81, 80, 0);          // HorzStride = 0
    set(&mut i, 84, 82, 0);          // Width = 1 element (encoded 0)
    set(&mut i, 88, 85, 0);          // VertStride = 0
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

/// `send(exec) gDst<1>:UD  gBase<8;8,1>:UD  desc` — a message that RETURNS data to a GRF
/// (Response Length > 0 in `desc`) and is NOT end-of-thread. The thread keeps running after
/// the writeback lands, so a later message (e.g. the RT write) can consume the result. This
/// is the first message in this file with a nonzero response length (the sampler `sample`).
fn send_msg(exec: u32, sfid: u32, dst_grf: u32, base_grf: u32, desc: u32) -> [u32; 4] {
    let mut i = base(OP_SEND, exec);
    set(&mut i, 27, 24, sfid);
    set_dst(&mut i, RF_GRF, TY_UD, dst_grf, 0, HS_1); // response lands here (rlen GRFs)
    set_src0_grf(&mut i, TY_UD, base_grf, 0, VS_8, W_8, HS_1);
    set(&mut i, 90, 89, RF_IMM); // immediate descriptor in DW3
    i[3] = desc;
    i
}

/// 3D sampler `sample` message descriptor. Field layout (PRM Vol7 "3D Sampler" + Mesa
/// `brw_sampler_desc`, gfx7+): Binding Table Index [7:0], Sampler Index [11:8], Message Type
/// [16:12], SIMD Mode [18:17]; then the send-common Header Present [19], Response Length
/// [24:20], Message Length [28:25], EOT [31]. Header is OMITTED — the Sampler State Pointer is
/// supplied by 3DSTATE_SAMPLER_STATE_POINTERS_PS (PRM Vol7: "may be delivered from the Command
/// Streamer without the need for a Message Header"). EOT stays 0 (the RT write is the EOT msg).
/// For SIMD8 the payload is N coordinate GRFs (mlen = N, u then v) and the writeback is 4 GRFs
/// of RGBA (rlen = 4) — PRM Vol7 "Message Lengths / Response Length" tables.
fn sampler_desc(bti: u32, sampler: u32, msg_type: u32, simd_mode: u32, mlen: u32, rlen: u32) -> u32 {
    (bti & 0xFF)
        | (sampler << 8)     // Sampler Index [11:8]
        | (msg_type << 12)   // Message Type [16:12] (SAMPLE = 0)
        | (simd_mode << 17)  // SIMD Mode [18:17] (SIMD8 = 1)
        // header present [19] = 0 (headerless)
        | (rlen << 20)       // Response Length [24:20]
        | (mlen << 25)       // Message Length [28:25]
    // EOT [31] = 0
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

/// URB SIMD8 write: opcode 7, header present, rlen 0.
/// `mlen` = 1 (header) + number of data GRFs. The data payload is limited to 1..8 message
/// phases (PRM Vol7 p288: "The write data payload can be between 1 and 8 message phases long")
/// -> a VUE larger than 8 dwords MUST be split across multiple writes. `offset` = Global Offset
/// [14:4]. For URB_SIMD8_WRITE this is in 128-bit (4-dword) units from the VUE start (PRM Vol7
/// line ~32047 -- NOT 256-bit like the HWORD messages). `eot` sets End-Of-Thread (desc bit 31);
/// only the LAST write of a thread is EOT.
fn urb_write_desc(mlen: u32, offset: u32, eot: bool) -> u32 {
    let opcode = 7u32; // GEN8_URB_OPCODE_SIMD8_WRITE, desc[3:0]
    (if eot { 1 << 31 } else { 0 })  // End Of Thread
        | (mlen << 25)               // message length [28:25]
        | (1 << 19)                  // header present (mandatory for URB messages)
        | (offset << 4)              // Global Offset [14:4], 256-bit units
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
    // Phase 7 Boot A: pass through position AND a per-vertex color varying.
    //
    // Output VUE layout (256-bit units; SBE reads attributes at Read Offset 1 = DW8):
    //   DW0..3  = VUE header (zeros)
    //   DW4..7  = clip-space position   (SF/CLIP read position here)
    //   DW8..11 = color varying          (SBE routes this to SF output attribute 0)
    //
    // Input payload (VF, Dispatch GRF Start = 2, deinterleaved SIMD8 = 4 GRFs/vec4):
    //   g1      = URB return handles (R1)
    //   g2..g5  = position x,y,z,w   (vertex element 0)
    //   g6..g9  = color r,g,b,a      (vertex element 1)
    //
    // ROOT CAUSE of the whole 2-slot-VUE hang saga (Boot #10, PRM Vol7 p288): a single URB
    // write payload is limited to 1..8 message phases. Our 2-slot VUE is 12 data dwords (header
    // 4 + pos 4 + color 4); writing all 12 in ONE URB write (mlen 13) exceeds the 8-phase limit
    // and silently WEDGES the URB unit -> no-fault drain hang. Boot #9 worked only because it
    // wrote exactly 8 phases (position-only). FIX: split into TWO URB writes, each <= 8 phases.
    //
    // Write 1 (NOT EOT): VUE DW0-7 = header + position, Global Offset 0, 8 data phases.
    //   g10 = handles, g11-14 = VUE header (zeros), g15-18 = position.
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_UD, 10, 1)); // handles from R1
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 11, 0.0)); // VUE header dw0
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 12, 0.0)); // dw1
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 13, 0.0)); // dw2
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 14, 0.0)); // dw3
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 15, 2)); // pos x <- g2
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 16, 3)); // pos y <- g3
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 17, 4)); // pos z <- g4
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 18, 5)); // pos w <- g5
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_URB, 10, urb_write_desc(9, 0, false)));
    // Write 2 (EOT): VUE DW8-11 = color. Global Offset is in 128-bit (4-dword) units for
    // URB_SIMD8_WRITE, so DW8 = unit 2 (unit0=header DW0-3, unit1=position DW4-7, unit2=color
    // DW8-11). Boot #11 used offset 1 -> wrote color onto DW4-7 = POSITION -> degenerate tris,
    // PS_inv=0. Correct offset is 2. 4 data phases.
    //   g19 = handles (again), g20-23 = color. Message must be contiguous so re-copy handles.
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_UD, 19, 1)); // handles from R1 (2nd message header)
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 20, 6)); // color r <- g6
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 21, 7)); // color g <- g7
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 22, 8)); // color b <- g8
    emit(&mut k, mov_grf(EXEC_SIMD8, TY_F, 23, 9)); // color a <- g9
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_URB, 19, urb_write_desc(5, 2, true)));
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

/// Phase 7 Boot A pixel shader: output the FLAT (constant-interpolated) per-vertex color
/// attribute delivered by the SF. Validates the whole VUE->SBE->PS attribute path with a
/// single kernel + single draw (replacing the 6-kernel constant-color hack).
///
/// PS payload with all optional phases (barycentric/depth/W/coverage) DISABLED and Dispatch
/// GRF Start for Setup Data = 2: g0/g1 = R0/R1 header, then attribute-0 plane setup at g2/g3
/// (one vec4 attribute = 2 GRFs, two channels per GRF). For a constant-interpolated
/// attribute a0 = provoking-vertex value and a1=a2=0, so the value is the PLN `r` term:
///   channel .0-based tuple -> r at sub-reg 3 (byte 12); .4-based tuple -> r at sub-reg 7 (byte 28).
///   R = g2.3, G = g2.7, B = g3.3, A = g3.7   (Vol2a "pln" + Mesa interp_reg agree).
/// Broadcast each scalar to all 8 lanes into the RT-write color payload g4..g7 (kept clear
/// of the g2/g3 setup registers), then a headerless RT write with EOT.
/// Boot A flat-attribute PS (kept for reference). Reads the constant term a0 directly.
pub fn build_ps_attr_flat() -> Vec<u8> {
    let mut k = Vec::new();
    emit(&mut k, mov_scalar_f(EXEC_SIMD8, 4, 2, 12)); // R <- g2.3
    emit(&mut k, mov_scalar_f(EXEC_SIMD8, 5, 2, 28)); // G <- g2.7
    emit(&mut k, mov_scalar_f(EXEC_SIMD8, 6, 3, 12)); // B <- g3.3
    emit(&mut k, mov_scalar_f(EXEC_SIMD8, 7, 3, 28)); // A <- g3.7
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_RENDER_CACHE, 4, rt_write_desc(4, 0)));
    k
}

/// Boot B: PERSPECTIVE-CORRECT interpolation via `pln`. With 3DSTATE_WM Barycentric
/// Interpolation Mode = BIM_PERSPECTIVE_PIXEL the PS payload is: g0/g1 = header, g2 = bary b1,
/// g3 = bary b2, then attribute-0 setup at g4/g5 (Dispatch GRF Start for Setup = 4). Each color
/// channel is pln'd: dst = a1*b1 + a2*b2 + a0. Per-channel setup 4-tuple lives at:
///   R = g4 sub .0, G = g4 sub .4 (byte 16), B = g5 sub .0, A = g5 sub .4.
/// A is set to a constant 1.0 (cube colors are opaque; avoids a 4th pln). RT payload g10-g13.
pub fn build_ps_attr() -> Vec<u8> {
    let mut k = Vec::new();
    emit(&mut k, pln(EXEC_SIMD8, 10, 4, 0, 2));   // R = pln(setup ch0 @g4.0, bary @g2)
    emit(&mut k, pln(EXEC_SIMD8, 11, 4, 16, 2));  // G = pln(setup ch1 @g4.4, bary @g2)
    emit(&mut k, pln(EXEC_SIMD8, 12, 5, 0, 2));   // B = pln(setup ch2 @g5.0, bary @g2)
    emit(&mut k, mov_imm_f(EXEC_SIMD8, 13, 1.0)); // A = 1.0
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_RENDER_CACHE, 10, rt_write_desc(4, 0)));
    k
}

/// Boot C: TEXTURED pixel shader. Interpolates the per-vertex UV varying (via `pln`, exactly
/// as Boot B did for color) and uses it to `sample` a texture, then RT-writes the sampled
/// RGBA. Reuses the whole Boot B payload layout (BIM_PERSPECTIVE_PIXEL, Dispatch GRF Start for
/// Setup = 4): g0/g1 = header, g2/g3 = barycentric (b1,b2), g4/g5 = attribute-0 plane setup.
/// UV rides in the first two channels of attribute 0 (the VS forwards it into VUE DW8-11 just
/// like the color varying — no VS/SBE change; only the vertex DATA is UV instead of RGB).
///   U = pln(setup ch0 @g4.0, bary @g2) -> g14
///   V = pln(setup ch1 @g4 sub .4=byte16, bary @g2) -> g15
///   sample(SIMD8, BTI=1, sampler 0): payload g14(U),g15(V) mlen 2, rlen 4 -> g16..g19 = RGBA
///   RT write g16..g19 (headerless, EOT, BTI 0) — the sampled texel becomes the pixel color.
/// The sampler `send` is NOT eot; the RT-write `send_eot` terminates the thread after the
/// texel returns. Coordinate order (u then v) and SIMD8 sizes are from PRM Vol7 message tables.
pub fn build_ps_tex() -> Vec<u8> {
    let mut k = Vec::new();
    emit(&mut k, pln(EXEC_SIMD8, 14, 4, 0, 2));  // U <- attribute-0 channel 0
    emit(&mut k, pln(EXEC_SIMD8, 15, 4, 16, 2)); // V <- attribute-0 channel 1
    // sample: BTI 1 (texture surface), sampler 0, msg type SAMPLE(0), SIMD8(1), mlen 2, rlen 4.
    emit(&mut k, send_msg(EXEC_SIMD8, SFID_SAMPLER, 16, 14, sampler_desc(1, 0, 0, 1, 2, 4)));
    // RT write the sampled RGBA (g16..g19), headerless, EOT.
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_RENDER_CACHE, 16, rt_write_desc(4, 0)));
    k
}

/// U4 Boot 2: TEXTURED pixel shader with PER-WINDOW OPACITY. Identical to `build_ps_tex`
/// (interpolate UV, sample BTI 1 -> g16..g19 = RGBA), but before the RT write it OVERWRITES the
/// sampled alpha (g19) with a per-window opacity value interpolated from attribute-0 CHANNEL 2
/// (the vertex `varying.z`). RGB (g16..g18) is untouched; the src-over blender then computes
/// `dst = O*texel.rgb + (1-O)*dst` — a uniform window fade, independent of whatever alpha the
/// app did (or, usually, did NOT) author into its SHM pixels. This is why opacity is *written
/// into* alpha rather than multiplied with the (commonly-zero) texel alpha: a plain `blend=true`
/// over an alpha=0 window would make it fully transparent (invisible).
///
/// Sourcing (NOT guessed): the channel-2 plane-setup register is `g5.0`, exactly the register
/// `build_ps_attr` (Boot B, HW-verified) reads for its 3rd color channel — `pln(.., 5, 0, 2)`.
/// The SF already delivers all four channels of attribute-0 (Boot B interpolated ch0/ch1/ch2),
/// so no SBE / 3DSTATE_SBE change is needed; only this one extra `pln` is added. The opacity
/// attribute is CONSTANT across the quad (all 4 verts carry the same `varying.z`), so `pln`
/// interpolates it to that constant on every pixel. Payload layout matches `build_ps_tex`:
/// g0/g1 = header, g2/g3 = barycentric, g4/g5 = attribute-0 plane setup.
pub fn build_ps_tex_opacity() -> Vec<u8> {
    let mut k = Vec::new();
    emit(&mut k, pln(EXEC_SIMD8, 14, 4, 0, 2));  // U <- attribute-0 channel 0 (g4.0)
    emit(&mut k, pln(EXEC_SIMD8, 15, 4, 16, 2)); // V <- attribute-0 channel 1 (g4.4)
    // sample: BTI 1, sampler 0, SAMPLE(0), SIMD8(1), mlen 2, rlen 4 -> g16..g19 = RGBA.
    emit(&mut k, send_msg(EXEC_SIMD8, SFID_SAMPLER, 16, 14, sampler_desc(1, 0, 0, 1, 2, 4)));
    // Opacity <- attribute-0 channel 2 (g5.0), overwriting the sampled alpha in g19. Same
    // channel-2 setup register proven by build_ps_attr's B = pln(.., 5, 0, 2).
    emit(&mut k, pln(EXEC_SIMD8, 19, 5, 0, 2));
    // RT write RGB (sampled) + A (opacity), headerless, EOT.
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_RENDER_CACHE, 16, rt_write_desc(4, 0)));
    k
}

/// U4 Boot 3: TEXTURED + OPACITY + ROUNDED-CORNER pixel shader. Extends `build_ps_tex_opacity` with a
/// SECOND texture sample — a corner alpha-MASK at BTI 2 — whose alpha is multiplied into the window
/// alpha. Where the mask alpha is 0 (the rounded-off corners) the final alpha is 0, so src-over shows
/// the desktop through → rounded corners. Elsewhere the mask alpha is 1, leaving the per-window opacity
/// untouched. Final alpha = opacity * mask_alpha.
///
/// Sampler choice: the WINDOW is sampled with sampler index 0 (NEAREST — byte-identical to
/// `build_ps_tex_opacity`, keeps content crisp), the MASK with sampler index 1 (BILINEAR — that's where
/// the smooth anti-aliased corner edge comes from). Both use the same interpolated UV (g14/g15). Payload
/// as before: g0/g1 header, g2/g3 barycentric, g4/g5 attribute-0 plane setup.
///   sample BTI 1, sampler 0 -> g16..g19  (window RGBA)
///   sample BTI 2, sampler 1 -> g20..g23  (mask; alpha in g23)
///   pln g19 = opacity (attr-0 ch2 @ g5.0)   ; alpha = per-window opacity
///   mul g19 = g19 * g23                      ; alpha = opacity * mask_alpha  -> rounded
///   RT-write g16..g19 (RGB from window, A folded), headerless, EOT.
pub fn build_ps_tex_opacity_rounded() -> Vec<u8> {
    let mut k = Vec::new();
    emit(&mut k, pln(EXEC_SIMD8, 14, 4, 0, 2));  // U <- attribute-0 channel 0 (g4.0)
    emit(&mut k, pln(EXEC_SIMD8, 15, 4, 16, 2)); // V <- attribute-0 channel 1 (g4.4)
    // Window texture: BTI 1, sampler 0 (NEAREST) -> g16..g19 = RGBA.
    emit(&mut k, send_msg(EXEC_SIMD8, SFID_SAMPLER, 16, 14, sampler_desc(1, 0, 0, 1, 2, 4)));
    // Corner mask: BTI 2, sampler 0 (same NEAREST sampler as the window — avoids relying on a 2nd
    // SAMPLER_STATE entry / Sampler Count prefetch; BTI and sampler index are independent). -> g20..g23
    // (mask alpha in g23), same U/V payload. Bilinear (sampler 1) is a smoothness upgrade once the mask
    // path is confirmed working.
    emit(&mut k, send_msg(EXEC_SIMD8, SFID_SAMPLER, 20, 14, sampler_desc(2, 0, 0, 1, 2, 4)));
    // Opacity <- attribute-0 channel 2 (g5.0), into g19 (overwrites the window's sampled alpha).
    emit(&mut k, pln(EXEC_SIMD8, 19, 5, 0, 2));
    // Fold the mask alpha in: g19 = opacity * mask_alpha.
    emit(&mut k, mul_grf(EXEC_SIMD8, 19, 19, 23));
    // RT write RGB (window) + A (opacity*mask), headerless, EOT.
    emit(&mut k, send_eot(EXEC_SIMD8, SFID_RENDER_CACHE, 16, rt_write_desc(4, 0)));
    k
}
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

    // Phase 7 Boot C: a SINGLE textured pixel shader — interpolate the UV varying (pln, as
    // Boot B) and sample a checkerboard texture. build_ps_attr (Boot A flat) / build_ps_attr
    // wrapper for Boot B color are kept above for reference.
    let ps = build_ps_tex();
    let ps_off = store.place(&ps)?;
    store.ps_off = ps_off;
    store.ps_offs = [ps_off; 6]; // all faces share the one textured PS (back-compat)
    store.dump("PS textured (sampler)", ps_off, ps.len());

    // U4 Boot 2: the per-window-opacity variant of the textured PS. Only window quads use it
    // (via Scene::add_window_quad); everything else keeps `ps_off` unchanged.
    let pso = build_ps_tex_opacity();
    let ps_opacity_off = store.place(&pso)?;
    store.ps_opacity_off = ps_opacity_off;
    store.dump("PS textured+opacity", ps_opacity_off, pso.len());

    // U4 Boot 3: opacity + corner-mask multiply (rounded corners). Window quads use it when a corner
    // mask is bound at BTI 2; else they fall back to ps_opacity_off. Cube/glcube keep `ps_off`.
    let psr = build_ps_tex_opacity_rounded();
    let ps_rounded_off = store.place(&psr)?;
    store.ps_rounded_off = ps_rounded_off;
    store.dump("PS textured+opacity+rounded", ps_rounded_off, psr.len());

    crate::serial_println!(
        "[EU] kernels placed: VS={:#x}, PS(attr)={:#x} (offsets from instruction base)",
        vs_off, ps_off
    );
    Ok(store)
}
