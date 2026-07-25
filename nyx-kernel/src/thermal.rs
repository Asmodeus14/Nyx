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
    let wake_ms = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) + ms; 
    
    unsafe {
        x86_64::instructions::interrupts::enable();
        loop {
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            {
                let task = &mut percpu.scheduler.tasks[curr_idx];
                task.state = crate::scheduler::TaskState::Blocked;
                task.wake_tsc = wake_ms; 
            }
            
            // Yield the CPU
            core::arch::asm!("int 0x41"); 
            
            // Did the time actually pass? If yes, break!
            if crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) >= wake_ms { break; } 
            
            // If we woke up illegally (scheduler fallback), HALT to save battery!
            x86_64::instructions::hlt(); 
        }
    }
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

/// One task's cumulative CPU-tick count, sampled from the scheduler.
#[derive(Clone, Copy)]
struct CpuSample { pid: u64, ticks: u64, name: [u8; 16], idle: bool }

const CPU_TRACK: usize = 48;

/// Snapshot every task's cumulative tick count. Fixed array, no allocation — this runs in the
/// governor, which has to stay dependable exactly when the machine is already in trouble.
fn sample_tasks(out: &mut [CpuSample; CPU_TRACK]) -> usize {
    let mut n = 0;
    unsafe {
        if let Some(cores) = &crate::percpu::PER_CPU {
            for core in cores.iter() {
                for t in core.scheduler.tasks.iter() {
                    if n >= CPU_TRACK { return n; }
                    out[n] = CpuSample { pid: t.pid, ticks: t.cpu_ticks, name: t.name, idle: t.is_idle };
                    n += 1;
                }
            }
        }
    }
    n
}

/// Log who actually burned the CPU over the last governor cycle.
///
/// "Something is pegging the CPU and cooking the laptop" is not a debuggable statement, and the trip
/// below halts the machine before anyone can look. The scheduler already counts one tick per task per
/// timer interrupt, so the delta across one cycle IS that task's CPU share for the second.
///
/// The IDLE task is reported first and deliberately: it is the single most informative number here.
/// A desktop sitting untouched should be almost entirely idle; if idle is near zero while nothing is
/// on screen, the kernel's own poll path is the suspect rather than any application.
fn report_cpu_share(prev: &[CpuSample; CPU_TRACK], prev_n: usize,
                    cur: &[CpuSample; CPU_TRACK], cur_n: usize, temp: u8) {
    let mut deltas = [(0u64, 0u64, [0u8; 16], false); CPU_TRACK];
    let mut n = 0;
    let mut total = 0u64;
    for c in cur.iter().take(cur_n) {
        let was = prev.iter().take(prev_n).find(|p| p.pid == c.pid).map(|p| p.ticks).unwrap_or(0);
        let d = c.ticks.saturating_sub(was);
        total += d;
        if d > 0 {
            deltas[n] = (d, c.pid, c.name, c.idle);
            n += 1;
        }
    }
    // Selection sort by descending delta — n is tiny and this avoids pulling in a sort.
    for i in 0..n {
        let mut best = i;
        for j in i + 1..n { if deltas[j].0 > deltas[best].0 { best = j; } }
        deltas.swap(i, best);
    }

    // ★ SUM every idle task, don't take the first one. This box has FOUR idle tasks — one per core
    // (pid 2 plus the 0xFFFF_0000+ SMP ones) — so counting only the first reported 87% busy on a
    // machine that was actually ~2.5% busy, and sent me hunting a CPU hog that did not exist.
    let idle: u64 = deltas.iter().take(n).filter(|d| d.3).map(|d| d.0).sum();
    let busy = total.saturating_sub(idle);
    let pct = if total == 0 { 0 } else { busy * 100 / total };
    crate::serial_println!("[Thermal] {}C | cpu {}% busy ({} of {} ticks), idle {}",
        temp, pct, busy, total, idle);
    for d in deltas.iter().take(n).take(4) {
        if d.3 { continue; }
        let nm = core::str::from_utf8(&d.2).unwrap_or("?");
        let nm = nm.trim_end_matches('\0');
        if nm.is_empty() {
            crate::serial_println!("[Thermal]   pid {} — {} ticks", d.1, d.0);
        } else {
            crate::serial_println!("[Thermal]   {} (pid {}) — {} ticks", nm, d.1, d.0);
        }
    }
}

pub extern "C" fn nyx_task_manager_daemon() {
    crate::serial_println!("[Thermal] NyxOS Mobile Thermal Governor Online.");
    identify_silicon();
    let mut is_throttled = false;
    let blank = CpuSample { pid: 0, ticks: 0, name: [0; 16], idle: false };
    let mut prev = [blank; CPU_TRACK];
    let mut prev_n = 0usize;
    let mut cycle = 0u32;
    kernel_sleep_ms(1000);

    loop {
        let temp = get_intel_silicon_temp();

        // Park the GPU once it has been idle a while. This lives here because it needs a periodic
        // tick that keeps running when the compositor has stopped issuing GPU work entirely — which
        // is precisely the situation that overheats the machine. try_lock, never lock: this task is
        // preemptible (IF=1) and INTEL_GPU is taken from syscalls with IF=0, so blocking here would
        // be the preemption-boundary deadlock documented in drivers/net/mod.rs. Missing a park
        // attempt costs nothing — the next tick is a second away.
        unsafe {
            if let Some(mut g) = crate::drivers::gpu::intel::INTEL_GPU.try_lock() {
                if let Some(gpu) = g.as_mut() { gpu.maybe_park_gpu(); }
            }
        }

        // CPU accounting every cycle; PRINTED once a minute when cool, every cycle once we're warm
        // enough to care. Silence while healthy keeps the boot log readable, but the moment the
        // machine starts heating we get a per-second record of who is responsible — which survives
        // the trip below, unlike anything a userspace monitor could show.
        let mut cur = [blank; CPU_TRACK];
        let cur_n = sample_tasks(&mut cur);
        cycle = cycle.wrapping_add(1);
        if temp >= 70 || cycle % 60 == 0 {
            report_cpu_share(&prev, prev_n, &cur, cur_n, temp);
        }
        prev = cur;
        prev_n = cur_n;

        if temp >= 95 {
            crate::serial_println!("[Thermal] CRITICAL THERMAL TRIP at {}C — attempting power off.", temp);
            crate::acpi::poweroff();
            // ★ If we get here, acpi::poweroff() did NOT power the machine down (it needs PM1a/PM1b
            // + SLP_TYP from the DSDT and returns silently when it can't). The loop below is an
            // interrupts-off halt, which IS the safe thing to do at 95C — but it is indistinguishable
            // from a crash unless we say so first. This line is the difference between "the box froze
            // and nothing was in the log" and a diagnosis.
            crate::serial_println!(
                "[Thermal] poweroff returned — firmware did not shut down. Parking the CPU in cli;hlt \
                 to stop generating heat. THIS IS AN INTENTIONAL EMERGENCY HALT, NOT A HANG.");
            loop { unsafe { core::arch::asm!("cli; hlt"); } }
        }

        // --------------------------------------------------------
        // THE FIX: Always assert maximum cooling while hot to 
        // fight the Embedded Controller / BIOS resets!
        // --------------------------------------------------------
        if temp >= 76 { 
            crate::laptop_fans::engage_maximum_cooling();
            
            // Only toggle the MSR hardware throttler once
            if !is_throttled {
                unsafe { force_hardware_cooling(true); } 
                is_throttled = true;
                crate::serial_println!("[Thermal] WARNING: CPU hit {}°C! Throttling active.", temp);
            }
        } else if temp <= 60 && is_throttled {
            unsafe { force_hardware_cooling(false); } 
            is_throttled = false;
            crate::serial_println!("[Thermal] CPU cooled to {}°C. Restoring baseline.", temp);
        }
        
        kernel_sleep_ms(1000); 
    }
}