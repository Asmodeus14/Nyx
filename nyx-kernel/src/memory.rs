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
pub static mut BOOTLOADER_CR3: u64 = 0;

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

        while self.current_region < self.memory_map.len() {
            let region = &self.memory_map[self.current_region];
            if region.kind == MemoryRegionKind::Usable {
                let target_addr = region.start + self.current_offset;
                if target_addr + 4096 <= region.end {
                    self.current_offset += 4096;
                    return Some(PhysFrame::containing_address(PhysAddr::new(target_addr)));
                }
            }
            self.current_region += 1;
            self.current_offset = 0;
        }
        None 
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
    unsafe { if PHYS_MEM_OFFSET == 0 { return None; } Some(phys_addr + PHYS_MEM_OFFSET) }
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

pub fn allocate_user_pages_at(start_vaddr: u64, num_pages: usize) -> Result<u64, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    let mut active_mapper = unsafe { active_mapper() }; 

    let start_page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(start_vaddr));
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    for i in 0..num_pages {
        let page = start_page + i as u64;
        
        unsafe {
            let frame = system.frame_allocator.allocate_frame().ok_or("Out of physical memory!")?;
            
            match active_mapper.map_to(page, frame, flags, &mut system.frame_allocator) {
                Ok(mapper) => mapper.flush(),
                Err(MapToError::PageAlreadyMapped(_)) => {
                    // 🚨 THE PIE ISOLATION FIX 🚨
                    // If the page exists (e.g. 0x0 BIOS Identity Map), we MUST unmap it first!
                    // This replaces the dangerous hardware memory with a safe, isolated RAM frame.
                    // When the program exits, it will safely free this RAM instead of the BIOS!
                    let (_phys, flush) = active_mapper.unmap(page).unwrap();
                    flush.flush();
                    
                    // Now safely map the isolated RAM frame!
                    active_mapper.map_to(page, frame, flags, &mut system.frame_allocator).unwrap().flush();
                },
                Err(_) => {
                    system.frame_allocator.deallocate_frame(frame);
                    return Err("Failed to map user page");
                }
            }
            
            // Zero-out the freshly allocated memory for security
            core::ptr::write_bytes(page.start_address().as_mut_ptr::<u8>(), 0, 4096);
        }
    }

    Ok(start_vaddr)
}
pub fn map_user_framebuffer(phys_addr: u64, size: u64) -> Result<u64, &'static str> {
    let mut system_lock = MEMORY_MANAGER.lock();
    let system = system_lock.as_mut().ok_or("Memory System not initialized")?;
    let mut active_mapper = unsafe { active_mapper() };
    
    let user_start = VirtAddr::new(0x9000_0000); 
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_CACHE;
    
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

// ─────────────────────────────────────────────────────────────────────────
// PROCESS CLONING AND WIPING (FORK / EXECV)
// ─────────────────────────────────────────────────────────────────────────

pub fn allocate_frame() -> Option<PhysFrame> {
    let mut lock = MEMORY_MANAGER.lock();
    lock.as_mut().and_then(|sys| sys.frame_allocator.allocate_frame())
}

pub fn clone_kernel_page_table(new_pml4_phys: PhysAddr) {
    unsafe {
        let offset = PHYS_MEM_OFFSET;
        let current_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
        let current_pml4 = (current_cr3 + offset) as *const u64;
        let new_pml4 = (new_pml4_phys.as_u64() + offset) as *mut u64;
        
        // Give the child a baseline copy of the current system
        core::ptr::copy_nonoverlapping(current_pml4, new_pml4, 512);
    }
}

pub fn clone_user_address_space(parent_cr3: PhysAddr, child_cr3: PhysAddr) {
    unsafe {
        let offset = PHYS_MEM_OFFSET;
        let parent_pml4 = (parent_cr3.as_u64() + offset) as *const u64;
        let child_pml4 = (child_cr3.as_u64() + offset) as *mut u64;

        let phys_mask = 0x000FFFFF_FFFFF000;
        let flags_mask = 0xFFF;

        for i4 in 0..256 {
            let pml4_entry = *parent_pml4.add(i4);
            if pml4_entry & 1 == 0 { continue; }

            let parent_pml3_phys = pml4_entry & phys_mask;
            let child_pml3_frame = allocate_frame().expect("OOM PML3");
            *child_pml4.add(i4) = child_pml3_frame.start_address().as_u64() | (pml4_entry & flags_mask);

            let parent_pml3 = (parent_pml3_phys + offset) as *const u64;
            let child_pml3 = (child_pml3_frame.start_address().as_u64() + offset) as *mut u64;
            core::ptr::write_bytes(child_pml3, 0, 512 * 8);

            for i3 in 0..512 {
                let pml3_entry = *parent_pml3.add(i3);
                if pml3_entry & 1 == 0 { continue; }

                let parent_pml2_phys = pml3_entry & phys_mask;
                let child_pml2_frame = allocate_frame().expect("OOM PML2");
                *child_pml3.add(i3) = child_pml2_frame.start_address().as_u64() | (pml3_entry & flags_mask);

                let parent_pml2 = (parent_pml2_phys + offset) as *const u64;
                let child_pml2 = (child_pml2_frame.start_address().as_u64() + offset) as *mut u64;
                core::ptr::write_bytes(child_pml2, 0, 512 * 8);

                for i2 in 0..512 {
                    let pml2_entry = *parent_pml2.add(i2);
                    if pml2_entry & 1 == 0 { continue; }
                    
                    // 🚨 THE HUGE PAGE FIX FOR REAL HARDWARE 🚨
                    // If Bit 7 is set, this is a 2MB Huge Page (likely the GPU Framebuffer).
                    // We simply shallow-copy the entry so the child inherits the hardware map!
                    if pml2_entry & (1 << 7) != 0 { 
                        *child_pml2.add(i2) = pml2_entry;
                        continue; 
                    }

                    let parent_pt_phys = pml2_entry & phys_mask;
                    let child_pt_frame = allocate_frame().expect("OOM PT");
                    *child_pml2.add(i2) = child_pt_frame.start_address().as_u64() | (pml2_entry & flags_mask);

                    let parent_pt = (parent_pt_phys + offset) as *const u64;
                    let child_pt = (child_pt_frame.start_address().as_u64() + offset) as *mut u64;
                    core::ptr::write_bytes(child_pt, 0, 512 * 8);

                    for i1 in 0..512 {
                        let pt_entry = *parent_pt.add(i1);
                        if pt_entry & 1 == 0 { continue; }

                        // Is this an MMIO or Framebuffer page? (Check NO_CACHE or lacking USER_ACCESSIBLE)
                        if (pt_entry & 0x4) == 0 || (pt_entry & 0x10) != 0 {
                            *child_pt.add(i1) = pt_entry; // SHALLOW COPY
                            continue;
                        }

                        // Otherwise, it is True User RAM. Deep copy it!
                        let parent_page_phys = pt_entry & phys_mask;
                        let child_page_frame = allocate_frame().expect("OOM Page");
                        *child_pt.add(i1) = child_page_frame.start_address().as_u64() | (pt_entry & flags_mask);

                        let parent_page = (parent_page_phys + offset) as *const u8;
                        let child_page = (child_page_frame.start_address().as_u64() + offset) as *mut u8;
                        core::ptr::copy_nonoverlapping(parent_page, child_page, 4096);
                    }
                }
            }
        }
    }
}

pub fn clear_user_address_space(cr3_phys: PhysAddr) {
    unsafe {
        let mut lock = MEMORY_MANAGER.lock();
        let system = lock.as_mut().expect("Memory System not initialized");
        let offset = PHYS_MEM_OFFSET;
        let pml4 = (cr3_phys.as_u64() + offset) as *mut u64;
        let phys_mask = 0x000FFFFF_FFFFF000;

        for i4 in 0..256 {
            let pml4_entry = *pml4.add(i4);
            if pml4_entry & 1 == 0 { continue; }

            let pml3_phys = pml4_entry & phys_mask;
            let pml3 = (pml3_phys + offset) as *mut u64;

            for i3 in 0..512 {
                let pml3_entry = *pml3.add(i3);
                if pml3_entry & 1 == 0 { continue; }

                let pml2_phys = pml3_entry & phys_mask;
                let pml2 = (pml2_phys + offset) as *mut u64;

                for i2 in 0..512 {
                    let pml2_entry = *pml2.add(i2);
                    if pml2_entry & 1 == 0 { continue; }
                    if pml2_entry & (1 << 7) != 0 { continue; }

                    let pt_phys = pml2_entry & phys_mask;
                    let pt = (pt_phys + offset) as *mut u64;

                    for i1 in 0..512 {
                        let pt_entry = *pt.add(i1);
                        if pt_entry & 1 == 0 { continue; }

                        // 🚨 THE WIPER LOGIC 🚨
                        // Only free the RAM if it is USER_ACCESSIBLE and NOT hardware MMIO
                        if (pt_entry & 0x4) != 0 && (pt_entry & 0x10) == 0 {
                            let page_phys = pt_entry & phys_mask;
                            system.frame_allocator.deallocate_frame(PhysFrame::containing_address(PhysAddr::new(page_phys)));
                            *pt.add(i1) = 0; // Erase it from the table
                        }
                    }
                    // NOTE: We intentionally DO NOT free the intermediate PT/PML2 tables here!
                    // Because the Kernel ELF lives in the lower half of memory, freeing these 
                    // tables will delete the kernel routing and cause an instant page fault!
                }
            }
        }
        core::arch::asm!("mov cr3, {}", in(reg) cr3_phys.as_u64()); // Flush TLB
    }
}