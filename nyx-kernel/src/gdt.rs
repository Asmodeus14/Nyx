use x86_64::VirtAddr;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use lazy_static::lazy_static;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            
            let stack_start = VirtAddr::from_ptr(unsafe { &STACK });
            let stack_end = stack_start + STACK_SIZE as u64;
            stack_end
        };
        tss
    };
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        
        // --- RING 0 (KERNEL) ---
        let k_code = gdt.add_entry(Descriptor::kernel_code_segment());
        let k_data = gdt.add_entry(Descriptor::kernel_data_segment());
        
        // --- RING 3 (USERSPACE) ---
        // Critical for Phase 2: These segments allow code to run with lower privileges.
        let u_data = gdt.add_entry(Descriptor::user_data_segment());
        let u_code = gdt.add_entry(Descriptor::user_code_segment());
        
        let tss = gdt.add_entry(Descriptor::tss_segment(&TSS));
        
        (gdt, Selectors { 
            kernel_code_selector: k_code, 
            kernel_data_selector: k_data, 
            user_data_selector: u_data, 
            user_code_selector: u_code, 
            tss_selector: tss 
        })
    };
}

pub struct Selectors {
    pub kernel_code_selector: SegmentSelector,
    pub kernel_data_selector: SegmentSelector,
    pub user_data_selector: SegmentSelector, 
    pub user_code_selector: SegmentSelector, 
    pub tss_selector: SegmentSelector,
}

pub fn init() {
    use x86_64::instructions::tables::load_tss;
    use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};

    GDT.0.load();
    unsafe {
        // Reload Code Segment
        CS::set_reg(GDT.1.kernel_code_selector);
        
        // Reload Data Segments (Critical for Interrupts)
        SS::set_reg(GDT.1.kernel_data_selector);
        DS::set_reg(GDT.1.kernel_data_selector);
        ES::set_reg(GDT.1.kernel_data_selector);
        
        // Load TSS
        load_tss(GDT.1.tss_selector);
    }
}