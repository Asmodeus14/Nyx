use crate::acpi::set_active_cooling;
use x86_64::instructions::port::Port;
use crate::laptop_fans::engage_maximum_cooling;

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
    }
}

unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32; let high: u32;
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
                return tj_max.saturating_sub(((msr_1b1 >> 16) & 0x7F) as u8);
            }
        } else if core_supported {
            let msr_19c = read_msr(0x19C);
            if (msr_19c & (1 << 31)) != 0 {
                return tj_max.saturating_sub(((msr_19c >> 16) & 0x7F) as u8);
            }
        }
    }
    50
}

const SMB_HST_STS: u16 = 0x00; const SMB_HST_CNT: u16 = 0x02;
const SMB_HST_CMD: u16 = 0x03; const SMB_XMIT_SLVA: u16 = 0x04;
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
    let candidates = [0x2C, 0x2D, 0x2E, 0x4C];
    for &addr in candidates.iter() {
        unsafe {
            if let Some(mfg_id) = smbus_read_byte(smbus_base, addr, 0xFE) {
                crate::serial_println!("[SMBus] Chip at {:#04x} responded! Mfg ID: {:#04x}", addr, mfg_id);
            }
        }
    }
}

fn kernel_sleep_ms(ms: u64) {
    let mut lo: u32; let mut hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
    let start_tsc = ((hi as u64) << 32) | (lo as u64);
    let target_tsc = start_tsc + (ms * 2_000_000); 
    
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    task.state = crate::scheduler::TaskState::Blocked;
    task.wake_tsc = target_tsc;
    unsafe { core::arch::asm!("int 0x40"); } 
}

unsafe fn force_hardware_cooling(throttle: bool) {
    let cpuid_6 = core::arch::x86_64::__cpuid(6);
    let hwp_supported = (cpuid_6.eax & (1 << 7)) != 0;

    if hwp_supported {
        // FIX: Hardware requires IA32_PM_ENABLE to be active BEFORE writing to IA32_HWP_REQUEST
        let mut pm_low: u32; let mut pm_high: u32;
        core::arch::asm!("rdmsr", in("ecx") 0x770u32, out("eax") pm_low, out("edx") pm_high, options(nomem, nostack));
        if (pm_low & 1) == 0 {
            core::arch::asm!("wrmsr", in("ecx") 0x770u32, in("eax") pm_low | 1, in("edx") pm_high, options(nomem, nostack));
        }

        if throttle {
            let hwp_request_low: u32 = 0xFF000808; 
            core::arch::asm!("wrmsr", in("ecx") 0x774u32, in("eax") hwp_request_low, in("edx") 0, options(nomem, nostack));
        } else {
            let hwp_request_low: u32 = 0x8000FF08;
            core::arch::asm!("wrmsr", in("ecx") 0x774u32, in("eax") hwp_request_low, in("edx") 0, options(nomem, nostack));
        }
    } else {
        crate::serial_println!("[Thermal] HWP not supported on this CPU, skipping MSR throttle.");
    }
}

pub extern "C" fn nyx_task_manager_daemon() {
    crate::serial_println!("[Thermal] NyxOS Mobile Thermal Governor Online.");
    identify_silicon();
    let mut is_throttled = false;
    kernel_sleep_ms(1000);

    loop {
        let temp = get_intel_silicon_temp();
        if temp >= 95 { // Intel TjMax is usually 100C
            crate::serial_println!("[Thermal] CRITICAL THERMAL TRIP! Halting system to prevent hardware damage!");
            
            // Fire an ACPI poweroff or an immediate hardware halt here
            crate::acpi::poweroff(); 
            loop { unsafe { core::arch::asm!("cli; hlt"); } }
        }
        

        if temp >= 76 && !is_throttled { // Threshold adjusted as per your test
            unsafe { force_hardware_cooling(true); } 
            crate::laptop_fans::engage_maximum_cooling();
            is_throttled = true;
            crate::serial_println!("[Thermal] WARNING: CPU hit {}°C! Throttling active.", temp);
            
        } else if temp <= 60 && is_throttled {
            unsafe { force_hardware_cooling(false); } 
            is_throttled = false;
            crate::serial_println!("[Thermal] CPU cooled to {}°C. Restoring baseline.", temp);
        }
        kernel_sleep_ms(1000); 

    }
}