//! TeX math as Unicode text (WP-C1, PAR-139): the TUI's math renderer
//! (exocortex `tui/src/markdown/mathparse.ts`, `mathlayout.ts`,
//! `mathsymbols.ts` and `math.ts`'s conversions) ported as is. Inline math
//! is one line (`x² + ½`); display math is rows in monospace columns, with
//! stacked fractions, stretchy delimiters, limits and aligned environments,
//! falling back to a linear form broken before relations when the width is
//! too small. Finding the delimiters in Markdown is the caller's.
//!
//! Columns are the TUI's (`textwidth.ts`, see `text.rs`): wide East Asian
//! and emoji glyphs take two, marks none, everything else one, including
//! the box-drawing and delimiter pieces the layout uses.

#![allow(dead_code)]

mod layout;
mod parse;
mod symbols;
mod text;
mod unicode;

pub use layout::DisplayLayout;
use layout::Layout;
use parse::{Guard, MAX_TREE_DEPTH, Node};

/// Longer sources are shown as they are.
const MAX_TEX_LENGTH: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Inline: no-break spaces (U+00A0) wherever TeX would not break the
    /// line, so a wrapper only splits after relations and operators (the
    /// TUI's prose rendering).
    pub nbsp: bool,
    /// Whether the renderer draws combining marks; otherwise accents
    /// without a precomposed character are written out, `bar(x)` (the TUI
    /// on st).
    pub combining_marks: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            nbsp: false,
            combining_marks: true,
        }
    }
}

/// TeX source (without delimiters) as one line; the trimmed source when
/// it cannot be rendered.
pub fn inline(tex: &str) -> String {
    inline_with(tex, Options::default())
}

pub fn inline_with(tex: &str, options: Options) -> String {
    let source = text::trim(tex);
    if source.is_empty() {
        return String::new();
    }
    if text::utf16_len(tex) > MAX_TEX_LENGTH {
        return source.to_string();
    }
    render(tex, options, |layout, nodes| {
        layout.inline(nodes, options.nbsp)
    })
    .unwrap_or_else(|| source.to_string())
}

/// A rendered row's grapheme clusters and their columns (wide glyphs
/// two, marks riding on their base): for drawing it on a grid.
pub fn cells(row: &str) -> Vec<(String, usize)> {
    text::cells(row)
}

/// Display rows, at most `width` columns wide when a narrower layout
/// exists (`usize::MAX`: unlimited); no rows for empty source.
pub fn display(tex: &str, width: usize) -> Vec<String> {
    display_layout(tex, width).rows
}

pub fn display_layout(tex: &str, width: usize) -> DisplayLayout {
    display_with(tex, width, Options::default())
}

pub fn display_with(tex: &str, width: usize, options: Options) -> DisplayLayout {
    let source = text::trim(tex);
    let plain = DisplayLayout {
        rows: vec![source.to_string()],
        last_base: 0,
    };
    if source.is_empty() {
        return DisplayLayout {
            rows: Vec::new(),
            last_base: 0,
        };
    }
    if text::utf16_len(tex) > MAX_TEX_LENGTH {
        return plain;
    }
    render(tex, options, |layout, nodes| layout.display(nodes, width)).unwrap_or(plain)
}

/// Parse and lay out, or None where the TUI would have thrown.
fn render<T>(
    tex: &str,
    options: Options,
    lay_out: impl FnOnce(&Layout, &[Node]) -> T,
) -> Option<T> {
    let guard = Guard::default();
    let nodes = parse::parse(tex, &guard);
    let result = if guard.failed.get() || parse::depth(&nodes) > MAX_TREE_DEPTH {
        None
    } else {
        let layout = Layout::new(options.combining_marks);
        let out = lay_out(&layout, &nodes);
        (!layout.failed()).then_some(out)
    };
    parse::dismantle(nodes);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_inline_and_display_math() {
        assert_eq!(inline(r"\frac12 + x^2 \le \sqrt{y}"), "½ + x² ≤ √y");
        assert_eq!(
            display(r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}", usize::MAX),
            [" n      n(n + 1)", " ∑  i = ────────", "i=1        2"]
        );
        assert_eq!(
            display("a = 1 \\\\ b = 22", usize::MAX),
            ["a = 1", "b = 22"]
        );
        assert_eq!(
            inline_with(
                r"a + b\,c",
                Options {
                    nbsp: true,
                    ..Options::default()
                }
            ),
            "a\u{A0}+ b\u{A0}c"
        );
        let plain = Options {
            combining_marks: false,
            ..Options::default()
        };
        assert_eq!(inline_with(r"\bar{x} + \hat{a}", plain), "bar(x) + â");
    }

    #[test]
    fn falls_back_to_the_source() {
        assert_eq!(inline("   "), "");
        assert!(display(" ", 40).is_empty());
        assert_eq!(inline(r" x \not "), r"x \not");
        assert_eq!(
            display(r"\begin{toString}a\end{toString}", 40),
            [r"\begin{toString}a\end{toString}"]
        );
        let long = "x+".repeat(MAX_TEX_LENGTH);
        assert_eq!(inline(&long), long);
    }

    /// Nesting the TUI renders out of its deep stack falls back here
    /// rather than overflowing a thread's: chains (`x^a^a…`, `a\over b\over
    /// …`, `((((…))))`) and recursion (`\sqrt\sqrt…`, `{{{…`) alike.
    #[test]
    fn deep_nesting_falls_back_without_overflowing() {
        let cases = [
            "x^a".repeat(5000),
            r"a\over ".repeat(2500),
            format!("{}\\frac{{a}}{{b}}{}", "(".repeat(5000), ")".repeat(5000)),
            format!("{}x", r"\sqrt".repeat(3000)),
            format!("{}x", r"\not".repeat(3000)),
            format!("{}x", r"\sqrt[\sqrt[".repeat(1500)),
            format!("{}x", r"\text{$".repeat(2000)),
            "{".repeat(10_000),
            r"\left(".repeat(3000),
            r"\begin{pmatrix}".repeat(1000),
            "x^{".repeat(5000),
        ];
        for tex in &cases {
            let shown = inline(tex);
            let rows = display(tex, 30);
            assert!(!shown.is_empty() && !rows.is_empty(), "{}", &tex[..40]);
        }
        let deep = format!("{}x", r"\sqrt".repeat(300));
        assert_eq!(inline(&deep), deep);
        let shallow = format!("{}{{x}}", r"\sqrt".repeat(30));
        assert_eq!(inline(&shallow), format!("{}x", "√".repeat(30)));
    }

    /// The TUI's own rendering of its math corpus (`fixture.jsonl`, made by
    /// `gen-math-fixture.ts` from exocortex `tui/src/markdown`: every
    /// fragment of `fixtures/math-cases.ts` and `fixtures/math-corpus.json`,
    /// every TeX string in `math.test.ts`, and edge cases). Per case: `t`
    /// the TeX, `i` inline, `n` inline with no-break spaces when it differs
    /// (the indices of the spaces that become U+00A0, or the text), `d`
    /// display `[width, last_base, rows]` at widths unlimited (0), 42, 20
    /// and 8, each listed when it changes (`w`: only these widths), and
    /// `c`/`cd` inline and unlimited display rows without combining marks
    /// when they differ.
    #[test]
    fn matches_the_tui_on_its_corpus() {
        const WIDTHS: [u64; 4] = [0, 42, 20, 8];
        let fixture = include_str!("fixture.jsonl");
        let mut checked = 0;
        let mut failures = Vec::new();
        for line in fixture.lines() {
            let case: serde_json::Value = serde_json::from_str(line).expect("fixture line");
            let tex = case["t"].as_str().expect("tex");
            let mut check = |what: &str, got: serde_json::Value, expected: &serde_json::Value| {
                if &got != expected {
                    failures.push(format!(
                        "{tex:?} {what}:\n  got      {got}\n  expected {expected}"
                    ));
                }
            };
            let expected_inline = &case["i"];
            check("inline", inline(tex).into(), expected_inline);
            let expected_nbsp = match case.get("n") {
                Some(serde_json::Value::Array(indices)) => {
                    let mut chars: Vec<char> = expected_inline
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .collect();
                    for index in indices.iter().filter_map(serde_json::Value::as_u64) {
                        chars[index as usize] = '\u{A0}';
                    }
                    chars.into_iter().collect::<String>().into()
                }
                Some(text) => text.clone(),
                None => expected_inline.clone(),
            };
            let nbsp = Options {
                nbsp: true,
                ..Options::default()
            };
            check("nbsp", inline_with(tex, nbsp).into(), &expected_nbsp);
            let entries = case["d"].as_array().expect("display");
            let widths = case.get("w").and_then(serde_json::Value::as_array);
            let widths: Vec<u64> = match widths {
                Some(widths) => widths
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .collect(),
                None => WIDTHS.to_vec(),
            };
            for (position, &width) in widths.iter().enumerate() {
                // The entry for the widest width listed at or before this one.
                let expected = entries
                    .iter()
                    .rev()
                    .find(|entry| {
                        widths[..=position].contains(&entry[0].as_u64().unwrap_or(u64::MAX))
                    })
                    .expect("display entry");
                let layout = display_layout(
                    tex,
                    if width == 0 {
                        usize::MAX
                    } else {
                        width as usize
                    },
                );
                let got = serde_json::json!([expected[0], layout.last_base, layout.rows]);
                check(&format!("display at {width}"), got, expected);
            }
            let plain = Options {
                combining_marks: false,
                ..Options::default()
            };
            check(
                "plain inline",
                inline_with(tex, plain).into(),
                case.get("c").unwrap_or(expected_inline),
            );
            let unlimited = &entries[0][2];
            check(
                "plain display",
                display_with(tex, usize::MAX, plain).rows.into(),
                case.get("cd").unwrap_or(unlimited),
            );
            checked += 1;
        }
        assert!(
            failures.is_empty(),
            "{} of {checked} cases differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
        assert!(checked > 450, "{checked} cases");
    }
}
