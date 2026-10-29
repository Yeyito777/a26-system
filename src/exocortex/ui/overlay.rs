//! Overlays (roadmap §3.6.17): what floats over a page. Modal ones (sheets,
//! action lists, the anchored menu, dialogs, prompts, the reader) take
//! every touch while open and dim the page; floating ones (toasts, the undo
//! snackbar) sit on top and pass on touches that miss them.
//!
//! The model asks for overlays (`model::Ui::open`, `toast`, …); the view
//! keeps them (`ViewState::overlays`) and draws them over the page. Each
//! describes its body per frame (`Body`): which rows it covers, which of
//! them are full-width and opaque (moved whole when it slides) and a key of
//! everything else, so an animation frame redraws only the rows that
//! changed and equals a full redraw (view/mod.rs, tested).
//!
//! The generic overlays are in `ui/sheet.rs` (sheet, action list, prompt),
//! `ui/menu.rs` (anchored menu, confirm dialog) and `ui/toast.rs` (toast,
//! snackbar); the reader is `reader.rs`.

#![allow(dead_code)]

use std::time::Instant;

use crate::control::KeyboardPurpose;
use crate::gfx::Canvas;
use crate::keys::KeyInput;

use super::super::anim::Animator;
use super::super::gesture::Gesture;
use super::super::model::{Action, Model};

/// What an overlay sees: the model, the window and the time.
pub struct Cx<'a> {
    pub model: &'a Model,
    pub width: f32,
    /// The window's height (Moon shrinks it above its keyboard).
    pub height: f32,
    /// Moon's keyboard is up.
    pub keyboard: bool,
    pub now: Instant,
}

impl Cx<'_> {
    /// Where content may go down to: above Moon's home gesture, or the
    /// window's bottom with the keyboard up.
    pub fn bottom(&self) -> f32 {
        if self.keyboard {
            self.height
        } else {
            self.height - super::super::theme::GESTURE_RESERVE
        }
    }
}

/// An overlay's answer to a gesture.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// Not on it: what is under it gets the gesture (floating overlays).
    Pass,
    /// Taken (a press, a scroll); draw again.
    Handled,
    /// Taken: perform this.
    Action(Action),
}

/// An overlay's body in one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// Rows it draws in (its shadow included).
    pub rows: (i32, i32),
    /// Rows that are full-width and opaque and only move with it (a sheet's
    /// panel): a frame where they only moved moves them instead of drawing
    /// them.
    pub solid: Option<(i32, i32)>,
    /// Rows whose content scrolls by `offset` px (a sheet's or the reader's
    /// content): a frame where only the offset changed moves them.
    pub scroller: Option<Scroller>,
    /// Everything else its pixels depend on (content, pressed item,
    /// opacity, scale): equal keys and equal geometry mean equal pixels.
    pub key: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroller {
    pub y0: i32,
    pub y1: i32,
    pub offset: i32,
}

impl Body {
    /// No rows: an overlay that draws nothing this frame.
    pub fn empty() -> Self {
        Self {
            rows: (0, 0),
            solid: None,
            scroller: None,
            key: 0,
        }
    }
}

pub trait Overlay {
    /// What it is, in words (tests, logs): a toast's text, a sheet's title.
    fn label(&self) -> String {
        String::new()
    }

    /// Closes when the open conversation changes (PAR-015).
    fn scoped(&self) -> bool {
        false
    }

    /// Modal: takes every touch and dims what is under it (a floating one
    /// passes touches that miss it).
    fn modal(&self) -> bool {
        true
    }

    /// Lays itself out for this frame (cheap when nothing changed).
    fn layout(&mut self, _cx: &Cx) {}

    /// What stays still while the body animates (the dim, a cut-out, a
    /// shadow), drawn under the body.
    fn draw_backdrop(&self, _canvas: &mut Canvas, _cx: &Cx) {}

    /// The body as it is at `cx.now`.
    fn body(&self, cx: &Cx) -> Body;

    /// Draws the body as it is at `cx.now` (the canvas is clipped to the
    /// rows being drawn).
    fn draw(&self, canvas: &mut Canvas, cx: &Cx);

    /// A gesture, while on top (or, floating, while it might be on it).
    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response;

    /// A key from Moon's keyboard, while on top.
    fn key(&mut self, _input: &KeyInput, _cx: &Cx) -> Response {
        Response::Pass
    }

    /// Moon's keyboard, wanted while on top (a prompt's field).
    fn keyboard(&self) -> Option<KeyboardPurpose> {
        None
    }

    /// Starts its exit animation.
    fn close(&mut self, now: Instant);

    /// Closing, its exit animation not over yet.
    fn closing(&self) -> bool;

    /// Closed and its exit animation over: removed.
    fn finished(&self, now: Instant) -> bool;

    /// Asks for the frames its animation needs (and one at a deadline, as
    /// a toast's end).
    fn animate(&self, _cx: &Cx, _animator: &mut Animator) {}

    /// Advances a fling of its content; true while it moves.
    fn step(&mut self, _cx: &Cx) -> bool {
        false
    }

    /// The image previews it shows, most wanted first (the viewer, C1):
    /// fetched while it is open.
    fn images(&self, _cx: &Cx) -> Vec<super::super::images::Want> {
        Vec::new()
    }
}

/// Overlays' ids and bodies, bottom up.
pub type Bodies = Vec<(u64, Body)>;

/// A shown overlay and its identity (a frame's record names overlays by
/// it: a different one is a different picture).
struct Shown {
    id: u64,
    overlay: Box<dyn Overlay>,
}

/// Where a touch went when it came down: the rest of it goes there too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    Overlay(u64),
    Page,
}

/// The overlays on screen: modal ones in the order they opened, floating
/// ones above them all.
#[derive(Default)]
pub struct OverlayStack {
    modal: Vec<Shown>,
    floating: Vec<Shown>,
    next: u64,
    capture: Option<Capture>,
}

impl OverlayStack {
    pub fn push(&mut self, overlay: Box<dyn Overlay>) {
        self.next += 1;
        let shown = Shown {
            id: self.next,
            overlay,
        };
        if shown.overlay.modal() {
            self.modal.push(shown);
        } else {
            self.floating.push(shown);
        }
    }

    /// A modal overlay is open (not closing): the page takes no touches.
    pub fn blocking(&self) -> bool {
        self.modal.iter().any(|shown| !shown.overlay.closing())
    }

    pub fn is_empty(&self) -> bool {
        self.modal.is_empty() && self.floating.is_empty()
    }

    /// A modal overlay is shown, closing ones included (a row stays
    /// lifted until its menu has faded).
    pub fn has_modal(&self) -> bool {
        !self.modal.is_empty()
    }

    pub fn len(&self) -> usize {
        self.modal.len() + self.floating.len()
    }

    /// The topmost modal overlay that is not closing.
    pub fn top(&self) -> Option<&dyn Overlay> {
        self.modal
            .iter()
            .rev()
            .find(|shown| !shown.overlay.closing())
            .map(|shown| shown.overlay.as_ref())
    }

    fn top_index(&self) -> Option<usize> {
        self.modal
            .iter()
            .rposition(|shown| !shown.overlay.closing())
    }

    /// Closes the topmost modal overlay; false when there is none.
    pub fn close_top(&mut self, now: Instant) -> bool {
        match self.top_index() {
            Some(index) => {
                self.modal[index].overlay.close(now);
                true
            }
            None => false,
        }
    }

    /// Closes every modal overlay (leaving a page, PAR-004).
    pub fn close_all(&mut self, now: Instant) {
        for shown in &mut self.modal {
            if !shown.overlay.closing() {
                shown.overlay.close(now);
            }
        }
    }

    /// Closes the conversation-scoped ones (PAR-015).
    pub fn close_scoped(&mut self, now: Instant) {
        for shown in self.modal.iter_mut().chain(&mut self.floating) {
            if shown.overlay.scoped() && !shown.overlay.closing() {
                shown.overlay.close(now);
            }
        }
    }

    /// Drops the ones whose exit animation is over; whether any went.
    pub fn sweep(&mut self, now: Instant) -> bool {
        let before = self.len();
        self.modal.retain(|shown| !shown.overlay.finished(now));
        self.floating.retain(|shown| !shown.overlay.finished(now));
        before != self.len()
    }

    pub fn layout(&mut self, cx: &Cx) {
        for shown in self.modal.iter_mut().chain(&mut self.floating) {
            shown.overlay.layout(cx);
        }
    }

    pub fn animate(&self, cx: &Cx, animator: &mut Animator) {
        for shown in self.modal.iter().chain(&self.floating) {
            shown.overlay.animate(cx, animator);
        }
    }

    /// Advances flings; true while any moves.
    pub fn step(&mut self, cx: &Cx) -> bool {
        let mut moving = false;
        for shown in self.modal.iter_mut().chain(&mut self.floating) {
            moving |= shown.overlay.step(cx);
        }
        moving
    }

    /// The image previews the overlays show, the topmost's first.
    pub fn images(&self, cx: &Cx) -> Vec<super::super::images::Want> {
        self.modal
            .iter()
            .rev()
            .filter(|shown| !shown.overlay.closing())
            .flat_map(|shown| shown.overlay.images(cx))
            .collect()
    }

    /// The modal overlays' and the floating ones' ids and bodies, bottom up.
    pub fn bodies(&self, cx: &Cx) -> (Bodies, Bodies) {
        let body = |shown: &Shown| (shown.id, shown.overlay.body(cx));
        (
            self.modal.iter().map(body).collect(),
            self.floating.iter().map(body).collect(),
        )
    }

    /// Draws the modal overlays (backdrop, then body, each in turn).
    pub fn draw_modal(&self, canvas: &mut Canvas, cx: &Cx) {
        for shown in &self.modal {
            shown.overlay.draw_backdrop(canvas, cx);
            shown.overlay.draw(canvas, cx);
        }
    }

    /// Draws the floating overlays.
    pub fn draw_floating(&self, canvas: &mut Canvas, cx: &Cx) {
        for shown in &self.floating {
            shown.overlay.draw_backdrop(canvas, cx);
            shown.overlay.draw(canvas, cx);
        }
    }

    /// Offers a gesture: floating overlays it lands on, then the top modal
    /// one (which takes everything). `Pass`: it is the page's.
    pub fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        if let Gesture::Down { .. } = gesture {
            self.capture = None;
            // Floating ones first, topmost first.
            for shown in self.floating.iter_mut().rev() {
                if shown.overlay.closing() {
                    continue;
                }
                let response = shown.overlay.gesture(gesture, cx);
                if response != Response::Pass {
                    self.capture = Some(Capture::Overlay(shown.id));
                    return response;
                }
            }
            if let Some(index) = self.top_index() {
                let shown = &mut self.modal[index];
                self.capture = Some(Capture::Overlay(shown.id));
                let response = shown.overlay.gesture(gesture, cx);
                return if response == Response::Pass {
                    Response::Handled
                } else {
                    response
                };
            }
            self.capture = Some(Capture::Page);
            return Response::Pass;
        }
        let target = match (gesture, self.capture) {
            // Back goes to the top modal overlay, whatever the touch began on.
            (Gesture::Back, _) => match self.top_index() {
                Some(index) => Some(self.modal[index].id),
                None => return Response::Pass,
            },
            (_, Some(Capture::Overlay(id))) => Some(id),
            (_, Some(Capture::Page)) | (_, None) => {
                // A modal overlay opened mid-touch takes nothing of it, but
                // the page does not either.
                return if self.blocking() {
                    Response::Handled
                } else {
                    Response::Pass
                };
            }
        };
        if matches!(
            gesture,
            Gesture::Tap { .. }
                | Gesture::DragEnd { .. }
                | Gesture::LongPressEnd { .. }
                | Gesture::Back
                | Gesture::Cancel
        ) {
            self.capture = None;
        }
        let Some(target) = target else {
            return Response::Pass;
        };
        match self
            .modal
            .iter_mut()
            .chain(&mut self.floating)
            .find(|shown| shown.id == target)
        {
            Some(shown) => {
                let response = shown.overlay.gesture(gesture, cx);
                if response == Response::Pass && shown.overlay.modal() {
                    Response::Handled
                } else {
                    response
                }
            }
            // Gone meanwhile.
            None => Response::Handled,
        }
    }

    /// Offers a key to the top modal overlay.
    pub fn key(&mut self, input: &KeyInput, cx: &Cx) -> Response {
        match self.top_index() {
            Some(index) => self.modal[index].overlay.key(input, cx),
            None => Response::Pass,
        }
    }

    /// The keyboard the top modal overlay wants.
    pub fn keyboard(&self) -> Option<KeyboardPurpose> {
        self.top().and_then(|overlay| overlay.keyboard())
    }
}
