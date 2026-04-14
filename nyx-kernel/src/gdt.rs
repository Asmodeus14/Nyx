use alloc::boxed::Box;
use x86_64::structures::gdt::{Descriptor, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use x86_64::registers::model_specific::KernelGsBase;
use x86_64::structures::DescriptorTablePointer;
use x86_64::PrivilegeLevel;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

pub struct PerCoreGdt {
    pub tss: &'static TaskStateSegment,
    pub gdt: &'static [u64; 9],
    pub code_selector: SegmentSelector,
    pub data_selector: SegmentSelector,
    pub user_code_selector: SegmentSelector,
    pub user_data_selector: SegmentSelector,
    pub tss_selector: SegmentSelector,
}

pub fn create_per_core_gdt(rsp0_stack_top: u64) -> PerCoreGdt {
    let mut tss = Box::new(TaskStateSegment::new());
    
    // 1. Set RSP0: The dedicated Ring 0 stack for this specific core
    tss.privilege_stack_table[0] = VirtAddr::new(rsp0_stack_top);
    
    // 🚨 THE FIX: We MUST configure the Double Fault IST!
    // Without this, any stack corruption throws a silent Triple Fault and hangs!
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        const STACK_SIZE: usize = 4096 * 5;
        static mut DF_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
        VirtAddr::from_ptr(unsafe { &DF_STACK }) + STACK_SIZE
    };
    
    // 🚨 MEMORY SAFETY: Leak the Box so it has a permanent &'static lifetime.
    let tss_ref: &'static TaskStateSegment = Box::leak(tss);

    let mut table = Box::new([0u64; 9]);
    let ext = |d: Descriptor| -> u64 { match d { Descriptor::UserSegment(v) => v, _ => 0 } };
    
    table[1] = ext(Descriptor::kernel_code_segment());    // 0x08 Kernel Code
    table[2] = ext(Descriptor::kernel_data_segment());    // 0x10 Kernel Data
    table[3] = ext(Descriptor::user_data_segment());      // 0x18 User Data 
    table[4] = ext(Descriptor::user_code_segment());      // 0x20 User Code 32 (STAR BASE)
    table[5] = ext(Descriptor::user_data_segment());      // 0x28 User Data (SYSRET SS)
    table[6] = ext(Descriptor::user_code_segment());      // 0x30 User Code 64 (SYSRET CS)
    
    match Descriptor::tss_segment(tss_ref) {
        Descriptor::SystemSegment(low, high) => {
            table[7] = low;  // 0x38 TSS Low
            table[8] = high; // 0x40 TSS High
        }
        _ => {}
    }

    let gdt_ref: &'static [u64; 9] = Box::leak(table);

    PerCoreGdt {
        tss: tss_ref,
        gdt: gdt_ref,
        code_selector: SegmentSelector::new(1, PrivilegeLevel::Ring0),
        data_selector: SegmentSelector::new(2, PrivilegeLevel::Ring0),
        user_data_selector: SegmentSelector::new(5, PrivilegeLevel::Ring3),
        user_code_selector: SegmentSelector::new(6, PrivilegeLevel::Ring3),
        tss_selector: SegmentSelector::new(7, PrivilegeLevel::Ring0),
    }
}

impl PerCoreGdt {
    pub fn load(&self) {
        let ptr = DescriptorTablePointer {
            limit: (core::mem::size_of::<[u64; 9]>() - 1) as u16,
            base: VirtAddr::new(self.gdt.as_ptr() as u64),
        };
        
        unsafe {
            x86_64::instructions::tables::lgdt(&ptr);
            use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
            use x86_64::instructions::tables::load_tss;
            CS::set_reg(self.code_selector);
            DS::set_reg(self.data_selector);
            ES::set_reg(self.data_selector);
            SS::set_reg(self.data_selector);
            load_tss(self.tss_selector);
        }
    }
}

pub fn get_kernel_code_selector() -> u16 { 8 }
pub fn get_kernel_data_selector() -> u16 { 16 }
pub fn get_user_data_selector() -> u16 { 40 | 3 } 
pub fn get_user_code_selector() -> u16 { 48 | 3 } 

pub fn load_kernel_gs(logical_id: usize) {
    unsafe {
        if let Some(per_cpu_array) = &crate::percpu::PER_CPU {
            let ptr = &per_cpu_array[logical_id] as *const _ as u64;
            KernelGsBase::write(VirtAddr::new(ptr));
        }
    }
}