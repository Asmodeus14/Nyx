use x86_64::instructions::port::Port;

// Standard Embedded Controller I/O Ports
const EC_STAT: u16 = 0x66;
const EC_CMD: u16 = 0x66;
const EC_DATA: u16 = 0x62;

// ==========================================
// 1. EC (EMBEDDED CONTROLLER) RAW I/O ENGINE
// ==========================================

unsafe fn wait_ec(write: bool) -> bool {
    let mut stat = Port::<u8>::new(EC_STAT);
    for _ in 0..100000 {
        let val = stat.read();
        if write {
            if (val & 2) == 0 { return true; } // Wait for Input Buffer Empty
        } else {
            if (val & 1) != 0 { return true; } // Wait for Output Buffer Full
        }
    }
    false
}

unsafe fn ec_write(addr: u8, data: u8) -> bool {
    let mut cmd = Port::<u8>::new(EC_CMD);
    let mut dat = Port::<u8>::new(EC_DATA);

    if !wait_ec(true) { return false; }
    cmd.write(0x81); // 0x81 = standard Write EC RAM Command

    if !wait_ec(true) { return false; }
    dat.write(addr);

    if !wait_ec(true) { return false; }
    dat.write(data);

    true
}

// ==========================================
// 2. ASUS (ROG / TUF / ZENBOOK)
// ==========================================
// ASUS maps their fan modes directly into EC memory. 
// Mode 0 = Balanced, Mode 1 = Overboost/Max, Mode 2 = Silent
pub unsafe fn trigger_asus_fans() -> bool {
    crate::serial_println!("[EC] Firing ASUS ROG/TUF Overboost Payload...");
    // 0x97 is modern ROG, 0x5E is older TUF/Zenbook
    let p1 = ec_write(0x97, 1); 
    let p2 = ec_write(0x5E, 1); 
    p1 || p2
}

// ==========================================
// 3. HP (OMEN / PAVILION GAMING)
// ==========================================
// HP Omen uses offset 0x85 or 0x86. 0x01 = Max Fan, 0x00 = Auto.
pub unsafe fn trigger_hp_fans() -> bool {
    crate::serial_println!("[EC] Firing HP Omen Max Fan Payload...");
    let p1 = ec_write(0x85, 0x01);
    let p2 = ec_write(0x86, 0x01);
    p1 || p2
}

// ==========================================
// 4. ACER (PREDATOR / NITRO) & CLEVO
// ==========================================
// Acer bypasses "modes" and allows raw PWM control from 0x00 (0%) to 0xFF (100%).
pub unsafe fn trigger_acer_fans() -> bool {
    crate::serial_println!("[EC] Firing Acer Predator 100% PWM Payload...");
    let cpu = ec_write(0x55, 0xFF); // CPU Fan 100%
    let gpu = ec_write(0x56, 0xFF); // GPU Fan 100%
    cpu || gpu
}

// ==========================================
// 5. MSI (STEALTH / RAIDER / TITAN)
// ==========================================
// MSI has a famous hardware toggle called "Cooler Boost".
// On many ECs, writing 0x80 to offset 0xF4 or 0x98 triggers it.
pub unsafe fn trigger_msi_fans() -> bool {
    crate::serial_println!("[EC] Firing MSI Cooler Boost Payload...");
    let p1 = ec_write(0xF4, 0x80);
    let p2 = ec_write(0x98, 0x80);
    p1 || p2
}

// ==========================================
// 6. LENOVO (LEGION / IDEAPAD GAMING)
// ==========================================
// Lenovo ECs map "Performance Mode" (Fn+Q Red LED) to 0xBB or 0x3F.
// Value 0x02 or 0x3F forces max fans.
pub unsafe fn trigger_lenovo_fans() -> bool {
    crate::serial_println!("[EC] Firing Lenovo Legion Performance Mode...");
    let p1 = ec_write(0xBB, 0x02);
    let p2 = ec_write(0x3F, 0x3F);
    p1 || p2
}

// ==========================================
// 7. DELL (G-SERIES / ALIENWARE / XPS)
// ==========================================
// Dell completely ignores standard EC RAM writes. They require a native 
// Ring-0 System Management Interrupt (SMI) handshake on Port 0xB2.

pub unsafe fn get_dell_fan_rpm(fan_idx: u32) -> u32 {
    let mut rpm: u32;
    core::arch::asm!(
        "push rbx", "mov ebx, {idx:e}", "out dx, al", "pop rbx",
        idx = in(reg) fan_idx,             
        inout("eax") 0x02A3u32 => rpm,     
        in("dx") 0xB2u16,
    );
    if rpm == 0x02A3 || rpm == 0x11A3 { 0 } else { rpm }
}

unsafe fn set_dell_fan_speed(fan_idx: u32, speed: u32) {
    let ebx_val = (speed << 8) | (fan_idx & 0xFF);
    core::arch::asm!(
        "push rbx", "mov ebx, {val:e}", "out dx, al", "pop rbx",
        val = in(reg) ebx_val,
        in("eax") 0x01A3u32,
        in("dx") 0xB2u16,
    );
}

unsafe fn disable_dell_bios_governor() {
    core::arch::asm!(
        "push rbx", "mov ebx, {val:e}", "out dx, al", "pop rbx",
        val = in(reg) 0x01u32,
        in("eax") 0x34A3u32,
        in("dx") 0xB2u16,
    );
}

pub unsafe fn trigger_dell_fans() {
    crate::serial_println!("[SMM] Firing Dell SMM Level 3 Maximum Overdrive...");
    disable_dell_bios_governor();
    set_dell_fan_speed(0, 3); // CPU Max
    set_dell_fan_speed(1, 3); // GPU Max
    
    let cpu_rpm = get_dell_fan_rpm(0);
    let gpu_rpm = get_dell_fan_rpm(1);
    crate::serial_println!("[SMM] Hardware Report -> CPU: {} RPM | GPU: {} RPM", cpu_rpm, gpu_rpm);
}

// ==========================================
// 8. THE UNIVERSAL NYXOS FAN OVERRIDE
// ==========================================
// This sweeps through the EC protocols for every major manufacturer. 
// If the EC ignores it, no harm is done. If the payload matches the EC, fans spool up.
pub fn engage_maximum_cooling() {
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