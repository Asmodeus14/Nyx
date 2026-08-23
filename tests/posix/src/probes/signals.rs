//! Signal delivery — does a handler actually RUN?
//!
//! The other signal rows (`rt_sigaction`, `rt_sigprocmask`, `sigaltstack`) can only report that a
//! syscall returned 0, which is exactly the state that is worse than an honest ENOSYS: a program
//! that registers a handler and is told "fine" will assume it runs. Nothing short of raising a
//! real signal and watching for the side effect can tell those two apart.
//!
//! Everything happens in a forked child. If delivery points the CPU somewhere it cannot return
//! from, the child dies and costs one row — rather than taking the whole probe run with it.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::raw;
use crate::report::{Baseline, Row, Verdict};

use super::announce;

const SIGUSR1: usize = 10;
const SA_RESTORER: u64 = 0x0400_0000;

/// Set by the handler, read after. `AtomicU32` rather than a plain `static mut` because the compiler
/// is entitled to assume nothing changes it between the `kill` and the check — a signal handler is
/// invisible to it, so a non-atomic flag can legally be optimised into a constant `0` and the row
/// would report failure no matter what the kernel did.
static HANDLER_SIG: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_signal(sig: i32) {
    // A store and nothing else. This runs on a frame the KERNEL built, with no libc underneath and
    // no guarantee about what the interrupted code was holding — anything that allocates or takes a
    // lock could deadlock against it.
    HANDLER_SIG.store(sig as u32, Ordering::SeqCst);
}

// x86-64 Linux has no kernel-provided signal trampoline: the C library supplies one through
// `sa_restorer`, and with no libc here we have to be that library.
//
// Its only job is to invoke rt_sigreturn. It must NOT touch the stack — on entry rsp points exactly
// at the frame the kernel built, and rt_sigreturn reads the saved context from there. A push, or a
// compiler-inserted prologue, would move rsp off that frame and the kernel would restore garbage.
// That is why this is raw asm and not a `#[naked]` Rust fn with a body.
core::arch::global_asm!(
    ".globl __nyx_sigrestorer",
    "__nyx_sigrestorer:",
    "mov eax, 15", // SYS_RT_SIGRETURN
    "syscall",
);

extern "C" {
    fn __nyx_sigrestorer();
}

pub fn probe(_base: Baseline, rows: &mut Vec<Row>) {
    announce("signal delivery (handler + kill)");
    let mut row = Row::new(62, "signal delivery");

    let got = crate::sandbox::run_isolated_reporting(|| {
        // struct kernel_sigaction { handler, flags, restorer, mask } — four u64s, no padding.
        let act: [u64; 4] = [
            on_signal as *const () as u64,
            SA_RESTORER,
            __nyx_sigrestorer as *const () as u64,
            0,
        ];
        let r = unsafe {
            raw::sys4(raw::SYS_RT_SIGACTION, SIGUSR1, act.as_ptr() as usize, 0, 8)
        };
        if r < 0 {
            return b'A';
        }
        let pid = unsafe { raw::sys0(raw::SYS_GETPID) };
        if unsafe { raw::sys2(raw::SYS_KILL, pid as usize, SIGUSR1) } < 0 {
            return b'K';
        }
        // Delivery happens on the way out of the `kill` syscall itself, so by the time control is
        // back here the handler has already run and rt_sigreturn has restored us.
        if HANDLER_SIG.load(Ordering::SeqCst) == SIGUSR1 as u32 { b'H' } else { b'N' }
    });

    row.raw = String::from(match got {
        Some(b'H') => "handler ran",
        Some(b'N') => "kill returned 0, handler never ran",
        Some(b'A') => "rt_sigaction refused the handler",
        Some(b'K') => "kill refused",
        Some(_) => "unexpected report byte",
        None => "child died during delivery",
    });

    row.verdict = match got {
        Some(b'H') => Verdict::Pass,
        // The dangerous state: both syscalls claim success and nothing happens.
        Some(b'N') => Verdict::Stub,
        Some(b'A') | Some(b'K') => Verdict::Fail,
        _ => Verdict::Fail,
    };

    row.note = String::from(match got {
        Some(b'N') => "registering a handler succeeds but it is never invoked",
        None => "delivery jumped somewhere the process could not return from",
        _ => "raises SIGUSR1 at itself and checks the handler's side effect",
    });

    rows.push(row);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 0x001;

/// Does a signal BREAK a blocking `poll`? (EINTR)
///
/// The one row that matters for an event loop. Everything else about signals can be true while
/// this is false: delivery only happens on the way *out* of a syscall, so a `poll` that only ends
/// when an fd becomes ready is a `poll` no signal can reach. `kill` a process parked in it and
/// nothing happens — the handler runs whenever the I/O eventually completes, which may be never.
///
/// ★ Structured so it CANNOT hang the machine, because Nyx is tested on bare metal and a wedged
/// probe costs a power cycle. `poll` gets a finite 500 ms timeout, so both outcomes terminate and
/// are told apart by the return value rather than by whether we come back:
///   - `-EINTR` → the signal broke in (PASS)
///   - `0`      → it timed out, meaning nothing interrupted it (the pre-fix behaviour)
/// The budget is deliberate: `run_isolated_reporting` gives up after 40 × 25 ms = 1 s and would
/// report a slower probe as "child died", turning the exact failure under test into the wrong
/// diagnosis.
pub fn probe_eintr(_base: Baseline, rows: &mut Vec<Row>) {
    announce("poll interrupted by signal (EINTR)");
    let mut row = Row::new(7, "poll EINTR");

    let got = crate::sandbox::run_isolated_reporting(|| {
        // A handler is REQUIRED, not incidental: SIGUSR1's default action is Terminate, so with no
        // handler the correct kernel kills us and the row cannot distinguish that from a crash.
        let act: [u64; 4] = [
            on_signal as *const () as u64,
            SA_RESTORER,
            __nyx_sigrestorer as *const () as u64,
            0,
        ];
        if unsafe { raw::sys4(raw::SYS_RT_SIGACTION, SIGUSR1, act.as_ptr() as usize, 0, 8) } < 0 {
            return b'A';
        }

        // A pipe nobody ever writes to: a descriptor guaranteed to stay un-ready, so the only ways
        // out of the poll below are the signal or the timeout.
        let mut fds = [0i32; 2];
        if unsafe { raw::sys1(raw::SYS_PIPE, fds.as_mut_ptr() as usize) } < 0 {
            return b'P';
        }

        let target = unsafe { raw::sys0(raw::SYS_GETPID) };
        let helper = unsafe { raw::sys0(raw::SYS_FORK) };
        if helper < 0 {
            return b'F';
        }
        if helper == 0 {
            // ★ The delay is load-bearing. Signalling immediately would race the parent into
            // `poll`: the signal would be delivered on the way out of some earlier syscall, the
            // handler would run, sigpending would clear, and the poll would then time out — a
            // FAIL for a kernel that is working correctly.
            crate::sandbox::sleep_ms(120);
            unsafe {
                raw::sys2(raw::SYS_KILL, target as usize, SIGUSR1);
                raw::raw_exit(0);
            }
        }

        let mut pfd = PollFd { fd: fds[0], events: POLLIN, revents: 0 };
        let r = unsafe {
            raw::sys3(raw::SYS_POLL, &mut pfd as *mut PollFd as usize, 1, 500)
        };

        unsafe {
            raw::sys1(raw::SYS_CLOSE, fds[0] as usize);
            raw::sys1(raw::SYS_CLOSE, fds[1] as usize);
        }

        match r {
            -4 => b'I',           // EINTR
            0 => b'T',            // timed out: the signal never broke in
            n if n > 0 => b'R',   // a pipe nobody wrote to came back readable
            _ => b'E',
        }
    });

    row.raw = String::from(match got {
        Some(b'I') => "poll returned EINTR",
        Some(b'T') => "poll ran to its timeout, uninterrupted",
        Some(b'R') => "poll reported readiness on an unwritten pipe",
        Some(b'A') => "rt_sigaction refused the handler",
        Some(b'P') => "pipe failed",
        Some(b'F') => "fork failed",
        Some(b'E') => "poll failed with an unexpected errno",
        Some(_) => "unexpected report byte",
        None => "child never reported",
    });

    row.verdict = match got {
        Some(b'I') => Verdict::Pass,
        // Not Fail-by-crash: the call SUCCEEDED, it just could not be interrupted. That is the
        // state an event loop silently hangs in, so it deserves its own name.
        Some(b'T') => Verdict::Stub,
        _ => Verdict::Fail,
    };

    row.note = String::from(match got {
        Some(b'T') => "a signal cannot break a blocking poll — an event loop would never wake",
        _ => "child polls an unwritten pipe while a helper signals it after 120 ms",
    });

    rows.push(row);
}
