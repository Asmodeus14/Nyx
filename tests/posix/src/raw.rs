//! Raw syscall wrappers and errno decoding.
//!
//! Deliberately not going through std: this layer has to report the *exact* `rax` the kernel
//! returned, because that is the only way to tell "syscall does not exist" from "you passed a bad
//! argument". std flattens both into an `io::Error`, and on Nyx those two currently share the same
//! errno anyway (see [`crate::report::Baseline`]).

#![allow(dead_code)]

use core::arch::asm;

// ---------------------------------------------------------------------------------------------
// Syscall numbers (Linux x86_64 numbering, which is what the Nyx kernel uses).
// ---------------------------------------------------------------------------------------------
pub const SYS_READ: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_OPEN: usize = 2;
pub const SYS_CLOSE: usize = 3;
pub const SYS_STAT: usize = 4;
pub const SYS_FSTAT: usize = 5;
pub const SYS_LSEEK: usize = 8;
pub const SYS_MMAP: usize = 9;
pub const SYS_MPROTECT: usize = 10;
pub const SYS_MUNMAP: usize = 11;
pub const SYS_BRK: usize = 12;
pub const SYS_RT_SIGACTION: usize = 13;
pub const SYS_RT_SIGPROCMASK: usize = 14;
pub const SYS_IOCTL: usize = 16;
pub const SYS_ACCESS: usize = 21;
pub const SYS_PIPE: usize = 22;
pub const SYS_SELECT: usize = 23;
pub const SYS_SCHED_YIELD: usize = 24;
pub const SYS_DUP: usize = 32;
pub const SYS_DUP2: usize = 33;
pub const SYS_NANOSLEEP: usize = 35;
pub const SYS_GETPID: usize = 39;
pub const SYS_SOCKET: usize = 41;
pub const SYS_KILL: usize = 62;
pub const SYS_FORK: usize = 57;
pub const SYS_EXIT: usize = 60;
pub const SYS_WAIT4: usize = 61;
pub const SYS_FCNTL: usize = 72;
pub const SYS_GETCWD: usize = 79;
pub const SYS_CHDIR: usize = 80;
pub const SYS_OPENAT: usize = 257;
pub const SYS_RENAME: usize = 82;
pub const SYS_MKDIR: usize = 83;
pub const SYS_RMDIR: usize = 84;
pub const SYS_UNLINK: usize = 87;
pub const SYS_SYMLINK: usize = 88;
pub const SYS_SIGALTSTACK: usize = 131;
pub const SYS_ARCH_PRCTL: usize = 158;
pub const SYS_GETTID: usize = 186;
pub const SYS_FUTEX: usize = 202;
pub const SYS_EPOLL_CREATE: usize = 213;
pub const SYS_GETDENTS64: usize = 217;
pub const SYS_SET_TID_ADDRESS: usize = 218;
pub const SYS_CLOCK_GETTIME: usize = 228;
pub const SYS_EPOLL_WAIT: usize = 232;
pub const SYS_EPOLL_CTL: usize = 233;
pub const SYS_NEWFSTATAT: usize = 262;
pub const SYS_EPOLL_PWAIT: usize = 281;
pub const SYS_EPOLL_CREATE1: usize = 291;
pub const SYS_PIPE2: usize = 293;
pub const SYS_GETRANDOM: usize = 318;
pub const SYS_POLL: usize = 7;
pub const SYS_CLONE: usize = 56;
pub const SYS_EXECVEAT: usize = 322;

/// Nyx-specific syscalls used as an *oracle*: these already work, so they tell us what the correct
/// answer is for the POSIX calls we are probing.
pub const SYS_NYX_DIR_COUNT: usize = 510;
pub const SYS_NYX_DIR_ENTRY: usize = 511;
pub const SYS_NYX_FILE_SIZE: usize = 541;

/// A number that must never be implemented. The first probe of every run calls this to learn what
/// *this* kernel returns for an unknown syscall — see `report::Baseline`.
pub const SYS_NEVER: usize = 57005; // 0xDEAD

// ---------------------------------------------------------------------------------------------
// Invocation
// ---------------------------------------------------------------------------------------------

/// Raw syscall. Returns the kernel's `rax` verbatim, sign-extended — negative means errno.
///
/// # Safety
/// The caller must guarantee every pointer argument is a live, correctly-sized mapping. Nyx's
/// `is_valid_user_ptr` accepts NULL with a non-zero length, and a kernel-mode page fault **panics
/// the whole machine** rather than killing this process. There is no safe way to probe with a bad
/// pointer, so every probe in this harness passes real locals only.
#[inline(never)]
pub unsafe fn syscall6(
    n: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8")  a5,
            in("r9")  a6,
            out("rcx") _,
            out("r11") _,
            clobber_abi("sysv64"),
        );
    }
    ret
}

pub unsafe fn sys0(n: usize) -> isize {
    unsafe { syscall6(n, 0, 0, 0, 0, 0, 0) }
}
pub unsafe fn sys1(n: usize, a1: usize) -> isize {
    unsafe { syscall6(n, a1, 0, 0, 0, 0, 0) }
}
pub unsafe fn sys2(n: usize, a1: usize, a2: usize) -> isize {
    unsafe { syscall6(n, a1, a2, 0, 0, 0, 0) }
}
pub unsafe fn sys3(n: usize, a1: usize, a2: usize, a3: usize) -> isize {
    unsafe { syscall6(n, a1, a2, a3, 0, 0, 0) }
}
pub unsafe fn sys4(n: usize, a1: usize, a2: usize, a3: usize, a4: usize) -> isize {
    unsafe { syscall6(n, a1, a2, a3, a4, 0, 0) }
}

/// `write(fd, buf, len)` with no allocation and no locks — safe to call from a forked child.
///
/// A child must never touch `println!`: std's stdout sits behind a mutex whose futex word is copied
/// verbatim by fork, so if the parent happened to hold it the child would block forever and take the
/// harness down with it.
pub unsafe fn raw_write(fd: usize, buf: &[u8]) -> isize {
    unsafe { sys3(SYS_WRITE, fd, buf.as_ptr() as usize, buf.len()) }
}

/// `_exit` — bypasses every atexit handler and destructor, which is what a forked child wants.
pub unsafe fn raw_exit(code: usize) -> ! {
    unsafe {
        sys1(SYS_EXIT, code);
    }
    // The kernel does not return from exit; this is only here to satisfy `!`.
    loop {
        unsafe { asm!("hlt") };
    }
}

// ---------------------------------------------------------------------------------------------
// errno names
// ---------------------------------------------------------------------------------------------

/// Human name for a negative syscall return. Anything unrecognised is reported numerically rather
/// than guessed at.
pub fn errno_name(ret: isize) -> &'static str {
    if ret >= 0 {
        return "ok";
    }
    match -ret {
        1 => "EPERM",
        2 => "ENOENT",
        3 => "ESRCH",
        4 => "EINTR",
        5 => "EIO",
        9 => "EBADF",
        11 => "EAGAIN",
        12 => "ENOMEM",
        13 => "EACCES",
        14 => "EFAULT",
        16 => "EBUSY",
        17 => "EEXIST",
        20 => "ENOTDIR",
        21 => "EISDIR",
        22 => "EINVAL",
        23 => "ENFILE",
        24 => "EMFILE",
        25 => "ENOTTY",
        28 => "ENOSPC",
        32 => "EPIPE",
        36 => "ENAMETOOLONG",
        38 => "ENOSYS",
        39 => "ENOTEMPTY",
        95 => "ENOTSUP",
        110 => "ETIMEDOUT",
        111 => "ECONNREFUSED",
        _ => "E?",
    }
}
