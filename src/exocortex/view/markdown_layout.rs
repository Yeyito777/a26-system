//! Markdown laid out as pieces: wrapped, styled runs of text, headings,
//! list items, quotes, rules and code boxes.

use crate::gfx::{RectF, Rgb};
use crate::text::{self, Weight};
use crate::ui::{BLUE, SECONDARY, SECONDARY_ALPHA, SEPARATOR, TERTIARY_ALPHA, WHITE};

#[cfg(test)]
use super::super::markdown;
use super::super::markdown::{MdBlock, Span, Style};
use super::ops::Op;

// Text.
pub(crate) const BODY: f32 = 44.0;
pub(crate) const BODY_LINE: f32 = 61.0;
pub(crate) const CODE: f32 = 34.0;
pub(crate) const CODE_LINE: f32 = 48.0;
pub(crate) const SMALL: f32 = 36.0;
pub(crate) const SMALL_LINE: f32 = 48.0;
pub(crate) const PARAGRAPH_GAP: f32 = 26.0;
pub(crate) const CODE_BG: Rgb = 0x1c1c1e;
pub(crate) const INLINE_CODE_BG: Rgb = 0x2c2c2e;
pub(crate) const INLINE_CODE: Rgb = 0xffb86b;

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub x: f32,
    pub w: f32,
    pub text: String,
    pub style: Style,
}

pub(crate) fn font(style: Style, size: f32) -> (Weight, f32, Rgb) {
    match style {
        Style::Plain => (Weight::Regular, size, WHITE),
        Style::Bold => (Weight::SemiBold, size, WHITE),
        Style::Italic => (Weight::Italic, size, WHITE),
        Style::Code => (Weight::Mono, (size * 0.86).round(), INLINE_CODE),
        Style::Link => (Weight::Regular, size, BLUE),
    }
}

/// How far a glued piece (punctuation after a code span) may overhang.
pub(crate) const GLUE_OVERHANG: f32 = 40.0;

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
pub(crate) fn pieces(text: &str) -> Vec<&str> {
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

pub(crate) fn line_width(line: &[Run]) -> f32 {
    line.last().map_or(0.0, |run| run.x + run.w)
}

/// Pushes a wrapped paragraph's runs as text ops; returns its height.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_lines(
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

pub(crate) fn plain(text: &str) -> Vec<Span> {
    vec![Span {
        text: text.to_owned(),
        style: Style::Plain,
    }]
}

/// Lays markdown out at `x`, `width` wide, from `top`; returns the bottom.
#[cfg(test)]
pub(crate) fn lay_markdown(ops: &mut Vec<Op>, source: &str, x: f32, width: f32, top: f32) -> f32 {
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
pub(crate) fn md_gap(previous: &MdBlock, block: &MdBlock) -> f32 {
    match (previous, block) {
        (MdBlock::Item(..), MdBlock::Item(..)) => 8.0,
        (_, MdBlock::Heading(..)) => PARAGRAPH_GAP + 12.0,
        _ => PARAGRAPH_GAP,
    }
}

/// Lays one markdown block out from `top`; returns its bottom.
pub(crate) fn lay_md_block(
    ops: &mut Vec<Op>,
    block: &MdBlock,
    x: f32,
    width: f32,
    top: f32,
) -> f32 {
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

pub(crate) const INLINE_CODE_BG_BLOCK: Rgb = CODE_BG;

/// Monospaced lines in a rounded box, broken at the box's width (code is
/// never reflowed by words). At most `max_lines`, then a count of the rest.
pub(crate) fn lay_code(
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
