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
use alloc::format;

// --- MODULES ---
mod allocator; mod memory; mod gui; mod shell; mod interrupts;
mod gdt; mod time; mod mouse; mod pci; mod task; mod executor; 
mod window; mod process; mod usb; mod scheduler; 
mod fs;
mod drivers; // Must contain 'pub mod nvme;' and 'pub mod ahci;'

// --- GLOBALS ---
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
    // 1. LOW LEVEL INIT
    gdt::init();
    interrupts::init_idt();
    
    // 2. MEMORY SETUP
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap Init Failed");

    {
        let mut mm = crate::memory::MEMORY_MANAGER.lock();
        *mm = Some(crate::memory::MemorySystem { mapper, frame_allocator });
    }

    // 3. GRAPHICS SETUP
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

    unsafe {
        if let Some(p) = &mut SCREEN_PAINTER {
            p.clear(Color::BLACK);
            p.draw_string(10, 10, "NyxOS Kernel v1.4 - NVMe Boot Sequence", Color::WHITE);
        }
    }

    // --- 4. DRIVER INITIALIZATION ---
    let mut nvme_driver = crate::drivers::nvme::NvmeDriver::init();
    let mut sata_driver = crate::drivers::ahci::AhciDriver::init();

    unsafe {
        if let Some(p) = &mut SCREEN_PAINTER {
            let mut y_pos = 40; // Dynamic Y position to avoid overwriting

            // --- A. NVMe CHECK (PRIMARY) ---
            if let Some(driver) = &mut nvme_driver {
                let (maj, min) = driver.get_version();
                p.draw_string(10, y_pos, &format!("NVMe ACTIVE: Ver {}.{}", maj, min), Color::GREEN);
                y_pos += 20;

                // 1. Detect Namespace
                let nsid = driver.find_active_namespace();
                p.draw_string(10, y_pos, &format!("- Active Namespace ID: {}", nsid), Color::CYAN);
                y_pos += 20;

                // 2. Create Queues
                if driver.create_io_queues() {
                    p.draw_string(10, y_pos, "- I/O Queues Created", Color::GREEN);
                    y_pos += 20;

                    // 3. Read Sector 0
                    p.draw_string(10, y_pos, "- Reading Sector 0...", Color::YELLOW);
                    y_pos += 20;
                    
                    if let Some(data) = driver.test_read_sector0() {
                         p.draw_string(10, y_pos, "READ SUCCESS!", Color::GREEN);
                         y_pos += 20;
                         
                         let hex = format!("DATA: {:02X} {:02X} {:02X} {:02X} ...", 
                            data[0], data[1], data[2], data[3]);
                         p.draw_string(10, y_pos, &hex, Color::WHITE);
                         y_pos += 20;
                         
                         if data[510] == 0x55 && data[511] == 0xAA {
                             p.draw_string(10, y_pos, "VALID MBR SIGNATURE (55AA)", Color::CYAN);
                         } else {
                             p.draw_string(10, y_pos, "NO MBR SIGNATURE (GPT or Raw?)", Color::YELLOW);
                         }
                         y_pos += 20;

                    } else {
                         p.draw_string(10, y_pos, "READ FAILED (Status != 0)", Color::RED);
                         y_pos += 20;
                    }
                } else {
                    p.draw_string(10, y_pos, "QUEUE CREATION FAILED", Color::RED);
                    y_pos += 20;
                }
            } else {
                p.draw_string(10, y_pos, "NVMe: NO CONTROLLER FOUND", Color::RED);
                y_pos += 20;
            }

            y_pos += 20; // Gap between sections

            // --- B. SATA CHECK (SECONDARY) ---
            if let Some(driver) = &mut sata_driver {
                p.draw_string(10, y_pos, &format!("SATA AHCI FOUND @ 0x{:X}", driver.abar), Color::WHITE);
                y_pos += 20;

                let pi = driver.mem.pi;
                let mut found_any = false;
                
                for i in 0..32 {
                    if (pi >> i) & 1 == 1 {
                        let det = driver.mem.ports[i].ssts & 0xF;
                        if det == 3 {
                            p.draw_string(10, y_pos, &format!("- Port {}: ACTIVE (HDD/SSD)", i), Color::CYAN);
                            found_any = true;
                        } else if det > 0 {
                            p.draw_string(10, y_pos, &format!("- Port {}: DETECTED (Status {})", i, det), Color::YELLOW);
                            found_any = true;
                        }
                        y_pos += 20; // Increment even if detected but not active
                    }
                }
                
                if !found_any {
                    p.draw_string(10, y_pos, "NO ACTIVE SATA DRIVES", Color::WHITE);
                    y_pos += 20;
                }
            } else {
                p.draw_string(10, y_pos, "SATA: NO CONTROLLER", Color::RED);
                y_pos += 20;
            }
            
            p.draw_string(10, y_pos + 20, "Booting Userspace in 3 seconds...", Color::WHITE);
        }
    }

    // 5. BOOT DELAY
    for _ in 0..600_000_000 { core::hint::spin_loop(); }

    crate::fs::FS.lock().init();

    // 6. USERSPACE JUMP
    const PAGE_COUNT: u64 = 8192; 
    let user_base = crate::memory::allocate_user_pages(PAGE_COUNT).expect("Alloc Failed");
    const USER_BIN: &[u8] = include_bytes!("nyx-user.bin");
    
    unsafe {
        let ptr = user_base.as_mut_ptr::<u8>();
        for (i, b) in USER_BIN.iter().enumerate() { ptr.add(i).write(*b); }
    }
    
    let entry = 0x1000000; 
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
            s.draw_string(10, 10, "KERNEL PANIC", Color::WHITE);
            s.draw_string(10, 30, &msg, Color::WHITE);
        }
    }
    loop { x86_64::instructions::hlt(); }
}