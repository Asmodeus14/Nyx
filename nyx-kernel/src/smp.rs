use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::memory::phys_to_virt;

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

        // Intel Standard Wakeup Sequence
        crate::apic::send_init(apic_id);
        crate::time::sleep_ms(10); 
        
        crate::apic::send_sipi(apic_id, trampoline_vector);
        crate::time::sleep_ms(1); // Wait 200us minimum
        
        // Second SIPI just in case it missed the first
        crate::apic::send_sipi(apic_id, trampoline_vector);
        
        // --- THE FIX: BOOT TIMEOUT ---
        // Wait maximum 500ms for the core to check in. If it triple faults,
        // we break the loop and continue booting the rest of the OS!
        let mut timeout = 0;
        while !AP_READY.load(Ordering::SeqCst) && timeout < 500 {
            crate::time::sleep_ms(1);
            timeout += 1;
        }

        if AP_READY.load(Ordering::SeqCst) {
            crate::vga_println!("      -> Core {} ONLINE!", logical_id);
        } else {
            crate::vga_println!("      -> ERR: Core {} FAILED (Timeout/Triple Fault)", logical_id);
        }
    }
    
    crate::vga_println!("[SMP] Hardware routing complete. Active Cores: {}", ACTIVE_CORES.load(Ordering::SeqCst));
}

#[no_mangle]
pub extern "C" fn ap_main(apic_id: u32, logical_id: usize) -> ! {
    x86_64::instructions::interrupts::disable();

    crate::gdt::load_kernel_gs(logical_id);
    crate::interrupts::init_idt();
    crate::apic::init_ap(); 

    ACTIVE_CORES.fetch_add(1, Ordering::SeqCst);
    
    // Set ready BEFORE printing, so the BSP immediately releases the boot lock
    AP_READY.store(true, Ordering::SeqCst);
    
    crate::vga_println!("[SMP] Core {} reporting for duty!", logical_id);

    x86_64::instructions::interrupts::enable();
    
    loop {
        x86_64::instructions::hlt();
    }
}