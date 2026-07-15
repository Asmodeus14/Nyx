//! Nyx stdio — real fd 0/1/2 backed by the Nyx read(0)/write(1) syscalls.
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};

#[inline]
unsafe fn sys_write(fd: usize, buf: &[u8]) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 1usize => ret,
            in("rdi") fd, in("rsi") buf.as_ptr(), in("rdx") buf.len(),
            out("rcx") _, out("r11") _, options(nostack),
        );
    }
    ret
}

#[inline]
unsafe fn sys_read(fd: usize, buf: &mut [u8]) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 0usize => ret,
            in("rdi") fd, in("rsi") buf.as_mut_ptr(), in("rdx") buf.len(),
            out("rcx") _, out("r11") _, options(nostack),
        );
    }
    ret
}

pub struct Stdin;
pub struct Stdout;
pub struct Stderr;

impl Stdin {
    pub const fn new() -> Stdin { Stdin }
}

impl io::Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { sys_read(0, buf) };
        if n < 0 { Ok(0) } else { Ok(n as usize) }
    }
    fn read_buf(&mut self, mut cursor: BorrowedCursor<'_, u8>) -> io::Result<()> {
        let mut tmp = [0u8; 512];
        let want = cursor.capacity().min(tmp.len());
        if want == 0 { return Ok(()); }
        let n = unsafe { sys_read(0, &mut tmp[..want]) };
        if n > 0 { cursor.append(&tmp[..n as usize]); }
        Ok(())
    }
    fn is_read_vectored(&self) -> bool { false }
}

impl Stdout {
    pub const fn new() -> Stdout { Stdout }
}

impl io::Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { sys_write(1, buf) };
        if n < 0 { Ok(buf.len()) } else { Ok(n as usize) }
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for b in bufs {
            let n = unsafe { sys_write(1, b) };
            if n > 0 { total += n as usize; }
        }
        Ok(total)
    }
    fn is_write_vectored(&self) -> bool { true }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl Stderr {
    pub const fn new() -> Stderr { Stderr }
}

impl io::Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { sys_write(2, buf) };
        if n < 0 { Ok(buf.len()) } else { Ok(n as usize) }
    }
    fn is_write_vectored(&self) -> bool { false }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

pub const STDIN_BUF_SIZE: usize = 512;

pub fn is_ebadf(_err: &io::Error) -> bool {
    false
}

pub fn panic_output() -> Option<impl io::Write> {
    Some(Stderr::new())
}
