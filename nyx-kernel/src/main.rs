#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::VirtAddr;
use crate::gui::{Painter, Rect, Color};

mod allocator; mod memory; mod gui; mod shell; mod interrupts; mod gdt; mod time; mod mouse; mod window;

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

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        unsafe { 
            SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info });
            BACK_BUFFER = Some(gui::BackBuffer::new(info));
        }
    }

    crate::mouse::MOUSE.lock().init();
    
    // High-Res Windows
    let mut wm = crate::window::WindowManager::new();
    wm.add(crate::window::Window::new(100, 100, 800, 500, "Nyx Terminal"));
    wm.add(crate::window::Window::new(1000, 150, 400, 300, "System Monitor"));

    // Enable interrupts
    x86_64::instructions::interrupts::enable();

    // MAIN LOOP (No Delays, Maximum Speed)
    loop {
        x86_64::instructions::interrupts::without_interrupts(|| {
            unsafe {
                if let (Some(bb), Some(sc)) = (BACK_BUFFER.as_mut(), SCREEN_PAINTER.as_mut()) {
                    let time = crate::time::CMOS.lock().read_rtc();
                    let mouse_state = crate::mouse::MOUSE_STATE.lock();

                    wm.update(&mouse_state);

                    crate::shell::draw(bb, &time);
                    wm.draw(bb);
                    
                    // Modern Cursor (White with Red Center)
                    bb.draw_rect(Rect::new(mouse_state.x, mouse_state.y, 10, 10), Color::WHITE);
                    bb.draw_rect(Rect::new(mouse_state.x+1, mouse_state.y+1, 8, 8), Color::RED);

                    bb.present(sc);
                }
            }
        });
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { 
    unsafe { if let Some(s) = &mut SCREEN_PAINTER { s.clear(Color::RED); } }
    loop {} 
}