// src/drivers/gpu/intel/render/decode.rs
//
// On-device Gen9 command-stream decoder.
//
// The test machine is a separate bare-metal laptop with no serial capture — the
// only way to read the pipeline off it is a photo of the screen. Dumping 250 raw
// hex dwords is unreadable/un-OCR-able, so instead we decode the stream *on the
// device* into ~30 short human-readable lines like:
//
//   #12 3DSTATE_URB_VS           len=2  entries=64 alloc=0 start=4
//   #45 3DSTATE_CLIP             len=4  clip_en=1 vp_xy=1 guardband=1
//   #58 3DPRIMITIVE              len=7  topo=0x4 vcount=3 icount=1
//
// so a single photo captures the whole pipeline with nothing to mistranscribe.
//
// Command identity/length come from the Gen9 header encoding (see cmd.rs); the
// field extractions target exactly what we need to debug why primitives don't
// reach the clipper (CL_inv=0): URB allocation, VF topology, CLIP/SF/RASTER
// enables, VS URB output length, and the 3DPRIMITIVE counts.

#![allow(dead_code)]

use alloc::format;
use alloc::string::String;

/// Decode a full command stream and print one compact line per command.
pub fn decode_and_print(dw: &[u32]) {
    crate::serial_println!("--DECODE-BEGIN ({} dw)--", dw.len());
    let mut i = 0usize;
    let mut idx = 0u32;
    while i < dw.len() {
        let (name, mut len) = ident(dw[i]);
        if len == 0 || len > dw.len() - i {
            len = 1; // clamp: unknown/garbled length shouldn't run off the end
        }
        // Coalesce a run of identical MI_NOOP padding into one line (keeps the
        // photo readable — the tail has 16 of them).
        if name == "MI_NOOP" {
            let start = i;
            while i < dw.len() && dw[i] == 0 {
                i += 1;
            }
            crate::serial_println!("#{:<2} {:<28} x{}", idx, "MI_NOOP", i - start);
            idx += 1;
            continue;
        }
        let body = &dw[i..i + len];
        let fields = decode_fields(name, body);
        crate::serial_println!("#{:<2} {:<28} len={} {}", idx, name, len, fields);
        i += len;
        idx += 1;
    }
    crate::serial_println!("--DECODE-END--");
}

/// Identify a command from its DW0 header: returns (name, length-in-dwords).
fn ident(dw0: u32) -> (&'static str, usize) {
    let ctype = dw0 >> 29;
    if ctype == 0 {
        // MI command: opcode in bits [28:23].
        let op = (dw0 >> 23) & 0x3F;
        return match op {
            0x00 => ("MI_NOOP", 1),
            0x0A => ("MI_BATCH_BUFFER_END", 1),
            0x20 => ("MI_STORE_DATA_IMM", ((dw0 & 0x3FF) + 2) as usize),
            0x31 => ("MI_BATCH_BUFFER_START", ((dw0 & 0xFF) + 2) as usize),
            _ => ("MI_?", 1),
        };
    }
    if ctype == 3 {
        let subtype = (dw0 >> 27) & 0x3;
        let opcode = (dw0 >> 24) & 0x7;
        let subop = (dw0 >> 16) & 0xFF;
        let vlen = ((dw0 & 0xFF) + 2) as usize;
        if subtype == 1 {
            return ("PIPELINE_SELECT", 1);
        }
        if subtype == 0 && opcode == 1 && subop == 1 {
            return ("STATE_BASE_ADDRESS", vlen);
        }
        if subtype == 3 {
            return match opcode {
                2 => ("PIPE_CONTROL", vlen),
                3 => ("3DPRIMITIVE", vlen),
                1 => (name_op1(subop), vlen),
                0 => (name_3dstate(subop), vlen),
                _ => ("GFXPIPE_?", vlen),
            };
        }
    }
    ("UNKNOWN", 1)
}

/// 3D pipeline, opcode 1 (DRAWING_RECTANGLE / PUSH_CONSTANT_ALLOC_*).
fn name_op1(subop: u32) -> &'static str {
    match subop {
        0 => "3DSTATE_DRAWING_RECTANGLE",
        18 => "3DSTATE_PUSH_CONSTANT_ALLOC_VS",
        22 => "3DSTATE_PUSH_CONSTANT_ALLOC_PS",
        _ => "3DSTATE_op1_?",
    }
}

/// 3D pipeline, opcode 0 (the bulk of 3DSTATE_*). Sub-opcodes mirror pipeline::so.
fn name_3dstate(subop: u32) -> &'static str {
    match subop {
        16 => "3DSTATE_VS",
        17 => "3DSTATE_GS",
        18 => "3DSTATE_CLIP",
        19 => "3DSTATE_SF",
        20 => "3DSTATE_WM",
        24 => "3DSTATE_SAMPLE_MASK",
        27 => "3DSTATE_HS",
        28 => "3DSTATE_TE",
        29 => "3DSTATE_DS",
        30 => "3DSTATE_STREAMOUT",
        31 => "3DSTATE_SBE",
        32 => "3DSTATE_PS",
        5 => "3DSTATE_DEPTH_BUFFER",
        8 => "3DSTATE_VERTEX_BUFFERS",
        9 => "3DSTATE_VERTEX_ELEMENTS",
        10 => "3DSTATE_INDEX_BUFFER",
        12 => "3DSTATE_VF",
        13 => "3DSTATE_MULTISAMPLE",
        14 => "3DSTATE_CC_STATE_POINTERS",
        33 => "3DSTATE_VIEWPORT_SF_CLIP",
        35 => "3DSTATE_VIEWPORT_CC",
        36 => "3DSTATE_BLEND_STATE_POINTERS",
        42 => "3DSTATE_BINDING_TABLE_PTRS_PS",
        47 => "3DSTATE_SAMPLER_STATE_PTRS_PS",
        48 => "3DSTATE_URB_VS",
        49 => "3DSTATE_URB_HS",
        50 => "3DSTATE_URB_DS",
        51 => "3DSTATE_URB_GS",
        73 => "3DSTATE_VF_INSTANCING",
        75 => "3DSTATE_VF_TOPOLOGY",
        77 => "3DSTATE_PS_BLEND",
        78 => "3DSTATE_WM_DEPTH_STENCIL",
        79 => "3DSTATE_PS_EXTRA",
        80 => "3DSTATE_RASTER",
        _ => "3DSTATE_?",
    }
}

/// Extract the diagnostically-useful fields per command into a short string.
fn decode_fields(name: &str, b: &[u32]) -> String {
    let d = |i: usize| -> u32 {
        if i < b.len() {
            b[i]
        } else {
            0
        }
    };
    match name {
        "3DSTATE_URB_VS" | "3DSTATE_URB_HS" | "3DSTATE_URB_DS" | "3DSTATE_URB_GS" => {
            let v = d(1);
            format!("entries={} alloc={} start={}", v & 0xFFFF, (v >> 16) & 0x1FF, (v >> 25) & 0x7F)
        }
        "3DSTATE_VF_TOPOLOGY" => format!("topology={:#x}", d(1) & 0x3F),
        "3DSTATE_VERTEX_BUFFERS" => {
            format!("pitch={} idx={} addr={:#x} size={}", d(1) & 0xFFF, (d(1) >> 26) & 0x3F, d(2), d(4))
        }
        "3DSTATE_VERTEX_ELEMENTS" => {
            // DW1: valid[25], src format [24:16], VB index [31:26]. DW2: component selects.
            format!("valid={} fmt={} comps={:#010x}", (d(1) >> 25) & 1, (d(1) >> 16) & 0x1FF, d(2))
        }
        "3DSTATE_VS" => {
            let dw7 = d(7);
            let dw8 = d(8);
            format!(
                "kern_off={:#x} enable={} simd8={} vue_out_len={}",
                d(1), dw7 & 1, (dw7 >> 2) & 1, (dw8 >> 16) & 0x1FF
            )
        }
        "3DSTATE_CLIP" => {
            let dw2 = d(2);
            format!(
                "clip_en={} vp_xy={} guardband={}",
                (dw2 >> 31) & 1, (dw2 >> 28) & 1, (dw2 >> 26) & 1
            )
        }
        "3DSTATE_SF" => format!("vp_transform={}", (d(1) >> 1) & 1),
        "3DSTATE_RASTER" => {
            // cull mode [17:16], front winding [21], DX-multisample etc. Raw is enough.
            format!("dw1={:#010x} cull={}", d(1), (d(1) >> 16) & 0x3)
        }
        "3DSTATE_SBE" => {
            // num output attrs [26:22], vertex URB read len/offset force bits.
            format!("dw1={:#010x} num_attr={}", d(1), (d(1) >> 22) & 0x3F)
        }
        "3DSTATE_WM" => format!("dw1={:#010x}", d(1)),
        "3DSTATE_PS" => {
            let dw6 = d(6);
            format!(
                "kern_off={:#x} 8px={} 16px={} 32px={}",
                d(1), dw6 & 1, (dw6 >> 1) & 1, (dw6 >> 2) & 1
            )
        }
        "3DSTATE_PS_EXTRA" => format!("ps_valid={}", (d(1) >> 31) & 1),
        "3DSTATE_PS_BLEND" => format!("dw1={:#010x} has_wr_rt={}", d(1), (d(1) >> 30) & 1),
        "3DSTATE_MULTISAMPLE" => format!("dw1={:#010x}", d(1)),
        "3DSTATE_SAMPLE_MASK" => format!("mask={:#x}", d(1) & 0xFFFF),
        "3DSTATE_CC_STATE_POINTERS" => format!("ptr={:#x}", d(1)),
        "3DSTATE_BLEND_STATE_POINTERS" => format!("ptr={:#x}", d(1)),
        "3DSTATE_VIEWPORT_SF_CLIP" => format!("ptr={:#x}", d(1)),
        "3DSTATE_VIEWPORT_CC" => format!("ptr={:#x}", d(1)),
        "3DSTATE_BINDING_TABLE_PTRS_PS" => format!("ptr={:#x}", d(1)),
        "3DSTATE_DEPTH_BUFFER" => format!("surftype={}", (d(1) >> 29) & 0x7),
        "3DSTATE_DRAWING_RECTANGLE" => {
            format!("clip=({},{})", d(2) & 0xFFFF, (d(2) >> 16) & 0xFFFF)
        }
        "3DSTATE_PUSH_CONSTANT_ALLOC_VS" | "3DSTATE_PUSH_CONSTANT_ALLOC_PS" => {
            format!("offset={} size={}", (d(1) >> 16) & 0x1F, d(1) & 0x1F)
        }
        "3DPRIMITIVE" => {
            format!(
                "topo={:#x} vcount={} start_v={} icount={}",
                d(1) & 0x3F, d(2), d(3), d(4)
            )
        }
        "STATE_BASE_ADDRESS" => {
            // surf=DW4, dyn=DW6, instr=DW10 (see emit_state_base_address).
            format!(
                "surf={:#x} dyn={:#x} instr={:#x}",
                d(4) & !0xFFF, d(6) & !0xFFF, d(10) & !0xFFF
            )
        }
        "PIPE_CONTROL" => {
            let f = d(1);
            format!(
                "cs_stall={} rt_flush={} depth_flush={} dc={} post_wr={} addr={:#x} imm={:#x}",
                (f >> 20) & 1, (f >> 12) & 1, f & 1, (f >> 5) & 1, (f >> 14) & 1, d(2), d(4)
            )
        }
        "MI_STORE_DATA_IMM" => format!("addr={:#x} val={:#x}", d(1), d(3)),
        "PIPELINE_SELECT" => format!("mode={}", d(0) & 0x3),
        _ => String::new(),
    }
}
