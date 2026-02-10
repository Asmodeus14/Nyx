use x86_64::{
    structures::paging::{
        OffsetPageTable, PageTable, FrameAllocator, Size4KiB, PhysFrame, Mapper, Page, 
        PageTableFlags, Translate, mapper::MapToError 
    },
    VirtAddr, PhysAddr,
};
use bootloader_api::info::MemoryRegionKind;
use spin::Mutex;

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

pub unsafe fn map_mmio(
    phys_addr: u64, 
    size: u64, 
    mapper: &mut OffsetPageTable, 
    frame_allocator: &mut BootInfoFrameAllocator
) -> Result<VirtAddr, &'static str> {
    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size - 1));
    let frame_range = PhysFrame::range_inclusive(start_frame, end_frame);

    let virt_start = VirtAddr::new(phys_addr);
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

    for (i, frame) in frame_range.enumerate() {
        let page = Page::<Size4KiB>::containing_address(virt_start + (i as u64 * 4096));
        let _ = mapper.map_to(page, frame, flags, frame_allocator);
    }
    Ok(virt_start)
}

pub fn virt_to_phys(virt_addr: u64) -> Option<u64> {
    let mut lock = MEMORY_MANAGER.lock();
    if let Some(mm) = lock.as_mut() {
        let addr = VirtAddr::new(virt_addr);
        mm.mapper.translate_addr(addr).map(|p| p.as_u64())
    } else {
        None
    }
}

pub fn allocate_user_pages(num_pages: u64) -> Result<VirtAddr, &'static str> {
    use x86_64::structures::paging::{PageTableFlags, Mapper};
    
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;

    let start_addr = VirtAddr::new(0x100_0000);
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    for i in 0..num_pages {
        let page_addr = start_addr + (i * 4096);
        let page = Page::containing_address(page_addr);

        unsafe {
            let frame = system.frame_allocator.allocate_frame()
                .ok_or("Frame allocation failed")?;
            if let Err(_) = system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                 return Err("Map failed");
            }
        }
    }
    Ok(start_addr)
}

// NEW: Map Framebuffer to User Space Fixed Address (0x80000000)
// This enables "God Mode" graphics for the user app.
pub fn map_user_framebuffer(phys_addr: u64, size: u64) -> Result<u64, &'static str> {
    use x86_64::structures::paging::{PageTableFlags, Mapper};
    
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;

    let user_start = VirtAddr::new(0x8000_0000); // 2GB Mark
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_CACHE;

    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size - 1));
    let frame_range = PhysFrame::range_inclusive(start_frame, end_frame);

    for (i, frame) in frame_range.enumerate() {
        let page = Page::<Size4KiB>::containing_address(user_start + (i as u64 * 4096));
        unsafe {
            // Ignore errors if already mapped
            let _ = system.mapper.map_to(page, frame, flags, &mut system.frame_allocator);
        }
    }

    Ok(user_start.as_u64())
}