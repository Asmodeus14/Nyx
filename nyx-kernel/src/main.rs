#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::VirtAddr;
use crate::gui::{Painter, Color};
use crate::task::{Task, Priority};

mod allocator; mod memory; mod gui; mod shell; 
mod interrupts; mod gdt; mod time; mod mouse; 
mod window; mod pci; mod task; mod executor;
mod xhci; 

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
    
    // Memory
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

    // Video
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        unsafe { 
            SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info });
            BACK_BUFFER = Some(gui::BackBuffer::new(info));
        }
    }
    
    // Timer
    init_timer();
    
    // Mouse HW Init
    {
        let mut driver = crate::mouse::MouseDriver::new();
        driver.init();
    }

    // Scheduler
    let mut executor = executor::Executor::new();
    executor.spawn(Task::new(async_shell_task(), Priority::Low));
    executor.spawn(Task::new(async_quantum_task(), Priority::High));

    x86_64::instructions::interrupts::enable();
    executor.run();
    
    loop { x86_64::instructions::hlt(); }
}

async fn async_shell_task() {
    loop {
        // 1. Process Hardware Inputs (Drains queues safely)
        crate::shell::update();
        crate::mouse::update();

        // 2. Draw
        if let Some(bb) = unsafe { BACK_BUFFER.as_mut() } {
             let time = crate::time::CMOS.lock().read_rtc();
             crate::shell::draw(bb, &time);
             
             // Draw Mouse Cursor
             let mouse = crate::mouse::MOUSE_STATE.lock();
             let color = if mouse.left_click { crate::gui::Color::RED } else { crate::gui::Color::WHITE };
             
             // Draw Box Cursor
             for i in 0..10 {
                 bb.draw_rect(crate::gui::Rect::new(mouse.x + i, mouse.y, 2, 2), color);
                 bb.draw_rect(crate::gui::Rect::new(mouse.x, mouse.y + i, 2, 2), color);
                 bb.draw_rect(crate::gui::Rect::new(mouse.x + i, mouse.y + i, 2, 2), color);
             }
        }

        // 3. Present (Interrupts off only for memory copy)
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

async fn async_quantum_task() {
    let mut cycles: u64 = 0;
    loop {
        cycles = cycles.wrapping_add(1);
        executor::yield_now().await;
    }
}

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
fn panic(_info: &core::panic::PanicInfo) -> ! { 
    unsafe { if let Some(s) = &mut SCREEN_PAINTER { s.clear(Color::RED); } }
    loop {} 
}