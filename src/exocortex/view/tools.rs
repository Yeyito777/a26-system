//! Tool call rows (WP-C2, roadmap §3.6.10, PAR-123–125, 146): a box in
//! the tool's colour with its rail, the call's label and summary (at most
//! three lines folded), ERR or a spinner, and opened, its output with Read
//! full output and Copy; a run of calls folds into one row that opens to
//! them.

use std::collections::HashMap;

use crate::gfx::{RectF, Rgb};
use crate::text::Weight;

use super::super::markdown::{Span, Style};
use super::super::model::chat::{Chat, OUTPUT_MISSING};
use super::super::model::{Action, chat};
use super::super::proto::{Block, Presentation, ToolStyle};
use super::super::theme::{
    self, HAIRLINE, RAIL, T_CAPTION, T_EYEBROW, T_SMALL, TRANSCRIPT_PAD as MARGIN, mix,
};
use super::super::ui::icons::Icon;
use super::W;
use super::blocks::{Mark, MarkKind, Piece, push_runs, text_op};
use super::markdown_layout::wrap;
use super::ops::Op;
use super::transcript::{Context, ToolRef};

/// Runs of this many tool calls fold into one row.
pub(crate) const GROUP_MIN: usize = 3;
/// A tool row's head at least (44 pt).
pub(crate) const TOOL_ROW: f32 = 122.0;
/// Head padding: top, right, bottom, left.
const PAD: (f32, f32, f32, f32) = theme::TOOL_PAD;
/// The right column (ERR, the spinner, the chevron).
const BADGES: f32 = 77.0;
/// Summary rows shown folded.
const FOLDED_ROWS: usize = 3;
/// Rows shown at most opened (a whole script).
const OPEN_ROWS: usize = 400;
/// The output preview (PAR-125).
pub(crate) const OUTPUT_PREVIEW: usize = 6000;
const CHEVRON: f32 = 31.0;
const SPINNER: f32 = 28.0;
/// The actions row of an opened output.
const ACTIONS: f32 = 122.0;

/// The tool a call names, as the TUI labels it (PAR-146): the call's own
/// style, the daemon's registry, an MCP server's name, else its name; in
/// its colour, else the theme's tool colour.
pub(crate) fn tool_label(
    name: &str,
    styles: &HashMap<String, ToolStyle>,
    own: Option<&ToolStyle>,
) -> (String, Rgb) {
    let p = theme::p();
    let lower = name.to_ascii_lowercase();
    if let Some(style) = own
        .filter(|style| style.name.eq_ignore_ascii_case(name) || style.name.is_empty())
        .or_else(|| styles.get(&lower))
    {
        let label = if style.label.is_empty() {
            name.to_owned()
        } else {
            style.label.clone()
        };
        return (label, theme::parse_hex(&style.color).unwrap_or(p.tool));
    }
    if let Some(rest) = name.strip_prefix("mcp__")
        && let Some((server, _)) = rest.split_once("__")
    {
        let server = server.trim_start_matches("claude_ai_").replace('_', " ");
        return (server, p.tool);
    }
    (name.to_owned(), p.tool)
}

/// What a call shows after its label: `presentation.detail`, its summary,
/// else the input's most telling field.
pub(crate) fn tool_detail(
    summary: &str,
    input: &serde_json::Value,
    presentation: Option<&Presentation>,
) -> String {
    if let Some(detail) = presentation.and_then(|p| p.detail.as_ref())
        && !detail.trim().is_empty()
    {
        return detail.trim_end().to_owned();
    }
    if !summary.trim().is_empty() {
        return summary.trim_end().to_owned();
    }
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "description",
        "prompt",
    ] {
        if let Some(value) = input.get(key).and_then(serde_json::Value::as_str) {
            return value.trim_end().to_owned();
        }
    }
    String::new()
}

/// A run of tool calls (and their results) starting at `from`.
pub(crate) fn tool_run(blocks: &[Block], from: usize) -> (usize, Vec<usize>) {
    let mut end = from;
    let mut calls = Vec::new();
    while let Some(block) = blocks.get(end) {
        match block {
            Block::ToolCall { .. } => calls.push(end),
            Block::ToolResult { .. } => {}
            Block::Text { text } | Block::Thinking { text, .. } if text.trim().is_empty() => {}
            _ => break,
        }
        end += 1;
    }
    (end, calls)
}

/// What an opened row's body shows (borrowed: outputs can be megabytes,
/// and every layout pass asks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Output<'a> {
    /// Still running (no result yet).
    Running,
    /// The output (known, fetched or empty).
    Text(&'a str),
    Loading,
    Failed(&'a str),
    /// Not asked for yet (asked for as the row opens).
    Unknown,
}

impl Output<'_> {
    /// What a piece's key needs of it: a tool's output never changes once
    /// known, so its length names it (no hashing megabytes).
    pub(crate) fn key(&self) -> (u8, usize, u64) {
        match self {
            Output::Running => (0, 0, 0),
            Output::Text(text) => (1, text.len(), 0),
            Output::Loading => (2, 0, 0),
            Output::Failed(message) => (3, 0, super::hash_of(message)),
            Output::Unknown => (4, 0, 0),
        }
    }
}

/// The output a call's result has here.
pub(crate) fn output_state<'a>(
    chat: &'a Chat,
    tool: &ToolRef,
    blocks: &'a [Block],
    live: bool,
) -> Output<'a> {
    let Some(Block::ToolResult { output, .. }) = tool.result.map(|at| &blocks[at]) else {
        return Output::Running;
    };
    let id = tool.key.trim_start_matches("tool:");
    if !output.is_empty() {
        return Output::Text(output);
    }
    if let Some(fetched) = chat.outputs.get(id) {
        return Output::Text(fetched);
    }
    if let Some(error) = chat.output_errors.get(id) {
        return Output::Failed(error);
    }
    if chat.outputs_requested.contains(id) {
        return Output::Loading;
    }
    // Streamed results carry their output: empty is empty.
    if live {
        Output::Text("")
    } else {
        Output::Unknown
    }
}

/// The call's lines: label (Bold) and detail, wrapped to `width`; folded,
/// at most three rows, and how many more there are.
fn call_lines(
    label: &str,
    detail: &str,
    width: f32,
    open: bool,
) -> (Vec<Vec<super::markdown_layout::Run>>, usize) {
    let size = T_CAPTION.px();
    let advance = crate::text::measure("M", Weight::Mono, size).max(1.0);
    let columns = ((width / advance).floor() as usize).max(8);
    let keep = if open { OPEN_ROWS } else { FOLDED_ROWS };
    // Wrap only what can show (a summary may be a whole script); count the
    // rest's rows from its lengths.
    let budget = columns * (keep + 1);
    let mut shown = String::new();
    let mut rest_rows = 0;
    for (index, line) in detail.split('\n').enumerate() {
        let chars = line.chars().count()
            + if index == 0 {
                label.chars().count() + 1
            } else {
                0
            };
        if index <= keep {
            if index > 0 {
                shown.push('\n');
            }
            shown.extend(line.chars().take(budget));
            rest_rows += chars.saturating_sub(budget).div_ceil(columns);
        } else {
            rest_rows += chars.div_ceil(columns).max(1);
        }
    }
    let mut spans = vec![Span {
        text: label.to_owned(),
        style: Style::Bold,
    }];
    if !shown.is_empty() {
        spans.push(Span {
            text: format!(" {shown}"),
            style: Style::Plain,
        });
    }
    let mut lines = wrap(&spans, width, size);
    let total = lines.len() + rest_rows;
    let hidden = total.saturating_sub(keep);
    lines.truncate(keep);
    (lines, hidden)
}

/// The bytes of `output` a row previews (on a character boundary).
fn preview(output: &str) -> (&str, bool) {
    if output.len() <= OUTPUT_PREVIEW {
        return (output, false);
    }
    let mut end = OUTPUT_PREVIEW;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    (&output[..end], true)
}

/// Whether a row offers its disclosure: a result whose output is not known
/// to be empty.
pub(crate) fn expandable(output: &Output) -> bool {
    match output {
        Output::Running => false,
        Output::Text(text) => !text.trim().is_empty(),
        _ => true,
    }
}

/// One tool call's row (and, opened, its output), from its top.
pub(crate) fn lay_tool(
    blocks: &[Block],
    tool: &ToolRef,
    indent: f32,
    open: bool,
    output: &Output,
    context: &Context,
) -> Piece {
    let mut piece = Piece::default();
    let Block::ToolCall {
        tool_call_id,
        tool_name,
        input,
        summary,
        presentation,
        ..
    } = &blocks[tool.call]
    else {
        return piece;
    };
    let p = theme::p();
    let app = p.app_background;
    let (label, color) = tool_label(
        tool_name,
        context.styles,
        presentation.as_ref().and_then(|p| p.tool_style.as_ref()),
    );
    let failed = tool
        .result
        .is_some_and(|at| matches!(&blocks[at], Block::ToolResult { is_error: true, .. }));
    let x0 = MARGIN + indent;
    let x1 = W - MARGIN;
    let width = x1 - x0;
    let tx = x0 + HAIRLINE + PAD.3;
    let text_w = x1 - HAIRLINE - PAD.1 - BADGES - tx;
    let detail = tool_detail(summary, input, presentation.as_ref());
    let expand = expandable(output);
    let open = open && expand;
    let (lines, hidden) = call_lines(&label, &detail, text_w, open);
    let ty = T_CAPTION;
    let line_h = ty.line();
    let top = HAIRLINE + PAD.0;
    let rows = lines.len() + usize::from(hidden > 0);
    let head = (top + rows as f32 * line_h + PAD.2 + HAIRLINE).max(TOOL_ROW);
    let ops = &mut piece.laid.ops;
    // The box and its rail; the body's rows are added below.
    ops.push(Op::Rect {
        rect: RectF::new(x0, 0.0, width, 0.0),
        radius: 0.0,
        color: mix(color, 5, app),
        alpha: 1.0,
    });
    let after = push_runs(ops, &lines, tx, top, ty, Some(color), 1.0);
    if failed && let Some(last) = lines.last() {
        let end = tx + super::markdown_layout::line_width(last);
        ops.push(text_op(
            end + super::super::ui::widgets::measure(" ", ty),
            ty.baseline(top + (lines.len() - 1) as f32 * line_h),
            "✗",
            ty,
            p.error,
        ));
    }
    if hidden > 0 {
        ops.push(text_op(
            tx,
            ty.baseline(after),
            &format!("… +{hidden} more lines"),
            ty,
            p.muted,
        ));
    }
    let first_cy = top + line_h / 2.0;
    let chevron_cx = x1 - HAIRLINE - PAD.1 - CHEVRON / 2.0;
    if failed {
        let err = ty.weight(Weight::MonoBold);
        let w = super::super::ui::widgets::measure("ERR", err);
        let right = if expand {
            chevron_cx - CHEVRON / 2.0 - 17.0
        } else {
            x1 - HAIRLINE - PAD.1
        };
        ops.push(text_op(right - w, ty.baseline(top), "ERR", err, p.error));
    }
    if matches!(output, Output::Running) && context.live {
        piece.marks.push(Mark {
            y: first_cy - SPINNER / 2.0 - 2.0,
            h: SPINNER + 4.0,
            kind: MarkKind::Spinner {
                cx: chevron_cx,
                cy: first_cy,
                size: SPINNER,
                color: p.accent,
                background: mix(color, 5, app),
            },
        });
    }
    if expand {
        ops.push(Op::Icon {
            icon: if open {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            cx: chevron_cx,
            cy: first_cy,
            size: CHEVRON,
            color: p.muted,
            alpha: 1.0,
        });
        piece.laid.hits.push((
            RectF::new(x0, 0.0, width, head),
            Action::Chat(chat::Act::ToggleExpanded(tool.key.clone())),
        ));
    }
    let mut h = head;
    if open {
        ops.push(Op::Rect {
            rect: RectF::new(
                x0 + HAIRLINE,
                h - HAIRLINE,
                width - 2.0 * HAIRLINE,
                HAIRLINE,
            ),
            radius: 0.0,
            color: mix(color, 22, app),
            alpha: 1.0,
        });
        h += body(&mut piece, tool_call_id, output, failed, (x0, x1), h);
    }
    let ops = &mut piece.laid.ops;
    if let Op::Rect { rect, .. } = &mut ops[0] {
        rect.h = h;
    }
    ops.push(Op::Border {
        rect: RectF::new(x0, 0.0, width, h),
        width: HAIRLINE,
        color: mix(color, 28, app),
    });
    ops.push(Op::Rect {
        rect: RectF::new(x0 + HAIRLINE, HAIRLINE, RAIL, h - 2.0 * HAIRLINE),
        radius: 0.0,
        color,
        alpha: 1.0,
    });
    piece.laid.h = h;
    piece
}

/// An opened row's body from `top`: the actions and the output, or what
/// stands for it. Returns its height.
fn body(
    piece: &mut Piece,
    tool: &str,
    output: &Output,
    failed: bool,
    (x0, x1): (f32, f32),
    top: f32,
) -> f32 {
    let p = theme::p();
    let c = theme::c();
    let ty = T_SMALL;
    let tx = x0 + HAIRLINE + 34.0;
    let width = x1 - HAIRLINE - 34.0 - tx;
    let ops = &mut piece.laid.ops;
    match output {
        Output::Text(text) if text.trim().is_empty() => {
            ops.push(text_op(
                tx,
                ty.baseline(top + 22.0),
                "No output.",
                ty,
                p.muted,
            ));
            22.0 + ty.line() + 22.0
        }
        Output::Text(text) => {
            // Read full output, Copy.
            let mut x = tx;
            for (label, icon, action) in [
                (
                    "Read full output",
                    Icon::FileText,
                    chat::Act::ReadOutput(tool.into()),
                ),
                ("Copy", Icon::Copy, chat::Act::CopyOutput(tool.into())),
            ] {
                let w = 39.0 + 14.0 + super::super::ui::widgets::measure(label, ty);
                ops.push(Op::Icon {
                    icon,
                    cx: x + 19.5,
                    cy: top + ACTIONS / 2.0,
                    size: 39.0,
                    color: p.secondary_accent,
                    alpha: 1.0,
                });
                ops.push(text_op(
                    x + 53.0,
                    ty.baseline(top + (ACTIONS - ty.line()) / 2.0),
                    label,
                    ty,
                    p.secondary_accent,
                ));
                piece.laid.hits.push((
                    RectF::new(x - 17.0, top, w + 34.0, ACTIONS),
                    Action::Chat(action),
                ));
                x += w + 50.0;
            }
            let (shown, cut) = preview(text.trim_end_matches('\n'));
            let color = if failed { p.error } else { c.text_output };
            let lines = wrap(
                &[Span {
                    text: shown.to_owned(),
                    style: Style::Plain,
                }],
                width,
                ty.px(),
            );
            let mut y = top + ACTIONS + 11.0 - 22.0;
            y = push_runs(ops, &lines, tx, y, ty, Some(color), 1.0);
            if cut {
                ops.push(text_op(
                    tx,
                    ty.baseline(y),
                    "… preview; open full output above",
                    ty,
                    p.muted,
                ));
                y += ty.line();
            }
            y + 28.0 - top
        }
        Output::Loading | Output::Unknown => {
            let cy = top + 22.0 + ty.line() / 2.0;
            piece.marks.push(Mark {
                y: cy - theme::SPINNER / 2.0 - 2.0,
                h: theme::SPINNER + 4.0,
                kind: MarkKind::Spinner {
                    cx: tx + theme::SPINNER / 2.0,
                    cy,
                    size: theme::SPINNER,
                    color: p.accent,
                    background: c.tool_bg,
                },
            });
            ops.push(text_op(
                tx + theme::SPINNER + 22.0,
                ty.baseline(top + 22.0),
                "Loading output…",
                ty,
                p.muted,
            ));
            22.0 + ty.line() + 22.0
        }
        Output::Failed(message) => {
            let lines = wrap(
                &[Span {
                    text: if *message == OUTPUT_MISSING || message.ends_with("Tap to retry.") {
                        (*message).to_owned()
                    } else {
                        format!("{message} Tap to retry.")
                    },
                    style: Style::Plain,
                }],
                width,
                ty.px(),
            );
            let y = push_runs(ops, &lines, tx, top + 22.0, ty, Some(p.error), 1.0);
            let h = y + 22.0 - top;
            piece.laid.hits.push((
                RectF::new(x0, top, x1 - x0, h),
                Action::Chat(chat::Act::RetryOutput(tool.into())),
            ));
            h
        }
        Output::Running => {
            ops.push(text_op(
                tx,
                ty.baseline(top + 22.0),
                "Running…",
                ty,
                p.muted,
            ));
            22.0 + ty.line() + 22.0
        }
    }
}

/// The folded row of a run of tool calls: how many, which tools, failures
/// (`4 TOOL CALLS  $, Read · 1 failed`).
pub(crate) fn lay_group(
    blocks: &[Block],
    calls: &[ToolRef],
    key: &str,
    open: bool,
    context: &Context,
) -> Piece {
    let mut piece = Piece::default();
    let mut names: Vec<String> = Vec::new();
    for tool in calls {
        if let Block::ToolCall {
            tool_name,
            presentation,
            ..
        } = &blocks[tool.call]
        {
            let (label, _) = tool_label(
                tool_name,
                context.styles,
                presentation.as_ref().and_then(|p| p.tool_style.as_ref()),
            );
            if !names.contains(&label) {
                names.push(label);
            }
        }
    }
    let failed = calls
        .iter()
        .filter(|tool| {
            tool.result
                .is_some_and(|at| matches!(&blocks[at], Block::ToolResult { is_error: true, .. }))
        })
        .count();
    let p = theme::p();
    let h = 100.0;
    let cy = h / 2.0;
    let ops = &mut piece.laid.ops;
    ops.push(Op::Icon {
        icon: if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        cx: MARGIN + 16.0,
        cy,
        size: 33.0,
        color: p.muted,
        alpha: 1.0,
    });
    let label = format!("{} tool calls", calls.len());
    let x = MARGIN + 33.0 + 22.0;
    let label_w = super::super::ui::widgets::measure(&label, T_EYEBROW);
    ops.push(text_op(
        x,
        T_EYEBROW.baseline(cy - T_EYEBROW.line() / 2.0),
        &label,
        T_EYEBROW,
        p.muted,
    ));
    let mut detail = names.join(", ");
    if failed > 0 {
        detail = format!("{detail} · {failed} failed");
    }
    let dx = x + label_w + 22.0;
    let detail = super::super::ui::widgets::fit(&detail, T_SMALL, W - MARGIN - dx);
    ops.push(text_op(
        dx,
        T_SMALL.baseline(cy - T_SMALL.line() / 2.0),
        &detail,
        T_SMALL,
        if failed > 0 { p.error } else { p.muted },
    ));
    piece.laid.hits.push((
        RectF::new(0.0, 0.0, W, h),
        Action::Chat(chat::Act::ToggleExpanded(key.to_owned())),
    ));
    piece.laid.h = h;
    piece
}
