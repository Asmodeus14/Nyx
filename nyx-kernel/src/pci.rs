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
    pub prog_if: u8,
}

impl PciDevice {
    pub fn to_string(&self) -> String {
        format!("Bus {:02x} Dev {:02x}: [{:04x}:{:04x}] Class {:02x}", 
            self.bus, self.device, self.vendor_id, self.device_id, self.class_id)
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

    pub unsafe fn write(&mut self, bus: u8, device: u8, func: u8, offset: u8, value: u32) {
        let address = 0x80000000 
            | ((bus as u32) << 16)
            | ((device as u32) << 11)
            | ((func as u32) << 8)
            | ((offset as u32) & 0xFC);

        self.address_port.write(address);
        self.data_port.write(value);
    }

    pub fn enable_bus_master(&mut self, dev: &PciDevice) {
        unsafe {
            let cmd_offset = 0x04;
            let command_reg = self.read(dev.bus, dev.device, 0, cmd_offset);
            // Enable IO, Memory (Bit 1), and Bus Master (Bit 2)
            let new_command = command_reg | 0x06; 
            self.write(dev.bus, dev.device, 0, cmd_offset, new_command);
        }
    }

    // FIX: Handle 64-bit Addresses properly
    pub fn get_bar_address(&mut self, dev: &PciDevice, bar_index: u8) -> Option<u64> {
        unsafe {
            let offset = 0x10 + (bar_index * 4);
            let bar_low = self.read(dev.bus, dev.device, 0, offset);
            
            // Check for valid MMIO (Bit 0 must be 0)
            if bar_low & 0x1 != 0 { return None; } // IO Space (not supported for xHCI)

            let bar_type = (bar_low >> 1) & 0x3; // Bits 1-2: Type
            
            let addr_low = (bar_low & 0xFFFFFFF0) as u64;

            if bar_type == 0x0 { // 32-bit Address
                Some(addr_low)
            } else if bar_type == 0x2 { // 64-bit Address
                let bar_high = self.read(dev.bus, dev.device, 0, offset + 4);
                let addr_high = (bar_high as u64) << 32;
                Some(addr_high | addr_low)
            } else {
                None // Reserved
            }
        }
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