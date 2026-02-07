use x86_64::VirtAddr;
use x86_64::registers::rflags::RFlags;

pub unsafe fn jump_to_userspace(entry_point: u64, stack_pointer: u64) -> ! {
    use crate::gdt;
    
    let user_code_sel = gdt::get_user_code_selector();
    let user_data_sel = gdt::get_user_data_selector();

    // Fix for "reserved_bits" error:
    // We manually set Bit 1 (which is always 1 on x86) + Interrupt Flag
    let rflags = RFlags::INTERRUPT_FLAG | RFlags::from_bits_truncate(2);

    core::arch::asm!(
        "mov ds, ax",
        "mov es, ax",
        "push rax",         // SS (User Data)
        "push rsi",         // RSP (User Stack)
        "push rdx",         // RFLAGS
        "push rcx",         // CS (User Code)
        "push rdi",         // RIP (Entry Point)
        "iretq",
        in("rdi") entry_point,
        in("rsi") stack_pointer,
        in("rdx") rflags.bits(),
        in("rcx") user_code_sel,
        in("rax") user_data_sel,
        options(noreturn)
    );
}