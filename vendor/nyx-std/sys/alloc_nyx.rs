//! Nyx global allocator — mmap(9)-backed, with real reclamation.
//!
//! Supersedes the original B-α/β bump arena, whose `dealloc` was a no-op: memory was only ever
//! reclaimed at process exit. That was fine for short-lived, allocate-once programs (notepad, a
//! qclang compile) but it is fatal for anything long-running — a browser churns allocations on every
//! GC cycle, re-layout and page navigation, so a non-reclaiming arena walks the address space until
//! mmap fails, in minutes.
//!
//! Structure — two tiers, both exact-fit, so a block is always returned to the list it came from:
//!
//!   * **Small (<= 16 KiB): segregated free lists**, one per power-of-two size class. Free blocks are
//!     intrusive — the first word of a free block is the next pointer, which costs no header. A class
//!     is refilled by carving a 64 KiB batch out of the arena, so the alignment padding needed to
//!     start a run of 16 KiB blocks is paid once per batch rather than once per block.
//!   * **Large (> 16 KiB): one address-sorted free list** with `{next, size}` in the block's first two
//!     words, **coalescing with both neighbours on free** and splitting on reuse. Large blocks are the
//!     ones a browser fragments worst (image buffers, JS heap chunks), and without coalescing a
//!     decode-then-free-then-decode-bigger loop would strand the address space in unusable holes.
//!
//! ★ No header is stored on live allocations, because `GlobalAlloc` hands the `Layout` back to
//! `dealloc`. That is what makes exact-fit safe, and it is also the one invariant to preserve when
//! editing: **`dealloc` must classify a block exactly as `alloc` did.** `realloc` therefore may only
//! return the pointer unchanged when the new layout lands in the same class (or, for large blocks,
//! rounds to the same page count) — otherwise the later `dealloc` would file the block under the
//! wrong size.
//!
//! Locking is one global spin lock. Userspace `Thread::new` is unsupported today, so it is
//! uncontended, but `GlobalAlloc` must be sound if that changes. The lock is never held across the
//! `mmap` syscall's return path doing anything that could re-enter the allocator: `mmap_anon` is raw
//! asm and allocates nothing.
use crate::alloc::{GlobalAlloc, Layout, System};
use crate::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const MMAP: usize = 9;
const CHUNK: usize = 8 * 1024 * 1024; // grow the arena 8 MiB at a time (kernel caps mmap at 32 MiB/call)
const PAGE: usize = 4096;

/// Smallest block, and the alignment floor every allocation gets for free.
const MIN_BLOCK: usize = 16;
/// Classes 16 B, 32 B, ... 16 KiB.
const NUM_CLASSES: usize = 11;
const MAX_SMALL: usize = MIN_BLOCK << (NUM_CLASSES - 1);
/// How much to carve per small-class refill.
const BATCH_BYTES: usize = 64 * 1024;
/// At or above this, a large request gets its own mmap instead of coming out of the shared arena —
/// otherwise a single multi-megabyte request could force a refill that abandons an almost-full
/// arena tail.
const DEDICATED: usize = 1024 * 1024;

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

/// mmap reports failure as a small errno-ish value; anything in the normal user range is a mapping.
#[inline]
fn mmap_ok(base: usize) -> bool {
    base > 0x1000 && base < 0xFFFF_FFFF_FFFF_F000
}

// All state below is guarded by LOCK; the atomics are for interior mutability without `static mut`,
// not for lock-free access, hence the uniformly Relaxed ordering.
static LOCK: AtomicBool = AtomicBool::new(false);
static SMALL_FREE: [AtomicUsize; NUM_CLASSES] = [const { AtomicUsize::new(0) }; NUM_CLASSES];
static LARGE_FREE: AtomicUsize = AtomicUsize::new(0);
static ARENA_CUR: AtomicUsize = AtomicUsize::new(0);
static ARENA_END: AtomicUsize = AtomicUsize::new(0);

struct Guard;

impl Guard {
    #[inline]
    fn new() -> Guard {
        while LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Guard
    }
}

impl Drop for Guard {
    #[inline]
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}

/// The size class that serves `size`/`align`, or `None` if this belongs in the large tier.
///
/// Folding alignment into the size is what lets blocks be header-free: a class-`n` block is
/// `MIN_BLOCK << n` bytes and is always carved on that same power-of-two boundary, so satisfying the
/// size implies satisfying the alignment.
#[inline]
fn class_of(size: usize, align: usize) -> Option<usize> {
    let mut need = if size > align { size } else { align };
    if need < MIN_BLOCK {
        need = MIN_BLOCK;
    }
    if need > MAX_SMALL {
        return None;
    }
    // ceil(log2(need)) - log2(MIN_BLOCK); need >= 16 so `need - 1` cannot underflow.
    let bits = (usize::BITS - (need - 1).leading_zeros()) as usize;
    Some(bits - 4)
}

#[inline]
fn round_page(n: usize) -> Option<usize> {
    n.checked_add(PAGE - 1).map(|v| v & !(PAGE - 1))
}

/// Take `size` bytes at `align` from the shared arena, refilling it from mmap if needed.
/// Returns 0 on failure. Caller must hold the lock.
unsafe fn arena_carve(size: usize, align: usize) -> usize {
    loop {
        let cur = ARENA_CUR.load(Ordering::Relaxed);
        let end = ARENA_END.load(Ordering::Relaxed);
        if cur != 0 {
            let aligned = match cur.checked_add(align - 1) {
                Some(v) => v & !(align - 1),
                None => return 0,
            };
            if let Some(next) = aligned.checked_add(size) {
                if next <= end {
                    ARENA_CUR.store(next, Ordering::Relaxed);
                    return aligned;
                }
            }
        }

        // Refill. Donate whatever whole pages are left in the old arena to the large list rather
        // than abandoning them — without this, every refill silently leaks the tail.
        if cur != 0 {
            let t_start = (cur + PAGE - 1) & !(PAGE - 1);
            let t_end = end & !(PAGE - 1);
            if t_end > t_start {
                unsafe { large_free(t_start, t_end - t_start) };
            }
        }

        let want = match size.checked_add(align) {
            Some(v) if v > CHUNK => match round_page(v) {
                Some(w) => w,
                None => return 0,
            },
            Some(_) => CHUNK,
            None => return 0,
        };
        let base = unsafe { mmap_anon(want) };
        if !mmap_ok(base) {
            return 0;
        }
        ARENA_CUR.store(base, Ordering::Relaxed);
        ARENA_END.store(base + want, Ordering::Relaxed);
    }
}

/// Pop a block of `class`, refilling that class from the arena if its list is empty.
unsafe fn small_alloc(class: usize) -> usize {
    let head = SMALL_FREE[class].load(Ordering::Relaxed);
    if head != 0 {
        SMALL_FREE[class].store(unsafe { *(head as *const usize) }, Ordering::Relaxed);
        return head;
    }

    let bs = MIN_BLOCK << class;
    let count = if BATCH_BYTES / bs == 0 { 1 } else { BATCH_BYTES / bs };
    let base = unsafe { arena_carve(bs * count, bs) };
    if base == 0 {
        return 0;
    }

    // Block 0 is returned; thread 1..count onto the free list.
    let mut list = 0usize;
    let mut i = count;
    while i > 1 {
        i -= 1;
        let b = base + i * bs;
        unsafe { *(b as *mut usize) = list };
        list = b;
    }
    SMALL_FREE[class].store(list, Ordering::Relaxed);
    base
}

/// Best-fit over the large list, splitting the remainder back; falls back to a fresh mapping.
unsafe fn large_alloc(size: usize, align: usize) -> usize {
    let need = match round_page(size) {
        Some(v) => v,
        None => return 0,
    };
    let align = if align < PAGE { PAGE } else { align };

    let mut best = 0usize;
    let mut best_prev = 0usize;
    let mut best_size = usize::MAX;
    let mut prev = 0usize;
    let mut cur = LARGE_FREE.load(Ordering::Relaxed);
    while cur != 0 {
        let next = unsafe { *(cur as *const usize) };
        let csz = unsafe { *((cur + 8) as *const usize) };
        if csz >= need && cur % align == 0 && csz < best_size {
            best = cur;
            best_prev = prev;
            best_size = csz;
            if csz == need {
                break;
            }
        }
        prev = cur;
        cur = next;
    }

    if best != 0 {
        let next = unsafe { *(best as *const usize) };
        if best_prev == 0 {
            LARGE_FREE.store(next, Ordering::Relaxed);
        } else {
            unsafe { *(best_prev as *mut usize) = next };
        }
        // Everything on this list is page-rounded, so any surplus is a whole number of pages and
        // can go straight back. Returning exactly `need` keeps alloc/dealloc sizes symmetric.
        if best_size > need {
            unsafe { large_free(best + need, best_size - need) };
        }
        return best;
    }

    // A big request gets its own mapping so it never forces the shared arena to be recycled.
    if need >= DEDICATED {
        let base = unsafe { mmap_anon(need) };
        return if mmap_ok(base) { base } else { 0 };
    }
    unsafe { arena_carve(need, align) }
}

/// Return a large block, merging it with an adjacent free block on either side.
///
/// The list is kept sorted by address purely so this merge is possible. Adjacent blocks originating
/// from different mmap calls may still be merged: the kernel hands out mappings by bumping a cursor,
/// so touching ranges really are contiguous mapped memory.
unsafe fn large_free(ptr: usize, size: usize) {
    let mut prev = 0usize;
    let mut cur = LARGE_FREE.load(Ordering::Relaxed);
    while cur != 0 && cur < ptr {
        prev = cur;
        cur = unsafe { *(cur as *const usize) };
    }

    let mut nsize = size;
    // Merge forward.
    if cur != 0 && ptr + nsize == cur {
        nsize += unsafe { *((cur + 8) as *const usize) };
        cur = unsafe { *(cur as *const usize) };
    }
    // Merge backward — extends the predecessor in place instead of linking a new node.
    if prev != 0 {
        let psz = unsafe { *((prev + 8) as *const usize) };
        if prev + psz == ptr {
            unsafe { *((prev + 8) as *mut usize) = psz + nsize };
            unsafe { *(prev as *mut usize) = cur };
            return;
        }
    }

    unsafe { *(ptr as *mut usize) = cur };
    unsafe { *((ptr + 8) as *mut usize) = nsize };
    if prev == 0 {
        LARGE_FREE.store(ptr, Ordering::Relaxed);
    } else {
        unsafe { *(prev as *mut usize) = ptr };
    }
}

#[stable(feature = "alloc_system_type", since = "1.28.0")]
unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        let align = layout.align().max(1);
        let _g = Guard::new();
        let p = match class_of(size, align) {
            Some(c) => unsafe { small_alloc(c) },
            None => unsafe { large_alloc(size, align) },
        };
        p as *mut u8
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }
        let size = layout.size().max(1);
        let align = layout.align().max(1);
        let _g = Guard::new();
        match class_of(size, align) {
            Some(c) => {
                // Intrusive push: the block is dead, so its first word is ours.
                unsafe { *(ptr as *mut usize) = SMALL_FREE[c].load(Ordering::Relaxed) };
                SMALL_FREE[c].store(ptr as usize, Ordering::Relaxed);
            }
            None => {
                if let Some(n) = round_page(size) {
                    unsafe { large_free(ptr as usize, n) };
                }
            }
        }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let align = layout.align().max(1);
        let old = layout.size().max(1);
        let new = new_size.max(1);

        // Resizing within the same class is a no-op, which turns the usual `Vec` doubling loop into
        // a handful of real moves. Safe only because the later `dealloc` — which sees the NEW layout
        // — will compute this same class and file the block correctly.
        match (class_of(old, align), class_of(new, align)) {
            (Some(a), Some(b)) if a == b => return ptr,
            (None, None) if round_page(old) == round_page(new) => return ptr,
            _ => {}
        }
        unsafe { super::realloc_fallback(self, ptr, layout, new_size) }
    }
}
