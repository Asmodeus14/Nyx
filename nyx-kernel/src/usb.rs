
use core::ptr::{read_volatile, write_volatile};
use alloc::alloc::{alloc, Layout};
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

const CMD_RUN: u32 = 0x00000001;
const CMD_HCRST: u32 = 0x00000002;
const CMD_INTE: u32 = 0x00000004;
const STS_HALT: u32 = 1 << 0;
const STS_CNR: u32 = 1 << 11;

// Capability Registers
#[repr(C)]
pub struct CapabilityRegisters {
    pub cap_length: u8, _reserved0: u8, pub hci_version: u16,
    pub hcsparams1: u32, pub hcsparams2: u32, pub hcsparams3: u32,
    pub hccparams1: u32, pub dboff: u32, pub rtsoff: u32, pub hccparams2: u32,
}
impl CapabilityRegisters {
    pub unsafe fn from_base(base: *const u8) -> &'static Self { &*(base as *const Self) }
    pub fn max_slots(&self) -> u8 { ((self.hcsparams1 >> 0) & 0xFF) as u8 }
    pub fn max_ports(&self) -> u8 { ((self.hcsparams1 >> 24) & 0xFF) as u8 }
    pub fn max_scratchpads(&self) -> usize {
        let hi = (self.hcsparams2 >> 21) & 0x1F; 
        let lo = (self.hcsparams2 >> 27) & 0x1F; 
        ((hi << 5) | lo) as usize
    }
}

// Operational Registers
#[repr(C)]
pub struct OperationalRegisters {
    pub usbcmd: u32, pub usbsts: u32, pub pagesize: u32, _reserved0: [u32; 2],
    pub dnctrl: u32, pub crcr: u64, _reserved1: [u32; 4],
    pub dcbaap: u64, pub config: u32, _reserved2: [u32; 241],
    pub portregs: [u32; 1024],
}
impl OperationalRegisters {
    pub unsafe fn from_base(base: *const u8) -> &'static mut Self { &mut *(base as *mut Self) }
    pub fn read_usbcmd(&self) -> u32 { unsafe { read_volatile(&self.usbcmd) } }
    pub fn write_usbcmd(&mut self, val: u32) { unsafe { write_volatile(&mut self.usbcmd, val) } }
    pub fn read_usbsts(&self) -> u32 { unsafe { read_volatile(&self.usbsts) } }
    pub fn write_usbsts(&mut self, val: u32) { unsafe { write_volatile(&mut self.usbsts, val) } }
    pub fn write_crcr(&mut self, val: u64) { unsafe { write_volatile(&mut self.crcr as *mut _ as *mut u64, val) } }
    pub fn write_dcbaap(&mut self, val: u64) { unsafe { write_volatile(&mut self.dcbaap as *mut _ as *mut u64, val) } }
    pub fn read_config(&self) -> u32 { unsafe { read_volatile(&self.config) } }
    pub fn write_config(&mut self, val: u32) { unsafe { write_volatile(&mut self.config, val) } }
}

// Runtime Registers
#[repr(C)]
pub struct RuntimeRegisters {
    pub mfindex: u32, _reserved0: [u32; 7],
    pub ir: [InterrupterRegisters; 32],
}
impl RuntimeRegisters {
    pub unsafe fn from_base(base: *const u8) -> &'static mut Self { &mut *(base as *mut Self) }
}

#[repr(C)]
pub struct InterrupterRegisters {
    pub iman: u32, pub imod: u32, pub erstsz: u32, pub rsvd: u32,
    pub erstba: u64, pub erdp: u64,
}

// Doorbell Registers
#[repr(C)]
pub struct DoorbellRegisters {
    pub doorbells: [u32; 256],
}
impl DoorbellRegisters {
    pub unsafe fn from_base(base: *const u8) -> &'static mut Self { &mut *(base as *mut Self) }
    pub fn ring(&mut self, target: usize) { unsafe { write_volatile(&mut self.doorbells[target], 0); } }
}

// TRB
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct Trb { pub parameter: u64, pub status: u32, pub control: u32 }
impl Trb {
    pub const TYPE_ENABLE_SLOT: u32 = 9 << 10;
    pub const TYPE_Address_DEVICE: u32 = 11 << 10;
    pub const TYPE_LINK: u32 = 6 << 10;
    pub const CYCLE_BIT: u32 = 1 << 0;
    pub const ENT_BIT: u32 = 1 << 1; 
    
    pub fn new() -> Self { Self { parameter: 0, status: 0, control: 0 } }
    pub fn get_type(&self) -> u32 { (self.control >> 10) & 0x3F }
}

// ERST Entry
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct ErstEntry { pub base_addr: u64, pub size: u16, pub rsvd: u16, pub rsvd2: u32 }

// --- CONTEXTS ---
#[repr(C)]
struct SlotContext {
    info1: u32, info2: u32, ttd: u32, state: u32, rsvd: [u32; 4],
}
#[repr(C)]
struct EndpointContext {
    info1: u32, info2: u32, tr_dequeue: u64, avg_trb_len: u32, rsvd: [u32; 3],
}
#[repr(C, align(64))]
struct DeviceContext {
    slot: SlotContext, ep0: EndpointContext, eps: [EndpointContext; 30],
}
#[repr(C, align(64))]
struct InputContext {
    input_ctrl: u32, rsvd: [u32; 7], 
    slot: SlotContext, ep0: EndpointContext, eps: [EndpointContext; 30],
}

// Controller Struct
pub struct XhciController {
    base: *const u8,
    caps: &'static CapabilityRegisters,
    op: &'static mut OperationalRegisters,
    runtime: &'static mut RuntimeRegisters,
    doorbell: &'static mut DoorbellRegisters,
    
    cmd_ring: *mut Trb,
    event_ring: *mut Trb,
    erst: *mut ErstEntry,
    dcbaa: *mut u64,
    
    scratchpad_array: *mut u64,
    scratchpad_pages: Vec<*mut u8>,
    
    cmd_index: usize,
    cmd_cycle: bool,
    event_index: usize,
    event_cycle: bool,
}

impl XhciController {
    unsafe fn alloc_aligned(size: usize, align_val: usize) -> Result<*mut u8, &'static str> {
        let layout = Layout::from_size_align(size, align_val).map_err(|_| "Layout")?;
        let ptr = alloc(layout);
        if ptr.is_null() { return Err("Alloc"); }
        core::ptr::write_bytes(ptr, 0, size);
        Ok(ptr)
    }
    
    unsafe fn clflush(addr: *const u8) {
        core::arch::asm!("clflush [{}]", in(reg) addr);
    }

    unsafe fn clflush_range(start: u64, size: usize) {
        let mut addr = start;
        let end = start + size as u64;
        while addr < end {
            Self::clflush(addr as *const u8);
            addr += 64;
        }
    }

    pub unsafe fn new(base_addr: u64) -> Result<Self, &'static str> {
        let base = base_addr as *const u8;
        let caps = CapabilityRegisters::from_base(base);
        let op = OperationalRegisters::from_base(base.add(caps.cap_length as usize));
        let runtime = RuntimeRegisters::from_base(base.add(caps.rtsoff as usize));
        let doorbell = DoorbellRegisters::from_base(base.add(caps.dboff as usize));

        let cmd_ring = Self::alloc_aligned(16 * 256, 64)? as *mut Trb;
        let event_ring = Self::alloc_aligned(16 * 256, 64)? as *mut Trb;
        let erst = Self::alloc_aligned(core::mem::size_of::<ErstEntry>(), 64)? as *mut ErstEntry;
        
        let max_slots = caps.max_slots() as usize + 1;
        let dcbaa = Self::alloc_aligned(8 * max_slots, 64)? as *mut u64;

        Ok(Self {
            base, caps, op, runtime, doorbell,
            cmd_ring, event_ring, erst, dcbaa,
            scratchpad_array: core::ptr::null_mut(),
            scratchpad_pages: Vec::new(),
            cmd_index: 0, cmd_cycle: true,
            event_index: 0, event_cycle: true,
        })
    }

    unsafe fn init_scratchpads(&mut self, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        let count = self.caps.max_scratchpads();
        log_win.buffer.push(alloc::format!("SP Needed: {}", count));
        
        if count > 0 {
            self.scratchpad_array = Self::alloc_aligned(4096, 4096)? as *mut u64;
            for i in 0..count {
                let page = Self::alloc_aligned(4096, 4096)?;
                self.scratchpad_pages.push(page);
                let phys = crate::memory::virt_to_phys(page as u64).ok_or("Phys Fail")?;
                *self.scratchpad_array.add(i) = phys;
            }
            Self::clflush_range(self.scratchpad_array as u64, 4096);
            let phys_arr = crate::memory::virt_to_phys(self.scratchpad_array as u64).ok_or("Phys Fail")?;
            *self.dcbaa = phys_arr;
            Self::clflush(self.dcbaa as *const u8);
        } else {
            *self.dcbaa = 0;
            Self::clflush(self.dcbaa as *const u8);
        }
        Ok(())
    }

    unsafe fn bios_handoff(&mut self) {
        let hcc1 = read_volatile(&self.caps.hccparams1);
        let off = ((hcc1 >> 16) & 0xFFFF) as usize;
        if off == 0 { return; }
        let mut ptr = self.base.add(off << 2) as *mut u32;
        loop {
            let val = read_volatile(ptr);
            if (val & 0xFF) == 1 {
                write_volatile(ptr, val | (1 << 24));
                for _ in 0..10000 {
                    if (read_volatile(ptr) & (1 << 16)) == 0 { break; }
                    core::hint::spin_loop();
                }
                let ctl = ptr.add(1);
                write_volatile(ctl, read_volatile(ctl) & 0xFFFF0000);
                break;
            }
            let next = (val >> 8) & 0xFF;
            if next == 0 { break; }
            ptr = ptr.add(next as usize);
        }
    }

    pub fn init(&mut self, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        unsafe {
            log_win.buffer.push(alloc::string::String::from("USB: Reset..."));
            self.bios_handoff();

            let mut cmd = self.op.read_usbcmd();
            cmd |= CMD_HCRST;
            self.op.write_usbcmd(cmd);
            for _ in 0..100000 { if (self.op.read_usbcmd() & CMD_HCRST) == 0 { break; } core::hint::spin_loop(); }
            for _ in 0..100000 { if (self.op.read_usbsts() & STS_CNR) == 0 { break; } core::hint::spin_loop(); }

            self.init_scratchpads(log_win)?;

            for i in 0..256 { *self.cmd_ring.add(i) = Trb::new(); }
            let phys_cmd = crate::memory::virt_to_phys(self.cmd_ring as u64).unwrap();
            let link = &mut *self.cmd_ring.add(255);
            link.parameter = phys_cmd;
            link.control = Trb::TYPE_LINK | Trb::CYCLE_BIT | Trb::ENT_BIT; 
            Self::clflush_range(self.cmd_ring as u64, 4096);
            
            self.op.write_crcr(phys_cmd | 1);

            for i in 0..256 { *self.event_ring.add(i) = Trb::new(); }
            Self::clflush_range(self.event_ring as u64, 4096);
            let phys_evt = crate::memory::virt_to_phys(self.event_ring as u64).unwrap();
            let erst_ent = &mut *self.erst;
            erst_ent.base_addr = phys_evt;
            erst_ent.size = 256;
            Self::clflush_range(self.erst as u64, 64);

            let ir0 = &mut self.runtime.ir[0];
            let phys_erst = crate::memory::virt_to_phys(self.erst as u64).unwrap();
            ir0.erstba = phys_erst;
            ir0.erstsz = 1;
            ir0.erdp = phys_evt | 8; 
            ir0.iman = 2; 
            ir0.imod = 4000;

            let phys_dcbaa = crate::memory::virt_to_phys(self.dcbaa as u64).unwrap();
            self.op.write_dcbaap(phys_dcbaa);
            Self::clflush_range(self.dcbaa as u64, 4096); 

            let max_slots = self.caps.max_slots();
            log_win.buffer.push(alloc::format!("Max Slots: {}", max_slots));
            let conf = (self.op.read_config() & !0xFF) | (max_slots as u32);
            self.op.write_config(conf);

            let mut run = self.op.read_usbcmd();
            run |= CMD_RUN | CMD_INTE;
            self.op.write_usbcmd(run);
            
            log_win.buffer.push(alloc::string::String::from("Controller Running"));
            self.repaint(log_win);
        }
        Ok(())
    }

    pub fn enable_slot(&mut self, log_win: &mut crate::window::Window) -> Result<u8, &'static str> {
        unsafe {
            let trb = &mut *self.cmd_ring.add(self.cmd_index);
            trb.parameter = 0; trb.status = 0;
            fence(Ordering::SeqCst);
            trb.control = Trb::TYPE_ENABLE_SLOT | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
            fence(Ordering::SeqCst);
            Self::clflush(trb as *const _ as *const u8);

            self.cmd_index = (self.cmd_index + 1) % 256;
            if self.cmd_index == 0 {
                let link = &mut *self.cmd_ring.add(255);
                link.control = Trb::TYPE_LINK | Trb::ENT_BIT | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
                Self::clflush(link as *const _ as *const u8);
                self.cmd_cycle = !self.cmd_cycle;
            }

            self.doorbell.ring(0);

            for _ in 0..2_000_000 {
                if let Some(slot) = self.check_event(log_win) { return Ok(slot); }
                core::hint::spin_loop();
            }
        }
        Err("Cmd Timeout")
    }

    unsafe fn check_event(&mut self, log_win: &mut crate::window::Window) -> Option<u8> {
        for _ in 0..16 { 
            let trb_ptr = self.event_ring.add(self.event_index);
            Self::clflush(trb_ptr as *const u8); 
            let trb = read_volatile(trb_ptr);

            if ((trb.control & 1) != 0) != self.event_cycle { return None; }

            self.event_index = (self.event_index + 1) % 256;
            if self.event_index == 0 { self.event_cycle = !self.event_cycle; }

            let ir0 = &mut self.runtime.ir[0];
            let phys = crate::memory::virt_to_phys(self.event_ring.add(self.event_index) as u64).unwrap();
            ir0.erdp = phys | 8; 
            ir0.iman = 3;

            let type_ = (trb.control >> 10) & 0x3F;
            if type_ == 33 { 
                let code = (trb.status >> 24) & 0xFF;
                if code == 1 { return Some(((trb.control >> 24) & 0xFF) as u8); }
                else { 
                    log_win.buffer.push(alloc::format!("Err Code: {}", code));
                    self.repaint(log_win);
                }
            }
        }
        None
    }

    pub fn address_device(&mut self, slot_id: u8, port_id: u8, speed: u8, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        unsafe {
            let out_ctx = Self::alloc_aligned(2048, 64)? as *mut DeviceContext;
            let phys_out = crate::memory::virt_to_phys(out_ctx as u64).unwrap();
            *self.dcbaa.add(slot_id as usize) = phys_out;
            Self::clflush(self.dcbaa.add(slot_id as usize) as *const u8);

            let in_ctx = Self::alloc_aligned(2048, 64)? as *mut InputContext;
            let phys_in = crate::memory::virt_to_phys(in_ctx as u64).unwrap();

            (*in_ctx).input_ctrl = 3; 
            // FIXED: Set Root Hub Port Num (Bits 16-23) AND Context Entries (Bits 27-31) AND Speed (Bits 20-23)
            (*in_ctx).slot.info1 = (1 << 27) | ((speed as u32) << 20); 
            (*in_ctx).slot.info2 = (port_id as u32) << 16; 

            (*in_ctx).ep0.info1 = (4 << 3); 
            (*in_ctx).ep0.info2 = (1 << 0) | (8 << 16); 
            
            let ep_ring = Self::alloc_aligned(4096, 64)? as *mut Trb;
            let phys_ep_ring = crate::memory::virt_to_phys(ep_ring as u64).unwrap();
            (*in_ctx).ep0.tr_dequeue = phys_ep_ring | 1; 

            Self::clflush_range(in_ctx as u64, 2048);
            Self::clflush_range(out_ctx as u64, 2048);
            Self::clflush_range(ep_ring as u64, 4096);

            let trb = &mut *self.cmd_ring.add(self.cmd_index);
            trb.parameter = phys_in;
            trb.status = 0;
            fence(Ordering::SeqCst);
            trb.control = Trb::TYPE_Address_DEVICE | (slot_id as u32) << 24 | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
            fence(Ordering::SeqCst);
            Self::clflush(trb as *const _ as *const u8);

            self.cmd_index = (self.cmd_index + 1) % 256;
            if self.cmd_index == 0 {
                let link = &mut *self.cmd_ring.add(255);
                link.control = Trb::TYPE_LINK | Trb::ENT_BIT | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
                Self::clflush(link as *const _ as *const u8);
                self.cmd_cycle = !self.cmd_cycle;
            }
            self.doorbell.ring(0);

            for _ in 0..5_000_000 {
                if let Some(s_id) = self.check_event(log_win) {
                    // Command Completion returns Slot ID in control field
                    if s_id == slot_id { 
                        log_win.buffer.push(alloc::format!("Slot {} Addressed", slot_id));
                        self.repaint(log_win);
                        return Ok(());
                    }
                }
                core::hint::spin_loop();
            }
        }
        Err("Address Dev Timeout")
    }

    pub fn check_ports(&mut self, log_win: &mut crate::window::Window) {
        unsafe {
            let max = self.caps.max_ports();
            for i in 1..=max {
                let idx = (i - 1) as usize * 4;
                if idx >= self.op.portregs.len() { break; }
                let portsc = read_volatile(&self.op.portregs[idx]);
                
                if (portsc & 1) != 0 { 
                    write_volatile(&mut self.op.portregs[idx], portsc | (1 << 24) | (1 << 20) | (1 << 17)); 
                    
                    if (portsc & (1<<1)) == 0 { 
                        write_volatile(&mut self.op.portregs[idx], portsc | (1 << 4)); 
                        for _ in 0..10_000_000 {
                            if (read_volatile(&self.op.portregs[idx]) & (1<<1)) != 0 { break; }
                            core::hint::spin_loop();
                        }
                    }
                    
                    if (read_volatile(&self.op.portregs[idx]) & (1<<1)) != 0 {
                        let final_portsc = read_volatile(&self.op.portregs[idx]);
                        let speed = (final_portsc >> 10) & 0xF; // Extract Speed
                        
                        log_win.buffer.push(alloc::format!("Port {} Spd {}", i, speed));
                        self.repaint(log_win);
                        
                        match self.enable_slot(log_win) {
                            Ok(id) => { 
                                if let Err(e) = self.address_device(id, i as u8, speed as u8, log_win) {
                                     log_win.buffer.push(alloc::string::String::from(e));
                                }
                            },
                            Err(e) => { log_win.buffer.push(alloc::string::String::from(e)); self.repaint(log_win); }
                        }
                    }
                }
            }
        }
    }

    fn repaint(&self, win: &crate::window::Window) {
        unsafe {
            if let Some(bb) = &mut crate::BACK_BUFFER {
                crate::window::WINDOW_MANAGER.lock().draw(bb);
                win.draw(bb, true);
                if let Some(s) = &mut crate::SCREEN_PAINTER { bb.present(s); }
            }
        }
    }
}