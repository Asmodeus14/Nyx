//! Emits `NYX_BUILD_UNIX`: the Unix time at which this terminal was compiled.
//!
//! ★ This is a security control, not a version string. `time sync` sets the clock — persistently,
//! via the RTC — from a `Date:` header fetched over PLAIN HTTP, which is unauthenticated by
//! construction (you cannot fetch the time over https when a wrong clock is what breaks https).
//! `SystemTime::now()` is the only input to certificate expiry checking, so an on-path attacker who
//! could answer that request could otherwise roll the clock BACK and revive expired — and, since
//! there is no CRL or OCSP here, revoked — certificates.
//!
//! A floor closes that whole class for one comparison: this build did not exist before it was
//! compiled, so any "now" earlier than the moment of compilation is a lie and is refused. Baking
//! the real build time in makes the floor advance with every image, which a hardcoded date does not.
//!
//! Mirrors `nyx-kernel/build.rs`, including its reason for the non-existent `rerun-if-changed`
//! path: a stamp refreshed only when some other file changes is a stamp that lies.

fn main() {
    println!("cargo:rerun-if-changed=.nyx-always-rebuild");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=NYX_BUILD_UNIX={}", stamp);
}
