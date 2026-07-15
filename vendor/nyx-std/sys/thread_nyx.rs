//! Nyx thread PAL. B-β.2c: real `thread::spawn`/`join`.
//!
//! Kernel contract (`spawn_thread`, syscall 58, see nyx-kernel/src/interrupts.rs): the new task
//! shares the parent's cr3 + fds, starts at `entry_point` with `RSP = user_stack` and **all GP
//! registers zeroed** — no argument is passed. So the parent pushes an argument record onto the
//! worker's fresh stack and the trampoline pops it off `[rsp]` itself.
//!
//! Division of labour: the PARENT builds the worker's stack AND TLS block (both mmaps happen on the
//! parent, avoiding concurrent-mmap races on the shared address space); the WORKER only installs the
//! prepared TCB via arch_prctl (FS is per-thread hardware state) before touching any thread-local.
//! Exit goes through `exit(60)` — since B-β.2a the kernel detects sibling threads sharing the cr3
//! and skips the address-space teardown (thread exit self-reaps the task slot).
//!
//! `join` is a futex wait on a completion word owned by an Arc shared between the `Thread` handle
//! and the worker; no kernel join primitive is needed.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::ffi::CStr;
use crate::io;
use crate::num::NonZero;
use crate::sync::atomic::{Atomic, Ordering};
use crate::sync::Arc;
use crate::sys::futex::{futex_wait, futex_wake_all};
// `tls` lives in pal/nyx and reaches us via sys/mod.rs's `pub use pal::*`, same as `sys::futex`.
use crate::sys::tls;
use crate::thread::ThreadInit;
use crate::time::Duration;

const SYS_NANOSLEEP: usize = 35;
const SYS_SCHED_YIELD: usize = 24;
const SYS_SPAWN_THREAD: usize = 58;
const SYS_EXIT: usize = 60;
const SYS_MMAP: usize = 9;
const SYS_GET_ACTIVE_CORES: usize = 522;

pub const DEFAULT_MIN_STACK_SIZE: usize = 256 * 1024;

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

/// Everything the trampoline needs, boxed by the parent and leaked across the spawn. The worker
/// reads the pointer to this off its initial stack.
struct SpawnArgs {
    /// TCB address of the worker's parent-built TLS block; the trampoline arch_prctl(SET_FS)s it.
    tcb: usize,
    /// std's thread-init payload: sets up thread::current(), returns the user closure.
    init: Box<ThreadInit>,
    /// Completion word: 0 = running, 1 = finished. The worker stores 1 + futex-wakes; join waits.
    done: Arc<Atomic<u32>>,
}

pub struct Thread {
    done: Arc<Atomic<u32>>,
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

/// Worker entry. Reached via `spawn_thread` with RSP -> [ &SpawnArgs ] and all GP regs zeroed.
/// MUST be naked: a normal fn's prologue would move RSP before we can read the argument slot.
/// Pops &SpawnArgs into rdi, aligns the stack, and calls the real (Rust, extern "C") body.
#[unsafe(naked)]
unsafe extern "C" fn thread_trampoline() -> ! {
    core::arch::naked_asm!(
        "mov rdi, [rsp]",   // &SpawnArgs, placed at the stack top by Thread::new
        "and rsp, -16",     // SysV alignment for the call below
        "call {main}",
        "ud2",              // thread_main never returns
        main = sym thread_main,
    )
}

/// Rust half of the worker entry. Never returns: ends in exit(60), which the B-β.2a kernel treats
/// as a THREAD exit (slot self-reap, shared address space untouched) because the parent still holds
/// the cr3.
unsafe extern "C" fn thread_main(args_ptr: *mut SpawnArgs) -> ! {
    let args = Box::from_raw(args_ptr);

    // Install this thread's TLS BEFORE any thread-local access (ThreadInit::init is full of them).
    tls::set_fs(args.tcb);

    // std thread bring-up, then the user closure. Panics inside the closure abort the process
    // (panic=abort target), so no unwind-catch is needed here.
    let done = args.done;
    let rust_start = args.init.init();
    rust_start();

    // Signal completion and wake every joiner, then exit this task only.
    done.store(1, Ordering::Release);
    futex_wake_all(&done);
    drop(done); // release our Arc ref before the syscall that never returns

    core::arch::asm!(
        "syscall",
        in("rax") SYS_EXIT,
        in("rdi") 0usize,
        options(noreturn),
    );
}

impl Thread {
    // unsafe: see thread::Builder::spawn_unchecked for safety requirements
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        // TLS is mandatory for a std thread (thread::current lives there). The template was cached
        // by the main thread's __nyx_setup_main_tls; a binary with no PT_TLS can't spawn.
        let tpl = match tls::cached_template() {
            Some(t) => t,
            None => return Err(io::Error::UNSUPPORTED_PLATFORM),
        };
        let tcb = tls::build_tls_block(&tpl);

        // Worker stack: parent-mmapped. Reserve 16 bytes at the very top for the SpawnArgs pointer
        // (and to keep RSP % 16 == 0 at the trampoline's entry `mov`, matching the kernel's iretq).
        let stack_size = stack.max(DEFAULT_MIN_STACK_SIZE);
        let stack_base = mmap_anon(stack_size);
        if stack_base <= 0x1000 || stack_base >= 0xFFFF_FFFF_FFFF_F000 {
            return Err(io::Error::from_raw_os_error(12)); // ENOMEM
        }

        let done = Arc::new(Atomic::<u32>::new(0));
        let args = Box::new(SpawnArgs { tcb, init, done: Arc::clone(&done) });
        let args_ptr = Box::into_raw(args);

        // Place &SpawnArgs at the stack top; hand the kernel an RSP pointing at it.
        let top = (stack_base + stack_size - 16) & !0xF;
        *(top as *mut usize) = args_ptr as usize;

        let tid: isize;
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_SPAWN_THREAD => tid,
            in("rdi") thread_trampoline as usize,
            in("rsi") top,
            out("rcx") _, out("r11") _, options(nostack),
        );
        if tid <= 0 {
            // Spawn failed: reclaim the args box; the stack + TLS mmaps leak (no munmap yet).
            drop(Box::from_raw(args_ptr));
            return Err(io::Error::from_raw_os_error(11)); // EAGAIN
        }

        Ok(Thread { done })
    }

    pub fn join(self) {
        // Wait until the worker stores 1 and wakes us. The futex loop re-checks on spurious wakes;
        // the kernel's WAIT self-heals with a 10ms re-poll, so a lost wake cannot hang us.
        while self.done.load(Ordering::Acquire) == 0 {
            futex_wait(&self.done, 0, None);
        }
    }
}

pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    let n: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_GET_ACTIVE_CORES => n,
            out("rcx") _, out("r11") _, options(nostack),
        );
    }
    Ok(NonZero::new(n.max(1)).unwrap())
}

pub fn current_os_id() -> Option<u64> {
    None
}

pub fn yield_now() {
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_SCHED_YIELD => _,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
}

pub fn set_name(_name: &CStr) {
    // No per-thread name storage in the kernel yet.
}

pub fn sleep(dur: Duration) {
    // timespec {tv_sec: i64, tv_nsec: i64}; kernel rounds up to whole ms (its tick granularity).
    let ts: [i64; 2] = [dur.as_secs() as i64, dur.subsec_nanos() as i64];
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_NANOSLEEP => _,
            in("rdi") ts.as_ptr(),
            in("rsi") 0usize, // rem
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
}
