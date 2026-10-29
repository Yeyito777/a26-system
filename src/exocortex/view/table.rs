//! Tables (WP-C1, PAR-138, roadmap §3.6.7): a grid of 2 px rules with a
//! header row (`selection_background`, bold `secondary_accent`), cells
//! wrapped in their column and aligned as the delimiter row says, columns
//! 443 / 377 / 343 px wide for 2 / 3 / more columns, at most 12. A grid
//! wider than the transcript scrolls sideways; one with more than 10 rows
//! is at most 997 px tall and its rows scroll inside it under the header.
//! "Open full response" under it opens the reply in the reader.

use crate::gfx::{RectF, Rgb};

use super::super::images::hash_of;
use super::super::markdown::{Align, Span, Style, Table};
use super::super::model::{Action, rich};
use super::super::theme::{self, HAIRLINE, T_SMALL, T_TABLE, TARGET};
use super::markdown_layout::{Run, font, line_width, wrap};
use super::ops::{self, Laid, Op};

/// At most this many columns (the iPhone's).
pub const MAX_COLUMNS: usize = 12;
/// More rows than this scroll inside the table…
pub const TALL_ROWS: usize = 10;
/// …which is at most this tall (360 pt).
pub const MAX_HEIGHT: f32 = 997.0;
/// Cell padding (y, x): 5 × 9 pt.
const PAD: (f32, f32) = (14.0, 25.0);
/// Between the grid and what is under it (4 pt).
const GAP: f32 = 11.0;

/// A column's width for a table of `columns` (160 / 136 / 124 pt).
pub fn column_width(columns: usize) -> f32 {
    match columns {
        0..=2 => 443.0,
        3 => 377.0,
        _ => 343.0,
    }
}

/// What identifies a table (its scrollers, "Open full response").
pub fn key(table: &Table) -> u64 {
    hash_of(("table", table))
}

/// The vertical scroller of the table `key`.
pub fn rows_key(key: u64) -> u64 {
    hash_of(("table-rows", key))
}

/// Lays a table out at `x`, `width` wide, from `top`; returns its bottom.
pub fn lay(laid: &mut Laid, table: &Table, x: f32, width: f32, top: f32) -> f32 {
    let p = theme::p();
    let c = theme::c();
    let key = key(table);
    let total = table.columns();
    let columns = total.clamp(1, MAX_COLUMNS);
    let col_w = column_width(columns);
    let grid_w = columns as f32 * col_w + HAIRLINE;
    let size = T_TABLE.px();
    let line_h = T_TABLE.line();
    let text_w = col_w - 2.0 * PAD.1 - HAIRLINE;

    // Every row's cells wrapped, and its height (its bottom rule included).
    let lay_row = |cells: &[Vec<Span>], header: bool| -> (Vec<Vec<Vec<Run>>>, f32) {
        let wrapped: Vec<Vec<Vec<Run>>> = (0..columns)
            .map(|index| match cells.get(index) {
                Some(spans) if !spans.is_empty() => {
                    let spans: Vec<Span> = if header {
                        spans.iter().map(bold).collect()
                    } else {
                        spans.clone()
                    };
                    wrap(&spans, text_w, size)
                }
                _ => Vec::new(),
            })
            .collect();
        let lines = wrapped.iter().map(Vec::len).max().unwrap_or(0).max(1);
        (wrapped, lines as f32 * line_h + 2.0 * PAD.0 + HAIRLINE)
    };
    let (header, header_h) = lay_row(&table.header, true);
    let rows: Vec<(Vec<Vec<Vec<Run>>>, f32)> =
        table.rows.iter().map(|row| lay_row(row, false)).collect();
    let rows_h: f32 = rows.iter().map(|(_, h)| h).sum();
    let body_top = top + HAIRLINE + header_h;
    let tall = table.rows.len() > TALL_ROWS;
    let body_h = if tall {
        rows_h.min(MAX_HEIGHT - HAIRLINE - header_h)
    } else {
        rows_h
    };
    let grid_h = HAIRLINE + header_h + body_h;
    let view_w = grid_w.min(width);

    let mut grid = Vec::new();
    // The header: its fill, its rules, its cells.
    grid.push(ops::rect(
        RectF::new(x, top, grid_w, HAIRLINE + header_h),
        p.selection_background,
    ));
    grid.push(ops::rect(
        RectF::new(x, top, grid_w, HAIRLINE),
        c.rule_table,
    ));
    grid.push(ops::rect(
        RectF::new(x, body_top - HAIRLINE, grid_w, HAIRLINE),
        c.rule_table,
    ));
    cells(
        &mut grid,
        &header,
        &table.align,
        x,
        top + HAIRLINE,
        col_w,
        Some(p.secondary_accent),
    );
    // Column rules, the whole height.
    for index in 0..=columns {
        let rx = if index == 0 {
            x
        } else {
            x + HAIRLINE + index as f32 * col_w - HAIRLINE
        };
        grid.push(ops::rect(
            RectF::new(rx, top, HAIRLINE, grid_h),
            c.rule_table,
        ));
    }
    // The rows, scrolled inside a tall table.
    let mut body = Vec::new();
    let mut y = body_top;
    for (row, h) in &rows {
        cells(&mut body, row, &table.align, x, y, col_w, None);
        body.push(ops::rect(
            RectF::new(x, y + h - HAIRLINE, grid_w, HAIRLINE),
            c.rule_table,
        ));
        y += h;
    }
    if tall {
        let rows_key = rows_key(key);
        grid.push(Op::Clip {
            rect: RectF::new(x, body_top, grid_w, body_h),
            xscroll: None,
            yscroll: Some(rows_key),
            ops: body,
        });
        // Column rules over the rows, which have none of their own.
        for index in 1..columns {
            let rx = x + HAIRLINE + index as f32 * col_w - HAIRLINE;
            grid.push(ops::rect(
                RectF::new(rx, body_top, HAIRLINE, body_h),
                c.rule_table,
            ));
        }
        grid.push(ops::rect(
            RectF::new(x + grid_w - HAIRLINE, body_top, HAIRLINE, body_h),
            c.rule_table,
        ));
        grid.push(ops::rect(
            RectF::new(x, body_top, HAIRLINE, body_h),
            c.rule_table,
        ));
        grid.push(ops::rect(
            RectF::new(x, body_top + body_h - HAIRLINE, grid_w, HAIRLINE),
            c.rule_table,
        ));
        laid.ymax.push((
            rows_key,
            (rows_h - body_h).max(0.0).ceil(),
            RectF::new(x, body_top, view_w, body_h),
        ));
    } else {
        grid.extend(body);
    }
    let overflow = (grid_w - view_w).ceil();
    if overflow > 0.0 {
        laid.ops.push(Op::Clip {
            rect: RectF::new(x, top, view_w, grid_h),
            xscroll: Some(key),
            yscroll: None,
            ops: grid,
        });
        laid.xmax.push((key, overflow));
    } else {
        laid.ops.extend(grid);
    }
    let mut y = top + grid_h + GAP;
    if total > columns {
        let ty = T_SMALL;
        laid.ops.push(ops::text(
            x + 6.0,
            ty.baseline(y),
            format!("Showing {columns} of {total} columns"),
            ty.weight,
            ty.px(),
            p.muted,
        ));
        y += ty.line();
    }
    let ty = T_SMALL;
    let label = "Open full response";
    let label_w = crate::text::measure(label, ty.weight, ty.px());
    laid.ops.push(ops::text(
        x + 6.0,
        y + (TARGET + ty.px() * 0.74) / 2.0,
        label,
        ty.weight,
        ty.px(),
        p.secondary_accent,
    ));
    laid.hits.push((
        RectF::new(x, y, (label_w + 12.0).max(TARGET), TARGET),
        Action::Rich(rich::Act::OpenResponse(key)),
    ));
    y + TARGET
}

fn bold(span: &Span) -> Span {
    Span {
        style: if span.style == Style::Plain {
            Style::Bold
        } else {
            span.style
        },
        ..span.clone()
    }
}

/// A row's cells, aligned in their columns, from `top`.
#[allow(clippy::too_many_arguments)]
fn cells(
    ops: &mut Vec<Op>,
    row: &[Vec<Vec<Run>>],
    align: &[Align],
    x: f32,
    top: f32,
    col_w: f32,
    color: Option<Rgb>,
) {
    let c = theme::c();
    let size = T_TABLE.px();
    let line_h = T_TABLE.line();
    for (index, lines) in row.iter().enumerate() {
        let left = x + HAIRLINE + index as f32 * col_w;
        for (number, line) in lines.iter().enumerate() {
            let w = line_width(line);
            let inner = col_w - 2.0 * PAD.1 - HAIRLINE;
            let offset = match align.get(index).copied().unwrap_or_default() {
                Align::Left => 0.0,
                Align::Center => ((inner - w) / 2.0).max(0.0).round(),
                Align::Right => (inner - w).max(0.0).round(),
            };
            let base = T_TABLE.baseline(top + PAD.0 + number as f32 * line_h);
            for run in line {
                let (weight, fsize, default) = font(run.style, size);
                let rx = left + PAD.1 + offset + run.x;
                if run.style == Style::Code {
                    ops.push(ops::rect(
                        RectF::new(rx - 8.0, base - fsize * 0.86, run.w + 16.0, fsize * 1.24),
                        c.inline_code_bg,
                    ));
                }
                let color = match (color, run.style) {
                    (_, Style::Code | Style::Link) => default,
                    (Some(color), _) => color,
                    (None, _) => default,
                };
                ops.push(Op::Text {
                    x: rx,
                    baseline: base,
                    text: run.text.clone(),
                    weight,
                    size: fsize,
                    tracking: 0.0,
                    color,
                    alpha: if run.style == Style::Strike { 0.6 } else { 1.0 },
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::markdown::{MdBlock, parse};

    fn table(source: &str) -> Table {
        match parse(source).into_iter().next() {
            Some(MdBlock::Table(table)) => table,
            other => panic!("{other:?}"),
        }
    }

    fn texts(ops: &[Op], out: &mut Vec<(f32, String)>) {
        for op in ops {
            match op {
                Op::Text { x, text, .. } => out.push((*x, text.clone())),
                Op::Clip { ops, .. } => texts(ops, out),
                _ => {}
            }
        }
    }

    #[test]
    fn columns_are_sized_aligned_and_scroll_sideways_when_wide() {
        let t = table(
            "| Metric | Q2 | Q3 |\n|:--|--:|--:|\n| Revenue | 100 | 112 |\n| Costs | 80 | 76 |",
        );
        let mut laid = Laid::default();
        let bottom = lay(&mut laid, &t, 40.0, 1000.0, 0.0);
        // Three columns of 377 px: wider than the transcript, so it scrolls.
        assert_eq!(
            laid.xmax,
            vec![(key(&t), (3.0 * 377.0 + HAIRLINE - 1000.0_f32).ceil())]
        );
        let mut found = Vec::new();
        texts(&laid.ops, &mut found);
        let x_of = |s: &str| found.iter().find(|(_, t)| t == s).map(|(x, _)| *x).unwrap();
        // Left-aligned first column, right-aligned numbers.
        assert_eq!(x_of("Revenue"), 40.0 + HAIRLINE + PAD.1);
        let w = crate::text::measure("100", crate::text::Weight::Mono, T_TABLE.px());
        assert_eq!(
            x_of("100"),
            40.0 + HAIRLINE + 377.0 + (377.0 - 2.0 * PAD.1 - HAIRLINE - w).round() + PAD.1
        );
        // The header is bold, the button is under it.
        assert!(laid.ops.iter().any(|op| matches!(op, Op::Clip { ops, .. } if ops.iter().any(|op| matches!(op, Op::Text { text, weight: crate::text::Weight::MonoBold, .. } if text == "Metric")))));
        assert_eq!(
            laid.hits[0].1,
            Action::Rich(rich::Act::OpenResponse(key(&t)))
        );
        assert_eq!(bottom, laid.hits[0].0.y + TARGET);
        // Two columns fit: no scroller.
        let narrow = table("| a | b |\n|---|---|\n| 1 | 2 |");
        let mut laid = Laid::default();
        lay(&mut laid, &narrow, 40.0, 1000.0, 0.0);
        assert!(laid.xmax.is_empty());
    }

    #[test]
    fn a_tall_table_scrolls_its_rows_under_the_header() {
        let rows: String = (0..200).map(|i| format!("| r{i} | {i} |\n")).collect();
        let t = table(&format!("| Name | Value |\n|---|--:|\n{rows}"));
        let mut laid = Laid::default();
        let bottom = lay(&mut laid, &t, 40.0, 1000.0, 0.0);
        assert!(bottom <= MAX_HEIGHT + GAP + TARGET + 1.0, "{bottom}");
        assert_eq!(laid.ymax.len(), 1);
        let (rows_key, max, area) = laid.ymax[0];
        assert_eq!(rows_key, super::rows_key(key(&t)));
        assert!(max > 200.0 * 50.0, "{max}");
        assert!(area.y > 0.0 && area.h < MAX_HEIGHT);
        // A short table shows every row.
        let short: String = (0..10).map(|i| format!("| r{i} | {i} |\n")).collect();
        let t = table(&format!("| Name | Value |\n|---|--:|\n{short}"));
        let mut laid = Laid::default();
        lay(&mut laid, &t, 40.0, 1000.0, 0.0);
        assert!(laid.ymax.is_empty());
    }

    #[test]
    fn more_than_twelve_columns_say_so() {
        let header: String = (0..14).map(|i| format!("| c{i} ")).collect();
        let t = table(&format!("{header}|"));
        let mut laid = Laid::default();
        lay(&mut laid, &t, 40.0, 1000.0, 0.0);
        let mut found = Vec::new();
        texts(&laid.ops, &mut found);
        assert!(found.iter().any(|(_, t)| t == "Showing 12 of 14 columns"));
        assert!(!found.iter().any(|(_, t)| t == "c12"));
    }
}
