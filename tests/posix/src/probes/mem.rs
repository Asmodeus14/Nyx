//! Memory-protection probes.
//!
//! These are the acceptance test for the W^X work. Every one of them asks the same question — *is
//! the protection actually enforced?* — and the only honest way to ask it is to violate the
//! protection and see whether the process survives. That is why they run in forked children.
//!
//! Reading the results: `not enforced` means the syscall returned success and changed nothing, so
//! anything relying on it (a JIT, a stack guard page) is silently unprotected. `enforced` means the
//! child faulted, which is the *desired* outcome.

use crate::raw;
use crate::report::{render_ret, Baseline, Row, Verdict};
use crate::sandbox::{run_isolated, Outcome};

use super::announce;

const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
const PAGE: usize = 4096;

unsafe fn map_anon(prot: usize) -> isize {
    unsafe {
        raw::syscall6(
            raw::SYS_MMAP,
            0,
            PAGE,
            prot,
            MAP_PRIVATE | MAP_ANONYMOUS,
            usize::MAX, // fd = -1
            0,
        )
    }
}

pub fn probe(base: Baseline, rows: &mut Vec<Row>) {
    // --- mmap: verified by the memory actually being usable --------------------------------
    announce("mmap(anon)");
    let addr = unsafe { map_anon(PROT_READ | PROT_WRITE) };
    let mut row = Row::new(9, "mmap anon");
    row.raw = render_ret(addr);
    if base.is_missing(addr) {
        row.verdict = Verdict::Missing;
        rows.push(row);
        return;
    }
    if addr <= 0 {
        row.verdict = Verdict::Fail;
        rows.push(row);
        return;
    }
    // Write and read back — a mapping that is not really there fails here, not at the syscall.
    let ptr = addr as *mut u8;
    unsafe { ptr.write_volatile(0xA5) };
    let usable = unsafe { ptr.read_volatile() } == 0xA5;
    row.verdict = if usable { Verdict::Pass } else { Verdict::Fail };
    if !usable {
        row.note = String::from("returned an address that does not hold what was written to it");
    }
    rows.push(row);

    // --- mprotect: is a read-only page actually read-only? ---------------------------------
    announce("mprotect (write to PROT_READ page)");
    let mut row = Row::new(10, "mprotect");
    let outcome = run_isolated(|| {
        // Child: fresh mapping, make it read-only, then write to it.
        let a = unsafe { map_anon(PROT_READ | PROT_WRITE) };
        if a <= 0 {
            return;
        }
        unsafe { raw::sys3(raw::SYS_MPROTECT, a as usize, PAGE, PROT_READ) };
        // If mprotect is real, this faults and the child dies before writing the sentinel.
        unsafe { (a as *mut u8).write_volatile(0x5A) };
    });
    // The child can only tell us survived/faulted — anything it computed died with it. So the
    // return value for the RAW column is measured here, in the parent, on the parent's own page.
    let mprotect_ret = unsafe { raw::sys3(raw::SYS_MPROTECT, addr as usize, PAGE, PROT_READ) };
    row.raw = render_ret(mprotect_ret);
    row.verdict = match outcome {
        _ if base.is_missing(mprotect_ret) => Verdict::Missing,
        Outcome::Faulted => Verdict::Pass,
        Outcome::Survived => Verdict::Stub,
        Outcome::Timeout => Verdict::Skip,
    };
    row.note = format!("write to a PROT_READ page: {}", outcome.describe());
    rows.push(row);

    // Put it back so the parent's own page stays writable for anything later.
    unsafe { raw::sys3(raw::SYS_MPROTECT, addr as usize, PAGE, PROT_READ | PROT_WRITE) };

    // --- W^X: is a non-executable page actually non-executable? ----------------------------
    //
    // Nyx maps every user page PRESENT|WRITABLE|USER_ACCESSIBLE with no NX bit, so today all
    // userspace memory is RWX and this is expected to report "not enforced". EFER.NXE itself is
    // already on (set in trampoline.asm), so this is a page-flag question, not an MSR one.
    announce("nx-exec (call into a data page)");
    let outcome = run_isolated(|| {
        let a = unsafe { map_anon(PROT_READ | PROT_WRITE) };
        if a <= 0 {
            return;
        }
        // 0xC3 = `ret`. Calling it is harmless if it executes, and faults if NX is enforced.
        unsafe { (a as *mut u8).write_volatile(0xC3) };
        let f: extern "C" fn() = unsafe { core::mem::transmute(a as usize) };
        f();
    });
    let mut row = Row::new(0, "nx-exec");
    row.raw = String::from("-");
    row.verdict = match outcome {
        Outcome::Faulted => Verdict::Pass,
        Outcome::Survived => Verdict::Stub,
        Outcome::Timeout => Verdict::Skip,
    };
    row.note = format!("executing a data page: {}", outcome.describe());
    rows.push(row);

    // --- munmap: verified by the address becoming unusable ---------------------------------
    announce("munmap");
    let unmap_ret = unsafe { raw::sys2(raw::SYS_MUNMAP, addr as usize, PAGE) };
    let outcome = run_isolated(|| {
        // Touch the address the parent just unmapped. If munmap is real this faults.
        unsafe { (addr as *mut u8).write_volatile(0x11) };
    });
    let mut row = Row::new(11, "munmap");
    row.raw = render_ret(unmap_ret);
    row.verdict = match outcome {
        _ if base.is_missing(unmap_ret) => Verdict::Missing,
        Outcome::Faulted => Verdict::Pass,
        Outcome::Survived => Verdict::Stub,
        Outcome::Timeout => Verdict::Skip,
    };
    row.note = format!("touching the unmapped page: {}", outcome.describe());
    rows.push(row);
}
