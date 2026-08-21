// src/postmortem.rs
//
// A breadcrumb that survives the power cycle.
//
// WHY THIS EXISTS. This machine has no serial console. `nyx-recv` is a UDP *framebuffer* viewer,
// not a log capture, and the laptop has no COM port — so every log line anyone has ever read on
// Nyx came out of `serial::BOOT_LOG`, a 128 KB RAM buffer that userspace reads through System
// Monitor. That works fine right up until the moment it matters: when the machine hard-freezes,
// nobody can open System Monitor, and the power cycle needed to recover wipes the buffer. The
// result is a class of bug that reports itself as "froze with zero output" no matter how much
// the kernel logged on the way down, which is precisely how the HelloC freeze burned six boots
// eliminating suspects on evidence that could never have existed.
//
// CMOS is the fix: 128 bytes of battery-backed NVRAM on the same MC146818 the RTC already uses,
// unaffected by reset or power loss. Writing one byte per stage costs two `out` instructions and
// no allocation, no lock and no formatting — so it is safe from any context, including a panic
// handler with a poisoned heap. On the next boot the kernel reads the record back, prints it into
// BOOT_LOG where System Monitor already shows it, and clears it.
//
// REGISTER CHOICE / RISK. We use 0x50..0x5A. The BIOS's own checksummed configuration block is
// the standard 0x10..0x2D range, so a byte written up here should not invalidate it; the RTC's
// own time and status registers (0x00..0x0D) are untouched. This is still writing to a machine's
// NVRAM, which is why the footprint is 11 bytes and no wider.

use x86_64::instructions::port::Port;

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

/// Marks the record as ours. Without it, uninitialised NVRAM would be read as a real death report.
const REG_MAGIC: u8 = 0x50;
const MAGIC: u8 = 0x4E; // 'N'
/// How far the operation under investigation got. See the `PM_*` constants.
const REG_STAGE: u8 = 0x51;
/// How it died, if it died somewhere that knows. See the `WHY_*` constants.
const REG_WHY: u8 = 0x52;
/// 8 little-endian bytes of context — CR2 for a kernel page fault, otherwise 0.
const REG_DETAIL: u8 = 0x53;
/// 8 little-endian bytes belonging to the STAGE (e.g. which segment vaddr was being copied).
/// Kept apart from `REG_DETAIL` so a fault's address and the work in progress never overwrite
/// each other — which is exactly what went wrong on the first post-mortem read.
const REG_STAGE_DETAIL: u8 = 0x5B;

// Stages, in the order exec walks them. Ordered on purpose: the value is "furthest point
// reached", so the report names the last thing that worked and the first thing that did not.
pub const PM_IDLE: u8 = 0; // nothing in flight — a clean run leaves this behind
pub const PM_FORK_ENTER: u8 = 1;
pub const PM_FORK_DONE: u8 = 2;
pub const PM_EXEC_ENTER: u8 = 3;
pub const PM_EXEC_PATH_OK: u8 = 4;
pub const PM_READ_ENTER: u8 = 5;
pub const PM_READ_DONE: u8 = 6;
pub const PM_CLEAR_ENTER: u8 = 7;
pub const PM_CLEAR_DONE: u8 = 8;
pub const PM_ELF_ENTER: u8 = 9;
pub const PM_ELF_MAP: u8 = 10;
pub const PM_ELF_COPY: u8 = 11;
pub const PM_ELF_PROT: u8 = 12;
pub const PM_ELF_DONE: u8 = 13;
pub const PM_EXEC_DONE: u8 = 14;

pub const WHY_NONE: u8 = 0;
pub const WHY_PANIC: u8 = 1;
pub const WHY_DOUBLE_FAULT: u8 = 2;
pub const WHY_KERNEL_PF: u8 = 3;
/// `MEMORY_MANAGER` was never acquired. Detail = the `MM_SITE_*` id from memory.rs.
pub const WHY_MM_DEADLOCK: u8 = 4;

fn stage_name(s: u8) -> &'static str {
    match s {
        PM_IDLE => "idle (no exec in flight)",
        PM_FORK_ENTER => "fork: entered",
        PM_FORK_DONE => "fork: child queued",
        PM_EXEC_ENTER => "execve: arm entered",
        PM_EXEC_PATH_OK => "execve: path validated+copied",
        PM_READ_ENTER => "execve: read_file_alloc started",
        PM_READ_DONE => "execve: read_file_alloc returned",
        PM_CLEAR_ENTER => "execve: clear_user_address_space started",
        PM_CLEAR_DONE => "execve: address space cleared",
        PM_ELF_ENTER => "elf: load_elf_full entered",
        PM_ELF_MAP => "elf: mapping a PT_LOAD segment",
        PM_ELF_COPY => "elf: copying segment bytes",
        PM_ELF_PROT => "elf: applying p_flags",
        PM_ELF_DONE => "elf: load complete",
        PM_EXEC_DONE => "execve: handed off to entry point",
        _ => "unknown",
    }
}

fn why_name(w: u8) -> &'static str {
    match w {
        WHY_NONE => "no fault recorded (hang, or died before any handler ran)",
        WHY_PANIC => "kernel panic",
        WHY_DOUBLE_FAULT => "double fault",
        WHY_KERNEL_PF => "KERNEL-MODE page fault (detail = CR2)",
        WHY_MM_DEADLOCK => "MEMORY_MANAGER lock DEADLOCK (detail = MM_SITE id; 1=allocate_frame, 2=allocate_user_pages_at)",
        _ => "unknown",
    }
}

unsafe fn write_reg(reg: u8, val: u8) {
    let mut addr: Port<u8> = Port::new(CMOS_ADDR);
    let mut data: Port<u8> = Port::new(CMOS_DATA);
    // Bit 7 of the index port is NMI-disable; keep it clear, exactly as rtc.rs does.
    addr.write(reg & 0x7F);
    data.write(val);
}

unsafe fn read_reg(reg: u8) -> u8 {
    let mut addr: Port<u8> = Port::new(CMOS_ADDR);
    let mut data: Port<u8> = Port::new(CMOS_DATA);
    addr.write(reg & 0x7F);
    data.read()
}

/// Record "we got this far". Two port writes, no lock, no allocation — callable from anywhere,
/// including a panic handler whose heap is unusable.
///
/// Interrupts are masked around the pair because the index and data ports are a two-step
/// sequence that `rtc.rs` also uses: an interrupt landing between them would read the clock
/// through our index and hand the caller a stage byte instead of the seconds register.
pub fn mark(stage: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        write_reg(REG_MAGIC, MAGIC);
        write_reg(REG_STAGE, stage);
    });
}

/// Record why we are dying, plus one 64-bit detail (CR2 for a page fault).
///
/// ★ FIRST CAUSE WINS. A kernel page fault records `WHY_KERNEL_PF` + CR2 and then falls through
/// to `panic!` (`interrupts.rs`), whose handler called this again — overwriting cause 3 with
/// cause 1 and, far worse, **overwriting CR2 with 0**. The root cause is the FIRST thing that
/// went wrong; everything after it is fallout, and fallout must not be allowed to erase the
/// evidence. This cost a boot: the record said "kernel panic, detail 0" for what was actually a
/// kernel page fault at an address we then had no way to read.
pub fn mark_why(why: u8, detail: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        if read_reg(REG_MAGIC) == MAGIC && read_reg(REG_WHY) != WHY_NONE {
            return; // a cause is already recorded — keep it
        }
        write_reg(REG_MAGIC, MAGIC);
        write_reg(REG_WHY, why);
        for i in 0..8 {
            write_reg(REG_DETAIL + i as u8, (detail >> (i * 8)) as u8);
        }
    });
}

/// Record a stage AND a 64-bit address for it (which segment, which page).
///
/// Separate storage from `mark_why`'s detail on purpose: the two answer different questions —
/// "what were we working on" vs "what blew up" — and the last boot showed how badly you need
/// both at once. `elf: copying segment bytes` does not say WHICH segment when the loop runs
/// four times and each pass overwrites the stage.
pub fn mark_at(stage: u8, addr: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        write_reg(REG_MAGIC, MAGIC);
        write_reg(REG_STAGE, stage);
        for i in 0..8 {
            write_reg(REG_STAGE_DETAIL + i as u8, (addr >> (i * 8)) as u8);
        }
    });
}

/// Clear the record. Called once the operation completed successfully, so that a later boot does
/// not report a death that never happened — a stale breadcrumb is worse than none, because it
/// looks exactly like a fresh one.
pub fn clear() {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        write_reg(REG_STAGE, PM_IDLE);
        write_reg(REG_WHY, WHY_NONE);
        // Both detail fields too. `clear()` used to leave them behind, so a later run could pair
        // a fresh cause with a stale address and read as a coherent report.
        for i in 0..8 {
            write_reg(REG_DETAIL + i as u8, 0);
            write_reg(REG_STAGE_DETAIL + i as u8, 0);
        }
        write_reg(REG_MAGIC, 0);
    });
}

/// Read back and print whatever the PREVIOUS run left behind, then clear it.
///
/// Call this early in boot — it is the first thing worth knowing after a freeze, and it must run
/// before anything else can fill BOOT_LOG or overwrite the record.
pub fn report_and_clear() {
    let (magic, stage, why, detail, stage_detail) = x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        let magic = read_reg(REG_MAGIC);
        let stage = read_reg(REG_STAGE);
        let why = read_reg(REG_WHY);
        let mut detail: u64 = 0;
        let mut stage_detail: u64 = 0;
        for i in 0..8 {
            detail |= (read_reg(REG_DETAIL + i as u8) as u64) << (i * 8);
            stage_detail |= (read_reg(REG_STAGE_DETAIL + i as u8) as u64) << (i * 8);
        }
        (magic, stage, why, detail, stage_detail)
    });

    if magic != MAGIC || (stage == PM_IDLE && why == WHY_NONE) {
        crate::serial_println!("[POSTMORTEM] no record from the previous run (clean shutdown or first boot)");
    } else {
        crate::serial_println!("[POSTMORTEM] ***** THE PREVIOUS RUN DIED *****");
        crate::serial_println!("[POSTMORTEM]   furthest stage: {} = {}", stage as u32, stage_name(stage));
        crate::serial_println!("[POSTMORTEM]   stage address:  {:#x}  (segment/page being worked on)", stage_detail);
        crate::serial_println!("[POSTMORTEM]   cause:          {} = {}", why as u32, why_name(why));
        crate::serial_println!("[POSTMORTEM]   fault detail:   {:#x}  (CR2 for a page fault)", detail);
    }

    clear();
}
