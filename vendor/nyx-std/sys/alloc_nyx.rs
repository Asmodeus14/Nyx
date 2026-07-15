//! Nyx global allocator — mmap(9)-backed.
//!
//! A pragmatic first cut for B-α/β: satisfy each allocation from a process-global bump arena that
//! grows by calling mmap when exhausted. `dealloc` is a no-op (memory is reclaimed at process
//! exit). This is enough to bring std up and run real programs; a real free-list replaces this in
//! B-γ once thread-safety + reuse matter under sustained allocation (e.g. qclang compiles).
use crate::alloc::{GlobalAlloc, Layout, System};
use crate::sync::atomic::{AtomicUsize, Ordering};

const MMAP: usize = 9;
const CHUNK: usize = 8 * 1024 * 1024; // grow the arena 8 MiB at a time (kernel caps mmap at 32 MiB/call)

#[inline]
unsafe fn mmap_anon(len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") MMAP => ret,
            in("rdi") 0usize,           // addr = 0 -> kernel bumps mmap_bump
            in("rsi") len,              // size
            in("rdx") 0x3usize,         // prot RW (advisory on Nyx)
            in("r10") 0x22usize,        // MAP_PRIVATE|MAP_ANONYMOUS (advisory)
            in("r8") (-1isize) as usize,// fd = -1
            in("r9") 0usize,            // offset
            out("rcx") _, out("r11") _, options(nostack),
        );
    }
    ret
}

// Current arena cursor and end. Single global arena; the Ordering keeps concurrent bumps sound.
static ARENA_CUR: AtomicUsize = AtomicUsize::new(0);
static ARENA_END: AtomicUsize = AtomicUsize::new(0);
// A tiny spin flag serializes the (rare) arena-refill path so two threads don't mmap simultaneously.
static REFILL_LOCK: AtomicUsize = AtomicUsize::new(0);

unsafe fn arena_alloc(size: usize, align: usize) -> *mut u8 {
    loop {
        let cur = ARENA_CUR.load(Ordering::Relaxed);
        let end = ARENA_END.load(Ordering::Relaxed);
        let aligned = (cur + align - 1) & !(align - 1);
        let next = aligned.checked_add(size);
        match next {
            Some(n) if n <= end && cur != 0 => {
                if ARENA_CUR
                    .compare_exchange(cur, n, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    return aligned as *mut u8;
                }
                // lost the race; retry
            }
            _ => {
                // Need more space: serialize refill.
                while REFILL_LOCK
                    .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    core::hint::spin_loop();
                }
                // Re-check under the lock (another thread may have just refilled).
                if ARENA_END.load(Ordering::Relaxed).wrapping_sub(ARENA_CUR.load(Ordering::Relaxed))
                    < size + align
                    || ARENA_CUR.load(Ordering::Relaxed) == 0
                {
                    let want = if size + align > CHUNK { (size + align + 0xFFF) & !0xFFF } else { CHUNK };
                    let base = unsafe { mmap_anon(want) };
                    // mmap returns a small (errno-ish) value on failure; treat high addresses as ok.
                    if base > 0x1000 && base < 0xFFFF_FFFF_FFFF_F000 {
                        ARENA_CUR.store(base, Ordering::Release);
                        ARENA_END.store(base + want, Ordering::Release);
                    } else {
                        REFILL_LOCK.store(0, Ordering::Release);
                        return core::ptr::null_mut();
                    }
                }
                REFILL_LOCK.store(0, Ordering::Release);
                // loop and try the bump again
            }
        }
    }
}

#[stable(feature = "alloc_system_type", since = "1.28.0")]
unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { arena_alloc(layout.size().max(1), layout.align().max(1)) }
    }

    #[inline]
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump arena: no per-allocation free yet (reclaimed at process exit).
    }
}
