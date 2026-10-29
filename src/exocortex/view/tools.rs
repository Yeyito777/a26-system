//! Tool call rows: one per call (opened: its input and output), or one
//! folded row for a run of calls, labelled as the TUI labels tools.

use std::collections::HashMap;

use crate::gfx::{RectF, Rgb};
use crate::text::{self, Weight};
use crate::ui::{BLUE, MARGIN, RED, SECONDARY, SECONDARY_ALPHA, WHITE};

use super::super::model::Action;
use super::super::proto::{Block, ToolStyle};
use super::W;
use super::markdown_layout::{CODE_BG, SMALL, lay_code};
use super::ops::{Laid, Op};
use super::transcript::{Context, ToolRef};

// Tool rows.
pub(crate) const TOOL_ROW: f32 = 84.0;
/// Runs of this many tool calls fold into one row.
pub(crate) const GROUP_MIN: usize = 3;

/// The tool a summary names, as the TUI labels it: its style's label, an
/// MCP server's name, or the tool's own name.
pub(crate) fn tool_label(name: &str, styles: &HashMap<String, ToolStyle>) -> (String, Rgb) {
    let lower = name.to_ascii_lowercase();
    if let Some(style) = styles.get(&lower) {
        return (
            style.label.clone(),
            parse_color(&style.color).unwrap_or(BLUE),
        );
    }
    if let Some(rest) = name.strip_prefix("mcp__")
        && let Some((server, _)) = rest.split_once("__")
    {
        let server = server.trim_start_matches("claude_ai_").replace('_', " ");
        return (server, 0xbf5af2);
    }
    (name.to_owned(), 0x64d2ff)
}

pub(crate) fn parse_color(color: &str) -> Option<Rgb> {
    let hex = color.strip_prefix('#')?;
    (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok())?
}

/// What a tool call shows on its row: its summary, else the input's most
/// telling field.
pub(crate) fn tool_summary(summary: &str, input: &serde_json::Value) -> String {
    // One line, and no longer than a row can show (a summary may be a whole
    // script).
    let line = |text: &str| {
        let mut out = String::new();
        for word in text.split_whitespace() {
            if out.len() >= 240 {
                break;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
        out
    };
    if !summary.trim().is_empty() {
        return line(summary);
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
            return line(value);
        }
    }
    String::new()
}

pub(crate) fn tool_input_text(name: &str, input: &serde_json::Value) -> Vec<String> {
    if let Some(command) = input.get("command").and_then(serde_json::Value::as_str)
        && matches!(
            name.to_ascii_lowercase().as_str(),
            "bash" | "shell" | "exec_command"
        )
    {
        return command.lines().map(str::to_owned).collect();
    }
    serde_json::to_string_pretty(input)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A run of tool calls (and their results) starting at `from`.
pub(crate) fn tool_run(blocks: &[Block], from: usize) -> (usize, Vec<usize>) {
    let mut end = from;
    let mut calls = Vec::new();
    while let Some(block) = blocks.get(end) {
        match block {
            Block::ToolCall { .. } => calls.push(end),
            Block::ToolResult { .. } => {}
            Block::Text { text } | Block::Thinking { text } if text.trim().is_empty() => {}
            _ => break,
        }
        end += 1;
    }
    (end, calls)
}

/// One tool call's row (and, opened, its input and output), from its top.
pub(crate) fn lay_tool(
    blocks: &[Block],
    tool: &ToolRef,
    indent: f32,
    open: bool,
    context: &Context,
) -> Laid {
    let mut laid = Laid::default();
    let Block::ToolCall {
        tool_call_id,
        tool_name,
        input,
        summary,
    } = &blocks[tool.call]
    else {
        return laid;
    };
    let x = MARGIN + indent;
    let (label, color) = tool_label(tool_name, context.styles);
    let result = tool.result.and_then(|at| match &blocks[at] {
        Block::ToolResult {
            output, is_error, ..
        } => Some((output.as_str(), *is_error)),
        _ => None,
    });
    let base = TOOL_ROW / 2.0 + 12.0;
    let label_w = text::measure(&label, Weight::SemiBold, SMALL);
    laid.ops.push(Op::Text {
        x,
        baseline: base,
        text: label,
        weight: Weight::SemiBold,
        size: SMALL,
        color,
        alpha: 1.0,
    });
    let status_w = 70.0;
    let summary = tool_summary(summary, input);
    let fitted = text::fit(
        &summary,
        Weight::Regular,
        SMALL,
        W - MARGIN - status_w - (x + label_w + 18.0),
    );
    laid.ops.push(Op::Text {
        x: x + label_w + 18.0,
        baseline: base,
        text: fitted,
        weight: Weight::Regular,
        size: SMALL,
        color: SECONDARY,
        alpha: SECONDARY_ALPHA,
    });
    match result {
        Some((_, true)) => laid.ops.push(Op::Text {
            x: W - MARGIN - 64.0,
            baseline: base,
            text: "✕".into(),
            weight: Weight::SemiBold,
            size: SMALL,
            color: RED,
            alpha: 1.0,
        }),
        None => laid.ops.push(Op::Text {
            x: W - MARGIN - 70.0,
            baseline: base,
            text: "…".into(),
            weight: Weight::Regular,
            size: SMALL,
            color: SECONDARY,
            alpha: SECONDARY_ALPHA,
        }),
        Some(_) => {}
    }
    laid.ops.push(Op::Chevron {
        x: W - MARGIN - 14.0,
        cy: TOOL_ROW / 2.0,
        open,
    });
    laid.hits.push((
        RectF::new(0.0, 0.0, W, TOOL_ROW),
        Action::ToggleExpanded(tool.key.clone()),
    ));
    laid.h = TOOL_ROW;
    if open {
        let width = W - MARGIN - x;
        let mut lines = tool_input_text(tool_name, input);
        lines.truncate(40);
        laid.h += lay_code(&mut laid.ops, &lines, x, width, laid.h, CODE_BG, 24) + 14.0;
        let output = match result {
            Some((output, _)) if !output.is_empty() => Some(output.to_owned()),
            _ => context.chat.outputs.get(tool_call_id).cloned(),
        };
        let (lines, color) = match output {
            Some(output) if output.trim().is_empty() => (vec!["(no output)".to_owned()], SECONDARY),
            Some(output) => (
                output.lines().map(str::to_owned).collect(),
                if result.is_some_and(|(_, error)| error) {
                    RED
                } else {
                    WHITE
                },
            ),
            None if result.is_none() => (vec!["Running…".to_owned()], SECONDARY),
            None => (vec!["Loading output…".to_owned()], SECONDARY),
        };
        let box_top = laid.h;
        let h = lay_code(&mut laid.ops, &lines, x, width, box_top, 0x101012, 60);
        if color != WHITE {
            for op in &mut laid.ops {
                if let Op::Text {
                    baseline, color: c, ..
                } = op
                    && *baseline > box_top
                    && *baseline < box_top + h
                {
                    *c = color;
                }
            }
        }
        laid.h += h + 18.0;
    }
    laid
}

/// The folded row of a run of tool calls: how many, which tools, failures.
pub(crate) fn lay_group(
    blocks: &[Block],
    calls: &[ToolRef],
    key: &str,
    open: bool,
    context: &Context,
) -> Laid {
    let mut laid = Laid::default();
    let mut names: Vec<String> = Vec::new();
    for tool in calls {
        if let Block::ToolCall { tool_name, .. } = &blocks[tool.call] {
            let (label, _) = tool_label(tool_name, context.styles);
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
    let title = format!("{} tool calls", calls.len());
    let title_w = text::measure(&title, Weight::SemiBold, SMALL);
    laid.ops.push(Op::Text {
        x: MARGIN,
        baseline: TOOL_ROW / 2.0 + 12.0,
        text: title,
        weight: Weight::SemiBold,
        size: SMALL,
        color: WHITE,
        alpha: 0.85,
    });
    let mut detail = names.join(", ");
    if failed > 0 {
        detail = format!("{detail} · {failed} failed");
    }
    let detail = text::fit(
        &detail,
        Weight::Regular,
        SMALL,
        W - 2.0 * MARGIN - title_w - 90.0,
    );
    laid.ops.push(Op::Text {
        x: MARGIN + title_w + 18.0,
        baseline: TOOL_ROW / 2.0 + 12.0,
        text: detail,
        weight: Weight::Regular,
        size: SMALL,
        color: if failed > 0 { RED } else { SECONDARY },
        alpha: if failed > 0 { 0.9 } else { SECONDARY_ALPHA },
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
    laid
}
