use crate::pci::PciDevice;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::Ordering;
use alloc::vec::Vec;

// The raw firmware blob from Linux (a TLV ".ucode" container; parsed in Phase 3, bundled now).
const FIRMWARE_BLOB: &[u8] = include_bytes!("../../iwlwifi.ucode");

// --- INTEL HARDWARE REGISTERS (CSR) --------------------------------------------------
// Offsets are relative to BAR0, taken verbatim from Linux `iwl-csr.h`. NOTE: the previous
// stub had CSR_RESET=0x010 (really FH_INT_STATUS) and CSR_GP_CNTRL=0x044 (really 0x024),
// so its "reset" poked the wrong registers. These are the correct values.
const CSR_HW_IF_CONFIG_REG: usize = 0x000;
const CSR_INT:              usize = 0x008;
const CSR_INT_MASK:         usize = 0x00C;
const CSR_FH_INT_STATUS:    usize = 0x010;
const CSR_RESET:            usize = 0x020;
const CSR_GP_CNTRL:         usize = 0x024;
const CSR_HW_REV:           usize = 0x028;
const CSR_GIO_REG:          usize = 0x03C;
const CSR_HW_RF_ID:         usize = 0x09C;

// --- CSR_HW_IF_CONFIG_REG bits (prepare-card / ownership handshake) ---
const CSR_HW_IF_CONFIG_REG_BIT_NIC_READY: u32 = 0x0040_0000; // "PCI_OWN_SET" — poll target
const CSR_HW_IF_CONFIG_REG_PREPARE:       u32 = 0x0800_0000; // request ownership

// --- CSR_GP_CNTRL bits ---
const CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY: u32 = 0x0000_0001;
const CSR_GP_CNTRL_REG_FLAG_INIT_DONE:       u32 = 0x0000_0004;
const CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ:  u32 = 0x0000_0008;

// --- CSR_RESET bits ---
const CSR_RESET_REG_FLAG_SW_RESET:        u32 = 0x0000_0080; // AX201 (family 22000, pre-BZ)
const CSR_RESET_REG_FLAG_MASTER_DISABLED: u32 = 0x0000_0100;

// --- Firmware TLV ".ucode" container (Linux iwlwifi fw/file.h) ---
const IWL_TLV_UCODE_MAGIC: u32 = 0x0a4c_5749; // "IWL\n"
const TLV_SEC_RT:      u32 = 19; // a runtime firmware section
const TLV_NUM_OF_CPU:  u32 = 27; // u32: 1 or 2 CPUs
const TLV_PHY_SKU:     u32 = 23; // u32 fw->phy_config: radio type/step/dash + tx/rx chains
// Debug-address TLVs: carry the fw error-table SRAM pointers so an error BEFORE ALIVE can be
// decoded (normally the ALIVE notif delivers them, but we never reach ALIVE).
const TLV_UMAC_DEBUG_ADDRS: u32 = 54; // struct iwl_umac_debug_addrs { error_info_addr@0, .. }
const TLV_LMAC_DEBUG_ADDRS: u32 = 55; // struct iwl_lmac_debug_addrs { error_event_table_ptr@0, .. }
const FW_ADDR_CACHE_CONTROL: u32 = 0xC000_0000; // high cache-control bits to strip off the ptr
const CPU1_CPU2_SEPARATOR: u32 = 0xFFFF_CCCC; // section-offset marker: CPU1(LMAC) -> CPU2(UMAC)
const PAGING_SEPARATOR:    u32 = 0xAAAA_BBBB; // section-offset marker: start of paging sections

#[inline]
fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

// --- Phase 3c: gen2 device-start / firmware-load registers (Linux iwl-csr.h / iwl-prph.h) ---
const CSR_INT_COALESCING:     usize = 0x004; // u8: host int timeout
const CSR_GIO_CHICKEN_BITS:   usize = 0x100;
const CSR_DBG_HPET_MEM_REG:   usize = 0x240;
const CSR_MAC_SHADOW_REG_CTRL:usize = 0x0A8;
const CSR_UCODE_DRV_GP1_CLR:  usize = 0x05C;
const CSR_CTXT_INFO_BA:       usize = 0x040; // 64-bit: context-info block phys addr (the kick)
const HBUS_TARG_PRPH_WADDR:   usize = 0x444;
const HBUS_TARG_PRPH_WDAT:    usize = 0x44C;
const HBUS_TARG_PRPH_RADDR:   usize = 0x448;
const HBUS_TARG_PRPH_RDAT:    usize = 0x450;
// Direct SRAM (device memory) read window — RADDR auto-increments by 4 on each RDAT read
// (Linux iwl_trans_pcie_read_mem). Used to pull the fw error table out of device SRAM.
const HBUS_TARG_MEM_RADDR:    usize = 0x40C;
const HBUS_TARG_MEM_RDAT:     usize = 0x41C;
// gen2 RX free-RBD doorbell (shadow write-index trigger, DIRECT MMIO — iwl_write32, not PRPH). For
// context-info devices the FW configures RFH itself during boot; the host just rings this AFTER the
// fw is alive to publish the posted free RBDs so the fw can DMA notifications. Pre-BZ restock path.
const RFH_Q0_FRBDCB_WIDX_TRG: usize = 0x1C80;

// --- P4b: gen2 host-command TX path (Linux iwl_pcie_gen2_enqueue_hcmd + iwl_txq_inc_wr_ptr) ---
// The command queue is pre-configured by the fw from the context-info hcmd_cfg; post-ALIVE the host
// builds an iwl_tfh_tfd in the ring, puts the command bytes (wide header + payload) in a slot, and
// rings the TX write-ptr doorbell. NO byte-count table for the command queue (that's data-TX only).
const HBUS_TARG_WRPTR: usize = 0x460;   // TX doorbell: value = write_ptr | (queue_id << 16)
const IWL_FIRST_TB_SIZE: usize = 20;    // TB0 must be a dedicated <=20B buffer (hw requirement)
const TFH_TFD_SIZE: usize = 256;        // sizeof(iwl_tfh_tfd): num_tbs(2) + 25*tb(10) + pad(4)
const CMD_QUEUE_ID: u32 = 0;            // gen2/DQA command queue is queue 0
// Real MAC from CSR (mac_addr_from_csr=0x380 for QuZ/9000-gen2). Try STRAP first, then OTP.
const CSR_MAC_ADDR0_OTP:   usize = 0x380;
const CSR_MAC_ADDR1_OTP:   usize = 0x384;
const CSR_MAC_ADDR0_STRAP: usize = 0x388;
const CSR_MAC_ADDR1_STRAP: usize = 0x38C;
// Host command groups/ids for the post-ALIVE unified-fw init handshake (Linux
// iwl_run_unified_mvm_ucode). NOTE: REGULATORY_AND_NVM_GROUP is 0x0C — 0x0B is PROT_OFFLOAD_GROUP.
const SYSTEM_GROUP:          u8  = 0x02;
const REGULATORY_NVM_GROUP:  u8  = 0x0C;
const INIT_EXTENDED_CFG_CMD: u8  = 0x03; // SYSTEM_GROUP: "NVM-access commands follow"
const INIT_COMPLETE_NOTIF:   u8  = 0x04; // legacy group (arrives with group_id=0x00)
const NVM_ACCESS_COMPLETE:   u8  = 0x00; // REGULATORY_NVM_GROUP: finish NVM phase → fw finalizes init
const NVM_GET_INFO:          u8  = 0x02; // REGULATORY_NVM_GROUP: query NVM (version/channels/chains)
const IWL_INIT_NVM_FLAG:     u32 = 0x0000_0002; // init_flags = BIT(IWL_INIT_NVM=1)
// PHY_CONFIGURATION_CMD is intentionally omitted: iwl_send_phy_cfg_cmd() returns early (no command)
// for unified ucode without SISO-diversity — so NVM_ACCESS_COMPLETE is what triggers INIT_COMPLETE.

// Firmware boot-status / program-counter PRPH registers (for diagnosing an early SW_ERR).
const SB_CPU_1_STATUS:      u32 = 0x00A0_1E30; // LMAC secure-boot status
const SB_CPU_2_STATUS:      u32 = 0x00A0_1E34; // UMAC secure-boot status
const UMAG_SB_CPU_1_STATUS: u32 = 0x00A0_38C0;
const UMAG_SB_CPU_2_STATUS: u32 = 0x00A0_38C4;
const UREG_UMAC_CURRENT_PC:  u32 = 0x00A0_5C18;
const UREG_LMAC1_CURRENT_PC: u32 = 0x00A0_5C1C;
const UREG_LMAC2_CURRENT_PC: u32 = 0x00A0_5C20;

// Extended fw-boot diagnostics (find WHY the UMAC asserts): FSEQ = CNVi RF frequency-sequencer,
// WFPM = wireless-FW power manager (PHYRF power FSM), UREG load status, general HW status.
const UREG_UCODE_LOAD_STATUS: u32 = 0x00A0_5C40;
const UMAG_GEN_HW_STATUS:     u32 = 0x00A0_38C8;
const WFPM_LMAC1_PS_CTL_RW:   u32 = 0x00A0_3380; // PHYRF power-state FSM; state 5 = PHYRF_STATE_ON
const WFPM_GP2:               u32 = 0x00A0_30B4;
const FSEQ_ERROR_CODE:        u32 = 0x00A3_40C8;
const FSEQ_ALIVE_TOKEN:       u32 = 0x00A3_40F0;
const CNVI_AUX_MISC_CHIP:     u32 = 0x00A2_00B0;
const WFPM_PHYRF_STATE_ON:    u32 = 5;

// iwl_pcie_get_crf_id: enabling WFPM peripheral access + reading the RF-companion (CRF) id. iwlwifi
// does this EARLY (before fw). Enabling WFPM is needed to reach the CNVR/SD register domain and is a
// candidate prerequisite for the RF power path we're still failing to complete (PHYRF stuck at 0).
const WFPM_CTRL_REG:          u32 = 0x00A0_3030;
const ENABLE_WFPM:            u32 = 0x8000_0000; // BIT(31)
const SD_REG_VER:             u32 = 0x00A2_9600; // CRF id (pre-AX210); Fedora read 0x2816 here
const WFPM_OTP_CFG1_ADDR:     u32 = 0x00A0_3098; // wfpm/cdb id (Fedora: 0x80000000)

const GIO_CHICKEN_L1A_NO_L0S_RX:  u32 = 0x0080_0000;
const DBG_HPET_MEM_REG_VAL:       u32 = 0xFFFF_0000;
const HW_IF_CONFIG_HAP_WAKE_L1A:  u32 = 0x0008_0000;
const INT_COALESCING_DEF:         u32 = 0x40;
const UCODE_SW_BIT_RFKILL:        u32 = 0x0000_0002;
const UCODE_DRV_GP1_BIT_CMD_BLK:  u32 = 0x0000_0004;
const MAC_SHADOW_REG_CTRL_VAL:    u32 = 0x800F_FFFF;
const UREG_CPU_INIT_RUN:          u32 = 0x00A0_5C44; // prph: start the fw CPU

// LTR (Latency Tolerance Reporting) — integrated CNVi path (iwl_pcie_set_ltr).
const HPM_MAC_LTR_CSR:            u32 = 0x00A0_348C;
const HPM_MAC_LRT_ENABLE_ALL:     u32 = 0x0000_000F;
const HPM_UMAC_LTR:               u32 = 0x00A0_3480;
// ~250 usec, snoop + no-snoop, scale=usec: SNOOP_REQ|SNOOP_SCALE(2)|SNOOP_VAL(250)
// | NO_SNOOP_REQ|NO_SNOOP_SCALE(2)|NO_SNOOP_VAL(250) = 0x88FA_88FA.
const LTR_VAL:                    u32 = 0x88FA_88FA;

// --- op_mode_nic_config: program the radio type/step/dash into CSR_HW_IF_CONFIG_REG BEFORE the
// fw kick (Linux iwl_mvm_nic_config, called from iwl_pcie_gen2_nic_init). The UMAC reads these
// bits to know which PHY is attached; leaving them zero is a candidate for the early assert. ---
const HW_IF_CONFIG_POS_PHY_TYPE:      u32 = 10; // CSR_HW_IF_CONFIG_REG_POS_PHY_TYPE
const HW_IF_CONFIG_POS_PHY_DASH:      u32 = 12; // POS_PHY_DASH
const HW_IF_CONFIG_POS_PHY_STEP:      u32 = 14; // POS_PHY_STEP
const HW_IF_CONFIG_MSK_MAC_STEP_DASH: u32 = 0x0000_000F;
const HW_IF_CONFIG_MSK_PHY_TYPE:      u32 = 0x0000_0C00;
const HW_IF_CONFIG_MSK_PHY_DASH:      u32 = 0x0000_3000;
const HW_IF_CONFIG_MSK_PHY_STEP:      u32 = 0x0000_C000;
// FW_PHY_CFG_* bit positions inside fw->phy_config (Linux enum iwl_fw_phy_cfg).
const FW_PHY_CFG_RADIO_TYPE_POS: u32 = 0; // 2 bits
const FW_PHY_CFG_RADIO_STEP_POS: u32 = 2; // 2 bits
const FW_PHY_CFG_RADIO_DASH_POS: u32 = 4; // 2 bits

// iwl_enable_fw_load_int_ctx_info (non-MSI-X): unmask only ALIVE + FH_RX before the kick.
const CSR_INT_BIT_MASK_FW_LOAD: u32 = INT_BIT_ALIVE | INT_BIT_FH_RX;

// --- Integrated-22000 (QuZ/AX201) pre-FW CNVi power sequencing (Linux _iwl_trans_pcie_start_hw).
// Without these the CNVi power-domain FSM is left in a state where the fw's FSEQ cannot power the
// RF (PHYRF FSM stuck at 0, FSEQ_ERROR_CODE=3) and the UMAC asserts before ALIVE. ---
const HPM_DEBUG:              u32 = 0x00A0_3440;
const PERSISTENCE_BIT:       u32 = 1 << 12; // set across a (warm) reset → CNVi won't re-power the RF
const PREG_PRPH_WPROT_22000: u32 = 0x00A0_4D00;
const PREG_WFPM_ACCESS:      u32 = 1 << 12;
const HPM_HIPM_GEN_CFG:              u32 = 0x00A0_3458;
const HPM_HIPM_GEN_CFG_CR_PG_EN:     u32 = 1 << 0;
const HPM_HIPM_GEN_CFG_CR_SLP_EN:    u32 = 1 << 1;
const HPM_HIPM_GEN_CFG_CR_FORCE_ACTIVE: u32 = 1 << 10;

// CSR_INT status bits.
const INT_BIT_ALIVE:  u32 = 1 << 0;
const INT_BIT_SW_RX:  u32 = 1 << 3;
const INT_BIT_RF_KILL:u32 = 1 << 7;
const INT_BIT_SW_ERR: u32 = 1 << 25;
const INT_BIT_HW_ERR: u32 = 1 << 29;
const INT_BIT_FH_RX:  u32 = 1 << 31;

/// A physically-contiguous, <4 GiB DMA region (virt is the kernel linear-map alias).
#[derive(Clone, Copy)]
struct DmaRegion {
    virt: u64,
    phys: u64,
    pages: usize,
}

/// A firmware section extracted from the TLV .ucode blob. `data_start`/`data_len` index into
/// FIRMWARE_BLOB (a 'static slice), so no lifetimes to thread through the driver.
struct FwSection {
    offset: u32,      // firmware-declared load offset (post-separator-classification)
    data_start: usize,
    data_len: usize,
    cpu: u8,          // 1 = CPU1/LMAC, 2 = CPU2/UMAC
    paging: bool,
}

pub struct IntelWifiDriver {
    pci_device: PciDevice,
    mmio_base: u64,
    hw_rev: u32,
    rf_id: u32,

    // --- Phase 2: DMA transport skeleton (no firmware yet) ---
    ctxt_info: Option<DmaRegion>,   // iwl_context_info block (fw self-load descriptor)
    rx_free: Option<DmaRegion>,     // free-RBD ring (host -> device buffer list)
    rx_used: Option<DmaRegion>,     // used-RBD ring (device -> host completion list)
    rx_buffers: Vec<DmaRegion>,     // the actual RX DMA buffers
    tx_cmd_ring: Option<DmaRegion>, // host-command TFD ring
    tx_cmd_buf: Option<DmaRegion>,  // backing bytes for host commands

    // --- Phase 3: parsed firmware image ---
    fw_sections: Vec<FwSection>,
    fw_ver: u32,
    fw_build: u32,
    num_cpus: u32,
    fw_phy_config: u32,           // from TLV_PHY_SKU: radio type/step/dash for CSR_HW_IF_CONFIG_REG
    umac_error_addr: u32,         // from TLV_UMAC_DEBUG_ADDRS: SRAM ptr to the UMAC error table
    lmac_error_addr: u32,         // from TLV_LMAC_DEBUG_ADDRS: SRAM ptr to the LMAC error table

    // --- Phase 3b: firmware in DMA + status write-back ---
    fw_dma: Vec<DmaRegion>,        // one DMA block per fw section (same order as fw_sections)
    rb_stts: Option<DmaRegion>,    // RX status write-back (device writes closed index here)

    // --- Phase 4b: host-command TX + RX consumption ---
    first_tb_bufs: Option<DmaRegion>, // per-slot 20-byte "first TB" (TB0) buffers
    cmd_write_ptr: u32,               // command-queue producer index
    rx_read_ptr: u32,                 // used-BD entries consumed so far
    mac_addr: [u8; 6],
    alive_umac_err: u32,              // UMAC error-table SRAM ptr (from the ALIVE ntf, runtime)
    alive_lmac_err: u32,              // LMAC error-table SRAM ptr (from the ALIVE ntf, runtime)
}

impl IntelWifiDriver {
    pub fn new(device: PciDevice, bar0_phys: u64) -> Self {
        // Map BAR0 EXPLICITLY as uncached MMIO (identity map, like the GPU driver) rather than
        // trusting the bootloader linear map — the AX201 CNVi BAR may sit in an MMIO hole that
        // phys_to_virt() blindly offsets into unmapped space (→ page fault → double fault).
        let mmio_virt = unsafe { crate::memory::map_mmio(bar0_phys, 0x4000).unwrap_or(0) };
        Self {
            pci_device: device,
            mmio_base: mmio_virt,
            hw_rev: 0,
            rf_id: 0,
            ctxt_info: None,
            rx_free: None,
            rx_used: None,
            rx_buffers: Vec::new(),
            tx_cmd_ring: None,
            tx_cmd_buf: None,
            fw_sections: Vec::new(),
            fw_ver: 0,
            fw_build: 0,
            num_cpus: 1,
            fw_phy_config: 0,
            umac_error_addr: 0,
            lmac_error_addr: 0,
            fw_dma: Vec::new(),
            rb_stts: None,
            first_tb_bufs: None,
            cmd_write_ptr: 0,
            rx_read_ptr: 0,
            mac_addr: [0; 6],
            alive_umac_err: 0,
            alive_lmac_err: 0,
        }
    }

    #[inline]
    unsafe fn write32(&self, offset: usize, value: u32) {
        write_volatile((self.mmio_base + offset as u64) as *mut u32, value);
    }

    #[inline]
    unsafe fn read32(&self, offset: usize) -> u32 {
        read_volatile((self.mmio_base + offset as u64) as *const u32)
    }

    unsafe fn set_bit(&self, offset: usize, bits: u32) {
        let v = self.read32(offset);
        self.write32(offset, v | bits);
    }

    /// Read-modify-write: clear the bits in `mask`, then OR in `value & mask` (Linux
    /// iwl_trans_set_bits_mask).
    unsafe fn set_bits_mask(&self, offset: usize, mask: u32, value: u32) {
        let v = self.read32(offset);
        self.write32(offset, (v & !mask) | (value & mask));
    }

    /// Busy-wait `us` microseconds using the invariant TSC (real frequency from time::TSC_MHZ,
    /// NOT the bogus fixed 24 MHz the old stub assumed).
    fn delay_us(&self, us: u64) {
        let mhz = crate::time::TSC_MHZ.load(Ordering::Relaxed).max(1);
        let wait = us.saturating_mul(mhz); // ticks = us * (ticks per us)
        let (mut lo, mut hi): (u32, u32);
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let start = ((hi as u64) << 32) | lo as u64;
        loop {
            unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
            let now = ((hi as u64) << 32) | lo as u64;
            if now.wrapping_sub(start) > wait { break; }
            core::hint::spin_loop();
        }
    }

    #[inline]
    fn delay_ms(&self, ms: u64) { self.delay_us(ms * 1000); }

    // ============================ PHASE 1: BRING-UP ============================

    pub fn initialize(&mut self) -> bool {
        crate::serial_println!("[WIFI] Initializing Intel AX201 CNVi (gen2 context-info device)...");
        crate::serial_println!("[WIFI] Bundled firmware blob: {} bytes", FIRMWARE_BLOB.len());

        if self.mmio_base == 0 {
            crate::serial_println!("[WIFI] FATAL: BAR0 not mapped — aborting Wi-Fi bring-up.");
            return false;
        }

        unsafe {
            // NOTE: ACPI _PS0 power-on is DEFERRED to Phase 3. The card already enumerates on the
            // PCI bus (8086:06f0), so it is powered enough to read CSRs; that untested ACPICA FFI
            // path is not worth a boot-time crash risk here.

            // 1. Prepare-card / ownership handshake (poll NIC_READY).
            if self.prepare_card() {
                crate::serial_println!("[WIFI] Prepare-card OK (NIC owns the MAC).");
            } else {
                crate::serial_println!("[WIFI] WARN: NIC_READY not asserted — continuing to probe anyway.");
            }

            // 2. Software reset (AX201/family-22000 uses CSR_RESET SW_RESET at the *correct* 0x020).
            self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
            self.delay_ms(6);
            let reset_state = self.read32(CSR_RESET);
            if reset_state & CSR_RESET_REG_FLAG_MASTER_DISABLED != 0 {
                crate::serial_println!("[WIFI] Note: bus-master disabled post-reset (expected pre-APM).");
            }

            // 3. Request the MAC clock (non-fatal — HW_REV reads without it).
            self.set_mac_access();

            // 4. Identity read — this is the Phase-1 success signal.
            self.hw_rev = self.read32(CSR_HW_REV);
            self.rf_id = self.read32(CSR_HW_RF_ID);
            crate::serial_println!(
                "[WIFI] HW_REV={:#010x}  RF_ID={:#010x}  GP_CNTRL={:#010x}",
                self.hw_rev, self.rf_id, self.read32(CSR_GP_CNTRL)
            );

            if self.hw_rev == 0 || self.hw_rev == 0xFFFF_FFFF {
                crate::serial_println!("[WIFI] WARN: implausible HW_REV — card may not be responding.");
                return false;
            }
            crate::serial_println!("[WIFI] Card is alive and addressable. Phase 1 OK.");
            true
        }
    }

    /// Poll the NIC_READY bit once (with a short bounded wait).
    unsafe fn set_hw_ready(&self) -> bool {
        for _ in 0..50 {
            if self.read32(CSR_HW_IF_CONFIG_REG) & CSR_HW_IF_CONFIG_REG_BIT_NIC_READY != 0 {
                return true;
            }
            self.delay_us(200);
        }
        false
    }

    /// Request ownership of the device from the CNVi/PCH side (Linux `iwl_pcie_prepare_card_hw`).
    unsafe fn prepare_card(&self) -> bool {
        if self.set_hw_ready() { return true; }
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_PREPARE);
        for _ in 0..15 {
            if self.set_hw_ready() { return true; }
            self.delay_us(200);
        }
        false
    }

    unsafe fn set_mac_access(&self) {
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
        for _ in 0..1000 {
            let v = self.read32(CSR_GP_CNTRL);
            if v & CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY != 0 {
                let init_done = v & CSR_GP_CNTRL_REG_FLAG_INIT_DONE != 0;
                crate::serial_println!("[WIFI] MAC clock granted (INIT_DONE={}).", init_done);
                return;
            }
            self.delay_us(10);
        }
        crate::serial_println!("[WIFI] Note: MAC clock not granted in time (OK for identity read).");
    }

    // ==================== PHASE 2: DMA TRANSPORT SKELETON ====================
    // Allocate (but do not yet program into the device) the structures the Phase-3 firmware
    // self-load will consume: the context-info block, the RX RBD rings + buffers, and the
    // host-command TX queue. All are <4 GiB physically-contiguous, x86-cache-coherent DMA.

    /// Allocate `pages` of zeroed, <4 GiB, physically-contiguous DMA memory.
    fn alloc_dma(&self, pages: usize) -> Option<DmaRegion> {
        let frame = crate::memory::allocate_contiguous(pages, 4096, true)?;
        let phys = frame.start_address().as_u64();
        let virt = crate::memory::phys_to_virt(phys)?;
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, pages * 4096); }
        Some(DmaRegion { virt, phys, pages })
    }

    pub fn setup_transport(&mut self) {
        if self.mmio_base == 0 { return; }
        crate::serial_println!("[WIFI] Phase 2: allocating DMA transport skeleton...");

        // Mask & clear all interrupts until Phase 3 wires the FW-load cause.
        unsafe {
            self.write32(CSR_INT_MASK, 0x0000_0000);
            self.write32(CSR_INT, 0xFFFF_FFFF);      // write-1-to-clear pending
            self.write32(CSR_FH_INT_STATUS, 0xFFFF_FFFF);
        }

        let mut ok = true;
        let mut log_region = |name: &str, r: &Option<DmaRegion>| {
            match r {
                Some(d) => {
                    let under_4g = d.phys + (d.pages as u64) * 4096 <= 0x1_0000_0000;
                    crate::serial_println!(
                        "[WIFI]   {:<12} phys={:#012x} ({} pg) <4GiB={}",
                        name, d.phys, d.pages, under_4g
                    );
                    if !under_4g { crate::serial_println!("[WIFI]   ^^ WARNING: {} is not <4 GiB!", name); }
                }
                None => crate::serial_println!("[WIFI]   {:<12} ALLOC FAILED", name),
            }
        };

        // Context-info block (one page is ample; the struct is a few hundred bytes).
        self.ctxt_info = self.alloc_dma(1);
        ok &= self.ctxt_info.is_some();
        log_region("ctxt_info", &self.ctxt_info);

        // RX free/used RBD rings. This is a Wireless-AC 9462 (JF, non-HE) → num_rbds =
        // IWL_NUM_RBDS_NON_HE = 512 (cb_size = ilog2(512) = 9). A 512-entry free ring is 512 × __le64
        // = 4 KiB (1 page); we keep 4 pages allocated (harmless over-allocation) so the declared
        // cb_size geometry is always fully DMA-backed regardless of HE/non-HE.
        self.rx_free = self.alloc_dma(4);
        self.rx_used = self.alloc_dma(4);
        ok &= self.rx_free.is_some() && self.rx_used.is_some();
        log_region("rx_free_rbd", &self.rx_free);
        log_region("rx_used_rbd", &self.rx_used);

        // 16 RX buffers of 4 KiB each; record each phys addr into the free-RBD ring.
        self.rx_buffers.clear();
        for i in 0..16usize {
            match self.alloc_dma(1) {
                Some(buf) => {
                    if let Some(free) = self.rx_free {
                        unsafe {
                            // gen2/22000 free-RBD = (buffer_dma | VID). VID is 1-based and the fw
                            // echoes it back in the used-BD ring so we can map a completion to its
                            // buffer. Buffers are 4 KiB-aligned so the low 12 bits are free for it.
                            let slot = (free.virt as *mut u64).add(i);
                            write_volatile(slot, buf.phys | (i as u64 + 1));
                        }
                    }
                    self.rx_buffers.push(buf);
                }
                None => { ok = false; break; }
            }
        }
        crate::serial_println!("[WIFI]   rx_buffers   count={} (16 x 4KiB)", self.rx_buffers.len());

        // Host-command TX queue (gen2). TFD ring = 32 × sizeof(iwl_tfh_tfd)=256 = 8 KiB (2 pg);
        // command backing = 32 × 256B slots = 8 KiB (2 pg); first-TB buffers = 32 × 64B (1 pg).
        self.tx_cmd_ring = self.alloc_dma(2);
        self.tx_cmd_buf = self.alloc_dma(2);
        self.first_tb_bufs = self.alloc_dma(1);
        ok &= self.tx_cmd_ring.is_some() && self.tx_cmd_buf.is_some() && self.first_tb_bufs.is_some();
        log_region("tx_cmd_ring", &self.tx_cmd_ring);
        log_region("tx_cmd_buf", &self.tx_cmd_buf);
        log_region("first_tb", &self.first_tb_bufs);

        if ok {
            crate::serial_println!("[WIFI] Phase 2 OK: transport skeleton allocated (no firmware loaded).");
        } else {
            crate::serial_println!("[WIFI] Phase 2 WARN: one or more DMA allocations failed.");
        }
        // NOTE: intentionally NOT writing CSR_CTXT_INFO_BA here — that kicks firmware self-load,
        // which is Phase 3.
    }

    // ==================== PHASE 3a: FIRMWARE TLV PARSE ====================
    // Pure-logic walk of the bundled .ucode TLV container. Extracts the runtime firmware sections
    // and classifies them CPU1(LMAC)/CPU2(UMAC)/paging via the offset-marker separators, so Phase
    // 3b can copy them into DMA and Phase 3c can hand the device a context-info block. No hardware
    // access here — just parse + log the section map.

    pub fn parse_firmware(&mut self) -> bool {
        let b = FIRMWARE_BLOB;
        // Header: zero(4) magic(4) human_readable[64] ver(4) build(4) ignore(8) = 88 bytes.
        if b.len() < 88 {
            crate::serial_println!("[WIFI] FW: blob too small ({} B).", b.len());
            return false;
        }
        let zero = rd32(b, 0);
        let magic = rd32(b, 4);
        if zero != 0 || magic != IWL_TLV_UCODE_MAGIC {
            crate::serial_println!("[WIFI] FW: bad TLV header (zero={:#x} magic={:#x}).", zero, magic);
            return false;
        }
        self.fw_ver = rd32(b, 72);
        self.fw_build = rd32(b, 76);

        let hr_end = (8..72).find(|&i| b[i] == 0).unwrap_or(72);
        if let Ok(s) = core::str::from_utf8(&b[8..hr_end]) {
            crate::serial_println!("[WIFI] FW: '{}'  ver={:#x} build={}", s, self.fw_ver, self.fw_build);
        }

        self.fw_sections.clear();
        let mut cur_cpu: u8 = 1;
        let mut in_paging = false;
        let mut pos = 88usize;
        let mut tlv_count = 0u32;

        while pos + 8 <= b.len() {
            let ttype = rd32(b, pos);
            let tlen = rd32(b, pos + 4) as usize;
            let doff = pos + 8;
            if doff + tlen > b.len() { break; }
            tlv_count += 1;

            match ttype {
                TLV_NUM_OF_CPU => {
                    if tlen >= 4 { self.num_cpus = rd32(b, doff); }
                }
                TLV_PHY_SKU => {
                    if tlen >= 4 { self.fw_phy_config = rd32(b, doff); }
                }
                TLV_UMAC_DEBUG_ADDRS => {
                    // error_info_addr is the first __le32; strip cache-control high bits.
                    if tlen >= 4 { self.umac_error_addr = rd32(b, doff) & !FW_ADDR_CACHE_CONTROL; }
                }
                TLV_LMAC_DEBUG_ADDRS => {
                    // error_event_table_ptr is the first __le32.
                    if tlen >= 4 { self.lmac_error_addr = rd32(b, doff) & !FW_ADDR_CACHE_CONTROL; }
                }
                TLV_SEC_RT => {
                    if tlen >= 4 {
                        let sec_off = rd32(b, doff);
                        if sec_off == CPU1_CPU2_SEPARATOR {
                            cur_cpu = 2;
                        } else if sec_off == PAGING_SEPARATOR {
                            in_paging = true;
                        } else {
                            self.fw_sections.push(FwSection {
                                offset: sec_off,
                                data_start: doff + 4,
                                data_len: tlen - 4,
                                cpu: cur_cpu,
                                paging: in_paging,
                            });
                        }
                    }
                }
                _ => {}
            }
            // Advance to the next TLV (payloads are padded to a 4-byte boundary).
            pos = doff + ((tlen + 3) & !3);
        }

        let (mut ln, mut lb, mut un, mut ub, mut pn, mut pb) = (0u32, 0usize, 0u32, 0usize, 0u32, 0usize);
        for s in &self.fw_sections {
            if s.paging { pn += 1; pb += s.data_len; }
            else if s.cpu == 2 { un += 1; ub += s.data_len; }
            else { ln += 1; lb += s.data_len; }
        }
        crate::serial_println!("[WIFI] FW: {} TLVs, num_cpus={}, {} runtime sections, phy_config={:#010x}.",
            tlv_count, self.num_cpus, self.fw_sections.len(), self.fw_phy_config);
        crate::serial_println!("[WIFI] FW:   LMAC(cpu1) {} secs / {} B", ln, lb);
        crate::serial_println!("[WIFI] FW:   UMAC(cpu2) {} secs / {} B", un, ub);
        crate::serial_println!("[WIFI] FW:   paging     {} secs / {} B", pn, pb);
        crate::serial_println!("[WIFI] FW:   err-table ptrs: umac={:#010x} lmac={:#010x}",
            self.umac_error_addr, self.lmac_error_addr);
        for (i, s) in self.fw_sections.iter().enumerate() {
            crate::serial_println!("[WIFI] FW:   sec[{}] cpu{} paging={} off={:#010x} len={}",
                i, s.cpu, s.paging as u8, s.offset, s.data_len);
        }

        if self.fw_sections.is_empty() {
            crate::serial_println!("[WIFI] FW: no sections parsed — cannot proceed to Phase 3b.");
            false
        } else {
            crate::serial_println!("[WIFI] FW: Phase 3a OK (section map built).");
            true
        }
    }

    // ==================== PHASE 3b: BUILD iwl_context_info ====================
    // Copy each firmware section into its own <4 GiB DMA block and assemble the 1792-byte
    // iwl_context_info descriptor (Linux iwl-context-info.h, gen2/family-22000 layout). Fills the
    // dram img[] arrays with section DMA addresses, points rbd_cfg/hcmd_cfg at the Phase-2 rings,
    // and sets version/control_flags. Does NOT write CSR_CTXT_INFO_BA — that kick is Phase 3c.

    // iwl_context_info byte offsets (total struct = 1792 B).
    // version{mac_id:le16@0, version:le16@2, size:le16@4}, control{flags:le32@8},
    // rbd_cfg{free@24, used@32, status@40}, hcmd_cfg{qaddr@48, qsize:u8@56},
    // dram{umac_img[64]@192, lmac_img[64]@704, virtual_img[64]@1216}.
    pub fn build_context_info(&mut self) -> bool {
        let ci = match self.ctxt_info { Some(c) => c, None => {
            crate::serial_println!("[WIFI] P3b: no context-info block (Phase 2 failed?).");
            return false;
        }};
        let (rx_free, rx_used, tx_ring) = match (self.rx_free, self.rx_used, self.tx_cmd_ring) {
            (Some(f), Some(u), Some(t)) => (f, u, t),
            _ => { crate::serial_println!("[WIFI] P3b: missing P2 rings."); return false; }
        };
        // RX status write-back region (device writes the closed RBD index here).
        if self.rb_stts.is_none() { self.rb_stts = self.alloc_dma(1); }
        let rb_stts = match self.rb_stts { Some(r) => r, None => {
            crate::serial_println!("[WIFI] P3b: rb_stts alloc failed."); return false;
        }};

        // --- Copy every fw section into DMA, recording its phys into the right img[] array. ---
        self.fw_dma.clear();
        let (mut li, mut ui, mut vi) = (0usize, 0usize, 0usize);
        let mut total_pages = 0usize;
        let n = self.fw_sections.len();
        for idx in 0..n {
            let (ds, dl, cpu, paging) = {
                let s = &self.fw_sections[idx];
                (s.data_start, s.data_len, s.cpu, s.paging)
            };
            let pages = (dl + 4095) / 4096;
            let region = match self.alloc_dma(pages) {
                Some(r) => r,
                None => {
                    crate::serial_println!("[WIFI] P3b: DMA OOM at section {} ({} B).", idx, dl);
                    return false;
                }
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    FIRMWARE_BLOB.as_ptr().add(ds), region.virt as *mut u8, dl,
                );
            }
            total_pages += pages;

            // Choose img array + slot. umac_img@192, lmac_img@704, virtual_img@1216 (64 le64 each).
            let (arr_off, slot) = if paging { let s = (1216usize, vi); vi += 1; s }
                                  else if cpu == 2 { let s = (192usize, ui); ui += 1; s }
                                  else { let s = (704usize, li); li += 1; s };
            if slot < 64 {
                unsafe {
                    write_volatile((ci.virt + (arr_off + slot * 8) as u64) as *mut u64, region.phys);
                }
            } else {
                crate::serial_println!("[WIFI] P3b: WARN >64 sections in one img array (slot {}).", slot);
            }
            self.fw_dma.push(region);
        }

        // --- Fill the fixed context-info fields. ---
        // control_flags = TFD_FORMAT_LONG(0x100) | cb_size<<4 | rb_size<<9  (masks: RB_CB_SIZE=0x00f0,
        // RB_SIZE=0x1e00, from Linux iwl-context-info.h). This device is actually a Wireless-AC 9462
        // (JF radio, crf-id 0x2816) — Wi-Fi 5, NOT the HE AX201 we assumed. Non-HE → num_rbds =
        // IWL_NUM_RBDS_NON_HE = 512, so cb_size = RX_QUEUE_CB_SIZE(512) = ilog2(512) = 9; rb_size =
        // RB_SIZE_4K = 0x4.  0x0100 | (9<<4=0x90) | (0x4<<9=0x800) = 0x0990.
        let control_flags: u32 = 0x0100 | (9u32 << 4) | (0x4u32 << 9); // = 0x0990 (non-HE 9462/JF)
        // cmd_queue_size = TFD_QUEUE_CB_SIZE(32) = ilog2(32)-3 = 2  (actual entries = 8<<2 = 32).
        let cmd_queue_size: u8 = 2;

        unsafe {
            // version: mac_id = low 16 of HW_REV, version = 0, size = struct DWs (1792/4 = 448).
            write_volatile((ci.virt + 0) as *mut u16, (self.hw_rev & 0xFFFF) as u16);
            write_volatile((ci.virt + 2) as *mut u16, 0u16);
            write_volatile((ci.virt + 4) as *mut u16, (1792u16) / 4);
            // control
            write_volatile((ci.virt + 8) as *mut u32, control_flags);
            // rbd_cfg
            write_volatile((ci.virt + 24) as *mut u64, rx_free.phys);
            write_volatile((ci.virt + 32) as *mut u64, rx_used.phys);
            write_volatile((ci.virt + 40) as *mut u64, rb_stts.phys);
            // hcmd_cfg
            write_volatile((ci.virt + 48) as *mut u64, tx_ring.phys);
            write_volatile((ci.virt + 56) as *mut u8, cmd_queue_size);
        }

        crate::serial_println!(
            "[WIFI] P3b: context-info @phys={:#012x} built. lmac={} umac={} paging={} secs, fw DMA={} pages ({} KiB).",
            ci.phys, li, ui, vi, total_pages, total_pages * 4
        );
        crate::serial_println!(
            "[WIFI] P3b: control_flags={:#06x} cmd_qsize={} rbd_free={:#012x} rbd_used={:#012x} rb_stts={:#012x} hcmd={:#012x}",
            control_flags, cmd_queue_size, rx_free.phys, rx_used.phys, rb_stts.phys, tx_ring.phys
        );
        crate::serial_println!("[WIFI] P3b OK: context-info assembled (NOT yet handed to device).");
        true
    }

    // ==================== PHASE 3c: KICK + WAIT FOR ALIVE ====================
    // gen2 device-start for a context-info device: APM init (clock/power), then hand the device
    // the context-info block via CSR_CTXT_INFO_BA and release the fw CPU (UREG_CPU_INIT_RUN). The
    // FIRMWARE self-loads its sections and configures RFH/TFH from the context-info rings — the host
    // does NOT program RFH here (Linux: "Restock will be done at alive, after firmware configured
    // the RFH"). We keep interrupts masked and POLL CSR_INT / rb_stts for the ALIVE notification.

    /// Indirect PRPH register write via the HBUS window (requires MAC access, asserted below).
    unsafe fn write_prph(&self, addr: u32, val: u32) {
        self.write32(HBUS_TARG_PRPH_WADDR, (addr & 0x000F_FFFF) | (3 << 24));
        self.write32(HBUS_TARG_PRPH_WDAT, val);
    }

    /// Indirect PRPH register read via the HBUS window.
    unsafe fn read_prph(&self, addr: u32) -> u32 {
        self.write32(HBUS_TARG_PRPH_RADDR, (addr & 0x000F_FFFF) | (3 << 24));
        self.read32(HBUS_TARG_PRPH_RDAT)
    }

    /// Read a device-SRAM dword via the HBUS_TARG_MEM window (Linux iwl_trans_pcie_read_mem).
    /// Requires MAC access to be held. We reprogram RADDR on every read for simplicity rather
    /// than relying on the hardware post-increment.
    unsafe fn read_mem32(&self, addr: u32) -> u32 {
        self.write32(HBUS_TARG_MEM_RADDR, addr);
        self.read32(HBUS_TARG_MEM_RDAT)
    }

    /// Decode the firmware error table straight out of device SRAM. The pointer came from the
    /// UMAC/LMAC debug-address TLVs (Linux iwl_fwrt_dump_umac_error_log). `valid`!=0 means the fw
    /// actually logged an assert; `error_id` is the specific fault (look up in iwlwifi's
    /// advanced_lookup[]). This is the ground-truth "why did the UMAC assert" that replaces the
    /// PHYRF/FSEQ guessing.
    unsafe fn dump_fw_error_table(&self, label: &str, base: u32) {
        if base == 0 {
            crate::serial_println!("[WIFI] P3c: {} error-table ptr is 0 (no debug-addr TLV?).", label);
            return;
        }
        // Table layout (struct iwl_umac_error_event_table): valid@0 error_id@4 blink1@8 blink2@12
        // ilink1@16 ilink2@20 data1@24 data2@28 data3@32 major@36 minor@40 fp@44 sp@48.
        let mut w = [0u32; 15];
        for (i, slot) in w.iter_mut().enumerate() {
            *slot = self.read_mem32(base + (i as u32) * 4);
        }
        let valid = w[0];
        crate::serial_println!("[WIFI] P3c: {} err-table @{:#010x} valid={:#010x}", label, base, valid);
        if valid == 0 {
            crate::serial_println!("[WIFI] P3c:   (valid=0 — fw did not write an assert here)");
            return;
        }
        crate::serial_println!(
            "[WIFI] P3c:   error_id={:#010x} data1={:#010x} data2={:#010x} data3={:#010x}",
            w[1], w[6], w[7], w[8]);
        crate::serial_println!(
            "[WIFI] P3c:   ucode_ver={}.{} blink={:#x}/{:#x} ilink={:#x}/{:#x} FP={:#010x} SP={:#010x}",
            w[9], w[10], w[2], w[3], w[4], w[5], w[11], w[12]);
    }

    unsafe fn set_bits_prph(&self, addr: u32, bits: u32) {
        let v = self.read_prph(addr);
        self.write_prph(addr, v | bits);
    }
    unsafe fn clear_bits_prph(&self, addr: u32, bits: u32) {
        let v = self.read_prph(addr);
        self.write_prph(addr, v & !bits);
    }

    /// finish_nic_init: set INIT_DONE, poll MAC_CLOCK_READY (~25 ms). Grants device-internal
    /// (PRPH/SRAM) access. Returns whether the clock came ready.
    unsafe fn finish_nic_init(&self) -> bool {
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_INIT_DONE);
        for _ in 0..2500 {
            if self.read32(CSR_GP_CNTRL) & CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY != 0 {
                return true;
            }
            self.delay_us(10);
        }
        false
    }

    /// iwl_trans_pcie_clear_persistence_bit: a (warm) reset can leave PERSISTENCE_BIT set in
    /// HPM_DEBUG, which tells the CNVi to keep its old power state and NOT re-power the RF — the
    /// prime suspect for our FSEQ-error / PHYRF-off assert, since each test is a warm reboot.
    unsafe fn clear_persistence_bit(&self) {
        // Linux only READS WPROT here to gate the clear — it never writes it (writing WPROT=0 was a
        // fabricated step on our side; the fw assert's data3 fingered the WPROT address 0xa04d00).
        let hpm = self.read_prph(HPM_DEBUG);
        let hw_err = (hpm & !1) == 0x5A5A_5A5A || hpm == 0xFFFF_FFFF || hpm == 0xA5A5_A5A1;
        crate::serial_println!("[WIFI] P3c: HPM_DEBUG={:#010x} persistence={} hw_err={}",
            hpm, (hpm & PERSISTENCE_BIT != 0) as u8, hw_err as u8);
        if !hw_err && (hpm & PERSISTENCE_BIT) != 0 {
            // Can only clear persistence if WFPM access isn't locked (PREG_WFPM_ACCESS clear).
            if self.read_prph(PREG_PRPH_WPROT_22000) & PREG_WFPM_ACCESS != 0 {
                crate::serial_println!("[WIFI] P3c: WARN cannot clear persistence (WFPM access locked).");
                return;
            }
            self.write_prph(HPM_DEBUG, hpm & !PERSISTENCE_BIT);
            crate::serial_println!("[WIFI] P3c: cleared PERSISTENCE_BIT in HPM_DEBUG.");
        }
    }

    /// iwl_pcie_gen2_force_power_gating (integrated family-22000 ONLY): configure the CNVi HIPM
    /// power-gating FSM before APM init so the fw's FSEQ can bring the RF to PHYRF_STATE_ON. Ends
    /// with a SW reset + re-take ownership (as Linux does), after which the caller re-inits.
    unsafe fn force_power_gating(&self) {
        // Linux iwl_pcie_gen2_force_power_gating opens with iwl_finish_nic_init — the HPM PRPH
        // writes below need MAC/PRPH access, so establish it here rather than relying on the caller.
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
        self.finish_nic_init();
        self.set_bits_prph(HPM_HIPM_GEN_CFG, HPM_HIPM_GEN_CFG_CR_FORCE_ACTIVE);
        self.delay_us(20);
        self.set_bits_prph(HPM_HIPM_GEN_CFG,
            HPM_HIPM_GEN_CFG_CR_PG_EN | HPM_HIPM_GEN_CFG_CR_SLP_EN);
        self.delay_us(20);
        self.clear_bits_prph(HPM_HIPM_GEN_CFG, HPM_HIPM_GEN_CFG_CR_FORCE_ACTIVE);
        crate::serial_println!("[WIFI] P3c: force_power_gating done, HPM_HIPM_GEN_CFG={:#010x}",
            self.read_prph(HPM_HIPM_GEN_CFG));
        // SW reset + re-take ownership (force_power_gating tail).
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
        self.delay_ms(6);
        self.prepare_card();
    }

    /// Dump firmware secure-boot status + LMAC/UMAC program counters — tells us how far the fw
    /// booted before an early SW_ERR (verified secure-boot? which CPU asserted? PC at death?).
    unsafe fn dump_boot_status(&self) {
        crate::serial_println!(
            "[WIFI] P3c: SB_CPU1={:#010x} SB_CPU2={:#010x} UMAG_SB1={:#010x} UMAG_SB2={:#010x}",
            self.read_prph(SB_CPU_1_STATUS), self.read_prph(SB_CPU_2_STATUS),
            self.read_prph(UMAG_SB_CPU_1_STATUS), self.read_prph(UMAG_SB_CPU_2_STATUS)
        );
        crate::serial_println!(
            "[WIFI] P3c: UMAC_PC={:#010x} LMAC1_PC={:#010x} LMAC2_PC={:#010x}",
            self.read_prph(UREG_UMAC_CURRENT_PC),
            self.read_prph(UREG_LMAC1_CURRENT_PC), self.read_prph(UREG_LMAC2_CURRENT_PC)
        );

        // Extended diagnostics: is the UMAC stuck waiting on the RF companion (PHYRF FSM / FSEQ)?
        let ps_ctl = self.read_prph(WFPM_LMAC1_PS_CTL_RW);
        let phyrf_state = ps_ctl & 0x7; // low bits = FSM current state (5 = PHYRF_STATE_ON)
        crate::serial_println!(
            "[WIFI] P3c: UCODE_LOAD_STATUS={:#010x} UMAG_GEN_HW_STATUS={:#010x} load_token(FSEQ_ALIVE)={:#010x}",
            self.read_prph(UREG_UCODE_LOAD_STATUS), self.read_prph(UMAG_GEN_HW_STATUS),
            self.read_prph(FSEQ_ALIVE_TOKEN)
        );
        crate::serial_println!(
            "[WIFI] P3c: WFPM_PS_CTL={:#010x} PHYRF_FSM_state={} (want {}=ON) WFPM_GP2={:#010x}",
            ps_ctl, phyrf_state, WFPM_PHYRF_STATE_ON, self.read_prph(WFPM_GP2)
        );
        crate::serial_println!(
            "[WIFI] P3c: FSEQ_ERROR_CODE={:#010x} CNVI_AUX_MISC_CHIP={:#010x}",
            self.read_prph(FSEQ_ERROR_CODE), self.read_prph(CNVI_AUX_MISC_CHIP)
        );

        // Ground truth: decode the fw's own error record from SRAM. UMAC ran (PC advanced) so the
        // UMAC table is the one to trust; LMAC never started (PC=0) but dump it too if present.
        self.dump_fw_error_table("UMAC", self.umac_error_addr);
        self.dump_fw_error_table("LMAC", self.lmac_error_addr);
    }

    /// iwl_pcie_get_crf_id: enable WFPM peripheral-register access, then read the CRF (RF companion),
    /// CNVi, and WFPM ids. Two purposes: (1) DIAGNOSTIC — crf-id should read 0x2816 (as Fedora does on
    /// this exact hardware) if the JF radio companion is powered and visible to us; a 0/garbage read
    /// means the RF companion isn't up on our side. (2) FUNCTIONAL — enabling WFPM is an early step
    /// iwlwifi always does and is a candidate prerequisite for the RF power-up the fw is failing.
    unsafe fn get_crf_id(&self) {
        let mut v = self.read_prph(WFPM_CTRL_REG);
        v |= ENABLE_WFPM;
        self.write_prph(WFPM_CTRL_REG, v);
        let crf = self.read_prph(SD_REG_VER);
        let cnv = self.read_prph(CNVI_AUX_MISC_CHIP);
        let wfpm = self.read_prph(WFPM_OTP_CFG1_ADDR);
        crate::serial_println!(
            "[WIFI] P3c: WFPM enabled -> crf-id={:#x} cnv-id={:#x} wfpm-id={:#x} (Fedora: 0x2816 / 0x20000302 / 0x80000000)",
            crf, cnv, wfpm
        );
    }

    /// P4: read + parse the firmware's ALIVE notification out of the RX ring. gen2/22000 RX
    /// completion: rb_stts.closed_rb_num (le16 @0) = how many RBs the fw has filled; used_bd[i]
    /// (le32) low-12 bits = the 1-based VID we encoded into the free-RBD, indexing our RX buffers.
    /// The buffer holds an iwl_rx_packet: len_n_flags@0, cmd@4, group@5, seq@6, data@8. The ALIVE
    /// notification is cmd=0x1/group=0; its payload is iwl_alive_ntf (status@0=0xCAFE ok, lmac[2]
    /// @4 [48 B each], umac @100). Returns true iff a valid status==0xCAFE ALIVE ntf was parsed.
    /// Wait up to `timeout_ms` for the next RX completion, consume it, and return its buffer virt
    /// address (the start of the iwl_rx_packet). gen2/22000: rb_stts.closed_rb_num (le16@0, 12-bit)
    /// is the fw's running fill count; used_bd[slot] (le32) low-12 bits = the 1-based VID we posted.
    /// Advances rx_read_ptr by one. Returns None on timeout.
    unsafe fn poll_next_rx(&mut self, timeout_ms: u32) -> Option<u64> {
        let (rb_stts, used) = match (self.rb_stts, self.rx_used) {
            (Some(r), Some(u)) => (r, u),
            _ => return None,
        };
        for _ in 0..timeout_ms {
            let closed = (read_volatile(rb_stts.virt as *const u16) & 0x0FFF) as u32;
            if closed != self.rx_read_ptr {
                let slot = (self.rx_read_ptr & 0x1FF) as usize; // 512-entry used ring
                let ub = read_volatile((used.virt as *const u32).add(slot));
                let vid = (ub & 0x0FFF) as usize;
                self.rx_read_ptr = self.rx_read_ptr.wrapping_add(1);
                let buf = if vid >= 1 && vid <= self.rx_buffers.len() {
                    self.rx_buffers[vid - 1]
                } else if !self.rx_buffers.is_empty() {
                    crate::serial_println!("[WIFI] rx: VID {} out of range — using buffer[0].", vid);
                    self.rx_buffers[0]
                } else {
                    return None;
                };
                return Some(buf.virt);
            }
            self.delay_ms(1);
        }
        None
    }

    /// P4: consume + parse the firmware's ALIVE notification from the RX ring. cmd=0x1/group=0;
    /// payload iwl_alive_ntf: status le16@0 (0xCAFE=ok), lmac_data[0]@4 (48B), umac_data@100,
    /// sku_id@116 (3×le32 — nonzero ⇒ PNVM required; zero ⇒ skipped). Returns true iff status ok.
    unsafe fn read_alive_notification(&mut self) -> bool {
        let v = match self.poll_next_rx(300) {
            Some(v) => v,
            None => {
                crate::serial_println!("[WIFI] P4: fw delivered no RX (closed_rb_num stayed 0).");
                return false;
            }
        };
        let p = v as *const u8;

        let mut hx = [0u8; 32];
        for k in 0..32 { hx[k] = read_volatile(p.add(k)); }
        crate::serial_println!("[WIFI] P4: alive rxb head: {:02x?}", &hx[..]);

        let cmd = read_volatile(p.add(4));
        let group = read_volatile(p.add(5));
        if !(cmd == 0x01 && group == 0x00) {
            crate::serial_println!("[WIFI] P4: first RX is not the ALIVE ntf (cmd={:#04x} grp={:#04x}).",
                cmd, group);
            return false;
        }

        // iwl_alive_ntf payload begins at data (packet offset 8).
        let a = p.add(8);
        let rd16 = |off: usize| read_volatile(a.add(off) as *const u16);
        let rd32 = |off: usize| read_volatile(a.add(off) as *const u32);
        let status = rd16(0);
        let lmac_major = rd32(4);   // lmac_data[0].ucode_major @ payload+4
        let lmac_minor = rd32(8);
        let lmac_err   = rd32(20);  // lmac_data[0].dbg_ptrs.error_event_table_ptr @ payload+4+16
        let umac_major = rd32(100); // umac_data.umac_major @ payload+100
        let umac_minor = rd32(104);
        let umac_err   = rd32(108); // umac_data.dbg_ptrs.error_info_addr @ payload+108
        // Stash the runtime error-table pointers so a post-alive assert can be decoded.
        self.alive_umac_err = umac_err;
        self.alive_lmac_err = lmac_err;
        let (sku0, sku1, sku2) = (rd32(116), rd32(120), rd32(124)); // iwl_sku_id @payload+116
        crate::serial_println!("[WIFI] P4: *** ALIVE ntf *** status={:#06x} ({})",
            status, if status == 0xCAFE { "OK" } else { "NOT-OK" });
        crate::serial_println!("[WIFI] P4:   LMAC ucode {}.{} err_table={:#010x}",
            lmac_major, lmac_minor, lmac_err);
        crate::serial_println!("[WIFI] P4:   UMAC ucode {}.{} err_info={:#010x}",
            umac_major, umac_minor, umac_err);
        crate::serial_println!("[WIFI] P4:   sku_id=[{:#010x},{:#010x},{:#010x}] -> PNVM {}",
            sku0, sku1, sku2,
            if (sku0 | sku1 | sku2) == 0 { "SKIPPED (zero sku)" } else { "REQUIRED" });
        status == 0xCAFE
    }

    /// Gen2 host-command enqueue (Linux iwl_pcie_gen2_enqueue_hcmd, simplified for one-at-a-time
    /// synchronous use). Builds the command bytes (8-byte iwl_cmd_header_wide + payload) in a cmd-buf
    /// slot, copies the first ≤20 bytes into a dedicated first-TB buffer (hw requires TB0 there),
    /// assembles the iwl_tfh_tfd (TB0 + optional TB1 for the rest), advances the producer index and
    /// rings HBUS_TARG_WRPTR. No byte-count table (command queue doesn't use one). Does NOT wait.
    unsafe fn send_host_cmd(&mut self, group: u8, cmd: u8, payload: &[u8]) -> bool {
        let (ring, buf, ftb) = match (self.tx_cmd_ring, self.tx_cmd_buf, self.first_tb_bufs) {
            (Some(r), Some(b), Some(f)) => (r, b, f),
            _ => { crate::serial_println!("[WIFI] cmd: TX rings missing."); return false; }
        };
        let wptr = self.cmd_write_ptr;
        let idx = (wptr & 0x1F) as usize;   // 32-entry command queue
        let slot = 256usize;                // per-command backing-slot stride
        let total = 8 + payload.len();
        if total > slot {
            crate::serial_println!("[WIFI] cmd: command too large ({} B).", total);
            return false;
        }

        // --- Command bytes (iwl_cmd_header_wide + payload) in the cmd-buf slot. ---
        let cb = (buf.virt + (idx * slot) as u64) as *mut u8;
        core::ptr::write_bytes(cb, 0, slot);
        write_volatile(cb.add(0), cmd);       // hdr_wide.cmd
        write_volatile(cb.add(1), group);     // hdr_wide.group_id
        let seq: u16 = (((CMD_QUEUE_ID as u16) & 0x1F) << 8) | (wptr as u16 & 0xFF); // QUEUE|INDEX
        write_volatile(cb.add(2) as *mut u16, seq);                 // hdr_wide.sequence
        write_volatile(cb.add(4) as *mut u16, payload.len() as u16); // hdr_wide.length (payload)
        // reserved@6, version@7 stay 0
        for (i, &b) in payload.iter().enumerate() { write_volatile(cb.add(8 + i), b); }

        // --- TB0: the first ≤20 bytes into the dedicated first-TB buffer. ---
        let tb0 = core::cmp::min(total, IWL_FIRST_TB_SIZE);
        let ftb_v = (ftb.virt + (idx * 64) as u64) as *mut u8;
        core::ptr::copy_nonoverlapping(cb, ftb_v, tb0);
        let ftb_p = ftb.phys + (idx * 64) as u64;

        // --- Build the TFD (iwl_tfh_tfd): num_tbs le16@0, then tbs[] {tb_len le16, addr le64}. ---
        let tfd = (ring.virt + (idx * TFH_TFD_SIZE) as u64) as *mut u8;
        core::ptr::write_bytes(tfd, 0, TFH_TFD_SIZE);
        let mut ntb = 0usize;
        // TB0 -> first-TB buffer.
        {
            let t = tfd.add(2 + ntb * 10);
            write_volatile(t as *mut u16, tb0 as u16);
            let ab = ftb_p.to_le_bytes();
            for k in 0..8 { write_volatile(t.add(2 + k), ab[k]); }
            ntb += 1;
        }
        // TB1 -> the remainder of the command (if any) directly out of the cmd-buf slot.
        if total > tb0 {
            let rest_phys = buf.phys + (idx * slot) as u64 + tb0 as u64;
            let t = tfd.add(2 + ntb * 10);
            write_volatile(t as *mut u16, (total - tb0) as u16);
            let ab = rest_phys.to_le_bytes();
            for k in 0..8 { write_volatile(t.add(2 + k), ab[k]); }
            ntb += 1;
        }
        write_volatile(tfd as *mut u16, ntb as u16); // num_tbs

        // --- Advance producer index + ring the doorbell (write_ptr | qid<<16). ---
        self.cmd_write_ptr = wptr.wrapping_add(1) & 0xFFFF;
        let door = self.cmd_write_ptr | (CMD_QUEUE_ID << 16);
        self.write32(HBUS_TARG_WRPTR, door);
        crate::serial_println!(
            "[WIFI] cmd: grp={:#04x} cmd={:#04x} len={} seq={:#06x} idx={} tbs={} door={:#x}",
            group, cmd, payload.len(), seq, idx, ntb, door);
        true
    }

    /// Read the base HW MAC address from CSR (unified-fw devices don't return it in NVM_GET_INFO).
    /// STRAP registers first; fall back to OTP if STRAP is empty/multicast. Byte order per Linux
    /// iwl_flip_hw_address: mac = [a0>>24, a0>>16, a0>>8, a0, a1>>8, a1].
    unsafe fn read_mac_from_csr(&mut self) {
        let flip = |a: u32, b: u32| [
            (a >> 24) as u8, (a >> 16) as u8, (a >> 8) as u8, a as u8,
            (b >> 8) as u8, b as u8,
        ];
        let mut mac = flip(self.read32(CSR_MAC_ADDR0_STRAP), self.read32(CSR_MAC_ADDR1_STRAP));
        let mut src = "strap";
        if mac.iter().all(|&x| x == 0) || (mac[0] & 0x01) != 0 {
            mac = flip(self.read32(CSR_MAC_ADDR0_OTP), self.read32(CSR_MAC_ADDR1_OTP));
            src = "otp";
        }
        self.mac_addr = mac;
        crate::serial_println!("[WIFI] P4: MAC ({}) = {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            src, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    }

    /// P4b: post-ALIVE bring-up. Read the real MAC from CSR, then exercise the host-command TX path
    /// by sending NVM_GET_INFO and consuming its response (proves TX end-to-end + yields NVM info).
    /// Consume RX packets until one with (group_id, cmd) matches, logging each. Returns its buffer
    /// virt on match; None on timeout or a firmware SW_ERR (checked between packets).
    unsafe fn wait_for_rx(&mut self, want_grp: u8, want_cmd: u8, per_pkt_ms: u32) -> Option<u64> {
        for _ in 0..64 {
            match self.poll_next_rx(per_pkt_ms) {
                Some(v) => {
                    let c = read_volatile((v as *const u8).add(4));
                    let g = read_volatile((v as *const u8).add(5));
                    crate::serial_println!("[WIFI] P4b: rx cmd={:#04x} grp={:#04x}", c, g);
                    if c == want_cmd && g == want_grp { return Some(v); }
                }
                None => {
                    if self.read32(CSR_INT) & INT_BIT_SW_ERR != 0 {
                        crate::serial_println!("[WIFI] P4b: SW_ERR while waiting for cmd={:#04x} grp={:#04x}",
                            want_cmd, want_grp);
                    } else {
                        crate::serial_println!("[WIFI] P4b: timeout waiting for cmd={:#04x} grp={:#04x}",
                            want_cmd, want_grp);
                    }
                    return None;
                }
            }
        }
        None
    }

    /// Dump interrupt state + the firmware error tables (called when the fw asserts / goes silent).
    unsafe fn report_fw_error(&self) {
        crate::serial_println!("[WIFI] P4b: CSR_INT={:#010x} FH_INT={:#010x}",
            self.read32(CSR_INT), self.read32(CSR_FH_INT_STATUS));
        self.dump_fw_error_table("UMAC", self.alive_umac_err);
        self.dump_fw_error_table("LMAC", self.alive_lmac_err);
    }

    /// P4b: post-ALIVE unified-fw init handshake, then NVM_GET_INFO.
    ///   1. INIT_EXTENDED_CFG_CMD (init_flags = BIT(IWL_INIT_NVM))  — "NVM-access commands follow"
    ///   2. NVM_ACCESS_COMPLETE                                     — finish NVM phase
    ///   3. wait INIT_COMPLETE_NOTIF (cmd 0x04, group 0x00)         — fw init done
    ///   4. NVM_GET_INFO -> parse (version/channels/chains). PHY_CONFIGURATION_CMD is skipped
    ///      (unified fw). MAC came from CSR already.
    unsafe fn bring_up_after_alive(&mut self) {
        self.read_mac_from_csr();

        crate::serial_println!("[WIFI] P4b: init handshake (INIT_EXTENDED_CFG + NVM_ACCESS_COMPLETE)...");
        self.send_host_cmd(SYSTEM_GROUP, INIT_EXTENDED_CFG_CMD, &IWL_INIT_NVM_FLAG.to_le_bytes());
        self.send_host_cmd(REGULATORY_NVM_GROUP, NVM_ACCESS_COMPLETE, &[0u8; 4]);

        if self.wait_for_rx(0x00, INIT_COMPLETE_NOTIF, 300).is_none() {
            crate::serial_println!("[WIFI] P4b: no INIT_COMPLETE — init handshake failed.");
            self.report_fw_error();
            return;
        }
        crate::serial_println!("[WIFI] P4b: *** INIT_COMPLETE_NOTIF *** — firmware init complete.");

        crate::serial_println!("[WIFI] P4b: sending NVM_GET_INFO...");
        self.send_host_cmd(REGULATORY_NVM_GROUP, NVM_GET_INFO, &[0u8; 4]);
        match self.wait_for_rx(REGULATORY_NVM_GROUP, NVM_GET_INFO, 300) {
            Some(v) => self.parse_nvm_info(v),
            None => {
                crate::serial_println!("[WIFI] P4b: no NVM_GET_INFO response.");
                self.report_fw_error();
            }
        }
    }

    /// Parse iwl_nvm_get_info_rsp: general{flags@0, nvm_version le16@4, board@6, n_hw_addrs@7},
    /// mac_sku{flags@8}, phy_sku{tx_chains@12, rx_chains@16}, regulatory{lar@20, n_channels@24}.
    unsafe fn parse_nvm_info(&self, v: u64) {
        let p = (v as *const u8).add(8); // payload after the 8-byte iwl_rx_packet header
        let rd16 = |o: usize| read_volatile(p.add(o) as *const u16);
        let rd32 = |o: usize| read_volatile(p.add(o) as *const u32);
        let flags = rd32(0);
        let nvm_ver = rd16(4);
        let board = read_volatile(p.add(6));
        let n_hw = read_volatile(p.add(7));
        let sku_flags = rd32(8);
        let tx_chains = rd32(12);
        let rx_chains = rd32(16);
        let lar = rd32(20);
        let n_ch = rd32(24);
        crate::serial_println!(
            "[WIFI] P4b: *** NVM_GET_INFO rsp *** flags={:#x} nvm_ver={:#06x} board={} n_hw_addrs={}",
            flags, nvm_ver, board, n_hw);
        crate::serial_println!(
            "[WIFI] P4b:   sku_flags={:#010x} tx_chains={:#x} rx_chains={:#x} lar={} n_channels={}",
            sku_flags, tx_chains, rx_chains, lar, n_ch);
        crate::serial_println!("[WIFI] P4b: host-command TX path VERIFIED (fw answered the NVM query).");
    }

    pub fn start_firmware(&mut self) -> bool {
        let ci = match self.ctxt_info { Some(c) => c, None => {
            crate::serial_println!("[WIFI] P3c: no context-info — aborting.");
            return false;
        }};
        crate::serial_println!("[WIFI] P3c: starting firmware (gen2 context-info self-load)...");

        unsafe {
            // ============ start_hw (Linux _iwl_trans_pcie_start_hw), integrated family-22000 ============
            // Exact Linux order: prepare -> clear_persistence -> sw_reset -> force_power_gating ->
            // apm_init. This leaves the CNVi power domain in the state the fw's FSEQ needs to bring
            // the RF to PHYRF_STATE_ON; wrong order is a candidate for FSEQ_ERROR_CODE=3 / PHYRF=0.
            self.prepare_card();
            self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
            self.finish_nic_init();                // clock up so read_prph(HPM_DEBUG) is valid
            self.clear_persistence_bit();          // clear stale warm-reboot persistence
            // sw_reset(true): SW reset then re-take ownership, BEFORE force_power_gating (was missing).
            self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
            self.delay_ms(6);
            self.prepare_card();
            self.force_power_gating();             // finish_nic_init + HPM PG dance + sw_reset + re-prepare
            // apm_init: L0s/L1 chicken bit, FH wait threshold, HAP-wake, then finish_nic_init. (apm_config
            // = PCIe L1/LTR-enable is handled via the HPM LTR path below; APMG is not supported on 22000.)
            self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
            self.set_bit(CSR_GIO_CHICKEN_BITS, GIO_CHICKEN_L1A_NO_L0S_RX);
            self.write32(CSR_DBG_HPET_MEM_REG, DBG_HPET_MEM_REG_VAL);
            self.set_bit(CSR_HW_IF_CONFIG_REG, HW_IF_CONFIG_HAP_WAKE_L1A);
            self.finish_nic_init();

            // ============ start_fw (Linux iwl_trans_pcie_gen2_start_fw) ============
            self.prepare_card();                   // start_fw re-prepares the card
            // Clear pending INT + rfkill handshake, then clear INT again — Linux does this BEFORE
            // gen2_nic_init (i.e. before the INIT_DONE below), not after nic_config.
            self.write32(CSR_INT, 0xFFFF_FFFF);
            self.write32(CSR_UCODE_DRV_GP1_CLR, UCODE_SW_BIT_RFKILL);
            self.write32(CSR_UCODE_DRV_GP1_CLR, UCODE_DRV_GP1_BIT_CMD_BLK);
            self.write32(CSR_INT, 0xFFFF_FFFF);

            // gen2_nic_init -> gen2_apm_init: set INIT_DONE, poll MAC_CLOCK_READY (~25 ms).
            self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
            let clock_ready = self.finish_nic_init();
            crate::serial_println!("[WIFI] P3c: gen2_apm_init clock_ready={} GP_CNTRL={:#010x}",
                clock_ready, self.read32(CSR_GP_CNTRL));

            // iwl_pcie_get_crf_id: enable WFPM peripheral access + read the RF-companion id. Done here
            // (after the last reset) so ENABLE_WFPM persists into fw boot. crf-id != 0x2816 would mean
            // the JF radio companion isn't powered/visible to us — the likely cause of PHYRF stuck at 0.
            self.get_crf_id();

            // --- op_mode_nic_config: program radio type/step/dash into CSR_HW_IF_CONFIG_REG so the
            // UMAC knows which PHY is attached (Linux iwl_mvm_nic_config, run inside gen2_nic_init
            // BEFORE the ctxt-info kick). radio_* come from the fw's PHY_SKU TLV; MAC step/dash from
            // HW_REV. Leaving these zero is a candidate for the deterministic early UMAC assert.
            let phy = self.fw_phy_config;
            let radio_type = (phy >> FW_PHY_CFG_RADIO_TYPE_POS) & 0x3;
            let radio_step = (phy >> FW_PHY_CFG_RADIO_STEP_POS) & 0x3;
            let radio_dash = (phy >> FW_PHY_CFG_RADIO_DASH_POS) & 0x3;
            let reg_val = (self.hw_rev & HW_IF_CONFIG_MSK_MAC_STEP_DASH)
                | (radio_type << HW_IF_CONFIG_POS_PHY_TYPE)
                | (radio_step << HW_IF_CONFIG_POS_PHY_STEP)
                | (radio_dash << HW_IF_CONFIG_POS_PHY_DASH);
            let msk = HW_IF_CONFIG_MSK_MAC_STEP_DASH | HW_IF_CONFIG_MSK_PHY_TYPE
                | HW_IF_CONFIG_MSK_PHY_STEP | HW_IF_CONFIG_MSK_PHY_DASH;
            self.set_bits_mask(CSR_HW_IF_CONFIG_REG, msk, reg_val);
            crate::serial_println!(
                "[WIFI] P3c: nic_config radio type={:#x} step={:#x} dash={:#x} -> HW_IF_CONFIG={:#010x}",
                radio_type, radio_step, radio_dash, self.read32(CSR_HW_IF_CONFIG_REG)
            );

            // rx_init (gen2): the only pre-kick RX register is INT_COALESCING (u8) — the fw
            // self-configures RFH/TFH from the context-info rbd_cfg/hcmd_cfg. Then enable shadow regs.
            write_volatile((self.mmio_base + CSR_INT_COALESCING as u64) as *mut u8, INT_COALESCING_DEF as u8);
            self.write32(CSR_MAC_SHADOW_REG_CTRL, MAC_SHADOW_REG_CTRL_VAL);

            // enable_fw_load_int_ctx_info (non-MSI-X): unmask ONLY ALIVE + FH_RX before the kick so
            // the fw-load handshake interrupt causes are armed (Linux does this inside ctxt_info_init
            // right before the CSR_CTXT_INFO_BA write). We still poll CSR_INT, but arm the mask to
            // match Linux exactly in case the HW gates cause latching on the enable.
            self.write32(CSR_INT_MASK, CSR_INT_BIT_MASK_FW_LOAD);

            // --- THE KICK: hand the device the context-info block (64-bit BA). ---
            self.write32(CSR_CTXT_INFO_BA, (ci.phys & 0xFFFF_FFFF) as u32);
            self.write32(CSR_CTXT_INFO_BA + 4, (ci.phys >> 32) as u32);
            crate::serial_println!("[WIFI] P3c: CSR_CTXT_INFO_BA <= {:#012x}", ci.phys);

            // --- Configure LTR (integrated CNVi path) BEFORE releasing the fw CPU. ---
            // iwl_pcie_set_ltr: missing this makes the fw assert early during boot on integrated
            // devices. This is integrated, so use the HPM PRPH path (not CSR_LTR_LONG_VAL_AD).
            self.write_prph(HPM_MAC_LTR_CSR, HPM_MAC_LRT_ENABLE_ALL);
            self.write_prph(HPM_UMAC_LTR, LTR_VAL);
            crate::serial_println!("[WIFI] P3c: LTR configured (HPM integrated path, val={:#010x}).", LTR_VAL);

            // NOTE: do NOT re-force HPM_HIPM_GEN_CFG_CR_FORCE_ACTIVE here. Upstream does nothing
            // between set_ltr and CPU_INIT_RUN; force_power_gating() already left the HPM power-
            // gating FSM in the state the fw's FSEQ needs (and deliberately CLEARED force-active).
            // Pinning the power domain active here blocks the CNVi power transitions FSEQ requires
            // to bring the RF to PHYRF_STATE_ON — the likely cause of FSEQ_ERROR_CODE=3 / PHYRF=0.

            // --- Release the firmware CPU. ---
            self.write_prph(UREG_CPU_INIT_RUN, 1);
            crate::serial_println!("[WIFI] P3c: UREG_CPU_INIT_RUN=1 — fw released. Waiting for ALIVE (2s)...");

            // --- Poll CSR_INT / FH / rb_stts for up to ~2 s. ---
            let rb_stts_virt = self.rb_stts.map(|r| r.virt).unwrap_or(0);
            let mut last_int = 0u32;
            let mut outcome = 0u32; // 0=timeout, 1=alive-ish, 2=error
            for i in 0..2000u32 {
                let int = self.read32(CSR_INT);
                if int != last_int {
                    crate::serial_println!("[WIFI] P3c: @{}ms CSR_INT={:#010x} FH_INT={:#010x}",
                        i, int, self.read32(CSR_FH_INT_STATUS));
                    last_int = int;
                }
                if int & (INT_BIT_SW_ERR | INT_BIT_HW_ERR) != 0 {
                    crate::serial_println!("[WIFI] P3c: *** FW ERROR *** CSR_INT={:#010x}", int);
                    outcome = 2; break;
                }
                if int & (INT_BIT_ALIVE | INT_BIT_SW_RX | INT_BIT_FH_RX) != 0 {
                    crate::serial_println!("[WIFI] P3c: *** ALIVE-class interrupt *** CSR_INT={:#010x}", int);
                    outcome = 1; break;
                }
                if int & INT_BIT_RF_KILL != 0 {
                    crate::serial_println!("[WIFI] P3c: RF_KILL asserted (hw/sw wifi switch off?).");
                }
                if rb_stts_virt != 0 {
                    let stts = read_volatile(rb_stts_virt as *const u32);
                    if stts != 0 {
                        crate::serial_println!("[WIFI] P3c: *** rb_stts={:#010x} @{}ms — device delivered RX ***", stts, i);
                        outcome = 1; break;
                    }
                }
                self.delay_ms(1);
            }

            match outcome {
                1 => {
                    crate::serial_println!("[WIFI] P3c: firmware initialized (alive-class int). Reading ALIVE ntf (P4)...");
                    // RESTOCK (gen2 context-info, at alive): the fw configured RFH during init but its
                    // free-RBD write pointer is still 0, so it sees no buffers to deliver into. Ring the
                    // free-RBD doorbell to publish our posted RBDs (write_actual = round_down(count,8)).
                    let widx = (self.rx_buffers.len() & !7) as u32;
                    self.write32(RFH_Q0_FRBDCB_WIDX_TRG, widx);
                    crate::serial_println!("[WIFI] P4: rang free-RBD doorbell WIDX_TRG <= {}.", widx);
                    // poll_next_rx waits for the fw to DMA the ALIVE notification into the RX ring.
                    if self.read_alive_notification() {
                        crate::serial_println!("[WIFI] P4: firmware ALIVE confirmed.");
                        self.bring_up_after_alive(); // P4b: MAC + NVM_GET_INFO via host command
                    } else {
                        crate::serial_println!("[WIFI] P4: fw initialized but ALIVE ntf not parsed (see rxb dump).");
                    }
                    true
                }
                2 => {
                    crate::serial_println!("[WIFI] P3c: firmware signalled an error during boot.");
                    self.dump_boot_status();
                    false
                }
                _ => {
                    crate::serial_println!(
                        "[WIFI] P3c: NO ALIVE in 2s. CSR_INT={:#010x} FH_INT={:#010x} GP_CNTRL={:#010x} RESET={:#010x}",
                        self.read32(CSR_INT), self.read32(CSR_FH_INT_STATUS),
                        self.read32(CSR_GP_CNTRL), self.read32(CSR_RESET)
                    );
                    self.dump_boot_status();
                    false
                }
            }
        }
    }
}
