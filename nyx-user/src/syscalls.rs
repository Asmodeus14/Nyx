// 0: Exit
pub fn sys_exit(code: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 0,
            in("rdi") code,
            options(noreturn)
        );
    }
}

// 1: Print
pub fn sys_print(c: char) {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 1,
            in("rdi") c as u64,
            out("rcx") _, out("r11") _,
        );
    }
}

// 2: Read Key
pub fn sys_read_key() -> Option<char> {
    let mut result: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 2,
            lateout("rax") result,
            out("rcx") _, out("r11") _,
        );
    }
    if result == 0 { None } else { Some(result as u8 as char) }
}

// 3: Get Mouse
pub fn sys_get_mouse() -> (usize, usize, bool, bool) {
    let mut result: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 3,
            lateout("rax") result,
            out("rcx") _, out("r11") _,
        );
    }
    let left = (result >> 63) & 1 == 1;
    let right = (result >> 62) & 1 == 1;
    let x = ((result >> 32) & 0xFFFF) as usize;
    let y = (result & 0xFFFF) as usize;
    (x, y, left, right)
}

// 4: Draw Pixel
pub fn sys_draw(x: usize, y: usize, color: u32) {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 4,
            in("rdi") x,
            in("rsi") y,
            in("rdx") color as u64,
            out("rcx") _, out("r11") _,
        );
    }
}

// 5: Blit
pub fn sys_blit(x: usize, y: usize, w: usize, h: usize, buffer: &[u32]) {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 5,
            in("rdi") x,
            in("rsi") y,
            in("rdx") w,
            in("r8") h,
            in("r9") buffer.as_ptr(),
            out("rcx") _, out("r11") _,
        );
    }
}

// 6: Get Screen Info
pub fn sys_get_screen_size() -> (usize, usize) {
    let mut result: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 6,
            lateout("rax") result,
            out("rcx") _, out("r11") _,
        );
    }
    let w = (result >> 32) as usize;
    let h = (result & 0xFFFFFFFF) as usize;
    (w, h)
}

// 7: Map Framebuffer
pub fn sys_map_framebuffer() -> u64 {
    let mut result: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 7,
            lateout("rax") result,
            out("rcx") _, out("r11") _,
        );
    }
    result
}

// 8: Get System Time (Ticks)
pub fn sys_get_time() -> u64 {
    let mut result: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 8,
            lateout("rax") result,
            out("rcx") _, out("r11") _,
        );
    }
    result
}