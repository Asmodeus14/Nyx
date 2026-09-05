//! Interface scale — how large Meridian draws itself on a given panel.
//!
//! The design is specified in absolute pixels against a **1440x900** scene (`.scene` in
//! `03-components.html`). Drawn literally on a 1080p panel that is about 20% too small, and on a
//! 4K one it is unusable. So every dimension in `layout` and every type size in `tokens` passes
//! through [`px`] before it reaches a pixel.
//!
//! ## Why a global rather than a parameter
//!
//! The alternative is threading a scale argument through some sixty constants and twenty-five
//! layout functions. That is a lot of signatures to get right, and the failure mode is the worst
//! one available here: a hit-test that scaled and a draw that did not, which is precisely the
//! `ctl_btn_rect` "five hand-copied rects" bug this crate exists to avoid.
//!
//! With one process-wide value, draw and hit-test call the *same* function and therefore cannot
//! disagree. A missed scale somewhere makes the layout slightly wrong — visible, and consistently
//! wrong — instead of making two subsystems believe different things.
//!
//! ## Testing
//!
//! The global defaults to 1.0 and the shell sets it once, before its first frame. Host tests run in
//! parallel, so nothing here mutates the global in a test: [`for_screen`] and [`px_at`] are pure and
//! are what the tests exercise, and every other test in the crate runs at the default 1.0.

use core::sync::atomic::{AtomicI32, Ordering};

/// The height the design was drawn against. `.scene { width: 1440px; height: 900px }`.
pub const REFERENCE_H: i32 = 900;

/// Scale is stored in thousandths, so 1.25x is `1250`. Integer throughout: this multiplies
/// coordinates in a hit-test, and a float rounding difference between two call sites is a click
/// that lands one pixel outside the button.
pub const UNIT: i32 = 1000;

/// The steps a screen can snap to.
///
/// Discrete rather than continuous, for one concrete reason: **icons are rasterized at build time**,
/// at a stroke weight tuned per size, and `icons::get` refuses a size it was not drawn at. A
/// continuous scale would mean either scaling a bitmap — which is exactly the soft, heavy result the
/// icon pipeline was built to avoid — or rasterizing at runtime. Four steps means the generator can
/// emit every size that will ever be asked for, and a test can prove it did.
pub const STEPS: [i32; 4] = [1000, 1250, 1500, 2000];

/// The base icon sizes the design draws at, before scaling.
pub const ICON_BASES: [usize; 5] = [12, 14, 16, 20, 22];

// Storage. On the target this is one process-wide atomic, written once at startup.
//
// Under `cargo test` it becomes THREAD-LOCAL instead. Rust runs tests in parallel, so a shared
// mutable scale would make any test that sets it race with every test that reads it — and the
// symptom would be an intermittent geometry failure, which is the least useful kind. Per-thread
// storage lets each test pick a scale and assert against it in isolation. Test-only, so nothing
// about the shipped binary changes.
#[cfg(not(test))]
static SCALE: AtomicI32 = AtomicI32::new(UNIT);

#[cfg(test)]
std::thread_local! {
    static SCALE_TLS: core::cell::Cell<i32> = core::cell::Cell::new(UNIT);
}

#[cfg(not(test))]
fn store(v: i32) {
    SCALE.store(v, Ordering::Release);
}
#[cfg(not(test))]
fn load() -> i32 {
    SCALE.load(Ordering::Acquire)
}

#[cfg(test)]
fn store(v: i32) {
    SCALE_TLS.with(|c| c.set(v));
}
#[cfg(test)]
fn load() -> i32 {
    SCALE_TLS.with(|c| c.get())
}

/// Force the scale, for tests. Thread-local under `cfg(test)`, so this affects only the calling
/// test.
#[cfg(test)]
pub fn force_for_test(v: i32) {
    store(v);
}

/// The scale for a panel of this size, in thousandths.
///
/// Derived from HEIGHT against the design's reference, because that is what actually runs out —
/// the Command is 620px wide on a 1440px scene and has room to spare, but its result list is capped
/// by vertical space (`layout::command_visible`) and the Entity surface grows upward from a fixed
/// bottom edge.
///
/// Never returns less than 1.0. On a 720p panel the honest answer is 0.8, but the design is already
/// compact — 11px label type at 0.8 is 9px — and shrinking it would trade a layout that overflows
/// for one that cannot be read. A 720p screen gets the design at its intended size and a tighter fit
/// instead, which `command_visible` already handles by showing fewer results.
///
/// ⚠️ This is a *resolution* heuristic, not a DPI one. Nyx has no reliable physical panel size —
/// SMBIOS reports a machine name, not a diagonal — so a 27" 1080p desktop monitor gets the same
/// 1.25x as a 14" 1080p laptop, where only the laptop really wants it. Erring toward larger is the
/// right side to be wrong on: the complaint this fixes is that everything was too small.
pub fn for_screen(_screen_w: i32, screen_h: i32) -> i32 {
    if screen_h <= 0 {
        return UNIT;
    }
    let raw = screen_h * UNIT / REFERENCE_H;
    let mut best = STEPS[0];
    let mut best_d = i32::MAX;
    for &s in STEPS.iter() {
        let d = (raw - s).abs();
        if d < best_d {
            best_d = d;
            best = s;
        }
    }
    best.max(UNIT)
}

/// Set the interface scale from the panel's dimensions. Called once by the shell, before the atlas
/// is built — the atlas rasterizes type at the scaled size, so changing this afterwards would leave
/// every glyph at the wrong size with no indication why.
pub fn init(screen_w: i32, screen_h: i32) -> i32 {
    let s = for_screen(screen_w, screen_h);
    store(s);
    s
}

/// The active scale in thousandths.
pub fn get() -> i32 {
    load()
}

/// A design dimension in real pixels.
pub fn px(v: i32) -> i32 {
    px_at(v, get())
}

/// [`px`] against an explicit scale. Pure, so the rounding can be tested without touching the
/// global.
///
/// Rounds half away from zero and preserves sign, because these are offsets as often as they are
/// sizes: `(w - CTL_SIZE) / 2` is negative for a moment during a resize, and truncating toward zero
/// there would move a control by a pixel depending on which side of the window it was on.
pub const fn px_at(v: i32, scale: i32) -> i32 {
    if v >= 0 {
        (v * scale + UNIT / 2) / UNIT
    } else {
        -((-v * scale + UNIT / 2) / UNIT)
    }
}

/// A design dimension as a `usize` — for the type sizes and icon sizes the atlas is keyed on.
pub fn upx(v: usize) -> usize {
    px(v as i32).max(1) as usize
}

/// The five icon sizes in use at the ACTIVE scale.
///
/// The generated table carries every size for every step ([`required_icon_sizes`]); the atlas packs
/// only these. The table is `.rodata` and costs bytes once; the atlas is a GPU surface and costs
/// bytes every frame it is sampled, so packing all sixteen would quadruple it for nothing.
pub fn active_icon_sizes() -> [usize; ICON_BASES.len()] {
    let mut out = [0usize; ICON_BASES.len()];
    let mut i = 0;
    while i < ICON_BASES.len() {
        out[i] = upx(ICON_BASES[i]);
        i += 1;
    }
    out
}

/// Every icon size the generator has to emit: each base size at each scale step.
///
/// This is the contract between `tools/icons` and the shell. The shell asks for `upx(20)`, and that
/// number has to be in the table or the dock draws nothing — so the generator iterates this and a
/// test checks the generated table against it.
pub fn required_icon_sizes() -> [usize; 20] {
    let mut out = [0usize; 20];
    let mut n = 0;
    let mut bi = 0;
    while bi < ICON_BASES.len() {
        let mut si = 0;
        while si < STEPS.len() {
            out[n] = px_at(ICON_BASES[bi] as i32, STEPS[si]).max(1) as usize;
            n += 1;
            si += 1;
        }
        bi += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn the_reference_screen_is_unscaled() {
        assert_eq!(for_screen(1440, 900), UNIT, "the design's own scene must draw 1:1");
    }

    /// The panels this actually runs on, and what each should pick.
    #[test]
    fn common_panels_land_on_sensible_steps() {
        assert_eq!(for_screen(1280, 720), 1000, "720p gets the design at its intended size");
        assert_eq!(for_screen(1366, 768), 1000);
        assert_eq!(for_screen(1920, 1080), 1250, "1080p is where 'everything is too small' bites");
        assert_eq!(for_screen(1920, 1200), 1250);
        assert_eq!(for_screen(2560, 1440), 1500);
        assert_eq!(for_screen(3840, 2160), 2000);
    }

    /// Never shrink. The design is already compact; 11px label type scaled down stops being type.
    #[test]
    fn small_panels_are_never_scaled_below_one() {
        for h in [400, 600, 720, 768, 800, 899] {
            assert_eq!(for_screen(1024, h), UNIT, "a {}px-tall panel must not shrink the UI", h);
        }
    }

    #[test]
    fn a_degenerate_screen_does_not_divide_by_zero() {
        assert_eq!(for_screen(0, 0), UNIT);
        assert_eq!(for_screen(-1, -1), UNIT);
    }

    /// Scaling must be monotonic in the value, or a padding could end up wider than the box it pads.
    #[test]
    fn scaling_preserves_order() {
        for &s in STEPS.iter() {
            let mut prev = i32::MIN;
            for v in 0..200 {
                let got = px_at(v, s);
                assert!(got >= prev, "px_at({}, {}) went backwards", v, s);
                prev = got;
            }
        }
    }

    /// Offsets go negative during a drag. Truncation toward zero would move a control by a pixel
    /// depending on which side of the origin it was on.
    #[test]
    fn negative_offsets_scale_symmetrically() {
        for &s in STEPS.iter() {
            for v in 1..80 {
                assert_eq!(px_at(-v, s), -px_at(v, s), "asymmetric at v={} scale={}", v, s);
            }
        }
    }

    #[test]
    fn scale_one_is_the_identity() {
        for v in -100..500 {
            assert_eq!(px_at(v, UNIT), v);
        }
    }

    /// A 1px hairline must survive every scale. Meridian carries its structure on hairlines — one
    /// that rounded to zero would delete the border of every window.
    #[test]
    fn a_hairline_never_vanishes() {
        for &s in STEPS.iter() {
            assert!(px_at(1, s) >= 1, "1px vanished at scale {}", s);
            assert!(upx_at(1, s) >= 1);
        }
    }
    fn upx_at(v: usize, s: i32) -> usize {
        px_at(v as i32, s).max(1) as usize
    }

    /// The whole reason the steps are discrete: every size the shell can ask an icon for has to be
    /// a size the generator emitted.
    #[test]
    fn the_required_icon_sizes_cover_every_base_at_every_step() {
        let req = required_icon_sizes();
        for &b in ICON_BASES.iter() {
            for &s in STEPS.iter() {
                let want = px_at(b as i32, s) as usize;
                assert!(req.contains(&want), "base {} at scale {} needs {}px", b, s, want);
            }
        }
        assert!(req.iter().all(|&v| v > 0), "no zero-size icon may be requested");
    }

    /// The union is what `tools/icons` iterates. Pinning it means a change to `STEPS` or
    /// `ICON_BASES` fails here rather than silently leaving the dock blank at one scale.
    #[test]
    fn the_size_union_is_what_we_expect() {
        let mut uniq: Vec<usize> = required_icon_sizes().to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(
            uniq,
            alloc::vec![12, 14, 15, 16, 18, 20, 21, 22, 24, 25, 28, 30, 32, 33, 40, 44],
            "the icon size ladder changed — regenerate icons_gen.rs"
        );
    }
}
