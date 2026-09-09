use x86_64::{
    structures::paging::{
        OffsetPageTable, PageTable, FrameAllocator, Size4KiB, PhysFrame, Mapper, Page, 
        PageTableFlags, Translate, mapper::MapToError 
    },
    VirtAddr, PhysAddr,
};
use bootloader_api::info::MemoryRegionKind;
use spin::Mutex;

use alloc::vec::Vec;

pub struct ShmBlock {
    pub id: u64,
    pub frames: Vec<PhysAddr>,
    pub size: usize,
}

lazy_static::lazy_static! {
    pub static ref SHM_REGISTRY: spin::Mutex<Vec<ShmBlock>> = spin::Mutex::new(Vec::new());
}
static NEXT_SHM_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

lazy_static::lazy_static! {
    pub static ref MEMORY_MANAGER: Mutex<Option<MemorySystem>> = Mutex::new(None);
}

pub static mut PHYS_MEM_OFFSET: u64 = 0;
pub static mut BOOTLOADER_CR3: u64 = 0;

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Physical frame accounting — the numbers System Monitor reports.
//
// Atomics rather than a getter behind `MEMORY_MANAGER`, deliberately. That lock is a bare spin lock
// taken by the page-fault handler, and a syscall arm (which runs at IF=0) blocking on it is the exact
// preemption-boundary deadlock this file's watchdog exists to catch — a monitoring app polling once
// a second would be a standing invitation to it. A counter costs one relaxed add on a path that is
// already walking page tables, and it is readable from anywhere without taking anything.
//
// Relaxed is right: these are a display value with no happens-before relationship to the mapping
// they describe. A reader that catches a torn pair is one frame out of ~4 million.
// ─────────────────────────────────────────────────────────────────────────────────────────────────
pub static FRAMES_TOTAL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static FRAMES_USED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// (used_bytes, total_bytes) of physical RAM. Total counts only regions the bootloader marked
/// `Usable`, so it is smaller than the sticker on the DIMM — firmware, ACPI and MMIO holes are not
/// memory this kernel can ever hand out, and reporting them would make usage look better than it is.
pub fn memory_usage() -> (u64, u64) {
    use core::sync::atomic::Ordering;
    let used = FRAMES_USED.load(Ordering::Relaxed);
    let total = FRAMES_TOTAL.load(Ordering::Relaxed);
    (used.saturating_mul(4096), total.saturating_mul(4096))
}

pub struct MemorySystem {
    pub mapper: OffsetPageTable<'static>,
    pub frame_allocator: BootInfoFrameAllocator,
}

pub unsafe fn init(physical_memory_offset: VirtAddr, memory_map: &'static [bootloader_api::info::MemoryRegion]) -> OffsetPageTable<'static> {
    let level_4_table_frame = x86_64::registers::control::Cr3::read().0;
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    let level_4_table = &mut *page_table_ptr;
    
    let mapper = OffsetPageTable::new(level_4_table, physical_memory_offset);
    let frame_allocator = BootInfoFrameAllocator::init(memory_map, physical_memory_offset);

    *MEMORY_MANAGER.lock() = Some(MemorySystem {
        mapper: OffsetPageTable::new(unsafe { &mut *page_table_ptr }, physical_memory_offset),
        frame_allocator,
    });

    mapper
}

pub unsafe fn active_mapper() -> OffsetPageTable<'static> {
    let phys_offset = VirtAddr::new(PHYS_MEM_OFFSET);
    let active_cr3 = x86_64::registers::control::Cr3::read().0.start_address();
    
    let pml4_virt = phys_offset + active_cr3.as_u64();
    let pml4_ptr = pml4_virt.as_mut_ptr() as *mut PageTable;
    
    OffsetPageTable::new(&mut *pml4_ptr, phys_offset)
}

pub struct BootInfoFrameAllocator {
    memory_map: &'static [bootloader_api::info::MemoryRegion],
    current_region: usize,
    current_offset: u64,
    phys_offset: VirtAddr,
    recycled_frames: Option<PhysFrame>,
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static [bootloader_api::info::MemoryRegion], phys_offset: VirtAddr) -> Self {
        // Count what this allocator will ever be able to hand out. Done here rather than lazily so
        // the total is available before the first allocation, and so it is measured from the same
        // slice the allocator walks — a second source would be free to disagree.
        let total: u64 = memory_map
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .map(|r| r.end.saturating_sub(r.start) / 4096)
            .sum();
        FRAMES_TOTAL.store(total, core::sync::atomic::Ordering::Relaxed);

        BootInfoFrameAllocator {
            memory_map, current_region: 0, current_offset: 0, phys_offset, recycled_frames: None
        }
    }

    pub fn deallocate_frame(&mut self, frame: PhysFrame) {
        let phys_addr = frame.start_address().as_u64();
        let virt_addr = self.phys_offset + phys_addr;
        let ptr = virt_addr.as_mut_ptr() as *mut u64;

        let next_ptr = match self.recycled_frames { Some(f) => f.start_address().as_u64(), None => 0, };
        unsafe { *ptr = next_ptr; }
        self.recycled_frames = Some(frame);
        // Saturating, not a bare `fetch_sub`: a frame freed that this allocator never handed out
        // would wrap the counter to ~16 exabytes used, which is a far worse lie than being one frame
        // optimistic.
        FRAMES_USED
            .fetch_update(
                core::sync::atomic::Ordering::Relaxed,
                core::sync::atomic::Ordering::Relaxed,
                |u| Some(u.saturating_sub(1)),
            )
            .ok();
    }

    pub fn allocate_contiguous_frames(&mut self, num_frames: usize, alignment: u64, below_4gb: bool) -> Option<PhysFrame> {
        let size = (num_frames as u64) * 4096;
        
        // Save state in case we need to search and fail
        let orig_region = self.current_region;
        let orig_offset = self.current_offset;

        while self.current_region < self.memory_map.len() {
            let region = &self.memory_map[self.current_region];
            // ⚠️ Same 1 MB floor as the single-frame path. See `LOW_MEM_RESERVED`.
            if region.kind == MemoryRegionKind::Usable
                && skip_low_memory(region.start, region.end, &mut self.current_offset)
            {
                let mut target_addr = region.start + self.current_offset;

                // Align the target address
                let remainder = target_addr % alignment;
                if remainder != 0 {
                    target_addr += alignment - remainder;
                }

                if target_addr + size <= region.end {
                    if below_4gb && (target_addr + size) > 0x1_0000_0000 {
                        self.current_region += 1;
                        self.current_offset = 0;
                        continue;
                    }

                    // Found a suitable block
                    self.current_offset = (target_addr - region.start) + size;
                    FRAMES_USED.fetch_add(num_frames as u64, core::sync::atomic::Ordering::Relaxed);
                    return Some(PhysFrame::containing_address(PhysAddr::new(target_addr)));
                }
            }
            self.current_region += 1;
            self.current_offset = 0;
        }
        
        // Restore state if failed
        self.current_region = orig_region;
        self.current_offset = orig_offset;
        None
    }
}

/// ★★★★ Physical memory below this is NEVER handed out as a general frame.
///
/// This fixes a real corruption bug, found 2026-09-09 by counting the ACPI namespace at boot
/// checkpoints: **1802 nodes before `smp::init_aps`, 637 and a dangling tail after it.**
///
/// The namespace was not the bug. The ACPI pool's first slab simply sits at the bottom of the kernel
/// heap, and **the bottom of the heap was backed by physical frames below 1 MB**: this allocator
/// walks `Usable` regions from the lowest address upward and excluded nothing, while UEFI marks
/// conventional memory from roughly 0x1000 up as usable.
///
/// Then AP bring-up writes the real-mode trampoline to **physical 0x8000** and its argument block to
/// **0x8F00** — addresses fixed by the architecture, because a starting AP begins in real mode and
/// can only reach the first megabyte. Those writes landed inside the kernel heap.
///
/// ★ The tell was exact: the smashed node's `Name` field read `0xffff9000`, the top half of
/// `*args_ptr.offset(3) = ap_stack_top` — a `0xFFFF_9000_…` address from `allocate_kernel_stack`,
/// written straight through a heap block.
///
/// ⚠️ The namespace was only the WITNESS — the first structure self-describing enough to notice.
/// Anything else allocated early was being corrupted too, silently. Weigh that against any
/// unexplained early-boot oddity in this project's history.
///
/// 1 MiB rather than just the trampoline page: everything below it is legacy territory — the IVT,
/// the BDA, the EBDA, VGA memory, option ROMs, and the trampoline. None of it is safe to hand to a
/// general allocator, and a megabyte is not worth being clever about.
pub const LOW_MEM_RESERVED: u64 = 0x10_0000;

/// Advance `(current_region, current_offset)` past reserved low memory, if it is sitting in it.
///
/// Returns `false` if this whole region is below the line and the caller should move to the next.
/// Shared by both allocation paths on purpose: they had already drifted apart once, and a reservation
/// that only one of them honours is not a reservation.
#[inline]
fn skip_low_memory(region_start: u64, region_end: u64, offset: &mut u64) -> bool {
    if region_start + *offset >= LOW_MEM_RESERVED {
        return true;
    }
    if region_end <= LOW_MEM_RESERVED {
        return false;
    }
    *offset = LOW_MEM_RESERVED - region_start;
    true
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        if let Some(frame) = self.recycled_frames {
            let phys_addr = frame.start_address().as_u64();
            let virt_addr = self.phys_offset + phys_addr;
            let ptr = virt_addr.as_ptr() as *const u64;
            unsafe {
                let next_addr = *ptr;
                if next_addr == 0 { self.recycled_frames = None; }
                else { self.recycled_frames = Some(PhysFrame::containing_address(PhysAddr::new(next_addr))); }
            }
            FRAMES_USED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return Some(frame);
        }

        while self.current_region < self.memory_map.len() {
            let region = &self.memory_map[self.current_region];
            if region.kind == MemoryRegionKind::Usable {
                // ⚠️ Never below 1 MB — see `LOW_MEM_RESERVED`. Physical 0x8000 is the SMP
                // trampoline, and handing it out as heap is what silently smashed the ACPI
                // namespace (and whatever else landed there) on every boot.
                if skip_low_memory(region.start, region.end, &mut self.current_offset) {
                    let target_addr = region.start + self.current_offset;
                    if target_addr + 4096 <= region.end {
                        self.current_offset += 4096;
                        FRAMES_USED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        return Some(PhysFrame::containing_address(PhysAddr::new(target_addr)));
                    }
                }
            }
            self.current_region += 1;
            self.current_offset = 0;
        }
        None
    }
}

pub fn virt_to_phys(virt_addr: u64) -> Option<u64> {
    // Instrumented, NOT watchdogged. `MEMORY_MANAGER` is a bare spin lock with no interrupt
    // masking, and this is called from the NIC, NVMe, AHCI and USB drivers — i.e. from paths that
    // run with interrupts off. That makes it the prime remaining suspect for a core that wedges
    // hard enough to stop taking timer interrupts. `lock_watched` only writes a byte and keeps
    // spinning; the halting version of this idea froze the machine at boot and was reverted.
    let mut lock = crate::postmortem::lock_watched(
        &MEMORY_MANAGER, crate::postmortem::LOCK_MEMORY_MANAGER_V2P);
    if let Some(mm) = lock.as_mut() {
        let addr = VirtAddr::new(virt_addr);
        mm.mapper.translate_addr(addr).map(|p| p.as_u64())
    } else { None }
}

pub fn phys_to_virt(phys_addr: u64) -> Option<u64> {
    unsafe { if PHYS_MEM_OFFSET == 0 { return None; } Some(phys_addr + PHYS_MEM_OFFSET) }
}

pub unsafe fn map_mmio(phys_addr: u64, size: usize) -> Result<u64, &'static str> {
    let mut lock = MEMORY_MANAGER.lock();
    let system = lock.as_mut().ok_or("Memory System not initialized")?;
    let mut active_mapper = unsafe { active_mapper() };

    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size as u64));
    
    for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(frame.start_address().as_u64()));
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;
        match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
            Ok(mapper) => mapper.flush(),
            Err(MapToError::PageAlreadyMapped(_)) => continue,
            Err(_) => return Err("Map Failed: MMIO"),
        }
    }
    Ok(phys_addr)
}

/// Map `num_pages` of fresh user memory at `start_vaddr`.
///
/// `exec` is required rather than defaulted on purpose: it decides whether this range can hold
/// executable code, which is a security property, and a default would let new call sites acquire
/// one silently. Only the ELF loader passes `true` today — stacks and anonymous mmap are data and
/// get NX, so a buffer overflow into them has nothing to jump to.
pub fn allocate_user_pages_at(
    start_vaddr: u64,
    num_pages: usize,
    exec: bool,
) -> Result<u64, &'static str> {
    // Watchdogged: this call holds the lock across the `write_bytes` below, which touches a
    // freshly mapped USER address. A fault there re-enters the lock via the page-fault handler.
    let mut system_lock = lock_mm_watchdog(MM_SITE_ALLOC_USER_PAGES);
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    let mut active_mapper = unsafe { active_mapper() };

    let start_page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(start_vaddr));
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    // Safe to set: EFER.NXE is on for the BSP (bootloader) and every AP (trampoline.asm), which is
    // already relied on by the kernel stacks being mapped NO_EXECUTE.
    if !exec { flags |= PageTableFlags::NO_EXECUTE; }

    for i in 0..num_pages {
        let page = start_page + i as u64;

        unsafe {
            let frame = system.frame_allocator.allocate_frame().ok_or("Out of physical memory!")?;

            match active_mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => {
                    mapper.flush();
                    // Zero only FRESH frames. (Reused pages must keep their contents — see below.)
                    core::ptr::write_bytes(page.start_address().as_mut_ptr::<u8>(), 0, 4096);
                }
                Err(MapToError::PageAlreadyMapped(_)) => {
                    // B-γ FIX: KEEP the existing mapping. ELF segments legitimately share a page
                    // (e.g. stdhello: .rodata ends at 0x...c148 and PT_TLS/.tdata STARTS there —
                    // both LOADs cover page 0x...c000). The old behavior unmapped the page and
                    // installed a fresh ZEROED frame, destroying the tail of the previous segment:
                    // that wiped the last bytes of .rodata — where core::fmt's DECIMAL_PAIRS digit
                    // table lives — so EVERY runtime-formatted integer became garbage bytes
                    // (in-memory format! corruption + "tofu" in the boot log, varying per build).
                    // Just release the unused frame and leave the page intact; the caller copies
                    // its segment bytes on top.
                    system.frame_allocator.deallocate_frame(frame);

                    // ★★★ ...but ONLY if the page is one WE could have mapped. "Keep whatever is
                    // there" was unconditional, and that is an arbitrary kernel-memory write.
                    //
                    // The Nyx kernel is linked at 0x200000..~0x15d5000 and `clone_kernel_page_table`
                    // copies ALL 512 PML4 entries, so the kernel image is mapped in EVERY address
                    // space; `clear_user_address_space` preserves it because it carries no USER
                    // bit. A binary linked at gcc's default 0x400000 therefore lands inside the
                    // kernel's own .text — `map_to` reports PageAlreadyMapped, we kept the
                    // kernel's mapping, and `load_elf_full` copied the segment ONTO THE RUNNING
                    // KERNEL. That was the HelloC freeze: CR2 0x400003, a kernel-mode #PF during
                    // the segment copy.
                    //
                    // It only faulted because .text is read-only. The kernel's .data/.bss above
                    // 0x15d2520 IS writable, so a binary linked a little higher would have
                    // silently corrupted live kernel state with no fault at all — an unprivileged
                    // ELF granted an arbitrary kernel write. Fail closed instead.
                    if !page_is_user_writable(page.start_address().as_u64()) {
                        return Err("ELF segment overlaps a non-user mapping (refusing to write through it)");
                    }
                },
                Err(_) => {
                    system.frame_allocator.deallocate_frame(frame);
                    return Err("Failed to map user page");
                }
            }
        }
    }
    Ok(start_vaddr)
}

pub fn map_user_framebuffer(phys_addr: u64, size: u64) -> Result<u64, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    let mut active_mapper = unsafe { active_mapper() };
    
    let user_start = VirtAddr::new(0x9000_0000); 

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITE_THROUGH | PageTableFlags::BIT_9;
    
    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size - 1));
    
    for (i, frame) in PhysFrame::range_inclusive(start_frame, end_frame).enumerate() {
        let page = Page::<Size4KiB>::containing_address(user_start + (i as u64 * 4096));
        unsafe { 
            match active_mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(MapToError::PageAlreadyMapped(_)) => continue,
                Err(_) => return Err("Map Failed: Framebuffer"),
            }
        }
    }
    Ok(user_start.as_u64())
}

pub fn map_user_mmio(phys_addr: u64, size: usize) -> Result<u64, &'static str> {
    let mut lock = MEMORY_MANAGER.lock();
    let system = lock.as_mut().ok_or("Memory System not initialized")?;
    let mut active_mapper = unsafe { active_mapper() };

    static mut NEXT_MMIO_VIRT: u64 = 0xA000_0000;
    let virt_base = unsafe { NEXT_MMIO_VIRT };

    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size as u64 - 1));

    let mut current_virt = virt_base;
    for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(current_virt));
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_CACHE;
        
        unsafe {
            match active_mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(_) => return Err("Map Failed: User MMIO"),
            }
        }
        current_virt += 4096;
    }
    
    unsafe { NEXT_MMIO_VIRT = current_virt; }
    Ok(virt_base)
}

pub fn allocate_kernel_stack(pages: usize) -> u64 {
    let mut lock = MEMORY_MANAGER.lock();
    let system = lock.as_mut().expect("Memory System not initialized");
    let mut active_mapper = unsafe { active_mapper() };

    static mut NEXT_STACK_VIRT: u64 = 0xFFFF_9000_0000_0000;
    let virt_base = unsafe { NEXT_STACK_VIRT };

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

    let mut top_of_stack = 0;
    for i in 0..pages {
        let addr = virt_base + (i as u64 * 4096);
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        unsafe {
            let frame = system.frame_allocator.allocate_frame().expect("OOM: Kernel Stack");
            match active_mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(_) => panic!("Failed to map kernel stack page"),
            }
        }
        top_of_stack = addr + 4096;
    }

    unsafe { NEXT_STACK_VIRT += (pages as u64 + 1) * 4096; }
    top_of_stack & !0xF 
}

pub fn identity_map_low_memory() {
    let mut lock = MEMORY_MANAGER.lock();
    if let Some(system) = lock.as_mut() {
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        for addr in (0..0x10_0000).step_by(4096) {
            let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(addr));
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
            unsafe {
                match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                    Ok(mapper) => mapper.flush(), Err(_) => {} 
                }
            }
        }
    }
}

/// Locate the page-table entry backing `vaddr` in the address space rooted at `cr3`.
///
/// Returns `None` if any level is absent, or if the mapping is a huge page. Splitting a 2 MiB or
/// 1 GiB page is a separate job, and quietly re-protecting the whole thing because the caller named
/// one 4 KiB page inside it would change memory the caller never asked about.
unsafe fn pte_for(cr3: u64, vaddr: u64) -> Option<*mut u64> {
    let offset = PHYS_MEM_OFFSET;
    let mut table = (cr3 + offset) as *mut u64;
    // PML4 -> PDPT -> PD, indexed at bits 39, 30 and 21 respectively.
    for level in (1..=3).rev() {
        let idx = ((vaddr >> (12 + 9 * level)) & 0x1FF) as usize;
        let e = *table.add(idx);
        if e & 1 == 0 { return None; }
        // Bit 7 is PS on a PDPT or PD entry; on a PML4 entry it is not a size bit.
        if level <= 2 && (e & (1 << 7)) != 0 { return None; }
        table = ((e & 0x000FFFFF_FFFFF000) + offset) as *mut u64;
    }
    let idx = ((vaddr >> 12) & 0x1FF) as usize;
    if *table.add(idx) & 1 == 0 { return None; }
    Some(table.add(idx))
}

/// Is `vaddr` actually backed by a present page in the current address space?
///
/// `is_valid_user_ptr` only RANGE-checks — it cannot tell a mapped page from an unmapped one, and a
/// kernel-mode write to an unmapped user address panics the whole MACHINE rather than killing the
/// process. Anywhere the kernel writes to a user address it was merely *told* about, rather than one
/// it just mapped itself, needs this as well as the range check.
pub unsafe fn user_addr_mapped(vaddr: u64) -> bool {
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    pte_for(cr3, vaddr).is_some()
}

/// Is `vaddr` mapped in the CURRENT address space as a USER + WRITABLE 4 KiB page — i.e. a page
/// this loader could plausibly have mapped itself?
///
/// Used to tell "an earlier segment of the SAME binary already mapped this page" (legitimate, and
/// the reason `allocate_user_pages_at` tolerates `PageAlreadyMapped` at all) from "something else
/// owns this address". Fails CLOSED: an absent level or a huge page returns false, because the one
/// thing we must never do is assume an unfamiliar mapping is safe to write through.
pub unsafe fn page_is_user_writable(vaddr: u64) -> bool {
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    match pte_for(cr3, vaddr) {
        // PRESENT (bit 0) + WRITABLE (bit 1) + USER (bit 2).
        Some(p) => { let e = *p; (e & 1) != 0 && (e & 0x2) != 0 && (e & 0x4) != 0 }
        None => false,
    }
}

/// Apply `PROT_WRITE` / `PROT_EXEC` to an already-mapped user range in the CURRENT address space.
///
/// The whole range is validated before a single PTE is touched: `mprotect(2)` must not partially
/// apply, because a caller that receives an error has no way to discover how far it got.
pub unsafe fn protect_user_range(
    start: u64,
    num_pages: usize,
    write: bool,
    exec: bool,
) -> Result<(), &'static str> {
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();

    for i in 0..num_pages {
        if pte_for(cr3, start + (i as u64) * 4096).is_none() {
            return Err("unmapped page in range");
        }
    }

    for i in 0..num_pages {
        let v = start + (i as u64) * 4096;
        let pte = match pte_for(cr3, v) { Some(p) => p, None => return Err("range changed") };
        let mut e = *pte;

        // ★ Never grant WRITABLE on a copy-on-write page. The CoW bit (0x400) means this frame is
        // shared with another address space, and the write FAULT is what triggers the private copy.
        // Setting the writable bit here would let the write land in the shared frame and silently
        // corrupt the other process — data loss with no crash to point at it. The CoW fault handler
        // grants writability itself on first write, which is the only correct moment for it.
        let is_cow = e & 0x400 != 0;
        if write {
            if !is_cow { e |= 1 << 1; }
        } else {
            e &= !(1 << 1);
        }

        // Bit 63 = NX. Safe to set: EFER.NXE is enabled on the BSP by the bootloader and on every
        // AP by trampoline.asm, so this can never be a reserved-bit fault.
        if exec { e &= !(1u64 << 63); } else { e |= 1u64 << 63; }

        *pte = e;
        core::arch::asm!("invlpg [{}]", in(reg) v, options(nostack, preserves_flags));
    }
    Ok(())
}

/// Site IDs for `lock_mm_watchdog`, so the post-mortem record names WHICH acquire wedged.
pub const MM_SITE_ALLOC_FRAME: u8 = 1;
pub const MM_SITE_ALLOC_USER_PAGES: u8 = 2;

/// Take `MEMORY_MANAGER`, but give up and REPORT rather than spin forever.
///
/// ★★★ Why this exists. `MEMORY_MANAGER` is a bare `spin::Mutex` with no interrupt masking and
/// no re-entrancy, and the CoW **page-fault handler** calls `allocate_frame()`, which takes it.
/// A page fault is an EXCEPTION — `cli` cannot mask it — so any fault on a CPU that already
/// holds this lock deadlocks against itself. `allocate_user_pages_at` holds it across a
/// `write_bytes` to a freshly mapped USER address, which is exactly such a fault window.
///
/// The result is the worst possible failure mode: instant, whole-machine, and completely
/// silent. `SYSCALL` cleared IF so the core never yields, and every other core wedges on the
/// same lock the moment it touches memory. Nothing is printed, nothing panics, the box is just
/// gone — which is indistinguishable from "the code never ran at all".
///
/// This does not FIX the deadlock (that needs the lock itself to mask interrupts, kernel-wide —
/// the same treatment the kernel heap already got). It makes the deadlock ANNOUNCE itself: the
/// bounded spin expires, the cause and site go into battery-backed CMOS, and the next boot says
/// which acquire wedged instead of leaving another mystery freeze. A legitimate contended
/// acquire completes in microseconds; 50M spins is several orders of magnitude beyond that, so
/// this cannot fire on a healthy machine.
fn lock_mm_watchdog(site: u8) -> spin::MutexGuard<'static, Option<MemorySystem>> {
    for _ in 0..50_000_000u64 {
        if let Some(g) = MEMORY_MANAGER.try_lock() { return g; }
        core::hint::spin_loop();
    }

    crate::postmortem::mark_why(crate::postmortem::WHY_MM_DEADLOCK, site as u64);
    crate::serial::bc("\n!!MEMORY_MANAGER DEADLOCK site=");
    crate::serial::bc_hex(site as u64);
    crate::serial::bc("\n");
    crate::serial_println!(
        "[FATAL] MEMORY_MANAGER lock never acquired (site {}). Almost certainly re-entered from \
         the page-fault handler while a mapping call already held it.", site as u32);
    loop { x86_64::instructions::hlt(); }
}

pub fn allocate_frame() -> Option<PhysFrame> {
    // Watchdogged because THIS is the acquire the page-fault handler makes: if a fault lands
    // while a mapping call holds the lock, this is where the machine would silently stop.
    let mut lock = lock_mm_watchdog(MM_SITE_ALLOC_FRAME);
    lock.as_mut().and_then(|sys| sys.frame_allocator.allocate_frame())
}

pub fn allocate_contiguous(num_frames: usize, alignment: u64, below_4gb: bool) -> Option<PhysFrame> {
    let mut lock = MEMORY_MANAGER.lock();
    lock.as_mut().and_then(|sys| sys.frame_allocator.allocate_contiguous_frames(num_frames, alignment, below_4gb))
}

pub fn clone_kernel_page_table(new_pml4_phys: PhysAddr) {
    unsafe {
        let offset = PHYS_MEM_OFFSET;
        let current_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
        let current_pml4 = (current_cr3 + offset) as *const u64;
        let new_pml4 = (new_pml4_phys.as_u64() + offset) as *mut u64;
        
        core::ptr::copy_nonoverlapping(current_pml4, new_pml4, 512);
    }
}

pub fn clone_user_address_space(parent_cr3: PhysAddr, child_cr3: PhysAddr) -> Result<(), &'static str> {
    unsafe {
        let mut lock = crate::memory::MEMORY_MANAGER.lock();
        let system = lock.as_mut().ok_or("Memory System not initialized")?;
        let offset = PHYS_MEM_OFFSET;
        let phys_mask = 0x000FFFFF_FFFFF000;
        let flags_mask = 0xFFF;

        let parent_pml4 = (parent_cr3.as_u64() + offset) as *const u64;
        let child_pml4 = (child_cr3.as_u64() + offset) as *mut u64;

        for i4 in 0..256 {
            let pml4_entry = *parent_pml4.add(i4);
            if pml4_entry & 1 == 0 { continue; }

            // A userspace fork() must never be able to panic the whole kernel: on allocation
            // failure, bail out with Err so the caller can return -ENOMEM to the process.
            let new_pml3 = system.frame_allocator.allocate_frame().ok_or("OOM: PML3")?;
            let child_pml3_ptr = (new_pml3.start_address().as_u64() + offset) as *mut u64;
            core::ptr::write_bytes(child_pml3_ptr as *mut u8, 0, 4096);
            *child_pml4.add(i4) = new_pml3.start_address().as_u64() | (pml4_entry & flags_mask);

            let parent_pml3_ptr = ((pml4_entry & phys_mask) + offset) as *const u64;
            for i3 in 0..512 {
                let pml3_entry = *parent_pml3_ptr.add(i3);
                if pml3_entry & 1 == 0 { continue; }

                let new_pml2 = system.frame_allocator.allocate_frame().ok_or("OOM: PML2")?;
                let child_pml2_ptr = (new_pml2.start_address().as_u64() + offset) as *mut u64;
                core::ptr::write_bytes(child_pml2_ptr as *mut u8, 0, 4096);
                *child_pml3_ptr.add(i3) = new_pml2.start_address().as_u64() | (pml3_entry & flags_mask);

                let parent_pml2_ptr = ((pml3_entry & phys_mask) + offset) as *const u64;
                for i2 in 0..512 {
                    let pml2_entry = *parent_pml2_ptr.add(i2);
                    if pml2_entry & 1 == 0 { continue; }

                    if pml2_entry & (1 << 7) != 0 {
                        *child_pml2_ptr.add(i2) = pml2_entry;
                        continue;
                    }

                    let new_pt = system.frame_allocator.allocate_frame().ok_or("OOM: PT")?;
                    let child_pt_ptr = (new_pt.start_address().as_u64() + offset) as *mut u64;
                    core::ptr::write_bytes(child_pt_ptr as *mut u8, 0, 4096);
                    *child_pml2_ptr.add(i2) = new_pt.start_address().as_u64() | (pml2_entry & flags_mask);

                    let parent_pt_ptr = ((pml2_entry & phys_mask) + offset) as *const u64;
                    for i1 in 0..512 {
                        let pt_entry = *parent_pt_ptr.add(i1);
                        if pt_entry & 1 == 0 { continue; }

                        // 🚨 COPY-ON-WRITE IMPLEMENTATION
                        // If it is User Memory (0x4), AND it is NOT MMIO (0x10), AND NOT Shared (0x200)
                        if (pt_entry & 0x4) != 0 && (pt_entry & 0x10) == 0 && (pt_entry & 0x200) == 0 {
                            
                            // 1. Strip the Writable flag (Bit 1), Add the CoW flag (Bit 10 = 0x400)
                            let cow_entry = (pt_entry & !(1 << 1)) | 0x400;
                            
                            // 2. Update Child to point to the SAME frame, but Read-Only (CoW)
                            *child_pt_ptr.add(i1) = cow_entry;
                            
                            // 3. Update the Parent's original table so it traps on write too!
                            let mut_parent_pt = parent_pt_ptr as *mut u64;
                            *mut_parent_pt.add(i1) = cow_entry;
                            
                        } else {
                            // It is Shared MMIO/Framebuffer! Keep it fully writable for both.
                            *child_pt_ptr.add(i1) = pt_entry;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn clear_user_address_space(cr3_phys: PhysAddr) {
    // `MEMORY_MANAGER` is a bare spin lock and this walks four levels of page tables while
    // holding it, inside a syscall (so IF is clear). If it ever deadlocks or faults mid-walk
    // the machine stops dead with nothing on the wire, so bracket it — see serial::bc.
    crate::serial::bc("(cl");
    unsafe {
        let mut lock = crate::memory::MEMORY_MANAGER.lock();
        let system = lock.as_mut().expect("Memory System not initialized");
        let offset = PHYS_MEM_OFFSET;
        let pml4 = (cr3_phys.as_u64() + offset) as *mut u64;
        let phys_mask = 0x000FFFFF_FFFFF000;

        for i4 in 0..256 {
            let pml4_entry = *pml4.add(i4);
            if pml4_entry & 1 == 0 { continue; }

            let pml3_phys = pml4_entry & phys_mask;
            let pml3 = (pml3_phys + offset) as *mut u64;
            let mut pml3_empty = true;

            for i3 in 0..512 {
                let pml3_entry = *pml3.add(i3);
                if pml3_entry & 1 == 0 { continue; }

                let pml2_phys = pml3_entry & phys_mask;
                let pml2 = (pml2_phys + offset) as *mut u64;
                let mut pml2_empty = true;

                for i2 in 0..512 {
                    let pml2_entry = *pml2.add(i2);
                    if pml2_entry & 1 == 0 { continue; }
                    
                    if pml2_entry & (1 << 7) != 0 {
                        if pml2_entry & 0x4 != 0 { 
                            *pml2.add(i2) = 0; 
                        } else {
                            pml2_empty = false; 
                        }
                        continue; 
                    }
                    
                    let pt_phys = pml2_entry & phys_mask;
                    let pt = (pt_phys + offset) as *mut u64;
                    let mut pt_empty = true;

                    for i1 in 0..512 {
                        let pt_entry = *pt.add(i1);
                        if pt_entry & 1 == 0 { continue; }
                        
                        if (pt_entry & 0x4) != 0 {
                            // 🚨 THE CoW LEAK FIX 🚨
                            // Do NOT free MMIO (0x10), Shared Memory (0x200), OR CoW Pages (0x400)!
                            // If it has the CoW bit, the Parent is still relying on this physical RAM!
                            if (pt_entry & 0x10) == 0 && (pt_entry & 0x200) == 0 && (pt_entry & 0x400) == 0 {
                                let page_phys = pt_entry & phys_mask;
                                use x86_64::structures::paging::PhysFrame;
                                system.frame_allocator.deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(page_phys)));
                            }
                            
                            // Safely detach this process's pointer to the memory
                            *pt.add(i1) = 0; 
                        } else {
                            pt_empty = false; 
                        }
                    }
                    
                    if pt_empty {
                        use x86_64::structures::paging::PhysFrame;
                        system.frame_allocator.deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(pt_phys)));
                        *pml2.add(i2) = 0;
                    } else {
                        pml2_empty = false;
                    }
                }
                
                if pml2_empty {
                    use x86_64::structures::paging::PhysFrame;
                    system.frame_allocator.deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(pml2_phys)));
                    *pml3.add(i3) = 0;
                } else {
                    pml3_empty = false;
                }
            }
            
            if pml3_empty {
                use x86_64::structures::paging::PhysFrame;
                system.frame_allocator.deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(pml3_phys)));
                *pml4.add(i4) = 0;
            }
        }
        
        let active_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
        core::arch::asm!("mov cr3, {}", in(reg) active_cr3);
    }
    crate::serial::bc("cl)");
}

/// Unmap `num_pages` starting at `start_vaddr` from the CURRENT address space, freeing the frames
/// we own. The inverse of `allocate_user_pages_at`; backs `munmap(2)`.
///
/// ★ The ownership rules are the same three the teardown path already encodes, and they are the
/// whole difficulty: a page carrying MMIO (0x10), SHM (0x200) or CoW (0x400) is mapped by someone
/// ELSE as well, so its PTE may be cleared but its FRAME MUST NOT BE FREED. Returning a shared
/// frame to the allocator hands live memory to the next allocation, and the corruption surfaces
/// somewhere unrelated with nothing pointing back here.
///
/// Only USER pages are touched. A supervisor mapping in this range is not ours to unmap — the
/// kernel image itself is mapped low in every address space (see the ELF-overlap check in
/// `allocate_user_pages_at`), and unmapping it on a bad `munmap` argument would be fatal.
///
/// Returns the number of pages actually unmapped.
pub fn unmap_user_pages(start_vaddr: u64, num_pages: usize) -> usize {
    let mut freed = 0usize;
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();

    let mut lock = MEMORY_MANAGER.lock();
    let system = match lock.as_mut() { Some(s) => s, None => return 0 };

    for i in 0..num_pages {
        let vaddr = start_vaddr + (i as u64) * 4096;
        let pte = match unsafe { pte_for(cr3, vaddr) } { Some(p) => p, None => continue };
        let entry = unsafe { *pte };

        if entry & 1 == 0 { continue; }        // not present
        if entry & 0x4 == 0 { continue; }      // not USER — not ours to unmap

        if (entry & 0x10) == 0 && (entry & 0x200) == 0 && (entry & 0x400) == 0 {
            let phys = entry & 0x000FFFFF_FFFFF000;
            system.frame_allocator.deallocate_frame(
                PhysFrame::containing_address(PhysAddr::new(phys)));
        }

        unsafe {
            *pte = 0;
            // Per-page invlpg rather than a CR3 reload: munmap is called constantly by any
            // allocator returning memory, and flushing the entire TLB on each one would undo
            // far more than it costs.
            core::arch::asm!("invlpg [{}]", in(reg) vaddr, options(nostack));
        }
        freed += 1;
    }
    freed
}

pub fn create_shm_block(size: usize) -> Option<u64> {
    let num_pages = (size + 0xFFF) / 0x1000;
    let mut frames = Vec::with_capacity(num_pages);
    
    let mut sys_lock = MEMORY_MANAGER.lock();
    let sys = sys_lock.as_mut()?;
    
    for _ in 0..num_pages {
        let frame = sys.frame_allocator.allocate_frame()?;
        unsafe {
            let ptr = (frame.start_address().as_u64() + PHYS_MEM_OFFSET) as *mut u8;
            core::ptr::write_bytes(ptr, 0, 4096);
        }
        frames.push(frame.start_address());
    }
    
    let id = NEXT_SHM_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    SHM_REGISTRY.lock().push(ShmBlock { id, frames, size });
    Some(id)
}

pub fn map_shm_block(id: u64, target_vaddr: u64) -> Result<u64, &'static str> {
    let registry = SHM_REGISTRY.lock();
    let block = registry.iter().find(|b| b.id == id).ok_or("SHM not found")?;
    
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    let mut active_mapper = unsafe { active_mapper() };

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::BIT_9;

    for (i, &phys_addr) in block.frames.iter().enumerate() {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(target_vaddr + (i as u64 * 4096)));
        let frame = PhysFrame::containing_address(phys_addr);
        
        unsafe {
            match active_mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_)) => continue,
                Err(_) => return Err("Failed to map SHM page"),
            }
        }
    }
    Ok(target_vaddr)
}

/// R1: tear down the CURRENT address space's mapping of `size` bytes starting at `base_vaddr`, WITHOUT
/// freeing the physical frames (they are shared — the owner frees them separately via destroy_shm_block).
/// The compositor calls this on its OLD window mapping after it has swapped to the resized buffer, so it
/// no longer references the old frames before the app frees them (prevents use-after-free of reused RAM).
pub fn unmap_shm_range(base_vaddr: u64, size: usize) {
    let num_pages = (size + 0xFFF) / 0x1000;
    let mut active_mapper = unsafe { active_mapper() };
    for i in 0..num_pages {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(base_vaddr + (i as u64 * 4096)));
        unsafe {
            if let Ok((_frame, flush)) = active_mapper.unmap(page) { flush.flush(); }
        }
    }
}

/// R1: destroy an SHM block — unmap the CALLER's mapping at `base_vaddr`, return every physical frame to
/// the allocator free-list (reused by the next allocate_frame), and remove the block from the registry.
/// Call ONLY when the block is single-owner: all OTHER mappings (e.g. the compositor's CPU mapping and its
/// GGTT mapping) must already be torn down/repointed, or those stale mappings would dangle onto RAM that
/// gets handed to another allocation. The resize handshake guarantees this (compositor releases + ACKs
/// first; the app destroys only on the ACK). Returns false if the id isn't in the registry.
pub fn destroy_shm_block(id: u64, base_vaddr: u64) -> bool {
    // Remove the block from the registry first (swap_remove: order doesn't matter).
    let block = {
        let mut reg = SHM_REGISTRY.lock();
        match reg.iter().position(|b| b.id == id) {
            Some(pos) => reg.swap_remove(pos),
            None => return false,
        }
    };
    // Drop the caller's virtual mapping of the block.
    unmap_shm_range(base_vaddr, block.size);
    // Return the physical frames to the allocator's free-list.
    {
        let mut sys_lock = MEMORY_MANAGER.lock();
        if let Some(sys) = sys_lock.as_mut() {
            for &phys in block.frames.iter() {
                sys.frame_allocator.deallocate_frame(PhysFrame::containing_address(phys));
            }
        }
    }
    // R1: the resize buffer was reclaimed here (per-free serial log silenced now that it's verified).
    true
}