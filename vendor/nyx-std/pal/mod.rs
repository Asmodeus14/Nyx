//! Nyx PAL — platform primitives for `target_os = "nyx"`.
//!
//! Source of truth lives in the Nyx repo at `vendor/nyx-std/pal/`; `apply.py` copies this into
//! the active rustup rust-src as `library/std/src/sys/pal/nyx/` and adds the `nyx` arm to
//! `sys/pal/mod.rs`. The heavy subsystems (fs/net/thread/…) live under `sys/*`, not here — this
//! module only provides the tiny PAL contract (init/cleanup/abort) plus the process entry point.
#![deny(unsafe_op_in_unsafe_fn)]

use crate::io as std_io;

// Re-exported as `crate::sys::futex` via `sys/mod.rs`'s `pub use pal::*` — the futex-based sync
// primitives (Mutex/Condvar/RwLock/Once/thread_parking) resolve `crate::sys::futex::*` from here.
pub mod futex;

// B-β.2b: variant-II TLS bring-up for native `#[thread_local]` (fs:-relative) storage. Shared by the
// main-thread startup shim and (B-β.2c) `thread::spawn`. Re-exported as `crate::sys::tls`.
pub mod tls;

// Nyx syscall numbers (Linux-compatible where they overlap; see nyx-kernel/src/interrupts.rs).
pub mod sysno {
    pub const WRITE: usize = 1;
    pub const EXIT_GROUP: usize = 231;
    pub const MMAP: usize = 9;
    pub const FUTEX: usize = 202;
    pub const CLOCK_GETTIME: usize = 228;
    pub const NANOSLEEP: usize = 35;
    pub const SCHED_YIELD: usize = 24;
}

/// Raw syscall with up to 6 args (SysV-ish: rax=nr, rdi,rsi,rdx,r10,r8,r9). Returns rax.
#[inline]
pub unsafe fn syscall6(n: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize, a6: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a1, in("rsi") a2, in("rdx") a3,
            in("r10") a4, in("r8") a5, in("r9") a6,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    ret
}

// SAFETY: must be called only once during runtime initialization.
pub unsafe fn init(_argc: isize, _argv: *const *const u8, _sigpipe: u8) {}

// SAFETY: must be called only once during runtime cleanup.
pub unsafe fn cleanup() {}

pub fn unsupported<T>() -> std_io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> std_io::Error {
    std_io::Error::UNSUPPORTED_PLATFORM
}

pub fn abort_internal() -> ! {
    core::intrinsics::abort();
}
