//! Running a probe that might fault, without taking the harness down with it.
//!
//! Some questions can only be answered by doing the forbidden thing — you cannot tell a real
//! `mprotect` from a stub without writing to a page you just made read-only. If protection is real
//! that write segfaults, and Nyx kills the offending process (`interrupts.rs:248` terminates on a
//! user-mode fault). So the probe runs in a forked child and reports through a pipe.
//!
//! The signal is the **presence of a sentinel byte**, not the child's exit status: a killed child's
//! status encoding is one more thing that could be wrong on a young kernel, and we are trying to
//! measure the kernel, not trust it.

use crate::raw;

pub enum Outcome {
    /// The child finished the probe and reported back — the operation did NOT fault.
    Survived,
    /// No sentinel arrived: the child died doing it.
    Faulted,
    /// The child neither reported nor was reaped in time. Reported honestly rather than guessed at.
    Timeout,
}

impl Outcome {
    pub fn describe(&self) -> &'static str {
        match self {
            Outcome::Survived => "not enforced",
            Outcome::Faulted => "enforced (faulted)",
            Outcome::Timeout => "child hung",
        }
    }
}

/// Run `probe` in a forked child; return whether it survived.
///
/// `probe` runs in a process that shares this one's address space layout but not its future. It
/// **must not** use `println!`, `std::process::exit`, or anything that allocates or takes a lock:
/// std's stdout sits behind a mutex whose futex word fork copies verbatim, so a child that touches
/// it while the parent held it blocks forever. Raw syscalls only.
pub fn run_isolated<F: FnOnce()>(probe: F) -> Outcome {
    let mut fds = [0i32; 2];
    let rc = unsafe { raw::sys1(raw::SYS_PIPE, fds.as_mut_ptr() as usize) };
    if rc < 0 {
        return Outcome::Timeout; // no pipe, no way to hear back
    }
    let (read_fd, write_fd) = (fds[0] as usize, fds[1] as usize);

    let pid = unsafe { raw::sys0(raw::SYS_FORK) };
    if pid < 0 {
        unsafe {
            raw::sys1(raw::SYS_CLOSE, read_fd);
            raw::sys1(raw::SYS_CLOSE, write_fd);
        }
        return Outcome::Timeout;
    }

    if pid == 0 {
        // --- child ---
        unsafe { raw::sys1(raw::SYS_CLOSE, read_fd) };
        probe();
        // Reaching here means the probe did not fault.
        unsafe {
            raw::raw_write(write_fd, b"K");
            raw::raw_exit(0);
        }
    }

    // --- parent ---
    unsafe { raw::sys1(raw::SYS_CLOSE, write_fd) };

    // Bounded wait. The read end stays open in this process, so a dead child does not necessarily
    // give us EOF — poll for the byte with a sleep between tries instead of blocking forever.
    let mut byte = [0u8; 1];
    let mut outcome = Outcome::Timeout;
    for _ in 0..40 {
        // Reap first: a child that already exited without writing tells us it faulted.
        let mut status: i32 = 0;
        let reaped = unsafe {
            raw::sys4(raw::SYS_WAIT4, pid as usize, &mut status as *mut i32 as usize, 1, 0)
        };

        let n = unsafe { raw::sys3(raw::SYS_READ, read_fd, byte.as_mut_ptr() as usize, 1) };
        if n == 1 {
            outcome = Outcome::Survived;
            break;
        }
        if reaped == pid {
            // Exited, and nothing in the pipe: it died before it could report.
            outcome = Outcome::Faulted;
            break;
        }
        sleep_ms(25);
    }

    unsafe { raw::sys1(raw::SYS_CLOSE, read_fd) };
    // Make sure the child is reaped even on the timeout path, so a long run does not accumulate
    // zombies in the scheduler's task table.
    let mut status: i32 = 0;
    unsafe { raw::sys4(raw::SYS_WAIT4, pid as usize, &mut status as *mut i32 as usize, 1, 0) };

    outcome
}

pub fn sleep_ms(ms: u64) {
    // {tv_sec, tv_nsec} on the stack — a live, correctly-sized local, per the harness's pointer rule.
    let ts: [i64; 2] = [(ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64];
    unsafe { raw::sys2(raw::SYS_NANOSLEEP, ts.as_ptr() as usize, 0) };
}
