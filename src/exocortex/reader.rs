//! The reader (PAR-332): a full-screen page over the app showing a text in
//! full, monospaced, with its own top bar (Done, the title, Copy). WP-A's
//! minimal version; WP-C1 adds Find (i/N), word selection and the rest.
//!
//! Open one with `reader::open(&mut model.ui, title, text)` or the
//! `UiAct::Read` action.

#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::comms::Scroll;
use crate::gfx::{Canvas, RectF};

use super::gesture::{Axis, Gesture};
use super::model::{Action, Ui, UiAct};
use super::theme::{self, HAIRLINE, T_NOTE, T_TITLE, TOP_BAR, TRANSCRIPT_PAD};
use super::ui::overlay::{Body, Cx, Overlay, Response, Scroller};
use super::ui::widgets;

/// Opens the reader on `text`.
pub fn open(ui: &mut Ui, title: impl Into<String>, text: impl Into<String>) {
    ui.open(Box::new(Reader::new(title.into(), text.into())));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Control {
    Done,
    Copy,
}

pub struct Reader {
    title: String,
    text: String,
    /// The text wrapped at `wrapped_for`.
    lines: Vec<String>,
    wrapped_for: f32,
    scroll: Scroll,
    scrolling: bool,
    pressed: Option<Control>,
    closed: bool,
}

impl Reader {
    pub fn new(title: String, text: String) -> Self {
        Self {
            title,
            text,
            lines: Vec::new(),
            wrapped_for: 0.0,
            scroll: Scroll::default(),
            scrolling: false,
            pressed: None,
            closed: false,
        }
    }

    fn area(&self, cx: &Cx) -> (f32, f32) {
        (TOP_BAR + HAIRLINE, cx.bottom())
    }

    fn content_h(&self) -> f32 {
        self.lines.len() as f32 * T_NOTE.line() + 2.0 * theme::TRANSCRIPT_TOP
    }

    fn max_scroll(&self, cx: &Cx) -> f32 {
        let (y0, y1) = self.area(cx);
        (self.content_h() - (y1 - y0)).max(0.0)
    }

    fn control_at(&self, cx: &Cx, x: f32, y: f32) -> Option<Control> {
        if y >= TOP_BAR {
            return None;
        }
        if x < 260.0 {
            Some(Control::Done)
        } else if x > cx.width - 260.0 {
            Some(Control::Copy)
        } else {
            None
        }
    }
}

impl Overlay for Reader {
    fn label(&self) -> String {
        format!("reader: {}", self.title)
    }

    fn layout(&mut self, cx: &Cx) {
        let width = cx.width - 2.0 * TRANSCRIPT_PAD;
        if self.wrapped_for != width {
            self.lines = widgets::wrap_plain(&self.text, T_NOTE, width);
            self.wrapped_for = width;
        }
    }

    fn body(&self, cx: &Cx) -> Body {
        let (y0, y1) = self.area(cx);
        let mut key = DefaultHasher::new();
        (&self.title, self.text.len(), self.pressed, self.lines.len()).hash(&mut key);
        Body {
            rows: (0, cx.height as i32),
            solid: None,
            scroller: Some(Scroller {
                y0: y0 as i32,
                y1: y1 as i32,
                offset: self.scroll.value(cx.now).round() as i32,
            }),
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        widgets::fill(
            canvas,
            RectF::new(0.0, 0.0, cx.width, cx.height),
            p.app_background,
        );
        let (y0, y1) = self.area(cx);
        let offset = self.scroll.value(cx.now).round();
        widgets::clipped(canvas, RectF::new(0.0, y0, cx.width, y1 - y0), |canvas| {
            let top = y0 + theme::TRANSCRIPT_TOP - offset;
            let line = T_NOTE.line();
            let first = (((y0 - top) / line).floor().max(0.0)) as usize;
            for (index, text) in self.lines.iter().enumerate().skip(first) {
                let y = top + index as f32 * line;
                if y > y1 {
                    break;
                }
                widgets::text(
                    canvas,
                    text,
                    TRANSCRIPT_PAD,
                    T_NOTE.baseline(y),
                    T_NOTE,
                    p.text,
                    1.0,
                );
            }
        });
        widgets::top_bar(canvas, p.app_background);
        let label_ty = T_NOTE.weight(crate::text::Weight::MonoSemiBold);
        let cy = TOP_BAR / 2.0;
        let baseline = label_ty.baseline(cy - label_ty.line() / 2.0);
        let alpha = |control| {
            if self.pressed == Some(control) {
                0.5
            } else {
                1.0
            }
        };
        widgets::text(
            canvas,
            "Done",
            44.0,
            baseline,
            label_ty,
            p.accent,
            alpha(Control::Done),
        );
        widgets::text_right(
            canvas,
            "Copy",
            cx.width - 44.0,
            baseline,
            label_ty,
            p.accent,
            alpha(Control::Copy),
        );
        let title = widgets::fit(&self.title, T_TITLE, cx.width - 2.0 * 260.0);
        widgets::text_centered(
            canvas,
            &title,
            cx.width / 2.0,
            T_TITLE.baseline(cy - T_TITLE.line() / 2.0),
            T_TITLE,
            p.text,
            1.0,
        );
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } => {
                self.pressed = self.control_at(cx, x, y);
                Response::Handled
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                start_y,
                y,
                ..
            } => {
                self.pressed = None;
                self.scrolling = true;
                self.scroll.begin(start_y, cx.now);
                self.scroll.moved(y, self.max_scroll(cx), cx.now);
                Response::Handled
            }
            Gesture::Drag { y, .. } if self.scrolling => {
                self.scroll.moved(y, self.max_scroll(cx), cx.now);
                Response::Handled
            }
            Gesture::DragEnd { .. } if self.scrolling => {
                self.scrolling = false;
                self.scroll.end(self.max_scroll(cx), cx.now);
                Response::Handled
            }
            Gesture::Tap { x, y } => {
                self.pressed = None;
                match self.control_at(cx, x, y) {
                    Some(Control::Done) => Response::Action(Action::Ui(UiAct::Dismiss)),
                    Some(Control::Copy) => {
                        Response::Action(Action::Ui(UiAct::Copy(self.text.clone())))
                    }
                    None => Response::Handled,
                }
            }
            Gesture::Back => Response::Action(Action::Ui(UiAct::Dismiss)),
            _ => {
                self.pressed = None;
                Response::Handled
            }
        }
    }

    fn close(&mut self, _now: Instant) {
        self.closed = true;
    }

    fn closing(&self) -> bool {
        self.closed
    }

    fn finished(&self, _now: Instant) -> bool {
        self.closed
    }

    fn step(&mut self, cx: &Cx) -> bool {
        let max = self.max_scroll(cx);
        self.scroll.step(max, cx.now)
    }
}
