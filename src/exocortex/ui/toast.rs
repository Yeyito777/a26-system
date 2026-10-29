//! Floating overlays (roadmap §3.6.17): a toast at the top that says
//! something for 3.5 s, and the undo snackbar at the bottom with its
//! button for 6 s. They pass on touches that miss them, and fade in and
//! out over their own rows only.

#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::gfx::{Canvas, RectF};

use super::super::anim::{Animator, FRAME, Tween};
use super::super::gesture::Gesture;
use super::super::model::Action;
use super::super::theme::{self, HAIRLINE, S2, T_CAPTION, T_NOTE, TARGET};
use super::overlay::{Body, Cx, Overlay, Response};
use super::widgets;

const IN_FOR: Duration = Duration::from_millis(250);
const OUT_FOR: Duration = Duration::from_millis(200);
const TOAST_FOR: Duration = Duration::from_millis(3500);
const SNACKBAR_FOR: Duration = Duration::from_secs(6);
const SHADOW: f32 = 61.0;

/// When a floating overlay shows, from the first frame it was in, and
/// how far its fade is.
#[derive(Debug, Clone, Copy, Default)]
struct Life {
    opened: Option<Instant>,
    closed: Option<Instant>,
    lasts: Duration,
}

impl Life {
    fn new(lasts: Duration) -> Self {
        Self {
            opened: None,
            closed: None,
            lasts,
        }
    }

    /// When it starts closing by itself.
    fn ends(&self) -> Option<Instant> {
        self.opened.map(|opened| opened + self.lasts)
    }

    fn closed_at(&self, now: Instant) -> Option<Instant> {
        match (self.closed, self.ends()) {
            (Some(closed), _) => Some(closed),
            (None, Some(ends)) if now >= ends => Some(ends),
            _ => None,
        }
    }

    /// 0..1, quantized to 32 steps (frames of one state agree).
    fn shown(&self, now: Instant) -> f32 {
        let value = match (self.opened, self.closed_at(now)) {
            (_, Some(closed)) => 1.0 - Tween::new(closed, OUT_FOR, 0.0, 1.0).value(now),
            (Some(opened), None) => Tween::new(opened, IN_FOR, 0.0, 1.0).value(now),
            (None, None) => 0.0,
        };
        (value * 32.0).round() / 32.0
    }

    fn finished(&self, now: Instant) -> bool {
        self.closed_at(now)
            .is_some_and(|closed| now >= closed + OUT_FOR)
    }

    fn animate(&self, now: Instant, animator: &mut Animator) {
        match (self.opened, self.closed_at(now)) {
            (_, Some(closed)) => animator.follow(&Tween::new(closed, OUT_FOR, 0.0, 1.0), now),
            (Some(opened), None) => {
                let tween = Tween::new(opened, IN_FOR, 0.0, 1.0);
                if tween.done(now) {
                    if let Some(ends) = self.ends() {
                        animator.request(ends);
                    }
                } else {
                    animator.follow(&tween, now);
                }
            }
            (None, None) => animator.request(now + FRAME),
        }
    }
}

/// A toast: a line of text at the top centre (an error in the error
/// colour).
pub struct Toast {
    text: String,
    /// The text as it fits the window.
    shown: String,
    error: bool,
    life: Life,
    rect: RectF,
}

impl Toast {
    pub fn new(text: String, error: bool) -> Self {
        Self {
            shown: text.clone(),
            text,
            error,
            life: Life::new(TOAST_FOR),
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Overlay for Toast {
    fn label(&self) -> String {
        self.text.clone()
    }

    fn modal(&self) -> bool {
        false
    }

    fn layout(&mut self, cx: &Cx) {
        if self.life.opened.is_none() {
            self.life.opened = Some(cx.now);
        }
        let max = cx.width - 68.0;
        self.shown = widgets::fit(&self.text, T_CAPTION, max - 78.0);
        let w = (widgets::measure(&self.shown, T_CAPTION) + 78.0).ceil();
        let h = T_CAPTION.line() + 44.0;
        self.rect = RectF::new(((cx.width - w) / 2.0).round(), 150.0, w, h);
    }

    fn body(&self, cx: &Cx) -> Body {
        let mut key = DefaultHasher::new();
        (
            (self.life.shown(cx.now) * 32.0) as u32,
            &self.text,
            self.error,
        )
            .hash(&mut key);
        Body {
            rows: (
                (self.rect.y - SHADOW).floor() as i32,
                (self.rect.y + self.rect.h + SHADOW).ceil() as i32,
            ),
            solid: None,
            scroller: None,
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        let c = theme::c();
        let alpha = self.life.shown(cx.now);
        if alpha <= 0.0 {
            return;
        }
        widgets::shadow(canvas, self.rect, SHADOW, 0.0, 0.45 * alpha);
        let face = theme::mix(p.surface_background, 95, p.app_background);
        let r = self.rect;
        canvas.fill_rect(r.x as i32, r.y as i32, r.w as i32, r.h as i32, face, alpha);
        let (text_color, border) = if self.error {
            (p.error, p.error)
        } else {
            (p.text, c.toast_border)
        };
        for edge in [
            RectF::new(r.x, r.y, r.w, HAIRLINE),
            RectF::new(r.x, r.y + r.h - HAIRLINE, r.w, HAIRLINE),
            RectF::new(r.x, r.y, HAIRLINE, r.h),
            RectF::new(r.x + r.w - HAIRLINE, r.y, HAIRLINE, r.h),
        ] {
            canvas.fill_rect(
                edge.x as i32,
                edge.y as i32,
                edge.w as i32,
                edge.h as i32,
                border,
                alpha,
            );
        }
        widgets::text(
            canvas,
            &self.shown,
            r.x + 39.0,
            T_CAPTION.baseline(r.y + 22.0),
            T_CAPTION,
            text_color,
            alpha,
        );
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } if self.rect.contains(x, y) => Response::Handled,
            Gesture::Tap { .. } => {
                self.close(cx.now);
                Response::Handled
            }
            Gesture::Down { .. } => Response::Pass,
            _ => Response::Handled,
        }
    }

    fn close(&mut self, now: Instant) {
        if self.life.closed_at(now).is_none() {
            self.life.closed = Some(now);
        }
    }

    fn closing(&self) -> bool {
        self.life.closed.is_some()
    }

    fn finished(&self, now: Instant) -> bool {
        self.life.finished(now)
    }

    fn animate(&self, cx: &Cx, animator: &mut Animator) {
        self.life.animate(cx.now, animator);
    }
}

/// The undo snackbar: a line and a button ("Moved to trash · UNDO") at the
/// bottom, its bottom edge `lift` px above the content's bottom (above the
/// composer on the chat, at the list's bottom).
pub struct Snackbar {
    text: String,
    button: String,
    action: Action,
    lift: f32,
    life: Life,
    rect: RectF,
    pressed: bool,
}

impl Snackbar {
    pub fn new(
        text: impl Into<String>,
        button: impl Into<String>,
        action: Action,
        lift: f32,
    ) -> Self {
        Self {
            text: text.into(),
            button: button.into(),
            action,
            lift,
            life: Life::new(SNACKBAR_FOR),
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
            pressed: false,
        }
    }

    fn button_rect(&self) -> RectF {
        let w = (widgets::measure(&self.button, T_NOTE) + 88.0).max(TARGET);
        RectF::new(self.rect.x + self.rect.w - w, self.rect.y, w, self.rect.h)
    }
}

impl Overlay for Snackbar {
    fn modal(&self) -> bool {
        false
    }

    fn layout(&mut self, cx: &Cx) {
        if self.life.opened.is_none() {
            self.life.opened = Some(cx.now);
        }
        let h = TARGET;
        self.rect = RectF::new(
            34.0,
            (cx.bottom() - S2 - self.lift - h).round(),
            cx.width - 68.0,
            h,
        );
    }

    fn body(&self, cx: &Cx) -> Body {
        let mut key = DefaultHasher::new();
        (
            (self.life.shown(cx.now) * 32.0) as u32,
            &self.text,
            &self.button,
            self.pressed,
        )
            .hash(&mut key);
        Body {
            rows: (
                (self.rect.y - SHADOW).floor() as i32,
                (self.rect.y + self.rect.h + SHADOW).ceil() as i32,
            ),
            solid: None,
            scroller: None,
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        let c = theme::c();
        let alpha = self.life.shown(cx.now);
        if alpha <= 0.0 {
            return;
        }
        widgets::shadow(canvas, self.rect, SHADOW, 0.0, 0.45 * alpha);
        let r = self.rect;
        canvas.fill_rect(
            r.x as i32,
            r.y as i32,
            r.w as i32,
            r.h as i32,
            p.surface_background,
            alpha,
        );
        let button = self.button_rect();
        if self.pressed {
            canvas.fill_rect(
                button.x as i32,
                button.y as i32,
                button.w as i32,
                button.h as i32,
                c.menu_item_pressed,
                alpha,
            );
        }
        for edge in [
            RectF::new(r.x, r.y, r.w, HAIRLINE),
            RectF::new(r.x, r.y + r.h - HAIRLINE, r.w, HAIRLINE),
            RectF::new(r.x, r.y, HAIRLINE, r.h),
            RectF::new(r.x + r.w - HAIRLINE, r.y, HAIRLINE, r.h),
        ] {
            canvas.fill_rect(
                edge.x as i32,
                edge.y as i32,
                edge.w as i32,
                edge.h as i32,
                c.toast_border,
                alpha,
            );
        }
        let text = widgets::fit(&self.text, T_NOTE, button.x - r.x - 44.0);
        let baseline = T_NOTE.baseline(r.y + (r.h - T_NOTE.line()) / 2.0);
        widgets::text(canvas, &text, r.x + 39.0, baseline, T_NOTE, p.text, alpha);
        let label_ty = T_NOTE.weight(crate::text::Weight::MonoBold);
        widgets::text_centered(
            canvas,
            &self.button.to_uppercase(),
            button.x + button.w / 2.0,
            baseline,
            label_ty,
            p.accent,
            alpha,
        );
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } if self.rect.contains(x, y) => {
                self.pressed = self.button_rect().contains(x, y);
                Response::Handled
            }
            Gesture::Down { .. } => Response::Pass,
            Gesture::Tap { x, y } => {
                self.pressed = false;
                if self.button_rect().contains(x, y) {
                    self.close(cx.now);
                    Response::Action(self.action.clone())
                } else {
                    Response::Handled
                }
            }
            _ => {
                self.pressed = false;
                Response::Handled
            }
        }
    }

    fn close(&mut self, now: Instant) {
        if self.life.closed_at(now).is_none() {
            self.life.closed = Some(now);
        }
    }

    fn closing(&self) -> bool {
        self.life.closed.is_some()
    }

    fn finished(&self, now: Instant) -> bool {
        self.life.finished(now)
    }

    fn animate(&self, cx: &Cx, animator: &mut Animator) {
        self.life.animate(cx.now, animator);
    }
}
