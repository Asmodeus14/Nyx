use crate::pci::{PciDriver, PciDevice};
use core::mem::size_of;

// --- AHCI MEMORY STRUCTURES ---

#[repr(C)]
pub struct HbaMemory {
    pub cap: u32,       pub ghc: u32,       pub is: u32,        pub pi: u32,
    pub vs: u32,        pub ccc_ctl: u32,   pub ccc_pts: u32,   pub em_loc: u32,
    pub em_ctl: u32,    pub cap2: u32,      pub bohc: u32,      pub rsv: [u8; 0x74],
    pub vendor: [u8; 0xA0], 
    pub ports: [HbaPort; 32], 
}

#[repr(C)]
pub struct HbaPort {
    pub clb: u32,   pub clbu: u32,  pub fb: u32,    pub fbu: u32,
    pub is: u32,    pub ie: u32,    pub cmd: u32,   pub rsv0: u32,
    pub tfd: u32,   pub sig: u32,   pub ssts: u32,  pub sctl: u32,
    pub serr: u32,  pub sact: u32,  pub ci: u32,    pub sntf: u32,
    pub fbs: u32,   pub rsv1: [u32; 11], pub vendor: [u32; 4],
}

#[repr(C, packed)]
pub struct FisRegH2D {
    pub fis_type: u8,   // 0x27
    pub pmport: u8,     
    pub command: u8,    
    pub featurel: u8,   
    pub lba0: u8,       
    pub lba1: u8,
    pub lba2: u8,
    pub device: u8,     
    pub lba3: u8,       
    pub lba4: u8,
    pub lba5: u8,
    pub featureh: u8,   
    pub countl: u8,     
    pub counth: u8,     
    pub icc: u8,        
    pub control: u8,    
    pub rsv1: [u8; 4],  
}

#[repr(C)]
pub struct CommandHeader {
    pub opts: u16,      // Bit 0-4: CFL, Bit 5: Atapi, Bit 6: Write, Bit 7: Prefetchable
    pub prdtl: u16,     // PRDT Length
    pub prdbc: u32,     // PRD Byte Count
    pub ctba: u32,      // Command Table Base Address
    pub ctbau: u32,     // Upper 32-bits
    pub rsv: [u32; 4],
}

#[repr(C)]
pub struct CommandTable {
    pub cfis: [u8; 64], 
    pub acmd: [u8; 16], 
    pub rsv: [u8; 48],  
    pub prdt: [PrdtEntry; 1], 
}

#[repr(C)]
pub struct PrdtEntry {
    pub dba: u32,       
    pub dbau: u32,      
    pub rsv: u32,       
    pub dbc: u32,       // Byte Count - 1
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum PortType { None, SATA, SATAPI, SEMB, PM, Unknown(u32) }

pub struct AhciDriver {
    pub device: PciDevice,
    pub abar: u64, 
    pub mem: &'static mut HbaMemory, 
}

impl AhciDriver {
    pub fn init() -> Option<Self> {
        let mut pci = PciDriver::new();
        let devices = pci.scan();

        for dev in devices {
            if dev.class_id == 0x01 && dev.subclass_id == 0x06 {
                if let Some(bar5) = pci.get_bar_address(&dev, 5) {
                    unsafe {
                        if crate::memory::map_mmio(bar5, 0x2000).is_ok() {
                            let hba_mem = &mut *(bar5 as *mut HbaMemory);
                            let mut driver = Self { device: dev, abar: bar5, mem: hba_mem };
                            driver.configure();
                            return Some(driver);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn configure(&mut self) {
        self.mem.ghc |= 1 << 31; // Enable AHCI Mode
        
        let cap = self.mem.cap;
        let supports_spinup = (cap & (1 << 27)) != 0;
        let pi = self.mem.pi;

        for i in 0..32 {
            if (pi >> i) & 1 == 1 {
                let port = &mut self.mem.ports[i];
                
                // Stop engine
                port.cmd &= !0x11;
                for _ in 0..1000 {
                    if (port.cmd & 0xC000) == 0 { break; }
                    core::hint::spin_loop();
                }

                if supports_spinup {
                    port.cmd |= 1 << 1; // SUD
                }

                // Power management: Active state
                port.cmd = (port.cmd & !0xF0000000) | 0x10000000;

                // COMRESET
                port.sctl = (port.sctl & !0x0F) | 1;
                for _ in 0..1_000_000 { core::hint::spin_loop(); }
                port.sctl &= !0x0F;

                // Wait for link
                for _ in 0..200 {
                    if (port.ssts & 0x0F) == 3 { break; }
                    for _ in 0..1_000_000 { core::hint::spin_loop(); }
                }

                port.serr = 0xFFFFFFFF;
            }
        }
    }

    pub fn check_type(&self, port_index: usize) -> PortType {
        let port = &self.mem.ports[port_index];
        let ssts = port.ssts;
        if (ssts & 0x0F) != 3 { return PortType::None; }

        match port.sig {
            0x00000101 => PortType::SATA,
            0xEB140101 => PortType::SATAPI,
            sig => PortType::Unknown(sig),
        }
    }

    pub unsafe fn read(&mut self, port_no: usize, sector: u64, buf: &mut [u8]) -> bool {
        let port = &mut self.mem.ports[port_no];
        port.is = 0xFFFFFFFF;

        let clb = (port.clb as u64) | ((port.clbu as u64) << 32);
        let cmd_header = &mut *(clb as *mut CommandHeader);
        
        // CFL is length in DWORDS
        let cfl = (size_of::<FisRegH2D>() / 4) as u16;
        cmd_header.opts = cfl; // Bit 6 (Write) is 0 for Read
        cmd_header.prdtl = 1;

        let ctba = (cmd_header.ctba as u64) | ((cmd_header.ctbau as u64) << 32);
        let cmd_table = &mut *(ctba as *mut CommandTable);
        
        for i in 0..64 { cmd_table.cfis[i] = 0; }
        let fis = &mut *(cmd_table.cfis.as_mut_ptr() as *mut FisRegH2D);
        
        fis.fis_type = 0x27;
        fis.command = 0x25; // READ DMA EXT
        fis.lba0 = sector as u8;
        fis.lba1 = (sector >> 8) as u8;
        fis.lba2 = (sector >> 16) as u8;
        fis.device = 1 << 6; // LBA mode
        fis.lba3 = (sector >> 24) as u8;
        fis.lba4 = (sector >> 32) as u8;
        fis.lba5 = (sector >> 40) as u8;
        fis.countl = 1;

        let buf_phys = crate::memory::virt_to_phys(buf.as_ptr() as u64).unwrap_or(0);
        if buf_phys == 0 { return false; }

        cmd_table.prdt[0].dba = buf_phys as u32;
        cmd_table.prdt[0].dbau = (buf_phys >> 32) as u32;
        cmd_table.prdt[0].dbc = 511; 

        while (port.ci & 1) != 0 { core::hint::spin_loop(); }
        port.ci |= 1;

        loop {
            if (port.ci & 1) == 0 { break; }
            if (port.is & (1 << 30)) != 0 { return false; }
            core::hint::spin_loop();
        }

        true
    }
}