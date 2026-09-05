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

// ---------------------------------------------------------------------------------------------
// LIVENESS HEARTBEAT (0x63..0x67)
//
// ★★★ Why this exists, and why it is in NVRAM rather than on screen.
//
// A freeze that leaves the desktop painted but dead has exactly one question behind it: is the
// KERNEL still running and merely never scheduling userspace, or is the whole box gone? Those are
// completely different bugs — starvation versus a deadlock or a silent fault — and every other
// channel for answering it is unavailable at the moment it matters:
//
//   * BOOT_LOG is read through System Monitor, which is a userspace app.
//   * There is no serial port on this machine.
//   * The screen is a GPU-owned scanout buffer written by BLT, so the kernel cannot paint into
//     what is actually being displayed (this is also why panics have been invisible since P1a).
//   * A `panic!` never happens in the starvation case, so the death record stays empty.
//
// CMOS is none of those things. Writing a handful of bytes once a second costs two `out`
// instructions per byte, needs no allocation, no lock and no driver, and survives the power cycle
// that is the only way out of a freeze. The NEXT boot then reports the last state the scheduler
// was in before it stopped answering.
// ---------------------------------------------------------------------------------------------

/// Rolling one-second counter. If this advanced right up to the freeze, the timer interrupt and the
/// scheduler were both still alive and the fault is above them.
const REG_HB_TICK: u8 = 0x63;
/// Total tasks in the scheduler.
const REG_HB_TASKS: u8 = 0x64;
/// How many of them were Ready or Running. Zero non-idle runnable tasks with the kernel still
/// ticking IS the starvation signature.
const REG_HB_RUNNABLE: u8 = 0x65;
/// Index of the task that was current when the beat was written.
const REG_HB_CURRENT: u8 = 0x66;
/// Seconds since a NON-IDLE task last got the CPU, saturating at 255. This is the money value: a
/// large number here means userspace stopped being scheduled while the kernel kept running.
const REG_HB_IDLE_SECS: u8 = 0x67;
/// Bitmask of cores that reached `schedule()` within the last 2 s. THE discriminator between "the
/// whole kernel stopped" and "one core wedged with interrupts masked and took its tasks with it".
const REG_HB_CORES_ALIVE: u8 = 0x69;
/// Which core wrote the beat — without it, `tasks`/`runnable`/`current` are unattributed, and each
/// core has its OWN task list (`PerCpu` owns its `Scheduler` by value).
const REG_HB_WRITER: u8 = 0x6A;
/// The blocking lock that seized first. See `stuck_lock` and `LOCK_SITE_*`.
const REG_STUCK_LOCK: u8 = 0x6D;

// Lock sites, for `stuck_lock`. These are the blocking acquisitions reachable from the network
// path with interrupts masked — i.e. the ones that can wedge a core so hard it stops taking timer
// interrupts, which is precisely what `cores alive` showed happening to core 0.
pub const LOCK_NET_DRIVER: u8 = 1;
pub const LOCK_NET_IFACE: u8 = 2;
pub const LOCK_GLOBAL_SOCKETS: u8 = 3;
pub const LOCK_DHCP_HANDLE: u8 = 4;
pub const LOCK_DNS_HANDLE: u8 = 5;
pub const LOCK_MEMORY_MANAGER_V2P: u8 = 6;
pub const LOCK_WIFI_SOCKETS: u8 = 7;
pub const LOCK_WIFI_IFACE: u8 = 8;

fn lock_site_name(s: u8) -> &'static str {
    match s {
        LOCK_NET_DRIVER => "NET_DRIVER (poll_network_locked, interrupts masked)",
        LOCK_NET_IFACE => "NET_IFACE (poll_network_locked, interrupts masked)",
        LOCK_GLOBAL_SOCKETS => "GLOBAL_SOCKETS (poll_network_locked, interrupts masked)",
        LOCK_DHCP_HANDLE => "DHCP_HANDLE (poll_network_locked, interrupts masked)",
        LOCK_DNS_HANDLE => "DNS_HANDLE (poll_network_locked, interrupts masked)",
        LOCK_MEMORY_MANAGER_V2P => "MEMORY_MANAGER via virt_to_phys (bare spin lock, no masking)",
        LOCK_WIFI_SOCKETS => "WIFI_SOCKETS (with_sockets)",
        LOCK_WIFI_IFACE => "WIFI_IFACE (with_stack)",
        _ => "unknown lock",
    }
}

/// A breadcrumb USERSPACE can drop, via syscall 551.
///
/// ★★★ The kernel's own `PM_*` stages only cover fork/exec, so a freeze inside an ordinary running
/// program leaves nothing behind but "it stopped". This gives a userspace program the same power
/// the ELF loader already has: write a byte before each risky step, and the next boot reports the
/// last one reached. That converts "`time sync` freezes" — where the command does DNS, a TCP
/// connect, a TLS-free HTTP transfer and a syscall, any of which could be the culprit — into a
/// single number naming the step it died on.
///
/// One byte, two `out` instructions, no allocation and no lock, so it is as safe to call as the
/// kernel's own `mark`.
const REG_USER_MARK: u8 = 0x68;

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
/// The KERNEL heap could not satisfy an allocation. Detail = the requested size in bytes.
///
/// Distinct from `WHY_PANIC` on purpose. `alloc_error_handler` calls `panic!`, so an out-of-memory
/// death used to record cause 1 with detail 0 — identical to every other panic, and therefore no
/// answer at all to "did it run out of memory?". That is a question you cannot answer any other way
/// after the fact: the freeze wipes BOOT_LOG and this machine has no serial console.
pub const WHY_KERNEL_OOM: u8 = 5;
/// A KERNEL-MODE #GP. Detail = the faulting instruction pointer.
///
/// Worth its own cause for the same reason as `WHY_KERNEL_OOM`: the `panic!` in `gpf_handler`
/// otherwise records `WHY_PANIC` at that handler's own file:line, which is identical for every
/// kernel #GP and so carries no information. The IP is the entire diagnosis — feed it to
/// `addr2line -e target/x86_64-unknown-none/release/nyx-kernel <ip>`.
///
/// It exists because the screen could not be trusted: a #GP IP read off the banded red screen was
/// off by one digit and pointed mid-instruction, which cost a debugging pass. CMOS survives the
/// power cycle and does not care what the display engine is doing.
pub const WHY_KERNEL_GPF: u8 = 6;

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
        WHY_MM_DEADLOCK => "MEMORY_MANAGER lock DEADLOCK (detail = MM_SITE id, see mm_site_name)",
        WHY_KERNEL_OOM => "KERNEL HEAP EXHAUSTED (detail = requested bytes)",
        WHY_KERNEL_GPF => "KERNEL-MODE #GP (detail = faulting IP; run addr2line on it)",
        _ => "unknown",
    }
}

/// Which `MEMORY_MANAGER` acquire wedged. Kept here rather than in memory.rs so the boot report is
/// self-describing — a bare site number means another round trip through the source to read.
fn mm_site_name(s: u64) -> &'static str {
    match s {
        // Only these two are watchdogged, deliberately — see the long note on MM_SITE_ALLOC_FRAME
        // in memory.rs. Converting the other acquires hard-froze the machine.
        1 => "allocate_frame",
        2 => "allocate_user_pages_at",
        _ => "unknown site",
    }
}

/// Pack a panic location into the 8-byte detail field: line number in the low 32 bits, the first
/// four bytes of the file's BASENAME in the high 32.
///
/// The basename bytes are stored raw rather than hashed so the boot report is readable without a
/// lookup table — "memo:412" names the panic on sight. A hash would have needed a tool to invert,
/// which is one more thing to be wrong at the exact moment nothing else is working.
pub fn pack_location(file: &str, line: u32) -> u64 {
    let base = match file.rfind(['/', '\\']) {
        Some(i) => &file[i + 1..],
        None => file,
    };
    let b = base.as_bytes();
    let mut tag: u64 = 0;
    let mut i = 0;
    while i < 4 {
        let c = if i < b.len() { b[i] } else { b' ' };
        tag |= (c as u64) << (i * 8);
        i += 1;
    }
    (tag << 32) | (line as u64)
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

/// Name a lock acquisition that has been spinning far too long — WITHOUT changing control flow.
///
/// ★★★ This is the corrected version of a mistake worth not repeating. An earlier attempt gave
/// fifteen `MEMORY_MANAGER` acquisitions a watchdog that ended in `loop { hlt() }`, which turned
/// "spin until the other core releases" into "kill the machine if the holder takes longer than half
/// a second" — and several of those acquisitions legitimately run long (page-table walks, zeroing
/// SHM frames) or are reached from IRQ context. It hard-froze the machine at boot.
///
/// The diagnosis was right; the remedy was not. So this only ever WRITES A BYTE and returns. The
/// caller keeps spinning exactly as it did before, so a slow-but-healthy acquire is unaffected and
/// nothing new can wedge — but a genuine deadlock leaves its name in NVRAM for the next boot.
///
/// First writer wins, so the report names the lock that seized first rather than whichever core
/// piled on last.
pub fn stuck_lock(site: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        if read_reg(REG_STUCK_LOCK) == 0 {
            write_reg(REG_STUCK_LOCK, site);
        }
    });
}

/// Take a blocking lock, and if it spins far too long, RECORD WHICH ONE — then carry on spinning.
///
/// Control flow is deliberately unchanged; see `stuck_lock` for why that matters. ~20M spins is
/// well under a second and orders of magnitude beyond honest contention on any lock this is used
/// for, all of which are held for a single bounded operation.
pub fn lock_watched<T>(m: &spin::Mutex<T>, site: u8) -> spin::MutexGuard<'_, T> {
    let mut spins: u64 = 0;
    loop {
        if let Some(g) = m.try_lock() {
            return g;
        }
        spins += 1;
        if spins == 20_000_000 {
            stuck_lock(site);
        }
        core::hint::spin_loop();
    }
}

/// Drop a userspace breadcrumb (syscall 551). `0` clears it.
pub fn user_mark(v: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        write_reg(REG_USER_MARK, v);
    });
}

/// Write one liveness beat. Called about once a second from the scheduler.
///
/// Deliberately NOT gated on `REG_MAGIC`: the heartbeat has to be readable after a freeze that
/// wrote no death record at all, which is precisely the starvation case this exists to catch.
pub fn heartbeat(
    tick: u8, tasks: u8, runnable: u8, current: u8, idle_secs: u8,
    cores_alive: u8, writer: u8,
) {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        write_reg(REG_HB_TICK, tick);
        write_reg(REG_HB_TASKS, tasks);
        write_reg(REG_HB_RUNNABLE, runnable);
        write_reg(REG_HB_CURRENT, current);
        write_reg(REG_HB_IDLE_SECS, idle_secs);
        write_reg(REG_HB_CORES_ALIVE, cores_alive);
        write_reg(REG_HB_WRITER, writer);
    });
}

/// Clear the record. Called once the operation completed successfully, so that a later boot does
/// not report a death that never happened — a stale breadcrumb is worse than none, because it
/// looks exactly like a fresh one.
pub fn clear() {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        write_reg(REG_STAGE, PM_IDLE);
        // The userspace breadcrumb too, or a run that ends cleanly still reports the last stage a
        // previous run reached — a stale breadcrumb reads exactly like a fresh one.
        write_reg(REG_USER_MARK, 0);
        write_reg(REG_STUCK_LOCK, 0);
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

/// Print to the SCREEN as well as the log.
///
/// ★★★ `serial_println!` goes to COM1 (which this laptop does not have) and to `BOOT_LOG`, a RAM
/// buffer read through System Monitor — **a userspace app**. So the entire post-mortem was
/// unreachable in precisely the case it exists for: a freeze that leaves ring 3 dead. The screen is
/// the only output that needs nothing from userspace, and this early in boot it is still the
/// firmware framebuffer, before the GPU plane flip that swallowed the RSOD.
macro_rules! say {
    ($($arg:tt)*) => {{
        crate::serial_println!($($arg)*);
        crate::vga_println!($($arg)*);
    }};
}

/// Roughly how long to hold a death report on screen, in TSC cycles, assuming ~2.5 GHz.
///
/// Deliberately crude: this runs before the TSC is calibrated and before `UPTIME_MS` ticks, so
/// there is no accurate clock to use. A hold somewhere between 4 s and 16 s depending on the part
/// is fine — the requirement is "long enough to read or photograph", not a precise interval. It
/// only happens after a real recorded death, so a healthy boot is never delayed.
const HOLD_CYCLES: u64 = 20_000_000_000;

fn hold_for_reading() {
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    while unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start) < HOLD_CYCLES {
        core::hint::spin_loop();
    }
}

/// What the previous run's death record said, in the two words the boot screen has room for.
///
/// The full report goes to the log, where it belongs. This is only enough for the cold-start screen
/// to stop being silent about it. Both fields point into the existing name tables, so carrying it
/// costs nothing and cannot allocate.
#[derive(Clone, Copy)]
pub struct Death {
    /// Furthest boot stage the previous run reached.
    pub stage: &'static str,
    /// What killed it.
    pub why: &'static str,
}

/// Read back and print whatever the PREVIOUS run left behind, then clear it.
///
/// Call this early in boot — it is the first thing worth knowing after a freeze, and it must run
/// before anything else can fill BOOT_LOG or overwrite the record.
///
/// Returns the record if there was one, so the cold-start screen can bring the mark up in its
/// attention state. On a machine with no serial port this CMOS record is the only evidence that
/// survives a freeze, and until now it was visible for exactly as long as it took the desktop to
/// paint over the log.
pub fn report_and_clear() -> Option<Death> {
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

    // The heartbeat is read and reported FIRST and unconditionally, because the freeze that needs
    // it hardest is the one that leaves no death record at all.
    let (hb_tick, hb_tasks, hb_runnable, hb_current, hb_idle, hb_cores, hb_writer) =
        x86_64::instructions::interrupts::without_interrupts(|| unsafe {
            (
                read_reg(REG_HB_TICK),
                read_reg(REG_HB_TASKS),
                read_reg(REG_HB_RUNNABLE),
                read_reg(REG_HB_CURRENT),
                read_reg(REG_HB_IDLE_SECS),
                read_reg(REG_HB_CORES_ALIVE),
                read_reg(REG_HB_WRITER),
            )
        });

    let stuck = x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        read_reg(REG_STUCK_LOCK)
    });
    if stuck != 0 {
        say!("[STUCKLOCK] ***** a blocking lock spun for over half a second *****");
        say!("[STUCKLOCK]   {}", lock_site_name(stuck));
        say!("[STUCKLOCK]   A core spinning on this with interrupts masked takes no timer, so it");
        say!("[STUCKLOCK]   never reaches the scheduler and strands every task it owns. Compare");
        say!("[STUCKLOCK]   with `cores alive` below: a dark bit is that core.");
    }

    let user_mark_v = x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        read_reg(REG_USER_MARK)
    });
    if user_mark_v != 0 {
        say!("[USERMARK] last userspace breadcrumb before the stop: {}", user_mark_v as u32);
        say!("[USERMARK]   `time sync` legend: 1=entered 2=DNS+connect+GET returned");
        say!("[USERMARK]     3=Date header parsed  4=RTC read  5=syscall 550 returned  6=done");
    }

    if hb_tasks != 0 {
        say!("[HEARTBEAT] last beat before the previous run stopped:");
        // ★ tasks/runnable/current describe ONLY the core named by `writer`. Each core owns its own
        // `Scheduler` (percpu.rs), so these are per-core numbers and saying otherwise is how the
        // first version of this report announced "every userspace task was Blocked" on the strength
        // of a secondary core having nothing but its idle task.
        say!("[HEARTBEAT]   tick={}  userspace idle {}s   cores alive: {:#010b}",
            hb_tick as u32, hb_idle as u32, hb_cores);
        say!("[HEARTBEAT]   (core {} wrote this; ITS OWN list: tasks={} runnable={} current={})",
            hb_writer as u32, hb_tasks as u32, hb_runnable as u32, hb_current as u32);

        let live = hb_cores.count_ones();
        if hb_cores != 0 && live < 4 {
            say!("[HEARTBEAT]   => ONLY {} core(s) still reached the scheduler. A core that is NOT", live);
            say!("[HEARTBEAT]      in that mask took no timer interrupt for 2s — i.e. it is spinning");
            say!("[HEARTBEAT]      with interrupts masked, and every task it owned is stranded.");
            say!("[HEARTBEAT]      Look for a blocking lock taken at IF=0 on the network path.");
        }
        if hb_idle >= 3 {
            say!("[HEARTBEAT]   => No core scheduled ANY userspace task for {}s, then the beat", hb_idle as u32);
            say!("[HEARTBEAT]      itself stopped. Kernel outlived userspace, then died too.");
        } else {
            say!("[HEARTBEAT]   => userspace was running right up to the stop, so whatever went");
            say!("[HEARTBEAT]      wrong took the kernel down with it immediately.");
        }
    } else {
        say!("[HEARTBEAT] no heartbeat recorded (first boot with this build, or it died before the");
        say!("[HEARTBEAT] scheduler ever ran a full second).");
    }

    if magic != MAGIC || (stage == PM_IDLE && why == WHY_NONE) {
        say!("[POSTMORTEM] no death record — no panic, no fault, no watchdog. Combined with the");
        say!("[POSTMORTEM] heartbeat above, that is the whole diagnosis.");
    } else {
        say!("[POSTMORTEM] ***** THE PREVIOUS RUN DIED *****");
        say!("[POSTMORTEM]   furthest stage: {} = {}", stage as u32, stage_name(stage));
        say!("[POSTMORTEM]   stage address:  {:#x}  (segment/page being worked on)", stage_detail);
        say!("[POSTMORTEM]   cause:          {} = {}", why as u32, why_name(why));
        say!("[POSTMORTEM]   fault detail:   {:#x}", detail);
        // Decode the detail per cause. A raw hex number is exactly the kind of "evidence" that
        // still costs a boot to interpret, and the whole point of this record is that it does not.
        match why {
            WHY_PANIC => {
                let tag = (detail >> 32) as u32;
                let b = [tag as u8, (tag >> 8) as u8, (tag >> 16) as u8, (tag >> 24) as u8];
                let name = core::str::from_utf8(&b).unwrap_or("????");
                say!("[POSTMORTEM]   -> panicked at {}…:{}  (file basename, line)",
                    name, detail as u32);
            }
            WHY_KERNEL_OOM => say!(
                "[POSTMORTEM]   -> the kernel heap could not allocate {} bytes. It is {} MiB total \
                 (allocator.rs HEAP_SIZE); this is a real exhaustion, not a lock bug.",
                detail, crate::allocator::HEAP_SIZE / (1024 * 1024)),
            WHY_MM_DEADLOCK => say!(
                "[POSTMORTEM]   -> wedged acquiring MEMORY_MANAGER in {}", mm_site_name(detail)),
            WHY_KERNEL_PF => say!(
                "[POSTMORTEM]   -> kernel touched unmapped/protected address {:#x}", detail),
            WHY_DOUBLE_FAULT => say!(
                "[POSTMORTEM]   -> faulting instruction pointer {:#x}", detail),
            WHY_NONE => say!(
                "[POSTMORTEM]   -> NO handler ran. Not a panic: a true hard hang, or userspace \
                 starvation with the kernel still alive."),
            _ => {}
        }
    }

    // Hold whenever there was ANYTHING to read — which now includes a heartbeat with no death
    // record, the exact combination a starvation freeze produces. Boot continues into the
    // compositor, which owns the framebuffer and paints straight over this, so without a pause the
    // one artifact that survived the freeze is destroyed a second later by the machine recovering.
    let had_record = magic == MAGIC && !(stage == PM_IDLE && why == WHY_NONE);
    // ★ Only when something actually DIED.
    //
    // This used to fire on a bare heartbeat too — and a heartbeat is written by any run that lasted
    // more than a second, so it fired on essentially every boot. Eight seconds of report before the
    // machine will show you anything is a real cost to pay on a normal power-on, and paying it
    // every time is how a warning stops being read at all. A death record or a stuck lock is
    // evidence; "the previous run ran" is not.
    //
    // Nothing is lost from the report itself — the heartbeat lines above are still printed, and
    // pressing a key at any point during boot brings the whole log back (`boot_screen`).
    if had_record || stuck != 0 {
        say!("[POSTMORTEM] (holding ~8s so this can be read/photographed — boot continues after)");
        hold_for_reading();
    }

    clear();

    // A stuck lock counts as a death even without a record: it is the freeze that leaves no death
    // record at all, which is exactly the case worth surfacing on the next boot's screen.
    if had_record {
        Some(Death { stage: stage_name(stage), why: why_name(why) })
    } else if stuck != 0 {
        Some(Death { stage: "scheduler", why: lock_site_name(stuck) })
    } else {
        None
    }
}
