#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(c_variadic)]
#![feature(naked_functions)]
#![allow(static_mut_refs)]

extern crate alloc;

pub mod vga_log;
pub mod serial;
pub mod interrupts;
pub mod gdt;
pub mod memory;
pub mod allocator;
pub mod acpi;
pub mod apic;
pub mod ioapic;
pub mod smp;
pub mod percpu;
pub mod time;
pub mod task;
pub mod executor;
pub mod scheduler;
pub mod pci;
pub mod drivers;
pub mod fs;
pub mod vfs;
pub mod process;
pub mod gui;
pub mod window;
pub mod mouse;
pub mod shell;
pub mod entity;
pub mod c_stubs;
pub mod drm;
pub mod usb;

pub use gui::{SCREEN_PAINTER, BACK_BUFFER};

use bootloader_api::{entry_point, BootInfo, config::{BootloaderConfig, Mapping}};
use x86_64::VirtAddr;
use x86_64::registers::control::{Cr4, Cr4Flags};
use x86_64::structures::gdt::{Descriptor, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::structures::DescriptorTablePointer;
use x86_64::PrivilegeLevel;

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.mappings.framebuffer = Mapping::Dynamic; 
    config
};

static INIT_FS: &[u8] = include_bytes!("nyx-user.bin");

lazy_static::lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.privilege_stack_table[0] = {
            const STACK_SIZE: usize = 4096 * 8;
            static mut RING0_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            VirtAddr::from_ptr(unsafe { &RING0_STACK }) + STACK_SIZE
        };
        tss.interrupt_stack_table[0] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut DF_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            VirtAddr::from_ptr(unsafe { &DF_STACK }) + STACK_SIZE
        };
        tss
    };

    static ref GDT_TABLE: [u64; 9] = {
        let mut table = [0u64; 9];
        let ext = |d: Descriptor| -> u64 { match d { Descriptor::UserSegment(v) => v, _ => 0 } };
        
        table[1] = ext(Descriptor::kernel_code_segment());    // 0x08 Kernel Code
        table[2] = ext(Descriptor::kernel_data_segment());    // 0x10 Kernel Data
        table[3] = ext(Descriptor::user_data_segment());      // 0x18 User Data 
        table[4] = ext(Descriptor::user_code_segment());      // 0x20 User Code 32 (STAR BASE)
        table[5] = ext(Descriptor::user_data_segment());      // 0x28 User Data (SYSRET SS)
        table[6] = ext(Descriptor::user_code_segment());      // 0x30 User Code 64 (SYSRET CS)
        
        match Descriptor::tss_segment(&TSS) {
            Descriptor::SystemSegment(low, high) => { table[7] = low; table[8] = high; }
            _ => {}
        }
        table
    };
}

pub fn init_hardened_gdt() {
    let ptr = DescriptorTablePointer {
        limit: (core::mem::size_of::<[u64; 9]>() - 1) as u16,
        base: VirtAddr::new(GDT_TABLE.as_ptr() as u64),
    };
    
    unsafe {
        x86_64::instructions::tables::lgdt(&ptr);
        use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
        CS::set_reg(SegmentSelector::new(1, PrivilegeLevel::Ring0));
        DS::set_reg(SegmentSelector::new(2, PrivilegeLevel::Ring0));
        ES::set_reg(SegmentSelector::new(2, PrivilegeLevel::Ring0));
        SS::set_reg(SegmentSelector::new(2, PrivilegeLevel::Ring0));
        x86_64::instructions::tables::load_tss(SegmentSelector::new(7, PrivilegeLevel::Ring0));
    }
}

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    crate::serial_println!("[BOOT] NyxOS Kernel Starting...");
    crate::vga_println!("[BOOT] NyxOS Kernel Boot Sequence Initiated...");

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    unsafe { crate::memory::PHYS_MEM_OFFSET = phys_mem_offset.as_u64(); }
    let mut mapper = unsafe { memory::init(phys_mem_offset, &boot_info.memory_regions) };
    
    allocator::init_heap(&mut mapper, &mut memory::MEMORY_MANAGER.lock().as_mut().unwrap().frame_allocator).unwrap();

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        let raw_buffer = fb.buffer_mut();
        let fb_virt_ptr = raw_buffer.as_ptr() as u64;

        unsafe { 
            crate::gui::SCREEN_PAINTER = Some(gui::VgaPainter { buffer: raw_buffer, info }); 
            if let Some(phys) = crate::memory::virt_to_phys(fb_virt_ptr) { crate::gui::FRAMEBUFFER_PHYS_ADDR = phys; } 
            else { crate::gui::FRAMEBUFFER_PHYS_ADDR = fb_virt_ptr; }
        }
        crate::window::WINDOW_MANAGER.lock().set_resolution(info.width, info.height);
        
        // 🚨 THE FIX: Unlock the mouse boundaries to match your actual laptop screen!
        {
            let mut mouse_state = crate::mouse::MOUSE_STATE.lock();
            mouse_state.screen_width = info.width;
            mouse_state.screen_height = info.height;
        }

        crate::vga_println!("[BOOT] Framebuffer Mapped: {}x{}", info.width, info.height);
    }

    init_hardened_gdt(); 
    interrupts::init_idt();

    // WAKE UP THE PS/2 TRACKPAD EMULATOR USING YOUR DRIVER
    crate::vga_println!("[BOOT] Initializing PS/2 Legacy Trackpad Emulator...");
    crate::serial_println!("[BOOT] Initializing PS/2 Legacy Trackpad Emulator...");
    let mut ps2_mouse = crate::mouse::MouseDriver::new();
    ps2_mouse.init();

    // Forcefully Disable the Legacy 8259 PIC so it doesn't freeze your IOAPIC line!
    unsafe { 
        let mut pics = interrupts::PICS.lock();
        pics.initialize(); 
        pics.write_masks(0xFF, 0xFF); // MASK ALL INTERRUPTS
    }
    x86_64::instructions::interrupts::enable();

    if let Some(rsdp_addr) = boot_info.rsdp_addr.into_option() {
        acpi::init(rsdp_addr);
        acpi::init_intel_acpica();
        acpi::scan_for_modern_inputs();

        apic::init();
        let apic_ids = crate::apic::get_cpu_apic_ids();
        percpu::init(&apic_ids);
        crate::memory::identity_map_low_memory();
        time::init();
        ioapic::init();
        
        // Route the IRQs to the IOAPIC
        let bsp_apic_id = apic_ids[0] as u8;
        crate::ioapic::route_irq(1, bsp_apic_id, crate::interrupts::InterruptIndex::Keyboard as u8);
        crate::ioapic::route_irq(12, bsp_apic_id, crate::interrupts::InterruptIndex::Mouse as u8);
        
        smp::init_aps(&apic_ids);
        pci::enumerate_pci();
    } else {
        crate::vga_println!("[BOOT] WARN: ACPI Tables missing! Attempting degraded boot.");
        let apic_ids = [0];
        percpu::init(&apic_ids);
        time::init();
        pci::enumerate_pci();
    }

    let mut nvme_driver_opt = crate::drivers::nvme::NvmeDriver::init();
    if let Some(ref mut driver) = nvme_driver_opt { driver.create_io_queues(); }
    
    crate::entity::awaken_entity(&mut nvme_driver_opt);
    if let Some(driver) = nvme_driver_opt { crate::fs::FS.lock().init(driver); }

    crate::vga_println!("[BOOT] Core initialized. Parsing Userspace ELF...");
    let entry_point = process::load_elf(INIT_FS).expect("ELF Parse Fail");

    let stack_base = 0x7FFF_0000_0000;
    let stack_pages = 16;
    let allocated_stack = memory::allocate_user_pages_at(stack_base, stack_pages).expect("Stack Map Fail");
    let stack_top = (allocated_stack + (stack_pages as u64 * 4096)) & !0xF;

    interrupts::init_syscalls();
    unsafe { percpu::current().user_rsp = stack_top; } 

    unsafe {
        let mut k_rsp: u64;
        core::arch::asm!("mov {}, rsp", out(reg) k_rsp);
        core::arch::asm!("mov gs:[0], {}", in(reg) k_rsp);
    }

    unsafe {
        let mut cr4 = Cr4::read();
        cr4.remove(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION);
        cr4.remove(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
        Cr4::write(cr4);
    }

    crate::vga_println!("[BOOT] Jumping to Ring 3 (Entry: {:#x})...", entry_point);
    unsafe { process::enter_userspace(entry_point, stack_top); }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let msg = alloc::format!("{}", info);
    trigger_rsod(&msg);
}

pub fn trigger_rsod(msg: &str) -> ! {
    x86_64::instructions::interrupts::disable();
    unsafe {
        if let Some(painter) = &mut crate::gui::SCREEN_PAINTER {
            let buf = painter.buffer.as_mut();
            for i in (0..buf.len()).step_by(4) {
                buf[i] = 0; buf[i+1] = 0; buf[i+2] = 255; buf[i+3] = 255;
            }
        }
    }
    crate::vga_println!("\n\n  [FATAL KERNEL PANIC]\n  -> {}", msg);
    loop { x86_64::instructions::hlt(); }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! { panic!("Alloc Error: {:?}", layout); }