//! Nyx randomness — getrandom(318).
pub fn fill_bytes(bytes: &mut [u8]) {
    const GETRANDOM: usize = 318;
    let mut off = 0;
    while off < bytes.len() {
        let ret: isize;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") GETRANDOM => ret,
                in("rdi") bytes.as_mut_ptr().add(off),
                in("rsi") bytes.len() - off,
                in("rdx") 0usize,
                out("rcx") _, out("r11") _, options(nostack),
            );
        }
        if ret <= 0 {
            // Kernel refused; fall back to a TSC-mixed fill so callers still get non-zero entropy.
            let mut s = rdtsc() ^ 0x9E37_79B9_7F4A_7C15u64;
            for b in &mut bytes[off..] {
                s ^= s >> 12; s ^= s << 25; s ^= s >> 27;
                *b = (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u8;
            }
            return;
        }
        off += ret as usize;
    }
}

#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)); }
    ((hi as u64) << 32) | (lo as u64)
}
