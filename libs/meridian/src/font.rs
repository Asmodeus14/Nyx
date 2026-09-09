//! Meridian's typefaces.
//!
//! The design is typographically led — headings are *large and light* rather than big and bold, and
//! that only works with a face that has genuine 200/300 weights. DejaVu Sans, which `nyx-gui` has
//! embedded since F1, has exactly one weight; setting 40px display type in it produces something
//! heavier and wider than the design at every size above body text. So Meridian ships its own:
//! Inter at four weights, and JetBrains Mono for the terminal and code surfaces.
//!
//! ## ★ The bytes are SHARED, not embedded (2026-09-08)
//!
//! They used to be `include_bytes!`d straight into this module, which meant every binary that
//! linked it carried its own copy — `--gc-sections` cannot drop a byte of a static something
//! reaches. That cost, measured across the eight Meridian binaries, was **10.6 MB in a 33.6 MB boot
//! image**: 1.93 MB each in the shell and System Monitor, 1.24 MB in the four that call
//! [`register_text`], and so on. Splitting registration into `register_text`/`register_mono`/
//! `register_display` trimmed the edges of that but could not fix its shape — eight copies of the
//! same five files.
//!
//! So the faces now ship **once**, in the initrd, at [`FONT_DIR`], and every binary reads the ones
//! it needs at startup. The image drops by about eight and a half megabytes and adding an
//! application no longer costs two.
//!
//! **What this trades away.** A missing or unreadable bundle means every face falls back to DejaVu.
//! That is a failure this module was already designed to survive (see [`register_all`]) and it is
//! the *visible* kind: the whole interface renders in a face that is obviously not Inter, on a
//! machine with no serial console where an invisible failure would be much worse. The filesystem is
//! no less available than the shell's own startup reads — `meridian.conf`, `entity.bin` and
//! `auth.bin` all come off `/mnt/nvme` before the first frame.
//!
//! **What it does not trade away.** RAM. The bytes used to sit in each binary's `.rodata`, which is
//! resident too; now they sit in each binary's heap. Roughly a wash. Making it a genuine single
//! copy needs the faces published in SHM and mapped read-only by every client, which is a discovery
//! protocol and an ordering guarantee — worth doing if RAM ever becomes the constraint, and not
//! before.
//!
//! ## Why the bytes live here and not in `nyx-gui`
//!
//! Eight applications link `nyx-gui`. Putting Inter there would have put it in every one of them,
//! for the benefit of the single binary that draws Meridian. So `nyx-gui::font` owns the rasterizer
//! and a small registry of face slots, and this module owns the *sourcing* and fills the slots.
//! Unported apps keep drawing in DejaVu and their binaries do not grow by a byte.
//!
//! ## Licensing
//!
//! Both families are SIL Open Font License 1.1. The licenses ship in the bundle next to the fonts,
//! as the OFL requires — they are copied into [`FONT_DIR`] by `Build.sh`, not left behind in the
//! source tree where the binary they cover cannot reach them.

use alloc::string::String;
use alloc::vec::Vec;
use nyx_gui::font;

use crate::tokens::Face;

/// Where `Build.sh` puts the shared typefaces. A second top-level directory in `initrd.tar`
/// alongside `apps`, extracted by `installer::extract_tar_to_ext4` like everything else.
pub const FONT_DIR: &str = "/mnt/nvme/fonts/";

/// Inter, weight 200. Display sizes only — `d1`, which is almost entirely digits.
const INTER_EXTRALIGHT: &str = "Inter-ExtraLight.ttf";
/// Inter, weight 300. Headings: `d2` and `d3`.
const INTER_LIGHT: &str = "Inter-Light.ttf";
/// Inter, weight 400. Body: `b1`, `b2`, `b3`.
const INTER_REGULAR: &str = "Inter-Regular.ttf";
/// Inter, weight 500. Labels and window captions.
const INTER_MEDIUM: &str = "Inter-Medium.ttf";
/// JetBrains Mono, weight 400. Terminal, code, and the Command's key hints.
const JETBRAINS_MONO: &str = "JetBrainsMono-Regular.ttf";

/// Every file the bundle must contain. One list, so the loader, the tests and anything that wants
/// to check the bundle cannot disagree about what "the fonts" means.
pub const FACE_FILES: [&str; 5] =
    [INTER_EXTRALIGHT, INTER_LIGHT, INTER_REGULAR, INTER_MEDIUM, JETBRAINS_MONO];

/// A sanity bound on a face file. The largest of the five is 417 KB; anything past this is a
/// corrupt read rather than a typeface, and reading it into a `no_std` app's heap would be an
/// out-of-memory abort in a startup path with nothing to report it.
const MAX_FACE_BYTES: usize = 2 * 1024 * 1024;

/// Read one face out of the bundle and leak it.
///
/// Leaked deliberately: `register_face` holds a `&'static [u8]` for the life of the process, and a
/// face is installed once at startup. [`install`] refuses to load a slot that is already filled, so
/// calling `register_text()` and then `register_mono()` — or either one twice — cannot leak a
/// second copy.
#[cfg(not(test))]
fn face_bytes(file: &str) -> Option<&'static [u8]> {
    let mut path = String::from(FONT_DIR);
    path.push_str(file);
    let fd = nyx_api::sys_open(&path);
    if fd < 0 {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = nyx_api::sys_read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
        if out.len() > MAX_FACE_BYTES {
            out.clear();
            break;
        }
    }
    nyx_api::sys_close(fd);
    if out.is_empty() {
        return None;
    }
    Some(alloc::boxed::Box::leak(out.into_boxed_slice()))
}

/// The host-test source for the same bytes.
///
/// ★ The seam is deliberately here and nowhere higher. `register_all`, `register_text`, `install`
/// and the slot mapping are the *same code* on the host and on the machine; only where the bytes
/// come from differs. A test that took a different path through the registration logic would prove
/// nothing about the path that ships.
#[cfg(test)]
fn face_bytes(file: &str) -> Option<&'static [u8]> {
    Some(match file {
        INTER_EXTRALIGHT => include_bytes!("../fonts/Inter-ExtraLight.ttf").as_slice(),
        INTER_LIGHT => include_bytes!("../fonts/Inter-Light.ttf").as_slice(),
        INTER_REGULAR => include_bytes!("../fonts/Inter-Regular.ttf").as_slice(),
        INTER_MEDIUM => include_bytes!("../fonts/Inter-Medium.ttf").as_slice(),
        JETBRAINS_MONO => include_bytes!("../fonts/JetBrainsMono-Regular.ttf").as_slice(),
        _ => return None,
    })
}

/// Fill one slot, if it is not already filled. Returns whether the slot has a face afterwards.
fn install(slot: usize, file: &str) -> bool {
    // Already installed — do not read the file again, and above all do not leak a second copy.
    if font::face_ready(slot) {
        return true;
    }
    match face_bytes(file) {
        Some(bytes) => font::register_face(slot, bytes),
        None => false,
    }
}

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
    faces.iter().filter(|(slot, file)| !install(*slot, file)).count()
}

/// Install the three faces a **ported application interior** sets type in: Inter Light, Regular
/// and Medium.
///
/// Between [`register_all`] and [`register_mono`] in cost, and it exists for the same reason
/// `register_mono` does — a face should cost only the binaries that render it. The five faces are
/// 1.93 MB and `register_all` pins every one of them, because they arrive as a single array and
/// `--gc-sections` cannot drop a byte of a static something reaches. An app that draws a heading, a
/// body column and a label needs three of them:
///
/// | style | face | drawn as |
/// |---|---|---|
/// | `d2`, `d3` | Light | section headings |
/// | `b1`, `b2`, `b3`, `hint` | Regular | names, metadata, hints |
/// | `lb`, `caption` | Medium | group headings, nav labels |
///
/// The two it leaves out are `ExtraLight` (415 KB — `d1` only, which is the System Monitor's four
/// large numbers) and `Mono` (274 KB — the terminal and code surfaces). An app that needs either
/// asks for it: `register_text()` then `register_mono()` is two calls and about 690 KB less than
/// `register_all` for an app that wants four of the five.
///
/// Returns the number of faces that failed to parse; zero is the only good answer. Non-fatal for
/// the same reason as [`register_all`] — the fallback is DejaVu, not a blank window.
pub fn register_text() -> usize {
    let faces = [
        (SLOT_LIGHT, INTER_LIGHT),
        (SLOT_REGULAR, INTER_REGULAR),
        (SLOT_MEDIUM, INTER_MEDIUM),
    ];
    faces.iter().filter(|(slot, file)| !install(*slot, file)).count()
}

/// Install **only** Inter ExtraLight — the `d1` face, and nothing else.
///
/// The companion to [`register_text`] for the one surface in the design that sets a large number:
/// System Monitor's four metrics are `d1`, 40px at weight 200, and that weight exists in no other
/// style. `register_text()` then `register_display()` is the whole type system of that window at
/// ~1.65 MB, against `register_all`'s 1.93 MB — the saving is Mono, which a monitor never draws.
///
/// Kept a separate call rather than folded into `register_text` because `d1` is genuinely rare: one
/// window out of seven uses it, and making the other six carry 415 KB for a style they never set is
/// the exact cost `register_text` was created to avoid.
///
/// Returns true if the face parsed and is ready to draw.
pub fn register_display() -> bool {
    install(SLOT_EXTRALIGHT, INTER_EXTRALIGHT)
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
    install(SLOT_MONO, JETBRAINS_MONO)
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
    use crate::tokens::{ALL_STYLES, B1, B2, B3, CAPTION, D1, D2, D3, HINT, LB, MONO};

    /// Every file in the bundle has to be a parseable TrueType face. A truncated or wrong-format
    /// font would otherwise surface as text silently rendering in DejaVu — which looks like a design
    /// disagreement, not a bug, and could survive a long time.
    #[test]
    fn every_face_parses() {
        assert_eq!(register_all(), 0, "at least one typeface failed to parse");
        assert!(all_ready());
    }

    /// ★ The bundle contract. Every name the loader will ask `/mnt/nvme/fonts/` for must be the
    /// name of a file that actually exists in `libs/meridian/fonts/`, because that directory is
    /// exactly what `Build.sh` copies into the bundle.
    ///
    /// This is the test that earns its keep now that the bytes are NOT `include_bytes!`d into the
    /// shipping binary. Rename a font file and fix only the `include_bytes!` path and the host
    /// tests stay green while the machine finds nothing and renders the entire interface in DejaVu
    /// — a failure that costs a power cycle to see and looks like a design change rather than a
    /// build break.
    #[test]
    fn every_face_file_exists_where_build_sh_will_look_for_it() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fonts");
        for file in FACE_FILES {
            let p = dir.join(file);
            assert!(p.is_file(), "{} is named by the loader but not in libs/meridian/fonts/", file);
            let n = std::fs::metadata(&p).unwrap().len() as usize;
            assert!(n > 100_000, "{file} is only {n} bytes — a truncated font, not a face");
            assert!(n < MAX_FACE_BYTES, "{file} is {n} bytes, past the loader's own bound");
        }
        // The OFL requires the licences travel with the fonts, and the bundle is where the binary
        // that uses them can actually reach one.
        for l in ["LICENSE-Inter.txt", "LICENSE-JetBrainsMono.txt"] {
            assert!(dir.join(l).is_file(), "{l} must ship in the bundle");
        }
    }

    /// The five names are distinct and the directory is a directory. Concatenation, not `join` —
    /// a `FONT_DIR` without its trailing slash would produce `/mnt/nvme/fontsInter-Light.ttf`,
    /// which opens nothing and reports itself as "no fonts installed".
    #[test]
    fn the_bundle_path_composes() {
        assert!(FONT_DIR.ends_with('/'), "FONT_DIR must end in a separator");
        for (i, a) in FACE_FILES.iter().enumerate() {
            assert!(face_bytes(a).is_some(), "{a} is in FACE_FILES but the loader cannot source it");
            for b in &FACE_FILES[i + 1..] {
                assert_ne!(a, b, "two slots would load the same file");
            }
        }
        assert!(face_bytes("Helvetica.ttf").is_none(), "an unknown face must not resolve");
    }

    /// Registering twice must not read or leak a second copy. On the machine `face_bytes` allocates
    /// ~1.9 MB and `Box::leak`s it, so an `install` that ignored an already-filled slot would spend
    /// that much heap again every time an app called two registration helpers in a row — which
    /// `apps/qcstudio` does, and which is the normal case rather than the odd one.
    #[test]
    fn installing_a_filled_slot_is_a_no_op() {
        assert_eq!(register_all(), 0);
        assert!(font::face_ready(SLOT_MONO));
        // Second time round every slot is already ready, so `install` returns true without
        // sourcing anything. Observable here only as success; on the target it is the difference
        // between one copy and two.
        assert_eq!(register_all(), 0);
        assert_eq!(register_text(), 0);
        assert!(register_mono() && register_display());
    }

    /// The subset an application interior installs has to cover every style a ported app actually
    /// draws. If a style's face is missing, `with_glyph_face` falls back to DejaVu and the app
    /// renders in the wrong typeface at the right size — which looks like a design decision.
    #[test]
    fn register_text_covers_every_style_an_app_interior_sets() {
        assert_eq!(register_text(), 0, "an Inter face failed to parse");
        // The styles a ported interior draws: headings, body, metadata, labels, captions, hints.
        for s in [D2, D3, B1, B2, B3, LB, CAPTION, HINT] {
            assert!(font::face_ready(slot_of(s.face)), "{:?} has no face after register_text", s.face);
        }
    }

    /// ...and it must NOT install the two it exists to leave out, or it is `register_all` with a
    /// different name and the binary carries 690 KB it never draws. Checked by identity rather than
    /// by `face_ready`, since another test in this module may have registered everything first.
    #[test]
    fn register_text_installs_exactly_three_faces() {
        let text = [SLOT_LIGHT, SLOT_REGULAR, SLOT_MEDIUM];
        assert_eq!(text.len(), 3);
        for s in [SLOT_EXTRALIGHT, SLOT_MONO] {
            assert!(!text.contains(&s), "slot {} should not be in the text subset", s);
        }
        assert_eq!(slot_of(D1.face), SLOT_EXTRALIGHT, "d1 is the style register_text deliberately omits");
        assert_eq!(slot_of(MONO.face), SLOT_MONO);
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
