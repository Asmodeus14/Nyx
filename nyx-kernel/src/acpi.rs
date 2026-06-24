#![allow(warnings)] // <--- THE FIX: Silence all warnings for this module and its included files!

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
        
        let get_status = AcpiGetTable(
            b"APIC\0".as_ptr() as *mut i8, 
            1, 
            &mut madt_header as *mut _ as *mut *mut _
        );
        
        if get_status == 0 && !madt_header.is_null() {
            let madt_virt = madt_header as u64;
            let madt_phys = crate::memory::virt_to_phys(madt_virt).unwrap_or(madt_virt);
            
            ACPI_INFO.madt_addr = Some(madt_phys);
            crate::serial_println!("[ACPI] MADT stored dynamically @ {:#x}", madt_phys);
        } else {
            crate::serial_println!("[ACPI] WARNING: MADT not found! Status: {}", get_status);
        }
        let mut mcfg_header: *mut core::ffi::c_void = core::ptr::null_mut();
        let mcfg_status = AcpiGetTable(
            b"MCFG\0".as_ptr() as *mut i8, 
            1, 
            &mut mcfg_header as *mut _ as *mut *mut _
        );

        if mcfg_status == 0 && !mcfg_header.is_null() {
            let mcfg_virt = mcfg_header as u64;
            let mcfg_phys = crate::memory::virt_to_phys(mcfg_virt).unwrap_or(mcfg_virt);
            
            ACPI_INFO.mcfg_addr = Some(mcfg_phys);
            crate::serial_println!("[ACPI] MCFG stored dynamically @ {:#x}", mcfg_phys);
        } else {
            crate::serial_println!("[ACPI] WARNING: MCFG not found! PCIe will fallback to Legacy Port I/O.");
        }
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
    fn acpi_find_i2c_hid() -> i32; 
    
    // --- NEW: THE ACPICA FAN CONTROLLER ---
    fn acpi_set_fan_state(turn_on: i32) -> i32;
    fn acpi_get_system_temp() -> i32;
}

pub fn get_acpi_temperature() -> u8 {
    unsafe { acpi_get_system_temp() as u8 }
}

pub fn power_on_wifi_via_acpi() -> bool {
    crate::serial_println!("[ACPI] Initiating Motherboard 'Wake Everything' sequence...");
    let count = unsafe { acpi_wake_cnvi_wifi() };
    crate::serial_println!("[ACPI] Blasted _PS0 (Power On) to {} hidden hardware nodes!", count);
    true
}

pub fn scan_for_modern_inputs() {
    crate::serial_println!("[ACPI] Scanning motherboard for I2C-HID devices (PNP0C50)...");
    crate::vga_println!("[ACPI] Scanning for I2C Trackpads...");
    
    let count = unsafe { acpi_find_i2c_hid() };
    
    if count > 0 {
        crate::serial_println!("[ACPI] SUCCESS: Found {} I2C-HID device(s)!", count);
        crate::vga_println!("[ACPI] Found {} I2C-HID device(s)!", count);
    } else {
        crate::serial_println!("[ACPI] No I2C-HID devices found. It might be USB-based.");
        crate::vga_println!("[ACPI] No I2C-HID found.");
    }
}

pub fn set_active_cooling(enable: bool) {
    let state = if enable { 1 } else { 0 };
    unsafe {
        acpi_set_fan_state(state);
    }
}

pub fn get_dsdt_data(buf_ptr: *mut u8, max_len: usize) -> usize {
    unsafe {
        let mut dsdt_header: *mut core::ffi::c_void = core::ptr::null_mut();
        
        // Ask Intel ACPICA to find the DSDT table for us
        let status = AcpiGetTable(
            b"DSDT\0".as_ptr() as *mut i8, 
            1, 
            &mut dsdt_header as *mut _ as *mut *mut _
        );
        
        if status == 0 && !dsdt_header.is_null() {
            let dsdt_virt = dsdt_header as u64;
            // Read the length of the table (stored at offset 4 of the header)
            let length = core::ptr::read_volatile((dsdt_virt + 4) as *const u32) as usize;
            
            let copy_len = core::cmp::min(length, max_len);
            core::ptr::copy_nonoverlapping(dsdt_virt as *const u8, buf_ptr, copy_len);
            return copy_len;
        }
        0
    }
}
// ==========================================
// 5. POWER MANAGEMENT (SLEEP / OFF)
// ==========================================
pub fn poweroff() {
    crate::serial_println!("\n[ACPI] Initiating Emergency Hardware Poweroff (S5)...");
    unsafe {
        let s5_state: u8 = 5; 
        AcpiEnterSleepStatePrep(s5_state);
        core::arch::asm!("cli", options(nomem, nostack));
        AcpiEnterSleepState(s5_state);
        loop { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}