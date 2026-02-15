use crate::pci::{PciDriver, PciDevice};
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

// --- CONSTANTS ---
const NVME_ADMIN_OP_CREATE_SQ: u8 = 0x01;
const NVME_ADMIN_OP_CREATE_CQ: u8 = 0x05;
const NVME_ADMIN_OP_IDENTIFY: u8 = 0x06;
const NVME_IO_OP_READ: u8 = 0x02;
const NVME_IO_OP_WRITE: u8 = 0x01;

// --- DMA BUFFERS (Aligned to 4096) ---
#[repr(align(4096))]
struct Page([u8; 4096]);

static mut ADMIN_SQ: Page = Page([0; 4096]);
static mut ADMIN_CQ: Page = Page([0; 4096]);
static mut IO_SQ: Page = Page([0; 4096]); 
static mut IO_CQ: Page = Page([0; 4096]); 
static mut DATA_BUF: Page = Page([0; 4096]); 

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
    pub status: u16, 
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
    pub admin_phase: u16,
    pub io_sq_tail: u16,
    pub io_cq_head: u16,
    pub io_phase: u16,    
    pub active_nsid: u32, 
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

                            // ZERO MEMORY TO PREVENT STALE COMPLETIONS
                            core::ptr::write_bytes(ADMIN_SQ.0.as_mut_ptr(), 0, 4096);
                            core::ptr::write_bytes(ADMIN_CQ.0.as_mut_ptr(), 0, 4096);
                            core::ptr::write_bytes(IO_SQ.0.as_mut_ptr(), 0, 4096);
                            core::ptr::write_bytes(IO_CQ.0.as_mut_ptr(), 0, 4096);

                            let mut driver = Self { 
                                device: dev, bar0, regs, doorbell_stride: stride,
                                sq_tail: 0, cq_head: 0, admin_phase: 1, 
                                io_sq_tail: 0, io_cq_head: 0, io_phase: 1,
                                active_nsid: 0
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
                for _ in 0..50000 { if (read_volatile(&self.regs.csts) & 1) == 0 { break; } core::hint::spin_loop(); }
            }
            
            let asq_phys = crate::memory::virt_to_phys(&ADMIN_SQ as *const _ as u64).unwrap();
            let acq_phys = crate::memory::virt_to_phys(&ADMIN_CQ as *const _ as u64).unwrap();
            
            write_volatile(&mut self.regs.aqa, (31 << 16) | 31); 
            write_volatile(&mut self.regs.asq, asq_phys);
            write_volatile(&mut self.regs.acq, acq_phys);
            
            // Enable
            write_volatile(&mut self.regs.cc, (6 << 16) | (4 << 20) | 1);
            
            for _ in 0..50000 { if (read_volatile(&self.regs.csts) & 1) != 0 { break; } core::hint::spin_loop(); }
        }
        true 
    }
    
    pub fn get_version(&self) -> (u16, u8) {
        let vs = self.regs.vs;
        ((vs >> 16) as u16, (vs >> 8) as u8)
    }

    unsafe fn submit_admin(&mut self, cmd: NvmeCmd) -> bool {
        let sq = &mut *(&mut ADMIN_SQ.0 as *mut _ as *mut [NvmeCmd; 64]);
        sq[self.sq_tail as usize] = cmd;
        self.sq_tail = (self.sq_tail + 1) % 32;
        
        let db_addr = self.bar0 + 0x1000;
        write_volatile(db_addr as *mut u32, self.sq_tail as u32);
        
        let cq = &mut *(&mut ADMIN_CQ.0 as *mut _ as *mut [NvmeCpl; 256]);
        
        for _ in 0..10_000_000 {
            let status_raw = read_volatile(&cq[self.cq_head as usize].status);
            let phase_tag = (status_raw & 1) as u16;

            if phase_tag == self.admin_phase {
                 let sc = (status_raw >> 1) & 0xFF;
                 self.cq_head = (self.cq_head + 1) % 32;
                 if self.cq_head == 0 { self.admin_phase ^= 1; }
                 
                 let cq_db = self.bar0 + 0x1000 + self.doorbell_stride as u64;
                 write_volatile(cq_db as *mut u32, self.cq_head as u32);
                 return sc == 0; 
            }
            core::hint::spin_loop();
        }
        false
    }

    pub fn find_active_namespace(&mut self) -> u32 {
        let buf_phys = crate::memory::virt_to_phys(unsafe { &DATA_BUF } as *const _ as u64).unwrap();
        let cmd = NvmeCmd {
            opcode: NVME_ADMIN_OP_IDENTIFY,
            flags: 0, cid: 2, nsid: 0, rsvd: 0, mptr: 0,
            prp1: buf_phys, prp2: 0,
            cdw10: 2, cdw11: 0, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
        };
        
        if unsafe { self.submit_admin(cmd) } {
            let ns_list = unsafe { &*(&DATA_BUF.0 as *const _ as *const [u32; 1024]) };
            if ns_list[0] != 0 {
                self.active_nsid = ns_list[0];
                return ns_list[0];
            }
        }
        self.active_nsid = 1; 
        1
    }

    pub fn create_io_queues(&mut self) -> bool {
        unsafe {
            // CQ
            let cq_phys = crate::memory::virt_to_phys(&IO_CQ as *const _ as u64).unwrap();
            let cmd_cq = NvmeCmd {
                opcode: NVME_ADMIN_OP_CREATE_CQ,
                flags: 0, cid: 3, nsid: 0, rsvd: 0, mptr: 0, prp1: cq_phys, prp2: 0,
                cdw10: (15 << 16) | 1, cdw11: 1, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
            };
            if !self.submit_admin(cmd_cq) { return false; }

            // SQ
            let sq_phys = crate::memory::virt_to_phys(&IO_SQ as *const _ as u64).unwrap();
            let cmd_sq = NvmeCmd {
                opcode: NVME_ADMIN_OP_CREATE_SQ,
                flags: 0, cid: 4, nsid: 0, rsvd: 0, mptr: 0, prp1: sq_phys, prp2: 0,
                cdw10: (15 << 16) | 1, cdw11: (1 << 16) | 1, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
            };
            if !self.submit_admin(cmd_sq) { return false; }
            true
        }
    }

    pub fn read_block(&mut self, lba: u64, buffer: &mut [u8]) -> bool {
        if self.active_nsid == 0 { self.find_active_namespace(); }
        if buffer.len() != 4096 { return false; } 

        unsafe {
            let buf_phys = crate::memory::virt_to_phys(unsafe { &DATA_BUF } as *const _ as u64).unwrap();
            
            let sq = &mut *(&mut IO_SQ.0 as *mut _ as *mut [NvmeCmd; 64]);
            sq[self.io_sq_tail as usize] = NvmeCmd {
                opcode: NVME_IO_OP_READ,
                flags: 0, cid: 5, nsid: self.active_nsid,
                rsvd: 0, mptr: 0, prp1: buf_phys, prp2: 0,
                cdw10: lba as u32, cdw11: (lba >> 32) as u32,
                cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
            };

            self.io_sq_tail = (self.io_sq_tail + 1) % 16;
            let db_addr = self.bar0 + 0x1000 + (2 * self.doorbell_stride as u64);
            write_volatile(db_addr as *mut u32, self.io_sq_tail as u32);

            let cq = &mut *(&mut IO_CQ.0 as *mut _ as *mut [NvmeCpl; 256]);
            
            for _ in 0..10_000_000 {
                let status_raw = read_volatile(&cq[self.io_cq_head as usize].status);
                let phase = (status_raw & 1) as u16;
                
                if phase == self.io_phase {
                    let sc = (status_raw >> 1) & 0xFF;
                    self.io_cq_head = (self.io_cq_head + 1) % 16;
                    
                    if self.io_cq_head == 0 { self.io_phase ^= 1; }
                    
                    let cq_db = self.bar0 + 0x1000 + (3 * self.doorbell_stride as u64);
                    write_volatile(cq_db as *mut u32, self.io_cq_head as u32);
                    
                    if sc == 0 {
                        buffer.copy_from_slice(&DATA_BUF.0);
                        return true;
                    } else {
                        return false;
                    }
                }
                core::hint::spin_loop();
            }
        }
        false
    }
    
    pub fn write_block(&mut self, lba: u64, data: &[u8]) -> bool {
        if self.active_nsid == 0 { self.find_active_namespace(); }
        if data.len() != 4096 { return false; } 

        unsafe {
            let dma = &mut DATA_BUF.0;
            dma.copy_from_slice(data);
            let buf_phys = crate::memory::virt_to_phys(dma as *const _ as u64).unwrap();
            
            let sq = &mut *(&mut IO_SQ.0 as *mut _ as *mut [NvmeCmd; 64]);
            sq[self.io_sq_tail as usize] = NvmeCmd {
                opcode: NVME_IO_OP_WRITE,
                flags: 0, cid: 6, nsid: self.active_nsid,
                rsvd: 0, mptr: 0, prp1: buf_phys, prp2: 0,
                cdw10: lba as u32, cdw11: (lba >> 32) as u32,
                cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0
            };

            self.io_sq_tail = (self.io_sq_tail + 1) % 16;
            let db_addr = self.bar0 + 0x1000 + (2 * self.doorbell_stride as u64);
            write_volatile(db_addr as *mut u32, self.io_sq_tail as u32);

            let cq = &mut *(&mut IO_CQ.0 as *mut _ as *mut [NvmeCpl; 256]);
            for _ in 0..10_000_000 {
                let status_raw = read_volatile(&cq[self.io_cq_head as usize].status);
                let phase = (status_raw & 1) as u16;
                
                if phase == self.io_phase {
                    let sc = (status_raw >> 1) & 0xFF;
                    self.io_cq_head = (self.io_cq_head + 1) % 16;
                    
                    if self.io_cq_head == 0 { self.io_phase ^= 1; }
                    
                    let cq_db = self.bar0 + 0x1000 + (3 * self.doorbell_stride as u64);
                    write_volatile(cq_db as *mut u32, self.io_cq_head as u32);
                    
                    return sc == 0;
                }
                core::hint::spin_loop();
            }
        }
        false
    }
}