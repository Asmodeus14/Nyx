use crate::acpi::set_active_cooling;
use x86_64::instructions::port::Port;
use crate::laptop_fans::engage_maximum_cooling;

// ==========================================
// 1. HARDWARE IDENTIFICATION
// ==========================================
pub fn identify_silicon() {
    let cpuid = unsafe { core::arch::x86_64::__cpuid(0) };
    
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&cpuid.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&cpuid.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&cpuid.ecx.to_le_bytes());
    
    if let Ok(s) = core::str::from_utf8(&vendor) {
        crate::serial_println!("\n==================================");
        crate::serial_println!("DETECTED BARE-METAL CPU: {}", s);
        crate::serial_println!("==================================\n");
    } else {
        crate::serial_println!("\n[Thermal] Failed to parse CPU Vendor String.");
    }
}

// ==========================================
// 2. HARDWARE SILICON THERMOMETER (MSR)
// ==========================================
unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack));
    ((high as u64) << 32) | (low as u64)
}

pub fn get_intel_silicon_temp() -> u8 {
    let cpuid_0 = unsafe { core::arch::x86_64::__cpuid(0) };
    let is_intel = cpuid_0.ebx == 0x756e6547 && cpuid_0.edx == 0x49656e69 && cpuid_0.ecx == 0x6c65746e;
    if !is_intel { return 50; }

    let cpuid_6 = unsafe { core::arch::x86_64::__cpuid(6) };
    let package_supported = (cpuid_6.eax & (1 << 6)) != 0;
    let core_supported = (cpuid_6.eax & 1) != 0;

    let tj_max: u8 = 100; 

    unsafe {
        if package_supported {
            let msr_1b1 = read_msr(0x1B1); 
            if (msr_1b1 & (1 << 31)) != 0 { 
                let delta = ((msr_1b1 >> 16) & 0x7F) as u8;
                return tj_max.saturating_sub(delta);
            }
        } else if core_supported {
            let msr_19c = read_msr(0x19C);
            if (msr_19c & (1 << 31)) != 0 {
                let delta = ((msr_19c >> 16) & 0x7F) as u8;
                return tj_max.saturating_sub(delta);
            }
        }
    }
    50
}

// ==========================================
// 3. SMBUS I2C FAN SCANNER
// ==========================================
const SMB_HST_STS: u16 = 0x00;
const SMB_HST_CNT: u16 = 0x02;
const SMB_HST_CMD: u16 = 0x03; 
const SMB_XMIT_SLVA: u16 = 0x04;
const SMB_HST_DAT0: u16 = 0x05; 

unsafe fn smbus_read_byte(smbus_base: u16, i2c_addr: u8, reg: u8) -> Option<u8> {
    let mut sts_port = Port::<u8>::new(smbus_base + SMB_HST_STS);
    let mut cnt_port = Port::<u8>::new(smbus_base + SMB_HST_CNT);
    let mut cmd_port = Port::<u8>::new(smbus_base + SMB_HST_CMD);
    let mut slva_port = Port::<u8>::new(smbus_base + SMB_XMIT_SLVA);
    let mut dat0_port = Port::<u8>::new(smbus_base + SMB_HST_DAT0);

    sts_port.write(0xFF);
    slva_port.write((i2c_addr << 1) | 1);
    cmd_port.write(reg);
    cnt_port.write(0x48);

    let mut status = 0;
    for _ in 0..100000 {
        status = sts_port.read();
        if (status & 0x02) != 0 || (status & 0x04) != 0 || (status & 0x08) != 0 { break; }
    }

    if (status & 0x02) != 0 {
        let result = dat0_port.read();
        sts_port.write(0xFF); 
        return Some(result);
    }
    
    sts_port.write(0xFF); 
    None
}
pub fn scan_smbus(smbus_base: u16) {
    crate::serial_println!("\n=== INTERROGATING FAN CONTROLLERS ON SMBUS ===");
    crate::serial_println!("SMBus Base: {:#06x}", smbus_base);

    // Standard Super I/O Hardware Monitor I2C Addresses
    let candidates = [0x2C, 0x2D, 0x2E, 0x4C];
    
    for &addr in candidates.iter() {
        unsafe {
            // Register 0xFE is the standard "Manufacturer ID"
            if let Some(mfg_id) = smbus_read_byte(smbus_base, addr, 0xFE) {
                crate::serial_println!("[SMBus] Chip at {:#04x} responded! Mfg ID (Reg 0xFE): {:#04x}", addr, mfg_id);
                
                // Let's also read Register 0x00 to see if it holds a temperature/voltage reading
                if let Some(reg_00) = smbus_read_byte(smbus_base, addr, 0x00) {
                    crate::serial_println!("        -> Value at Reg 0x00: {:#04x}", reg_00);
                }
            } else {
                crate::serial_println!("[SMBus] No valid hardware monitor at {:#04x}", addr);
            }
        }
    }
    crate::serial_println!("==============================================\n");
}

// ==========================================
// 4. NATIVE RING-0 SLEEP HELPER (FIXED)
// ==========================================
fn kernel_sleep_ms(ms: u64) {
    let mut lo: u32; let mut hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
    let start_tsc = ((hi as u64) << 32) | (lo as u64);
    
    // Calculate the future wake timestamp
    let target_tsc = start_tsc + (ms * 2_000_000); 
    
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    // CRITICAL: Actually suspend the kernel thread so the Scheduler picks the Idle Task!
    task.state = crate::scheduler::TaskState::Blocked;
    task.wake_tsc = target_tsc;
    
    // Force immediate context switch
    unsafe { core::arch::asm!("int 0x40"); } 
}

// ==========================================
// 5. INTEL SPEED SHIFT (HWP) DOOMSDAY GOVERNOR
// ==========================================
unsafe fn force_hardware_cooling(throttle: bool) {
    let mut low: u32;
    let mut high: u32;

    // 1. DISABLE LEGACY TURBO BOOST (MSR 0x1A0)
    core::arch::asm!("rdmsr", in("ecx") 0x1A0u32, out("eax") low, out("edx") high, options(nomem, nostack));
    if throttle { high |= 1 << 6; } else { high &= !(1 << 6); }
    core::arch::asm!("wrmsr", in("ecx") 0x1A0u32, in("eax") low, in("edx") high, options(nomem, nostack));

    // 2. INTEL SPEED SHIFT (HWP) OVERRIDE (MSR 0x774)
    if throttle {
        // PACKED VALUE: 0xFF000808 (EPP = Max Power Saving, Min/Max = 800MHz)
        let hwp_request_low: u32 = 0xFF000808; 
        core::arch::asm!("wrmsr", in("ecx") 0x774u32, in("eax") hwp_request_low, in("edx") 0, options(nomem, nostack));
    } else {
        // RESTORE VALUE: 0x8000FF08 (EPP = Balanced, Max = Hardware Max)
        let hwp_request_low: u32 = 0x8000FF08;
        core::arch::asm!("wrmsr", in("ecx") 0x774u32, in("eax") hwp_request_low, in("edx") 0, options(nomem, nostack));
    }
}

// ==========================================
// 6. THE NYXOS TASK MANAGER & COOLING DAEMON
// ==========================================
pub extern "C" fn nyx_task_manager_daemon() {
    crate::serial_println!("[Thermal] NyxOS Mobile Thermal Governor Online.");
    
    identify_silicon();
    let mut is_throttled = false;

    kernel_sleep_ms(1000);

    loop {
        let temp = get_intel_silicon_temp();

        if temp >= 80 && !is_throttled {
            // 1. Drop HWP Voltage immediately
            unsafe { force_hardware_cooling(true); } 
            
            // 2. Blast the SMM Overrides
            crate::laptop_fans::engage_maximum_cooling();
            
            is_throttled = true;
            crate::serial_println!("[Thermal] WARNING: CPU hit {}°C! HWP Throttling active and waking EC.", temp);
            
        } else if temp <= 60 && is_throttled {
            unsafe { force_hardware_cooling(false); } 
            
            is_throttled = false;
            crate::serial_println!("[Thermal] CPU cooled to {}°C. Restoring baseline HWP State.", temp);
        }

        kernel_sleep_ms(1000); 
    }
}