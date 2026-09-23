// src/drivers/gpu/intel/render/ring.rs
//
// RCS legacy ring-buffer submission (Phase 1).
//
// Mirrors the proven BLT ring in `intel/mod.rs::submit_command` (a 1024-dword
// circular buffer with HEAD/TAIL polling, per-dword clflush + mfence, then a TAIL
// doorbell write), but targets the RENDER engine registers at base 0x2000.

#![allow(dead_code)]

use super::{
    RenderEngine, RenderError, RCS_RING_DWORDS, RENDER_FAULT_REG, RENDER_RING_HEAD,
    RENDER_RING_TAIL,
};

impl RenderEngine {
    /// Push `dwords` onto the RCS ring and advance the tail (ring the doorbell).
    /// Blocks while the ring is full; returns `RingFull` on a suspected hang.
    ///
    /// ★★★ Padded to an EVEN number of dwords with MI_NOOP. On Gen8+ the ring TAIL must be
    /// QWORD-aligned — i915 pads every emission for exactly this reason — and a misaligned tail
    /// leaves the command streamer's behaviour undefined. Every submission this driver made with an
    /// even count worked on the hardware (the 4-dword MI_STORE_DATA_IMM self-test, the 6-dword
    /// PIPE_CONTROL fence). The ONE odd-length submission was MI_BATCH_BUFFER_START — 3 dwords —
    /// which is how every composite and every GPU text draw was issued, and every one of them hung:
    /// `gpu` on the hardware read "text drawn 0, refused 233", with the render latch tripped. The boot
    /// code had written this off as "BB_END doesn't return on this HW" and simply avoided batches in
    /// its self-tests; the batch itself was probably never entered correctly. And one misaligned
    /// submission misaligns every submission after it.
    pub unsafe fn rcs_submit(&mut self, dwords: &[u32]) -> Result<(), RenderError> {
        let ring_virt = self.ring_virt.ok_or(RenderError::NotInitialized)?;
        let ring = ring_virt as *mut u32;
        let pad = (dwords.len() % 2) as u32;
        let needed = dwords.len() as u32 + pad;

        // 1. Wait for enough free space (HEAD chases TAIL as the GPU consumes).
        // Time-bounded rather than iteration-bounded: see `SpinDeadline`.
        let deadline = super::SpinDeadline::new(super::RING_TIMEOUT_US);
        loop {
            let head_idx = self.read_reg(RENDER_RING_HEAD) / 4;
            let tail_idx = self.read_reg(RENDER_RING_TAIL) / 4;
            let free = if head_idx > tail_idx {
                head_idx - tail_idx - 1
            } else {
                RCS_RING_DWORDS - tail_idx + head_idx - 1
            };
            if free >= needed {
                break;
            }
            if deadline.expired() {
                let fault = self.read_reg(RENDER_FAULT_REG);
                crate::serial_println!(
                    "[RCS] FATAL: ring full / hang (HEAD={:#x} TAIL={:#x} FAULT={:#010x})",
                    head_idx * 4, tail_idx * 4, fault
                );
                return Err(RenderError::RingFull);
            }
            core::hint::spin_loop();
        }

        // 2. Write the dwords into the ring, flushing each cache line to RAM so the
        //    GPU's command fetch sees them.
        let mut tail_idx = self.read_reg(RENDER_RING_TAIL) / 4;
        let noop = [super::cmd::MI_NOOP];
        let fill: &[u32] = if pad != 0 { &noop } else { &[] };
        for &dw in dwords.iter().chain(fill.iter()) {
            let ptr = ring.add(tail_idx as usize);
            ptr.write_volatile(dw);
            core::arch::asm!("clflush [{}]", in(reg) ptr, options(nostack, preserves_flags));
            tail_idx += 1;
            if tail_idx >= RCS_RING_DWORDS {
                tail_idx = 0;
            }
        }

        // 3. Fence, then ring the doorbell by advancing TAIL.
        core::arch::asm!("mfence", options(nostack, preserves_flags));
        self.write_reg(RENDER_RING_TAIL, tail_idx * 4);
        Ok(())
    }
}
