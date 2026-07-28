//! Entropy for the TLS stack.
//!
//! `getrandom` 0.2 has no `target_os = "nyx"` backend, and everything under rustls-rustcrypto
//! (p256/p384 via rand_core, RSA via rand) reaches for it — so without a registered backend the
//! crate does not build, and with a *bad* one it builds and is insecure.
//!
//! This routes to the kernel's `getrandom(318)`, which since 2026-07-27 is backed by RDSEED (on-die
//! thermal noise) falling back to RDRAND — see `nyx-kernel/src/random.rs`. It previously returned a
//! TSC-seeded xorshift, which was correct for its original job (seeding std's `HashMap`) and would
//! have been a serious vulnerability here: rustls draws ECDHE ephemeral private keys, GCM nonces and
//! the client random from this exact call.

/// Only on nyx. On the host, getrandom has a real OS backend and registering a custom one would be
/// both unnecessary and wrong — this crate is host-testable precisely so the HTTP parsing can be
/// exercised without hardware.
#[cfg(target_os = "nyx")]
mod imp {
    const SYS_GETRANDOM: usize = 318;

    fn nyx_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
        if buf.is_empty() {
            return Ok(());
        }
        let ret: isize;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") SYS_GETRANDOM => ret,
                in("rdi") buf.as_mut_ptr() as usize,
                in("rsi") buf.len(),
                in("rdx") 0usize,
                out("rcx") _, out("r11") _,
                options(nostack),
            );
        }
        // The kernel fills the whole buffer or reports EFAULT; a short count would mean key material
        // with predictable bytes in it, so treat anything but an exact fill as failure.
        if ret < 0 || ret as usize != buf.len() {
            return Err(getrandom::Error::UNSUPPORTED);
        }
        Ok(())
    }

    getrandom::register_custom_getrandom!(nyx_getrandom);
}
