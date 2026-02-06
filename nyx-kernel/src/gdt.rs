use x86_64::VirtAddr;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use lazy_static::lazy_static;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

// NEW: Helper struct to enforce 16-byte alignment (Critical for x86 Stacks)
#[repr(align(16))]
struct Stack {
    data: [u8; 4096 * 5],
}

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        
        // 1. Double Fault Stack
        // Used if the kernel crashes so hard it can't use the normal stack.
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            static mut STACK: Stack = Stack { data: [0; 4096 * 5] };
            let stack_start = VirtAddr::from_ptr(unsafe { &STACK });
            let stack_end = stack_start + (4096 * 5) as u64;
            stack_end
        };

        // 2. Privilege Stack (RSP0) - THE MISSING PIECE
        // When 'int 0x80' is called from Ring 3, the CPU needs a Kernel Stack 
        // to switch to. We provide it here.
        tss.privilege_stack_table[0] = {
            static mut KERNEL_STACK: Stack = Stack { data: [0; 4096 * 5] };
            let stack_start = VirtAddr::from_ptr(unsafe { &KERNEL_STACK });
            let stack_end = stack_start + (4096 * 5) as u64;
            stack_end
        };

        tss
    };
}

lazy_static! {
    pub static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        
        // Ring 0 (Kernel)
        let k_code = gdt.add_entry(Descriptor::kernel_code_segment());
        let k_data = gdt.add_entry(Descriptor::kernel_data_segment());
        
        // Ring 3 (User)
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
        CS::set_reg(GDT.1.kernel_code_selector);
        SS::set_reg(GDT.1.kernel_data_selector);
        DS::set_reg(GDT.1.kernel_data_selector);
        ES::set_reg(GDT.1.kernel_data_selector);
        load_tss(GDT.1.tss_selector);
    }
}