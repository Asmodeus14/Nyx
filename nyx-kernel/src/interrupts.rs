use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use crate::gdt;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::fs;
use crate::gui::{Painter, Rect, Color};
use alloc::format;
use x86_64::VirtAddr;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use crate::scheduler::{FileDescriptor, KernelSocket, SocketKind};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, AtomicU16, Ordering};
use x86_64::registers::model_specific::GsBase;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });
pub static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

// Atomic counter prevents Ephemeral Port exhaustion!
static NEXT_LOCAL_PORT: AtomicU16 = AtomicU16::new(49152);

const EPERM: i64 = -1;
const ENOENT: i64 = -2;
const EINTR: i64 = -4;
const EBADF: i64 = -9;
const EAGAIN: i64 = -11;
const ENOMEM: i64 = -12;
const EACCES: i64 = -13;
const EFAULT: i64 = -14;
const EEXIST: i64 = -17;
const ENOTDIR: i64 = -20;
const EISDIR: i64 = -21;
const EINVAL: i64 = -22;
const EMFILE: i64 = -24;
const ENOSPC: i64 = -28;
const ESPIPE: i64 = -29;
const ESRCH: i64 = -3;
const ERANGE: i64 = -34;
const ENOSYS: i64 = -38;
const ENOTEMPTY: i64 = -39;
const EPIPE: i64 = -32;
const EAFNOSUPPORT: i64 = -97;

#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskInfo {
    pub pid: u64,
    pub cpu_ticks: u64,
    pub state: u8, // 0 = Ready, 1 = Running, 2 = Blocked
    pub name: [u8; 16],
}

#[repr(C)]
pub struct SystemInfo {
    pub current_temp: u8,
    pub active_cooling: u8, // 1 = On, 0 = Off
    pub cpu_fan_rpm: u32,
    pub gpu_fan_rpm: u32,
    pub task_count: u64,
    pub tasks: [TaskInfo; 64],
    /// SMBIOS manufacturer + product, NUL-padded. Shown next to the fan readout: when the fans read
    /// as unknown, the reason is "we have no driver for THIS machine", and naming the machine is what
    /// turns that from a shrug into something actionable.
    pub machine: [u8; 80],
}

use crate::process::FD_MAX;

/// Size of x86_64 `struct stat`, and the offset of the name inside `struct linux_dirent64`.
const STAT_SIZE: usize = 144;
const DIRENT_HEADER: usize = 19; // u64 d_ino + i64 d_off + u16 d_reclen + u8 d_type

/// Read a NUL-terminated C string from userspace, resolved like any other path.
///
/// ★★★ This is the ONLY way a Linux-numbered path syscall may read its path argument.
///
/// There used to be a sibling, `user_path(ptr, len)`, reflecting Nyx's native `(ptr, len)`
/// convention, and the Linux-numbered arms used it. That is not a stylistic difference — it
/// silently changes what every LATER argument means. `stat(path, statbuf)` became
/// `stat(path, len, statbuf)`, so musl's `statbuf` pointer was read as a length and every
/// `stat` from a C program returned EFAULT; `mkdir(path, mode)` read 0o777 as a 511-byte path.
/// The failure surfaces as `std::filesystem` reporting "Bad address" for operations that never
/// touched a bad address, which points the investigation at the filesystem instead of the ABI.
///
/// The two shapes cannot be told apart at runtime — for mkdir, a mode of 0o777 and a length of
/// 511 are the same 64 bits — so there is no compatibility shim, only one convention. Nyx's own
/// std was moved to this one. `open` (arm 2) is the single deliberate exception; see the note there.
///
/// ★ Relative paths are resolved HERE — the single choke point every path syscall funnels through
/// — rather than in individual arms. Resolution used to live only in `open`, so a relative path
/// worked for open() and silently failed for stat/mkdir/unlink/rename. Whether a path works
/// depending on which call you happen to make is undebuggable from the outside.
///
/// Returns an owned String because the resolved path is built, not borrowed.
///
/// ★ Walking to a NUL is precisely the operation that can run off the end of a mapping, and a
/// kernel-mode read of an unmapped user address panics the MACHINE. So it is bounded by PATH_MAX
/// *and* re-checks that the page is present every time it crosses a page boundary — a string
/// starting three bytes before the end of a mapped page is otherwise a one-instruction machine kill.
unsafe fn user_cstr(ptr: u64) -> Option<alloc::string::String> {
    let raw = unsafe { user_cstr_raw(ptr) }?;
    Some(unsafe { resolve_path(&raw) })
}

/// The same read, but WITHOUT cwd resolution.
///
/// Only the `*at()` calls want this. A dirfd-relative path must be joined to the DIRECTORY'S path,
/// so resolving it against the cwd first would silently produce a valid-looking path to the wrong
/// file — the failure mode that made `openat` refuse a real dirfd outright until now.
unsafe fn user_cstr_raw(ptr: u64) -> Option<alloc::string::String> {
    const PATH_MAX: usize = 4096;
    if ptr == 0 { return None; }
    let mut buf: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(64);
    for i in 0..PATH_MAX {
        let a = ptr.checked_add(i as u64)?;
        if i == 0 || (a & 0xFFF) == 0 {
            if !is_valid_user_ptr(a as *const u8, 1) { return None; }
            if !unsafe { crate::memory::user_addr_mapped(a) } { return None; }
        }
        let b = unsafe { (a as *const u8).read_volatile() };
        if b == 0 {
            return Some(alloc::string::String::from(core::str::from_utf8(&buf).ok()?));
        }
        buf.push(b);
    }
    None // no terminator within PATH_MAX: refuse rather than keep walking
}

/// `AT_FDCWD`, as every `*at()` syscall spells it.
const AT_FDCWD: i64 = -100;

/// Resolve an `*at()` pair `(dirfd, path)` to one absolute path.
///
/// POSIX order, and the order matters: an ABSOLUTE path ignores `dirfd` entirely (even a nonsense
/// one), `AT_FDCWD` means resolve against the cwd, and only a relative path with a real `dirfd`
/// consults the descriptor. Checking `dirfd` first would make `openat(-1, "/etc/x")` fail, which
/// is legal and which libc++ relies on.
///
/// Returns `None` if `dirfd` names no open file — the caller turns that into EBADF, not EFAULT.
unsafe fn resolve_at(dirfd: i64, raw: &str) -> Option<alloc::string::String> {
    if raw.starts_with('/') || dirfd == AT_FDCWD {
        return Some(unsafe { resolve_path(raw) });
    }
    // ★ Clone the directory's path out of the fd table BEFORE touching anything else: the same
    // rule fstat documents, since holding a borrow of `tasks` across a VFS call is how the Vec
    // gets reallocated under a live reference.
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return None; }
    if dirfd < 0 || dirfd as usize >= FD_MAX { return None; }
    let dir = match &percpu.scheduler.tasks[curr_idx].fd_table[dirfd as usize] {
        Some(FileDescriptor::File(f)) => f.path.clone(),
        _ => return None, // a pipe or socket is not a directory
    };
    let mut joined = dir;
    if !joined.ends_with('/') { joined.push('/'); }
    joined.push_str(raw);
    // Normalise here rather than leaning on resolve_path: its absolute fast-path returns the
    // string untouched, so "/a/b/../c" would survive as-is.
    Some(normalize_abs(&joined))
}

/// Collapse `.`, `..` and empty components in an already-absolute path.
fn normalize_abs(path: &str) -> alloc::string::String {
    let mut parts: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => { parts.pop(); } // `..` at the root pops nothing: / is its own parent
            other => parts.push(other),
        }
    }
    let mut out = alloc::string::String::from("/");
    for (i, p) in parts.iter().enumerate() {
        if i > 0 { out.push('/'); }
        out.push_str(p);
    }
    out
}

/// Write an x86_64 `struct stat` into userspace.
///
/// The layout is fixed by the ABI, so the offsets are spelled out rather than mirrored in a Rust
/// struct — a `#[repr(C)]` copy would look tidier and would silently drift the day someone adds a
/// field. Everything not backed by real data is zeroed, which is the honest answer: Nyx has no uid,
/// gid, device numbers or link counts, and writing plausible-looking values for them would be
/// inventing facts a caller might act on.
///
/// # Safety
/// `buf` must be a valid, writable user pointer to at least `STAT_SIZE` bytes.
unsafe fn write_stat(buf: *mut u8, st: &crate::vfs::FileStat) {
    unsafe {
        core::ptr::write_bytes(buf, 0, STAT_SIZE);
        core::ptr::write_unaligned(buf.add(0) as *mut u64, 0);                 // st_dev
        core::ptr::write_unaligned(buf.add(8) as *mut u64, st.ino as u64);     // st_ino
        core::ptr::write_unaligned(buf.add(16) as *mut u64, 1);                // st_nlink
        core::ptr::write_unaligned(buf.add(24) as *mut u32, st.mode);          // st_mode
        core::ptr::write_unaligned(buf.add(28) as *mut u32, 0);                // st_uid
        core::ptr::write_unaligned(buf.add(32) as *mut u32, 0);                // st_gid
        core::ptr::write_unaligned(buf.add(40) as *mut u64, 0);                // st_rdev
        core::ptr::write_unaligned(buf.add(48) as *mut i64, st.size as i64);   // st_size
        core::ptr::write_unaligned(buf.add(56) as *mut i64, 4096);             // st_blksize
        core::ptr::write_unaligned(buf.add(64) as *mut i64, ((st.size + 511) / 512) as i64); // st_blocks
        core::ptr::write_unaligned(buf.add(72) as *mut i64, st.atime as i64);  // st_atime
        core::ptr::write_unaligned(buf.add(88) as *mut i64, st.mtime as i64);  // st_mtime
        core::ptr::write_unaligned(buf.add(104) as *mut i64, st.ctime as i64); // st_ctime
    }
}

// ===================== Phase 5: signal delivery =====================

const SA_RESTORER: u64 = 0x0400_0000;
const SIGKILL: usize = 9;
const SIGSTOP: usize = 19;

/// What we push on the user stack before jumping to a handler, and what `rt_sigreturn` reads back.
/// The layout is ours alone — the kernel writes it and the kernel reads it — so it holds exactly
/// the registers `sysretq` needs restored, and nothing more.
#[repr(C)]
#[derive(Clone, Copy)]
struct SigFrame {
    r15: u64, r14: u64, r13: u64, r12: u64, r11: u64, r10: u64, r9: u64, r8: u64,
    rdi: u64, rsi: u64, rbp: u64, rdx: u64, rcx: u64, rbx: u64, rax: u64,
    rip: u64, rflags: u64, rsp: u64, oldmask: u64,
}

/// What SIG_DFL means for a given signal.
enum SigDefault { Terminate, Ignore, Stop }

/// POSIX default dispositions. Only the three outcomes Nyx can distinguish are modelled: the
/// core-dumping signals (SIGQUIT/SIGILL/SIGABRT/SIGFPE/SIGSEGV/SIGBUS/SIGSYS/SIGTRAP/SIGXCPU/
/// SIGXFSZ) terminate like the rest, since there is no core file to write.
///
/// ★ The default for an UNKNOWN signal must be Terminate, not Ignore. Getting that backwards makes
/// an unimplemented signal look like a working one — the program keeps running as though it had
/// been handled, and the divergence surfaces arbitrarily far away.
fn sig_default_action(sig: usize) -> SigDefault {
    match sig {
        // SIGCHLD, SIGURG, SIGWINCH, SIGCONT — the only signals a process ignores by default.
        17 | 23 | 28 | 18 => SigDefault::Ignore,
        // SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU.
        19 | 20 | 21 | 22 => SigDefault::Stop,
        _ => SigDefault::Terminate,
    }
}

/// Deliver one pending, unblocked signal by redirecting the return-to-user.
///
/// Called on the way OUT of the dispatcher, after the syscall's result is already in `frame.rax`,
/// so a handler sees a completed call and `rt_sigreturn` restores that result untouched.
///
/// Mechanically: `sysretq` takes its target from `rcx` and its flags from `r11`, so pointing `rcx`
/// at the handler and swapping `user_rsp` for a freshly built frame IS the delivery. There is no
/// separate "enter userspace" path to hook.
unsafe fn deliver_pending_signal(frame: &mut SyscallStackFrame) {
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return; }
    if GsBase::read().as_u64() == 0 { return; }
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return; }

    loop {
        let (sig, act, oldmask) = {
            let t = &percpu.scheduler.tasks[curr_idx];
            let deliverable = t.sigpending & !t.sigmask;
            if deliverable == 0 { return; }
            // Lowest-numbered first, which is what Linux does and what programs expect.
            let sig = deliverable.trailing_zeros() as usize + 1;
            (sig, t.sigactions[sig], t.sigmask)
        };
        let bit = 1u64 << (sig - 1);
        percpu.scheduler.tasks[curr_idx].sigpending &= !bit;

        let handler = act[0];
        if handler == 1 { continue; }            // SIG_IGN
        if handler == 0 {
            match sig_default_action(sig) {
                SigDefault::Ignore => continue,
                SigDefault::Stop => {
                    // No job control on Nyx: nothing can ever send SIGCONT, so actually stopping
                    // would be an unrecoverable hang dressed up as correctness. Ignoring is the
                    // lesser wrong, and it is logged so it is never a silent no-op.
                    crate::serial_println!("[SIG] {} default=STOP — no job control, ignored",
                        sig as u32);
                    continue;
                }
                SigDefault::Terminate => {
                    crate::serial_println!("[SIG] {} default=TERMINATE — killing pid {}",
                        sig as u32, percpu.scheduler.tasks[curr_idx].pid);

                    // ★ Re-enter the dispatcher as exit_group rather than duplicating its
                    // teardown. That teardown is ~100 lines of fd/socket reclamation, the
                    // shared-address-space sibling scan, clear_child_tid + futex wake, and the
                    // Zombie/Empty decision — every app exit on this system goes through it, and
                    // a second copy would rot out of sync with no test that could tell. This path
                    // is also the FIRST caller that is not a userspace `exit()`, so the risk of
                    // getting a hand-rolled copy subtly wrong is highest exactly here.
                    //
                    // The arm disables interrupts, tears down, and ends in `hlt` forever — it
                    // never returns, so nothing below runs.
                    frame.rax = 231;                        // SYS_EXIT_GROUP
                    // 128+signum is the shell's convention for "died from signal N" and cannot be
                    // confused with a normal small exit status. Nyx's wait4 reports exit_code
                    // directly rather than a Linux wstatus, so an encoded W_IFSIGNALED would be
                    // decoded by nobody.
                    frame.rdi = 128 + sig as u64;
                    syscall_dispatch_inner(frame);
                    return;                                 // defensive; unreachable
                }
            }
        }
        if act[1] & SA_RESTORER == 0 {
            // Linux x86-64 has no kernel-provided trampoline; libc supplies one via sa_restorer.
            // Without it there is no way back from the handler, so refuse rather than jump into a
            // handler that can never return.
            crate::serial_println!("[SIG] {} has no SA_RESTORER — cannot deliver", sig as u32);
            continue;
        }

        let saved = SigFrame {
            r15: frame.r15, r14: frame.r14, r13: frame.r13, r12: frame.r12,
            r11: frame.r11, r10: frame.r10, r9: frame.r9, r8: frame.r8,
            rdi: frame.rdi, rsi: frame.rsi, rbp: frame.rbp, rdx: frame.rdx,
            rcx: frame.rcx, rbx: frame.rbx, rax: frame.rax,
            rip: frame.rcx, rflags: frame.r11, rsp: frame.user_rsp, oldmask,
        };

        // The 128-byte red zone below rsp belongs to the interrupted function and must survive.
        let mut sp = frame.user_rsp.saturating_sub(128) & !0xF;
        sp = sp.saturating_sub(core::mem::size_of::<SigFrame>() as u64) & !0xF;
        let ret_slot = sp.saturating_sub(8);

        // ★ Both checks. is_valid_user_ptr only range-checks, and writing to an unmapped user
        // address from kernel mode panics the MACHINE — a thread with a nearly-exhausted stack
        // would otherwise take the whole box down at the moment it received a signal.
        let span = frame.user_rsp.saturating_sub(ret_slot) as usize;
        if ret_slot == 0
            || !is_valid_user_ptr(ret_slot as *const u8, span)
            || !crate::memory::user_addr_mapped(sp)
            || !crate::memory::user_addr_mapped(ret_slot)
        {
            crate::serial_println!("[SIG] {} undeliverable: user stack unwritable", sig as u32);
            continue;
        }

        (sp as *mut SigFrame).write_volatile(saved);
        // The handler returns into sa_restorer, whose only job is to call rt_sigreturn.
        (ret_slot as *mut u64).write_volatile(act[2]);

        // SysV entry contract: RSP % 16 == 8 on entry, as if reached by `call`. `sp` is
        // 16-aligned, so `sp - 8` satisfies that by construction.
        frame.user_rsp = ret_slot;
        frame.rcx = handler;      // sysretq jumps here
        frame.rdi = sig as u64;   // handler(int signum)
        frame.rsi = 0;            // siginfo — SA_SIGINFO is not supported
        frame.rdx = 0;            // ucontext — likewise
        // Block this signal, plus whatever the action asked for, until rt_sigreturn restores it.
        percpu.scheduler.tasks[curr_idx].sigmask = oldmask | bit | act[3];
        return;
    }
}

// ===================== Phase 4: current working directory =====================

/// The calling task's cwd. Cloned rather than borrowed — callers go on to touch the VFS, and
/// holding a borrow into `scheduler.tasks` across that is the same dangling hazard `poll` documents.
unsafe fn current_cwd() -> alloc::string::String {
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() {
        return alloc::string::String::from("/");
    }
    percpu.scheduler.tasks[curr_idx].cwd.clone()
}

/// Resolve a possibly-relative path against the calling task's cwd, collapsing `.` and `..`.
///
/// Absolute paths are returned untouched. Normalising them too would also strip trailing slashes,
/// and VFS mount resolution is sensitive to those — not a risk worth taking for a case nothing
/// currently produces. The consequence, stated plainly: an ABSOLUTE path containing `..` is still
/// passed through literally and will not resolve. That was already true before this change.
///
/// The `..` collapsing matters because the ext4 driver resolves names literally: it would look for
/// a directory actually called `..`, fail, and report ENOENT — sending the caller to debug a
/// missing file rather than an unresolved path.
unsafe fn resolve_path(path: &str) -> alloc::string::String {
    if path.starts_with('/') {
        return alloc::string::String::from(path);
    }
    let mut joined = current_cwd();
    if !joined.ends_with('/') { joined.push('/'); }
    joined.push_str(path);

    let mut parts: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for c in joined.split('/') {
        match c {
            "" | "." => {}
            // `..` at the root pops nothing, which is what POSIX requires: / is its own parent.
            ".." => { parts.pop(); }
            other => parts.push(other),
        }
    }
    let mut out = alloc::string::String::from("/");
    for (i, p) in parts.iter().enumerate() {
        if i > 0 { out.push('/'); }
        out.push_str(p);
    }
    out
}

// ===================== Phase 2: readiness (poll / select) =====================

const POLLIN: i16 = 0x001;
const POLLOUT: i16 = 0x004;
const POLLHUP: i16 = 0x010;
const POLLNVAL: i16 = 0x020;

/// Largest `nfds` serviced in one call. Bounds the work done per pass and keeps a `select` fd_set
/// to a single 64-bit word. The fd table is 32 entries today, so this is already generous.
const POLL_MAX: usize = 64;

/// Sample readiness for a set of descriptors, retaining nothing.
///
/// Writes one entry per requested fd: `None` if it is not open, else `(readable, writable, hup)`.
///
/// Two properties have to hold at once, and they pull against each other:
///
/// 1. No borrow of `fd_table` may span a yield. The scheduler's `tasks: Vec<Process>` can
///    reallocate while this task is descheduled, so a borrow held across `int 0x41` dangles.
/// 2. Nothing may hold an extra `Arc` reference to a pipe's queue. HUP is derived from
///    `Arc::strong_count == 1`, so any additional reference makes a hung-up pipe look alive.
///
/// The first version satisfied (1) by cloning the descriptor — which broke (2), because the clone
/// was itself a reference and `strong_count` could never reach 1. Pipe HUP was silently
/// unreachable. Taking a short borrow and keeping only the resulting tuple satisfies both: the
/// borrow ends before any yield, and no reference outlives it.
unsafe fn sample_readiness(fds: &[i32], out: &mut [Option<(bool, bool, bool)>]) {
    // Pass 1: find which stacks back these descriptors. `poll_stack` has to run BEFORE readiness is
    // sampled, and exactly once per pass — never per fd.
    let (mut wired, mut wifi) = (false, false);
    {
        let percpu = crate::percpu::current();
        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
        if curr_idx >= percpu.scheduler.tasks.len() {
            for o in out.iter_mut() { *o = None; }
            return;
        }
        let table = &percpu.scheduler.tasks[curr_idx].fd_table;
        for &fd in fds {
            if fd < 0 { continue; }
            if let Some(Some(FileDescriptor::Socket(m))) = table.get(fd as usize) {
                match m.lock().stack {
                    crate::drivers::net::NetStack::Wired => wired = true,
                    crate::drivers::net::NetStack::Wifi => wifi = true,
                }
            }
        }
    }
    if wired { crate::drivers::net::poll_stack(crate::drivers::net::NetStack::Wired); }
    if wifi { crate::drivers::net::poll_stack(crate::drivers::net::NetStack::Wifi); }

    // Pass 2: sample under a fresh short borrow. Nothing here yields.
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() {
        for o in out.iter_mut() { *o = None; }
        return;
    }
    let table = &percpu.scheduler.tasks[curr_idx].fd_table;
    for (i, &fd) in fds.iter().enumerate() {
        out[i] = if fd < 0 {
            None
        } else {
            match table.get(fd as usize) {
                Some(Some(d)) => Some(fd_readiness(d)),
                _ => None,
            }
        };
    }
}

/// What would this descriptor do right now, without actually reading or writing it?
/// Returns `(readable, writable, hup)`.
///
/// Deliberately does NOT poll the network stack — the caller does that once per pass. Polling per
/// fd would re-run the whole smoltcp state machine N times per loop; that is a heat and latency
/// problem this kernel has already been burned by once.
fn fd_readiness(fd: &FileDescriptor) -> (bool, bool, bool) {
    match fd {
        // A regular file never blocks — both directions always complete. POSIX requires both bits.
        FileDescriptor::File(_) => (true, true, false),

        FileDescriptor::PipeRead(q) => {
            let has_data = !q.lock().is_empty();
            // ★ INVARIANT: callers must pass a descriptor BORROWED from the fd table, never a
            // clone. The count reaching 1 means this read end is the last reference — every writer
            // has closed — and any extra reference the caller is holding makes that unreachable,
            // so a hung-up pipe reads as alive forever and a reader never sees EOF. That is exactly
            // the bug an earlier snapshot-by-clone version of `sample_readiness` had.
            let hup = alloc::sync::Arc::strong_count(q) == 1;
            // Readable on HUP too: read() returns 0 at once, which is a completed call, not a block.
            (has_data || hup, false, hup)
        }

        FileDescriptor::PipeWrite(q) => {
            let hup = alloc::sync::Arc::strong_count(q) == 1;
            (false, !hup, hup)
        }

        // Bidirectional, so unlike a pipe end this reports BOTH directions. Readability is decided
        // by `rx` and hangup by `tx`: the peer holds our `tx` as its own `rx`, so `tx` dropping to
        // one reference is what "the peer is gone" means. Reading the wrong queue here would make a
        // half-closed connection look alive, which an IPC event loop would spin on forever.
        FileDescriptor::UnixStream { rx, tx } => {
            let has_data = !rx.lock().is_empty();
            let hup = alloc::sync::Arc::strong_count(tx) == 1;
            (has_data || hup, !hup, hup)
        }

        FileDescriptor::Socket(sock_mtx) => {
            let (kind, stack, gen) = {
                let s = sock_mtx.lock();
                (s.kind.clone(), s.stack, s.gen)
            };
            let r = match kind {
                SocketKind::Udp(h) => crate::drivers::net::with_sockets(stack, gen, |set| {
                    let s = set.get_mut::<smoltcp::socket::udp::Socket>(h);
                    (s.can_recv(), s.can_send(), false)
                }),
                SocketKind::Tcp(h) => crate::drivers::net::with_sockets(stack, gen, |set| {
                    let s = set.get_mut::<smoltcp::socket::tcp::Socket>(h);
                    // A peer that has sent FIN is READABLE: recv() returns 0 immediately. Calling
                    // that "not ready" is precisely how a poll loop hangs on a closed connection.
                    let eof = !s.may_recv();
                    (s.can_recv() || eof, s.can_send(), eof && !s.may_send())
                }),
            };
            // None means the link and its whole SocketSet went away. That is an error, not quiet
            // inactivity, so report hup rather than leave the caller blocking on a dead stack.
            match r { Some(v) => v, None => (false, false, true) }
        }
    }
}

/// Give up the CPU while waiting for readiness to change.
///
/// SYSCALL clears IF, so the `enable()` is not optional: without it this yields with interrupts
/// masked and the timer that would wake us never fires — the machine freezes, mouse included.
/// This is the idiom the socket paths already use; it is copied rather than reinvented.
#[inline]
unsafe fn readiness_yield() {
    x86_64::instructions::interrupts::enable();
    core::arch::asm!("int 0x41");
    x86_64::instructions::interrupts::disable();
}

/// Is a signal waiting that would actually run code (or kill us) if we returned right now?
///
/// ★★★ This is the second legal exit from any blocking loop, and without it there is no `EINTR`
/// on Nyx at all. Signal delivery happens in `syscall_dispatcher` AFTER `syscall_dispatch_inner`
/// returns, so a loop that only ends on "the I/O completed" is a loop **no signal can ever
/// interrupt** — `kill` a process parked in `poll` and nothing happens until the fd wakes up.
/// An event loop is built precisely on being woken by a signal, so this is a prerequisite for one.
///
/// ★ Disposition matters, and checking only `sigpending & !sigmask` is the trap. POSIX interrupts
/// a syscall when a signal is *caught* — an IGNORED signal must not. `SIGCHLD` defaults to ignore
/// and arrives every time a child exits, so the naive test would make every blocking `poll` return
/// a spurious `EINTR` in any program that spawns processes. That reads as random I/O failure.
///
/// Terminating defaults DO interrupt: returning lets the dispatcher run the kill, which is the
/// only way a `SIGTERM` reaches a process parked in `poll`.
unsafe fn signal_would_interrupt() -> bool {
    // Same two guards `deliver_pending_signal` opens with: before percpu is live there is no task
    // to have signals, and reading GsBase-relative state then would fault.
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return false; }
    if GsBase::read().as_u64() == 0 { return false; }
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return false; }
    let t = &percpu.scheduler.tasks[curr_idx];

    let mut deliverable = t.sigpending & !t.sigmask;
    while deliverable != 0 {
        let sig = deliverable.trailing_zeros() as usize + 1;
        deliverable &= !(1u64 << (sig - 1));
        match t.sigactions[sig][0] {
            1 => {}                                   // SIG_IGN — never interrupts
            0 => match sig_default_action(sig) {      // SIG_DFL
                SigDefault::Ignore => {}
                _ => return true,                     // Terminate or Stop
            },
            _ => return true,                         // a real handler will run
        }
    }
    false
}

pub fn is_valid_user_ptr(ptr: *const u8, len: usize) -> bool {
    let start = ptr as u64;
    if start == 0 && len == 0 { return false; }
    if let Some(end) = start.checked_add(len as u64) {
        return end <= 0x0000_7FFF_FFFF_FFFF;
    }
    false
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(pf_handler);
        idt.general_protection_fault.set_handler_fn(gpf_handler);
        idt.invalid_opcode.set_handler_fn(ud_handler);

        // ★★★ An exception vector with NO handler is a MACHINE KILLER: the CPU cannot deliver the
        // exception, so it escalates to #DF and takes the whole box down instead of the one
        // process at fault. Only 3/6/8/13/14 were registered, which was survivable exactly as long
        // as nothing ever raised anything else.
        //
        // musl's strtod raised #MF (vector 16) and double-faulted the machine. The IDT is the
        // wrong place to be selective: these all now kill only the offending process.
        idt.divide_error.set_handler_fn(divide_error_handler);              // 0  #DE
        idt.device_not_available.set_handler_fn(device_not_available_handler); // 7 #NM
        idt.x87_floating_point.set_handler_fn(x87_fp_handler);              // 16 #MF
        idt.simd_floating_point.set_handler_fn(simd_fp_handler);            // 19 #XM
        idt.overflow.set_handler_fn(overflow_handler);                      // 4  #OF
        idt.bound_range_exceeded.set_handler_fn(bound_range_handler);       // 5  #BR
        idt.alignment_check.set_handler_fn(alignment_check_handler);        // 17 #AC
        idt.stack_segment_fault.set_handler_fn(stack_segment_handler);      // 12 #SS
        idt.segment_not_present.set_handler_fn(segment_not_present_handler);// 11 #NP
        
        unsafe {
            idt[0x40].set_handler_addr(VirtAddr::new(timer_interrupt_stub as *const () as u64));
            idt[0x41].set_handler_addr(VirtAddr::new(yield_interrupt_stub as *const () as u64));
            idt[InterruptIndex::Keyboard.as_usize()].set_handler_addr(VirtAddr::new(keyboard_interrupt_stub as *const () as u64));
            idt[InterruptIndex::Mouse.as_usize()].set_handler_addr(VirtAddr::new(mouse_interrupt_stub as *const () as u64));
            
            // REMOVE the old ethernet_interrupt_stub line from inside the unsafe block
        }
        
        // new x86-interrupt handler directly to slot 0x30 (48) outside the unsafe block
        idt[0x30].set_handler_fn(rtl8168_interrupt_handler);
        
        idt
    };
}

pub fn init_idt() { IDT.load(); }

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard = PIC_1_OFFSET + 1,
    Mouse = PIC_2_OFFSET + 4,
}

impl InterruptIndex {
    fn as_usize(self) -> usize { self as usize }
    fn as_u8(self) -> u8 { self as u8 }
}

extern "x86-interrupt" fn breakpoint_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    if (stack_frame.code_segment & 3) == 3 { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
    // Before `panic!`, which formats and takes the serial lock — neither is trustworthy once we
    // are here, and a double fault that cannot announce itself is indistinguishable from a hang.
    crate::postmortem::mark_why(
        crate::postmortem::WHY_DOUBLE_FAULT,
        stack_frame.instruction_pointer.as_u64(),
    );
    crate::serial::bc("\n!!DOUBLE FAULT ip=");
    crate::serial::bc_hex(stack_frame.instruction_pointer.as_u64());
    crate::serial::bc("\n");
    panic!("EXCEPTION: DOUBLE FAULT");
}

extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    let was_user = (stack_frame.code_segment & 3) == 3;
    if was_user { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
    if was_user {
        // A userspace #GP must kill only the offending process, NOT panic the whole kernel.
        crate::serial_println!("\n[GPF] User Process Terminated. err={:#x} IP={:#x}",
            error_code, stack_frame.instruction_pointer.as_u64());
        terminate_current_user_process();
    }
    panic!("EXCEPTION: GPF Error: {} ({:#x})\nIP: {:#x}", error_code, error_code, stack_frame.instruction_pointer.as_u64());
}

// #UD (invalid opcode, vector 6). Rust's `core::intrinsics::abort()` and failed `debug_assert`s lower
// to `ud2`; without this handler a userspace abort escalated to a triple fault and froze the whole
// machine (dead mouse, blank screen). A userspace #UD now kills only that process.
extern "x86-interrupt" fn ud_handler(stack_frame: InterruptStackFrame) {
    let was_user = (stack_frame.code_segment & 3) == 3;
    if was_user { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
    if was_user {
        crate::serial_println!("\n[#UD] User Process Terminated (invalid opcode / abort). IP={:#x}",
            stack_frame.instruction_pointer.as_u64());
        terminate_current_user_process();
    }
    panic!("EXCEPTION: INVALID OPCODE (#UD)\nIP: {:#x}\nCS: {:#x}",
        stack_frame.instruction_pointer.as_u64(), stack_frame.code_segment);
}

/// The CPU exceptions that had no IDT entry at all until musl's `strtod` raised one.
///
/// Every one of these was previously a guaranteed double fault — the CPU cannot deliver an
/// exception with no handler, so it escalates, and the machine dies with no message rather than
/// the offending process dying with one. They share this macro because the handling is identical
/// and the only thing that matters is that NONE of them is missing: being selective about which
/// CPU exceptions to catch is how you end up debugging a silent freeze for a week.
///
/// The kernel arm still panics — a fault of this kind in ring 0 IS fatal — but it now says so.
macro_rules! fatal_user_exception {
    ($name:ident, $label:literal) => {
        extern "x86-interrupt" fn $name(stack_frame: InterruptStackFrame) {
            let was_user = (stack_frame.code_segment & 3) == 3;
            if was_user { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
            if was_user {
                crate::serial_println!("\n[{}] User Process Terminated. IP={:#x}",
                    $label, stack_frame.instruction_pointer.as_u64());
                terminate_current_user_process();
            }
            panic!("EXCEPTION: {}\nIP: {:#x}", $label, stack_frame.instruction_pointer.as_u64());
        }
    };
    ($name:ident, $label:literal, err) => {
        extern "x86-interrupt" fn $name(stack_frame: InterruptStackFrame, error_code: u64) {
            let was_user = (stack_frame.code_segment & 3) == 3;
            if was_user { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
            if was_user {
                crate::serial_println!("\n[{}] User Process Terminated. err={:#x} IP={:#x}",
                    $label, error_code, stack_frame.instruction_pointer.as_u64());
                terminate_current_user_process();
            }
            panic!("EXCEPTION: {} err={:#x}\nIP: {:#x}",
                $label, error_code, stack_frame.instruction_pointer.as_u64());
        }
    };
}

fatal_user_exception!(divide_error_handler,          "#DE divide error");
fatal_user_exception!(device_not_available_handler,  "#NM device not available (FPU)");
fatal_user_exception!(x87_fp_handler,                "#MF x87 floating-point");
fatal_user_exception!(simd_fp_handler,               "#XM SIMD floating-point");
fatal_user_exception!(overflow_handler,              "#OF overflow");
fatal_user_exception!(bound_range_handler,           "#BR bound range exceeded");
fatal_user_exception!(alignment_check_handler,       "#AC alignment check", err);
fatal_user_exception!(stack_segment_handler,         "#SS stack segment fault", err);
fatal_user_exception!(segment_not_present_handler,   "#NP segment not present", err);

/// Tear down the currently-running user process (close FDs, shred its address space, mark Zombie) and
/// hlt-loop until the scheduler switches away. Shared by the page-fault, #GP, and #UD handlers so a
/// crashing userspace program never takes the kernel down with it. Assumes swapgs already ran (we're
/// on the kernel GS). Never returns.
fn terminate_current_user_process() -> ! {
    if GsBase::read().as_u64() != 0 {
        let percpu = crate::percpu::current();
        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
        if curr_idx < percpu.scheduler.tasks.len() {
            let task = &mut percpu.scheduler.tasks[curr_idx];
            // Collect first, free once the fd table is clear — the free path must not run while a
            // borrow of `task` is live (see destroy_socket).
            let mut doomed_sockets = alloc::vec::Vec::new();
            for i in 0..FD_MAX {
                if let Some(crate::scheduler::FileDescriptor::Socket(sock_mtx)) = &task.fd_table[i] {
                    if alloc::sync::Arc::strong_count(sock_mtx) == 1 {
                        let s = sock_mtx.lock();
                        doomed_sockets.push((s.stack, s.gen, s.kind.clone()));
                    }
                }
                task.fd_table[i] = None;
            }
            crate::memory::clear_user_address_space(task.cr3);
            task.state = crate::scheduler::TaskState::Zombie;
            for (stack, gen, kind) in doomed_sockets {
                destroy_socket(stack, gen, kind);
            }
        }
    }
    crate::apic::end_of_interrupt();
    unsafe {
        x86_64::instructions::interrupts::enable();
        loop { core::arch::asm!("hlt") }
    }
}

extern "x86-interrupt" fn pf_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    let was_user = (stack_frame.code_segment & 3) == 3;
    if was_user { unsafe { core::arch::asm!("swapgs", options(nostack)); } }

    // Breadcrumb KERNEL-mode faults only. A user CoW fault happens constantly right after every
    // fork, so marking those would flood the wire and change the timing of the very thing being
    // measured. A kernel-mode fault is the case that kills the machine, and it is supposed to be
    // impossible — so if this ever appears, it is the answer.
    if !was_user {
        // Recorded FIRST: a kernel-mode fault is the one that kills the machine, and CR2 is the
        // single most useful number in this whole investigation — if it comes back 0x40xxxx the
        // load-address theory is confirmed without another boot.
        crate::postmortem::mark_why(
            crate::postmortem::WHY_KERNEL_PF,
            x86_64::registers::control::Cr2::read().as_u64(),
        );
        crate::serial::bc("\n!KPF cr2=");
        crate::serial::bc_hex(x86_64::registers::control::Cr2::read().as_u64());
        crate::serial::bc(" ip=");
        crate::serial::bc_hex(stack_frame.instruction_pointer.as_u64());
        crate::serial::bc("\n");
    }

    let cr2 = x86_64::registers::control::Cr2::read().as_u64();
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    
    // 🚨 COPY-ON-WRITE (CoW) TRAP HANDLER 🚨
    if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) && error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
        unsafe {
            let offset = crate::memory::PHYS_MEM_OFFSET;
            let pml4 = (cr3 + offset) as *mut u64;
            
            let i4 = (cr2 >> 39) & 0x1FF;
            let i3 = (cr2 >> 30) & 0x1FF;
            let i2 = (cr2 >> 21) & 0x1FF;
            let i1 = (cr2 >> 12) & 0x1FF;

            let pml4_entry = *pml4.add(i4 as usize);
            if pml4_entry & 1 != 0 {
                let pml3 = ((pml4_entry & 0x000FFFFF_FFFFF000) + offset) as *mut u64;
                let pml3_entry = *pml3.add(i3 as usize);
                if pml3_entry & 1 != 0 {
                    let pml2 = ((pml3_entry & 0x000FFFFF_FFFFF000) + offset) as *mut u64;
                    let pml2_entry = *pml2.add(i2 as usize);
                    if pml2_entry & 1 != 0 && (pml2_entry & (1 << 7)) == 0 {
                        let pt = ((pml2_entry & 0x000FFFFF_FFFFF000) + offset) as *mut u64;
                        let pt_entry = *pt.add(i1 as usize);
                        
                        // Check if Bit 10 (CoW Flag) is set!
                        if pt_entry & 0x400 != 0 {
                            // 1. Allocate a fresh physical frame
                            if let Some(new_frame) = crate::memory::allocate_frame() {
                                let old_phys = pt_entry & 0x000FFFFF_FFFFF000;
                                let new_phys = new_frame.start_address().as_u64();
                                
                                // 2. Copy the 4KB data from the old frame to the new frame
                                core::ptr::copy_nonoverlapping(
                                    (old_phys + offset) as *const u8,
                                    (new_phys + offset) as *mut u8,
                                    4096
                                );
                                
                                // 3. Update the PTE: Point to new frame, clear CoW bit, set Writable bit
                                let mut new_entry = (pt_entry & !0x000FFFFF_FFFFF000) | new_phys;
                                new_entry &= !0x400; // Clear CoW
                                new_entry |= 1 << 1; // Make Writable
                                *pt.add(i1 as usize) = new_entry;
                                
                                // 4. Flush the specific TLB page and instantly resume the app!
                                core::arch::asm!("invlpg [{}]", in(reg) cr2);
                                
                                if was_user { core::arch::asm!("swapgs", options(nostack)); }
                                return; // 🚨 BYPASS THE CRASH AND RESUME!
                            }
                        }
                    }
                }
            }
        }
    }

    if error_code.contains(PageFaultErrorCode::USER_MODE) {
        crate::serial_println!("\n[SEGFAULT] User Process Terminated. Invalid Memory Access at: {:#x}", cr2);
        terminate_current_user_process();
    } else {
        if !was_user && (stack_frame.code_segment & 3) == 3 { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
        panic!("KERNEL PAGE FAULT\nAddr: {:#x}\nError: {:?}\nIP: {:#x}\nCS: {:#x}\nCR3: {:#x}", 
             cr2, error_code, stack_frame.instruction_pointer.as_u64(), stack_frame.code_segment, cr3);
    }
}

core::arch::global_asm!(r#"
.global timer_interrupt_stub
timer_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15
    mov rax, rsp
    and rsp, -16
    sub rsp, 512
    fxsave [rsp]
    sub rsp, 8
    push rax
    mov rdi, rsp
    call timer_context_switch
    mov rsp, rax
    pop rbx
    add rsp, 8
    fxrstor [rsp]
    mov rsp, rbx
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax
    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.global yield_interrupt_stub
yield_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15
    mov rax, rsp
    and rsp, -16
    sub rsp, 512
    fxsave [rsp]
    sub rsp, 8
    push rax
    mov rdi, rsp
    call yield_context_switch
    mov rsp, rax
    pop rbx
    add rsp, 8
    fxrstor [rsp]
    mov rsp, rbx
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax
    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.global keyboard_interrupt_stub
keyboard_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15
    mov rax, rsp
    and rsp, -16
    sub rsp, 512
    fxsave [rsp]
    sub rsp, 8
    push rax
    mov rdi, rsp
    call keyboard_context_switch
    mov rsp, rax
    pop rbx
    add rsp, 8
    fxrstor [rsp]
    mov rsp, rbx
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax
    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.global mouse_interrupt_stub
mouse_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15
    mov rax, rsp
    and rsp, -16
    sub rsp, 512
    fxsave [rsp]
    sub rsp, 8
    push rax
    mov rdi, rsp
    call mouse_context_switch
    mov rsp, rax
    pop rbx
    add rsp, 8
    fxrstor [rsp]
    mov rsp, rbx
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax
    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.global ethernet_interrupt_stub
ethernet_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15
    mov rax, rsp
    and rsp, -16
    sub rsp, 8
    push rax
    call ethernet_handler_impl
    pop rax
    mov rsp, rax
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax
    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq
"#);



extern "C" { 
    fn timer_interrupt_stub(); 
    fn keyboard_interrupt_stub();
    fn mouse_interrupt_stub();
    fn ethernet_interrupt_stub();
    fn syscall_handler_asm();
    fn yield_interrupt_stub();
}

#[no_mangle]
pub extern "C" fn timer_context_switch(current_rsp: u64) -> u64 {
    crate::apic::end_of_interrupt();
    
    // Safety check: Don't schedule if percpu isn't loaded
    if x86_64::registers::model_specific::GsBase::read().as_u64() == 0 { 
        return current_rsp; 
    }

    // --- THE TRUE WALL CLOCK ---
    crate::time::UPTIME_MS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // ---------------------------
    
    let percpu = crate::percpu::current();
    
    // Increment the tick counter BEFORE we schedule a new task
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx < percpu.scheduler.tasks.len() {
        percpu.scheduler.tasks[curr_idx].cpu_ticks += 1;
    }
    // ------------------------------------

    let new_rsp = percpu.scheduler.schedule(current_rsp);
    
    // Grab the NEXT task that the scheduler just picked
    let next_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if next_idx < percpu.scheduler.tasks.len() {
        let task = &percpu.scheduler.tasks[next_idx];
        let task_stack = task.kernel_stack_top;
        
        unsafe {
            // THE CRITICAL FIX: Swap CR3 to the new task's Address Space!
            let current_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
            if current_cr3 != task.cr3.as_u64() {
                core::arch::asm!("mov cr3, {}", in(reg) task.cr3.as_u64());
            }

            // Update Syscall and Hardware Interrupt Stacks
            let percpu_base = percpu as *const _ as *mut u64;
            *percpu_base = task_stack;
            
            let tss_ptr = percpu.gdt_state.tss as *const _ as *mut x86_64::structures::tss::TaskStateSegment;
            (*tss_ptr).privilege_stack_table[0] = x86_64::VirtAddr::new(task_stack);
        }
    }
    
    new_rsp
}

#[no_mangle]
pub extern "C" fn yield_context_switch(current_rsp: u64) -> u64 {
    // 🚨 NO EOI IS SENT HERE. This prevents APIC corruption! 🚨
    if x86_64::registers::model_specific::GsBase::read().as_u64() == 0 { return current_rsp; }
    
    let percpu = crate::percpu::current();
    let new_rsp = percpu.scheduler.schedule(current_rsp);
    
    let next_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if next_idx < percpu.scheduler.tasks.len() {
        let task = &percpu.scheduler.tasks[next_idx];
        let task_stack = task.kernel_stack_top;
        unsafe {
            let current_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
            if current_cr3 != task.cr3.as_u64() { core::arch::asm!("mov cr3, {}", in(reg) task.cr3.as_u64()); }
            let percpu_base = percpu as *const _ as *mut u64;
            *percpu_base = task_stack;
            let tss_ptr = percpu.gdt_state.tss as *const _ as *mut x86_64::structures::tss::TaskStateSegment;
            (*tss_ptr).privilege_stack_table[0] = x86_64::VirtAddr::new(task_stack);
        }
    }
    new_rsp
}
#[no_mangle]
pub extern "C" fn keyboard_context_switch(current_rsp: u64) -> u64 {
    // 1. Let the driver read the keystroke (This naturally drains port 0x60!)
    keyboard_handler_impl(); 
    
    // 2. SAFE EOI (Fired exactly ONCE!)
    crate::apic::end_of_interrupt(); 
    
    // 3. Human Input Override
    if x86_64::registers::model_specific::GsBase::read().as_u64() != 0 {
        let percpu = crate::percpu::current();
        for task in percpu.scheduler.tasks.iter_mut() {
            if task.state == crate::scheduler::TaskState::Blocked && task.wake_tsc > 0 && task.wake_tsc != u64::MAX {
                task.state = crate::scheduler::TaskState::Ready;
                task.wake_tsc = 0;
            }
        }
    }
    yield_context_switch(current_rsp) 
}

#[no_mangle]
pub extern "C" fn mouse_context_switch(current_rsp: u64) -> u64 {
    // 1. Let the driver read the mouse movement (This naturally drains port 0x60!)
    mouse_handler_impl(); 
    
    // 2. SAFE EOI (Fired exactly ONCE!)
    crate::apic::end_of_interrupt(); 
    
    // 3. Human Input Override
    if x86_64::registers::model_specific::GsBase::read().as_u64() != 0 {
        let percpu = crate::percpu::current();
        for task in percpu.scheduler.tasks.iter_mut() {
            if task.state == crate::scheduler::TaskState::Blocked && task.wake_tsc > 0 && task.wake_tsc != u64::MAX {
                task.state = crate::scheduler::TaskState::Ready;
                task.wake_tsc = 0;
            }
        }
    }
    yield_context_switch(current_rsp) 
}

#[no_mangle]
pub extern "C" fn keyboard_handler_impl() {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::shell::handle_key(scancode);
    // 🚨 EOI REMOVED FROM HERE!
}

#[no_mangle]
pub extern "C" fn mouse_handler_impl() {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let packet_byte: u8 = unsafe { port.read() };
    crate::mouse::handle_interrupt(packet_byte);
    // 🚨 EOI REMOVED FROM HERE!
}

#[no_mangle]
pub extern "C" fn ethernet_handler_impl() {
    if let Some(mut driver_guard) = crate::drivers::net::NET_DRIVER.try_lock() {
        if let Some(driver) = driver_guard.as_mut() { driver.ack_interrupt(); }
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    crate::drivers::net::NETWORK_PENDING.store(true, core::sync::atomic::Ordering::Release);
    crate::apic::end_of_interrupt();
}

pub fn init_syscalls() {
    use x86_64::registers::model_specific::{Efer, EferFlags, Msr};
    use x86_64::registers::rflags::RFlags;

    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    KERNEL_CR3.store(cr3, Ordering::SeqCst);

    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !(1 << 2); 
        cr0 |= (1 << 1) | (1 << 5); 
        core::arch::asm!("mov cr0, {}", in(reg) cr0);

        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= (1 << 9) | (1 << 10); 
        core::arch::asm!("mov cr4, {}", in(reg) cr4);

        Efer::update(|flags| *flags |= EferFlags::SYSTEM_CALL_EXTENSIONS);

        let mut star_msr = Msr::new(0xC0000081);
        star_msr.write((0x20_u64 << 48) | (0x08_u64 << 32)); 

        let mut lstar_msr = Msr::new(0xC0000082);
        lstar_msr.write(syscall_handler_asm as *const () as u64);

        let mut fmask_msr = Msr::new(0xC0000084);
        fmask_msr.write(RFlags::INTERRUPT_FLAG.bits());
    }
}

#[repr(C)]
pub struct SyscallStackFrame {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9:  u64, pub r8:  u64,
    pub rdi: u64, pub rsi: u64, pub rbp: u64, pub rdx: u64,
    pub rcx: u64, pub rbx: u64, pub rax: u64, 
    pub user_rsp: u64, // <--- ADD THIS AT THE BOTTOM
}

core::arch::global_asm!(r#"
.global syscall_handler_asm
syscall_handler_asm:
    swapgs
    mov gs:[8], rsp           
    mov rsp, gs:[0]           
    
    push qword ptr gs:[8]    // <--- PUSH USER RSP INTO THE FRAME
    push rax
    push rbx
    push rcx
    push rdx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rax, rsp
    mov rdi, rsp
    
    and rsp, -16
    sub rsp, 512
    fxsave [rsp]
    
    sub rsp, 8
    push rax
    
    call syscall_dispatcher
    
    pop rax
    add rsp, 8
    
    fxrstor [rsp]
    mov rsp, rax

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rdx
    pop rcx
    pop rbx
    pop rax
    pop qword ptr gs:[8]    // <--- POP IT SAFELY BACK

    mov rsp, gs:[8]
    swapgs
    sysretq
"#);

/// Convert the RTC's packed wall-clock (see rtc::read_packed) into Unix epoch seconds.
/// Packed layout: [7:0]=sec [15:8]=min [23:16]=hour [31:24]=day [39:32]=month [63:40]=year(full).
/// Uses Howard Hinnant's days-from-civil algorithm (proleptic Gregorian, UTC — no timezone).
/// Signed correction applied to CLOCK_REALTIME, set by syscall 550.
///
/// Exists because TLS is only as good as the clock: rustls checks a certificate's validity window
/// against `SystemTime::now()`, so a machine whose RTC is in the past rejects every real
/// certificate as "not valid yet" and no `https://` fetch can ever succeed. That failure names TLS
/// and the network, which is why it costs an afternoon to recognise as a clock problem.
pub static REALTIME_OFFSET_SECS: core::sync::atomic::AtomicI64 =
    core::sync::atomic::AtomicI64::new(0);

/// Unix seconds -> (year, month, day, hour, min, sec) UTC.
///
/// Hinnant's `civil_from_days`, the exact inverse of the `days_from_civil` in `rtc_packed_to_unix`
/// below. They must stay inverses: this one decides what gets WRITTEN to the hardware clock and
/// that one decides what gets read back, so any disagreement between them is a clock that drifts
/// by a fixed amount every time it is set.
fn unix_to_civil(secs: i64) -> (u16, u8, u8, u8, u8, u8) {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u16, m as u8, d as u8, (tod / 3600) as u8, ((tod % 3600) / 60) as u8, (tod % 60) as u8)
}

fn rtc_packed_to_unix(p: u64) -> i64 {
    let sec = (p & 0xFF) as i64;
    let min = ((p >> 8) & 0xFF) as i64;
    let hour = ((p >> 16) & 0xFF) as i64;
    let day = ((p >> 24) & 0xFF) as i64;
    let month = ((p >> 32) & 0xFF) as i64;
    let year = ((p >> 40) & 0xFFFFFF) as i64;
    if year == 0 || month == 0 || day == 0 { return 0; } // RTC not readable yet

    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;                                   // [0, 399]
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;           // [0, 146096]
    let days = era * 146097 + doe - 719468;                    // days since 1970-01-01
    days * 86400 + hour * 3600 + min * 60 + sec
}

#[no_mangle]
pub extern "C" fn syscall_dispatcher(frame: &mut SyscallStackFrame) {
    syscall_dispatch_inner(frame);
    // Wrapped rather than appended to the body: the dispatcher has dozens of early `return`s, and
    // any one of them would skip delivery — a signal that only arrives after *some* syscalls is
    // worse than one that never arrives, because the bug is intermittent.
    unsafe { deliver_pending_signal(frame); }
}

fn syscall_dispatch_inner(frame: &mut SyscallStackFrame) {
    // ── Breadcrumbs (see serial::bc) ──────────────────────────────────────────────────────────
    // Gated to fork/execve only: this is a debugging aid for one specific freeze, and a
    // breadcrumb on every syscall would drown the log. It sits ABOVE the three guards below
    // because two of them return ENOSYS silently — a launch that died there would look
    // identical to a launch that never issued the syscall at all.
    match frame.rax { 57 => crate::serial::bc("\n<57"), 59 => crate::serial::bc("\n<59"), _ => {} }

    if !is_valid_user_ptr(frame.rcx as *const u8, 1) { frame.rcx = 0; }
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { frame.rax = ENOSYS as u64; return; }
    if GsBase::read().as_u64() == 0 { frame.rax = ENOSYS as u64; return; }
    
    let percpu = crate::percpu::current();
    let id = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.r10; 
    let arg5 = frame.r8;
    let arg6 = frame.r9;

    match id {
        0 => { frame.rax = sys_read_internal(arg1 as usize, arg2 as *mut u8, arg3 as usize) as u64; },
        1 => { frame.rax = sys_write_internal(arg1 as usize, arg2 as *const u8, arg3 as usize) as u64; },
        2 => {
            let buf_ptr = arg1 as *const u8;
            let len = arg2 as usize;

            if !is_valid_user_ptr(buf_ptr, len) { frame.rax = EFAULT as u64; return; }

            let path_slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
            if let Ok(raw_path) = core::str::from_utf8(path_slice) {
                // Phase 4: relative paths resolve against the task's cwd, as they now do in every
                // path syscall (user_cstr calls resolve_path too).
                let resolved = unsafe { resolve_path(raw_path) };
                let path = resolved.as_str();
                // ★ This is the ONE surviving path syscall on Nyx's native (ptr, len, flags) shape,
                // and it is safe only because the vendored musl has no __NR_open — arch/x86_64
                // omits number 2 deliberately, so musl lowers open() to openat (arm 257) and can
                // never reach here. Restore that line to musl's table and this arm silently reads
                // a `struct stat`-shaped pointer as a length. Everything else moved to user_cstr.
                const O_CREAT: u64 = 0x40;
                const O_TRUNC: u64 = 0x200;
                let flags = arg3;
                if flags & O_TRUNC != 0 {
                    // Truncate-to-zero: recreate the file. delete_file is a no-op if absent.
                    crate::vfs::VFS.delete_file(path);
                    crate::vfs::VFS.create_file(path);
                } else if flags & O_CREAT != 0 {
                    // Create if missing; harmless if it already exists (driver returns AlreadyExists).
                    crate::vfs::VFS.create_file(path);
                }

                if let Some(vnode) = crate::vfs::VFS.open_path(path) {
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
                    
                    let task = &mut percpu.scheduler.tasks[curr_idx];
                    let mut allocated_fd = -1isize;
                    for i in 3..FD_MAX {
                        if task.fd_table[i].is_none() {
                            task.fd_table[i] = Some(crate::scheduler::FileDescriptor::File(
                                alloc::sync::Arc::new(crate::vfs::OpenFile::new(vnode))
                            ));
                            allocated_fd = i as isize;
                            break;
                        }
                    }
                    frame.rax = allocated_fd as u64; 
                } else { frame.rax = EBADF as u64; } 
            } else { frame.rax = EINVAL as u64; }
        },
        3 => { // SYS_CLOSE
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];

            if arg1 < FD_MAX as u64 {
                // Cleanly tear down TCP sockets to avoid Windows NAT exhaustion!
                //
                // Snapshot, clear the slot, THEN destroy — destroy_socket may yield (see its note).
                // The strong_count test matches the exit path: a socket still reachable through a
                // dup2'd or inherited fd must not be RST out from under the other holder.
                let doomed = match &task.fd_table[arg1 as usize] {
                    Some(FileDescriptor::Socket(m)) if alloc::sync::Arc::strong_count(m) == 1 => {
                        let s = m.lock();
                        Some((s.stack, s.gen, s.kind.clone()))
                    }
                    _ => None,
                };
                task.fd_table[arg1 as usize] = None;
                if let Some((stack, gen, kind)) = doomed { destroy_socket(stack, gen, kind); }
            }
            frame.rax = 0;
        },
        9 => { 
            let addr = arg1 as u64;       
            let size = arg2 as usize;     
            let fd = arg5 as isize;       
            let offset = frame.r9 as usize;     
            
            if size == 0 || size > 0x200_0000 { frame.rax = ENOMEM as u64; return; }
            let num_pages = (size + 0xFFF) / 0x1000;

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            
            let task = &mut percpu.scheduler.tasks[curr_idx];

            if fd == -1 {
                let target_addr = if addr == 0 {
                    let next_addr = task.mmap_bump;
                    task.mmap_bump += (num_pages as u64) * 0x1000;
                    next_addr
                } else { addr };

                // Anonymous memory is data: NX. Anything wanting to execute here has to ask for it
                // through mprotect, which is the whole point of W^X.
                match crate::memory::allocate_user_pages_at(target_addr, num_pages, false) {
                    Ok(mapped_addr) => frame.rax = mapped_addr,
                    Err(_) => frame.rax = ENOMEM as u64,
                }
            } else {
                if fd >= 0 && fd < FD_MAX as isize {
                    if let Some(crate::scheduler::FileDescriptor::File(open_file)) = &task.fd_table[fd as usize] {
                        match open_file.mmap(offset, size){
                            Ok(phys_addr) => {
                                if let Ok(virt_addr) = crate::memory::map_user_mmio(phys_addr, size) {
                                    frame.rax = virt_addr;
                                } else { frame.rax = ENOMEM as u64; }
                            },
                            Err(e) => frame.rax = e as u64,
                        }
                    } else { frame.rax = EBADF as u64; } 
                } else { frame.rax = EBADF as u64; } 
            }
        },
        
        // 10 (SYS_MPROTECT) is implemented further down — it used to be a `frame.rax = 0` stub
        // here, which silently shadowed the real arm because match arms are tried in order.
        12 => { frame.rax = 0; }, // SYS_BRK
        13 => { // SYS_RT_SIGACTION(sig, act, oldact, sigsetsize)
            // struct kernel_sigaction { handler@0, flags@8, restorer@16, mask@24 } — which is
            // exactly the [u64; 4] we store, so no field shuffling is needed.
            let sig = arg1 as usize;
            if sig == 0 || sig > 64 { frame.rax = EINVAL as u64; return; }
            // SIGKILL and SIGSTOP may never be caught or ignored. Accepting a handler for them
            // would let a process make itself unkillable.
            if sig == SIGKILL || sig == SIGSTOP { frame.rax = EINVAL as u64; return; }
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = ESRCH as u64; return; }

            let cur = percpu.scheduler.tasks[curr_idx].sigactions[sig];
            if arg3 != 0 {
                if !is_valid_user_ptr(arg3 as *const u8, 32) { frame.rax = EFAULT as u64; return; }
                unsafe {
                    let p = arg3 as *mut u64;
                    for k in 0..4 { p.add(k).write_unaligned(cur[k]); }
                }
            }
            if arg2 != 0 {
                if !is_valid_user_ptr(arg2 as *const u8, 32) { frame.rax = EFAULT as u64; return; }
                let mut new = [0u64; 4];
                unsafe {
                    let p = arg2 as *const u64;
                    for k in 0..4 { new[k] = p.add(k).read_unaligned(); }
                }
                percpu.scheduler.tasks[curr_idx].sigactions[sig] = new;
            }
            frame.rax = 0;
        },

        14 => { // SYS_RT_SIGPROCMASK(how, set, oldset, sigsetsize)
            const SIG_BLOCK: u64 = 0;
            const SIG_UNBLOCK: u64 = 1;
            const SIG_SETMASK: u64 = 2;
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = ESRCH as u64; return; }
            let old = percpu.scheduler.tasks[curr_idx].sigmask;

            if arg3 != 0 {
                if !is_valid_user_ptr(arg3 as *const u8, 8) { frame.rax = EFAULT as u64; return; }
                unsafe { (arg3 as *mut u64).write_unaligned(old); }
            }
            if arg2 != 0 {
                if !is_valid_user_ptr(arg2 as *const u8, 8) { frame.rax = EFAULT as u64; return; }
                let set = unsafe { (arg2 as *const u64).read_unaligned() };
                let newmask = match arg1 {
                    SIG_BLOCK => old | set,
                    SIG_UNBLOCK => old & !set,
                    SIG_SETMASK => set,
                    _ => { frame.rax = EINVAL as u64; return; }
                };
                // SIGKILL and SIGSTOP can never be blocked, however they were asked for.
                let undeniable = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
                percpu.scheduler.tasks[curr_idx].sigmask = newmask & !undeniable;
            }
            frame.rax = 0;
        },

        15 => { // SYS_RT_SIGRETURN — restore the context saved before the handler ran.
            let sp = frame.user_rsp;
            if !is_valid_user_ptr(sp as *const u8, core::mem::size_of::<SigFrame>())
                || !unsafe { crate::memory::user_addr_mapped(sp) }
            {
                frame.rax = EFAULT as u64;
                return;
            }
            let sf = unsafe { (sp as *const SigFrame).read_volatile() };
            frame.r15 = sf.r15; frame.r14 = sf.r14; frame.r13 = sf.r13; frame.r12 = sf.r12;
            frame.r10 = sf.r10; frame.r9 = sf.r9; frame.r8 = sf.r8;
            frame.rdi = sf.rdi; frame.rsi = sf.rsi; frame.rbp = sf.rbp; frame.rdx = sf.rdx;
            frame.rbx = sf.rbx;
            frame.rcx = sf.rip;       // sysretq returns here
            frame.r11 = sf.rflags;    // ...with these flags
            frame.user_rsp = sf.rsp;
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx < percpu.scheduler.tasks.len() {
                percpu.scheduler.tasks[curr_idx].sigmask = sf.oldmask;
            }
            // ★ rax LAST and unconditional: it carries the interrupted syscall's return value.
            // Every other arm ends by setting rax to its own result; doing that here would
            // overwrite the very thing the interrupted call returned, and the program would see a
            // read() or write() report a wrong length purely because a signal arrived.
            frame.rax = sf.rax;
            return;
        },

        62 => { // SYS_KILL(pid, sig)
            let target = arg1;
            let sig = arg2 as usize;
            if sig > 64 { frame.rax = EINVAL as u64; return; }
            let percpu = crate::percpu::current();
            let mut found = false;
            for t in percpu.scheduler.tasks.iter_mut() {
                if t.pid == target && t.state != crate::scheduler::TaskState::Empty {
                    found = true;
                    // sig 0 is the existence probe: report whether the target is there, send nothing.
                    if sig != 0 { t.sigpending |= 1u64 << (sig - 1); }
                }
            }
            frame.rax = if found { 0 } else { ESRCH as u64 };
        },

        200 | 234 => { // SYS_TKILL(tid, sig) / SYS_TGKILL(tgid, tid, sig)
            // ★ This is the syscall musl's raise() issues, and therefore the one abort() depends
            // on. Without it abort() got ENOSYS and fell through to a_crash() = `hlt`, which is
            // privileged — so every failed assert surfaced as a #GP at an address inside abort()
            // with nothing anywhere pointing at the actual cause. That cost two rounds on the C++
            // gate; libc++ and Ladybird assert constantly, so it would have kept costing.
            //
            // Nyx gives every task its own unique `pid` and has no separate tgid (see getpid/gettid,
            // which return the same value), so a tid IS a pid here and tgkill's first argument has
            // nothing to check against. It is accepted and ignored rather than validated against a
            // thread-group field that does not exist.
            let (tid, sig) = if id == 200 { (arg1, arg2 as usize) } else { (arg2, arg3 as usize) };
            if sig > 64 { frame.rax = EINVAL as u64; return; }

            // Unlike kill(2) this targets exactly ONE task: tids are unique, so a loop that
            // signalled every match would be masking a duplicate-pid bug rather than working.
            let mut found = false;
            for t in percpu.scheduler.tasks.iter_mut() {
                if t.pid == tid && t.state != crate::scheduler::TaskState::Empty {
                    found = true;
                    if sig != 0 { t.sigpending |= 1u64 << (sig - 1); }
                    break;
                }
            }
            frame.rax = if found { 0 } else { ESRCH as u64 };
            // Delivery happens at syscall return (deliver_pending_signal), which is what makes
            // this work for abort(): musl blocks all signals around raise(), so SIGABRT is masked
            // here and lands on the return from the following rt_sigprocmask instead.
        },

        16 => { // SYS_IOCTL 
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            if arg1 < FD_MAX as u64 {
                if let Some(FileDescriptor::File(open_file)) = &task.fd_table[arg1 as usize] {
                    match open_file.ioctl(arg2 as usize, arg3 as usize) {
                        Ok(res) => frame.rax = res as u64,
                        Err(e) => frame.rax = e as u64,
                    }
                } else { 
                    frame.rax = -25isize as u64; // ENOTTY
                }
            } else { frame.rax = EBADF as u64; }
        },
        
        11 => { // SYS_MUNMAP(addr, len)
            // Without this, free() could never return memory to the kernel: musl unmaps any block
            // big enough to have been mmap'd, so a long-running program leaks every large
            // allocation it ever makes. Harmless at hello-world scale, fatal for a browser.
            let addr = arg1 as u64;
            let len = arg2 as usize;

            // POSIX: addr must be page-aligned; length is rounded UP. A misaligned addr is EINVAL
            // rather than silently rounded, because rounding it would unmap a page the caller
            // never named.
            if addr == 0 || (addr & 0xFFF) != 0 || len == 0 { frame.rax = EINVAL as u64; return; }
            if !is_valid_user_ptr(addr as *const u8, len) { frame.rax = EINVAL as u64; return; }

            let num_pages = (len + 0xFFF) / 0x1000;
            crate::memory::unmap_user_pages(addr, num_pages);
            // munmap returns 0 on success even if part of the range was already unmapped —
            // POSIX explicitly permits unmapping a range containing no mappings.
            frame.rax = 0;
        },

        17 | 18 => { // SYS_PREAD64 / SYS_PWRITE64(fd, buf, count, offset)
            // Positional I/O. Only meaningful for a real file: a pipe or socket has no offset to
            // seek to, and POSIX says ESPIPE for exactly that — the same answer lseek already
            // gives them, rather than pretending offset 0 was intended.
            let (fd, buf_ptr, len, off) = (arg1 as usize, arg2, arg3 as usize, arg4 as i64);
            if fd >= FD_MAX { frame.rax = EBADF as u64; return; }
            if off < 0 { frame.rax = EINVAL as u64; return; }
            if !is_valid_user_ptr(buf_ptr as *const u8, len) { frame.rax = EFAULT as u64; return; }
            if len == 0 { frame.rax = 0; return; }

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            // Clone the Arc out before touching the VFS: the same rule fstat documents, since the
            // VFS takes the mount lock and the scheduler's task Vec can move under a live borrow.
            let file = match &percpu.scheduler.tasks[curr_idx].fd_table[fd] {
                Some(FileDescriptor::File(f)) => f.clone(),
                Some(_) => { frame.rax = ESPIPE as u64; return; }
                None => { frame.rax = EBADF as u64; return; }
            };

            frame.rax = if id == 17 {
                let dst = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len) };
                file.read_at(off as usize, dst) as u64
            } else {
                let src = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
                file.write_at(off as usize, src) as u64
            };
        },

        19 => { // SYS_READV(fd, iov, iovcnt)
            // musl's __stdio_read issues readv, so WITHOUT this every buffered read fails while
            // writes work (writev was implemented) — which presents as "the file contents differ"
            // rather than as a missing syscall. Deliberately the mirror image of writev below;
            // a second shape for the same operation is how the two drift apart.
            let fd = arg1 as usize;
            let iov_ptr = arg2 as *const u64;
            let iovcnt = arg3 as usize;

            if !iov_array_ok(iov_ptr, iovcnt) { frame.rax = EFAULT as u64; return; }

            let mut total_read = 0isize;
            for i in 0..iovcnt {
                unsafe {
                    let base = *iov_ptr.add(i * 2);
                    let len = *iov_ptr.add(i * 2 + 1) as usize;

                    if len > 0 {
                        let got = sys_read_internal(fd, base as *mut u8, len);
                        if got < 0 {
                            if total_read == 0 { total_read = got; }
                            break;
                        }
                        total_read += got;
                        // A short read ends the call. Continuing into the next iovec would
                        // silently stitch together non-contiguous file data, and the caller has
                        // no way to detect that from the returned count.
                        if (got as usize) < len { break; }
                    }
                }
            }
            frame.rax = total_read as u64;
        },

        20 => { // SYS_WRITEV
            let fd = arg1 as usize;
            let iov_ptr = arg2 as *const u64;
            let iovcnt = arg3 as usize;

            if !iov_array_ok(iov_ptr, iovcnt) { frame.rax = EFAULT as u64; return; }

            let mut total_written = 0isize;
            for i in 0..iovcnt {
                unsafe {
                    let base = *iov_ptr.add(i * 2);
                    let len = *iov_ptr.add(i * 2 + 1) as usize;
                    
                    if len > 0 {
                        let written = sys_write_internal(fd, base as *const u8, len);
                        if written < 0 {
                            if total_written == 0 { total_written = written; }
                            break;
                        }
                        total_written += written;
                    }
                }
            }
            frame.rax = total_written as u64;
        },
        
        53 => { // SYS_SOCKETPAIR(domain, type, protocol, sv)
            // The IPC primitive every multi-process program uses: make a connected pair, then fork
            // so each side inherits one end. Ladybird's chrome/WebContent split is exactly this.
            const AF_UNIX: u64 = 1;
            if arg1 != AF_UNIX {
                // Only AF_UNIX pairs exist here. Refusing loudly matters: `sys_socket` used to
                // ignore its domain entirely and hand back a TCP socket for ANY family, so an
                // AF_UNIX request silently became a network socket that never connected.
                frame.rax = EAFNOSUPPORT as u64;
                return;
            }
            let sv = arg4 as *mut i32;
            if !is_valid_user_ptr(sv as *const u8, 8) { frame.rax = EFAULT as u64; return; }

            let q_a = alloc::sync::Arc::new(spin::Mutex::new(alloc::collections::VecDeque::<u8>::new()));
            let q_b = alloc::sync::Arc::new(spin::Mutex::new(alloc::collections::VecDeque::<u8>::new()));

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];

            let (mut fd0, mut fd1) = (-1i32, -1i32);
            for i in 3..FD_MAX {
                if task.fd_table[i].is_none() {
                    if fd0 == -1 { fd0 = i as i32; } else { fd1 = i as i32; break; }
                }
            }
            if fd0 == -1 || fd1 == -1 { frame.rax = EMFILE as u64; return; }

            // ★ The SWAP is the connection: what one end writes, the other reads.
            task.fd_table[fd0 as usize] = Some(FileDescriptor::UnixStream {
                rx: q_a.clone(), tx: q_b.clone(),
            });
            task.fd_table[fd1 as usize] = Some(FileDescriptor::UnixStream {
                rx: q_b, tx: q_a,
            });
            unsafe { *sv.add(0) = fd0; *sv.add(1) = fd1; }
            frame.rax = 0;
        },

        22 | 293 => { // SYS_PIPE / SYS_PIPE2
            // pipe2's flags (O_CLOEXEC, O_NONBLOCK) are accepted and ignored: Nyx has no exec-time
            // fd closing and pipes are already non-blocking on the read side. Refusing the call
            // outright would block every libc that only knows pipe2, for no gain.
            let fd_array_ptr = arg1 as *mut i32;
            if !is_valid_user_ptr(fd_array_ptr as *const u8, 8) { frame.rax = EFAULT as u64; return; }
            
            let pipe = alloc::sync::Arc::new(spin::Mutex::new(alloc::collections::VecDeque::<u8>::new()));

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            let mut read_fd = -1;
            let mut write_fd = -1;

            for i in 3..FD_MAX {
                if task.fd_table[i].is_none() {
                    if read_fd == -1 { read_fd = i as i32; }
                    else if write_fd == -1 { write_fd = i as i32; break; }
                }
            }

            if read_fd != -1 && write_fd != -1 {
                task.fd_table[read_fd as usize] = Some(crate::scheduler::FileDescriptor::PipeRead(pipe.clone()));
                task.fd_table[write_fd as usize] = Some(crate::scheduler::FileDescriptor::PipeWrite(pipe));
                unsafe {
                    *fd_array_ptr.add(0) = read_fd;
                    *fd_array_ptr.add(1) = write_fd;
                }
                frame.rax = 0;
            } else { frame.rax = EMFILE as u64; }
        },

        33 => { // SYS_DUP2
            let oldfd = arg1 as usize;
            let newfd = arg2 as usize;

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            if oldfd < FD_MAX && newfd < FD_MAX {
                if let Some(fd_obj) = task.fd_table[oldfd].clone() {
                    task.fd_table[newfd] = Some(fd_obj);
                    frame.rax = newfd as u64;
                } else { frame.rax = EBADF as u64; }
            } else { frame.rax = EBADF as u64; }
        },

        // ═══════════════════════════════════════════════════════════════════════════════════════
        // Phase 1 — the POSIX filesystem floor.
        //
        // Every arm here is a thin cover over `vfs.rs`, which in turn covers lwext4. Nothing new is
        // invented; these are the calls a libc reaches for on its first day and that Nyx simply
        // never claimed.
        //
        // **Path convention.** Paths are `(ptr, len)` — a slice, not a NUL-terminated C string —
        // because that is what `open(2)` has always been on Nyx and a kernel with two conventions
        // at once is worse than a kernel with one unusual one. Converting everything to C strings is
        // a single atomic change owed to the musl port, not something to do half of now.
        // ═══════════════════════════════════════════════════════════════════════════════════════

        8 => { // SYS_LSEEK(fd, offset, whence) -> new absolute offset
            const SEEK_SET: u64 = 0;
            const SEEK_CUR: u64 = 1;
            const SEEK_END: u64 = 2;

            let fd = arg1 as usize;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];
            if fd >= FD_MAX { frame.rax = EBADF as u64; return; }

            match &task.fd_table[fd] {
                Some(FileDescriptor::File(open_file)) => {
                    // SEEK_END needs the size, and file_size takes the VFS lock — resolve it BEFORE
                    // taking the offset lock, never inside it.
                    let size = crate::vfs::VFS.file_size(&open_file.path).unwrap_or(0) as i64;
                    let mut off = open_file.offset.lock();
                    let base = match arg3 {
                        SEEK_SET => 0i64,
                        SEEK_CUR => *off as i64,
                        SEEK_END => size,
                        _ => { frame.rax = EINVAL as u64; return; }
                    };
                    let target = base.saturating_add(arg2 as i64);
                    // Seeking past EOF is legal (that is how sparse files are made); seeking to a
                    // negative offset is not.
                    if target < 0 { frame.rax = EINVAL as u64; return; }
                    *off = target as usize;
                    frame.rax = target as u64;
                }
                // ESPIPE is the specific, expected answer for a pipe or socket. Reporting EBADF
                // instead would send a libc looking for a closed descriptor.
                Some(_) => frame.rax = ESPIPE as u64,
                None => frame.rax = EBADF as u64,
            }
        },

        4 | 6 => { // SYS_STAT / SYS_LSTAT(path_cstr, statbuf)
            // lstat is an ALIAS, not an implementation: the VFS resolves symlinks down in the
            // driver and exposes no no-follow variant, so there is nothing to distinguish. The
            // alternative — ENOSYS — makes std::filesystem unusable, because remove_all and
            // directory_iterator reach for symlink_status on every entry they touch. That is the
            // same choice arm 262 already makes by ignoring AT_SYMLINK_NOFOLLOW, so aliasing here
            // is consistency rather than a new lie. It IS a lie for a tree containing symlinks:
            // a walker relying on not following one will follow it.
            let path = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = path.as_str();
            let buf = arg2 as *mut u8;
            if !is_valid_user_ptr(buf, STAT_SIZE) { frame.rax = EFAULT as u64; return; }
            match crate::vfs::VFS.stat(path) {
                Some(st) => { unsafe { write_stat(buf, &st) }; frame.rax = 0; }
                None => frame.rax = ENOENT as u64,
            }
        },

        5 => { // SYS_FSTAT(fd, statbuf)
            let fd = arg1 as usize;
            let buf = arg2 as *mut u8;
            if !is_valid_user_ptr(buf, STAT_SIZE) { frame.rax = EFAULT as u64; return; }

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            if fd >= FD_MAX { frame.rax = EBADF as u64; return; }

            // Copy the path out before touching the VFS: `stat` takes the mount lock, and holding a
            // borrow of the task's fd_table across that is how the scheduler's `tasks` Vec gets
            // reallocated under a live reference.
            let path = match &percpu.scheduler.tasks[curr_idx].fd_table[fd] {
                Some(FileDescriptor::File(f)) => f.path.clone(),
                Some(_) => {
                    // A pipe or socket has no path. Report a plausible fstat rather than an error —
                    // libcs call fstat on every descriptor to size their I/O buffers, and failing
                    // here breaks stdio on a pipe.
                    let st = crate::vfs::FileStat { mode: 0o010000 | 0o600, ..Default::default() };
                    unsafe { write_stat(buf, &st) };
                    frame.rax = 0;
                    return;
                }
                None => { frame.rax = EBADF as u64; return; }
            };

            match crate::vfs::VFS.stat(&path) {
                Some(st) => { unsafe { write_stat(buf, &st) }; frame.rax = 0; }
                None => frame.rax = ENOENT as u64,
            }
        },

        262 => { // SYS_NEWFSTATAT(dirfd, path_cstr, statbuf, flags)
            // `dirfd` is now honoured via resolve_at. It used to be accepted and IGNORED, which
            // meant a relative path resolved against the cwd instead of the named directory —
            // stat succeeding on the wrong file, which is worse than failing.
            // AT_SYMLINK_NOFOLLOW in arg4 is still ignored; the VFS resolves links in the driver
            // and has no no-follow variant (same limitation as the lstat alias on arm 6).
            let raw = match unsafe { user_cstr_raw(arg2) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = match unsafe { resolve_at(arg1 as i64, &raw) } {
                Some(p) => p,
                None => { frame.rax = EBADF as u64; return; }
            };
            let path = path.as_str();
            let buf = arg3 as *mut u8;
            if !is_valid_user_ptr(buf, STAT_SIZE) { frame.rax = EFAULT as u64; return; }
            match crate::vfs::VFS.stat(path) {
                Some(st) => { unsafe { write_stat(buf, &st) }; frame.rax = 0; }
                None => frame.rax = ENOENT as u64,
            }
        },

        217 => { // SYS_GETDENTS64(fd, buf, buf_len) -> bytes written, 0 at end of directory
            let fd = arg1 as usize;
            let buf = arg2 as *mut u8;
            let buf_len = arg3 as usize;
            if buf_len == 0 || !is_valid_user_ptr(buf, buf_len) { frame.rax = EFAULT as u64; return; }

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            if fd >= FD_MAX { frame.rax = EBADF as u64; return; }

            let open_file = match &percpu.scheduler.tasks[curr_idx].fd_table[fd] {
                Some(FileDescriptor::File(f)) => f.clone(),
                Some(_) => { frame.rax = ENOTDIR as u64; return; }
                None => { frame.rax = EBADF as u64; return; }
            };

            let entries = crate::vfs::VFS.list_dir(&open_file.path);
            // The fd's `offset` doubles as the entry cursor for a directory. That is safe because a
            // descriptor is either read() as a file or getdents64()'d as a directory, never both —
            // and it is what makes the "call until it returns 0" contract work without a second
            // per-fd field.
            let mut cursor = open_file.offset.lock();
            let mut written = 0usize;

            while *cursor < entries.len() {
                let raw = &entries[*cursor];
                // `list_dir` marks directories with a trailing '/'. Strip it for d_name and use it
                // for d_type — a name that still carries the marker would break every consumer.
                let is_dir = raw.ends_with('/');
                let name = raw.trim_end_matches('/');
                let name_bytes = name.as_bytes();

                // struct linux_dirent64: u64 d_ino, i64 d_off, u16 d_reclen, u8 d_type, then the
                // NUL-terminated name. Records are 8-byte aligned.
                let reclen = (DIRENT_HEADER + name_bytes.len() + 1 + 7) & !7;
                if written + reclen > buf_len {
                    // Not even one more record fits. If we have written nothing at all the caller's
                    // buffer is too small for this entry and must be told so, or it will spin.
                    if written == 0 { frame.rax = EINVAL as u64; return; }
                    break;
                }

                // A real inode number if the entry can be stat'd, 0 otherwise. Never a hash: a
                // fabricated inode that collides makes two distinct files look like the same file.
                let child = alloc::format!("{}/{}", open_file.path.trim_end_matches('/'), name);
                let ino = crate::vfs::VFS.stat(&child).map(|s| s.ino as u64).unwrap_or(0);

                unsafe {
                    let rec = buf.add(written);
                    core::ptr::write_unaligned(rec as *mut u64, ino);
                    core::ptr::write_unaligned(rec.add(8) as *mut i64, (*cursor as i64) + 1);
                    core::ptr::write_unaligned(rec.add(16) as *mut u16, reclen as u16);
                    // DT_DIR = 4, DT_REG = 8.
                    core::ptr::write_unaligned(rec.add(18), if is_dir { 4u8 } else { 8u8 });
                    core::ptr::copy_nonoverlapping(name_bytes.as_ptr(), rec.add(DIRENT_HEADER), name_bytes.len());
                    // NUL, plus the alignment padding — zeroed so the caller never reads stack junk.
                    for p in (DIRENT_HEADER + name_bytes.len())..reclen {
                        core::ptr::write(rec.add(p), 0u8);
                    }
                }

                written += reclen;
                *cursor += 1;
            }

            frame.rax = written as u64;
        },

        21 => { // SYS_ACCESS(path_cstr, mode)
            // Nyx has no permission model, so this answers the only question it can answer
            // truthfully: does the path exist? R_OK/W_OK/X_OK all resolve to that.
            let path = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = path.as_str();
            frame.rax = if crate::vfs::VFS.stat(path).is_some() { 0 } else { ENOENT as u64 };
        },

        32 => { // SYS_DUP(oldfd) -> lowest free fd
            let oldfd = arg1 as usize;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];
            if oldfd >= FD_MAX { frame.rax = EBADF as u64; return; }

            match task.fd_table[oldfd].clone() {
                Some(fd_obj) => {
                    let mut out = EMFILE;
                    for i in 3..32 {
                        if task.fd_table[i].is_none() {
                            task.fd_table[i] = Some(fd_obj);
                            out = i as i64;
                            break;
                        }
                    }
                    frame.rax = out as u64;
                }
                None => frame.rax = EBADF as u64,
            }
        },

        72 => { // SYS_FCNTL(fd, cmd, arg)
            const F_DUPFD: u64 = 0;
            const F_GETFD: u64 = 1;
            const F_SETFD: u64 = 2;
            const F_GETFL: u64 = 3;
            const F_SETFL: u64 = 4;

            let fd = arg1 as usize;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];
            if fd >= FD_MAX || task.fd_table[fd].is_none() { frame.rax = EBADF as u64; return; }

            match arg2 {
                F_DUPFD => {
                    let fd_obj = task.fd_table[fd].clone();
                    let mut out = EMFILE;
                    for i in (arg3 as usize).max(3)..32 {
                        if task.fd_table[i].is_none() {
                            task.fd_table[i] = fd_obj;
                            out = i as i64;
                            break;
                        }
                    }
                    frame.rax = out as u64;
                }
                // Nyx stores no per-fd flags: there is no close-on-exec (execve rebuilds the table)
                // and no O_NONBLOCK. Report the cleared state rather than inventing one, and accept
                // the setters so a libc's start-up sequence completes.
                F_GETFD | F_GETFL => frame.rax = 0,
                F_SETFD | F_SETFL => frame.rax = 0,
                _ => frame.rax = EINVAL as u64,
            }
        },

        83 => { // SYS_MKDIR(path_cstr, mode)
            let path = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = path.as_str();
            // mkdir on an existing path is EEXIST, not success — std::fs::create_dir relies on the
            // distinction, and create_dir_all relies on std::fs::create_dir making it.
            if crate::vfs::VFS.stat(path).is_some() { frame.rax = EEXIST as u64; return; }
            frame.rax = if crate::vfs::VFS.create_dir(path) { 0 } else { EACCES as u64 };
        },

        84 => { // SYS_RMDIR(path_cstr)
            let path = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = path.as_str();
            match crate::vfs::VFS.stat(path) {
                None => { frame.rax = ENOENT as u64; return; }
                Some(st) if !st.is_dir() => { frame.rax = ENOTDIR as u64; return; }
                Some(_) => {}
            }
            // lwext4's ext4_dir_rm refuses a non-empty directory; surface that as ENOTEMPTY rather
            // than a generic failure, because it is the one case callers routinely handle.
            frame.rax = if crate::vfs::VFS.remove_dir(path) { 0 } else { ENOTEMPTY as u64 };
        },

        87 => { // SYS_UNLINK(path_cstr)
            let path = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = path.as_str();
            match crate::vfs::VFS.stat(path) {
                None => { frame.rax = ENOENT as u64; return; }
                // unlink is for names, not directories. Silently removing one would make rmdir's
                // emptiness check bypassable.
                Some(st) if st.is_dir() => { frame.rax = EISDIR as u64; return; }
                Some(_) => {}
            }
            frame.rax = if crate::vfs::VFS.delete_file(path) { 0 } else { EACCES as u64 };
        },

        263 => { // SYS_UNLINKAT(dirfd, path_cstr, flags)
            // The only remover std::filesystem::remove_all uses. Its absence is why remove_all
            // reported success-shaped failure ("directory still exists"): it walks a tree with
            // openat + unlinkat and has no unlink/rmdir fallback path.
            const AT_REMOVEDIR: u64 = 0x200;
            let raw = match unsafe { user_cstr_raw(arg2) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = match unsafe { resolve_at(arg1 as i64, &raw) } {
                Some(p) => p,
                None => { frame.rax = EBADF as u64; return; }
            };
            let path = path.as_str();
            let st = match crate::vfs::VFS.stat(path) {
                Some(st) => st,
                None => { frame.rax = ENOENT as u64; return; }
            };
            // The flag picks the operation, and getting it backwards is silently destructive, so
            // the type is checked against it rather than inferred from the path.
            if arg3 & AT_REMOVEDIR != 0 {
                if !st.is_dir() { frame.rax = ENOTDIR as u64; return; }
                frame.rax = if crate::vfs::VFS.remove_dir(path) { 0 } else { ENOTEMPTY as u64 };
            } else {
                if st.is_dir() { frame.rax = EISDIR as u64; return; }
                frame.rax = if crate::vfs::VFS.delete_file(path) { 0 } else { EACCES as u64 };
            }
        },

        82 => { // SYS_RENAME(old_cstr, new_cstr)
            let from = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let from = from.as_str();
            let to = match unsafe { user_cstr(arg2) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let to = to.as_str();
            if crate::vfs::VFS.stat(from).is_none() { frame.rax = ENOENT as u64; return; }
            let ok = crate::vfs::VFS.rename(from, to);
            // Instrumented because the probe reports "returned success but the file did not move",
            // and that is indistinguishable from the outside between ext4_frename lying and the
            // probe's own check being wrong. stat() BOTH paths afterwards: that is the ground truth
            // and it costs one line.
            let (src_left, dst_here) = (
                crate::vfs::VFS.stat(from).is_some(),
                crate::vfs::VFS.stat(to).is_some(),
            );
            crate::serial_println!(
                "[FS] rename \"{}\" -> \"{}\": ok={} old_still_there={} new_present={}",
                from, to, ok, src_left, dst_here);
            frame.rax = if ok { 0 } else { EACCES as u64 };
        },

        88 => { // SYS_SYMLINK(target_cstr, linkpath_cstr)
            let target = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let target = target.as_str();
            let path = match unsafe { user_cstr(arg2) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = path.as_str();
            if crate::vfs::VFS.stat(path).is_some() { frame.rax = EEXIST as u64; return; }
            frame.rax = if crate::vfs::VFS.symlink(target, path) { 0 } else { EACCES as u64 };
        },

        41 => frame.rax = sys_socket(arg1, arg2, arg3) as u64,
        42 => frame.rax = sys_connect(arg1 as usize, arg2 as *const u8, arg3 as usize) as u64,
        44 => frame.rax = sys_write_internal(arg1 as usize, arg2 as *const u8, arg3 as usize) as u64, 
        45 => frame.rax = sys_read_internal(arg1 as usize, arg2 as *mut u8, arg3 as usize) as u64,

        56 => { // SYS_CLONE(flags, child_stack, ptid, ctid, tls)
            // What musl's pthread_create issues. Nyx already HAS threads (syscall 58) and already
            // has the hard half of the contract: the exit path zeroes clear_child_tid and
            // FUTEX_WAKEs whoever is joining on it, which is how pthread_join terminates. This arm
            // maps clone's flags onto that machinery.
            const CLONE_VM: u64 = 0x100;
            const CLONE_THREAD: u64 = 0x10000;
            const CLONE_SETTLS: u64 = 0x80000;
            const CLONE_PARENT_SETTID: u64 = 0x100000;
            const CLONE_CHILD_CLEARTID: u64 = 0x200000;
            const CLONE_CHILD_SETTID: u64 = 0x1000000;

            let flags = arg1;
            let child_stack = arg2;
            let ptid = arg3;
            let ctid = arg4;
            let tls = arg5;

            // Only a genuine THREAD clone is supported. Without CLONE_VM|CLONE_THREAD the caller
            // wants a new address space, which is fork's job — and quietly giving it a thread
            // that shares memory instead would corrupt whatever it did next. ENOSYS is the honest
            // answer; musl's pthread_create always sets both.
            if flags & CLONE_VM == 0 || flags & CLONE_THREAD == 0 {
                crate::serial_println!("[56] clone: only CLONE_VM|CLONE_THREAD supported (flags={:#x})", flags);
                frame.rax = ENOSYS as u64;
                return;
            }
            if child_stack == 0 || !is_valid_user_ptr(child_stack as *const u8, 8) {
                frame.rax = EFAULT as u64;
                return;
            }

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EINVAL as u64; return; }
            let parent_cr3 = percpu.scheduler.tasks[curr_idx].cr3;

            // Fallible: a userspace clone() must never panic the kernel. (Arm 58 still .expect()s.)
            let mut thread = match crate::process::Process::new_thread(parent_cr3) {
                Ok(t) => t,
                Err(_) => { frame.rax = (-11i64) as u64; return; } // -EAGAIN, what pthread expects
            };

            {
                let parent = &percpu.scheduler.tasks[curr_idx];
                thread.parent_pid = Some(parent.pid);
                thread.mmap_bump = parent.mmap_bump;
                thread.cwd = parent.cwd.clone();
                for i in 0..FD_MAX {
                    if let Some(fd) = &parent.fd_table[i] { thread.fd_table[i] = Some(fd.clone()); }
                }
            }

            // CLONE_SETTLS: the new thread's FS base. The scheduler restores saved_fs_base on
            // switch-in, so setting it here is all that is needed — musl puts its whole thread
            // descriptor (including the tid that pthread_join reads) at this address.
            if flags & CLONE_SETTLS != 0 { thread.saved_fs_base = tls; }

            // CLONE_CHILD_CLEARTID: hand the address to the exit path, which zeroes it and wakes
            // joiners. This single line is what makes pthread_join return instead of hanging.
            if flags & CLONE_CHILD_CLEARTID != 0 { thread.clear_child_tid = ctid; }

            let tid = thread.pid;

            // CLONE_PARENT_SETTID / CLONE_CHILD_SETTID: publish the tid. Both are written from
            // here because parent and child share the address space, so there is no difference in
            // what they observe. Mapped-checked, not merely range-checked: a kernel-mode write to
            // an unmapped user address panics the MACHINE.
            for (want, addr) in [(CLONE_PARENT_SETTID, ptid), (CLONE_CHILD_SETTID, ctid)] {
                if flags & want != 0 && addr != 0
                    && is_valid_user_ptr(addr as *const u8, 4)
                    && unsafe { crate::memory::user_addr_mapped(addr) }
                {
                    unsafe { (addr as *mut u32).write_volatile(tid as u32); }
                }
            }

            // Build the child's resume frame. Modelled on FORK, not on arm 58: fork COPIES the
            // parent's registers, and musl's __clone stub depends on that — after the syscall it
            // does `pop %rdi; call *%r9`, so r9 (the thread entry point) must survive. Arm 58
            // zeroes the registers, which would jump the thread to 0.
            let stack_top = thread.kernel_stack_top;
            let iretq_ptr = stack_top - 40;
            unsafe {
                let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
                iret_slice[0] = frame.rcx;          // RIP: resume right after the syscall
                iret_slice[1] = 0x33;               // CS
                iret_slice[2] = frame.r11 | 0x200;  // RFLAGS, interrupts on
                iret_slice[3] = child_stack;        // RSP: the caller-supplied thread stack
                iret_slice[4] = 0x2B;               // SS
            }

            let regs_ptr = iretq_ptr - 120;
            unsafe {
                let regs = core::slice::from_raw_parts_mut(regs_ptr as *mut u64, 15);
                regs[0] = frame.r15; regs[1] = frame.r14; regs[2] = frame.r13; regs[3] = frame.r12;
                regs[4] = frame.r11; regs[5] = frame.r10; regs[6] = frame.r9;  regs[7] = frame.r8;
                regs[8] = frame.rdi; regs[9] = frame.rsi; regs[10] = frame.rbp; regs[11] = frame.rdx;
                regs[12] = frame.rcx; regs[13] = frame.rbx;
                regs[14] = 0;                        // rax: the child sees clone() return 0
            }

            let fxsave_ptr = (regs_ptr - 512) & !0xF;
            unsafe { crate::process::init_fpu_state(fxsave_ptr as u64); }

            let final_rsp = fxsave_ptr - 16;
            unsafe {
                let bottom = core::slice::from_raw_parts_mut(final_rsp as *mut u64, 2);
                bottom[0] = regs_ptr;
                bottom[1] = 0;
            }
            thread.saved_rsp = final_rsp;

            frame.rax = tid;                         // the parent sees the new tid
            percpu.scheduler.tasks.push(thread);
        },

        57 => { // SYS_FORK
            crate::serial::bc("[fork");
            crate::postmortem::mark(crate::postmortem::PM_FORK_ENTER);
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = ENOSYS as u64; return; }

            // A userspace fork() must NEVER be able to panic the whole kernel. Allocate the child
            // process fallibly and return -ENOMEM on failure instead of .expect()-halting the CPU.
            let mut child = match crate::process::Process::new() {
                Ok(c) => c,
                Err(_) => { frame.rax = (-12i64) as u64; return; } // -ENOMEM
            };

            {
                let parent = &percpu.scheduler.tasks[curr_idx];
                child.parent_pid = Some(parent.pid);
                child.mmap_bump = parent.mmap_bump;
                // cwd is inherited, not reset — a child that silently starts at / would resolve
                // every relative path somewhere its parent never intended.
                child.cwd = parent.cwd.clone();

                // 1. Share memory frames (CoW implementation). Fallible: on OOM mid-walk, tear the
                // half-built child address space back down and fail the fork gracefully.
                if crate::memory::clone_user_address_space(parent.cr3, child.cr3).is_err() {
                    crate::serial_println!("[57] fork FAILED: clone_user_address_space OOM");
                    crate::memory::clear_user_address_space(child.cr3);
                    frame.rax = (-12i64) as u64; // -ENOMEM
                    return;
                }

                // 🚨 CoW FIX: Flush the Parent's TLB!
                // Since we just marked the parent's active pages as Read-Only, we MUST 
                // force the CPU to forget the old Writable cached pages immediately.
                unsafe {
                    let cr3 = x86_64::registers::control::Cr3::read();
                    x86_64::registers::control::Cr3::write(cr3.0, cr3.1); 
                }

                // 2. Clone file descriptors (Sockets, files, pipes)
                for i in 0..FD_MAX {
                    if let Some(fd) = &parent.fd_table[i] {
                        child.fd_table[i] = Some(fd.clone());
                    }
                }
            }

            // 3. Setup the child's return stack frame
            let stack_top = child.kernel_stack_top;
            let iretq_ptr = stack_top - 40;

            unsafe {
                let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
                iret_slice[0] = frame.rcx;         
                iret_slice[1] = 0x33;              
                iret_slice[2] = frame.r11 | 0x200; 
                iret_slice[3] = frame.user_rsp;   
                iret_slice[4] = 0x2B;              
            }

            let regs_ptr = iretq_ptr - 120;
            unsafe {
                let regs = core::slice::from_raw_parts_mut(regs_ptr as *mut u64, 15);
                regs[0] = frame.r15;
                regs[1] = frame.r14;
                regs[2] = frame.r13;
                regs[3] = frame.r12;
                regs[4] = frame.r11; 
                regs[5] = frame.r10; 
                regs[6] = frame.r9;  
                regs[7] = frame.r8;  
                regs[8] = frame.rdi; 
                regs[9] = frame.rsi; 
                regs[10] = frame.rbp; 
                regs[11] = frame.rdx; 
                regs[12] = frame.rcx; 
                regs[13] = frame.rbx; 
                regs[14] = 0; // 🚨 The child receives PID 0 as its return value!
            }

            let fxsave_ptr = (regs_ptr - 512) & !0xF;
            unsafe {
                crate::process::init_fpu_state(fxsave_ptr as u64);
            }

            let final_rsp = fxsave_ptr - 16;
            unsafe {
                let bottom = core::slice::from_raw_parts_mut(final_rsp as *mut u64, 2);
                bottom[0] = regs_ptr; 
                bottom[1] = 0;        
            }

            child.saved_rsp = final_rsp;

            // 4. The parent process receives the child's actual PID!
            frame.rax = child.pid;

            percpu.scheduler.tasks.push(child);
            crate::serial::bc("fork]");
            crate::postmortem::mark(crate::postmortem::PM_FORK_DONE);
        },
        58 => { // SYS_SPAWN_THREAD
            let entry_point = arg1;
            let user_stack = arg2;

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = ENOSYS as u64; return; }
            
            let parent_cr3 = percpu.scheduler.tasks[curr_idx].cr3;
            let mut thread = crate::process::Process::new_thread(parent_cr3).expect("Failed to spawn thread");
            
            {
                let parent = &percpu.scheduler.tasks[curr_idx];
                thread.parent_pid = Some(parent.pid);
                thread.mmap_bump = parent.mmap_bump;

                // Share the File Descriptors (Sockets)
                for i in 0..FD_MAX {
                    if let Some(fd) = &parent.fd_table[i] {
                        thread.fd_table[i] = Some(fd.clone());
                    }
                }
            }

            let stack_top = thread.kernel_stack_top;
            let iretq_ptr = stack_top - 40;

            unsafe {
                let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
                iret_slice[0] = entry_point;       // RIP: Where the thread starts executing
                iret_slice[1] = 0x33;              // CS: Userspace Code Segment
                iret_slice[2] = frame.r11 | 0x200; // RFLAGS: Enable Interrupts
                iret_slice[3] = user_stack;        // RSP: The custom stack we allocated for the thread
                iret_slice[4] = 0x2B;              // SS: Userspace Stack Segment
            }

            let regs_ptr = iretq_ptr - 120;
            unsafe {
                core::ptr::write_bytes(regs_ptr as *mut u8, 0, 120); // Zero out general registers
            }

            let fxsave_ptr = (regs_ptr - 512) & !0xF;
            unsafe {
                crate::process::init_fpu_state(fxsave_ptr as u64);
            }

            let final_rsp = fxsave_ptr - 16;
            unsafe {
                let bottom = core::slice::from_raw_parts_mut(final_rsp as *mut u64, 2);
                bottom[0] = regs_ptr; 
                bottom[1] = 0;        
            }

            thread.saved_rsp = final_rsp;
            frame.rax = thread.pid;
            
            // --- THE TRUE SMP LOAD BALANCER ---
            unsafe {
                let active_cores = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                let mut target_core = percpu.logical_id;
                let mut min_tasks = usize::MAX;

                // 1. Scan all active CPU cores
                if let Some(all_cores) = &mut crate::percpu::PER_CPU {
                    for i in 0..active_cores {
                        let count = all_cores[i].scheduler.tasks.len();
                        
                        // 2. Find the core with the lightest workload
                        if count < min_tasks {
                            min_tasks = count;
                            target_core = i;
                        }
                    }
                    
                    crate::serial_println!("[SMP] Load Balancer: Offloading Thread to Core {} (Tasks: {})", target_core, min_tasks);
                    
                    // 3. Inject the thread directly into the idle core's hardware queue!
                    all_cores[target_core].scheduler.tasks.push(thread);
                }
            }
            // ----------------------------------
        },
          
       
        59 => { // sys_execve
            crate::serial::bc("[exec");
            crate::postmortem::mark(crate::postmortem::PM_EXEC_ENTER);
            let ptr = arg1 as *const u8;
            let len = arg2 as usize;
            // FIRST thing, before touching the pointer: this is the line that says whether the arm
            // is entered at all. Nothing printed for one particular binary while the identical
            // click path worked for others, and everything below dereferences user memory.
            crate::serial_println!("[EXEC] arm entered: ptr={:#x} len={}", arg1, len as u32);

            // ★ This read had NO validation. `from_raw_parts` on an unvalidated user pointer is a
            // kernel-mode read, and a kernel-mode read of an unmapped address panics the MACHINE
            // rather than killing the caller — so a bad path argument here took the whole box down
            // instead of returning an error. Both checks: range, then actually-mapped.
            if len == 0 || len > 4096 || !is_valid_user_ptr(ptr, len) {
                crate::serial_println!("[EXEC] refused: bad path pointer/len");
                frame.rax = EFAULT as u64;
                return;
            }
            if !unsafe { crate::memory::user_addr_mapped(arg1) } {
                crate::serial_println!("[EXEC] refused: path page not mapped");
                frame.rax = EFAULT as u64;
                return;
            }

            // 1. Copy the path to a safe Kernel String BEFORE shredding user memory!
            let path_str = if let Ok(s) = core::str::from_utf8(unsafe { core::slice::from_raw_parts(ptr, len) }) {
                alloc::string::String::from(s.trim_matches(char::from(0)).trim())
            } else {
                frame.rax = (-1i64) as u64;
                return;
            };

            // 2. Read the file using the safe Kernel String
            // Logged either side: execve reaches read_file_alloc but never reaches load_elf_full for
            // one particular binary, so the read itself is the remaining suspect and this says
            // whether it returns at all.
            crate::postmortem::mark(crate::postmortem::PM_EXEC_PATH_OK);
            crate::serial_println!("[EXEC] reading \"{}\"", path_str);
            crate::serial::bc(" read>");
            crate::postmortem::mark(crate::postmortem::PM_READ_ENTER);
            let read = crate::vfs::VFS.read_file_alloc(&path_str);
            crate::serial::bc("<read ");
            crate::postmortem::mark(crate::postmortem::PM_READ_DONE);
            crate::serial_println!("[EXEC] read returned {} bytes",
                read.as_ref().map(|d| d.len()).unwrap_or(0) as u32);
            if let Some(elf_data) = read {
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                let task = &mut percpu.scheduler.tasks[curr_idx];

                // 3. Shred the old memory
                crate::serial::bc(" clear>");
                crate::postmortem::mark(crate::postmortem::PM_CLEAR_ENTER);
                crate::memory::clear_user_address_space(task.cr3);
                crate::serial::bc("<clear ");
                crate::postmortem::mark(crate::postmortem::PM_CLEAR_DONE);

                // 🚨 THE FIX: Reset the bump allocator to a VALID canonical address! 🚨
                // 0x1000_0000_0000 is safely inside the lower user half.
                task.mmap_bump = 0x1000_0000_0000;

                
                // 4. Flush the CPU TLB
                unsafe {
                    let cr3 = x86_64::registers::control::Cr3::read();
                    x86_64::registers::control::Cr3::write(cr3.0, cr3.1);
                }
                
                // 5. Load the new ELF
                if let Ok(loaded) = crate::process::load_elf_full(&elf_data) {
                    let entry_point = loaded.entry;
                    let stack_base = 0x7FFF_0000_0000;
                    let stack_pages = 32;
                    // A stack holding executable code is the classic overflow target; NX it.
                    if crate::memory::allocate_user_pages_at(stack_base, stack_pages, false).is_ok() {
                        let stack_top = ((stack_base + (stack_pages as u64 * 4096)) & !0xF) - 8;

                        // B2: build the SysV entry stack (argc/argv/envp/auxv) in the freshly loaded
                        // address space so a std runtime can start. We're already on the task's CR3.
                        let entry_rsp = unsafe {
                            crate::process::build_initial_stack(stack_top, &path_str, &loaded)
                        };

                        // Override the Syscall Return Frame!
                        frame.rcx = entry_point;    // Jump to the new App's _start
                        frame.user_rsp = entry_rsp; // Give it the fresh SysV stack
                        
                        // 🚨 SECURITY FIX: Zero out ALL general purpose registers!
                        // This prevents the new app from inheriting garbage state from the old app.
                        frame.rdi = 0; frame.rsi = 0; frame.rdx = 0; frame.rbp = 0;
                        frame.r8 = 0; frame.r9 = 0; frame.r10 = 0; 
                        frame.r11 = 0x202; // RFLAGS: Ensure hardware interrupts stay enabled!
                        frame.r12 = 0; frame.r13 = 0; frame.r14 = 0; frame.r15 = 0;
                        frame.rbx = 0;

                        // 6. Name the task for the System Monitor and the thermal CPU-share report.
                        // Use the last path component, not the raw path: 16 bytes of
                        // "/mnt/nvme/apps/WindowServer.nyx/run.bin" truncates to "/mnt/nvme/apps/W",
                        // so every app showed up under an identical, useless name.
                        let leaf = path_str.rsplit('/')
                            .find(|s| !s.is_empty() && *s != "run.bin")
                            .unwrap_or(&path_str);
                        let leaf = leaf.strip_suffix(".nyx").unwrap_or(leaf);
                        let mut name_arr = [0u8; 16];
                        let bytes = leaf.as_bytes();
                        let copy_len = core::cmp::min(16, bytes.len());
                        name_arr[..copy_len].copy_from_slice(&bytes[..copy_len]);
                        task.name = name_arr;
                        
                        frame.rax = 0; // Success
                        crate::serial::bc("exec]\n");
                        // The operation completed. Clear the record rather than leaving the last
                        // stage behind: a stale breadcrumb reads exactly like a fresh one.
                        crate::postmortem::mark(crate::postmortem::PM_EXEC_DONE);
                        crate::postmortem::clear();
                        return;        // Bypass default block exit
                    }
                }
            }
            frame.rax = (-1i64) as u64; // File Not Found or Parse Error
        },

        60 | 231 => { // SYS_EXIT / SYS_EXIT_GROUP (single-threaded teardown covers both here)
            // 0. DISABLE INTERRUPTS to prevent getting buried alive!
            x86_64::instructions::interrupts::disable();

            let exit_code = arg1 as i64;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];

            let my_cr3 = task.cr3;
            let my_pid = task.pid;
            crate::serial_println!("[PID {}] Exited (Code: {})", my_pid, exit_code);
            task.exit_code = exit_code; // B1: retained for the parent's wait4 before reaping.

            // 1. Safe FD Teardown using Arc Reference Counting.
            //    Collect first and free after the fd table is clear: the free path must not run
            //    while a borrow of `task` is live (see destroy_socket).
            let mut doomed_sockets = alloc::vec::Vec::new();
            for i in 0..FD_MAX {
                if let Some(FileDescriptor::Socket(sock_mtx)) = &task.fd_table[i] {
                    if alloc::sync::Arc::strong_count(sock_mtx) == 1 {
                        let s = sock_mtx.lock();
                        doomed_sockets.push((s.stack, s.gen, s.kind.clone()));
                    }
                }
                // Safely drop our reference to the FD
                task.fd_table[i] = None;
            }
            for (stack, gen, kind) in doomed_sockets {
                destroy_socket(stack, gen, kind);
            }

            // B-β.2a: threads share `cr3`. Count OTHER live tasks in this address space across all
            // cores; if any exist, this is a *thread* exit — we must NOT shred the shared user memory
            // (that would kill the siblings). Only the sole owner of the cr3 tears the memory down.
            let mut siblings_alive = false;
            unsafe {
                let active = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                if let Some(cores) = &mut crate::percpu::PER_CPU {
                    'scan: for i in 0..active {
                        for t in cores[i].scheduler.tasks.iter() {
                            if t.pid != my_pid
                                && t.cr3 == my_cr3
                                && t.state != crate::scheduler::TaskState::Empty
                                && t.state != crate::scheduler::TaskState::Zombie {
                                siblings_alive = true;
                                break 'scan;
                            }
                        }
                    }
                }
            }

            let task = &mut percpu.scheduler.tasks[curr_idx];
            if siblings_alive {
                // 2a. Thread exit: leave the shared address space intact. Self-reap this slot
                // (Empty), so no wait4 is required for a joined worker. The kernel stack leaks,
                // consistent with the existing process-exit path (which also never frees it).

                // set_tid_address: zero the word and wake whoever is joining on it. Done ONLY on
                // this branch — in the sole-owner branch below the address space is about to be
                // shredded, and there is by definition nobody left to join.
                let ctid = task.clear_child_tid;
                task.clear_child_tid = 0;
                if ctid != 0
                    && is_valid_user_ptr(ctid as *const u8, 4)
                    // ★ The range check alone is not enough. A kernel-mode write to an unmapped
                    // user address panics the MACHINE rather than killing this thread, and `ctid`
                    // came from userspace — the thread could have unmapped its own stack since.
                    && unsafe { crate::memory::user_addr_mapped(ctid) }
                {
                    unsafe { (ctid as *mut u32).write_volatile(0); }
                    // FUTEX_WAKE on that word. Matched on cr3 as well as address so two processes
                    // at the same numeric address cannot cross-wake; threads share cr3, so a real
                    // joiner in this process does match.
                    for t in percpu.scheduler.tasks.iter_mut() {
                        if t.futex_addr == ctid && t.cr3 == my_cr3
                            && t.state == crate::scheduler::TaskState::Blocked
                        {
                            t.futex_addr = 0;
                            t.state = crate::scheduler::TaskState::Ready;
                        }
                    }
                }

                let task = &mut percpu.scheduler.tasks[curr_idx];
                task.state = crate::scheduler::TaskState::Empty;
            } else {
                // 2b. Sole owner: shred ONLY the user memory tables securely.
                // DO NOT swap CR3 to KERNEL_CR3, or the CPU will instantly Triple Fault using the stack!
                crate::memory::clear_user_address_space(my_cr3);
                // 3. Mark as Zombie at the VERY END, once all locks are released, for the parent's wait4.
                task.state = crate::scheduler::TaskState::Zombie;
            }

            // 4. Re-enable interrupts and wait for the scheduler to context-switch away natively
            unsafe {
                x86_64::instructions::interrupts::enable();
                loop { core::arch::asm!("hlt") }
            }
        },

        131 => { frame.rax = 0; }, // SYS_SIGALTSTACK

        158 => { // SYS_ARCH_PRCTL (TLS Support)
            let code = arg1;
            let addr = arg2;
            match code {
                0x1002 => { // ARCH_SET_FS
                    unsafe { x86_64::registers::model_specific::FsBase::write(x86_64::VirtAddr::new(addr)); }
                    frame.rax = 0;
                },
                0x1003 => { // ARCH_GET_FS
                    // SYSCALL does not swap FS, so the MSR still holds the caller's own base and is
                    // authoritative for the running task; `Process::saved_fs_base` is only the
                    // context-switch spill and can be stale while the task is on-CPU. `addr` is a
                    // userspace `unsigned long *`.
                    if is_valid_user_ptr(addr as *const u8, 8) {
                        let base = x86_64::registers::model_specific::FsBase::read().as_u64();
                        unsafe { (addr as *mut u64).write_volatile(base); }
                        frame.rax = 0;
                    } else {
                        frame.rax = EFAULT as u64;
                    }
                },
                // ARCH_SET_GS / ARCH_GET_GS (0x1001 / 0x1004) stay refused deliberately. GS_BASE
                // holds this core's `percpu` block while we are in kernel mode, so servicing a GS
                // request from inside the syscall handler would overwrite it and take the core down.
                // Doing it properly means going through KERNEL_GS_BASE and swapgs; nothing needs GS
                // TLS yet, and an honest EINVAL beats a machine-killing guess.
                _ => { frame.rax = EINVAL as u64; },
            }
        },

        218 => { // SYS_SET_TID_ADDRESS(tidptr) -> caller's tid
            // Was `frame.rax = 1`: it claimed success, recorded nothing, and returned a tid of 1
            // for every thread. Now it stores the address, which is what makes the clear-on-exit
            // below possible — that clear is the mechanism pthread_join is built on.
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = ESRCH as u64; return; }
            percpu.scheduler.tasks[curr_idx].clear_child_tid = arg1;
            frame.rax = percpu.scheduler.tasks[curr_idx].pid;
        },

        318 => { // SYS_GETRANDOM (Required for Rust HashMaps)
            let buf_ptr = arg1 as *mut u8;
            let len = arg2 as usize;
            if is_valid_user_ptr(buf_ptr, len) {
                // Hardware RDSEED/RDRAND (see random.rs), falling back to the old TSC-seeded
                // xorshift only on a CPU with no DRNG at all. This used to be the xorshift
                // unconditionally, which was written for HashMap seeding and is guessable — and
                // rustls draws ECDHE keys and GCM nonces from right here.
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                crate::random::fill(buf);
                frame.rax = len as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },

        // ====================================================================
        // std-port syscalls (B1): the Linux-numbered primitives std needs that
        // were previously falling through to the `_ => EINVAL` arm below.
        // ====================================================================
        10 => { // SYS_MPROTECT(addr, len, prot)
            const PROT_READ: u64 = 1;
            const PROT_WRITE: u64 = 2;
            const PROT_EXEC: u64 = 4;
            let (addr, len, prot) = (arg1, arg2 as usize, arg3);

            if addr & 0xFFF != 0 { frame.rax = EINVAL as u64; return; }
            if len == 0 { frame.rax = 0; return; }
            // Range-check before walking anything: a page walk driven by an unchecked user address
            // would happily reach kernel PTEs.
            if !is_valid_user_ptr(addr as *const u8, len) { frame.rax = EFAULT as u64; return; }
            // PROT_NONE means clearing PRESENT, which this kernel's fault handler would then read as
            // a demand-paging miss rather than a protection error. Refused rather than half-done.
            if prot & (PROT_READ | PROT_WRITE | PROT_EXEC) == 0 { frame.rax = EINVAL as u64; return; }

            let pages = (len + 0xFFF) / 0x1000;
            frame.rax = match unsafe {
                crate::memory::protect_user_range(
                    addr, pages, prot & PROT_WRITE != 0, prot & PROT_EXEC != 0)
            } {
                Ok(()) => 0,
                // POSIX: ENOMEM when the range contains pages that are not mapped.
                Err(_) => ENOMEM as u64,
            };
        },

        257 => { // SYS_OPENAT(dirfd, path_cstr, flags, mode)
            // The libc-facing open. Nyx's own open(2) is `(ptr, len, flags)` — a Nyx-ism no C
            // library will ever produce — so rather than break every existing caller, the POSIX
            // shape lands here. musl already prefers openat on architectures that lack SYS_open,
            // so pointing its open at this is a one-line arch decision rather than a port.
            // Real dirfd-relative resolution now works: fds carry their path, so `resolve_at` can
            // join against it. This used to be ENOSYS, which broke std::filesystem's `remove_all`
            // — it recurses with `openat(dirfd, name, ...)` and cannot fall back to anything.
            let raw = match unsafe { user_cstr_raw(arg2) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = match unsafe { resolve_at(arg1 as i64, &raw) } {
                Some(p) => p,
                None => { frame.rax = EBADF as u64; return; }
            };
            const O_CREAT: u64 = 0x40;
            const O_TRUNC: u64 = 0x200;
            const O_DIRECTORY: u64 = 0o200000;
            let flags = arg3;
            // ★ O_DIRECTORY must be honoured, not ignored. `remove_all` opens every entry with it
            // to ASK "is this a directory?", and treats ENOTDIR as "it is a file, unlink it". Let
            // a regular file open here and libc++ hands it to fdopendir instead, and the file is
            // never removed — a silent wrong answer rather than an error.
            if flags & O_DIRECTORY != 0 {
                match crate::vfs::VFS.stat(&path) {
                    Some(st) if st.is_dir() => {}
                    Some(_) => { frame.rax = ENOTDIR as u64; return; }
                    None => { frame.rax = ENOENT as u64; return; }
                }
            }
            if flags & O_TRUNC != 0 {
                crate::vfs::VFS.delete_file(&path);
                crate::vfs::VFS.create_file(&path);
            } else if flags & O_CREAT != 0 {
                crate::vfs::VFS.create_file(&path);
            }
            match crate::vfs::VFS.open_path(&path) {
                Some(vnode) => {
                    let percpu = crate::percpu::current();
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
                    let task = &mut percpu.scheduler.tasks[curr_idx];
                    let mut got = -1isize;
                    for i in 3..FD_MAX {
                        if task.fd_table[i].is_none() {
                            task.fd_table[i] = Some(crate::scheduler::FileDescriptor::File(
                                alloc::sync::Arc::new(crate::vfs::OpenFile::new(vnode)),
                            ));
                            got = i as isize;
                            break;
                        }
                    }
                    frame.rax = if got < 0 { EMFILE as u64 } else { got as u64 };
                }
                None => frame.rax = ENOENT as u64,
            }
        },

        79 => { // SYS_GETCWD(buf, size)
            let buf = arg1 as *mut u8;
            let size = arg2 as usize;
            if size == 0 || !is_valid_user_ptr(buf, size) { frame.rax = EFAULT as u64; return; }
            let cwd = unsafe { current_cwd() };
            let need = cwd.len() + 1; // + NUL
            if need > size { frame.rax = ERANGE as u64; return; }
            unsafe {
                core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf, cwd.len());
                *buf.add(cwd.len()) = 0;
            }
            // Linux returns the length INCLUDING the terminating NUL, and both musl and glibc
            // depend on that: their wrappers return `buf` on a positive result, so returning the
            // string length instead would silently under-report by one and truncate long paths.
            frame.rax = need as u64;
        },

        80 => { // SYS_CHDIR(path_cstr)
            let path = match unsafe { user_cstr(arg1) } {
                Some(p) => p,
                None => { frame.rax = EFAULT as u64; return; }
            };
            let path = path.as_str();
            // Already absolute: user_cstr resolves against cwd for EVERY path syscall now, so
            // resolving again here would be a no-op that implies the opposite.
            let target = alloc::string::String::from(path);
            // It must exist AND be a directory. Letting chdir into a regular file "succeed" would
            // push the failure into every later relative open, somewhere else entirely.
            match crate::vfs::VFS.stat(&target) {
                Some(st) if st.is_dir() => {}
                Some(_) => { frame.rax = ENOTDIR as u64; return; }
                None => { frame.rax = ENOENT as u64; return; }
            }
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            percpu.scheduler.tasks[curr_idx].cwd = target;
            frame.rax = 0;
        },

        24 => { // SYS_SCHED_YIELD — give up the rest of this quantum.
            unsafe { core::arch::asm!("int 0x41"); }
            frame.rax = 0;
        },

        7 => { // SYS_POLL(fds_ptr, nfds, timeout_ms). timeout < 0 blocks indefinitely.
            let ptr = arg1 as *mut u8;
            let nfds = arg2 as usize;
            let timeout = arg3 as i64;
            if nfds > POLL_MAX { frame.rax = EINVAL as u64; return; }
            if nfds > 0 && !is_valid_user_ptr(ptr, nfds * 8) { frame.rax = EFAULT as u64; return; }

            // Copy the request in ONCE. The wait below spans yields, and re-reading user memory on
            // each pass would let another thread rewrite the request mid-call.
            let mut want = [(0i32, 0i16); POLL_MAX];
            for i in 0..nfds {
                let e = unsafe { ptr.add(i * 8) };
                want[i] = unsafe {
                    ((e as *const i32).read_unaligned(), (e.add(4) as *const i16).read_unaligned())
                };
            }

            let start = crate::time::UPTIME_MS.load(Ordering::Relaxed);
            let mut out = [0i16; POLL_MAX];
            let mut fdlist = [0i32; POLL_MAX];
            for i in 0..nfds { fdlist[i] = want[i].0; }
            let mut samp = [None; POLL_MAX];

            loop {
                // Re-sampled every pass: a descriptor can be closed by another thread between
                // passes, and stale readiness would report a dead pipe as live forever.
                unsafe { sample_readiness(&fdlist[..nfds], &mut samp[..nfds]); }

                let mut ready = 0usize;
                for i in 0..nfds {
                    let (fd, ev) = want[i];
                    let mut re = 0i16;
                    if fd < 0 {
                        // POSIX: a negative fd is ignored and reports revents 0.
                    } else if let Some((r, w, h)) = samp[i] {
                        if r && (ev & POLLIN) != 0 { re |= POLLIN; }
                        if w && (ev & POLLOUT) != 0 { re |= POLLOUT; }
                        // HUP is reported whether or not it was asked for — that is what lets a
                        // caller watching only POLLIN still learn the peer is gone.
                        if h { re |= POLLHUP; }
                    } else {
                        re = POLLNVAL;
                    }
                    out[i] = re;
                    if re != 0 { ready += 1; }
                }

                let expired = timeout >= 0 && {
                    let now = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                    timeout == 0 || now.saturating_sub(start) >= timeout as u64
                };
                if ready > 0 || expired {
                    // `out` is all zeros on the timeout path, which is exactly what POSIX wants.
                    for i in 0..nfds {
                        unsafe { (ptr.add(i * 8 + 6) as *mut i16).write_unaligned(out[i]); }
                    }
                    frame.rax = ready as u64;
                    return;
                }
                // ★ Readiness is checked FIRST and wins. A pass that found live fds reports them
                // even if a signal is also pending: the work is already done, and throwing it away
                // for EINTR would lose edge-triggered readiness the caller cannot ask for again.
                // POSIX agrees — EINTR is for a call that accomplished nothing.
                if unsafe { signal_would_interrupt() } {
                    // revents are deliberately NOT written back: EINTR means "nothing happened".
                    frame.rax = EINTR as u64;
                    return;
                }
                unsafe { readiness_yield(); }
            }
        },

        23 => { // SYS_SELECT(nfds, readfds, writefds, exceptfds, timeout)
            let nfds = arg1 as usize;
            let (rp, wp, ep) = (arg2 as *mut u64, arg3 as *mut u64, arg4 as *mut u64);
            let tp = arg5 as *const i64;
            if nfds > POLL_MAX { frame.rax = EINVAL as u64; return; }

            // Only one 64-bit word is examined: POSIX says bits at or above nfds are not looked at,
            // and POLL_MAX caps nfds at 64. Touching only what we validated keeps us away from the
            // rest of the caller's 128-byte fd_set, which we have no business writing.
            for p in [rp, wp, ep] {
                if !p.is_null() && !is_valid_user_ptr(p as *const u8, 8) {
                    frame.rax = EFAULT as u64; return;
                }
            }
            // struct timeval { i64 tv_sec; i64 tv_usec }. A NULL pointer means block indefinitely.
            let timeout: i64 = if tp.is_null() { -1 } else {
                if !is_valid_user_ptr(tp as *const u8, 16) { frame.rax = EFAULT as u64; return; }
                let (s, us) = unsafe { (tp.read_unaligned(), tp.add(1).read_unaligned()) };
                s.saturating_mul(1000).saturating_add(us / 1000)
            };

            let rin = if rp.is_null() { 0u64 } else { unsafe { rp.read_unaligned() } };
            let win = if wp.is_null() { 0u64 } else { unsafe { wp.read_unaligned() } };

            let start = crate::time::UPTIME_MS.load(Ordering::Relaxed);
            let mut fdlist = [0i32; POLL_MAX];
            for fd in 0..nfds { fdlist[fd] = fd as i32; }
            let mut samp = [None; POLL_MAX];

            loop {
                unsafe { sample_readiness(&fdlist[..nfds], &mut samp[..nfds]); }

                let (mut rout, mut wout, mut count) = (0u64, 0u64, 0usize);
                for fd in 0..nfds {
                    let bit = 1u64 << fd;
                    let (want_r, want_w) = (rin & bit != 0, win & bit != 0);
                    if !want_r && !want_w { continue; }
                    match samp[fd] {
                        // select has no per-fd error slot, so a bad descriptor fails the whole
                        // call. That is the documented behaviour and it beats silently dropping it.
                        None => { frame.rax = EBADF as u64; return; }
                        Some((r, w, h)) => {
                            if want_r && (r || h) { rout |= bit; count += 1; }
                            if want_w && w { wout |= bit; count += 1; }
                        }
                    }
                }

                let expired = timeout >= 0 && {
                    let now = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                    timeout == 0 || now.saturating_sub(start) >= timeout as u64
                };
                if count > 0 || expired {
                    unsafe {
                        if !rp.is_null() { rp.write_unaligned(rout); }
                        if !wp.is_null() { wp.write_unaligned(wout); }
                        // We surface no exceptional conditions, so this is always cleared rather
                        // than left holding whatever the caller passed in.
                        if !ep.is_null() { ep.write_unaligned(0); }
                    }
                    frame.rax = count as u64;
                    return;
                }
                // See the poll arm: readiness wins over a pending signal, and on EINTR the three
                // fd_sets are left untouched rather than cleared — POSIX leaves them unspecified,
                // and a caller that retries after its handler must not be told "nothing is ready"
                // by a call that never looked.
                if unsafe { signal_would_interrupt() } {
                    frame.rax = EINTR as u64;
                    return;
                }
                unsafe { readiness_yield(); }
            }
        },

        39 | 186 => { // SYS_GETPID / SYS_GETTID (threads carry their own pid here).
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            frame.rax = percpu.scheduler.tasks[curr_idx].pid;
        },

        228 => { // SYS_CLOCK_GETTIME(clockid, *timespec{tv_sec:i64, tv_nsec:i64})
            let clockid = arg1 as i32;
            let ts_ptr = arg2 as *mut i64;
            if !is_valid_user_ptr(ts_ptr as *const u8, 16) { frame.rax = EFAULT as u64; return; }
            let (sec, nsec) = match clockid {
                // CLOCK_REALTIME, plus whatever correction `time sync` established (syscall 550).
                0 => (
                    rtc_packed_to_unix(crate::rtc::read_packed())
                        + REALTIME_OFFSET_SECS.load(core::sync::atomic::Ordering::Relaxed),
                    0i64,
                ),
                _ => { // CLOCK_MONOTONIC / BOOTTIME / everything else -> uptime
                    let ms = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
                    ((ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64)
                }
            };
            unsafe { *ts_ptr = sec; *ts_ptr.add(1) = nsec; }
            frame.rax = 0;
        },

        35 | 230 => { // SYS_NANOSLEEP(*req,*rem) / SYS_CLOCK_NANOSLEEP(clockid,flags,*req,*rem)
            let req_ptr = (if id == 35 { arg1 } else { arg3 }) as *const i64;
            if !is_valid_user_ptr(req_ptr as *const u8, 16) { frame.rax = EFAULT as u64; return; }
            let (sec, nsec) = unsafe { (*req_ptr, *req_ptr.add(1)) };
            // Round up to whole milliseconds (the scheduler's tick granularity).
            let ms = (sec.max(0) as u64) * 1000 + ((nsec.max(0) as u64) + 999_999) / 1_000_000;
            if ms > 0 {
                let wake_ms = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) + ms;
                unsafe {
                    x86_64::instructions::interrupts::enable();
                    loop {
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        percpu.scheduler.tasks[curr_idx].state = crate::scheduler::TaskState::Blocked;
                        percpu.scheduler.tasks[curr_idx].wake_tsc = wake_ms;
                        core::arch::asm!("int 0x41");
                        if crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) >= wake_ms { break; }
                        x86_64::instructions::hlt();
                    }
                    x86_64::instructions::interrupts::disable();
                }
            }
            frame.rax = 0;
        },

        202 => { // SYS_FUTEX(uaddr, op, val, timeout, uaddr2, val3)
            const FUTEX_WAIT: u32 = 0;
            const FUTEX_WAKE: u32 = 1;
            const FUTEX_PRIVATE_FLAG: u32 = 128;
            const FUTEX_CLOCK_REALTIME: u32 = 256;
            let uaddr = arg1;
            let cmd = (arg2 as u32) & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
            let val = arg3 as u32;

            if !is_valid_user_ptr(uaddr as *const u8, 4) { frame.rax = EFAULT as u64; return; }
            let cr3 = {
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                percpu.scheduler.tasks[curr_idx].cr3
            };

            match cmd {
                FUTEX_WAIT => {
                    // Optional relative timeout (timespec at arg4). None => wait indefinitely.
                    let deadline = if arg4 != 0 && is_valid_user_ptr(arg4 as *const u8, 16) {
                        let tp = arg4 as *const i64;
                        let (s, n) = unsafe { (*tp, *tp.add(1)) };
                        let ms = (s.max(0) as u64) * 1000 + ((n.max(0) as u64) + 999_999) / 1_000_000;
                        Some(crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) + ms)
                    } else { None };

                    unsafe {
                        x86_64::instructions::interrupts::enable();
                        let mut ret = 0i64;
                        loop {
                            // Durable re-check: the waker stores a new value at *uaddr BEFORE waking.
                            // Reading it each iteration means a lost FUTEX_WAKE cannot hang us forever.
                            let cur = core::ptr::read_volatile(uaddr as *const u32);
                            if cur != val { ret = 0; break; }                   // value changed -> woken
                            let now = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
                            if let Some(d) = deadline { if now >= d { ret = -110; break; } } // ETIMEDOUT

                            // Block, but cap the sleep at 10 ms so a missed wake self-heals into a
                            // re-poll of *uaddr (liveness insurance for this lock-free wake path).
                            let repoll = now + 10;
                            let cap = match deadline { Some(d) => core::cmp::min(d, repoll), None => repoll };
                            let percpu = crate::percpu::current();
                            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                            percpu.scheduler.tasks[curr_idx].state = crate::scheduler::TaskState::Blocked;
                            percpu.scheduler.tasks[curr_idx].wake_tsc = cap;
                            percpu.scheduler.tasks[curr_idx].futex_addr = uaddr;

                            core::arch::asm!("int 0x41");

                            let percpu = crate::percpu::current();
                            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                            percpu.scheduler.tasks[curr_idx].futex_addr = 0;
                        }
                        x86_64::instructions::interrupts::disable();
                        frame.rax = ret as u64;
                    }
                },
                FUTEX_WAKE => {
                    // Wake up to `val` waiters parked on this (cr3, uaddr). Scan every core's run queue.
                    let mut woken = 0u32;
                    let want = val;
                    unsafe {
                        let active = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                        if let Some(cores) = &mut crate::percpu::PER_CPU {
                            'outer: for i in 0..active {
                                for task in cores[i].scheduler.tasks.iter_mut() {
                                    if woken >= want { break 'outer; }
                                    if task.futex_addr == uaddr && task.cr3 == cr3
                                        && task.state == crate::scheduler::TaskState::Blocked {
                                        task.state = crate::scheduler::TaskState::Ready;
                                        task.wake_tsc = 0;
                                        task.futex_addr = 0;
                                        woken += 1;
                                    }
                                }
                            }
                        }
                    }
                    frame.rax = woken as u64;
                },
                _ => { frame.rax = EINVAL as u64; }
            }
        },

        61 => { // SYS_WAIT4(pid, *wstatus, options, *rusage) — reap a Zombie child.
            const WNOHANG: u64 = 1;
            let want_pid = arg1 as i64;   // -1 = any child
            let wstatus = arg2 as *mut i32;
            let options = arg3;

            let my_pid = {
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                percpu.scheduler.tasks[curr_idx].pid
            };

            // Scan all cores for a matching Zombie child; capture + tombstone it (state=Empty).
            let reap = |wpid: i64, parent: u64| -> Option<(u64, i64)> {
                unsafe {
                    let active = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                    if let Some(cores) = &mut crate::percpu::PER_CPU {
                        for i in 0..active {
                            for task in cores[i].scheduler.tasks.iter_mut() {
                                if task.state == crate::scheduler::TaskState::Zombie
                                    && task.parent_pid == Some(parent)
                                    && (wpid == -1 || task.pid as i64 == wpid) {
                                    let out = (task.pid, task.exit_code);
                                    task.state = crate::scheduler::TaskState::Empty; // reaped tombstone
                                    return Some(out);
                                }
                            }
                        }
                    }
                }
                None
            };

            if let Some((cpid, code)) = reap(want_pid, my_pid) {
                if !wstatus.is_null() && is_valid_user_ptr(wstatus as *const u8, 4) {
                    unsafe { *wstatus = ((code as i32) & 0xff) << 8; } // WEXITSTATUS encoding
                }
                frame.rax = cpid;
            } else if options & WNOHANG != 0 {
                frame.rax = 0; // nothing ready, caller asked not to block
            } else {
                // Block until a child becomes reapable (re-poll; no explicit exit-waker exists yet).
                unsafe {
                    x86_64::instructions::interrupts::enable();
                    let mut result = 0u64;
                    loop {
                        if let Some((cpid, code)) = reap(want_pid, my_pid) {
                            if !wstatus.is_null() && is_valid_user_ptr(wstatus as *const u8, 4) {
                                *wstatus = ((code as i32) & 0xff) << 8;
                            }
                            result = cpid;
                            break;
                        }
                        let now = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        percpu.scheduler.tasks[curr_idx].state = crate::scheduler::TaskState::Blocked;
                        percpu.scheduler.tasks[curr_idx].wake_tsc = now + 20; // re-scan every 20 ms
                        core::arch::asm!("int 0x41");
                        x86_64::instructions::hlt();
                    }
                    x86_64::instructions::interrupts::disable();
                    frame.rax = result;
                }
            }
        },
        
        // --- CUSTOM NYXOS SYSCALLS ---
        501 => {
            unsafe {
                let raw_color = arg5 as u32;

                let mut hardware_accelerated = false;

                // Try to use the GPU first!
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    if let Some(p) = &crate::gui::SCREEN_PAINTER {
                        let screen_w = p.info.width as u32;
                        let screen_h = p.info.height as u32;
                        let pitch = (p.info.stride * 4) as u32;
                        
                        let start_x = core::cmp::min(arg1 as u32, screen_w);
                        let start_y = core::cmp::min(arg2 as u32, screen_h);
                        let max_w = screen_w.saturating_sub(start_x);
                        let max_h = screen_h.saturating_sub(start_y);
                        let w = core::cmp::min(arg3 as u32, max_w);
                        let h = core::cmp::min(arg4 as u32, max_h);

                        let _ = gpu.fill_rect(0x1400_0000, start_x, start_y, w, h, raw_color, pitch);
                        hardware_accelerated = true;
                    }
                }

                // CPU Fallback (If GPU is offline or not Intel)
                if !hardware_accelerated {
                    if let Some(p) = &mut crate::gui::SCREEN_PAINTER {
                        let screen_w = p.info.width;
                        let screen_h = p.info.height;
                        let start_x = core::cmp::min(arg1 as usize, screen_w);
                        let start_y = core::cmp::min(arg2 as usize, screen_h);
                        let max_w = screen_w.saturating_sub(start_x);
                        let max_h = screen_h.saturating_sub(start_y);
                        let w = core::cmp::min(arg3 as usize, max_w);
                        let h = core::cmp::min(arg4 as usize, max_h);
                        let rect = Rect { x: start_x, y: start_y, w, h };
                        
                        let r = ((raw_color >> 16) & 0xFF) as u8;
                        let g = ((raw_color >> 8) & 0xFF) as u8;
                        let b = (raw_color & 0xFF) as u8;
                        let color = Color::new(r, g, b);
                        p.draw_rect(rect, color);
                    }
                }
            }
        },

        502 => { // sys_swap_buffers — present backbuffer -> owned scanout buffer (P1a), plane armed once.
             unsafe {
                 if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                     if let Some(p) = &crate::gui::SCREEN_PAINTER {
                         let w = p.info.width as u32;
                         let h = p.info.height as u32;
                         let pitch = (p.info.stride * 4) as u32;
                         // BLT the backbuffer into our owned scanout buffer (active_gva == scanout_gva when
                         // P1a init succeeded, else the firmware-luck GVA 0). Same fast path as before —
                         // NO per-frame wait_for_idle or flip (those stalled on the fence timeout).
                         let _ = gpu.copy_rect(
                             0, 0, pitch, 0x1400_0000,
                             0, 0, pitch, gpu.active_gva,
                             w, h
                         );
                         gpu.submit_fence();
                         gpu.arm_scanout_plane(); // point the plane at scanout_gva once (no-op after first)
                     }
                 }
             }
        },

        538 => { // sys_swap_buffers_rect(x, y, w, h) — D1 region present.
            // Identical to 502 (backbuffer 0x1400_0000 -> active_gva at the full screen pitch) except it
            // BLTs only the damage sub-rect. Because src_x==dst_x==x, src_y==dst_y==y and the pitch is the
            // full screen stride for BOTH surfaces, the rect lands at the exact same scanout pixels 502
            // would have written — so it inherits whatever makes the full present visible (no new address
            // assumptions). The compositor uses this instead of 502 when only a partial region changed.
            let x = arg1 as u32;
            let y = arg2 as u32;
            let w = arg3 as u32;
            let h = arg4 as u32;
            unsafe {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    if let Some(p) = &crate::gui::SCREEN_PAINTER {
                        let sw = p.info.width as u32;
                        let sh = p.info.height as u32;
                        let pitch = (p.info.stride * 4) as u32;
                        // Clamp the rect to the screen so a stale/oversized damage box can never blit
                        // out of bounds (the copy_rect BLT has no internal clip).
                        if w > 0 && h > 0 && x < sw && y < sh {
                            let cw = w.min(sw - x);
                            let ch = h.min(sh - y);
                            // D1 region present into the owned scanout buffer (active_gva == scanout_gva
                            // after P1a init). Single buffer, so a partial BLT is valid — the untouched
                            // pixels retain the previous frame. Arm the plane once (no-op after first).
                            let _ = gpu.copy_rect(
                                x, y, pitch, 0x1400_0000,
                                x, y, pitch, gpu.active_gva,
                                cw, ch
                            );
                            gpu.submit_fence();
                            gpu.arm_scanout_plane();
                        }
                    }
                }
            }
        },

        503 => { // sys_gpu_sync
             unsafe {
                 if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                     gpu.submit_fence();
                     gpu.wait_for_idle();
                     // NOTE: deliberately does NOT park here. Parking at every frame's sync point
                     // cost a forcewake wake + RC6 exit on the next submission and made the desktop
                     // sluggish. The GT is parked by the thermal governor once it has been idle for
                     // GPU_PARK_IDLE_MS — see `maybe_park_gpu`.
                 }
             }
        },

        504 => {
            // Return true uptime, completely immune to CPU frequency scaling!
            frame.rax = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
        },

        528 => {
            // SYS_GET_RTC: battery-backed wall clock, packed year<<40|month<<32|day<<24|hour<<16|min<<8|sec.
            frame.rax = crate::rtc::read_packed();
        },

        529 => {
            // SYS_CURSOR_INIT: bring up the display hardware cursor plane. 1 = enabled, 0 = fell back.
            frame.rax = crate::drivers::gpu::intel::cursor::init() as u64;
        },

        535 => {
            // SYS_CURSOR_SET_IMAGE(argb_ptr): upload a 64x64 ARGB cursor bitmap. 1 ok / 0 err.
            let ptr = arg1 as *const u32;
            let bytes = 64 * 64 * 4;
            if is_valid_user_ptr(ptr as *const u8, bytes) {
                let img = unsafe { core::slice::from_raw_parts(ptr, 64 * 64) };
                frame.rax = crate::drivers::gpu::intel::cursor::set_image(img) as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },

        536 => {
            // SYS_GPU_COMPOSITE(list_ptr, count): GPU-composite `count` WindowQuads into the backbuffer.
            // 1 ok / 0 => caller falls back to CPU compositing.
            use crate::drivers::gpu::intel::render::compositor::WindowQuad;
            let ptr = arg1 as *const WindowQuad;
            let count = arg2 as usize;
            let bytes = count.saturating_mul(core::mem::size_of::<WindowQuad>());
            if count == 0 {
                frame.rax = 1;
            } else if is_valid_user_ptr(ptr as *const u8, bytes) {
                let quads = unsafe { core::slice::from_raw_parts(ptr, count) };
                frame.rax = crate::drivers::gpu::intel::render::compositor::composite(quads) as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },

        537 => {
            // SYS_GPU_DRAW_TEXT(atlas_gva, atlas_w, atlas_h, atlas_pitch, glyph_ptr, glyph_count):
            // U5 — GPU-draw a batched glyph run sampling the userspace font atlas into the backbuffer.
            // 1 ok / 0 => caller falls back to CPU text.
            use crate::drivers::gpu::intel::render::text::GlyphQuad;
            let atlas_gva = arg1 as u32;
            let atlas_w = arg2 as u32;
            let atlas_h = arg3 as u32;
            let atlas_pitch = arg4 as u32;
            let ptr = arg5 as *const GlyphQuad;
            let count = arg6 as usize;
            let bytes = count.saturating_mul(core::mem::size_of::<GlyphQuad>());
            if count == 0 {
                frame.rax = 1;
            } else if is_valid_user_ptr(ptr as *const u8, bytes) {
                let glyphs = unsafe { core::slice::from_raw_parts(ptr, count) };
                frame.rax = crate::drivers::gpu::intel::render::text::draw_text(
                    atlas_gva, atlas_w, atlas_h, atlas_pitch, glyphs,
                ) as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },


        505 => {
            // THE FIX: Shield the spinlock from hardware interrupts!
            // This prevents IRQ 12 from firing while we are reading the mouse state.
            let m_val = x86_64::instructions::interrupts::without_interrupts(|| {
                let m = crate::mouse::MOUSE_STATE.lock();
                (m.x as u64) << 32 | (m.y as u64) << 16 | (if m.left_click {1} else {0}) << 1 | (if m.right_click {1} else {0})
            });
            frame.rax = m_val;
        },

        506 => { if let Some(c) = crate::shell::pop_key() { frame.rax = c as u64; } else { frame.rax = 0; } },

        507 => { 
             unsafe {
                 if let Some(p) = &crate::SCREEN_PAINTER {
                     if is_valid_user_ptr(arg1 as *const u8, 8) && is_valid_user_ptr(arg2 as *const u8, 8) && is_valid_user_ptr(arg3 as *const u8, 8) {
                         *(arg1 as *mut u64) = p.info.width as u64;
                         *(arg2 as *mut u64) = p.info.height as u64;
                         *(arg3 as *mut u64) = if p.info.stride > 0 { p.info.stride } else { p.info.width } as u64;
                         frame.rax = 1;
                     } else { frame.rax = EFAULT as u64; }
                 } else { frame.rax = 0; }
            }
        },

        508 => { 
            unsafe {
                let mut mapped_phys = 0;
                let mut size = 0;
                
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_ref() {
                    if gpu.backbuffer_phys != 0 {
                        mapped_phys = gpu.backbuffer_phys;
                        size = gpu.backbuffer_size;
                    }
                }
                
                if mapped_phys == 0 {
                    if let Some(p) = &mut crate::gui::SCREEN_PAINTER {
                        let virt_start = p.buffer.as_ptr() as u64;
                        if let Some(phys) = crate::memory::virt_to_phys(virt_start) {
                            mapped_phys = phys;
                            size = p.buffer.len() as u64;
                        }
                    }
                }
                
                if mapped_phys != 0 && size != 0 {
                    if let Ok(user_virt) = crate::memory::map_user_framebuffer(mapped_phys, size) {
                        frame.rax = user_virt;
                    } else { frame.rax = 0; }
                } else { frame.rax = 0; }
            }
        },
        509 => { // sys_gpu_map_shm
            let shm_id = arg1;
            let gva = arg2 as u32;
            let mut success = 0;
            
            // Map SHM pages into GGTT
            let registry = crate::memory::SHM_REGISTRY.lock();
            if let Some(block) = registry.iter().find(|b| b.id == shm_id) {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    for (i, frame) in block.frames.iter().enumerate() {
                        unsafe {
                            gpu.map_ggtt_page(gva / 4096 + i as u32, frame.as_u64(), true);
                        }
                    }
                    success = 1;
                }
            }
            frame.rax = success;
        },

        512 => { // sys_gpu_copy_rect
            let src_gva = arg1 as u32;
            let dst_gva = arg2 as u32;
            let w = arg3 as u32;
            let h = arg4 as u32;
            let dst_x = arg5 as u32;
            let dst_y = arg6 as u32;
            
            unsafe {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    if let Some(p) = &crate::gui::SCREEN_PAINTER {
                        let src_pitch = w * 4;
                        let dst_pitch = (p.info.stride * 4) as u32;
                        let _ = gpu.copy_rect(
                            0, 0, src_pitch, src_gva,
                            dst_x, dst_y, dst_pitch, dst_gva,
                            w, h
                        );
                    }
                }
            }
        },



        // -----------------------------------------------------
        // VFS DIRECTORY LISTING SYSCALLS
        // -----------------------------------------------------
        
        // Syscall 510: Get Directory Item Count
        510 => {
            let path_ptr = arg1 as *const u8;
            let path_len = arg2 as usize;
            
            // 🔥 FIX: Wrap raw slice creation in an unsafe block
            let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            
            if let Ok(path) = core::str::from_utf8(path_slice) {
                let list = crate::vfs::VFS.list_dir(path);
                frame.rax = list.len() as u64;
            } else {
                frame.rax = 0;
            }
        }
        
        // Syscall 511: Get Directory Item String by Index
        511 => {
            let index = arg1 as usize;
            let buf_ptr = arg2 as *mut u8;
            let path_ptr = arg3 as *const u8;
            let path_len = arg4 as usize;
            
            // 🔥 FIX: Wrap raw slice creation in an unsafe block
            let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            
            if let Ok(path) = core::str::from_utf8(path_slice) {
                let list = crate::vfs::VFS.list_dir(path);
                
                if let Some(entry) = list.get(index) {
                    let bytes = entry.as_bytes();
                    
                    // 🔥 FIX: Wrap the memory copy in an unsafe block
                    unsafe {
                        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_ptr, bytes.len());
                    }
                    
                    frame.rax = bytes.len() as u64;
                } else {
                    frame.rax = 0;
                }
            } else {
                frame.rax = 0;
            }
        }
        // 541: sys_file_size(path_ptr, path_len) -> i64 bytes, or -1 if missing / not a plain file.
        541 => {
            let path_ptr = arg1 as *const u8;
            let path_len = arg2 as usize;
            let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            frame.rax = if let Ok(path) = core::str::from_utf8(path_slice) {
                match crate::vfs::VFS.file_size(path) {
                    Some(sz) => sz as u64,
                    None => (-1i64) as u64,
                }
            } else {
                (-1i64) as u64
            };
        }

        // 542: sys_statfs(out_ptr) -> 0 ok / -1. Writes {u64 total, u64 free, u32 block_size} of the
        // /mnt/nvme ext4 mount into user memory at out_ptr (offsets 0/8/16, matching nyx_api::StatFs).
        542 => {
            let out_ptr = arg1 as *mut u8;
            match crate::vfs::VFS.statfs("/mnt/nvme") {
                Some(sf) => {
                    unsafe {
                        core::ptr::write_unaligned(out_ptr as *mut u64, sf.total_bytes);
                        core::ptr::write_unaligned(out_ptr.add(8) as *mut u64, sf.free_bytes);
                        core::ptr::write_unaligned(out_ptr.add(16) as *mut u32, sf.block_size);
                    }
                    frame.rax = 0;
                }
                None => frame.rax = (-1i64) as u64,
            }
        }

        // ---------------------------------------------------------------------
        // P8: WiFi — the syscalls behind the desktop's network picker. The driver has no built-in
        // SSID any more; userspace chooses the network and supplies the passphrase.
        //
        // Lock discipline: every arm below takes WIFI_DRIVER and MUST drop it before calling
        // anything in drivers::net that re-acquires it (init_wifi_iface does) — hence the explicit
        // scopes rather than one long `if let`.
        //
        // ★ P8c INTERRUPT DISCIPLINE — see the long comment at the top of drivers/net/mod.rs.
        // SYSCALL clears IF, so these arms start with interrupts OFF. The three that touch the
        // driver run for SECONDS, and leaving IF off for that long does two bad things:
        //   1. it deadlocks against the idle task's poll_wifi, which holds the very same spin locks
        //      with interrupts on and so can be preempted mid-hold — the spinner then waits on a
        //      task that only the timer it has masked could resume; and
        //   2. even absent that, it starves the thermal governor, which is an ordinary kernel task
        //      driven by the timer. No scheduler means no fan control and no HWP throttle, so the
        //      package just keeps climbing.
        // Both symptoms — a wedged machine with a core pinned at 100%, running hot — trace back to
        // this. So the long arms re-enable IF for their duration (the sys_sleep_ms arm sets the
        // precedent) and serialise on wifi_try_begin() instead of on the mutex, while the cheap
        // polled arms (544/546) never touch the driver at all and read the published snapshot.
        // ---------------------------------------------------------------------

        // 543: sys_wifi_scan() -> number of visible networks (re-runs a passive sweep; blocks ~7 s).
        543 => {
            let mut n = 0usize;
            // A scan with the radio switched off would retune and light up the antenna — precisely
            // what "off" is supposed to stop. The picker greys the button out; this is the backstop.
            if crate::drivers::net::wifi_radio_on() && crate::drivers::net::wifi_try_begin() {
                unsafe { x86_64::instructions::interrupts::enable(); }
                if let Some(w) = crate::drivers::net::WIFI_DRIVER.lock().as_mut() {
                    n = unsafe { w.rescan() };
                    crate::drivers::net::publish_wifi_snapshot(w);
                }
                unsafe { x86_64::instructions::interrupts::disable(); }
                crate::drivers::net::wifi_end();
            } else {
                // Another long operation owns the radio; report what we last saw rather than
                // blocking behind it.
                n = crate::drivers::net::WIFI_SNAPSHOT.lock().nets.len();
            }
            frame.rax = n as u64;
        }

        // 544: sys_wifi_list(out_ptr, max_entries) -> entries written. Each entry is 44 bytes:
        // ssid[32], ssid_len u8, channel u8, band u8, flags u8 (bit0 secure, bit1 rsn, bit2 current),
        // bssid[6], pad[2]. Matches nyx_api::WifiNetwork.
        544 => {
            let out = arg1 as *mut u8;
            let max = arg2 as usize;
            let mut written = 0usize;
            // Snapshot only — never WIFI_DRIVER. The picker calls this on a timer, and a blocking
            // acquire here is one of the two ways the machine used to wedge.
            if is_valid_user_ptr(out as *const u8, max.saturating_mul(44)) {
                let snap = crate::drivers::net::WIFI_SNAPSHOT.lock();
                for e in snap.nets.iter().take(max) {
                    unsafe { core::ptr::copy_nonoverlapping(e.as_ptr(), out.add(written * 44), 44) };
                    written += 1;
                }
            }
            frame.rax = written as u64;
        }

        // 545: sys_wifi_connect(ssid_ptr, ssid_len, psk_ptr, psk_len) -> LinkState (see nyx_api).
        // Blocks for the full association + 4-way handshake + DHCP exchange (several seconds).
        545 => {
            // Validate only non-empty buffers: an open network legitimately passes a zero-length
            // passphrase, and is_valid_user_ptr rejects (null, 0) — which would refuse the join for
            // entirely the wrong reason.
            let ssid_ok = arg2 == 0 || is_valid_user_ptr(arg1 as *const u8, arg2 as usize);
            let psk_ok  = arg4 == 0 || is_valid_user_ptr(arg3 as *const u8, arg4 as usize);
            if ssid_ok && psk_ok
                && crate::drivers::net::wifi_radio_on()
                && crate::drivers::net::wifi_try_begin()
            {
                let ssid = unsafe { core::slice::from_raw_parts(arg1 as *const u8, arg2 as usize) };
                let psk  = unsafe { core::slice::from_raw_parts(arg3 as *const u8, arg4 as usize) };
                // ~10 s of association, 4-way handshake and DHCP. Interrupts stay ON throughout, so
                // the desktop keeps redrawing and the thermal governor keeps running while we join.
                unsafe { x86_64::instructions::interrupts::enable(); }
                // Drop any existing interface FIRST: a switch invalidates the old lease, and leaving
                // the stale one in place would have smoltcp polling a device that is mid-retune.
                crate::drivers::net::teardown_wifi_iface();
                let mut ok = false;
                {
                    let mut lock = crate::drivers::net::WIFI_DRIVER.lock();
                    if let Some(w) = lock.as_mut() {
                        ok = unsafe { w.try_connect(ssid, psk) };
                    }
                } // driver lock released here — init_wifi_iface takes it again
                if ok { crate::drivers::net::init_wifi_iface(); }
                if let Some(w) = crate::drivers::net::WIFI_DRIVER.lock().as_ref() {
                    crate::drivers::net::publish_wifi_snapshot(w);
                }
                unsafe { x86_64::instructions::interrupts::disable(); }
                crate::drivers::net::wifi_end();
            }
            frame.rax = crate::drivers::net::wifi_cached_state() as u64;
        }

        // 546: sys_wifi_status(out_ptr) -> 0 ok / -1 no adapter. Writes 40 bytes:
        // state u32, ip[4], mask[4], router[4], dns[4], ssid_len u8, ssid[32] → see nyx_api::WifiStatus.
        546 => {
            // The tray polls this every second. Snapshot only — see 544.
            let out = arg1 as *mut u8;
            let snap = crate::drivers::net::WIFI_SNAPSHOT.lock();
            if snap.present && is_valid_user_ptr(out as *const u8, snap.status.len()) {
                unsafe {
                    core::ptr::copy_nonoverlapping(snap.status.as_ptr(), out, snap.status.len())
                };
                frame.rax = 0;
            } else {
                frame.rax = (-1i64) as u64;
            }
        }

        // 547: sys_wifi_disconnect() -> 0. Leaves the network and frees the radio to scan/retune.
        547 => {
            if crate::drivers::net::wifi_try_begin() {
                // Teardown walks the firmware through a chain of REMOVE commands, each of which
                // waits on a response — seconds in the worst case, so interrupts stay on.
                unsafe { x86_64::instructions::interrupts::enable(); }
                crate::drivers::net::teardown_wifi_iface();
                if let Some(w) = crate::drivers::net::WIFI_DRIVER.lock().as_mut() {
                    unsafe { w.disconnect() };
                    crate::drivers::net::publish_wifi_snapshot(w);
                }
                unsafe { x86_64::instructions::interrupts::disable(); }
                crate::drivers::net::wifi_end();
            }
            frame.rax = 0;
        }

        // 548: sys_wifi_set_radio(on) -> LinkState after the change (WIFI_RADIO_OFF when off).
        //
        // Turning it off leaves the network and drops the interface; turning it on only re-arms the
        // radio, it does not rejoin — reconnecting is the saved-network agent's job, and joining
        // something the user didn't ask for the instant they flick a switch is the wrong default.
        548 => {
            let want_on = arg1 != 0;
            // Refuse while another long operation owns the radio, exactly like 543/545/547: a join in
            // flight would otherwise have the link ripped out from under it halfway through the
            // handshake. The picker sees the state come back unchanged and can say so.
            if want_on != crate::drivers::net::wifi_radio_on()
                && crate::drivers::net::wifi_try_begin()
            {
                // Teardown talks to the firmware and waits on each response — seconds in the worst
                // case, so interrupts stay on for the same reason 547 enables them.
                unsafe { x86_64::instructions::interrupts::enable(); }
                if want_on {
                    crate::drivers::net::set_wifi_radio_off(false);
                } else {
                    // Latch BEFORE tearing down. publish_wifi_snapshot reads this flag, and anything
                    // that races in behind us must already see the radio as down — otherwise the
                    // teardown's own publish would report a plain "not connected" and the switch
                    // would appear to have flipped back on by itself.
                    crate::drivers::net::set_wifi_radio_off(true);
                    crate::drivers::net::teardown_wifi_iface();
                    if let Some(w) = crate::drivers::net::WIFI_DRIVER.lock().as_mut() {
                        unsafe { w.disconnect() };
                    }
                }
                if let Some(w) = crate::drivers::net::WIFI_DRIVER.lock().as_ref() {
                    crate::drivers::net::publish_wifi_snapshot(w);
                }
                unsafe { x86_64::instructions::interrupts::disable(); }
                crate::drivers::net::wifi_end();
            }
            frame.rax = crate::drivers::net::wifi_cached_state() as u64;
        }

        // sys_socket_set_timeout(fd, ms) — blocking read/write deadline for a socket. 0 = forever.
        //
        // The moral equivalent of SO_RCVTIMEO/SO_SNDTIMEO, as one knob because nothing here needs
        // them to differ. Exists because the alternative is unbounded: a TLS handshake that stalls
        // mid-flight leaves the calling thread inside the kernel's blocking loop with no way out,
        // and userspace cannot time it out from the outside because it never regains control.
        552 => { // SYS_SET_RTC(unix_secs) — write the battery-backed clock so a fix SURVIVES REBOOT.
            // Without this the only cure for a wrong clock is the firmware setup screen, which is
            // not something an OS should require of anyone. With it, a time sync is permanent and
            // the correction offset (550) becomes unnecessary — so we clear it here, or the next
            // reading would apply the correction twice: once from the RTC we just fixed, and again
            // from the offset that fixed it.
            let secs = arg1 as i64;
            // Refuse a time before 2001. A zero or garbage argument would otherwise brick the clock
            // into a state that this very syscall is the only way to repair.
            if secs < 978_307_200 { frame.rax = EINVAL as u64; return; }
            let (y, mo, d, h, mi, s) = unix_to_civil(secs);
            if crate::rtc::set_time(y, mo, d, h, mi, s) {
                REALTIME_OFFSET_SECS.store(0, Ordering::Relaxed);
                crate::serial_println!("[TIME] RTC set to {}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                    y, mo, d, h, mi, s);
                frame.rax = 0;
            } else {
                crate::serial_println!("[TIME] RTC write refused (bad value, or clock never idled)");
                frame.rax = EINVAL as u64;
            }
        },

        553 => { // SYS_GET_TZ() -> minutes east of UTC (signed), for DISPLAY only.
            // Read straight from CMOS rather than cached in a static: it is two port reads, it
            // needs no boot-order initialisation, and there is exactly one source of truth.
            frame.rax = crate::rtc::tz_offset_min() as i64 as u64;
        },

        554 => { // SYS_SET_TZ(minutes) — persist the display timezone.
            // ★ Deliberately does NOT touch the clock. The RTC and CLOCK_REALTIME stay in UTC; this
            // only changes how a human-facing string is rendered. Shifting the actual clock would
            // break TLS validity windows and every file timestamp on the disk.
            let minutes = arg1 as i64;
            if minutes < i16::MIN as i64 || minutes > i16::MAX as i64 {
                frame.rax = EINVAL as u64;
            } else if crate::rtc::set_tz_offset_min(minutes as i16) {
                crate::serial_println!("[TIME] display timezone set to UTC{:+}:{:02}",
                    minutes / 60, (minutes % 60).abs());
                frame.rax = 0;
            } else {
                frame.rax = EINVAL as u64;
            }
        },

        551 => { // SYS_DEBUG_MARK(byte) — userspace breadcrumb into CMOS. 0 clears it.
            // Lets a userspace program leave evidence that survives a hard freeze plus the power
            // cycle needed to recover from it. See `postmortem::user_mark`.
            crate::postmortem::user_mark(arg1 as u8);
            frame.rax = 0;
        },

        550 => { // SYS_SET_REALTIME_OFFSET(secs) — shift CLOCK_REALTIME by a signed second count.
            // ★ Corrects a wrong hardware clock WITHOUT writing to the RTC. Writing CMOS means
            // setting the SET bit in status register B, hand-rolling BCD into the very registers
            // the clock is ticking in, and clearing it again — and getting that wrong corrupts the
            // only timepiece the machine has. An offset is a pure software correction: reversible,
            // and incapable of damaging anything.
            //
            // The honest cost is that it does not survive a reboot. That is deliberate: this
            // corrects the epoch, it does not take over timekeeping — the RTC still supplies every
            // tick, and syscall 528 still reports the hardware unmodified so `date` can show both.
            REALTIME_OFFSET_SECS.store(arg1 as i64, Ordering::Relaxed);
            crate::serial_println!("[TIME] CLOCK_REALTIME offset set to {} s", arg1 as i64);
            frame.rax = 0;
        },

        549 => {
            let fd = arg1 as usize;
            frame.rax = EBADF as u64;
            if fd < FD_MAX && KERNEL_CR3.load(Ordering::Relaxed) != 0 {
                let percpu = crate::percpu::current();
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                if curr_idx < percpu.scheduler.tasks.len() {
                    let task = &mut percpu.scheduler.tasks[curr_idx];
                    if let Some(FileDescriptor::Socket(sock_mtx)) = &task.fd_table[fd] {
                        sock_mtx.lock().timeout_ms = arg2;
                        frame.rax = 0;
                    }
                }
            }
        }

        513 => { // sys_wait_vsync — returns 1 if the vblank was observed, 0 on timeout / no GPU.
            let mut seen = 0u64;
            unsafe {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    seen = if gpu.wait_for_vsync() { 1 } else { 0 };
                }
            }
            frame.rax = seen;
        },

        // ---------------------------------------------------------------------
        // Phase 9: mini-GL 3D syscalls. These wrap the RCS render engine's Scene
        // path (SSAA-antialiased, textured, multi-mesh) for userspace. The heavy
        // data (verts/texels) is copied into kernel Vecs once at upload; per frame
        // only the MVP matrices cross the boundary. See render/gl.rs.
        // ---------------------------------------------------------------------
        514 => { // sys_gl_init(width, height, dst_pixels_ptr) -> 1 ok / 0 err
            // Windowed GL: the app passes the CPU pointer to ITS OWN SHM window pixel buffer. The GL
            // path resolves into a private kernel backbuffer and copies it here each frame, so the
            // compositor composites glcube as a normal window (no scanout blit). Validate the buffer.
            let width = arg1 as u32;
            let height = arg2 as u32;
            let dst_ptr = arg3 as *const u8;
            let bytes = (width as usize).saturating_mul(height as usize).saturating_mul(4);
            if width == 0 || height == 0 || bytes == 0 || !is_valid_user_ptr(dst_ptr, bytes) {
                frame.rax = 0;
            } else {
                match crate::drivers::gpu::intel::render::gl::gl_init(width, height, dst_ptr as u64) {
                    Ok(()) => frame.rax = 1,
                    Err(_) => frame.rax = 0,
                }
            }
        },

        515 => { // sys_gl_upload_mesh(desc_ptr) -> handle (>=0) or u64::MAX on error
            // desc layout (repr(C), all u64): [verts_ptr, vert_count, idx_ptr, idx_count,
            //                                  texel_ptr, tex_w, tex_h]
            let desc_ptr = arg1 as *const u64;
            if !is_valid_user_ptr(desc_ptr as *const u8, 7 * 8) {
                frame.rax = u64::MAX;
            } else {
                let d = unsafe { core::slice::from_raw_parts(desc_ptr, 7) };
                let verts_ptr = d[0] as *const crate::drivers::gpu::intel::render::gl::GlVertex;
                let vert_count = d[1] as usize;
                let idx_ptr = d[2] as *const u32;
                let idx_count = d[3] as usize;
                let texel_ptr = d[4] as *const u32;
                let tex_w = d[5] as u32;
                let tex_h = d[6] as u32;
                let vbytes = vert_count.saturating_mul(28); // 7 f32
                let ibytes = idx_count.saturating_mul(4);
                let tbytes = (tex_w as usize).saturating_mul(tex_h as usize).saturating_mul(4);
                if vert_count == 0 || idx_count == 0 || tex_w == 0 || tex_h == 0
                    || !is_valid_user_ptr(verts_ptr as *const u8, vbytes)
                    || !is_valid_user_ptr(idx_ptr as *const u8, ibytes)
                    || !is_valid_user_ptr(texel_ptr as *const u8, tbytes)
                {
                    frame.rax = u64::MAX;
                } else {
                    let verts = unsafe { core::slice::from_raw_parts(verts_ptr, vert_count) };
                    let indices = unsafe { core::slice::from_raw_parts(idx_ptr, idx_count) };
                    let texels = unsafe {
                        core::slice::from_raw_parts(texel_ptr, (tex_w * tex_h) as usize)
                    };
                    match crate::drivers::gpu::intel::render::gl::gl_upload_mesh(
                        verts, indices, texels, tex_w, tex_h,
                    ) {
                        Ok(h) => frame.rax = h as u64,
                        Err(_) => frame.rax = u64::MAX,
                    }
                }
            }
        },

        516 => { // sys_gl_render(mvps_ptr, count) -> 1 ok / 0 err. mvps = count * 16 f32 (column-major).
            let mvps_ptr = arg1 as *const f32;
            let count = arg2 as usize;
            let floats = count.saturating_mul(16);
            let bytes = floats.saturating_mul(4);
            if count == 0 || !is_valid_user_ptr(mvps_ptr as *const u8, bytes) {
                frame.rax = 0;
            } else {
                let flat = unsafe { core::slice::from_raw_parts(mvps_ptr, floats) };
                match crate::drivers::gpu::intel::render::gl::gl_render_flat(flat, count) {
                    Ok(()) => frame.rax = 1,
                    Err(_) => frame.rax = 0,
                }
            }
        },

        527 => { // sys_gl_reset()
            crate::drivers::gpu::intel::render::gl::gl_reset();
            frame.rax = 1;
        },

        517 => {
            let buf_ptr = arg1 as *mut u8;
            let buf_len = arg2 as usize;
            if !is_valid_user_ptr(buf_ptr, buf_len) { frame.rax = EFAULT as u64; return; }
            
            let mcfg = unsafe { crate::acpi::ACPI_INFO.mcfg_addr.unwrap_or(0) };
            let madt = unsafe { crate::acpi::ACPI_INFO.madt_addr.unwrap_or(0) };
            let info = format!("Hardware Discovery Report:\nMCFG: {:#x}\nMADT: {:#x}", mcfg, madt);
            let bytes = info.as_bytes();
            let len = core::cmp::min(bytes.len(), buf_len);
            unsafe { for i in 0..len { *buf_ptr.add(i) = bytes[i]; } }
            frame.rax = len as u64;
        },

        518 => { 
            let buf_ptr = arg1 as *mut u8;
            let buf_len = arg2 as usize;
            if !is_valid_user_ptr(buf_ptr, buf_len) { frame.rax = EFAULT as u64; return; }
            
            unsafe {
                // Copy as much of the boot log as the caller's buffer holds. (Was
                // capped at 8192, which truncated the GPU pipeline decode mid-stream
                // in the sysmon viewer.) BOOT_LOG_IDX is always <= BOOT_LOG_SIZE.
                let log_len = crate::serial::BOOT_LOG_IDX;
                let copy_len = core::cmp::min(buf_len, log_len);
                for i in 0..copy_len { *buf_ptr.add(i) = crate::serial::BOOT_LOG[i]; }
                frame.rax = copy_len as u64;
            }
        },

        519 => { 
            let num_pages = arg1 as usize;
            if num_pages == 0 || num_pages > 8192 { frame.rax = 0; return; }
            
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = 0; return; }
            
            let task = &mut percpu.scheduler.tasks[curr_idx];
            let target_addr = task.mmap_bump;
            task.mmap_bump += (num_pages as u64) * 0x1000;

            // Anonymous memory is data: NX. See the mmap arm above.
            match crate::memory::allocate_user_pages_at(target_addr, num_pages, false) {
                Ok(mapped_addr) => frame.rax = mapped_addr,
                Err(_) => frame.rax = 0,
            }
        },

        520 => { 
            let buf_ptr = arg1 as *mut u8;
            if arg2 as usize >= 32 && is_valid_user_ptr(buf_ptr, 32) { 
                unsafe { for i in 0..32 { *buf_ptr.add(i) = crate::entity::seed::GENETIC_SEED[i]; } }
                frame.rax = 1; 
            } else { frame.rax = EFAULT as u64; }
        },

        521 => { 
            let buf_ptr = arg1 as *mut f32;
            if arg2 as usize >= 4 && is_valid_user_ptr(buf_ptr as *const u8, 16) {
                unsafe {
                    *buf_ptr.add(0) = crate::entity::state::ENTITY_STATE.get_energy();
                    *buf_ptr.add(1) = crate::entity::state::ENTITY_STATE.get_entropy();
                    *buf_ptr.add(2) = crate::entity::state::ENTITY_STATE.get_stability();
                    *buf_ptr.add(3) = crate::entity::state::ENTITY_STATE.get_curiosity();
                }
                frame.rax = 1; 
            } else { frame.rax = EFAULT as u64; }
        },

        522 => { frame.rax = crate::smp::ACTIVE_CORES.load(Ordering::SeqCst) as u64; },
        523 => { frame.rax = crate::scheduler::CONTEXT_SWITCHES.load(Ordering::Relaxed); },
        524 => { 
            // SYSCALL 524: sys_get_system_info
            let info_ptr = arg1 as *mut SystemInfo;
            
            // SECURITY: Prevent Userspace from tricking the Kernel into overwriting Ring 0 memory!
            if !is_valid_user_ptr(info_ptr as *const u8, core::mem::size_of::<SystemInfo>()) {
                frame.rax = EFAULT as u64;
                return;
            }
            
            unsafe {
                // 1. Thermal Telemetry
                let temp = crate::thermal::get_intel_silicon_temp();
                (*info_ptr).current_temp = temp;
                (*info_ptr).active_cooling = if temp >= 75 { 1 } else { 0 };
                
                // 2. Hardware Fan Telemetry (SMM)
                // FAN_RPM_UNKNOWN when we have no way to read the tachometer. Reporting 0 here was
                // worse than reporting nothing: 0 RPM is a plausible-looking measurement, so it read
                // as "the fans are dead" during a thermal investigation when in fact we simply were
                // not measuring. Don't let a UI state a number it doesn't have.
                (*info_ptr).cpu_fan_rpm = crate::laptop_fans::fan_rpm(0);
                (*info_ptr).gpu_fan_rpm = crate::laptop_fans::fan_rpm(1);

                // Name the machine next to that, so "fans not readable" says which machine we have
                // no driver for rather than leaving it a mystery.
                let mut machine = [0u8; 80];
                crate::smbios::machine_string(&mut machine);
                (*info_ptr).machine = machine;


                // 3. Task Scheduler Telemetry
                let mut count = 0;
                if let Some(cores) = &crate::percpu::PER_CPU {
                    for core in cores.iter() {
                        for task in core.scheduler.tasks.iter() {
                            if task.cpu_ticks > 0 || task.state == crate::scheduler::TaskState::Running {
                                if count < 64 {
                                    (*info_ptr).tasks[count] = TaskInfo {
                                        pid: task.pid,
                                        cpu_ticks: task.cpu_ticks,
                                        state: task.state as u8,
                                        name: task.name,
                                    };
                                    count += 1;
                                }
                            }
                        }
                    }
                }
                (*info_ptr).task_count = count as u64;
            }
            frame.rax = 0;
        },
        525 => { 
            // SYSCALL 525: sys_sleep_ms (THE SELF-HEALING FIX)
            let ms = arg1 as u64;
            let wake_ms = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) + ms; 
            
            unsafe {
                // 1. We MUST re-enable interrupts so the APIC timer can tick while we sleep!
                x86_64::instructions::interrupts::enable();
                
                loop {
                    let percpu = crate::percpu::current();
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    {
                        let task = &mut percpu.scheduler.tasks[curr_idx];
                        task.state = crate::scheduler::TaskState::Blocked;
                        task.wake_tsc = wake_ms; 
                    }
                    
                    // 2. Yield the CPU
                    core::arch::asm!("int 0x41"); 
                    
                    // 3. When we wake up, check WHY we woke up
                    let percpu = crate::percpu::current();
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    let task = &mut percpu.scheduler.tasks[curr_idx];
                    
                    if task.wake_tsc == 0 { break; } // Human Input Override (Mouse Touched!)
                    if crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) >= wake_ms { break; } // Time passed!
                    
                    // 4. If we woke up illegally (scheduler fallback), HALT to save battery!
                    x86_64::instructions::hlt(); 
                }
                
                // 5. Safely disable interrupts before returning to the syscall dispatcher
                x86_64::instructions::interrupts::disable();
            }
            frame.rax = 0;
        },

        526 => { 
            let buf_ptr = arg1 as *mut u8;
            let max_len = arg2 as usize;
            
            if !is_valid_user_ptr(buf_ptr, max_len) { 
                frame.rax = EFAULT as u64; 
                return; 
            }
            frame.rax = crate::acpi::get_dsdt_data(buf_ptr, max_len) as u64;
        },

        530 => { // SYS_CREATE_SHM
            let size = arg1 as usize;
            if let Some(id) = crate::memory::create_shm_block(size) {
                frame.rax = id;
            } else { frame.rax = 0; }
        },

        539 => { // SYS_DESTROY_SHM(id, base_vaddr) — R1: owner frees a resized-away buffer.
            let id = arg1;
            let base_vaddr = arg2;
            frame.rax = if crate::memory::destroy_shm_block(id, base_vaddr) { 1 } else { 0 };
        },

        540 => { // SYS_UNMAP_SHM(base_vaddr, size) — R1: release caller's mapping only (no frame free).
            let base_vaddr = arg1;
            let size = arg2 as usize;
            crate::memory::unmap_shm_range(base_vaddr, size);
            frame.rax = 1;
        },

        531 => { // SYS_MAP_SHM
            let shm_id = arg1;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            // Align the bump allocator to a 2MB boundary to ensure SHM never overlaps with normal heap
            let target_addr = (task.mmap_bump + 0x1FFFFF) & !0x1FFFFF; 
            
            let size = {
                let reg = crate::memory::SHM_REGISTRY.lock();
                if let Some(b) = reg.iter().find(|b| b.id == shm_id) { b.size } else { 0 }
            };
            
            if size > 0 {
                let num_pages = (size + 0xFFF) / 0x1000;
                task.mmap_bump = target_addr + ((num_pages as u64) * 0x1000); 
                
                if let Ok(vaddr) = crate::memory::map_shm_block(shm_id, target_addr) {
                    frame.rax = vaddr;
                } else { frame.rax = 0; }
            } else { frame.rax = 0; }
        },
        532 => { // SYS_IPC_SEND
            let target_pid = arg1;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let sender_pid = percpu.scheduler.tasks[curr_idx].pid;
            
            let msg = crate::process::IpcMessage {
                sender_pid, msg_type: arg2, data1: arg3, data2: arg4,
            };
            
            let mut found = false;
            unsafe {
                let active_cores = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                if let Some(cores) = &mut crate::percpu::PER_CPU {
                    for i in 0..active_cores {
                        for task in cores[i].scheduler.tasks.iter_mut() {
                            if task.pid == target_pid {
                                task.mailbox.push_back(msg);
                                // If the task was sleeping forever waiting for IPC, wake it up!
                                if task.state == crate::scheduler::TaskState::Blocked && task.wake_tsc == u64::MAX {
                                    task.state = crate::scheduler::TaskState::Ready;
                                    task.wake_tsc = 0;
                                }
                                found = true;
                                break;
                            }
                        }
                        if found { break; }
                    }
                }
            }
            frame.rax = if found { 1 } else { 0 }; 
        },

        533 => { 
            // SYSCALL 533: sys_ipc_recv
            let msg_ptr = arg1 as *mut crate::process::IpcMessage;
            let block = arg2 == 1;
            
            if !is_valid_user_ptr(msg_ptr as *const u8, core::mem::size_of::<crate::process::IpcMessage>()) {
                frame.rax = EFAULT as u64; return;
            }
            
            if block {
                unsafe {
                    // Re-enable interrupts to prevent timer deadlocks
                    x86_64::instructions::interrupts::enable();
                    loop {
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        let task = &mut percpu.scheduler.tasks[curr_idx];
                        
                        if let Some(msg) = task.mailbox.pop_front() {
                            *msg_ptr = msg;
                            frame.rax = 1;
                            break;
                        }
                        
                        task.state = crate::scheduler::TaskState::Blocked;
                        task.wake_tsc = u64::MAX; 
                        
                        core::arch::asm!("int 0x41"); 
                        
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        let task = &mut percpu.scheduler.tasks[curr_idx];
                        
                        if task.mailbox.is_empty() {
                            x86_64::instructions::hlt();
                        }
                    }
                    x86_64::instructions::interrupts::disable();
                }
            } else {
                let percpu = crate::percpu::current();
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                let task = &mut percpu.scheduler.tasks[curr_idx];
                if let Some(msg) = task.mailbox.pop_front() {
                    unsafe { *msg_ptr = msg; }
                    frame.rax = 1; 
                } else {
                    frame.rax = 0; 
                }
            }
        },
        534 => { 
            frame.rax = sys_dns_resolve(arg1 as usize, arg2 as usize); 
        },
        // An unimplemented syscall must be distinguishable from a bad argument.
        //
        // This arm used to return EINVAL, which made "this kernel has never heard of syscall N"
        // byte-identical to "you passed nonsense to a syscall it does have". Nothing could tell the
        // two apart — not the PAL (which now maps ENOSYS to `ErrorKind::Unsupported`), not a libc,
        // and not a person reading a log. The serial line is the other half: a port failing on a
        // missing syscall should say WHICH one, rather than leaving it to be bisected.
        _ => {
            // ★ Logged ONCE per syscall number, not once per call.
            //
            // Unbounded logging here is itself a machine freeze. SYSCALL clears IF, so this line is
            // emitted with interrupts DISABLED, and the UART runs at roughly 11 KB/s. A libc that
            // retries an unknown syscall in a loop therefore parks the CPU in the serial port with
            // interrupts off — no timer, no scheduler, no mouse. Indistinguishable from a hang.
            //
            // This was harmless while nothing reached the arm. The first real libc reached it
            // immediately, which is exactly when a diagnostic aid turns into the fault.
            static UNIMPL_LOGGED: [core::sync::atomic::AtomicU64; 16] =
                [const { core::sync::atomic::AtomicU64::new(0) }; 16];
            let n = id as usize;
            if n < 1024 {
                let prev = UNIMPL_LOGGED[n / 64]
                    .fetch_or(1u64 << (n % 64), core::sync::atomic::Ordering::Relaxed);
                if prev & (1u64 << (n % 64)) == 0 {
                    crate::serial_println!("[SYSCALL] unimplemented syscall {} (logged once)", id);
                }
            }
            frame.rax = ENOSYS as u64;
        }
    }
}

/// Validate the `struct iovec[]` that readv/writev are handed, before the arm dereferences it.
///
/// Two things the old writev did not do. `iovcnt * 16` can OVERFLOW, and an overflowed product
/// makes the range check pass for an arbitrarily large array — so cap at IOV_MAX first. And the
/// array itself is read by the KERNEL (`*iov_ptr.add(..)`), so a range check is not enough:
/// `is_valid_user_ptr` cannot tell mapped from unmapped, and a kernel-mode read of an unmapped
/// address panics the MACHINE rather than killing the caller. The individual `iov_base` pointers
/// need no check here — they are passed to sys_read_internal / sys_write_internal, which validate
/// exactly as they would for a plain read(2)/write(2).
fn iov_array_ok(iov_ptr: *const u64, iovcnt: usize) -> bool {
    const IOV_MAX: usize = 1024;
    if iovcnt == 0 || iovcnt > IOV_MAX { return false; }
    let bytes = iovcnt * 16;
    if !is_valid_user_ptr(iov_ptr as *const u8, bytes) { return false; }
    // Check every page the array spans, not just the first: it can straddle a boundary where the
    // second page is absent.
    let start = iov_ptr as u64;
    let mut a = start & !0xFFF;
    while a < start + bytes as u64 {
        if !unsafe { crate::memory::user_addr_mapped(a) } { return false; }
        a += 4096;
    }
    true
}

fn sys_read_internal(fd: usize, buf_ptr: *mut u8, len: usize) -> isize {
    if !is_valid_user_ptr(buf_ptr, len) { return EFAULT as isize; }
    if len == 0 || fd >= FD_MAX { return EBADF as isize; }
    
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF as isize; }
    let percpu = crate::percpu::current();
    let p_addr = percpu as *const _ as u64;
    if unsafe { core::ptr::read_volatile(&p_addr) } == 0 { return EBADF as isize; }
    
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return EBADF as isize; }
    
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    if let Some(fd_enum) = &task.fd_table[fd] {
        match fd_enum {
            FileDescriptor::File(open_file) => {
                let buf_slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return open_file.read(buf_slice) as isize;
            },
            FileDescriptor::Socket(sock_mtx) => {
                // ★ Copy everything needed out of the fd table BEFORE the wait loop below, which
                // re-enables interrupts and yields via `int 0x41`. `task` is a raw &mut into the
                // scheduler's task Vec; holding a borrow of its fd_table across a deschedule would
                // dangle if that Vec reallocated while this task was away.
                let (kind, is_non_blocking, stack, gen, timeout_ms) = {
                    let s = sock_mtx.lock();
                    (s.kind.clone(), s.non_blocking, s.stack, s.gen, s.timeout_ms)
                };
                crate::drivers::net::poll_stack(stack);

                match kind {
                    SocketKind::Udp(handle) => {
                        let got = crate::drivers::net::with_sockets(stack, gen, |sockets| {
                            let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(handle);
                            match socket.recv() {
                                Ok((data, _meta)) => {
                                    let copy_len = core::cmp::min(data.len(), len);
                                    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr, copy_len); }
                                    Some(copy_len as isize)
                                }
                                Err(_) => None,
                            }
                        });
                        if let Some(Some(n)) = got { return n; }
                        return EAGAIN as isize;
                    },
                    SocketKind::Tcp(handle) => {
                        let start_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                        loop {
                            crate::drivers::net::poll_stack(stack);

                            let got = crate::drivers::net::with_sockets(stack, gen, |sockets| {
                                let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                                if socket.can_recv() {
                                    let user_slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                                    match socket.recv_slice(user_slice) {
                                        Ok(n) if n > 0 => Some(n as isize),
                                        _ => None,
                                    }
                                } else if !socket.may_recv() {
                                    Some(0) // Connection closed gracefully (EOF)
                                } else {
                                    None
                                }
                            });
                            match got {
                                Some(Some(n)) => return n,
                                Some(None) => {}   // nothing readable yet
                                None => return 0,  // link (and its whole SocketSet) went away: EOF
                            }

                            if is_non_blocking {
                                return EAGAIN as isize; // Async bypass
                            }

                            // Bail out on a stalled peer rather than blocking this thread forever.
                            if timeout_ms != 0 {
                                let now = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                                if now.saturating_sub(start_ms) > timeout_ms {
                                    return ETIMEDOUT as isize;
                                }
                            }

                            // Yield thread safely while waiting for new hardware frames
                            unsafe {
                                x86_64::instructions::interrupts::enable();
                                core::arch::asm!("int 0x41");
                                x86_64::instructions::interrupts::disable();
                            }
                        }
                    }
                }
            },
            FileDescriptor::PipeRead(pipe_mtx) => {
                let mut pipe = pipe_mtx.lock();
                let mut bytes_read = 0;
                while bytes_read < len {
                    if let Some(b) = pipe.pop_front() {
                        unsafe { *buf_ptr.add(bytes_read) = b; }
                        bytes_read += 1;
                    } else { break; }
                }
                
                if bytes_read == 0 { 
                    if alloc::sync::Arc::strong_count(pipe_mtx) == 1 {
                        return 0; // EOF
                    } else {
                        return EAGAIN as isize; 
                    }
                }
                return bytes_read as isize;
            },
            FileDescriptor::PipeWrite(_) => return EBADF as isize,
            FileDescriptor::UnixStream { rx, .. } => {
                let mut q = rx.lock();
                let mut n = 0;
                while n < len {
                    match q.pop_front() {
                        Some(b) => { unsafe { *buf_ptr.add(n) = b; } n += 1; }
                        None => break,
                    }
                }
                if n == 0 {
                    // ★ Same rule as the pipe arm, and the reason `rx` must be BORROWED from the
                    // fd table rather than cloned: a count of 1 means we are the only holder, so
                    // the peer's `tx` is gone and no more data can ever arrive — EOF. Any extra
                    // reference makes that unreachable and the reader hangs forever instead.
                    if alloc::sync::Arc::strong_count(rx) == 1 { return 0; }
                    return EAGAIN as isize;
                }
                return n as isize;
            }
        }
    }
    EBADF as isize
}

fn sys_write_internal(fd: usize, buf_ptr: *const u8, len: usize) -> isize {
    if !is_valid_user_ptr(buf_ptr, len) { return EFAULT as isize; }
    if len == 0 || fd >= FD_MAX { return EBADF as isize; }
    
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF as isize; }
    let percpu = crate::percpu::current();
    let p_addr = percpu as *const _ as u64;
    if unsafe { core::ptr::read_volatile(&p_addr) } == 0 { return EBADF as isize; }
    
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return EBADF as isize; }
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    let buf_slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };

    if let Some(fd_enum) = &task.fd_table[fd] {
        match fd_enum {
            FileDescriptor::File(open_file) => return open_file.write(buf_slice) as isize,
            FileDescriptor::Socket(sock_mtx) => {
                // Same reason as the read path: snapshot the socket before any yield, never hold a
                // borrow of the fd table across `int 0x41`.
                let (kind, is_non_blocking, stack, gen, remote, timeout_ms) = {
                    let s = sock_mtx.lock();
                    (s.kind.clone(), s.non_blocking, s.stack, s.gen, s.remote, s.timeout_ms)
                };
                crate::drivers::net::poll_stack(stack);

                match kind {
                    SocketKind::Udp(handle) => {
                        let endpoint = match remote { Some(e) => e, None => return EAGAIN as isize };
                        let sent = crate::drivers::net::with_sockets(stack, gen, |sockets| {
                            let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(handle);
                            socket.send_slice(buf_slice, endpoint).is_ok()
                        });
                        if sent == Some(true) { return buf_slice.len() as isize; }
                        return EAGAIN as isize;
                    },
                    SocketKind::Tcp(handle) => {
                        let start_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                        loop {
                            crate::drivers::net::poll_stack(stack);

                            let sent = crate::drivers::net::with_sockets(stack, gen, |sockets| {
                                let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                                if socket.can_send() { socket.send_slice(buf_slice).ok() } else { None }
                            });
                            match sent {
                                Some(Some(n)) => return n as isize,
                                Some(None) => {}                 // send window full; wait
                                None => return EBADF as isize,   // link went away
                            }

                            if is_non_blocking {
                                return EAGAIN as isize;
                            }

                            // A peer that stops draining our send window must not wedge us forever.
                            if timeout_ms != 0 {
                                let now = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                                if now.saturating_sub(start_ms) > timeout_ms {
                                    return ETIMEDOUT as isize;
                                }
                            }

                            unsafe {
                                x86_64::instructions::interrupts::enable();
                                core::arch::asm!("int 0x41");
                                x86_64::instructions::interrupts::disable();
                            }
                        }
                    }
                }
            },
            FileDescriptor::PipeWrite(pipe_mtx) => {
                let mut pipe = pipe_mtx.lock();
                for &b in buf_slice { pipe.push_back(b); }
                return len as isize;
            },
            FileDescriptor::PipeRead(_) => return EBADF as isize,
            FileDescriptor::UnixStream { tx, .. } => {
                // A write with no reader left should be EPIPE (and SIGPIPE) on a real system.
                // Reported as EPIPE rather than silently succeeding, because an IPC peer that
                // died is exactly the case the caller must notice; buffering into a queue nobody
                // will ever drain would present as a hang instead.
                if alloc::sync::Arc::strong_count(tx) == 1 { return EPIPE as isize; }
                let mut q = tx.lock();
                for &b in buf_slice { q.push_back(b); }
                return len as isize;
            }
        }
    }

    if fd == 1 || fd == 2 {
        if let Ok(s) = core::str::from_utf8(buf_slice) {
            crate::serial_print!("{}", s); 
        }
        return len as isize;
    }
    EBADF as isize
}

// Add these POSIX error code constants at the top of interrupts.rs if they aren't there:
const EINPROGRESS: i64 = -115;
const ECONNREFUSED: i64 = -111;
const ETIMEDOUT: i64 = -110;

/// Abort and free a socket in whichever stack owns it.
///
/// Takes the socket's identity BY VALUE so no borrow of the fd table is alive across the call. It
/// never blocks on a WiFi lock — if that set is busy the free is deferred to the poller — so this is
/// safe from the interrupts-disabled teardown paths.
fn destroy_socket(stack: crate::drivers::net::NetStack, gen: u64, kind: SocketKind) {
    crate::drivers::net::destroy_socket_deferred(stack, gen, kind);
}

#[no_mangle]
pub extern "C" fn sys_socket(_domain: u64, _typ: u64, _protocol: u64) -> i64 {
    // ★ The domain used to be ignored outright, so socket(AF_UNIX, ...) returned a TCP socket
    // that would never connect to anything — a silent wrong answer rather than an error, and the
    // worst possible failure for IPC code, which then blocks on a descriptor that cannot work.
    // AF_INET(2) is what the stack below actually implements; AF_UNIX(1) has no named sockets
    // here, so it is refused and callers are expected to use socketpair(2), which does exist.
    const AF_INET: u64 = 2;
    if _domain != AF_INET { return EAFNOSUPPORT; }

    // Route to whichever stack actually has a link. Chosen ONCE, here, and remembered on the socket:
    // the handle below is an index into this stack's SocketSet and is meaningless in the other one.
    let (stack, gen) = crate::drivers::net::active_stack();

    // 🔥 MILESTONE 3.1: Linux checks if SOCK_NONBLOCK (2048 / 0x800) is set in the type parameter
    let is_non_blocking = (_typ & 2048) != 0;
    let clean_type = _typ & !2048; // Strip the flag out to get the raw type (1 = TCP, 2 = UDP)
    let local_port = NEXT_LOCAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let handle = crate::drivers::net::with_sockets(stack, gen, |sockets| {
        if clean_type == 1 {
            let rx_buffer = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0; 32768]);
            let tx_buffer = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0; 32768]);
            let socket = smoltcp::socket::tcp::Socket::new(rx_buffer, tx_buffer);
            sockets.add(socket)
        } else {
            let rx_buffer = smoltcp::socket::udp::PacketBuffer::new(alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 8], alloc::vec![0; 2048]);
            let tx_buffer = smoltcp::socket::udp::PacketBuffer::new(alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 8], alloc::vec![0; 2048]);
            let mut socket = smoltcp::socket::udp::Socket::new(rx_buffer, tx_buffer);
            let _ = socket.bind(local_port);
            sockets.add(socket)
        }
    });

    if let Some(handle) = handle {
        let kind = if clean_type == 1 { SocketKind::Tcp(handle) } else { SocketKind::Udp(handle) };

        // ★ Every failure path from here on must remove the socket again. It is already in the
        // SocketSet, and each one owns 64 KiB of TX/RX buffers — leaking one per failed open would
        // exhaust the heap under a browser, which opens sockets constantly.
        let placed = 'place: {
            if KERNEL_CR3.load(Ordering::Relaxed) == 0 { break 'place None; }
            let percpu = crate::percpu::current();

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { break 'place None; }

            let task = &mut percpu.scheduler.tasks[curr_idx];
            for i in 3..32 {
                if task.fd_table[i].is_none() {
                    let ks = KernelSocket {
                        kind: kind.clone(), local_port, remote: None,
                        non_blocking: is_non_blocking, stack, gen,
                        timeout_ms: 0, // block forever until userspace asks otherwise (syscall 549)
                    };
                    task.fd_table[i] = Some(FileDescriptor::Socket(Arc::new(Mutex::new(ks))));
                    break 'place Some(i as i64);
                }
            }
            None
        };

        match placed {
            Some(fd) => return fd,
            None => destroy_socket(stack, gen, kind),
        }
    }
    -24 // EMFILE
}

#[no_mangle]
pub extern "C" fn sys_connect(fd: usize, addr_ptr: *const u8, addr_len: usize) -> i64 {
    if addr_len < 16 || !is_valid_user_ptr(addr_ptr, addr_len) { return EFAULT; }
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF; }
    
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return EBADF; }
    
    let sockaddr = unsafe { &*(addr_ptr as *const SockAddrIn) };
    if sockaddr.sin_family != 2 { return EINVAL; }

    let port = u16::from_be(sockaddr.sin_port);
    let ip = sockaddr.sin_addr;
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    if fd >= FD_MAX { return EBADF; }
    
    if let Some(FileDescriptor::Socket(sock_mtx)) = &task.fd_table[fd] {
        let mut sock = sock_mtx.lock();
        let addr = IpAddress::Ipv4(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]));
        sock.remote = Some(IpEndpoint::new(addr, port));
        
        if let SocketKind::Tcp(handle) = sock.kind {
            let local_port = sock.local_port;
            let is_non_blocking = sock.non_blocking;
            let stack = sock.stack;
            let gen = sock.gen;

            match crate::drivers::net::with_stack(stack, gen, |sockets, iface| {
                let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                socket.connect(iface.context(), IpEndpoint::new(addr, port), local_port).is_ok()
            }) {
                Some(true) => {}
                Some(false) => return ECONNREFUSED,
                None => return EBADF, // stack gone (link dropped) or never came up
            }

            // Drop lock while we poll/sleep to avoid cross-thread deadlocks
            drop(sock);

            // 🔥 MILESTONE 3.1 & 3.3: Handling Blocking vs Non-Blocking states + State Transitions
            if is_non_blocking {
                return EINPROGRESS; // Return instantly for async GUI apps
            }

            // Blocking Loop: Put thread to sleep until TCP handshakes finish or fail
            let start_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
            loop {
                crate::drivers::net::poll_stack(stack);

                match crate::drivers::net::with_sockets(stack, gen, |sockets| {
                    sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle).state()
                }) {
                    Some(smoltcp::socket::tcp::State::Established) => return 0,
                    Some(smoltcp::socket::tcp::State::Closed) => return ECONNREFUSED,
                    Some(_) => {}
                    None => return ECONNREFUSED, // link went away mid-handshake
                }

                // Timeout check (safely timeout after 10 seconds)
                let current_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                if current_ms.saturating_sub(start_ms) > 10000 {
                    return ETIMEDOUT;
                }

                // Yield the CPU core to other threads while waiting for the network card interrupt
                unsafe {
                    x86_64::instructions::interrupts::enable();
                    core::arch::asm!("int 0x41");
                    x86_64::instructions::interrupts::disable();
                }
            }
        }
        return 0;
    }
    EBADF
}

pub extern "x86-interrupt" fn rtl8168_interrupt_handler(_stack_frame: x86_64::structures::idt::InterruptStackFrame) {
   
    crate::serial_println!("[ISR] Hardware Interrupt Fired! NIC Woke up the CPU!");
    
    crate::drivers::net::NETWORK_PENDING.store(true, core::sync::atomic::Ordering::Release);
    crate::apic::end_of_interrupt();
}

// Domain Name Resolution (DNS)
#[no_mangle]
pub extern "C" fn sys_dns_resolve(hostname_ptr: usize, hostname_len: usize) -> u64 {
    if hostname_len == 0 { return 0; }
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return 0; }
    
    let hostname_slice = unsafe { core::slice::from_raw_parts(hostname_ptr as *const u8, hostname_len) };
    let hostname_str = match core::str::from_utf8(hostname_slice) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    // Resolve on whichever stack has the link, and read that stack's DNS socket — the WiFi one is
    // configured from the DHCP lease the driver negotiated, the wired one from its own DHCP.
    let (stack, gen) = crate::drivers::net::active_stack();
    crate::serial_println!("[DNS] Resolving {} via {:?} (gen {})", hostname_str, stack, gen);

    let dns_handle = match crate::drivers::net::dns_handle(stack) {
        Some(h) => h,
        None => {
            // Wired: poll_network has never built its socket set. WiFi: no interface, or its
            // handle lock was busy. Either way there is no resolver to ask.
            crate::serial_println!("[DNS] no DNS socket on {:?} — is the link up?", stack);
            return 0;
        }
    };

    // 1. Fire the DNS Query
    let query_handle = match crate::drivers::net::with_stack(stack, gen, |sockets, iface| {
        let dns_socket = sockets.get_mut::<smoltcp::socket::dns::Socket>(dns_handle);
        dns_socket.start_query(iface.context(), hostname_str, smoltcp::wire::DnsQueryType::A)
    }) {
        Some(Ok(handle)) => handle,
        Some(Err(e)) => {
            crate::serial_println!("[DNS] Failed to start query: {:?}", e);
            return 0;
        }
        None => {
            crate::serial_println!("[DNS] {:?} stack has no interface/socket set (gen {})", stack, gen);
            return 0;
        }
    };

    let start_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
    
    // 2. Safely Block until the DNS Server Replies
    loop {
        crate::drivers::net::poll_stack(stack);

        let result = crate::drivers::net::with_sockets(stack, gen, |sockets| {
            let dns_socket = sockets.get_mut::<smoltcp::socket::dns::Socket>(dns_handle);
            match dns_socket.get_query_result(query_handle) {
                Ok(addrs) => {
                    for addr in addrs.iter() {
                        if let smoltcp::wire::IpAddress::Ipv4(ipv4) = addr {
                            let o = ipv4.0;
                            // Pack the 4 bytes into a single u64 to return across the Syscall boundary
                            return Some((o[0] as u64) | ((o[1] as u64) << 8)
                                | ((o[2] as u64) << 16) | ((o[3] as u64) << 24));
                        }
                    }
                    Some(0) // answered, but no IPv4 in it
                }
                Err(smoltcp::socket::dns::GetQueryResultError::Pending) => None,
                Err(_) => {
                    crate::serial_println!("[DNS] Query Failed/NXDOMAIN.");
                    Some(0)
                }
            }
        });
        match result {
            Some(Some(packed)) => {
                if packed != 0 {
                    crate::serial_println!("[DNS] Resolved {} -> {}.{}.{}.{}", hostname_str,
                        packed & 0xff, (packed >> 8) & 0xff, (packed >> 16) & 0xff, (packed >> 24) & 0xff);
                }
                return packed;
            }
            Some(None) => {}   // still waiting
            None => return 0,  // stack went away
        }

        let current_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
        if current_ms.saturating_sub(start_ms) > 5000 {
            crate::serial_println!("[DNS] Timeout.");
            // Cancel before giving up. `get_query_result` frees the slot on success or failure, but
            // a query still Pending is freed ONLY here — and the socket is built with a growable
            // `vec![]`, so an abandoned query is not merely a leaked slot: smoltcp keeps
            // retransmitting it forever, and every `poll_stack` (which every blocking socket
            // syscall calls in a tight loop) walks the whole list. Enough timeouts and the machine
            // spends all its time re-sending dead lookups.
            crate::drivers::net::with_sockets(stack, gen, |sockets| {
                sockets
                    .get_mut::<smoltcp::socket::dns::Socket>(dns_handle)
                    .cancel_query(query_handle);
            });
            return 0;
        }

        // Sleep the thread via the Scheduler Gateway
        unsafe {
            x86_64::instructions::interrupts::enable();
            core::arch::asm!("int 0x41");
            x86_64::instructions::interrupts::disable();
        }
    }
}