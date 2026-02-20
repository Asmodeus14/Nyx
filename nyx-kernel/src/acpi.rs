use core::slice;
use crate::memory::phys_to_virt;

#[repr(C, packed)]
struct RsdpDescriptor {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

#[repr(C, packed)]
struct RsdpDescriptor20 {
    first_part: RsdpDescriptor,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

pub struct AcpiTables {
    pub mcfg_addr: Option<u64>,
    pub madt_addr: Option<u64>,
}

pub static mut ACPI_INFO: AcpiTables = AcpiTables { mcfg_addr: None, madt_addr: None };

pub fn init(rsdp_physical_addr: u64) {
    crate::serial_println!("[ACPI] Found RSDP at physical address: {:#x}", rsdp_physical_addr);
    crate::vga_println!("[ACPI] Found RSDP at physical address: {:#x}", rsdp_physical_addr);

    // Convert the physical address to a virtual address our kernel can read
    let rsdp_virt = phys_to_virt(rsdp_physical_addr).expect("Failed to map RSDP");
    
    let rsdp = unsafe { &*(rsdp_virt as *const RsdpDescriptor) };
    let sig = core::str::from_utf8(&rsdp.signature).unwrap_or("INVALID");
    
    if sig != "RSD PTR " {
        crate::vga_println!("[ACPI] ERR: Invalid RSDP signature!");
        return;
    }

    let mut xsdt_virt = 0;
    let mut entries_count = 0;
    let mut is_xsdt = false;

    // ACPI 2.0+ uses XSDT (64-bit pointers), ACPI 1.0 uses RSDT (32-bit pointers)
    if rsdp.revision >= 2 {
        let rsdp20 = unsafe { &*(rsdp_virt as *const RsdpDescriptor20) };
        xsdt_virt = phys_to_virt(rsdp20.xsdt_address).unwrap();
        let header = unsafe { &*(xsdt_virt as *const SdtHeader) };
        entries_count = (header.length as usize - core::mem::size_of::<SdtHeader>()) / 8;
        is_xsdt = true;
        crate::serial_println!("[ACPI] Parsed XSDT (ACPI 2.0+). Entries: {}", entries_count);
    } else {
        xsdt_virt = phys_to_virt(rsdp.rsdt_address as u64).unwrap();
        let header = unsafe { &*(xsdt_virt as *const SdtHeader) };
        entries_count = (header.length as usize - core::mem::size_of::<SdtHeader>()) / 4;
        crate::serial_println!("[ACPI] Parsed RSDT (ACPI 1.0). Entries: {}", entries_count);
    }

    // Now we iterate through all the tables looking for MCFG and APIC
    let entry_base = xsdt_virt + core::mem::size_of::<SdtHeader>() as u64;
    
    for i in 0..entries_count {
        let table_phys_addr = if is_xsdt {
            unsafe { *((entry_base + (i * 8) as u64) as *const u64) }
        } else {
            // FIX: The cast is now safely inside the unsafe block
            unsafe { *((entry_base + (i * 4) as u64) as *const u32) as u64 }
        };

        if let Some(table_virt) = phys_to_virt(table_phys_addr) {
            let header = unsafe { &*(table_virt as *const SdtHeader) };
            if let Ok(sig) = core::str::from_utf8(&header.signature) {
                if sig == "MCFG" {
                    crate::serial_println!("[ACPI] -> Found MCFG (PCIe Config) at {:#x}", table_phys_addr);
                    unsafe { ACPI_INFO.mcfg_addr = Some(table_phys_addr); }
                } else if sig == "APIC" {
                    crate::serial_println!("[ACPI] -> Found MADT (APIC) at {:#x}", table_phys_addr);
                    unsafe { ACPI_INFO.madt_addr = Some(table_phys_addr); }
                }
            }
        }
    }
    
    if unsafe { ACPI_INFO.mcfg_addr.is_some() } {
        crate::vga_println!("[ACPI] Motherboard PCIe & APIC tables parsed!");
    } else {
        crate::vga_println!("[ACPI] Warning: MCFG not found. PCIe might fallback to legacy.");
    }
}