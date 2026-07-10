// src/drivers/gpu/intel/cursor.rs
//
// Phase U1: the display controller's HARDWARE CURSOR plane (Gen9.5 / Comet Lake).
//
// The scanout engine composites a dedicated 64x64 ARGB cursor overlay in hardware every scan, wholly
// independent of the framebuffer. Once programmed, moving the pointer is a single MMIO write to CURPOS
// (driven straight from the PS/2 mouse IRQ here) — no window redraw, no BLT, no tearing. This replaces
// the compositor's per-frame software cursor for the common case; the software path stays as a fallback
// when the plane can't be brought up.
//
// EVERY register offset and bit below is SOURCED, not guessed (a wrong display-register write can wedge
// scanout). Cross-referenced against the KBL Gen9.5 PRM Vol 12 (Display) and the Linux i915 driver
// (i915_reg.h, display/intel_cursor.c, display/skl_watermark.c/.h):
//
//   Per-pipe registers (pipe_base = 0x70000 + pipe*0x1000; pipe A/B/C = 0x70000/0x71000/0x72000):
//     CURCNTR      pipe_base+0x080   control: mode field, mask 0x27
//     CURBASE      pipe_base+0x084   GGTT byte address of the bitmap; WRITING IT ARMS the atomic latch
//     CURPOS       pipe_base+0x088   X[14:0] (sign b15), Y[30:16] (sign b31)
//     CUR_WM(lvl)  pipe_base+0x140 + 4*lvl   PLANE_WM_EN(1<<31) | lines[26:14] | blocks[11:0]
//     CUR_BUF_CFG  pipe_base+0x17C   DBUF alloc: end-1[?:16] | start[?:0]
//     PLANE_BUF_CFG_1 pipe_base+0x27C   primary plane's live DBUF range (READ only, never touch)
//     PLANE_CTL_1  pipe_base+0x180   primary plane control (bit31 = plane enable) — used to find the pipe
//
//   CURCNTR mode: MCURSOR_MODE_64_ARGB_AX = 0x20 (ARGB) | 0x07 (64x64) = 0x27.
//   Write order (i915, DISPLAY_VER>=9): CURCNTR -> CURPOS -> CURBASE (last, arms). A lone CURPOS can be
//   left unarmed, so move_to() always follows CURPOS with a CURBASE re-write.
//   Gen9 REQUIRES cursor watermark + DBUF or the plane never fetches (i915 skl_write_cursor_wm on ver>=9).
//   PRM basic allocation: 32 blocks for the cursor (64*64*4 = 16 KB / 512 B-per-block = 32).

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::IntelGpuDriver;

/// GGTT address for the 64x64 ARGB cursor bitmap (16 KB / 4 pages). Chosen HIGH and clear of the whole
/// GVA map: the 3D engine owns 0x10-0x1E and 0x40 (SSAA RT), the GL window backbuffer sits at
/// 0x5000_0000 (~1.2 MB). 0x5100_0000 clears all of them with wide margin. (Cross-checked against
/// render/mod.rs GVA_* — a collision here is exactly the class of bug that ate the GL bring-up.)
pub const GVA_HW_CURSOR: u32 = 0x5100_0000;

const CURSOR_W: u32 = 64;
const CURSOR_H: u32 = 64;
const CURSOR_BYTES: usize = (CURSOR_W * CURSOR_H * 4) as usize; // 16384

// Per-pipe register offsets (added to pipe_base).
const OFF_CURCNTR: u32 = 0x080;
const OFF_CURBASE: u32 = 0x084;
const OFF_CURPOS: u32 = 0x088;
const OFF_CUR_WM0: u32 = 0x140;
const OFF_CUR_BUF_CFG: u32 = 0x17C;
const OFF_PLANE_CTL_1: u32 = 0x180;
const OFF_PLANE_BUF_CFG_1: u32 = 0x27C;

// CURCNTR: 64x64 ARGB, no rotation / gamma / CSC (pipe is implied by the register block on Gen9).
const MCURSOR_MODE_64_ARGB_AX: u32 = 0x27;

// PLANE_WM level format.
const PLANE_WM_EN: u32 = 1 << 31;
// A DBUF block is 512 bytes; the PRM "basic allocation" reserves 32 blocks for the cursor.
const CURSOR_DBUF_BLOCKS: u32 = 32;

/// Lockless global cursor state. Populated ONCE by init(); the mouse IRQ only loads these to move the
/// cursor, so the IRQ path takes NO locks (no INTEL_GPU / render-engine mutex) and can never deadlock.
static MMIO_BASE: AtomicU64 = AtomicU64::new(0);
static PIPE_BASE: AtomicU32 = AtomicU32::new(0);
static CURSOR_CPU: AtomicU64 = AtomicU64::new(0); // CPU vaddr of the bitmap page (for set_image)
static ENABLED: AtomicBool = AtomicBool::new(false);

#[inline]
unsafe fn wr(mmio: u64, off: u32, val: u32) {
    core::ptr::write_volatile((mmio + off as u64) as *mut u32, val);
}
#[inline]
unsafe fn rd(mmio: u64, off: u32) -> u32 {
    core::ptr::read_volatile((mmio + off as u64) as *const u32)
}

/// Is the hardware cursor live? The compositor uses this (via syscall) to decide whether to keep
/// drawing the software cursor.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Find the pipe currently scanning out: the first pipe whose primary plane (PLANE_CTL_1) has the
/// enable bit (31) set. Mirrors the scan in mod.rs::test_screen_blit. Returns the pipe_base, or None.
unsafe fn find_active_pipe(mmio: u64) -> Option<u32> {
    for &pipe_base in &[0x70000u32, 0x71000, 0x72000] {
        let ctl = rd(mmio, pipe_base + OFF_PLANE_CTL_1);
        if ctl & (1 << 31) != 0 {
            return Some(pipe_base);
        }
    }
    None
}

/// Bring up the hardware cursor plane on the active pipe. Returns true on success; on any failure the
/// caller keeps the software cursor (this never disables the software path itself). Idempotent-ish:
/// a second call just reprograms. Locks INTEL_GPU internally (NOT called from IRQ context).
pub fn init() -> bool {
    let (mmio, pipe_base) = {
        let guard = super::INTEL_GPU.lock();
        let gpu = match guard.as_ref() {
            Some(g) => g,
            None => {
                crate::serial_println!("[HWCURSOR] no Intel GPU; keeping software cursor");
                return false;
            }
        };
        let mmio = gpu.mmio_base;
        let pipe_base = unsafe {
            match find_active_pipe(mmio) {
                Some(p) => p,
                None => {
                    crate::serial_println!("[HWCURSOR] no active display pipe found; keeping software cursor");
                    return false;
                }
            }
        };

        // Allocate 4 contiguous GGTT pages for the bitmap and map them at GVA_HW_CURSOR.
        let pages = (CURSOR_BYTES + 4095) / 4096; // 4
        let frame = match crate::memory::allocate_contiguous(pages, 4096, true) {
            Some(f) => f,
            None => {
                crate::serial_println!("[HWCURSOR] page alloc failed; keeping software cursor");
                return false;
            }
        };
        let phys = frame.start_address().as_u64();
        let cpu = crate::memory::phys_to_virt(phys).unwrap();
        unsafe {
            core::ptr::write_bytes(cpu as *mut u8, 0, pages * 4096); // start fully transparent
            for i in 0..pages {
                gpu.map_ggtt_page((GVA_HW_CURSOR / 4096) + i as u32, phys + (i as u64 * 4096), true);
            }
        }
        CURSOR_CPU.store(cpu, Ordering::SeqCst);

        // --- DBUF allocation: place the cursor's 32 blocks ABOVE the primary plane's live range so we
        // never shrink/overlap the plane currently feeding the panel (that would underrun scanout). ---
        unsafe {
            let plane_buf = rd(mmio, pipe_base + OFF_PLANE_BUF_CFG_1);
            // skl_ddb_entry format: start in low bits, (end-1) in the high half. Masks are wide (>=10
            // bits); 0xFFFF is safe for KBL's ~892-block DBUF.
            let plane_start = plane_buf & 0xFFFF;
            let plane_end_incl = (plane_buf >> 16) & 0xFFFF; // = end-1
            let plane_end = if plane_buf == 0 { 0 } else { plane_end_incl + 1 };

            // Reserve [cur_start, cur_end) just past the primary plane.
            let cur_start = plane_end;
            let cur_end = cur_start + CURSOR_DBUF_BLOCKS;
            let buf_cfg = ((cur_end - 1) << 16) | cur_start;
            wr(mmio, pipe_base + OFF_CUR_BUF_CFG, buf_cfg);

            // Level-0 watermark: enabled, small fixed budget (blocks <= allocation), 2 lines. Higher
            // levels left disabled (bit31 = 0) so low-power watermarks stay off — matches the PRM
            // pre-boot guidance. A 64-wide cursor needs very few blocks; 8 is comfortably safe.
            wr(mmio, pipe_base + OFF_CUR_WM0, PLANE_WM_EN | (2 << 14) | 8);
            // Zero the remaining WM levels (1..7) for this cursor plane.
            for lvl in 1..8u32 {
                wr(mmio, pipe_base + OFF_CUR_WM0 + 4 * lvl, 0);
            }

            crate::serial_println!(
                "[HWCURSOR] pipe_base={:#x} plane_buf={:#010x} (start={} end={}) cursor_dbuf=[{}, {}) gva={:#x}",
                pipe_base, plane_buf, plane_start, plane_end, cur_start, cur_end, GVA_HW_CURSOR
            );
        }
        (mmio, pipe_base)
    };

    MMIO_BASE.store(mmio, Ordering::SeqCst);
    PIPE_BASE.store(pipe_base, Ordering::SeqCst);

    // Program the plane: CURCNTR (mode) -> CURPOS (center) -> CURBASE (arms). Start centered-ish.
    unsafe {
        wr(mmio, pipe_base + OFF_CURCNTR, MCURSOR_MODE_64_ARGB_AX);
        wr(mmio, pipe_base + OFF_CURPOS, 0);
        wr(mmio, pipe_base + OFF_CURBASE, GVA_HW_CURSOR);
    }

    ENABLED.store(true, Ordering::SeqCst);
    crate::serial_println!("[HWCURSOR] hardware cursor enabled (CURCNTR=0x27, 64x64 ARGB)");
    true
}

/// Upload a 64x64 ARGB (0xAARRGGBB) bitmap into the cursor page and re-arm. If the cursor isn't
/// initialized this is a no-op returning false. `argb` must be exactly 64*64 pixels.
pub fn set_image(argb: &[u32]) -> bool {
    if !ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    if argb.len() < (CURSOR_W * CURSOR_H) as usize {
        return false;
    }
    let cpu = CURSOR_CPU.load(Ordering::SeqCst);
    if cpu == 0 {
        return false;
    }
    unsafe {
        let dst = cpu as *mut u32;
        for i in 0..(CURSOR_W * CURSOR_H) as usize {
            core::ptr::write_volatile(dst.add(i), argb[i]);
        }
        // Flush the bitmap out of CPU cache so the display engine sees it.
        let mut b = 0usize;
        while b < CURSOR_BYTES {
            core::arch::asm!("clflush [{}]", in(reg) (cpu as usize + b), options(nostack, preserves_flags));
            b += 64;
        }
        core::arch::asm!("mfence", options(nostack, preserves_flags));
        // Re-arm so the new pixels latch on the next vblank.
        let mmio = MMIO_BASE.load(Ordering::Relaxed);
        let pipe = PIPE_BASE.load(Ordering::Relaxed);
        wr(mmio, pipe + OFF_CURBASE, GVA_HW_CURSOR);
    }
    true
}

/// Move the cursor to (x, y) in pixels. Lockless — safe to call from the mouse IRQ. Writes CURPOS then
/// re-arms via CURBASE (a lone CURPOS may not latch). No-op if the cursor isn't enabled.
#[inline]
pub fn move_to(x: usize, y: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mmio = MMIO_BASE.load(Ordering::Relaxed);
    let pipe = PIPE_BASE.load(Ordering::Relaxed);
    if mmio == 0 {
        return;
    }
    // On-screen coordinates are non-negative: X in [14:0], Y in [30:16], sign bits clear.
    let pos = (((y as u32) & 0x7FFF) << 16) | ((x as u32) & 0x7FFF);
    unsafe {
        wr(mmio, pipe + OFF_CURPOS, pos);
        wr(mmio, pipe + OFF_CURBASE, GVA_HW_CURSOR);
    }
}

/// Disable the cursor plane (CURCNTR mode -> DISABLE, then arm). Falls back to software cursor.
pub fn disable() {
    if !ENABLED.swap(false, Ordering::SeqCst) {
        return;
    }
    let mmio = MMIO_BASE.load(Ordering::Relaxed);
    let pipe = PIPE_BASE.load(Ordering::Relaxed);
    if mmio == 0 {
        return;
    }
    unsafe {
        wr(mmio, pipe + OFF_CURCNTR, 0);
        wr(mmio, pipe + OFF_CURBASE, GVA_HW_CURSOR);
    }
}
