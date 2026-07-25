use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};
use core::alloc::{GlobalAlloc, Layout};
use linked_list_allocator::LockedHeap;

pub const HEAP_START: usize = 0x_4444_4444_0000;
pub const HEAP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

/// ★ The kernel heap must be interrupt-safe, not merely thread-safe.
///
/// `LockedHeap` is a bare spin lock, and this heap is shared across the preemption boundary:
/// preemptible contexts allocate with interrupts ON (kernel tasks, page-fault handling, and the
/// syscall arms that re-enable IF around a long operation — a Wi-Fi join allocates heavily), while
/// non-preemptible contexts allocate with interrupts OFF (every syscall, since SYSCALL clears IF via
/// FMASK, and the idle task's `poll_network`, which masks interrupts for its whole critical section).
///
/// That mix deadlocks the machine: the timer preempts a task in the middle of an allocation, the
/// scheduler runs something that allocates with IF=0, and it spins on a lock whose owner can never be
/// rescheduled. On that core nothing progresses again — including the thermal governor, so the box
/// also cooks. It presents as "froze a few seconds after going idle", because going idle is exactly
/// what reliably runs the other allocator.
///
/// Masking interrupts around the lock makes every holder non-preemptible, which removes the cycle.
/// `serial::_print` already does this for SERIAL1; this is the same rule applied to the heap. See the
/// `project_syscall-interrupt-discipline` notes for the kernel-wide statement of it.
struct IrqSafeHeap(LockedHeap);

unsafe impl GlobalAlloc for IrqSafeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        x86_64::instructions::interrupts::without_interrupts(|| self.0.alloc(layout))
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        x86_64::instructions::interrupts::without_interrupts(|| self.0.dealloc(ptr, layout))
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        x86_64::instructions::interrupts::without_interrupts(|| self.0.alloc_zeroed(layout))
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        x86_64::instructions::interrupts::without_interrupts(|| self.0.realloc(ptr, layout, new_size))
    }
}

#[global_allocator]
static ALLOCATOR: IrqSafeHeap = IrqSafeHeap(LockedHeap::empty());

pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE as u64 - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush();
        }
    }

    unsafe {
        // FIX: Pass HEAP_START directly (it is already a usize)
        ALLOCATOR.0.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}