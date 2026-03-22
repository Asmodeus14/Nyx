use crate::acpi::ACPI_INFO;
use crate::memory::phys_to_virt;
use core::ptr::{read_volatile, write_volatile};
use alloc::vec::Vec;

// --- FAST HARDWARE CACHE ---
// This prevents the OS from re-parsing ACPI tables during high-speed interrupts!
static mut LOCAL_APIC_VIRT: u64 = 0;

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
    
    // 👉 CACHE IT FOR LIGHTNING-FAST INTERRUPTS!
    unsafe { LOCAL_APIC_VIRT = apic_virt; }

    unsafe {
        let sivr_ptr = (apic_virt + 0xF0) as *mut u32;
        let current_sivr = read_volatile(sivr_ptr);
        write_volatile(sivr_ptr, current_sivr | 0x1FF); 
    }

    crate::vga_println!("[APIC] Local APIC Enabled! Ready for MSI (Modern Interrupts).");
}

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
        if entry.entry_type == 0 { 
            let proc = unsafe { &*(current_ptr as *const ProcessorLocalApic) };
            if (proc.flags & 1) != 0 {
                ids.push(proc.apic_id as u32);
            }
        }
        if entry.length == 0 { break; }
        current_ptr += entry.length as u64;
    }

    crate::serial_println!("[APIC] Detected {} active CPU cores dynamically.", ids.len());
    ids
}

const ICR_LOW: u64 = 0x300;
const ICR_HIGH: u64 = 0x310;

// Uses the ultra-fast static cache instead of parsing ACPI
fn get_apic_virt_base() -> u64 {
    unsafe {
        if LOCAL_APIC_VIRT != 0 {
            return LOCAL_APIC_VIRT;
        }
    }
    // Emergency Fallback
    let madt_phys = unsafe { ACPI_INFO.madt_addr.expect("MADT missing") };
    let madt_virt = crate::memory::phys_to_virt(madt_phys).unwrap();
    let madt = unsafe { &*(madt_virt as *const MadtHeader) };
    crate::memory::phys_to_virt(madt.local_apic_addr as u64).unwrap()
}

pub fn send_init(target_apic_id: u32) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let icr_high = (apic_virt + ICR_HIGH) as *mut u32;
        let icr_low = (apic_virt + ICR_LOW) as *mut u32;
        write_volatile(icr_high, target_apic_id << 24);
        write_volatile(icr_low, 0x0000_4500); 
    }
}

pub fn send_sipi(target_apic_id: u32, vector: u8) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let icr_high = (apic_virt + ICR_HIGH) as *mut u32;
        let icr_low = (apic_virt + ICR_LOW) as *mut u32;
        write_volatile(icr_high, target_apic_id << 24);
        write_volatile(icr_low, 0x0000_4600 | (vector as u32));
    }
}

pub fn init_ap() {
    let apic_virt = get_apic_virt_base();
    unsafe {
        let sivr_ptr = (apic_virt + 0xF0) as *mut u32;
        let current_sivr = core::ptr::read_volatile(sivr_ptr);
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

pub fn get_ioapic_phys_addr() -> Option<u64> {
    let madt_phys = unsafe { ACPI_INFO.madt_addr }?;
    let madt_virt = crate::memory::phys_to_virt(madt_phys)?;
    let header = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let end_ptr = madt_virt + header.length as u64;
    let mut current_ptr = madt_virt + core::mem::size_of::<MadtHeader>() as u64;

    while current_ptr < end_ptr {
        let entry = unsafe { &*(current_ptr as *const MadtEntryHeader) };
        if entry.entry_type == 1 { 
            let io_apic = unsafe { &*(current_ptr as *const IoApicEntry) };
            return Some(io_apic.io_apic_address as u64);
        }
        if entry.length == 0 { break; }
        current_ptr += entry.length as u64;
    }
    None
}

pub fn end_of_interrupt() {
    let apic_virt = get_apic_virt_base(); 
    unsafe {
        let eoi_ptr = (apic_virt + 0xB0) as *mut u32;
        core::ptr::write_volatile(eoi_ptr, 0);
    }
}

const LVT_TIMER: u64 = 0x320;
const TIMER_INITIAL_COUNT: u64 = 0x380;
const TIMER_DIVIDE_CONFIG: u64 = 0x3E0;
pub fn init_timer(vector: u8) {
    let apic_virt = get_apic_virt_base();
    unsafe {
        // Clear Task Priority Register (TPR)
        let tpr_ptr = (apic_virt + 0x80) as *mut u32;
        core::ptr::write_volatile(tpr_ptr, 0);

        // Divide Configuration Register
        let dcr_ptr = (apic_virt + 0x3E0) as *mut u32;
        core::ptr::write_volatile(dcr_ptr, 0x3);

        // LVT Timer Register
        let lvt_timer_ptr = (apic_virt + 0x320) as *mut u32;
        core::ptr::write_volatile(lvt_timer_ptr, 0x20000 | (vector as u32));

        // 👉 THE FIX: A safer, slightly slower tick rate for GUI rendering
        let icr_ptr = (apic_virt + 0x380) as *mut u32;
        core::ptr::write_volatile(icr_ptr, 0x0100_0000); 
    }
}