// nyx-kernel/src/drivers/ahci.rs

use crate::pci::{PciDriver, PciDevice};
use crate::println;

pub struct AhciDriver {
    pub pci_device: PciDevice,
    pub abar: u64, // AHCI Base Address
}

impl AhciDriver {
    pub fn init() -> Option<Self> {
        println!("AHCI: Scanning for SATA Controller...");
        
        // 1. Use the EXISTING PCI Driver to scan
        let mut pci = PciDriver::new();
        let devices = pci.scan();

        // 2. Find the SATA Controller (Class 0x01, Subclass 0x06)
        for dev in devices {
            if dev.class_id == 0x01 && dev.subclass_id == 0x06 {
                println!("AHCI: Found SATA Device at Bus {:02X} Dev {:02X}", dev.bus, dev.device);
                
                // 3. Get BAR5 (ABAR - AHCI Base Address Register)
                // This is the memory address we write to for sending commands.
                // Note: BAR index 5 is usually the ABAR for AHCI.
                unsafe {
                    if let Some(bar5) = pci.get_bar(&dev, 5) {
                        println!("AHCI: ABAR found at 0x{:08X}", bar5);
                        
                        return Some(Self {
                            pci_device: dev,
                            abar: bar5,
                        });
                    } else {
                        println!("AHCI: Error - Could not retrieve BAR5");
                    }
                }
            }
        }
        
        println!("AHCI: No SATA Controller found.");
        None
    }
}