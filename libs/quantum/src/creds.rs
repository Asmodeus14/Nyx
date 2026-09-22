//! Credential parsing — pure string work, no filesystem.
//!
//! ## Why this is here and not in `libs/quantum-rt`
//!
//! Two consumers need to read the same credential file and they cannot share a filesystem API:
//!
//! * `libs/quantum-rt` is `std` and uses `std::fs`.
//! * `apps/settings` is `no_std` and uses `nyx_api::sys_open`/`sys_read`.
//!
//! Duplicating the parser would mean Settings could disagree with the runtime about whether a
//! credential is configured — the kind of split-brain that makes a screen and a command line tell a
//! user two different things. So the parsing, the redaction and the source labels live here, in the
//! `no_std` crate both can link, and each side supplies only its own file read.
//!
//! ## Format
//!
//! `key = value`, one per line. `#` starts a comment, blank lines are ignored, keys are lowercased.
//!
//! ⚠️ **Only the first `=` splits.** An IBM instance CRN is
//! `crn:v1:bluemix:public:quantum-computing:us-east:a/0123::` — colons, slashes, and sometimes an
//! `=` in the trailing segment. Splitting on every `=` would truncate it into something that
//! authenticates against nothing and fails in a way that looks like a bad key.
//!
//! Deliberately not JSON: this file is hand-edited in a text editor on the host, and a missing comma
//! should not cost a boot test.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Where the credentials in use came from.
///
/// Two files, and the distinction matters to a user trying to work out why a key they just changed
/// did not take effect:
///
/// | file | in the initrd tar? | lifetime |
/// |---|---|---|
/// | `…/quantum-credentials.baked` | **yes** | rewritten from the image every boot |
/// | `…/quantum-credentials` | **no** | written on-device, survives reboots |
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CredSource {
    /// Nothing stored anywhere.
    #[default]
    None,
    /// From the image, via `Build.sh`.
    Baked,
    /// Entered on the device.
    Runtime,
    /// Both exist, and the on-device file changes at least one baked value.
    RuntimeOverBaked,
}

impl CredSource {
    /// The label shown in Settings and the terminal. All four are distinct strings — a user has to
    /// be able to tell which file is actually in play.
    pub fn label(self) -> &'static str {
        match self {
            CredSource::None => "none",
            CredSource::Baked => "baked into image",
            CredSource::Runtime => "entered on device",
            CredSource::RuntimeOverBaked => "on device (overrides image)",
        }
    }
}

/// A parsed credential set.
#[derive(Clone, Debug, Default)]
pub struct CredSet {
    entries: Vec<(String, String)>,
    source: CredSource,
}

impl CredSet {
    /// Parse the baked file, then overlay the runtime one.
    ///
    /// Either may be empty or absent — that is a legitimate state and the one every machine starts
    /// in, so this never fails.
    pub fn merge(baked_text: &str, runtime_text: &str) -> CredSet {
        let mut entries = parse(baked_text);
        let have_baked = !entries.is_empty();

        let over = parse(runtime_text);
        let have_runtime = !over.is_empty();
        let mut overrode = false;

        for (k, v) in over {
            match entries.iter_mut().find(|(ek, _)| *ek == k) {
                Some(slot) => {
                    if slot.1 != v {
                        overrode = true;
                    }
                    slot.1 = v;
                }
                None => {
                    overrode = have_baked;
                    entries.push((k, v));
                }
            }
        }

        let source = match (have_baked, have_runtime) {
            (false, false) => CredSource::None,
            (true, false) => CredSource::Baked,
            (false, true) => CredSource::Runtime,
            (true, true) => {
                if overrode {
                    CredSource::RuntimeOverBaked
                } else {
                    CredSource::Baked
                }
            }
        };

        CredSet { entries, source }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn source(&self) -> CredSource {
        self.source
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    /// Set or replace one key, returning the full set re-serialised for writing.
    pub fn with(&self, key: &str, value: &str) -> Vec<(String, String)> {
        let key = key.trim().to_ascii_lowercase();
        let mut out = self.entries.clone();
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value.trim().to_string(),
            None => out.push((key, value.trim().to_string())),
        }
        out
    }

    /// Providers with enough configured to attempt a connection.
    ///
    /// ⚠️ IBM needs **both** a token and a CRN. A token alone authenticates nothing, and reporting
    /// IBM as "configured" on a token alone would turn a missing-CRN error into something that reads
    /// like a rejected key.
    pub fn configured_providers(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.get("ionq.token").is_some() {
            out.push("ionq");
        }
        if self.get("ibm.token").is_some() && self.get("ibm.crn").is_some() {
            out.push("ibm");
        }
        out
    }
}

/// Parse `key = value` lines.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // ⚠️ `split_once`, not `split` — see the module docs on CRNs.
        let Some((k, v)) = split_once_eq(line) else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        if k.is_empty() || v.is_empty() {
            continue;
        }
        out.push((k, v.to_string()));
    }
    out
}

fn split_once_eq(s: &str) -> Option<(&str, &str)> {
    let i = s.find('=')?;
    Some((&s[..i], &s[i + 1..]))
}

/// Serialise entries back to the file format.
pub fn serialise(entries: &[(String, String)]) -> String {
    let mut text = String::from("# Nyx quantum credentials, written on the device.\n");
    text.push_str("# Overrides the baked-in copy from the image. Readable by any process.\n");
    for (k, v) in entries {
        text.push_str(k);
        text.push_str(" = ");
        text.push_str(v);
        text.push('\n');
    }
    text
}

/// Redact a credential for display. **Never show one in full.**
///
/// Shows at most the last four characters — enough to tell two keys apart, not enough to use one.
/// The star run is capped so a 120-character CRN does not produce a 120-character line.
pub fn redact(value: &str) -> String {
    let n = value.chars().count();
    if n <= 4 {
        return "****".to_string();
    }
    let tail: String = value.chars().skip(n - 4).collect();
    let stars = n.min(12) - 4;
    let mut out = String::with_capacity(stars + 4);
    for _ in 0..stars {
        out.push('*');
    }
    out.push_str(&tail);
    out
}

/// Whether a value is safe to put in an HTTP header.
///
/// ⚠️ A credential containing CR or LF would be silently dropped by
/// `nyx_net::Request::header`'s injection guard, producing a 401 that looks like a wrong key.
/// Callers refuse at storage time, where the message can explain.
pub fn is_header_safe(value: &str) -> bool {
    !value.is_empty() && !value.contains('\r') && !value.contains('\n')
}

/// What the user must be told wherever a credential is stored or displayed.
///
/// ★ Not optional and not deferred to a document. Nyx has no permission model — `struct Process`
/// has no uid and no syscall arm consults caller identity — so this file is readable by every
/// process on the machine. A user who does not know that cannot choose which token to use.
pub fn warning() -> &'static str {
    "Nyx has no permission model: any process can read this. Use a revocable token."
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_format_is_forgiving_of_hand_editing() {
        let e = parse(
            "\
# a comment
ionq.token = abc123

  IBM.Token   =   def456
malformed line with no equals
empty.value =
",
        );
        let get = |k: &str| e.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.as_str());
        assert_eq!(get("ionq.token"), Some("abc123"));
        // Keys are case-insensitive; values are not.
        assert_eq!(get("ibm.token"), Some("def456"));
        assert_eq!(e.len(), 2, "junk lines must be skipped, not fatal");
    }

    /// ★ The bug this guards: an IBM CRN is full of colons and slashes and can end in `::`.
    /// Splitting on every `=` would truncate it into something that authenticates against nothing.
    #[test]
    fn a_crn_survives_intact() {
        let crn = "crn:v1:bluemix:public:quantum-computing:us-east:a/0123456789abcdef::";
        let e = parse(&alloc::format!("ibm.crn = {crn}"));
        assert_eq!(e[0].1, crn);

        // And a value containing its own '=' is preserved whole.
        assert_eq!(parse("k = a=b=c")[0].1, "a=b=c");
    }

    #[test]
    fn merging_lets_the_device_override_the_image() {
        let baked = "ionq.token = from-image\nibm.token = image-ibm";
        let runtime = "ionq.token = from-device";
        let c = CredSet::merge(baked, runtime);

        assert_eq!(c.get("ionq.token"), Some("from-device"), "runtime must win");
        assert_eq!(c.get("ibm.token"), Some("image-ibm"), "unoverridden baked keys survive");
        assert_eq!(c.source(), CredSource::RuntimeOverBaked);
    }

    #[test]
    fn an_identical_runtime_file_is_not_reported_as_an_override() {
        // Otherwise Settings would say "overrides image" for a file that changes nothing.
        let c = CredSet::merge("ionq.token = same", "ionq.token = same");
        assert_eq!(c.source(), CredSource::Baked);
    }

    #[test]
    fn source_reflects_which_files_exist() {
        assert_eq!(CredSet::merge("", "").source(), CredSource::None);
        assert_eq!(CredSet::merge("a = 1", "").source(), CredSource::Baked);
        assert_eq!(CredSet::merge("", "a = 1").source(), CredSource::Runtime);
        // A runtime key the image never had is still an override.
        assert_eq!(CredSet::merge("a = 1", "b = 2").source(), CredSource::RuntimeOverBaked);
    }

    #[test]
    fn source_labels_are_all_distinct() {
        let l = [
            CredSource::None.label(),
            CredSource::Baked.label(),
            CredSource::Runtime.label(),
            CredSource::RuntimeOverBaked.label(),
        ];
        for i in 0..l.len() {
            for j in (i + 1)..l.len() {
                assert_ne!(l[i], l[j]);
            }
        }
    }

    /// ⚠️ IBM needs both a token AND a ~120-char CRN. Reporting it configured on the token alone
    /// turns a missing-CRN failure into something that reads like a rejected key.
    #[test]
    fn ibm_is_only_configured_with_both_a_token_and_a_crn() {
        assert!(CredSet::merge("ibm.token = t", "").configured_providers().is_empty());
        assert!(CredSet::merge("ibm.crn = c", "").configured_providers().is_empty());
        assert_eq!(
            CredSet::merge("ibm.token = t\nibm.crn = c", "").configured_providers(),
            vec!["ibm"]
        );
        assert_eq!(CredSet::merge("ionq.token = t", "").configured_providers(), vec!["ionq"]);
    }

    #[test]
    fn redaction_never_reveals_more_than_the_last_four() {
        let r = redact("abcdefghijklmnop");
        assert!(r.ends_with("mnop"));
        assert!(!r.contains("abcdefghijkl"));
        assert_eq!(redact("abc"), "****");
        assert_eq!(redact(""), "****");
        // A 120-character CRN must not become a 120-character line of stars.
        assert!(redact(&"x".repeat(120)).chars().count() <= 12);
    }

    #[test]
    fn header_unsafe_values_are_caught() {
        assert!(is_header_safe("normal-key-123"));
        assert!(!is_header_safe("abc\r\nX-Evil: 1"));
        assert!(!is_header_safe("abc\ndef"));
        assert!(!is_header_safe(""));
    }

    #[test]
    fn serialise_round_trips_through_parse() {
        let c = CredSet::merge("", "ionq.token = abc\nibm.crn = crn:v1:a/b::");
        let text = serialise(c.entries());
        let back = CredSet::merge("", &text);
        assert_eq!(back.get("ionq.token"), Some("abc"));
        assert_eq!(back.get("ibm.crn"), Some("crn:v1:a/b::"));
    }

    #[test]
    fn with_replaces_without_disturbing_other_keys() {
        let c = CredSet::merge("", "ionq.token = old\nibm.token = keep");
        let updated = c.with("IONQ.TOKEN", " new ");
        let back = CredSet::merge("", &serialise(&updated));
        assert_eq!(back.get("ionq.token"), Some("new"), "trimmed and case-folded");
        assert_eq!(back.get("ibm.token"), Some("keep"));
    }
}
