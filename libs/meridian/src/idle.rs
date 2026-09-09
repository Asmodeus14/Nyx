//! Idle, sleep and the lock — `parts/19-idle-lock.html`.
//!
//! Three states of the desktop, not three applications, and one rule behind all of them: *"the
//! Entity is never on the desktop — idle is not a contradiction of that, it is the reason for it.
//! Keeping the creature off the working screen is what makes it worth something when the machine is
//! not being worked on."*
//!
//! Everything here is arithmetic over one number — how long since the last input — plus the
//! geometry of two screens. All of it is testable on the host, which matters more here than usual:
//! the shell-side half is a state machine whose bugs only appear after a five-minute wait on a
//! machine with no serial console.
//!
//! ## The timings are the design's, in milliseconds
//!
//! | After | State | What changes |
//! |---|---|---|
//! | 90 s | [`Phase::Receded`] | dock and Entity mark fade out; windows stay |
//! | 5 min | [`Phase::Idle`] | desktop cross-fades to black, the creature comes up at 120 px |
//! | 15 min | [`Phase::Asleep`] | eyes close, float slows to 11 s, backlight to 15% |
//!
//! ## No trigonometry
//!
//! The drift the design specifies is not an ellipse equation — it is four CSS keyframes with linear
//! interpolation between them. So [`drift`] is exact rather than approximated: the same four
//! waypoints, `lerp`ed. That is worth saying because "40 px ellipse over four minutes" reads like it
//! wants `sin`, and this crate has no float.

use crate::layout::Rect;
use crate::scale::px as sc;

// ─────────────────────────────── The state machine ───────────────────────────────

/// How far into idling the desktop is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Being used. Everything drawn as normal.
    Active,
    /// The chrome has stopped asking for attention. Windows are untouched and at full strength —
    /// *"nothing has been hidden"*.
    Receded,
    /// The screensaver, and it is the Entity.
    Idle,
    /// The same screen with the eyes shut, the float slowed and the panel down.
    Asleep,
}

/// `.idle` step 1 — 90 s.
pub const RECEDE_MS: usize = 90_000;
/// `.idle` step 2 — 5 min.
pub const IDLE_MS: usize = 5 * 60_000;
/// `.idle` step 3 — 15 min.
pub const SLEEP_MS: usize = 15 * 60_000;

/// What the desktop should be showing, given how long since the last key or pointer movement.
pub fn phase_for(idle_ms: usize) -> Phase {
    if idle_ms >= SLEEP_MS { Phase::Asleep }
    else if idle_ms >= IDLE_MS { Phase::Idle }
    else if idle_ms >= RECEDE_MS { Phase::Receded }
    else { Phase::Active }
}

impl Phase {
    /// Is the creature on screen? True from [`Phase::Idle`] on — [`Phase::Receded`] is still the
    /// desktop.
    pub fn shows_creature(self) -> bool {
        matches!(self, Phase::Idle | Phase::Asleep)
    }
}

/// How opaque the dock and the Entity mark are, 0..=255.
///
/// Fades out over [`RECEDE_FRAMES`] once past 90 s. ⚠️ This is one-way: waking does NOT run it
/// backwards. *"Moving the pointer reverses it in five frames"* is a separate, much faster ramp —
/// see [`wake_alpha`] — because a chrome that took 20 frames to come back would feel like lag on
/// the one interaction where responsiveness is the entire point.
pub const RECEDE_FRAMES: usize = 20;

pub fn chrome_alpha(idle_ms: usize) -> u8 {
    if idle_ms < RECEDE_MS { return 255; }
    let f = (idle_ms - RECEDE_MS) / 16;
    if f >= RECEDE_FRAMES { return 0; }
    (255 - (255 * f / RECEDE_FRAMES)) as u8
}

/// *"Moving the pointer reverses it in five frames."*
pub const WAKE_FRAMES: usize = 5;

/// Chrome opacity `since_ms` after the pointer moved, having faded to `from`.
pub fn wake_alpha(from: u8, since_ms: usize) -> u8 {
    let f = since_ms / 16;
    if f >= WAKE_FRAMES { return 255; }
    let span = 255 - from as usize;
    (from as usize + span * f / WAKE_FRAMES) as u8
}

/// *"The desktop cross-fades to black over 40 frames."* 0 = desktop, 255 = the idle screen.
pub const FADE_FRAMES: usize = 40;

pub fn idle_fade(idle_ms: usize) -> u8 {
    if idle_ms < IDLE_MS { return 0; }
    let f = (idle_ms - IDLE_MS) / 16;
    if f >= FADE_FRAMES { return 255; }
    (255 * f / FADE_FRAMES) as u8
}

// ─────────────────────────────── Drift ───────────────────────────────

/// `@keyframes ent-drift` — 240 s, `linear`.
pub const DRIFT_MS: usize = 240_000;

/// The design's four waypoints, in design px. The path is closed: the last returns to the first.
const DRIFT_PATH: [(i32, i32); 5] =
    [(0, 0), (18, -12), (0, -22), (-18, -10), (0, 0)];

/// Where the creature sits relative to its resting place, `ms` into the machine's life.
///
/// ⚠️ Phase comes from *absolute* time, not from time-since-idle. If it restarted at each idle the
/// creature would sit at exactly (0,0) — the same pixels — at the start of every single idle
/// period, which is the one thing the drift exists to prevent.
///
/// The animation is `linear` between four keyframes, so this is a lerp and not a curve. Do not
/// "improve" it into an ellipse: 40 px over four minutes is roughly one pixel every six seconds,
/// and the difference between a polygon and an ellipse at that rate is not observable, while the
/// difference between integer arithmetic and a `sin` in a `no_std` crate is a dependency.
pub fn drift(ms: usize) -> (i32, i32) {
    let t = ms % DRIFT_MS;
    let seg_ms = DRIFT_MS / 4;
    let i = t / seg_ms;
    let f = t % seg_ms;
    let (x0, y0) = DRIFT_PATH[i];
    let (x1, y1) = DRIFT_PATH[i + 1];
    let lerp = |a: i32, b: i32| a + (b - a) * f as i32 / seg_ms as i32;
    (sc(lerp(x0, x1)), sc(lerp(y0, y1)))
}

// ─────────────────────────────── Float and blink ───────────────────────────────

/// The creature on the idle screen. `E.svg(g, 4, inf, 5)` — cell 5, and 5 × 24 = 120.
pub const IDLE_PX: i32 = 120;

/// `.crt.asleep .body { animation-duration: 11s }` in 16 ms ticks.
///
/// A constant for every personality, which is where the design's *"a Calm Entity barely changes, an
/// Energetic one visibly settles"* actually comes from: `float_ticks` is already 350×5/4 = 437 for
/// Calm and 181×3/4 = 135 for Energetic, so the same 688 is a small step for one and a fivefold
/// slowing for the other. Nothing had to be written to make that true.
pub const ASLEEP_FLOAT_TICKS: u16 = 688;

/// `@keyframes ent-float { 50% { translateY(-2.2%) } }` — of the creature's own height.
///
/// At the idle size that is 120 × 2.2% = 2.6 px, and the design's own comment is *"two-pixel
/// amplitude, never more — at three it stops reading as breathing and starts reading as bobbing"*.
/// So this truncates rather than rounds: 2, not 3.
pub fn float_amplitude(px: i32) -> i32 {
    (px * 22 / 1000).max(1)
}

/// Vertical offset of the float, `ms` into the machine's life, for a creature `px` tall whose
/// personality floats on a `period_ticks` cycle. Always ≤ 0 — the creature rises and settles, it
/// never sinks below its resting line.
///
/// `ease-in-out` is implemented as smoothstep over the half-cycle, which is what makes the creature
/// dwell at the top and bottom of the breath. At a 2 px amplitude that dwell is the *entire*
/// perceptible difference from a linear ramp — there are only three positions, so what distinguishes
/// breathing from a sawtooth is how long it spends at each.
pub fn float_dy(ms: usize, period_ticks: u16, px: i32) -> i32 {
    let period = (period_ticks as usize).max(1) * 16;
    let t = ms % period;
    // 0..=1000 across the cycle, folded so the second half mirrors the first.
    let half = period / 2;
    let p = if t < half { t * 1000 / half.max(1) } else { (period - t) * 1000 / half.max(1) };
    let p = p.min(1000);
    // smoothstep: p²(3 - 2p), in thousandths.
    let s = p * p / 1000 * (3000 - 2 * p) / 1000;
    // ⚠️ ROUND, do not truncate. At a 2 px amplitude there are three positions, and truncating
    // `2 * s / 1000` reaches -2 only at the single instant s == 1000 — so the creature would spend
    // half the cycle at -1 and effectively never touch the top of its breath. That is a flicker
    // between two positions, not a breath. Rounding splits the cycle roughly 32/37/32.
    -((float_amplitude(px) * s as i32 + 500) / 1000)
}

/// `@keyframes ent-blink { 0%,96.5%,100% { opacity: 0 } 97.6%,99.2% { opacity: 1 } }`
///
/// The lid is fully down between 97.6% and 99.2% of the cycle — 1.6% of it, which at the default
/// 7 s period is the *"110 ms closed, which is two frames at 60 Hz plus one"* the design describes.
/// The 96.5%→97.6% and 99.2%→100% ramps are opacity fades on a one-cell rect; there is no partial
/// alpha in a palette-indexed field, so they collapse to the closed window.
pub fn blink_closed(ms: usize, period_ticks: u16) -> bool {
    let period = (period_ticks as usize).max(1) * 16;
    let p = (ms % period) * 1000 / period;
    (976..=992).contains(&p)
}

// ─────────────────────────────── The idle screen ───────────────────────────────

/// `.idle-t { font-size: 13px; margin-top: 42px }` — the clock, below the creature.
const IDLE_CLOCK_GAP: i32 = 42;
/// `.idle-t { color: #3A3E43 }`. Dimmer than any token on the working desktop, because this is read
/// from across a room in a dark room and a brighter one would be the only light source.
pub const IDLE_CLOCK: u32 = 0xFF3A_3E43;
/// `.idle-s { color: #2E3237 }`, and `.lock-id`.
pub const IDLE_DIM: u32 = 0xFF2E_3237;
/// `.lock-h { color: #565B61 }`
pub const LOCK_HINT: u32 = 0xFF56_5B61;
/// `.lock-h.err { color: #8E939A }` — brighter, not red. *"Nyx has no red anywhere and a lock
/// screen is a poor place to introduce one."*
pub const LOCK_HINT_ERR: u32 = 0xFF8E_939A;
/// `.idle { background: #060708 }`
pub const IDLE_BG: u32 = 0xFF06_0708;
/// The sleep scene's ground — `background:#030405`. Darker than idle, and the only thing besides the
/// backlight that says "asleep".
pub const SLEEP_BG: u32 = 0xFF03_0405;

/// Where the creature sits on the idle screen, before the drift is added.
///
/// `.idle-c { left: 50%; top: 50%; transform: translate(-50%,-50%) }` — the whole stack (creature +
/// clock + line) is centred, not the creature alone. So this centres the *stack* and returns the
/// creature's box within it, which is why it needs to know how tall the text below is.
pub fn idle_creature(screen_w: i32, screen_h: i32, px: i32, text_h: i32) -> Rect {
    let stack_h = px + sc(IDLE_CLOCK_GAP) + text_h;
    Rect::new((screen_w - px) / 2, (screen_h - stack_h) / 2, px, px)
}

/// The baseline row for the clock, given the creature's box.
pub fn idle_clock_y(creature: Rect) -> i32 {
    creature.bottom() + sc(IDLE_CLOCK_GAP)
}

// ─────────────────────────────── The lock ───────────────────────────────

/// `.lock-f { width: 264px }`
const LOCK_W: i32 = 264;
/// `.lock-d { height: 26px; gap: 9px }`
const LOCK_DOTS_H: i32 = 26;
const LOCK_DOT_GAP: i32 = 9;
/// `.lock-d i { width: 6px; height: 6px; border-radius: 3px }`
const LOCK_DOT: i32 = 6;
/// `.lock-d .car { width: 1px; height: 18px }`
const LOCK_CARET_W: i32 = 1;
const LOCK_CARET_H: i32 = 18;
/// `.lock-h { margin-top: 16px }`
const LOCK_HINT_GAP: i32 = 16;
/// `.idle-c { top: 46% }` on the lock scene, and `margin-top: 44px` on the field.
const LOCK_FIELD_GAP: i32 = 44;
/// `.lock-id { bottom: 44px }`
const LOCK_SEED_BOTTOM: i32 = 44;

/// The 264px field, centred, `gap` below the creature.
pub fn lock_field(screen_w: i32, creature: Rect) -> Rect {
    Rect::new((screen_w - sc(LOCK_W)) / 2,
              creature.bottom() + sc(LOCK_FIELD_GAP),
              sc(LOCK_W),
              sc(LOCK_DOTS_H))
}

/// The hairline under the field. `.lock-r { height: 1px }`, `.act` when the field has focus — which
/// on a single-user machine with one field is always.
pub fn lock_rule(field: Rect) -> Rect {
    Rect::new(field.x, field.bottom(), field.w, 1)
}

/// The hint line's top, below the rule.
pub fn lock_hint_y(field: Rect) -> i32 {
    field.bottom() + 1 + sc(LOCK_HINT_GAP)
}

/// The seed, pinned to the bottom of the screen rather than to the stack.
pub fn lock_seed_y(screen_h: i32, line_h: i32) -> i32 {
    screen_h - sc(LOCK_SEED_BOTTOM) - line_h
}

/// One 6px dot per typed character, plus the caret, laid out centred in `field`.
///
/// Returns `(dots, caret)`. ⚠️ The dot count is **clamped** — a 264px field fits a bounded number of
/// them and a long password must not draw off the ends of the rule. The clamp is silent because the
/// alternative is telling an onlooker how long the password is, which is exactly the disclosure the
/// design's *"no username, no avatar"* rule exists to avoid.
pub fn lock_dots(field: Rect, typed: usize) -> (impl Iterator<Item = Rect>, Rect) {
    let d = sc(LOCK_DOT);
    let gap = sc(LOCK_DOT_GAP);
    let caret_w = sc(LOCK_CARET_W).max(1);
    let caret_h = sc(LOCK_CARET_H);

    let n = typed.min(lock_dot_capacity(field));
    // n dots + n gaps + the caret. The trailing gap before the caret is one of the n.
    let total = n as i32 * (d + gap) + caret_w;
    let x0 = field.x + (field.w - total) / 2;
    let cy = field.y + field.h / 2;

    let dots = (0..n).map(move |i| {
        Rect::new(x0 + i as i32 * (d + gap), cy - d / 2, d, d)
    });
    let caret = Rect::new(x0 + n as i32 * (d + gap), cy - caret_h / 2, caret_w, caret_h);
    (dots, caret)
}

/// How many dots fit before the row would overflow the field.
pub fn lock_dot_capacity(field: Rect) -> usize {
    let step = sc(LOCK_DOT) + sc(LOCK_DOT_GAP);
    let room = field.w - sc(LOCK_CARET_W).max(1);
    if step <= 0 || room <= 0 { return 0; }
    (room / step).max(0) as usize
}

// ─────────────────────────────── Rate limiting ───────────────────────────────

/// *"After five failures the field stops accepting input for thirty seconds and says so, which is
/// rate limiting rather than theatre; there is no lockout and nothing is escalated, because there is
/// nobody to escalate to."*
pub const MAX_FAILURES: u32 = 5;
pub const PENALTY_MS: usize = 30_000;

/// Seconds still to wait, or `None` if the field is accepting input.
///
/// ⚠️ Uses `saturating_sub` on the elapsed time deliberately. `sys_get_time` is uptime, so it cannot
/// go backwards — but a wrap or a bad `since` must open the field, not seal it forever. A lock that
/// can wedge itself shut is worse than one that can be retried, given it protects a drawing routine.
pub fn penalty_remaining(failures: u32, since_ms: usize, now_ms: usize) -> Option<usize> {
    if failures < MAX_FAILURES { return None; }
    let elapsed = now_ms.saturating_sub(since_ms);
    if elapsed >= PENALTY_MS { None } else { Some((PENALTY_MS - elapsed).div_ceil(1000)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_steps_happen_at_the_designs_times() {
        assert_eq!(phase_for(0), Phase::Active);
        assert_eq!(phase_for(89_999), Phase::Active);
        assert_eq!(phase_for(90_000), Phase::Receded);
        assert_eq!(phase_for(299_999), Phase::Receded);
        assert_eq!(phase_for(300_000), Phase::Idle);
        assert_eq!(phase_for(899_999), Phase::Idle);
        assert_eq!(phase_for(900_000), Phase::Asleep);
        // And it never goes back on its own.
        assert_eq!(phase_for(usize::MAX / 2), Phase::Asleep);
    }

    /// The creature is on screen for exactly the two states the design puts it in. `Receded` is the
    /// one that is easy to get wrong — the chrome is gone but the desktop is still the desktop.
    #[test]
    fn the_creature_arrives_only_at_the_second_step() {
        assert!(!Phase::Active.shows_creature());
        assert!(!Phase::Receded.shows_creature());
        assert!(Phase::Idle.shows_creature());
        assert!(Phase::Asleep.shows_creature());
    }

    #[test]
    fn the_chrome_fades_out_and_the_wake_ramp_is_faster() {
        assert_eq!(chrome_alpha(0), 255);
        assert_eq!(chrome_alpha(RECEDE_MS - 1), 255);
        assert_eq!(chrome_alpha(RECEDE_MS), 255);
        assert_eq!(chrome_alpha(RECEDE_MS + 16 * RECEDE_FRAMES), 0);
        assert_eq!(chrome_alpha(RECEDE_MS + 60_000), 0);
        // Monotone down.
        let mut prev = 255;
        for f in 0..=RECEDE_FRAMES {
            let a = chrome_alpha(RECEDE_MS + f * 16);
            assert!(a <= prev, "chrome alpha rose at frame {}", f);
            prev = a;
        }
        // Waking is four times faster than receding, and always lands on fully opaque.
        assert_eq!(wake_alpha(0, 0), 0);
        assert_eq!(wake_alpha(0, 16 * WAKE_FRAMES), 255);
        assert_eq!(wake_alpha(120, 16 * WAKE_FRAMES), 255);
        assert!(WAKE_FRAMES * 4 <= RECEDE_FRAMES);
    }

    #[test]
    fn the_cross_fade_to_black_takes_forty_frames() {
        assert_eq!(idle_fade(IDLE_MS - 1), 0);
        assert_eq!(idle_fade(IDLE_MS), 0);
        assert_eq!(idle_fade(IDLE_MS + 16 * FADE_FRAMES), 255);
        assert_eq!(idle_fade(SLEEP_MS), 255);
    }

    /// The whole point of the drift: no pixel is held for long, and the path closes.
    #[test]
    fn the_drift_traces_the_designs_path_and_closes() {
        assert_eq!(drift(0), (0, 0));
        assert_eq!(drift(DRIFT_MS), (0, 0), "the path must close, or it would jump");
        assert_eq!(drift(DRIFT_MS / 4), (sc(18), sc(-12)));
        assert_eq!(drift(DRIFT_MS / 2), (sc(0), sc(-22)));
        assert_eq!(drift(DRIFT_MS * 3 / 4), (sc(-18), sc(-10)));
        // Halfway along the first segment, linearly.
        assert_eq!(drift(DRIFT_MS / 8), (sc(9), sc(-6)));
    }

    /// "40 px over four minutes" — the extent has to actually be about 40 px, and it has to be
    /// travelled slowly enough to be invisible while you are looking at it.
    #[test]
    fn the_drift_is_forty_pixels_and_slow() {
        let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        let mut prev = drift(0);
        let (mut max_dx, mut max_dy) = (0, 0);
        for ms in (0..DRIFT_MS).step_by(1000) {
            let (x, y) = drift(ms);
            lo_x = lo_x.min(x); hi_x = hi_x.max(x);
            lo_y = lo_y.min(y); hi_y = hi_y.max(y);
            max_dx = max_dx.max((x - prev.0).abs());
            max_dy = max_dy.max((y - prev.1).abs());
            prev = (x, y);
        }
        assert_eq!(hi_x - lo_x, sc(36));
        assert_eq!(hi_y - lo_y, sc(22));
        // At most one pixel per second **per axis**. Not the Manhattan sum — both axes do tick in
        // the same second sometimes, and a diagonal single-pixel step is still a single-pixel step.
        assert!(max_dx <= sc(1).max(1), "the drift moved {}px horizontally in one second", max_dx);
        assert!(max_dy <= sc(1).max(1), "the drift moved {}px vertically in one second", max_dy);
    }

    /// A drift keyed to time-since-idle would put the creature on the same pixels at the start of
    /// every idle period, which defeats it entirely. This pins the argument as absolute time.
    #[test]
    fn the_drift_does_not_restart_with_each_idle() {
        assert_ne!(drift(3 * 60 * 60_000), drift(3 * 60 * 60_000 + 40_000));
    }

    #[test]
    fn the_float_breathes_upward_only_and_stays_at_two_pixels() {
        assert_eq!(float_amplitude(IDLE_PX), 2, "the design says two, never three");
        let period = 350u16;
        let mut lo = 0;
        for ms in 0..(period as usize * 16) {
            let dy = float_dy(ms, period, IDLE_PX);
            assert!(dy <= 0, "the creature sank below its resting line at {}ms", ms);
            lo = lo.min(dy);
        }
        assert_eq!(lo, -2);
        // It closes: the end of the cycle is back at rest.
        assert_eq!(float_dy(0, period, IDLE_PX), 0);
        assert_eq!(float_dy(period as usize * 16, period, IDLE_PX), 0);
        // And the top of the breath is the middle of it.
        assert_eq!(float_dy(period as usize * 8, period, IDLE_PX), -2);
    }

    /// `ease-in-out` against a linear ramp: the difference is dwell, and at a 2 px amplitude the
    /// dwell is the *only* difference — there are three positions, so what separates breathing from
    /// a sawtooth is how long it spends at each. Measured against a linear float rather than against
    /// a number I picked, because the design says `ease-in-out` and not "75%".
    #[test]
    fn the_float_dwells_longer_at_the_extremes_than_a_linear_ramp_would() {
        let period = 350u16;
        let total = period as usize * 16;
        let half = total / 2;
        let amp = float_amplitude(IDLE_PX);

        let mut eased = 0;
        let mut linear = 0;
        for ms in 0..total {
            let t = ms % total;
            let p = if t < half { t * 1000 / half } else { (total - t) * 1000 / half };
            if float_dy(ms, period, IDLE_PX).abs() % amp == 0 { eased += 1; }
            if (amp * p.min(1000) as i32 + 500) / 1000 % amp == 0 { linear += 1; }
        }
        assert!(eased > linear,
                "smoothstep dwelt {}/{} vs linear {}/{} — that is not an ease",
                eased, total, linear, total);
        // And it does reach both ends for a meaningful share of the cycle, rather than grazing one.
        let at_top = (0..total).filter(|&ms| float_dy(ms, period, IDLE_PX) == -amp).count();
        let at_rest = (0..total).filter(|&ms| float_dy(ms, period, IDLE_PX) == 0).count();
        assert!(at_top * 100 / total >= 25, "only {}% at the top of the breath", at_top * 100 / total);
        assert!(at_rest * 100 / total >= 25, "only {}% at rest", at_rest * 100 / total);
    }

    /// Blinking is rare and brief, which is what makes it read as a blink and not a flicker.
    #[test]
    fn the_blink_is_a_hundred_odd_milliseconds_once_a_cycle() {
        let period = 437u16; // 7.0 s — the design's default.
        let total = period as usize * 16;
        let closed: usize = (0..total).filter(|&ms| blink_closed(ms, period)).count();
        assert!((100..=130).contains(&closed), "the lid was down for {}ms", closed);
        // Once per cycle, not scattered: the closed window is contiguous.
        let first = (0..total).find(|&ms| blink_closed(ms, period)).unwrap();
        for ms in first..first + closed {
            assert!(blink_closed(ms, period), "the blink was not contiguous at {}", ms);
        }
        assert!(!blink_closed(0, period), "a creature must not be caught mid-blink at t=0");
    }

    /// The design's claim about personality reaching idle. Nothing implements it — it falls out of
    /// `float_ticks` already differing and `ASLEEP_FLOAT_TICKS` being a constant.
    #[test]
    fn sleep_settles_an_energetic_entity_far_more_than_a_calm_one() {
        let calm = 350u16 * 5 / 4;   // 437 — Calm/Quiet slow the float
        let energetic = 181u16 * 3 / 4; // 135 — Energetic quickens it
        let calm_factor = ASLEEP_FLOAT_TICKS as u32 * 100 / calm as u32;
        let energetic_factor = ASLEEP_FLOAT_TICKS as u32 * 100 / energetic as u32;
        assert!(calm_factor < 200, "a Calm entity should barely change: {}x", calm_factor);
        assert!(energetic_factor > 400, "an Energetic one should visibly settle: {}x",
                energetic_factor);
    }

    #[test]
    fn the_lock_field_and_its_parts_stack_without_overlapping() {
        let creature = idle_creature(1440, 900, IDLE_PX, 0);
        let f = lock_field(1440, creature);
        assert_eq!(f.w, sc(264));
        assert_eq!(f.x + f.w / 2, 720, "the field is centred on the screen");
        assert!(f.y > creature.bottom());
        let r = lock_rule(f);
        assert_eq!(r.y, f.bottom());
        assert!(lock_hint_y(f) > r.bottom());
        // The seed is pinned to the screen, not to the stack, so it sits below everything.
        assert!(lock_seed_y(900, 14) > lock_hint_y(f));
    }

    #[test]
    fn the_dots_stay_centred_and_inside_the_field() {
        let creature = idle_creature(1440, 900, IDLE_PX, 0);
        let f = lock_field(1440, creature);
        let cap = lock_dot_capacity(f);
        assert!(cap >= 7, "the design's own scene draws 7 dots; the field fits {}", cap);

        for typed in [0, 1, 7, cap, cap + 40, 500] {
            let (dots, caret) = lock_dots(f, typed);
            let ds: alloc::vec::Vec<Rect> = dots.collect();
            assert_eq!(ds.len(), typed.min(cap), "dot count for {} typed", typed);
            for d in &ds {
                assert!(d.x >= f.x && d.right() <= f.right(),
                        "a dot escaped the field with {} typed", typed);
            }
            assert!(caret.right() <= f.right(), "the caret escaped with {} typed", typed);
            // The run is centred: the slack on the left matches the slack on the right.
            let left = ds.first().map_or(caret.x, |d| d.x) - f.x;
            let right = f.right() - caret.right();
            assert!((left - right).abs() <= 1,
                    "off centre by {} with {} typed", left - right, typed);
        }
    }

    #[test]
    fn five_failures_buy_thirty_seconds_and_then_it_opens_again() {
        assert_eq!(penalty_remaining(0, 0, 0), None);
        assert_eq!(penalty_remaining(4, 0, 0), None);
        assert_eq!(penalty_remaining(5, 1_000, 1_000), Some(30));
        assert_eq!(penalty_remaining(5, 1_000, 16_000), Some(15));
        assert_eq!(penalty_remaining(5, 1_000, 30_999), Some(1), "never says 0 while still shut");
        assert_eq!(penalty_remaining(5, 1_000, 31_000), None);
        // A clock that appears to run backwards must open the field, not seal it.
        assert_eq!(penalty_remaining(5, 90_000, 1_000), Some(30));
        assert_eq!(penalty_remaining(9, 0, PENALTY_MS), None, "no escalation past five");
    }
}
