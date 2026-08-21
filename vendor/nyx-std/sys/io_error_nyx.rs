//! Nyx errno → `io::ErrorKind` and error strings.
//!
//! This replaces the `generic` module std would otherwise use, which is a stub that reports
//! `ErrorKind::Uncategorized` for every code and the string `"operation successful"` for every
//! errno. Both of those are actively harmful:
//!
//! * `"operation successful (os error 110)"` is what a *timeout* printed — a message that says the
//!   opposite of what happened.
//! * More seriously, code that branches on `e.kind()` silently takes the wrong path. The browser's
//!   stepped fetch treats `ErrorKind::TimedOut` as "no data yet, come back next frame"; with every
//!   kind collapsing to `Uncategorized`, that check was dead and a routine 150 ms quiet period was
//!   read as a hard failure.
//!
//! The kernel returns Linux-compatible negative errnos from every syscall (see
//! `nyx-kernel/src/interrupts.rs`), so the numbering here is Linux's.

use crate::io::ErrorKind;

// Only the codes the kernel actually returns are listed. An unknown code must map to
// `Uncategorized` rather than to a plausible guess — a wrong `ErrorKind` is worse than no kind,
// because callers act on it.
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const EBADF: i32 = 9;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const ENOSPC: i32 = 28;
pub const EPIPE: i32 = 32;
pub const ENAMETOOLONG: i32 = 36;
pub const ENOSYS: i32 = 38;
pub const ENOTEMPTY: i32 = 39;
pub const EADDRINUSE: i32 = 98;
pub const EADDRNOTAVAIL: i32 = 99;
pub const ENETDOWN: i32 = 100;
pub const ENETUNREACH: i32 = 101;
pub const ECONNABORTED: i32 = 103;
pub const ECONNRESET: i32 = 104;
pub const ENOBUFS: i32 = 105;
pub const EISCONN: i32 = 106;
pub const ENOTCONN: i32 = 107;
pub const ETIMEDOUT: i32 = 110;
pub const ECONNREFUSED: i32 = 111;
pub const EHOSTUNREACH: i32 = 113;
pub const EALREADY: i32 = 114;
pub const EINPROGRESS: i32 = 115;

/// There is no thread-local errno on Nyx — every syscall returns its error in the return value —
/// so nothing ever reads this. It exists because `io::Error::last_os_error` is part of the std API.
pub fn errno() -> i32 {
    0
}

/// `EINTR` is what makes std retry a syscall instead of surfacing an error. Getting this wrong
/// turns a signal-interrupted read into a spurious I/O failure.
pub fn is_interrupted(code: i32) -> bool {
    code == EINTR
}

pub fn decode_error_kind(code: i32) -> ErrorKind {
    match code {
        EPERM | EACCES => ErrorKind::PermissionDenied,
        ENOENT | ENXIO | ENODEV => ErrorKind::NotFound,
        ESRCH => ErrorKind::NotFound,
        EINTR => ErrorKind::Interrupted,
        EAGAIN => ErrorKind::WouldBlock,
        ENOMEM | ENOBUFS => ErrorKind::OutOfMemory,
        EEXIST => ErrorKind::AlreadyExists,
        ENOTDIR | EISDIR | EBADF | EINVAL | ENAMETOOLONG => ErrorKind::InvalidInput,
        EBUSY => ErrorKind::ResourceBusy,
        ENOSPC => ErrorKind::StorageFull,
        EPIPE => ErrorKind::BrokenPipe,
        ENOSYS => ErrorKind::Unsupported,
        ENOTEMPTY => ErrorKind::DirectoryNotEmpty,
        EADDRINUSE => ErrorKind::AddrInUse,
        EADDRNOTAVAIL => ErrorKind::AddrNotAvailable,
        ENETDOWN | ENETUNREACH | EHOSTUNREACH => ErrorKind::NetworkUnreachable,
        ECONNABORTED => ErrorKind::ConnectionAborted,
        ECONNRESET => ErrorKind::ConnectionReset,
        ENOTCONN => ErrorKind::NotConnected,
        // The one this module was written for: a stepped read uses a short deadline to ask "is
        // there anything yet?", so this must be distinguishable from a real failure.
        ETIMEDOUT => ErrorKind::TimedOut,
        ECONNREFUSED => ErrorKind::ConnectionRefused,
        EINPROGRESS | EALREADY => ErrorKind::WouldBlock,
        EFAULT | EIO | EMFILE | ENFILE | EISCONN => ErrorKind::Other,
        _ => ErrorKind::Uncategorized,
    }
}

pub fn error_string(errno: i32) -> String {
    let text = match errno {
        0 => "success",
        EPERM => "operation not permitted",
        ENOENT => "no such file or directory",
        ESRCH => "no such process",
        EINTR => "interrupted system call",
        EIO => "input/output error",
        ENXIO => "no such device or address",
        EBADF => "bad file descriptor",
        EAGAIN => "resource temporarily unavailable",
        ENOMEM => "cannot allocate memory",
        EACCES => "permission denied",
        EFAULT => "bad address",
        EBUSY => "device or resource busy",
        EEXIST => "file exists",
        ENODEV => "no such device",
        ENOTDIR => "not a directory",
        EISDIR => "is a directory",
        EINVAL => "invalid argument",
        ENFILE => "too many open files in system",
        EMFILE => "too many open files",
        ENOSPC => "no space left on device",
        EPIPE => "broken pipe",
        ENAMETOOLONG => "file name too long",
        ENOSYS => "function not implemented",
        ENOTEMPTY => "directory not empty",
        EADDRINUSE => "address already in use",
        EADDRNOTAVAIL => "cannot assign requested address",
        ENETDOWN => "network is down",
        ENETUNREACH => "network is unreachable",
        ECONNABORTED => "software caused connection abort",
        ECONNRESET => "connection reset by peer",
        ENOBUFS => "no buffer space available",
        EISCONN => "transport endpoint is already connected",
        ENOTCONN => "transport endpoint is not connected",
        ETIMEDOUT => "connection timed out",
        ECONNREFUSED => "connection refused",
        EHOSTUNREACH => "no route to host",
        EALREADY => "operation already in progress",
        EINPROGRESS => "operation now in progress",
        _ => return format!("unknown error (os error {errno})"),
    };
    text.to_string()
}
