//! Nyx pluggable toolchain registry.
//!
//! A single place the terminal (or any driver) hands a source file to and gets back a result,
//! without knowing which language it is. Each language is a [`Toolchain`] implementation; the
//! [`Registry`] dispatches to one by file extension. Adding a compiler/interpreter is a one-line
//! change in [`Registry::with_defaults`] — the whole point of the design.
//!
//! The registry is deliberately shape-agnostic. Backends fall into three kinds, all expressed
//! through one [`Artifact`] enum so the driver handles them uniformly:
//!   * **Transpiler / codegen-to-text** — source in, text out (e.g. qclang `.ql` → OpenQASM).
//!     Returns [`Artifact::Text`].
//!   * **Interpreter** — runs the program in-process and returns whatever it printed.
//!     Returns [`Artifact::Ran`].
//!   * **Native compiler** — emits a runnable Nyx-ABI ELF the driver can `execve`.
//!     Returns [`Artifact::Executable`]. (No backend does this yet; the variant exists so the
//!     trait doesn't need to change when one lands.)
//!
//! Portable: plain `std`, no OS coupling — host-testable today, and links into a `target_os="nyx"`
//! terminal unchanged once that exists.

mod qclang;

pub use qclang::QclangToolchain;

/// A compiler/interpreter backend for one source language.
pub trait Toolchain {
    /// Human-facing name, e.g. `"qclang"`. Shown by the terminal's `toolchains` command.
    fn name(&self) -> &str;

    /// One-line description of what this backend does (language → output).
    fn describe(&self) -> &str;

    /// File extensions this backend claims, WITHOUT the dot, lowercase — e.g. `["ql"]`.
    /// The registry routes a file to the first backend whose list contains its extension.
    fn extensions(&self) -> &[&str];

    /// Compile/run `source` (the full file contents). `filename` is passed through for diagnostics
    /// only. On success returns the produced [`Artifact`]; on failure, the list of diagnostics.
    fn run(&self, filename: &str, source: &str) -> Result<Artifact, Vec<Diagnostic>>;
}

/// What a backend produced. The driver matches on this to decide what to do next (print text,
/// show captured output, or execve a binary).
#[derive(Debug, Clone)]
pub enum Artifact {
    /// Text output — the transpiled/compiled source as a string (e.g. OpenQASM). `label` names the
    /// format for display ("OpenQASM"); `suggested_ext` is the extension to save it under ("qasm").
    Text {
        label: String,
        suggested_ext: String,
        content: String,
    },
    /// The backend interpreted the program and captured whatever it wrote to stdout.
    Ran { output: String },
    /// The backend emitted a runnable Nyx-ABI ELF. `bytes` is the image the driver can write to the
    /// VFS and `execve`. (Reserved for a future native-codegen backend.)
    Executable { bytes: Vec<u8> },
}

/// A single compile-time message. Severity is advisory — a backend returns `Err(diags)` to signal
/// failure regardless; warnings on a successful build are surfaced via [`Artifact`] channels later.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self { severity: Severity::Error, message: message.into() }
    }
    pub fn warning(message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, message: message.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// The set of installed backends, dispatched by file extension.
pub struct Registry {
    backends: Vec<Box<dyn Toolchain>>,
}

impl Registry {
    /// An empty registry. Use [`Registry::register`] to add backends, or [`Registry::with_defaults`]
    /// for the standard set.
    pub fn new() -> Self {
        Self { backends: Vec::new() }
    }

    /// The registry the terminal ships with. THIS is where a new language plugs in — add one line.
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register(Box::new(QclangToolchain::new()));
        // Future: r.register(Box::new(BasicToolchain::new()));  // .bas interpreter
        // Future: r.register(Box::new(CcToolchain::new()));     // .c -> Nyx ELF
        r
    }

    /// Install a backend. Later registrations lose extension conflicts to earlier ones (first wins),
    /// so `with_defaults` order is the priority order.
    pub fn register(&mut self, backend: Box<dyn Toolchain>) {
        self.backends.push(backend);
    }

    /// All installed backends, in registration order (for a `toolchains` listing).
    pub fn backends(&self) -> &[Box<dyn Toolchain>] {
        &self.backends
    }

    /// The backend that claims `ext` (without dot; case-insensitive), or `None`.
    pub fn for_extension(&self, ext: &str) -> Option<&dyn Toolchain> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        self.backends
            .iter()
            .find(|b| b.extensions().iter().any(|e| *e == ext))
            .map(|b| b.as_ref())
    }

    /// The backend for a filename, keyed off its extension. `None` if no extension or no match.
    pub fn for_filename(&self, filename: &str) -> Option<&dyn Toolchain> {
        let ext = extension_of(filename)?;
        self.for_extension(ext)
    }

    /// Convenience: route `filename`/`source` to the matching backend and run it. Returns an
    /// [`Err`] with a synthetic diagnostic when no backend claims the extension.
    pub fn compile(&self, filename: &str, source: &str) -> Result<Artifact, Vec<Diagnostic>> {
        match self.for_filename(filename) {
            Some(tc) => tc.run(filename, source),
            None => Err(vec![Diagnostic::error(format!(
                "no toolchain for '{}' (known: {})",
                filename,
                self.known_extensions().join(", ")
            ))]),
        }
    }

    /// Every extension any backend claims, for help text / error messages.
    pub fn known_extensions(&self) -> Vec<String> {
        let mut v = Vec::new();
        for b in &self.backends {
            for e in b.extensions() {
                let dotted = format!(".{e}");
                if !v.contains(&dotted) {
                    v.push(dotted);
                }
            }
        }
        v
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// The lowercase extension (without dot) of a path, or `None`. Handles both `/` and `\` separators
/// so it's correct for Nyx VFS paths regardless of style.
pub fn extension_of(filename: &str) -> Option<&str> {
    let base = filename.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(filename);
    let dot = base.rfind('.')?;
    // A leading-dot file ("...") with no other dot has no extension.
    if dot == 0 {
        return None;
    }
    let ext = &base[dot + 1..];
    if ext.is_empty() {
        None
    } else {
        Some(ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_parsing() {
        assert_eq!(extension_of("sample.ql"), Some("ql"));
        assert_eq!(extension_of("/mnt/nvme/a/b.QASM"), Some("QASM"));
        assert_eq!(extension_of("no_ext"), None);
        assert_eq!(extension_of(".hidden"), None);
        assert_eq!(extension_of("a.b.c"), Some("c"));
        assert_eq!(extension_of("dir.d/file"), None);
    }

    #[test]
    fn dispatch_by_extension() {
        let reg = Registry::with_defaults();
        assert!(reg.for_extension("ql").is_some());
        assert!(reg.for_extension(".QL").is_some()); // case + leading dot tolerated
        assert!(reg.for_extension("rs").is_none());
    }

    #[test]
    fn qclang_bell_pair_roundtrips() {
        let reg = Registry::with_defaults();
        let src = "fn main() -> int {\n\
                   qubit a = |0>;\n\
                   qubit b = |0>;\n\
                   qubit c = CNOT(H(a), b);\n\
                   cbit r1 = measure(a);\n\
                   cbit r2 = measure(b);\n\
                   return 0;\n\
                   }\n";
        match reg.compile("bell.ql", src) {
            Ok(Artifact::Text { content, label, suggested_ext }) => {
                assert_eq!(label, "OpenQASM");
                assert_eq!(suggested_ext, "qasm");
                assert!(content.contains("h q[0];"), "missing H gate:\n{content}");
                assert!(content.contains("cx q[0], q[1];"), "missing CNOT:\n{content}");
            }
            Ok(other) => panic!("unexpected artifact: {other:?}"),
            Err(e) => panic!("compile failed: {e:?}"),
        }
    }

    #[test]
    fn unknown_extension_is_a_clean_error() {
        let reg = Registry::with_defaults();
        let err = reg.compile("main.rs", "fn main() {}").unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].message.contains("no toolchain"));
    }
}
