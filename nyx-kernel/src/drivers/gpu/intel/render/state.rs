// src/drivers/gpu/intel/render/state.rs
//
// 3D pipeline state objects (Phase 4). These are packed data structures the GPU
// reads via pointers programmed relative to STATE_BASE_ADDRESS.
//
// To build here (byte layouts from PRM Vol 5 "Shared Functions" + Mesa `isl`):
//   - RENDER_SURFACE_STATE: the render target = existing linear B8G8R8A8_UNORM
//     backbuffer (SURFTYPE_2D, LINEAR, 64B-aligned base, pitch % 64 == 0). Gen9
//     allows linear RTs, so no tiling is needed to present.
//   - BINDING_TABLE_STATE: array of 256B-granular offsets to surface states;
//     entry[0] = the RT (entry[1] = texture in Phase 7).
//   - BLEND_STATE / 3DSTATE_PS_BLEND: blend disabled, RGBA write mask.
//   - COLOR_CALC_STATE.
//   - SF_CLIP_VIEWPORT + CC_VIEWPORT.
//   - SAMPLER_STATE (dummy for Phase 5; real for Phase 7).
//   - Depth state: 3DSTATE_DEPTH_BUFFER SURFTYPE_NULL (Phase 5), real depth surface
//     (Phase 6).
//
// Surface format codes (PRM Vol 2b enumerations): R8G8B8A8_UNORM = 0x0C7,
// B8G8R8A8_UNORM used by the BGRA framebuffer.
//
// Placeholder at Phase 0.

#![allow(dead_code)]

/// Common Gen9 surface format codes (PRM Vol 2b "SURFACE_FORMAT").
pub const SURFACE_FORMAT_R8G8B8A8_UNORM: u32 = 0x0C7;
pub const SURFACE_FORMAT_B8G8R8A8_UNORM: u32 = 0x0C0;

/// Surface types (RENDER_SURFACE_STATE / 3DSTATE_DEPTH_BUFFER).
pub const SURFTYPE_1D: u32 = 0x0;
pub const SURFTYPE_2D: u32 = 0x1;
pub const SURFTYPE_3D: u32 = 0x2;
pub const SURFTYPE_CUBE: u32 = 0x3;
pub const SURFTYPE_NULL: u32 = 0x7;

/// Tile modes.
pub const TILEMODE_LINEAR: u32 = 0x0;
pub const TILEMODE_WMAJOR: u32 = 0x1;
pub const TILEMODE_XMAJOR: u32 = 0x2;
pub const TILEMODE_YMAJOR: u32 = 0x3;

use super::RenderError;

/// A GGTT-mapped scratch region for GPU-readable state objects (surface state, binding
/// tables, dynamic state, viewports, vertex buffers). Bump-allocates aligned sub-blocks
/// and hands back both the CPU pointer (to fill) and the GVA offset (to point the GPU
/// at). Layout-independent; the packed object encoders write through the CPU pointers.
pub struct StateBuffer {
    pub virt: u64,
    pub gva: u32,
    pub used: usize,
    pub capacity: usize,
}

impl StateBuffer {
    /// Allocate `pages` contiguous 4KB pages, zero them, and GGTT-map at `gva`.
    pub unsafe fn new(mmio_base: u64, gva: u32, pages: usize) -> Result<Self, RenderError> {
        let frame = crate::memory::allocate_contiguous(pages, 4096, true)
            .ok_or(RenderError::NotInitialized)?;
        let phys = frame.start_address().as_u64();
        let virt = crate::memory::phys_to_virt(phys).ok_or(RenderError::NotInitialized)?;
        core::ptr::write_bytes(virt as *mut u8, 0, pages * 4096);
        for i in 0..pages {
            let gtt_offset = 0x800000 + (((gva / 4096) as u64 + i as u64) * 8);
            let gtt_ptr = (mmio_base + gtt_offset) as *mut u64;
            let pte = ((phys + (i as u64 * 4096)) & 0xFFFF_FFFF_FFFF_F000) | 0x07;
            core::ptr::write_volatile(gtt_ptr, pte);
            let _ = core::ptr::read_volatile(gtt_ptr);
        }
        Ok(Self { virt, gva, used: 0, capacity: pages * 4096 })
    }

    /// Reserve `dwords*4` bytes aligned to `align`. Returns (cpu_ptr, gva) of the block.
    pub unsafe fn alloc(&mut self, dwords: usize, align: usize) -> Result<(*mut u32, u32), RenderError> {
        let off = (self.used + align - 1) & !(align - 1);
        let bytes = dwords * 4;
        if off + bytes > self.capacity {
            return Err(RenderError::NotInitialized);
        }
        self.used = off + bytes;
        let cpu = (self.virt as *mut u8).add(off) as *mut u32;
        Ok((cpu, self.gva + off as u32))
    }

    /// Write a slice of dwords into a freshly-allocated aligned block; return its GVA.
    pub unsafe fn write(&mut self, data: &[u32], align: usize) -> Result<u32, RenderError> {
        let (cpu, gva) = self.alloc(data.len(), align)?;
        for (i, &dw) in data.iter().enumerate() {
            cpu.add(i).write_volatile(dw);
        }
        self.flush_range(gva, data.len() * 4);
        Ok(gva)
    }

    /// clflush a written range (by GVA offset within this buffer) so the GPU sees it.
    pub unsafe fn flush_range(&self, gva: u32, bytes: usize) {
        let off = (gva - self.gva) as usize;
        let mut p = off & !63;
        while p < off + bytes {
            core::arch::asm!("clflush [{}]", in(reg) (self.virt as usize + p),
                options(nostack, preserves_flags));
            p += 64;
        }
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}

// ===========================================================================
// Packed Gen9 state-object builders. Bit layouts from Mesa genxml gen9.xml
// (RENDER_SURFACE_STATE / BLEND_STATE / COLOR_CALC_STATE / *_VIEWPORT /
// BINDING_TABLE_STATE) and PRM Vol 2a (STATE_BASE_ADDRESS). Every object is
// validated end-to-end at the Phase 5 first-triangle gate.
// ===========================================================================

/// Surface Horizontal/Vertical alignment enums (HALIGN_4/VALIGN_4 = 1).
pub const HALIGN_4: u32 = 1;
pub const VALIGN_4: u32 = 1;

/// Shader Channel Select values.
pub const SCS_RED: u32 = 4;
pub const SCS_GREEN: u32 = 5;
pub const SCS_BLUE: u32 = 6;
pub const SCS_ALPHA: u32 = 7;

/// MOCS index for state objects. 0 = follow-PTE / uncached; safe because we clflush
/// all state to RAM before submit. (Revisit for perf once drawing works.)
pub const MOCS: u32 = 0;

/// Build a RENDER_SURFACE_STATE (16 dwords) for a 2D render target/texture.
/// `base_gva` is the surface's GGTT byte address; `pitch` in bytes; `tile_mode` is one of
/// TILEMODE_LINEAR / TILEMODE_YMAJOR / TILEMODE_XMAJOR. For a Y-tiled (YMAJOR) surface the
/// pitch must be a 128-byte multiple and the base 4KB-aligned (VALIGN_4 is already set, which
/// the PRM requires for all tiled-Y render targets).
pub fn render_surface_state_2d(
    format: u32,
    width: u32,
    height: u32,
    pitch: u32,
    base_gva: u32,
    tile_mode: u32,
) -> [u32; 16] {
    let mut s = [0u32; 16];
    // DW0: type[31:29]=2D, format[27:18], HAlign[15:14], VAlign[17:16], TileMode[13:12].
    s[0] = (SURFTYPE_2D << 29)
        | ((format & 0x3FF) << 18)
        | (VALIGN_4 << 16)
        | (HALIGN_4 << 14)
        | ((tile_mode & 0x3) << 12);
    // DW1: MOCS[62:56] -> bits [30:24] of DW1.
    s[1] = (MOCS & 0x7F) << 24;
    // DW2: Width-1 [13:0], Height-1 [29:16].
    s[2] = ((width - 1) & 0x3FFF) | (((height - 1) & 0x3FFF) << 16);
    // DW3: Pitch-1 [17:0]. (Depth[31:21]=0 for a single 2D surface.)
    s[3] = (pitch - 1) & 0x3FFFF;
    // DW7: Shader Channel Select A[242:240] B[245:243] G[248:246] R[251:249]
    //      -> DW7 bits [18:16],[21:19],[24:22],[27:25]. Identity RGBA.
    s[7] = (SCS_ALPHA << 16) | (SCS_BLUE << 19) | (SCS_GREEN << 22) | (SCS_RED << 25);
    // DW8/9: Surface Base Address (64-bit; GVA is 32-bit in our GGTT map).
    s[8] = base_gva;
    s[9] = 0;
    s
}

/// A single-entry BINDING_TABLE_STATE. Entry = surface-state offset from Surface State
/// Base (64B-aligned) placed in bits [31:6].
pub fn binding_table_1(surface_state_offset: u32) -> [u32; 1] {
    [surface_state_offset & !0x3F]
}

/// A two-entry BINDING_TABLE_STATE (Boot C): entry[0] = render target, entry[1] = texture.
/// Each entry is a surface-state offset from Surface State Base in bits [31:6]. The PS RT
/// write targets BTI 0; the sampler `sample` message targets BTI 1.
pub fn binding_table_2(rt_surface_offset: u32, tex_surface_offset: u32) -> [u32; 2] {
    [rt_surface_offset & !0x3F, tex_surface_offset & !0x3F]
}

/// A three-entry BINDING_TABLE_STATE (U4 Boot 3 rounded corners): entry[0] = render target,
/// entry[1] = window texture, entry[2] = corner alpha-mask. The rounded-corner PS RT-writes to
/// BTI 0, samples the window at BTI 1, and samples the mask at BTI 2. Each entry is a surface-state
/// offset from Surface State Base in bits [31:6] (64B-aligned).
pub fn binding_table_3(rt_surface_offset: u32, tex_surface_offset: u32, mask_surface_offset: u32) -> [u32; 3] {
    [rt_surface_offset & !0x3F, tex_surface_offset & !0x3F, mask_surface_offset & !0x3F]
}

/// SAMPLER_STATE (4 dwords, PRM Vol5 / genxml). Boot C sampler: NEAREST min/mag (crisp
/// checkerboard, no blur), no mipmapping (single level), CLAMP address modes on all axes,
/// normalized [0,1] coordinates, sampler enabled. Field bits (genxml SAMPLER_STATE):
///   DW0: Min Mode Filter [16:14]=NEAREST(0), Mag Mode Filter [19:17]=NEAREST(0),
///        Mip Mode Filter [21:20]=NONE(0), Sampler Disable [31]=0  -> all zero.
///   DW1: Min LOD [63:52]=0, Max LOD [51:40]=0, Border Color Pointer [87:70]=0 -> zero
///        (Min=Max LOD 0 pins sampling to the base level regardless of computed LOD).
///   DW2: TCZ [98:96], TCY [101:99], TCX [102:104] Address Control = CLAMP(2); within DW2
///        that is bits [2:0],[5:3],[8:6]. Non-normalized Coordinate Enable [106]=0.
///   DW3: 0.
pub fn sampler_state_nearest_clamp() -> [u32; 4] {
    const CLAMP: u32 = 2; // TCM_CLAMP
    let dw2 = CLAMP | (CLAMP << 3) | (CLAMP << 6); // TCZ | TCY | TCX
    [0, 0, dw2, 0]
}

/// SAMPLER_STATE with BILINEAR (MAPFILTER_LINEAR) Min/Mag filtering + CLAMP addressing. Used by
/// the MSAA-track Boot-3 SSAA resolve: the resolve quad samples the 2x-larger scene RT so each
/// backbuffer pixel's coordinate lands EXACTLY on a 2x2 texel-block center — bilinear then averages
/// the 4 texels 0.25 each, giving a free, exact box-filter downsample (no shader arithmetic, which
/// the EU encoder can't emit). Field layout (mesa-gen9.xml SAMPLER_STATE):
///   DW0: Min Mode Filter [16:14] = MAPFILTER_LINEAR(1); Mag Mode Filter [19:17] = LINEAR(1);
///        Mip Mode Filter [21:20] = MIPFILTER_NONE(0) so bilinear reads the base level only.
///   DW2: TCX/TCY/TCZ Address Control = CLAMP(2) (same as the nearest sampler).
pub fn sampler_state_linear_clamp() -> [u32; 4] {
    const CLAMP: u32 = 2; // TCM_CLAMP
    const LINEAR: u32 = 1; // MAPFILTER_LINEAR
    let dw0 = (LINEAR << 14) | (LINEAR << 17); // Min[16:14] | Mag[19:17]; Mip[21:20]=NONE(0)
    let dw2 = CLAMP | (CLAMP << 3) | (CLAMP << 6); // TCZ | TCY | TCX
    [dw0, 0, dw2, 0]
}

/// Fill a `w`x`h` B8G8R8A8_UNORM checkerboard into `cpu` (row pitch `pitch` bytes), with
/// `square` texels per checker cell. Two colors so UV bugs are obvious: white vs. blue.
/// B8G8R8A8 in memory is [B,G,R,A]; as a little-endian u32 that is A<<24 | R<<16 | G<<8 | B.
pub unsafe fn fill_checkerboard(cpu: *mut u8, w: u32, h: u32, pitch: u32, square: u32) {
    const WHITE: u32 = 0xFFFF_FFFF; // A=FF R=FF G=FF B=FF
    const BLUE: u32 = 0xFF00_00FF; // A=FF R=00 G=00 B=FF
    let mut y = 0u32;
    while y < h {
        let mut x = 0u32;
        while x < w {
            let cell = ((x / square) + (y / square)) & 1;
            let texel = if cell == 0 { WHITE } else { BLUE };
            let p = cpu.add((y * pitch + x * 4) as usize) as *mut u32;
            p.write_volatile(texel);
            x += 1;
        }
        y += 1;
    }
}

/// BLEND_STATE (1 dw header) + one BLEND_STATE_ENTRY (2 dw): blend disabled, write RGBA.
/// Matches blorp: Pre/Post-blend color clamp enabled, clamp range = RT format — without
/// this the shader's float output isn't converted into the render-target format.
pub fn blend_state() -> [u32; 3] {
    // DW0: header (no alpha-to-coverage / independent blend).
    // Entry DW0 (bits 32..63): write-disables=0 (all channels), blend disabled.
    // Entry DW1 (bits 64..95): PostBlendColorClampEnable(bit0) | PreBlendColorClampEnable(bit1)
    //                          | ColorClampRange=RTFORMAT(2) at bits[3:2].
    [0, 0, (1 << 0) | (1 << 1) | (2 << 2)]
}

/// BLEND_STATE + one BLEND_STATE_ENTRY configured for SOURCE-OVER alpha blending (U2):
///   out.rgb = src.rgb * src.a + dst.rgb * (1 - src.a)      [SRC_ALPHA / INV_SRC_ALPHA, ADD]
///   out.a   = src.a   * 1     + dst.a   * (1 - src.a)      [ONE       / INV_SRC_ALPHA, ADD]
/// Only the entry's DW0 (blend enable + factors) differs from `blend_state()`; the clamp DW1 is
/// identical. All field bit positions + blend-factor enum values are sourced from the SKL genxml
/// (BLEND_STATE_ENTRY: Color Buffer Blend Enable=31, Source Blend Factor[30:26], Destination Blend
/// Factor[25:21], Color Blend Function[20:18], Source Alpha Blend Factor[17:13], Destination Alpha
/// Blend Factor[12:8], Alpha Blend Function[7:5]; SRC_ALPHA=3, INV_SRC_ALPHA=19, ONE=1, ADD=0).
pub fn blend_state_src_over() -> [u32; 3] {
    // Entry DW0:
    //   (1<<31) enable | (3<<26) srcBF=SRC_ALPHA | (19<<21) dstBF=INV_SRC_ALPHA
    //   | (0<<18) colorFn=ADD | (1<<13) srcABF=ONE | (19<<8) dstABF=INV_SRC_ALPHA | (0<<5) alphaFn=ADD
    //   = 0x8E60_3300.
    let entry_dw0: u32 = (1 << 31) | (3 << 26) | (19 << 21) | (1 << 13) | (19 << 8);
    [0, entry_dw0, (1 << 0) | (1 << 1) | (2 << 2)]
}

/// COLOR_CALC_STATE (6 dwords): defaults (blend constant color 0, alpha ref 0).
pub fn color_calc_state() -> [u32; 6] {
    [0; 6]
}

/// CC_VIEWPORT (2 dwords): min/max depth as floats.
pub fn cc_viewport(min_depth: f32, max_depth: f32) -> [u32; 2] {
    [min_depth.to_bits(), max_depth.to_bits()]
}

/// SF_CLIP_VIEWPORT (16 dwords): NDC->screen matrix + a wide guardband + viewport extent.
pub fn sf_clip_viewport(width: u32, height: u32) -> [u32; 16] {
    let w = width as f32;
    let h = height as f32;
    let mut v = [0u32; 16];
    // Viewport matrix: NDC [-1,1] -> screen [0,W]x[0,H], y flipped (framebuffer origin top-left).
    v[0] = (w * 0.5).to_bits(); // m00
    v[1] = (-h * 0.5).to_bits(); // m11
    v[2] = (1.0f32).to_bits(); // m22 (depth range far-near = 1)
    v[3] = (w * 0.5).to_bits(); // m30 (x center)
    v[4] = (h * 0.5).to_bits(); // m31 (y center)
    v[5] = (0.0f32).to_bits(); // m32 (near)
    // Guardband (NDC units) — wide so setup doesn't over-clip. DW8..11.
    v[8] = (-16.0f32).to_bits(); // X Min Clip Guardband
    v[9] = (16.0f32).to_bits(); // X Max Clip Guardband
    v[10] = (-16.0f32).to_bits(); // Y Min Clip Guardband
    v[11] = (16.0f32).to_bits(); // Y Max Clip Guardband
    // Viewport screen extent. DW12..15.
    v[12] = (0.0f32).to_bits(); // X Min ViewPort
    v[13] = (w - 1.0).to_bits(); // X Max ViewPort
    v[14] = (0.0f32).to_bits(); // Y Min ViewPort
    v[15] = (h - 1.0).to_bits(); // Y Max ViewPort
    v
}

/// GVAs of all placed state objects, produced by [`setup_render_state`] and consumed by
/// the Phase 5 pipeline assembler. Offsets are relative to the respective State Base.
pub struct RenderState {
    pub surface_state_base: u32, // == GVA_SURFACE_STATE (Surface State Base Address)
    pub dynamic_state_base: u32, // == GVA_DYNAMIC_STATE (Dynamic State Base Address)
    pub rt_surface_off: u32,     // RENDER_SURFACE_STATE offset from surface base
    pub binding_table_off: u32,  // BINDING_TABLE_STATE offset from surface base
    pub blend_off: u32,          // BLEND_STATE offset from dynamic base (opaque, blend disabled)
    pub blend_src_over_off: u32,  // BLEND_STATE offset for SRC_ALPHA/INV_SRC_ALPHA src-over (U2)
    pub cc_off: u32,             // COLOR_CALC_STATE offset from dynamic base
    pub cc_viewport_off: u32,    // CC_VIEWPORT offset from dynamic base
    pub sf_clip_viewport_off: u32, // SF_CLIP_VIEWPORT offset from dynamic base
}

/// Phase 4: allocate the surface-state and dynamic-state buffers, build all state
/// objects for a solid-color triangle into the linear RT at `rt_gva`, and return their
/// GVAs. `surf_buf`/`dyn_buf` are retained by the caller (mapped for Phase 5).
pub unsafe fn setup_render_state(
    surf_buf: &mut StateBuffer,
    dyn_buf: &mut StateBuffer,
    rt_gva: u32,
    width: u32,
    height: u32,
    pitch: u32,
    rt_tile_mode: u32,
) -> Result<RenderState, RenderError> {
    // --- Surface State Base contents: RENDER_SURFACE_STATE + BINDING_TABLE ---
    let ss = render_surface_state_2d(SURFACE_FORMAT_B8G8R8A8_UNORM, width, height, pitch, rt_gva, rt_tile_mode);
    let rt_surface_gva = surf_buf.write(&ss, 64)?; // surface state: 64B aligned
    let rt_surface_off = rt_surface_gva - surf_buf.gva;

    let bt = binding_table_1(rt_surface_off);
    let bt_gva = surf_buf.write(&bt, 64)?; // binding table: 32/64B aligned
    let binding_table_off = bt_gva - surf_buf.gva;

    // --- Dynamic State Base contents: BLEND / COLOR_CALC / viewports ---
    let blend_gva = dyn_buf.write(&blend_state(), 64)?;
    let blend_src_over_gva = dyn_buf.write(&blend_state_src_over(), 64)?;
    let cc_gva = dyn_buf.write(&color_calc_state(), 64)?;
    let ccv_gva = dyn_buf.write(&cc_viewport(0.0, 1.0), 32)?;
    let sfv_gva = dyn_buf.write(&sf_clip_viewport(width, height), 64)?;

    let rs = RenderState {
        surface_state_base: surf_buf.gva,
        dynamic_state_base: dyn_buf.gva,
        rt_surface_off,
        binding_table_off,
        blend_off: blend_gva - dyn_buf.gva,
        blend_src_over_off: blend_src_over_gva - dyn_buf.gva,
        cc_off: cc_gva - dyn_buf.gva,
        cc_viewport_off: ccv_gva - dyn_buf.gva,
        sf_clip_viewport_off: sfv_gva - dyn_buf.gva,
    };

    crate::serial_println!(
        "[STATE] surf_base={:#x} rt_surf_off={:#x} bt_off={:#x} | dyn_base={:#x} blend={:#x} cc={:#x} ccv={:#x} sfv={:#x}",
        rs.surface_state_base, rs.rt_surface_off, rs.binding_table_off,
        rs.dynamic_state_base, rs.blend_off, rs.cc_off, rs.cc_viewport_off, rs.sf_clip_viewport_off
    );
    Ok(rs)
}
