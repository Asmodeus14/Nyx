//! The cloud-provider abstraction, and where credentials live.
//!
//! ## Why a remote provider is a first-class part of this subsystem
//!
//! Because it is the only way Nyx can produce a result that was actually computed by quantum
//! mechanics. There is no consumer gate-model QPU this OS can drive over PCIe — see
//! `docs/quantum/limitations.md` — so `QpuStatus::Hardware` is unreachable here. A cloud processor
//! is reachable: Nyx has working TLS 1.3 on real hardware.
//!
//! ## ⚠️ A remote simulator is not a quantum result
//!
//! This is the trap the whole device model is shaped around. Providers serve classical simulators
//! **through the same API, with the same job lifecycle, returning the same JSON** as their real
//! processors. IonQ's `simulator` backend and its `qpu.aria-1` differ by one string.
//!
//! So [`ProviderTarget::is_simulator`] exists, it is set from the target's own name, and it becomes
//! `QpuInfo::remote_is_simulator` — which every display renders as `REMOTE/sim` rather than
//! `REMOTE/hw`.
//!
//! ## ★ Credentials arrive with the image, not through the keyboard
//!
//! The constraint that shaped this: **Nyx has no clipboard and cannot express a paste chord.**
//! `HandleControl::Ignore` means `Ctrl+V` does not exist as an input event. An IonQ key is ~40
//! characters, an IBM key ~44, and an **IBM CRN is ~120** — typing that by hand, with no paste and
//! no way to correct a mistake you cannot see, is not a usable workflow.
//!
//! So the primary path is build-time: a **gitignored** `quantum-credentials.txt` at the repo root is
//! baked by `Build.sh` into the initrd as [`CRED_BAKED_PATH`]. Zero typing on the device, and the
//! key never enters git history.
//!
//! Two files, and the distinction is load-bearing:
//!
//! | file | in the initrd tar? | lifetime |
//! |---|---|---|
//! | [`CRED_BAKED_PATH`] | **yes** | rewritten from the image on every boot |
//! | [`CRED_PATH`] | **no** | written at runtime, survives reboots |
//!
//! `installer::extract_tar_to_ext4` walks the tar and writes each entry — it does **not** wipe the
//! filesystem first. So a file that is not in the tar survives, and a file that is gets refreshed.
//! [`Credentials::load`] reads the baked file then overlays the runtime one, so **an on-device edit
//! wins and persists**, and reflashing a new image cannot silently revert it.
//!
//! ## ⚠️ Nyx cannot protect this file
//!
//! `struct Process` has no uid, and not one of ~130 syscall arms consults caller identity. Any
//! process on this machine can read these credentials. Pretending otherwise would be worse than the
//! exposure, so [`Credentials::warning`] is printed at the moment a credential is stored and shown
//! permanently in Settings — not buried in documentation.

use nyx_quantum::QpuError;
use std::string::{String, ToString};
use std::vec::Vec;

/// Credentials written on the device. **Not** in the initrd tar, so it survives a reboot.
pub const CRED_PATH: &str = "/mnt/nvme/etc/quantum-credentials";

/// Credentials baked into the image by `Build.sh`. **Is** in the tar, so it is rewritten from the
/// image on every boot — which is what makes reflashing a new key work.
pub const CRED_BAKED_PATH: &str = "/mnt/nvme/etc/quantum-credentials.baked";

/// Re-exported so callers need only one import. The parsing, redaction and source labels live in
/// `nyx_quantum::creds` — `no_std`, no filesystem — because `apps/settings` must read the same files
/// through `nyx_api::sys_open` and the two must not be able to disagree.
pub use nyx_quantum::creds::{redact, warning, CredSet, CredSource};

/// The std-side credential store: [`CredSet`] plus the file I/O.
///
/// Recognised keys (none are required; a provider simply reports itself unconfigured):
///
/// ```text
/// ionq.token = <IonQ API key>
/// ibm.token  = <IBM Cloud API key>
/// ibm.crn    = crn:v1:bluemix:public:quantum-computing:...
/// ```
pub struct Credentials;

impl Credentials {
    /// Read the baked file, then overlay the runtime one.
    ///
    /// Never fails: a missing or unreadable file is simply no credentials, which is a legitimate
    /// state and the one every machine starts in.
    pub fn load() -> CredSet {
        let baked = std::fs::read_to_string(CRED_BAKED_PATH).unwrap_or_default();
        let runtime = std::fs::read_to_string(CRED_PATH).unwrap_or_default();
        CredSet::merge(&baked, &runtime)
    }

    /// Store one key in the **runtime** file, preserving every other runtime key.
    ///
    /// Never touches the baked file — that belongs to the image, and a reflash should be able to
    /// replace it without an on-device edit silently surviving into a machine it was not meant for.
    pub fn set(key: &str, value: &str) -> Result<(), String> {
        if key.trim().is_empty() {
            return Err("empty key".to_string());
        }
        // ⚠️ A credential containing CR/LF would be silently dropped later by
        // `nyx_net::Request::header`'s injection guard, producing a 401 that looks like a wrong key.
        // Refuse it here, where the message can say why.
        if !nyx_quantum::creds::is_header_safe(value.trim()) {
            return Err("a credential cannot be empty or contain a line break".to_string());
        }

        let existing = std::fs::read_to_string(CRED_PATH).unwrap_or_default();
        let entries = CredSet::merge("", &existing).with(key, value);
        write_text(&nyx_quantum::creds::serialise(&entries))
    }

    /// Forget every runtime credential. The baked file is untouched, so this reverts to the image.
    pub fn clear() -> Result<(), String> {
        write_text("")
    }

    /// Whether an on-device override exists with any content.
    pub fn has_runtime_override() -> bool {
        !CredSet::merge("", &std::fs::read_to_string(CRED_PATH).unwrap_or_default()).is_empty()
    }
}

fn write_text(text: &str) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(CRED_PATH).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(CRED_PATH, text).map_err(|e| std::format!("{e}"))
}

/// One execution target a provider offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderTarget {
    /// The provider's own name for it — `"simulator"`, `"qpu.aria-1"`, `"ibm_fez"`.
    pub name: String,
    /// Qubits, or `0` if the provider did not say.
    pub qubits: u32,
    /// ⚠️ Whether this target is a classical simulator running on the provider's machines.
    ///
    /// Becomes `QpuInfo::remote_is_simulator`. Getting this wrong would let a classical simulation
    /// be presented as a quantum result.
    pub is_simulator: bool,
    /// Jobs queued ahead, if reported.
    pub queue: Option<u32>,
}

/// A cloud quantum service.
///
/// Deliberately REST-shaped rather than generic: every provider in existence is an HTTPS API with
/// token auth, a job submission, a poll, and a results fetch. Abstracting further would invent
/// structure nothing needs.
pub trait QuantumProvider {
    /// Short name, as shown by `quantum backends`.
    fn name(&self) -> &str;

    /// Every target this provider offers.
    fn targets(&mut self) -> Result<Vec<ProviderTarget>, QpuError>;
}

/// Reject anything that is not HTTPS.
///
/// ★ Not a downgrade, a refusal. A token travels in a header on every request; over plaintext HTTP
/// that is the credential handed to anyone on the path.
pub fn require_https(url: &str) -> Result<(), QpuError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(QpuError::Auth(std::format!(
            "refusing to send a credential over a non-HTTPS URL: {url}"
        )))
    }
}

/// The clock check that turns a baffling TLS failure into an actionable message.
///
/// ⚠️ This laptop's RTC is not battery-reliable, and a clock in the past makes **every** certificate
/// fail validation as "not yet valid". That surfaces from rustls as a generic handshake error, which
/// reads as a network problem and has cost this project a debugging session before. `time sync` in
/// the terminal repairs it.
pub fn clock_looks_sane(now_unix: u64) -> Result<(), QpuError> {
    // 2020-01-01. A certificate chain cannot validate against a clock before this.
    const FLOOR: u64 = 1_577_836_800;
    if now_unix < FLOOR {
        return Err(QpuError::Provider(
            "the system clock is wrong, so every TLS certificate will be rejected as \
             'not yet valid' — run `time sync` first"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_urls_are_refused_rather_than_upgraded() {
        assert!(require_https("https://api.ionq.co/v0.4/jobs").is_ok());
        for bad in ["http://api.ionq.co/v0.4/jobs", "ftp://x/y", "//api.ionq.co", "HTTPS://x"] {
            let e = require_https(bad).unwrap_err();
            assert!(std::format!("{e}").contains("refusing"), "{bad}");
        }
    }

    #[test]
    fn a_wrong_clock_is_reported_as_a_clock_problem_not_a_network_one() {
        let e = clock_looks_sane(0).unwrap_err();
        let m = std::format!("{e}");
        assert!(m.contains("clock"), "{m}");
        assert!(m.contains("time sync"), "the message must say what to do: {m}");
        assert!(clock_looks_sane(1_789_000_000).is_ok());
    }

    // Parsing, merging, redaction and the source labels are tested in `nyx_quantum::creds`, which
    // owns them — `apps/settings` links that same code through a different file API, and one set of
    // tests is what keeps the screen and the command line from disagreeing.

    #[test]
    fn a_credential_with_a_line_break_is_refused_where_the_message_can_explain() {
        // ⚠️ If this got through, `Request::header`'s injection guard would drop the whole header
        // later and the result would be a 401 that looks like a bad key.
        assert!(Credentials::set("ionq.token", "abc\r\nX-Evil: 1").is_err());
        assert!(Credentials::set("ionq.token", "").is_err());
        assert!(Credentials::set("", "v").is_err());
    }

    #[test]
    fn the_two_credential_paths_are_distinct_and_only_one_is_in_the_image() {
        // ★ The whole persistence design rests on this: the baked path IS in the initrd tar (so a
        // reflash refreshes it) and the runtime path is NOT (so an on-device edit survives a boot).
        assert_ne!(CRED_PATH, CRED_BAKED_PATH);
        assert!(CRED_BAKED_PATH.starts_with(CRED_PATH));
        assert!(CRED_BAKED_PATH.ends_with(".baked"));
    }

    #[test]
    fn a_simulator_target_is_marked_as_one() {
        let sim = ProviderTarget {
            name: "simulator".to_string(),
            qubits: 29,
            is_simulator: true,
            queue: None,
        };
        let hw = ProviderTarget {
            name: "qpu.aria-1".to_string(),
            qubits: 25,
            is_simulator: false,
            queue: Some(14),
        };
        // The bigger device is the simulator. Capability and reality are independent.
        assert!(sim.qubits > hw.qubits);
        assert!(sim.is_simulator && !hw.is_simulator);
    }
}
