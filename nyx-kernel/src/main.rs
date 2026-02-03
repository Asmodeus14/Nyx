#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use core::panic::PanicInfo;
use x86_64::VirtAddr;
use gui::{Painter, Rect, Color, BackBuffer};
use window::{Window, WindowManager};

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
    interrupts::init_idt(); // This now initializes the PICS

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap Init Failed");

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        unsafe { 
            SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info });
            BACK_BUFFER = Some(BackBuffer::new(info));
        }
    }

    crate::mouse::MOUSE.lock().init(); // Wakes up the PS/2 controller
    let mut wm = WindowManager::new();
    wm.add(Window::new(100, 100, 400, 300, "Welcome to NyxOS"));

    x86_64::instructions::interrupts::enable();

    loop {
        x86_64::instructions::interrupts::without_interrupts(|| {
            unsafe {
                if let (Some(back_buffer), Some(screen)) = (BACK_BUFFER.as_mut(), SCREEN_PAINTER.as_mut()) {
                    let time = crate::time::CMOS.lock().read_rtc();
                    let mouse_state = crate::mouse::MOUSE_STATE.lock();
                    
                    crate::shell::draw(back_buffer, &time);
                    wm.update(&mouse_state);
                    wm.draw(back_buffer);
                    back_buffer.draw_cursor(mouse_state.x, mouse_state.y);
                    back_buffer.present(screen);
                }
            }
        });
        x86_64::instructions::hlt(); // Will now wake up because PIC IRQ0 is enabled
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe { if let Some(s) = &mut SCREEN_PAINTER { s.clear(Color::RED); } }
    loop {}
}