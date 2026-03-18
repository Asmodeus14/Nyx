use crate::pci::PciDevice;
use crate::gui::Painter;
use core::ptr::{read_volatile, write_volatile};
use alloc::vec::Vec;
use alloc::vec;
use alloc::boxed::Box;
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken, ChecksumCapabilities};
use smoltcp::time::Instant;

const NUM_TX_DESC: usize = 16;
const NUM_RX_DESC: usize = 16;
const BUFFER_SIZE: usize = 2048;

const REG_MAC_0: usize = 0x00;
const REG_CR: usize = 0x37;    
const REG_TNPDS: usize = 0x20; 
const REG_RDSAR: usize = 0xE4; 
const REG_RCR: usize = 0x44;   
const REG_TCR: usize = 0x40;   
const REG_RMS: usize = 0xDA;   

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct RtkDescriptor {
    pub command_status: u32,
    pub vlan: u32,
    pub buf_addr_low: u32,
    pub buf_addr_high: u32,
}

// STRICT 256-BYTE ALIGNMENT FOR HARDWARE DMA RINGS
#[repr(C, align(256))]
#[derive(Clone)]
pub struct DmaRing {
    pub descs: [RtkDescriptor; 16],
}

// STRICT 256-BYTE ALIGNMENT FOR PAYLOAD BUFFERS
#[repr(C, align(256))]
pub struct DmaBuffer {
    pub data: [u8; BUFFER_SIZE],
}

pub struct Rtl8168Driver {
    pci_device: PciDevice,
    mmio_base: u64,
    pub mac_address: [u8; 6],
    
    tx_ring: Box<DmaRing>,
    rx_ring: Box<DmaRing>,
    tx_buffers: Vec<Box<DmaBuffer>>,
    rx_buffers: Vec<Box<DmaBuffer>>,
    
    tx_index: usize,
    rx_index: usize,
}

impl Rtl8168Driver {
    pub fn new(pci_device: PciDevice, mmio_phys: u64) -> Self {
        let mmio_virt = crate::memory::phys_to_virt(mmio_phys).unwrap();
        let empty_desc = RtkDescriptor { command_status: 0, vlan: 0, buf_addr_low: 0, buf_addr_high: 0 };
        
        let mut tx_bufs = Vec::new();
        let mut rx_bufs = Vec::new();
        for _ in 0..NUM_TX_DESC { tx_bufs.push(Box::new(DmaBuffer { data: [0; BUFFER_SIZE] })); }
        for _ in 0..NUM_RX_DESC { rx_bufs.push(Box::new(DmaBuffer { data: [0; BUFFER_SIZE] })); }

        let mut driver = Rtl8168Driver {
            pci_device,
            mmio_base: mmio_virt,
            mac_address: [0; 6],
            tx_ring: Box::new(DmaRing { descs: [empty_desc; 16] }),
            rx_ring: Box::new(DmaRing { descs: [empty_desc; 16] }),
            tx_buffers: tx_bufs,
            rx_buffers: rx_bufs,
            tx_index: 0,
            rx_index: 0,
        };
        
        for i in 0..6 { driver.mac_address[i] = driver.read8(REG_MAC_0 + i); }
        driver
    }

    fn read8(&self, offset: usize) -> u8 { unsafe { read_volatile((self.mmio_base + offset as u64) as *const u8) } }
    fn write8(&mut self, offset: usize, val: u8) { unsafe { write_volatile((self.mmio_base + offset as u64) as *mut u8, val) } }
    fn write16(&mut self, offset: usize, val: u16) { unsafe { write_volatile((self.mmio_base + offset as u64) as *mut u16, val) } }
    fn write32(&mut self, offset: usize, val: u32) { unsafe { write_volatile((self.mmio_base + offset as u64) as *mut u32, val) } }

    pub fn initialize(&mut self) {
        crate::serial_println!("[RTL8168] Resetting NIC...");
        self.write8(REG_CR, 0x10); 
        while (self.read8(REG_CR) & 0x10) != 0 { core::hint::spin_loop(); }

        let bus = self.pci_device.bus as u32;
        let slot = self.pci_device.device as u32;
        let func = self.pci_device.func as u32;
        let address = 0x80000000 | (bus << 16) | (slot << 11) | (func << 8) | 0x04;
        unsafe {
            let mut port_addr = x86_64::instructions::port::Port::<u32>::new(0xCF8);
            let mut port_data = x86_64::instructions::port::Port::<u32>::new(0xCFC);
            port_addr.write(address);
            let mut cmd = port_data.read();
            cmd |= 0x06; 
            port_addr.write(address);
            port_data.write(cmd);
        }

        self.write8(0x50, 0xC0); 
        let config3 = self.read8(0x52); 
        self.write8(0x52, config3 & !0x01); 
        self.write8(0x50, 0x00); 

        self.write16(REG_RMS, 1536);

        for i in 0..NUM_RX_DESC {
            let phys_addr = crate::memory::virt_to_phys(self.rx_buffers[i].data.as_ptr() as u64).unwrap();
            self.rx_ring.descs[i].buf_addr_low = (phys_addr & 0xFFFFFFFF) as u32;
            self.rx_ring.descs[i].buf_addr_high = (phys_addr >> 32) as u32;
            let mut cmd = BUFFER_SIZE as u32 | (1 << 31);
            if i == NUM_RX_DESC - 1 { cmd |= 1 << 30; } 
            self.rx_ring.descs[i].command_status = cmd;
        }

        for i in 0..NUM_TX_DESC {
            let phys_addr = crate::memory::virt_to_phys(self.tx_buffers[i].data.as_ptr() as u64).unwrap();
            self.tx_ring.descs[i].buf_addr_low = (phys_addr & 0xFFFFFFFF) as u32;
            self.tx_ring.descs[i].buf_addr_high = (phys_addr >> 32) as u32;
        }

        let rx_ring_phys = crate::memory::virt_to_phys(self.rx_ring.descs.as_ptr() as u64).unwrap();
        self.write32(REG_RDSAR, (rx_ring_phys & 0xFFFFFFFF) as u32);
        self.write32(REG_RDSAR + 4, (rx_ring_phys >> 32) as u32);
        
        let tx_ring_phys = crate::memory::virt_to_phys(self.tx_ring.descs.as_ptr() as u64).unwrap();
        self.write32(REG_TNPDS, (tx_ring_phys & 0xFFFFFFFF) as u32);
        self.write32(REG_TNPDS + 4, (tx_ring_phys >> 32) as u32);

        self.write32(REG_RCR, 0xE73F); 
        self.write32(REG_TCR, 0x03000700); 
        self.write8(REG_CR, 0x0C); 
    }
}

// ==========================================
// SMOLTCP BINDINGS
// ==========================================
impl Device for Rtl8168Driver {
    type RxToken<'a> = RtkRxToken where Self: 'a;
    type TxToken<'a> = RtkTxToken<'a> where Self: 'a;

    fn receive<'a>(&'a mut self, _timestamp: Instant) -> Option<(Self::RxToken<'a>, Self::TxToken<'a>)> {
        let cmd_ptr = core::ptr::addr_of!(self.rx_ring.descs[self.rx_index].command_status);
        let status = unsafe { core::ptr::read_volatile(cmd_ptr) };
        
        if (status & (1 << 31)) == 0 {
            // STRIP THE 4-BYTE HARDWARE CRC
            let length = ((status & 0x3FFF) as usize).saturating_sub(4); 
            
            let mut packet = alloc::vec![0; length];
            packet.copy_from_slice(&self.rx_buffers[self.rx_index].data[..length]);

            // --- THE VISUAL HEX DUMP SNIFFER (LIVE MODE) ---
            if length >= 14 {
                let dest_mac = alloc::format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", packet[0], packet[1], packet[2], packet[3], packet[4], packet[5]);
                let eth_type = (packet[12] as u16) << 8 | (packet[13] as u16);
                
                if eth_type == 0x0806 {
                    unsafe {
                        if let Some(painter) = &mut crate::SCREEN_PAINTER {
                            let msg1 = alloc::format!("RX: {}b, Dest MAC: {}     ", length, dest_mac);
                            painter.draw_string(10, 80, &msg1, crate::gui::Color::CYAN);
                            
                            let msg2 = alloc::format!("Raw Bytes 12-15: {:02X} {:02X} {:02X} {:02X}", packet[12], packet[13], packet[14], packet[15]);
                            painter.draw_string(10, 100, &msg2, crate::gui::Color::WHITE);

                            painter.draw_string(10, 120, "SNIFFER: TARGET ARP REQUEST CAUGHT!        ", crate::gui::Color::YELLOW);
                        }
                    }
                }
            }

            let cmd_mut_ptr = core::ptr::addr_of_mut!(self.rx_ring.descs[self.rx_index].command_status);
            let mut reset_cmd = BUFFER_SIZE as u32 | (1 << 31);
            if self.rx_index == NUM_RX_DESC - 1 { reset_cmd |= 1 << 30; } 
            unsafe { core::ptr::write_volatile(cmd_mut_ptr, reset_cmd) };

            self.rx_index = (self.rx_index + 1) % NUM_RX_DESC;

            Some((RtkRxToken(packet), RtkTxToken(self)))
        } else {
            None 
        }
    }

    fn transmit<'a>(&'a mut self, _timestamp: Instant) -> Option<Self::TxToken<'a>> {
        Some(RtkTxToken(self))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500; 
        caps.max_burst_size = Some(1);
        caps.checksum = ChecksumCapabilities::default();
        caps
    }
}

pub struct RtkRxToken(Vec<u8>);
impl smoltcp::phy::RxToken for RtkRxToken {
    fn consume<R, F>(mut self, f: F) -> R where F: FnOnce(&mut [u8]) -> R {
        f(&mut self.0)
    }
}

pub struct RtkTxToken<'a>(&'a mut Rtl8168Driver);
impl<'a> smoltcp::phy::TxToken for RtkTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R where F: FnOnce(&mut [u8]) -> R {
        let mut buffer = alloc::vec![0; len];
        let result = f(&mut buffer);

        let idx = self.0.tx_index;
        self.0.tx_buffers[idx].data[..len].copy_from_slice(&buffer);

        let cmd_mut_ptr = core::ptr::addr_of_mut!(self.0.tx_ring.descs[idx].command_status);
        let mut cmd = (len as u32) | (1 << 31) | (1 << 29) | (1 << 28);
        if idx == NUM_TX_DESC - 1 { cmd |= 1 << 30; } 
        
        unsafe { core::ptr::write_volatile(cmd_mut_ptr, cmd) };
        self.0.tx_index = (idx + 1) % NUM_TX_DESC;

        self.0.write8(0x38, 0x40); 

        unsafe {
            if let Some(painter) = &mut crate::SCREEN_PAINTER {
                let msg = alloc::format!("TX REPLY: {} bytes!    ", len);
                painter.draw_string(10, 140, &msg, crate::gui::Color::GREEN);
            }
        }

        result
    }
}