//! Named access to the generated icon set, plus the tests that keep it honest.
//!
//! [`icons_gen`](crate::icons_gen) is machine-written and indexed by string id. Referring to icons
//! by string at every call site would mean a typo renders nothing and says nothing — the exact
//! failure mode the design's own note warns about ("a missing shape is invisible, not obvious").
//! [`Icons`] gives every icon the shell draws a name the compiler checks.

use crate::icons_gen::{self, Icon, IconSet};

/// Every icon Meridian draws, by role rather than by id.
///
/// The `&'static str` values are the design document's `<symbol>` ids, so this table is also the
/// map between "what the shell calls it" and "what the design calls it" — and
/// [`tests::every_named_icon_exists`] fails the build if the two drift apart.
pub struct Icons;

impl Icons {
    // The Entity's three states. Each is a separate coverage cell, so a state change costs exactly
    // what drawing the previous state cost.
    pub const ENTITY_NOMINAL: &'static str = "e-nominal";
    pub const ENTITY_WORKING: &'static str = "e-working";
    pub const ENTITY_ATTENTION: &'static str = "e-attention";

    // The dock, left to right, with SETTINGS after the divider.
    pub const APP_FILES: &'static str = "a-files";
    pub const APP_TERMINAL: &'static str = "a-terminal";
    pub const APP_CODE: &'static str = "a-code";
    pub const APP_BROWSER: &'static str = "a-browser";
    pub const APP_MONITOR: &'static str = "a-monitor";
    pub const APP_QCLANG: &'static str = "a-qclang";
    pub const APP_ENTITY: &'static str = "a-entity";
    pub const APP_SETTINGS: &'static str = "a-settings";

    /// The dock, in the order the design draws it. `None` is the divider before Settings — a 1px
    /// rule, not a glyph.
    pub const DOCK: [Option<&'static str>; 9] = [
        Some(Self::APP_FILES),
        Some(Self::APP_TERMINAL),
        Some(Self::APP_CODE),
        Some(Self::APP_BROWSER),
        Some(Self::APP_MONITOR),
        Some(Self::APP_QCLANG),
        Some(Self::APP_ENTITY),
        None,
        Some(Self::APP_SETTINGS),
    ];

    // Window controls. There are two, not three: maximise is a double-click on the caption line,
    // a gesture the surface already affords, so it needs no glyph.
    pub const WIN_FOLD: &'static str = "w-fold";
    pub const WIN_CLOSE: &'static str = "w-close";
    pub const WIN_EXPAND: &'static str = "w-expand";

    pub const SEARCH: &'static str = "u-search";
    pub const RETURN: &'static str = "u-return";
    pub const CHEVRON: &'static str = "u-chev";
    pub const CHEVRON_DOWN: &'static str = "u-chev-d";
    pub const WIFI: &'static str = "u-wifi";
    pub const POWER: &'static str = "u-power";
    pub const VOLUME: &'static str = "u-volume";
    /// ★ The one symbol here that is NOT in the design as authored. Added to `parts/02-icons.html`
    /// for the Entity's Brightness control row, which is itself an extension — the design puts
    /// Network, Power and Volume there and stops. Drawn in the sheet's own idiom: 24 grid, stroked,
    /// no fill, so it rasterizes through the same path as the other 35 with no special case.
    pub const BRIGHT: &'static str = "u-bright";
    pub const THERMAL: &'static str = "u-thermal";
    pub const FOLDER: &'static str = "u-folder";
    pub const DOC: &'static str = "u-doc";
    pub const CLOCK: &'static str = "u-clock";
    pub const BACK: &'static str = "u-back";
    pub const FORWARD: &'static str = "u-fwd";
    pub const RELOAD: &'static str = "u-reload";
    pub const LOCK: &'static str = "u-lock";
    pub const PLUS: &'static str = "u-plus";
    pub const GRID: &'static str = "u-grid";
    pub const BRANCH: &'static str = "u-branch";
    pub const SHIELD: &'static str = "u-shield";
    pub const CHECK: &'static str = "u-check";
    pub const WARN: &'static str = "u-warn";

    /// The Nyx mark. Solid, never stroked — the solidity is what survives 16px, and it is the one
    /// deliberate difference from the stroked system icons.
    ///
    /// Three placements in the running system: boot/shutdown, the Command footer, and Settings ·
    /// About. Not the dock, not a window caption, not the wallpaper.
    pub const MARK: &'static str = "nyx-mark";
}

/// Every icon the shell can name. Used by the atlas to decide what to pack, and by the test that
/// proves none of these ids has gone stale.
pub const NAMED: [&str; 37] = [
    Icons::ENTITY_NOMINAL, Icons::ENTITY_WORKING, Icons::ENTITY_ATTENTION,
    Icons::APP_FILES, Icons::APP_TERMINAL, Icons::APP_CODE, Icons::APP_BROWSER,
    Icons::APP_MONITOR, Icons::APP_QCLANG, Icons::APP_ENTITY, Icons::APP_SETTINGS,
    Icons::WIN_FOLD, Icons::WIN_CLOSE, Icons::WIN_EXPAND,
    Icons::SEARCH, Icons::RETURN, Icons::CHEVRON, Icons::CHEVRON_DOWN,
    Icons::WIFI, Icons::POWER, Icons::VOLUME, Icons::BRIGHT, Icons::THERMAL,
    Icons::FOLDER, Icons::DOC, Icons::CLOCK, Icons::BACK, Icons::FORWARD,
    Icons::RELOAD, Icons::LOCK, Icons::PLUS, Icons::GRID, Icons::BRANCH,
    Icons::SHIELD, Icons::CHECK, Icons::WARN, Icons::MARK,
];

/// The icon `id` at exactly `px`, or None.
///
/// Deliberately does NOT fall back to a nearby size. The design rasterizes each icon at the size it
/// is drawn at, with a stroke width tuned per size (1.6 at 12px down to 1.25 at 22px); scaling a
/// 12px bitmap up to 20px would throw that away and produce a soft, heavy icon next to crisp text.
/// A caller asking for a size that does not exist has a bug, and should hear about it.
pub fn get(id: &str, px: usize) -> Option<&'static Icon> {
    let set: &IconSet = icons_gen::find(id)?;
    let idx = icons_gen::size_index(px)?;
    Some(&set.sizes[idx])
}

/// The sizes every icon is available at.
pub fn sizes() -> &'static [usize] {
    &icons_gen::SIZES
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names in [`Icons`] are the contract between this crate and the design document. If the
    /// design renames or drops a symbol, this is what says so — otherwise the first symptom is a
    /// blank space on screen, found on hardware, a power cycle later.
    #[test]
    fn every_named_icon_exists() {
        for id in NAMED {
            assert!(
                icons_gen::find(id).is_some(),
                "icon '{}' is named in Icons but absent from the generated table. Re-run \
                 `cargo run --release -p nyx-icons-gen -- ../Nyx-ui`, and if it is still missing, \
                 the design document no longer defines it.",
                id
            );
        }
    }

    /// The reverse direction: an icon in the table that nothing names is either a gap in [`Icons`]
    /// or dead weight in the binary. Both are worth knowing about.
    #[test]
    fn every_generated_icon_is_named() {
        for set in icons_gen::ICONS.iter() {
            assert!(
                NAMED.contains(&set.id),
                "generated icon '{}' has no name in Icons — add one, or remove it from the \
                 generator's source list so it stops costing bytes.",
                set.id
            );
        }
    }

    /// A truncated or mis-sized coverage slice would index out of bounds the first time it is
    /// blitted — on hardware, inside the compositor, which is the desktop.
    #[test]
    fn coverage_lengths_match_their_dimensions() {
        for set in icons_gen::ICONS.iter() {
            for (i, icon) in set.sizes.iter().enumerate() {
                let expect = icon.w as usize * icon.h as usize;
                assert_eq!(
                    icon.cov.len(),
                    expect,
                    "{} @ {}px: {} coverage bytes for a {}x{} bitmap",
                    set.id, icons_gen::SIZES[i], icon.cov.len(), icon.w, icon.h
                );
            }
        }
    }

    /// Every icon must have ink at every size. An icon that rasterized to nothing is invisible on
    /// screen and looks exactly like a layout bug.
    #[test]
    fn no_icon_is_blank() {
        for set in icons_gen::ICONS.iter() {
            for (i, icon) in set.sizes.iter().enumerate() {
                assert!(icon.w > 0 && icon.h > 0, "{} @ {}px is empty", set.id, icons_gen::SIZES[i]);
                assert!(
                    icon.cov.iter().any(|&c| c != 0),
                    "{} @ {}px has no ink",
                    set.id, icons_gen::SIZES[i]
                );
            }
        }
    }

    /// Cropping must be tight: every edge of the stored bitmap has to carry ink, or the table is
    /// padding the atlas with transparent pixels at every size of every icon.
    #[test]
    fn bitmaps_are_cropped_tight() {
        for set in icons_gen::ICONS.iter() {
            for (i, icon) in set.sizes.iter().enumerate() {
                let (w, h) = (icon.w as usize, icon.h as usize);
                let px = icons_gen::SIZES[i];
                assert!(
                    (0..w).any(|x| icon.cov[x] != 0),
                    "{} @ {}px: top row is blank", set.id, px
                );
                assert!(
                    (0..w).any(|x| icon.cov[(h - 1) * w + x] != 0),
                    "{} @ {}px: bottom row is blank", set.id, px
                );
                assert!(
                    (0..h).any(|y| icon.cov[y * w] != 0),
                    "{} @ {}px: left column is blank", set.id, px
                );
                assert!(
                    (0..h).any(|y| icon.cov[y * w + w - 1] != 0),
                    "{} @ {}px: right column is blank", set.id, px
                );
            }
        }
    }

    /// Icons must fit inside the box they claim, or they will overhang their atlas cell and bleed
    /// into a neighbour's.
    #[test]
    fn icons_fit_their_box() {
        for set in icons_gen::ICONS.iter() {
            for (i, icon) in set.sizes.iter().enumerate() {
                let px = icons_gen::SIZES[i];
                assert!(
                    icon.off_y as usize + icon.h as usize <= px,
                    "{} @ {}px: overhangs vertically ({} + {} > {})",
                    set.id, px, icon.off_y, icon.h, px
                );
            }
        }
    }

    /// Antialiasing has to survive into the table. If every icon came back 1-bit, the rasterizer's
    /// supersampling regressed and the icons will look like a bitmap font next to the AA text.
    #[test]
    fn icons_are_antialiased() {
        let soft: usize = icons_gen::ICONS
            .iter()
            .flat_map(|s| s.sizes.iter())
            .flat_map(|i| i.cov.iter())
            .filter(|&&c| c > 0 && c < 255)
            .count();
        assert!(soft > 1000, "only {} partially-covered pixels across the whole set", soft);
    }

    #[test]
    fn get_refuses_a_size_it_does_not_have() {
        assert!(get(Icons::APP_FILES, 20).is_some());
        assert!(get(Icons::APP_FILES, 19).is_none(), "must not silently substitute a nearby size");
        assert!(get("no-such-icon", 20).is_none());
    }

    /// The dock is eight glyphs and one divider. The design counts the permanent chrome as "eight
    /// glyphs and one mark" and builds its whole argument on that number.
    #[test]
    fn the_dock_is_eight_glyphs_and_one_divider() {
        assert_eq!(Icons::DOCK.iter().filter(|d| d.is_some()).count(), 8);
        assert_eq!(Icons::DOCK.iter().filter(|d| d.is_none()).count(), 1);
        for id in Icons::DOCK.iter().flatten() {
            assert!(icons_gen::find(id).is_some(), "dock icon '{}' is missing", id);
        }
    }
}
