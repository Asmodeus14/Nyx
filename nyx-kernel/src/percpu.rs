// nyx-kernel/src/percpu.rs

use alloc::vec::Vec;
use x86_64::registers::model_specific::KernelGsBase;
use x86_64::VirtAddr;

// #[derive(Debug)]
pub struct PerCpu {
    pub logical_id: usize,
    pub apic_id: u32,
    pub scheduler: crate::scheduler::Scheduler,
    pub stack_top: u64,
}

pub static mut PER_CPU: Option<Vec<PerCpu>> = None;

pub fn init(apic_ids: &[u32]) {
    crate::serial_println!("[PERCPU] Allocating isolated structures for {} cores...", apic_ids.len());
    let mut data = Vec::with_capacity(apic_ids.len());

    for (logical_id, &apic_id) in apic_ids.iter().enumerate() {
        // 1. Allocate a unique 32 KiB stack for this specific core
        let stack_top = crate::memory::allocate_kernel_stack(8);

        // 2. Give this core its own local scheduler instance
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
        });
    }

    unsafe { PER_CPU = Some(data); }

    // Immediately load the GS base for Core 0 (the core we are currently running on)
    crate::gdt::load_kernel_gs(0);
    crate::serial_println!("[PERCPU] Core 0 initialized and GS register loaded.");
}

/// Returns a mutable reference to the current core's isolated data
pub fn current() -> &'static mut PerCpu {
    let base = KernelGsBase::read().as_u64() as usize;
    unsafe { &mut *(base as *mut PerCpu) }
}