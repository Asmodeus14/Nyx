use alloc::vec::Vec;
use x86_64::instructions::port::Port;
use crate::memory::phys_to_virt;
use crate::acpi::ACPI_INFO;

// ==========================================
// 1. LEGACY PCI STRUCTURES
// ==========================================
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

    pub fn read_config(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
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
                for func in 0..8 {
                    let vendor = Self::read_config(bus, device, func, 0) as u16;
                    if vendor != 0xFFFF {
                        let class_sub = Self::read_config(bus, device, func, 0x08);
                        devices.push(PciDevice {
                            bus, device, func,
                            vendor_id: vendor,
                            device_id: (Self::read_config(bus, device, func, 0) >> 16) as u16,
                            class_id: (class_sub >> 24) as u8,
                            subclass_id: (class_sub >> 16) as u8,
                        });
                    }
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

// ==========================================
// 2. MODERN PCIe (MCFG) STRUCTURES
// ==========================================
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

// ==========================================
// 3. THE DISPATCHER
// ==========================================
pub fn enumerate_pci() {
    crate::serial_println!("[PCI] Starting PCI(e) Bus Enumeration...");
    
    let mcfg_phys = unsafe { ACPI_INFO.mcfg_addr };
    if mcfg_phys.is_none() {
        crate::serial_println!("[PCI] No MCFG table found. Falling back to Legacy Port I/O...");
        enumerate_pci_legacy();
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
    
    crate::serial_println!("[PCI] PCIe Enumeration Complete.");
}

// ==========================================
// 4. THE LEGACY FALLBACK SCANNER
// ==========================================
fn enumerate_pci_legacy() {
    let mut driver = PciDriver::new();
    let devices = driver.scan();

    for dev in devices {
        match dev.class_id {
            0x02 => {
                crate::serial_println!("[PCI] *** FOUND NETWORK CARD: Vendor {:#06x}, Device {:#06x} ***", dev.vendor_id, dev.device_id); 
                
                // --- INTEL AX201 CNVi WI-FI (QUARANTINED) ---
                if dev.vendor_id == 0x8086 && dev.device_id == 0x06f0 {
                    crate::serial_println!("[PCI] Found Intel AX201 CNVi (Quarantined).");
                    crate::serial_println!("[PCI] WARNING: CNVi requires Intel ME sideband handshakes. Skipping to prevent CPU halt.");
                }
                
                // --- REALTEK RTL8168 ETHERNET ---
                if dev.vendor_id == 0x10ec && dev.device_id == 0x8168 {
                    crate::serial_println!("[PCI] Binding Realtek RTL8168 Ethernet Driver...");
                    
                    // Enable Bus Mastering
                    let mut cmd = PciDriver::read_config(dev.bus, dev.device, dev.func, 0x04);
                    cmd |= 0x06; 
                    let address = 0x80000000 | ((dev.bus as u32) << 16) | ((dev.device as u32) << 11) | ((dev.func as u32) << 8) | 0x04;
                    unsafe { Port::<u32>::new(0xCF8).write(address); Port::<u32>::new(0xCFC).write(cmd); }
                    
                    let mut mmio_phys = driver.get_bar_address(&dev, 2).unwrap_or(0);
                    if mmio_phys == 0 { mmio_phys = driver.get_bar_address(&dev, 0).unwrap_or(0); }

                    if mmio_phys != 0 {
                        crate::serial_println!("[PCI] RTL8168 Physical MMIO Base: {:#x}", mmio_phys);
                        let mut eth_driver = crate::drivers::net::rtl8168::Rtl8168Driver::new(dev.clone(), mmio_phys);
                        eth_driver.initialize();

                        // ADD THIS LINE TO VERIFY YOUR HARDWARE MAC:
                        crate::serial_println!("[NET] Hardware MAC Read: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", 
                            eth_driver.mac_address[0], eth_driver.mac_address[1], eth_driver.mac_address[2], 
                            eth_driver.mac_address[3], eth_driver.mac_address[4], eth_driver.mac_address[5]);

                        // === ULTIMATE MSI SETUP — 64-BIT AWARE + CORRECT ORDER + IMR + INTx DISABLE ===
                        let mut cap_ptr = (PciDriver::read_config(dev.bus, dev.device, dev.func, 0x34) & 0xFF) as u8;
                        while cap_ptr != 0 {
                            let cap = PciDriver::read_config(dev.bus, dev.device, dev.func, cap_ptr as u8);
                            let cap_id = (cap & 0xFF) as u8;
                            if cap_id == 0x05 { // MSI capability
                                let msg_ctrl = PciDriver::read_config(dev.bus, dev.device, dev.func, (cap_ptr + 2) as u8);
                                let is_64bit = (msg_ctrl & (1 << 7)) != 0;
                                
                                // 1. Address low
                                let core1_apic_id = unsafe { crate::percpu::PER_CPU.as_ref().unwrap()[1].apic_id } as u32;
                                let msi_addr_low = 0xFEE0_0000 | (core1_apic_id << 12);
                                let addr_reg = cap_ptr + 4;
                                let addr_cfg = 0x80000000 | ((dev.bus as u32)<<16) | ((dev.device as u32)<<11) | ((dev.func as u32)<<8) | (addr_reg as u32);
                                unsafe { Port::<u32>::new(0xCF8).write(addr_cfg); Port::<u32>::new(0xCFC).write(msi_addr_low); }
                                
                                // 2. Address high if 64-bit
                                if is_64bit {
                                    let high_reg = cap_ptr + 8;
                                    let high_cfg = 0x80000000 | ((dev.bus as u32)<<16) | ((dev.device as u32)<<11) | ((dev.func as u32)<<8) | (high_reg as u32);
                                    unsafe { Port::<u32>::new(0xCF8).write(high_cfg); Port::<u32>::new(0xCFC).write(0); }
                                }
                                
                                // 3. Data (vector 0x30)
                                let msi_data = 0x30u32;
                                let data_reg = if is_64bit { cap_ptr + 12 } else { cap_ptr + 8 };
                                let data_cfg = 0x80000000 | ((dev.bus as u32)<<16) | ((dev.device as u32)<<11) | ((dev.func as u32)<<8) | (data_reg as u32);
                                unsafe { Port::<u32>::new(0xCF8).write(data_cfg); Port::<u32>::new(0xCFC).write(msi_data); }
                                
                                // 4. Enable MSI + disable legacy INTx
                                let mut msg_ctrl_new = PciDriver::read_config(dev.bus, dev.device, dev.func, (cap_ptr + 2) as u8);
                                msg_ctrl_new |= 1;
                                let mut cmd_new = PciDriver::read_config(dev.bus, dev.device, dev.func, 0x04);
                                cmd_new |= 1 << 10;
                                let ctrl_cfg = 0x80000000 | ((dev.bus as u32)<<16) | ((dev.device as u32)<<11) | ((dev.func as u32)<<8) | ((cap_ptr + 2) as u32);
                                let cmd_cfg  = 0x80000000 | ((dev.bus as u32)<<16) | ((dev.device as u32)<<11) | ((dev.func as u32)<<8) | 0x04;
                                unsafe {
                                    Port::<u32>::new(0xCF8).write(ctrl_cfg); Port::<u32>::new(0xCFC).write(msg_ctrl_new);
                                    Port::<u32>::new(0xCF8).write(cmd_cfg);  Port::<u32>::new(0xCFC).write(cmd_new);
                                }
                                
                                // 5. Enable interrupts inside the NIC (IMR)
                                eth_driver.write16(0x3C, 0xFFFF);
                                crate::serial_println!("[PCI] MSI correctly enabled for RTL8168 → Core 1 (APIC ID {}, Vector 0x30, 64-bit: {})", core1_apic_id, is_64bit);
                                break;
                            }
                            cap_ptr = ((cap >> 8) & 0xFF) as u8;
                        }

                        use smoltcp::iface::{Config, Interface};
                        use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};

                        let hw_addr = HardwareAddress::Ethernet(EthernetAddress::from_bytes(&eth_driver.mac_address));
                        let mut config = Config::new();
                        config.hardware_addr = Some(hw_addr);
                        
                        let mut iface = Interface::new(config, &mut eth_driver);
                        let ip_addr = IpCidr::new(IpAddress::v4(192, 168, 1, 99), 24);
                        iface.update_ip_addrs(|ip_addrs| {
                            ip_addrs.push(ip_addr).expect("Failed to assign IP");
                        });

                        crate::serial_println!("[NET] TCP/IP Stack Online!");
                        crate::serial_println!("[NET] Assigned Static IP: {}", ip_addr);

                        *crate::drivers::net::NET_DRIVER.lock() = Some(eth_driver);
                        *crate::drivers::net::NET_IFACE.lock() = Some(iface);
                    }
                }
            }
            0x01 => crate::serial_println!("[PCI] Found Mass Storage: Vendor {:#06x}, Device {:#06x}", dev.vendor_id, dev.device_id),
            0x03 => crate::serial_println!("[PCI] *** FOUND GPU: Vendor {:#06x}, Device {:#06x} ***", dev.vendor_id, dev.device_id),
            0x0C => crate::serial_println!("[PCI] Found USB Controller: Vendor {:#06x}, Device {:#06x}", dev.vendor_id, dev.device_id),
            _ => {}
        }
    }
}

// ==========================================
// 5. THE MODERN MCFG SCANNER
// ==========================================
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
                        let subclass = unsafe { core::ptr::read_volatile((device_virt + 10) as *const u8) };

                        match class_code {
                            0x02 => {
                                crate::serial_println!("[PCI] *** FOUND NETWORK CARD: Vendor {:#06x}, Device {:#06x} ***", vendor_id, device_id); 
                                
                                // --- INTEL AX201 CNVi WI-FI (QUARANTINED) ---
                                if vendor_id == 0x8086 && device_id == 0x06f0 {
                                    crate::serial_println!("[PCI] Found Intel AX201 CNVi (Quarantined).");
                                    crate::serial_println!("[PCI] WARNING: CNVi requires Intel ME sideband handshakes. Skipping to prevent CPU halt.");
                                }
                                
                                // --- REALTEK RTL8168 ETHERNET ---
                                if vendor_id == 0x10ec && device_id == 0x8168 {
                                    crate::serial_println!("[PCI] Binding Realtek RTL8168 Ethernet Driver...");
                                    
                                    let command_ptr = (device_virt + 0x04) as *mut u16;
                                    let mut command = unsafe { core::ptr::read_volatile(command_ptr) };
                                    command |= 0x06; 
                                    unsafe { core::ptr::write_volatile(command_ptr, command) };
                                    
                                    let bar2 = unsafe { core::ptr::read_volatile((device_virt + 0x18) as *const u32) };
                                    let bar3 = unsafe { core::ptr::read_volatile((device_virt + 0x1C) as *const u32) };
                                    
                                    let mut mmio_phys = (bar2 & 0xFFFFFFF0) as u64;
                                    if (bar2 & 0b100) != 0 { mmio_phys |= (bar3 as u64) << 32; }

                                    if mmio_phys == 0 {
                                        let bar0 = unsafe { core::ptr::read_volatile((device_virt + 0x10) as *const u32) };
                                        let bar1 = unsafe { core::ptr::read_volatile((device_virt + 0x14) as *const u32) };
                                        mmio_phys = (bar0 & 0xFFFFFFF0) as u64;
                                        if (bar0 & 0b100) != 0 { mmio_phys |= (bar1 as u64) << 32; }
                                    }

                                    crate::serial_println!("[PCI] RTL8168 Physical MMIO Base: {:#x}", mmio_phys);
                                    
                                    let pci_dev = crate::pci::PciDevice {
                                        bus, device, func,
                                        vendor_id, device_id,
                                        class_id: class_code,
                                        subclass_id: subclass,
                                    };
                                    
                                    let mut eth_driver = crate::drivers::net::rtl8168::Rtl8168Driver::new(pci_dev, mmio_phys);
                                    eth_driver.initialize();

                                    // ADD THIS LINE TO VERIFY YOUR HARDWARE MAC:
                                    crate::serial_println!("[NET] Hardware MAC Read: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", 
                                        eth_driver.mac_address[0], eth_driver.mac_address[1], eth_driver.mac_address[2], 
                                        eth_driver.mac_address[3], eth_driver.mac_address[4], eth_driver.mac_address[5]);

                                    // === ULTIMATE MSI SETUP — 64-BIT AWARE + CORRECT ORDER + IMR + INTx DISABLE ===
                                    let mut cap_ptr = (PciDriver::read_config(pci_dev.bus, pci_dev.device, pci_dev.func, 0x34) & 0xFF) as u8;
                                    while cap_ptr != 0 {
                                        let cap = PciDriver::read_config(pci_dev.bus, pci_dev.device, pci_dev.func, cap_ptr as u8);
                                        let cap_id = (cap & 0xFF) as u8;
                                        if cap_id == 0x05 { // MSI capability
                                            let msg_ctrl = PciDriver::read_config(pci_dev.bus, pci_dev.device, pci_dev.func, (cap_ptr + 2) as u8);
                                            let is_64bit = (msg_ctrl & (1 << 7)) != 0;
                                            
                                            // 1. Address low
                                            let core1_apic_id = unsafe { crate::percpu::PER_CPU.as_ref().unwrap()[1].apic_id } as u32;
                                            let msi_addr_low = 0xFEE0_0000 | (core1_apic_id << 12);
                                            let addr_reg = cap_ptr + 4;
                                            let addr_cfg = 0x80000000 | ((pci_dev.bus as u32)<<16) | ((pci_dev.device as u32)<<11) | ((pci_dev.func as u32)<<8) | (addr_reg as u32);
                                            unsafe { Port::<u32>::new(0xCF8).write(addr_cfg); Port::<u32>::new(0xCFC).write(msi_addr_low); }
                                            
                                            // 2. Address high if 64-bit
                                            if is_64bit {
                                                let high_reg = cap_ptr + 8;
                                                let high_cfg = 0x80000000 | ((pci_dev.bus as u32)<<16) | ((pci_dev.device as u32)<<11) | ((pci_dev.func as u32)<<8) | (high_reg as u32);
                                                unsafe { Port::<u32>::new(0xCF8).write(high_cfg); Port::<u32>::new(0xCFC).write(0); }
                                            }
                                            
                                            // 3. Data (vector 0x30)
                                            let msi_data = 0x30u32;
                                            let data_reg = if is_64bit { cap_ptr + 12 } else { cap_ptr + 8 };
                                            let data_cfg = 0x80000000 | ((pci_dev.bus as u32)<<16) | ((pci_dev.device as u32)<<11) | ((pci_dev.func as u32)<<8) | (data_reg as u32);
                                            unsafe { Port::<u32>::new(0xCF8).write(data_cfg); Port::<u32>::new(0xCFC).write(msi_data); }
                                            
                                            // 4. Enable MSI + disable legacy INTx
                                            let mut msg_ctrl_new = PciDriver::read_config(pci_dev.bus, pci_dev.device, pci_dev.func, (cap_ptr + 2) as u8);
                                            msg_ctrl_new |= 1;
                                            let mut cmd_new = PciDriver::read_config(pci_dev.bus, pci_dev.device, pci_dev.func, 0x04);
                                            cmd_new |= 1 << 10;
                                            let ctrl_cfg = 0x80000000 | ((pci_dev.bus as u32)<<16) | ((pci_dev.device as u32)<<11) | ((pci_dev.func as u32)<<8) | ((cap_ptr + 2) as u32);
                                            let cmd_cfg  = 0x80000000 | ((pci_dev.bus as u32)<<16) | ((pci_dev.device as u32)<<11) | ((pci_dev.func as u32)<<8) | 0x04;
                                            unsafe {
                                                Port::<u32>::new(0xCF8).write(ctrl_cfg); Port::<u32>::new(0xCFC).write(msg_ctrl_new);
                                                Port::<u32>::new(0xCF8).write(cmd_cfg);  Port::<u32>::new(0xCFC).write(cmd_new);
                                            }
                                            
                                            // 5. Enable interrupts inside the NIC (IMR)
                                            eth_driver.write16(0x3C, 0xFFFF);
                                            crate::serial_println!("[PCI] MSI correctly enabled for RTL8168 → Core 1 (APIC ID {}, Vector 0x30, 64-bit: {})", core1_apic_id, is_64bit);
                                            break;
                                        }
                                        cap_ptr = ((cap >> 8) & 0xFF) as u8;
                                    }

                                    use smoltcp::iface::{Config, Interface};
                                    use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};

                                    let hw_addr = HardwareAddress::Ethernet(EthernetAddress::from_bytes(&eth_driver.mac_address));
                                    let mut config = Config::new();
                                    config.hardware_addr = Some(hw_addr);
                                    
                                    let mut iface = Interface::new(config, &mut eth_driver);
                                    let ip_addr = IpCidr::new(IpAddress::v4(192, 168, 1, 99), 24);
                                    iface.update_ip_addrs(|ip_addrs| {
                                        ip_addrs.push(ip_addr).expect("Failed to assign IP");
                                    });

                                    crate::serial_println!("[NET] TCP/IP Stack Online!");
                                    crate::serial_println!("[NET] Assigned Static IP: {}", ip_addr);

                                    *crate::drivers::net::NET_DRIVER.lock() = Some(eth_driver);
                                    *crate::drivers::net::NET_IFACE.lock() = Some(iface);
                                }
                            }
                            0x01 => crate::serial_println!("[PCI] Found Mass Storage: Vendor {:#06x}, Device {:#06x}", vendor_id, device_id),
                            0x03 => crate::serial_println!("[PCI] *** FOUND GPU: Vendor {:#06x}, Device {:#06x} ***", vendor_id, device_id),
                            0x0C => crate::serial_println!("[PCI] Found USB Controller: Vendor {:#06x}, Device {:#06x}", vendor_id, device_id),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}