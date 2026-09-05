use core::ffi::{c_char, c_void};
use x86_64::instructions::port::Port;

// ==========================================
// 0. MEMORY TRANSLATION HELPER
// ==========================================
#[inline]
fn phys_to_virt(phys: u64) -> u64 {
    // Unwrap the Option from your memory module, or fallback to raw phys
    crate::memory::phys_to_virt(phys).unwrap_or(phys)
}
// ==========================================
// 1. MEMORY MANAGEMENT
// ==========================================

// ★★ `AcpiOsFree` USED TO BE A NO-OP. ACPICA leaked every allocation it ever made.
//
// That is not a slow leak. The AML interpreter allocates a parse object per operator, so a single
// `AcpiEvaluateObject` churns through hundreds; a namespace walk churns through thousands. Once the
// thermal governor started refreshing the ACPI cache once a second, the kernel heap drained in
// seconds, `alloc` began returning null, and ACPICA dereferenced it — a #GP inside
// `AcpiPsGetNextArg` roughly two seconds after reaching ring 3.
//
// It went unnoticed for as long as it did because almost nothing evaluated AML: `_PS0` and the fan
// methods run once at boot, and `get_acpi_temperature` was dead code. The moment anything called
// ACPI repeatedly, the leak became fatal.
//
// Rust's `dealloc` needs the original `Layout`, which C does not hand back, so the size rides in a
// 16-byte header before the pointer we return. 16 rather than 8 keeps the payload 16-byte aligned —
// ACPICA puts 64-bit fields and `ACPI_OBJECT` unions in these blocks, and a misaligned SSE access
// on one raises #GP, which is exactly the failure mode we are here to stop.
// ★★★ AND IT MUST NOT USE THE GLOBAL HEAP.
//
// The first version of this fix called `alloc`/`dealloc` directly and the machine stopped booting —
// stuck on the Nyx mark. Not a crash: pathologically SLOW.
//
// The kernel heap is `linked_list_allocator::LockedHeap`, a first-fit free list. `dealloc` inserts
// into a sorted list and `alloc` walks it, so both are O(n) in the number of free blocks. Loading a
// 306 KB DSDT makes on the order of 10^5 small allocations; freeing them into that list makes the
// load O(n^2). The old no-op `free` had been hiding this by keeping the free list at one block —
// the leak was accidentally what made it fast.
//
// So ACPICA gets its own pool: per-size-class LIFO free lists, O(1) both ways, carved from slabs
// taken off the global heap in big chunks. Slabs are never handed back, which is fine — ACPICA's
// working set is bounded once bring-up finishes, and the whole point is to keep this traffic off
// the global free list entirely.
//
// Blocks larger than the biggest class fall through to the global heap. Those are rare (table
// copies, `ACPI_ALLOCATE_BUFFER` results) and few enough not to matter.
const ACPI_ALLOC_HDR: usize = 16;
const ACPI_CLASS_SIZES: [usize; 8] = [32, 64, 128, 256, 512, 1024, 2048, 4096];
/// Marks a block that came from the global heap rather than a pool class.
const ACPI_CLASS_LARGE: u64 = u64::MAX;
/// Slab granularity. Big enough that carving is rare, small enough not to strand memory.
const ACPI_SLAB: usize = 64 * 1024;

struct AcpiPool {
    free: [*mut u8; 8],
}
unsafe impl Send for AcpiPool {}

static ACPI_POOL: spin::Mutex<AcpiPool> = spin::Mutex::new(AcpiPool { free: [core::ptr::null_mut(); 8] });

/// Take one block of class `c`, carving a fresh slab if that list is empty.
///
/// Interrupts are masked around the pool lock for the same reason the global heap masks them: this
/// is reached from the thermal governor at IF=1 and from bring-up, and a lock held across the
/// preemption boundary is the documented way to wedge a core on this machine.
unsafe fn acpi_pool_take(c: usize) -> *mut u8 {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut p = ACPI_POOL.lock();
        if p.free[c].is_null() {
            let bs = ACPI_CLASS_SIZES[c];
            let n = ACPI_SLAB / bs;
            let layout = match core::alloc::Layout::from_size_align(ACPI_SLAB, ACPI_ALLOC_HDR) {
                Ok(l) => l,
                Err(_) => return core::ptr::null_mut(),
            };
            let slab = alloc::alloc::alloc(layout);
            if slab.is_null() {
                return core::ptr::null_mut();
            }
            // Thread the slab onto the free list, first word of each block = next.
            for i in 0..n {
                let b = slab.add(i * bs);
                *(b as *mut *mut u8) = p.free[c];
                p.free[c] = b;
            }
        }
        let b = p.free[c];
        if !b.is_null() {
            p.free[c] = *(b as *mut *mut u8);
        }
        b
    })
}

unsafe fn acpi_pool_give(c: usize, b: *mut u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut p = ACPI_POOL.lock();
        *(b as *mut *mut u8) = p.free[c];
        p.free[c] = b;
    })
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsAllocate(size: u64) -> *mut c_void {
    if size == 0 { return core::ptr::null_mut(); }
    let total = match (size as usize).checked_add(ACPI_ALLOC_HDR) {
        Some(t) => t,
        None => return core::ptr::null_mut(),
    };

    let base = match ACPI_CLASS_SIZES.iter().position(|&s| s >= total) {
        Some(c) => {
            let b = acpi_pool_take(c);
            if b.is_null() { return core::ptr::null_mut(); }
            *(b.add(8) as *mut u64) = c as u64;
            b
        }
        None => {
            let layout = match core::alloc::Layout::from_size_align(total, ACPI_ALLOC_HDR) {
                Ok(l) => l,
                Err(_) => return core::ptr::null_mut(),
            };
            let b = alloc::alloc::alloc(layout);
            // Null rather than a panic: ACPICA checks for null and unwinds with AE_NO_MEMORY,
            // whereas `handle_alloc_error` would take the kernel down instead.
            if b.is_null() { return core::ptr::null_mut(); }
            *(b.add(8) as *mut u64) = ACPI_CLASS_LARGE;
            b
        }
    };

    *(base as *mut u64) = size;
    base.add(ACPI_ALLOC_HDR) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsFree(ptr: *mut c_void) {
    if ptr.is_null() { return; }
    let base = (ptr as *mut u8).sub(ACPI_ALLOC_HDR);
    let size = *(base as *const u64) as usize;
    let class = *(base.add(8) as *const u64);

    if class == ACPI_CLASS_LARGE {
        let total = size + ACPI_ALLOC_HDR;
        if let Ok(layout) = core::alloc::Layout::from_size_align(total, ACPI_ALLOC_HDR) {
            alloc::alloc::dealloc(base, layout);
        }
    } else if (class as usize) < ACPI_CLASS_SIZES.len() {
        acpi_pool_give(class as usize, base);
    }
    // An out-of-range class means the header was overwritten. Dropping the block leaks it, which is
    // survivable; handing a corrupt pointer to the allocator is not.
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsMapMemory(phys: u64, _size: u64) -> *mut c_void {
    phys_to_virt(phys) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsUnmapMemory(_virt: *mut c_void, _size: u64) {}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsReadMemory(address: u64, value: *mut u64, width: u32) -> u32 {
    let virt_addr = phys_to_virt(address) as *const u8;
    if !value.is_null() {
        match width {
            8 => { *value = core::ptr::read_volatile(virt_addr) as u64; }
            16 => { *value = core::ptr::read_volatile(virt_addr as *const u16) as u64; }
            32 => { *value = core::ptr::read_volatile(virt_addr as *const u32) as u64; }
            64 => { *value = core::ptr::read_volatile(virt_addr as *const u64); }
            _ => return 1, // AE_BAD_PARAMETER
        }
    }
    0 // AE_OK
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsWriteMemory(address: u64, value: u64, width: u32) -> u32 {
    let virt_addr = phys_to_virt(address) as *mut u8;
    match width {
        8 => { core::ptr::write_volatile(virt_addr, value as u8); }
        16 => { core::ptr::write_volatile(virt_addr as *mut u16, value as u16); }
        32 => { core::ptr::write_volatile(virt_addr as *mut u32, value as u32); }
        64 => { core::ptr::write_volatile(virt_addr as *mut u64, value); }
        _ => return 1,
    }
    0
}

// ==========================================
// 2. HARDWARE I/O
// ==========================================

#[no_mangle]
pub unsafe extern "C" fn AcpiOsReadPort(address: u64, value: *mut u32, width: u32) -> u32 {
    if value.is_null() { return 1; }
    match width {
        8 => { *value = Port::<u8>::new(address as u16).read() as u32; }
        16 => { *value = Port::<u16>::new(address as u16).read() as u32; }
        32 => { *value = Port::<u32>::new(address as u16).read(); }
        _ => return 1,
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsWritePort(address: u64, value: u32, width: u32) -> u32 {
    match width {
        8 => { Port::<u8>::new(address as u16).write(value as u8); }
        16 => { Port::<u16>::new(address as u16).write(value as u16); }
        32 => { Port::<u32>::new(address as u16).write(value); }
        _ => return 1,
    }
    0
}

// ==========================================
// 3. PCI CONFIGURATION
// ==========================================

#[no_mangle]
pub unsafe extern "C" fn AcpiOsReadPciConfiguration(_pci_id: *mut c_void, _reg: u32, value: *mut u64, _width: u32) -> u32 {
    // Return all 1s (0xFFFFFFFF) so ACPICA knows the slot is empty and doesn't infinitely poll
    if !value.is_null() { *value = 0xFFFF_FFFF_FFFF_FFFF; }
    0
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsWritePciConfiguration(_pci_id: *mut c_void, _reg: u32, _value: u64, _width: u32) -> u32 {
    0
}

// ==========================================
// 4. TABLE POINTERS
// ==========================================

#[no_mangle]
pub unsafe extern "C" fn AcpiOsGetRootPointer() -> u64 {
    crate::acpi::ACPI_INFO.rsdp_addr.unwrap_or(0)
}

// ==========================================
// 5. SYNCHRONIZATION
// ==========================================

// ★★ THE OS LAYER, FOR REAL. These were all no-ops handing back the fake handle `1`.
//
// ACPICA's contract is not "these may be stubs". `AcpiUtAcquireMutex` tracks an owning thread id
// per mutex and uses it for recursion and deadlock detection, and `AcpiUtAcquireReadLock` keeps a
// reader count guarded by two DISTINCT mutexes. Handing every semaphore the same fake handle makes
// `Lock->ReaderMutex` and `Lock->WriterMutex` the same object, so that bookkeeping is nonsense.
//
// This matters because `AcpiNsWalkNamespace(… ACPI_NS_WALK_UNLOCK …)` releases and re-acquires
// `ACPI_MTX_NAMESPACE` around EVERY callback, and `AcpiWalkNamespace` takes a reader lock on
// `AcpiGbl_NamespaceRwLock` on top. That is the one path in ACPICA that leans hardest on these
// primitives — and it is the one that #GPs, while direct `AcpiGetHandle` lookups (which never
// unlock/relock) work fine. See task #30 for the full evidence table.
//
// Correct-but-simple: real counts, real exclusion, no blocking. ACPICA is called from exactly one
// task here (the thermal governor) plus boot, which never overlap, so a wait that cannot be
// satisfied means a genuine logic error and should report AE_TIME rather than pretend success.

const AE_TIME: u32 = 0x0011;
const AE_NO_MEMORY: u32 = 0x0004;
const AE_BAD_PARAMETER: u32 = 0x1001;

struct AcpiSem {
    count: core::sync::atomic::AtomicI64,
    max: i64,
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsCreateSemaphore(max: u32, initial: u32, out: *mut *mut c_void) -> u32 {
    if out.is_null() { return AE_BAD_PARAMETER; }
    let sem = alloc::boxed::Box::new(AcpiSem {
        count: core::sync::atomic::AtomicI64::new(initial as i64),
        max: max as i64,
    });
    // Each semaphore is a DISTINCT object. That is the whole point — see above.
    *out = alloc::boxed::Box::into_raw(sem) as *mut c_void;
    0
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsDeleteSemaphore(handle: *mut c_void) -> u32 {
    if handle.is_null() { return AE_BAD_PARAMETER; }
    drop(alloc::boxed::Box::from_raw(handle as *mut AcpiSem));
    0
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsWaitSemaphore(handle: *mut c_void, units: u32, _timeout: u16) -> u32 {
    use core::sync::atomic::Ordering;
    if handle.is_null() { return AE_BAD_PARAMETER; }
    let sem = &*(handle as *const AcpiSem);
    let want = units as i64;

    // Bounded spin, never an unbounded block: this runs in the thermal governor and in boot, and a
    // core that spins forever here takes scheduling down with it — the failure mode this whole
    // subsystem keeps rediscovering.
    for _ in 0..1_000_000u32 {
        let cur = sem.count.load(Ordering::Acquire);
        if cur >= want
            && sem.count
                .compare_exchange_weak(cur, cur - want, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            return 0;
        }
        core::hint::spin_loop();
    }
    AE_TIME
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsSignalSemaphore(handle: *mut c_void, units: u32) -> u32 {
    use core::sync::atomic::Ordering;
    if handle.is_null() { return AE_BAD_PARAMETER; }
    let sem = &*(handle as *const AcpiSem);
    let n = sem.count.fetch_add(units as i64, Ordering::AcqRel) + units as i64;
    // Clamp rather than let it drift above max — an over-signal is a caller bug, but a count that
    // grows without bound would silently defeat the exclusion these are here to provide.
    if sem.max > 0 && n > sem.max {
        sem.count.store(sem.max, Ordering::Release);
    }
    0
}

#[no_mangle] 
// ACPICA's raw spin locks. Held across very short critical sections, and — unlike the semaphores —
// genuinely reachable from interrupt context, so acquiring one must mask interrupts and releasing
// must restore the caller's previous state. That is exactly what the `Flags` return value is for,
// and returning a constant 0 from `AcpiOsAcquireLock` meant `AcpiOsReleaseLock` could re-enable
// interrupts that were never on — the same preemption-boundary hazard documented on the kernel heap.
#[no_mangle]
pub unsafe extern "C" fn AcpiOsCreateLock(out: *mut *mut c_void) -> u32 {
    if out.is_null() { return AE_BAD_PARAMETER; }
    let lock = alloc::boxed::Box::new(core::sync::atomic::AtomicBool::new(false));
    *out = alloc::boxed::Box::into_raw(lock) as *mut c_void;
    0
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsDeleteLock(handle: *mut c_void) {
    if handle.is_null() { return; }
    drop(alloc::boxed::Box::from_raw(handle as *mut core::sync::atomic::AtomicBool));
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsAcquireLock(handle: *mut c_void) -> usize {
    use core::sync::atomic::Ordering;
    let were_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    if !handle.is_null() {
        let lock = &*(handle as *const core::sync::atomic::AtomicBool);
        // Bounded: a lock that cannot be taken with interrupts already masked is a genuine bug, and
        // spinning forever would wedge the core rather than report it.
        for _ in 0..10_000_000u32 {
            if lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
    }
    were_enabled as usize
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsReleaseLock(handle: *mut c_void, flags: usize) {
    use core::sync::atomic::Ordering;
    if !handle.is_null() {
        let lock = &*(handle as *const core::sync::atomic::AtomicBool);
        lock.store(false, Ordering::Release);
    }
    // Restore, do not blindly enable: this may have been entered with interrupts already off.
    if flags != 0 {
        x86_64::instructions::interrupts::enable();
    }
}

// ==========================================
// 6. SCHEDULING & TIME
// ==========================================

/// ACPICA requires this to be **unique per thread and never zero**.
///
/// It used to return a constant `1`, which makes every caller look like the same thread. That
/// defeats `AcpiUtAcquireMutex`'s recursion and deadlock detection outright: with a shared id, two
/// genuinely different contexts appear to already own each other's mutexes, and ACPICA's own
/// consistency checks operate on a fiction.
///
/// Derived from the running task, with the CPU folded in so two tasks at the same scheduler index
/// on different cores never collide. Zero is impossible by construction — the `+ 1` on both halves
/// is load-bearing, not defensive.
#[no_mangle]
pub unsafe extern "C" fn AcpiOsGetThreadId() -> usize {
    // ⚠️ MSR read, NOT `GS::read_base()`.
    //
    // The x86_64 crate implements `Segment64::read_base` with the `rdgsbase` instruction, which is
    // only valid when CR4.FSGSBASE is set. This kernel does not set it, so that call raised #UD at
    // this exact function on the first boot after the OSL rewrite. `GsBase::read()` goes through
    // IA32_GS_BASE via `rdmsr` and is always available — it is what the rest of the kernel uses.
    use x86_64::registers::model_specific::GsBase;
    // Before per-CPU state exists (early boot) there is exactly one context. Any non-zero constant
    // is correct there, and touching GS-relative state would fault.
    if GsBase::read().as_u64() == 0 {
        return 1;
    }
    let percpu = crate::percpu::current();
    let cpu = percpu.logical_id as usize;
    let task = percpu.scheduler.core_task_idx[cpu % 32];
    ((cpu + 1) << 32) | (task + 1)
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsGetTimer() -> u64 {
    let mut lo: u32;
    let mut hi: u32;
    core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    let tsc = ((hi as u64) << 32) | (lo as u64);
    tsc / 100 
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsSleep(milliseconds: u64) {
    let start = AcpiOsGetTimer();
    let wait_ticks = milliseconds * 10_000;
    while AcpiOsGetTimer() - start < wait_ticks {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsStall(microseconds: u32) {
    let start = AcpiOsGetTimer();
    let wait_ticks = (microseconds as u64) * 10;
    while AcpiOsGetTimer() - start < wait_ticks {
        core::hint::spin_loop();
    }
}

#[no_mangle] pub unsafe extern "C" fn AcpiOsWaitEventsComplete() {}

#[no_mangle] pub unsafe extern "C" fn AcpiOsExecute(_type: u32, _function: *mut c_void, _context: *mut c_void) -> u32 { 0 }

// ==========================================
// 7. INTERRUPTS
// ==========================================

#[no_mangle] pub unsafe extern "C" fn AcpiOsInstallInterruptHandler(_num: u32, _handler: *const c_void, _context: *mut c_void) -> u32 { 0 }
#[no_mangle] pub unsafe extern "C" fn AcpiOsRemoveInterruptHandler(_num: u32, _handler: *const c_void) -> u32 { 0 }

// ==========================================
// 8. DEBUG & OUTPUT
// ==========================================

// ★ `AcpiOsPrintf`/`AcpiOsVprintf` now live in `custom_acpi.c`, NOT here.
//
// They used to be empty Rust stubs with the comment "You can wire this to your print macro later to
// see logs!". Nobody did, and every diagnostic ACPICA has ever emitted was discarded — including the
// one naming why the DSDT failed to load, which cost three power cycles to rediscover from the
// outside. On a machine with NO SERIAL CONSOLE, a silent log channel is not a TODO, it is a blindfold.
//
// They are in C because they are variadic: ACPICA supplies its own `vsnprintf` (utprint.c) and
// `va_list` (acgcc.h), so C formats the message and hands the finished string to `nyx_acpi_log`
// below. Do not re-add Rust definitions of these two — they would be duplicate symbols.

/// Where formatted ACPICA messages land. Called from C, on any CPU, at any interrupt state.
///
/// Deliberately lock-free. This is reached from both boot context and syscall context (IF=0), and a
/// spin lock touched from both sides of the preemption boundary is exactly the wedge documented in
/// the syscall-discipline notes. `panic_screen.rs` is lock-free for the same reason: a diagnostic
/// that can deadlock is worse than one that can interleave.
const ACPI_LOG_CAP: usize = 16 * 1024;
static mut ACPI_LOG: [u8; ACPI_LOG_CAP] = [0; ACPI_LOG_CAP];
static ACPI_LOG_POS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[no_mangle]
pub unsafe extern "C" fn nyx_acpi_log(s: *const core::ffi::c_char) {
    use core::sync::atomic::Ordering;
    if s.is_null() { return; }

    let mut len = 0usize;
    while len < 1024 && *s.add(len) != 0 { len += 1; }
    if len == 0 { return; }

    // Reserve first, then fill. Past the end we simply stop recording rather than wrap: the
    // interesting failures happen during table load, at the very beginning, and a ring buffer would
    // overwrite precisely the part worth reading.
    let start = ACPI_LOG_POS.fetch_add(len, Ordering::Relaxed);
    if start >= ACPI_LOG_CAP { return; }
    let n = len.min(ACPI_LOG_CAP - start);
    core::ptr::copy_nonoverlapping(s as *const u8, ACPI_LOG.as_mut_ptr().add(start), n);
}

/// Whether `AcpiOsVprintf` records anything.
///
/// Open during ACPICA bring-up, closed once the namespace is up. See the comment on
/// `AcpiOsVprintf` in `custom_acpi.c`: ACPICA's runtime warnings fire from inside the AML
/// interpreter, which on Nyx runs in a syscall at IF=0, and adding a formatter to that path
/// crashed `panel` and `battery`. Everything worth reading happens during bring-up.
static ACPI_LOG_OPEN: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

#[no_mangle]
pub extern "C" fn nyx_acpi_log_enabled() -> i32 {
    ACPI_LOG_OPEN.load(core::sync::atomic::Ordering::Relaxed) as i32
}

/// Close the ACPICA log. Called once, after bring-up.
pub fn close_acpi_log() {
    ACPI_LOG_OPEN.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// Leave a one-byte CMOS breadcrumb from C.
///
/// The 64-bit post-mortem detail field is unreliable on this machine (bytes 1 and 3 read back as 0),
/// so this single byte is the only diagnostic that survives a power cycle intact. It is what
/// localised every ACPI failure this session; exposing it to C lets `custom_acpi.c` mark individual
/// calls instead of whole steps.
#[no_mangle]
pub extern "C" fn nyx_mark(v: u8) {
    crate::postmortem::user_mark(v);
}

/// The captured ACPICA log so far.
pub fn acpi_log() -> &'static [u8] {
    use core::sync::atomic::Ordering;
    let n = ACPI_LOG_POS.load(Ordering::Relaxed).min(ACPI_LOG_CAP);
    unsafe { &core::ptr::addr_of!(ACPI_LOG).as_ref().unwrap()[..n] }
}

#[no_mangle] 
pub unsafe extern "C" fn AcpiOsRedirectOutput(_handle: *mut c_void) {}
// ==========================================
// 9. MEMORY CHECKS 
// ==========================================

#[no_mangle] pub unsafe extern "C" fn AcpiOsReadable(_ptr: *mut c_void, _len: u32) -> u8 { 1 }
#[no_mangle] pub unsafe extern "C" fn AcpiOsWritable(_ptr: *mut c_void, _len: u32) -> u8 { 1 }

// ==========================================
// 10. GLIBC STRING & LOCALE GHOSTS
// ==========================================
#[no_mangle]
pub extern "C" fn toupper(c: i32) -> i32 {
    if c >= 'a' as i32 && c <= 'z' as i32 {
        c - ('a' as i32 - 'A' as i32)
    } else {
        c
    }
}

#[no_mangle]
pub extern "C" fn tolower(c: i32) -> i32 {
    if c >= 'A' as i32 && c <= 'Z' as i32 {
        c + ('a' as i32 - 'A' as i32)
    } else {
        c
    }
}

#[no_mangle] pub unsafe extern "C" fn strcmp(mut s1: *const u8, mut s2: *const u8) -> i32 {
    while *s1 != 0 && *s1 == *s2 { s1 = s1.add(1); s2 = s2.add(1); }
    (*s1 as i32) - (*s2 as i32)
}

#[no_mangle] pub unsafe extern "C" fn strncmp(mut s1: *const u8, mut s2: *const u8, mut n: usize) -> i32 {
    while n > 0 && *s1 != 0 && *s1 == *s2 { s1 = s1.add(1); s2 = s2.add(1); n -= 1; }
    if n == 0 { 0 } else { (*s1 as i32) - (*s2 as i32) }
}

#[no_mangle] pub unsafe extern "C" fn strcpy(mut dest: *mut u8, mut src: *const u8) -> *mut u8 {
    let ret = dest;
    while *src != 0 { *dest = *src; dest = dest.add(1); src = src.add(1); }
    *dest = 0;
    ret
}

#[no_mangle] 
pub unsafe extern "C" fn strncpy(mut dest: *mut u8, mut src: *const u8, mut n: usize) -> *mut u8 {
    let ret = dest;
    while n > 0 && *src != 0 { *dest = *src; dest = dest.add(1); src = src.add(1); n -= 1; }
    while n > 0 { *dest = 0; dest = dest.add(1); n -= 1; }
    ret
}

#[no_mangle] pub unsafe extern "C" fn strcat(mut dest: *mut u8, mut src: *const u8) -> *mut u8 {
    let ret = dest;
    while *dest != 0 { dest = dest.add(1); }
    while *src != 0 { *dest = *src; dest = dest.add(1); src = src.add(1); }
    *dest = 0;
    ret
}

#[no_mangle] pub unsafe extern "C" fn __memcpy_chk(dest: *mut u8, src: *const u8, len: usize, _destlen: usize) -> *mut u8 {
    core::ptr::copy_nonoverlapping(src, dest, len);
    dest
}

#[repr(transparent)]
struct SyncPtr<T>(*const T);
unsafe impl<T> Sync for SyncPtr<T> {}

// ★ These three tables were ALL ZEROS, which made every glibc ctype macro answer "no".
//
// That is not a harmless stub. `<ctype.h>` implements `isdigit(c)` as
// `(*__ctype_b_loc())[c] & _ISdigit`, so a zero table means isdigit/isalpha/isspace are
// permanently false and toupper/tolower return 0 for every character.
//
// What it cost: ACPICA's `vsnprintf` parses field widths with `isdigit`. With it stuck false,
// `%-4.4s` never consumed its width, fell through to the `default:` arm, and — crucially —
// **consumed no vararg**. Every later conversion in that format string then read the wrong
// argument, so a `%s` was handed `AslCompilerRevision` (0x20251212, an iasl build date) as a
// char* and the kernel page-faulted at 0x20251212 during ACPI bring-up.
//
// It stayed hidden because `AcpiOsVprintf` was an empty stub, so `vsnprintf` was never called.
// Connecting ACPICA's log lit up a latent bug that had been in the build the whole time.
//
// Root cause one level down: `build.rs` sets `ACPI_USE_SYSTEM_CLIBRARY=0`, but ACPICA tests it
// with `#ifdef`/`#ifndef` — so defining it to 0 means "yes, a system C library is available".
// `accommon.h` then skips `acclib.h`, ACPICA's own `isdigit` macro never appears, and the host
// glibc `<ctype.h>` declaration wins. See the note in build.rs before touching that define.
//
// The bit values below are glibc's, read off the host rather than recalled:
//   upper 0x0100  lower 0x0200  alpha 0x0400  digit 0x0800  xdigit 0x1000
//   space 0x2000  print 0x4000  graph 0x8000  blank 0x0001  cntrl 0x0002
//   punct 0x0004  alnum 0x0008
//
// The tables are indexed from -128 to 255, which is why they are 384 wide and the exported
// pointer is offset by 128.

const fn ctype_b_table() -> [u16; 384] {
    let mut t = [0u16; 384];
    let mut c: usize = 0;
    while c < 128 {
        let ch = c as u8;
        let upper = ch >= b'A' && ch <= b'Z';
        let lower = ch >= b'a' && ch <= b'z';
        let digit = ch >= b'0' && ch <= b'9';
        let hex = digit || (ch >= b'A' && ch <= b'F') || (ch >= b'a' && ch <= b'f');
        let cntrl = ch < 0x20 || ch == 0x7F;
        let space = ch == b' ' || (ch >= 0x09 && ch <= 0x0D);
        let blank = ch == b' ' || ch == 0x09;
        let graph = ch > 0x20 && ch < 0x7F;
        let print = ch >= 0x20 && ch < 0x7F;
        let alpha = upper || lower;
        let punct = graph && !alpha && !digit;

        let mut f = 0u16;
        if upper { f |= 0x0100; }
        if lower { f |= 0x0200; }
        if alpha { f |= 0x0400; }
        if digit { f |= 0x0800; }
        if hex   { f |= 0x1000; }
        if space { f |= 0x2000; }
        if print { f |= 0x4000; }
        if graph { f |= 0x8000; }
        if blank { f |= 0x0001; }
        if cntrl { f |= 0x0002; }
        if punct { f |= 0x0004; }
        if alpha || digit { f |= 0x0008; }

        t[128 + c] = f;
        c += 1;
    }
    t
}

/// `toupper`/`tolower` as glibc's macros expect: a table lookup, not a function call.
const fn ctype_case_table(to_upper: bool) -> [i32; 384] {
    let mut t = [0i32; 384];
    // Negative indices (EOF and friends) map to themselves.
    let mut i: usize = 0;
    while i < 128 {
        t[i] = i as i32 - 128;
        i += 1;
    }
    let mut c: usize = 0;
    while c < 256 {
        let ch = c as u8;
        let mapped = if to_upper && ch >= b'a' && ch <= b'z' {
            (ch - 32) as i32
        } else if !to_upper && ch >= b'A' && ch <= b'Z' {
            (ch + 32) as i32
        } else {
            ch as i32
        };
        t[128 + c] = mapped;
        c += 1;
    }
    t
}

static CTYPE_B: [u16; 384] = ctype_b_table();
static CTYPE_B_PTR: SyncPtr<u16> = SyncPtr(CTYPE_B.as_ptr().wrapping_add(128));
#[no_mangle] pub extern "C" fn __ctype_b_loc() -> *const *const u16 { &CTYPE_B_PTR.0 }

static CTYPE_TOUPPER: [i32; 384] = ctype_case_table(true);
static CTYPE_TOUPPER_PTR: SyncPtr<i32> = SyncPtr(CTYPE_TOUPPER.as_ptr().wrapping_add(128));
#[no_mangle] pub extern "C" fn __ctype_toupper_loc() -> *const *const i32 { &CTYPE_TOUPPER_PTR.0 }

static CTYPE_TOLOWER: [i32; 384] = ctype_case_table(false);
static CTYPE_TOLOWER_PTR: SyncPtr<i32> = SyncPtr(CTYPE_TOLOWER.as_ptr().wrapping_add(128));
#[no_mangle] pub extern "C" fn __ctype_tolower_loc() -> *const *const i32 { &CTYPE_TOLOWER_PTR.0 }

// ==========================================
// 11. ACPICA TABLE OVERRIDES
// ==========================================

#[no_mangle] pub unsafe extern "C" fn AcpiOsTableOverride(_existing_table: *mut c_void, new_table: *mut *mut c_void) -> u32 {
    *new_table = core::ptr::null_mut(); 0
}

#[no_mangle] pub unsafe extern "C" fn AcpiOsPhysicalTableOverride(_existing_table: *mut c_void, new_address: *mut u64, new_table_length: *mut u32) -> u32 {
    *new_address = 0; *new_table_length = 0; 0
}

#[no_mangle] pub unsafe extern "C" fn AcpiOsPredefinedOverride(_init_val: *mut c_void, new_val: *mut *mut c_void) -> u32 {
    *new_val = core::ptr::null_mut(); 0
}

// ==========================================
// 12. ACPICA MISC
// ==========================================

#[no_mangle] pub unsafe extern "C" fn AcpiOsSignal(_function: u32, _info: *mut c_void) -> u32 { 0 }
#[no_mangle] pub unsafe extern "C" fn AcpiOsInitialize() -> u32 { 0 }
#[no_mangle] pub unsafe extern "C" fn AcpiOsTerminate() -> u32 { 0 }

// ==========================================
// 13. ACPICA DEBUGGER
// ==========================================

#[no_mangle] pub unsafe extern "C" fn AcpiDbDumpMethodInfo(_info: *mut c_void, _walk_state: *mut c_void) {}
#[no_mangle] pub unsafe extern "C" fn AcpiDbDisplayArgumentObject(_obj_desc: *mut c_void, _walk_state: *mut c_void) {}
#[no_mangle] pub unsafe extern "C" fn AcpiDbDisplayResultObject(_obj_desc: *mut c_void, _walk_state: *mut c_void) {}
#[no_mangle] pub unsafe extern "C" fn AcpiDbSingleStep(_walk_state: *mut c_void, _op: *mut c_void, _op_class: u32) -> u32 { 0 }
#[no_mangle] pub unsafe extern "C" fn AcpiDbSignalBreakPoint(_walk_state: *mut c_void) {}
#[no_mangle] pub unsafe extern "C" fn AcpiDmDisassemble(_walk_state: *mut c_void, _origin: *mut c_void, _num_opcodes: u32) {}

use core::alloc::Layout;
use alloc::alloc::{alloc, dealloc};

// ==========================================
// C-LIBRARY MEMORY ALLOCATOR (MALLOC/FREE)
// ==========================================
// We allocate 8 extra bytes to store the size of the allocation, 
// so `free()` knows exactly how much memory to deallocate!

#[no_mangle]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    if size == 0 { return core::ptr::null_mut(); }
    let layout = Layout::from_size_align(size + 8, 8).unwrap();
    unsafe {
        let ptr = alloc(layout);
        if ptr.is_null() { return core::ptr::null_mut(); }
        *(ptr as *mut usize) = size; // Store the size at the very beginning
        ptr.add(8) // Return the pointer just after our hidden size tag
    }
}

#[no_mangle]
pub extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() { return; }
    unsafe {
        let actual_ptr = ptr.sub(8); // Step back to read our hidden size tag
        let size = *(actual_ptr as *const usize);
        let layout = Layout::from_size_align(size + 8, 8).unwrap();
        dealloc(actual_ptr, layout);
    }
}

#[no_mangle]
pub extern "C" fn calloc(num: usize, size: usize) -> *mut u8 {
    let total = num * size;
    let ptr = malloc(total);
    if !ptr.is_null() {
        unsafe { core::ptr::write_bytes(ptr, 0, total); } // Zero out the memory
    }
    ptr
}

// ==========================================
// C-LIBRARY STUBS (PRINTING & ASSERTIONS)
// ==========================================

#[no_mangle]
pub static mut stdout: *mut u8 = core::ptr::null_mut();

#[no_mangle]
pub extern "C" fn fflush(_stream: *mut u8) -> i32 { 0 }

#[no_mangle]
pub unsafe extern "C" fn printf(_format: *const u8, _args: ...) -> i32 { 0 } // Silently drop C prints

#[no_mangle]
pub extern "C" fn __assert_fail(_assertion: *const u8, _file: *const u8, _line: u32, _function: *const u8) -> ! {
    panic!("FATAL: lwext4 C-library internal assertion failed!");
}

// ==========================================
// C-LIBRARY ALGORITHMS (QSORT)
// ==========================================
// lwext4 needs to sort directory indexes. Here is a bare-metal bubble sort for C.
#[no_mangle]
pub extern "C" fn qsort(
    base: *mut u8,
    num: usize,
    size: usize,
    compar: extern "C" fn(*const u8, *const u8) -> i32,
) {
    if num < 2 || size == 0 { return; }
    let mut temp = alloc::vec![0u8; size];
    
    for i in 0..num {
        for j in i + 1..num {
            unsafe {
                let a = base.add(i * size);
                let b = base.add(j * size);
                if compar(a, b) > 0 {
                    core::ptr::copy_nonoverlapping(a, temp.as_mut_ptr(), size);
                    core::ptr::copy_nonoverlapping(b, a, size);
                    core::ptr::copy_nonoverlapping(temp.as_ptr(), b, size);
                }
            }
        }
    }
}