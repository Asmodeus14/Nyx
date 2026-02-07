use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use lazy_static::lazy_static;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        
        // 1. IST for Double Faults
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            let stack_start = VirtAddr::from_ptr(unsafe { &STACK });
            let stack_end = stack_start + STACK_SIZE;
            stack_end
        };
        
        // 2. Kernel Stack for Syscalls (RSP0)
        tss.privilege_stack_table[0] = {
             const STACK_SIZE: usize = 4096 * 5;
             static mut KERNEL_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
             let stack_start = VirtAddr::from_ptr(unsafe { &KERNEL_STACK });
             let stack_end = stack_start + STACK_SIZE;
             stack_end
        };

        tss
    };
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        
        // Kernel Segments
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment()); // Index 1 (0x08)
        let data_selector = gdt.add_entry(Descriptor::kernel_data_segment()); // Index 2 (0x10)
        
        // User Segments (Data MUST be before Code for sysret/iretq compatibility)
        let user_data_selector = gdt.add_entry(Descriptor::user_data_segment()); // Index 3 (0x18)
        let user_code_selector = gdt.add_entry(Descriptor::user_code_segment()); // Index 4 (0x20)
        
        let tss_selector = gdt.add_entry(Descriptor::tss_segment(&TSS));
        
        (gdt, Selectors { 
            code_selector, 
            data_selector, 
            user_code_selector, 
            user_data_selector,
            tss_selector 
        })
    };
}

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

pub fn init() {
    use x86_64::instructions::tables::load_tss;
    use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        DS::set_reg(GDT.1.data_selector); 
        ES::set_reg(GDT.1.data_selector);
        SS::set_reg(GDT.1.data_selector);
        load_tss(GDT.1.tss_selector);
    }
}

// --- PUBLIC ACCESSORS ---

pub fn get_user_code_selector() -> u16 {
    GDT.1.user_code_selector.0
}

pub fn get_user_data_selector() -> u16 {
    GDT.1.user_data_selector.0
}

// FIX: These were missing, causing the compile error!
pub fn get_kernel_code_selector() -> u16 {
    GDT.1.code_selector.0
}

pub fn get_kernel_data_selector() -> u16 {
    GDT.1.data_selector.0
}