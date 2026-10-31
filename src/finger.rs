//! A dragged finger as the eye wants it, on one axis: the touchscreen's
//! reports placed on its own steady clock, resampled at the time each frame
//! is drawn, and smoothed by how fast the finger moves.
//!
//! On the phone a drag's frames were uneven two ways (Exocortex app on
//! Wayland, the user's own drags):
//! - Slowly (under 150 px/s), 45 % of the frames showed no movement: the
//!   touchscreen reports whole pixels, so a slow finger comes in hops of 0,
//!   1 and 2 px.
//! - Faster, 12-16 % of the frames showed two reports' movement after one
//!   that showed none: reports reach the app unevenly (4 to 10 ms apart)
//!   against the panel's steady 8.33 ms.
//!
//! The touchscreen scans at a steady rate (about 8.25 ms), so each report's
//! time is put on that clock (`Clock`: its period and phase, learned from the
//! reports' times, which come late by a varying amount), and each frame shows
//! where the finger was a few milliseconds before it is drawn: between the
//! reports around that moment, or a little past the newest one at the
//! finger's recent speed. How few: as late as reports have lately reached
//! the app (2 ms when they come promptly, up to 8 ms when the phone is busy),
//! so a late one still falls between two frames' moments. A finger also
//! twitches a pixel or two; following that exactly is accurate but looks
//! rough, so the result is smoothed: carried on at the finger's trend and
//! pulled toward where it is by little while it moves slowly and steadily,
//! by more as it speeds up or strays far from the trend. A steady movement
//! is not delayed (the trend carries it: a plain low-pass, a 1€ filter,
//! drew a slow drag 10 ms late), a twitch is spread over a few frames, and
//! a lurch (the touchscreen holds a starting finger back, then catches up
//! 30-40 px at once) is followed.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Instant;

/// How the finger is followed. Measuring:
/// `EXOCORTEX_APP_DRAG=lag=4,predict=8,min=5,beta=0.1,gamma=0.02` (any of
/// them) in place of these.
#[derive(Debug, Clone, Copy)]
struct Tuning {
    /// A frame shows where the finger was at least this long before it is
    /// drawn (ms); longer while reports reach the app late.
    lag: f64,
    /// Past the newest report, the finger is carried on at its speed this
    /// far at most (ms).
    predict: f64,
    /// How hard the drawn finger is pulled toward the finger, as a cutoff
    /// (Hz): at rest, how much it rises per px/s of the trend, and per px/s
    /// of how far the finger is from where the trend put it.
    min_cutoff: f32,
    beta: f32,
    gamma: f32,
}

const TUNING: Tuning = Tuning {
    lag: 2.0,
    predict: 8.0,
    min_cutoff: 5.0,
    beta: 0.1,
    gamma: 0.02,
};

fn tuning() -> Tuning {
    static TUNED: OnceLock<Tuning> = OnceLock::new();
    *TUNED.get_or_init(|| {
        let mut tuning = TUNING;
        for pair in std::env::var("EXOCORTEX_APP_DRAG")
            .unwrap_or_default()
            .split([',', ' '])
        {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let Ok(value) = value.parse::<f32>() else {
                continue;
            };
            match key {
                "lag" => tuning.lag = f64::from(value.max(0.0)),
                "predict" => tuning.predict = f64::from(value.max(0.0)),
                "min" => tuning.min_cutoff = value,
                "beta" => tuning.beta = value,
                "gamma" => tuning.gamma = value,
                _ => {}
            }
        }
        tuning
    })
}

/// Not moved this long (ms) since it last did (or one and a half of the
/// last gap between its moves, and 4 ms, when longer: a slow finger moves a
/// pixel seldom; at most `STILL_MOST`): the finger has stopped there. (The
/// touchscreen goes on reporting a resting finger every scan.)
const STILL_AFTER: f64 = 20.0;
const STILL_MOST: f64 = 50.0;
/// Reports kept (ms): enough to span a frame's lag and a speed's estimate.
const KEEP: f64 = 150.0;
/// The finger's speed past the newest report is measured over this much of
/// its last reports (ms).
const SPEED_SPAN: f64 = 30.0;
/// Closer than this to where the finger rests, the drag is drawn there.
const SETTLE_PX: f32 = 0.25;
/// Once it is still, how fast the drawn finger goes to where it rests (a
/// cutoff, Hz).
const SETTLE_CUTOFF: f32 = 30.0;

/// The touchscreen's scan clock, as its reports' times show it.
#[derive(Debug, Clone, Copy)]
struct Clock {
    /// The newest report's place on the clock (ms since the drag began).
    last: f64,
    /// The scan period (ms).
    period: f64,
}

/// How far a report's time pulls the clock's phase, and its period.
const PHASE_GAIN: f64 = 0.15;
const PERIOD_GAIN: f64 = 0.02;
/// What the scan period may be (ms; the A26's is about 8.25).
const PERIOD_RANGE: (f64, f64) = (6.0, 11.0);
/// The most a frame waits for late reports (ms).
const MAX_LAG: f64 = 8.0;
/// How fast a late report's mark fades, per report (about a second).
const LATE_FADE: f64 = 0.97;

thread_local! {
    /// The scan period, learned across drags.
    static PERIOD: Cell<f64> = const { Cell::new(1000.0 / 120.0) };
    /// How late reports reach the app (ms): the latest lateness, fading.
    static LATE: Cell<f64> = const { Cell::new(0.0) };
}

impl Clock {
    /// Puts a report that came with time `t` (ms) on the clock: the scan it
    /// is from is a whole number of periods after the last one. A time far
    /// off the clock (after a pause, a burst) starts it again there.
    fn place(&mut self, t: f64) -> f64 {
        let steps = ((t - self.last) / self.period).round().max(1.0);
        let expected = self.last + steps * self.period;
        let error = t - expected;
        if steps > 6.0 || error.abs() > 0.6 * self.period {
            self.last = t;
            return t;
        }
        self.last = expected + PHASE_GAIN * error;
        self.period =
            (self.period + PERIOD_GAIN * error / steps).clamp(PERIOD_RANGE.0, PERIOD_RANGE.1);
        self.last
    }
}

#[derive(Debug, Clone)]
pub struct Follow {
    /// When the drag began: times below are ms since.
    began: Instant,
    clock: Clock,
    /// The reports (when on the clock, where), oldest first.
    reports: VecDeque<(f64, f32)>,
    /// When the newest report came (its own time, ms), and the gap before it.
    newest_at: f64,
    gap: f64,
    /// When the finger last moved (ms), and the gap before that move.
    moved_at: f64,
    move_gap: f64,
    /// The last result: when, where, and the trend it is carried on at
    /// (px/s).
    out: (f64, f32, f32),
}

impl Follow {
    /// A finger at `y` since `at` (the drag's start).
    pub fn new(at: Instant, y: f32) -> Self {
        Self {
            began: at,
            clock: Clock {
                last: 0.0,
                period: PERIOD.with(Cell::get),
            },
            reports: VecDeque::from([(0.0, y)]),
            newest_at: 0.0,
            gap: 0.0,
            moved_at: 0.0,
            move_gap: 0.0,
            out: (0.0, y, 0.0),
        }
    }

    fn ms(&self, at: Instant) -> f64 {
        match at.checked_duration_since(self.began) {
            Some(after) => after.as_secs_f64() * 1000.0,
            None => -(self.began.duration_since(at).as_secs_f64() * 1000.0),
        }
    }

    /// A report: the finger at `y` at `at` (the touchscreen's time), taken
    /// in at `arrived`.
    pub fn report(&mut self, at: Instant, y: f32, arrived: Instant) {
        let t = self.ms(at).max(self.newest_at);
        // The same report again (on X, Moon passes each one on twice, a
        // tenth of a millisecond apart): not another scan.
        if y == self.newest() && t - self.newest_at < 0.5 * self.clock.period {
            return;
        }
        let late = arrived.saturating_duration_since(at).as_secs_f64() * 1000.0;
        LATE.with(|mark| mark.set(late.min(50.0).max(mark.get() * LATE_FADE)));
        self.gap = t - self.newest_at;
        self.newest_at = t;
        if y != self.newest() {
            self.move_gap = t - self.moved_at;
            self.moved_at = t;
        }
        let placed = self.clock.place(t);
        PERIOD.with(|period| period.set(self.clock.period));
        self.reports.push_back((placed, y));
        while self.reports.len() > 3 && placed - self.reports[0].0 > KEEP {
            self.reports.pop_front();
        }
    }

    /// The newest report.
    pub fn newest(&self) -> f32 {
        self.reports.back().map_or(self.out.1, |&(_, y)| y)
    }

    /// Where the finger is drawn in a frame drawn at `now`.
    pub fn at(&mut self, now: Instant) -> f32 {
        let tuning = tuning();
        let t = self.ms(now);
        let (last, y, trend) = self.out;
        let dt = ((t - last) / 1000.0) as f32;
        if dt <= 0.0 {
            return y;
        }
        // Still: no trend, and the drawn finger goes to where it rests.
        if self.still(t) {
            let rest = self.newest();
            let mut next = y + alpha(SETTLE_CUTOFF, dt) * (rest - y);
            if (next - rest).abs() < SETTLE_PX {
                next = rest;
            }
            self.out = (t, next, 0.0);
            return next;
        }
        let lag = LATE
            .with(Cell::get)
            .clamp(tuning.lag, tuning.lag.max(MAX_LAG));
        let target = self.resample(t, t - lag);
        // Carried on at the trend, then pulled toward the finger, and the
        // trend corrected by what it missed (an alpha-beta filter, its gains
        // from one cutoff).
        let carried = y + trend * dt;
        let miss = target - carried;
        let cutoff = tuning.min_cutoff + tuning.beta * trend.abs() + tuning.gamma * miss.abs() / dt;
        let pull = alpha(cutoff, dt);
        let next = carried + pull * miss;
        let trend = trend + pull * pull / (2.0 - pull) * miss / dt;
        self.out = (t, next, trend);
        next
    }

    /// Whether the drawn finger rests where the finger does: no more frames
    /// are needed until it moves.
    pub fn settled(&self, now: Instant) -> bool {
        self.still(self.ms(now)) && (self.out.1 - self.newest()).abs() < SETTLE_PX
    }

    /// Not moved for a while (`now` in ms).
    fn still(&self, now: f64) -> bool {
        now - self.moved_at >= (1.5 * self.move_gap + 4.0).clamp(STILL_AFTER, STILL_MOST)
    }

    /// The finger at clock time `t` (ms), `now` being the frame's: between
    /// the reports around it, or carried on a little past the newest at its
    /// recent speed while reports still come.
    fn resample(&self, now: f64, t: f64) -> f32 {
        let Some(&(last_t, last_y)) = self.reports.back() else {
            return self.out.1;
        };
        if t >= last_t {
            if self.still(now) {
                return last_y;
            }
            // No further than the gap before it: where the finger stops,
            // it is overshot little.
            let gap = self
                .reports
                .len()
                .checked_sub(2)
                .map_or(0.0, |i| last_t - self.reports[i].0);
            let ahead = (t - last_t).min(tuning().predict).min(gap);
            return last_y + self.speed() * (ahead / 1000.0) as f32;
        }
        let mut previous = self.reports[0];
        if t <= previous.0 {
            return previous.1;
        }
        for &(at, y) in self.reports.iter().skip(1) {
            if t <= at {
                let span = at - previous.0;
                if span <= 0.0 {
                    return y;
                }
                return previous.1 + (y - previous.1) * ((t - previous.0) / span) as f32;
            }
            previous = (at, y);
        }
        last_y
    }

    /// The finger's speed over its last reports (px/s): the slope of the
    /// line that fits them best.
    fn speed(&self) -> f32 {
        let Some(&(last_t, _)) = self.reports.back() else {
            return 0.0;
        };
        let recent: Vec<(f64, f64)> = self
            .reports
            .iter()
            .rev()
            .take_while(|(t, _)| last_t - t <= SPEED_SPAN)
            .map(|&(t, y)| (t, f64::from(y)))
            .collect();
        if recent.len() < 2 {
            return 0.0;
        }
        let n = recent.len() as f64;
        let (mt, my) = recent
            .iter()
            .fold((0.0, 0.0), |(a, b), &(t, y)| (a + t / n, b + y / n));
        let (num, den) = recent.iter().fold((0.0, 0.0), |(num, den), &(t, y)| {
            (num + (t - mt) * (y - my), den + (t - mt) * (t - mt))
        });
        if den <= 0.0 {
            return 0.0;
        }
        (num / den * 1000.0) as f32
    }
}

/// A low-pass filter's weight for a step of `dt` seconds at `cutoff` Hz.
fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const REFRESH: Duration = Duration::from_micros(8333);
    const SCAN: Duration = Duration::from_micros(8250);

    /// A finger moving at a steady `speed` (px/s), scanned every 8.25 ms,
    /// reported whole pixels with times up to `jitter` ms late and delivered
    /// up to 6 ms late, drawn at the panel's refresh: the frames' steps after
    /// the first half second.
    fn steps(speed: f32, jitter: f64) -> Vec<f32> {
        let start = Instant::now();
        let mut follow = Follow::new(start, 0.0);
        let mut out = Vec::new();
        let mut scan = 1u32;
        let mut previous = 0.0;
        let mut reported = 0.0;
        for frame in 1..=180u32 {
            let now = start + REFRESH * frame;
            loop {
                let at = start + SCAN * scan;
                let late = Duration::from_millis(u64::from((scan * 37) % 7));
                if at + late > now {
                    break;
                }
                let y = (speed * SCAN.as_secs_f32() * scan as f32).round();
                if y != reported {
                    let off = jitter * f64::from((scan * 53) % 5) / 4.0;
                    follow.report(at + Duration::from_secs_f64(off / 1000.0), y, at + late);
                    reported = y;
                }
                scan += 1;
            }
            let y = follow.at(now);
            if frame > 60 {
                out.push(y - previous);
            }
            previous = y;
        }
        out
    }

    /// Each frame moves as far as the finger did over a refresh, give or
    /// take the pixel the touchscreen rounds each report to; a report late
    /// enough makes a frame fall a little short, but none stalls or moves
    /// twice as far.
    #[test]
    fn steady_movement_draws_even_steps() {
        for speed in [60.0_f32, 400.0, 1500.0, 3000.0] {
            for jitter in [0.0, 2.0] {
                let steps = steps(speed, jitter);
                let expected = speed * REFRESH.as_secs_f32();
                let even = steps
                    .iter()
                    .filter(|step| (*step - expected).abs() < expected * 0.08 + 1.0)
                    .count();
                assert!(
                    even * 100 >= steps.len() * 95,
                    "{speed} px/s, jitter {jitter}: {even} of {} even, expected {expected}: {steps:?}",
                    steps.len()
                );
                for step in &steps {
                    assert!(
                        (step - expected).abs() < expected * 0.5 + 1.5,
                        "{speed} px/s, jitter {jitter}: step {step}, expected {expected}: {steps:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_fast_drag_does_not_trail() {
        // 1500 px/s: drawn within 12 px (8 ms) of the finger.
        let start = Instant::now();
        let mut follow = Follow::new(start, 0.0);
        let mut y = 0.0;
        for n in 1..60u32 {
            let at = start + SCAN * n;
            follow.report(
                at,
                (1500.0 * at.duration_since(start).as_secs_f32()).round(),
                at,
            );
            y = follow.at(at + Duration::from_millis(3));
        }
        let finger = follow.newest();
        assert!(finger - y < 12.0, "drawn at {y}, finger at {finger}");
    }

    #[test]
    fn a_twitch_is_smoothed_and_a_rest_is_reached() {
        let start = Instant::now();
        let mut follow = Follow::new(start, 100.0);
        // Resting, then a 3 px twitch, held.
        follow.report(start + SCAN, 100.0, start + SCAN);
        follow.report(start + SCAN * 2, 103.0, start + SCAN * 2);
        let mut drawn = Vec::new();
        for frame in 3..40u32 {
            drawn.push(follow.at(start + REFRESH * frame));
        }
        // No frame jumps the whole twitch at once.
        let biggest = drawn
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(biggest < 2.0, "{drawn:?}");
        assert_eq!(*drawn.last().unwrap(), 103.0);
        assert!(follow.settled(start + REFRESH * 40));
    }

    /// Where the finger stops it is drawn at once, past it by less than a
    /// report's step while the stop is not known yet.
    #[test]
    fn a_stop_is_overshot_little_and_briefly() {
        for (name, steps) in [
            // Slowing down over five reports, as fingers do.
            (
                "slowing",
                vec![4.0_f32; 15]
                    .into_iter()
                    .chain([3.0, 2.0, 1.0, 1.0])
                    .collect(),
            ),
            // Stopping dead from 485 px/s.
            ("dead", vec![4.0_f32; 19]),
        ] {
            let start = Instant::now();
            let mut follow = Follow::new(start, 0.0);
            let mut y = 0.0;
            for (n, step) in steps.iter().enumerate() {
                y += step;
                let at = start + SCAN * (n as u32 + 1);
                follow.report(at, y, at);
            }
            let end = start + SCAN * steps.len() as u32;
            let bound = if name == "dead" { 4.0 } else { 1.5 };
            let mut drawn = 0.0;
            for frame in 1..30u32 {
                drawn = follow.at(end + REFRESH * frame);
                assert!(
                    drawn < y + bound,
                    "{name}, frame {frame}: {drawn}, finger at {y}"
                );
            }
            assert_eq!(drawn, y, "{name}");
        }
    }

    /// A slow steady drag (the gate's: 80 px/s, a report only when the row
    /// changes, on time) is drawn no later than the frame's moment, and the
    /// page, at whole pixels, moves on two refreshes of three: one still
    /// at a time, never a 2 px jump.
    #[test]
    fn a_slow_steady_drag_is_drawn_on_time_and_even() {
        let start = Instant::now();
        let mut follow = Follow::new(start, 0.0);
        let mut row = 0.0;
        let mut scan = 1u32;
        let (mut behind, mut frames) = (0.0, 0);
        let (mut previous, mut was_still) = (None, false);
        for frame in 1..300u32 {
            let now = start + REFRESH * frame + Duration::from_micros(2900);
            loop {
                let at = start + REFRESH * scan;
                if at > now {
                    break;
                }
                let y = (80.0 * at.duration_since(start).as_secs_f32()).round();
                if y != row {
                    follow.report(at, y, at);
                    row = y;
                }
                scan += 1;
            }
            let drawn = follow.at(now);
            if frame < 60 {
                continue;
            }
            let finger = 80.0 * now.duration_since(start).as_secs_f32();
            behind += (finger - drawn) / 80.0 * 1000.0;
            frames += 1;
            let page = (0.3 - drawn).round();
            if let Some(last) = previous {
                let step = (page - last as f32).abs();
                assert!(step < 2.0, "frame {frame}: a {step} px jump");
                assert!(!(was_still && step == 0.0), "frame {frame}: still twice");
                was_still = step == 0.0;
            }
            previous = Some(page as i32);
        }
        let behind = behind / frames as f32;
        assert!(behind < 4.0, "drawn {behind} ms behind on average");
    }

    /// A finger that stops dead while the touchscreen goes on reporting it
    /// every scan (it does, resting) is reached in a few frames.
    #[test]
    fn a_resting_finger_still_reported_is_reached_soon() {
        let start = Instant::now();
        let mut follow = Follow::new(start, 0.0);
        let mut y = 0.0;
        let mut drawn = 0.0;
        for n in 1..60u32 {
            if n <= 30 {
                y += 4.0;
            }
            let at = start + SCAN * n;
            follow.report(at, y, at);
            drawn = follow.at(at + Duration::from_millis(3));
            assert!(drawn < y + 4.0, "scan {n}: drawn at {drawn}, finger at {y}");
            if n >= 30 + 7 {
                assert_eq!(drawn, y, "scan {n}");
            }
        }
        assert_eq!(drawn, y);
    }

    /// Reports that come twice (as Moon passes them on over X) are drawn
    /// as if they came once.
    #[test]
    fn a_report_that_comes_twice_counts_once() {
        let run = |twice: bool| {
            let start = Instant::now();
            let mut follow = Follow::new(start, 0.0);
            let mut drawn = Vec::new();
            for n in 1..80u32 {
                let at = start + SCAN * n;
                let y = (8.0 * n as f32).round();
                follow.report(at, y, at);
                if twice {
                    let again = at + Duration::from_micros(100);
                    follow.report(again, y, again);
                }
                drawn.push(follow.at(start + REFRESH * n + Duration::from_millis(3)));
            }
            drawn
        };
        let (once, twice) = (run(false), run(true));
        for (a, b) in once.iter().zip(&twice).skip(20) {
            assert!((a - b).abs() < 0.5, "{once:?}\n{twice:?}");
        }
    }

    #[test]
    fn the_clock_learns_the_scan_rate_through_late_times() {
        let mut clock = Clock {
            last: 0.0,
            period: 1000.0 / 120.0,
        };
        for n in 1..400 {
            // Scans every 8.25 ms, each time 0 to 3 ms late.
            clock.place(8.25 * f64::from(n) + f64::from((n * 7) % 4));
        }
        assert!((clock.period - 8.25).abs() < 0.05, "{}", clock.period);
    }
}
