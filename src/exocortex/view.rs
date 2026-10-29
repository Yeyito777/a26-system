//! The Exocortex app drawn in the System and Messages apps' iOS dark
//! style: the conversation list (folders, pinned first), a conversation with
//! the user's messages in blue bubbles and replies as full-width markdown,
//! tool calls folded to one row each (tap to open), a composer above Moon's
//! keyboard, and a model sheet. Layout is cached per history entry, so a
//! streaming reply re-lays out only itself.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;

use crate::comms::Scroll;
use crate::gfx::{Canvas, Path, RectF, Rgb};
use crate::text::{self, Weight};
use crate::ui::{
    BG, BLUE, CELL, MARGIN, RED, SECONDARY, SECONDARY_ALPHA, SEPARATOR, TERTIARY_ALPHA, WHITE,
    back_chevron, baseline, chevron, large_title,
};

use super::markdown::{self, MdBlock, Span, Style};
use super::model::{Action, Chat, Model, Page, Row};
use super::proto::{Block, Entry, ToolStyle};

pub const W: f32 = 1080.0;
/// Moon owns the bottom of the window for its swipe-up gesture.
const GESTURE_RESERVE: f32 = 170.0;
const NAV_H: f32 = 150.0;
const TITLE_BASELINE: f32 = 132.0;
const LIST_TOP: f32 = 190.0;
const ROW_H: f32 = 168.0;
const INSET: f32 = MARGIN + 16.0;
const ROW_PRESSED: Rgb = 0x2c2c2e;
const ORANGE: Rgb = 0xff9f0a;
const YELLOW: Rgb = 0xffd60a;

// Text.
const BODY: f32 = 44.0;
const BODY_LINE: f32 = 61.0;
const CODE: f32 = 34.0;
const CODE_LINE: f32 = 48.0;
const SMALL: f32 = 36.0;
const SMALL_LINE: f32 = 48.0;
const PARAGRAPH_GAP: f32 = 26.0;
const CODE_BG: Rgb = 0x1c1c1e;
const INLINE_CODE_BG: Rgb = 0x2c2c2e;
const INLINE_CODE: Rgb = 0xffb86b;

// Bubbles.
const BUBBLE_TEXT: f32 = 46.0;
const BUBBLE_LINE: f32 = 60.0;
const BUBBLE_PAD_X: f32 = 36.0;
const BUBBLE_PAD_Y: f32 = 24.0;
const BUBBLE_MAX: f32 = 800.0;
const BUBBLE_RADIUS: f32 = 44.0;

// Tool rows.
const TOOL_ROW: f32 = 84.0;
/// Runs of this many tool calls fold into one row.
const GROUP_MIN: usize = 3;

// Composer.
const FIELD_TEXT: f32 = 44.0;
const FIELD_LINE: f32 = 58.0;
const FIELD_MIN_H: f32 = 104.0;
const FIELD_MAX_LINES: usize = 6;
const SEND_R: f32 = 48.0;

// Model sheet.
const SHEET_TOP: f32 = 300.0;
const SHEET_HEADER: f32 = 150.0;
const SHEET_ROW: f32 = 118.0;
const SHEET_SECTION: f32 = 84.0;
const CHIP_H: f32 = 84.0;

/// Something on screen a touch is on.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Action(Action),
    /// The composer's text field.
    Composer,
    /// The transcript, away from anything tappable.
    Transcript,
    /// Outside the model sheet (closes it).
    SheetBackdrop,
}

/// Drawing commands of a laid-out piece, relative to its top.
#[derive(Debug, Clone, PartialEq)]
enum Op {
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
fn snap(value: f32) -> f32 {
    (value * 16.0).round() / 16.0
}

fn quantized(mut laid: Laid) -> Laid {
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
struct Laid {
    h: f32,
    ops: Vec<Op>,
    hits: Vec<(RectF, Action)>,
    /// Rows its ops may touch (glyphs reach outside `0..h`).
    ink: (f32, f32),
}

struct Placed {
    y: f32,
    laid: Rc<Laid>,
}

#[derive(Default)]
struct ChatLayout {
    key: u64,
    items: Vec<Placed>,
    height: f32,
}

/// A cached piece and the layout pass that last used it.
struct CachedPiece {
    laid: Rc<Laid>,
    used: u64,
}

/// An entry's plan and the layout pass that last used it.
struct CachedPlan {
    plan: Rc<Plan>,
    used: u64,
}

/// The view's own state: scroll positions, focus and layout caches.
#[derive(Default)]
pub struct ViewState {
    pub list_scroll: Scroll,
    pub chat_scroll: Scroll,
    pub sheet_scroll: Scroll,
    /// The composer has focus and Moon's keyboard is up.
    pub keyboard: bool,
    /// The chat follows its newest message.
    pub follow: bool,
    layout: ChatLayout,
    /// Pieces by what they show (`piece_key`): an entry that comes back
    /// unchanged (a window resent, a reply committed, a chat reopened) is
    /// not laid out again.
    pieces: HashMap<u64, CachedPiece>,
    /// Entries' plans by stamp.
    plans: HashMap<u64, CachedPlan>,
    pass: u64,
    /// Phase of the "working" dots.
    pub dots: u8,
}

// ----- Text layout -------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub x: f32,
    pub w: f32,
    pub text: String,
    pub style: Style,
}

fn font(style: Style, size: f32) -> (Weight, f32, Rgb) {
    match style {
        Style::Plain => (Weight::Regular, size, WHITE),
        Style::Bold => (Weight::SemiBold, size, WHITE),
        Style::Italic => (Weight::Italic, size, WHITE),
        Style::Code => (Weight::Mono, (size * 0.86).round(), INLINE_CODE),
        Style::Link => (Weight::Regular, size, BLUE),
    }
}

/// How far a glued piece (punctuation after a code span) may overhang.
const GLUE_OVERHANG: f32 = 40.0;

/// Word-wraps styled text to `width`; a newline breaks the line. Words
/// wider than a line are broken between characters.
pub fn wrap(spans: &[Span], width: f32, size: f32) -> Vec<Vec<Run>> {
    let mut lines: Vec<Vec<Run>> = Vec::new();
    let mut line: Vec<Run> = Vec::new();
    let mut x = 0.0_f32;
    let finish = |lines: &mut Vec<Vec<Run>>, line: &mut Vec<Run>, x: &mut f32| {
        // Trailing spaces take no room at a break.
        if let Some(last) = line.last_mut() {
            let trimmed = last.text.trim_end_matches(' ').len();
            if trimmed < last.text.len() {
                let (weight, size, _) = font(last.style, size);
                last.text.truncate(trimmed);
                last.w = text::measure(&last.text, weight, size);
            }
        }
        lines.push(std::mem::take(line));
        *x = 0.0;
    };
    let append = |line: &mut Vec<Run>, x: &mut f32, piece: &str, w: f32, style: Style| {
        match line.last_mut() {
            Some(last) if last.style == style && (last.x + last.w - *x).abs() < 0.01 => {
                last.text.push_str(piece);
                last.w += w;
            }
            _ => line.push(Run {
                x: *x,
                w,
                text: piece.to_owned(),
                style,
            }),
        }
        *x += w;
    };
    // Whether a break may come before the next word: after a space, not
    // between a code span and the comma that follows it.
    let mut after_space = true;
    for span in spans {
        let (weight, fsize, _) = font(span.style, size);
        for (index, part) in span.text.split('\n').enumerate() {
            if index > 0 {
                finish(&mut lines, &mut line, &mut x);
                after_space = true;
            }
            for piece in pieces(part) {
                let w = text::measure(piece, weight, fsize);
                if piece.starts_with(' ') || piece.starts_with('\t') {
                    if x > 0.0 && x + w <= width {
                        append(&mut line, &mut x, piece, w, span.style);
                    }
                    after_space = true;
                    continue;
                }
                let glued = !after_space && x > 0.0;
                after_space = false;
                if glued && x + w > width && w <= GLUE_OVERHANG {
                    // Short punctuation stays with what it follows.
                    append(&mut line, &mut x, piece, w, span.style);
                    continue;
                }
                if x > 0.0 && x + w > width {
                    finish(&mut lines, &mut line, &mut x);
                }
                if w <= width {
                    append(&mut line, &mut x, piece, w, span.style);
                    continue;
                }
                // Longer than a line: as many characters as fit, repeatedly.
                let mut chunk = String::new();
                let mut chunk_w = 0.0;
                for character in piece.chars() {
                    let cw = text::measure(character.encode_utf8(&mut [0; 4]), weight, fsize);
                    if x + chunk_w + cw > width && (x > 0.0 || !chunk.is_empty()) {
                        append(&mut line, &mut x, &chunk, chunk_w, span.style);
                        finish(&mut lines, &mut line, &mut x);
                        chunk.clear();
                        chunk_w = 0.0;
                    }
                    chunk.push(character);
                    chunk_w += cw;
                }
                append(&mut line, &mut x, &chunk, chunk_w, span.style);
            }
        }
    }
    if !line.is_empty() || lines.is_empty() {
        finish(&mut lines, &mut line, &mut x);
    }
    lines
}

/// Splits a line into words and runs of spaces.
fn pieces(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (index, character) in text.char_indices() {
        let is_space = character == ' ' || character == '\t';
        if space.is_some_and(|s| s != is_space) {
            out.push(&text[start..index]);
            start = index;
        }
        space = Some(is_space);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn line_width(line: &[Run]) -> f32 {
    line.last().map_or(0.0, |run| run.x + run.w)
}

/// Pushes a wrapped paragraph's runs as text ops; returns its height.
#[allow(clippy::too_many_arguments)]
fn push_lines(
    ops: &mut Vec<Op>,
    lines: &[Vec<Run>],
    x: f32,
    top: f32,
    size: f32,
    line_h: f32,
    color: Option<Rgb>,
    alpha: f32,
) -> f32 {
    for (index, line) in lines.iter().enumerate() {
        let base = top + index as f32 * line_h + line_h * 0.72;
        for run in line {
            let (weight, fsize, default) = font(run.style, size);
            if run.style == Style::Code {
                ops.push(Op::Rect {
                    rect: RectF::new(
                        x + run.x - 6.0,
                        base - fsize * 0.95,
                        run.w + 12.0,
                        fsize * 1.32,
                    ),
                    radius: 10.0,
                    color: INLINE_CODE_BG,
                    alpha: 1.0,
                });
            }
            let color = match (color, run.style) {
                (_, Style::Code | Style::Link) => default,
                (Some(color), _) => color,
                (None, _) => default,
            };
            ops.push(Op::Text {
                x: x + run.x,
                baseline: base,
                text: run.text.clone(),
                weight,
                size: fsize,
                color,
                alpha,
            });
        }
    }
    lines.len() as f32 * line_h
}

fn plain(text: &str) -> Vec<Span> {
    vec![Span {
        text: text.to_owned(),
        style: Style::Plain,
    }]
}

// ----- Markdown ------------------------------------------------------------

/// Lays markdown out at `x`, `width` wide, from `top`; returns the bottom.
#[cfg(test)]
fn lay_markdown(ops: &mut Vec<Op>, source: &str, x: f32, width: f32, top: f32) -> f32 {
    let mut y = top;
    let blocks = markdown::parse(source);
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            y += md_gap(&blocks[index - 1], block);
        }
        y = lay_md_block(ops, block, x, width, y);
    }
    y
}

/// The space between two markdown blocks: list items sit closer together
/// than paragraphs, a heading has more room above it.
fn md_gap(previous: &MdBlock, block: &MdBlock) -> f32 {
    match (previous, block) {
        (MdBlock::Item(..), MdBlock::Item(..)) => 8.0,
        (_, MdBlock::Heading(..)) => PARAGRAPH_GAP + 12.0,
        _ => PARAGRAPH_GAP,
    }
}

/// Lays one markdown block out from `top`; returns its bottom.
fn lay_md_block(ops: &mut Vec<Op>, block: &MdBlock, x: f32, width: f32, top: f32) -> f32 {
    let mut y = top;
    match block {
        MdBlock::Paragraph(spans) => {
            let lines = wrap(spans, width, BODY);
            y += push_lines(ops, &lines, x, y, BODY, BODY_LINE, None, 1.0);
        }
        MdBlock::Heading(level, spans) => {
            let (size, line, weight_spans) = match level {
                1 => (58.0, 74.0, true),
                2 => (52.0, 68.0, true),
                3 => (47.0, 63.0, true),
                _ => (BODY, BODY_LINE, true),
            };
            let spans: Vec<Span> = spans
                .iter()
                .map(|span| Span {
                    text: span.text.clone(),
                    style: if weight_spans && span.style == Style::Plain {
                        Style::Bold
                    } else {
                        span.style
                    },
                })
                .collect();
            let lines = wrap(&spans, width, size);
            y += push_lines(ops, &lines, x, y, size, line, None, 1.0);
        }
        MdBlock::Item(depth, marker, spans) => {
            let indent = f32::from(*depth) * 48.0;
            let marker_w = if marker.ends_with('.') {
                text::measure(marker, Weight::Regular, BODY).max(34.0) + 16.0
            } else {
                48.0
            };
            let shown = if marker == "•" && *depth > 0 {
                "◦"
            } else {
                marker
            };
            ops.push(Op::Text {
                x: x + indent + if marker.ends_with('.') { 0.0 } else { 6.0 },
                baseline: y + BODY_LINE * 0.72,
                text: shown.to_owned(),
                weight: Weight::Regular,
                size: BODY,
                color: if marker.ends_with('.') {
                    WHITE
                } else {
                    SECONDARY
                },
                alpha: if marker.ends_with('.') {
                    1.0
                } else {
                    SECONDARY_ALPHA
                },
            });
            let lines = wrap(spans, width - indent - marker_w, BODY);
            y += push_lines(
                ops,
                &lines,
                x + indent + marker_w,
                y,
                BODY,
                BODY_LINE,
                None,
                1.0,
            );
        }
        MdBlock::Quote(spans) => {
            let lines = wrap(spans, width - 36.0, BODY);
            let h = push_lines(
                ops,
                &lines,
                x + 36.0,
                y,
                BODY,
                BODY_LINE,
                Some(SECONDARY),
                0.85,
            );
            ops.push(Op::Rect {
                rect: RectF::new(x + 4.0, y + 4.0, 7.0, h - 8.0),
                radius: 3.5,
                color: SECONDARY,
                alpha: TERTIARY_ALPHA,
            });
            y += h;
        }
        MdBlock::Code(lines) => {
            y += lay_code(ops, lines, x, width, y, INLINE_CODE_BG_BLOCK, usize::MAX);
        }
        MdBlock::Rule => {
            ops.push(Op::Rect {
                rect: RectF::new(x, y + 12.0, width, 2.0),
                radius: 0.0,
                color: SEPARATOR,
                alpha: 1.0,
            });
            y += 26.0;
        }
    }
    y
}

const INLINE_CODE_BG_BLOCK: Rgb = CODE_BG;

/// Monospaced lines in a rounded box, broken at the box's width (code is
/// never reflowed by words). At most `max_lines`, then a count of the rest.
fn lay_code(
    ops: &mut Vec<Op>,
    lines: &[String],
    x: f32,
    width: f32,
    top: f32,
    background: Rgb,
    max_lines: usize,
) -> f32 {
    let pad = 28.0;
    let advance = text::measure("M", Weight::Mono, CODE).max(1.0);
    let columns = (((width - 2.0 * pad) / advance).floor() as usize).max(8);
    let mut rows: Vec<String> = Vec::new();
    for line in lines {
        let characters: Vec<char> = line.chars().collect();
        if characters.is_empty() {
            rows.push(String::new());
        }
        for chunk in characters.chunks(columns) {
            rows.push(chunk.iter().collect());
        }
    }
    let hidden = rows.len().saturating_sub(max_lines);
    if hidden > 0 {
        rows.truncate(max_lines);
        rows.push(format!("… {hidden} more lines"));
    }
    let h = rows.len().max(1) as f32 * CODE_LINE + 2.0 * pad - 12.0;
    ops.push(Op::Rect {
        rect: RectF::new(x, top, width, h),
        radius: 24.0,
        color: background,
        alpha: 1.0,
    });
    for (index, row) in rows.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        let muted = hidden > 0 && index == rows.len() - 1;
        ops.push(Op::Text {
            x: x + pad,
            baseline: top + pad - 6.0 + index as f32 * CODE_LINE + CODE_LINE * 0.7,
            text: row.clone(),
            weight: if muted { Weight::Regular } else { Weight::Mono },
            size: CODE,
            color: if muted { SECONDARY } else { 0xe5e5ea },
            alpha: if muted { SECONDARY_ALPHA } else { 1.0 },
        });
    }
    h
}

// ----- Transcript ------------------------------------------------------------

struct Context<'a> {
    chat: &'a Chat,
    styles: &'a HashMap<String, ToolStyle>,
    styles_version: u64,
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn thinking_key(blocks: &[Block], index: usize) -> String {
    // Stable within an entry: the first tool call before it, or the start.
    let anchor = blocks[..index].iter().rev().find_map(|block| match block {
        Block::ToolCall { tool_call_id, .. } => Some(tool_call_id.as_str()),
        _ => None,
    });
    format!(
        "think:{}:{}",
        anchor.unwrap_or("start"),
        blocks[..index].len()
    )
}

/// The tool a summary names, as the TUI labels it: its style's label, an
/// MCP server's name, or the tool's own name.
fn tool_label(name: &str, styles: &HashMap<String, ToolStyle>) -> (String, Rgb) {
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

fn parse_color(color: &str) -> Option<Rgb> {
    let hex = color.strip_prefix('#')?;
    (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok())?
}

/// What a tool call shows on its row: its summary, else the input's most
/// telling field.
fn tool_summary(summary: &str, input: &serde_json::Value) -> String {
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

fn tool_input_text(name: &str, input: &serde_json::Value) -> Vec<String> {
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
fn tool_run(blocks: &[Block], from: usize) -> (usize, Vec<usize>) {
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

/// What an entry is drawn as, worked out once per entry (by its stamp; a
/// live reply's on every pass): its pieces, a hash of what each shows (the
/// piece cache's key, so the same content is laid out once) and the space
/// above each.
#[derive(Default)]
struct Plan {
    segments: Vec<Segment>,
}

enum Segment {
    /// The whole entry as one piece: a bubble or a notice.
    Whole { hash: u64 },
    /// A block of a reply's markdown.
    Markdown { block: MdBlock, hash: u64, gap: f32 },
    /// A thinking row, which opens to the thinking.
    Thinking {
        index: usize,
        key: String,
        hash: u64,
        gap: f32,
    },
    /// A run of tool calls: a row each, or one folded row (`group`: its
    /// toggle key and what it shows) that opens to them.
    Tools {
        calls: Vec<ToolRef>,
        group: Option<(String, u64)>,
        gap: f32,
    },
}

struct ToolRef {
    call: usize,
    /// Its result's block, if it has one yet.
    result: Option<usize>,
    key: String,
    hash: u64,
}

fn plan_entry(entry: &Entry) -> Plan {
    let whole = |hash: u64| Plan {
        segments: vec![Segment::Whole { hash }],
    };
    match entry {
        Entry::User { text, metadata } => {
            let automated = metadata
                .as_ref()
                .is_some_and(|m| m.automation.is_some() || m.system);
            whole(hash_of(("user", text, automated)))
        }
        Entry::Ai { blocks, .. } => plan_blocks(blocks),
        Entry::System { text, color } => whole(hash_of(("system", text, color))),
        _ => Plan::default(),
    }
}

fn plan_blocks(blocks: &[Block]) -> Plan {
    let results: HashMap<&str, usize> = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| match block {
            Block::ToolResult { tool_call_id, .. } => Some((tool_call_id.as_str(), index)),
            _ => None,
        })
        .collect();
    let mut segments = Vec::new();
    let mut index = 0;
    let mut previous_text = false;
    while index < blocks.len() {
        let first = segments.is_empty();
        match &blocks[index] {
            Block::Text { text } => {
                if !text.trim().is_empty() {
                    let gap = match (first, previous_text) {
                        (true, _) => 0.0,
                        (false, true) => PARAGRAPH_GAP,
                        (false, false) => 18.0,
                    };
                    let parsed = markdown::parse(text);
                    for (at, block) in parsed.iter().enumerate() {
                        segments.push(Segment::Markdown {
                            block: block.clone(),
                            hash: hash_of(("md", block)),
                            gap: if at == 0 {
                                gap
                            } else {
                                md_gap(&parsed[at - 1], block)
                            },
                        });
                    }
                    previous_text = true;
                }
                index += 1;
            }
            Block::Thinking { text } => {
                if !text.trim().is_empty() {
                    let key = thinking_key(blocks, index);
                    segments.push(Segment::Thinking {
                        index,
                        hash: hash_of(("think", &key, text)),
                        key,
                        gap: if first { 0.0 } else { 10.0 },
                    });
                    previous_text = false;
                }
                index += 1;
            }
            Block::ToolCall { tool_call_id, .. } => {
                let (end, calls) = tool_run(blocks, index);
                let calls: Vec<ToolRef> = calls
                    .into_iter()
                    .filter_map(|call| {
                        let Block::ToolCall {
                            tool_call_id,
                            tool_name,
                            input,
                            summary,
                        } = &blocks[call]
                        else {
                            return None;
                        };
                        let result = results.get(tool_call_id.as_str()).copied();
                        let outcome = result.and_then(|at| match &blocks[at] {
                            Block::ToolResult {
                                output, is_error, ..
                            } => Some((output, is_error)),
                            _ => None,
                        });
                        Some(ToolRef {
                            call,
                            result,
                            key: format!("tool:{tool_call_id}"),
                            hash: hash_of((
                                "tool",
                                tool_call_id,
                                tool_name,
                                summary,
                                input.to_string(),
                                outcome,
                            )),
                        })
                    })
                    .collect();
                let group = (calls.len() >= GROUP_MIN).then(|| {
                    let key = format!("group:{tool_call_id}");
                    let shown: Vec<u64> = calls.iter().map(|tool| tool.hash).collect();
                    let hash = hash_of(("group", &key, shown));
                    (key, hash)
                });
                segments.push(Segment::Tools {
                    calls,
                    group,
                    gap: if first { 0.0 } else { 10.0 },
                });
                previous_text = false;
                index = end.max(index + 1);
            }
            _ => index += 1,
        }
    }
    Plan { segments }
}

/// One tool call's row (and, opened, its input and output), from its top.
fn lay_tool(blocks: &[Block], tool: &ToolRef, indent: f32, open: bool, context: &Context) -> Laid {
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
fn lay_group(
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

/// A thinking row and, opened, the thinking.
fn lay_thinking(text: &str, key: &str, open: bool) -> Laid {
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

fn lay_md_piece(block: &MdBlock) -> Laid {
    let mut laid = Laid::default();
    laid.h = lay_md_block(&mut laid.ops, block, MARGIN, W - 2.0 * MARGIN, 0.0);
    laid
}

fn lay_user(text: &str, alpha: f32, label: Option<&str>) -> Laid {
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
fn lay_notice(text: &str, color: Rgb, max_lines: usize) -> Laid {
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

fn notice_color(color: Option<&str>) -> Rgb {
    match color {
        Some("error") => RED,
        Some("warning") => YELLOW,
        _ => SECONDARY,
    }
}

/// An entry drawn whole: a bubble, or a gray or colored notice.
fn lay_whole(entry: &Entry) -> Laid {
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

/// More pieces and plans than this are kept, the least recently used go.
const PIECES_KEPT: usize = 6000;
const PLANS_KEPT: usize = 1500;

impl ViewState {
    pub fn new() -> Self {
        Self {
            follow: true,
            ..Self::default()
        }
    }

    /// The piece that shows `key`, laid out by `make` if none does yet.
    fn piece(&mut self, key: u64, make: impl FnOnce() -> Laid) -> Rc<Laid> {
        let pass = self.pass;
        let cached = self.pieces.entry(key).or_insert_with(|| CachedPiece {
            laid: Rc::new(quantized(make())),
            used: pass,
        });
        cached.used = pass;
        cached.laid.clone()
    }

    fn plan(&mut self, stamp: u64, entry: &Entry) -> Rc<Plan> {
        let pass = self.pass;
        let cached = self.plans.entry(stamp).or_insert_with(|| CachedPlan {
            plan: Rc::new(plan_entry(entry)),
            used: pass,
        });
        cached.used = pass;
        cached.plan.clone()
    }

    /// Places a planned entry's pieces from `y`; returns where the next
    /// entry goes.
    fn place(
        &mut self,
        items: &mut Vec<Placed>,
        y: f32,
        plan: &Plan,
        entry: Option<&Entry>,
        blocks: &[Block],
        context: &Context,
    ) -> f32 {
        let mut h = 0.0;
        let push = |items: &mut Vec<Placed>, h: &mut f32, laid: Rc<Laid>| {
            if laid.h > 0.0 {
                items.push(Placed {
                    y: y + *h,
                    laid: laid.clone(),
                });
                *h += laid.h;
            }
        };
        let expanded = &context.chat.expanded;
        for segment in &plan.segments {
            match segment {
                Segment::Whole { hash } => {
                    let Some(entry) = entry else { continue };
                    let laid = self.piece(*hash, || lay_whole(entry));
                    push(items, &mut h, laid);
                }
                Segment::Markdown { block, hash, gap } => {
                    h += gap;
                    let laid = self.piece(*hash, || lay_md_piece(block));
                    push(items, &mut h, laid);
                }
                Segment::Thinking {
                    index,
                    key,
                    hash,
                    gap,
                } => {
                    h += gap;
                    let open = expanded.contains(key);
                    let text = match &blocks[*index] {
                        Block::Thinking { text } => text.as_str(),
                        _ => "",
                    };
                    let laid = self.piece(hash_of((hash, open)), || lay_thinking(text, key, open));
                    push(items, &mut h, laid);
                }
                Segment::Tools { calls, group, gap } => {
                    h += gap;
                    let indent = match group {
                        Some((key, hash)) => {
                            let open = expanded.contains(key);
                            let laid = self
                                .piece(hash_of((hash, open, context.styles_version)), || {
                                    lay_group(blocks, calls, key, open, context)
                                });
                            push(items, &mut h, laid);
                            if !open {
                                continue;
                            }
                            34.0
                        }
                        None => 0.0,
                    };
                    for tool in calls {
                        let open = expanded.contains(&tool.key);
                        let id = tool.key.trim_start_matches("tool:");
                        // Opened, it shows the output fetched for it.
                        let fetched = open.then(|| {
                            (
                                context.chat.outputs.get(id),
                                context.chat.outputs_requested.contains(id),
                            )
                        });
                        let key = hash_of((
                            tool.hash,
                            open,
                            indent as i32,
                            context.styles_version,
                            fetched,
                        ));
                        let laid =
                            self.piece(key, || lay_tool(blocks, tool, indent, open, context));
                        push(items, &mut h, laid);
                    }
                }
            }
        }
        if h > 0.0 { y + h + 30.0 } else { y }
    }

    /// Lays the open chat out if it changed since the last frame: every
    /// entry's pieces, from the cache unless what they show is new.
    fn layout(&mut self, model: &Model) {
        let chat = &model.chat;
        let queued = model.queued_here();
        let key = hash_of((
            chat.version,
            chat.id.as_deref(),
            queued.len(),
            chat.loaded,
            model.styles_version,
        ));
        if self.layout.key == key && !self.layout.items.is_empty() {
            return;
        }
        self.pass += 1;
        let context = Context {
            chat,
            styles: &model.tool_styles,
            styles_version: model.styles_version,
        };
        let mut items = Vec::new();
        let mut y = 24.0;
        let single = |view: &mut Self,
                      items: &mut Vec<Placed>,
                      y: f32,
                      key: u64,
                      make: &dyn Fn() -> Laid| {
            let laid = view.piece(key, make);
            if laid.h <= 0.0 {
                return y;
            }
            let next = y + laid.h + 30.0;
            items.push(Placed { y, laid });
            next
        };
        if chat.older_request.is_some() {
            y = single(self, &mut items, y, 1, &|| {
                lay_notice("Loading…", SECONDARY, 1)
            });
        }
        debug_assert_eq!(chat.entries.len(), chat.stamps.len());
        for (entry, stamp) in chat.entries.iter().zip(&chat.stamps) {
            let plan = self.plan(*stamp, entry);
            let blocks = match entry {
                Entry::Ai { blocks, .. } => blocks.as_slice(),
                _ => &[],
            };
            y = self.place(&mut items, y, &plan, Some(entry), blocks, &context);
        }
        for echo in &chat.echoes {
            let key = hash_of(("echo", &echo.text));
            y = single(self, &mut items, y, key, &|| {
                lay_user(&echo.text, 0.75, None)
            });
        }
        if let Some(live) = &chat.live {
            let plan = plan_blocks(&live.blocks);
            y = self.place(&mut items, y, &plan, None, &live.blocks, &context);
        }
        for queued in queued {
            let key = hash_of(("queued", &queued.text));
            y = single(self, &mut items, y, key, &|| {
                lay_user(&queued.text, 0.45, Some("Queued"))
            });
        }
        if let Some(error) = &chat.error {
            let key = hash_of(("error", error));
            y = single(self, &mut items, y, key, &|| lay_notice(error, RED, 3));
        }
        self.prune();
        self.layout = ChatLayout {
            key,
            items,
            height: y + 40.0,
        };
    }

    /// Forgets the least recently used pieces and plans past the limits.
    fn prune(&mut self) {
        if self.pieces.len() > PIECES_KEPT {
            let mut used: Vec<u64> = self.pieces.values().map(|p| p.used).collect();
            used.sort_unstable();
            let cut = used[self.pieces.len() - PIECES_KEPT / 2];
            let pass = self.pass;
            self.pieces.retain(|_, p| p.used >= cut || p.used == pass);
        }
        if self.plans.len() > PLANS_KEPT {
            let mut used: Vec<u64> = self.plans.values().map(|p| p.used).collect();
            used.sort_unstable();
            let cut = used[self.plans.len() - PLANS_KEPT / 2];
            let pass = self.pass;
            self.plans.retain(|_, p| p.used >= cut || p.used == pass);
        }
    }
}

// ----- Geometry ----------------------------------------------------------------

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
    let lines = wrap(&plain(&model.composer), field_w - 60.0, FIELD_TEXT);
    let shown = lines.len().clamp(1, FIELD_MAX_LINES);
    let h = (shown as f32 * FIELD_LINE + 46.0).max(FIELD_MIN_H);
    ComposerGeometry {
        field: RectF::new(MARGIN, bottom - h, field_w, h),
        first: lines.len().saturating_sub(FIELD_MAX_LINES),
        lines,
        send: (W - MARGIN - SEND_R, bottom - FIELD_MIN_H / 2.0),
    }
}

fn transcript_bounds(model: &Model, height: f32, keyboard: bool) -> (f32, f32) {
    (NAV_H, composer(model, height, keyboard).field.y - 20.0)
}

fn list_top(model: &Model) -> f32 {
    let base = if model.folders.is_empty() {
        LIST_TOP
    } else {
        NAV_H + 10.0
    };
    if model.offline { base + 70.0 } else { base }
}

fn list_bottom(height: f32) -> f32 {
    height - GESTURE_RESERVE
}

pub fn list_max(model: &mut Model, height: f32) -> f32 {
    let top = list_top(model);
    let folder = model.current_folder().map(str::to_owned);
    let rows = model.sidebar.rows(folder.as_deref()).len();
    (rows as f32 * ROW_H + 40.0 - (list_bottom(height) - top)).max(0.0)
}

impl ViewState {
    pub fn chat_max(&mut self, model: &Model, height: f32) -> f32 {
        self.layout(model);
        let (top, bottom) = transcript_bounds(model, height, self.keyboard);
        (self.layout.height - (bottom - top)).max(0.0)
    }

    /// The transcript is scrolled near its oldest loaded message.
    pub fn near_top(&self, now: Instant) -> bool {
        self.chat_scroll.value(now) < 1500.0
    }

    /// Height of the transcript above `y` of the first item (for keeping the
    /// view still when older history is added above it).
    pub fn content_height(&mut self, model: &Model) -> f32 {
        self.layout(model);
        self.layout.height
    }
}

fn sheet_content(model: &Model) -> Vec<(f32, SheetItem)> {
    let mut items = Vec::new();
    let mut y = 0.0;
    let selection = model.selection();
    let efforts: Vec<String> = model
        .providers
        .iter()
        .find(|p| p.id == selection.provider)
        .and_then(|p| p.models.iter().find(|m| m.id == selection.model))
        .map(|m| {
            m.supported_efforts
                .iter()
                .map(|e| e.effort.clone())
                .collect()
        })
        .unwrap_or_default();
    if !efforts.is_empty() {
        items.push((y, SheetItem::Section("Effort".into())));
        y += SHEET_SECTION;
        items.push((y, SheetItem::Efforts(efforts)));
        y += CHIP_H + 40.0;
    }
    // The conversation's own provider first.
    let mut providers = model.usable_providers();
    providers.sort_by_key(|p| p.id != selection.provider);
    for provider in providers {
        items.push((y, SheetItem::Section(provider.label.clone())));
        y += SHEET_SECTION;
        for m in &provider.models {
            items.push((
                y,
                SheetItem::Model {
                    provider: provider.id.clone(),
                    id: m.id.clone(),
                    label: m.label.clone(),
                },
            ));
            y += SHEET_ROW;
        }
        y += 24.0;
    }
    items.push((y, SheetItem::End));
    items
}

#[derive(Debug, Clone)]
enum SheetItem {
    Section(String),
    Efforts(Vec<String>),
    Model {
        provider: String,
        id: String,
        label: String,
    },
    End,
}

pub fn sheet_max(model: &Model, height: f32) -> f32 {
    let content = sheet_content(model).last().map_or(0.0, |(y, _)| *y);
    let visible = height - SHEET_TOP - SHEET_HEADER;
    (content + 60.0 - visible).max(0.0)
}

fn effort_chips(efforts: &[String], top: f32) -> Vec<(RectF, String)> {
    let mut x = MARGIN + 10.0;
    let mut out = Vec::new();
    for effort in efforts {
        let w = text::measure(effort, Weight::Medium, 34.0) + 56.0;
        out.push((RectF::new(x, top, w, CHIP_H - 16.0), effort.clone()));
        x += w + 16.0;
    }
    // Too many for one line: shrink them evenly.
    let right = out.last().map_or(0.0, |(r, _)| r.x + r.w);
    if right > W - MARGIN {
        let scale = (W - 2.0 * MARGIN - 10.0) / (right - MARGIN);
        for (rect, _) in &mut out {
            rect.x = MARGIN + (rect.x - MARGIN) * scale;
            rect.w *= scale;
        }
    }
    out
}

// ----- Drawing ---------------------------------------------------------------

/// Rows an op may touch, relative to its piece (generous: glyphs, emoji
/// and anti-aliasing included).
fn op_rows(op: &Op) -> (f32, f32) {
    match op {
        Op::Text { baseline, size, .. } => (baseline - size * 1.4, baseline + size * 0.7),
        Op::Rect { rect, .. } => (rect.y - 2.0, rect.y + rect.h + 2.0),
        Op::Chevron { cy, .. } => (cy - 20.0, cy + 20.0),
    }
}

/// Draws the ops of a piece placed at `dy` that can reach rows `clip`.
fn draw_ops(canvas: &mut Canvas, ops: &[Op], dy: f32, clip: (f32, f32)) {
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

fn small_chevron(canvas: &mut Canvas, x: f32, cy: f32, open: bool) {
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

pub fn draw(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
) {
    let height = canvas.height as f32;
    match model.page {
        Page::List => draw_list(canvas, model, view, pressed, height, now),
        Page::Chat => draw_chat(canvas, model, view, pressed, height, now),
    }
    if model.picker {
        draw_sheet(canvas, model, view, pressed, height, now);
    }
}

fn pressed_action(pressed: Option<&Hit>) -> Option<&Action> {
    match pressed {
        Some(Hit::Action(action)) => Some(action),
        _ => None,
    }
}

fn compose_glyph(canvas: &mut Canvas, cx: f32, cy: f32, alpha: f32) {
    // A square with a pencil across its corner, as iOS's compose button.
    let mut square = Path::new();
    square.stroke_polyline(
        &[
            (cx + 2.0, cy - 26.0),
            (cx - 30.0, cy - 26.0),
            (cx - 30.0, cy + 30.0),
            (cx + 26.0, cy + 30.0),
            (cx + 26.0, cy - 2.0),
        ],
        6.0,
    );
    canvas.fill_path(&square, BLUE, alpha);
    let mut pencil = Path::new();
    pencil.stroke_segment((cx - 6.0, cy + 8.0), (cx + 30.0, cy - 28.0), 9.0);
    canvas.fill_path(&pencil, BLUE, alpha);
}

fn folder_glyph(canvas: &mut Canvas, cx: f32, cy: f32) {
    canvas.fill_rrect(RectF::new(cx - 40.0, cy - 30.0, 36.0, 16.0), 6.0, BLUE, 1.0);
    canvas.fill_rrect(RectF::new(cx - 40.0, cy - 22.0, 80.0, 54.0), 9.0, BLUE, 1.0);
    canvas.fill_rect((cx - 40.0) as i32, (cy - 12.0) as i32, 80, 3, BG, 0.35);
}

fn pin_glyph(canvas: &mut Canvas, cx: f32, cy: f32) {
    let mut pin = Path::new();
    pin.polygon(&[
        (cx - 12.0, cy - 16.0),
        (cx + 12.0, cy - 16.0),
        (cx + 8.0, cy + 2.0),
        (cx - 8.0, cy + 2.0),
    ]);
    pin.stroke_segment((cx, cy + 2.0), (cx, cy + 20.0), 4.0);
    canvas.fill_path(&pin, ORANGE, 0.9);
}

fn status_line(canvas: &mut Canvas, model: &Model, y: f32) {
    if !model.offline {
        return;
    }
    let message = if model.was_connected {
        "Reconnecting to Exocortex…"
    } else {
        "Connecting to Exocortex…"
    };
    text::draw(
        canvas,
        message,
        MARGIN,
        y,
        Weight::Regular,
        38.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}

/// The list's rows that fall in rows `clip`, scrolled to `offset`.
fn draw_list_rows(
    canvas: &mut Canvas,
    model: &Model,
    rows: &[Row],
    pressed: Option<&Action>,
    clip: (f32, f32),
    offset: f32,
) {
    let list_top = list_top(model);
    canvas.set_clip(0, clip.0 as i32, W as i32, clip.1 as i32);
    let today = crate::comms_ui::local_day(crate::clock::unix_now());
    for (index, row) in rows.iter().enumerate() {
        let y = list_top + index as f32 * ROW_H - offset;
        if y + ROW_H < clip.0 || y > clip.1 {
            continue;
        }
        match row {
            Row::Folder {
                id,
                name,
                count,
                pinned,
            } => {
                if pressed == Some(&Action::OpenFolder(id.clone())) {
                    canvas.fill_rect(0, y as i32, W as i32, ROW_H as i32, ROW_PRESSED, 1.0);
                }
                folder_glyph(canvas, INSET + 44.0, y + ROW_H / 2.0);
                let x = INSET + 120.0;
                let right = W - MARGIN - 20.0;
                let count_text = count.to_string();
                text::draw_right(
                    canvas,
                    &count_text,
                    right - 34.0,
                    baseline(y + ROW_H / 2.0, Weight::Regular, 42.0),
                    Weight::Regular,
                    42.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
                if *pinned {
                    pin_glyph(
                        canvas,
                        right - 34.0 - text::measure(&count_text, Weight::Regular, 42.0) - 30.0,
                        y + ROW_H / 2.0,
                    );
                }
                chevron(
                    canvas,
                    right - 6.0,
                    y + ROW_H / 2.0,
                    SECONDARY,
                    TERTIARY_ALPHA,
                );
                let name = text::fit(name, Weight::SemiBold, 46.0, right - 160.0 - x);
                text::draw(
                    canvas,
                    &name,
                    x,
                    baseline(y + ROW_H / 2.0, Weight::SemiBold, 46.0),
                    Weight::SemiBold,
                    46.0,
                    WHITE,
                    1.0,
                );
                canvas.fill_rect(
                    x as i32,
                    (y + ROW_H - 2.0) as i32,
                    (W - x) as i32,
                    2,
                    SEPARATOR,
                    1.0,
                );
            }
            Row::Conversation(id) => {
                let Some(summary) = model.sidebar.conversations.get(id) else {
                    continue;
                };
                if pressed == Some(&Action::OpenConversation(id.clone())) {
                    canvas.fill_rect(0, y as i32, W as i32, ROW_H as i32, ROW_PRESSED, 1.0);
                }
                if summary.unread {
                    canvas.fill_circle(30.0, y + 66.0, 11.0, BLUE, 1.0);
                }
                let x = INSET;
                let right = W - MARGIN - 20.0;
                let when = crate::comms_ui::when_text((summary.updated_at / 1000.0) as i64, today);
                let when_w = text::measure(&when, Weight::Regular, 38.0);
                text::draw_right(
                    canvas,
                    &when,
                    right - 34.0,
                    y + 78.0,
                    Weight::Regular,
                    38.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
                chevron(canvas, right - 6.0, y + 66.0, SECONDARY, TERTIARY_ALPHA);
                let mut title_right = right - 70.0 - when_w;
                if summary.pinned {
                    pin_glyph(canvas, title_right + 14.0, y + 64.0);
                    title_right -= 44.0;
                }
                let title = text::fit(
                    summary.display_title(),
                    Weight::SemiBold,
                    46.0,
                    title_right - x,
                );
                text::draw(
                    canvas,
                    &title,
                    x,
                    y + 80.0,
                    Weight::SemiBold,
                    46.0,
                    WHITE,
                    1.0,
                );
                let selection = super::proto::Selection {
                    provider: summary.provider.clone(),
                    model: summary.model.clone(),
                    effort: summary.effort.clone(),
                };
                let (detail, color, alpha) = if summary.streaming {
                    ("Working…".to_owned(), BLUE, 1.0)
                } else {
                    (model.model_label(&selection), SECONDARY, SECONDARY_ALPHA)
                };
                let detail = text::fit(&detail, Weight::Regular, 40.0, right - x);
                text::draw(
                    canvas,
                    &detail,
                    x,
                    y + 136.0,
                    Weight::Regular,
                    40.0,
                    color,
                    alpha,
                );
                canvas.fill_rect(
                    x as i32,
                    (y + ROW_H - 2.0) as i32,
                    (W - x) as i32,
                    2,
                    SEPARATOR,
                    1.0,
                );
            }
        }
    }
    canvas.reset_clip();
}

/// What a list frame showed, so a scroll moves its rows (`draw_list_update`).
#[derive(Clone, PartialEq)]
pub struct ListFrame {
    bounds: (i32, i32),
    offset: f32,
    key: u64,
    pressed: Option<Action>,
}

/// The current list frame's record, or None on other pages and the sheet.
pub fn list_frame(
    model: &Model,
    view: &ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) -> Option<ListFrame> {
    if model.page != Page::List || model.picker {
        return None;
    }
    let mut key = DefaultHasher::new();
    // A pressed row is redrawn alone; the header's buttons are not rows.
    let header = matches!(
        pressed_action(pressed),
        Some(Action::NewConversation | Action::Back)
    );
    (
        model.sidebar.version,
        model.sidebar.loaded,
        &model.folders,
        model.offline,
        model.was_connected,
        height as i32,
        header,
    )
        .hash(&mut key);
    Some(ListFrame {
        bounds: (list_top(model) as i32, list_bottom(height) as i32),
        offset: view.list_scroll.value(now).round(),
        key: key.finish(),
        pressed: pressed_action(pressed).cloned(),
    })
}

/// The list frame on `canvas` (drawn for `old`) scrolled to `new`'s offset:
/// its rows moved, the uncovered strip drawn. None when more changed.
pub fn draw_list_update(
    canvas: &mut Canvas,
    model: &mut Model,
    old: &ListFrame,
    new: &ListFrame,
) -> Option<FrameDamage> {
    if old.key != new.key || old.bounds != new.bounds {
        return None;
    }
    let (top, bottom) = new.bounds;
    let shift = (new.offset - old.offset) as i32;
    let folder = model.current_folder().map(str::to_owned);
    let rows: Vec<Row> = model.sidebar.rows(folder.as_deref()).to_vec();
    if shift == 0 {
        if old.pressed == new.pressed {
            return Some(FrameDamage::default());
        }
        // A row pressed or let go: only its rows.
        let list_top = list_top(model);
        let mut damage = FrameDamage::default();
        for action in old.pressed.iter().chain(&new.pressed) {
            let Some(index) = rows.iter().position(|row| match (row, action) {
                (Row::Folder { id, .. }, Action::OpenFolder(open)) => id == open,
                (Row::Conversation(id), Action::OpenConversation(open)) => id == open,
                _ => false,
            }) else {
                continue;
            };
            let y = list_top + index as f32 * ROW_H - new.offset;
            let strip = ((y as i32).max(top), ((y + ROW_H) as i32 + 1).min(bottom));
            if strip.0 >= strip.1 {
                continue;
            }
            canvas.fill_rect(0, strip.0, W as i32, strip.1 - strip.0, BG, 1.0);
            draw_list_rows(
                canvas,
                model,
                &rows,
                new.pressed.as_ref(),
                (strip.0 as f32, strip.1 as f32),
                new.offset,
            );
            damage.rows.push(strip);
        }
        return Some(damage);
    }
    if shift.abs() > (bottom - top) / 2 || old.pressed != new.pressed {
        return None;
    }
    let width = canvas.width as usize;
    let strip = if shift > 0 {
        canvas.pixels.copy_within(
            (top + shift) as usize * width..bottom as usize * width,
            top as usize * width,
        );
        (bottom - shift, bottom)
    } else {
        canvas.pixels.copy_within(
            top as usize * width..(bottom + shift) as usize * width,
            (top - shift) as usize * width,
        );
        (top, top - shift)
    };
    canvas.fill_rect(0, strip.0, W as i32, strip.1 - strip.0, BG, 1.0);
    draw_list_rows(
        canvas,
        model,
        &rows,
        new.pressed.as_ref(),
        (strip.0 as f32, strip.1 as f32),
        new.offset,
    );
    Some(FrameDamage {
        moved: Some((top, bottom, -shift)),
        rows: vec![strip],
    })
}

fn draw_list(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    let pressed = pressed_action(pressed).cloned();
    let top = list_top(model);
    let bottom = list_bottom(height);
    let folder = model.current_folder().map(str::to_owned);
    let rows: Vec<Row> = model.sidebar.rows(folder.as_deref()).to_vec();
    // Whole pixels, so a scrolled frame equals a shifted one.
    let offset = view.list_scroll.value(now).round();
    draw_list_rows(
        canvas,
        model,
        &rows,
        pressed.as_ref(),
        (top, bottom),
        offset,
    );
    if rows.is_empty() {
        let (title, detail) = if !model.sidebar.loaded {
            ("", "")
        } else if folder.is_some() {
            ("Empty Folder", "")
        } else {
            ("No Conversations", "Tap the compose button to start one.")
        };
        if !title.is_empty() {
            let cy = (top + bottom) / 2.0;
            text::draw_centered(
                canvas,
                title,
                W / 2.0,
                cy,
                Weight::SemiBold,
                58.0,
                WHITE,
                0.9,
            );
            if !detail.is_empty() {
                text::draw_centered(
                    canvas,
                    detail,
                    W / 2.0,
                    cy + 70.0,
                    Weight::Regular,
                    40.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
            }
        }
    }
    // Header.
    canvas.fill_rect(0, 0, W as i32, top as i32 - 1, BG, 1.0);
    let compose_alpha = if pressed == Some(Action::NewConversation) {
        0.4
    } else {
        1.0
    };
    match &folder {
        None => {
            large_title(canvas, "Exocortex", TITLE_BASELINE);
            compose_glyph(canvas, W - MARGIN - 40.0, 92.0, compose_alpha);
            status_line(canvas, model, LIST_TOP + 34.0);
        }
        Some(id) => {
            let parent = model
                .folders
                .len()
                .checked_sub(2)
                .and_then(|i| model.sidebar.folders.get(&model.folders[i]))
                .map_or("Exocortex".to_owned(), |f| f.name.clone());
            let back_alpha = if pressed == Some(Action::Back) {
                0.4
            } else {
                1.0
            };
            back_chevron(canvas, MARGIN + 6.0, 72.0);
            let parent = text::fit(&parent, Weight::Regular, 47.0, 300.0);
            text::draw(
                canvas,
                &parent,
                MARGIN + 36.0,
                baseline(72.0, Weight::Regular, 47.0),
                Weight::Regular,
                47.0,
                BLUE,
                back_alpha,
            );
            let name = model
                .sidebar
                .folders
                .get(id)
                .map_or("Folder", |f| f.name.as_str());
            let name = text::fit(name, Weight::SemiBold, 47.0, 400.0);
            text::draw_centered(
                canvas,
                &name,
                W / 2.0 + 60.0,
                baseline(72.0, Weight::SemiBold, 47.0),
                Weight::SemiBold,
                47.0,
                WHITE,
                1.0,
            );
            compose_glyph(canvas, W - MARGIN - 40.0, 72.0, compose_alpha);
            canvas.fill_rect(0, NAV_H as i32 - 2, W as i32, 2, SEPARATOR, 1.0);
            status_line(canvas, model, NAV_H + 56.0);
        }
    }
}

/// The transcript's items (and the working dots after them) that fall in
/// rows `clip`, scrolled to `offset`.
fn draw_transcript(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    pressed: Option<&Action>,
    clip: (f32, f32),
    offset: f32,
) {
    let (top, bottom) = clip;
    let area_top = NAV_H;
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    // Pieces are in order: skip to the first that may reach the clip (no
    // glyph reaches a screen beyond its piece).
    let items = &view.layout.items;
    let first =
        items.partition_point(|item| area_top + item.y + item.laid.h - offset < top - 400.0);
    for item in &items[first..] {
        let y = area_top + item.y - offset;
        if y - 400.0 > bottom {
            break;
        }
        if y + item.laid.ink.1 < top || y + item.laid.ink.0 > bottom {
            continue;
        }
        for (rect, action) in &item.laid.hits {
            if pressed == Some(action) {
                canvas.fill_rect(
                    0,
                    (y + rect.y) as i32,
                    W as i32,
                    rect.h as i32,
                    ROW_PRESSED,
                    1.0,
                );
            }
        }
        draw_ops(canvas, &item.laid.ops, y, clip);
    }
    if model.streaming() {
        // Three dots after the reply while the model works.
        let cy = area_top + view.layout.height - offset - 34.0;
        for index in 0..3 {
            let lit = usize::from(view.dots % 3) == index;
            canvas.fill_circle(
                MARGIN + 16.0 + index as f32 * 34.0,
                cy,
                10.0,
                SECONDARY,
                if lit { 0.9 } else { 0.35 },
            );
        }
    }
    canvas.reset_clip();
}

/// What a chat frame showed: enough to draw the next one by changing only
/// what differs (`draw_chat_update`).
#[derive(Clone)]
pub struct ChatFrame {
    bounds: (i32, i32),
    height: i32,
    offset: f32,
    items: Vec<(f32, Rc<Laid>)>,
    /// The working dots' phase and their center in content coordinates.
    dots: Option<(u8, f32)>,
    pressed: Option<Action>,
    nav: u64,
    composer: u64,
}

/// Rows a frame changed: rows moved inside the presented copy (`y0, y1,
/// dy`, for `Presenter::scroll`), then rows drawn anew (for
/// `Presenter::present_rows`).
#[derive(Debug, Default, PartialEq)]
pub struct FrameDamage {
    pub moved: Option<(i32, i32, i32)>,
    pub rows: Vec<(i32, i32)>,
}

/// The current chat frame's record, or None when it shows something the
/// incremental path does not draw (another page, the model sheet, a notice
/// or connection pill, a loading or empty chat).
pub fn chat_frame(
    model: &Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) -> Option<ChatFrame> {
    if model.page != Page::Chat
        || model.picker
        || model.offline
        || model.notice.is_some()
        || !model.chat.loaded
        || model.chat.id.is_none()
    {
        return None;
    }
    view.layout(model);
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    let mut nav = DefaultHasher::new();
    let selection = model.selection();
    (selection.provider, selection.model, selection.effort).hash(&mut nav);
    model
        .open_summary()
        .map(|s| s.display_title().to_owned())
        .hash(&mut nav);
    let mut composer = DefaultHasher::new();
    (&model.composer, view.keyboard, model.streaming()).hash(&mut composer);
    Some(ChatFrame {
        bounds: (top as i32, bottom as i32),
        height: height as i32,
        offset: view.chat_scroll.value(now).round(),
        items: view
            .layout
            .items
            .iter()
            .map(|item| (item.y, item.laid.clone()))
            .collect(),
        dots: model
            .streaming()
            .then_some((view.dots % 3, view.layout.height - 34.0)),
        pressed: pressed_action(pressed).cloned(),
        nav: nav.finish(),
        composer: composer.finish(),
    })
}

/// Where two layouts of one piece start to differ (relative to it), if
/// anywhere: the top of every op after the first that differs.
fn first_change(old: &Laid, new: &Laid) -> Option<f32> {
    let shared = old.ops.len().min(new.ops.len());
    let first = (0..shared)
        .find(|&index| old.ops[index] != new.ops[index])
        .unwrap_or(shared);
    if first == old.ops.len() && first == new.ops.len() && old.h == new.h {
        return None;
    }
    let top = old.ops[first..]
        .iter()
        .chain(&new.ops[first..])
        .map(|op| op_rows(op).0)
        .fold(old.h.min(new.h), f32::min);
    Some(top)
}

/// Turns the frame on `canvas` (drawn for `old`) into the one for `new` by
/// moving the transcript's rows as far as it scrolled and drawing only what
/// changed: the uncovered strip, the transcript from the first change down,
/// the dots, the bar or the composer. None when a full redraw is needed.
/// The result is pixel-identical to a full redraw (tested).
pub fn draw_chat_update(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    old: &ChatFrame,
    new: &ChatFrame,
) -> Option<FrameDamage> {
    if old.bounds != new.bounds || old.height != new.height {
        return None;
    }
    let (top, bottom) = new.bounds;
    let shift = (new.offset - old.offset) as i32;
    if shift.abs() > (bottom - top) / 2 {
        return None;
    }
    // Content (in transcript coordinates) from which the frames differ.
    let mut change = f32::INFINITY;
    for index in 0..old.items.len().max(new.items.len()) {
        match (old.items.get(index), new.items.get(index)) {
            (Some((old_y, old_laid)), Some((new_y, new_laid))) if old_y == new_y => {
                if Rc::ptr_eq(old_laid, new_laid) {
                    continue;
                }
                if let Some(at) = first_change(old_laid, new_laid) {
                    change = new_y + at;
                    break;
                }
            }
            (old_item, new_item) => {
                change = old_item
                    .map(|(y, _)| *y)
                    .into_iter()
                    .chain(new_item.map(|(y, _)| *y))
                    .fold(f32::INFINITY, f32::min);
                break;
            }
        }
    }
    if old.dots != new.dots {
        for (_, y) in old.dots.iter().chain(&new.dots) {
            change = change.min(y - 16.0);
        }
    }
    let mut damage = FrameDamage::default();
    let mut redraw: Vec<(i32, i32)> = Vec::new();
    if shift != 0 {
        let width = canvas.width as usize;
        if shift > 0 {
            canvas.pixels.copy_within(
                (top + shift) as usize * width..bottom as usize * width,
                top as usize * width,
            );
            redraw.push((bottom - shift, bottom));
        } else {
            canvas.pixels.copy_within(
                top as usize * width..(bottom + shift) as usize * width,
                (top - shift) as usize * width,
            );
            redraw.push((top, top - shift));
        }
        damage.moved = Some((top, bottom, -shift));
    }
    if change.is_finite() {
        let from = (NAV_H + change - new.offset).floor() as i32;
        if from < bottom {
            redraw.push((from.max(top), bottom));
        }
    }
    // A row pressed or let go: its highlight's rows.
    let mut nav_pressed = false;
    let mut composer_pressed = false;
    if old.pressed != new.pressed {
        for action in old.pressed.iter().chain(&new.pressed) {
            match action {
                Action::Back | Action::ShowPicker => nav_pressed = true,
                Action::Send | Action::Abort => composer_pressed = true,
                _ => {}
            }
            for (y, laid) in &new.items {
                for (rect, hit) in &laid.hits {
                    if hit == action {
                        let y0 = (NAV_H + y + rect.y - new.offset).floor() as i32 - 1;
                        let y1 = (NAV_H + y + rect.y + rect.h - new.offset).ceil() as i32 + 1;
                        if y1 > top && y0 < bottom {
                            redraw.push((y0.max(top), y1.min(bottom)));
                        }
                    }
                }
            }
        }
    }
    redraw.sort_unstable();
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (y0, y1) in redraw {
        match merged.last_mut() {
            Some(last) if y0 <= last.1 => last.1 = last.1.max(y1),
            _ => merged.push((y0, y1)),
        }
    }
    for &(y0, y1) in &merged {
        canvas.fill_rect(0, y0, W as i32, y1 - y0, BG, 1.0);
        draw_transcript(
            canvas,
            model,
            view,
            new.pressed.as_ref(),
            (y0 as f32, y1 as f32),
            new.offset,
        );
    }
    damage.rows = merged;
    if old.nav != new.nav || nav_pressed {
        draw_nav(canvas, model, new.pressed.as_ref());
        damage.rows.push((0, NAV_H as i32));
    }
    if old.composer != new.composer || composer_pressed {
        draw_composer(canvas, model, view, new.pressed.as_ref(), new.height as f32);
        damage.rows.push((bottom, new.height));
    }
    Some(damage)
}

fn draw_chat(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    view.layout(model);
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    // Whole pixels, so a scrolled frame equals a shifted one (scroll_chat).
    let offset = view.chat_scroll.value(now).round();
    let pressed_action = pressed_action(pressed).cloned();
    draw_transcript(
        canvas,
        model,
        view,
        pressed_action.as_ref(),
        (top, bottom),
        offset,
    );
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    if !model.chat.loaded && model.chat.id.is_some() {
        text::draw_centered(
            canvas,
            "Loading…",
            W / 2.0,
            (top + bottom) / 2.0,
            Weight::Regular,
            40.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    } else if model.chat.id.is_none() && model.chat.echoes.is_empty() {
        let selection = model.selection();
        let label = model.model_label(&selection);
        let cy = (top + bottom) / 2.0 - 60.0;
        text::draw_centered(
            canvas,
            "New Conversation",
            W / 2.0,
            cy,
            Weight::SemiBold,
            58.0,
            WHITE,
            0.9,
        );
        let detail = if label.is_empty() {
            String::new()
        } else {
            format!("with {label}")
        };
        text::draw_centered(
            canvas,
            &detail,
            W / 2.0,
            cy + 70.0,
            Weight::Regular,
            40.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
    canvas.reset_clip();
    draw_nav(canvas, model, pressed_action.as_ref());
    if model.offline {
        let message = if model.was_connected {
            "Reconnecting…"
        } else {
            "Connecting…"
        };
        let w = text::measure(message, Weight::Medium, 34.0) + 60.0;
        let pill = RectF::new((W - w) / 2.0, NAV_H + 20.0, w, 64.0);
        canvas.fill_rrect(pill, 32.0, 0x2c2c2e, 0.95);
        text::draw_centered(
            canvas,
            message,
            W / 2.0,
            NAV_H + 64.0,
            Weight::Medium,
            34.0,
            WHITE,
            0.9,
        );
    }
    if let Some((notice, _)) = &model.notice {
        let notice = text::fit(notice, Weight::Medium, 34.0, W - 2.0 * MARGIN - 60.0);
        let w = text::measure(&notice, Weight::Medium, 34.0) + 60.0;
        let pill = RectF::new((W - w) / 2.0, NAV_H + 20.0, w, 64.0);
        canvas.fill_rrect(pill, 32.0, 0x3a1d1d, 0.95);
        text::draw_centered(
            canvas,
            &notice,
            W / 2.0,
            NAV_H + 64.0,
            Weight::Medium,
            34.0,
            RED,
            1.0,
        );
    }
    draw_composer(canvas, model, view, pressed_action.as_ref(), height);
}

/// The chat's navigation bar: back, title, model.
fn draw_nav(canvas: &mut Canvas, model: &Model, pressed_action: Option<&Action>) {
    canvas.fill_rect(0, 0, W as i32, NAV_H as i32, BG, 1.0);
    let back_alpha = if pressed_action == Some(&Action::Back) {
        0.4
    } else {
        1.0
    };
    back_chevron(canvas, MARGIN + 6.0, 72.0);
    text::draw(
        canvas,
        "Chats",
        MARGIN + 36.0,
        baseline(72.0, Weight::Regular, 47.0),
        Weight::Regular,
        47.0,
        BLUE,
        back_alpha,
    );
    let title = model
        .open_summary()
        .map_or("New Conversation", |s| s.display_title())
        .to_owned();
    let title = text::fit(&title, Weight::SemiBold, 44.0, 560.0);
    text::draw_centered(
        canvas,
        &title,
        W / 2.0 + 40.0,
        66.0,
        Weight::SemiBold,
        44.0,
        WHITE,
        1.0,
    );
    let selection = model.selection();
    let mut label = model.model_label(&selection);
    if !selection.effort.is_empty() {
        label = format!("{label} · {}", selection.effort);
    }
    let picker_alpha = if pressed_action == Some(&Action::ShowPicker) {
        0.4
    } else {
        1.0
    };
    let label = text::fit(&label, Weight::Medium, 34.0, 520.0);
    let label_w = text::measure(&label, Weight::Medium, 34.0);
    let lx = W / 2.0 + 40.0 - (label_w + 30.0) / 2.0;
    text::draw(
        canvas,
        &label,
        lx,
        116.0,
        Weight::Medium,
        34.0,
        BLUE,
        picker_alpha,
    );
    let mut caret = Path::new();
    let cx = lx + label_w + 20.0;
    caret.stroke_polyline(&[(cx - 9.0, 100.0), (cx, 109.0), (cx + 9.0, 100.0)], 4.5);
    canvas.fill_path(&caret, BLUE, picker_alpha);
    canvas.fill_rect(0, NAV_H as i32 - 2, W as i32, 2, SEPARATOR, 1.0);
}

/// The composer: its field, text, caret, and the send or stop button.
fn draw_composer(
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

fn sheet_rect(height: f32) -> RectF {
    RectF::new(0.0, SHEET_TOP, W, height - SHEET_TOP + 60.0)
}

fn draw_sheet(
    canvas: &mut Canvas,
    model: &Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    canvas.fill_rect(0, 0, W as i32, height as i32, 0x000000, 0.55);
    let sheet = sheet_rect(height);
    canvas.fill_rrect(sheet, 40.0, 0x1c1c1e, 1.0);
    canvas.fill_rrect(
        RectF::new(W / 2.0 - 60.0, SHEET_TOP + 18.0, 120.0, 12.0),
        6.0,
        0x5a5a5e,
        1.0,
    );
    text::draw(
        canvas,
        "Model",
        MARGIN + 10.0,
        SHEET_TOP + 104.0,
        Weight::Bold,
        56.0,
        WHITE,
        1.0,
    );
    let pressed = pressed_action(pressed).cloned();
    let done_alpha = if pressed == Some(Action::HidePicker) {
        0.4
    } else {
        1.0
    };
    text::draw_right(
        canvas,
        "Done",
        W - MARGIN - 10.0,
        SHEET_TOP + 100.0,
        Weight::SemiBold,
        46.0,
        BLUE,
        done_alpha,
    );
    let top = SHEET_TOP + SHEET_HEADER;
    let bottom = height - 40.0;
    let offset = view.sheet_scroll.value(now);
    let selection = model.selection();
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    for (y, item) in sheet_content(model) {
        let y = top + y - offset;
        if y > bottom || y + SHEET_ROW < top {
            continue;
        }
        match item {
            SheetItem::Section(title) => {
                text::draw(
                    canvas,
                    &title.to_uppercase(),
                    MARGIN + 10.0,
                    y + 58.0,
                    Weight::Regular,
                    34.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
            }
            SheetItem::Efforts(efforts) => {
                for (rect, effort) in effort_chips(&efforts, y) {
                    let rect = RectF::new(rect.x, rect.y, rect.w, rect.h);
                    let current = effort == selection.effort;
                    let down = pressed == Some(Action::PickEffort(effort.clone()));
                    canvas.fill_rrect(
                        rect,
                        rect.h / 2.0,
                        if current { BLUE } else { 0x2c2c2e },
                        if down { 0.6 } else { 1.0 },
                    );
                    text::draw_centered(
                        canvas,
                        &effort,
                        rect.x + rect.w / 2.0,
                        baseline(rect.y + rect.h / 2.0, Weight::Medium, 34.0),
                        Weight::Medium,
                        34.0,
                        WHITE,
                        1.0,
                    );
                }
            }
            SheetItem::Model {
                provider,
                id,
                label,
            } => {
                let rect = RectF::new(MARGIN, y, W - 2.0 * MARGIN, SHEET_ROW);
                if pressed == Some(Action::PickModel(provider.clone(), id.clone())) {
                    canvas.fill_rrect(rect, 20.0, 0x3a3a3c, 1.0);
                }
                text::draw(
                    canvas,
                    &label,
                    MARGIN + 30.0,
                    baseline(y + SHEET_ROW / 2.0, Weight::Regular, 46.0),
                    Weight::Regular,
                    46.0,
                    WHITE,
                    1.0,
                );
                let id_text = text::fit(&id, Weight::Regular, 32.0, 380.0);
                let current = selection.provider == provider && selection.model == id;
                text::draw_right(
                    canvas,
                    &id_text,
                    W - MARGIN - 90.0,
                    baseline(y + SHEET_ROW / 2.0, Weight::Regular, 32.0),
                    Weight::Regular,
                    32.0,
                    SECONDARY,
                    TERTIARY_ALPHA,
                );
                if current {
                    let mut check = Path::new();
                    let (cx, cy) = (W - MARGIN - 44.0, y + SHEET_ROW / 2.0);
                    check.stroke_polyline(
                        &[
                            (cx - 18.0, cy),
                            (cx - 5.0, cy + 14.0),
                            (cx + 18.0, cy - 14.0),
                        ],
                        7.0,
                    );
                    canvas.fill_path(&check, BLUE, 1.0);
                }
                canvas.fill_rect(
                    (MARGIN + 30.0) as i32,
                    (y + SHEET_ROW - 2.0) as i32,
                    (W - 2.0 * MARGIN - 30.0) as i32,
                    2,
                    SEPARATOR,
                    1.0,
                );
            }
            SheetItem::End => {}
        }
    }
    canvas.reset_clip();
}

// ----- Hit testing -------------------------------------------------------------

pub enum ScrollArea {
    List,
    Chat,
    Sheet,
}

/// The list a touch at `y` may scroll.
pub fn scroll_area(model: &Model, view: &ViewState, height: f32, y: f32) -> Option<ScrollArea> {
    if model.picker {
        return (y >= SHEET_TOP + SHEET_HEADER).then_some(ScrollArea::Sheet);
    }
    match model.page {
        Page::List => (y >= list_top(model) && y < list_bottom(height)).then_some(ScrollArea::List),
        Page::Chat => {
            let (top, bottom) = transcript_bounds(model, height, view.keyboard);
            (y >= top && y < bottom).then_some(ScrollArea::Chat)
        }
    }
}

pub fn hit(
    model: &mut Model,
    view: &mut ViewState,
    height: f32,
    x: i16,
    y: i16,
    now: Instant,
) -> Option<Hit> {
    let (x, y) = (f32::from(x), f32::from(y));
    if model.picker {
        if y < SHEET_TOP {
            return Some(Hit::SheetBackdrop);
        }
        if y < SHEET_TOP + SHEET_HEADER {
            return (x > W - 300.0).then_some(Hit::Action(Action::HidePicker));
        }
        let top = SHEET_TOP + SHEET_HEADER;
        let offset = view.sheet_scroll.value(now);
        for (item_y, item) in sheet_content(model) {
            let iy = top + item_y - offset;
            match item {
                SheetItem::Efforts(efforts) => {
                    for (rect, effort) in effort_chips(&efforts, iy) {
                        if rect.inset(-10.0).contains(x, y) {
                            return Some(Hit::Action(Action::PickEffort(effort)));
                        }
                    }
                }
                SheetItem::Model { provider, id, .. } if y >= iy && y < iy + SHEET_ROW => {
                    return Some(Hit::Action(Action::PickModel(provider, id)));
                }
                _ => {}
            }
        }
        return None;
    }
    match model.page {
        Page::List => {
            if y < list_top(model) {
                if x > W - 200.0
                    && y < if model.folders.is_empty() {
                        170.0
                    } else {
                        NAV_H
                    }
                {
                    return Some(Hit::Action(Action::NewConversation));
                }
                if !model.folders.is_empty() && y < NAV_H && x < 420.0 {
                    return Some(Hit::Action(Action::Back));
                }
                return None;
            }
            if y >= list_bottom(height) {
                return None;
            }
            let offset = view.list_scroll.value(now);
            let index = ((y - list_top(model) + offset) / ROW_H).floor();
            if index < 0.0 {
                return None;
            }
            let folder = model.current_folder().map(str::to_owned);
            let rows = model.sidebar.rows(folder.as_deref());
            match rows.get(index as usize)? {
                Row::Folder { id, .. } => Some(Hit::Action(Action::OpenFolder(id.clone()))),
                Row::Conversation(id) => Some(Hit::Action(Action::OpenConversation(id.clone()))),
            }
        }
        Page::Chat => {
            if y < NAV_H {
                if x < 300.0 {
                    return Some(Hit::Action(Action::Back));
                }
                if y > 80.0 && (W / 2.0 - 300.0..W / 2.0 + 380.0).contains(&x) {
                    return Some(Hit::Action(Action::ShowPicker));
                }
                return (x > 300.0 && x < W - 160.0).then_some(Hit::Action(Action::ShowPicker));
            }
            let geometry = composer(model, height, view.keyboard);
            let (sx, sy) = geometry.send;
            if (x - sx).hypot(y - sy) < SEND_R + 30.0 {
                let typed = !model.composer.trim().is_empty();
                return Some(Hit::Action(if model.streaming() && !typed {
                    Action::Abort
                } else {
                    Action::Send
                }));
            }
            if geometry.field.inset(-16.0).contains(x, y) {
                return Some(Hit::Composer);
            }
            let (top, bottom) = transcript_bounds(model, height, view.keyboard);
            if y < top || y >= bottom {
                return None;
            }
            view.layout(model);
            let offset = view.chat_scroll.value(now);
            for item in &view.layout.items {
                let iy = top + item.y - offset;
                if y < iy || y >= iy + item.laid.h {
                    continue;
                }
                for (rect, action) in &item.laid.hits {
                    if y >= iy + rect.y && y < iy + rect.y + rect.h {
                        return Some(Hit::Action(action.clone()));
                    }
                }
            }
            Some(Hit::Transcript)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;

    #[test]
    fn wrapping_fits_width_breaks_lines_and_long_words() {
        let spans = vec![
            Span {
                text: "Hello world, this is ".into(),
                style: Style::Plain,
            },
            Span {
                text: "bold".into(),
                style: Style::Bold,
            },
            Span {
                text: " text that wraps across several lines\nand breaks".into(),
                style: Style::Plain,
            },
        ];
        let lines = wrap(&spans, 400.0, BODY);
        assert!(lines.len() >= 4);
        for line in &lines {
            assert!(line_width(line) <= 400.5, "{line:?}");
            assert!(!line.first().is_some_and(|run| run.text.starts_with(' ')));
        }
        assert!(
            lines
                .iter()
                .any(|line| line.iter().any(|run| run.style == Style::Bold))
        );
        let last: String = lines
            .last()
            .unwrap()
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(last, "and breaks");
        let long = wrap(&plain(&"x".repeat(200)), 300.0, BODY);
        assert!(long.len() > 3);
        assert!(long.iter().all(|line| line_width(line) <= 300.5));
        assert_eq!(wrap(&plain(""), 300.0, BODY).len(), 1);
    }

    #[test]
    fn markdown_lays_out_top_to_bottom() {
        let mut ops = Vec::new();
        let bottom = lay_markdown(
            &mut ops,
            "# Title\nText **bold**\n\n- a\n- b\n\n```\ncode\n```",
            MARGIN,
            900.0,
            0.0,
        );
        let baselines: Vec<f32> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Text { baseline, .. } => Some(*baseline),
                _ => None,
            })
            .collect();
        assert!(
            baselines.windows(2).all(|w| w[1] >= w[0] - 0.1),
            "{baselines:?}"
        );
        assert!(bottom > *baselines.last().unwrap());
        assert!(ops.iter().any(|op| matches!(
            op,
            Op::Text {
                weight: Weight::Mono,
                ..
            }
        )));
        assert!(ops.iter().any(|op| matches!(
            op,
            Op::Text {
                weight: Weight::SemiBold,
                ..
            }
        )));
    }

    fn chat_model() -> Model {
        let mut model = Model::new();
        model.on_connected();
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","provider":"anthropic","model":"m","effort":"low","title":"Hi","pinned":false,"sortOrder":0,"streaming":false,"unread":false}}"#),
            Instant::now(),
        );
        let commands = model.handle(Action::OpenConversation("c".into()));
        let request = commands[0]["reqId"].as_str().unwrap().to_owned();
        let calls: Vec<String> = (0..4)
            .map(|i| format!(r#"{{"type":"tool_call","toolCallId":"t{i}","toolName":"Bash","input":{{"command":"ls {i}"}},"summary":"ls {i}"}},{{"type":"tool_result","toolCallId":"t{i}","toolName":"Bash","output":"ok","isError":false}}"#))
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"user","text":"Please list"}},{{"type":"ai","blocks":[{},{{"type":"text","text":"Done."}}]}}],"historyStartIndex":0}}"#,
                calls.join(",")
            )),
            Instant::now(),
        );
        model
    }

    #[test]
    fn tool_runs_fold_and_expand_on_tap() {
        let now = Instant::now();
        let mut model = chat_model();
        let mut view = ViewState::default();
        view.layout(&model);
        let height = 2216.0;
        // A bubble, then one folded row for four calls, then the text.
        let group = view.layout.items[1].laid.hits.clone();
        assert_eq!(group.len(), 1);
        assert_eq!(group[0].1, Action::ToggleExpanded("group:t0".into()));
        let (top, _) = transcript_bounds(&model, height, false);
        let y = top + view.layout.items[1].y + group[0].0.y + 10.0;
        let hit_action = hit(&mut model, &mut view, height, 300, y as i16, now);
        assert_eq!(
            hit_action,
            Some(Hit::Action(Action::ToggleExpanded("group:t0".into())))
        );
        // The reply's pieces: the folded row, then the text.
        assert_eq!(view.layout.items.len(), 3);
        let before = view.layout.height;
        let text = view.layout.items[2].laid.clone();
        model.handle(Action::ToggleExpanded("group:t0".into()));
        view.layout(&model);
        // Opened: the row, a row per call, and the same text piece below.
        let hits: Vec<&Action> = view.layout.items[1..]
            .iter()
            .flat_map(|item| item.laid.hits.iter().map(|(_, action)| action))
            .collect();
        assert_eq!(hits.len(), 5);
        assert_eq!(hits[1], &Action::ToggleExpanded("tool:t0".into()));
        assert!(view.layout.height > before + 3.0 * TOOL_ROW);
        assert!(Rc::ptr_eq(&view.layout.items[6].laid, &text));
        // The composer and send button are hit; the transcript elsewhere.
        let geometry = composer(&model, height, false);
        assert_eq!(
            hit(
                &mut model,
                &mut view,
                height,
                300,
                (geometry.field.y + 20.0) as i16,
                now
            ),
            Some(Hit::Composer)
        );
        model.composer = "x".into();
        assert_eq!(
            hit(
                &mut model,
                &mut view,
                height,
                geometry.send.0 as i16,
                geometry.send.1 as i16,
                now
            ),
            Some(Hit::Action(Action::Send))
        );
        let mut canvas = Canvas::new(1080, height as i32);
        draw(&mut canvas, &mut model, &mut view, None, now);
        assert!(canvas.pixels.contains(&BLUE));
    }

    #[test]
    fn a_streaming_reply_lays_out_only_what_grew() {
        let now = Instant::now();
        let mut model = chat_model();
        let mut view = ViewState::new();
        for line in [
            r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":7}"#,
            r#"{"type":"text_chunk","convId":"c","text":"First paragraph.\n\nSecond"}"#,
        ] {
            model.apply(parse_event(line), now);
        }
        view.layout(&model);
        let before: Vec<Rc<Laid>> = view.layout.items.iter().map(|i| i.laid.clone()).collect();
        model.apply(
            parse_event(r#"{"type":"text_chunk","convId":"c","text":" grows"}"#),
            now,
        );
        view.layout(&model);
        let after: Vec<Rc<Laid>> = view.layout.items.iter().map(|i| i.laid.clone()).collect();
        assert_eq!(before.len(), after.len());
        let last = after.len() - 1;
        // Everything but the growing paragraph is the same piece.
        for (index, (old, new)) in before.iter().zip(&after).enumerate() {
            assert_eq!(Rc::ptr_eq(old, new), index != last, "piece {index}");
        }
        // Committed, the reply is the same pieces: nothing is laid out again.
        model.apply(
            parse_event(r#"{"type":"message_complete","convId":"c","blocks":[{"type":"text","text":"First paragraph.\n\nSecond grows"}]}"#),
            now,
        );
        view.layout(&model);
        let committed: Vec<Rc<Laid>> = view.layout.items.iter().map(|i| i.laid.clone()).collect();
        assert!(after.iter().zip(&committed).all(|(a, b)| Rc::ptr_eq(a, b)));
    }

    /// Draws the frame, applies `change`, updates the frame incrementally,
    /// and checks it against a full redraw and that nothing outside the
    /// damage changed (what the presenter skips comparing).
    fn update_matches(
        model: &mut Model,
        view: &mut ViewState,
        height: f32,
        change: impl FnOnce(&mut Model, &mut ViewState),
    ) -> FrameDamage {
        update_matches_with(model, view, height, None, None, change)
    }

    fn update_matches_with(
        model: &mut Model,
        view: &mut ViewState,
        height: f32,
        before: Option<&Hit>,
        after: Option<&Hit>,
        change: impl FnOnce(&mut Model, &mut ViewState),
    ) -> FrameDamage {
        let now = Instant::now();
        let mut canvas = Canvas::new(1080, height as i32);
        canvas.clear(BG);
        draw(&mut canvas, model, view, before, now);
        let old = chat_frame(model, view, before, height, now).expect("a chat frame");
        let mut shadow = canvas.pixels.clone();
        change(model, view);
        let new = chat_frame(model, view, after, height, now).expect("a chat frame");
        let damage = draw_chat_update(&mut canvas, model, view, &old, &new).expect("incremental");
        let mut full = Canvas::new(1080, height as i32);
        full.clear(BG);
        draw(&mut full, model, view, after, now);
        assert!(
            full.pixels == canvas.pixels,
            "incremental frame differs: {damage:?}"
        );
        let width = 1080;
        if let Some((y0, y1, dy)) = damage.moved {
            let rows = y1 - y0 - dy.abs();
            let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
            shadow.copy_within(
                (src * width) as usize..((src + rows) * width) as usize,
                (dst * width) as usize,
            );
        }
        for y in 0..height as i32 {
            if damage.rows.iter().any(|(y0, y1)| (*y0..*y1).contains(&y)) {
                continue;
            }
            let row = (y * width) as usize..((y + 1) * width) as usize;
            assert!(
                shadow[row.clone()] == canvas.pixels[row],
                "row {y} changed outside the damage"
            );
        }
        damage
    }

    fn long_chat(height: f32) -> (Model, ViewState) {
        let mut model = chat_model();
        model.handle(Action::ToggleExpanded("group:t0".into()));
        model.handle(Action::ToggleExpanded("tool:t1".into()));
        model.handle(Action::ToggleExpanded("tool:t2".into()));
        let mut view = ViewState::new();
        let max = view.chat_max(&model, height);
        assert!(max > 300.0, "the transcript must scroll: {max}");
        view.chat_scroll.offset = 120.0;
        (model, view)
    }

    #[test]
    fn a_scrolled_frame_equals_a_redrawn_one() {
        let height = 900.0;
        let (mut model, mut view) = long_chat(height);
        for to in [177.0, 140.0, 260.0, 200.0, 20.0] {
            let damage = update_matches(&mut model, &mut view, height, |_, view| {
                view.chat_scroll.offset = to;
            });
            assert!(damage.moved.is_some());
            let redrawn: i32 = damage.rows.iter().map(|(a, b)| b - a).sum();
            assert!(redrawn <= 200, "only the uncovered strip: {damage:?}");
        }
        // Unchanged: nothing to draw.
        let damage = update_matches(&mut model, &mut view, height, |_, _| {});
        assert_eq!(damage, FrameDamage::default());
    }

    #[test]
    fn streaming_typing_and_the_bar_update_only_what_changed() {
        let height = 900.0;
        let (mut model, mut view) = long_chat(height);
        let now = Instant::now();
        for line in [
            r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":7}"#,
            r#"{"type":"text_chunk","convId":"c","text":"A reply that **grows**, word by word, "}"#,
        ] {
            model.apply(parse_event(line), now);
        }
        view.chat_scroll.offset = view.chat_max(&model, height);
        for chunk in [
            "and wraps onto",
            " a second line with `code`,",
            " then\n\n- a list",
        ] {
            let damage = update_matches(&mut model, &mut view, height, |model, view| {
                model.apply(
                    parse_event(&format!(
                        r#"{{"type":"text_chunk","convId":"c","text":"{}"}}"#,
                        chunk.replace('\n', "\\n")
                    )),
                    now,
                );
                // Following the newest line, as the app does.
                view.chat_scroll.offset = view.chat_max(model, height);
            });
            let redrawn: i32 = damage.rows.iter().map(|(a, b)| b - a).sum();
            assert!(redrawn < 400, "only the reply's end: {damage:?}");
        }
        update_matches(&mut model, &mut view, height, |_, view| view.dots += 1);
        // Pressing a tool row, the back button and the stop button.
        let mut previous: Option<Hit> = None;
        for pressed in [
            Some(Hit::Action(Action::ToggleExpanded("tool:t1".into()))),
            Some(Hit::Action(Action::Back)),
            Some(Hit::Action(Action::Abort)),
            None,
        ] {
            let damage = update_matches_with(
                &mut model,
                &mut view,
                height,
                previous.as_ref(),
                pressed.as_ref(),
                |_, _| {},
            );
            if pressed == Some(Hit::Action(Action::Back)) {
                assert!(damage.rows.contains(&(0, NAV_H as i32)));
            }
            previous = pressed;
        }
        let damage = update_matches(&mut model, &mut view, height, |model, _| {
            model.composer.push('h');
        });
        assert!(damage.moved.is_none() && damage.rows.len() == 1);
        update_matches(&mut model, &mut view, height, |model, _| {
            model.apply(
                parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","provider":"anthropic","model":"m","effort":"high","title":"Renamed","pinned":false,"sortOrder":0,"streaming":true,"unread":false}}"#),
                now,
            );
        });
    }

    #[test]
    fn a_scrolled_list_equals_a_redrawn_one() {
        let now = Instant::now();
        let mut model = Model::new();
        model.on_connected();
        let rows: Vec<String> = (0..30)
            .map(|i| format!(r#"{{"id":"c{i}","provider":"anthropic","model":"m","title":"Conversation {i}","pinned":{},"sortOrder":{i},"streaming":{},"unread":{}}}"#, i < 2, i % 7 == 0, i % 3 == 0))
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversations_list","conversations":[{}],"folders":[]}}"#,
                rows.join(",")
            )),
            now,
        );
        let mut view = ViewState::new();
        let height = 2216.0;
        let mut canvas = Canvas::new(1080, height as i32);
        canvas.clear(BG);
        view.list_scroll.offset = 100.0;
        draw(&mut canvas, &mut model, &mut view, None, now);
        for to in [161.0, 140.0, 900.0, 600.0] {
            let old = list_frame(&model, &view, None, height, now).unwrap();
            view.list_scroll.offset = to;
            let new = list_frame(&model, &view, None, height, now).unwrap();
            let damage =
                draw_list_update(&mut canvas, &mut model, &old, &new).expect("incremental");
            assert_eq!(damage.rows.len(), 1);
            let mut full = Canvas::new(1080, height as i32);
            full.clear(BG);
            draw(&mut full, &mut model, &mut view, None, now);
            assert!(full.pixels == canvas.pixels, "list frame at {to} differs");
        }
        // Pressing a row and letting go redraws that row alone.
        let press = Hit::Action(Action::OpenConversation("c5".into()));
        for (before, after) in [(None, Some(&press)), (Some(&press), None)] {
            let old = list_frame(&model, &view, before, height, now).unwrap();
            let new = list_frame(&model, &view, after, height, now).unwrap();
            let damage =
                draw_list_update(&mut canvas, &mut model, &old, &new).expect("incremental");
            assert_eq!(damage.rows.len(), 1);
            assert!(damage.rows[0].1 - damage.rows[0].0 <= ROW_H as i32 + 1);
            let mut full = Canvas::new(1080, height as i32);
            full.clear(BG);
            draw(&mut full, &mut model, &mut view, after, now);
            assert!(full.pixels == canvas.pixels, "pressed list frame differs");
        }
    }

    #[test]
    fn list_rows_and_compose_are_hit() {
        let now = Instant::now();
        let mut model = chat_model();
        model.handle(Action::Back);
        let mut view = ViewState::default();
        let y = (list_top(&model) + ROW_H / 2.0) as i16;
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 400, y, now),
            Some(Hit::Action(Action::OpenConversation("c".into())))
        );
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 1020, 90, now),
            Some(Hit::Action(Action::NewConversation))
        );
        let mut canvas = Canvas::new(1080, 2216);
        draw(&mut canvas, &mut model, &mut view, None, now);
    }

    #[test]
    fn the_model_sheet_lists_signed_in_providers() {
        let now = Instant::now();
        let mut model = chat_model();
        model.apply(
            parse_event(r#"{"type":"tools_available","providers":[{"id":"anthropic","label":"Anthropic","defaultModel":"m","models":[{"id":"m","label":"Haiku","defaultEffort":"high","supportedEfforts":[{"effort":"low"},{"effort":"high"}]}]},{"id":"openai","label":"OpenAI","defaultModel":"g","models":[{"id":"g","label":"GPT","defaultEffort":"low","supportedEfforts":[]}]}],"authByProvider":{"anthropic":true,"openai":false},"tools":[]}"#),
            now,
        );
        model.handle(Action::ShowPicker);
        let items = sheet_content(&model);
        assert!(
            items
                .iter()
                .any(|(_, item)| matches!(item, SheetItem::Model { id, .. } if id == "m"))
        );
        assert!(
            !items
                .iter()
                .any(|(_, item)| matches!(item, SheetItem::Model { id, .. } if id == "g"))
        );
        let mut view = ViewState::default();
        let chips = effort_chips(
            &["low".into(), "high".into()],
            SHEET_TOP + SHEET_HEADER + SHEET_SECTION,
        );
        let (cx, cy) = chips[1].0.center();
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, cx as i16, cy as i16, now),
            Some(Hit::Action(Action::PickEffort("high".into())))
        );
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 500, 100, now),
            Some(Hit::SheetBackdrop)
        );
    }
}
