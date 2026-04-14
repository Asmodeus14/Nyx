use alloc::vec::Vec;
use crate::gdt::PerCoreGdt;
use x86_64::registers::model_specific::Msr;

#[repr(C)] 
pub struct PerCpu {
    pub kernel_rsp: u64,        
    pub user_rsp: u64,          
    pub self_ptr: *mut PerCpu,  
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
        let stack_top = crate::memory::allocate_kernel_stack(8);
        let rsp0_top = crate::memory::allocate_kernel_stack(8);
        let gdt_state = crate::gdt::create_per_core_gdt(rsp0_top);
        let sched = crate::scheduler::Scheduler::new();

        data.push(PerCpu {
            kernel_rsp: rsp0_top, 
            user_rsp: 0,          
            self_ptr: core::ptr::null_mut(), 
            logical_id,
            apic_id,
            scheduler: sched,
            stack_top,
            gdt_state,
        });
    }

    unsafe { 
        PER_CPU = Some(data); 
        let per_cpu_slice = PER_CPU.as_mut().unwrap();
        for i in 0..per_cpu_slice.len() {
            let ptr = &mut per_cpu_slice[i] as *mut PerCpu;
            per_cpu_slice[i].self_ptr = ptr;
        }
    }

    // 🚨 THE FATAL FIX (Core 0): Force the ACTIVE GS_BASE to point to our struct immediately!
    let ptr = unsafe { &PER_CPU.as_ref().unwrap()[0] as *const _ as u64 };
    unsafe { Msr::new(0xC0000101).write(ptr); }

    crate::gdt::load_kernel_gs(0);
    
    // This is now mathematically safe to call!
    current().gdt_state.load(); 
    
    crate::serial_println!("[PERCPU] Core 0 initialized, GS loaded, and GDT/TSS active.");
}

pub fn current() -> &'static mut PerCpu {
    unsafe {
        let ptr: *mut PerCpu;
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) ptr);
        &mut *ptr
    }
}