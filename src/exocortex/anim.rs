//! Motion (roadmap §3.7): the curves, animations as pure functions of time,
//! and the `Animator` through which anything on screen that moves asks the
//! event loop for its next frame. When nothing asks, the loop sleeps.
//!
//! An animation's look at a time depends on that time alone, so a frame
//! drawn in part and the same frame drawn whole agree.

#![allow(dead_code)]

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// `1 − (1 − t)^5`: the iPhone's `cubic-bezier(0.22, 1, 0.36, 1)`.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(5)
}

/// The usual cubic ease-in-out.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Where a repeating motion of `period_ms` is at `now`, 0..1, the same for
/// everything on screen (spinners turn together).
pub fn phase(now: Instant, period_ms: u64) -> f32 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    let ms = now.saturating_duration_since(epoch).as_millis() as u64;
    (ms % period_ms.max(1)) as f32 / period_ms.max(1) as f32
}

/// The streaming and activity pulse: opacity 100 % → 45 % → 100 % over
/// `period_ms` (ease-in-out), in whole percent and quantized to 30 steps a
/// second, so a pulsing dot redraws at 30 Hz and its frame key
/// (`pulse_percent`) names exactly what it draws.
pub fn pulse(now: Instant, period_ms: u64) -> f32 {
    pulse_percent(now, period_ms) as f32 / 100.0
}

pub fn pulse_percent(now: Instant, period_ms: u64) -> u32 {
    let t = phase(now, period_ms);
    let steps = (period_ms as f32 / 1000.0 * 30.0).max(1.0);
    let t = (t * steps).floor() / steps;
    let half = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
    ((1.0 - 0.55 * ease_in_out(half)) * 100.0).round() as u32
}

/// The shimmer band's position across a label, 0..1, per §3.7 (1.6 s).
pub fn shimmer(now: Instant) -> f32 {
    phase(now, 1600)
}

/// The caret: on for 530 ms, off for 530 ms, from `since` (it shows at once
/// after typing).
pub fn caret_on(since: Instant, now: Instant) -> bool {
    (now.saturating_duration_since(since).as_millis() / 530) % 2 == 0
}

/// One run of a curve from `from` to `to`.
#[derive(Debug, Clone, Copy)]
pub struct Tween {
    pub start: Instant,
    pub duration: Duration,
    pub from: f32,
    pub to: f32,
    pub curve: fn(f32) -> f32,
}

impl Tween {
    pub fn new(start: Instant, duration: Duration, from: f32, to: f32) -> Self {
        Self {
            start,
            duration,
            from,
            to,
            curve: ease_out,
        }
    }

    pub fn with_curve(mut self, curve: fn(f32) -> f32) -> Self {
        self.curve = curve;
        self
    }

    /// Progress 0..1 at `now`.
    pub fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.saturating_duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32())
            .clamp(0.0, 1.0)
    }

    pub fn value(&self, now: Instant) -> f32 {
        let t = (self.curve)(self.progress(now));
        self.from + (self.to - self.from) * t
    }

    pub fn done(&self, now: Instant) -> bool {
        now >= self.start + self.duration
    }

    /// The tween's end.
    pub fn end(&self) -> Instant {
        self.start + self.duration
    }
}

/// Frames per second animations are drawn at (120 Hz panel).
pub const FRAME: Duration = Duration::from_micros(8333);
/// Spinners, pulses and shimmers redraw at 30 Hz.
pub const SLOW_FRAME: Duration = Duration::from_micros(33_333);

/// Collects, during a frame, when the next one is wanted; the event loop
/// reads it after drawing (`take`) and sleeps until then.
#[derive(Debug, Default)]
pub struct Animator {
    next: Option<Instant>,
}

impl Animator {
    /// Asks for a frame at `at` (the soonest ask wins).
    pub fn request(&mut self, at: Instant) {
        self.next = Some(self.next.map_or(at, |next| next.min(at)));
    }

    /// A frame as soon as the panel can show one.
    pub fn frame(&mut self, now: Instant) {
        self.request(now + FRAME);
    }

    /// A 30 Hz frame (spinners, pulses).
    pub fn slow_frame(&mut self, now: Instant) {
        self.request(now + SLOW_FRAME);
    }

    /// While `tween` runs: a frame; at its end, the last one.
    pub fn follow(&mut self, tween: &Tween, now: Instant) {
        if !tween.done(now) {
            self.request((now + FRAME).min(tween.end()));
        }
    }

    pub fn take(&mut self) -> Option<Instant> {
        self.next.take()
    }

    pub fn peek(&self) -> Option<Instant> {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_and_tweens_end_where_they_should() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.9, "front-loaded");
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
        let start = Instant::now();
        let tween = Tween::new(start, Duration::from_millis(280), 100.0, 0.0);
        assert_eq!(tween.value(start), 100.0);
        assert_eq!(tween.value(start + Duration::from_millis(400)), 0.0);
        assert!(!tween.done(start + Duration::from_millis(100)));
        assert!(tween.done(start + Duration::from_millis(280)));
    }

    #[test]
    fn the_animator_keeps_the_soonest_frame() {
        let now = Instant::now();
        let mut animator = Animator::default();
        assert_eq!(animator.take(), None);
        animator.slow_frame(now);
        animator.frame(now);
        assert_eq!(animator.take(), Some(now + FRAME));
        assert_eq!(animator.take(), None);
        let tween = Tween::new(now, Duration::from_millis(4), 0.0, 1.0);
        animator.follow(&tween, now);
        assert_eq!(animator.take(), Some(now + Duration::from_millis(4)));
        animator.follow(&tween, now + Duration::from_millis(5));
        assert_eq!(animator.take(), None);
    }

    #[test]
    fn pulses_and_carets_repeat() {
        let now = Instant::now();
        let p = pulse(now, 1600);
        assert!((0.45..=1.0).contains(&p));
        let later = now + Duration::from_millis(1234);
        assert_eq!(
            phase(later, 700),
            phase(later + Duration::from_millis(700), 700)
        );
        assert!(caret_on(now, now));
        assert!(!caret_on(now, now + Duration::from_millis(600)));
        assert!(caret_on(now, now + Duration::from_millis(1100)));
    }
}
