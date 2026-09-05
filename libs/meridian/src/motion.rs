//! Motion: one ease-out table, seven transitions, and nothing else.
//!
//! ## Why there are no springs
//!
//! Spring physics needs velocity handed off from a continuously tracked gesture. Nyx polls the
//! mouse and has no sub-frame timing, so a spring here would be a fixed curve wearing a physics
//! costume — the same twelve numbers, dressed up. A precomputed table indexed by frame is honest
//! about what the input layer can actually supply, and it is integer-only, which matters because
//! some of this runs where there is no FPU state worth trusting.
//!
//! There is no bounce anywhere either, and no overshoot. Every curve in this file is monotonic: a
//! window that grows past its size and settles back is asking to be looked at, and Meridian's
//! motion is metered rather than decorative.
//!
//! ## Why motion is metered at all
//!
//! Any window mid-fade forces the compositor to a full-screen frame — a scissored present could
//! clip a translucent quad, and the next partial frame would double-blend it. So an animation is
//! not free the way it is on a system with a real compositor: it is *the whole screen*, per frame,
//! for its duration. That is the reason every transition below is short, local, and rare, and the
//! reason each one's cost is written down next to it.
//!
//! ## Frames, not milliseconds
//!
//! Everything is counted in frames at 60 Hz because that is what the shell's loop can actually
//! observe. The millisecond figures in the design are the consequence, not the input, and are
//! repeated here only so the two can be checked against each other.

/// The ease-out curve, sampled at twelve points, in thousandths.
///
/// Cubic ease-out: `1 - (1 - t)^3`. Front-loaded — a quarter of the distance is covered in the
/// first step — which is what makes a short transition read as *responsive* rather than as a
/// delay. Linear over eight frames reads as sluggish at exactly this duration, which is why the
/// window-open fade got noticed before anything else did.
///
/// Twelve entries rather than one per frame: transitions run 5 to 10 frames, and a single shared
/// table means they are all recognisably the same movement at different lengths.
pub const EASE: [u16; 12] = [0, 249, 452, 615, 742, 838, 906, 952, 980, 994, 999, 1000];

/// Full progress, in the same thousandths as [`EASE`].
pub const UNIT: u32 = 1000;

/// Eased progress at `num`/`den` along a transition, 0..=[`UNIT`].
///
/// ★ **Interpolates between table entries. Snapping to the nearest one is a stutter.**
///
/// The table has twelve entries and transitions run anywhere from 5 to 22 frames, so the mapping
/// from frame to table index is almost never 1:1. Rounding to the nearest entry means consecutive
/// frames can land on the *same* one — and the first version of this did exactly that: the 22-frame
/// cold-start handoff produced eleven distinct positions, alternating between a large jump and a
/// frame of no movement at all. It read as the mark being dragged a step at a time, which is what
/// it was.
///
/// Linear interpolation between adjacent entries costs one multiply and makes the curve continuous
/// at any frame count. The endpoints stay exact.
pub fn ease(num: u32, den: u32) -> u32 {
    if den == 0 || num >= den {
        return UNIT;
    }
    let span = (EASE.len() - 1) as u32;
    // Position along the table, kept scaled by `den` so this stays integer.
    let pos = num * span;
    let i = (pos / den) as usize;
    let frac = pos % den;
    let a = EASE[i] as u32;
    let b = EASE[(i + 1).min(span as usize)] as u32;
    a + (b - a) * frac / den
}

/// Every animation in the system. There are seven, and adding an eighth should be an argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transition {
    /// Opacity 0→255 and scale 0.985→1. 8 frames · 130 ms.
    ///
    /// Scale is a per-frame change to the destination rectangle — no scene rebuild.
    WindowOpen,
    /// Height→0, opacity 255→0. 7 frames · 115 ms.
    ///
    /// The quad leaves the composite entirely; only the caption text run remains.
    Fold,
    /// Scrim opacity, panel scale 0.98→1. 7 frames · 115 ms.
    ///
    /// One full-screen alpha fill plus one small surface.
    Command,
    /// Opacity and scale, anchored to the mark it grew from. 8 frames · 130 ms.
    EntityExpand,
    /// Cross-fade between two glyphs. 10 frames · 165 ms.
    ///
    /// Two coverage glyphs at 22px — roughly 500 pixels of damage.
    EntityState,
    /// The two caption controls fading in on hover. 5 frames · 80 ms.
    Controls,
    /// Caption tint and shadow. 5 frames · 80 ms.
    ///
    /// Two window footprints dirty, not the screen.
    Focus,
}

impl Transition {
    /// How many frames this runs for. Frame 0 is the start; the last frame is exactly complete.
    pub const fn frames(self) -> u8 {
        match self {
            Transition::WindowOpen => 8,
            Transition::Fold => 7,
            Transition::Command => 7,
            Transition::EntityExpand => 8,
            Transition::EntityState => 10,
            Transition::Controls => 5,
            Transition::Focus => 5,
        }
    }

    /// Duration in milliseconds at 60 Hz, rounded — the design's own figures.
    pub const fn ms(self) -> u32 {
        (self.frames() as u32 * 1000 + 30) / 60
    }
}

/// One running animation.
///
/// Deliberately tiny and `Copy`: a window carries one of these, and the shell steps it once per
/// frame alongside everything else it already does. Holds no clock — the shell's frame loop *is*
/// the clock, and a timeline that read the RTC would drift against the frames it is drawn on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Motion {
    kind: Transition,
    /// Frames elapsed. Saturates at `frames()`, which is the finished state.
    frame: u8,
}

impl Motion {
    /// A transition at its first frame — nothing has moved yet.
    pub const fn starting(kind: Transition) -> Motion {
        Motion { kind, frame: 0 }
    }

    /// A transition already finished. The resting state for anything not currently animating.
    pub const fn done(kind: Transition) -> Motion {
        Motion { kind, frame: kind.frames() }
    }

    pub const fn kind(&self) -> Transition {
        self.kind
    }

    pub const fn is_done(&self) -> bool {
        self.frame >= self.kind.frames()
    }

    /// Advance one frame. Returns true while there is still movement left to draw.
    ///
    /// The return value is what the caller marks damage on. Returning true on the frame that
    /// *completes* the transition is deliberate: that frame still has to be drawn, and a caller
    /// that stopped one early would leave the window at 98% opacity forever.
    pub fn tick(&mut self) -> bool {
        if self.is_done() {
            return false;
        }
        self.frame += 1;
        true
    }

    /// Restart, keeping the same transition.
    pub fn restart(&mut self) {
        self.frame = 0;
    }

    /// Eased progress, 0..=[`UNIT`].
    ///
    /// The table is sampled across its full width regardless of how many frames this transition
    /// has, so a 5-frame fade and a 10-frame cross-fade trace the same curve at different speeds.
    pub fn progress(&self) -> u32 {
        ease(self.frame as u32, self.kind.frames().max(1) as u32)
    }

    /// Interpolate between two values along the eased curve. Endpoints are exact.
    pub fn lerp(&self, from: i32, to: i32) -> i32 {
        let p = self.progress();
        if p == 0 {
            return from;
        }
        if p >= UNIT {
            return to;
        }
        from + ((to - from) as i64 * p as i64 / UNIT as i64) as i32
    }

    /// The same, clamped into a byte — the common case, since most of these transitions are alpha.
    pub fn lerp_u8(&self, from: u8, to: u8) -> u8 {
        self.lerp(from as i32, to as i32).clamp(0, 255) as u8
    }

    /// Scale a length by a factor that runs `from`..`to`, both in thousandths.
    ///
    /// Used for the two transitions that grow: a window opening at 0.985 and the Command's panel at
    /// 0.98. Rounds to nearest, and never returns 0 — a quad with a zero dimension is not a small
    /// quad, it is a quad the compositor may reject.
    pub fn scale(&self, len: i32, from: u32, to: u32) -> i32 {
        let f = self.lerp(from as i32, to as i32).max(1) as i64;
        (((len as i64 * f) + UNIT as i64 / 2) / UNIT as i64).max(1) as i32
    }
}

/// The scale a window starts at when it opens, in thousandths. 0.985.
pub const WINDOW_OPEN_SCALE: u32 = 985;
/// The scale the Command's panel starts at. 0.98.
pub const COMMAND_OPEN_SCALE: u32 = 980;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_is_monotonic_and_lands_exactly() {
        // Overshoot is the failure this catches. A curve that peaks above UNIT and settles back is
        // a bounce, and the design rules bounce out of the system entirely.
        assert_eq!(EASE[0], 0);
        assert_eq!(*EASE.last().unwrap(), UNIT as u16);
        for w in EASE.windows(2) {
            assert!(w[1] >= w[0], "curve went backwards: {:?}", w);
            assert!(w[1] as u32 <= UNIT, "curve overshot: {}", w[1]);
        }
    }

    #[test]
    fn the_curve_is_front_loaded() {
        // What separates ease-out from linear, and the whole reason the table exists. At the
        // halfway frame the movement must be most of the way there; linear would be at 500.
        let mid = EASE[EASE.len() / 2] as u32;
        assert!(mid > 800, "halfway progress is {}, which is not an ease-out", mid);
        // And the first step alone should cover a visible fraction.
        assert!(EASE[1] > 200, "first step is only {}", EASE[1]);
    }

    #[test]
    fn every_transition_starts_at_nothing_and_finishes_exactly() {
        // The two ends are the ones anybody would notice: a window that never reaches full opacity,
        // or one that is already visible on the frame it was created.
        for t in [
            Transition::WindowOpen,
            Transition::Fold,
            Transition::Command,
            Transition::EntityExpand,
            Transition::EntityState,
            Transition::Controls,
            Transition::Focus,
        ] {
            let mut m = Motion::starting(t);
            assert_eq!(m.progress(), 0, "{:?} starts mid-way", t);
            assert_eq!(m.lerp_u8(0, 255), 0, "{:?}", t);
            let mut steps = 0;
            while m.tick() {
                steps += 1;
                assert!(steps <= 32, "{:?} never finished", t);
            }
            assert_eq!(steps, t.frames() as usize, "{:?} took {} frames", t, steps);
            assert_eq!(m.progress(), UNIT, "{:?} stopped short", t);
            assert_eq!(m.lerp_u8(0, 255), 255, "{:?} never reached full opacity", t);
            assert_eq!(m.lerp(100, 40), 40, "{:?} endpoint is not exact", t);
        }
    }

    /// ★ The stutter test. This is the bug the cold-start handoff shipped with.
    #[test]
    fn a_long_transition_moves_on_every_single_frame() {
        // 22 frames against a 12-entry table. Snapping to the nearest entry gave eleven distinct
        // positions — jump, freeze, jump, freeze — which is exactly what "it moved frame by frame"
        // looks like. Interpolation has to produce a new value every frame.
        const N: u32 = 22;
        let mut prev = ease(0, N);
        assert_eq!(prev, 0);
        // Only over the first two thirds: cubic ease-out is genuinely asymptotic at the end, where
        // 994 -> 999 -> 1000 is the curve being right rather than the sampler being coarse.
        for f in 1..(N * 2 / 3) {
            let p = ease(f, N);
            assert!(p > prev, "frame {} did not move: {} -> {}", f, prev, p);
            prev = p;
        }
        assert_eq!(ease(N, N), UNIT);
    }

    #[test]
    fn ease_is_exact_at_both_ends_for_any_length() {
        for den in 1..=64u32 {
            assert_eq!(ease(0, den), 0, "den {}", den);
            assert_eq!(ease(den, den), UNIT, "den {}", den);
            assert_eq!(ease(den + 5, den), UNIT, "overrun must clamp, den {}", den);
        }
        assert_eq!(ease(0, 0), UNIT, "a zero-length transition is already over");
    }

    #[test]
    fn ease_interpolates_rather_than_snapping() {
        // Halfway between two table entries must land between them, not on one of them.
        let a = EASE[1] as u32;
        let b = EASE[2] as u32;
        let mid = ease(3, 22); // 3*11/22 = 1.5 — exactly between entries 1 and 2
        assert!(mid > a && mid < b, "{} should sit strictly between {} and {}", mid, a, b);
    }

    #[test]
    fn progress_never_goes_backwards_within_a_transition() {
        // The table is resampled per transition length, and a rounding mistake in that mapping
        // shows up as a single frame that dips — which on an opacity fade is a visible flicker.
        for t in [Transition::Controls, Transition::WindowOpen, Transition::EntityState] {
            let mut m = Motion::starting(t);
            let mut prev = m.progress();
            while m.tick() {
                let p = m.progress();
                assert!(p >= prev, "{:?} dipped from {} to {}", t, prev, p);
                prev = p;
            }
        }
    }

    #[test]
    fn the_frame_counts_match_the_designs_milliseconds() {
        // The design states both the frame count and a duration, and they have to agree or one of
        // them is decoration. Its durations are rounded to the nearest 5 ms — 8 frames at 60 Hz is
        // 133.3, printed as 130 — so this checks agreement within that rounding rather than
        // pretending the stated figures are exact.
        for (t, stated) in [
            (Transition::WindowOpen, 130),
            (Transition::Fold, 115),
            (Transition::Command, 115),
            (Transition::EntityExpand, 130),
            (Transition::EntityState, 165),
            (Transition::Controls, 80),
            (Transition::Focus, 80),
        ] {
            let real = t.ms() as i32;
            assert!(
                (real - stated as i32).abs() <= 5,
                "{:?} is {} frames = {} ms; the design says {} ms",
                t, t.frames(), real, stated
            );
        }
    }

    #[test]
    fn a_finished_motion_stays_finished() {
        let mut m = Motion::done(Transition::Focus);
        assert!(m.is_done());
        assert!(!m.tick(), "a finished motion must not ask for another frame");
        assert_eq!(m.progress(), UNIT);
        m.restart();
        assert!(!m.is_done());
        assert_eq!(m.progress(), 0);
    }

    #[test]
    fn the_open_scale_grows_and_arrives_at_full_size() {
        // 0.985 of a 704px window is 693 — a seven-pixel grow, which is the point. Anything larger
        // reads as a zoom, and the design is explicit that this is "a small scale".
        let mut m = Motion::starting(Transition::WindowOpen);
        let start = m.scale(704, WINDOW_OPEN_SCALE, UNIT);
        assert_eq!(start, 693);
        while m.tick() {}
        assert_eq!(m.scale(704, WINDOW_OPEN_SCALE, UNIT), 704, "must land on the real size");
    }

    #[test]
    fn scale_never_collapses_a_quad_to_zero() {
        // A zero-width quad is not a small quad; it is one the compositor may reject outright.
        let m = Motion::starting(Transition::Command);
        assert!(m.scale(1, COMMAND_OPEN_SCALE, UNIT) >= 1);
        assert!(m.scale(0, COMMAND_OPEN_SCALE, UNIT) >= 1);
    }

    #[test]
    fn fold_runs_the_curve_backwards_without_a_second_table() {
        // Fold is the only transition that goes away rather than arrives, and it uses the same
        // curve — `lerp` from 255 to 0 rather than a mirrored copy of the table.
        let mut m = Motion::starting(Transition::Fold);
        assert_eq!(m.lerp_u8(255, 0), 255);
        m.tick();
        assert!(m.lerp_u8(255, 0) < 255);
        while m.tick() {}
        assert_eq!(m.lerp_u8(255, 0), 0);
    }
}
