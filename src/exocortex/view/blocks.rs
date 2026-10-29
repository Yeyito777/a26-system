//! Transcript pieces other than markdown and tool rows (WP-C2, roadmap
//! §3.6.6–3.6.13): the user's bubbles (own, sent, automated, queued),
//! notification rows, thinking, notices and the compaction divider,
//! `[Context:` text, the metadata footer, the live status lines, the
//! history edge; and the marks: what changes with time inside a piece
//! (spinners, the WORKING shimmer, the live footer's clock), drawn and
//! redrawn alone so a ticking second or a turning spinner costs its own
//! rows only.

use std::time::Instant;

use crate::gfx::{Canvas, RectF, Rgb};
use crate::text::Weight;

use super::super::anim;
use super::super::footer;
use super::super::markdown::{self, MdBlock, Style};
use super::super::model::{Action, Model, chat, rich::Rich};
use super::super::proto::Metadata;
use super::super::theme::{
    self, BUBBLE_PAD_X, BUBBLE_PAD_Y, HAIRLINE, RAIL, T_BODY, T_CAPTION, T_EDGE, T_EYEBROW,
    T_FOOTER, T_META, T_NOTE, T_SMALL, TARGET, TRANSCRIPT_PAD as MARGIN, Type, mix,
};
use super::super::ui::icons::{self, Icon};
use super::super::ui::widgets;
use super::W;
use super::markdown_layout::{Run, lay_block, line_width, plain, wrap};
use super::ops::{Laid, Op};

/// The widest bubble: the row less the 24 pt leading gap.
pub(crate) const BUBBLE_MAX: f32 = W - 2.0 * MARGIN - theme::BUBBLE_GAP;
/// A disclosure's head (thinking, notices): at least 44 pt.
pub(crate) const HEAD: f32 = theme::DISCLOSURE_HEAD;
/// The footer piece: its line centred 47 px below the reply, a 108 px tap
/// target (the iPhone's 44 pt row pulled 5 pt up into the reply).
pub(crate) const FOOTER_H: f32 = 108.0;
const FOOTER_CENTER: f32 = 47.0;
/// What the footer row leaves of the gap to the next row (its 44 pt
/// target reaches 12 pt into it).
pub(crate) const FOOTER_GAP: f32 = theme::ROW_GAP - 33.0;
/// The history edge's band.
pub(crate) const EDGE_H: f32 = 78.0;
/// A live status line, the WORKING row.
pub(crate) const STATUS_H: f32 = 44.0;
const STATUS_SPINNER: f32 = 28.0;

/// A laid-out piece and its marks (relative to its top).
#[derive(Default)]
pub(crate) struct Piece {
    pub laid: Laid,
    pub marks: Vec<Mark>,
}

impl From<Laid> for Piece {
    fn from(laid: Laid) -> Self {
        Self {
            laid,
            marks: Vec::new(),
        }
    }
}

/// Something in a piece that changes with time: rows `y .. y + h` of the
/// piece, drawn after it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Mark {
    pub y: f32,
    pub h: f32,
    pub kind: MarkKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MarkKind {
    /// A spinner centred at (`cx`, `cy`) of the piece, over `background`.
    Spinner {
        cx: f32,
        cy: f32,
        size: f32,
        color: Rgb,
        background: Rgb,
    },
    /// A label the piece draws muted, with a bright band moving across it.
    Shimmer {
        x: f32,
        baseline: f32,
        text: String,
        ty: Type,
    },
    /// The live reply's footer line, its duration ticking: the run's
    /// metadata before the live reply (`base`) and the live reply's.
    LiveFooter {
        baseline: f32,
        base: Option<Metadata>,
    },
    /// A live status line (PAR-144).
    Status(StatusKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusKind {
    Compaction,
    Wait,
    Retry,
}

// ----- Text ---------------------------------------------------------------------

/// A text op in a type token (uppercased if the token is).
pub(crate) fn text_op(x: f32, baseline: f32, text: &str, ty: Type, color: Rgb) -> Op {
    Op::Text {
        x,
        baseline,
        text: widgets::shown(text, &ty).into_owned(),
        weight: ty.weight,
        size: ty.px(),
        tracking: ty.tracking(),
        color,
        alpha: 1.0,
    }
}

/// The face a run is drawn in at `ty`.
fn run_weight(style: Style, ty: Type) -> Weight {
    match style {
        Style::Bold => Weight::MonoBold,
        Style::Italic => theme::T_ITALIC,
        Style::Plain | Style::Code | Style::Link | Style::Strike | Style::Math => ty.weight,
    }
}

/// Pushes wrapped lines from `top` in `ty` (code and links keep their
/// colours); returns the bottom.
pub(crate) fn push_runs(
    ops: &mut Vec<Op>,
    lines: &[Vec<Run>],
    x: f32,
    top: f32,
    ty: Type,
    color: Option<Rgb>,
    alpha: f32,
) -> f32 {
    let p = theme::p();
    let line_h = ty.line();
    for (index, line) in lines.iter().enumerate() {
        let baseline = ty.baseline(top + index as f32 * line_h);
        for run in line {
            if run.text.is_empty() {
                continue;
            }
            let run_color = match run.style {
                Style::Code => p.secondary_accent,
                Style::Link => p.link,
                _ => color.unwrap_or(p.text),
            };
            ops.push(Op::Text {
                x: x + run.x,
                baseline,
                text: run.text.clone(),
                weight: run_weight(run.style, ty),
                size: ty.px(),
                tracking: 0.0,
                color: run_color,
                alpha,
            });
        }
    }
    top + lines.len() as f32 * line_h
}

/// Plain text wrapped to `width` in `ty`, as lines of runs.
pub(crate) fn wrap_text(text: &str, ty: Type, width: f32) -> Vec<Vec<Run>> {
    wrap(&plain(text), width, ty.px())
}

fn joined(line: &[Run]) -> String {
    line.iter().map(|run| run.text.as_str()).collect()
}

fn icon_op(icon: Icon, cx: f32, cy: f32, size: f32, color: Rgb) -> Op {
    Op::Icon {
        icon,
        cx,
        cy,
        size,
        color,
        alpha: 1.0,
    }
}

fn boxed(ops: &mut Vec<Op>, rect: RectF, fill: Rgb, border: Rgb) {
    ops.push(Op::Rect {
        rect,
        radius: 0.0,
        color: fill,
        alpha: 1.0,
    });
    ops.push(Op::Border {
        rect,
        width: HAIRLINE,
        color: border,
    });
}

// ----- Markdown -------------------------------------------------------------------

/// One block of a reply's markdown (C1 lays it out).
/// A markdown block (C1's `lay_block`); its key must include
/// `markdown_layout::state_key`.
pub(crate) fn lay_md_piece(block: &MdBlock, rich: &Rich) -> Piece {
    lay_block(block, MARGIN, W - 2.0 * MARGIN, Some(rich)).into()
}

/// A text block that starts `[Context:` (PAR-133): plain, dim.
pub(crate) fn lay_context(text: &str) -> Piece {
    let mut laid = Laid::default();
    let lines = wrap_text(
        text.trim_start_matches('\n').trim_end(),
        T_CAPTION,
        W - 2.0 * MARGIN,
    );
    laid.h = push_runs(
        &mut laid.ops,
        &lines,
        MARGIN,
        0.0,
        T_CAPTION,
        Some(theme::p().muted),
        1.0,
    );
    laid.into()
}

// ----- Bubbles --------------------------------------------------------------------

/// How a bubble is drawn (§3.6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tone {
    /// The user's own words.
    Own,
    /// Sent from here, not yet echoed (86 %).
    Sending,
    /// An automated prompt (notification, chrono, delegation).
    Automated,
    /// Waiting in the queue (58 %, a muted rail).
    Queued,
}

/// A user message's bubble, right-aligned and hugging its longest line,
/// with the line under it (why it is there, or how it waits).
pub(crate) fn lay_bubble(text: &str, tone: Tone, label: Option<&str>, gear: bool) -> Piece {
    let p = theme::p();
    let c = theme::c();
    let app = p.app_background;
    let fade = |color: Rgb, percent: u32| mix(color, percent, app);
    let (fill, border, rail, ink, meta) = match tone {
        Tone::Own => (
            p.user_message_background,
            c.bubble_border,
            p.accent,
            p.text,
            p.muted,
        ),
        Tone::Sending => (
            fade(p.user_message_background, 86),
            fade(c.bubble_border, 86),
            fade(p.accent, 86),
            fade(p.text, 86),
            p.muted,
        ),
        Tone::Automated => (
            p.automated_user_background,
            mix(p.accent, 22, p.automated_user_background),
            p.secondary_accent,
            p.text,
            p.muted,
        ),
        Tone::Queued => (
            fade(p.user_message_background, 58),
            fade(c.bubble_border, 58),
            fade(p.muted, 58),
            fade(p.text, 58),
            fade(p.muted, 58),
        ),
    };
    let mut laid = Laid::default();
    let ty = T_BODY;
    let inner = BUBBLE_MAX - 2.0 * HAIRLINE - 2.0 * BUBBLE_PAD_X;
    let trimmed = text.trim();
    let (lines, ink) = if trimmed.is_empty() {
        (wrap_text("(empty message)", ty, inner), p.muted)
    } else {
        (wrap_text(trimmed, ty, inner), ink)
    };
    let widest = lines
        .iter()
        .map(|line| line_width(line))
        .fold(0.0, f32::max);
    let width = (widest + 2.0 * BUBBLE_PAD_X + 2.0 * HAIRLINE)
        .ceil()
        .min(BUBBLE_MAX);
    let h = lines.len().max(1) as f32 * ty.line() + 2.0 * BUBBLE_PAD_Y + 2.0 * HAIRLINE;
    let x = (W - MARGIN - width).round();
    let bubble = RectF::new(x, 0.0, width, h);
    boxed(&mut laid.ops, bubble, fill, border);
    laid.ops.push(Op::Rect {
        rect: RectF::new(x + HAIRLINE, HAIRLINE, RAIL, h - 2.0 * HAIRLINE),
        radius: 0.0,
        color: rail,
        alpha: 1.0,
    });
    // Right-aligned, line by line.
    let right = x + width - HAIRLINE - BUBBLE_PAD_X;
    for (index, line) in lines.iter().enumerate() {
        let shift = right - line_width(line);
        let one = [line.clone()];
        push_runs(
            &mut laid.ops,
            &one,
            shift,
            HAIRLINE + BUBBLE_PAD_Y + index as f32 * ty.line(),
            ty,
            Some(ink),
            1.0,
        );
    }
    laid.h = h;
    if let Some(label) = label {
        let ty = T_META.weight(theme::T_ITALIC);
        let w = widgets::measure(label, ty);
        let top = h + 17.0;
        let right = W - MARGIN;
        laid.ops
            .push(text_op(right - w, ty.baseline(top), label, ty, meta));
        if gear {
            laid.ops.push(icon_op(
                Icon::Gear,
                right - w - 14.0 - 18.0,
                top + ty.line() / 2.0,
                36.0,
                meta,
            ));
        }
        laid.h += 17.0 + ty.line();
    }
    laid.into()
}

/// A mini spinner before a queued bubble's caption while the daemon has not
/// confirmed it (D's `optimistic`).
pub(crate) fn queued_spinner(piece: &mut Piece, caption: &str) {
    let ty = T_META.weight(theme::T_ITALIC);
    let w = widgets::measure(caption, ty);
    let size = 28.0;
    let cy = piece.laid.h - ty.line() / 2.0;
    piece.marks.push(Mark {
        y: cy - size / 2.0 - 2.0,
        h: size + 4.0,
        kind: MarkKind::Spinner {
            cx: W - MARGIN - w - 17.0 - size / 2.0,
            cy,
            size,
            color: theme::p().muted,
            background: theme::p().app_background,
        },
    });
}

/// A notification's folded row (PAR-402): a chevron, its label and the
/// first words of it; opened, the automated bubble follows it.
pub(crate) fn lay_notification(label: &str, preview: &str, key: &str, open: bool) -> Piece {
    let p = theme::p();
    let mut laid = Laid::default();
    let cy = HEAD / 2.0;
    laid.ops.push(icon_op(
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        MARGIN + 16.0,
        cy,
        33.0,
        p.muted,
    ));
    let ty = T_EYEBROW.weight(theme::T_ITALIC);
    let x = MARGIN + 33.0 + 22.0;
    let w = widgets::measure(label, ty);
    laid.ops.push(text_op(
        x,
        ty.baseline(cy - ty.line() / 2.0),
        label,
        ty,
        p.muted,
    ));
    if !open && !preview.is_empty() {
        let px = x + w + 22.0;
        let shown = widgets::fit(preview, T_SMALL, W - MARGIN - px);
        laid.ops.push(text_op(
            px,
            T_SMALL.baseline(cy - T_SMALL.line() / 2.0),
            &shown,
            T_SMALL,
            mix(p.muted, 85, p.app_background),
        ));
    }
    laid.hits.push((
        RectF::new(0.0, 0.0, W, HEAD),
        Action::Chat(chat::Act::ToggleExpanded(key.to_owned())),
    ));
    laid.h = HEAD;
    laid.into()
}

// ----- Thinking -------------------------------------------------------------------

/// Thinking (§3.6.9, PAR-121): a box with THINKING (shimmering while it
/// grows, its newest line beside it when folded); opened, OPEN and the
/// thinking under a rule.
pub(crate) fn lay_thinking(text: &str, key: &str, stamp: u64, open: bool, growing: bool) -> Piece {
    let p = theme::p();
    let c = theme::c();
    let mut piece = Piece::default();
    let x0 = MARGIN;
    let x1 = W - MARGIN;
    let width = x1 - x0;
    let ops = &mut piece.laid.ops;
    ops.push(Op::Rect {
        rect: RectF::new(x0, 0.0, width, 0.0),
        radius: 0.0,
        color: c.thinking_bg,
        alpha: 1.0,
    });
    let cy = HEAD / 2.0;
    ops.push(icon_op(
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        x0 + HAIRLINE + 28.0 + 16.0,
        cy,
        33.0,
        p.muted,
    ));
    let label_x = x0 + HAIRLINE + 28.0 + 33.0 + 22.0;
    let baseline = T_EYEBROW.baseline(cy - T_EYEBROW.line() / 2.0);
    ops.push(text_op(label_x, baseline, "Thinking", T_EYEBROW, p.muted));
    if growing {
        piece.marks.push(Mark {
            y: 0.0,
            h: HEAD,
            kind: MarkKind::Shimmer {
                x: label_x,
                baseline,
                text: "THINKING".into(),
                ty: T_EYEBROW,
            },
        });
    }
    let label_w = widgets::measure("Thinking", T_EYEBROW);
    let mut head_hit = width;
    if open {
        let w = widgets::measure("Open", T_EYEBROW);
        let right = x1 - HAIRLINE - 34.0;
        ops.push(text_op(
            right - w,
            baseline,
            "Open",
            T_EYEBROW,
            p.secondary_accent,
        ));
        let target = (w + 68.0).max(TARGET);
        head_hit = width - target;
        piece.laid.hits.push((
            RectF::new(x1 - target, 0.0, target, HEAD),
            Action::Chat(chat::Act::ReadThinking(stamp, key.to_owned())),
        ));
    } else if growing {
        let last = text
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let px = label_x + label_w + 22.0;
        let shown = widgets::fit(last.trim(), T_SMALL, x1 - HAIRLINE - 28.0 - px);
        ops.push(text_op(
            px,
            T_SMALL.baseline(cy - T_SMALL.line() / 2.0),
            &shown,
            T_SMALL,
            mix(p.muted, 85, p.app_background),
        ));
    }
    piece.laid.hits.insert(
        0,
        (
            RectF::new(x0, 0.0, head_hit, HEAD),
            Action::Chat(chat::Act::ToggleExpanded(key.to_owned())),
        ),
    );
    let mut h = HEAD;
    if open {
        ops.push(Op::Rect {
            rect: RectF::new(
                x0 + HAIRLINE,
                h - HAIRLINE,
                width - 2.0 * HAIRLINE,
                HAIRLINE,
            ),
            radius: 0.0,
            color: c.rule_box,
            alpha: 1.0,
        });
        h += 22.0;
        h = lay_note_markdown(
            ops,
            text,
            x0 + HAIRLINE + 34.0,
            width - 2.0 * HAIRLINE - 68.0,
            h,
            c.thinking_text,
        );
        h += 28.0;
    }
    if let Op::Rect { rect, .. } = &mut ops[0] {
        rect.h = h;
    }
    ops.push(Op::Border {
        rect: RectF::new(x0, 0.0, width, h),
        width: HAIRLINE,
        color: c.rule_box,
    });
    piece.laid.h = h;
    piece
}

/// Markdown at the note size (thinking, notices): paragraphs and list
/// items with their inline styles, in `color`. Returns the bottom.
fn lay_note_markdown(
    ops: &mut Vec<Op>,
    text: &str,
    x: f32,
    width: f32,
    top: f32,
    color: Rgb,
) -> f32 {
    let ty = T_NOTE;
    let mut y = top;
    for (index, block) in markdown::parse(text).iter().enumerate() {
        if index > 0 {
            y += 16.0;
        }
        let (indent, marker, spans) = match block {
            MdBlock::Paragraph(spans) | MdBlock::Quote(spans) | MdBlock::Heading(_, spans) => {
                (0.0, None, spans.clone())
            }
            MdBlock::Item(depth, marker, spans) => (
                f32::from(*depth) * 30.0,
                Some(marker.clone()),
                spans.clone(),
            ),
            MdBlock::Code { lines, .. } => (0.0, None, plain(&lines.join("\n"))),
            MdBlock::Table(_) => (0.0, None, plain("(a table: OPEN shows it)")),
            MdBlock::Math(tex) => (0.0, None, plain(&super::super::math::inline(tex))),
            MdBlock::Long { preview, .. } => (0.0, None, plain(preview)),
            MdBlock::More { .. } => continue,
            MdBlock::Rule => (0.0, None, plain("———")),
        };
        let marker_w = if let Some(marker) = &marker {
            ops.push(text_op(
                x + indent,
                ty.baseline(y),
                marker,
                ty,
                theme::p().accent,
            ));
            (widgets::measure(marker, ty) + 16.0).max(34.0)
        } else {
            0.0
        };
        let lines = wrap(&spans, width - indent - marker_w, ty.px());
        y = push_runs(ops, &lines, x + indent + marker_w, y, ty, Some(color), 1.0);
    }
    y
}

// ----- Notices --------------------------------------------------------------------

/// A notice's colour (`msg.color`: muted, warning, error, success or
/// `#RRGGBB`; muted by default).
pub(crate) fn notice_color(color: Option<&str>) -> Rgb {
    let p = theme::p();
    match color {
        Some("error") => p.error,
        Some("warning") => p.warning,
        Some("success") => p.success,
        Some("muted") | None => p.muted,
        Some(other) => theme::parse_hex(other).unwrap_or(p.muted),
    }
}

/// A system notice (§3.6.11, PAR-128): a box in the notice's colour with
/// an info icon and three lines of it; opened, "SYSTEM NOTICE", all of it
/// and Copy. `muted`: a legacy call status, at 60 %.
pub(crate) fn lay_notice(
    text: &str,
    color: Rgb,
    muted: bool,
    key: &str,
    stamp: u64,
    open: bool,
) -> Piece {
    let p = theme::p();
    let app = p.app_background;
    let color = if muted { mix(color, 60, app) } else { color };
    let mut laid = Laid::default();
    let x0 = MARGIN;
    let x1 = W - MARGIN;
    let width = x1 - x0;
    let top = HAIRLINE + 34.0;
    let ty = T_CAPTION;
    let first_cy = top + ty.line() / 2.0;
    let ops = &mut laid.ops;
    ops.push(Op::Rect {
        rect: RectF::new(x0, 0.0, width, 0.0),
        radius: 0.0,
        color: mix(color, 6, app),
        alpha: 1.0,
    });
    let icon_cx = x0 + HAIRLINE + 28.0 + 21.0;
    ops.push(icon_op(Icon::InfoCircle, icon_cx, first_cy, 42.0, color));
    let tx = icon_cx + 21.0 + 22.0;
    let chevron_cx = x1 - HAIRLINE - 28.0 - 16.0;
    let text_w = chevron_cx - 16.0 - 22.0 - tx;
    ops.push(icon_op(
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        chevron_cx,
        first_cy,
        33.0,
        p.muted,
    ));
    let bottom = if open {
        ops.push(text_op(
            tx,
            T_EYEBROW.baseline(first_cy - T_EYEBROW.line() / 2.0),
            "System notice",
            T_EYEBROW,
            color,
        ));
        top + ty.line()
    } else {
        let mut lines = wrap_text(text.trim(), ty, text_w);
        if lines.len() > 3 {
            lines.truncate(3);
            if let Some(last) = lines.last_mut() {
                let joined = joined(last);
                let fitted = widgets::fit(&format!("{joined}…"), ty, text_w);
                *last = vec![Run {
                    x: 0.0,
                    w: widgets::measure(&fitted, ty),
                    text: fitted,
                    style: Style::Plain,
                    link: None,
                }];
            }
        }
        push_runs(ops, &lines, tx, top, ty, Some(color), 1.0)
    };
    let head = (bottom + 34.0 + HAIRLINE).max(HEAD);
    laid.hits.push((
        RectF::new(x0, 0.0, width, head),
        Action::Chat(chat::Act::ToggleExpanded(key.to_owned())),
    ));
    let mut h = head;
    if open {
        let body = mix(color, 70, p.text);
        h = lay_note_markdown(
            ops,
            text.trim(),
            tx,
            x1 - HAIRLINE - 34.0 - tx,
            h - 22.0,
            body,
        );
        // Copy.
        let label = "Copy";
        let w = 39.0 + 14.0 + widgets::measure(label, T_SMALL);
        ops.push(icon_op(
            Icon::Copy,
            tx + 19.5,
            h + TARGET / 2.0,
            39.0,
            p.secondary_accent,
        ));
        ops.push(text_op(
            tx + 53.0,
            T_SMALL.baseline(h + (TARGET - T_SMALL.line()) / 2.0),
            label,
            T_SMALL,
            p.secondary_accent,
        ));
        laid.hits.push((
            RectF::new(tx - 17.0, h, w + 34.0, TARGET),
            Action::Chat(chat::Act::CopyNotice(stamp)),
        ));
        h += TARGET + 6.0;
    }
    if let Op::Rect { rect, .. } = &mut laid.ops[0] {
        rect.h = h;
    }
    laid.ops.push(Op::Border {
        rect: RectF::new(x0, 0.0, width, h),
        width: HAIRLINE,
        color: mix(color, 35, app),
    });
    laid.h = h;
    laid.into()
}

/// "Compaction finished" between two rules (PAR-131).
pub(crate) fn lay_divider(label: &str) -> Piece {
    let p = theme::p();
    let c = theme::c();
    let mut laid = Laid::default();
    let ty = T_META;
    let w = widgets::measure(label, ty);
    let top = 28.0;
    let cy = top + ty.line() / 2.0;
    let tx = ((W - w) / 2.0).round();
    laid.ops
        .push(text_op(tx, ty.baseline(top), label, ty, p.muted));
    for (from, to) in [(MARGIN, tx - 28.0), (tx + w + 28.0, W - MARGIN)] {
        laid.ops.push(Op::Rect {
            rect: RectF::new(from, (cy - HAIRLINE / 2.0).round(), to - from, HAIRLINE),
            radius: 0.0,
            color: c.rule_strong,
            alpha: 1.0,
        });
    }
    laid.h = top + ty.line() + 28.0;
    laid.into()
}

/// The transcript's end when the conversation did not load (PAR-102): an
/// error box with the message and Retry.
pub(crate) fn lay_load_error(message: &str) -> Piece {
    let p = theme::p();
    let mut piece = lay_notice(message, p.error, false, "", 0, false);
    // No disclosure: Retry instead.
    piece.laid.hits.clear();
    let top = piece.laid.h + 22.0;
    let button = retry_button(
        "Retry",
        top,
        Action::Chat(chat::Act::Retry),
        &mut piece.laid,
    );
    piece.laid.h = button.y + button.h;
    piece
}

/// A secondary button centred at `top` (accent text and a 50 % rule).
fn retry_button(label: &str, top: f32, action: Action, laid: &mut Laid) -> RectF {
    let p = theme::p();
    let ty = T_NOTE.weight(Weight::MonoSemiBold);
    let w = (widgets::measure(label, ty) + 88.0)
        .round()
        .min(W - 2.0 * MARGIN);
    let rect = RectF::new(((W - w) / 2.0).round(), top, w, TARGET);
    boxed(
        &mut laid.ops,
        rect,
        p.app_background,
        mix(p.accent, 50, p.app_background),
    );
    let shown = widgets::fit(label, ty, w - 44.0);
    let tw = widgets::measure(&shown, ty);
    laid.ops.push(text_op(
        rect.x + (w - tw) / 2.0,
        ty.baseline(top + (TARGET - ty.line()) / 2.0),
        &shown,
        ty,
        p.accent,
    ));
    laid.hits.push((rect, action));
    rect
}

// ----- The history edge -------------------------------------------------------------

/// What the top of the transcript says.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Edge {
    /// The whole conversation is loaded.
    Beginning,
    /// An older page is on its way.
    Loading,
    /// Older history is there to load.
    More,
    /// The older page failed: Retry.
    Failed(String),
}

pub(crate) fn lay_edge(edge: &Edge) -> Piece {
    let p = theme::p();
    let c = theme::c();
    let mut piece = Piece::default();
    let cy = EDGE_H / 2.0;
    match edge {
        Edge::Beginning => {
            let w = widgets::measure("Beginning of conversation", T_EDGE);
            piece.laid.ops.push(text_op(
                ((W - w) / 2.0).round(),
                T_EDGE.baseline(cy - T_EDGE.line() / 2.0),
                "Beginning of conversation",
                T_EDGE,
                c.text_meta,
            ));
            piece.laid.h = EDGE_H;
        }
        Edge::Loading => {
            let label = "Loading earlier…";
            let w = widgets::measure(label, T_META);
            let spinner = theme::SPINNER;
            let x = ((W - w - spinner - 22.0) / 2.0).round();
            piece.laid.ops.push(text_op(
                x + spinner + 22.0,
                T_META.baseline(cy - T_META.line() / 2.0),
                label,
                T_META,
                p.muted,
            ));
            piece.marks.push(Mark {
                y: cy - spinner / 2.0 - 2.0,
                h: spinner + 4.0,
                kind: MarkKind::Spinner {
                    cx: x + spinner / 2.0,
                    cy,
                    size: spinner,
                    color: p.accent,
                    background: p.app_background,
                },
            });
            piece.laid.h = EDGE_H;
        }
        Edge::More => piece.laid.h = EDGE_H,
        Edge::Failed(message) => {
            let rect = retry_button(
                message,
                0.0,
                Action::Chat(chat::Act::RetryOlder),
                &mut piece.laid,
            );
            piece.laid.h = rect.h;
        }
    }
    piece
}

// ----- Footers and the live rows ------------------------------------------------------

/// A finished run's footer (§3.6.12): its line, uppercase; a tap opens the
/// message menu. `suspended`: a durable sleep, in the warning colour.
pub(crate) fn lay_footer(line: &str, stamp: u64, suspended: bool) -> Piece {
    let p = theme::p();
    let c = theme::c();
    let mut laid = Laid::default();
    let ty = T_FOOTER;
    let color = if suspended { p.warning } else { c.text_meta };
    let shown = widgets::fit(line, ty, W - 2.0 * MARGIN);
    laid.ops
        .push(text_op(MARGIN, footer_baseline(), &shown, ty, color));
    laid.hits.push((
        RectF::new(0.0, 0.0, W, FOOTER_H),
        Action::Chat(chat::Act::MessageMenu(stamp)),
    ));
    laid.h = FOOTER_H;
    laid.into()
}

fn footer_baseline() -> f32 {
    T_FOOTER.baseline(FOOTER_CENTER - T_FOOTER.line() / 2.0)
}

/// The live reply's footer: its row, the line drawn by its mark.
pub(crate) fn lay_live_footer(stamp: u64, base: Option<Metadata>) -> Piece {
    let mut laid = Laid::default();
    laid.hits.push((
        RectF::new(0.0, 0.0, W, FOOTER_H),
        Action::Chat(chat::Act::MessageMenu(stamp)),
    ));
    laid.h = FOOTER_H;
    Piece {
        laid,
        marks: vec![Mark {
            y: FOOTER_CENTER - T_FOOTER.line(),
            h: 2.0 * T_FOOTER.line(),
            kind: MarkKind::LiveFooter {
                baseline: footer_baseline(),
                base,
            },
        }],
    }
}

/// WORKING, shimmering, while the reply shows nothing yet.
pub(crate) fn lay_working() -> Piece {
    let p = theme::p();
    let mut laid = Laid::default();
    let baseline = T_EYEBROW.baseline((STATUS_H - T_EYEBROW.line()) / 2.0);
    laid.ops
        .push(text_op(MARGIN, baseline, "Working", T_EYEBROW, p.muted));
    laid.h = STATUS_H;
    Piece {
        laid,
        marks: vec![Mark {
            y: 0.0,
            h: STATUS_H,
            kind: MarkKind::Shimmer {
                x: MARGIN,
                baseline,
                text: "WORKING".into(),
                ty: T_EYEBROW,
            },
        }],
    }
}

/// A live status line's row (its words and clock are its mark's).
pub(crate) fn lay_status(kind: StatusKind) -> Piece {
    let laid = Laid {
        h: STATUS_H,
        ..Laid::default()
    };
    Piece {
        laid,
        marks: vec![Mark {
            y: 0.0,
            h: STATUS_H,
            kind: MarkKind::Status(kind),
        }],
    }
}

// ----- Marks --------------------------------------------------------------------------

/// The wall clock at `now` (epoch ms), the same for every draw of a frame.
pub(crate) fn wall_ms(now: Instant) -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local! {
        static ANCHOR: (Instant, f64) = (
            Instant::now(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64() * 1000.0),
        );
    }
    ANCHOR.with(|(at, ms)| {
        if now >= *at {
            ms + now.duration_since(*at).as_secs_f64() * 1000.0
        } else {
            ms - at.duration_since(now).as_secs_f64() * 1000.0
        }
    })
}

/// Spinners turn in 21 steps (30 Hz), shimmers in 48.
fn spinner_step(now: Instant) -> u32 {
    (anim::phase(now, 700) * 21.0).floor() as u32
}

fn shimmer_step(now: Instant) -> u32 {
    (anim::shimmer(now) * 48.0).floor() as u32
}

/// The live footer's line now.
fn live_footer_text(base: Option<&Metadata>, model: &Model, now: Instant) -> Option<String> {
    let live = model.chat.live.as_ref()?;
    let meta = footer::combine_segment(base, Some(&live.meta))?;
    Some(footer::text(&meta, wall_ms(now), true))
}

/// A status line's words now, its colour, and whether it spins.
fn status_text(kind: StatusKind, model: &Model, now: Instant) -> Option<(String, Rgb, bool)> {
    let p = theme::p();
    let status = &model.chat.status;
    let wall = wall_ms(now);
    match kind {
        StatusKind::Retry => {
            let retry = status.retry.as_ref()?;
            let when = match retry.reset_at {
                Some(reset) => {
                    // SAFETY: localtime_r fills the zeroed tm from a valid time_t.
                    let tm = unsafe {
                        let mut tm: libc::tm = std::mem::zeroed();
                        let time = (reset / 1000.0).floor() as _;
                        libc::localtime_r(&time, &mut tm);
                        tm
                    };
                    format!("retrying at {:02}:{:02}", tm.tm_hour, tm.tm_min)
                }
                None => {
                    let wait = ((retry.at - wall) / 1000.0).ceil().max(0.0);
                    format!("retrying in {wait}s")
                }
            };
            Some((
                format!(
                    "{} — {when} ({}/{})…",
                    retry.message.lines().next().unwrap_or_default(),
                    retry.attempt,
                    retry.max_attempts
                ),
                p.warning,
                false,
            ))
        }
        StatusKind::Compaction => {
            let since = status.compaction?;
            Some((
                format!(
                    "Compacting conversation context… {}",
                    footer::format_duration(wall - since)
                ),
                p.muted,
                true,
            ))
        }
        StatusKind::Wait => {
            let wait = status.wait.as_ref()?;
            let label = wait.label.as_deref().unwrap_or("Waiting for connection…");
            Some((
                format!(
                    "{label} {}",
                    footer::format_duration(wall - wait.started_at)
                ),
                p.warning,
                true,
            ))
        }
    }
}

/// What a mark shows at `now`: equal keys draw the same pixels.
pub(crate) fn mark_key(mark: &Mark, model: &Model, now: Instant) -> u64 {
    use super::hash_of;
    match &mark.kind {
        MarkKind::Spinner { .. } => u64::from(spinner_step(now)),
        MarkKind::Shimmer { .. } => u64::from(shimmer_step(now)),
        MarkKind::LiveFooter { base, .. } => hash_of(live_footer_text(base.as_ref(), model, now)),
        MarkKind::Status(kind) => {
            let text = status_text(*kind, model, now);
            let spins = text.as_ref().is_some_and(|(_, _, spins)| *spins);
            hash_of((
                text.map(|(text, ..)| text),
                spins.then(|| spinner_step(now)),
            ))
        }
    }
}

/// Whether a mark needs frames (and how often): spinners and shimmers at
/// 30 Hz, clocks once a second.
pub(crate) fn mark_frames(mark: &Mark) -> Option<std::time::Duration> {
    match &mark.kind {
        MarkKind::Spinner { .. } | MarkKind::Shimmer { .. } | MarkKind::Status(_) => {
            Some(anim::SLOW_FRAME)
        }
        MarkKind::LiveFooter { .. } => Some(std::time::Duration::from_millis(1000)),
    }
}

/// Draws a mark of the piece whose top is at `dy`.
pub(crate) fn draw_mark(canvas: &mut Canvas, mark: &Mark, dy: f32, model: &Model, now: Instant) {
    let p = theme::p();
    match &mark.kind {
        MarkKind::Spinner {
            cx,
            cy,
            size,
            color,
            background,
        } => {
            let at = anim::phase(now, 700);
            let stepped = (at * 21.0).floor() / 21.0;
            spinner_at(canvas, *cx, cy + dy, *size, *color, *background, stepped);
        }
        MarkKind::Shimmer {
            x,
            baseline,
            text,
            ty,
        } => {
            let w = widgets::measure(text, *ty);
            let band = (w * 0.3).max(40.0);
            let t = shimmer_step(now) as f32 / 48.0;
            let from = (x - band + (w + band) * t).round();
            let rect = RectF::new(from, dy + mark.y, band, mark.h);
            widgets::clipped(canvas, rect, |canvas| {
                widgets::text(canvas, text, *x, baseline + dy, *ty, p.text, 1.0);
            });
        }
        MarkKind::LiveFooter { baseline, base } => {
            if let Some(line) = live_footer_text(base.as_ref(), model, now) {
                let line = widgets::fit(&line, T_FOOTER, W - 2.0 * MARGIN);
                widgets::text(
                    canvas,
                    &line,
                    MARGIN,
                    baseline + dy,
                    T_FOOTER,
                    theme::c().text_meta,
                    1.0,
                );
            }
        }
        MarkKind::Status(kind) => {
            let Some((text, color, spins)) = status_text(*kind, model, now) else {
                return;
            };
            let cy = dy + mark.y + STATUS_H / 2.0;
            let mut x = MARGIN;
            if kind == &StatusKind::Retry {
                // ⟳, drawn (the type has no such glyph).
                icons::draw(
                    canvas,
                    Icon::Refresh,
                    x + STATUS_SPINNER / 2.0,
                    cy,
                    34.0,
                    color,
                    1.0,
                );
                x += STATUS_SPINNER + 22.0;
            } else if spins {
                let at = (anim::phase(now, 700) * 21.0).floor() / 21.0;
                spinner_at(
                    canvas,
                    x + STATUS_SPINNER / 2.0,
                    cy,
                    STATUS_SPINNER,
                    color,
                    p.app_background,
                    at,
                );
                x += STATUS_SPINNER + 22.0;
            }
            let shown = widgets::fit(&text, T_META, W - MARGIN - x);
            widgets::text(
                canvas,
                &shown,
                x,
                T_META.baseline(cy - T_META.line() / 2.0),
                T_META,
                color,
                1.0,
            );
        }
    }
}

/// A spinner at a turn's fraction `at` (`widgets::spinner` at a fixed
/// phase, so a frame's key names exactly what it draws).
fn spinner_at(
    canvas: &mut Canvas,
    cx: f32,
    cy: f32,
    size: f32,
    color: Rgb,
    background: Rgb,
    at: f32,
) {
    let stroke = theme::SPINNER_STROKE * size / theme::SPINNER;
    let radius = (size - stroke) / 2.0;
    let track = mix(theme::p().text, 25, background);
    let tau = std::f32::consts::TAU;
    let (cx, cy) = (cx.round(), cy.round());
    canvas.ring_arc(cx, cy, radius, stroke, 0.0, tau, track, 1.0);
    let start = at * tau;
    canvas.ring_arc(cx, cy, radius, stroke, start, start + tau / 4.0, color, 1.0);
}
