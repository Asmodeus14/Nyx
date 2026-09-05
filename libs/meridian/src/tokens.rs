//! Meridian's design tokens — the colour palette and the type scale.
//!
//! Transcribed from `Nyx-ui/design/ver3.0/parts/01-head.html`, where they are CSS custom
//! properties. The names here match those properties one for one (`--fg-2` → `fg_2`) so a rule in
//! the design document can be found in this file without translation, and so a change there is a
//! mechanical change here.
//!
//! Both themes ship from the start. The design specifies light and dark as equals rather than as a
//! theme and its inversion — light is not a tint of dark, it has its own accent (`#2B55C8` vs
//! `#4676F0`) because the darker blue is what stays legible on white.
//!
//! This does NOT replace `nyx_gui::canvas::Color`. That palette (`WARM_BG`, `WARM_SURFACE`, …) is
//! still what every unported app draws with, and deleting it would break all of them at once. The
//! two coexist until the apps are ported one at a time.

/// One resolved theme. Colours are `0xAARRGGBB`, matching `Canvas::fill_rect` and every other
/// drawing entry point in `nyx_gui`.
///
/// Several tokens are deliberately **translucent** rather than solid. `line` is
/// `rgba(255,255,255,0.070)`, not a flat grey, because the same hairline has to read correctly over
/// a window surface *and* over the wallpaper — a solid line tuned for one is wrong on the other.
/// This is also what lets the design avoid a blur pass: flat alpha over a low-frequency wallpaper
/// reads as translucency on its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    /// The deepest ground. Behind everything, including the wallpaper's darkest stop.
    pub void: u32,
    /// A window's content surface.
    pub surface: u32,
    /// One step above `surface` — used where two panels meet and a border alone is not enough.
    pub raised: u32,
    /// One step below `surface`. The terminal and the code pane sit on this.
    pub sunken: u32,
    /// Hairline. Translucent on purpose — see the struct note.
    pub line: u32,
    /// The stronger hairline: focused window borders, floating surface edges, the fold rule.
    pub line_2: u32,
    /// Primary text.
    pub fg: u32,
    /// Secondary text — running dock glyphs, inactive captions, measurement keys.
    pub fg_2: u32,
    /// Tertiary text — labels, metadata, the Entity at rest.
    pub fg_3: u32,
    /// The faintest legible tone. Dock glyphs at rest, group headings, hints.
    pub fg_4: u32,
    /// The single accent. Used perhaps three times per screen: focus marks, the caret, the running
    /// dot, a measurement fill.
    pub accent: u32,
    /// The accent at low alpha, for a selected row's ground.
    pub acc_dim: u32,
    /// Hover ground.
    pub wash: u32,
    /// Selected/pressed ground.
    pub wash_2: u32,
    /// Floating-surface ground (the Command, the Entity surface). Translucent: these sit over the
    /// wallpaper and are supposed to read as material.
    pub chrome: u32,
    /// The scrim drawn behind the Command.
    pub scrim: u32,
    /// True for the dark theme. A few decisions (which way a shadow tints, whether to lighten or
    /// darken on hover) need to know rather than infer it from a luminance test.
    pub is_dark: bool,
}

/// `rgba()` from the design document → `0xAARRGGBB`. `a` is thousandths, so CSS `0.070` is `70`
/// and the rounding is explicit rather than hidden in a float literal.
const fn rgba(r: u8, g: u8, b: u8, a_milli: u32) -> u32 {
    let a = ((a_milli * 255 + 500) / 1000) as u32;
    (a << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// An opaque `#RRGGBB` from the design document.
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
// ★ THE FOREGROUND TIERS WERE RE-TARGETED FOR CONTRAST
// ─────────────────────────────────────────────────────────────────────────────
//
// The design document's `--fg-3` and `--fg-4` are unreadable on a real panel. Measured against
// their own grounds, as sRGB contrast ratios:
//
//     dark, on --void #08090A          light, on --surface #FFFFFF
//       fg    #F4F5F6   18.3:1           fg    #0E1013   19.1:1
//       fg-2  #8E939A    6.4:1           fg-2  #6A7078    5.0:1
//       fg-3  #565B61    2.9:1  ✗        fg-3  #9AA0A8    2.6:1  ✗
//       fg-4  #3A3E43    1.9:1  ✗✗       fg-4  #C3C8CE    1.7:1  ✗✗
//
// WCAG's floor is 4.5:1 for body text and 3:1 for large text. `fg-4` at 1.9:1 is below the point
// where type is legible at all — and `fg-4` is not a decorative tier here, it carries **the dock at
// rest**, every group heading, and every hint. `fg-3` carries inactive captions, the Entity's phrase
// and every icon at rest. Between them they are most of the interface that is not focused.
//
// Two things compound it on hardware. Meridian sets 11-15px type in Inter Light and Regular, so
// there is very little ink per glyph; and the icons are ~1px strokes, which antialiasing spreads
// across two pixel rows so they never reach full coverage — an `fg-4` icon renders dimmer than
// `fg-4` text does.
//
// So the three lower tiers are re-solved to 4.5 / 7 / 11 against their own ground, preserving the
// design's slight blue lean (B > G > R) and leaving `fg` and `accent` exactly as specified. The
// original values are recorded above rather than deleted: this is a deliberate departure from the
// document, and `tests::every_tier_is_legible` is what stops it drifting back.

impl Theme {
    /// `.dark` — the primary theme. Black / graphite / soft grey / white, one blue.
    pub const fn dark() -> Theme {
        Theme {
            void: rgb(0x08, 0x09, 0x0A),
            surface: rgb(0x0F, 0x11, 0x13),
            raised: rgb(0x16, 0x18, 0x1B),
            sunken: rgb(0x0B, 0x0C, 0x0E),
            line: rgba(255, 255, 255, 70),
            line_2: rgba(255, 255, 255, 130),
            fg: rgb(0xF4, 0xF5, 0xF6),      // 18.3:1
            fg_2: rgb(0xBC, 0xC0, 0xC6),    // 10.9:1  (design: #8E939A, 6.4:1)
            fg_3: rgb(0x96, 0x9A, 0xA0),    //  7.0:1  (design: #565B61, 2.9:1)
            fg_4: rgb(0x74, 0x78, 0x7E),    //  4.5:1  (design: #3A3E43, 1.9:1)
            accent: rgb(0x46, 0x76, 0xF0),
            acc_dim: rgba(70, 118, 240, 160),
            wash: rgba(255, 255, 255, 35),
            wash_2: rgba(255, 255, 255, 65),
            chrome: rgba(14, 16, 18, 800),
            scrim: rgba(6, 7, 8, 520),
            is_dark: true,
        }
    }

    /// `.light` — a peer of `dark`, not an inversion of it.
    pub const fn light() -> Theme {
        Theme {
            void: rgb(0xF2, 0xF3, 0xF5),
            surface: rgb(0xFF, 0xFF, 0xFF),
            raised: rgb(0xFF, 0xFF, 0xFF),
            sunken: rgb(0xF7, 0xF8, 0xFA),
            line: rgba(10, 12, 16, 85),
            line_2: rgba(10, 12, 16, 150),
            fg: rgb(0x0E, 0x10, 0x13),      // 19.1:1
            fg_2: rgb(0x36, 0x3D, 0x45),    // 11.0:1  (design: #6A7078, 5.0:1)
            fg_3: rgb(0x52, 0x5A, 0x62),    //  7.0:1  (design: #9AA0A8, 2.6:1)
            fg_4: rgb(0x70, 0x78, 0x80),    //  4.5:1  (design: #C3C8CE, 1.7:1)
            accent: rgb(0x2B, 0x55, 0xC8),
            acc_dim: rgba(43, 85, 200, 110),
            wash: rgba(10, 12, 16, 35),
            wash_2: rgba(10, 12, 16, 60),
            chrome: rgba(255, 255, 255, 820),
            scrim: rgba(226, 230, 236, 550),
            is_dark: false,
        }
    }

    /// The ground each foreground tier is read against: the wallpaper in the dark theme (the dock,
    /// the Entity's phrase and window captions all sit directly on it), the window surface in the
    /// light one.
    pub const fn ground(&self) -> u32 {
        if self.is_dark {
            self.void
        } else {
            self.surface
        }
    }
}

/// sRGB relative luminance of an `0xAARRGGBB` colour, per WCAG 2.x. Alpha is ignored — every tier
/// this is used on is opaque.
///
/// Float maths, but it runs in a test and in nothing else. Kept here rather than in the test module
/// so the numbers in the table above can be re-derived by anyone reading the file.
pub fn luminance(c: u32) -> f32 {
    fn ch(v: u32) -> f32 {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            // No `powf` in core; this crate has `libm`.
            libm::powf((c + 0.055) / 1.055, 2.4)
        }
    }
    0.2126 * ch((c >> 16) & 0xFF) + 0.7152 * ch((c >> 8) & 0xFF) + 0.0722 * ch(c & 0xFF)
}

/// WCAG contrast ratio between two opaque colours, 1.0..=21.0.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

// ─────────────────────────────────────────────────────────────────────────────
// SHADOW
// ─────────────────────────────────────────────────────────────────────────────

/// The one shadow in the system, marking the focused window and floating surfaces.
///
/// ⚠️ The design document carries these values twice and they disagree, so both are recorded here.
/// The CSS token is `0 8px 28px rgba(0,0,0,0.46)`. But `08-tail`'s constraints table describes the
/// shipping implementation as "one CPU SDF shadow pass (`ui.rs:256`), tuned to blur 9, offset 4,
/// peak alpha 42" and accepts it as-is. The second is the one that renders: `nyx_gui::ui::
/// draw_window_shadow` is already tuned to those numbers, hugs the rounded outline, and skips
/// occluded pixels — reproducing the CSS values would mean a wider, heavier shadow and a new pass.
///
/// So the shell reuses `draw_window_shadow`, and these constants exist to document the gap rather
/// than to hide it. Revisit only if the shadow visibly reads as too tight on hardware.
pub const SHADOW_BLUR: usize = 9;
pub const SHADOW_OFFSET_Y: i32 = 4;
pub const SHADOW_PEAK_ALPHA: u8 = 42;

/// Window corner radius. The design uses 6px for windows and 8px for floating surfaces.
pub const RADIUS_WINDOW: usize = 6;
pub const RADIUS_SURFACE: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// TYPE
// ─────────────────────────────────────────────────────────────────────────────

/// Which embedded face a style draws with. The atlas keys on this, so adding a variant means
/// adding a shelf — see `atlas.rs`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    /// Inter 200. Display sizes only.
    ExtraLight,
    /// Inter 300. Headings.
    Light,
    /// Inter 400. Body.
    Regular,
    /// Inter 500. Labels and window names.
    Medium,
    /// JetBrains Mono 400. Terminal, code, and the Command's key hints.
    Mono,
}

/// One step of the type scale.
///
/// Seven discrete sizes, which is also seven shelves in the GPU atlas — the design picked the count
/// with the atlas in mind, not just the page. `tracking` is CSS letter-spacing in **thousandths of
/// an em**, kept integer because the layout path has no business doing float maths per glyph.
/// Negative tracking on the display sizes is what stops 40px light type from looking loose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style {
    pub px: usize,
    pub line_h: usize,
    pub tracking: i32,
    pub face: Face,
    /// `lb` is the only uppercase style. Stored here so callers do not each remember to transform
    /// the string — and so the atlas knows it never needs lowercase on that shelf.
    pub upper: bool,
}

impl Style {
    /// This style's size in REAL pixels on the current panel.
    ///
    /// ★ `px` and `line_h` are DESIGN units, measured against the design's 1440x900 scene — the same
    /// rule as `layout`'s constants. Rasterize at `size()`, not at `px`, or the type stays 15px on a
    /// 4K panel while the box around it grows.
    pub fn size(&self) -> usize {
        crate::scale::upx(self.px)
    }

    /// This style's line box in real pixels — what a caller centres text within.
    pub fn line(&self) -> usize {
        crate::scale::upx(self.line_h)
    }

    /// Letter-spacing in whole real pixels. Rounds toward zero; at 11px the `lb` style's `0.115em`
    /// is 1px, which is the visible difference between a label and a word.
    pub fn tracking_px(&self) -> i32 {
        (self.size() as i32 * self.tracking) / 1000
    }
}

/// `d1` — 40px/200. Almost entirely digits: the System Monitor's four numbers. The atlas shelf for
/// this style packs 0-9 and a handful of letters rather than the full ASCII range.
pub const D1: Style = Style { px: 40, line_h: 44, tracking: -32, face: Face::ExtraLight, upper: false };
/// `d2` — 28px/300. Section headings.
pub const D2: Style = Style { px: 28, line_h: 34, tracking: -24, face: Face::Light, upper: false };
/// `d3` — 20px/300. The Command's query line, the Entity surface's statement.
pub const D3: Style = Style { px: 20, line_h: 27, tracking: -16, face: Face::Light, upper: false };
/// `b1` — 15px/400. Primary body: file names, result names, settings keys.
pub const B1: Style = Style { px: 15, line_h: 22, tracking: -5, face: Face::Regular, upper: false };
/// `b2` — 13px/400. Standard body.
pub const B2: Style = Style { px: 13, line_h: 19, tracking: 0, face: Face::Regular, upper: false };
/// `b3` — 12px/400. Metadata, captions, the Entity's phrase.
pub const B3: Style = Style { px: 12, line_h: 17, tracking: 4, face: Face::Regular, upper: false };
/// `lb` — 11px/500 uppercase, wide tracking. Group headings and nav labels.
pub const LB: Style = Style { px: 11, line_h: 14, tracking: 115, face: Face::Medium, upper: true };
/// Window captions: 12px/500. Not a numbered step in the design's scale, but a distinct style —
/// `b3`'s size at `lb`'s weight, without the uppercasing.
pub const CAPTION: Style = Style { px: 12, line_h: 17, tracking: 4, face: Face::Medium, upper: false };
/// Terminal and code. 13px mono on a 22px line — looser than `b2` because code needs the air.
pub const MONO: Style = Style { px: 13, line_h: 22, tracking: 0, face: Face::Mono, upper: false };
/// 11px/400, lightly tracked, **not** uppercased. `lb` is the design's only 11px style and it
/// transforms its text, but two surfaces set 11px lowercase — `.cmd-q .hint` ("esc to dismiss",
/// `.04em`) and `.cmd-f` ("navigate", "open", "refine"). Rendering those through `lb` would shout
/// them; rendering them at `b3`'s 12px would make the Command's quietest line its largest hint.
/// One extra shelf is about 10 KiB of atlas, which is the cheaper mistake.
pub const HINT: Style = Style { px: 11, line_h: 14, tracking: 40, face: Face::Regular, upper: false };

/// Every style, in the order the atlas builds them. `atlas.rs` iterates this, so a style that is
/// not listed here has no shelf and cannot be drawn on the GPU path.
pub const ALL_STYLES: [Style; 10] = [D1, D2, D3, B1, B2, B3, LB, CAPTION, MONO, HINT];

#[cfg(test)]
mod tests {
    use super::*;

    /// The alpha conversions are the part most likely to be silently wrong, and a hairline at the
    /// wrong alpha is invisible rather than obviously broken.
    #[test]
    fn alpha_channels_match_the_css() {
        let d = Theme::dark();
        assert_eq!(d.line >> 24, 18, "0.070 * 255");
        assert_eq!(d.line_2 >> 24, 33, "0.130 * 255");
        assert_eq!(d.wash >> 24, 9, "0.035 * 255");
        assert_eq!(d.wash_2 >> 24, 17, "0.065 * 255");
        assert_eq!(d.chrome >> 24, 204, "0.80 * 255");
        assert_eq!(d.acc_dim >> 24, 41, "0.16 * 255");

        let l = Theme::light();
        assert_eq!(l.line >> 24, 22, "0.085 * 255");
        assert_eq!(l.line_2 >> 24, 38, "0.150 * 255");
        assert_eq!(l.chrome >> 24, 209, "0.82 * 255");
    }

    /// ★ Every foreground tier must be legible on its own ground.
    ///
    /// This is the test the design document needed and did not have. `--fg-4` shipped at 1.9:1,
    /// which is not "quiet" — it is invisible, and it carries the dock. Found by measuring, not by
    /// looking at a screen, which matters on a machine where looking costs a power cycle.
    ///
    /// The thresholds are WCAG's: 4.5:1 for text, and every one of these tiers sets text somewhere.
    #[test]
    fn every_tier_is_legible_on_its_own_ground() {
        for t in [Theme::dark(), Theme::light()] {
            let g = t.ground();
            for (name, c, min) in [
                ("fg", t.fg, 12.0),
                ("fg_2", t.fg_2, 8.0),
                ("fg_3", t.fg_3, 5.5),
                ("fg_4", t.fg_4, 4.4),
                ("accent", t.accent, 4.4),
            ] {
                let r = contrast(c, g);
                assert!(
                    r >= min,
                    "{} theme: {} (#{:06X}) is {:.2}:1 against its ground — needs {:.1}:1",
                    if t.is_dark { "dark" } else { "light" },
                    name,
                    c & 0xFF_FFFF,
                    r,
                    min
                );
            }
        }
    }

    /// The tiers have to stay *distinguishable*, or lifting them for legibility destroys the thing
    /// they exist for: saying which of two pieces of text is the focused one. A step of less than
    /// 1.3x in contrast is not visible as a step.
    #[test]
    fn the_tiers_remain_distinct_steps() {
        for t in [Theme::dark(), Theme::light()] {
            let g = t.ground();
            let tiers = [("fg_4", t.fg_4), ("fg_3", t.fg_3), ("fg_2", t.fg_2), ("fg", t.fg)];
            for w in tiers.windows(2) {
                let (lo, hi) = (contrast(w[0].1, g), contrast(w[1].1, g));
                assert!(
                    hi / lo >= 1.3,
                    "{} -> {} is only a {:.2}x step ({:.1}:1 to {:.1}:1) — the tiers have collapsed",
                    w[0].0, w[1].0, hi / lo, lo, hi
                );
            }
        }
    }

    /// Sanity on the maths itself, against values with known answers.
    #[test]
    fn the_contrast_formula_is_right() {
        let r = contrast(0xFF_FFFFFF, 0xFF_000000);
        assert!((r - 21.0).abs() < 0.01, "white on black should be 21:1, got {}", r);
        assert!((contrast(0xFF_808080, 0xFF_808080) - 1.0).abs() < 0.001, "a colour on itself is 1:1");
        // #767676 on white is the canonical 4.5:1 boundary case from the WCAG docs.
        let b = contrast(0xFF_767676, 0xFF_FFFFFF);
        assert!((b - 4.5).abs() < 0.06, "expected ~4.5:1 for #767676 on white, got {:.3}", b);
    }

    #[test]
    fn opaque_tokens_are_opaque() {
        for t in [Theme::dark(), Theme::light()] {
            for c in [t.void, t.surface, t.raised, t.sunken, t.fg, t.fg_2, t.fg_3, t.fg_4, t.accent] {
                assert_eq!(c >> 24, 255, "{:#010x} should be fully opaque", c);
            }
        }
    }

    #[test]
    fn rgb_channels_round_trip() {
        assert_eq!(Theme::dark().accent, 0xFF_4676F0);
        assert_eq!(Theme::light().accent, 0xFF_2B55C8);
        assert_eq!(Theme::dark().fg, 0xFF_F4F5F6);
        assert_eq!(Theme::light().fg, 0xFF_0E1013);
    }

    /// Tracking is the reason 40px weight-200 type does not look like a mistake, so it has to
    /// survive the em → px conversion as a non-zero number.
    #[test]
    fn tracking_resolves_to_useful_pixels() {
        assert_eq!(D1.tracking_px(), -1, "40px * -0.032em");
        assert_eq!(LB.tracking_px(), 1, "11px * 0.115em");
        assert_eq!(B2.tracking_px(), 0, "13px * 0em");
    }

    /// The atlas builds exactly what this table lists; a style missing from it is a style that
    /// silently cannot be drawn.
    #[test]
    fn every_style_is_in_the_atlas_table() {
        for s in [D1, D2, D3, B1, B2, B3, LB, CAPTION, MONO] {
            assert!(ALL_STYLES.contains(&s), "{:?} is missing from ALL_STYLES", s);
        }
    }
}
