#![cfg_attr(not(test), no_std)]

//! # `nyx-crypto` — SHA-1, HMAC-SHA1 and PBKDF2-SHA1
//!
//! **This is code motion, not new cryptography.** Every routine here was lifted verbatim out of
//! `nyx-kernel/src/drivers/net/iwlwifi.rs`, where it was written for the WPA2 four-way handshake and
//! has been exercised on every Wi-Fi connect this machine has ever made. It moved because Meridian's
//! lock screen needs a key derivation function and the Wi-Fi driver is a poor place to import one
//! from.
//!
//! Two callers, one implementation: `iwlwifi` for WPA2, `apps/shell` for the lock. The driver's old
//! `wpa_crypto_selftest` known-answer checks moved here as host tests, so what used to be verified
//! once per boot on the screen is now verified on every `cargo test`.
//!
//! ## ⚠️ What this is not
//!
//! SHA-1 is **broken for collision resistance** and has been since 2017. That is fine for both
//! callers and it is worth writing down why, because "the OS uses SHA-1 for passwords" is otherwise
//! an alarming sentence:
//!
//! - WPA2 mandates PBKDF2-HMAC-SHA1. There is no choice to make there.
//! - The lock needs *preimage* resistance, not collision resistance. SHA-1's preimage resistance is
//!   unbroken, and HMAC-SHA1 is not affected by the collision attacks at all. PBKDF2-HMAC-SHA1 is
//!   still what a great deal of shipping software uses for exactly this.
//!
//! The honest weakness of Nyx's lock is not the hash — see the note on `libs/crypto`'s consumer in
//! `apps/shell`: there is no disk encryption, so `auth.bin` gates a drawing routine and nothing
//! more. Swapping in SHA-256 would change nothing about that, which is why it was not worth doing as
//! part of a UI step.
//!
//! `no_std`. Everything that needs a heap says so in its signature.

extern crate alloc;

use alloc::vec::Vec;

/// One 64-byte block into the five-word state.
fn sha1_block(h: &mut [u32; 5], blk: &[u8]) {
    let mut w = [0u32; 80];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([blk[i * 4], blk[i * 4 + 1], blk[i * 4 + 2], blk[i * 4 + 3]]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
    for (i, &wi) in w.iter().enumerate() {
        let (f, k) = if i < 20 { ((b & c) | ((!b) & d), 0x5A82_7999u32) }
            else if i < 40 { (b ^ c ^ d, 0x6ED9_EBA1) }
            else if i < 60 { ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC) }
            else { (b ^ c ^ d, 0xCA62_C1D6) };
        let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
        e = d; d = c; c = b.rotate_left(30); b = a; a = t;
    }
    h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d); h[4] = h[4].wrapping_add(e);
}

/// SHA-1 of a message held entirely in memory.
///
/// ⚠️ The tail buffer is 128 bytes and the message length is appended into it, so this is only
/// correct because a final partial block plus padding plus the 8-byte length can never exceed two
/// blocks. It is not a streaming API and must not be turned into one by widening `last`.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = [0x6745_2301u32, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let ml = (data.len() as u64) * 8;
    let mut i = 0;
    while i + 64 <= data.len() { sha1_block(&mut h, &data[i..i + 64]); i += 64; }
    let rem = data.len() - i;
    let mut last = [0u8; 128];
    last[..rem].copy_from_slice(&data[i..]);
    last[rem] = 0x80;
    let total = if rem + 1 + 8 <= 64 { 64 } else { 128 };
    last[total - 8..total].copy_from_slice(&ml.to_be_bytes());
    sha1_block(&mut h, &last[..64]);
    if total == 128 { sha1_block(&mut h, &last[64..128]); }
    let mut out = [0u8; 20];
    for j in 0..5 { out[j * 4..j * 4 + 4].copy_from_slice(&h[j].to_be_bytes()); }
    out
}

/// HMAC-SHA1 (RFC 2104). A key longer than the 64-byte block is hashed first, as the RFC requires.
pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 { k[..20].copy_from_slice(&sha1(key)); }
    else { k[..key.len()].copy_from_slice(key); }
    let mut inner = Vec::with_capacity(64 + msg.len());
    let mut opad = [0x5cu8; 64];
    for i in 0..64 { inner.push(0x36 ^ k[i]); opad[i] ^= k[i]; }
    inner.extend_from_slice(msg);
    let ih = sha1(&inner);
    let mut outer = [0u8; 84];
    outer[..64].copy_from_slice(&opad);
    outer[64..].copy_from_slice(&ih);
    sha1(&outer)
}

/// PBKDF2-HMAC-SHA1 (RFC 2898).
///
/// ⚠️ `iters` is the whole cost of this function and the whole security margin of the lock. WPA2's
/// 4096 is calibrated for a handshake that has to complete while a radio is waiting; it is far too
/// low for a password store. The lock calibrates its own count by measurement and writes it into
/// `auth.bin` — see `apps/shell`. Do not copy 4096 into a new caller because the Wi-Fi one uses it.
pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], iters: u32, dklen: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bi = 1u32;
    while out.len() < dklen {
        let mut si = Vec::with_capacity(salt.len() + 4);
        si.extend_from_slice(salt);
        si.extend_from_slice(&bi.to_be_bytes());
        let mut u = hmac_sha1(password, &si);
        let mut t = u;
        for _ in 1..iters {
            u = hmac_sha1(password, &u);
            for k in 0..20 { t[k] ^= u[k]; }
        }
        out.extend_from_slice(&t);
        bi += 1;
    }
    out.truncate(dklen);
    out
}

/// Compare two byte strings in time that depends on their length and not on their contents.
///
/// Behind a PBKDF2 front door, at human typing speeds, on a machine with no network service to probe
/// this from, the timing channel here is very likely worth nothing. It is four lines. The reason to
/// write it now is that it is free now and awkward to retrofit once something is built on top of a
/// function that returns early.
///
/// ⚠️ Unequal lengths return `false` immediately — the *length* is not secret and pretending
/// otherwise would mean hashing to a fixed width first for no gain.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for i in 0..a.len() { diff |= a[i] ^ b[i]; }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three vectors the Wi-Fi driver used to check on every boot, in `wpa_crypto_selftest`.
    /// They ran once per power cycle and printed pass/fail to a screen nobody was reading; here they
    /// run on every `cargo test`. The first two are RFC 3174 / RFC 2202; the third is this machine's
    /// own network, computed offline, and is kept because it is the only vector that exercises a
    /// realistic iteration count.
    #[test]
    fn the_drivers_own_known_answers_still_hold() {
        assert_eq!(&sha1(b"abc")[..4], &[0xa9, 0x99, 0x3e, 0x36]);
        assert_eq!(&hmac_sha1(&[0x0b; 20], b"Hi There")[..4], &[0xb6, 0x17, 0x31, 0x86]);
        assert_eq!(&pbkdf2_sha1(b"dipti1978", b"dipti", 4096, 32)[..4], &[0x73, 0x04, 0x56, 0x57]);
    }

    /// RFC 3174 in full, not just the four bytes the boot-time check could afford to compare.
    #[test]
    fn sha1_matches_rfc_3174() {
        assert_eq!(sha1(b"abc"), [
            0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e,
            0x25, 0x71, 0x78, 0x50, 0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
        ]);
        assert_eq!(sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"), [
            0x84, 0x98, 0x3e, 0x44, 0x1c, 0x3b, 0xd2, 0x6e, 0xba, 0xae,
            0x4a, 0xa1, 0xf9, 0x51, 0x29, 0xe5, 0xe5, 0x46, 0x70, 0xf1,
        ]);
        // The empty message. Nothing in the tree hashes one, which is exactly why it is here — the
        // padding path for `rem == 0` is otherwise never taken.
        assert_eq!(sha1(b""), [
            0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55,
            0xbf, 0xef, 0x95, 0x60, 0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09,
        ]);
    }

    /// The block boundary is where a hash implementation goes wrong, so walk across it. 55 bytes is
    /// the last length whose padding fits one block, 56 the first that needs two.
    #[test]
    fn the_padding_boundary_is_not_off_by_one() {
        // Reference digests for 'a' * n, n = 54..=65.
        const EXPECTED: [(usize, [u8; 4]); 4] = [
            (55, [0xc1, 0xc8, 0xbb, 0xdc]),
            (56, [0xc2, 0xdb, 0x33, 0x0f]),
            (63, [0x03, 0xf0, 0x9f, 0x5b]),
            (64, [0x00, 0x98, 0xba, 0x82]),
        ];
        for (n, want) in EXPECTED {
            let msg = alloc::vec![b'a'; n];
            assert_eq!(&sha1(&msg)[..4], &want[..], "sha1 of {} 'a's", n);
        }
    }

    /// RFC 2202 case 3 uses a 20-byte key; case 6 uses an 80-byte one, which is the only thing that
    /// exercises the "hash the key first" branch. The driver's own self-test never took it.
    #[test]
    fn hmac_hashes_an_oversized_key() {
        let long = [0xaau8; 80];
        let m = hmac_sha1(&long, b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(m, [
            0xaa, 0x4a, 0xe5, 0xe1, 0x52, 0x72, 0xd0, 0x0e, 0x95, 0x70,
            0x56, 0x37, 0xce, 0x8a, 0x3b, 0x55, 0xed, 0x40, 0x21, 0x12,
        ]);
    }

    /// RFC 6070. Two of these ask for a key longer than one SHA-1 block, which is the only thing
    /// that exercises PBKDF2's `bi` counter past 1 — the WPA2 caller always asks for exactly 32
    /// bytes and so never increments it twice.
    #[test]
    fn pbkdf2_matches_rfc_6070() {
        assert_eq!(pbkdf2_sha1(b"password", b"salt", 1, 20), alloc::vec![
            0x0c, 0x60, 0xc8, 0x0f, 0x96, 0x1f, 0x0e, 0x71, 0xf3, 0xa9,
            0xb5, 0x24, 0xaf, 0x60, 0x12, 0x06, 0x2f, 0xe0, 0x37, 0xa6,
        ]);
        assert_eq!(pbkdf2_sha1(b"password", b"salt", 2, 20), alloc::vec![
            0xea, 0x6c, 0x01, 0x4d, 0xc7, 0x2d, 0x6f, 0x8c, 0xcd, 0x1e,
            0xd9, 0x2a, 0xce, 0x1d, 0x41, 0xf0, 0xd8, 0xde, 0x89, 0x57,
        ]);
        // 25 bytes out of a 20-byte hash — two blocks, so the counter has to advance.
        assert_eq!(
            pbkdf2_sha1(b"passwordPASSWORDpassword",
                        b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, 25),
            alloc::vec![
                0x3d, 0x2e, 0xec, 0x4f, 0xe4, 0x1c, 0x84, 0x9b, 0x80, 0xc8, 0xd8, 0x36, 0x62,
                0xc0, 0xe4, 0x4a, 0x8b, 0x29, 0x1a, 0x96, 0x4c, 0xf2, 0xf0, 0x70, 0x38,
            ]
        );
    }

    /// A different iteration count must produce a different key, or a stored count means nothing and
    /// raising it later would silently accept every old password.
    #[test]
    fn the_iteration_count_actually_changes_the_key() {
        let a = pbkdf2_sha1(b"hunter2", b"same-salt", 1000, 32);
        let b = pbkdf2_sha1(b"hunter2", b"same-salt", 2000, 32);
        assert_ne!(a, b);
    }

    #[test]
    fn ct_eq_agrees_with_ordinary_equality() {
        assert!(ct_eq(b"", b""));
        assert!(ct_eq(&[1, 2, 3], &[1, 2, 3]));
        assert!(!ct_eq(&[1, 2, 3], &[1, 2, 4]));
        // Differing in the FIRST byte is the case an early-exit compare gets wrong.
        assert!(!ct_eq(&[9, 2, 3], &[1, 2, 3]));
        assert!(!ct_eq(&[1, 2, 3], &[1, 2, 3, 4]));
        assert!(!ct_eq(&[1, 2, 3, 4], &[1, 2, 3]));
    }
}
