//! Nyx TLS bring-up — x86-64 variant-II thread-local storage over PT_TLS + arch_prctl(SET_FS).
//!
//! B-β.2b flips nyx from std's `no_threads` TLS arm (single global) to **native** `#[thread_local]`
//! TLS, which the compiler lowers to `fs:`-relative accesses. That requires each thread to have its
//! own TLS block with the FS base pointing at its **thread-control-block (TCB)**. On x86-64 the ABI
//! is *variant II*: the static TLS segment sits just BELOW the TCB, the TCB's first word is a
//! self-pointer (`tcb[0] == &tcb`), and `fs:0` therefore reads that self-pointer. A `#[thread_local]`
//! static at PT_TLS offset `k` (0-based from the segment start) is accessed at `fs:-(tls_size - k)`.
//!
//! This module is shared by the main thread (walks auxv for the PT_TLS phdr at startup) and by
//! `thread::spawn` (reuses the cached template). It uses ONLY raw syscalls + a raw mmap — no heap,
//! no thread-locals — because it runs before the TLS it is setting up exists.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const SYS_MMAP: usize = 9;
const SYS_ARCH_PRCTL: usize = 158;
const ARCH_SET_FS: usize = 0x1002;

// auxv tags (must match nyx-kernel/src/process.rs). The AT_NYX_* tags are NYX-private: our
// linker.ld emits program headers OUTSIDE any PT_LOAD segment and no PT_PHDR, so AT_PHDR is 0 and
// we cannot walk phdrs to find PT_TLS. The kernel already parsed PT_TLS, so it hands us the geometry
// directly via these tags. AT_NYX_TLS_VADDR points at the init image inside the loaded binary.
const AT_NULL: u64 = 0;
const AT_NYX_TLS_VADDR: u64 = 0x7000_0001;
const AT_NYX_TLS_FILESZ: u64 = 0x7000_0002;
const AT_NYX_TLS_MEMSZ: u64 = 0x7000_0003;
const AT_NYX_TLS_ALIGN: u64 = 0x7000_0004;

/// The PT_TLS template describing the initial-image bytes and the segment geometry. Cached at main
/// thread bring-up so `thread::spawn` can build an identical block without re-walking auxv.
#[derive(Clone, Copy)]
pub struct TlsTemplate {
    /// Pointer to the initialized TLS image (the PT_TLS segment's in-memory `p_vaddr`).
    pub image: *const u8,
    /// Bytes of initialized data to copy from `image` (`p_filesz`).
    pub filesz: usize,
    /// Total TLS segment size (`p_memsz`); bytes past `filesz` are the zeroed `.tbss`.
    pub memsz: usize,
    /// Alignment of the TLS segment (`p_align`, at least 1).
    pub align: usize,
}

// SAFETY: `image` points into the process's own mapped, read-only TLS init segment; it is only ever
// read. The template is set once at startup and thereafter immutable, so sharing it across threads
// is sound.
unsafe impl Send for TlsTemplate {}
unsafe impl Sync for TlsTemplate {}

// Cached template. `TEMPLATE_SET` gates validity; `image` is stored as usize in an atomic so this
// needs no lock and no thread-local. (memsz==0 is a legitimate "no PT_TLS" state, handled below.)
static TEMPLATE_IMAGE: AtomicUsize = AtomicUsize::new(0);
static TEMPLATE_FILESZ: AtomicUsize = AtomicUsize::new(0);
static TEMPLATE_MEMSZ: AtomicUsize = AtomicUsize::new(0);
static TEMPLATE_ALIGN: AtomicUsize = AtomicUsize::new(0);
static TEMPLATE_SET: AtomicU64 = AtomicU64::new(0);

#[inline]
unsafe fn mmap_anon(len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        inlateout("rax") SYS_MMAP => ret,
        in("rdi") 0usize,            // addr = 0 -> kernel bump-allocates
        in("rsi") len,
        in("rdx") 0x3usize,          // PROT_READ|PROT_WRITE
        in("r10") 0x22usize,         // MAP_PRIVATE|MAP_ANONYMOUS
        in("r8") (-1isize) as usize, // fd
        in("r9") 0usize,             // offset
        out("rcx") _, out("r11") _, options(nostack),
    );
    ret
}

/// Set this thread's FS base via arch_prctl(ARCH_SET_FS). Public because the spawn trampoline in
/// `sys/thread` must install its (parent-built) TCB as the very first thing it does.
#[inline]
pub unsafe fn set_fs(base: usize) {
    core::arch::asm!(
        "syscall",
        inlateout("rax") SYS_ARCH_PRCTL => _,
        in("rdi") ARCH_SET_FS,
        in("rsi") base,
        out("rcx") _, out("r11") _, options(nostack),
    );
}

/// Build one variant-II TLS block from `tpl` and install it as the current thread's FS base.
///
/// Layout of the mmap'd region (low → high):
///   [ padding for align ][ TLS segment: memsz bytes ][ TCB: 8 bytes ]
/// The FS base is set to the TCB start; `tcb[0]` is a self-pointer. Returns the TCB address (the FS
/// base) so the caller can hand it to `spawn_thread` bookkeeping if needed.
///
/// SAFETY: must run with a valid mmap syscall available and before any `#[thread_local]` access on
/// this thread. `tpl.image[..filesz]` must be readable.
pub unsafe fn install_tls_block(tpl: &TlsTemplate) -> usize {
    let tcb = build_tls_block(tpl);
    set_fs(tcb);
    tcb
}

/// Build a variant-II TLS block from `tpl` but DO NOT set FS — returns the TCB address (the value
/// the owning thread must later pass to `set_fs`). Used by `thread::spawn`: the PARENT builds the
/// worker's block (so the mmap happens on the parent's well-defined allocator state), and the worker
/// trampoline only does `set_fs(tcb)` (FS is per-thread hardware state, must be set on the worker).
///
/// Layout of the mmap'd region (low → high): `[ align slack ][ TLS segment: memsz ][ TCB: 16 ]`.
/// `tcb[0]` is a self-pointer so `fs:0` reads the TCB address.
///
/// SAFETY: mmap must be available; `tpl.image[..filesz]` must be readable.
pub unsafe fn build_tls_block(tpl: &TlsTemplate) -> usize {
    let p_align = tpl.align.max(1);
    let seg = tpl.memsz;

    // OFFSET base: the linker lowers a local-exec `#[thread_local]` at PT_TLS offset `k` to
    // `fs:-(round_up(memsz, p_align) - k)`, rounding with the segment's own p_align. We MUST use the
    // same rounding here or every thread-local lands a few bytes off. `seg_aligned` is that base: the
    // TLS block occupies `[tcb - seg_aligned, tcb)`, so the variable at offset k sits at
    // `tcb - seg_aligned + k`.
    let seg_aligned = (seg + p_align - 1) & !(p_align - 1);

    // TCB (== thread pointer == fs base) alignment: at least 8 for the usize self-pointer write, and
    // at least p_align so that `tcb - seg_aligned` (the block start) stays p_align-aligned.
    let tp_align = p_align.max(16);

    // Region: enough for the segment below the TCB + the 16-byte TCB + slack to align the TCB up.
    let total = seg_aligned + 16 + tp_align;
    let raw = mmap_anon(total);
    if raw <= 0x1000 || raw >= 0xFFFF_FFFF_FFFF_F000 {
        // mmap failed; nothing sane to do this early. Spin-abort.
        core::intrinsics::abort();
    }

    // Place the TCB at the first tp_align-aligned address at or above `raw + seg_aligned`, so the
    // segment `[tcb - seg_aligned, tcb)` fits inside the mapping starting at/after `raw`.
    let tcb = (raw + seg_aligned + tp_align - 1) & !(tp_align - 1);
    let seg_start = tcb - seg_aligned;

    // Copy the initialized image, then zero the .tbss tail.
    if tpl.filesz > 0 {
        core::ptr::copy_nonoverlapping(tpl.image, seg_start as *mut u8, tpl.filesz);
    }
    if seg > tpl.filesz {
        core::ptr::write_bytes((seg_start + tpl.filesz) as *mut u8, 0, seg - tpl.filesz);
    }

    // Variant II: tcb[0] = &tcb (self-pointer). fs:0 must read the TCB address.
    *(tcb as *mut usize) = tcb;
    tcb
}

/// Read the cached template, if the main thread already discovered one. Returns None before
/// `setup_main_tls` runs or if the binary has no PT_TLS segment.
pub fn cached_template() -> Option<TlsTemplate> {
    if TEMPLATE_SET.load(Ordering::Acquire) == 0 {
        return None;
    }
    let memsz = TEMPLATE_MEMSZ.load(Ordering::Relaxed);
    if memsz == 0 {
        return None;
    }
    Some(TlsTemplate {
        image: TEMPLATE_IMAGE.load(Ordering::Relaxed) as *const u8,
        filesz: TEMPLATE_FILESZ.load(Ordering::Relaxed),
        memsz,
        align: TEMPLATE_ALIGN.load(Ordering::Relaxed),
    })
}

/// Walk the SysV auxv (found just past envp on the initial stack) for the PT_TLS geometry, cache the
/// template, and install the main thread's TLS block.
///
/// We read Nyx-private tags AT_NYX_TLS_{VADDR,FILESZ,MEMSZ,ALIGN} (0x7000_00xx) rather than walking
/// program headers via AT_PHDR: the Nyx `linker.ld` emits the phdrs OUTSIDE any PT_LOAD segment and
/// no PT_PHDR, so AT_PHDR is 0 in practice. The kernel already parsed PT_TLS, so it hands the fields
/// directly. (AT_PHDR walking is kept as a fallback for binaries that DO map their phdrs.)
///
/// `stack_at_entry` points at `argc` (i.e. the value of RSP the kernel handed `_start`). Layout:
///   [argc][argv[0..argc]][NULL][envp[0..]][NULL][auxv pairs..][AT_NULL,0]
///
/// SAFETY: `stack_at_entry` must be the untouched entry RSP; called exactly once, before main.
pub unsafe fn setup_main_tls(stack_at_entry: *const usize) {
    // 1. Walk past argc, argv, the argv NULL, envp, and the envp NULL to reach the auxv.
    let argc = *stack_at_entry;
    // argv starts at index 1; there are `argc` argv entries then a NULL terminator.
    let mut p = stack_at_entry.add(1 + argc as usize + 1); // now at envp[0]
    // Skip envp until its NULL terminator.
    while *p != 0 {
        p = p.add(1);
    }
    p = p.add(1); // step over the envp NULL -> first auxv tag

    // 2. Scan auxv pairs (tag, val) for the Nyx-private TLS geometry tags.
    let (mut tls_vaddr, mut tls_filesz, mut tls_memsz, mut tls_align) = (0usize, 0usize, 0usize, 0usize);
    loop {
        let tag = *p as u64;
        let val = *p.add(1);
        match tag {
            AT_NULL => break,
            AT_NYX_TLS_VADDR => tls_vaddr = val,
            AT_NYX_TLS_FILESZ => tls_filesz = val,
            AT_NYX_TLS_MEMSZ => tls_memsz = val,
            AT_NYX_TLS_ALIGN => tls_align = val,
            _ => {}
        }
        p = p.add(2);
    }

    // 3. Resolve the PT_TLS template from the Nyx-private tags (memsz==0 means "no PT_TLS").
    let mut tpl = TlsTemplate { image: core::ptr::null(), filesz: 0, memsz: 0, align: 1 };
    if tls_memsz != 0 {
        tpl = TlsTemplate {
            image: tls_vaddr as *const u8,
            filesz: tls_filesz,
            memsz: tls_memsz,
            align: tls_align.max(1),
        };
    }

    // 4. Cache the template for thread::spawn, then install the main thread's block.
    TEMPLATE_IMAGE.store(tpl.image as usize, Ordering::Relaxed);
    TEMPLATE_FILESZ.store(tpl.filesz, Ordering::Relaxed);
    TEMPLATE_MEMSZ.store(tpl.memsz, Ordering::Relaxed);
    TEMPLATE_ALIGN.store(tpl.align, Ordering::Relaxed);
    TEMPLATE_SET.store(1, Ordering::Release);

    install_tls_block(&tpl);
}

/// C-ABI entry point a nyx std binary's `_start` calls **before** invoking `main`, passing the
/// untouched entry RSP (pointer to `argc`). This installs the main thread's TLS block so the very
/// first `#[thread_local]` access in `rt::init` (`thread::current_id()`) reads a valid `fs:` base.
///
/// Exported unmangled so the app's tiny asm `_start` can `call __nyx_setup_main_tls` without linking
/// against a Rust symbol name. Idempotent-safe is NOT required: `_start` calls it exactly once.
///
/// SAFETY: `stack_at_entry` must be the entry RSP the kernel handed `_start` (pointing at argc),
/// and this must run before any thread-local access on the main thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __nyx_setup_main_tls(stack_at_entry: *const usize) {
    setup_main_tls(stack_at_entry);
}
