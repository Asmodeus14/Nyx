use core::arch::asm;
use crate::gdt;

pub unsafe fn jump_to_userspace(entry_point: u64, user_stack_top: u64) -> ! {
    let user_code_selector = gdt::GDT.1.user_code_selector.0 | 3;
    let user_data_selector = gdt::GDT.1.user_data_selector.0 | 3;

    asm!(
        "mov ds, ax",
        "mov es, ax",
        in("ax") user_data_selector,
    );

    let rflags = 0x202; 

    asm!(
        "push rax",         // SS
        "push rsi",         // RSP
        "push rdx",         // RFLAGS
        "push rcx",         // CS
        "push rdi",         // RIP
        "iretq",            // "Return" to Ring 3
        in("rax") user_data_selector,
        in("rsi") user_stack_top,
        in("rdx") rflags,
        in("rcx") user_code_selector,
        in("rdi") entry_point,
        options(noreturn)
    );
}