//! Whole-entry pieces (WP-C2): the user's bubbles, notices, thinking rows,
//! in Whale's colours (their redesign, §3.6.6–3.6.11, is C2's).

use crate::gfx::{RectF, Rgb};
use crate::text::Weight;

use super::super::markdown::{MdBlock, Span, Style};
use super::super::model::{Action, chat};
use super::super::proto::Entry;
use super::super::theme::{self, BUBBLE_PAD_X, BUBBLE_PAD_Y, HAIRLINE, RAIL, TRANSCRIPT_PAD};
use super::W;
use super::markdown_layout::{
    SMALL, SMALL_LINE, lay_md_block, line_width, plain, push_lines, wrap,
};
use super::ops::{self, Laid, Op};
use super::tools::TOOL_ROW;

// Bubbles.
pub(crate) const BUBBLE_TEXT: f32 = 40.0;
pub(crate) const BUBBLE_LINE: f32 = 60.0;
pub(crate) const BUBBLE_MAX: f32 = W - 2.0 * TRANSCRIPT_PAD - theme::BUBBLE_GAP;

const MARGIN: f32 = TRANSCRIPT_PAD;

/// A thinking row and, opened, the thinking.
pub(crate) fn lay_thinking(text: &str, key: &str, open: bool) -> Laid {
    let p = theme::p();
    let c = theme::c();
    let mut laid = Laid::default();
    let width = W - 2.0 * MARGIN;
    let label = theme::T_EYEBROW;
    laid.ops.push(Op::Text {
        x: MARGIN,
        baseline: TOOL_ROW / 2.0 + label.px() * 0.37,
        text: "THINKING".into(),
        weight: label.weight,
        size: label.px(),
        tracking: label.tracking(),
        color: p.muted,
        alpha: 1.0,
    });
    laid.ops.push(Op::Chevron {
        x: W - MARGIN - 14.0,
        cy: TOOL_ROW / 2.0,
        open,
    });
    laid.hits.push((
        RectF::new(0.0, 0.0, W, TOOL_ROW),
        Action::Chat(chat::Act::ToggleExpanded(key.to_owned())),
    ));
    laid.h = TOOL_ROW;
    if open {
        let spans = vec![Span {
            text: text.trim().to_owned(),
            style: Style::Italic,
        }];
        let size = theme::T_NOTE.px();
        let lines = wrap(&spans, width - 30.0, size);
        let h = push_lines(
            &mut laid.ops,
            &lines,
            MARGIN + 30.0,
            laid.h,
            size,
            theme::T_NOTE.line(),
            Some(c.thinking_text),
            1.0,
        );
        laid.ops
            .push(ops::rect(RectF::new(MARGIN, laid.h, RAIL, h), c.rule_box));
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
    let p = theme::p();
    let c = theme::c();
    let mut laid = Laid::default();
    let lines = wrap(
        &plain(text.trim()),
        BUBBLE_MAX - 2.0 * BUBBLE_PAD_X - RAIL,
        BUBBLE_TEXT,
    );
    let width = (lines
        .iter()
        .map(|line| line_width(line))
        .fold(0.0, f32::max)
        + 2.0 * BUBBLE_PAD_X
        + RAIL)
        .ceil();
    let h = lines.len().max(1) as f32 * BUBBLE_LINE + 2.0 * BUBBLE_PAD_Y;
    let x = (W - MARGIN - width).round();
    let bubble = RectF::new(x, 0.0, width, h);
    laid.ops.push(Op::Rect {
        rect: bubble,
        radius: 0.0,
        color: p.user_message_background,
        alpha,
    });
    if alpha >= 1.0 {
        laid.ops.push(Op::Border {
            rect: bubble,
            width: HAIRLINE,
            color: c.bubble_border,
        });
    }
    laid.ops.push(Op::Rect {
        rect: RectF::new(x, 0.0, RAIL, h),
        radius: 0.0,
        color: if alpha >= 1.0 { p.accent } else { p.muted },
        alpha,
    });
    for (index, line) in lines.iter().enumerate() {
        for run in line {
            laid.ops.push(Op::Text {
                x: x + RAIL + BUBBLE_PAD_X + run.x,
                baseline: BUBBLE_PAD_Y
                    + index as f32 * BUBBLE_LINE
                    + (BUBBLE_LINE + BUBBLE_TEXT * 0.74) / 2.0,
                text: run.text.clone(),
                weight: Weight::Mono,
                size: BUBBLE_TEXT,
                tracking: 0.0,
                color: p.text,
                alpha: alpha.max(0.58),
            });
        }
    }
    laid.h = h;
    if let Some(label) = label {
        let ty = theme::T_META.weight(theme::T_ITALIC);
        let w = crate::text::measure(label, ty.weight, ty.px());
        laid.ops.push(ops::text(
            W - MARGIN - w,
            h + 17.0 + ty.px() * 0.9,
            label,
            ty.weight,
            ty.px(),
            p.muted,
        ));
        laid.h += 17.0 + ty.line();
    }
    laid
}

/// A notice: centred small text (errors in the error colour, warnings in
/// the warning colour).
pub(crate) fn lay_notice(text: &str, color: Rgb, max_lines: usize) -> Laid {
    let mut laid = Laid::default();
    let mut lines = wrap(&plain(text.trim()), W - 2.0 * MARGIN - 80.0, SMALL);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut().and_then(|line| line.last_mut()) {
            last.text.push('…');
        }
    }
    for (index, line) in lines.iter().enumerate() {
        let joined: String = line.iter().map(|run| run.text.as_str()).collect();
        let w = crate::text::measure(&joined, Weight::Mono, SMALL);
        laid.ops.push(ops::text(
            ((W - w) / 2.0).round(),
            index as f32 * SMALL_LINE + 36.0,
            joined,
            Weight::Mono,
            SMALL,
            color,
        ));
    }
    laid.h = lines.len() as f32 * SMALL_LINE + 8.0;
    laid
}

pub(crate) fn notice_color(color: Option<&str>) -> Rgb {
    let p = theme::p();
    match color {
        Some("error") => p.error,
        Some("warning") => p.warning,
        Some("success") => p.success,
        Some(other) => theme::parse_hex(other).unwrap_or(p.muted),
        None => p.muted,
    }
}

/// An entry drawn whole: a bubble, or a notice.
pub(crate) fn lay_whole(entry: &Entry) -> Laid {
    match entry {
        Entry::User { text, metadata, .. } => {
            let automated = metadata
                .as_ref()
                .is_some_and(|m| m.automation.is_some() || m.system);
            if automated {
                let first = text.trim().lines().next().unwrap_or_default();
                lay_notice(first, theme::p().muted, 2)
            } else {
                lay_user(text, 1.0, None)
            }
        }
        Entry::System { text, color, .. } => lay_notice(text, notice_color(color.as_deref()), 4),
        _ => Laid::default(),
    }
}
