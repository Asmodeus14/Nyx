//! Precision-touchpad gesture recognition.
//!
//! In its default mouse mode the touchpad's own firmware turns fingers into motion and taps into
//! clicks. Switched to precision mode it reports every contact instead, and all of that becomes the
//! OS's job — this module. It sees one FRAME at a time (every finger currently down, in the device's
//! logical units) and decides what the frame means:
//!
//! | Fingers | Movement                 | Result                                   |
//! |---------|--------------------------|------------------------------------------|
//! | 1       | moves                    | pointer motion                           |
//! | 1       | down + up quickly, still | left click (tap)                         |
//! | 2       | move together            | scroll                                   |
//! | 2       | down + up quickly, still | right click (tap)                        |
//! | 3       | swipe up / down, lift    | open / close the Command                 |
//! | any     | clickpad pressed         | left button; right if 2+ fingers are down |
//!
//! Pure: no hardware, no kernel state, no clock of its own — the caller passes `now_ms`. That is
//! what lets every rule above be tested on the host.

/// One finger that is down and deliberate (tip switch set, and not flagged as a palm).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: u8,
    pub x: i32,
    pub y: i32,
}

/// A tap, reported once, on the frame the fingers lift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    None,
    Left,
    Right,
}

/// What one frame means. Motion and scroll are in the device's logical units; the caller scales.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Out {
    pub dx: i32,
    pub dy: i32,
    /// Positive = the view moves DOWN the content (the scroll offset grows).
    pub scroll_y: i32,
    /// Buttons held by the physical clickpad: bit 0 left, bit 1 right.
    pub buttons: u8,
    pub tap: Tap,
    /// Three-finger swipe completed on this frame: +1 up, -1 down, 0 none.
    pub swipe3: i8,
}

impl Out {
    const NONE: Out = Out { dx: 0, dy: 0, scroll_y: 0, buttons: 0, tap: Tap::None, swipe3: 0 };
}

/// Thresholds, in the device's logical units and milliseconds. Derived from the pad's size so the
/// same rules hold on a small pad and a large one.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// A touch this short, that lifts…
    pub tap_ms: u64,
    /// …having travelled no further than this, is a tap.
    pub tap_slop: i32,
    /// A three-finger swipe must travel at least this far vertically.
    pub swipe_min: i32,
}

impl Config {
    /// Thresholds for a pad whose logical X range is `x_max`: 3% of the width is "didn't move",
    /// 15% of it is a deliberate swipe. 180 ms is the tap window macOS and libinput use.
    pub fn for_pad(x_max: i32) -> Config {
        let w = x_max.max(100);
        Config { tap_ms: 180, tap_slop: w * 3 / 100, swipe_min: w * 15 / 100 }
    }
}

/// Most fingers a precision touchpad reports.
const MAX: usize = 5;

pub struct Engine {
    cfg: Config,
    prev: [Contact; MAX],
    prev_n: usize,
    /// When the current touch began (first finger down).
    down_at: u64,
    /// Most fingers seen at once during this touch — what decides a tap's button.
    max_fingers: usize,
    /// Total movement during this touch, for telling a tap from a short drag.
    travel: i32,
    /// Vertical travel while three fingers were down.
    swipe_y: i32,
    /// The clickpad was pressed during this touch — the lift is then a press ending, not a tap.
    pressed: bool,
}

impl Engine {
    pub fn new(cfg: Config) -> Engine {
        Engine {
            cfg,
            prev: [Contact { id: 0, x: 0, y: 0 }; MAX],
            prev_n: 0,
            down_at: 0,
            max_fingers: 0,
            travel: 0,
            swipe_y: 0,
            pressed: false,
        }
    }

    /// Process one frame: every contact currently down (at most 5 are used), and whether the
    /// clickpad is pressed.
    pub fn frame(&mut self, now_ms: u64, contacts: &[Contact], clickpad: bool) -> Out {
        let n = contacts.len().min(MAX);
        let cur = &contacts[..n];
        let mut out = Out::NONE;

        // A new touch begins.
        if self.prev_n == 0 && n > 0 {
            self.down_at = now_ms;
            self.max_fingers = 0;
            self.travel = 0;
            self.swipe_y = 0;
            self.pressed = false;
        }
        self.max_fingers = self.max_fingers.max(n);
        if clickpad {
            self.pressed = true;
            // Clickpad convention: pressing with two fingers down is a right click.
            out.buttons = if n >= 2 { 0b10 } else { 0b01 };
        }

        // Motion: only when the SAME set of fingers is down in both frames. When a finger lands or
        // lifts, the average position jumps, and applying that jump would throw the pointer.
        if n > 0 && n == self.prev_n {
            let (mut sx, mut sy, mut m) = (0i32, 0i32, 0i32);
            for c in cur {
                if let Some(p) = self.prev[..self.prev_n].iter().find(|p| p.id == c.id) {
                    sx += c.x - p.x;
                    sy += c.y - p.y;
                    m += 1;
                }
            }
            if m == n as i32 {
                let (dx, dy) = (sx / m, sy / m);
                self.travel = self.travel.saturating_add(dx.abs() + dy.abs());
                match n {
                    1 => {
                        out.dx = dx;
                        out.dy = dy;
                    }
                    // Natural scrolling: the content follows the fingers, so fingers moving UP
                    // (dy < 0) move the view further down the content.
                    2 => out.scroll_y = -dy,
                    3 => self.swipe_y = self.swipe_y.saturating_add(dy),
                    _ => {}
                }
            }
        }

        // The touch ends.
        if self.prev_n > 0 && n == 0 {
            let quick = now_ms.saturating_sub(self.down_at) <= self.cfg.tap_ms;
            let still = self.travel <= self.cfg.tap_slop;
            if quick && still && !self.pressed {
                out.tap = match self.max_fingers {
                    1 => Tap::Left,
                    2 => Tap::Right,
                    _ => Tap::None,
                };
            }
            if self.max_fingers >= 3 && self.swipe_y.abs() >= self.cfg.swipe_min {
                out.swipe3 = if self.swipe_y < 0 { 1 } else { -1 };
            }
        }

        self.prev[..n].copy_from_slice(cur);
        self.prev_n = n;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eng() -> Engine {
        Engine::new(Config::for_pad(3000)) // slop 90, swipe 450
    }
    fn c(id: u8, x: i32, y: i32) -> Contact {
        Contact { id, x, y }
    }

    #[test]
    fn one_finger_moves_the_pointer_by_its_delta() {
        let mut e = eng();
        assert_eq!(e.frame(0, &[c(1, 100, 100)], false).dx, 0, "landing is not motion");
        let o = e.frame(8, &[c(1, 130, 90)], false);
        assert_eq!((o.dx, o.dy), (30, -10));
    }

    #[test]
    fn a_quick_still_touch_is_a_left_tap() {
        let mut e = eng();
        e.frame(0, &[c(1, 100, 100)], false);
        e.frame(40, &[c(1, 102, 101)], false);
        assert_eq!(e.frame(90, &[], false).tap, Tap::Left);
    }

    #[test]
    fn a_slow_touch_is_not_a_tap() {
        let mut e = eng();
        e.frame(0, &[c(1, 100, 100)], false);
        assert_eq!(e.frame(400, &[], false).tap, Tap::None);
    }

    #[test]
    fn a_short_drag_is_not_a_tap() {
        let mut e = eng();
        e.frame(0, &[c(1, 100, 100)], false);
        e.frame(30, &[c(1, 300, 100)], false);
        assert_eq!(e.frame(60, &[], false).tap, Tap::None);
    }

    #[test]
    fn a_two_finger_tap_is_a_right_click_even_if_they_land_one_after_the_other() {
        let mut e = eng();
        e.frame(0, &[c(1, 100, 100)], false);
        e.frame(20, &[c(1, 100, 100), c(2, 400, 100)], false);
        e.frame(60, &[c(1, 101, 100), c(2, 401, 100)], false);
        assert_eq!(e.frame(100, &[], false).tap, Tap::Right);
    }

    #[test]
    fn two_fingers_moving_up_scroll_down_the_content_and_do_not_move_the_pointer() {
        let mut e = eng();
        e.frame(0, &[c(1, 100, 500), c(2, 400, 500)], false);
        let o = e.frame(8, &[c(1, 100, 480), c(2, 400, 480)], false);
        assert_eq!(o.scroll_y, 20);
        assert_eq!((o.dx, o.dy), (0, 0));
    }

    #[test]
    fn a_finger_landing_mid_motion_does_not_throw_the_pointer() {
        let mut e = eng();
        e.frame(0, &[c(1, 100, 100)], false);
        // Second finger lands far away: the set changed, so this frame must produce no motion.
        let o = e.frame(8, &[c(1, 100, 100), c(2, 2000, 1500)], false);
        assert_eq!((o.dx, o.dy, o.scroll_y), (0, 0, 0));
    }

    #[test]
    fn three_finger_swipe_up_and_down() {
        let mut e = eng();
        let three = |y| [c(1, 100, y), c(2, 300, y), c(3, 500, y)];
        e.frame(0, &three(1500), false);
        e.frame(50, &three(1200), false);
        e.frame(100, &three(900), false);
        let o = e.frame(150, &[], false);
        assert_eq!(o.swipe3, 1);
        assert_eq!(o.tap, Tap::None, "a swipe is never also a tap");

        e.frame(1000, &three(200), false);
        e.frame(1050, &three(800), false);
        assert_eq!(e.frame(1100, &[], false).swipe3, -1);
    }

    #[test]
    fn a_short_three_finger_move_is_not_a_swipe() {
        let mut e = eng();
        let three = |y| [c(1, 100, y), c(2, 300, y), c(3, 500, y)];
        e.frame(0, &three(1000), false);
        e.frame(50, &three(900), false);
        assert_eq!(e.frame(300, &[], false).swipe3, 0);
    }

    #[test]
    fn clickpad_press_is_left_with_one_finger_and_right_with_two() {
        let mut e = eng();
        assert_eq!(e.frame(0, &[c(1, 100, 100)], true).buttons, 0b01);
        let mut e = eng();
        assert_eq!(e.frame(0, &[c(1, 100, 100), c(2, 400, 100)], true).buttons, 0b10);
    }

    #[test]
    fn releasing_the_clickpad_is_not_also_a_tap() {
        let mut e = eng();
        e.frame(0, &[c(1, 100, 100)], true);
        e.frame(40, &[c(1, 100, 100)], false);
        assert_eq!(e.frame(80, &[], false).tap, Tap::None);
    }
}
