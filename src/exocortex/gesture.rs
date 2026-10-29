//! Touches into gestures (roadmap §2.3, §3.7): a tap; a press held still
//! lifts at 150 ms and is a long press at 600 ms; a move past the slop is a
//! drag along the axis it started on (a vertical drag always wins over the
//! long press); a horizontal drag from the left edge is Back when it
//! travels far or fast enough. X core events carry one pointer, so there is
//! one touch at a time.
//!
//! Pure: fed positions and times (`down`, `moved`, `up`, `tick`), it returns
//! what they mean, so every rule is tested with synthetic timings.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A move this far from where the finger came down starts a drag (and
/// cancels a tap or a long press).
pub const SLOP: f32 = 32.0;
/// Held still this long: the pressed thing lifts (long-press feedback).
pub const LIFT_AFTER: Duration = Duration::from_millis(150);
/// Held still this long: a long press.
pub const LONG_PRESS_AFTER: Duration = Duration::from_millis(600);
/// A swipe starting this close to the left edge can be Back.
pub const EDGE: f32 = 60.0;
/// Back needs this share of the width…
pub const BACK_TRAVEL: f32 = 0.3;
/// …or a flick this fast (px/s), mostly horizontal.
pub const BACK_FLICK: f32 = 1500.0;
/// Velocity is measured over the last this long of a drag.
const VELOCITY_WINDOW: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// Where a page swipe goes (PAR-001: Settings | List | Chat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Right: the page to the left (Back).
    Back,
    /// Left: the page to the right.
    Forward,
}

/// A horizontal drag nothing inside the page took, ended `(dx, dy)` from
/// where it started at `(vx, vy)` px/s: a page swipe when it is mostly
/// horizontal and went a third of the width or was flicked, as the edge
/// swipe is.
pub fn page_swipe(dx: f32, dy: f32, vx: f32, vy: f32, width: f32) -> Option<Page> {
    let horizontal = dx.abs() > dy.abs() * 1.5;
    let far = dx.abs() > width * BACK_TRAVEL;
    let fast = vx.abs() > BACK_FLICK && vx.abs() > vy.abs() * 1.5 && vx * dx > 0.0;
    (horizontal && (far || fast)).then_some(if dx > 0.0 { Page::Back } else { Page::Forward })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// A finger came down: highlight what it is on.
    Down { x: f32, y: f32 },
    /// Held still 150 ms: lift what it is on.
    Lift { x: f32, y: f32 },
    /// Held still 600 ms: a long press (open a menu; medium haptic). The
    /// finger is still down.
    LongPress { x: f32, y: f32 },
    /// After a long press, the finger moved (dragging what it pressed).
    LongPressMove { x: f32, y: f32 },
    /// Let go after a long press.
    LongPressEnd { x: f32, y: f32 },
    /// The finger moved past the slop: a drag begins (whatever was pressed
    /// is let go). `edge`: a horizontal drag from the left edge, which is
    /// Back or nothing at its end, never a scroll.
    DragStart {
        axis: Axis,
        x: f32,
        y: f32,
        start_x: f32,
        start_y: f32,
        edge: bool,
    },
    /// The finger moved while dragging.
    Drag { axis: Axis, x: f32, y: f32 },
    /// Let go while dragging; velocity in px/s (positive down and right).
    DragEnd {
        axis: Axis,
        x: f32,
        y: f32,
        vx: f32,
        vy: f32,
    },
    /// Let go without moving, before a long press.
    Tap { x: f32, y: f32 },
    /// A left-edge swipe that went far or fast enough: Back.
    Back,
    /// The touch ended as nothing (an edge swipe too short, a cancel).
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// Down, not moved past the slop.
    Pressed {
        lifted: bool,
    },
    Dragging(Axis),
    EdgeSwipe,
    LongPressed,
}

#[derive(Debug, Clone)]
struct Touch {
    start: (f32, f32),
    at: Instant,
    last: (f32, f32),
    samples: VecDeque<(Instant, f32, f32)>,
    mode: Mode,
}

impl Touch {
    fn velocity(&self) -> (f32, f32) {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, x0, y0)), Some(&(t1, x1, y1))) if t1 > t0 => {
                let dt = t1.duration_since(t0).as_secs_f32();
                ((x1 - x0) / dt, (y1 - y0) / dt)
            }
            _ => (0.0, 0.0),
        }
    }

    fn sample(&mut self, x: f32, y: f32, now: Instant) {
        self.last = (x, y);
        self.samples.push_back((now, x, y));
        while self
            .samples
            .front()
            .is_some_and(|(at, _, _)| now.saturating_duration_since(*at) > VELOCITY_WINDOW)
            && self.samples.len() > 2
        {
            self.samples.pop_front();
        }
    }
}

/// The touch recognizer for a window `width` px wide.
#[derive(Debug, Clone)]
pub struct Recognizer {
    width: f32,
    touch: Option<Touch>,
}

impl Recognizer {
    pub fn new(width: f32) -> Self {
        Self { width, touch: None }
    }

    pub fn set_width(&mut self, width: f32) {
        self.width = width;
    }

    /// A touch is in progress.
    pub fn active(&self) -> bool {
        self.touch.is_some()
    }

    /// Whether the touch is a drag along `axis` (the app scrolls with it).
    pub fn dragging(&self) -> Option<Axis> {
        match self.touch.as_ref()?.mode {
            Mode::Dragging(axis) => Some(axis),
            _ => None,
        }
    }

    pub fn down(&mut self, x: f32, y: f32, now: Instant) -> Vec<Gesture> {
        // A touch never released (a lost event) ends as nothing.
        let mut out = self.cancel();
        let mut samples = VecDeque::new();
        samples.push_back((now, x, y));
        self.touch = Some(Touch {
            start: (x, y),
            at: now,
            last: (x, y),
            samples,
            mode: Mode::Pressed { lifted: false },
        });
        out.push(Gesture::Down { x, y });
        out
    }

    pub fn moved(&mut self, x: f32, y: f32, now: Instant) -> Vec<Gesture> {
        let mut out = self.tick(now);
        let Some(touch) = self.touch.as_mut() else {
            return out;
        };
        if touch.last == (x, y) {
            return out;
        }
        touch.sample(x, y, now);
        let (dx, dy) = (x - touch.start.0, y - touch.start.1);
        match touch.mode {
            Mode::Pressed { .. } => {
                if dx * dx + dy * dy > SLOP * SLOP {
                    let axis = if dx.abs() > dy.abs() {
                        Axis::Horizontal
                    } else {
                        Axis::Vertical
                    };
                    let edge = axis == Axis::Horizontal && touch.start.0 < EDGE && dx > 0.0;
                    touch.mode = if edge {
                        Mode::EdgeSwipe
                    } else {
                        Mode::Dragging(axis)
                    };
                    out.push(Gesture::DragStart {
                        axis,
                        x,
                        y,
                        start_x: touch.start.0,
                        start_y: touch.start.1,
                        edge,
                    });
                }
            }
            Mode::Dragging(axis) => out.push(Gesture::Drag { axis, x, y }),
            Mode::EdgeSwipe => {}
            Mode::LongPressed => out.push(Gesture::LongPressMove { x, y }),
        }
        out
    }

    pub fn up(&mut self, x: f32, y: f32, now: Instant) -> Vec<Gesture> {
        let mut out = self.moved(x, y, now);
        let Some(touch) = self.touch.take() else {
            return out;
        };
        out.push(match touch.mode {
            Mode::Pressed { .. } => Gesture::Tap { x, y },
            Mode::Dragging(axis) => {
                let (vx, vy) = touch.velocity();
                Gesture::DragEnd { axis, x, y, vx, vy }
            }
            Mode::EdgeSwipe => {
                let (dx, dy) = (x - touch.start.0, y - touch.start.1);
                let (vx, vy) = touch.velocity();
                let horizontal = dx.abs() > dy.abs() * 1.5;
                let far = dx > self.width * BACK_TRAVEL;
                let fast = vx > BACK_FLICK && vx.abs() > vy.abs() * 1.5;
                if horizontal && (far || fast) {
                    Gesture::Back
                } else {
                    Gesture::Cancel
                }
            }
            Mode::LongPressed => Gesture::LongPressEnd { x, y },
        });
        out
    }

    /// Timers: the lift at 150 ms, the long press at 600 ms.
    pub fn tick(&mut self, now: Instant) -> Vec<Gesture> {
        let mut out = Vec::new();
        let Some(touch) = self.touch.as_mut() else {
            return out;
        };
        if let Mode::Pressed { lifted } = touch.mode {
            let held = now.saturating_duration_since(touch.at);
            let (x, y) = touch.start;
            if !lifted && held >= LIFT_AFTER {
                touch.mode = Mode::Pressed { lifted: true };
                out.push(Gesture::Lift { x, y });
            }
            if held >= LONG_PRESS_AFTER {
                touch.mode = Mode::LongPressed;
                out.push(Gesture::LongPress { x, y });
            }
        }
        out
    }

    /// When `tick` next has something to say.
    pub fn deadline(&self) -> Option<Instant> {
        let touch = self.touch.as_ref()?;
        match touch.mode {
            Mode::Pressed { lifted: false } => Some(touch.at + LIFT_AFTER),
            Mode::Pressed { lifted: true } => Some(touch.at + LONG_PRESS_AFTER),
            _ => None,
        }
    }

    /// Ends the touch as nothing (the window went away).
    pub fn cancel(&mut self) -> Vec<Gesture> {
        match self.touch.take() {
            Some(_) => vec![Gesture::Cancel],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_swipe_goes_a_third_of_the_width_or_is_flicked() {
        let w = 1080.0;
        assert_eq!(page_swipe(400.0, 30.0, 300.0, 0.0, w), Some(Page::Back));
        assert_eq!(
            page_swipe(-400.0, 30.0, -300.0, 0.0, w),
            Some(Page::Forward)
        );
        // Short but fast; short and slow; fast the other way.
        assert_eq!(
            page_swipe(-150.0, 10.0, -2400.0, 100.0, w),
            Some(Page::Forward)
        );
        assert_eq!(page_swipe(150.0, 10.0, 800.0, 0.0, w), None);
        assert_eq!(page_swipe(150.0, 10.0, -2400.0, 0.0, w), None);
        // Diagonal is not a page swipe.
        assert_eq!(page_swipe(400.0, 350.0, 3000.0, 2500.0, w), None);
    }

    fn ms(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_quick_touch_is_a_tap() {
        let t = Instant::now();
        let mut r = Recognizer::new(1080.0);
        assert_eq!(
            r.down(500.0, 700.0, t),
            vec![Gesture::Down { x: 500.0, y: 700.0 }]
        );
        assert!(r.moved(505.0, 704.0, ms(t, 40)).is_empty());
        assert_eq!(
            r.up(505.0, 704.0, ms(t, 90)),
            vec![Gesture::Tap { x: 505.0, y: 704.0 }]
        );
        assert!(!r.active());
    }

    #[test]
    fn a_held_press_lifts_at_150_ms_and_is_a_long_press_at_600() {
        let t = Instant::now();
        let mut r = Recognizer::new(1080.0);
        r.down(300.0, 900.0, t);
        assert_eq!(r.deadline(), Some(ms(t, 150)));
        assert!(r.tick(ms(t, 149)).is_empty());
        assert_eq!(
            r.tick(ms(t, 150)),
            vec![Gesture::Lift { x: 300.0, y: 900.0 }]
        );
        assert_eq!(r.deadline(), Some(ms(t, 600)));
        assert!(r.tick(ms(t, 599)).is_empty());
        // A little movement within the slop keeps it.
        assert!(r.moved(310.0, 910.0, ms(t, 500)).is_empty());
        assert_eq!(
            r.tick(ms(t, 600)),
            vec![Gesture::LongPress { x: 300.0, y: 900.0 }]
        );
        assert_eq!(r.deadline(), None);
        assert_eq!(
            r.moved(320.0, 960.0, ms(t, 700)),
            vec![Gesture::LongPressMove { x: 320.0, y: 960.0 }]
        );
        assert_eq!(
            r.up(320.0, 960.0, ms(t, 800)),
            vec![Gesture::LongPressEnd { x: 320.0, y: 960.0 }],
            "no tap after a long press"
        );
    }

    #[test]
    fn a_40_px_move_cancels_the_long_press_and_starts_a_drag() {
        let t = Instant::now();
        let mut r = Recognizer::new(1080.0);
        r.down(300.0, 900.0, t);
        let moved = r.moved(300.0, 940.0, ms(t, 100));
        assert!(matches!(
            moved[..],
            [Gesture::DragStart {
                axis: Axis::Vertical,
                edge: false,
                ..
            }]
        ));
        assert_eq!(r.dragging(), Some(Axis::Vertical));
        assert!(r.tick(ms(t, 700)).is_empty(), "no long press");
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn a_vertical_scroll_wins_over_a_lifted_press() {
        let t = Instant::now();
        let mut r = Recognizer::new(1080.0);
        r.down(500.0, 1200.0, t);
        assert_eq!(r.tick(ms(t, 200)).len(), 1, "lifted");
        let moved = r.moved(505.0, 1150.0, ms(t, 300));
        assert!(matches!(
            moved[..],
            [Gesture::DragStart {
                axis: Axis::Vertical,
                ..
            }]
        ));
        assert!(matches!(
            r.moved(505.0, 1000.0, ms(t, 340))[..],
            [Gesture::Drag {
                axis: Axis::Vertical,
                ..
            }]
        ));
        assert!(r.tick(ms(t, 700)).is_empty());
        let end = r.up(505.0, 900.0, ms(t, 360));
        let Some(Gesture::DragEnd { vy, .. }) = end.last() else {
            panic!("{end:?}")
        };
        assert!(*vy < -1000.0, "flung upward: {vy}");
    }

    #[test]
    fn a_left_edge_swipe_is_back_when_far_or_fast() {
        let t = Instant::now();
        let mut r = Recognizer::new(1080.0);
        // Far.
        r.down(20.0, 1000.0, t);
        let moved = r.moved(80.0, 1005.0, ms(t, 50));
        assert!(matches!(
            moved[..],
            [Gesture::DragStart {
                axis: Axis::Horizontal,
                edge: true,
                ..
            }]
        ));
        assert!(r.moved(300.0, 1010.0, ms(t, 200)).is_empty(), "no scroll");
        assert_eq!(r.up(420.0, 1010.0, ms(t, 400)), vec![Gesture::Back]);
        // Fast: a short flick.
        r.down(20.0, 1000.0, t);
        r.moved(70.0, 1000.0, ms(t, 20));
        assert_eq!(r.up(150.0, 1004.0, ms(t, 60)), vec![Gesture::Back]);
        // Neither: nothing.
        r.down(20.0, 1000.0, t);
        r.moved(70.0, 1000.0, ms(t, 100));
        assert_eq!(r.up(160.0, 1000.0, ms(t, 600)), vec![Gesture::Cancel]);
        // Mostly vertical from the edge: a scroll, not Back.
        r.down(20.0, 1000.0, t);
        assert!(matches!(
            r.moved(30.0, 1100.0, ms(t, 50))[..],
            [Gesture::DragStart {
                axis: Axis::Vertical,
                edge: false,
                ..
            }]
        ));
        // A horizontal drag away from the edge is a horizontal scroll.
        r.down(500.0, 1000.0, t);
        assert!(matches!(
            r.moved(400.0, 1004.0, ms(t, 50))[..],
            [Gesture::DragStart {
                axis: Axis::Horizontal,
                edge: false,
                ..
            }]
        ));
    }

    #[test]
    fn a_new_touch_ends_a_lost_one() {
        let t = Instant::now();
        let mut r = Recognizer::new(1080.0);
        r.down(100.0, 100.0, t);
        assert_eq!(
            r.down(200.0, 200.0, ms(t, 50)),
            vec![Gesture::Cancel, Gesture::Down { x: 200.0, y: 200.0 }]
        );
        assert_eq!(r.cancel(), vec![Gesture::Cancel]);
        assert!(r.cancel().is_empty());
    }
}
