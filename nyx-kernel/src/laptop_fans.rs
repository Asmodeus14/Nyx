use x86_64::instructions::port::Port;

const EC_STAT: u16 = 0x66;
const EC_CMD: u16 = 0x66;
const EC_DATA: u16 = 0x62;

unsafe fn wait_ec(write: bool) -> bool {
    let mut stat = Port::<u8>::new(EC_STAT);
    for _ in 0..100000 {
        let val = stat.read();
        if write {
            if (val & 2) == 0 { return true; } 
        } else {
            if (val & 1) != 0 { return true; } 
        }
    }
    false
}

unsafe fn ec_write(addr: u8, data: u8) -> bool {
    let mut cmd = Port::<u8>::new(EC_CMD);
    let mut dat = Port::<u8>::new(EC_DATA);

    if !wait_ec(true) { return false; }
    cmd.write(0x81); 

    if !wait_ec(true) { return false; }
    dat.write(addr);

    if !wait_ec(true) { return false; }
    dat.write(data);

    true
}

pub unsafe fn trigger_asus_fans() -> bool {
    let p1 = ec_write(0x97, 1); 
    let p2 = ec_write(0x5E, 1); 
    p1 || p2
}

pub unsafe fn trigger_hp_fans() -> bool {
    let p1 = ec_write(0x85, 0x01);
    let p2 = ec_write(0x86, 0x01);
    p1 || p2
}

pub unsafe fn trigger_acer_fans() -> bool {
    let cpu = ec_write(0x55, 0xFF); 
    let gpu = ec_write(0x56, 0xFF); 
    cpu || gpu
}

pub unsafe fn trigger_msi_fans() -> bool {
    let p1 = ec_write(0xF4, 0x80);
    let p2 = ec_write(0x98, 0x80);
    p1 || p2
}

pub unsafe fn trigger_lenovo_fans() -> bool {
    let p1 = ec_write(0xBB, 0x02);
    let p2 = ec_write(0x3F, 0x3F);
    p1 || p2
}

// FIX: Pushing and popping RBX is mathematically safe and protects the stack
// AS LONG AS we omit 'options(nostack)'. 

/// Sentinel for "we cannot read this fan", as opposed to "this fan is stopped". Callers must render
/// it as unknown/na, never as a speed.
pub const FAN_RPM_UNKNOWN: u32 = u32::MAX;

/// Fan tachometer, or `FAN_RPM_UNKNOWN` when we have no supported way to read it.
///
/// Gated on SMBIOS, not on a constant. The only tachometer protocol implemented here is Dell's SMI,
/// and firing an undocumented SMI at port 0xB2 on a machine that isn't a Dell — once per System
/// Monitor refresh — is not something to do speculatively. Now that `smbios` can actually ask the
/// firmware who we are, the answer decides.
///
/// Note this reads only. The fan *writes* stay behind `VENDOR_FAN_OVERRIDE` below even on a
/// confirmed Dell: knowing the vendor makes the tachometer safe to read, it does not make taking the
/// fan governor away from the firmware a good idea.
pub fn fan_rpm(fan_idx: u32) -> u32 {
    if crate::smbios::vendor() != crate::smbios::Vendor::Dell {
        return FAN_RPM_UNKNOWN;
    }
    unsafe { get_dell_fan_rpm(fan_idx) }
}

pub unsafe fn get_dell_fan_rpm(fan_idx: u32) -> u32 {
    if crate::smbios::vendor() != crate::smbios::Vendor::Dell {
        return FAN_RPM_UNKNOWN;
    }
    let mut rpm: u32;
    core::arch::asm!(
        "push rbx",
        "mov ebx, {idx:e}",
        "out dx, al",
        "pop rbx",
        idx = in(reg) fan_idx,
        inout("eax") 0x02A3u32 => rpm,     
        in("dx") 0xB2u16
        // 'nostack' removed! Compiler will safely track the stack frame.
    );
    // The SMI echoing the command back, or the "not supported" code, means the call didn't happen —
    // that is "unknown", not "the fan is stopped". Same trap as returning 0 for an unread tachometer.
    if rpm == 0x02A3 || rpm == 0x11A3 { FAN_RPM_UNKNOWN } else { rpm }
}

unsafe fn set_dell_fan_speed(fan_idx: u32, speed: u32) {
    let ebx_val = (speed << 8) | (fan_idx & 0xFF);
    core::arch::asm!(
        "push rbx",
        "mov ebx, {val:e}",
        "out dx, al",
        "pop rbx",
        val = in(reg) ebx_val,
        in("eax") 0x01A3u32,
        in("dx") 0xB2u16
    );
}

unsafe fn disable_dell_bios_governor() {
    core::arch::asm!(
        "push rbx",
        "mov ebx, {val:e}",
        "out dx, al",
        "pop rbx",
        val = in(reg) 0x01u32,
        in("eax") 0x34A3u32,
        in("dx") 0xB2u16
    );
}

pub unsafe fn trigger_dell_fans() {
    crate::serial_println!("[SMM] Firing Dell SMM Level 3 Maximum Overdrive...");
    disable_dell_bios_governor();
    set_dell_fan_speed(0, 3); 
    set_dell_fan_speed(1, 3); 
    
    let cpu_rpm = get_dell_fan_rpm(0);
    let gpu_rpm = get_dell_fan_rpm(1);
    crate::serial_println!("[SMM] Hardware Report -> CPU: {} RPM | GPU: {} RPM", cpu_rpm, gpu_rpm);
}

/// ★★★ Vendor fan overrides are OFF. This is the fix for "the laptop cooks itself while idle".
///
/// What the code above actually does: `ec_write` talks to ports 0x62/0x66, the **standard ACPI
/// embedded controller** on essentially every x86 laptop. So "universal cooling" wrote the ASUS *and*
/// HP *and* Acer *and* MSI *and* Lenovo register values into the real EC of a machine whose vendor we
/// never identified. EC register 0x55 is a fan target on one vendor and a battery-charge threshold on
/// another. There is no safe "just try them all".
///
/// The specific way it burned us: `trigger_dell_fans()` calls `disable_dell_bios_governor()`, which
/// switches the FIRMWARE's automatic fan control OFF, then sets a speed manually. When that manual
/// set doesn't take (wrong vendor, wrong protocol) we have disabled the thermostat and put nothing in
/// its place. Observed: 0 RPM on both fans and 95 °C at ~2.5 % CPU load. And because the governor only
/// poked the fans while `temp >= 76` and never restored automatic control on the way down, the fans
/// also never came back when the temperature climbed again.
///
/// The firmware's own fan control works correctly on this hardware — the same laptop runs cool under
/// Fedora. So leaving the EC alone is not a compromise, it is strictly better than anything we can do
/// blind. Heat is still handled, by the HWP throttle in `thermal.rs`, which is architectural Intel.
///
/// `crate::smbios` now identifies the machine, so this no longer has to be decided blind — but it
/// still gates WRITES only. Knowing the vendor is enough to make the tachometer safe to READ (see
/// `fan_rpm`); it is not enough to justify taking fan control away from firmware that is doing the
/// job correctly. To re-enable on a machine whose EC map you have actually confirmed: set this true
/// AND cut the dispatch below down to that one vendor, keyed off `smbios::vendor()`. Never restore
/// the fire-everything version.
const VENDOR_FAN_OVERRIDE: bool = false;

pub fn engage_maximum_cooling() {
    if !VENDOR_FAN_OVERRIDE {
        // Deliberately silent: this is called once per second while hot, and the old banner spam
        // buried the temperature readings that actually matter.
        return;
    }
    crate::serial_println!("\n=== [NyxOS] EXECUTING UNIVERSAL COOLING OVERRIDES ===");
    unsafe {
        trigger_asus_fans();
        trigger_hp_fans();
        trigger_acer_fans();
        trigger_msi_fans();
        trigger_lenovo_fans();
        trigger_dell_fans();
    }
    crate::serial_println!("=====================================================\n");
}