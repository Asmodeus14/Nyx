use x86_64::{
    structures::paging::{OffsetPageTable, PageTable, FrameAllocator, Size4KiB, PhysFrame, Mapper, Page, PageTableFlags},
    VirtAddr, PhysAddr,
};
use bootloader_api::info::MemoryRegionKind;
use spin::Mutex;

// GLOBAL MEMORY MANAGER (Required for Drivers)
lazy_static::lazy_static! {
    pub static ref MEMORY_MANAGER: Mutex<Option<MemorySystem>> = Mutex::new(None);
}

pub struct MemorySystem {
    pub mapper: OffsetPageTable<'static>,
    pub frame_allocator: BootInfoFrameAllocator,
}

pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table_frame = x86_64::registers::control::Cr3::read().0;
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    let level_4_table = &mut *page_table_ptr;
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

pub struct BootInfoFrameAllocator {
    memory_map: &'static [bootloader_api::info::MemoryRegion],
    next: usize,
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static [bootloader_api::info::MemoryRegion]) -> Self {
        BootInfoFrameAllocator { memory_map, next: 0 }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        self.memory_map.iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .map(|r| r.start..r.end)
            .flat_map(|range| range.step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

// --- NEW: MMIO MAPPING ---
// Maps a physical hardware address to a virtual address so drivers can read/write it.
pub unsafe fn map_mmio(
    phys_addr: u64, 
    size: u64, 
    mapper: &mut OffsetPageTable, 
    frame_allocator: &mut BootInfoFrameAllocator
) -> Result<VirtAddr, &'static str> {
    
    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size - 1));
    let frame_range = PhysFrame::range_inclusive(start_frame, end_frame);

    // Identity map roughly (Phys = Virt) for simplicity in this stage
    let virt_start = VirtAddr::new(phys_addr);

    // CRITICAL: NO_CACHE ensures we read real hardware status, not stale CPU cache
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

    for (i, frame) in frame_range.enumerate() {
        let page = Page::<Size4KiB>::containing_address(virt_start + (i as u64 * 4096));
        match mapper.map_to(page, frame, flags, frame_allocator) {
            Ok(tlb) => tlb.flush(),
            Err(_) => return Err("Failed to map MMIO page"),
        };
    }

    Ok(virt_start)
}