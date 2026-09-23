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

/// How many redirection entries this IOAPIC has (IOAPICVER bits 23:16, plus one). `None` if it was
/// never mapped. The PCH's IOAPIC has 120; a GSI at or past this number is not on it.
pub fn entries() -> Option<u32> {
    unsafe {
        IOAPIC_VIRT?;
        Some(((read(0x01) >> 16) & 0xFF) + 1)
    }
}

/// Route a PCH interrupt that is LEVEL-triggered and ACTIVE-LOW — what an ACPI
/// `Interrupt (ResourceConsumer, Level, ActiveLow, …)` descriptor asks for — to `vector` on
/// `apic_id`, starting MASKED. Unmask with [`set_masked`] once a handler can take it.
///
/// ⚠️ Unlike [`route_irq`], which programs the ISA-style edge/active-high defaults the 8042 lines
/// need. A level line routed as edge fires once and never again; routed active-high it fires
/// constantly while idle.
pub fn route_gsi_level_low(gsi: u8, apic_id: u8, vector: u8) {
    const POLARITY_LOW: u32 = 1 << 13;
    const TRIGGER_LEVEL: u32 = 1 << 15;
    const MASKED: u32 = 1 << 16;
    let reg = 0x10u8.wrapping_add(gsi.wrapping_mul(2));
    unsafe {
        write(reg, MASKED | TRIGGER_LEVEL | POLARITY_LOW | vector as u32);
        write(reg + 1, (apic_id as u32) << 24);
    }
}

/// Mask or unmask one redirection entry, leaving the rest of it as programmed.
///
/// ⚠️ The IOREGSEL/IOWIN pair is a two-step access, so callers must not be interruptible between
/// them on this core — call with interrupts masked (an interrupt handler already is).
pub fn set_masked(gsi: u8, masked: bool) {
    let reg = 0x10u8.wrapping_add(gsi.wrapping_mul(2));
    unsafe {
        let low = read(reg);
        let new = if masked { low | (1 << 16) } else { low & !(1 << 16) };
        write(reg, new);
    }
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