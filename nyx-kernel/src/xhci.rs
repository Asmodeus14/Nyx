use crate::pci::{PciDriver, PciDevice};
use alloc::string::String;
use alloc::format;

pub struct XhciDriver {
    pub controller: Option<PciDevice>,
    pub mmio_base: u32,
}

impl XhciDriver {
    pub fn new() -> Self {
        Self {
            controller: None,
            mmio_base: 0,
        }
    }

    pub fn init(&mut self) -> Result<String, &'static str> {
        let mut pci = PciDriver::new();
        let devices = pci.scan();

        // 1. Hunt for XHCI Controller (Class 0x0C, Sub 0x03, ProgIF 0x30)
        for dev in devices {
            if dev.class_id == 0x0C && dev.subclass_id == 0x03 && dev.prog_if == 0x30 {
                
                // Found Hardware. Get Physical Address.
                let bar0 = pci.get_bar0(&dev);
                let phys_addr = (bar0 & 0xFFFFFFF0) as u64; // Mask information bits
                
                // 2. Map Memory (Physical -> Virtual)
                // We need about 64KB mapped to access all registers
                let mmio_size = 65536; 

                // Acquire Memory Manager Lock
                let virt_addr = {
                    let mut mm_lock = crate::memory::MEMORY_MANAGER.lock();
                    let mm = mm_lock.as_mut().ok_or("Memory Manager not initialized")?;
                    
                    unsafe {
                        crate::memory::map_mmio(phys_addr, mmio_size, &mut mm.mapper, &mut mm.frame_allocator)?
                    }
                };
                
                self.mmio_base = virt_addr.as_u64() as u32;
                self.controller = Some(dev);

                return Ok(format!("XHCI Active @ 0x{:08x} (Mapped)", self.mmio_base));
            }
        }

        Err("No USB 3.0 (XHCI) Controller found. (Did you add -device qemu-xhci?)")
    }
}