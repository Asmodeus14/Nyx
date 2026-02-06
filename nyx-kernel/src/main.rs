#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::VirtAddr;
use crate::gui::{Painter, Color};
use crate::task::{Task, Priority};

// --- MODULE REGISTRATION ---
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
mod window; // Legacy stub, kept to prevent linker errors if referenced

// --- GLOBAL RESOURCES ---
pub static mut SCREEN_PAINTER: Option<gui::VgaPainter> = None;
pub static mut BACK_BUFFER: Option<gui::BackBuffer> = None;

// --- BOOT CONFIG ---
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. HARDWARE FOUNDATIONS
    gdt::init();
    interrupts::init_idt();

    // 2. MEMORY MANAGEMENT
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap Init Failed");

    // Save global MM for Phase 2 Drivers
    {
        let mut mm = crate::memory::MEMORY_MANAGER.lock();
        *mm = Some(crate::memory::MemorySystem {
            mapper,
            frame_allocator,
        });
    }

    // 3. GRAPHICS INIT
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        unsafe {
            SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info });
            BACK_BUFFER = Some(gui::BackBuffer::new(info));
        }
    }

    // 4. PERIPHERALS
    // Start the heartbeat timer (Phase 1 Requirement)
    init_timer(); 
    
    // Init PS/2 Mouse (Phase 0/1 Requirement)
    {
        let mut driver = crate::mouse::MouseDriver::new();
        driver.init();
    }

    // 5. ASYNC SCHEDULER (The Heart of Phase 1)
    let mut executor = executor::Executor::new();

    // Task A: The UI Shell (Runs at Low Priority)
    executor.spawn(Task::new(async_shell_task(), Priority::Low));

    // Task B: The Heartbeat (Runs at Low Priority)
    // This task runs alongside the shell to prove the OS isn't blocking.
    executor.spawn(Task::new(async_heartbeat_task(), Priority::Low));

    // 6. LAUNCH
    x86_64::instructions::interrupts::enable();
    executor.run();

    loop { x86_64::instructions::hlt(); }
}

// --- TASKS ---

/// The Shell Task: Handles User Input and Rendering
async fn async_shell_task() {
    loop {
        // 1. Drain Input Queues (Keyboard/Mouse)
        crate::shell::update();
        crate::mouse::update();

        // 2. Render UI
        if let Some(bb) = unsafe { BACK_BUFFER.as_mut() } {
            let time = crate::time::CMOS.lock().read_rtc();
            
            crate::shell::draw(bb, &time);

            // Draw Software Cursor (Red if clicking, White if idle)
            let mouse = crate::mouse::MOUSE_STATE.lock();
            let color = if mouse.left_click { crate::gui::Color::RED } else { crate::gui::Color::WHITE };
            
            for i in 0..10 {
                bb.draw_rect(crate::gui::Rect::new(mouse.x + i, mouse.y, 2, 2), color);
                bb.draw_rect(crate::gui::Rect::new(mouse.x, mouse.y + i, 2, 2), color);
                bb.draw_rect(crate::gui::Rect::new(mouse.x + i, mouse.y + i, 2, 2), color);
            }
        }

        // 3. Present Frame
        x86_64::instructions::interrupts::without_interrupts(|| {
            unsafe {
                if let (Some(bb), Some(sc)) = (BACK_BUFFER.as_mut(), SCREEN_PAINTER.as_mut()) {
                    bb.present(sc);
                }
            }
        });

        executor::yield_now().await;
    }
}

/// The Heartbeat Task: Proves Multitasking works
async fn async_heartbeat_task() {
    let mut last_tick = 0;
    loop {
        let current_tick = crate::time::get_ticks();
        
        // Logic check: Ensure time is moving forward
        if current_tick > last_tick + 100 {
            last_tick = current_tick;
            // In a future phase, this could update a system status LED on screen
        }
        
        executor::yield_now().await;
    }
}

// --- HARDWARE HELPERS ---

fn init_timer() {
    use x86_64::instructions::port::Port;
    let mut command_port = Port::<u8>::new(0x43);
    let mut data_port = Port::<u8>::new(0x40);

    unsafe {
        command_port.write(0x36);
        let divisor = 11931u16; // ~100Hz frequency
        data_port.write((divisor & 0xFF) as u8);
        data_port.write((divisor >> 8) as u8);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { if let Some(s) = &mut SCREEN_PAINTER { s.clear(Color::RED); } }
    loop { x86_64::instructions::hlt(); }
}