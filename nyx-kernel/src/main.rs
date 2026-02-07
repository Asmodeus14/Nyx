#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::VirtAddr;
use crate::gui::{Painter, Color};

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

pub static mut SCREEN_PAINTER: Option<gui::VgaPainter> = None;
pub static mut BACK_BUFFER: Option<gui::BackBuffer> = None;

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Hardware Initialization
    gdt::init();
    interrupts::init_idt();
    interrupts::init_syscalls();

    // 2. Memory Initialization
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap Init Failed");

    {
        let mut mm = crate::memory::MEMORY_MANAGER.lock();
        *mm = Some(crate::memory::MemorySystem { mapper, frame_allocator });
    }

    // 3. Graphics Initialization
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        let width = info.width;
        let height = info.height;

        {
            let mut wm = crate::window::WINDOW_MANAGER.lock();
            wm.set_resolution(width, height);
        }
        {
            let mut mouse = crate::mouse::MOUSE_STATE.lock();
            mouse.screen_width = width;
            mouse.screen_height = height;
            mouse.x = width / 2;
            mouse.y = height / 2;
        }

        unsafe {
            SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info });
            BACK_BUFFER = Some(gui::BackBuffer::new(info));
        }
    }

    // 4. Timer & Interrupts (THE FIX IS HERE)
    crate::time::init(); // 1. Configure Chip to 1000Hz
    
    // 5. Mouse Init
    {
        let mut driver = crate::mouse::MouseDriver::new();
        driver.init();
    }

    // --- CRITICAL FIX: ENABLE INTERRUPTS ---
    // This tells the CPU to start listening to the "Tick" doorbell.
    x86_64::instructions::interrupts::enable(); 
    // ---------------------------------------

    // 6. Userspace Loading
    const PAGE_COUNT: u64 = 100;
    const PAGE_SIZE: u64 = 4096;
    const TOTAL_MEM: u64 = PAGE_COUNT * PAGE_SIZE;

    let user_base_addr = crate::memory::allocate_user_pages(PAGE_COUNT).expect("Failed to allocate user pages");
    const USER_BIN: &[u8] = include_bytes!("nyx-user.bin");
    
    unsafe {
        let code_ptr = user_base_addr.as_mut_ptr::<u8>();
        for (i, byte) in USER_BIN.iter().enumerate() {
            code_ptr.add(i).write(*byte);
        }
    }

    let offset = if USER_BIN.len() > 0 && USER_BIN[0] == 0 { 0x10 } else { 0 };
    let entry_point_addr = user_base_addr.as_u64() + offset;
    let user_stack_top = user_base_addr + TOTAL_MEM - 64u64;

    unsafe {
        if let Some(painter) = &mut SCREEN_PAINTER {
            painter.clear(Color::BLACK);
        }
        crate::process::jump_to_userspace(entry_point_addr, user_stack_top.as_u64());
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe {
        if let Some(s) = &mut SCREEN_PAINTER {
            s.clear(Color::RED);
            s.draw_string(20, 20, "KERNEL PANIC", Color::WHITE);
            let msg = alloc::format!("{}", info);
            s.draw_string(20, 50, &msg, Color::WHITE);
        }
    }
    loop { x86_64::instructions::hlt(); }
}