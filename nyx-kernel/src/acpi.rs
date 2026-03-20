// Turn off Rust's strict naming rules so it doesn't complain about C-style variable names
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

// ==========================================
// 1. INJECT THE GENERATED C-TO-RUST BINDINGS
// ==========================================
include!(concat!(env!("OUT_DIR"), "/acpi_bindings.rs"));

// Legacy ACPI info to keep the PCI module compiling while we transition
pub struct AcpiInfo {
    pub rsdp_addr: Option<u64>,
    pub mcfg_addr: Option<u64>,
    pub madt_addr: Option<u64>, 
}

pub static mut ACPI_INFO: AcpiInfo = AcpiInfo { 
    rsdp_addr: None, 
    mcfg_addr: None, 
    madt_addr: None 
};

pub fn init(rsdp: u64) {
    unsafe { ACPI_INFO.rsdp_addr = Some(rsdp); }
}

// ==========================================
// 2. THE NEW INTEL INIT SEQUENCE
// ==========================================
pub fn init_intel_acpica() {
    crate::serial_println!("[ACPI] Booting Intel ACPICA Engine...");
    
    unsafe {
        // 1. Boot the core engine
        let mut status = AcpiInitializeSubsystem();
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Subsystem Init Failed! Status: {}", status);
            return;
        }

        // 2. Copy the ACPI tables from the motherboard into our memory
        crate::serial_println!("[ACPI] Initializing ACPI Tables...");
        status = AcpiInitializeTables(core::ptr::null_mut(), 16, 0);
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Table Init Failed! Status: {}", status);
            return;
        }

        // --- PHASE 0: DYNAMIC MADT DETECTION ---
        crate::serial_println!("[ACPI] Extracting MADT for real SMP...");
        let mut madt_header: *mut core::ffi::c_void = core::ptr::null_mut();
        
        // Ask ACPICA to find the "APIC" table (MADT). Instance 1.
        let get_status = AcpiGetTable(
            b"APIC\0".as_ptr() as *mut i8, 
            1, 
            &mut madt_header as *mut _ as *mut *mut _
        );
        
        if get_status == 0 && !madt_header.is_null() {
            let madt_virt = madt_header as u64;
            // ACPICA maps this for us. We need to find the physical address for our APIC module.
            let madt_phys = crate::memory::virt_to_phys(madt_virt).unwrap_or(madt_virt);
            
            ACPI_INFO.madt_addr = Some(madt_phys);
            crate::serial_println!("[ACPI] MADT stored dynamically @ {:#x}", madt_phys);
        } else {
            crate::serial_println!("[ACPI] WARNING: MADT not found! Status: {}", get_status);
        }
        // ---------------------------------------

        // 3. Build the Hardware Namespace Tree
        crate::serial_println!("[ACPI] Loading Hardware Namespace...");
        status = AcpiLoadTables();
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Namespace Load Failed! Status: {}", status);
            return;
        }

        // 4. Transition the motherboard from Legacy mode to ACPI mode
        crate::serial_println!("[ACPI] Enabling ACPI Hardware Mode...");
        status = AcpiEnableSubsystem(0);
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Subsystem Enable Failed! Status: {}", status);
            return;
        }

        // 5. Execute the `_INI` methods to turn on the hidden hardware
        crate::serial_println!("[ACPI] Initializing Hardware Objects...");
        status = AcpiInitializeObjects(0);
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Object Init Failed! Status: {}", status);
            return;
        }

        crate::serial_println!("[ACPI] Intel ACPICA is FULLY ONLINE.");
    }
}

// ==========================================
// 3. CUSTOM ACPI METHODS
// ==========================================
extern "C" {
    fn acpi_wake_cnvi_wifi() -> i32;
}

pub fn power_on_wifi_via_acpi() -> bool {
    crate::serial_println!("[ACPI] Initiating Motherboard 'Wake Everything' sequence...");
    let count = unsafe { acpi_wake_cnvi_wifi() };
    crate::serial_println!("[ACPI] Blasted _PS0 (Power On) to {} hidden hardware nodes!", count);
    true
}