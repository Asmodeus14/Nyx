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
// `pub mod ui;` lived here until Meridian step 20 (2026-09-08). It was the old desktop's widget set
// — taskbar, window chrome, control-button plates, scrollbar, software cursor — and its last
// consumer was `apps/compositor`, retired at that step alongside `apps/wifi`. Every ported
// application had already stopped calling it. Meridian draws its own from `libs/meridian`: `shapes`
// for the rounded rects and shadows, `layout` for every rect that used to be a hand-copied literal
// in that file, `cursor` for the twelve-shape set.
//
// Deleted rather than left in place: this crate compiles with warnings allowed, so 723 lines of
// unreachable code would never have announced themselves, and a second source of truth for window
// geometry is the exact drift `libs/meridian/src/layout.rs` was created to end.
pub mod canvas;
pub mod effects;
pub mod app;
pub mod gpu_text;