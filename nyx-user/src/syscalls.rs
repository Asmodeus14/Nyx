use core::arch::asm;

// --- SYSCALL HELPERS ---
// These helpers handle the register setup for the 'syscall' instruction.
// Arguments: RDI, RSI, RDX, R10, R8, R9
// Return: RAX
// Clobbers: RCX, R11 (Used by CPU for return address and flags)

unsafe fn syscall0(n: usize) -> usize {
    let mut res: usize;
    asm!(
        "syscall",
        in("rax") n,
        lateout("rax") res,
        out("rcx") _, out("r11") _,
    );
    res
}

unsafe fn syscall1(n: usize, arg1: usize) -> usize {
    let mut res: usize;
    asm!(
        "syscall",
        in("rax") n,
        in("rdi") arg1,
        lateout("rax") res,
        out("rcx") _, out("r11") _,
    );
    res
}

unsafe fn syscall2(n: usize, arg1: usize, arg2: usize) -> usize {
    let mut res: usize;
    asm!(
        "syscall",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
        lateout("rax") res,
        out("rcx") _, out("r11") _,
    );
    res
}

unsafe fn syscall3(n: usize, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let mut res: usize;
    asm!(
        "syscall",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") res,
        out("rcx") _, out("r11") _,
    );
    res
}

unsafe fn syscall4(n: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    let mut res: usize;
    asm!(
        "syscall",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        in("r10") arg4, // System V ABI uses R10 for 4th arg in syscalls
        lateout("rax") res,
        out("rcx") _, out("r11") _,
    );
    res
}

// --- STANDARD SYSCALLS ---

// 0: Exit
pub fn sys_exit(code: u64) -> ! {
    unsafe { 
        asm!("syscall", in("rax") 0, in("rdi") code, options(noreturn)); 
    }
}

// 1: Print
pub fn sys_print(c: char) {
    unsafe { syscall1(1, c as usize); }
}

// 2: Read Key
pub fn sys_read_key() -> Option<char> {
    let res = unsafe { syscall0(2) };
    if res == 0 { None } else { Some(res as u8 as char) }
}

// 3: Get Mouse
pub fn sys_get_mouse() -> (usize, usize, bool, bool) {
    let res = unsafe { syscall0(3) } as u64;
    // Unpack (Left, Right, X, Y)
    ((res >> 32) as usize & 0xFFFF, res as usize & 0xFFFF, (res >> 63) & 1 == 1, (res >> 62) & 1 == 1)
}

// 6: Get Screen Info (Width, Height, Stride)
// This one is special because it returns multiple values in registers.
// We'll keep the raw ASM for this one to handle the specific packing.
pub fn sys_get_screen_info() -> (usize, usize, usize) {
    let mut res: u64;
    let mut stride: u64;
    unsafe {
        asm!(
            "syscall", in("rax") 6, 
            lateout("rax") res, lateout("rdi") stride,
            out("rcx") _, out("r11") _
        );
    }
    ((res >> 32) as usize, res as usize & 0xFFFFFFFF, stride as usize)
}

// 7: Map Framebuffer
pub fn sys_map_framebuffer() -> u64 {
    unsafe { syscall0(7) as u64 }
}

// 8: Time
pub fn sys_get_time() -> u64 {
    unsafe { syscall0(8) as u64 }
}

// 9: Allocate Memory
pub fn sys_alloc(size: usize) -> u64 {
    unsafe { syscall1(9, size) as u64 }
}

// --- FILE SYSTEM SYSCALLS ---

pub const SYS_FS_COUNT: usize = 10;
pub const SYS_FS_GET_NAME: usize = 11;
pub const SYS_FS_OPEN: usize = 12;
pub const SYS_FS_READ: usize = 13;
pub const SYS_FS_WRITE: usize = 14;

pub fn sys_fs_count() -> usize {
    unsafe { syscall0(SYS_FS_COUNT) }
}

pub fn sys_fs_get_name(idx: usize, buf: &mut [u8]) -> usize {
    unsafe { syscall3(SYS_FS_GET_NAME, idx, buf.as_mut_ptr() as usize, buf.len()) }
}

pub fn sys_fs_open(name: &str) -> usize {
    unsafe { syscall2(SYS_FS_OPEN, name.as_ptr() as usize, name.len()) }
}

pub fn sys_fs_read(id: usize, buf: &mut [u8]) -> usize {
    unsafe { syscall3(SYS_FS_READ, id, buf.as_mut_ptr() as usize, buf.len()) }
}

pub fn sys_fs_write(name: &str, data: &[u8]) -> usize {
    unsafe { 
        syscall4(
            SYS_FS_WRITE, 
            name.as_ptr() as usize, name.len(),
            data.as_ptr() as usize, data.len()
        ) 
    }
}