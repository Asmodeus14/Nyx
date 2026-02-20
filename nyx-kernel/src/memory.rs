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

// --- NEW: Global physical memory offset for hardware drivers ---
pub static mut PHYS_MEM_OFFSET: u64 = 0;

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

pub struct BootInfoFrameAllocator {
    memory_map: &'static [bootloader_api::info::MemoryRegion],
    next: usize,
    phys_offset: VirtAddr,
    recycled_frames: Option<PhysFrame>,
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static [bootloader_api::info::MemoryRegion], phys_offset: VirtAddr) -> Self {
        BootInfoFrameAllocator { memory_map, next: 0, phys_offset, recycled_frames: None }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        self.memory_map.iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .map(|r| r.start..r.end)
            .flat_map(|range| range.step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }

    pub fn deallocate_frame(&mut self, frame: PhysFrame) {
        let phys_addr = frame.start_address().as_u64();
        let virt_addr = self.phys_offset + phys_addr;
        let ptr = virt_addr.as_mut_ptr() as *mut u64;
        let next_ptr = match self.recycled_frames {
            Some(f) => f.start_address().as_u64(),
            None => 0,
        };
        unsafe { *ptr = next_ptr; }
        self.recycled_frames = Some(frame);
    }
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
            return Some(frame);
        }
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

pub fn virt_to_phys(virt_addr: u64) -> Option<u64> {
    let mut lock = MEMORY_MANAGER.lock();
    if let Some(mm) = lock.as_mut() {
        let addr = VirtAddr::new(virt_addr);
        mm.mapper.translate_addr(addr).map(|p| p.as_u64())
    } else { None }
}

pub fn phys_to_virt(phys_addr: u64) -> Option<u64> {
    unsafe {
        if PHYS_MEM_OFFSET == 0 { return None; }
        Some(phys_addr + PHYS_MEM_OFFSET)
    }
}

pub unsafe fn map_mmio(phys_addr: u64, size: usize) -> Result<u64, &'static str> {
    let mut lock = MEMORY_MANAGER.lock();
    let system = lock.as_mut().ok_or("Memory System not initialized")?;

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

pub fn allocate_user_pages(num_pages: u64) -> Result<VirtAddr, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    
    let start_addr = VirtAddr::new(0x200_0000);
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    
    for i in 0..num_pages {
        let page = Page::<Size4KiB>::containing_address(start_addr + (i * 4096));
        unsafe {
            let frame = system.frame_allocator.allocate_frame().ok_or("OOM: User Code")?;
            match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(MapToError::PageAlreadyMapped(_)) => {
                     system.frame_allocator.deallocate_frame(frame);
                     continue;
                },
                Err(_) => {
                    system.frame_allocator.deallocate_frame(frame);
                    return Err("Map Failed: User Code");
                },
            }
        }
    }
    Ok(start_addr)
}

pub fn map_user_framebuffer(phys_addr: u64, size: u64) -> Result<u64, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    let user_start = VirtAddr::new(0x8000_0000); 
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_CACHE;
    
    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size - 1));
    
    for (i, frame) in PhysFrame::range_inclusive(start_frame, end_frame).enumerate() {
        let page = Page::<Size4KiB>::containing_address(user_start + (i as u64 * 4096));
        unsafe { 
            match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(MapToError::PageAlreadyMapped(_)) => continue,
                Err(_) => return Err("Map Failed: Framebuffer"),
            }
        }
    }
    Ok(user_start.as_u64())
}

// --- NEW: USER MEMORY MAPPING FOR GPU (mmap) ---
pub fn map_user_mmio(phys_addr: u64, size: usize) -> Result<u64, &'static str> {
    let mut lock = MEMORY_MANAGER.lock();
    let system = lock.as_mut().ok_or("Memory System not initialized")?;

    // Start placing hardware mappings at virtual address 0xA000_0000
    static mut NEXT_MMIO_VIRT: u64 = 0xA000_0000;
    let virt_base = unsafe { NEXT_MMIO_VIRT };

    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size as u64 - 1));

    let mut current_virt = virt_base;
    for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(current_virt));
        
        // CRITICAL: USER_ACCESSIBLE allows Ring-3. NO_CACHE prevents CPU from caching VRAM.
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE 
                  | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_CACHE;
        
        unsafe {
            match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(_) => return Err("Map Failed: User MMIO"),
            }
        }
        current_virt += 4096;
    }
    
    unsafe { NEXT_MMIO_VIRT = current_virt; }
    Ok(virt_base)
}