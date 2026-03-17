use crate::pci::PciDevice;
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::vec;

// smoltcp imports
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken, Medium};
use smoltcp::time::Instant;

const NUM_TX_DESC: usize = 64;
const NUM_RX_DESC: usize = 64;

#[repr(C)]
#[derive(Copy, Clone)]
struct Descriptor {
    command_status: u32,
    vlan: u32,
    buf_addr_low: u32,
    buf_addr_high: u32,
}

#[repr(C, align(256))]
struct DescriptorRing {
    desc: [Descriptor; 64],
}

pub struct Rtl8168Driver {
    pci_device: PciDevice,
    mmio_base: u64,
    pub mac_address: [u8; 6],
    tx_ring: Box<DescriptorRing>,
    rx_ring: Box<DescriptorRing>,
    tx_buffers: Vec<Box<[u8; 1536]>>,
    rx_buffers: Vec<Box<[u8; 1536]>>,
    tx_index: usize,
    rx_index: usize, 
}

impl Rtl8168Driver {
    pub fn new(pci_device: PciDevice, bar_phys: u64) -> Self {
        let mmio_base = crate::memory::phys_to_virt(bar_phys).unwrap();
        Self {
            pci_device, mmio_base, mac_address: [0; 6],
            tx_ring: Box::new(DescriptorRing { desc: [Descriptor { command_status: 0, vlan: 0, buf_addr_low: 0, buf_addr_high: 0 }; 64] }),
            rx_ring: Box::new(DescriptorRing { desc: [Descriptor { command_status: 0, vlan: 0, buf_addr_low: 0, buf_addr_high: 0 }; 64] }),
            tx_buffers: Vec::new(), rx_buffers: Vec::new(),
            tx_index: 0, rx_index: 0, 
        }
    }

    pub fn initialize(&mut self) {
        crate::serial_println!("[RTL8168] Initializing Realtek Gigabit Ethernet...");
        
        for i in 0..6 { self.mac_address[i] = unsafe { core::ptr::read_volatile((self.mmio_base + i as u64) as *const u8) }; }
        crate::serial_println!("[RTL8168] MAC Address: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", 
            self.mac_address[0], self.mac_address[1], self.mac_address[2], self.mac_address[3], self.mac_address[4], self.mac_address[5]);

        unsafe { core::ptr::write_volatile((self.mmio_base + 0x37) as *mut u8, 0x10); }
        let mut reset_cleared = false;
        for _ in 0..1000 {
            if unsafe { core::ptr::read_volatile((self.mmio_base + 0x37) as *const u8) } & 0x10 == 0 {
                reset_cleared = true; break;
            }
        }
        if !reset_cleared { return; }

        let phys_offset = unsafe { crate::memory::PHYS_MEM_OFFSET };

        for _ in 0..NUM_RX_DESC { self.rx_buffers.push(Box::new([0u8; 1536])); }
        for _ in 0..NUM_TX_DESC { self.tx_buffers.push(Box::new([0u8; 1536])); }

        for i in 0..NUM_RX_DESC {
            let buf_phys = (self.rx_buffers[i].as_ptr() as u64) - phys_offset;
            self.rx_ring.desc[i].buf_addr_low = (buf_phys & 0xFFFFFFFF) as u32;
            self.rx_ring.desc[i].buf_addr_high = (buf_phys >> 32) as u32;
            let mut status = (1 << 31) | 1536; 
            if i == NUM_RX_DESC - 1 { status |= 1 << 30; } 
            self.rx_ring.desc[i].command_status = status;
        }

        for i in 0..NUM_TX_DESC {
            let buf_phys = (self.tx_buffers[i].as_ptr() as u64) - phys_offset;
            self.tx_ring.desc[i].buf_addr_low = (buf_phys & 0xFFFFFFFF) as u32;
            self.tx_ring.desc[i].buf_addr_high = (buf_phys >> 32) as u32;
            let mut status = 0;
            if i == NUM_TX_DESC - 1 { status |= 1 << 30; } 
            self.tx_ring.desc[i].command_status = status;
        }

        let rx_ring_phys = (&*self.rx_ring as *const _ as u64) - phys_offset;
        let tx_ring_phys = (&*self.tx_ring as *const _ as u64) - phys_offset;

        unsafe {
            core::ptr::write_volatile((self.mmio_base + 0x50) as *mut u8, 0xC0);
            core::ptr::write_volatile((self.mmio_base + 0xE4) as *mut u32, (rx_ring_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.mmio_base + 0xE8) as *mut u32, (rx_ring_phys >> 32) as u32);
            core::ptr::write_volatile((self.mmio_base + 0x20) as *mut u32, (tx_ring_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.mmio_base + 0x24) as *mut u32, (tx_ring_phys >> 32) as u32);
            core::ptr::write_volatile((self.mmio_base + 0xDA) as *mut u16, 1536);
            core::ptr::write_volatile((self.mmio_base + 0x44) as *mut u32, 0xE70F);
            core::ptr::write_volatile((self.mmio_base + 0x40) as *mut u32, 0x03000700);
            core::ptr::write_volatile((self.mmio_base + 0x37) as *mut u8, 0x0C);
            core::ptr::write_volatile((self.mmio_base + 0x50) as *mut u8, 0x00);
        }

        crate::serial_println!("[RTL8168] DMA Rings linked. Network Hardware Ready.");
    }

    pub fn transmit_raw(&mut self, data: &[u8]) {
        if data.len() > 1536 { return; }
        let idx = self.tx_index;
        
        if self.tx_ring.desc[idx].command_status & (1 << 31) != 0 { return; }

        self.tx_buffers[idx][..data.len()].copy_from_slice(data);
        let mut cmd = data.len() as u32;
        cmd |= (1 << 28) | (1 << 29) | (1 << 31);
        if idx == NUM_TX_DESC - 1 { cmd |= 1 << 30; } 

        self.tx_ring.desc[idx].command_status = cmd;
        self.tx_index = (self.tx_index + 1) % NUM_TX_DESC;

        unsafe { core::ptr::write_volatile((self.mmio_base + 0x38) as *mut u8, 0x40); }
    }

    pub fn receive_raw(&mut self) -> Option<Vec<u8>> {
        let idx = self.rx_index;
        let status = self.rx_ring.desc[idx].command_status;

        if (status & (1 << 31)) != 0 { return None; }

        let len = (status & 0x3FFF) as usize;
        let mut packet = Vec::with_capacity(len);
        packet.extend_from_slice(&self.rx_buffers[idx][..len]);

        let mut new_status = (1 << 31) | 1536; 
        if idx == NUM_RX_DESC - 1 { new_status |= 1 << 30; } 
        self.rx_ring.desc[idx].command_status = new_status;
        self.rx_index = (self.rx_index + 1) % NUM_RX_DESC;

        Some(packet)
    }
}

// ==========================================
// SMOLTCP HARDWARE BRIDGE
// ==========================================

pub struct NyxRxToken(Vec<u8>);
impl RxToken for NyxRxToken {
    fn consume<R, F>(mut self, f: F) -> R where F: FnOnce(&mut [u8]) -> R {
        f(&mut self.0)
    }
}

pub struct NyxTxToken<'a>(&'a mut Rtl8168Driver);
impl<'a> TxToken for NyxTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R where F: FnOnce(&mut [u8]) -> R {
        let mut buffer = vec![0; len];
        let result = f(&mut buffer);
        self.0.transmit_raw(&buffer);
        result
    }
}

impl Device for Rtl8168Driver {
    type RxToken<'a> = NyxRxToken where Self: 'a;
    type TxToken<'a> = NyxTxToken<'a> where Self: 'a;

    fn receive<'a>(&'a mut self, _timestamp: Instant) -> Option<(Self::RxToken<'a>, Self::TxToken<'a>)> {
        if let Some(packet) = self.receive_raw() {
            Some((NyxRxToken(packet), NyxTxToken(self)))
        } else {
            None
        }
    }

    fn transmit<'a>(&'a mut self, _timestamp: Instant) -> Option<Self::TxToken<'a>> {
        Some(NyxTxToken(self))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(1);
        caps.medium = Medium::Ethernet;
        caps
    }
}