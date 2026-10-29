//! Tool call rows (WP-C2, roadmap §3.6.10, PAR-123–126, 146, 147): a box
//! in the tool's colour with its rail; the call's lines as the TUI shows
//! them (`toolcall.rs`: a bash command per segment, each in its tool's
//! label and colour), folded to three rows for a shell call and to eight
//! of a call longer than twelve; ERR or a spinner; a subagent's
//! conversation to open; opened, its output with Read full output and
//! Copy.

use std::collections::HashMap;

use crate::gfx::{RectF, Rgb};
use crate::text::Weight;

use super::super::markdown::{Span, Style};
use super::super::model::chat::{Chat, OUTPUT_MISSING};
use super::super::model::{Action, Nav, chat};
use super::super::proto::{Block, ToolStyle};
use super::super::theme::{self, HAIRLINE, RAIL, T_CAPTION, T_SMALL, TARGET, mix};
use super::super::toolcall::{self, COLLAPSE_AFTER_ROWS, COLLAPSED_ROWS, CallDisplay};
use super::super::ui::icons::Icon;
use super::W;
use super::blocks::{Mark, MarkKind, Piece, push_runs, text_op};
use super::markdown_layout::{Run, wrap};
use super::ops::Op;
use super::transcript::{Context, ToolRef};

use super::super::theme::TRANSCRIPT_PAD as MARGIN;

/// A tool row's head at least (44 pt).
pub(crate) const TOOL_ROW: f32 = 122.0;
/// Head padding: top, right, bottom, left.
const PAD: (f32, f32, f32, f32) = theme::TOOL_PAD;
/// The right column (ERR, the spinner, the chevron).
const BADGES: f32 = 77.0;
/// A shell call's rows shown folded.
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

/// A call's rows: its logical lines wrapped to `width`, each its label
/// (Bold) then its text, in its colour; at most `keep` of them, and how
/// many rows it has in all. Only what can show is wrapped (a summary may be
/// a whole script): the rest is counted from its lengths.
fn call_rows(display: &CallDisplay, width: f32, keep: usize) -> (Vec<(Vec<Run>, Rgb)>, usize) {
    let p = theme::p();
    let size = T_CAPTION.px();
    let advance = crate::text::measure("M", Weight::Mono, size).max(1.0);
    let columns = ((width / advance).floor() as usize).max(8);
    let budget = columns * (keep + 1);
    let mut rows: Vec<(Vec<Run>, Rgb)> = Vec::new();
    let mut total = 0;
    for line in &display.lines {
        let color = line
            .style
            .color
            .as_deref()
            .and_then(theme::parse_hex)
            .unwrap_or(p.tool);
        let (label, rest) = if line.has_label {
            let label = line.style.label.as_str();
            (label, line.text.get(label.len()..).unwrap_or_default())
        } else {
            ("", line.text.as_str())
        };
        for (index, hard) in rest.split('\n').enumerate() {
            let label = if index == 0 { label } else { "" };
            if total > keep {
                total += (label.chars().count() + hard.chars().count())
                    .div_ceil(columns)
                    .max(1);
                continue;
            }
            let shown: String = hard.chars().take(budget).collect();
            let cut = hard.chars().count().saturating_sub(budget);
            let mut spans = Vec::new();
            if !label.is_empty() {
                spans.push(Span {
                    text: label.to_owned(),
                    style: Style::Bold,
                    link: None,
                });
            }
            spans.push(Span {
                text: if shown.is_empty() { " ".into() } else { shown },
                style: Style::Plain,
                link: None,
            });
            let wrapped = wrap(&spans, width, size);
            total += wrapped.len() + cut.div_ceil(columns);
            rows.extend(wrapped.into_iter().map(|row| (row, color)));
        }
    }
    rows.truncate(keep);
    (rows, total.max(1))
}

/// How many of a call's rows show: folded, three of a shell call, all of
/// a call of twelve or fewer, else eight (PAR-126); opened, all of them.
fn rows_shown(display: &CallDisplay, total: usize, open: bool) -> usize {
    if open {
        OPEN_ROWS
    } else if display.shell {
        FOLDED_ROWS
    } else if total <= COLLAPSE_AFTER_ROWS {
        COLLAPSE_AFTER_ROWS
    } else {
        COLLAPSED_ROWS
    }
}

/// The conversation an `exo` call started (PAR-147), as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Child {
    /// Not a subagent's start.
    None,
    /// A subagent's start whose result (`{"conversation_id": …}`) is not
    /// here yet, or names a conversation the list does not have: its row
    /// keeps the space (so the row's height never changes), dimmed.
    Unknown,
    Known(String),
}

/// Which conversation an `exo` call started, if the list has it.
pub(crate) fn subagent_child(
    tool_name: &str,
    input: &serde_json::Value,
    output: &Output,
    known: impl Fn(&str) -> bool,
) -> Child {
    if tool_name != "exo" || input.get("subagent").is_none() {
        return Child::None;
    }
    let id = match output {
        Output::Text(text) if text.contains("conversation_id") => {
            serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|v| v.get("conversation_id")?.as_str().map(str::to_owned))
        }
        _ => None,
    };
    match id {
        Some(id) if known(&id) => Child::Known(id),
        _ => Child::Unknown,
    }
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

/// One tool call's row (and, opened, its output), from its top. `child`:
/// the subagent conversation it started.
pub(crate) fn lay_tool(
    blocks: &[Block],
    tool: &ToolRef,
    open: bool,
    output: &Output,
    child: &Child,
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
    let display = toolcall::call_display(
        tool_name,
        summary,
        input,
        presentation.as_ref(),
        context.styles,
        &context.model.ops.external_styles,
    );
    let color = display
        .color
        .as_deref()
        .and_then(theme::parse_hex)
        .unwrap_or(p.tool);
    let failed = tool
        .result
        .is_some_and(|at| matches!(&blocks[at], Block::ToolResult { is_error: true, .. }));
    let x0 = MARGIN;
    let x1 = W - MARGIN;
    let width = x1 - x0;
    let tx = x0 + HAIRLINE + PAD.3;
    let text_w = x1 - HAIRLINE - PAD.1 - BADGES - tx;
    let expand = expandable(output);
    let open = open && expand;
    // Folded, enough rows to tell twelve from more.
    let wrapped = if open {
        OPEN_ROWS
    } else {
        COLLAPSE_AFTER_ROWS + 1
    };
    let (mut rows, total) = call_rows(&display, text_w, wrapped);
    let keep = rows_shown(&display, total, open);
    rows.truncate(keep);
    let hidden = total.saturating_sub(rows.len());
    let ty = T_CAPTION;
    let line_h = ty.line();
    let top = HAIRLINE + PAD.0;
    let shown = rows.len() + usize::from(hidden > 0);
    let head = (top + shown as f32 * line_h + PAD.2 + HAIRLINE).max(TOOL_ROW);
    let ops = &mut piece.laid.ops;
    // The box and its rail; the body's rows are added below.
    ops.push(Op::Rect {
        rect: RectF::new(x0, 0.0, width, 0.0),
        radius: 0.0,
        color: mix(color, 5, app),
        alpha: 1.0,
    });
    for (index, (row, row_color)) in rows.iter().enumerate() {
        push_runs(
            ops,
            std::slice::from_ref(row),
            tx,
            top + index as f32 * line_h,
            ty,
            Some(*row_color),
            1.0,
        );
    }
    let after = top + rows.len() as f32 * line_h;
    if failed && let Some((last, _)) = rows.last() {
        let end = tx + super::markdown_layout::line_width(last);
        ops.push(text_op(
            end + super::super::ui::widgets::measure(" ", ty),
            ty.baseline(top + (rows.len() - 1) as f32 * line_h),
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
    let mut head = head;
    // A subagent it started: its conversation (PAR-147).
    if *child != Child::None {
        let label = "Open subagent conversation";
        let w = super::super::ui::widgets::measure(label, T_SMALL);
        let known = match child {
            Child::Known(id) => Some(id),
            _ => None,
        };
        ops.push(text_op(
            tx,
            T_SMALL.baseline(head - 22.0 + (TARGET - T_SMALL.line()) / 2.0),
            label,
            T_SMALL,
            if known.is_some() {
                p.secondary_accent
            } else {
                mix(p.muted, 60, mix(color, 5, app))
            },
        ));
        if let Some(id) = known {
            piece.laid.hits.push((
                RectF::new(tx - 17.0, head - 22.0, w + 34.0, TARGET),
                Action::Nav(Nav::OpenConversation(id.clone())),
            ));
        }
        head += TARGET - 22.0;
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
                    link: None,
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
                    link: None,
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
