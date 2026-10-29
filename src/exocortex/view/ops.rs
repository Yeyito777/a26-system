//! Laid-out pieces: drawing commands relative to a piece's top, snapped so
//! a moved piece draws the same pixels, and how they are drawn.

use crate::gfx::{Canvas, Path, RectF, Rgb};
use crate::text::{self, Weight};
use crate::ui::{SECONDARY, TERTIARY_ALPHA};

use super::super::model::Action;

/// Drawing commands of a laid-out piece, relative to its top.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    Text {
        x: f32,
        baseline: f32,
        text: String,
        weight: Weight,
        size: f32,
        color: Rgb,
        alpha: f32,
    },
    Rect {
        rect: RectF,
        radius: f32,
        color: Rgb,
        alpha: f32,
    },
    Chevron {
        x: f32,
        cy: f32,
        open: bool,
    },
}

/// Vertical positions on a 1/16 pixel grid: then moving a piece by whole
/// pixels changes no float rounding, so a scrolled frame (rows moved) and a
/// redrawn one agree to the last bit.
pub(crate) fn snap(value: f32) -> f32 {
    (value * 16.0).round() / 16.0
}

pub(crate) fn quantized(mut laid: Laid) -> Laid {
    let mut ink = (0.0_f32, laid.h);
    for op in &mut laid.ops {
        match op {
            Op::Text { baseline, .. } => *baseline = snap(*baseline),
            Op::Rect { rect, .. } => {
                rect.y = snap(rect.y);
                rect.h = snap(rect.h);
            }
            Op::Chevron { cy, .. } => *cy = snap(*cy),
        }
        let (top, bottom) = op_rows(op);
        ink = (ink.0.min(top), ink.1.max(bottom));
    }
    for (rect, _) in &mut laid.hits {
        rect.y = snap(rect.y);
        rect.h = snap(rect.h);
    }
    laid.h = snap(laid.h);
    laid.ink = (ink.0.floor(), ink.1.max(laid.h).ceil());
    laid
}

/// One laid-out transcript piece (a bubble, a markdown block, a tool or
/// thinking row): its height, drawing and tap targets, relative to its top.
#[derive(Debug, Default)]
pub(crate) struct Laid {
    pub(crate) h: f32,
    pub(crate) ops: Vec<Op>,
    pub(crate) hits: Vec<(RectF, Action)>,
    /// Rows its ops may touch (glyphs reach outside `0..h`).
    pub(crate) ink: (f32, f32),
}

/// Rows an op may touch, relative to its piece (generous: glyphs, emoji
/// and anti-aliasing included).
pub(crate) fn op_rows(op: &Op) -> (f32, f32) {
    match op {
        Op::Text { baseline, size, .. } => (baseline - size * 1.4, baseline + size * 0.7),
        Op::Rect { rect, .. } => (rect.y - 2.0, rect.y + rect.h + 2.0),
        Op::Chevron { cy, .. } => (cy - 20.0, cy + 20.0),
    }
}

/// Draws the ops of a piece placed at `dy` that can reach rows `clip`.
pub(crate) fn draw_ops(canvas: &mut Canvas, ops: &[Op], dy: f32, clip: (f32, f32)) {
    for op in ops {
        let (top, bottom) = op_rows(op);
        if top + dy >= clip.1 || bottom + dy <= clip.0 {
            continue;
        }
        match op {
            Op::Text {
                x,
                baseline,
                text: string,
                weight,
                size,
                color,
                alpha,
            } => {
                text::draw(
                    canvas,
                    string,
                    *x,
                    baseline + dy,
                    *weight,
                    *size,
                    *color,
                    *alpha,
                );
            }
            Op::Rect {
                rect,
                radius,
                color,
                alpha,
            } => {
                let r = RectF::new(rect.x, rect.y + dy, rect.w, rect.h);
                if *radius > 0.0 {
                    canvas.fill_rrect(r, *radius, *color, *alpha);
                } else {
                    canvas.fill_rect(
                        r.x as i32,
                        r.y as i32,
                        r.w as i32,
                        r.h.max(1.0) as i32,
                        *color,
                        *alpha,
                    );
                }
            }
            Op::Chevron { x, cy, open } => small_chevron(canvas, *x, cy + dy, *open),
        }
    }
}

pub(crate) fn small_chevron(canvas: &mut Canvas, x: f32, cy: f32, open: bool) {
    let mut path = Path::new();
    if open {
        path.stroke_polyline(
            &[(x - 14.0, cy - 7.0), (x, cy + 7.0), (x + 14.0, cy - 7.0)],
            5.0,
        );
    } else {
        path.stroke_polyline(
            &[(x - 7.0, cy - 14.0), (x + 7.0, cy), (x - 7.0, cy + 14.0)],
            5.0,
        );
    }
    canvas.fill_path(&path, SECONDARY, TERTIARY_ALPHA);
}
