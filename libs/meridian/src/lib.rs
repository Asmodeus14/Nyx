//! # Meridian — the Nyx interface language
//!
//! Not a restyled desktop. The conventional shell is replaced outright: no panel, no menu bar, no
//! system tray, no taskbar. Permanent chrome is two things — a containerless row of glyphs at
//! bottom-left, and one 22px mark at bottom-right that is simultaneously the system's status
//! surface and its intelligence. Windows have no chrome; a window's name lives on a caption line
//! *outside* the surface, and minimise is replaced by FOLD, which collapses a window to that
//! caption line and leaves it where it was. Applications are reached by typing.
//!
//! The full specification is `Nyx-ui/design/ver3.0/`. This crate is its foundation layer: the
//! tokens, the glyph/icon atlas and the shapes. `apps/shell` is the shell that draws with them.
//!
//! ## Why this is a separate crate
//!
//! Nothing in here touches hardware, so all of it can be exercised with `cargo test` on the
//! development host. That matters more than usual: this project has no QEMU, so every on-hardware
//! test costs a power cycle. Atlas packing, glyph metrics, hit-test geometry and icon coverage are
//! all things that can be wrong in ways a boot would not immediately reveal — so they are proved
//! here instead, and the boot is spent on the things only hardware can answer.
//!
//! ## The rendering contract
//!
//! Every rule in the design maps to a primitive Nyx already has: a GPU quad, a `fill_rect`, a
//! coverage glyph, an ortho MVP. The design deliberately avoids backdrop blur, per-element
//! gradients, coloured text, layered shadows and animated layout properties — not for taste, but
//! because the OS cannot do them. Those omissions are documented at their call sites rather than
//! quietly worked around.

#![no_std]
// Same rustc workaround as `nyx-gui`: on the pinned nightly the `annotate_snippets` diagnostic
// renderer ICEs while drawing a lint on some items in this dependency graph, aborting an otherwise
// valid build. Lints are diagnostics only, so this changes no runtime behaviour.
#![allow(warnings)]

extern crate alloc;

// The crate is `no_std` for the target, but the host test runner needs `std` for the harness and
// for the diagnostics some tests print. Test-only, so it never reaches a Nyx binary.
#[cfg(test)]
extern crate std;

pub mod atlas;
pub mod brand;
pub mod charset;
pub mod cursor;
pub mod font;
pub mod icons;
pub mod layout;
pub mod motion;
pub mod paper;
pub mod scale;
pub mod shapes;
pub mod tokens;

/// The Meridian icon set, rasterized from the design document at build time.
///
/// Regenerate after any change to the icons in `Nyx-ui`:
///
/// ```text
/// cargo run --release -p nyx-icons-gen -- ../Nyx-ui
/// ```
///
/// To look at one without booting — the check that actually catches a mis-parsed curve, since a
/// wrong icon is still a valid bitmap:
///
/// ```text
/// cargo run --release -p nyx-icons-gen -- ../Nyx-ui --preview a-browser
/// ```
#[rustfmt::skip]
pub mod icons_gen;

/// The twelve Meridian cursors, composited from the design document at build time.
///
/// Written by the same command as [`icons_gen`], from `10-cursors.html`. Unlike the icons these are
/// finished ARGB rather than coverage: the cursor plane never meets the luminance-only text shader,
/// so it is the one surface in the system where colour is free — and two of the shapes use it.
///
/// To look at the whole set without booting:
///
/// ```text
/// cargo run --release -p nyx-icons-gen -- ../Nyx-ui --preview-cursors
/// ```
#[rustfmt::skip]
pub mod cursors_gen;

/// The identity, byte-identical to the kernel's copy in `nyx-kernel/src/brand_gen.rs`.
///
/// Both are written by the same command from the same source, because cold start hands the mark
/// across the ring boundary: the kernel draws it at stage 1, and the shell picks it up at stage 3
/// and carries it to the corner. Two copies of one generated table beats inventing a way to share
/// static data between ring 0 and ring 3 for one animation.
#[rustfmt::skip]
pub mod brand_gen;

pub use atlas::Atlas;
pub use icons::Icons;
pub use tokens::{Face, Style, Theme};
