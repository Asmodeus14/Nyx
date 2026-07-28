// src/random.rs
//
// Hardware entropy, for the cases where guessable bytes are a security bug rather than a
// performance note.
//
// Why this exists: syscall 318 (`getrandom`) was written to seed std's `HashMap` RandomState, and
// its implementation was a TSC-seeded xorshift64* — which is exactly right for hash-DoS resistance
// and exactly wrong for TLS. rustls draws ECDHE ephemeral private keys, GCM nonces and the client
// random from `getrandom`. An attacker who can bound the boot time and TSC can search a xorshift
// seed space small enough to recover session keys, so "not cryptographically strong" silently
// becomes "every HTTPS session on this machine is breakable". Browser work made that reachable.
//
// The source, canonical (Intel SDM Vol. 1 §7.3.17, "Random Number Generator Instructions"):
//   - RDSEED returns bits straight from the on-die entropy source (a non-deterministic thermal-noise
//     circuit). It is the seed-grade primitive — use it when available.
//   - RDRAND returns bits from a CSPRNG that is itself reseeded from that entropy source. Still
//     cryptographic quality; it is the fallback because it is available on strictly more parts
//     (Ivy Bridge 2012+ vs Broadwell 2014+ for RDSEED).
//   - BOTH set CF=1 on success and CF=0 on *transient* failure (the entropy pool was momentarily
//     drained). A failure is not fatal and not permanent — the SDM's guidance is a bounded retry,
//     which is why neither is called in an unbounded loop here.
//
// Detection is CPUID, not a guess: RDRAND = leaf 1, ECX bit 30. RDSEED = leaf 7 subleaf 0, EBX
// bit 18. This laptop is a Comet Lake (Skylake-derived, 10th gen) and has both, but the detection
// is real because the fallback path below is a security decision, not a compatibility one.
use core::sync::atomic::{AtomicU8, Ordering};

/// What the CPU actually offers. Written once at boot, read everywhere after.
const SRC_NONE: u8 = 0;
const SRC_RDRAND: u8 = 1;
const SRC_RDSEED: u8 = 2;

static SOURCE: AtomicU8 = AtomicU8::new(SRC_NONE);

/// Probe CPUID for the DRNG instructions. Call once, early, before anything wants entropy.
pub fn init() {
    let mut src = SRC_NONE;

    // Leaf 1: ECX bit 30 = RDRAND.
    let (_, _, ecx1, _) = cpuid(1, 0);
    if ecx1 & (1 << 30) != 0 {
        src = SRC_RDRAND;
    }

    // Leaf 7 subleaf 0: EBX bit 18 = RDSEED. Only meaningful if leaf 7 exists at all.
    let (max_leaf, _, _, _) = cpuid(0, 0);
    if max_leaf >= 7 {
        let (_, ebx7, _, _) = cpuid(7, 0);
        if ebx7 & (1 << 18) != 0 {
            src = SRC_RDSEED;
        }
    }

    SOURCE.store(src, Ordering::Relaxed);

    match src {
        SRC_RDSEED => crate::serial_println!("[RANDOM] entropy source: RDSEED (on-die, seed grade)"),
        SRC_RDRAND => crate::serial_println!("[RANDOM] entropy source: RDRAND (hardware CSPRNG)"),
        _ => {
            // Loud on purpose. On such a machine `getrandom` still works — HashMap must never fail
            // — but it is NOT cryptographic, and anything building TLS on it is insecure.
            crate::serial_println!(
                "[RANDOM] ★ NO HARDWARE RNG (no RDRAND/RDSEED). getrandom() falls back to a"
            );
            crate::serial_println!(
                "[RANDOM] ★ TSC-seeded PRNG: fine for hash seeding, NOT SAFE FOR TLS KEYS."
            );
        }
    }
}

/// True when `fill` produces cryptographic-quality bytes. Callers that are choosing key material
/// should refuse to proceed when this is false rather than quietly using guessable bytes.
pub fn is_cryptographic() -> bool {
    SOURCE.load(Ordering::Relaxed) != SRC_NONE
}

#[inline]
fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx);
    unsafe {
        // RBX is the PIC base register in some builds, so it must be preserved by hand rather than
        // named as an operand — same reason the SMM fan code pushes it.
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx:e}, ebx",
            "pop rbx",
            ebx = out(reg) ebx,
            inlateout("eax") leaf => eax,
            inlateout("ecx") sub => ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}

/// One 64-bit draw from the best available hardware source. `None` means either there is no DRNG or
/// it transiently refused for the whole retry budget.
///
/// The retry counts are the SDM's own guidance: 10 for RDRAND, and a longer budget for RDSEED
/// because seed-grade bits are rate-limited by the physical entropy source and legitimately fail
/// more often under load.
fn hw_u64() -> Option<u64> {
    match SOURCE.load(Ordering::Relaxed) {
        SRC_RDSEED => {
            for _ in 0..64 {
                if let Some(v) = rdseed_step() {
                    return Some(v);
                }
                core::hint::spin_loop();
            }
            // Seed-grade drained: RDRAND is implied present (RDSEED never ships without it) and is
            // still cryptographic, so this is a quality-preserving fallback, not a downgrade to junk.
            for _ in 0..10 {
                if let Some(v) = rdrand_step() {
                    return Some(v);
                }
                core::hint::spin_loop();
            }
            None
        }
        SRC_RDRAND => {
            for _ in 0..10 {
                if let Some(v) = rdrand_step() {
                    return Some(v);
                }
                core::hint::spin_loop();
            }
            None
        }
        _ => None,
    }
}

#[inline]
fn rdrand_step() -> Option<u64> {
    let val: u64;
    let ok: u8;
    unsafe {
        core::arch::asm!(
            "rdrand {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) ok,
            options(nostack),
        );
    }
    if ok != 0 { Some(val) } else { None }
}

#[inline]
fn rdseed_step() -> Option<u64> {
    let val: u64;
    let ok: u8;
    unsafe {
        core::arch::asm!(
            "rdseed {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) ok,
            options(nostack),
        );
    }
    if ok != 0 { Some(val) } else { None }
}

/// Non-cryptographic fallback: the original TSC-seeded xorshift64*. Kept only so `getrandom` never
/// fails on a machine with no DRNG — std's `HashMap` calls it during startup and cannot cope with
/// an error. Never reached on any x86-64 part from 2012 onward.
fn weak_fill(buf: &mut [u8]) {
    let mut seed = {
        let tsc: u64;
        unsafe {
            core::arch::asm!("rdtsc", out("rax") tsc, out("rdx") _, options(nomem, nostack));
        }
        tsc ^ crate::time::UPTIME_MS
            .load(Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
    };
    let mut i = 0;
    while i < buf.len() {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let r = seed.wrapping_mul(0x2545_F491_4F6C_DD1D);
        let n = core::cmp::min(8, buf.len() - i);
        buf[i..i + n].copy_from_slice(&r.to_le_bytes()[..n]);
        i += n;
    }
}

/// Fill `buf` with random bytes. Returns true when those bytes are cryptographic quality.
///
/// Always fills, even when returning false — see `weak_fill`.
pub fn fill(buf: &mut [u8]) -> bool {
    let mut i = 0;
    while i < buf.len() {
        match hw_u64() {
            Some(v) => {
                let n = core::cmp::min(8, buf.len() - i);
                buf[i..i + n].copy_from_slice(&v.to_le_bytes()[..n]);
                i += n;
            }
            None => {
                // No DRNG, or it gave up. Fill the REMAINDER weakly and report non-crypto for the
                // whole buffer — a partially-strong buffer is still a weak key.
                weak_fill(&mut buf[i..]);
                return false;
            }
        }
    }
    true
}
