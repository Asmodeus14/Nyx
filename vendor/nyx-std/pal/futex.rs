//! Nyx futex primitives — the backend for std's futex-based `Mutex`/`Condvar`/`RwLock`/`Once`/
//! thread-parking. Wired in via `pub mod futex;` in `pal/nyx/mod.rs`, so `crate::sys::futex`
//! (re-exported through `sys/mod.rs: pub use pal::*`) resolves here, exactly like `motor`.
//!
//! Backed by the Nyx kernel's `SYS_FUTEX (202)`. Two important ABI notes vs Linux:
//!   * The kernel takes a **relative** timeout timespec at arg4 (a plain `{tv_sec,tv_nsec}`),
//!     NOT Linux's absolute `FUTEX_WAIT_BITSET` clock. So we pass the Duration straight through.
//!   * The kernel's WAIT loop re-reads `*uaddr` every iteration and returns as soon as it differs
//!     from `expected`; a WAKE just flips waiters Ready. std always mutates the futex word before
//!     waking, so this lock-free wake path is safe.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::sync::atomic::Atomic;
use crate::time::Duration;

/// An atomic for use as a futex that is at least 32-bits but may be larger.
pub type Futex = Atomic<Primitive>;
/// Must be the underlying type of Futex.
pub type Primitive = u32;

/// An atomic for use as a futex that is at least 8-bits but may be larger.
pub type SmallFutex = Atomic<SmallPrimitive>;
/// Must be the underlying type of SmallFutex.
pub type SmallPrimitive = u32;

const SYS_FUTEX: usize = 202;
const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const FUTEX_PRIVATE_FLAG: usize = 128;

#[inline]
unsafe fn futex_call(uaddr: *const Atomic<u32>, op: usize, val: u32, timeout: *const i64) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        inlateout("rax") SYS_FUTEX => ret,
        in("rdi") uaddr as usize,
        in("rsi") op,
        in("rdx") val as usize,
        in("r10") timeout as usize,
        in("r8") 0usize,
        in("r9") 0usize,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

/// Waits for a `futex_wake` operation to wake us.
///
/// Returns directly if the futex doesn't hold the expected value.
///
/// Returns false on timeout, and true in all other cases.
pub fn futex_wait(futex: &Atomic<u32>, expected: u32, timeout: Option<Duration>) -> bool {
    use crate::sync::atomic::Ordering::Relaxed;

    // No need to wait if the value already changed.
    if futex.load(Relaxed) != expected {
        return true;
    }

    // Nyx's FUTEX_WAIT wants a *relative* timespec (or null for "wait forever").
    let ts: Option<[i64; 2]> = timeout.map(|d| [d.as_secs() as i64, d.subsec_nanos() as i64]);
    let ts_ptr = ts.as_ref().map_or(core::ptr::null(), |t| t.as_ptr());

    let r = unsafe {
        futex_call(
            futex as *const Atomic<u32>,
            FUTEX_WAIT | FUTEX_PRIVATE_FLAG,
            expected,
            ts_ptr,
        )
    };
    // Kernel returns 0 when woken / value changed, -110 (ETIMEDOUT) on deadline.
    r != -110
}

/// Wakes up one thread that's blocked on `futex_wait` on this futex.
///
/// Returns true if this actually woke up such a thread, false if none was waiting.
pub fn futex_wake(futex: &Atomic<u32>) -> bool {
    let r = unsafe {
        futex_call(futex as *const Atomic<u32>, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, 1, core::ptr::null())
    };
    r > 0
}

/// Wakes up all threads that are waiting on `futex_wait` on this futex.
pub fn futex_wake_all(futex: &Atomic<u32>) {
    unsafe {
        futex_call(
            futex as *const Atomic<u32>,
            FUTEX_WAKE | FUTEX_PRIVATE_FLAG,
            i32::MAX as u32,
            core::ptr::null(),
        );
    }
}
