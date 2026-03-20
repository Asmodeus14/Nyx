// nyx-kernel/src/ioapic.rs

use core::ptr::{read_volatile, write_volatile};

static mut IOAPIC_VIRT: Option<u64> = None;

// IOAPIC Register Offsets
const IOREGSEL: u64 = 0x00; // Index Register
const IOWIN: u64 = 0x10;    // Data Register

pub fn init() {
    crate::serial_println!("[IOAPIC] Initializing Modern Hardware Routing...");

    let phys_addr = match crate::apic::get_ioapic_phys_addr() {
        Some(addr) => addr,
        None => {
            crate::serial_println!("[IOAPIC] ERR: I/O APIC not found in ACPI tables!");
            return;
        }
    };

    // Map the physical IOAPIC memory space to virtual memory
    if unsafe { crate::memory::map_mmio(phys_addr, 4096) }.is_err() {
        crate::serial_println!("[IOAPIC] ERR: Failed to map I/O APIC MMIO!");
        return;
    }

    unsafe {
        IOAPIC_VIRT = crate::memory::phys_to_virt(phys_addr);
    }
    
    crate::serial_println!("[IOAPIC] Online at physical address {:#x}", phys_addr);
}

/// Read a 32-bit value from an IOAPIC register
unsafe fn read(reg: u8) -> u32 {
    let base = IOAPIC_VIRT.expect("IOAPIC not mapped!");
    write_volatile((base + IOREGSEL) as *mut u32, reg as u32);
    read_volatile((base + IOWIN) as *const u32)
}

/// Write a 32-bit value to an IOAPIC register
unsafe fn write(reg: u8, data: u32) {
    let base = IOAPIC_VIRT.expect("IOAPIC not mapped!");
    write_volatile((base + IOREGSEL) as *mut u32, reg as u32);
    write_volatile((base + IOWIN) as *mut u32, data);
}

/// Routes a specific hardware IRQ to a specific CPU core's Local APIC.
/// `irq`: The hardware IRQ line (e.g., 1 for Keyboard, 12 for Mouse)
/// `apic_id`: The destination CPU core (0 for BSP, 1-7 for APs)
/// `vector`: The IDT vector index to trigger (e.g., 33 for Keyboard)
pub fn route_irq(irq: u8, apic_id: u8, vector: u8) {
    // Each IRQ redirection entry is 64 bits wide (two 32-bit registers)
    // IRQ 0 starts at register 0x10, IRQ 1 at 0x12, etc.
    let reg_low = 0x10 + (irq * 2);
    let reg_high = 0x10 + (irq * 2) + 1;

    // Build the Redirection Table Entry (RTE)
    // Low 32 bits: Vector, Delivery Mode (Fixed), Polarity, Trigger Mode, Unmasked
    let low_value = vector as u32; 
    
    // High 32 bits: Destination Local APIC ID in the top 8 bits
    let high_value = (apic_id as u32) << 24;

    unsafe {
        write(reg_low, low_value);
        write(reg_high, high_value);
    }
    
    crate::serial_println!("[IOAPIC] Routed IRQ {} -> CPU {} (Vector {})", irq, apic_id, vector);
}