use crate::pci::{PciDriver, PciDevice};
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

// --- CONSTANTS ---
const NVME_ADMIN_OP_CREATE_SQ: u8 = 0x01;
const NVME_ADMIN_OP_CREATE_CQ: u8 = 0x05;
const NVME_ADMIN_OP_IDENTIFY: u8 = 0x06;
const NVME_IO_OP_READ: u8 = 0x02;

// --- DMA BUFFERS (Aligned to 4096) ---
#[repr(align(4096))]
struct Page([u8; 4096]);

static mut ADMIN_SQ: Page = Page([0; 4096]);
static mut ADMIN_CQ: Page = Page([0; 4096]);
static mut IO_SQ: Page = Page([0; 4096]); 
static mut IO_CQ: Page = Page([0; 4096]); 
static mut DATA_BUF: Page = Page([0; 4096]); // Unified buffer for Identify/Read

// --- STRUCTS ---
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct NvmeCmd {
    pub opcode: u8,
    pub flags: u8,
    pub cid: u16,
    pub nsid: u32,
    pub rsvd: u64,
    pub mptr: u64,
    pub prp1: u64,
    pub prp2: u64,
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct NvmeCpl {
    pub cdw0: u32,
    pub rsvd: u32,
    pub sq_head: u16,
    pub sq_id: u16,
    pub cid: u16,
    pub status: u16, // bit 0: Phase, bit 1-14: Status Code
}

#[repr(C)]
pub struct NvmeRegisters {
    pub cap: u64, pub vs: u32, pub intms: u32, pub intmc: u32,
    pub cc: u32, pub rsv0: u32, pub csts: u32, pub nssr: u32,
    pub aqa: u32, pub asq: u64, pub acq: u64,
    pub cmbloc: u32, pub cmbsz: u32,
}

pub struct NvmeDriver {
    pub device: PciDevice,
    pub bar0: u64,
    pub regs: &'static mut NvmeRegisters,
    pub doorbell_stride: usize,
    pub sq_tail: u16,
    pub cq_head: u16,
    // IO Queue State
    pub io_sq_tail: u16,
    pub io_cq_head: u16,
    pub active_nsid: u32, // Stored Namespace ID
}

impl NvmeDriver {
    pub fn init() -> Option<Self> {
        let mut pci = PciDriver::new();
        let devices = pci.scan();

        for dev in devices {
            if dev.class_id == 0x01 && dev.subclass_id == 0x08 {
                if let Some(bar0) = pci.get_bar_address(&dev, 0) {
                    unsafe {
                        if crate::memory::map_mmio(bar0, 0x4000).is_ok() {
                            let regs = &mut *(bar0 as *mut NvmeRegisters);
                            let cap = regs.cap;
                            let dstrd = ((cap >> 32) & 0xF) as usize;
                            let stride = 1 << (2 + dstrd);

                            let mut driver = Self { 
                                device: dev, bar0, regs, doorbell_stride: stride,
                                sq_tail: 0, cq_head: 0,
                                io_sq_tail: 0, io_cq_head: 0, active_nsid: 0
                            };
                            
                            if driver.init_controller() { return Some(driver); }
                        }
                    }
                }
            }
        }
        None
    }

    fn init_controller(&mut self) -> bool {
        unsafe {
            // Disable
            let cc = read_volatile(&self.regs.cc);
            if (cc & 1) != 0 {
                write_volatile(&mut self.regs.cc, cc & !1);
                for _ in 0..5000 { if (read_volatile(&self.regs.csts) & 1) == 0 { break; } core::hint::spin_loop(); }
            }
            
            // Set Admin Queues
            let asq_phys = crate::memory::virt_to_phys(&ADMIN_SQ as *const _ as u64).unwrap();
            let acq_phys = crate::memory::virt_to_phys(&ADMIN_CQ as *const _ as u64).unwrap();
            
            write_volatile(&mut self.regs.aqa, (1 << 16) | 1); // Depth 2
            write_volatile(&mut self.regs.asq, asq_phys);
            write_volatile(&mut self.regs.acq, acq_phys);
            
            // Enable (IOSQES=6, IOCQES=4, EN=1)
            write_volatile(&mut self.regs.cc, (6 << 16) | (4 << 20) | 1);
            for _ in 0..5000 { if (read_volatile(&self.regs.csts) & 1) != 0 { break; } core::hint::spin_loop(); }
        }
        true 
    }
    
    pub fn get_version(&self) -> (u16, u8) {
        let vs = self.regs.vs;
        ((vs >> 16) as u16, (vs >> 8) as u8)
    }

    // --- ADMIN COMMANDS ---
    
    unsafe fn submit_admin(&mut self, cmd: NvmeCmd) -> bool {
        let sq = &mut *(&mut ADMIN_SQ.0 as *mut _ as *mut [NvmeCmd; 64]);
        sq[self.sq_tail as usize] = cmd;
        
        self.sq_tail = (self.sq_tail + 1) % 2;
        let db_addr = self.bar0 + 0x1000;
        write_volatile(db_addr as *mut u32, self.sq_tail as u32);
        
        let cq = &mut *(&mut ADMIN_CQ.0 as *mut _ as *mut [NvmeCpl; 256]);
        for _ in 0..1_000_000 {
            let status = read_volatile(&cq[self.cq_head as usize].status);
            // Check Phase Bit (We expect it to flip, but simple check is active)
            if (status & 1) != 0 || cq[self.cq_head as usize].cid == cmd.cid {
                 
                 // CHECK SC (Status Code) - Bits 1-8 should be 0 for Success
                 let sc = (status >> 1) & 0xFF;
                 
                 self.cq_head = (self.cq_head + 1) % 2;
                 let cq_db = self.bar0 + 0x1000 + self.doorbell_stride as u64;
                 write_volatile(cq_db as *mut u32, self.cq_head as u32);
                 
                 return sc == 0; // Return TRUE only if Status is Success
            }
            core::hint::spin_loop();
        }
        false
    }

    pub fn identify_controller(&mut self) -> Option<&'static [u8; 4096]> {
        let buf_phys = crate::memory::virt_to_phys(unsafe { &DATA_BUF } as *const _ as u64).unwrap();
        let cmd = NvmeCmd {
            opcode: NVME_ADMIN_OP_IDENTIFY,
            flags: 0, cid: 1, nsid: 0, rsvd: 0, mptr: 0,
            prp1: buf_phys, prp2: 0,
            cdw10: 1, // CNS=1 (Identify Controller)
            cdw11: 0, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
        };
        if unsafe { self.submit_admin(cmd) } { unsafe { Some(&DATA_BUF.0) } } else { None }
    }

    // NEW: Find the first active Namespace ID
    pub fn find_active_namespace(&mut self) -> u32 {
        let buf_phys = crate::memory::virt_to_phys(unsafe { &DATA_BUF } as *const _ as u64).unwrap();
        // CNS=2 (Active Namespace ID list)
        let cmd = NvmeCmd {
            opcode: NVME_ADMIN_OP_IDENTIFY,
            flags: 0, cid: 2, nsid: 0, rsvd: 0, mptr: 0,
            prp1: buf_phys, prp2: 0,
            cdw10: 2, cdw11: 0, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
        };
        
        if unsafe { self.submit_admin(cmd) } {
            // The buffer now contains a list of u32 Namespace IDs
            let ns_list = unsafe { &*(&DATA_BUF.0 as *const _ as *const [u32; 1024]) };
            if ns_list[0] != 0 {
                self.active_nsid = ns_list[0];
                return ns_list[0];
            }
        }
        // Fallback
        self.active_nsid = 1; 
        1
    }

    pub fn create_io_queues(&mut self) -> bool {
        unsafe {
            // CQ
            let cq_phys = crate::memory::virt_to_phys(&IO_CQ as *const _ as u64).unwrap();
            let cmd_cq = NvmeCmd {
                opcode: NVME_ADMIN_OP_CREATE_CQ,
                flags: 0, cid: 3, nsid: 0, rsvd: 0, mptr: 0,
                prp1: cq_phys, prp2: 0,
                cdw10: (1 << 16) | 1, cdw11: 1, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
            };
            if !self.submit_admin(cmd_cq) { return false; }

            // SQ
            let sq_phys = crate::memory::virt_to_phys(&IO_SQ as *const _ as u64).unwrap();
            let cmd_sq = NvmeCmd {
                opcode: NVME_ADMIN_OP_CREATE_SQ,
                flags: 0, cid: 4, nsid: 0, rsvd: 0, mptr: 0,
                prp1: sq_phys, prp2: 0,
                cdw10: (1 << 16) | 1, cdw11: (1 << 16) | 1, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
            };
            if !self.submit_admin(cmd_sq) { return false; }
            
            true
        }
    }

    // Uses the INTERNAL buffer to guarantee alignment
    pub fn test_read_sector0(&mut self) -> Option<&'static [u8]> {
        if self.active_nsid == 0 { self.find_active_namespace(); }
        
        unsafe {
            let buf_phys = crate::memory::virt_to_phys(&DATA_BUF as *const _ as u64).unwrap();
            let sq = &mut *(&mut IO_SQ.0 as *mut _ as *mut [NvmeCmd; 64]);
            
            sq[self.io_sq_tail as usize] = NvmeCmd {
                opcode: NVME_IO_OP_READ,
                flags: 0, cid: 5, 
                nsid: self.active_nsid, // Use detected NSID
                rsvd: 0, mptr: 0,
                prp1: buf_phys, prp2: 0,
                cdw10: 0, // LBA 0
                cdw11: 0, 
                cdw12: 0, // 1 Block
                cdw13: 0, cdw14: 0, cdw15: 0
            };

            self.io_sq_tail = (self.io_sq_tail + 1) % 2;
            let db_addr = self.bar0 + 0x1000 + (2 * self.doorbell_stride as u64);
            write_volatile(db_addr as *mut u32, self.io_sq_tail as u32);

            let cq = &mut *(&mut IO_CQ.0 as *mut _ as *mut [NvmeCpl; 256]);
            for _ in 0..1_000_000 {
                let status = read_volatile(&cq[self.io_cq_head as usize].status);
                if (status & 1) != 0 || cq[self.io_cq_head as usize].cid == 5 {
                    
                    let sc = (status >> 1) & 0xFF; // Status Code

                    self.io_cq_head = (self.io_cq_head + 1) % 2;
                    let cq_db = self.bar0 + 0x1000 + (3 * self.doorbell_stride as u64);
                    write_volatile(cq_db as *mut u32, self.io_cq_head as u32);
                    
                    if sc == 0 { return Some(&DATA_BUF.0); } else { return None; }
                }
                core::hint::spin_loop();
            }
        }
        None
    }
}