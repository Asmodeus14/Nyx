use crate::pci::PciDevice;
use core::ptr::{read_volatile, write_volatile};

// The raw firmware blob from Linux
const FIRMWARE_BLOB: &[u8] = include_bytes!("../../iwlwifi.ucode");

// --- INTEL HARDWARE REGISTERS (CSR) ---
const CSR_HW_IF_CONFIG_REG: usize = 0x000;
const CSR_RESET: usize = 0x010;
const CSR_GP_CNTRL: usize = 0x044;

// --- REGISTER BIT FLAGS ---
const CSR_RESET_REG_FLAG_SW_RESET: u32 = 0x80;
const CSR_RESET_REG_FLAG_MASTER_DISABLED: u32 = 0x100;
const CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY: u32 = 0x01;
const CSR_GP_CNTRL_REG_FLAG_INIT_DONE: u32 = 0x04;
const CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ: u32 = 0x08;

pub struct IntelWifiDriver {
    pci_device: PciDevice,
    mmio_base: u64,
}

impl IntelWifiDriver {
    pub fn new(device: PciDevice, bar0_phys: u64) -> Self {
        // Map the raw physical address to a safe virtual address
        let mmio_virt = crate::memory::phys_to_virt(bar0_phys).expect("Failed to map Wi-Fi BAR0");
        Self {
            pci_device: device,
            mmio_base: mmio_virt,
        }
    }

    unsafe fn write32(&self, offset: usize, value: u32) {
        write_volatile((self.mmio_base + offset as u64) as *mut u32, value);
    }

    unsafe fn read32(&self, offset: usize) -> u32 {
        read_volatile((self.mmio_base + offset as u64) as *const u32)
    }

    fn delay_ms(&self, ms: u64) {
        // A quick and dirty delay using RDTSC so we don't spam the hardware
        let mut lo: u32; let mut hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let start = ((hi as u64) << 32) | (lo as u64);
        
        // --- FIXED: Use the 24MHz invariant clock multiplier! ---
        let wait_ticks = ms * 24_000; 
        
        loop {
            unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
            let now = ((hi as u64) << 32) | (lo as u64);
            if now - start > wait_ticks { break; }
            core::hint::spin_loop();
        }
    }

    pub fn initialize(&mut self) {
        crate::serial_println!("[WIFI] Initializing Intel AX201 CNVi...");
        crate::serial_println!("[WIFI] Loaded Firmware Blob size: {} bytes", FIRMWARE_BLOB.len());

        unsafe {
            // 1. Ask the hardware for permission to configure it
            self.set_mac_access();

            // 2. Trigger a Software Reset
            let mut reset_val = self.read32(CSR_RESET);
            self.write32(CSR_RESET, reset_val | CSR_RESET_REG_FLAG_SW_RESET);
            self.delay_ms(10); 

            // 3. Clear the reset flag
            self.write32(CSR_RESET, reset_val & !CSR_RESET_REG_FLAG_SW_RESET);
            self.delay_ms(10);

            // 4. Verify the hardware survived the reset
            let reset_check = self.read32(CSR_RESET);
            if (reset_check & CSR_RESET_REG_FLAG_MASTER_DISABLED) != 0 {
                crate::serial_println!("[WIFI] FATAL: Hardware Master is disabled after reset!");
                return;
            }

            crate::serial_println!("[WIFI] Hardware reset successful. Ready for Firmware Push.");
        }
    }

    unsafe fn set_mac_access(&self) {
        crate::serial_println!("[WIFI] Requesting MAC Clock Access...");
        
        let mut val = self.read32(CSR_GP_CNTRL);
        val |= CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ;
        self.write32(CSR_GP_CNTRL, val);

        // We have to wait for the card's internal processor to wake up and grant us the clock
        let mut ready = false;
        for _ in 0..1000 {
            let check = self.read32(CSR_GP_CNTRL);
            if (check & CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY) != 0 &&
               (check & CSR_GP_CNTRL_REG_FLAG_INIT_DONE) != 0 {
                ready = true;
                break;
            }
            self.delay_ms(1);
        }

        if ready {
            crate::serial_println!("[WIFI] MAC Access GRANTED!");
        } else {
            crate::serial_println!("[WIFI] ERR: Timeout waiting for MAC Access.");
        }
    }
}