use alloc::vec::Vec;
use x86_64::registers::model_specific::KernelGsBase;
use x86_64::VirtAddr;
use crate::gdt::PerCoreGdt;

pub struct PerCpu {
    pub logical_id: usize,
    pub apic_id: u32,
    pub scheduler: crate::scheduler::Scheduler,
    pub stack_top: u64,
    pub gdt_state: PerCoreGdt,
}

pub static mut PER_CPU: Option<Vec<PerCpu>> = None;

pub fn init(apic_ids: &[u32]) {
    crate::serial_println!("[PERCPU] Allocating isolated structures for {} cores...", apic_ids.len());
    let mut data = Vec::with_capacity(apic_ids.len());

    for (logical_id, &apic_id) in apic_ids.iter().enumerate() {
        // 1. General Kernel Execution Stack for this core
        let stack_top = crate::memory::allocate_kernel_stack(8);
        
        // 2. Dedicated Ring 0 Transition Stack (RSP0) for Syscalls/Interrupts from Ring 3
        let rsp0_top = crate::memory::allocate_kernel_stack(8);

        // 3. Generate a stable, leaked GDT and TSS for this core
        let gdt_state = crate::gdt::create_per_core_gdt(rsp0_top);

        // 4. Local Scheduler
        let mut sched = crate::scheduler::Scheduler::new();
        if logical_id == 0 {
            // Core 0 (the BSP) is already running the main thread
            sched.register_main_thread();
        }

        data.push(PerCpu {
            logical_id,
            apic_id,
            scheduler: sched,
            stack_top,
            gdt_state,
        });
    }

    unsafe { PER_CPU = Some(data); }

    // Initialize Core 0's Hardware State immediately
    crate::gdt::load_kernel_gs(0);
    current().gdt_state.load(); // Load Core 0's dedicated GDT/TSS
    
    crate::serial_println!("[PERCPU] Core 0 initialized, GS loaded, and GDT/TSS active.");
}

/// Returns a mutable reference to the current core's isolated data
pub fn current() -> &'static mut PerCpu {
    let base = KernelGsBase::read().as_u64() as usize;
    unsafe { &mut *(base as *mut PerCpu) }
}