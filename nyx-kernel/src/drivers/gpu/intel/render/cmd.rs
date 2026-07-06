// src/drivers/gpu/intel/render/cmd.rs
//
// Typed GEN9 command DWORD builders.
//
// Rationale: on real hardware with no GPU debugger, a single wrong bit in a command
// packet silently hangs the engine. Rather than scatter magic numbers, all command
// headers are built here from named opcode fields cross-referenced to the PRM
// (Vol 2a "Command Reference: Instructions"). Every command header on Gen9 encodes:
//
//   bits [31:29] = Command Type    (0x3 for GFXPIPE / render commands, 0x0 for MI)
//   bits [28:27] = Pipeline        (GFXPIPE only: 0=common, 1=single-dw, 2=media, 3=3D)
//   bits [26:24] = Opcode
//   bits [23:16] = Sub-opcode
//   bits [15:0]  = DWord Length    (total dwords - 2), for variable-length commands
//
// MI commands (type 0x0) use bits [28:23] as the opcode and a command-specific
// low field. See PRM Vol 2a "MI_* " sections.

#![allow(dead_code)]

use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Command type / pipeline nibbles
// ---------------------------------------------------------------------------
pub const CMD_TYPE_MI: u32 = 0x0 << 29;
pub const CMD_TYPE_GFXPIPE: u32 = 0x3 << 29;

pub const PIPELINE_COMMON: u32 = 0x0 << 27;
pub const PIPELINE_SINGLE_DW: u32 = 0x1 << 27;
pub const PIPELINE_MEDIA: u32 = 0x2 << 27;
pub const PIPELINE_3D: u32 = 0x3 << 27;

/// Build a GFXPIPE (render) command header.
/// `dword_len` is the full command length in dwords minus 2 (the standard bias).
#[inline]
pub const fn gfxpipe_header(pipeline: u32, opcode: u32, subopcode: u32, dword_len: u32) -> u32 {
    CMD_TYPE_GFXPIPE
        | (pipeline & (0x3 << 27))
        | ((opcode & 0x7) << 24)
        | ((subopcode & 0xFF) << 16)
        | (dword_len & 0xFFFF)
}

// ---------------------------------------------------------------------------
// MI commands (PRM Vol 2a). Opcode occupies bits [28:23].
// ---------------------------------------------------------------------------
pub const MI_NOOP: u32 = 0x00 << 23; // 0x00000000
pub const MI_BATCH_BUFFER_END: u32 = 0x0A << 23;

/// MI_STORE_DATA_IMM header. Writes an immediate dword (or qword) to a GTT/PPGTT
/// address. Matches the form already used by the BLT driver's fence
/// (`0x20<<23 | (1<<22) | len`). `use_gtt` sets the "Use Global GTT" bit (22).
/// `dword_len` = total dwords - 2.
#[inline]
pub const fn mi_store_data_imm(use_gtt: bool, dword_len: u32) -> u32 {
    (0x20 << 23) | ((use_gtt as u32) << 22) | (dword_len & 0x3FF)
}

/// MI_BATCH_BUFFER_START header. `second_level` selects a 2nd-level batch;
/// `use_ppgtt` clear => address space is GGTT. Length bias per PRM.
#[inline]
pub const fn mi_batch_buffer_start(use_ppgtt: bool, dword_len: u32) -> u32 {
    // Opcode 0x31, bit 8 = Address Space Indicator (1=PPGTT,0=GGTT).
    (0x31 << 23) | ((use_ppgtt as u32) << 8) | (dword_len & 0xFF)
}

// ---------------------------------------------------------------------------
// PIPELINE_SELECT (PRM Vol 2a) — type 3, pipeline 1 (single-dw), opcode 1, sub 4.
// Requires mask bits [15:8] set to validate the [7:0] selection bits (Gen9 gotcha).
// ---------------------------------------------------------------------------
pub const PIPELINE_SELECT_3D: u32 = 0x0;
pub const PIPELINE_SELECT_MEDIA: u32 = 0x1;
pub const PIPELINE_SELECT_GPGPU: u32 = 0x2;

#[inline]
pub const fn pipeline_select(mode: u32) -> u32 {
    // 3/1/1/04h, single dword (no length field). Mask byte [15:8]=0x3 validates
    // the pipeline-select field [1:0]; also validate media-sampler dop-clock-gate bits.
    CMD_TYPE_GFXPIPE | PIPELINE_SINGLE_DW | (0x1 << 24) | (0x04 << 16) | (0x3 << 8) | (mode & 0x3)
}

// ---------------------------------------------------------------------------
// Opcode table for the 3D-pipeline commands built in later phases. Encoded as
// (pipeline, opcode, subopcode); the length is command-specific. Kept here so the
// PRM cross-reference lives in one place. (Sub-opcodes per PRM Vol 2a TOC.)
// ---------------------------------------------------------------------------
pub mod op3d {
    // pipeline = PIPELINE_3D (0x3<<27), opcode nibble, sub-opcode
    pub const STATE_BASE_ADDRESS: (u32, u32) = (0x0, 0x01); // 3/0/1/01h (pipeline COMMON)
    pub const STATE_SIP: (u32, u32) = (0x0, 0x02);

    pub const PIPE_CONTROL: (u32, u32) = (0x2, 0x00); // 3/3/2/00h in 3D pipeline
    pub const _3DPRIMITIVE: (u32, u32) = (0x3, 0x00); // 3/3/3/00h

    // 3DSTATE_* (pipeline 3D, opcode 0). Sub-opcodes:
    pub const _3DSTATE_VERTEX_BUFFERS: u32 = 0x08;
    pub const _3DSTATE_VERTEX_ELEMENTS: u32 = 0x09;
    pub const _3DSTATE_INDEX_BUFFER: u32 = 0x0A;
    pub const _3DSTATE_VF_STATISTICS: u32 = 0x0B;
    pub const _3DSTATE_VF: u32 = 0x0C;
    pub const _3DSTATE_MULTISAMPLE: u32 = 0x0D;
    pub const _3DSTATE_DRAWING_RECTANGLE: u32 = 0x00;
    pub const _3DSTATE_DEPTH_BUFFER: u32 = 0x05;
    pub const _3DSTATE_URB_VS: u32 = 0x30;
    pub const _3DSTATE_URB_HS: u32 = 0x31;
    pub const _3DSTATE_URB_DS: u32 = 0x32;
    pub const _3DSTATE_URB_GS: u32 = 0x33;
    pub const _3DSTATE_PUSH_CONSTANT_ALLOC_VS: u32 = 0x12;
    pub const _3DSTATE_PUSH_CONSTANT_ALLOC_PS: u32 = 0x16;
    pub const _3DSTATE_VS: u32 = 0x10;
    pub const _3DSTATE_GS: u32 = 0x11;
    pub const _3DSTATE_HS: u32 = 0x1B;
    pub const _3DSTATE_TE: u32 = 0x1C;
    pub const _3DSTATE_DS: u32 = 0x1D;
    pub const _3DSTATE_STREAMOUT: u32 = 0x1E;
    pub const _3DSTATE_CLIP: u32 = 0x12;
    pub const _3DSTATE_SF: u32 = 0x13;
    pub const _3DSTATE_WM: u32 = 0x14;
    pub const _3DSTATE_PS: u32 = 0x20;
    pub const _3DSTATE_PS_EXTRA: u32 = 0x4F;
    pub const _3DSTATE_PS_BLEND: u32 = 0x4D;
    pub const _3DSTATE_WM_DEPTH_STENCIL: u32 = 0x4E;
    pub const _3DSTATE_SBE: u32 = 0x1F;
    pub const _3DSTATE_RASTER: u32 = 0x50;
    pub const _3DSTATE_SAMPLE_MASK: u32 = 0x18;
    pub const _3DSTATE_BINDING_TABLE_POINTERS_PS: u32 = 0x2A;
    pub const _3DSTATE_SAMPLER_STATE_POINTERS_PS: u32 = 0x2F;
    pub const _3DSTATE_VIEWPORT_STATE_POINTERS_SF_CLIP: u32 = 0x21;
    pub const _3DSTATE_VIEWPORT_STATE_POINTERS_CC: u32 = 0x23;
    pub const _3DSTATE_CC_STATE_POINTERS: u32 = 0x0E;
    // NOTE: sub-opcodes above are transcribed from the PRM TOC and MUST be
    // re-verified byte-for-byte against Vol 2a / Mesa genxml before first submit
    // in Phase 5 — a wrong sub-opcode is an instant hang.
}

// ---------------------------------------------------------------------------
// PIPE_CONTROL (PRM Vol 2a "PIPE_CONTROL"). Type 3, pipeline 3D, opcode 2, sub 0.
// Gen8/Gen9 form is 6 dwords: header, flags(DW1), addr_lo, addr_hi, imm_lo, imm_hi.
// The known-good header for a 6-dword PIPE_CONTROL is 0x7A000004.
// DW1 flag bits (Gen9):
// ---------------------------------------------------------------------------
pub mod pc {
    pub const DEPTH_CACHE_FLUSH: u32 = 1 << 0;
    pub const STALL_AT_PIXEL_SCOREBOARD: u32 = 1 << 1;
    pub const STATE_CACHE_INVALIDATE: u32 = 1 << 2;
    pub const CONSTANT_CACHE_INVALIDATE: u32 = 1 << 3;
    pub const VF_CACHE_INVALIDATE: u32 = 1 << 4;
    pub const DC_FLUSH_ENABLE: u32 = 1 << 5;
    pub const PIPE_CONTROL_FLUSH: u32 = 1 << 7;
    pub const NOTIFY_ENABLE: u32 = 1 << 8;
    pub const INDIRECT_STATE_PTRS_DISABLE: u32 = 1 << 9;
    pub const TEXTURE_CACHE_INVALIDATE: u32 = 1 << 10;
    pub const INSTRUCTION_CACHE_INVALIDATE: u32 = 1 << 11;
    pub const RENDER_TARGET_CACHE_FLUSH: u32 = 1 << 12;
    pub const DEPTH_STALL: u32 = 1 << 13;
    // Post-sync operation, bits [15:14]: 01=write immediate, 10=write PS depth, 11=timestamp.
    pub const POST_SYNC_WRITE_IMM: u32 = 1 << 14;
    pub const POST_SYNC_WRITE_TIMESTAMP: u32 = 3 << 14;
    pub const GENERIC_MEDIA_STATE_CLEAR: u32 = 1 << 16;
    pub const TLB_INVALIDATE: u32 = 1 << 18;
    pub const CS_STALL: u32 = 1 << 20;
    pub const STORE_DATA_INDEX: u32 = 1 << 21;
    pub const LRI_POST_SYNC: u32 = 1 << 23;
    /// Destination Address Type: 1 = GGTT, 0 = PPGTT.
    pub const DEST_ADDRESS_GTT: u32 = 1 << 24;
    pub const FLUSH_LLC: u32 = 1 << 26;
}

/// Emit a Gen9 PIPE_CONTROL (6 dwords). `addr_gva` + `imm` are only meaningful when a
/// post-sync write bit is set in `flags`; pass 0 otherwise.
pub fn pipe_control(cs: &mut CmdStream, flags: u32, addr_gva: u32, imm: u64) {
    let (op, sub) = op3d::PIPE_CONTROL;
    cs.push(gfxpipe_header(PIPELINE_3D, op, sub, 6 - 2)); // 0x7A000004
    cs.push(flags);
    cs.push(addr_gva & !0x3); // address low (dword aligned)
    cs.push(0); // address high (GVA is 32-bit in our GGTT map)
    cs.push(imm as u32); // immediate low
    cs.push((imm >> 32) as u32); // immediate high
}

// ---------------------------------------------------------------------------
// CmdStream — a growable dword buffer for assembling a batch/ring command list.
// ---------------------------------------------------------------------------
pub struct CmdStream {
    pub dw: Vec<u32>,
}

impl CmdStream {
    pub fn new() -> Self {
        Self { dw: Vec::new() }
    }

    #[inline]
    pub fn push(&mut self, v: u32) -> &mut Self {
        self.dw.push(v);
        self
    }

    /// Push a 64-bit address as low/high dwords (GEN packets order low then high).
    #[inline]
    pub fn push_addr64(&mut self, addr: u64) -> &mut Self {
        self.dw.push((addr & 0xFFFF_FFFF) as u32);
        self.dw.push((addr >> 32) as u32);
        self
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.dw.len()
    }

    /// Pad to an even dword count with MI_NOOP (some engines require qword-aligned tails).
    pub fn pad_to_qword(&mut self) {
        if self.dw.len() % 2 != 0 {
            self.dw.push(MI_NOOP);
        }
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.dw
    }
}

impl Default for CmdStream {
    fn default() -> Self {
        Self::new()
    }
}

/// Dump a command stream as hex over serial for offline decoding in WSL with
/// `tools/gen9_decode.py`. Bracketed with `--CSDUMP-BEGIN/END--` markers so the
/// decoder can lift exactly this block out of a noisy boot log. 8 dwords/line.
///
/// This is our "GPU X-ray": we can't execute the Gen9 pipeline off-hardware, but
/// we can decode every command field-by-field against Mesa's genxml and catch a
/// malformed 3DSTATE that silently drops primitives (e.g. CL_inv=0).
pub fn hexdump_stream(label: &str, dwords: &[u32]) {
    crate::serial_println!("--CSDUMP-BEGIN {} ({} dw)--", label, dwords.len());
    let mut line = alloc::string::String::new();
    for (i, &dw) in dwords.iter().enumerate() {
        line.push_str(&alloc::format!("{:08x} ", dw));
        if (i + 1) % 8 == 0 {
            crate::serial_println!("{}", line);
            line.clear();
        }
    }
    if !line.is_empty() {
        crate::serial_println!("{}", line);
    }
    crate::serial_println!("--CSDUMP-END--");
}
