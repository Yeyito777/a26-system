//! Markdown laid out as pieces (WP-C1): wrapped, styled runs of text,
//! headings, list items, quotes, rules and code boxes, in Whale (roadmap
//! §3.3, §3.6.7). Others use `wrap`, `Run` and `font` (the composer's and
//! bubbles' text) read-only; C1 owns the rest.
//!
//! Everything here bakes the current theme's colours and sizes into its
//! ops: the view caches pieces by `theme::generation()`.

use std::rc::Rc;

use crate::gfx::{RectF, Rgb};
use crate::text::{self, Weight};

use super::super::markdown::{self, LinkKind, MdBlock, Span, Style, link_kind};
use super::super::math;
use super::super::model::rich::Rich;
use super::super::model::{Action, UiAct, rich};
use super::super::theme::{
    self, HAIRLINE, LIST_INDENT, MARKDOWN_GAP, MARKER_GAP, QUOTE_BAR, QUOTE_INDENT, T_BODY, T_H1,
    T_H2, T_H3, T_H4, T_INLINE_CODE, T_SMALL, TARGET,
};
use super::code;
use super::ops::{self, Laid, Op};
use super::table;

// Text.
pub(crate) const BODY: f32 = 40.0;
pub(crate) const BODY_LINE: f32 = 60.0;
/// Tool lines, notices.
#[allow(dead_code)]
pub(crate) const SMALL: f32 = 34.0;
#[allow(dead_code)]
pub(crate) const SMALL_LINE: f32 = 48.0;
pub(crate) const PARAGRAPH_GAP: f32 = MARKDOWN_GAP;

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub x: f32,
    pub w: f32,
    pub text: String,
    pub style: Style,
    /// A link's target (`Style::Link`).
    pub link: Option<Rc<str>>,
}

/// A span style's face, size and colour at body size `size`.
pub fn font(style: Style, size: f32) -> (Weight, f32, Rgb) {
    let p = theme::p();
    match style {
        Style::Plain => (Weight::Mono, size, p.text),
        Style::Bold => (Weight::MonoBold, size, p.text),
        Style::Italic => (theme::T_ITALIC, size, p.text),
        Style::Code => (
            Weight::Mono,
            (size * T_INLINE_CODE.px() / T_BODY.px()).round(),
            p.secondary_accent,
        ),
        Style::Link => (Weight::Mono, size, p.link),
        Style::Strike | Style::Math => (Weight::Mono, size, p.text),
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
    let append = |line: &mut Vec<Run>, x: &mut f32, piece: &str, w: f32, span: &Span| {
        match line.last_mut() {
            Some(last)
                if last.style == span.style
                    && last.link == span.link
                    && (last.x + last.w - *x).abs() < 0.01 =>
            {
                last.text.push_str(piece);
                last.w += w;
            }
            _ => line.push(Run {
                x: *x,
                w,
                text: piece.to_owned(),
                style: span.style,
                link: span.link.clone(),
            }),
        }
        *x += w;
    };
    // Whether a break may come before the next word: after a space, not
    // between a code span and the comma that follows it.
    let mut after_space = true;
    for span in spans {
        let (weight, fsize, _) = font(span.style, size);
        // Inline math is drawn as its Unicode rendering.
        let rendered;
        let text = if span.style == Style::Math {
            rendered = math::inline(&span.text);
            rendered.as_str()
        } else {
            span.text.as_str()
        };
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                finish(&mut lines, &mut line, &mut x);
                after_space = true;
            }
            for piece in pieces(part) {
                let w = text::measure(piece, weight, fsize);
                if piece.starts_with(' ') || piece.starts_with('\t') {
                    if x > 0.0 && x + w <= width {
                        append(&mut line, &mut x, piece, w, span);
                    }
                    after_space = true;
                    continue;
                }
                let glued = !after_space && x > 0.0;
                after_space = false;
                if glued && x + w > width && w <= GLUE_OVERHANG {
                    // Short punctuation stays with what it follows.
                    append(&mut line, &mut x, piece, w, span);
                    continue;
                }
                if x > 0.0 && x + w > width {
                    finish(&mut lines, &mut line, &mut x);
                }
                if w <= width {
                    append(&mut line, &mut x, piece, w, span);
                    continue;
                }
                // Longer than a line: as many characters as fit, repeatedly.
                let mut chunk = String::new();
                let mut chunk_w = 0.0;
                for character in piece.chars() {
                    let cw = text::measure(character.encode_utf8(&mut [0; 4]), weight, fsize);
                    if x + chunk_w + cw > width && (x > 0.0 || !chunk.is_empty()) {
                        append(&mut line, &mut x, &chunk, chunk_w, span);
                        finish(&mut lines, &mut line, &mut x);
                        chunk.clear();
                        chunk_w = 0.0;
                    }
                    chunk.push(character);
                    chunk_w += cw;
                }
                append(&mut line, &mut x, &chunk, chunk_w, span);
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
/// `color` overrides the text's (not code's or links').
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
    lines_into(ops, None, lines, x, top, size, line_h, color, alpha)
}

/// `push_lines`, its links tappable: a hit per link run (C1's blocks).
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_lines_linked(
    laid: &mut Laid,
    lines: &[Vec<Run>],
    x: f32,
    top: f32,
    size: f32,
    line_h: f32,
    color: Option<Rgb>,
) -> f32 {
    lines_into(
        &mut laid.ops,
        Some(&mut laid.hits),
        lines,
        x,
        top,
        size,
        line_h,
        color,
        1.0,
    )
}

#[allow(clippy::too_many_arguments)]
fn lines_into(
    ops: &mut Vec<Op>,
    mut hits: Option<&mut Vec<(RectF, Action)>>,
    lines: &[Vec<Run>],
    x: f32,
    top: f32,
    size: f32,
    line_h: f32,
    color: Option<Rgb>,
    alpha: f32,
) -> f32 {
    let c = theme::c();
    for (index, line) in lines.iter().enumerate() {
        let line_top = top + index as f32 * line_h;
        let base = line_top + (line_h + size * 0.74) / 2.0;
        for run in line {
            let (weight, fsize, default) = font(run.style, size);
            if run.style == Style::Code {
                ops.push(Op::Rect {
                    rect: RectF::new(
                        x + run.x - 8.0,
                        base - fsize * 0.86,
                        run.w + 16.0,
                        fsize * 1.24,
                    ),
                    radius: 0.0,
                    color: c.inline_code_bg,
                    alpha: 1.0,
                });
            }
            let color = match (color, run.style) {
                (_, Style::Code | Style::Link) => default,
                (Some(color), _) => color,
                (None, _) => default,
            };
            // Struck through: at 60 %, a 2 px line through the x-height.
            let alpha = if run.style == Style::Strike {
                alpha * 0.6
            } else {
                alpha
            };
            ops.push(Op::Text {
                x: x + run.x,
                baseline: base,
                text: run.text.clone(),
                weight,
                size: fsize,
                tracking: 0.0,
                color,
                alpha,
            });
            match run.style {
                Style::Strike => ops.push(Op::Rect {
                    rect: RectF::new(x + run.x, (base - fsize * 0.28).round(), run.w, HAIRLINE),
                    radius: 0.0,
                    color,
                    alpha,
                }),
                Style::Link => {
                    // Underlined 4 px under the baseline; dashed for a file.
                    let file = run
                        .link
                        .as_deref()
                        .is_some_and(|link| matches!(link_kind(link), Some(LinkKind::File(_))));
                    let y = (base + 4.0).round();
                    if file {
                        let mut dx = 0.0;
                        while dx < run.w {
                            let w = 8.0_f32.min(run.w - dx);
                            ops.push(ops::rect(
                                RectF::new((x + run.x + dx).round(), y, w, HAIRLINE),
                                c.link_underline,
                            ));
                            dx += 14.0;
                        }
                    } else {
                        ops.push(ops::rect(
                            RectF::new(x + run.x, y, run.w, HAIRLINE),
                            c.link_underline,
                        ));
                    }
                    if let (Some(hits), Some(link)) = (hits.as_deref_mut(), &run.link) {
                        hits.push((
                            RectF::new(x + run.x - 6.0, line_top, run.w + 12.0, line_h),
                            Action::Rich(rich::Act::OpenLink(link.to_string())),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    lines.len() as f32 * line_h
}

pub(crate) fn plain(text: &str) -> Vec<Span> {
    vec![Span::new(text, Style::Plain)]
}

/// How a piece of markdown is set: the transcript's body for now (C1 adds
/// the reader's and tool outputs').
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MdStyle {
    #[default]
    Body,
}

/// Markdown laid out as one piece at `x`, `width` wide, from `top` (its
/// height is the last block's bottom).
#[allow(dead_code)] // The reader's and C2's.
pub fn lay(source: &str, x: f32, width: f32, top: f32, style: MdStyle) -> Laid {
    lay_with(source, x, width, top, style, None)
}

/// `lay` with rich content's state (which code blocks wrap; none without
/// it).
pub fn lay_with(
    source: &str,
    x: f32,
    width: f32,
    top: f32,
    style: MdStyle,
    rich: Option<&Rich>,
) -> Laid {
    let MdStyle::Body = style;
    let mut laid = Laid::default();
    let mut y = top;
    let blocks = markdown::parse_capped(source);
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            y += md_gap(&blocks[index - 1], block);
        }
        y = block_into(&mut laid, block, x, width, y, rich);
    }
    laid.h = y;
    ops::quantized(laid)
}

/// One markdown block as a piece at `x`, `width` wide, its top at 0: the
/// transcript lays a reply out block by block (a streamed chunk lays out
/// only the block that grew). Its cache key must include `state_key`.
pub fn lay_block(block: &MdBlock, x: f32, width: f32, rich: Option<&Rich>) -> Laid {
    let mut laid = Laid::default();
    laid.h = block_into(&mut laid, block, x, width, 0.0, rich);
    laid
}

/// What a block's look depends on besides its content (a code block
/// wrapped or not): part of its piece's key.
pub fn state_key(block: &MdBlock, rich: Option<&Rich>) -> u64 {
    match (block, rich) {
        (MdBlock::Code { lang, lines }, Some(rich)) => {
            u64::from(rich.wrapped.contains(&code::key(lang, lines)))
        }
        _ => 0,
    }
}

/// Lays markdown out at `x`, `width` wide, from `top`; returns the bottom.
#[cfg(test)]
pub(crate) fn lay_markdown(ops: &mut Vec<Op>, source: &str, x: f32, width: f32, top: f32) -> f32 {
    let laid = lay_with(source, x, width, top, MdStyle::Body, None);
    ops.extend(laid.ops);
    laid.h
}

/// The space between two markdown blocks: list items sit closer together
/// than paragraphs, a heading has more room above it.
pub(crate) fn md_gap(previous: &MdBlock, block: &MdBlock) -> f32 {
    match (previous, block) {
        (MdBlock::Item(..), MdBlock::Item(..)) => 8.0,
        (_, MdBlock::Heading(..)) => PARAGRAPH_GAP + 10.0,
        _ => PARAGRAPH_GAP,
    }
}

/// Lays one markdown block out into `laid` from `top`; returns its bottom.
fn block_into(
    laid: &mut Laid,
    block: &MdBlock,
    x: f32,
    width: f32,
    top: f32,
    rich: Option<&Rich>,
) -> f32 {
    let p = theme::p();
    let c = theme::c();
    let mut y = top;
    match block {
        MdBlock::Paragraph(spans) => {
            let lines = wrap(spans, width, BODY);
            y += push_lines_linked(laid, &lines, x, y, BODY, BODY_LINE, None);
        }
        MdBlock::Heading(level, spans) => {
            let (ty, color) = match level {
                1 => (T_H1, p.accent),
                2 => (T_H2, p.accent),
                3 => (T_H3, p.secondary_accent),
                _ => (T_H4, p.secondary_accent),
            };
            let spans: Vec<Span> = spans
                .iter()
                .map(|span| Span {
                    style: if span.style == Style::Plain {
                        Style::Bold
                    } else {
                        span.style
                    },
                    ..span.clone()
                })
                .collect();
            let lines = wrap(&spans, width, ty.px());
            y += push_lines_linked(laid, &lines, x, y, ty.px(), ty.line(), Some(color));
        }
        MdBlock::Item(depth, marker, spans) => {
            // Markers 34 px in, 30 more per level; the text after them
            // (the iPhone's 12 pt list padding and its marker box).
            let indent = LIST_INDENT + f32::from(*depth) * MARKER_GAP;
            let marker_w = (text::measure(marker, Weight::Mono, BODY) + 14.0).max(46.0);
            let shown = if marker == "•" && *depth > 0 {
                "◦"
            } else {
                marker
            };
            laid.ops.push(ops::text(
                x + indent,
                y + (BODY_LINE + BODY * 0.74) / 2.0,
                shown,
                Weight::Mono,
                BODY,
                p.accent,
            ));
            let lines = wrap(spans, (width - indent - marker_w).max(BODY * 4.0), BODY);
            y += push_lines_linked(
                laid,
                &lines,
                x + indent + marker_w,
                y,
                BODY,
                BODY_LINE,
                None,
            );
        }
        MdBlock::Quote(spans) => {
            let lines = wrap(spans, width - QUOTE_INDENT, BODY);
            let h = push_lines_linked(
                laid,
                &lines,
                x + QUOTE_INDENT,
                y,
                BODY,
                BODY_LINE,
                Some(c.text_quote),
            );
            laid.ops
                .push(ops::rect(RectF::new(x, y, QUOTE_BAR, h), c.quote_bar));
            y += h;
        }
        MdBlock::Code { lang, lines } => {
            let wrapped = rich.is_some_and(|rich| rich.wrapped.contains(&code::key(lang, lines)));
            y = code::lay(laid, lang, lines, x, width, y, wrapped);
        }
        MdBlock::Table(grid) => {
            y = table::lay(laid, grid, x, width, y);
        }
        MdBlock::Math(tex) => {
            y = lay_display_math(laid, tex, x, width, y);
        }
        // Over the caps (PAR-134): the start, and the whole in the reader.
        MdBlock::Long { preview, full } => {
            let lines = wrap(&plain(&format!("{preview}…")), width, BODY);
            y += push_lines(&mut laid.ops, &lines, x, y, BODY, BODY_LINE, None, 1.0);
            let label = format!(
                "Read full paragraph ({} characters)",
                group_digits(full.chars().count())
            );
            y = more_button(
                laid,
                &label,
                Action::Ui(UiAct::Read {
                    title: "Paragraph".into(),
                    text: full.to_string(),
                }),
                x,
                y + MARKDOWN_GAP,
            );
        }
        MdBlock::More { total, full } => {
            let label = format!(
                "{}-piece preview · Open all {total} pieces",
                markdown::MAX_PIECES
            );
            y = more_button(
                laid,
                &label,
                Action::Ui(UiAct::Read {
                    title: "Response".into(),
                    text: full.to_string(),
                }),
                x,
                y,
            );
        }
        MdBlock::Rule => {
            laid.ops.push(ops::rect(
                RectF::new(x, y + 12.0, width, HAIRLINE),
                c.rule_strong,
            ));
            y += 26.0;
        }
    }
    y
}

/// Display math (§3.6.7): its rows in a monospaced grid (as the TUI lays
/// them out), centred, scrolled sideways when wider than the transcript;
/// "Copy LaTeX" under it. Returns the bottom.
fn lay_display_math(laid: &mut Laid, tex: &str, x: f32, width: f32, top: f32) -> f32 {
    let p = theme::p();
    let size = BODY;
    let line_h = (size * 1.3).round();
    let cell = text::measure("M", Weight::Mono, size).max(1.0);
    let columns = (width / cell).floor().max(8.0) as usize;
    let rows = math::display(tex, columns);
    let cols = rows
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    let grid_w = cols as f32 * cell;
    let left = if grid_w <= width {
        x + ((width - grid_w) / 2.0).round()
    } else {
        x
    };
    let mut row_ops = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let baseline = top + index as f32 * line_h + (line_h + size * 0.74) / 2.0;
        grid_text(&mut row_ops, row, left, baseline, size, cell, p.text);
    }
    let h = rows.len().max(1) as f32 * line_h;
    if grid_w > width {
        let key = super::super::images::hash_of(("math", tex));
        laid.ops.push(Op::Clip {
            rect: RectF::new(x, top - 8.0, width, h + 16.0),
            xscroll: Some(key),
            yscroll: None,
            ops: row_ops,
        });
        laid.xmax.push((key, (grid_w - width).ceil()));
    } else {
        laid.ops.extend(row_ops);
    }
    // "Copy LaTeX", a target under it.
    let ty = T_SMALL;
    let y = top + h + 4.0;
    let label = "Copy LaTeX";
    let label_w = text::measure(label, ty.weight, ty.px());
    laid.ops.push(ops::text(
        x + 6.0,
        y + (TARGET + ty.px() * 0.74) / 2.0,
        label,
        ty.weight,
        ty.px(),
        p.muted,
    ));
    laid.hits.push((
        RectF::new(x, y, (label_w + 12.0).max(TARGET), TARGET),
        Action::Ui(UiAct::Copy(tex.to_owned())),
    ));
    y + TARGET
}

/// A "Read full …" / "Open all …" button (§3.6.7): 122 high, a 2 px
/// border in `secondary_accent` at 40 %, its label in `T_SMALL`. Returns
/// its bottom.
fn more_button(laid: &mut Laid, label: &str, action: Action, x: f32, top: f32) -> f32 {
    let p = theme::p();
    let ty = T_SMALL;
    let w = (text::measure(label, ty.weight, ty.px()) + 56.0).max(TARGET);
    let rect = RectF::new(x, top, w, TARGET);
    laid.ops.push(Op::Border {
        rect,
        width: HAIRLINE,
        color: theme::mix(p.secondary_accent, 40, p.app_background),
    });
    laid.ops.push(ops::text(
        x + 28.0,
        top + (TARGET + ty.px() * 0.74) / 2.0,
        label,
        ty.weight,
        ty.px(),
        p.secondary_accent,
    ));
    laid.hits.push((rect, action));
    top + TARGET
}

/// 12345 as "12,345".
fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A row of a monospaced grid: ASCII runs as text, every other character
/// in its own cell (glyphs from fallback fonts are not a cell wide).
fn grid_text(
    ops: &mut Vec<Op>,
    row: &str,
    left: f32,
    baseline: f32,
    size: f32,
    cell: f32,
    color: Rgb,
) {
    let mut run = String::new();
    let mut run_start = 0;
    let flush = |ops: &mut Vec<Op>, run: &mut String, start: usize| {
        if !run.trim().is_empty() {
            ops.push(ops::text(
                left + start as f32 * cell,
                baseline,
                std::mem::take(run),
                Weight::Mono,
                size,
                color,
            ));
        }
        run.clear();
    };
    for (column, c) in row.chars().enumerate() {
        if c.is_ascii() {
            if run.is_empty() {
                run_start = column;
            }
            run.push(c);
        } else {
            flush(ops, &mut run, run_start);
            if !c.is_whitespace() {
                let glyph = c.to_string();
                let w = text::measure(&glyph, Weight::Mono, size);
                ops.push(ops::text(
                    (left + column as f32 * cell + (cell - w) / 2.0).round(),
                    baseline,
                    glyph,
                    Weight::Mono,
                    size,
                    color,
                ));
            }
        }
    }
    flush(ops, &mut run, run_start);
}
