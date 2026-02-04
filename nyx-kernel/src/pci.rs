use x86_64::instructions::port::Port;
use alloc::vec::Vec;
use alloc::format;
use alloc::string::String;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

#[derive(Debug, Clone)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_id: u8,
    pub subclass_id: u8,
    pub prog_if: u8, // NEW: Interface type (Required for XHCI)
}

impl PciDevice {
    pub fn to_string(&self) -> String {
        let class_name = match self.class_id {
            0x01 => "Mass Storage",
            0x02 => "Network",
            0x03 => "Display",
            0x06 => "Bridge",
            0x0C => match self.subclass_id {
                0x03 => match self.prog_if {
                    0x00 => "USB (UHCI)",
                    0x10 => "USB (OHCI)",
                    0x20 => "USB (EHCI)",
                    0x30 => "USB 3.0 (XHCI)", // Target
                    _ => "USB Controller",
                },
                _ => "Serial Bus",
            },
            _ => "Unknown",
        };

        format!("Bus {:02x} Dev {:02x}: [{:04x}:{:04x}] {}", 
            self.bus, self.device, self.vendor_id, self.device_id, class_name)
    }
}

pub struct PciDriver {
    address_port: Port<u32>,
    data_port: Port<u32>,
}

impl PciDriver {
    pub fn new() -> Self {
        Self {
            address_port: Port::new(CONFIG_ADDRESS),
            data_port: Port::new(CONFIG_DATA),
        }
    }

    pub unsafe fn read(&mut self, bus: u8, device: u8, func: u8, offset: u8) -> u32 {
        let address = 0x80000000 
            | ((bus as u32) << 16)
            | ((device as u32) << 11)
            | ((func as u32) << 8)
            | ((offset as u32) & 0xFC);

        self.address_port.write(address);
        self.data_port.read()
    }

    /// Helper to get the MMIO address (Base Address Register 0)
    pub fn get_bar0(&mut self, dev: &PciDevice) -> u32 {
        unsafe { self.read(dev.bus, dev.device, 0, 0x10) }
    }

    pub fn scan(&mut self) -> Vec<PciDevice> {
        let mut devices = Vec::new();

        for bus in 0..=255 {
            for dev in 0..32 {
                unsafe {
                    let offset0 = self.read(bus, dev, 0, 0);
                    let vendor_id = (offset0 & 0xFFFF) as u16;

                    if vendor_id != 0xFFFF {
                        let device_id = (offset0 >> 16) as u16;
                        let offset8 = self.read(bus, dev, 0, 0x08);
                        let class_id = (offset8 >> 24) as u8;
                        let subclass_id = (offset8 >> 16) as u8;
                        let prog_if = (offset8 >> 8) as u8;

                        devices.push(PciDevice {
                            bus, device: dev,
                            vendor_id, device_id, class_id, subclass_id, prog_if
                        });
                    }
                }
            }
        }
        devices
    }
}