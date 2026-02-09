use core::ptr::{read_volatile, write_volatile};
use alloc::alloc::{alloc, Layout};
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref USB_CONTROLLER: Mutex<Option<XhciController>> = Mutex::new(None);
}

const CMD_RUN: u32 = 0x00000001;
const CMD_HCRST: u32 = 0x00000002;
const CMD_INTE: u32 = 0x00000004;
const STS_HALT: u32 = 1 << 0;
const STS_CNR: u32 = 1 << 11;

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
    pub fn context_size(&self) -> usize { if (self.hccparams1 & (1 << 2)) != 0 { 64 } else { 32 } }
    pub fn max_scratchpads(&self) -> usize {
        let hi = (self.hcsparams2 >> 21) & 0x1F; 
        let lo = (self.hcsparams2 >> 27) & 0x1F; 
        ((hi << 5) | lo) as usize
    }
    pub fn xecp(&self) -> u32 { (self.hccparams1 >> 16) & 0xFFFF }
}

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
    pub fn write_crcr(&mut self, val: u64) { unsafe { write_volatile(&mut self.crcr as *mut _ as *mut u64, val) } }
    pub fn write_dcbaap(&mut self, val: u64) { 
        unsafe {
            let ptr = &mut self.dcbaap as *mut u64 as *mut u32;
            write_volatile(ptr, val as u32);
            write_volatile(ptr.add(1), (val >> 32) as u32);
        }
    }
    pub fn read_config(&self) -> u32 { unsafe { read_volatile(&self.config) } }
    pub fn write_config(&mut self, val: u32) { unsafe { write_volatile(&mut self.config, val) } }
    pub fn write_dnctrl(&mut self, val: u32) { unsafe { write_volatile(&mut self.dnctrl, val) } }
}

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

#[repr(C)]
pub struct DoorbellRegisters {
    pub doorbells: [u32; 256],
}
impl DoorbellRegisters {
    pub unsafe fn from_base(base: *const u8) -> &'static mut Self { &mut *(base as *mut Self) }
    pub fn ring(&mut self, target: usize, value: u32) { unsafe { write_volatile(&mut self.doorbells[target], value); } }
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct Trb { pub parameter: u64, pub status: u32, pub control: u32 }
impl Trb {
    pub const TYPE_NORMAL: u32 = 1 << 10;
    pub const TYPE_SETUP: u32 = 2 << 10;
    pub const TYPE_DATA: u32 = 3 << 10;
    pub const TYPE_STATUS: u32 = 4 << 10;
    pub const TYPE_LINK: u32 = 6 << 10;
    pub const TYPE_ENABLE_SLOT: u32 = 9 << 10;
    pub const TYPE_ADDRESS_DEVICE: u32 = 11 << 10;
    pub const TYPE_CONFIGURE_ENDPOINT: u32 = 12 << 10;
    pub const TYPE_NOOP: u32 = 23 << 10;
    
    pub const CYCLE_BIT: u32 = 1 << 0;
    pub const ENT_BIT: u32 = 1 << 1; 
    pub const IDT_BIT: u32 = 1 << 6; 
    pub const IOC_BIT: u32 = 1 << 5;
    pub const BSR_BIT: u32 = 1 << 9; 
    pub fn new() -> Self { Self { parameter: 0, status: 0, control: 0 } }
}

#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct ErstEntry { pub base_addr: u64, pub size: u16, pub rsvd: u16, pub rsvd2: u32 }

#[repr(C)] struct SlotContext { info1: u32, info2: u32, ttd: u32, state: u32, rsvd: [u32; 4] }
#[repr(C)] struct EndpointContext { info1: u32, info2: u32, tr_dequeue: u64, avg_trb_len: u32, rsvd: [u32; 3] }

pub struct XhciController {
    base: *const u8,
    caps: &'static CapabilityRegisters,
    op: &'static mut OperationalRegisters,
    runtime: &'static mut RuntimeRegisters,
    doorbell: &'static mut DoorbellRegisters,
    
    cmd_ring: *mut Trb, event_ring: *mut Trb, erst: *mut ErstEntry, dcbaa: *mut u64,
    scratchpad_array: *mut u64, scratchpad_pages: Vec<*mut u8>,
    
    ep0_rings: Vec<*mut Trb>, 
    ep0_cycles: Vec<bool>, 
    ep0_indices: Vec<usize>, 

    ep1_rings: Vec<*mut Trb>,
    ep1_cycles: Vec<bool>,
    ep1_indices: Vec<usize>,
    ep1_configured: Vec<bool>,
    mouse_pending: Vec<bool>, 

    cmd_index: usize, cmd_cycle: bool, event_index: usize, event_cycle: bool, ctx_size: usize,
    mouse_buf_virt: *mut u8,
    mouse_buf_phys: u64,
}

unsafe impl Send for XhciController {}
unsafe impl Sync for XhciController {}

impl XhciController {
    unsafe fn alloc_aligned(size: usize, align_val: usize) -> Result<*mut u8, &'static str> {
        let layout = Layout::from_size_align(size, align_val).map_err(|_| "Layout")?;
        let ptr = alloc(layout);
        if ptr.is_null() { return Err("Alloc"); }
        core::ptr::write_bytes(ptr, 0, size);
        Ok(ptr)
    }
    
    unsafe fn clflush(addr: *const u8) { core::arch::asm!("clflush [{}]", in(reg) addr); }
    unsafe fn clflush_range(start: u64, size: usize) {
        let mut addr = start; let end = start + size as u64;
        while addr < end { Self::clflush(addr as *const u8); addr += 64; }
    }

    pub unsafe fn new(base_addr: u64) -> Result<Self, &'static str> {
        let base = base_addr as *const u8;
        let caps = CapabilityRegisters::from_base(base);
        let op = OperationalRegisters::from_base(base.add(caps.cap_length as usize));
        let runtime = RuntimeRegisters::from_base(base.add(caps.rtsoff as usize));
        let doorbell = DoorbellRegisters::from_base(base.add(caps.dboff as usize));

        let cmd_ring = Self::alloc_aligned(4096, 64)? as *mut Trb;
        let event_ring = Self::alloc_aligned(4096, 64)? as *mut Trb;
        let erst = Self::alloc_aligned(core::mem::size_of::<ErstEntry>(), 64)? as *mut ErstEntry;
        let dcbaa = Self::alloc_aligned(4096, 4096)? as *mut u64;

        let max_slots = caps.max_slots() as usize + 1;
        
        let mut ep0_rings = Vec::with_capacity(max_slots);
        let mut ep0_cycles = Vec::with_capacity(max_slots);
        let mut ep0_indices = Vec::with_capacity(max_slots);

        let mut ep1_rings = Vec::with_capacity(max_slots);
        let mut ep1_cycles = Vec::with_capacity(max_slots);
        let mut ep1_indices = Vec::with_capacity(max_slots);
        let mut ep1_configured = Vec::with_capacity(max_slots);
        let mut mouse_pending = Vec::with_capacity(max_slots);

        for _ in 0..max_slots { 
            ep0_rings.push(core::ptr::null_mut()); 
            ep0_cycles.push(true); 
            ep0_indices.push(0);

            ep1_rings.push(core::ptr::null_mut());
            ep1_cycles.push(true);
            ep1_indices.push(0);
            ep1_configured.push(false);
            mouse_pending.push(false);
        }

        let mouse_buf_virt = Self::alloc_aligned(64, 64)? as *mut u8;
        let mouse_buf_phys = crate::memory::virt_to_phys(mouse_buf_virt as u64).unwrap();

        Ok(Self {
            base, caps, op, runtime, doorbell,
            cmd_ring, event_ring, erst, dcbaa,
            scratchpad_array: core::ptr::null_mut(), scratchpad_pages: Vec::new(),
            ep0_rings, ep0_cycles, ep0_indices, 
            ep1_rings, ep1_cycles, ep1_indices, ep1_configured, mouse_pending,
            cmd_index: 0, cmd_cycle: true, event_index: 0, event_cycle: true,
            ctx_size: caps.context_size(),
            mouse_buf_virt, mouse_buf_phys,
        })
    }

    unsafe fn push_ring_trb(ring_ptr: *mut Trb, idx: &mut usize, cycle: &mut bool, trb: Trb) {
        let dest = ring_ptr.add(*idx);
        let mut new_trb = trb;
        if *cycle { new_trb.control |= Trb::CYCLE_BIT; } else { new_trb.control &= !Trb::CYCLE_BIT; }
        *dest = new_trb;
        Self::clflush(dest as *const u8);

        *idx += 1;
        if *idx == 255 {
            let link = ring_ptr.add(255);
            let phys = crate::memory::virt_to_phys(ring_ptr as u64).unwrap();
            let mut link_trb = Trb::new();
            link_trb.parameter = phys;
            link_trb.control = Trb::TYPE_LINK | Trb::ENT_BIT; 
            if *cycle { link_trb.control |= Trb::CYCLE_BIT; } else { link_trb.control &= !Trb::CYCLE_BIT; }
            *link = link_trb;
            Self::clflush(link as *const u8);
            *idx = 0;
            *cycle = !*cycle;
        }
    }

    unsafe fn push_ep0_trb(&mut self, slot_id: usize, trb: Trb) {
        Self::push_ring_trb(self.ep0_rings[slot_id], &mut self.ep0_indices[slot_id], &mut self.ep0_cycles[slot_id], trb);
    }

    unsafe fn push_ep1_trb(&mut self, slot_id: usize, trb: Trb) {
        Self::push_ring_trb(self.ep1_rings[slot_id], &mut self.ep1_indices[slot_id], &mut self.ep1_cycles[slot_id], trb);
    }

    unsafe fn perform_bios_handoff(&self, log_win: &mut crate::window::Window) {
        let mut xecp_offset = self.caps.xecp();
        while xecp_offset != 0 {
            let cap_ptr = self.base.add((xecp_offset << 2) as usize) as *mut u32;
            let cap_val = read_volatile(cap_ptr);
            if ((cap_val & 0xFF) as u8) == 1 { 
                log_win.buffer.push(alloc::string::String::from("BIOS Handoff..."));
                if (cap_val & (1 << 16)) != 0 {
                    write_volatile(cap_ptr, cap_val | (1 << 24));
                    for _ in 0..1000000 { if (read_volatile(cap_ptr) & (1 << 16)) == 0 { break; } core::hint::spin_loop(); }
                    while (read_volatile(cap_ptr) & (1 << 24)) == 0 { core::hint::spin_loop(); }
                    log_win.buffer.push(alloc::string::String::from("BIOS Released."));
                } else { write_volatile(cap_ptr, cap_val | (1 << 24)); }
                break;
            }
            let next = (cap_val >> 8) & 0xFF;
            if next == 0 { break; }
            xecp_offset += next;
        }
    }

    unsafe fn init_scratchpads(&mut self, _log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        let count = self.caps.max_scratchpads();
        if count > 0 {
            self.scratchpad_array = Self::alloc_aligned(4096, 4096)? as *mut u64;
            for i in 0..count {
                let page = Self::alloc_aligned(4096, 4096)?;
                self.scratchpad_pages.push(page);
                *self.scratchpad_array.add(i) = crate::memory::virt_to_phys(page as u64).unwrap();
            }
            Self::clflush_range(self.scratchpad_array as u64, 4096);
            *self.dcbaa = crate::memory::virt_to_phys(self.scratchpad_array as u64).unwrap();
        } else { *self.dcbaa = 0; }
        Self::clflush(self.dcbaa as *const u8);
        Ok(())
    }

    pub fn init(&mut self, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        unsafe {
            self.perform_bios_handoff(log_win);
            self.repaint(log_win);
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
            (*self.erst).base_addr = phys_evt; (*self.erst).size = 256;
            Self::clflush_range(self.erst as u64, 64);
            let ir0 = &mut self.runtime.ir[0];
            ir0.erstba = crate::memory::virt_to_phys(self.erst as u64).unwrap();
            ir0.erstsz = 1; ir0.erdp = phys_evt | 8; ir0.iman = 2; ir0.imod = 4000;

            let phys_dcbaa = crate::memory::virt_to_phys(self.dcbaa as u64).unwrap();
            self.op.write_dcbaap(phys_dcbaa);
            Self::clflush_range(self.dcbaa as u64, 4096); 

            let max_slots = self.caps.max_slots();
            self.op.write_config((self.op.read_config() & !0xFF) | (max_slots as u32));
            self.op.write_dnctrl(0x2);

            let mut run = self.op.read_usbcmd();
            run |= CMD_RUN | CMD_INTE;
            self.op.write_usbcmd(run);
            
            let mut started = false;
            for _ in 0..100_000_000 {
                let sts = self.op.read_usbsts();
                if (sts & STS_HALT) == 0 { started = true; break; }
                core::hint::spin_loop();
            }
            if !started { return Err("Ctlr Halted"); }

            let trb = &mut *self.cmd_ring.add(self.cmd_index);
            trb.parameter = 0; trb.status = 0;
            fence(Ordering::SeqCst);
            trb.control = Trb::TYPE_NOOP | Trb::IOC_BIT | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
            fence(Ordering::SeqCst);
            Self::clflush(trb as *const _ as *const u8);
            self.cmd_index = (self.cmd_index + 1) % 256;
            if self.cmd_index == 0 {
                let link = &mut *self.cmd_ring.add(255);
                link.control = Trb::TYPE_LINK | Trb::ENT_BIT | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
                Self::clflush(link as *const _ as *const u8);
                self.cmd_cycle = !self.cmd_cycle;
            }
            self.doorbell.ring(0, 0);
            let mut noop_ok = false;
            for _ in 0..1_000_000 { if let Some(_) = self.check_event(log_win) { noop_ok = true; break; } core::hint::spin_loop(); }
            if noop_ok { log_win.buffer.push(alloc::string::String::from("NoOp: OK")); } 
            else { log_win.buffer.push(alloc::string::String::from("NoOp: Fail")); }
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
            self.doorbell.ring(0, 0); 
            for _ in 0..2_000_000 { if let Some(slot) = self.check_event(log_win) { return Ok(slot); } core::hint::spin_loop(); }
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
            ir0.erdp = phys | 8; ir0.iman = 3;
            let type_ = (trb.control >> 10) & 0x3F;
            if type_ == 33 || type_ == 32 { 
                let code = (trb.status >> 24) & 0xFF;
                let slot = ((trb.control >> 24) & 0xFF) as u8;
                if code == 1 || code == 0 || code == 13 { return Some(slot); }
                else { 
                    log_win.buffer.push(alloc::format!("Err: {} Typ: {} Sl: {}", code, type_, slot));
                    self.repaint(log_win);
                    return None; 
                }
            }
        }
        None
    }

    // --- NON-BLOCKING POLL: PROCESS ALL EVENTS ---
    pub fn poll_mouse(&mut self, target_slot: u8) {
        unsafe {
            // 1. Drain Event Ring completely (process all completions for ALL slots)
            for _ in 0..32 { // Process up to 32 events per tick
                if let Some((event_slot, code)) = self.check_event_async() {
                    // Update pending state for whichever slot reported in
                    if (event_slot as usize) < self.mouse_pending.len() {
                        self.mouse_pending[event_slot as usize] = false;
                        
                        // If success (1) or Short Packet (13), read the data
                        if code == 1 || code == 13 || code == 0 {
                            // Note: Buffer is shared, so this assumes the last interrupt overwrote it.
                            // Ideally each slot has its own buffer, but for simple mice this is 'okay'.
                            let buffer = self.mouse_buf_virt;
                            Self::clflush(buffer);
                            
                            let buttons = *buffer.add(0);
                            let dx = *buffer.add(1) as i8;
                            let dy = *buffer.add(2) as i8;
                            
                            if dx != 0 || dy != 0 || buttons != 0 {
                                crate::mouse::update_from_usb(dx, dy, buttons);
                            }
                        }
                    }
                } else {
                    break; // No more events
                }
            }

            // 2. Submit new request for the TARGET slot (if idle)
            let s_id = target_slot as usize;
            if s_id < self.ep1_configured.len() && self.ep1_configured[s_id] && !self.mouse_pending[s_id] {
                let ring = self.ep1_rings[s_id];
                if ring.is_null() { return; } 

                let phys_buf = self.mouse_buf_phys;
                let mut trb = Trb::new();
                trb.parameter = phys_buf;
                trb.status = 8; 
                trb.control = Trb::TYPE_NORMAL | Trb::IOC_BIT | (1 << 16); 
                
                self.push_ep1_trb(s_id, trb);
                self.doorbell.ring(s_id, 3); // DCI 3 = EP1 IN
                
                self.mouse_pending[s_id] = true;
            }
        }
    }

    unsafe fn check_event_async(&mut self) -> Option<(u8, u8)> {
        let trb_ptr = self.event_ring.add(self.event_index);
        let trb = read_volatile(trb_ptr);

        if ((trb.control & 1) != 0) != self.event_cycle { 
            return None; 
        }

        self.event_index = (self.event_index + 1) % 256;
        if self.event_index == 0 { self.event_cycle = !self.event_cycle; }

        let ir0 = &mut self.runtime.ir[0];
        let phys = crate::memory::virt_to_phys(self.event_ring.add(self.event_index) as u64).unwrap();
        ir0.erdp = phys | 8; ir0.iman = 3;

        let type_ = (trb.control >> 10) & 0x3F;
        if type_ == 32 || type_ == 33 { 
            let code = (trb.status >> 24) & 0xFF;
            let slot = ((trb.control >> 24) & 0xFF) as u8;
            return Some((slot, code as u8));
        }
        // Advance but ignore non-transfer events
        None
    }

    pub fn set_configuration(&mut self, slot_id: u8, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        unsafe {
            let s_id = slot_id as usize;
            let mut setup = Trb::new(); setup.parameter = 0x0000_0000_0001_0900; setup.status = 8; setup.control = Trb::TYPE_SETUP | Trb::IDT_BIT;
            self.push_ep0_trb(s_id, setup);
            let mut status = Trb::new(); status.parameter = 0; status.status = 0; status.control = Trb::TYPE_STATUS | Trb::IOC_BIT | (1 << 16);
            self.push_ep0_trb(s_id, status);
            self.doorbell.ring(s_id, 1);
            for _ in 0..5_000_000 { if let Some(id) = self.check_event(log_win) { if id == slot_id { return Ok(()); } } core::hint::spin_loop(); }
        }
        Err("Cfg Timeout")
    }

    pub fn set_idle(&mut self, slot_id: u8, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        unsafe {
            let s_id = slot_id as usize;
            let mut setup = Trb::new(); setup.parameter = 0x0000_0000_0000_0A21; setup.status = 8; setup.control = Trb::TYPE_SETUP | Trb::IDT_BIT;
            self.push_ep0_trb(s_id, setup);
            let mut status = Trb::new(); status.parameter = 0; status.status = 0; status.control = Trb::TYPE_STATUS | Trb::IOC_BIT | (1 << 16);
            self.push_ep0_trb(s_id, status);
            self.doorbell.ring(s_id, 1);
            for _ in 0..5_000_000 { if let Some(id) = self.check_event(log_win) { if id == slot_id { return Ok(()); } } core::hint::spin_loop(); }
        }
        Err("Idle Timeout")
    }

    pub fn set_boot_protocol(&mut self, slot_id: u8, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        unsafe {
            let s_id = slot_id as usize;
            let mut setup = Trb::new(); setup.parameter = 0x0000_0000_0000_0B21; setup.status = 8; setup.control = Trb::TYPE_SETUP | Trb::IDT_BIT;
            self.push_ep0_trb(s_id, setup);
            let mut status = Trb::new(); status.parameter = 0; status.status = 0; status.control = Trb::TYPE_STATUS | Trb::IOC_BIT | (1 << 16);
            self.push_ep0_trb(s_id, status);
            self.doorbell.ring(s_id, 1);
            for _ in 0..5_000_000 { if let Some(id) = self.check_event(log_win) { if id == slot_id { return Ok(()); } } core::hint::spin_loop(); }
        }
        Err("Proto Timeout")
    }

    pub fn get_descriptor(&mut self, slot_id: u8, log_win: &mut crate::window::Window, short_read: bool) -> Result<u8, &'static str> {
        unsafe {
            let s_id = slot_id as usize;
            let len = if short_read { 8 } else { 18 };
            let buffer = Self::alloc_aligned(len as usize, 64)? as *mut u8;
            let phys_buf = crate::memory::virt_to_phys(buffer as u64).unwrap();
            let mut setup = Trb::new(); let param_low = 0x0100_0680; let param_high = (len as u64) << 48; setup.parameter = param_high | param_low; setup.status = 8; setup.control = Trb::TYPE_SETUP | Trb::IDT_BIT;
            self.push_ep0_trb(s_id, setup);
            let mut data = Trb::new(); data.parameter = phys_buf; data.status = len; data.control = Trb::TYPE_DATA | (1 << 16);
            self.push_ep0_trb(s_id, data);
            let mut status = Trb::new(); status.parameter = 0; status.status = 0; status.control = Trb::TYPE_STATUS | Trb::IOC_BIT;
            self.push_ep0_trb(s_id, status);
            self.doorbell.ring(s_id, 1); 
            for _ in 0..5_000_000 {
                if let Some(id) = self.check_event(log_win) {
                    if id == 0 { return Err("Desc Fail"); }
                    Self::clflush(buffer); 
                    let max_p = *buffer.add(7);
                    if !short_read {
                        let vid = (*buffer.add(8) as u16) | ((*buffer.add(9) as u16) << 8);
                        let pid = (*buffer.add(10) as u16) | ((*buffer.add(11) as u16) << 8);
                        log_win.buffer.push(alloc::format!("S{}: {:x}:{:x} MP:{}", slot_id, vid, pid, max_p));
                        self.repaint(log_win);
                    }
                    return Ok(max_p);
                }
                core::hint::spin_loop();
            }
        }
        Err("Desc Timeout")
    }

    pub fn configure_interrupt_endpoint(&mut self, slot_id: u8, log_win: &mut crate::window::Window) -> Result<(), &'static str> {
        unsafe {
            let s_id = slot_id as usize;
            
            let ep1_ring = Self::alloc_aligned(4096, 64)? as *mut Trb;
            for i in 0..256 { *ep1_ring.add(i) = Trb::new(); }
            let link = &mut *ep1_ring.add(255);
            link.parameter = crate::memory::virt_to_phys(ep1_ring as u64).unwrap();
            link.control = Trb::TYPE_LINK | Trb::ENT_BIT; 
            Self::clflush_range(ep1_ring as u64, 4096);
            
            self.ep1_rings[s_id] = ep1_ring;
            self.ep1_cycles[s_id] = true;
            self.ep1_indices[s_id] = 0;

            let in_ctx_mem = Self::alloc_aligned(self.ctx_size * 34, 64)?;
            let phys_in = crate::memory::virt_to_phys(in_ctx_mem as u64).unwrap();
            
            let icc_ptr = in_ctx_mem as *mut u32; 
            *icc_ptr.add(0) = 0; 
            *icc_ptr.add(1) = (1 << 3) | (1 << 0); // Add EP1 (DCI 3) & Slot

            let slot_ptr = in_ctx_mem.add(self.ctx_size) as *mut SlotContext;
            (*slot_ptr).info1 = (1 << 27) | (3 << 27); // Context Entries = 3

            let ep1_ptr = in_ctx_mem.add(self.ctx_size * 4) as *mut EndpointContext; 
            
            // FIX: info1=Interval, info2=Type/MaxPacket
            (*ep1_ptr).info1 = (8 << 16); 
            (*ep1_ptr).info2 = (8 << 16) | (7 << 3) | (3 << 1); 
            
            (*ep1_ptr).tr_dequeue = crate::memory::virt_to_phys(ep1_ring as u64).unwrap() | 1; 
            (*ep1_ptr).avg_trb_len = 8;

            Self::clflush_range(in_ctx_mem as u64, self.ctx_size * 34);

            let trb = &mut *self.cmd_ring.add(self.cmd_index);
            trb.parameter = phys_in;
            trb.status = 0;
            fence(Ordering::SeqCst);
            trb.control = Trb::TYPE_CONFIGURE_ENDPOINT | (slot_id as u32) << 24 | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
            fence(Ordering::SeqCst);
            Self::clflush(trb as *const _ as *const u8);

            self.cmd_index = (self.cmd_index + 1) % 256;
            if self.cmd_index == 0 {
                let link = &mut *self.cmd_ring.add(255);
                link.control = Trb::TYPE_LINK | Trb::ENT_BIT | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
                Self::clflush(link as *const _ as *const u8);
                self.cmd_cycle = !self.cmd_cycle;
            }
            self.doorbell.ring(0, 0);

            for _ in 0..5_000_000 {
                if let Some(id) = self.check_event(log_win) {
                    if id == slot_id { 
                        log_win.buffer.push(alloc::string::String::from("EP1: Config"));
                        self.ep1_configured[s_id] = true;
                        self.repaint(log_win);
                        return Ok(()); 
                    }
                }
                core::hint::spin_loop();
            }
        }
        Err("EP1 Fail")
    }

    pub fn address_device(&mut self, slot_id: u8, port_id: u8, speed: u8, log_win: &mut crate::window::Window, bsr: bool, packet_size_override: Option<u32>) -> Result<(), &'static str> {
        unsafe {
            if *self.dcbaa.add(slot_id as usize) == 0 {
                let out_ctx_mem = Self::alloc_aligned(self.ctx_size * 33, 64)?;
                *self.dcbaa.add(slot_id as usize) = crate::memory::virt_to_phys(out_ctx_mem as u64).unwrap();
                Self::clflush(self.dcbaa.add(slot_id as usize) as *const u8);
            }
            let in_ctx_mem = Self::alloc_aligned(self.ctx_size * 34, 64)?;
            let phys_in = crate::memory::virt_to_phys(in_ctx_mem as u64).unwrap();
            let icc_ptr = in_ctx_mem as *mut u32; *icc_ptr.add(1) = 3; 
            let slot_ptr = in_ctx_mem.add(self.ctx_size) as *mut SlotContext;
            (*slot_ptr).info1 = (1 << 27) | ((speed as u32) << 20); 
            (*slot_ptr).info2 = (port_id as u32) << 16; 
            let ep0_ptr = in_ctx_mem.add(self.ctx_size * 2) as *mut EndpointContext;
            let max_packet: u32 = if let Some(sz) = packet_size_override { sz } else { match speed { 1 => 8, 3 | 4 => 512, _ => 64 } };
            (*ep0_ptr).info1 = 0; (*ep0_ptr).info2 = (max_packet << 16) | (4 << 3) | (3 << 1); (*ep0_ptr).avg_trb_len = 8;
            
            let ep_ring = Self::alloc_aligned(4096, 64)? as *mut Trb;
            for i in 0..256 { *ep_ring.add(i) = Trb::new(); } 
            let link = &mut *ep_ring.add(255); link.parameter = crate::memory::virt_to_phys(ep_ring as u64).unwrap(); link.control = Trb::TYPE_LINK | Trb::CYCLE_BIT | Trb::ENT_BIT;
            Self::clflush_range(ep_ring as u64, 4096);
            self.ep0_rings[slot_id as usize] = ep_ring; self.ep0_cycles[slot_id as usize] = true; self.ep0_indices[slot_id as usize] = 0; 
            (*ep0_ptr).tr_dequeue = crate::memory::virt_to_phys(ep_ring as u64).unwrap() | 1; 
            Self::clflush_range(in_ctx_mem as u64, self.ctx_size * 34);

            let trb = &mut *self.cmd_ring.add(self.cmd_index);
            trb.parameter = phys_in; trb.status = 0;
            fence(Ordering::SeqCst);
            let bsr_flag = if bsr { Trb::BSR_BIT } else { 0 };
            trb.control = Trb::TYPE_ADDRESS_DEVICE | bsr_flag | (slot_id as u32) << 24 | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
            fence(Ordering::SeqCst);
            Self::clflush(trb as *const _ as *const u8);
            self.cmd_index = (self.cmd_index + 1) % 256;
            if self.cmd_index == 0 {
                let link = &mut *self.cmd_ring.add(255);
                link.control = Trb::TYPE_LINK | Trb::ENT_BIT | (if self.cmd_cycle { Trb::CYCLE_BIT } else { 0 });
                Self::clflush(link as *const _ as *const u8);
                self.cmd_cycle = !self.cmd_cycle;
            }
            self.doorbell.ring(0, 0); 
            for _ in 0..10_000_000 { if let Some(s_id) = self.check_event(log_win) { if s_id == slot_id { return Ok(()); } } core::hint::spin_loop(); }
        }
        Err("Addr Timeout")
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
                        for _ in 0..10_000_000 { if (read_volatile(&self.op.portregs[idx]) & (1<<4)) == 0 { break; } core::hint::spin_loop(); }
                        for _ in 0..50_000_000 { core::hint::spin_loop(); }
                    }
                    if (read_volatile(&self.op.portregs[idx]) & (1<<1)) != 0 {
                        let speed = (read_volatile(&self.op.portregs[idx]) >> 10) & 0xF; 
                        log_win.buffer.push(alloc::format!("P{} Spd{}", i, speed));
                        self.repaint(log_win);
                        
                        if let Ok(id) = self.enable_slot(log_win) {
                            if id > 0 { 
                                if self.address_device(id, i as u8, speed as u8, log_win, true, None).is_ok() {
                                    if let Ok(real_mp) = self.get_descriptor(id, log_win, true) {
                                        if self.address_device(id, i as u8, speed as u8, log_win, false, Some(real_mp as u32)).is_ok() {
                                            let _ = self.get_descriptor(id, log_win, false);
                                            let _ = self.set_configuration(id, log_win);
                                            for _ in 0..10_000_000 { core::hint::spin_loop(); }
                                            let _ = self.set_idle(id, log_win);
                                            let _ = self.set_boot_protocol(id, log_win);
                                            let _ = self.configure_interrupt_endpoint(id, log_win);
                                        }
                                    }
                                }
                            }
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