//! qclang backend: `.ql` (QCLang) → OpenQASM 2.0, via the portable `qclang_compiler` core.
//!
//! This is backend #1 and the reference pattern for every future one: a small adapter that maps
//! the language's own compile entry point onto the [`Toolchain`] trait. It links only the portable
//! core (`default-features = false` → logos + thiserror), so it carries no host CLI/simulator deps
//! and builds for `target_os = "nyx"` unchanged.

use crate::{Artifact, Diagnostic, Toolchain};
use qclang_compiler::Compiler;

pub struct QclangToolchain;

impl QclangToolchain {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QclangToolchain {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolchain for QclangToolchain {
    fn name(&self) -> &str {
        "qclang"
    }

    fn describe(&self) -> &str {
        "QCLang (.ql) quantum source -> OpenQASM 2.0"
    }

    fn extensions(&self) -> &[&str] {
        &["ql"]
    }

    fn run(&self, _filename: &str, source: &str) -> Result<Artifact, Vec<Diagnostic>> {
        // The portable core returns Ok(qasm) or Err(Vec<String>) of formatted diagnostics; the
        // compile path itself never touches stderr (feature-gated), so this is fully in-process.
        match Compiler::compile(source) {
            Ok(qasm) => Ok(Artifact::Text {
                label: "OpenQASM".to_string(),
                suggested_ext: "qasm".to_string(),
                content: qasm,
            }),
            Err(errors) => Err(errors.into_iter().map(Diagnostic::error).collect()),
        }
    }
}
