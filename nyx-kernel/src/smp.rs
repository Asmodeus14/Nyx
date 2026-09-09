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

    // ★ `pre-smp` read 1802 and `post-smp` read 637, so the namespace dies inside this function.
    // Split it further: the trampoline copy, then one mark per AP brought up. If the count is still
    // 1802 after the copy and drops on the first AP, it is something an AP does in `ap_main` — and
    // the smashed block held a `0xFFFF_9000_...` value, which is the kernel-stack region.
    crate::acpi::boot_checkpoint("smp-trampoline");

    /// Per-AP checkpoint tags. `boot_checkpoint` takes a `&'static str`, and a fixed table is the
    /// cheapest way to get one per core without formatting in a path this delicate.
    const AP_TAGS: [&str; 8] = [
        "smp-ap0", "smp-ap1", "smp-ap2", "smp-ap3", "smp-ap4", "smp-ap5", "smp-ap6", "smp-ap7",
    ];

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
        // After this core has finished `ap_main` (or timed out). The first tag that shows a drop
        // names the core, and one core dropping it means the damage is per-AP work — the idle-task
        // allocation, the GS write, or the stack the trampoline handed it.
        if let Some(t) = AP_TAGS.get(logical_id) {
            crate::acpi::boot_checkpoint(t);
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
    
    // B-β.2c: give this core an IDLE TASK before it becomes a load-balancer target. The scheduler's
    // save-step writes the interrupted context into tasks[core_task_idx]; without a resident task,
    // the first task pushed here lands at index 0 and the first timer tick OVERWRITES its entry
    // frame with this boot hlt-loop's RSP — the task then never runs (this is why cross-core
    // spawn_thread hung: worker marked Running but executing the idle loop). With the idle task at
    // index 0, the first save adopts this hlt loop AS the idle task — exactly what the BSP does in
    // main.rs where its boot context becomes init's context. MUST happen before ACTIVE_CORES is
    // incremented (that's what makes this core visible to the load balancer).
    {
        // new_idle_ap: reserved-range PID! Process::new() here consumed PIDs 1..N BEFORE the boot
        // daemons were created, shifting init/compositor off their well-known numbers (apps hardcode
        // COMPOSITOR_PID=4 for window IPC) — that regression made every app unable to open a window.
        let mut idle_task = crate::process::Process::new_idle_ap().expect("AP idle task alloc failed");
        idle_task.name = *b"kernel-idle\0\0\0\0\0";
        // Crafted resume frame (mirrors main.rs). In practice the first timer save replaces
        // saved_rsp with this boot context before it is ever restored, but build it anyway so the
        // slot is valid even if scheduling order ever changes.
        unsafe {
            let iretq_ptr = idle_task.kernel_stack_top - 40;
            let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
            iret_slice[0] = crate::process::nyx_idle_task as u64;
            iret_slice[1] = 0x08; iret_slice[2] = 0x202;
            iret_slice[3] = idle_task.kernel_stack_top; iret_slice[4] = 0x10;
            let regs_ptr = iretq_ptr - 120;
            core::ptr::write_bytes(regs_ptr as *mut u8, 0, 120);
            let fxsave_ptr = (regs_ptr - 512) & !0xF;
            crate::process::init_fpu_state(fxsave_ptr as u64);
            let final_rsp = fxsave_ptr - 16;
            let bottom = core::slice::from_raw_parts_mut(final_rsp as *mut u64, 2);
            bottom[0] = regs_ptr; bottom[1] = 0;
            idle_task.saved_rsp = final_rsp;
        }
        let percpu = crate::percpu::current();
        percpu.scheduler.tasks.push(idle_task);
        percpu.scheduler.core_task_idx[logical_id % 32] = 0;
    }

    crate::smp::AP_READY.store(true, core::sync::atomic::Ordering::SeqCst);
    crate::smp::ACTIVE_CORES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);

    crate::apic::init_timer(0x40);
    unsafe { x86_64::instructions::interrupts::enable(); }

    loop { unsafe { x86_64::instructions::hlt(); } }
}