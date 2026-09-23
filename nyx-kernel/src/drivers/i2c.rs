//! Intel LPSS (Synopsys Designware) I2C host controller, polled.
//!
//! The touchpad's real interface is I2C-HID behind one of these (`\_SB.PCI0.I2C1` = PCI 00:15.1 on
//! this laptop). Everything here is register programming and FIFO polling — no interrupts, no DMA.
//! That is deliberate for bring-up: a polled transfer either completes or reports exactly which
//! step failed, and nothing it does can wedge an interrupt line.
//!
//! ⚠️ **Every wait is a [`SpinDeadline`] in microseconds, never an iteration count.** The GPU driver
//! spent three failed fixes learning that an iteration-capped spin means a different duration in
//! every loop body and a different one again on every CPU.
//!
//! ⚠️ **Nothing in this file talks to ACPI.** In particular it never evaluates the device's `_DSM`:
//! on this firmware that call makes the EC switch the touchpad OFF PS/2 (see `custom_acpi.c`,
//! `NyxHidDescriptorRegister`). Plain I2C traffic does not.

use crate::drivers::gpu::intel::render::SpinDeadline;
use crate::pci::PciDriver;

// Designware core registers (offsets from BAR0).
const IC_CON: u64 = 0x00;
const IC_TAR: u64 = 0x04;
const IC_DATA_CMD: u64 = 0x10;
const IC_SS_SCL_HCNT: u64 = 0x14;
const IC_SS_SCL_LCNT: u64 = 0x18;
const IC_FS_SCL_HCNT: u64 = 0x1C;
const IC_FS_SCL_LCNT: u64 = 0x20;
const IC_INTR_MASK: u64 = 0x30;
const IC_RAW_INTR_STAT: u64 = 0x34;
const IC_RX_TL: u64 = 0x38;
const IC_TX_TL: u64 = 0x3C;
const IC_CLR_INTR: u64 = 0x40;
const IC_CLR_TX_ABRT: u64 = 0x54;
const IC_CLR_STOP_DET: u64 = 0x60;
const IC_ENABLE: u64 = 0x6C;
const IC_TXFLR: u64 = 0x74;
const IC_RXFLR: u64 = 0x78;
const IC_SDA_HOLD: u64 = 0x7C;
const IC_TX_ABRT_SOURCE: u64 = 0x80;
const IC_ENABLE_STATUS: u64 = 0x9C;
const IC_COMP_PARAM_1: u64 = 0xF4;
const IC_COMP_TYPE: u64 = 0xFC;

/// What `IC_COMP_TYPE` reads on every Designware I2C block ("DW" + 0x0140). Anything else means the
/// BAR is wrong, the function is powered down (all-ones), or it is still held in reset.
pub const DW_COMP_TYPE: u32 = 0x4457_0140;

/// LPSS private register space, and its reset control. Bits 0-1 release the function, bit 2 the
/// integrated DMA; firmware may leave any of them asserted, and a held function reads as zeros.
const LPSS_PRIV_RESETS: u64 = 0x204;
const LPSS_RESETS_RELEASE: u32 = 0x7;

// IC_CON bits.
const CON_MASTER: u32 = 1 << 0;
const CON_SPEED_SS: u32 = 1 << 1;
const CON_SPEED_FS: u32 = 2 << 1;
const CON_RESTART_EN: u32 = 1 << 5;
const CON_SLAVE_DISABLE: u32 = 1 << 6;

// IC_DATA_CMD bits.
const CMD_READ: u32 = 1 << 8;
const CMD_STOP: u32 = 1 << 9;
const CMD_RESTART: u32 = 1 << 10;

// IC_RAW_INTR_STAT bits.
const INTR_TX_ABRT: u32 = 1 << 6;
const INTR_STOP_DET: u32 = 1 << 9;

/// Budget for one transfer. A 64-byte read at 100 kHz is ~6 ms on the wire, so 25 ms only ever
/// expires on a bus that is genuinely stuck (a device holding SCL low, or a controller not clocking).
const XFER_TIMEOUT_US: u64 = 25_000;
/// Budget for IC_ENABLE to take effect. The controller finishes the current byte first.
const ENABLE_TIMEOUT_US: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum I2cError {
    /// The transfer was aborted; the payload is `IC_TX_ABRT_SOURCE`. Bit 0 = the 7-bit address was
    /// not acknowledged — nobody answered at that address.
    Abort(u32),
    /// Nothing finished in time.
    Timeout,
    /// More bytes than the FIFOs can hold in one go. Callers stay under [`Controller::fifo_depth`].
    TooLong,
    /// A large transfer ended early: the TX FIFO ran dry between refills and the controller sent
    /// a STOP. Retrying is safe.
    Underrun,
}

/// Which bus timing the controller was programmed with, and where the numbers came from.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    /// 1 = standard mode (100 kHz), 2 = fast mode (400 kHz).
    pub mode: u32,
    pub hcnt: u32,
    pub lcnt: u32,
    pub hold: u32,
    /// True if these came from the firmware's own per-board values rather than our defaults.
    pub from_firmware: bool,
}

pub struct Controller {
    base: u64,
    tx_depth: usize,
    rx_depth: usize,
    tar: u32,
    pub timing: Timing,
}

/// Why bringing the controller up failed, at the step it failed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BringUpError {
    /// Vendor ID reads 0xFFFF: the function is hidden (the firmware's `IMn` mode is not PCI) or off.
    Absent,
    /// The function is not a serial-bus controller (payload: its class code), so it is not the I2C
    /// controller whatever ACPI said, and nothing was touched.
    WrongClass(u8),
    /// BAR0 is unassigned and no address could be given to it safely. See [`BringUpInfo`] for which
    /// check refused.
    NoSafeAddress,
    /// BAR0 is not the 4 KiB 64-bit memory BAR an LPSS controller has — this is not the device the
    /// assignment logic was written for, so it is left alone.
    UnexpectedBar,
    /// Wrote an address into BAR0 and it did not read back.
    AssignFailed,
    MapFailed,
    /// `IC_COMP_TYPE` was not [`DW_COMP_TYPE`] even after releasing reset. Payload is what it read.
    NotDesignware(u32),
    /// IC_ENABLE never cleared.
    DisableTimeout,
}

/// Everything learned while bringing the controller up, for the diagnostic.
#[derive(Clone, Copy, Debug, Default)]
pub struct BringUpInfo {
    pub vendor_device: u32,
    pub pmcsr_before: u32,
    /// BAR0 as the firmware left it (address bits only), and as it was finally used.
    pub bar0_before: u64,
    pub bar0: u64,
    pub bar0_size: u32,
    /// True if BAR0 was unassigned and this driver gave it an address.
    pub assigned: bool,
    /// Top of upper usable DRAM (host bridge TOUUD). Addresses above it route to PCI.
    pub touud: u64,
    /// Where everything other PCI functions claim above 4 GiB provably ENDS (0 = nothing there).
    pub claims_end_above_4g: u64,
    /// The address BAR0 would be given — reported whether or not the checks allowed it.
    pub candidate: u64,
    pub phys_bits: u32,
    pub resets_before: u32,
    pub comp_type: u32,
    pub comp_param1: u32,
}

/// Where an unassigned LPSS BAR0 goes when NOTHING above 4 GiB is claimed yet, with BAR1 after it.
///
/// ★ This firmware leaves the Serial IO controllers in PCI mode with BAR0 = 0 and the function in
/// D3 — measured on hardware: `PMCSR 0xb, BAR0 0x0`. The OS is expected to allocate it, as Windows'
/// and Linux's PCI cores do from the root bridge's 64-bit window. Linux uses 0x40_1000_0000 on
/// platforms whose window starts at 256 GiB.
///
/// ⚠️ On THIS laptop that address was refused, correctly: another device already had a BAR at
/// 0x60_01B2_8000, so the firmware's 64-bit devices live near 384 GiB and 256 GiB is not proven to
/// be in the window at all. When anything is claimed up there, [`assign_bar0`] instead goes
/// immediately ABOVE the highest claim — inside the region the firmware itself already routes.
///
/// Nyx has no general PCI resource allocator, and the root window cannot be read from ACPI here
/// (`PCI0._CRS` computes it from host-bridge PCI_Config fields, and `AcpiOsReadPciConfiguration`
/// returns all-ones). So the address is PROVEN free rather than trusted: see
/// [`claims_end_above_4g`].
const FALLBACK_BASE: u64 = 0x40_1000_0000;

#[inline(always)]
fn rd(base: u64, off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}
#[inline(always)]
fn wr(base: u64, off: u64, v: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) }
}

/// Find a PCI capability by ID. Returns its config-space offset.
fn find_cap(bus: u8, dev: u8, func: u8, id: u8) -> Option<u8> {
    let status = PciDriver::read_config(bus, dev, func, 0x04) >> 16;
    if status & (1 << 4) == 0 {
        return None;
    }
    let mut p = (PciDriver::read_config(bus, dev, func, 0x34) & 0xFC) as u8;
    let mut guard = 0;
    while p != 0 && guard < 48 {
        let cap = PciDriver::read_config(bus, dev, func, p);
        if (cap & 0xFF) as u8 == id {
            return Some(p);
        }
        p = ((cap >> 8) & 0xFC) as u8;
        guard += 1;
    }
    None
}

/// BAR0's address bits, 64-bit aware. 0 for an I/O BAR.
fn bar0_address(bus: u8, dev: u8, func: u8) -> u64 {
    bar_address(bus, dev, func, 0x10).0
}

/// A memory BAR at config offset `off`: (address, is_64bit). An I/O BAR reads as (0, false).
fn bar_address(bus: u8, dev: u8, func: u8, off: u8) -> (u64, bool) {
    let lo = PciDriver::read_config(bus, dev, func, off);
    if lo & 1 != 0 {
        return (0, false);
    }
    let is64 = (lo >> 1) & 3 == 2;
    let mut addr = (lo & !0xF) as u64;
    if is64 {
        addr |= (PciDriver::read_config(bus, dev, func, off + 4) as u64) << 32;
    }
    (addr, is64)
}

/// Size a 64-bit memory BAR by the all-ones probe, restoring it afterwards.
///
/// ⚠️ Only ever called on the controller being brought up, with its memory decode OFF — sizing
/// rewrites the BAR, and a device decoding while that happens answers at a garbage address. It is
/// never done to any other device: those are live (the GPU, the NVMe disk) and this is not boot.
fn size_bar64(bus: u8, dev: u8, func: u8, off: u8) -> u64 {
    let lo = PciDriver::read_config(bus, dev, func, off);
    let hi = PciDriver::read_config(bus, dev, func, off + 4);
    PciDriver::write_config(bus, dev, func, off, 0xFFFF_FFFF);
    PciDriver::write_config(bus, dev, func, off + 4, 0xFFFF_FFFF);
    let mlo = PciDriver::read_config(bus, dev, func, off);
    let mhi = PciDriver::read_config(bus, dev, func, off + 4);
    PciDriver::write_config(bus, dev, func, off, lo);
    PciDriver::write_config(bus, dev, func, off + 4, hi);
    let mask = ((mhi as u64) << 32) | (mlo & !0xF) as u64;
    if mask == 0 { 0 } else { (!mask).wrapping_add(1) }
}

/// Where everything any OTHER PCI function claims above 4 GiB provably ends (exclusive). 0 if
/// nothing is claimed up there.
///
/// Other devices' BARs are live (the GPUs, the NVMe disk), so they are only READ, never sized — see
/// [`size_bar64`]. Their extent is bounded instead by the rule every PCI BAR obeys: it is naturally
/// aligned to its size. So a BAR at base B is at most `B & -B` long. That bound is only needed for
/// the HIGHEST base: any lower BAR extending past it would overlap it, which PCI forbids.
///
/// Bridge prefetchable windows are claims too, and route their whole range downstream — their
/// LIMIT counts, not their base.
fn claims_end_above_4g(skip: (u8, u8, u8)) -> u64 {
    let mut top_base = 0u64;
    let mut end = 0u64;
    for d in PciDriver::new().scan() {
        if (d.bus, d.device, d.func) == skip {
            continue;
        }
        let header = (PciDriver::read_config(d.bus, d.device, d.func, 0x0C) >> 16) & 0x7F;
        let nbars = match header {
            0 => 6,
            1 => 2,
            _ => 0,
        };
        let mut i = 0u8;
        while i < nbars {
            let (addr, is64) = bar_address(d.bus, d.device, d.func, 0x10 + i * 4);
            if addr >= 0x1_0000_0000 {
                top_base = top_base.max(addr);
            }
            i += if is64 { 2 } else { 1 };
        }
        if header == 1 {
            // Prefetchable window: base/limit in 0x24 (bits 31:20 of each), upper halves 0x28/0x2C.
            let pl = PciDriver::read_config(d.bus, d.device, d.func, 0x24);
            let base_hi = PciDriver::read_config(d.bus, d.device, d.func, 0x28) as u64;
            let limit_hi = PciDriver::read_config(d.bus, d.device, d.func, 0x2C) as u64;
            let base = (base_hi << 32) | (((pl & 0xFFF0) as u64) << 16);
            let limit = (limit_hi << 32) | ((((pl >> 16) & 0xFFF0) as u64) << 16) | 0xF_FFFF;
            if base <= limit && limit >= 0x1_0000_0000 {
                end = end.max(limit + 1);
            }
        }
    }
    if top_base != 0 {
        // Lowest set bit = the largest size a BAR at this base can have.
        end = end.max(top_base + (top_base & top_base.wrapping_neg()));
    }
    end
}

/// Give an unassigned LPSS BAR0 (and its BAR1) an address, or explain why not.
fn assign_bar0(
    bus: u8,
    dev: u8,
    func: u8,
    fw: &crate::acpi::I2cHidInfo,
    info: &mut BringUpInfo,
) -> Result<u64, BringUpError> {
    let lo = PciDriver::read_config(bus, dev, func, 0x10);
    if lo & 1 != 0 || (lo >> 1) & 3 != 2 {
        return Err(BringUpError::UnexpectedBar);
    }

    // Decode off for everything below: sizing and reprogramming a live BAR is how a device ends up
    // answering at an address that belongs to something else.
    let cmd = PciDriver::read_config(bus, dev, func, 0x04) & 0xFFFF;
    PciDriver::write_config(bus, dev, func, 0x04, cmd & !0x6);

    let size = size_bar64(bus, dev, func, 0x10);
    info.bar0_size = size as u32;
    if size != 0x1000 {
        return Err(BringUpError::UnexpectedBar);
    }

    // TOUUD (host bridge 00:00.0, 0xA8): everything at or above it routes to PCI rather than DRAM.
    let touud = ((PciDriver::read_config(0, 0, 0, 0xAC) as u64) << 32
        | PciDriver::read_config(0, 0, 0, 0xA8) as u64)
        & 0x0000_007F_FFF0_0000;
    info.touud = touud;
    let phys_bits = unsafe { core::arch::x86_64::__cpuid(0x8000_0008).eax & 0xFF };
    info.phys_bits = phys_bits;
    let claims_end = claims_end_above_4g((bus, dev, func));
    info.claims_end_above_4g = claims_end;

    // ★ Inside the root bridge's 64-bit window when the firmware declares one (`M64B`/`M64L`, the
    // same numbers its PCI0._CRS hands Windows and Linux), and above everything already claimed
    // in it, on a 1 MiB boundary. Without a declared window, the fallback — still subject to every
    // check below.
    const MIB: u64 = 0x10_0000;
    let (win_lo, win_hi) = if fw.m64_len != 0 {
        (fw.m64_base, fw.m64_base.saturating_add(fw.m64_len))
    } else {
        (0, u64::MAX)
    };
    let floor = claims_end.max(win_lo);
    let base = if floor == 0 { FALLBACK_BASE } else { (floor + MIB - 1) & !(MIB - 1) };
    info.candidate = base;

    let end = base + 0x2000; // BAR0 + BAR1
    let safe = phys_bits >= 36
        && base >= win_lo
        && end <= win_hi
        && end <= (1u64 << phys_bits.min(63))
        // Above DRAM, or it would decode as memory rather than reach the PCI side.
        && touud != 0
        && base >= touud
        && base >= claims_end
        // The identity mapping must not land on anything already mapped at that virtual address.
        && unsafe { !crate::memory::user_addr_mapped(base) }
        && unsafe { !crate::memory::user_addr_mapped(base + 0x1000) };
    if !safe {
        return Err(BringUpError::NoSafeAddress);
    }

    PciDriver::write_config(bus, dev, func, 0x10, base as u32);
    PciDriver::write_config(bus, dev, func, 0x14, (base >> 32) as u32);
    if bar0_address(bus, dev, func) != base {
        return Err(BringUpError::AssignFailed);
    }

    // BAR1 is the LPSS config-space mirror. Unused here, but a BAR left at zero with memory decode
    // on claims physical page 0 — so it gets the next page, if it is the same 4 KiB 64-bit shape.
    let (b1, b1_64) = bar_address(bus, dev, func, 0x18);
    if b1_64 && b1 < 0x10_0000 && size_bar64(bus, dev, func, 0x18) == 0x1000 {
        PciDriver::write_config(bus, dev, func, 0x18, (base + 0x1000) as u32);
        PciDriver::write_config(bus, dev, func, 0x1C, ((base + 0x1000) >> 32) as u32);
    }

    info.assigned = true;
    Ok(base)
}

impl Controller {
    /// Power the controller up and program it as a bus master at `speed_hz`.
    ///
    /// `fw` carries the firmware's tuned timings (all-zero if the firmware had none). Called from a
    /// kernel task at IF=1, because the D3→D0 transition needs a real 10 ms wait.
    pub fn bring_up(
        bus: u8,
        dev: u8,
        func: u8,
        speed_hz: u32,
        fw: &crate::acpi::I2cHidInfo,
        info: &mut BringUpInfo,
    ) -> Result<Self, BringUpError> {
        let id = PciDriver::read_config(bus, dev, func, 0x00);
        info.vendor_device = id;
        if id & 0xFFFF == 0xFFFF {
            return Err(BringUpError::Absent);
        }
        // ⚠️ Only a serial-bus controller (class 0x0C — LPSS I2C is 0x0C80) is touched. Everything
        // below powers the function up, may size and reprogram its BAR, and writes its registers;
        // aimed at the wrong function that is dangerous. QEMU caught exactly that: an unresolved
        // controller path decoded as device 0 function 0 and the probe landed on the HOST BRIDGE.
        let class = PciDriver::read_config(bus, dev, func, 0x08) >> 24;
        if class != 0x0C {
            return Err(BringUpError::WrongClass(class as u8));
        }

        // D0. The firmware's own `_PS0` for this controller is exactly this: zero the low byte of
        // PMCSR (config 0x84 here). Done through the capability list rather than the hardcoded
        // offset so a different board still finds it.
        if let Some(pm) = find_cap(bus, dev, func, 0x01) {
            let pmcsr = PciDriver::read_config(bus, dev, func, pm + 4);
            info.pmcsr_before = pmcsr;
            if pmcsr & 0x3 != 0 {
                PciDriver::write_config(bus, dev, func, pm + 4, pmcsr & !0x3);
                // PCI PM: 10 ms from D3hot to D0 before the function may be touched.
                crate::scheduler::kernel_sleep_ms(10);
            }
        }

        let bar = bar0_address(bus, dev, func);
        info.bar0_before = bar;
        // ⚠️ "Unassigned" is anything that is not a plausible MMIO address, not just zero. The
        // first hardware run mapped BAR0 = 0 and read physical page 0 as if it were the
        // controller — and leaving page 0 mapped means a kernel null dereference no longer faults.
        let bar = if bar < 0x10_0000 || bar & 0xFFF != 0 {
            assign_bar0(bus, dev, func, fw, info)?
        } else {
            bar
        };
        info.bar0 = bar;

        // Memory space + bus master — only now that BAR0 holds a real address.
        let cmd = PciDriver::read_config(bus, dev, func, 0x04);
        if cmd & 0x6 != 0x6 {
            PciDriver::write_config(bus, dev, func, 0x04, (cmd & 0xFFFF) | 0x6);
        }

        // ⚠️ With interrupts masked: `map_mmio` takes MEMORY_MANAGER, which does not mask them
        // itself, and a syscall on this core taking it after a preemption here would deadlock.
        let mapped = x86_64::instructions::interrupts::without_interrupts(|| unsafe {
            crate::memory::map_mmio(bar, 0x1000)
        });
        let base = mapped.map_err(|_| BringUpError::MapFailed)?;

        // Release the LPSS function (and its DMA) from reset. A held function reads zeros.
        let resets = rd(base, LPSS_PRIV_RESETS);
        info.resets_before = resets;
        if resets & LPSS_RESETS_RELEASE != LPSS_RESETS_RELEASE {
            wr(base, LPSS_PRIV_RESETS, LPSS_RESETS_RELEASE);
            crate::scheduler::kernel_sleep_ms(1);
        }

        let ct = rd(base, IC_COMP_TYPE);
        info.comp_type = ct;
        if ct != DW_COMP_TYPE {
            return Err(BringUpError::NotDesignware(ct));
        }
        let p1 = rd(base, IC_COMP_PARAM_1);
        info.comp_param1 = p1;
        // COMP_PARAM_1: RX depth-1 in 15:8, TX depth-1 in 23:16. Zero means "not reported"; 8 is
        // the smallest depth a Designware block is built with, so assume that.
        let rx_depth = if p1 == 0 { 8 } else { (((p1 >> 8) & 0xFF) + 1) as usize };
        let tx_depth = if p1 == 0 { 8 } else { (((p1 >> 16) & 0xFF) + 1) as usize };

        let mut c = Controller { base, tx_depth, rx_depth, tar: 0, timing: Timing::default() };
        c.disable()?;

        // Timing. Prefer the firmware's per-board values: they were tuned against the real input
        // clock and bus capacitance, which is not something that can be derived here. Our defaults
        // are standard mode, computed for a 216 MHz input clock (Cannon Point's) — the fastest this
        // family uses, so on a slower clock the bus only runs SLOWER than 100 kHz, never faster.
        let fast = speed_hz >= 400_000 && fw.fm_hcnt != 0 && fw.fm_lcnt != 0;
        let timing = if fast {
            Timing { mode: 2, hcnt: fw.fm_hcnt, lcnt: fw.fm_lcnt, hold: fw.fm_hold, from_firmware: true }
        } else if fw.ss_hcnt != 0 && fw.ss_lcnt != 0 {
            Timing { mode: 1, hcnt: fw.ss_hcnt, lcnt: fw.ss_lcnt, hold: fw.ss_hold, from_firmware: true }
        } else {
            // 4.63 us high + 5.37 us low at 216 MHz = 10 us = 100 kHz; 300 ns SDA hold.
            Timing { mode: 1, hcnt: 1000, lcnt: 1160, hold: 65, from_firmware: false }
        };
        if timing.mode == 2 {
            wr(base, IC_FS_SCL_HCNT, timing.hcnt);
            wr(base, IC_FS_SCL_LCNT, timing.lcnt);
            wr(base, IC_CON, CON_MASTER | CON_SPEED_FS | CON_RESTART_EN | CON_SLAVE_DISABLE);
        } else {
            wr(base, IC_SS_SCL_HCNT, timing.hcnt);
            wr(base, IC_SS_SCL_LCNT, timing.lcnt);
            wr(base, IC_CON, CON_MASTER | CON_SPEED_SS | CON_RESTART_EN | CON_SLAVE_DISABLE);
        }
        if timing.hold != 0 {
            wr(base, IC_SDA_HOLD, timing.hold & 0xFFFF);
        }
        c.timing = timing;

        // Polled: no interrupt sources, thresholds at zero.
        wr(base, IC_INTR_MASK, 0);
        wr(base, IC_RX_TL, 0);
        wr(base, IC_TX_TL, 0);
        let _ = rd(base, IC_CLR_INTR);
        Ok(c)
    }

    /// The smaller of the two FIFOs — the most bytes one [`Controller::write_read`] may move in
    /// each direction.
    pub fn fifo_depth(&self) -> usize {
        self.tx_depth.min(self.rx_depth)
    }

    fn disable(&mut self) -> Result<(), BringUpError> {
        wr(self.base, IC_ENABLE, 0);
        let dl = SpinDeadline::new(ENABLE_TIMEOUT_US);
        while rd(self.base, IC_ENABLE_STATUS) & 1 != 0 {
            if dl.expired() {
                return Err(BringUpError::DisableTimeout);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    fn enable(&mut self) -> Result<(), I2cError> {
        wr(self.base, IC_ENABLE, 1);
        let dl = SpinDeadline::new(ENABLE_TIMEOUT_US);
        while rd(self.base, IC_ENABLE_STATUS) & 1 == 0 {
            if dl.expired() {
                return Err(I2cError::Timeout);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    /// Write `wbuf`, then (with a repeated START) read `rbuf.len()` bytes, from 7-bit `addr`.
    ///
    /// This is the shape of every I2C-HID register access: a 2-byte little-endian register number
    /// out, the register's contents back.
    pub fn write_read(&mut self, addr: u8, wbuf: &[u8], rbuf: &mut [u8]) -> Result<(), I2cError> {
        let total = wbuf.len() + rbuf.len();
        if total == 0 {
            return Ok(());
        }
        if total > self.tx_depth || rbuf.len() > self.rx_depth {
            return self.write_read_paced(addr, wbuf, rbuf);
        }

        // The target can only change while the controller is disabled.
        if self.tar != addr as u32 {
            self.disable().map_err(|_| I2cError::Timeout)?;
            wr(self.base, IC_TAR, addr as u32 & 0x7F);
            self.tar = addr as u32;
        }
        if rd(self.base, IC_ENABLE_STATUS) & 1 == 0 {
            self.enable()?;
        }
        let _ = rd(self.base, IC_CLR_INTR);

        // Queue the WHOLE transfer at once, interrupts masked. It fits the FIFO (checked above), and
        // a preemption between commands would let the TX FIFO run dry mid-message — on a controller
        // without "hold bus when empty" that ends the transfer with a STOP the device never asked for.
        x86_64::instructions::interrupts::without_interrupts(|| {
            for i in 0..total {
                let mut cmd = if i < wbuf.len() { wbuf[i] as u32 } else { CMD_READ };
                if i == wbuf.len() && !wbuf.is_empty() && !rbuf.is_empty() {
                    cmd |= CMD_RESTART;
                }
                if i == total - 1 {
                    cmd |= CMD_STOP;
                }
                wr(self.base, IC_DATA_CMD, cmd);
            }
        });

        let dl = SpinDeadline::new(XFER_TIMEOUT_US);
        let mut got = 0usize;
        loop {
            let raw = rd(self.base, IC_RAW_INTR_STAT);
            if raw & INTR_TX_ABRT != 0 {
                let src = rd(self.base, IC_TX_ABRT_SOURCE);
                let _ = rd(self.base, IC_CLR_TX_ABRT);
                let _ = rd(self.base, IC_CLR_STOP_DET);
                return Err(I2cError::Abort(src));
            }
            while got < rbuf.len() && rd(self.base, IC_RXFLR) > 0 {
                rbuf[got] = rd(self.base, IC_DATA_CMD) as u8;
                got += 1;
            }
            if got == rbuf.len() && raw & INTR_STOP_DET != 0 && rd(self.base, IC_TXFLR) == 0 {
                let _ = rd(self.base, IC_CLR_STOP_DET);
                return Ok(());
            }
            if dl.expired() {
                return Err(I2cError::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// [`write_read`] for transfers larger than the FIFOs — the 381-byte report descriptor.
    ///
    /// Commands are fed as FIFO space allows, never more reads outstanding than the RX FIFO holds.
    /// Interrupts are masked only while a refill is pushed (microseconds), not for the whole
    /// transfer, which at 400 kHz is ~9 ms for a report descriptor — far too long at IF=0.
    ///
    /// ⚠️ A preemption between refills can let the TX FIFO run dry. LPSS controllers normally hold
    /// SCL low until more commands arrive, but a controller built without that option ends the
    /// message with a STOP instead. That shows up here as STOP_DET before the last command was
    /// issued, and is reported as [`I2cError::Underrun`] rather than returned as a short read.
    fn write_read_paced(&mut self, addr: u8, wbuf: &[u8], rbuf: &mut [u8]) -> Result<(), I2cError> {
        let total = wbuf.len() + rbuf.len();
        if self.tar != addr as u32 {
            self.disable().map_err(|_| I2cError::Timeout)?;
            wr(self.base, IC_TAR, addr as u32 & 0x7F);
            self.tar = addr as u32;
        }
        if rd(self.base, IC_ENABLE_STATUS) & 1 == 0 {
            self.enable()?;
        }
        let _ = rd(self.base, IC_CLR_INTR);

        // ~23 us a byte at 400 kHz and ~90 us at 100 kHz; allow the slow case with margin.
        let dl = SpinDeadline::new(XFER_TIMEOUT_US + total as u64 * 120);
        let mut issued = 0usize;
        let mut got = 0usize;
        loop {
            let raw = rd(self.base, IC_RAW_INTR_STAT);
            if raw & INTR_TX_ABRT != 0 {
                let src = rd(self.base, IC_TX_ABRT_SOURCE);
                let _ = rd(self.base, IC_CLR_TX_ABRT);
                let _ = rd(self.base, IC_CLR_STOP_DET);
                return Err(I2cError::Abort(src));
            }
            if raw & INTR_STOP_DET != 0 && issued < total {
                let _ = rd(self.base, IC_CLR_STOP_DET);
                return Err(I2cError::Underrun);
            }

            // Refill.
            x86_64::instructions::interrupts::without_interrupts(|| {
                while issued < total && (rd(self.base, IC_TXFLR) as usize) < self.tx_depth {
                    if issued >= wbuf.len() {
                        let reads_issued = issued - wbuf.len();
                        if reads_issued - got >= self.rx_depth {
                            break;
                        }
                    }
                    let mut cmd = if issued < wbuf.len() { wbuf[issued] as u32 } else { CMD_READ };
                    if issued == wbuf.len() && !wbuf.is_empty() && !rbuf.is_empty() {
                        cmd |= CMD_RESTART;
                    }
                    if issued == total - 1 {
                        cmd |= CMD_STOP;
                    }
                    wr(self.base, IC_DATA_CMD, cmd);
                    issued += 1;
                }
            });

            while got < rbuf.len() && rd(self.base, IC_RXFLR) > 0 {
                rbuf[got] = rd(self.base, IC_DATA_CMD) as u8;
                got += 1;
            }
            if issued == total && got == rbuf.len() && raw & INTR_STOP_DET != 0 {
                let _ = rd(self.base, IC_CLR_STOP_DET);
                return Ok(());
            }
            if dl.expired() {
                return Err(I2cError::Timeout);
            }
            core::hint::spin_loop();
        }
    }
}
