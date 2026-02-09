use core::arch::asm;

// --- NEW SYSCALLS (sys_ prefix) ---

pub fn sys_exit(code: u64) -> ! {
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

pub fn sys_print(c: char) {
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

pub fn sys_read_key() -> Option<char> {
    let mut result: u64;
    unsafe {
        asm!(
            "syscall", 
            inout("rax") 2u64 => result, 
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

pub fn sys_get_mouse() -> (usize, usize) {
    let mut result: u64;
    unsafe {
        asm!(
            "syscall", 
            inout("rax") 3u64 => result, 
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    let x = (result >> 32) as usize;
    let y = (result & 0xFFFFFFFF) as usize;
    (x, y)
}

// --- LEGACY WRAPPERS (Prevent console.rs break) ---

pub fn exit(code: u64) -> ! { sys_exit(code) }
pub fn print(c: char) { sys_print(c) }
pub fn read_key() -> Option<char> { sys_read_key() }