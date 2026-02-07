use core::arch::asm;

pub fn exit(code: u64) -> ! {
    unsafe {
        asm!(
            "syscall",
            in("rax") 0,
            in("rdi") code,
            out("rcx") _,
            out("r11") _,
        );
        loop {}
    }
}

pub fn print(c: char) {
    unsafe {
        asm!(
            "syscall",
            in("rax") 1,
            in("rdi") c as u64,
            out("rcx") _,
            out("r11") _,
        );
    }
}

pub fn read_key() -> Option<char> {
    let mut result: u64;
    unsafe {
        asm!(
            "syscall", 
            in("rax") 2, 
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    
    if result == 0 {
        None
    } else {
        use core::convert::TryFrom;
        char::try_from(result as u32).ok()
    }
}