use x86_64::VirtAddr;

/// Jumps directly to a Ring 3 Userspace ELF entry point.
/// This function constructs an iretq stack frame and safely swaps the GS base.
pub unsafe fn jump_to_userspace(entry: u64, stack: u64) -> ! {
    core::arch::asm!(
        "cli",           // 1. Disable interrupts while we build the sensitive stack frame
        
        "mov ds, ax",    // 2. Load the Ring 3 Data Segment (0x1B) into data registers
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",
        
        // 🚨 SMP CRITICAL: Swap the Kernel's PerCpu GS pointer into hidden storage 
        // so it isn't active/exposed while userspace is running!
        "swapgs",        
        
        // 3. Construct the 5-part `iretq` stack frame for an x86_64 privilege drop:
        "push rax",      // Push SS (Stack Segment) -> 0x1B (Ring 3 Data)
        "push rcx",      // Push RSP (User Stack Pointer)
        "push 0x202",    // Push RFLAGS -> Enable Hardware Interrupts in Ring 3
        "push rdx",      // Push CS (Code Segment) -> 0x23 (Ring 3 Code)
        
        // 🚨 THE FIX: Use r8 instead of rbx to hold the entry point!
        "push r8",       // Push RIP (User Entry Point) 
        
        // 4. Fire the transition! The CPU pops the 5 values above and drops to Ring 3.
        "iretq",
        
        in("ax") 0x1B_u64,   // 27: Ring 3 Data Segment
        in("rdx") 0x23_u64,  // 35: Ring 3 Code Segment
        in("rcx") stack,
        in("r8") entry,      // 🚨 THE FIX: Bind the 'entry' variable to r8
        options(noreturn)
    );
}