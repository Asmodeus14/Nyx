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
    /// BAR0 is unassigned.
    NoBar,
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
    pub bar0: u64,
    pub resets_before: u32,
    pub comp_type: u32,
    pub comp_param1: u32,
}

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

        // Memory space + bus master.
        let cmd = PciDriver::read_config(bus, dev, func, 0x04);
        if cmd & 0x6 != 0x6 {
            PciDriver::write_config(bus, dev, func, 0x04, (cmd & 0xFFFF) | 0x6);
        }

        let bar = PciDriver::new()
            .get_bar_address(
                &crate::pci::PciDevice {
                    bus, device: dev, func,
                    vendor_id: id as u16, device_id: (id >> 16) as u16,
                    class_id: 0, subclass_id: 0,
                },
                0,
            )
            .ok_or(BringUpError::NoBar)?;
        info.bar0 = bar;

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
            return Err(I2cError::TooLong);
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
}
