//! Whole-entry pieces: the user's bubbles, notices, thinking rows.

use crate::gfx::{RectF, Rgb};
use crate::text::{self, Weight};
use crate::ui::{BLUE, MARGIN, RED, SECONDARY, SECONDARY_ALPHA, WHITE};

use super::super::markdown::{MdBlock, Span, Style};
use super::super::model::Action;
use super::super::proto::Entry;
use super::W;
use super::markdown_layout::{
    SMALL, SMALL_LINE, lay_md_block, line_width, plain, push_lines, wrap,
};
use super::ops::{Laid, Op};
use super::tools::TOOL_ROW;

pub(crate) const YELLOW: Rgb = 0xffd60a;

// Bubbles.
pub(crate) const BUBBLE_TEXT: f32 = 46.0;
pub(crate) const BUBBLE_LINE: f32 = 60.0;
pub(crate) const BUBBLE_PAD_X: f32 = 36.0;
pub(crate) const BUBBLE_PAD_Y: f32 = 24.0;
pub(crate) const BUBBLE_MAX: f32 = 800.0;
pub(crate) const BUBBLE_RADIUS: f32 = 44.0;

/// A thinking row and, opened, the thinking.
pub(crate) fn lay_thinking(text: &str, key: &str, open: bool) -> Laid {
    let mut laid = Laid::default();
    let width = W - 2.0 * MARGIN;
    laid.ops.push(Op::Text {
        x: MARGIN,
        baseline: TOOL_ROW / 2.0 + 12.0,
        text: "Thinking".into(),
        weight: Weight::Italic,
        size: SMALL,
        color: SECONDARY,
        alpha: SECONDARY_ALPHA,
    });
    laid.ops.push(Op::Chevron {
        x: W - MARGIN - 14.0,
        cy: TOOL_ROW / 2.0,
        open,
    });
    laid.hits.push((
        RectF::new(0.0, 0.0, W, TOOL_ROW),
        Action::ToggleExpanded(key.to_owned()),
    ));
    laid.h = TOOL_ROW;
    if open {
        let spans = vec![Span {
            text: text.trim().to_owned(),
            style: Style::Italic,
        }];
        let lines = wrap(&spans, width - 30.0, SMALL + 2.0);
        let h = push_lines(
            &mut laid.ops,
            &lines,
            MARGIN + 30.0,
            laid.h,
            SMALL + 2.0,
            SMALL_LINE + 4.0,
            Some(SECONDARY),
            SECONDARY_ALPHA,
        );
        laid.ops.push(Op::Rect {
            rect: RectF::new(MARGIN + 4.0, laid.h, 6.0, h),
            radius: 3.0,
            color: SECONDARY,
            alpha: 0.2,
        });
        laid.h += h + 12.0;
    }
    laid
}

pub(crate) fn lay_md_piece(block: &MdBlock) -> Laid {
    let mut laid = Laid::default();
    laid.h = lay_md_block(&mut laid.ops, block, MARGIN, W - 2.0 * MARGIN, 0.0);
    laid
}

pub(crate) fn lay_user(text: &str, alpha: f32, label: Option<&str>) -> Laid {
    let mut laid = Laid::default();
    let lines = wrap(
        &plain(text.trim()),
        BUBBLE_MAX - 2.0 * BUBBLE_PAD_X,
        BUBBLE_TEXT,
    );
    let width = lines
        .iter()
        .map(|line| line_width(line))
        .fold(0.0, f32::max)
        + 2.0 * BUBBLE_PAD_X;
    let width = width.max(2.0 * BUBBLE_RADIUS);
    let h = lines.len().max(1) as f32 * BUBBLE_LINE + 2.0 * BUBBLE_PAD_Y;
    let x = W - MARGIN - width;
    laid.ops.push(Op::Rect {
        rect: RectF::new(x, 0.0, width, h),
        radius: BUBBLE_RADIUS.min(h / 2.0),
        color: BLUE,
        alpha,
    });
    for (index, line) in lines.iter().enumerate() {
        for run in line {
            laid.ops.push(Op::Text {
                x: x + BUBBLE_PAD_X + run.x,
                baseline: BUBBLE_PAD_Y + index as f32 * BUBBLE_LINE + 45.0,
                text: run.text.clone(),
                weight: Weight::Regular,
                size: BUBBLE_TEXT,
                color: WHITE,
                alpha: alpha.max(0.7),
            });
        }
    }
    laid.h = h;
    if let Some(label) = label {
        laid.ops.push(Op::Text {
            x: W - MARGIN - text::measure(label, Weight::SemiBold, 30.0) - 10.0,
            baseline: h + 40.0,
            text: label.to_owned(),
            weight: Weight::SemiBold,
            size: 30.0,
            color: SECONDARY,
            alpha: SECONDARY_ALPHA,
        });
        laid.h += 50.0;
    }
    laid
}

/// A notice: centered small text (errors red, warnings yellow).
pub(crate) fn lay_notice(text: &str, color: Rgb, max_lines: usize) -> Laid {
    let mut laid = Laid::default();
    let mut lines = wrap(&plain(text.trim()), W - 2.0 * MARGIN - 80.0, SMALL - 2.0);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut().and_then(|line| line.last_mut()) {
            last.text.push('…');
        }
    }
    for (index, line) in lines.iter().enumerate() {
        let joined: String = line.iter().map(|run| run.text.as_str()).collect();
        let w = text::measure(&joined, Weight::Regular, SMALL - 2.0);
        laid.ops.push(Op::Text {
            x: (W - w) / 2.0,
            baseline: index as f32 * SMALL_LINE + 36.0,
            text: joined,
            weight: Weight::Regular,
            size: SMALL - 2.0,
            color,
            alpha: if color == SECONDARY {
                SECONDARY_ALPHA
            } else {
                0.95
            },
        });
    }
    laid.h = lines.len() as f32 * SMALL_LINE + 8.0;
    laid
}

pub(crate) fn notice_color(color: Option<&str>) -> Rgb {
    match color {
        Some("error") => RED,
        Some("warning") => YELLOW,
        _ => SECONDARY,
    }
}

/// An entry drawn whole: a bubble, or a gray or colored notice.
pub(crate) fn lay_whole(entry: &Entry) -> Laid {
    match entry {
        Entry::User { text, metadata } => {
            let automated = metadata
                .as_ref()
                .is_some_and(|m| m.automation.is_some() || m.system);
            if automated {
                let first = text.trim().lines().next().unwrap_or_default();
                lay_notice(first, SECONDARY, 2)
            } else {
                lay_user(text, 1.0, None)
            }
        }
        Entry::System { text, color } => lay_notice(text, notice_color(color.as_deref()), 4),
        _ => Laid::default(),
    }
}
