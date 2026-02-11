// 0: Exit
pub fn sys_exit(code: u64) -> ! {
    unsafe { core::arch::asm!("syscall", in("rax") 0, in("rdi") code, options(noreturn)); }
}

// 1: Print
pub fn sys_print(c: char) {
    unsafe { core::arch::asm!("syscall", in("rax") 1, in("rdi") c as u64, out("rcx") _, out("r11") _); }
}

// 2: Read Key
pub fn sys_read_key() -> Option<char> {
    let mut res: u64;
    unsafe { core::arch::asm!("syscall", in("rax") 2, lateout("rax") res, out("rcx") _, out("r11") _); }
    if res == 0 { None } else { Some(res as u8 as char) }
}

// 3: Get Mouse
pub fn sys_get_mouse() -> (usize, usize, bool, bool) {
    let mut res: u64;
    unsafe { core::arch::asm!("syscall", in("rax") 3, lateout("rax") res, out("rcx") _, out("r11") _); }
    // Unpack (Left, Right, X, Y)
    ((res >> 32) as usize & 0xFFFF, res as usize & 0xFFFF, (res >> 63) & 1 == 1, (res >> 62) & 1 == 1)
}

// 6: Get Screen Info (Width, Height, STRIDE)
pub fn sys_get_screen_info() -> (usize, usize, usize) {
    let mut res: u64;
    let mut stride: u64;
    unsafe {
        core::arch::asm!(
            "syscall", in("rax") 6, 
            lateout("rax") res, lateout("rdi") stride,
            out("rcx") _, out("r11") _
        );
    }
    ((res >> 32) as usize, res as usize & 0xFFFFFFFF, stride as usize)
}

// 7: Map Framebuffer
pub fn sys_map_framebuffer() -> u64 {
    let mut res: u64;
    unsafe { core::arch::asm!("syscall", in("rax") 7, lateout("rax") res, out("rcx") _, out("r11") _); }
    res
}

// 8: Time
pub fn sys_get_time() -> u64 {
    let mut res: u64;
    unsafe { core::arch::asm!("syscall", in("rax") 8, lateout("rax") res, out("rcx") _, out("r11") _); }
    res
}
// 9: Allocate Memory (Dynamic Backbuffer)
// Returns pointer to new memory
pub fn sys_alloc(size: usize) -> u64 {
    let mut res: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 9,
            in("rdi") size as u64,
            lateout("rax") res,
            out("rcx") _, out("r11") _,
        );
    }
    res
}