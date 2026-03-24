use alloc::boxed::Box;
use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use x86_64::registers::model_specific::KernelGsBase;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

pub struct PerCoreGdt {
    pub tss: &'static TaskStateSegment,
    pub gdt: &'static GlobalDescriptorTable,
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
    
    // 🚨 MEMORY SAFETY: Leak the Box so it has a permanent &'static lifetime.
    let tss_ref: &'static TaskStateSegment = Box::leak(tss);

    let mut gdt = Box::new(GlobalDescriptorTable::new());
    let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
    let data_selector = gdt.add_entry(Descriptor::kernel_data_segment());
    let user_data_selector = gdt.add_entry(Descriptor::user_data_segment());
    let user_code_selector = gdt.add_entry(Descriptor::user_code_segment());
    let tss_selector = gdt.add_entry(Descriptor::tss_segment(tss_ref));

    let gdt_ref: &'static GlobalDescriptorTable = Box::leak(gdt);

    PerCoreGdt {
        tss: tss_ref,
        gdt: gdt_ref,
        code_selector,
        data_selector,
        user_code_selector,
        user_data_selector,
        tss_selector,
    }
}

impl PerCoreGdt {
    /// Loads this specific GDT and TSS into the current physical CPU
    pub fn load(&self) {
        self.gdt.load();
        unsafe {
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

// ─────────────────────────────────────────────────────────────────────────
// UNIVERSAL ACCESSORS (WITH RING 3 RPL MASKS)
// ─────────────────────────────────────────────────────────────────────────
// Index 1 = 0x08, Index 2 = 0x10, Index 3 = 0x18, Index 4 = 0x20

pub fn get_kernel_code_selector() -> u16 { 8 }
pub fn get_kernel_data_selector() -> u16 { 16 }

// 🚨 THE FIX: Bitwise OR with 3 forces the CPU Requested Privilege Level to Ring 3
pub fn get_user_data_selector() -> u16 { 24 | 3 } // Evaluates to 27 (0x1B)
pub fn get_user_code_selector() -> u16 { 32 | 3 } // Evaluates to 35 (0x23)

pub fn load_kernel_gs(logical_id: usize) {
    unsafe {
        if let Some(per_cpu_array) = &crate::percpu::PER_CPU {
            let ptr = &per_cpu_array[logical_id] as *const _ as u64;
            KernelGsBase::write(VirtAddr::new(ptr));
        }
    }
}