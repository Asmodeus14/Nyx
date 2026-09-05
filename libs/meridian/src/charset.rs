//! Which characters each type style packs into the atlas.
//!
//! The atlas is the gate on this whole design: today `nyx_gui::gpu_text` holds ONE size (14px) over
//! printable ASCII, and Meridian needs nine styles across five typefaces plus the icon set. Packing
//! every style over the full ASCII range would work, and would also spend about a third of a
//! megabyte rasterizing 40px lowercase letters that the design never sets at 40px.
//!
//! So the two display shelves are restricted, which is the design's own instruction: *"The two
//! largest are used almost entirely for digits, so those shelves pack 0-9 and a handful of letters
//! rather than the full ASCII range."*
//!
//! The risk in restricting a charset is that a glyph goes missing and nothing says so — a word
//! renders with a hole in it and reads as a layout bug. Two things guard against that: the shelves
//! that carry running text are unrestricted, and [`Atlas::misses`](crate::atlas::Atlas::misses)
//! counts every character the shell asked for and could not draw, so the answer is a serial line
//! rather than an afternoon.

/// Printable ASCII, the range `nyx_gui::gpu_text` already uses.
pub const ASCII_FIRST: char = ' ';
pub const ASCII_LAST: char = '~';

/// Non-ASCII characters the design actually sets, and the surface each one appears on. Every style
/// that carries running text packs these, because they are punctuation in this interface rather
/// than decoration — losing the middle dot would break `15 items · 58.2 GB free`.
pub const SYMBOLS: &[char] = &[
    '\u{00B7}', // ·  the separator: "15 items · 58.2 GB free", "Curious · Focused · Calm"
    '\u{00B0}', // °  "82 °C" — System Monitor and the Entity surface
    '\u{2191}', // ↑  the Command footer: "↑↓ navigate"
    '\u{2193}', // ↓
    '\u{21B5}', // ↵  "↵ open"
    '\u{21E5}', // ⇥  "⇥ refine"
    '\u{203A}', // ›  the terminal prompt: "~/src/bell ›"
    '\u{27E9}', // ⟩  QCLang ket notation: "|00⟩"
];

/// Characters the `d1` display shelf (40px) packs.
///
/// Digits, the punctuation that appears between them, and uppercase for units. Lowercase is
/// deliberately absent: `d1`'s only use in the design is the System Monitor's four numbers, and it
/// puts the unit in a separate 15px span, so the 40px run is genuinely numeric.
///
/// ⚠️ This does **not** apply to `d2` at 28px, which sets Title Case section headings and needs the
/// full range. "Display size" and "digits only" are separate claims; the design makes the second
/// one about the largest shelf specifically.
pub const DISPLAY: &str = "0123456789 .,:;%+-/()°·ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Characters the `lb` label shelf packs. It is uppercase-only by design — [`crate::tokens::Style`]
/// carries `upper: true` and the layout path transforms the string — so lowercase would be dead
/// weight.
pub const LABEL: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,:;·-/()%";

/// Which set a style draws from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Charset {
    /// Printable ASCII plus [`SYMBOLS`]. Everything that carries running text.
    Full,
    /// [`DISPLAY`] — digits, punctuation and uppercase.
    Display,
    /// [`LABEL`] — uppercase, digits and a little punctuation.
    Label,
}

impl Charset {
    /// Call `f` once per character in this set, in a stable order. The order is the order cells are
    /// packed in, so keeping it deterministic is what makes the atlas layout reproducible.
    pub fn for_each(self, mut f: impl FnMut(char)) {
        match self {
            Charset::Full => {
                let mut c = ASCII_FIRST as u32;
                while c <= ASCII_LAST as u32 {
                    if let Some(ch) = char::from_u32(c) {
                        f(ch);
                    }
                    c += 1;
                }
                for &s in SYMBOLS {
                    f(s);
                }
            }
            Charset::Display => DISPLAY.chars().for_each(f),
            Charset::Label => LABEL.chars().for_each(f),
        }
    }

    /// Whether `c` is in this set. Used by the layout path to tell "not packed" from "packed but
    /// blank", which are very different bugs.
    pub fn contains(self, c: char) -> bool {
        match self {
            Charset::Full => (ASCII_FIRST..=ASCII_LAST).contains(&c) || SYMBOLS.contains(&c),
            Charset::Display => DISPLAY.contains(c),
            Charset::Label => LABEL.contains(c),
        }
    }

    /// How many characters this set holds.
    pub fn len(self) -> usize {
        let mut n = 0;
        self.for_each(|_| n += 1);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_covers_printable_ascii_and_the_design_symbols() {
        assert_eq!(Charset::Full.len(), 95 + SYMBOLS.len());
        for c in ['A', 'z', '0', ' ', '~', '{', '\\'] {
            assert!(Charset::Full.contains(c), "'{}' should be in the full set", c);
        }
        for &c in SYMBOLS {
            assert!(Charset::Full.contains(c), "symbol {:?} should be in the full set", c);
        }
    }

    /// The strings the design actually sets at `d1` — the System Monitor's four numbers, and the
    /// "82 °C" thermal reading. Lifted from the mockup. If a charset trim ever drops one of these
    /// characters, this says so instead of the screen.
    ///
    /// Note what is NOT here: `4 h 20 m` and `58.2 GB free` look like display strings but are set
    /// at 12px in the design, not 40px. Assuming otherwise is what made the first version of this
    /// restriction wrong.
    #[test]
    fn display_covers_what_the_design_sets_at_40px() {
        for s in ["64", "42", "31", "82", "100", "0.5", "82 °C", "1,024", "14:32"] {
            for c in s.chars() {
                assert!(
                    Charset::Display.contains(c),
                    "'{}' from {:?} is not in the display charset",
                    c, s
                );
            }
        }
    }

    /// The corollary: Title Case headings must NOT be assumed to fit the display set. `d2` sets
    /// these at 28px and therefore has to use the full range.
    #[test]
    fn title_case_headings_do_not_fit_the_display_set() {
        assert!(
            !"Applications".chars().all(|c| Charset::Display.contains(c)),
            "if this ever passes, the display set grew lowercase and the d1 restriction is moot"
        );
    }

    /// Same for the label shelf, after the uppercasing the layout path applies.
    #[test]
    fn label_covers_the_design_headings() {
        for s in ["LOCATIONS", "DEVICES", "APPLICATIONS", "COMMANDS", "FILES", "64 G", "12 M"] {
            for c in s.chars() {
                assert!(Charset::Label.contains(c), "'{}' from {:?} is not in the label charset", c, s);
            }
        }
    }

    /// The restriction has to actually save something, or it is complexity for nothing.
    #[test]
    fn the_restricted_sets_are_smaller() {
        assert!(Charset::Display.len() < Charset::Full.len());
        assert!(Charset::Label.len() < Charset::Full.len());
        assert!(!Charset::Display.contains('a'), "display sizes do not set lowercase");
        assert!(!Charset::Label.contains('a'), "lb is uppercased before layout");
    }

    /// `for_each` order is the packing order; a non-deterministic one would make the atlas layout
    /// differ between builds for no reason.
    #[test]
    fn iteration_order_is_stable() {
        let collect = |cs: Charset| {
            let mut v = alloc::vec::Vec::new();
            cs.for_each(|c| v.push(c));
            v
        };
        for cs in [Charset::Full, Charset::Display, Charset::Label] {
            assert_eq!(collect(cs), collect(cs));
            assert_eq!(collect(cs).len(), cs.len());
        }
    }
}
