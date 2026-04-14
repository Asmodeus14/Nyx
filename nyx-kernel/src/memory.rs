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

// --- INSTANT BARE-METAL ALLOCATOR (O(1) Speed) ---
pub struct BootInfoFrameAllocator {
    memory_map: &'static [bootloader_api::info::MemoryRegion],
    current_region: usize,
    current_offset: u64,
    phys_offset: VirtAddr,
    recycled_frames: Option<PhysFrame>,
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static [bootloader_api::info::MemoryRegion], phys_offset: VirtAddr) -> Self {
        BootInfoFrameAllocator { 
            memory_map, 
            current_region: 0, 
            current_offset: 0,
            phys_offset, 
            recycled_frames: None 
        }
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
        // 1. Check for recycled/freed frames first
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

        // 2. O(1) Hardware Memory Scan
        while self.current_region < self.memory_map.len() {
            let region = &self.memory_map[self.current_region];

            if region.kind == MemoryRegionKind::Usable {
                let target_addr = region.start + self.current_offset;

                // Make sure we have a full 4K page left in this region
                if target_addr + 4096 <= region.end {
                    self.current_offset += 4096;
                    return Some(PhysFrame::containing_address(PhysAddr::new(target_addr)));
                }
            }
            
            // If this region is full or not usable, move to the next one
            self.current_region += 1;
            self.current_offset = 0;
        }
        None // Out of Physical Memory!
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

// --- SPECIFIC VIRTUAL ADDRESS MAPPING (For ELF Loading & Anonymous mmap) ---
pub fn allocate_user_pages_at(start_vaddr: u64, num_pages: usize) -> Result<u64, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;

    let start_page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(start_vaddr));
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    for i in 0..num_pages {
        let page = start_page + i as u64;
        
        unsafe {
            let frame = system.frame_allocator.allocate_frame().ok_or("Out of physical memory!")?;
            
            match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(MapToError::PageAlreadyMapped(_)) => {
                    system.frame_allocator.deallocate_frame(frame);
                    continue;
                },
                Err(_) => {
                    system.frame_allocator.deallocate_frame(frame);
                    return Err("Failed to map user page");
                }
            }
            
            core::ptr::write_bytes(page.start_address().as_mut_ptr::<u8>(), 0, 4096);
        }
    }

    Ok(start_vaddr)
}

pub fn map_user_framebuffer(phys_addr: u64, size: u64) -> Result<u64, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    
    // 🚨 FIX: Moved the Framebuffer mapping out of the way to 0x9000_0000
    let user_start = VirtAddr::new(0x9000_0000); 
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

// --- USER MEMORY MAPPING FOR GPU (mmap) ---
pub fn map_user_mmio(phys_addr: u64, size: usize) -> Result<u64, &'static str> {
    let mut lock = MEMORY_MANAGER.lock();
    let system = lock.as_mut().ok_or("Memory System not initialized")?;

    static mut NEXT_MMIO_VIRT: u64 = 0xA000_0000;
    let virt_base = unsafe { NEXT_MMIO_VIRT };

    let start_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr));
    let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys_addr + size as u64 - 1));

    let mut current_virt = virt_base;
    for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(current_virt));
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

// --- DYNAMIC USERSPACE HEAP ALLOCATOR ---
pub fn allocate_user_heap_pages(num_pages: usize) -> Result<u64, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;

    // 🚨 FIX: Placed the Userspace Heap correctly at 0x8000_0000 (2GB mark)
    static mut NEXT_HEAP_VIRT: u64 = 0x8000_0000;
    let virt_base = unsafe { NEXT_HEAP_VIRT };

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    for i in 0..num_pages {
        let addr = virt_base + (i as u64 * 4096);
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        unsafe {
            let frame = system.frame_allocator.allocate_frame().ok_or("OOM: User Heap")?;
            match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(_) => return Err("Map Failed: User Heap"),
            }
        }
    }

    unsafe { NEXT_HEAP_VIRT += num_pages as u64 * 4096; }
    Ok(virt_base)
}

pub fn allocate_kernel_stack(pages: usize) -> u64 {
    let mut lock = MEMORY_MANAGER.lock();
    let system = lock.as_mut().expect("Memory System not initialized");

    static mut NEXT_STACK_VIRT: u64 = 0xFFFF_8000_0000_0000;
    let virt_base = unsafe { NEXT_STACK_VIRT };

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    let mut top_of_stack = 0;

    for i in 0..pages {
        let addr = virt_base + (i as u64 * 4096);
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));

        unsafe {
            let frame = system.frame_allocator.allocate_frame().expect("OOM: Kernel Stack");
            match system.mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
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
                    Ok(mapper) => mapper.flush(),
                    Err(_) => {} 
                }
            }
        }
    }
}