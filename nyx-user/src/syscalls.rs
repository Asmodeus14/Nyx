use core::arch::asm;

// ─────────────────────────────────────────────────────────────────────────
// LINUX X86_64 SYSCALL ID CONSTANTS (POSIX)
// ─────────────────────────────────────────────────────────────────────────
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_CLOCK_GETTIME: u64 = 228;

// ─────────────────────────────────────────────────────────────────────────
// RAW SYSCALL INVOCATION
// ─────────────────────────────────────────────────────────────────────────
#[inline(always)]
pub fn syscall(n: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64, arg6: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            in("r9") arg6,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

// ─────────────────────────────────────────────────────────────────────────
// POSIX STANDARD I/O
// ─────────────────────────────────────────────────────────────────────────
pub fn sys_read(fd: i64, buf: &mut [u8]) -> i64 {
    syscall(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0) as i64
}

pub fn sys_write(fd: i64, buf: &[u8]) -> i64 {
    syscall(SYS_WRITE, fd as u64, buf.as_ptr() as u64, buf.len() as u64, 0, 0, 0) as i64
}

pub fn sys_open(path: &str) -> i64 {
    syscall(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0) as i64
}

pub fn sys_close(fd: i64) -> i64 {
    syscall(SYS_CLOSE, fd as u64, 0, 0, 0, 0, 0) as i64
}

pub fn sys_exit(code: i64) -> ! {
    syscall(SYS_EXIT, code as u64, 0, 0, 0, 0, 0);
    loop {}
}

// ─────────────────────────────────────────────────────────────────────────
// CUSTOM NYX-OS GUI & SYSTEM SYSCALLS
// ─────────────────────────────────────────────────────────────────────────

pub fn sys_print(text: &str) {
    sys_write(1, text.as_bytes()); // FD 1 = STDOUT
}

pub fn sys_get_time() -> usize {
    syscall(504, 0, 0, 0, 0, 0, 0) as usize
}

pub fn sys_get_context_switches() -> u64 {
    syscall(523, 0, 0, 0, 0, 0, 0)
}

pub fn sys_get_boot_logs(buf: &mut [u8]) -> usize {
    syscall(518, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0) as usize
}

pub fn sys_get_hw_info(buf: &mut [u8]) -> usize {
    syscall(517, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0) as usize
}

// 🚨 THE FIX: Terminal expects a 32-byte DNA seed (Syscall 520)
pub fn sys_get_entity_state(state: &mut [u8; 32]) -> bool {
    syscall(520, state.as_mut_ptr() as u64, 32, 0, 0, 0, 0) == 1
}

// Sysinfo uses this one for the 4 floats! (Syscall 521)
pub fn sys_get_entity_stats(stats: &mut [f32; 4]) {
    syscall(521, stats.as_mut_ptr() as u64, 4, 0, 0, 0, 0);
}

pub fn sys_get_active_cores() -> usize {
    syscall(522, 0, 0, 0, 0, 0, 0) as usize
}

pub fn sys_get_mouse() -> (usize, usize, bool, bool) {
    let m = syscall(505, 0, 0, 0, 0, 0, 0);
    let x = (m >> 32) as usize;
    let y = ((m >> 16) & 0xFFFF) as usize;
    let left = ((m >> 1) & 1) == 1;
    let right = (m & 1) == 1;
    (x, y, left, right)
}

pub fn sys_read_key() -> Option<char> {
    let k = syscall(506, 0, 0, 0, 0, 0, 0);
    if k == 0 { None } else { core::char::from_u32(k as u32) }
}

pub fn sys_get_screen_info() -> (usize, usize, usize) {
    let mut w: u64 = 0;
    let mut h: u64 = 0;
    let mut s: u64 = 0;
    syscall(507, &mut w as *mut u64 as u64, &mut h as *mut u64 as u64, &mut s as *mut u64 as u64, 0, 0, 0);
    (w as usize, h as usize, s as usize)
}

pub fn sys_map_framebuffer() -> u64 {
    syscall(508, 0, 0, 0, 0, 0, 0)
}

pub fn sys_fs_count(path: &str) -> usize {
    syscall(510, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0) as usize
}

pub fn sys_fs_get_name(path: &str, idx: usize, buf: &mut [u8]) -> usize {
    syscall(511, idx as u64, buf.as_mut_ptr() as u64, path.as_ptr() as u64, path.len() as u64, 0, 0) as usize
}

pub fn sys_alloc_pages(pages: usize) -> u64 {
    syscall(519, pages as u64, 0, 0, 0, 0, 0)
}

// Added standard POSIX execve to launch ELF files!
pub fn sys_execve(path: &str) -> i64 {
    syscall(SYS_EXECVE, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0) as i64
}

// ─────────────────────────────────────────────────────────────────────────
// POSIX SAFE NETWORK ABSTRACTION
// ─────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct sockaddr_in {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

pub struct UdpSocket {
    fd: i64,
}

impl UdpSocket {
    pub fn new() -> Option<Self> {
        let fd = syscall(SYS_SOCKET, 2, 2, 0, 0, 0, 0) as i64; 
        if fd >= 0 {
            Some(Self { fd })
        } else {
            None
        }
    }

    pub fn connect(&self, ip_a: u8, ip_b: u8, ip_c: u8, ip_d: u8, port: u16) -> bool {
        let addr = sockaddr_in {
            sin_family: 2,          
            sin_port: port.to_be(), 
            sin_addr: [ip_a, ip_b, ip_c, ip_d],
            sin_zero: [0; 8],
        };
        
        let ret = syscall(
            SYS_CONNECT, 
            self.fd as u64, 
            &addr as *const _ as u64, 
            core::mem::size_of::<sockaddr_in>() as u64, 
            0, 0, 0
        ) as i64;
        
        ret == 0
    }

    pub fn send(&self, data: &[u8]) -> bool {
        sys_write(self.fd, data) > 0
    }

    pub fn recv(&self, buf: &mut [u8]) -> i64 {
        sys_read(self.fd, buf)
    }
}