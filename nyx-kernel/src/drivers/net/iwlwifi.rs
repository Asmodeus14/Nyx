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
const TLV_CMD_VERSIONS: u32 = 48; // array of iwl_fw_cmd_version{cmd,group,cmd_ver,notif_ver} (4B each)
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

// P6 association. This fw is MLD (capa MLD_API_SUPPORT=1) → contexts use MAC_CONF_GROUP(0x03):
// MAC_CONFIG=0x08, LINK_CONFIG=0x09, STA_CONFIG=0x0A, AUX_STA=0x0B, SESSION_PROTECTION=0x05. MLD
// uses flat fw ids (no color). PHY_CONTEXT_CMD stays in LEGACY group 0x00 but wants the newer struct.
const FW_CTXT_ACTION_ADD: u8 = 1;   // enum iwl_ctxt_action: INVALID=0, ADD=1, MODIFY=2, REMOVE=3
#[allow(dead_code)]
const MAC_CONF_GROUP:     u8 = 0x03; // used by the P6 MAC_CONFIG/LINK_CONFIG/STA_CONFIG commands
const DATA_PATH_GROUP:    u8 = 0x05; // SCD_QUEUE_CONFIG_CMD=0x17, etc.
const IWL_MGMT_TID:       u8 = 15;   // management-frame TID for the AP station's TX queue
const TARGET_SSID: &[u8] = b"dipti";
// P6c will derive a PMK per candidate and keep whichever makes the EAPOL MIC verify (the only
// definitive test of the passphrase). Exact case is unknown, so we try both.
#[allow(dead_code)]
const TARGET_PSK_CANDIDATES: &[&[u8]] = &[b"dipti1978", b"Dipti1978"];
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

/// One discovered network (deduped by BSSID) from a scan, kept so `connect` can look up the target
/// AP's channel/band/BSSID by SSID name.
#[derive(Clone, Copy)]
pub struct ScanResult {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub band: u8,        // PHY_BAND_24=1 (2.4GHz), PHY_BAND_5=0 (5GHz)
    pub beacon_int: u16, // beacon interval in TU (from the beacon fixed body)
}

// ===================== WPA2 crypto primitives (P6c 4-way handshake) =====================
// From-scratch SHA1 / HMAC-SHA1 / PBKDF2-SHA1 / IEEE-802.11 PRF. no_std, alloc-only.

fn sha1_block(h: &mut [u32; 5], blk: &[u8]) {
    let mut w = [0u32; 80];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([blk[i * 4], blk[i * 4 + 1], blk[i * 4 + 2], blk[i * 4 + 3]]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
    for (i, &wi) in w.iter().enumerate() {
        let (f, k) = if i < 20 { ((b & c) | ((!b) & d), 0x5A82_7999u32) }
            else if i < 40 { (b ^ c ^ d, 0x6ED9_EBA1) }
            else if i < 60 { ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC) }
            else { (b ^ c ^ d, 0xCA62_C1D6) };
        let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
        e = d; d = c; c = b.rotate_left(30); b = a; a = t;
    }
    h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d); h[4] = h[4].wrapping_add(e);
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = [0x6745_2301u32, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let ml = (data.len() as u64) * 8;
    let mut i = 0;
    while i + 64 <= data.len() { sha1_block(&mut h, &data[i..i + 64]); i += 64; }
    let rem = data.len() - i;
    let mut last = [0u8; 128];
    last[..rem].copy_from_slice(&data[i..]);
    last[rem] = 0x80;
    let total = if rem + 1 + 8 <= 64 { 64 } else { 128 };
    last[total - 8..total].copy_from_slice(&ml.to_be_bytes());
    sha1_block(&mut h, &last[..64]);
    if total == 128 { sha1_block(&mut h, &last[64..128]); }
    let mut out = [0u8; 20];
    for j in 0..5 { out[j * 4..j * 4 + 4].copy_from_slice(&h[j].to_be_bytes()); }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 { k[..20].copy_from_slice(&sha1(key)); }
    else { k[..key.len()].copy_from_slice(key); }
    let mut inner = alloc::vec::Vec::with_capacity(64 + msg.len());
    let mut opad = [0x5cu8; 64];
    for i in 0..64 { inner.push(0x36 ^ k[i]); opad[i] ^= k[i]; }
    inner.extend_from_slice(msg);
    let ih = sha1(&inner);
    let mut outer = [0u8; 84];
    outer[..64].copy_from_slice(&opad);
    outer[64..].copy_from_slice(&ih);
    sha1(&outer)
}

fn pbkdf2_sha1(password: &[u8], salt: &[u8], iters: u32, dklen: usize) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    let mut bi = 1u32;
    while out.len() < dklen {
        let mut si = alloc::vec::Vec::with_capacity(salt.len() + 4);
        si.extend_from_slice(salt);
        si.extend_from_slice(&bi.to_be_bytes());
        let mut u = hmac_sha1(password, &si);
        let mut t = u;
        for _ in 1..iters {
            u = hmac_sha1(password, &u);
            for k in 0..20 { t[k] ^= u[k]; }
        }
        out.extend_from_slice(&t);
        bi += 1;
    }
    out.truncate(dklen);
    out
}

/// IEEE 802.11 PRF: repeatedly HMAC-SHA1(key, label || 0x00 || data || counter).
fn sha1_prf(key: &[u8], label: &[u8], data: &[u8], out_len: usize) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    let mut ctr = 0u8;
    while out.len() < out_len {
        let mut buf = alloc::vec::Vec::with_capacity(label.len() + 2 + data.len());
        buf.extend_from_slice(label);
        buf.push(0x00);
        buf.extend_from_slice(data);
        buf.push(ctr);
        out.extend_from_slice(&hmac_sha1(key, &buf));
        ctr += 1;
    }
    out.truncate(out_len);
    out
}

/// Self-test the crypto against known vectors (RFC 3174/2202 + our offline-computed PMK). Logs pass/fail.
fn wpa_crypto_selftest() {
    let s = sha1(b"abc");
    let sha_ok = s[..4] == [0xa9, 0x99, 0x3e, 0x36];
    let m = hmac_sha1(&[0x0b; 20], b"Hi There");
    let hmac_ok = m[..4] == [0xb6, 0x17, 0x31, 0x86];
    let pmk = pbkdf2_sha1(b"dipti1978", b"dipti", 4096, 32);
    let pmk_ok = pmk[..4] == [0x73, 0x04, 0x56, 0x57];
    crate::serial_println!("[WIFI] P6c: crypto self-test — sha1={} hmac={} pbkdf2/PMK={}",
        sha_ok, hmac_ok, pmk_ok);
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
    big_cmd_buf: Option<DmaRegion>,   // 4KB scratch for large commands (e.g. the ~1.9KB scan req)
    cmd_write_ptr: u32,               // command-queue producer index
    rx_read_ptr: u32,                 // used-BD entries consumed so far
    free_write_ptr: u32,              // free-RBD producer index (for restocking during a scan)
    last_rx_vid: usize,               // vid of the just-consumed buffer, reposted on the next poll
    mac_addr: [u8; 6],
    alive_umac_err: u32,              // UMAC error-table SRAM ptr (from the ALIVE ntf, runtime)
    alive_lmac_err: u32,              // LMAC error-table SRAM ptr (from the ALIVE ntf, runtime)

    // --- Phase 5: scan bring-up ---
    fw_cmd_versions: Vec<[u8; 4]>,    // TLV_CMD_VERSIONS entries [cmd, group, cmd_ver, notif_ver]
    valid_tx_ant: u8,                 // tx antenna mask (from NVM tx_chains)
    valid_rx_ant: u8,                 // rx antenna mask (from NVM rx_chains)

    // --- Phase 6: association ---
    scan_results: Vec<ScanResult>,    // networks discovered by the last scan (deduped by BSSID)

    // --- Phase 6b: TX data queue for the AP station's mgmt/data frames ---
    tx_data_ring: Option<DmaRegion>,  // TFD ring (SCD_QUEUE_CONFIG tfdq_dram_addr)
    tx_data_bc: Option<DmaRegion>,    // byte-count table (bc_dram_addr)
    tx_data_cmdbuf: Option<DmaRegion>,// per-slot dev_cmd bytes (iwl_cmd_header + tx_cmd_v9 + frame)
    tx_data_ftb: Option<DmaRegion>,   // per-slot dedicated first-TB (≤20 B) buffers
    tx_queue_id: u16,                 // queue id returned by the fw
    tx_data_wptr: u32,                // producer index for the data queue

    // --- Phase 6c: WPA2 4-way handshake state ---
    eapol_anonce: [u8; 32],           // AP's nonce (from EAPOL msg 1/4)
    eapol_replay: [u8; 8],            // key replay counter (echo the AP's)
    eapol_kck: [u8; 16],              // PTK key-confirmation key (for EAPOL MICs)
    eapol_tk: [u8; 16],               // PTK temporal key (the CCMP pairwise key)
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
            big_cmd_buf: None,
            cmd_write_ptr: 0,
            rx_read_ptr: 0,
            free_write_ptr: 0,
            last_rx_vid: 0,
            mac_addr: [0; 6],
            alive_umac_err: 0,
            alive_lmac_err: 0,
            fw_cmd_versions: Vec::new(),
            valid_tx_ant: 1,
            valid_rx_ant: 1,
            scan_results: Vec::new(),
            tx_data_ring: None,
            tx_data_bc: None,
            tx_data_cmdbuf: None,
            tx_data_ftb: None,
            tx_queue_id: 0,
            tx_data_wptr: 0,
            eapol_anonce: [0; 32],
            eapol_replay: [0; 8],
            eapol_kck: [0; 16],
            eapol_tk: [0; 16],
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

        // 128 RX buffers of 4 KiB each; record each phys addr into the free-RBD ring. 16 was far too
        // few: once the radio is on, the multi-AP beacon flood fills the ring during any gap where
        // we're not polling and the fw stalls RX. 128 gives plenty of slack (free ring holds 512).
        self.rx_buffers.clear();
        for i in 0..128usize {
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
        crate::serial_println!("[WIFI]   rx_buffers   count={} (x 4KiB)", self.rx_buffers.len());
        // Free-RBD producer starts past the posted buffers; restocking advances it as we consume.
        self.free_write_ptr = self.rx_buffers.len() as u32;

        // Host-command TX queue (gen2). TFD ring = 32 × sizeof(iwl_tfh_tfd)=256 = 8 KiB (2 pg);
        // command backing = 32 × 256B slots = 8 KiB (2 pg); first-TB buffers = 32 × 64B (1 pg).
        self.tx_cmd_ring = self.alloc_dma(2);
        self.tx_cmd_buf = self.alloc_dma(2);
        self.first_tb_bufs = self.alloc_dma(1);
        self.big_cmd_buf = self.alloc_dma(1); // scratch for large commands (scan request ~1.9KB)
        ok &= self.tx_cmd_ring.is_some() && self.tx_cmd_buf.is_some()
            && self.first_tb_bufs.is_some() && self.big_cmd_buf.is_some();
        log_region("tx_cmd_ring", &self.tx_cmd_ring);
        log_region("tx_cmd_buf", &self.tx_cmd_buf);
        log_region("first_tb", &self.first_tb_bufs);
        log_region("big_cmd_buf", &self.big_cmd_buf);

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
                TLV_CMD_VERSIONS => {
                    // Array of iwl_fw_cmd_version{cmd, group, cmd_ver, notif_ver} — tells us which
                    // struct variant each command expects (critical for the versioned scan structs).
                    self.fw_cmd_versions.clear();
                    let mut k = 0usize;
                    while k + 4 <= tlen {
                        let e = doff + k;
                        self.fw_cmd_versions.push([b[e], b[e + 1], b[e + 2], b[e + 3]]);
                        k += 4;
                    }
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
        // Decode the assert class (iwlwifi advanced_lookup[]). BAD_COMMAND ⇒ data1 = the cmd id.
        let name = match (w[1] & 0xFF) as u8 {
            0x34 => "NMI_INTERRUPT_WDG",
            0x35 => "SYSASSERT",
            0x37 => "UCODE_VERSION_MISMATCH",
            0x38 | 0x39 => "BAD_COMMAND (data1 = unsupported cmd id)",
            0x3D => "FATAL_ERROR",
            0x25 => "NMI_TRM_HW_ERR",
            0x2D => "NMI_INTERRUPT_HOST",
            _ => "(unlisted)",
        };
        crate::serial_println!("[WIFI] P3c:   -> assert class {:#04x} = {}", w[1] & 0xFF, name);
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
        // Recycle the previously-returned buffer (the caller has finished reading it by now) so the
        // fw keeps having free RBDs to DMA into — essential during a scan that produces many frames.
        if self.last_rx_vid != 0 {
            self.repost_rx_buffer(self.last_rx_vid);
            self.last_rx_vid = 0;
        }
        for _ in 0..timeout_ms {
            let closed = (read_volatile(rb_stts.virt as *const u16) & 0x0FFF) as u32;
            if closed != self.rx_read_ptr {
                let slot = (self.rx_read_ptr & 0x1FF) as usize; // 512-entry used ring
                let ub = read_volatile((used.virt as *const u32).add(slot));
                let vid = (ub & 0x0FFF) as usize;
                self.rx_read_ptr = self.rx_read_ptr.wrapping_add(1);
                let buf = if vid >= 1 && vid <= self.rx_buffers.len() {
                    self.last_rx_vid = vid; // recycle on the next poll
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

    /// Repost a consumed RX buffer back onto the free-RBD ring (gen2: entry = phys | vid) and ring
    /// the free-RBD doorbell every time. Once the radio is on, the multi-AP beacon flood can drain
    /// our small RX ring during the connect sequence; throttling the doorbell (every 8) let the fw
    /// run out of free RBDs and stall RX → command responses time out. Ringing every repost keeps
    /// the fw maximally supplied.
    unsafe fn repost_rx_buffer(&mut self, vid: usize) {
        if vid < 1 || vid > self.rx_buffers.len() { return; }
        let free = match self.rx_free { Some(f) => f, None => return };
        let phys = self.rx_buffers[vid - 1].phys;
        let slot = (self.free_write_ptr & 0x1FF) as usize; // 512-entry free ring
        write_volatile((free.virt as *mut u64).add(slot), phys | (vid as u64));
        self.free_write_ptr = self.free_write_ptr.wrapping_add(1);
        self.write32(RFH_Q0_FRBDCB_WIDX_TRG, self.free_write_ptr & 0x1FF);
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
        let total = 8 + payload.len();
        // Small commands live in a per-idx 256B slot; large ones (e.g. the ~1.9KB scan request) use
        // a dedicated 4KB scratch buffer — we send synchronously so only one big cmd is ever pending.
        let (cb_virt, cb_phys, cap) = if total > 256 {
            match self.big_cmd_buf {
                Some(b) => (b.virt, b.phys, b.pages * 4096),
                None => { crate::serial_println!("[WIFI] cmd: big_cmd_buf missing."); return false; }
            }
        } else {
            (buf.virt + (idx * 256) as u64, buf.phys + (idx * 256) as u64, 256usize)
        };
        if total > cap {
            crate::serial_println!("[WIFI] cmd: command too large ({} B, cap {}).", total, cap);
            return false;
        }

        // --- Command bytes (iwl_cmd_header_wide + payload). ---
        let cb = cb_virt as *mut u8;
        core::ptr::write_bytes(cb, 0, total);
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
        // TB1 -> the remainder of the command (if any) directly out of the cmd-buf.
        if total > tb0 {
            let rest_phys = cb_phys + tb0 as u64;
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
        // Time-budget loop (not a fixed packet count): once the radio is on, beacons (cmd 0xc1)
        // flood the ring, so a command response can sit behind many of them. Keep consuming until
        // the match appears or ~2 s of idle elapses. Beacons are consumed quietly to avoid spam.
        let mut budget_ms: i32 = 2000;
        while budget_ms > 0 {
            match self.poll_next_rx(per_pkt_ms) {
                Some(v) => {
                    let c = read_volatile((v as *const u8).add(4));
                    let g = read_volatile((v as *const u8).add(5));
                    if c == want_cmd && g == want_grp { return Some(v); }
                    if !(c == 0xC1 && g == 0x00) { // don't log the beacon flood
                        crate::serial_println!("[WIFI] P4b: rx cmd={:#04x} grp={:#04x}", c, g);
                    }
                }
                None => {
                    if self.read32(CSR_INT) & INT_BIT_SW_ERR != 0 {
                        crate::serial_println!("[WIFI] P4b: SW_ERR while waiting for cmd={:#04x} grp={:#04x}",
                            want_cmd, want_grp);
                        return None;
                    }
                    budget_ms -= per_pkt_ms as i32;
                }
            }
        }
        crate::serial_println!("[WIFI] P4b: timeout waiting for cmd={:#04x} grp={:#04x}", want_cmd, want_grp);
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
                return;
            }
        }

        self.start_scan_bringup(); // P5
    }

    /// Send a fire-and-forget config command, then check for a fw BAD_COMMAND/assert. Returns true
    /// if the fw did NOT assert within a short window. `label` is for logging.
    unsafe fn send_cfg_checked(&mut self, label: &str, group: u8, cmd: u8, payload: &[u8]) -> bool {
        self.send_host_cmd(group, cmd, payload);
        self.delay_ms(10);
        if self.read32(CSR_INT) & INT_BIT_SW_ERR != 0 {
            crate::serial_println!("[WIFI] P5: SW_ERR after {} — decoding:", label);
            self.report_fw_error();
            false
        } else {
            crate::serial_println!("[WIFI] P5: {} accepted (no SW_ERR).", label);
            true
        }
    }

    /// P5: start the scan bring-up. Log the fw command versions (so later phases build the correct
    /// struct variants), then send SCAN_CFG_CMD. NOTE: TX_ANT_CONFIGURATION_CMD (0x98) is NOT sent —
    /// this unified fw rejects it as BAD_COMMAND (verified: error_id 0x…38, data1=0x98).
    unsafe fn start_scan_bringup(&mut self) {
        crate::serial_println!("[WIFI] P5: fw lists {} command versions.", self.fw_cmd_versions.len());
        crate::serial_println!(
            "[WIFI] P5: cmd_ver scan_cfg(g1,0x0c)={} scan_req_umac(g1,0x0d)={} scan_abort(g1,0x0e)={}",
            self.cmd_version(0x01, 0x0c), self.cmd_version(0x01, 0x0d), self.cmd_version(0x01, 0x0e));
        crate::serial_println!(
            "[WIFI] P5: cmd_ver add_sta(g0,0x18)={} mac_ctxt(g0,0x28)={} phy_ctxt(g0,0x08)={} binding(g0,0x2b)={} time_event(g0,0x29)={}",
            self.cmd_version(0x00, 0x18), self.cmd_version(0x00, 0x28), self.cmd_version(0x00, 0x08),
            self.cmd_version(0x00, 0x2b), self.cmd_version(0x00, 0x29));

        // P5b: SCAN_CFG_CMD (IWL_ALWAYS_LONG_GROUP=0x1, cmd 0x0c). fw scan_cfg version = 5 → the
        // reduced iwl_scan_config (SCAN_CONFIG_CMD_API_S_VER_5): {flags le32, tx_chains le32,
        // rx_chains le32} = 12 bytes; flags=0. No aux-station / bcast_sta_id needed at v5.
        let mut cfg = [0u8; 12];
        cfg[4..8].copy_from_slice(&(self.valid_tx_ant.max(1) as u32).to_le_bytes());
        cfg[8..12].copy_from_slice(&(self.valid_rx_ant.max(1) as u32).to_le_bytes());
        crate::serial_println!("[WIFI] P5b: SCAN_CFG (v5) tx_chains={:#x} rx_chains={:#x}",
            self.valid_tx_ant, self.valid_rx_ant);
        if self.send_cfg_checked("SCAN_CFG", 0x01, 0x0c, &cfg) {
            crate::serial_println!("[WIFI] P5b: SCAN_CFG done.");
            self.start_scan();
            self.connect(TARGET_SSID);
        }
    }

    /// P6a: begin associating with `ssid` (found in the last scan). Kernel-first: builds the fw
    /// context topology so the radio is tuned and a station slot exists. Order (iwlwifi):
    /// PHY_CONTEXT (tune to the AP channel) → MAC_CONTEXT → BINDING → ADD_STA. Crypto (P6c) later.
    unsafe fn connect(&mut self, ssid: &[u8]) {
        let target = match self.find_network(ssid) {
            Some(t) => t,
            None => {
                crate::serial_println!("[WIFI] P6: target SSID not found in scan results — aborting.");
                return;
            }
        };
        crate::serial_println!(
            "[WIFI] P6: connecting to \"{}\"  {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  ch{} band{}",
            core::str::from_utf8(&target.ssid[..target.ssid_len as usize]).unwrap_or("?"),
            target.bssid[0], target.bssid[1], target.bssid[2], target.bssid[3], target.bssid[4],
            target.bssid[5], target.channel, target.band);

        // --- P6a.1: PHY_CONTEXT_CMD (grp0 cmd0x08, MLD 32-byte layout) ---
        crate::serial_println!("[WIFI] P6a: MLD firmware — using MAC_CONF_GROUP context commands.");
        if !self.send_phy_context(target.channel, target.band) {
            crate::serial_println!("[WIFI] P6a: PHY_CONTEXT rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6a: PHY_CONTEXT accepted.");

        // --- P6a.1b: RLC_CONFIG_CMD — configure the RX chains (our fw has RLC v2, so this is where
        // rx_chain_info goes, not PHY_CONTEXT). Fedora sends this right after PHY_CONTEXT; without it
        // the radio has no RX chains and receives nothing. ---
        if !self.send_rlc_config() {
            crate::serial_println!("[WIFI] P6a: RLC_CONFIG rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6a: RLC_CONFIG accepted (RX chains configured).");

        // --- P6a.2: MAC_CONFIG_CMD (grp0x03 cmd0x08) — create our BSS-station interface ---
        if !self.send_mac_config() {
            crate::serial_println!("[WIFI] P6a: MAC_CONFIG rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6a: MAC_CONFIG accepted.");

        // --- P6a.3: LINK_CONFIG_CMD ADD (grp0x03 cmd0x09) — create the link (unbound/inactive) ---
        if !self.send_link_config_add() {
            crate::serial_println!("[WIFI] P6a: LINK_CONFIG(add) rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6a: LINK_CONFIG(add) accepted.");

        // --- P6a.4: activate the link in TWO steps, exactly like mvm assign_vif_chanctx:
        // (1) MODIFY with mask=0, active=0 → binds phy_id while INACTIVE (phy_id/addr are only
        // applied until the link is active); (2) MODIFY mask=ACTIVE|RATES_INFO, active=1 → activate.
        // Doing both in one command (phy_id + active=1 together) makes the fw ignore the phy binding
        // and activate a phy-less link → assert 0x2010330f.
        let bi = if target.beacon_int == 0 { 100 } else { target.beacon_int };
        if !self.send_link_modify(0x00, 0, bi, "LINK_BINDPHY") {
            crate::serial_println!("[WIFI] P6a: LINK bind-phy rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6a: LINK phy bound (inactive).");
        // ACTIVE|RATES_INFO|BEACON_TIMING (BIT0|BIT1|BIT4=0x13): include BEACON_TIMING so the fw
        // actually applies bi/dtim — session protection (CONF_ASSOC) schedules relative to the AP's
        // beacons and needs a non-zero beacon interval in the link.
        if !self.send_link_modify(0x13, 1, bi, "LINK_ACTIVATE") {
            crate::serial_println!("[WIFI] P6a: LINK activate rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6a: LINK activated (bound to phy, active=1).");
        let bssid = target.bssid;

        // Device power (POWER_TABLE_CMD, CAM = no power-save) — init the power/time subsystem.
        if self.send_device_power() {
            crate::serial_println!("[WIFI] P6b: DEVICE_POWER (CAM) accepted.");
        }

        // --- STA_CONFIG_CMD — add the AP as a peer station ---
        if !self.send_sta_config(&bssid) {
            crate::serial_println!("[WIFI] P6a: STA_CONFIG rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6a: STA_CONFIG accepted.");

        // --- TLC_MNG_CONFIG_CMD — rate-scaling table for the AP station. Fedora sends this before
        // the auth; without a rate table the fw can't TX to the station and asserts (class 0x47). ---
        if self.send_tlc_config() {
            crate::serial_println!("[WIFI] P6b: TLC_CONFIG accepted (rate table set).");
        } else {
            crate::serial_println!("[WIFI] P6b: TLC_CONFIG rejected (continuing).");
        }

        // --- allocate a TX queue for the AP station ---
        if !self.send_scd_queue_config() {
            crate::serial_println!("[WIFI] P6b: TX queue allocation failed — stopping.");
            return;
        }

        // --- session protection LAST, right before the auth, so the ~900 ms on-medium window is
        // still fresh when the frame goes out. The queue-config's beacon-flood wait can consume a
        // lot of time, so scheduling it earlier let the window expire → TX-on-expired-window assert. ---
        if !self.send_session_protection() {
            crate::serial_println!("[WIFI] P6b: SESSION_PROTECTION rejected — stopping.");
            return;
        }
        crate::serial_println!("[WIFI] P6b: SESSION_PROTECTION accepted.");
        match self.wait_for_rx(MAC_CONF_GROUP, 0xFB, 30) {
            Some(_) => crate::serial_println!("[WIFI] P6b: session protection started (radio on-channel)."),
            None => crate::serial_println!("[WIFI] P6b: no SESSION_PROTECTION_NOTIF (continuing)."),
        }

        // --- TX the auth frame, then (on success) the association request ---
        if self.send_auth(&bssid) {
            crate::serial_println!("[WIFI] P6b: authenticated — sending association request.");
            if self.send_assoc(&bssid, ssid) {
                crate::serial_println!("[WIFI] P6c: associated — waiting for WPA2 4-way handshake.");
                self.watch_eapol(&bssid, ssid);
            }
        }
    }

    /// P6c: after association the AP starts the WPA2 4-way handshake by sending EAPOL-Key msg 1/4 (a
    /// data frame, LLC/SNAP ethertype 0x888e, carrying the AP's ANonce). Watch for it and dump the
    /// key_info + ANonce — the seed for deriving the PTK.
    unsafe fn watch_eapol(&mut self, bssid: &[u8; 6], ssid: &[u8]) {
        wpa_crypto_selftest();
        // 1) wait for EAPOL-Key msg 1/4 (extracts ANonce + replay counter into self fields).
        let mut got1 = false;
        let mut budget_ms: i32 = 1500;
        while budget_ms > 0 && !got1 {
            match self.poll_next_rx(100) {
                Some(v) => {
                    let cmd = read_volatile((v as *const u8).add(4));
                    let grp = read_volatile((v as *const u8).add(5));
                    if cmd == 0xC1 && grp == 0x00 && self.scan_eapol(v) == Some(1) { got1 = true; }
                }
                None => budget_ms -= 100,
            }
        }
        if !got1 { crate::serial_println!("[WIFI] P6c: no EAPOL-Key msg 1/4 (yet)."); return; }

        // 2) send msg 2/4 (candidate passphrase 0) + 3) watch for msg 3/4 (proves PTK/MIC correct).
        self.send_eapol_msg2(bssid, ssid, TARGET_PSK_CANDIDATES[0]);
        let mut budget_ms: i32 = 1500;
        while budget_ms > 0 {
            match self.poll_next_rx(100) {
                Some(v) => {
                    let cmd = read_volatile((v as *const u8).add(4));
                    let grp = read_volatile((v as *const u8).add(5));
                    if cmd == 0xC1 && grp == 0x00 {
                        if let Some(msg) = self.scan_eapol(v) {
                            if msg == 3 {
                                crate::serial_println!("[WIFI] P6c: *** msg 3/4 received — PTK/MIC verified by AP! ***");
                                self.send_eapol_msg4(bssid);
                                if self.install_ptk() {
                                    crate::serial_println!("[WIFI] P6c: *** PTK installed — WPA2 handshake COMPLETE, link encrypted! ***");
                                }
                                return;
                            }
                        }
                    }
                }
                None => budget_ms -= 100,
            }
        }
        crate::serial_println!("[WIFI] P6c: no msg 3/4 (MIC may be wrong / try other passphrase).");
    }

    /// Scan an RX_MPDU for the EAPOL ethertype (0x888e). EAPOL-Key layout from the ethertype: +2 ver,
    /// +3 type, +4 len(BE), +6 desc_type, +7 key_info(BE), +9 key_len, +11 replay(8), +19 nonce(32).
    /// For msg 1/4 (ACK set, MIC clear) captures ANonce + replay counter. Returns the message number
    /// (1 or 3) or None. msg 1/4: key_info ACK(0x80) & !MIC(0x100). msg 3/4: ACK & MIC & Install(0x40).
    unsafe fn scan_eapol(&mut self, v: u64) -> Option<u8> {
        let p = v as *const u8;
        let rd = |o: usize| read_volatile(p.add(o));
        let total = (read_volatile(p as *const u32) & 0x3FFF) as usize;
        let cap = if (40..=4096).contains(&total) { total } else { 512 };
        let mut o = 8;
        while o + 51 <= cap {
            if rd(o) == 0x88 && rd(o + 1) == 0x8e {
                let key_info = ((rd(o + 7) as u16) << 8) | (rd(o + 8) as u16);
                let mic = key_info & 0x0100 != 0;
                let ack = key_info & 0x0080 != 0;
                let install = key_info & 0x0040 != 0;
                if ack && !mic {
                    for k in 0..8 { self.eapol_replay[k] = rd(o + 11 + k); }
                    for k in 0..32 { self.eapol_anonce[k] = rd(o + 19 + k); }
                    crate::serial_println!("[WIFI] P6c: *** EAPOL msg 1/4 *** key_info={:#06x}", key_info);
                    crate::serial_println!("[WIFI] P6c: ANonce {:02x?}", &self.eapol_anonce[..16]);
                    return Some(1);
                } else if mic && ack && install {
                    for k in 0..8 { self.eapol_replay[k] = rd(o + 11 + k); } // echo in msg 4/4
                    crate::serial_println!("[WIFI] P6c: *** EAPOL msg 3/4 *** key_info={:#06x}", key_info);
                    return Some(3);
                } else {
                    crate::serial_println!("[WIFI] P6c: EAPOL key_info={:#06x} (other)", key_info);
                    return Some(0);
                }
            }
            o += 1;
        }
        None
    }

    /// Build + TX EAPOL-Key msg 2/4: derive PTK from PMK(psk,ssid) + both nonces + both MACs, fill the
    /// EAPOL-Key with our SNonce + RSN IE, then set MIC = HMAC-SHA1(KCK, eapol)[..16]. Sent as an
    /// 802.11 data frame (ToDS) with LLC/SNAP + ethertype 0x888e on the AP-station queue.
    unsafe fn send_eapol_msg2(&mut self, bssid: &[u8; 6], ssid: &[u8], psk: &[u8]) {
        // PMK + PTK.
        let pmk = pbkdf2_sha1(psk, ssid, 4096, 32);
        let mut snonce = [0u8; 32];
        let seed = core::arch::x86_64::_rdtsc();
        let s1 = sha1(&{ let mut b = alloc::vec::Vec::new(); b.extend_from_slice(&seed.to_le_bytes()); b.extend_from_slice(&self.mac_addr); b });
        let s2 = sha1(&{ let mut b = alloc::vec::Vec::new(); b.extend_from_slice(&self.mac_addr); b.extend_from_slice(&seed.rotate_left(17).to_le_bytes()); b });
        snonce[..20].copy_from_slice(&s1);
        snonce[20..32].copy_from_slice(&s2[..12]);
        // ptk_data = min(AA,SPA)|max|min(ANonce,SNonce)|max.
        let (aa, spa) = (bssid, &self.mac_addr);
        let (lo_mac, hi_mac) = if aa[..] < spa[..] { (aa, spa) } else { (spa, aa) };
        let (lo_n, hi_n) = if self.eapol_anonce < snonce { (&self.eapol_anonce, &snonce) } else { (&snonce, &self.eapol_anonce) };
        let mut pdata = alloc::vec::Vec::with_capacity(76);
        pdata.extend_from_slice(lo_mac); pdata.extend_from_slice(hi_mac);
        pdata.extend_from_slice(lo_n); pdata.extend_from_slice(hi_n);
        let ptk = sha1_prf(&pmk, b"Pairwise key expansion", &pdata, 48);
        self.eapol_kck.copy_from_slice(&ptk[0..16]);   // KCK — EAPOL MICs
        self.eapol_tk.copy_from_slice(&ptk[32..48]);   // TK  — CCMP pairwise key
        let kck = self.eapol_kck;

        // RSN IE (key_data), same as assoc.
        let rsn: [u8; 22] = [0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f,
            0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, 0x00, 0x00];

        // Frame: 802.11 data hdr(24) + LLC/SNAP(8) + EAPOL(4) + key body(117) = 153.
        let mut f = [0u8; 160];
        f[0] = 0x08; f[1] = 0x01;                    // data, ToDS
        f[4..10].copy_from_slice(bssid);             // Addr1 = RA = AP
        f[10..16].copy_from_slice(&self.mac_addr);   // Addr2 = TA = us
        f[16..22].copy_from_slice(bssid);            // Addr3 = DA = AP
        f[24..32].copy_from_slice(&[0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00, 0x88, 0x8e]); // LLC/SNAP + EAPOL
        f[32] = 0x02; f[33] = 0x03;                  // EAPOL version 2, type Key
        f[34..36].copy_from_slice(&117u16.to_be_bytes()); // EAPOL length
        f[36] = 0x02;                                // descriptor type = RSN
        f[37..39].copy_from_slice(&0x010au16.to_be_bytes()); // key_info: v2|pairwise|MIC
        // key_length@39 = 0, replay@41:
        f[41..49].copy_from_slice(&self.eapol_replay);
        f[49..81].copy_from_slice(&snonce);          // key_nonce = SNonce
        // key_iv@81, key_rsc@97, key_id@105 all 0. key_mic@113 = 0 (computed below).
        f[129..131].copy_from_slice(&22u16.to_be_bytes()); // key_data_length
        f[131..153].copy_from_slice(&rsn);
        // MIC = HMAC-SHA1(KCK, eapol[32..153])[..16] with the MIC field (113..129) zeroed.
        let mic = hmac_sha1(&kck, &f[32..153]);
        f[113..129].copy_from_slice(&mic[..16]);

        crate::serial_println!("[WIFI] P6c: TX EAPOL msg 2/4 (SNonce+MIC, psk={:?})…",
            core::str::from_utf8(psk).unwrap_or("?"));
        self.tx_data_frame(&f[..153], 24);
    }

    /// Build + TX EAPOL-Key msg 4/4 (final ack): key_info=0x030a (v2|pairwise|MIC|Secure), zero
    /// nonce, no key data, echoing msg 3/4's replay counter, MIC = HMAC-SHA1(KCK, eapol). 131 B.
    unsafe fn send_eapol_msg4(&mut self, bssid: &[u8; 6]) {
        let mut f = [0u8; 140];
        f[0] = 0x08; f[1] = 0x01;
        f[4..10].copy_from_slice(bssid);
        f[10..16].copy_from_slice(&self.mac_addr);
        f[16..22].copy_from_slice(bssid);
        f[24..32].copy_from_slice(&[0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00, 0x88, 0x8e]);
        f[32] = 0x02; f[33] = 0x03;
        f[34..36].copy_from_slice(&95u16.to_be_bytes());   // EAPOL length (no key_data)
        f[36] = 0x02;
        f[37..39].copy_from_slice(&0x030au16.to_be_bytes()); // v2|pairwise|MIC|Secure
        f[41..49].copy_from_slice(&self.eapol_replay);
        // nonce@49, iv@81, rsc@97, id@105 all 0. key_mic@113. key_data_length@129 = 0.
        let kck = self.eapol_kck;
        let mic = hmac_sha1(&kck, &f[32..131]);
        f[113..129].copy_from_slice(&mic[..16]);
        crate::serial_println!("[WIFI] P6c: TX EAPOL msg 4/4 (final ack)…");
        self.tx_data_frame(&f[..131], 24);
    }

    /// Install the pairwise CCMP key (TK) into the fw via SEC_KEY_CMD so it can encrypt/decrypt
    /// unicast traffic. iwl_sec_key_cmd (80 B): action@0=ADD, add{sta_mask@4=BIT(0), key_id@8=0,
    /// key_flags@12=CIPHER_CCMP(0x02), key@16 (TK in first 16 B), tkip@48, rx_seq@64, tx_seq@72}.
    unsafe fn install_ptk(&mut self) -> bool {
        let mut c = [0u8; 80];
        c[0..4].copy_from_slice(&(FW_CTXT_ACTION_ADD as u32).to_le_bytes()); // action = ADD
        c[4..8].copy_from_slice(&1u32.to_le_bytes());   // sta_mask = BIT(sta_id 0)
        // key_id@8 = 0 (pairwise)
        c[12..16].copy_from_slice(&0x02u32.to_le_bytes()); // key_flags = CIPHER_CCMP
        c[16..32].copy_from_slice(&self.eapol_tk);      // TK (16 B)
        self.send_cfg_checked("SEC_KEY(PTK)", DATA_PATH_GROUP, 0x18, &c)
    }

    /// POWER_TABLE_CMD (device power, legacy id 0x77 → LONG_GROUP). iwl_device_power_cmd (4 B):
    /// flags le16@0 = 0 (CAM, no power-save) + reserved@2. Initializes the device power subsystem
    /// and keeps the radio awake.
    unsafe fn send_device_power(&mut self) -> bool {
        let c = [0u8; 4];
        self.send_cfg_checked("DEVICE_POWER", 0x01, 0x77, &c)
    }

    /// Legacy TIME_EVENT_CMD (grp0 cmd0x29 → LONG_GROUP on the wire). iwl_time_event_cmd (36 B):
    /// unused — this fw rejects it as BAD_COMMAND (only SESSION_PROTECTION_CMD is supported).
    #[allow(dead_code)]
    /// id_and_color@0=0, action@4=ADD, id@8=TE_BSS_STA_AGGRESSIVE_ASSOC(0), apply_time@12=0,
    /// max_delay@16=0, depends_on@20=0, interval@24=1, duration@28 (TU), repeat@32=1,
    /// max_frags@33=TE_V2_FRAG_NONE(0), policy@34=NOTIF_START|NOTIF_END|START_IMMEDIATELY(0x803).
    unsafe fn send_time_event(&mut self) -> bool {
        let mut c = [0u8; 36];
        c[4..8].copy_from_slice(&(FW_CTXT_ACTION_ADD as u32).to_le_bytes()); // action = ADD
        // id@8 = 0 (TE_BSS_STA_AGGRESSIVE_ASSOC)
        c[24..28].copy_from_slice(&1u32.to_le_bytes());     // interval = 1
        c[28..32].copy_from_slice(&878u32.to_le_bytes());   // duration (TU)
        c[32] = 1;                                          // repeat
        // max_frags@33 = 0 (FRAG_NONE)
        c[34..36].copy_from_slice(&0x0803u16.to_le_bytes()); // policy
        self.send_cfg_checked("TIME_EVENT", 0x01, 0x29, &c)
    }

    /// Enqueue a full 802.11 `frame` (MAC header + body) on the AP station's TX queue. Wraps it in
    /// dev_cmd = iwl_cmd_header(4){cmd=TX_CMD,grp=0,seq=queue|idx} + iwl_tx_cmd_v9(20){len,
    /// offload_assist=MH_SIZE, flags=ENCRYPT_DIS|HIGH_PRI, dram=0, rate_n_flags=0} + frame, laid out
    /// contiguously. TFD: TB0 = first 20 B (dedicated first-TB buf), TB1 = the rest. Byte-count
    /// table (gen2): DIV_ROUND_UP(len,4) | (fetch_chunks<<12). Doorbell = (wptr+1)|(qid<<16).
    unsafe fn tx_data_frame(&mut self, frame: &[u8], hdr_len: usize) -> bool {
        let (ring, cmdbuf, ftb, bc) =
            match (self.tx_data_ring, self.tx_data_cmdbuf, self.tx_data_ftb, self.tx_data_bc) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => { crate::serial_println!("[WIFI] tx: rings missing."); return false; }
            };
        let qid = self.tx_queue_id;
        let wptr = self.tx_data_wptr;
        let idx = (wptr & 0x3F) as usize;              // 64-entry queue
        let dev_len = 4 + 20 + frame.len();            // cmd_header + tx_cmd_v9 fixed + frame
        if dev_len > 256 { crate::serial_println!("[WIFI] tx: frame too large."); return false; }

        // --- dev_cmd bytes ---
        let cb = (cmdbuf.virt + (idx * 256) as u64) as *mut u8;
        core::ptr::write_bytes(cb, 0, dev_len);
        write_volatile(cb.add(0), 0x1c);               // iwl_cmd_header.cmd = TX_CMD
        write_volatile(cb.add(1), 0x00);               // .group_id = LEGACY
        let seq: u16 = (((qid & 0x1f) << 8) | (idx as u16 & 0xff)) as u16;
        write_volatile(cb.add(2) as *mut u16, seq);    // .sequence = queue|index
        // iwl_tx_cmd_v9 @4:
        write_volatile(cb.add(4) as *mut u16, frame.len() as u16);           // len
        write_volatile(cb.add(6) as *mut u16, ((hdr_len / 2) as u16) << 8);  // offload_assist = MH_SIZE
        write_volatile(cb.add(8) as *mut u32, 0x0000_0006u32);               // flags = ENCRYPT_DIS|HIGH_PRI
        // dram_info@12 (8 B) = 0. rate_n_flags@20 = 0: with TLC configured the fw picks the rate
        // from the station's rate table (like the driver does for mgmt) — no CMD_RATE forcing.
        for (i, &b) in frame.iter().enumerate() { write_volatile(cb.add(24 + i), b); } // frame @24
        let cb_phys = cmdbuf.phys + (idx * 256) as u64;

        // --- TB0: first 20 B into the dedicated first-TB buffer ---
        let ftbv = (ftb.virt + (idx * 64) as u64) as *mut u8;
        core::ptr::copy_nonoverlapping(cb, ftbv, IWL_FIRST_TB_SIZE);
        let ftbp = ftb.phys + (idx * 64) as u64;

        // --- TFD (iwl_tfh_tfd): num_tbs@0, tbs[]{tb_len le16, addr le64} @2,@12 ---
        let tfd = (ring.virt + (idx * TFH_TFD_SIZE) as u64) as *mut u8;
        core::ptr::write_bytes(tfd, 0, TFH_TFD_SIZE);
        write_volatile(tfd.add(2) as *mut u16, IWL_FIRST_TB_SIZE as u16);
        for (k, b) in ftbp.to_le_bytes().iter().enumerate() { write_volatile(tfd.add(4 + k), *b); }
        let rest = dev_len - IWL_FIRST_TB_SIZE;
        write_volatile(tfd.add(12) as *mut u16, rest as u16);
        for (k, b) in (cb_phys + IWL_FIRST_TB_SIZE as u64).to_le_bytes().iter().enumerate() {
            write_volatile(tfd.add(14 + k), *b);
        }
        write_volatile(tfd as *mut u16, 2u16);         // num_tbs = 2

        // --- byte-count table entry (gen2: len in dwords, fetch_chunks=0 for our small frame) ---
        let bcv = bc.virt as *mut u16;
        let len_div4 = (frame.len() + 3) / 4;
        write_volatile(bcv.add(idx), len_div4 as u16);

        // --- advance + doorbell ---
        self.tx_data_wptr = wptr.wrapping_add(1);
        let door = (self.tx_data_wptr & 0xFFFF) | ((qid as u32) << 16);
        self.write32(HBUS_TARG_WRPTR, door);
        crate::serial_println!("[WIFI] tx: qid={} idx={} frame_len={} door={:#x}",
            qid, idx, frame.len(), door);
        true
    }

    /// P6b.2: build + TX an open-system 802.11 authentication frame; wait for the AP's auth reply.
    /// Frame = 24-byte MAC header + {auth_algo=0 (open), auth_seq=1, status=0}. Returns true on reply.
    unsafe fn send_auth(&mut self, bssid: &[u8; 6]) -> bool {
        let mut f = [0u8; 30];
        f[0] = 0xB0; f[1] = 0x00;                   // frame control: mgmt / auth
        f[4..10].copy_from_slice(bssid);            // Address1 (DA) = AP
        f[10..16].copy_from_slice(&self.mac_addr);  // Address2 (SA) = us
        f[16..22].copy_from_slice(bssid);           // Address3 (BSSID) = AP
        f[26] = 0x01;                               // auth_algo=0@24, auth_seq=1@26, status=0@28
        crate::serial_println!("[WIFI] P6b: TX auth (open, seq 1)…");
        self.tx_mgmt_await_reply(&f, "auth")
    }

    /// P6b.3: build + TX an 802.11 (re)association request carrying the WPA2 RSN IE, then wait for
    /// the AP's assoc response (which yields our AID). Body: capability, listen interval, SSID IE,
    /// supported-rates IEs, and the RSN IE (group/pairwise = CCMP, AKM = PSK) — the RSN IE is what
    /// tells the AP to run WPA2, so a 4-way handshake follows.
    unsafe fn send_assoc(&mut self, bssid: &[u8; 6], ssid: &[u8]) -> bool {
        let mut f = [0u8; 128];
        let mut n = 0usize;
        // MAC header (24 B): FC=0x0000 (mgmt, assoc-req subtype 0).
        f[4..10].copy_from_slice(bssid);            // Address1 = AP
        f[10..16].copy_from_slice(&self.mac_addr);  // Address2 = us
        f[16..22].copy_from_slice(bssid);           // Address3 = AP
        n = 24;
        // Fixed fields: capability info (ESS|Privacy|ShortPreamble|ShortSlot) + listen interval.
        f[n..n + 2].copy_from_slice(&0x0431u16.to_le_bytes()); n += 2;
        f[n..n + 2].copy_from_slice(&0x000Au16.to_le_bytes()); n += 2; // listen interval = 10
        // SSID IE (tag 0).
        f[n] = 0x00; f[n + 1] = ssid.len() as u8; n += 2;
        f[n..n + ssid.len()].copy_from_slice(ssid); n += ssid.len();
        // Supported Rates IE (tag 1): 1,2,5.5,11 (basic) + 6,9,12,18 Mbps (0.5 Mbps units, hi bit=basic).
        f[n] = 0x01; f[n + 1] = 8; n += 2;
        f[n..n + 8].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]); n += 8;
        // Extended Supported Rates IE (tag 50): 24,36,48,54 Mbps.
        f[n] = 0x32; f[n + 1] = 4; n += 2;
        f[n..n + 4].copy_from_slice(&[0x30, 0x48, 0x60, 0x6c]); n += 4;
        // HT Capabilities IE (tag 45, 26 B): the AP is 802.11n and rejects a legacy-only assoc
        // (status 18 = basic HT-MCS not supported). Advertise 1×1 HT: cap_info=SMPS-disabled(0x0c),
        // A-MPDU=0, supported MCS set = MCS 0-7 (byte0=0xFF), rest 0.
        f[n] = 0x2d; f[n + 1] = 26; n += 2;
        f[n..n + 2].copy_from_slice(&0x000Cu16.to_le_bytes()); n += 2; // HT cap info
        f[n] = 0x00; n += 1;                                          // A-MPDU params
        f[n] = 0xFF; n += 16;                                         // MCS set: MCS 0-7 (1 stream)
        // HT ext cap (2) + TX beamforming (4) + ASEL (1) = 7 bytes, all 0.
        n += 7;
        // RSN IE (tag 48): WPA2 — group=CCMP, 1 pairwise=CCMP, 1 AKM=PSK, caps=0.
        f[n] = 0x30; f[n + 1] = 20; n += 2;
        f[n..n + 20].copy_from_slice(&[
            0x01, 0x00,                   // version 1
            0x00, 0x0f, 0xac, 0x04,       // group cipher = CCMP
            0x01, 0x00,                   // pairwise count
            0x00, 0x0f, 0xac, 0x04,       // pairwise = CCMP
            0x01, 0x00,                   // AKM count
            0x00, 0x0f, 0xac, 0x02,       // AKM = PSK
            0x00, 0x00,                   // RSN capabilities
        ]); n += 20;
        crate::serial_println!("[WIFI] P6b: TX assoc-req ({} B, RSN/WPA2)…", n);
        self.tx_mgmt_await_reply(&f[..n], "assoc")
    }

    /// TX a management `frame` (24-byte header) on the AP station's queue and watch the RX ring for a
    /// reply addressed to us (within the session-protection window). Logs the TX_RESP. Returns true
    /// if any frame addressed to us arrives.
    unsafe fn tx_mgmt_await_reply(&mut self, frame: &[u8], label: &str) -> bool {
        if !self.tx_data_frame(frame, 24) {
            crate::serial_println!("[WIFI] P6b: {} TX enqueue failed.", label);
            return false;
        }
        if self.read32(CSR_INT) & INT_BIT_SW_ERR != 0 {
            crate::serial_println!("[WIFI] P6b: SW_ERR after {} TX — decoding:", label);
            self.report_fw_error();
            return false;
        }
        let mut got = false;
        let mut budget_ms: i32 = 700;
        while budget_ms > 0 && !got {
            match self.poll_next_rx(100) {
                Some(v) => {
                    let cmd = read_volatile((v as *const u8).add(4));
                    let grp = read_volatile((v as *const u8).add(5));
                    if cmd == 0xC1 && grp == 0x00 {
                        if self.scan_rx_for_us(v) { got = true; }
                    } else if cmd == 0x1C && grp == 0x00 {
                        let mut h = [0u8; 24];
                        for k in 0..24 { h[k] = read_volatile((v as *const u8).add(k)); }
                        crate::serial_println!("[WIFI] P6b: TX_RESP head: {:02x?}", &h[..]);
                    }
                }
                None => budget_ms -= 100,
            }
        }
        if !got { crate::serial_println!("[WIFI] P6b: no {} reply.", label); }
        got
    }

    /// Brute-force scan an RX_MPDU buffer for our MAC — it's Address1 (offset +4) of any 802.11
    /// frame the AP sends to us, so its presence means the frame is addressed to us regardless of
    /// the (version-dependent) iwl_rx_mpdu_desc size. Reports the frame-control subtype and, for an
    /// auth frame, the auth seq + status. Returns true iff a frame addressed to us was found.
    unsafe fn scan_rx_for_us(&self, v: u64) -> bool {
        let p = v as *const u8;
        let rd = |o: usize| read_volatile(p.add(o));
        let total = (read_volatile(p as *const u32) & 0x3FFF) as usize;
        let cap = if (40..=4096).contains(&total) { total } else { 512 };
        let mut o = 12; // need o-4 ≥ 8 for the frame-control byte
        while o + 6 <= cap {
            if (0..6).all(|k| rd(o + k) == self.mac_addr[k]) {
                let f = o - 4;            // our MAC is Address1 → frame starts 4 B earlier
                let fc = rd(f);
                let ftype = (fc >> 2) & 0x3;
                let subtype = fc >> 4;
                if ftype == 0 {           // management frame
                    if subtype == 0x0B {  // authentication
                        let algo = (rd(f + 24) as u16) | ((rd(f + 25) as u16) << 8);
                        let aseq = (rd(f + 26) as u16) | ((rd(f + 27) as u16) << 8);
                        let status = (rd(f + 28) as u16) | ((rd(f + 29) as u16) << 8);
                        crate::serial_println!(
                            "[WIFI] P6b: *** AUTH RESP *** algo={} seq={} status={} {}",
                            algo, aseq, status, if status == 0 { "(OK)" } else { "(REJECTED)" });
                    } else if subtype == 0x01 {  // association response
                        // body: capability@24, status@26, AID@28.
                        let status = (rd(f + 26) as u16) | ((rd(f + 27) as u16) << 8);
                        let aid = ((rd(f + 28) as u16) | ((rd(f + 29) as u16) << 8)) & 0x3FFF;
                        crate::serial_println!(
                            "[WIFI] P6b: *** ASSOC RESP *** status={} aid={} {}",
                            status, aid, if status == 0 { "(ASSOCIATED)" } else { "(REJECTED)" });
                    } else {
                        crate::serial_println!("[WIFI] P6b: rx mgmt to us: fc={:#04x} subtype={:#x}",
                            fc, subtype);
                    }
                    return true;
                }
                // data/control to us — note it but keep scanning for a mgmt match.
                crate::serial_println!("[WIFI] P6b: rx frame to us: fc={:#04x} (type {})", fc, ftype);
                return true;
            }
            o += 1;
        }
        false
    }

    /// P6b.1: allocate a dynamic TX queue for the AP station so we can transmit mgmt/data frames.
    /// gen2 SCD_QUEUE_CONFIG_CMD (DATA_PATH_GROUP 0x05, cmd 0x17, ver3 "new" format): the driver
    /// owns the DRAM — a TFD ring (TX_Q_SIZE × 256 B stride) and a byte-count table (gen2:
    /// TFD_QUEUE_BC_SIZE=320 entries × 2 B = 640 B) — and passes their phys addrs; the fw replies
    /// with the queue id. cb_size = ilog2(size) − 3. Response = iwl_tx_queue_cfg_rsp{queue_number
    /// le16, write_pointer le16}.
    unsafe fn send_scd_queue_config(&mut self) -> bool {
        const TX_Q_SIZE: usize = 64;      // TFDs (≥ IWL_MGMT_QUEUE_SIZE=16, power of 2)
        const CB_SIZE: u32 = 3;           // ilog2(64) − 3
        let ring_pages = (TX_Q_SIZE * TFH_TFD_SIZE + 4095) / 4096; // 64*256 = 16 KiB = 4 pages
        let ring = match self.alloc_dma(ring_pages) {
            Some(r) => r,
            None => { crate::serial_println!("[WIFI] P6b: TFD ring alloc failed."); return false; }
        };
        let bc = match self.alloc_dma(1) {
            Some(b) => b,
            None => { crate::serial_println!("[WIFI] P6b: bc-table alloc failed."); return false; }
        };
        let cmdbuf = match self.alloc_dma(1) {
            Some(b) => b,
            None => { crate::serial_println!("[WIFI] P6b: tx cmdbuf alloc failed."); return false; }
        };
        let ftb = match self.alloc_dma(1) {
            Some(b) => b,
            None => { crate::serial_println!("[WIFI] P6b: tx ftb alloc failed."); return false; }
        };
        self.tx_data_ring = Some(ring);
        self.tx_data_bc = Some(bc);
        self.tx_data_cmdbuf = Some(cmdbuf);
        self.tx_data_ftb = Some(ftb);

        let mut c = [0u8; 36];
        // operation@0 = IWL_SCD_QUEUE_ADD (0)
        c[4..8].copy_from_slice(&1u32.to_le_bytes());          // add.sta_mask = BIT(sta_id 0)
        c[8] = IWL_MGMT_TID;                                   // add.tid
        // add.reserved[3]@9, add.flags@12 = 0
        c[16..20].copy_from_slice(&CB_SIZE.to_le_bytes());     // add.cb_size
        c[20..28].copy_from_slice(&bc.phys.to_le_bytes());     // add.bc_dram_addr
        c[28..36].copy_from_slice(&ring.phys.to_le_bytes());   // add.tfdq_dram_addr

        if !self.send_host_cmd(DATA_PATH_GROUP, 0x17, &c) { return false; }
        self.delay_ms(5);
        if self.read32(CSR_INT) & INT_BIT_SW_ERR != 0 {
            crate::serial_println!("[WIFI] P6b: SW_ERR after SCD_QUEUE_CONFIG — decoding:");
            self.report_fw_error();
            return false;
        }
        match self.wait_for_rx(DATA_PATH_GROUP, 0x17, 20) {
            Some(v) => {
                // iwl_tx_queue_cfg_rsp: queue_number le16@0, flags@2, write_pointer@4 (data starts
                // at packet offset 8). The write_pointer is at +4, NOT +2 (that's flags) — reading
                // flags as the pointer made us start at index 1, leaving an empty TFD at index 0
                // that the fw reads forever → nothing transmits.
                let qid = read_volatile((v as *const u8).add(8) as *const u16);
                let wptr = read_volatile((v as *const u8).add(12) as *const u16);
                self.tx_queue_id = qid;
                self.tx_data_wptr = wptr as u32;
                crate::serial_println!("[WIFI] P6b: TX queue allocated — id={} write_ptr={}", qid, wptr);
                true
            }
            None => { crate::serial_println!("[WIFI] P6b: no SCD_QUEUE_CONFIG response."); false }
        }
    }

    /// SESSION_PROTECTION_CMD (mvm iwl_mvm_schedule_session_protection). v1 struct (24 B):
    /// id_and_color@0 = vif id (0; ver<2 uses mvmvif->id not the link id), action@4=ADD,
    /// conf_id@8=SESSION_PROTECT_CONF_ASSOC(0), duration_tu@12, repetition_count@16=0, interval@20=0.
    unsafe fn send_session_protection(&mut self) -> bool {
        let mut c = [0u8; 24];
        // id_and_color@0 = 0 (vif id)
        c[4..8].copy_from_slice(&(FW_CTXT_ACTION_ADD as u32).to_le_bytes()); // action = ADD
        // conf_id@8 = 0 (ASSOC)
        c[12..16].copy_from_slice(&878u32.to_le_bytes()); // duration_tu = MSEC_TO_TU(900)
        self.send_cfg_checked("SESSION_PROTECTION", MAC_CONF_GROUP, 0x05, &c)
    }

    /// LINK_CONFIG_CMD MODIFY with LINK_CONTEXT_MODIFY_ACTIVE (mvm/link.c): bind the link to our phy
    /// context and flip it active. phy_id/mac_id/local_link_addr are (re)applied on the transition
    /// to active; rates/bi/dtim are only applied when their own mask bits are set (not here), so we
    /// leave them 0 — they get filled by a later MODIFY at assoc. action=MODIFY(2), modify_mask@24=
    /// ACTIVE(BIT0), phy_id@12=0 (our phy), active@28=1. Full 208-byte struct (same as the ADD).
    /// Send a LINK_CONFIG_CMD MODIFY (mvm/link.c iwl_mvm_link_changed). phy_id@12=0 (our phy) and
    /// local_link_addr are always applied (but only take effect while the link is inactive), while
    /// `modify_mask` gates the rest. Used twice: (mask=0, active=0) binds the phy, then
    /// (mask=ACTIVE|RATES_INFO, active=1) activates. Full 208-byte struct.
    unsafe fn send_link_modify(&mut self, modify_mask: u32, active: u32, beacon_int: u16,
                               label: &str) -> bool {
        let mut c = [0u8; 208];
        c[0..4].copy_from_slice(&2u32.to_le_bytes());          // action = FW_CTXT_ACTION_MODIFY
        // link_id@4 = 0, mac_id@8 = 0, phy_id@12 = 0 (our phy context)
        c[16..22].copy_from_slice(&self.mac_addr);             // local_link_addr
        c[24..28].copy_from_slice(&modify_mask.to_le_bytes()); // modify_mask
        c[28..32].copy_from_slice(&active.to_le_bytes());      // active
        // Basic rates (2.4 GHz standard): cck@36 = 1/2/5.5/11 (0xF), ofdm@40 = mandatory 6/12/24.
        c[36..40].copy_from_slice(&0x0Fu32.to_le_bytes());
        c[40..44].copy_from_slice(&0x15u32.to_le_bytes());
        // bi@136 = beacon interval, dtim_interval@140 = bi * dtim_period (assume dtim_period=1).
        c[136..140].copy_from_slice(&(beacon_int as u32).to_le_bytes());
        c[140..144].copy_from_slice(&(beacon_int as u32).to_le_bytes());
        self.send_cfg_checked(label, MAC_CONF_GROUP, 0x09, &c)
    }

    /// Build + send STA_CONFIG_CMD to add the AP as a peer station. This fw doesn't list STA_CONFIG
    /// (cmd_ver 0), so the MVM MLD path uses iwl_sta_cfg_cmd_v1 (96 B). Layout: sta_id@0=0, link_id@4
    /// =0 (our link — v1/v2 use a single link_id, NOT the v3 link_mask), peer_mld_address@8 +
    /// peer_link_address@16 = AP BSSID, station_type@24=STATION_TYPE_PEER(0), assoc_id@28=0 (pre
    /// -assoc), mfp@36=0 (fw lacks STA_EXP_MFP_SUPPORT). HE/ampdu/rates are filled in at assoc time.
    unsafe fn send_sta_config(&mut self, bssid: &[u8; 6]) -> bool {
        let mut c = [0u8; 96];
        // sta_id@0 = 0, link_id@4 = 0
        c[8..14].copy_from_slice(bssid);   // peer_mld_address
        c[16..22].copy_from_slice(bssid);  // peer_link_address
        // station_type@24 = 0 (PEER), everything else 0.
        self.send_cfg_checked("STA_CONFIG", MAC_CONF_GROUP, 0x0a, &c)
    }

    /// Build + send LINK_CONFIG_CMD with action ADD (MLD). Mirrors mld/link.c iwl_mld_add_link: the
    /// ADD is minimal — link_id@4=0, mac_id@8=0 (our vif), phy_id@12=INVALID (bound later at MODIFY),
    /// local_link_addr@16=our MAC; everything else 0/inactive. The full iwl_link_config_cmd is 208 B
    /// (AC_NUM=4: ac[5] + trig_based_txf[4], both iwl_ac_qos/he_backoff = 8 B each). The details
    /// (phy binding, active=1, AP BSSID, beacon interval, rates) come in the P6b MODIFY at assoc.
    unsafe fn send_link_config_add(&mut self) -> bool {
        let mut c = [0u8; 208];
        c[0..4].copy_from_slice(&(FW_CTXT_ACTION_ADD as u32).to_le_bytes()); // action = ADD
        // link_id@4 = 0, mac_id@8 = 0
        c[12..16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());            // phy_id = FW_CTXT_ID_INVALID
        c[16..22].copy_from_slice(&self.mac_addr);                          // local_link_addr
        // active@28 = 0 (inactive), spec_link_id@166 = 0, all else 0.
        self.send_cfg_checked("LINK_CONFIG(add)", MAC_CONF_GROUP, 0x09, &c)
    }

    /// Build + send MAC_CONFIG_CMD (MLD) to create our station (vif) context. Layout
    /// (iwl_mac_config_cmd): id_and_color@0=0 (first vif fw id), action@4=ADD, mac_type@8=
    /// FW_MAC_TYPE_BSS_STA(5), local_mld_addr@12 (our MAC), reserved@18, filter_flags@20, wifi_gen
    /// union@24 (8B, 0 = no HE yet), nic_not_ack_enabled@32=0, client data@36 (16B, is_assoc=0,
    /// aid=0 pre-assoc). filter = ACCEPT_GRP(BIT2) | ACCEPT_BEACON(BIT3) while unassociated. The fw
    /// wants the v3 (52B) length when MAC_CONFIG_CMD version < 4, else the full v4 (56B).
    unsafe fn send_mac_config(&mut self) -> bool {
        let ver = self.cmd_version(MAC_CONF_GROUP, 0x08);
        let mut c = [0u8; 56];
        // id_and_color@0 = 0
        c[4..8].copy_from_slice(&(FW_CTXT_ACTION_ADD as u32).to_le_bytes()); // action = ADD
        c[8..12].copy_from_slice(&5u32.to_le_bytes());                       // mac_type = BSS_STA
        c[12..18].copy_from_slice(&self.mac_addr);                           // local_mld_addr
        // ACCEPT_GRP(multicast) | ACCEPT_BEACON — matches the driver for a station. NOT promiscuous:
        // PROMISC(BIT0) accepts every data frame on the channel, which floods our small RX ring
        // faster than we can restock and stalls RX. Unicast frames to us (the auth/assoc replies)
        // are delivered regardless of this filter.
        let filter: u32 = 0x4 | 0x8;
        c[20..24].copy_from_slice(&filter.to_le_bytes());
        // wifi_gen@24, nic_not_ack@32, client@36 all stay zero (unassociated add).
        let len = if ver >= 4 { 56 } else { 52 };
        crate::serial_println!("[WIFI] P6a: MAC_CONFIG_CMD version={} len={}", ver, len);
        self.send_cfg_checked("MAC_CONFIG", MAC_CONF_GROUP, 0x08, &c[..len])
    }

    /// Build + send PHY_CONTEXT_CMD to tune a phy context to `channel`/`band`. This fw is MLD
    /// (capa MLD_API_SUPPORT=1), so it wants the newer iwl_phy_context_cmd (VER_5/6, 32 B) — the v1
    /// layout is rejected as BAD_COMMAND. Layout: id_and_color@0=0 (flat fw phy id 0, no color),
    /// action@4=ADD, ci@8 {channel le32, band, width=MODE20, ctrl_pos=0, rsvd}, lmac_id@16=0
    /// (single-LMAC device), union@20 {sbb_bandwidth=0, sbb_ctrl_channel_loc=0, puncture_mask le16
    /// =0}, dsp_cfg_flags@24=0, secondary_ctrl_chnl_loc@28 = ctrl_pos ^ ABOVE = 0x4, reserved[3].
    unsafe fn send_phy_context(&mut self, channel: u8, band: u8) -> bool {
        let mut c = [0u8; 32];
        // id_and_color@0 = 0
        c[4..8].copy_from_slice(&(FW_CTXT_ACTION_ADD as u32).to_le_bytes());
        c[8..12].copy_from_slice(&(channel as u32).to_le_bytes()); // ci.channel
        c[12] = band;   // ci.band
        c[13] = 0;      // ci.width = MODE20
        c[14] = 0;      // ci.ctrl_pos
        // lmac_id@16 = 0. union@20 = 0: our fw has RLC_CONFIG_CMD v2, so the RX chains are configured
        // by a SEPARATE RLC_CONFIG_CMD (see send_rlc_config), NOT here — phy-ctxt.c only fills
        // rxchain_info in PHY_CONTEXT when RLC ver < 2. dsp_cfg_flags@24 = 0.
        c[28] = 0x04;   // secondary_ctrl_chnl_loc = ctrl_pos(0) ^ IWL_PHY_CTRL_POS_ABOVE(0x4)
        // Group 0x01 (LONG_GROUP): this gen2 fw puts legacy-id commands on the wire as LONG_GROUP,
        // not LEGACY(0) — same as the scan commands (0x0c/0x0d worked only in group 1). Group 0 for
        // cmd 0x08 gets BAD_COMMAND because the fw's group-0 table has no such entry.
        self.send_cfg_checked("PHY_CONTEXT(mld)", 0x01, 0x08, &c)
    }

    /// TLC_MNG_CONFIG_CMD (DATA_PATH_GROUP 0x05, cmd 0x0f). Our fw uses v4 → iwl_tlc_config_cmd_v4
    /// (28 B): sta_id@0=0, max_ch_width@4=20MHz(0), mode@5=NON_HT(0), chains@6=CHAIN_A(1),
    /// sgi@7=0, flags@8=0, non_ht_rates@10=0x0FFF (all 12 legacy 2.4 GHz rates: BIT(hw_value)),
    /// ht_rates@12 (12 B)=0, max_mpdu_len@24=0, max_tx_op@26=0.
    unsafe fn send_tlc_config(&mut self) -> bool {
        let mut c = [0u8; 28];
        // sta_id@0 = 0
        c[4] = 0;    // max_ch_width = 20 MHz
        c[5] = 0;    // mode = NON_HT
        c[6] = 1;    // chains = CHAIN_A
        c[10..12].copy_from_slice(&0x0FFFu16.to_le_bytes()); // non_ht_rates
        self.send_cfg_checked("TLC_CONFIG", DATA_PATH_GROUP, 0x0f, &c)
    }

    /// RLC_CONFIG_CMD (DATA_PATH_GROUP 0x05, cmd 0x08, ver2). For fw with RLC v2 the RX chains are
    /// configured here (not in PHY_CONTEXT). iwl_rlc_config_cmd (32 B): phy_id@0=0, rlc.rx_chain_info
    /// @4 = valid_rx_ant<<1 | idle(1)<<10 | active(1)<<12 (=0x1402 for 1×1 ant A), rlc.reserved@8,
    /// sad{chain_a@12, chain_b@16, mac_id@20, reserved@24}=0, flags@28=0, reserved[3].
    unsafe fn send_rlc_config(&mut self) -> bool {
        let mut c = [0u8; 32];
        // phy_id@0 = 0 (our phy context id)
        let rx_ant = self.valid_rx_ant.max(1) as u32;
        let rx_chain_info: u32 = (rx_ant << 1) | (1 << 10) | (1 << 12); // 0x1402
        c[4..8].copy_from_slice(&rx_chain_info.to_le_bytes());
        self.send_cfg_checked("RLC_CONFIG", DATA_PATH_GROUP, 0x08, &c)
    }

    /// P5d: build + send a passive 2.4 GHz UMAC scan. The v15 command uses the iwl_scan_req_umac_v17
    /// layout (versions 14-17 share it): uid@0, ooc_priority@4, then scan_params = general_v11(36) +
    /// channel_v7(540) + periodic_v1(12) + probe_v4(1344) = 1940 B total. Passive scan (FORCE_PASSIVE)
    /// leaves the whole probe-params region zero — no probe requests, so no aux station/TX queue. Then
    /// collect RX: beacons/probe-responses arrive as RX_MPDU (cmd 0xc1, grp 0); the scan finishes with
    /// SCAN_COMPLETE_UMAC (cmd 0x0f, grp 0x01).
    unsafe fn start_scan(&mut self) {
        // (channel_num, band) — band 1 = PHY_BAND_24 (2.4 GHz), 0 = PHY_BAND_5 (5 GHz). 2.4 GHz
        // ch1-11 plus the common 5 GHz UNII channels (incl. DFS, scanned passively). The fw filters
        // any that regulatory/NVM disallows. Room for up to 67 channels before periodic_params@584.
        const CHANS: [(u8, u8); 36] = [
            (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1), (8, 1), (9, 1), (10, 1), (11, 1),
            (36, 0), (40, 0), (44, 0), (48, 0),                 // UNII-1
            (52, 0), (56, 0), (60, 0), (64, 0),                 // UNII-2 (DFS)
            (100, 0), (104, 0), (108, 0), (112, 0), (116, 0),   // UNII-2e (DFS)
            (120, 0), (124, 0), (128, 0), (132, 0), (136, 0), (140, 0), (144, 0),
            (149, 0), (153, 0), (157, 0), (161, 0), (165, 0),   // UNII-3
        ];
        let n_chan = CHANS.len();
        self.scan_results.clear();
        let mut c = [0u8; 1940];

        // uid@0 = 1, ooc_priority@4 = IWL_SCAN_PRIORITY_EXT_6 (6).
        c[0..4].copy_from_slice(&1u32.to_le_bytes());
        c[4..8].copy_from_slice(&6u32.to_le_bytes());

        // --- general_params_v11 @8 ---
        let g = 8;
        // FORCE_PASSIVE(BIT11) | PASS_ALL(BIT1) | ADAPTIVE_DWELL(BIT7). PASS_ALL is essential:
        // without it the fw defaults to MATCH mode and reports only profile-matched APs (we have
        // none) → zero beacons, even though the scan runs and completes.
        let flags: u16 = 0x0800 | 0x0002 | 0x0080;
        c[g..g + 2].copy_from_slice(&flags.to_le_bytes());
        c[g + 3] = 0;                   // scan_start_mac_or_link_id (no MAC context yet → 0)
        c[g + 4] = 10; c[g + 5] = 10;   // active_dwell[LB, HB]
        c[g + 6] = 2;                   // adwell_default_2g
        c[g + 7] = 8;                   // adwell_default_5g
        c[g + 8] = 10;                  // adwell_default_social_chn
        c[g + 10..g + 12].copy_from_slice(&300u16.to_le_bytes()); // adwell_max_budget
        c[g + 28..g + 32].copy_from_slice(&6u32.to_le_bytes());   // scan_priority
        c[g + 32] = 110; c[g + 33] = 110; // passive_dwell[LB, HB]

        // --- channel_params_v7 @44 ---
        let ch = 44;
        c[ch] = 0x20;             // flags = IWL_SCAN_CHANNEL_FLAG_ENABLE_CHAN_ORDER (BIT5)
        c[ch + 1] = n_chan as u8; // count
        for (i, &(num, band)) in CHANS.iter().enumerate() {
            let b = ch + 4 + i * 8;   // channel_config[i] (8 B)
            // flags le32 @b = 0 (no directed-SSID bits, passive)
            c[b + 4] = num;   // v2.channel_num
            c[b + 5] = band;  // v2.band (PHY_BAND_24=1 for 2.4 GHz, PHY_BAND_5=0 for 5 GHz)
            c[b + 6] = 1;     // v2.iter_count
            // v2.iter_interval @b+7 = 0
        }
        // periodic_params @584 and probe_params @596 stay zero (one-shot passive scan).

        crate::serial_println!("[WIFI] P5d: SCAN_REQ_UMAC v15 (passive, {} chans 2.4+5GHz, {} B)...",
            n_chan, c.len());
        if !self.send_host_cmd(0x01, 0x0d, &c) { return; }
        self.delay_ms(10);
        if self.read32(CSR_INT) & INT_BIT_SW_ERR != 0 {
            crate::serial_println!("[WIFI] P5d: SW_ERR after SCAN_REQ — decoding:");
            self.report_fw_error();
            return;
        }
        crate::serial_println!("[WIFI] P5d: scan request accepted — collecting results...");

        let mut frames = 0u32;
        let mut done = false;
        // A passive sweep of 11 channels (~110 ms dwell each) takes >1 s of airtime, with idle
        // gaps between channels while the radio retunes. An empty poll therefore means "nothing
        // yet", NOT "scan finished" — we must keep collecting until SCAN_COMPLETE_UMAC arrives or a
        // generous overall budget elapses. Only idle 100 ms windows draw down the budget; beacons
        // arrive quickly and cost ~no time, so a busy channel can't starve us out early.
        let mut budget_ms: i32 = 6000;
        while budget_ms > 0 && !done {
            match self.poll_next_rx(100) {
                Some(v) => {
                    let cmd = read_volatile((v as *const u8).add(4));
                    let grp = read_volatile((v as *const u8).add(5));
                    if cmd == 0xC1 && grp == 0x00 {
                        frames += 1;
                        self.parse_beacon(v, frames);
                    } else if cmd == 0x0F && grp == 0x00 {
                        // SCAN_COMPLETE_UMAC arrives in LEGACY group 0x00 (the scan *commands* go
                        // out in group 0x01, but this notification comes back in group 0).
                        let uid = read_volatile((v as *const u8).add(8) as *const u32);
                        let st = read_volatile((v as *const u8).add(8 + 6)); // iwl_umac_scan_complete.status
                        crate::serial_println!(
                            "[WIFI] P5d: *** SCAN_COMPLETE_UMAC *** uid={:#x} status={:#04x} beacons={}",
                            uid, st, frames);
                        done = true;
                    } else {
                        let mut h = [0u8; 16];
                        for k in 0..16 { h[k] = read_volatile((v as *const u8).add(k)); }
                        crate::serial_println!("[WIFI] P5d: rx cmd={:#04x} grp={:#04x} hdr={:02x?}",
                            cmd, grp, &h[..]);
                    }
                }
                None => budget_ms -= 100,
            }
        }
        if !done {
            crate::serial_println!("[WIFI] P5d: scan ended without SCAN_COMPLETE (beacons={}).", frames);
            if self.read32(CSR_INT) & INT_BIT_SW_ERR != 0 { self.report_fw_error(); }
        }
        crate::serial_println!("[WIFI] P5d: {} unique network(s) stored.", self.scan_results.len());
    }

    /// Find a stored scan result by SSID name (exact match). Used by `connect` to resolve the
    /// target AP's BSSID/channel/band from the last scan.
    #[allow(dead_code)]
    fn find_network(&self, target: &[u8]) -> Option<ScanResult> {
        self.scan_results.iter()
            .find(|r| &r.ssid[..r.ssid_len as usize] == target)
            .copied()
    }

    /// Parse an RX_MPDU (cmd 0xc1) beacon and print SSID / BSSID / channel. We avoid depending on
    /// the exact iwl_rx_mpdu_desc size (v1 vs v3 differ) by locating the 802.11 MAC header via its
    /// beacon signature: frame-control 0x80 0x00, then (4 B later) the broadcast destination
    /// ff:ff:ff:ff:ff:ff. From there the fixed body is 24+8+2+2 B and the tagged IEs follow, first
    /// of which is the SSID (tag 0); the DS-parameter IE (tag 3) carries the channel number.
    unsafe fn parse_beacon(&mut self, v: u64, n: u32) {
        let p = v as *const u8;
        let rd = |o: usize| read_volatile(p.add(o));
        // len_n_flags@0 low 14 bits = packet byte count; bound our scan/reads by it (fallback 4096).
        let total = (read_volatile(p as *const u32) & 0x3FFF) as usize;
        let cap = if (40..=4096).contains(&total) { total } else { 4096 };

        // The iwl_rx_mpdu_desc (~40-60 B) precedes the frame; scan a generous window past it.
        let mut f = 0usize;
        let mut found = false;
        let mut o = 8;
        while o + 16 < cap && o < 220 {
            if rd(o) == 0x80 && rd(o + 1) == 0x00
                && rd(o + 4) == 0xff && rd(o + 5) == 0xff && rd(o + 6) == 0xff
                && rd(o + 7) == 0xff && rd(o + 8) == 0xff && rd(o + 9) == 0xff {
                f = o; found = true; break;
            }
            o += 1;
        }
        if !found {
            crate::serial_println!("[WIFI]   #{}: (beacon MAC header not located)", n);
            return;
        }

        let mut bssid = [0u8; 6];               // Address3 @ f+16
        for k in 0..6 { bssid[k] = rd(f + 16 + k); }

        // Tagged IEs start at f+36 (24 hdr + 8 timestamp + 2 bcn-int + 2 cap).
        let mut ssid = [0u8; 32];
        let mut ssid_len = 0usize;
        let mut channel = 0u8;
        let mut ie = f + 36;
        while ie + 2 <= cap {
            let tag = rd(ie);
            let len = rd(ie + 1) as usize;
            if ie + 2 + len > cap { break; }
            if tag == 0x00 {                    // SSID element
                ssid_len = if len > 32 { 32 } else { len };
                for k in 0..ssid_len { ssid[k] = rd(ie + 2 + k); }
            } else if tag == 0x03 && len >= 1 {  // DS Parameter Set → channel
                channel = rd(ie + 2);
            }
            ie += 2 + len;
        }

        // Sanitise SSID to printable ASCII; empty/all-zero ⇒ hidden network.
        let mut name = [0u8; 32];
        let mut hidden = ssid_len == 0;
        let mut all_zero = true;
        for k in 0..ssid_len {
            let c = ssid[k];
            if c != 0 { all_zero = false; }
            name[k] = if (0x20..0x7f).contains(&c) { c } else { b'.' };
        }
        if all_zero { hidden = true; }
        let s = core::str::from_utf8(&name[..ssid_len]).unwrap_or("?");

        // Store the network (dedup by BSSID) so `connect` can target it by SSID later. Band is
        // inferred from the channel number: ch 1-14 = 2.4 GHz (PHY_BAND_24=1), else 5 GHz (=0).
        let band = if channel <= 14 { 1 } else { 0 };
        // beacon interval (TU) sits in the fixed body: 24 hdr + 8 timestamp = f+32, le16.
        let beacon_int = (rd(f + 32) as u16) | ((rd(f + 33) as u16) << 8);
        if !self.scan_results.iter().any(|r| r.bssid == bssid) {
            let mut rec = ScanResult {
                ssid: [0; 32], ssid_len: ssid_len as u8, bssid, channel, band, beacon_int,
            };
            rec.ssid[..ssid_len].copy_from_slice(&ssid[..ssid_len]);
            self.scan_results.push(rec);
        }

        if hidden {
            crate::serial_println!(
                "[WIFI]   #{}: <hidden>  {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  ch{}",
                n, bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5], channel);
        } else {
            crate::serial_println!(
                "[WIFI]   #{}: \"{}\"  {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  ch{}",
                n, s, bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5], channel);
        }
    }

    /// Parse iwl_nvm_get_info_rsp: general{flags@0, nvm_version le16@4, board@6, n_hw_addrs@7},
    /// mac_sku{flags@8}, phy_sku{tx_chains@12, rx_chains@16}, regulatory{lar@20, n_channels@24}.
    /// Look up the fw's supported version for a command (group, cmd). 0 if the fw didn't list it.
    fn cmd_version(&self, group: u8, cmd: u8) -> u8 {
        for e in &self.fw_cmd_versions {
            if e[0] == cmd && e[1] == group { return e[2]; }
        }
        0
    }

    unsafe fn parse_nvm_info(&mut self, v: u64) {
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
        self.valid_tx_ant = (tx_chains & 0xFF) as u8;
        self.valid_rx_ant = (rx_chains & 0xFF) as u8;
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
