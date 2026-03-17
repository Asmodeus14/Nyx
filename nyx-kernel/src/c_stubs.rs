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

#[no_mangle]
pub unsafe extern "C" fn AcpiOsAllocate(size: u64) -> *mut c_void {
    if size == 0 { return core::ptr::null_mut(); }
    let layout = core::alloc::Layout::from_size_align(size as usize, 8).unwrap();
    alloc::alloc::alloc(layout) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsFree(ptr: *mut c_void) {
    if ptr.is_null() { return; }
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

#[no_mangle] 
pub unsafe extern "C" fn AcpiOsCreateSemaphore(_max: u32, _initial: u32, out: *mut *mut c_void) -> u32 { 
    if !out.is_null() { *out = 1 as *mut c_void; }
    0 
}
#[no_mangle] pub unsafe extern "C" fn AcpiOsDeleteSemaphore(_handle: *mut c_void) -> u32 { 0 }
#[no_mangle] pub unsafe extern "C" fn AcpiOsWaitSemaphore(_handle: *mut c_void, _units: u32, _timeout: u16) -> u32 { 0 }
#[no_mangle] pub unsafe extern "C" fn AcpiOsSignalSemaphore(_handle: *mut c_void, _units: u32) -> u32 { 0 }

#[no_mangle] 
pub unsafe extern "C" fn AcpiOsCreateLock(out: *mut *mut c_void) -> u32 { 
    if !out.is_null() { *out = 1 as *mut c_void; }
    0 
}
#[no_mangle] pub unsafe extern "C" fn AcpiOsDeleteLock(_handle: *mut c_void) {}
#[no_mangle] pub unsafe extern "C" fn AcpiOsAcquireLock(_handle: *mut c_void) -> usize { 0 }
#[no_mangle] pub unsafe extern "C" fn AcpiOsReleaseLock(_handle: *mut c_void, _flags: usize) {}

// ==========================================
// 6. SCHEDULING & TIME
// ==========================================

#[no_mangle] pub unsafe extern "C" fn AcpiOsGetThreadId() -> usize { 1 }

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

#[no_mangle]
pub unsafe extern "C" fn AcpiOsPrintf(_fmt: *const core::ffi::c_char, _: ...) {
    // You can wire this to your print macro later to see logs!
}

#[no_mangle]
pub unsafe extern "C" fn AcpiOsVprintf(_fmt: *const core::ffi::c_char, _args: *mut core::ffi::c_void) {
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

static CTYPE_B: [u16; 384] = [0; 384];
static CTYPE_B_PTR: SyncPtr<u16> = SyncPtr(CTYPE_B.as_ptr().wrapping_add(128));
#[no_mangle] pub extern "C" fn __ctype_b_loc() -> *const *const u16 { &CTYPE_B_PTR.0 }

static CTYPE_TOUPPER: [i32; 384] = [0; 384];
static CTYPE_TOUPPER_PTR: SyncPtr<i32> = SyncPtr(CTYPE_TOUPPER.as_ptr().wrapping_add(128));
#[no_mangle] pub extern "C" fn __ctype_toupper_loc() -> *const *const i32 { &CTYPE_TOUPPER_PTR.0 }

static CTYPE_TOLOWER: [i32; 384] = [0; 384];
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