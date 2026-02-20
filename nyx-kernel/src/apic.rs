use crate::acpi::ACPI_INFO;
use crate::memory::phys_to_virt;
use core::ptr::{read_volatile, write_volatile};

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

pub fn init() {
    let madt_phys = unsafe { ACPI_INFO.madt_addr };
    if madt_phys.is_none() { return; }

    let madt_virt = phys_to_virt(madt_phys.unwrap()).unwrap();
    let madt = unsafe { &*(madt_virt as *const MadtHeader) };
    
    let apic_phys = madt.local_apic_addr as u64;
    
    if unsafe { crate::memory::map_mmio(apic_phys, 4096) }.is_err() { return; }
    
    let apic_virt = phys_to_virt(apic_phys).unwrap();

    unsafe {
        let sivr_ptr = (apic_virt + 0xF0) as *mut u32;
        let current_sivr = read_volatile(sivr_ptr);
        write_volatile(sivr_ptr, current_sivr | 0x1FF); 
    }

    crate::vga_println!("[APIC] Local APIC Enabled! Ready for MSI (Modern Interrupts).");
}