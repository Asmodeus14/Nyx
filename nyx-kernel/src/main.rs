#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::{VirtAddr, structures::paging::PageTableFlags};
use crate::gui::{Painter, Color};
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::format;

pub mod serial;
pub mod vga_log; 
pub mod vfs;
pub mod acpi; 

mod allocator;
mod memory;
mod gui;
mod shell;
mod interrupts;
mod gdt;
mod time;
mod mouse;
mod pci;
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

// --- ELF STRUCTURES ---
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

    // 1. HARDWARE INIT
    gdt::init();
    interrupts::init_idt();
    
    unsafe { 
        let mut pics = interrupts::PICS.lock();
        pics.initialize();
        pics.write_masks(0xF8, 0xEF);
    };

    // 2. MEMORY
    if let Some(offset) = boot_info.physical_memory_offset.into_option() {
        let phys_mem_offset = VirtAddr::new(offset);
        
        // --- NEW: Save the offset globally for ACPI & PCI parsing ---
        unsafe { crate::memory::PHYS_MEM_OFFSET = offset; }

        let mut mapper = unsafe { memory::init(phys_mem_offset, &boot_info.memory_regions) };
        {
            let mut system_lock = memory::MEMORY_MANAGER.lock();
            let system = system_lock.as_mut().expect("Mem Fail");
            allocator::init_heap(&mut mapper, &mut system.frame_allocator).expect("Heap Fail");
        }
    } else {
        panic!("Memory Error");
    }

    // 3. GRAPHICS 
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        unsafe { SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info }); }
        { let mut wm = crate::window::WINDOW_MANAGER.lock(); wm.set_resolution(info.width, info.height); }
        { let mut m = crate::mouse::MOUSE_STATE.lock(); m.screen_width = info.width; m.screen_height = info.height; m.x = info.width/2; m.y = info.height/2; }
    }

    crate::vga_println!("[INIT] Graphics initialized. VGA Logger online.");

    // ==========================================
    // 3.5 ACPI INITIALIZATION
    // ==========================================
    if let Some(rsdp_addr) = boot_info.rsdp_addr.into_option() {
        acpi::init(rsdp_addr);
    } else {
        crate::vga_println!("[ACPI] ERR: Bootloader did not provide RSDP!");
    }
    // ==========================================

    // 4. DEVICE INIT
    crate::time::init();
    { 
        let mut driver = crate::mouse::MouseDriver::new(); 
        driver.init(); 
    }

    unsafe {
        if let Some(p) = &mut SCREEN_PAINTER {
            p.draw_string(10, 10, "NyxOS Kernel v2.5 - Multitasking Active", Color::WHITE);
        }
    }

    // --- INITIALIZE SCHEDULER ---
    unsafe {
        crate::vga_println!("[SCHEDULER] Spawning Background Threads...");
        let mut scheduler = crate::scheduler::Scheduler::new();
        
        scheduler.register_main_thread();
        
        scheduler.spawn(crate::scheduler::clock_task, 50);
        scheduler.spawn(crate::scheduler::background_worker, 10);
        
        crate::scheduler::SCHEDULER = Some(scheduler);
        crate::vga_println!("[SCHEDULER] Ready.");
    }

    // 5. ENABLE INTERRUPTS
    crate::vga_println!("[INIT] Enabling Hardware Interrupts...");
    x86_64::instructions::interrupts::enable();

    // 6. STORAGE
    let mut nvme_driver_opt = crate::drivers::nvme::NvmeDriver::init();
    if let Some(driver) = nvme_driver_opt {
        crate::fs::FS.lock().init(driver);
        crate::vga_println!("[STORAGE] NVMe Mounted successfully.");
    } else {
        crate::vga_println!("[STORAGE] ERR: No NVMe Found!");
    }

    // --- USERSPACE LOADER ---
    crate::vga_println!("[LOADER] Allocating Userspace Pages...");
    const PAGE_COUNT: u64 = 8192; 
    let _ = crate::memory::allocate_user_pages(PAGE_COUNT).expect("Alloc User Failed");
    
    const USER_BIN: &[u8] = include_bytes!("nyx-user.bin");
    let entry_point_addr = unsafe { load_elf(USER_BIN) };
    
    crate::vga_println!("[LOADER] Executing Userspace at: {:#x}", entry_point_addr);

    let stack_addr = 0x4000000; 

    USER_ENTRY.store(entry_point_addr, Ordering::SeqCst);
    USER_STACK.store(stack_addr, Ordering::SeqCst);

    interrupts::init_syscalls(); 
    
    enter_userspace_trampoline();

    loop { x86_64::instructions::hlt(); }
}

pub extern "C" fn enter_userspace_trampoline() {
    let entry = USER_ENTRY.load(Ordering::SeqCst);
    let stack = USER_STACK.load(Ordering::SeqCst);
    if entry != 0 { unsafe { crate::process::jump_to_userspace(entry, stack); } }
}

unsafe fn load_elf(binary: &[u8]) -> u64 {
    if binary[0] != 0x7F || binary[1] != b'E' || binary[2] != b'L' || binary[3] != b'F' {
        panic!("Invalid ELF Magic! Binary is not ELF.");
    }

    let header = &*(binary.as_ptr() as *const Elf64Ehdr);
    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    let ph_size = header.e_phentsize as usize;

    for i in 0..ph_count {
        let ph_ptr = binary.as_ptr().add(ph_offset + i * ph_size) as *const Elf64Phdr;
        let ph = &*ph_ptr;

        if ph.p_type == 1 { // PT_LOAD
            if ph.p_vaddr < 0x2000000 {
                 panic!("ELF Segment at {:#x} is below 0x2000000! Fix Linker Script.", ph.p_vaddr);
            }

            let dest = ph.p_vaddr as *mut u8;
            let src = binary.as_ptr().add(ph.p_offset as usize);
            let filesz = ph.p_filesz as usize;
            let memsz = ph.p_memsz as usize;

            core::ptr::copy_nonoverlapping(src, dest, filesz);

            if memsz > filesz {
                core::ptr::write_bytes(dest.add(filesz), 0, memsz - filesz);
            }
        }
    }
    header.e_entry
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    
    crate::serial_println!("[KERNEL PANIC] {}", info);
    crate::vga_println!("[KERNEL PANIC] {}", info);

    unsafe {
        if let Some(s) = &mut SCREEN_PAINTER {
            s.draw_string(10, 10, "KERNEL PANIC", Color::RED);
        }
    }
    loop { x86_64::instructions::hlt(); }
}