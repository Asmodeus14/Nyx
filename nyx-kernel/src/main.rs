#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::VirtAddr;
use crate::gui::{Painter, Color};
use crate::window::{Window, WindowType, WINDOW_MANAGER};
use alloc::string::String;
use alloc::vec;
use alloc::format;
use alloc::boxed::Box; 

mod allocator; mod memory; mod gui; mod shell; mod interrupts;
mod gdt; mod time; mod mouse; mod pci; mod task;     
mod executor; mod window; mod process; mod usb; 

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
    interrupts::init_syscalls();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap Init Failed");

    {
        let mut mm = crate::memory::MEMORY_MANAGER.lock();
        *mm = Some(crate::memory::MemorySystem { mapper, frame_allocator });
    }

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        {
            let mut wm = crate::window::WINDOW_MANAGER.lock();
            wm.set_resolution(info.width, info.height);
        }
        {
            let mut mouse = crate::mouse::MOUSE_STATE.lock();
            mouse.screen_width = info.width; mouse.screen_height = info.height;
            mouse.x = info.width / 2; mouse.y = info.height / 2;
        }
        unsafe {
            SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info });
            BACK_BUFFER = Some(gui::BackBuffer::new(info));
        }
    }

    crate::time::init();
    { let mut driver = crate::mouse::MouseDriver::new(); driver.init(); }
    
    // --- CREATE DEBUG WINDOW ---
    let mut log_window = Window::new(500, 100, 250, 400, "USB Log", WindowType::DebugLog);
    
    // --- SHOW WINDOW IMMEDIATELY ---
    { let mut wm = crate::window::WINDOW_MANAGER.lock(); wm.add(log_window); }
    // Force first paint
    crate::window::compositor_paint();

    // Re-lock window to modify buffer (We need to retrieve it from WM or use a shared reference)
    // NOTE: For simplicity in this single-threaded init, we will just create a temporary window struct 
    // for logging and add the FINAL state to the WM later, OR modify it in place.
    // ACTUALLY: The WM owns the window now. We can't modify `log_window` easily if it's moved.
    // FIX: We will create the window, Log to it, and Add it at the end? NO, user wants to see progress.
    // FIX: We will remove it, modify it, add it back in the loop? Slow but works.
    // BETTER FIX: We will just keep it local, paint it manually, and add it to WM at the end.
    
    // RESET: Let's keep `log_window` local for the init phase so we can write to it.
    // We will draw it manually using `log_window.draw(...)` to the backbuffer and present.
    
    let mut log_window = Window::new(500, 100, 250, 400, "USB Log", WindowType::DebugLog);

    // --- USB INIT ---
    {
        let mut pci = crate::pci::PciDriver::new();
        let devices = pci.scan();
        let mut found_xhci = false;

        for dev in devices {
            if dev.class_id == 0x0C && dev.subclass_id == 0x03 && dev.prog_if == 0x30 {
                found_xhci = true;
                log_window.buffer.push(String::from("xHCI Found"));
                
                unsafe {
                    if let Some(phys_addr) = pci.get_bar_address(&dev, 0) {
                        pci.enable_bus_master(&dev);

                        let virt_addr_opt = {
                            let mut mm_lock = crate::memory::MEMORY_MANAGER.lock();
                            if let Some(mm) = mm_lock.as_mut() {
                                crate::memory::map_mmio(phys_addr, 0x200000, &mut mm.mapper, &mut mm.frame_allocator).ok()
                            } else { None }
                        }; 

                        if let Some(virt_addr) = virt_addr_opt {
                            log_window.buffer.push(String::from("Map OK"));
                            
                            match crate::usb::XhciController::new(virt_addr.as_u64()) {
                                Ok(mut xhci) => {
                                    log_window.buffer.push(String::from("Alloc OK"));
                                    match xhci.init(&mut log_window) {
                                        Ok(_) => {
                                            log_window.buffer.push(String::from("Init: OK"));
                                            xhci.check_ports(&mut log_window); 
                                            core::mem::forget(xhci); 
                                        },
                                        Err(e) => {
                                            log_window.buffer.push(String::from("Init: Fail"));
                                            log_window.buffer.push(String::from(e));
                                        }
                                    }
                                },
                                Err(e) => {
                                    log_window.buffer.push(String::from("New Fail:"));
                                    log_window.buffer.push(String::from(e));
                                }
                            }
                        } else {
                            log_window.buffer.push(String::from("Map Fail!"));
                        }
                    } else {
                        log_window.buffer.push(String::from("No BAR 0!"));
                    }
                }
            }
        }
        if !found_xhci { log_window.buffer.push(String::from("No xHCI Device")); }
    }
    
    // Add Final State to WM
    { let mut wm = crate::window::WINDOW_MANAGER.lock(); wm.add(log_window); }

    x86_64::instructions::interrupts::enable(); 

    const PAGE_COUNT: u64 = 100;
    const PAGE_SIZE: u64 = 4096;
    const TOTAL_MEM: u64 = PAGE_COUNT * PAGE_SIZE;

    let user_base_addr = crate::memory::allocate_user_pages(PAGE_COUNT).expect("Failed to allocate user pages");
    const USER_BIN: &[u8] = include_bytes!("nyx-user.bin");
    
    unsafe {
        let code_ptr = user_base_addr.as_mut_ptr::<u8>();
        for (i, byte) in USER_BIN.iter().enumerate() { code_ptr.add(i).write(*byte); }
    }

    let offset = if USER_BIN.len() > 0 && USER_BIN[0] == 0 { 0x10 } else { 0 };
    let entry_point_addr = user_base_addr.as_u64() + offset;
    let user_stack_top = user_base_addr + TOTAL_MEM - 64u64;

    unsafe {
        if let Some(painter) = &mut SCREEN_PAINTER { painter.clear(Color::BLACK); }
        crate::process::jump_to_userspace(entry_point_addr, user_stack_top.as_u64());
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable(); 
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