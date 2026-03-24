#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(c_variadic)]
#![feature(naked_functions)] // 🚨 REQUIRED: For our SMP-safe context switchers
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::{VirtAddr, structures::paging::PageTableFlags};
use crate::gui::{Painter, Color, Rect};
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::format;

// --- KERNEL MODULES ---
pub mod serial;
pub mod vga_log; 
pub mod vfs;
pub mod acpi;
pub mod apic; 
pub mod ioapic;
pub mod pci;  
pub mod drm;  
pub mod entity; 
pub mod c_stubs; 
pub mod percpu; 
pub mod smp;    

mod allocator;
mod memory;
mod gui;
mod shell;
mod interrupts;
mod gdt;
mod time;
mod mouse;
mod task;
mod executor;
mod window;
mod process;
mod usb;
mod scheduler;
mod fs;
mod drivers;

pub static mut SCREEN_PAINTER: Option<gui::VgaPainter> = None;
pub static mut BACK_BUFFER: Option<gui::BackBuffer> = None;

static USER_ENTRY: AtomicU64 = AtomicU64::new(0);
static USER_STACK: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Ehdr {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.mappings.framebuffer = bootloader_api::config::Mapping::Dynamic; 
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    crate::serial_println!("[INIT] NyxOS Kernel Booting...");

    // 1. Core CPU Architecture Init (Per-core GDT is handled in percpu::init)
    interrupts::init_idt();
    
    // Mask legacy PICs completely
    unsafe { 
        let mut pics = interrupts::PICS.lock();
        pics.initialize();
        pics.write_masks(0xFF, 0xFF); 
    };

    // 2. Memory Subsystem Init
    if let Some(offset) = boot_info.physical_memory_offset.into_option() {
        let phys_mem_offset = VirtAddr::new(offset);
        unsafe { crate::memory::PHYS_MEM_OFFSET = offset; }
        
        let mut mapper = unsafe { memory::init(phys_mem_offset, &boot_info.memory_regions) };
        {
            let mut system_lock = memory::MEMORY_MANAGER.lock();
            let system = system_lock.as_mut().expect("Mem Fail");
            allocator::init_heap(&mut mapper, &mut system.frame_allocator).expect("Heap Fail");
        }
    } else { panic!("Memory Error"); }

    // 3. Graphical Interface Init
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        unsafe { SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info }); }
        { let mut wm = crate::window::WINDOW_MANAGER.lock(); wm.set_resolution(info.width, info.height); }
        { let mut m = crate::mouse::MOUSE_STATE.lock(); m.screen_width = info.width; m.screen_height = info.height; m.x = info.width/2; m.y = info.height/2; }
    }

    crate::serial_println!("[INIT] Graphics initialized.");
    
    // 4. ACPI, APIC, and SMP Discovery
    if let Some(rsdp_addr) = boot_info.rsdp_addr.into_option() {
        acpi::init(rsdp_addr);
        acpi::init_intel_acpica();
        apic::init();
        
        // --- THE SMP BRINGUP ---
        let apic_ids = crate::apic::get_cpu_apic_ids();
        
        // Setup isolated stacks, GDTs, and map the trampoline
        crate::percpu::init(&apic_ids);
        crate::memory::identity_map_low_memory();
        
        crate::time::init();
        crate::ioapic::init();
        
        // Route hardware IRQs to Core 0 (BSP)
        crate::ioapic::route_irq(1, 0, crate::interrupts::InterruptIndex::Keyboard as u8);
        crate::ioapic::route_irq(12, 0, crate::interrupts::InterruptIndex::Mouse as u8);

        // Wake the Application Processors!
        crate::smp::init_aps(&apic_ids);

        // Turn on hardware interrupts
        x86_64::instructions::interrupts::enable();

        crate::pci::enumerate_pci();
    } else {
        crate::serial_println!("[ACPI] ERR: Bootloader did not provide RSDP!");
    }

    // 5. Hardware Drivers Init
    { let mut driver = crate::mouse::MouseDriver::new(); driver.init(); }

    // 6. OS Thread Spawning (Directly to the SMP GLOBAL_RUNQUEUE)
    unsafe {
        crate::serial_println!("[SCHEDULER] Spawning Threads...");
        
        crate::scheduler::spawn(crate::scheduler::clock_task, 1);
        crate::scheduler::spawn(crate::scheduler::background_worker, 1);
        crate::scheduler::spawn(network_thread, 1000);
        
        crate::serial_println!("[SMP] ✅ Global Work Queue populated");
    }

    // 7. Storage and Entity Init
    let mut nvme_driver_opt = crate::drivers::nvme::NvmeDriver::init();
    if let Some(ref mut driver) = nvme_driver_opt { driver.create_io_queues(); }
    
    crate::entity::awaken_entity(&mut nvme_driver_opt);
    
    if let Some(driver) = nvme_driver_opt { crate::fs::FS.lock().init(driver); }

    // 8. Userspace Setup
    crate::serial_println!("[OS] Loading Userspace Environment...");
    crate::vga_println!("[OS] Loading Userspace Environment...");
    
    const PAGE_COUNT: u64 = 8192; 
    let _ = crate::memory::allocate_user_pages(PAGE_COUNT).expect("Alloc User Failed");
    
    const USER_BIN: &[u8] = include_bytes!("nyx-user.bin");
    let entry_point_addr = unsafe { load_elf(USER_BIN) };
    
    let stack_addr = 0x4000000; 
    USER_ENTRY.store(entry_point_addr, Ordering::SeqCst);
    USER_STACK.store(stack_addr, Ordering::SeqCst);

    // Initialize BSP syscalls
    interrupts::init_syscalls(); 

    crate::serial_println!("[OS] Handing control to Ring 3...");
    crate::vga_println!("[OS] Handing control to Ring 3...");

    // Arm Core 0's timer so the local scheduler ticks
    crate::apic::init_timer(0x40);

    // Launch GUI and jump to Ring 3 on Core 0
    enter_userspace_trampoline();

    // The main thread drops into this loop if userspace exits
    loop { x86_64::instructions::hlt(); }
}

// --- HYBRID NETWORK THREAD ---
pub extern "C" fn network_thread() {
    loop {
        x86_64::instructions::interrupts::without_interrupts(|| {
            crate::drivers::net::poll_network();
        });
        
        crate::drivers::net::NETWORK_PENDING.store(false, core::sync::atomic::Ordering::Release);
        
        // THE FIX: Remove the 50,000 cycle spin-loop and use cooperative yielding.
        // Because this thread has 1000 tickets, it will instantly be scheduled again,
        // resulting in ultra-low latency polling without freezing the core.
        crate::scheduler::yield_now(); 
    }
}

pub extern "C" fn enter_userspace_trampoline() {
    let entry = USER_ENTRY.load(Ordering::SeqCst);
    let stack = USER_STACK.load(Ordering::SeqCst);
    if entry != 0 { unsafe { crate::process::jump_to_userspace(entry, stack); } }
}

unsafe fn load_elf(binary: &[u8]) -> u64 {
    let header = &*(binary.as_ptr() as *const Elf64Ehdr);
    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    let ph_size = header.e_phentsize as usize;

    for i in 0..ph_count {
        let ph_ptr = binary.as_ptr().add(ph_offset + i * ph_size) as *const Elf64Phdr;
        let ph = &*ph_ptr;

        if ph.p_type == 1 { 
            let dest = ph.p_vaddr as *mut u8;
            let src = binary.as_ptr().add(ph.p_offset as usize);
            let filesz = ph.p_filesz as usize;
            let memsz = ph.p_memsz as usize;
            core::ptr::copy_nonoverlapping(src, dest, filesz);
            if memsz > filesz { core::ptr::write_bytes(dest.add(filesz), 0, memsz - filesz); }
        }
    }
    header.e_entry
}

// ─────────────────────────────────────────────────────────────────────────
// 🚨 THE RSOD (RED SCREEN OF DEATH) PANIC HANDLER
// ─────────────────────────────────────────────────────────────────────────
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    
    // 1. Paint the entire screen stark red
    unsafe {
        if let Some(painter) = &mut SCREEN_PAINTER {
            let width = painter.info.width;
            let height = painter.info.height;
            painter.draw_rect(Rect { x: 0, y: 0, w: width, h: height }, Color::RED);
        }
    }

    // 2. Print the panic trace to the serial port
    crate::serial_println!("[PANIC] {}", info);
    
    // 3. Print the panic trace directly to the red VGA screen
    crate::vga_println!("\n\n[PANIC] {}", info);
    
    loop { x86_64::instructions::hlt(); }
}