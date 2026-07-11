#![no_std]
// Silence all lints in this crate. This is a WORKAROUND for a rustc ICE: on some nightlies
// the `annotate_snippets` diagnostic renderer panics ("slice index starts at N but ends at M")
// while drawing a lint warning emitted on an item in this crate, aborting compilation even
// though the code is valid. Suppressing the lints stops rustc from entering the buggy renderer.
// Lints are diagnostics only, so this changes no runtime behavior. Safe to remove once the
// toolchain bug is fixed upstream. See rust-toolchain.toml for the pinned nightly.
#![allow(warnings)]
extern crate alloc;

pub mod font;
pub mod draw;
pub mod ui;
pub mod canvas;
pub mod effects;
pub mod app;
pub mod gpu_text;