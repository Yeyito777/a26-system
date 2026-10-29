//! The composer: its field, wrapped text, caret and send or stop button.

use crate::gfx::{Canvas, Path, RectF};
use crate::text::{self, Weight};
use crate::ui::{BG, BLUE, CELL, MARGIN, SECONDARY, SEPARATOR, TERTIARY_ALPHA, WHITE};

use super::super::model::{Action, Model};
use super::markdown_layout::{Run, line_width, plain, wrap};
use super::{GESTURE_RESERVE, ViewState, W};

// Composer.
pub(crate) const FIELD_TEXT: f32 = 44.0;
pub(crate) const FIELD_LINE: f32 = 58.0;
pub(crate) const FIELD_MIN_H: f32 = 104.0;
pub(crate) const FIELD_MAX_LINES: usize = 6;
pub(crate) const SEND_R: f32 = 48.0;

/// Where the composer, its text and the send button are.
pub struct ComposerGeometry {
    pub field: RectF,
    pub lines: Vec<Vec<Run>>,
    /// The first line shown (the field shows the last few).
    pub first: usize,
    pub send: (f32, f32),
}

pub fn composer(model: &Model, height: f32, keyboard: bool) -> ComposerGeometry {
    let bottom = height - if keyboard { 24.0 } else { GESTURE_RESERVE };
    let field_w = W - 2.0 * MARGIN - 2.0 * SEND_R - 24.0;
    let lines = composer_lines(&model.composer, field_w - 60.0);
    let shown = lines.len().clamp(1, FIELD_MAX_LINES);
    let h = (shown as f32 * FIELD_LINE + 46.0).max(FIELD_MIN_H);
    ComposerGeometry {
        field: RectF::new(MARGIN, bottom - h, field_w, h),
        first: lines.len().saturating_sub(FIELD_MAX_LINES),
        lines,
        send: (W - MARGIN - SEND_R, bottom - FIELD_MIN_H / 2.0),
    }
}

/// The composer's text wrapped, kept until it changes: the field's height
/// (and so the transcript's bounds) is asked for many times a frame.
pub(crate) fn composer_lines(text: &str, width: f32) -> Vec<Vec<Run>> {
    use std::cell::RefCell;
    thread_local! {
        static LAST: RefCell<(String, f32, Vec<Vec<Run>>)> =
            const { RefCell::new((String::new(), 0.0, Vec::new())) };
    }
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        if last.0 != text || last.1 != width || last.2.is_empty() {
            *last = (
                text.to_owned(),
                width,
                wrap(&plain(text), width, FIELD_TEXT),
            );
        }
        last.2.clone()
    })
}

/// The composer: its field, text, caret, and the send or stop button.
pub(crate) fn draw_composer(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    pressed_action: Option<&Action>,
    height: f32,
) {
    let geometry = composer(model, height, view.keyboard);
    canvas.fill_rect(
        0,
        (geometry.field.y - 18.0) as i32,
        W as i32,
        (height - geometry.field.y + 18.0) as i32,
        BG,
        1.0,
    );
    canvas.fill_rrect(
        geometry.field,
        48.0_f32.min(geometry.field.h / 2.0),
        CELL,
        1.0,
    );
    canvas.stroke_rrect(
        geometry.field,
        48.0_f32.min(geometry.field.h / 2.0),
        2.0,
        SEPARATOR,
        1.0,
    );
    let text_x = geometry.field.x + 34.0;
    if model.composer.is_empty() {
        text::draw(
            canvas,
            "Message",
            text_x,
            geometry.field.y + 23.0 + FIELD_LINE * 0.72,
            Weight::Regular,
            FIELD_TEXT,
            SECONDARY,
            TERTIARY_ALPHA,
        );
    }
    let mut caret_at = (text_x, geometry.field.y + 23.0);
    for (index, line) in geometry.lines.iter().skip(geometry.first).enumerate() {
        let line_top = geometry.field.y + 23.0 + index as f32 * FIELD_LINE;
        for run in line {
            text::draw(
                canvas,
                &run.text,
                text_x + run.x,
                line_top + FIELD_LINE * 0.72,
                Weight::Regular,
                FIELD_TEXT,
                WHITE,
                1.0,
            );
        }
        caret_at = (text_x + line_width(line), line_top);
    }
    if view.keyboard {
        canvas.fill_rect(
            caret_at.0 as i32 + 3,
            caret_at.1 as i32 + 8,
            4,
            (FIELD_LINE - 10.0) as i32,
            BLUE,
            1.0,
        );
    }
    let (sx, sy) = geometry.send;
    let typed = !model.composer.trim().is_empty();
    if model.streaming() && !typed {
        let down = pressed_action == Some(&Action::Abort);
        canvas.fill_circle(sx, sy, SEND_R, 0x3a3a3c, if down { 0.6 } else { 1.0 });
        canvas.fill_rrect(
            RectF::new(sx - 16.0, sy - 16.0, 32.0, 32.0),
            6.0,
            WHITE,
            1.0,
        );
    } else {
        let down = pressed_action == Some(&Action::Send);
        let alpha = if !typed || !model.connected {
            0.35
        } else if down {
            0.6
        } else {
            1.0
        };
        canvas.fill_circle(sx, sy, SEND_R, BLUE, alpha);
        let mut arrow = Path::new();
        arrow.stroke_segment((sx, sy + 22.0), (sx, sy - 20.0), 8.0);
        arrow.stroke_polyline(
            &[
                (sx - 18.0, sy - 2.0),
                (sx, sy - 21.0),
                (sx + 18.0, sy - 2.0),
            ],
            8.0,
        );
        canvas.fill_path(&arrow, WHITE, if typed { 1.0 } else { 0.6 });
    }
}
