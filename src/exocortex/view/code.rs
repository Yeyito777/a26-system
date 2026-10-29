//! Code blocks (WP-C1, PAR-136, roadmap §3.6.7): a box with a header (the
//! language, WRAP / NO WRAP, OPEN, COPY) over the code in `T_CODE`, kept
//! as written and scrolled sideways (a clip the view offsets, so a drag
//! redraws only the block's rows) unless it wraps. Over 8000 characters
//! it shows a preview and says OPEN and COPY take the whole code. A fence
//! still streaming is code to the end of the text.

use crate::gfx::RectF;
use crate::text::{self, Weight};

use super::super::images::hash_of;
use super::super::model::{Action, UiAct, rich};
use super::super::theme::{self, HAIRLINE, T_CODE, T_EYEBROW, T_SMALL, TARGET};
use super::super::ui::widgets;
use super::ops::{self, Laid, Op};

/// The code shown before "Preview · …" (the iPhone's).
pub const PREVIEW_CHARS: usize = 8000;
/// The header's height.
pub const HEAD: f32 = TARGET;
/// Padding of the code (y, x).
const PAD: (f32, f32) = (28.0, 34.0);
/// The actions' label size (0.76 em).
const ACTION_SIZE: f32 = 30.0;

/// What identifies a code block (its wrap state and its scroller).
pub fn key(lang: &str, lines: &[String]) -> u64 {
    hash_of(("code", lang, lines))
}

/// The code as one text.
pub fn source(lines: &[String]) -> String {
    lines.join("\n")
}

/// Lays a code block out at `x`, `width` wide, from `top`; returns its
/// bottom.
pub fn lay(
    laid: &mut Laid,
    lang: &str,
    lines: &[String],
    x: f32,
    width: f32,
    top: f32,
    wrapped: bool,
) -> f32 {
    let p = theme::p();
    let c = theme::c();
    let key = key(lang, lines);
    let full = source(lines);
    let (shown, cut) = preview(lines);
    let size = T_CODE.px();
    let line_h = T_CODE.line();
    let advance = text::measure("M", Weight::Mono, size).max(1.0);
    let rows: Vec<String> = if wrapped {
        let columns = (((width - 2.0 * PAD.1) / advance).floor() as usize).max(8);
        let mut rows = Vec::new();
        for line in &shown {
            let characters: Vec<char> = line.chars().collect();
            if characters.is_empty() {
                rows.push(String::new());
            }
            for chunk in characters.chunks(columns) {
                rows.push(chunk.iter().collect());
            }
        }
        rows
    } else {
        shown
    };
    let rows_h = rows.len().max(1) as f32 * line_h;
    let note_h = if cut { T_SMALL.line() + 8.0 } else { 0.0 };
    let body_top = top + HEAD + HAIRLINE;
    let h = HEAD + HAIRLINE + 2.0 * PAD.0 + rows_h + note_h;
    let frame = RectF::new(x, top, width, h);
    laid.ops.push(ops::rect(frame, c.code_bg));

    // The header: the language, then the actions from the right.
    let labels = [if wrapped { "NO WRAP" } else { "WRAP" }, "OPEN", "COPY"];
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| (text::measure(label, Weight::Mono, ACTION_SIZE) + 44.0).max(TARGET))
        .collect();
    let mut right = x + width - HAIRLINE;
    let mut cells = Vec::new();
    for (label, w) in labels.iter().zip(&widths).rev() {
        right -= w;
        cells.push((*label, right, *w));
    }
    cells.reverse();
    let baseline = |size: f32| top + (HEAD + size * 0.74) / 2.0;
    let label_room = cells.first().map_or(width, |cell| cell.1) - (x + 28.0) - 11.0;
    let label = if lang.is_empty() {
        "CODE".to_owned()
    } else {
        lang.to_uppercase()
    };
    let ty = T_EYEBROW;
    let label = widgets::fit(&label, ty, label_room);
    laid.ops.push(Op::Text {
        x: x + 28.0,
        baseline: baseline(ty.px()),
        text: label,
        weight: ty.weight,
        size: ty.px(),
        tracking: ty.tracking(),
        color: p.muted,
        alpha: 1.0,
    });
    for (index, (label, left, w)) in cells.iter().enumerate() {
        let tw = text::measure(label, Weight::Mono, ACTION_SIZE);
        laid.ops.push(ops::text(
            (left + (w - tw) / 2.0).round(),
            baseline(ACTION_SIZE),
            *label,
            Weight::Mono,
            ACTION_SIZE,
            p.secondary_accent,
        ));
        let action = match index {
            0 => Action::Rich(rich::Act::ToggleWrap(key)),
            1 => Action::Ui(UiAct::Read {
                title: title(lang),
                text: full.clone(),
            }),
            _ => Action::Ui(UiAct::Copy(full.clone())),
        };
        laid.hits.push((RectF::new(*left, top, *w, HEAD), action));
    }
    laid.ops.push(ops::rect(
        RectF::new(x + HAIRLINE, top + HEAD, width - 2.0 * HAIRLINE, HAIRLINE),
        c.rule_code_head,
    ));

    // The code.
    let mut text_ops = Vec::with_capacity(rows.len());
    let mut widest = 0.0_f32;
    for (index, row) in rows.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        let y = body_top + PAD.0 + index as f32 * line_h;
        widest = widest.max(row.chars().count() as f32 * advance);
        text_ops.push(ops::text(
            x + PAD.1,
            T_CODE.baseline(y),
            row.clone(),
            Weight::Mono,
            size,
            p.text,
        ));
    }
    let clip = RectF::new(
        x + HAIRLINE,
        body_top,
        width - 2.0 * HAIRLINE,
        h - HEAD - 2.0 * HAIRLINE,
    );
    // How far the code reaches past the clip's right edge.
    let overflow = (2.0 * PAD.1 + widest - HAIRLINE - clip.w).max(0.0).ceil();
    if wrapped || overflow <= 0.0 {
        laid.ops.extend(text_ops);
    } else {
        laid.ops.push(Op::Clip {
            rect: clip,
            xscroll: Some(key),
            yscroll: None,
            ops: text_ops,
        });
        laid.xmax.push((key, overflow));
    }
    if cut {
        let ty = T_SMALL;
        laid.ops.push(ops::text(
            x + PAD.1,
            ty.baseline(body_top + PAD.0 + rows_h),
            "Preview · OPEN/COPY includes the complete code.",
            ty.weight,
            ty.px(),
            p.muted,
        ));
    }
    laid.ops.push(Op::Border {
        rect: frame,
        width: HAIRLINE,
        color: c.rule_code,
    });
    top + h
}

/// The reader's title for a block of code.
fn title(lang: &str) -> String {
    if lang.is_empty() {
        "Code".into()
    } else {
        format!("{lang} code")
    }
}

/// The lines shown: all, or the first `PREVIEW_CHARS` characters (and
/// whether that cut any).
fn preview(lines: &[String]) -> (Vec<String>, bool) {
    let mut left = PREVIEW_CHARS;
    let mut shown = Vec::new();
    for line in lines {
        let count = line.chars().count();
        if count + 1 > left {
            if left > 0 {
                shown.push(line.chars().take(left).collect());
            }
            return (shown, true);
        }
        left -= count + 1;
        shown.push(line.clone());
    }
    (shown, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize, width: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("{i:04} {}", "x".repeat(width)))
            .collect()
    }

    #[test]
    fn a_long_line_scrolls_sideways_unless_wrapped() {
        let code = lines(4, 120);
        let mut laid = Laid::default();
        let bottom = lay(&mut laid, "rust", &code, 40.0, 1000.0, 0.0, false);
        assert_eq!(bottom, HEAD + HAIRLINE + 2.0 * PAD.0 + 4.0 * T_CODE.line());
        assert_eq!(laid.xmax.len(), 1);
        let (key, max) = laid.xmax[0];
        assert_eq!(key, super::key("rust", &code));
        assert!(max > 1000.0, "{max}");
        assert!(laid.ops.iter().any(
            |op| matches!(op, Op::Clip { xscroll: Some(k), ops, .. } if *k == key && ops.len() == 4)
        ));
        // The actions: WRAP toggles, OPEN reads, COPY copies the code.
        let actions: Vec<&Action> = laid.hits.iter().map(|(_, a)| a).collect();
        assert_eq!(actions[0], &Action::Rich(rich::Act::ToggleWrap(key)));
        assert!(
            matches!(actions[1], Action::Ui(UiAct::Read { title, text }) if title == "rust code" && text == &code.join("\n"))
        );
        assert!(matches!(actions[2], Action::Ui(UiAct::Copy(text)) if text == &code.join("\n")));
        // Header actions sit side by side, 122 wide at least, at the right.
        assert!(laid.hits[2].0.x + laid.hits[2].0.w <= 1040.0);
        assert!(laid.hits.iter().all(|(rect, _)| rect.w >= TARGET));
        // Wrapped: more rows, no scroller.
        let mut wrapped = Laid::default();
        let bottom = lay(&mut wrapped, "rust", &code, 40.0, 1000.0, 0.0, true);
        assert!(wrapped.xmax.is_empty());
        assert!(bottom > HEAD + 8.0 * T_CODE.line());
        assert!(
            wrapped
                .ops
                .iter()
                .any(|op| matches!(op, Op::Text { text, .. } if text == "NO WRAP"))
        );
    }

    #[test]
    fn over_8000_characters_shows_a_preview_but_opens_and_copies_it_all() {
        let code = lines(2000, 60);
        let mut laid = Laid::default();
        let bottom = lay(&mut laid, "", &code, 40.0, 1000.0, 0.0, false);
        let shown = laid
            .ops
            .iter()
            .find_map(|op| match op {
                Op::Clip { ops, .. } => Some(ops.len()),
                _ => None,
            })
            .unwrap_or(0);
        assert!(shown > 100 && shown < 150, "{shown}");
        assert!(bottom < 160.0 * T_CODE.line());
        assert!(
            laid.ops
                .iter()
                .any(|op| matches!(op, Op::Text { text, .. } if text.starts_with("Preview")))
        );
        assert!(
            laid.ops
                .iter()
                .any(|op| matches!(op, Op::Text { text, .. } if text == "CODE"))
        );
        assert!(
            matches!(&laid.hits[2].1, Action::Ui(UiAct::Copy(text)) if text.len() == code.join("\n").len())
        );
    }
}
