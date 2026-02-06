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
    gdt::init();
    interrupts::init_idt();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap Init Failed");

    {
        let mut mm = crate::memory::MEMORY_MANAGER.lock();
        *mm = Some(crate::memory::MemorySystem {
            mapper,
            frame_allocator,
        });
    }

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        unsafe {
            SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info });
            BACK_BUFFER = Some(gui::BackBuffer::new(info));
        }
    }

    init_timer();
    {
        let mut driver = crate::mouse::MouseDriver::new();
        driver.init();
    }

    // --- PHASE 2: PROCESS LIFECYCLE TEST ---
    let user_stack_top = crate::memory::create_user_stack()
        .expect("Failed to create user stack");

    // SHELLCODE: Print 'H', Print 'i', then Exit.
    // ---------------------------------------------
    // FIX: Updated size from 40 to 41 bytes
    let shellcode: [u8; 41] = [
        // -- STEP 1: Print 'H' --
        0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // MOV RAX, 1
        0x48, 0xC7, 0xC7, 0x48, 0x00, 0x00, 0x00, // MOV RDI, 'H'
        0xCD, 0x80,                               // INT 0x80
        
        // -- STEP 2: Print 'i' --
        0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // MOV RAX, 1
        0x48, 0xC7, 0xC7, 0x69, 0x00, 0x00, 0x00, // MOV RDI, 'i'
        0xCD, 0x80,                               // INT 0x80

        // -- STEP 3: Exit --
        0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // MOV RAX, 0
        0xCD, 0x80                                // INT 0x80
    ];
    
    let code_addr = user_stack_top - 4096u64;
    unsafe {
        let code_ptr = code_addr.as_mut_ptr::<u8>();
        for (i, byte) in shellcode.iter().enumerate() {
            code_ptr.add(i).write(*byte);
        }
    }

    unsafe {
        if let Some(painter) = &mut SCREEN_PAINTER {
            painter.clear(Color::BLACK);
            painter.draw_string(20, 20, "NyxOS Phase 2: Process Lifecycle", Color::CYAN);
            painter.draw_string(20, 40, "Running Script: Print('H') -> Print('i') -> Exit()", Color::WHITE);
            painter.draw_string(20, 60, "Watch for the boxes below...", Color::YELLOW);
        }
        
        crate::process::jump_to_userspace(code_addr.as_u64(), user_stack_top.as_u64());
    }
}

// Unused Rust stub
#[no_mangle]
extern "C" fn sample_user_program() { loop {} }

fn init_timer() {
    use x86_64::instructions::port::Port;
    let mut command_port = Port::<u8>::new(0x43);
    let mut data_port = Port::<u8>::new(0x40);

    unsafe {
        command_port.write(0x36);
        let divisor = 11931u16; 
        data_port.write((divisor & 0xFF) as u8);
        data_port.write((divisor >> 8) as u8);
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