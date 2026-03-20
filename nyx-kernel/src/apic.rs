use crate::acpi::ACPI_INFO;
use crate::memory::phys_to_virt;
use core::ptr::{read_volatile, write_volatile};
use alloc::vec::Vec;

#[repr(C, packed)]
struct MadtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
    local_apic_addr: u32, 
    flags: u32,
}

#[repr(C, packed)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub length: u8,
}

#[repr(C, packed)]
pub struct ProcessorLocalApic {
    pub entry_type: u8,
    pub length: u8,
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
}

pub fn init() {
    let madt_phys = unsafe { ACPI_INFO.madt_addr };
    if madt_phys.is_none() { 
        crate::serial_println!("[APIC] Cannot init: MADT address is missing.");
        return; 
    }

    let madt_virt = phys_to_virt(madt_phys.unwrap()).unwrap();
    let madt = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let apic_phys = madt.local_apic_addr as u64;
    
    if unsafe { crate::memory::map_mmio(apic_phys, 4096) }.is_err() { 
        crate::serial_println!("[APIC] FATAL: Failed to map Local APIC MMIO.");
        return; 
    }
    
    let apic_virt = phys_to_virt(apic_phys).unwrap();

    unsafe {
        let sivr_ptr = (apic_virt + 0xF0) as *mut u32;
        let current_sivr = read_volatile(sivr_ptr);
        write_volatile(sivr_ptr, current_sivr | 0x1FF); 
    }

    crate::vga_println!("[APIC] Local APIC Enabled! Ready for MSI (Modern Interrupts).");
}

/// Dynamically parses the MADT to find all enabled CPU cores
pub fn get_cpu_apic_ids() -> Vec<u32> {
    let madt_phys = unsafe { ACPI_INFO.madt_addr };
    if madt_phys.is_none() {
        crate::serial_println!("[APIC] MADT missing. Defaulting to 1 core.");
        return alloc::vec![0]; 
    }

    let madt_virt = phys_to_virt(madt_phys.unwrap()).unwrap();
    let header = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let end_ptr = madt_virt + header.length as u64;
    let mut current_ptr = madt_virt + core::mem::size_of::<MadtHeader>() as u64;
    
    let mut ids = Vec::new();

    while current_ptr < end_ptr {
        let entry = unsafe { &*(current_ptr as *const MadtEntryHeader) };
        
        // Type 0 is a Processor Local APIC (a CPU Core)
        if entry.entry_type == 0 { 
            let proc = unsafe { &*(current_ptr as *const ProcessorLocalApic) };
            
            // Bit 0 of the flags indicates if the processor is enabled
            if (proc.flags & 1) != 0 {
                ids.push(proc.apic_id as u32);
            }
        }
        
        if entry.length == 0 {
            crate::serial_println!("[APIC] ERR: Zero-length MADT entry detected, aborting parse.");
            break;
        }
        
        current_ptr += entry.length as u64;
    }

    crate::serial_println!("[APIC] Detected {} active CPU cores dynamically.", ids.len());
    ids
}
const ICR_LOW: u64 = 0x300;
const ICR_HIGH: u64 = 0x310;

// Helper function to dynamically find the mapped APIC Base
fn get_apic_virt_base() -> u64 {
    let madt_phys = unsafe { ACPI_INFO.madt_addr.expect("MADT missing") };
    let madt_virt = crate::memory::phys_to_virt(madt_phys).unwrap();
    let madt = unsafe { &*(madt_virt as *const MadtHeader) };
    let apic_phys = madt.local_apic_addr as u64;
    crate::memory::phys_to_virt(apic_phys).unwrap()
}

pub fn send_init(target_apic_id: u32) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let icr_high = (apic_virt + ICR_HIGH) as *mut u32;
        let icr_low = (apic_virt + ICR_LOW) as *mut u32;

        write_volatile(icr_high, target_apic_id << 24);
        // Delivery mode: INIT (0b101 << 8), Assert (1 << 14), Level (1 << 15)
        write_volatile(icr_low, 0x0000_4500); 
    }
}

pub fn send_sipi(target_apic_id: u32, vector: u8) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let icr_high = (apic_virt + ICR_HIGH) as *mut u32;
        let icr_low = (apic_virt + ICR_LOW) as *mut u32;

        write_volatile(icr_high, target_apic_id << 24);
        // Delivery mode: Startup (0b110 << 8) | Vector
        let command = 0x0000_4600 | (vector as u32);
        write_volatile(icr_low, command);
    }
}

pub fn init_ap() {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let sivr_ptr = (apic_virt + 0xF0) as *mut u32;
        let current_sivr = core::ptr::read_volatile(sivr_ptr);
        // Enable APIC (bit 8) and set spurious interrupt vector to 0xFF
        write_volatile(sivr_ptr, current_sivr | 0x1FF); 
    }
}


#[repr(C, packed)]
pub struct IoApicEntry {
    pub entry_type: u8,
    pub length: u8,
    pub io_apic_id: u8,
    pub reserved: u8,
    pub io_apic_address: u32,
    pub global_system_interrupt_base: u32,
}

/// Parses the MADT to find the physical memory address of the I/O APIC
pub fn get_ioapic_phys_addr() -> Option<u64> {
    let madt_phys = unsafe { ACPI_INFO.madt_addr }?;
    let madt_virt = crate::memory::phys_to_virt(madt_phys)?;
    let header = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let end_ptr = madt_virt + header.length as u64;
    let mut current_ptr = madt_virt + core::mem::size_of::<MadtHeader>() as u64;

    while current_ptr < end_ptr {
        let entry = unsafe { &*(current_ptr as *const MadtEntryHeader) };
        
        // Type 1 is the I/O APIC
        if entry.entry_type == 1 { 
            let io_apic = unsafe { &*(current_ptr as *const IoApicEntry) };
            return Some(io_apic.io_apic_address as u64);
        }
        
        if entry.length == 0 { break; }
        current_ptr += entry.length as u64;
    }
    None
}
// Add to the bottom of nyx-kernel/src/apic.rs

/// Sends the End Of Interrupt (EOI) signal to the modern Local APIC.
/// Without this, the APIC will block all future hardware interrupts!
pub fn end_of_interrupt() {
    let apic_virt = get_apic_virt_base(); 
    unsafe {
        // The EOI register is always located at offset 0xB0 from the APIC base
        let eoi_ptr = (apic_virt + 0xB0) as *mut u32;
        core::ptr::write_volatile(eoi_ptr, 0);
    }
}