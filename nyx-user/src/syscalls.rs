use core::arch::asm;

pub const SYS_PRINT: u64 = 0;
pub const SYS_DRAW_RECT: u64 = 1;
pub const SYS_ALLOC: u64 = 2;
pub const SYS_EXIT: u64 = 3;
pub const SYS_GET_TIME: u64 = 4;
pub const SYS_GET_MOUSE: u64 = 5;
pub const SYS_READ_KEY: u64 = 6;
pub const SYS_GET_SCREEN_INFO: u64 = 7;
pub const SYS_MAP_FRAMEBUFFER: u64 = 8;
pub const SYS_FS_COUNT: u64 = 10;
pub const SYS_FS_GET_NAME: u64 = 11;
pub const SYS_FS_READ: u64 = 12;
pub const SYS_FS_WRITE: u64 = 13;

#[inline(always)]
pub fn syscall(id: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> u64 {
    let mut ret;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") id => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("rcx") arg4,
            in("r8") arg5,
            options(nostack, preserves_flags)
        );
    }
    ret
}

pub fn sys_exit(code: u64) -> ! {
    syscall(SYS_EXIT, code, 0, 0, 0, 0);
    loop {}
}

pub fn sys_print(s: &str) {
    syscall(SYS_PRINT, s.as_ptr() as u64, s.len() as u64, 0, 0, 0);
}

pub fn sys_get_time() -> usize {
    syscall(SYS_GET_TIME, 0, 0, 0, 0, 0) as usize
}

pub fn sys_get_mouse() -> (usize, usize, bool, bool) {
    let packed = syscall(SYS_GET_MOUSE, 0, 0, 0, 0, 0);
    let x = (packed >> 32) as usize;
    let y = (packed >> 16) & 0xFFFF;
    let left = ((packed >> 1) & 1) == 1;
    let right = (packed & 1) == 1;
    (x as usize, y as usize, left, right)
}

pub fn sys_read_key() -> Option<char> {
    let ret = syscall(SYS_READ_KEY, 0, 0, 0, 0, 0);
    if ret == 0 { None } else { core::char::from_u32(ret as u32) }
}

pub fn sys_get_screen_info() -> (usize, usize, usize) {
    let mut w: u64 = 0;
    let mut h: u64 = 0;
    let mut s: u64 = 0;
    let ret = syscall(SYS_GET_SCREEN_INFO, &mut w as *mut u64 as u64, &mut h as *mut u64 as u64, &mut s as *mut u64 as u64, 0, 0);
    if ret == 1 { (w as usize, h as usize, s as usize) } else { (1024, 768, 1024) }
}

pub fn sys_map_framebuffer() -> u64 {
    syscall(SYS_MAP_FRAMEBUFFER, 0, 0, 0, 0, 0)
}

// UPDATED TO PASS PATH
pub fn sys_fs_count(path: &str) -> usize {
    syscall(SYS_FS_COUNT, path.as_ptr() as u64, path.len() as u64, 0, 0, 0) as usize
}

pub fn sys_fs_get_name(path: &str, index: usize, buffer: &mut [u8]) -> usize {
    // Arg1: idx, Arg2: buf_ptr, Arg3: path_ptr, Arg4: path_len
    syscall(SYS_FS_GET_NAME, index as u64, buffer.as_mut_ptr() as u64, path.as_ptr() as u64, path.len() as u64, 0) as usize
}

pub fn sys_fs_read(name: &str, buffer: &mut [u8]) -> usize {
    syscall(SYS_FS_READ, name.as_ptr() as u64, name.len() as u64, buffer.as_mut_ptr() as u64, 0, 0) as usize
}

pub fn sys_fs_write(name: &str, data: &[u8]) -> bool {
    syscall(SYS_FS_WRITE, name.as_ptr() as u64, name.len() as u64, data.as_ptr() as u64, data.len() as u64, 0) == 1
}