#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(c_variadic)]
#![feature(naked_functions)]
#![allow(static_mut_refs)]
#![allow(warnings)]

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
pub mod partitioner;
pub mod thermal;
pub mod laptop_fans;
pub mod installer;

use alloc::boxed::Box;
pub use gui::{SCREEN_PAINTER, BACK_BUFFER};
use bootloader_api::{entry_point, BootInfo, config::{BootloaderConfig, Mapping}};
use x86_64::VirtAddr;
use x86_64::registers::control::{Cr4, Cr4Flags};
use x86_64::structures::gdt::{Descriptor, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::structures::DescriptorTablePointer;
use x86_64::PrivilegeLevel;

// ==========================================
// BAKED-IN TINY APP TARBALL
// ==========================================
pub static INITRD_TAR: &[u8] = include_bytes!("initrd.tar");

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.mappings.framebuffer = Mapping::Dynamic;
    config
};

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
        
        table[1] = ext(Descriptor::kernel_code_segment());    
        table[2] = ext(Descriptor::kernel_data_segment());    
        table[3] = ext(Descriptor::user_data_segment());      
        table[4] = ext(Descriptor::user_code_segment());      
        table[5] = ext(Descriptor::user_data_segment());      
        table[6] = ext(Descriptor::user_code_segment());      
        
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
    unsafe { crate::memory::BOOTLOADER_CR3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64(); }
    
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
        
        {
            let mut mouse_state = crate::mouse::MOUSE_STATE.lock();
            mouse_state.screen_width = info.width;
            mouse_state.screen_height = info.height;
        }
        crate::vga_println!("[BOOT] Framebuffer Mapped: {}x{}", info.width, info.height);
    }

    init_hardened_gdt(); 
    interrupts::init_idt();

    crate::vga_println!("[BOOT] Initializing PS/2 Legacy Trackpad Emulator...");
    let mut ps2_mouse = crate::mouse::MouseDriver::new();
    ps2_mouse.init();

    unsafe { 
        let mut pics = interrupts::PICS.lock();
        pics.initialize(); 
        pics.write_masks(0xFF, 0xFF); 
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
        crate::time::calibrate_tsc();
        ioapic::init();
        
        let bsp_apic_id = apic_ids[0] as u8;
        crate::ioapic::route_irq(1, bsp_apic_id, crate::interrupts::InterruptIndex::Keyboard as u8);
        crate::ioapic::route_irq(12, bsp_apic_id, crate::interrupts::InterruptIndex::Mouse as u8);
        
        // 🔥 THE FIX: Route the RTL8168 MSI Vector (0x30 = 48) directly to the CPU!
        crate::ioapic::route_irq(11, bsp_apic_id, 48); 
        
        smp::init_aps(&apic_ids);
        pci::enumerate_pci();
    } else {
        crate::vga_println!("[BOOT] WARN: ACPI Tables missing! Attempting degraded boot.");
        let apic_ids = [0];
        percpu::init(&apic_ids);
        time::init();
        crate::time::calibrate_tsc();
        pci::enumerate_pci();
    }

    // ==========================================
    // NVME HARDWARE DRIVER INITIALIZATION
    // ==========================================
    unsafe { crate::fs::GLOBAL_NVME = crate::drivers::nvme::NvmeDriver::init(); }
    
    unsafe {
        if let Some(ref mut driver) = crate::fs::GLOBAL_NVME { 
            driver.create_io_queues(); 
        }
        crate::entity::awaken_entity(&mut crate::fs::GLOBAL_NVME);
    }

    // ==========================================
    // PHYSICAL NVME VFS MOUNT POINT
    // ==========================================
    unsafe {
        if crate::fs::GLOBAL_NVME.is_some() {
            if let Some(ext4_fs) = crate::fs::NvmeLwExt4Fs::new() {
                crate::vfs::VFS.mount("/mnt/nvme", Box::new(ext4_fs));
                crate::vga_println!("[BOOT] Physical NVMe Hardware (lwext4 R/W) Mounted to /mnt/nvme");
                
                crate::installer::extract_tar_to_ext4(INITRD_TAR);
                
            } else {
                panic!("FATAL: NVMe Drive Found but no ext4 partition detected.");
            }
        } else {
            panic!("FATAL: No NVMe Drive Detected! Cannot boot without a system drive.");
        }
    }

    // ==========================================
    // SYSTEM BOOTSTRAP
    // ==========================================
    crate::vga_println!("[BOOT] Bootstrapping System Daemons...");
    x86_64::instructions::interrupts::disable();
    let percpu = crate::percpu::current();

    // 1. Thermal Governor
    let mut thermal_task = crate::process::Process::new().unwrap();
    thermal_task.name = *b"thermal-governor";
    unsafe {
        let iretq_ptr = thermal_task.kernel_stack_top - 40;
        let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
        iret_slice[0] = crate::thermal::nyx_task_manager_daemon as u64; 
        iret_slice[1] = 0x08; iret_slice[2] = 0x202;             
        iret_slice[3] = thermal_task.kernel_stack_top; iret_slice[4] = 0x10;              
        let regs_ptr = iretq_ptr - 120;
        core::ptr::write_bytes(regs_ptr as *mut u8, 0, 120); 
        let fxsave_ptr = (regs_ptr - 512) & !0xF;
        core::ptr::write_bytes(fxsave_ptr as *mut u8, 0, 512); 
        *(fxsave_ptr as *mut u32).add(6) = 0x1F80; 
        let final_rsp = fxsave_ptr - 16;
        let bottom = core::slice::from_raw_parts_mut(final_rsp as *mut u64, 2);
        bottom[0] = regs_ptr; bottom[1] = 0;
        thermal_task.saved_rsp = final_rsp;
    }

    // 2. Idle Task
    let mut idle_task = crate::process::Process::new().unwrap();
    idle_task.name = *b"kernel-idle\0\0\0\0\0";
    idle_task.is_idle = true; 
    unsafe {
        let iretq_ptr = idle_task.kernel_stack_top - 40;
        let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
        iret_slice[0] = crate::process::nyx_idle_task as u64; 
        iret_slice[1] = 0x08; iret_slice[2] = 0x202;             
        iret_slice[3] = idle_task.kernel_stack_top; iret_slice[4] = 0x10;              
        let regs_ptr = iretq_ptr - 120;
        core::ptr::write_bytes(regs_ptr as *mut u8, 0, 120);
        let fxsave_ptr = (regs_ptr - 512) & !0xF;
        core::ptr::write_bytes(fxsave_ptr as *mut u8, 0, 512);
        *(fxsave_ptr as *mut u32).add(6) = 0x1F80;
        let final_rsp = fxsave_ptr - 16;
        let bottom = core::slice::from_raw_parts_mut(final_rsp as *mut u64, 2);
        bottom[0] = regs_ptr; bottom[1] = 0;
        idle_task.saved_rsp = final_rsp;
    }

    // 3. Init Process (PID 1)
    crate::vga_println!("[BOOT] Loading Init.nyx into PID 1 directly from NVMe...");
    let mut init_process = crate::process::Process::new().expect("Failed to create init process");
    init_process.state = crate::scheduler::TaskState::Running;
    init_process.name = *b"nyx-init\0\0\0\0\0\0\0\0";
    
    let init_cr3 = init_process.cr3.as_u64();
    let init_kernel_stack = init_process.kernel_stack_top;
    
    percpu.scheduler.tasks.push(idle_task);    
    percpu.scheduler.tasks.push(init_process); 
    percpu.scheduler.tasks.push(thermal_task); 
    
    percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32] = 1;

    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) init_cr3);
        let percpu_base = percpu as *const _ as *mut u64;
        *percpu_base = init_kernel_stack;
    }

    let init_data = crate::vfs::VFS.read_file_alloc("/mnt/nvme/apps/Init.nyx/run.bin")
        .expect("VFS FATAL: Failed to load /mnt/nvme/apps/Init.nyx/run.bin from SSD!");
        
    let entry_point = crate::process::load_elf(&init_data).expect("ELF Parse Fail");
    
    let stack_base = 0x7FFF_0000_0000;
    let stack_pages = 32; 
    crate::memory::allocate_user_pages_at(stack_base, stack_pages).expect("Stack Map Fail");
    let stack_top = ((stack_base + (stack_pages as u64 * 4096)) & !0xF) - 8; 

    interrupts::init_syscalls();
    unsafe { percpu.user_rsp = stack_top; } 
    unsafe {
        let mut cr4 = Cr4::read();
        cr4.remove(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION);
        cr4.remove(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
        Cr4::write(cr4);
    }

    // 🔥 ADDED HERE: Safe Hardware Timer Initialization
    crate::apic::init_timer(0x40);

    crate::vga_println!("[BOOT] Jumping to Ring 3 Natively (Entry: {:#x})...", entry_point);
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
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("Alloc Error: {:?}", layout);
}