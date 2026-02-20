use alloc::vec::Vec;
use x86_64::instructions::port::Port;
use crate::memory::phys_to_virt;
use crate::acpi::ACPI_INFO;

// --- LEGACY PCI (For NVMe & AHCI) ---
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub func: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_id: u8,
    pub subclass_id: u8,
}

pub struct PciDriver;

impl PciDriver {
    pub fn new() -> Self { Self }

    fn read_config(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
        let address = 0x80000000 | ((bus as u32) << 16) | ((device as u32) << 11) | ((func as u32) << 8) | (offset as u32 & 0xFC);
        let mut port_addr: Port<u32> = Port::new(0xCF8);
        let mut port_data: Port<u32> = Port::new(0xCFC);
        unsafe {
            port_addr.write(address);
            port_data.read()
        }
    }

    pub fn scan(&mut self) -> Vec<PciDevice> {
        let mut devices = Vec::new();
        for bus in 0..=255 {
            for device in 0..32 {
                let vendor = Self::read_config(bus, device, 0, 0) as u16;
                if vendor != 0xFFFF {
                    let class_sub = Self::read_config(bus, device, 0, 0x08);
                    devices.push(PciDevice {
                        bus, device, func: 0,
                        vendor_id: vendor,
                        device_id: (Self::read_config(bus, device, 0, 0) >> 16) as u16,
                        class_id: (class_sub >> 24) as u8,
                        subclass_id: (class_sub >> 16) as u8,
                    });
                }
            }
        }
        devices
    }

    pub fn get_bar_address(&self, dev: &PciDevice, bar_idx: u8) -> Option<u64> {
        let offset = 0x10 + (bar_idx * 4);
        let bar = Self::read_config(dev.bus, dev.device, dev.func, offset);
        if bar & 1 == 0 && bar != 0 {
            let mut addr = (bar & 0xFFFFFFF0) as u64;
            if (bar >> 1) & 3 == 2 {
                let bar_high = Self::read_config(dev.bus, dev.device, dev.func, offset + 4);
                addr |= (bar_high as u64) << 32;
            }
            Some(addr)
        } else { None }
    }
}

// --- MODERN PCIe (For GPU Discovery) ---
#[repr(C, packed)]
struct McfgHeader {
    signature: [u8; 4], length: u32, revision: u8, checksum: u8,
    oem_id: [u8; 6], oem_table_id: [u8; 8], oem_revision: u32,
    creator_id: u32, creator_revision: u32, reserved: u64,
}

#[repr(C, packed)]
struct McfgAllocation {
    base_address: u64, pci_segment_group: u16,
    start_bus_number: u8, end_bus_number: u8, reserved: u32,
}

pub fn enumerate_pci() {
    crate::serial_println!("[PCI] Starting PCIe Bus Enumeration...");
    
    let mcfg_phys = unsafe { ACPI_INFO.mcfg_addr };
    if mcfg_phys.is_none() {
        crate::vga_println!("[PCI] ERR: No MCFG table found.");
        return;
    }

    let mcfg_virt = phys_to_virt(mcfg_phys.unwrap()).unwrap();
    let header = unsafe { &*(mcfg_virt as *const McfgHeader) };
    let allocations_size = header.length as usize - core::mem::size_of::<McfgHeader>();
    let num_allocations = allocations_size / core::mem::size_of::<McfgAllocation>();

    let alloc_ptr = (mcfg_virt + core::mem::size_of::<McfgHeader>() as u64) as *const McfgAllocation;

    for i in 0..num_allocations {
        let alloc = unsafe { &*alloc_ptr.add(i) };
        scan_bus_range(alloc.base_address, alloc.start_bus_number, alloc.end_bus_number);
    }
}

fn scan_bus_range(base_addr: u64, start_bus: u8, end_bus: u8) {
    for bus in start_bus..=end_bus {
        for device in 0..32 {
            for func in 0..8 {
                let offset = ((bus as u64) << 20) | ((device as u64) << 15) | ((func as u64) << 12);
                if let Some(device_virt) = phys_to_virt(base_addr + offset) {
                    let vendor_id = unsafe { core::ptr::read_volatile(device_virt as *const u16) };
                    if vendor_id != 0xFFFF {
                        let device_id = unsafe { core::ptr::read_volatile((device_virt + 2) as *const u16) };
                        let class_code = unsafe { core::ptr::read_volatile((device_virt + 11) as *const u8) };

                        if class_code == 0x03 {
                            crate::vga_println!("[PCI] *** FOUND GPU: Vendor {:#06x}, Device {:#06x} ***", vendor_id, device_id);
                        }
                    }
                }
            }
        }
    }
}