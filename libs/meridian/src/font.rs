//! Meridian's typefaces.
//!
//! The design is typographically led — headings are *large and light* rather than big and bold, and
//! that only works with a face that has genuine 200/300 weights. DejaVu Sans, which `nyx-gui` has
//! embedded since F1, has exactly one weight; setting 40px display type in it produces something
//! heavier and wider than the design at every size above body text. So Meridian ships its own:
//! Inter at four weights, and JetBrains Mono for the terminal and code surfaces.
//!
//! ## Why the bytes live here and not in `nyx-gui`
//!
//! Eight applications link `nyx-gui`. Embedding two megabytes of Inter there would put it in every
//! one of them, for the benefit of the single binary that draws Meridian. So `nyx-gui::font` owns
//! the rasterizer and a small registry of face slots, and this module owns the bytes and fills the
//! slots. Unported apps keep drawing in DejaVu and their binaries do not grow by a byte.
//!
//! ## Licensing
//!
//! Both families are SIL Open Font License 1.1. The licenses ship alongside the fonts in
//! `libs/meridian/fonts/` as the OFL requires.

use nyx_gui::font;

use crate::tokens::Face;

/// Inter, weight 200. Display sizes only — `d1`, which is almost entirely digits.
static INTER_EXTRALIGHT: &[u8] = include_bytes!("../fonts/Inter-ExtraLight.ttf");
/// Inter, weight 300. Headings: `d2` and `d3`.
static INTER_LIGHT: &[u8] = include_bytes!("../fonts/Inter-Light.ttf");
/// Inter, weight 400. Body: `b1`, `b2`, `b3`.
static INTER_REGULAR: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");
/// Inter, weight 500. Labels and window captions.
static INTER_MEDIUM: &[u8] = include_bytes!("../fonts/Inter-Medium.ttf");
/// JetBrains Mono, weight 400. Terminal, code, and the Command's key hints.
static JETBRAINS_MONO: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");

/// Slot assignments in `nyx_gui::font`'s registry. Slot 0 stays DejaVu so anything that has not
/// been ported keeps working, and so there is always a fallback that is known to parse.
pub const SLOT_EXTRALIGHT: usize = 1;
pub const SLOT_LIGHT: usize = 2;
pub const SLOT_REGULAR: usize = 3;
pub const SLOT_MEDIUM: usize = 4;
pub const SLOT_MONO: usize = 5;

/// The registry slot a style's face draws from.
pub const fn slot_of(face: Face) -> usize {
    match face {
        Face::ExtraLight => SLOT_EXTRALIGHT,
        Face::Light => SLOT_LIGHT,
        Face::Regular => SLOT_REGULAR,
        Face::Medium => SLOT_MEDIUM,
        Face::Mono => SLOT_MONO,
    }
}

/// Install every Meridian typeface. Call once at shell startup, before building the atlas.
///
/// Returns the number of faces that failed to parse — **zero is the only good answer**, but a
/// non-zero result is deliberately not fatal. `with_glyph_face` falls back to slot 0, so a font
/// that fails to load costs the design's typography and nothing else: the shell still draws every
/// word, in DejaVu. On a machine where the alternative is a blank desktop and a power cycle to work
/// out why, degrading is worth more than failing.
///
/// The caller should log a non-zero return. Silently rendering in the wrong face is the kind of
/// thing that gets explained away as "the mockup just looked different".
pub fn register_all() -> usize {
    let faces = [
        (SLOT_EXTRALIGHT, INTER_EXTRALIGHT),
        (SLOT_LIGHT, INTER_LIGHT),
        (SLOT_REGULAR, INTER_REGULAR),
        (SLOT_MEDIUM, INTER_MEDIUM),
        (SLOT_MONO, JETBRAINS_MONO),
    ];
    let mut failed = 0;
    for (slot, bytes) in faces {
        if !font::register_face(slot, bytes) {
            failed += 1;
        }
    }
    failed
}

/// Install **only** JetBrains Mono.
///
/// For the terminal and other monospace surfaces, which need exactly one face. `register_all` would
/// pull four weights of Inter into a binary that never sets a heading in any of them — about two
/// megabytes of font in an app that draws nothing but fixed-pitch text. Same reasoning that keeps
/// the bytes out of `nyx-gui`: a face should cost only the binaries that actually render it.
///
/// Returns true if the face parsed and is ready to draw.
pub fn register_mono() -> bool {
    font::register_face(SLOT_MONO, JETBRAINS_MONO)
}

/// True once every Meridian face is installed.
pub fn all_ready() -> bool {
    [SLOT_EXTRALIGHT, SLOT_LIGHT, SLOT_REGULAR, SLOT_MEDIUM, SLOT_MONO]
        .iter()
        .all(|&s| font::face_ready(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{ALL_STYLES, B1, D1, LB, MONO};

    /// Every embedded file has to be a parseable TrueType face. A truncated or wrong-format font
    /// would otherwise surface as text silently rendering in DejaVu — which looks like a design
    /// disagreement, not a bug, and could survive a long time.
    #[test]
    fn every_face_parses() {
        assert_eq!(register_all(), 0, "at least one embedded typeface failed to parse");
        assert!(all_ready());
    }

    /// Slots must be distinct, or two weights collapse into one and the type scale quietly loses
    /// its contrast.
    #[test]
    fn slots_are_distinct_and_never_shadow_the_fallback() {
        let slots = [SLOT_EXTRALIGHT, SLOT_LIGHT, SLOT_REGULAR, SLOT_MEDIUM, SLOT_MONO];
        for (i, a) in slots.iter().enumerate() {
            assert_ne!(*a, font::FACE_DEFAULT, "slot {} would overwrite the DejaVu fallback", i);
            assert!(*a < font::MAX_FACES, "slot {} is out of range", a);
            for b in &slots[i + 1..] {
                assert_ne!(a, b, "two faces share slot {}", a);
            }
        }
    }

    #[test]
    fn every_style_maps_to_a_registered_slot() {
        register_all();
        for s in ALL_STYLES {
            assert!(font::face_ready(slot_of(s.face)), "{:?} has no registered face", s.face);
        }
    }

    /// The whole reason for shipping Inter: the weights must actually differ. If the build ever
    /// pointed two slots at the same file this would catch it, where a glance at the screen might
    /// not.
    #[test]
    fn weights_are_visibly_different() {
        register_all();
        let ink = |slot: usize, px: usize| -> u32 {
            font::with_glyph_face(slot, 'H', px, |g| g.cov.iter().map(|&c| c as u32).sum::<u32>())
                .unwrap_or(0)
        };
        let light = ink(SLOT_EXTRALIGHT, 40);
        let medium = ink(SLOT_MEDIUM, 40);
        assert!(light > 0 && medium > 0, "no ink rasterized at all");
        assert!(
            medium > light + light / 5,
            "weight 500 should carry noticeably more ink than 200 ({} vs {})",
            medium, light
        );
    }

    /// Mono has to be monospaced — the terminal's whole layout assumes a fixed cell, and a
    /// proportional face there would misalign every column while still looking like text.
    #[test]
    fn the_mono_face_is_monospaced() {
        register_all();
        let w = |c: char| font::advance_px_face(SLOT_MONO, c, MONO.px);
        let i = w('i');
        assert!(i > 0);
        for c in ['W', 'm', '.', '0', 'l'] {
            assert_eq!(w(c), i, "'{}' has a different advance from 'i' in the mono face", c);
        }
    }

    /// ...and Inter must NOT be, or the design's proportional body text would be set on a grid.
    #[test]
    fn the_text_face_is_proportional() {
        register_all();
        let w = |c: char| font::advance_px_face(SLOT_REGULAR, c, B1.px);
        assert!(w('W') > w('i'), "Inter should be proportional");
    }

    /// The two display shelves exist to be lighter and larger than body text. If the metrics come
    /// back identical, the style table is not reaching the faces it names.
    #[test]
    fn display_type_is_larger_than_body() {
        register_all();
        let d = font::advance_px_face(slot_of(D1.face), '8', D1.px);
        let b = font::advance_px_face(slot_of(B1.face), '8', B1.px);
        assert!(d > b * 2, "40px digits should be far wider than 15px ones ({} vs {})", d, b);
    }

    /// `lb` is uppercase-only by design. It still has to be able to draw digits, which appear in
    /// nav counts like "15" and "64 G".
    #[test]
    fn the_label_face_covers_uppercase_and_digits() {
        register_all();
        for c in "ABCXYZ0123456789 ".chars() {
            let adv = font::advance_px_face(slot_of(LB.face), c, LB.px);
            assert!(adv > 0, "'{}' has no advance in the label face", c);
        }
    }
}
