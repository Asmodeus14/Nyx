use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::memory::phys_to_virt;
use x86_64::registers::model_specific::Msr;

pub static AP_READY: AtomicBool = AtomicBool::new(false);
pub static ACTIVE_CORES: AtomicUsize = AtomicUsize::new(1); 

static TRAMPOLINE_BYTES: &[u8] = include_bytes!("trampoline.bin"); 

pub fn init_aps(apic_ids: &[u32]) {
    crate::vga_println!("[SMP] Preparing to wake {} Application Processors...", apic_ids.len() - 1);

    let trampoline_phys = 0x8000_u64; 
    let trampoline_virt = phys_to_virt(trampoline_phys).expect("Failed to map trampoline");
    
    unsafe {
        core::ptr::copy_nonoverlapping(
            TRAMPOLINE_BYTES.as_ptr(),
            trampoline_virt as *mut u8,
            TRAMPOLINE_BYTES.len(),
        );
    }

    let trampoline_vector = (trampoline_phys >> 12) as u8;
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();

    for (logical_id, &apic_id) in apic_ids.iter().enumerate().skip(1) {
        crate::vga_println!("[SMP] Booting APIC ID {} (Logical Core {})...", apic_id, logical_id);
        
        let ap_stack_top = unsafe { crate::percpu::PER_CPU.as_ref().unwrap()[logical_id].stack_top };
        
        unsafe {
            let args_ptr = (trampoline_virt + 0xF00) as *mut u64; 
            *args_ptr.offset(0) = cr3;
            *args_ptr.offset(1) = apic_id as u64;
            *args_ptr.offset(2) = logical_id as u64;
            *args_ptr.offset(3) = ap_stack_top;
            *args_ptr.offset(4) = ap_main as u64; 
        }

        AP_READY.store(false, Ordering::SeqCst);

        crate::apic::send_init(apic_id);
        crate::time::sleep_ms(10); 
        
        crate::apic::send_sipi(apic_id, trampoline_vector);
        crate::time::sleep_ms(1);
        
        crate::apic::send_sipi(apic_id, trampoline_vector);
        
        let mut timeout = 0;
        while !AP_READY.load(Ordering::SeqCst) && timeout < 500 {
            crate::time::sleep_ms(1);
            timeout += 1;
        }

        if AP_READY.load(Ordering::SeqCst) {
            crate::vga_println!("      -> Core {} ONLINE!", logical_id);
        } else {
            crate::vga_println!("      -> ERR: Core {} FAILED (Timeout)", logical_id);
        }
    }
    
    crate::vga_println!("[SMP] Hardware routing complete. Active Cores: {}", ACTIVE_CORES.load(Ordering::SeqCst));
}

#[no_mangle]
pub extern "C" fn ap_main(_apic_id: u32, logical_id: usize) -> ! {
    // 🚨 THE FATAL FIX (APs): Force Active GS_BASE for the AP BEFORE it calls current()
    let ptr = unsafe { &crate::percpu::PER_CPU.as_ref().unwrap()[logical_id] as *const _ as u64 };
    unsafe { Msr::new(0xC0000101).write(ptr); }

    crate::gdt::load_kernel_gs(logical_id);
    
    // Safe to call now!
    crate::percpu::current().gdt_state.load();
    crate::interrupts::init_idt();
    crate::interrupts::init_syscalls();

    unsafe {
        let ap_stack = crate::percpu::PER_CPU.as_ref().unwrap()[logical_id].stack_top;
        core::arch::asm!("mov gs:[0], {}", in(reg) ap_stack);
    }

    unsafe {
        let apic_base = 0xFEE0_0000u64;
        let virt_base = crate::memory::phys_to_virt(apic_base).unwrap_or(apic_base);
        let svr_ptr = (virt_base + 0xF0) as *mut u32;
        let mut svr = core::ptr::read_volatile(svr_ptr);
        svr |= 0x100;
        svr |= 0xFF;
        core::ptr::write_volatile(svr_ptr, svr);
    }
    
    crate::smp::AP_READY.store(true, core::sync::atomic::Ordering::SeqCst);
    crate::smp::ACTIVE_CORES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);

    crate::apic::init_timer(0x40);
    unsafe { x86_64::instructions::interrupts::enable(); }

    loop { unsafe { x86_64::instructions::hlt(); } }
}