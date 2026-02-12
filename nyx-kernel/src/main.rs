#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;
use bootloader_api::{entry_point, BootInfo, config::Mapping, config::BootloaderConfig};
use x86_64::VirtAddr;
use crate::gui::{Painter, Color};
use crate::scheduler::{Scheduler, clock_task, SCHEDULER};
use core::sync::atomic::{AtomicU64, Ordering};

mod allocator; mod memory; mod gui; mod shell; mod interrupts;
mod gdt; mod time; mod mouse; mod pci; mod task; mod executor; 
mod window; mod process; mod usb; mod scheduler; 
mod fs;

pub static mut SCREEN_PAINTER: Option<gui::VgaPainter> = None;
pub static mut BACK_BUFFER: Option<gui::BackBuffer> = None;

static USER_ENTRY: AtomicU64 = AtomicU64::new(0);
static USER_STACK: AtomicU64 = AtomicU64::new(0);

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.mappings.framebuffer = bootloader_api::config::Mapping::Dynamic; 
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

    // Global Memory Manager
    {
        let mut mm = crate::memory::MEMORY_MANAGER.lock();
        *mm = Some(crate::memory::MemorySystem { mapper, frame_allocator });
    }

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        let virt_addr = fb.buffer().as_ptr() as u64;
        let phys_addr = crate::memory::virt_to_phys(virt_addr).expect("FB Phys Failed");
        unsafe { crate::gui::FRAMEBUFFER_PHYS_ADDR = phys_addr; }

        { let mut wm = crate::window::WINDOW_MANAGER.lock(); wm.set_resolution(info.width, info.height); }
        { let mut m = crate::mouse::MOUSE_STATE.lock(); m.screen_width = info.width; m.screen_height = info.height; m.x = info.width/2; m.y = info.height/2; }
        unsafe { SCREEN_PAINTER = Some(gui::VgaPainter { buffer: fb.buffer_mut(), info }); }
    }

    crate::time::init();
    { let mut driver = crate::mouse::MouseDriver::new(); driver.init(); }

    crate::fs::FS.lock().init();

    // --- USER MODE SETUP ---
    const PAGE_COUNT: u64 = 8192; 
    let user_base = crate::memory::allocate_user_pages(PAGE_COUNT).expect("Alloc Failed");
    const USER_BIN: &[u8] = include_bytes!("nyx-user.bin");
    
    unsafe {
        if let Some(p) = &mut SCREEN_PAINTER {
            use alloc::format;
            p.draw_string(10, 30, "KERNEL: FILESYSTEM LOADED", Color::GREEN);
            let msg = format!("Loading User Bin: {} bytes", USER_BIN.len());
            p.draw_string(10, 50, &msg, Color::YELLOW);
        }
        let ptr = user_base.as_mut_ptr::<u8>();
        for (i, b) in USER_BIN.iter().enumerate() { ptr.add(i).write(*b); }
    }
    
    let entry = 0x1000000; 
    
    // --- FIX: LOWER STACK POINTER ---
    // We subtract 4096 (1 Page) to ensure the stack top isn't touching unmapped memory (0x3000000).
    // This prevents "prefetch" page faults when the Kernel reads buffers on the stack.
    let stack = user_base.as_u64() + (PAGE_COUNT * 4096) - 4096;

    USER_ENTRY.store(entry, Ordering::SeqCst);
    USER_STACK.store(stack, Ordering::SeqCst);

    unsafe {
        SCHEDULER = Some(Scheduler::new());
        if let Some(sched) = &mut SCHEDULER {
            sched.spawn(clock_task, 10);
            sched.spawn(enter_userspace_trampoline, 500); 
        }
    }

    interrupts::init_syscalls(); 
    x86_64::instructions::interrupts::enable(); 
    loop { x86_64::instructions::hlt(); }
}

pub extern "C" fn enter_userspace_trampoline() {
    let entry = USER_ENTRY.load(Ordering::SeqCst);
    let stack = USER_STACK.load(Ordering::SeqCst);
    if entry != 0 { unsafe { crate::process::jump_to_userspace(entry, stack); } }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable(); 
    unsafe {
        if let Some(s) = &mut SCREEN_PAINTER {
            s.clear(Color::RED);
            use alloc::format;
            let msg = format!("{}", info);
            s.draw_string(10, 10, "PANIC", Color::WHITE);
            s.draw_string(10, 30, &msg, Color::WHITE);
        }
    }
    loop { x86_64::instructions::hlt(); }
}