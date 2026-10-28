//! The markdown the Exocortex app draws: paragraphs, headings, bullet and
//! numbered lists, block quotes, rules, fenced code (and tables, kept as
//! monospaced lines), with bold, italic, inline code and link text inside.
//! It is tolerant of text still streaming in: an unclosed fence is code to
//! the end, an unclosed `**` is plain text. A newline inside a paragraph is a
//! line break, as chat apps show replies.
#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Bold,
    Italic,
    Code,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdBlock {
    Paragraph(Vec<Span>),
    Heading(u8, Vec<Span>),
    /// A list item: nesting depth, its marker ("•", "3."), and its text.
    Item(u8, String, Vec<Span>),
    Quote(Vec<Span>),
    /// Fenced code or a table: monospaced lines, never wrapped by words.
    Code(Vec<String>),
    Rule,
}

pub fn parse(text: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut quote: Vec<&str> = Vec::new();
    let mut lines = text.lines().peekable();
    let flush = |blocks: &mut Vec<MdBlock>, paragraph: &mut Vec<&str>, quote: &mut Vec<&str>| {
        if !paragraph.is_empty() {
            blocks.push(MdBlock::Paragraph(inline(&paragraph.join("\n"))));
            paragraph.clear();
        }
        if !quote.is_empty() {
            blocks.push(MdBlock::Quote(inline(&quote.join("\n"))));
            quote.clear();
        }
    };
    while let Some(raw) = lines.next() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if let Some(fence) = fence_of(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            let indent = line.len() - trimmed.len();
            let mut code = Vec::new();
            for raw in lines.by_ref() {
                let inner = raw.trim_end();
                if inner.trim_start().starts_with(fence)
                    && inner
                        .trim()
                        .trim_start_matches(fence.chars().next().unwrap_or('`'))
                        .is_empty()
                {
                    break;
                }
                // Drop the fence's own indentation from its lines.
                let strip = inner.len() - inner.trim_start().len();
                code.push(inner[strip.min(indent)..].replace('\t', "    "));
            }
            blocks.push(MdBlock::Code(code));
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut quote);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            if !paragraph.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut quote);
            }
            quote.push(rest.strip_prefix(' ').unwrap_or(rest));
            continue;
        }
        if !quote.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut quote);
        }
        if is_rule(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            blocks.push(MdBlock::Rule);
            continue;
        }
        if let Some((level, title)) = heading(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            blocks.push(MdBlock::Heading(level, inline(title)));
            continue;
        }
        if trimmed.starts_with('|') {
            flush(&mut blocks, &mut paragraph, &mut quote);
            let mut rows = vec![table_row(trimmed)];
            while let Some(next) = lines.peek() {
                let next = next.trim();
                if !next.starts_with('|') {
                    break;
                }
                rows.push(table_row(next));
                lines.next();
            }
            blocks.push(MdBlock::Code(align_table(rows)));
            continue;
        }
        let indent = line.len() - trimmed.len();
        if let Some((marker, rest)) = list_item(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            let depth = (indent / 2).min(4) as u8;
            blocks.push(MdBlock::Item(depth, marker, inline(rest)));
            continue;
        }
        // A wrapped list item's continuation line stays with it.
        if indent >= 2
            && paragraph.is_empty()
            && let Some(MdBlock::Item(_, _, spans)) = blocks.last_mut()
        {
            spans.push(Span {
                text: "\n".into(),
                style: Style::Plain,
            });
            spans.extend(inline(trimmed));
            continue;
        }
        paragraph.push(trimmed);
    }
    flush(&mut blocks, &mut paragraph, &mut quote);
    blocks
}

fn fence_of(line: &str) -> Option<&'static str> {
    if line.starts_with("```") {
        Some("```")
    } else if line.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|mark| compact.chars().all(|c| c == *mark))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if rest.is_empty() {
        return Some((level as u8, ""));
    }
    rest.strip_prefix(' ')
        .map(|title| (level as u8, title.trim().trim_end_matches('#').trim_end()))
}

fn list_item(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            // A task list item keeps its box.
            if let Some(rest) = rest.strip_prefix("[ ] ") {
                return Some(("☐".into(), rest));
            }
            if let Some(rest) = rest
                .strip_prefix("[x] ")
                .or_else(|| rest.strip_prefix("[X] "))
            {
                return Some(("☑".into(), rest));
            }
            return Some(("•".into(), rest));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &line[digits..];
        for close in [". ", ") "] {
            if let Some(text) = rest.strip_prefix(close) {
                return Some((format!("{}.", &line[..digits]), text));
            }
        }
    }
    None
}

fn table_row(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|cell| strip_inline(cell.trim()))
        .collect()
}

/// Pads cells into columns; the `|---|` separator becomes a rule of dashes.
fn align_table(rows: Vec<Vec<String>>) -> Vec<String> {
    let separator = |row: &Vec<String>| {
        row.iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':' | ' ')))
    };
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for row in rows.iter().filter(|row| !separator(row)) {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    rows.iter()
        .map(|row| {
            if separator(row) {
                return widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
            }
            let cells: Vec<String> = (0..columns)
                .map(|index| {
                    let cell = row.get(index).map_or("", String::as_str);
                    let pad = widths[index].saturating_sub(cell.chars().count());
                    format!("{cell}{}", " ".repeat(pad))
                })
                .collect();
            cells.join(" │ ").trim_end().to_owned()
        })
        .collect()
}

/// The text of inline markdown without its markers (for previews and tables).
pub fn strip_inline(text: &str) -> String {
    inline(text).into_iter().map(|span| span.text).collect()
}

/// Inline markup: `**bold**`, `__bold__`, `*italic*`, `_italic_`,
/// `` `code` ``, `[link](url)`, `~~strike~~` (drawn plain), `\` escapes.
pub fn inline(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    parse_inline(text, Style::Plain, &mut out);
    merge(out)
}

fn push(out: &mut Vec<Span>, text: &str, style: Style) {
    if !text.is_empty() {
        out.push(Span {
            text: text.to_owned(),
            style,
        });
    }
}

fn merge(spans: Vec<Span>) -> Vec<Span> {
    let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            Some(last) if last.style == span.style => last.text.push_str(&span.text),
            _ => merged.push(span),
        }
    }
    merged
}

fn parse_inline(text: &str, style: Style, out: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    let mut plain_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\\' if i + 1 < bytes.len() && bytes[i + 1].is_ascii_punctuation() => {
                push(out, &text[plain_start..i], style);
                plain_start = i + 1;
                i += 2;
            }
            b'`' => {
                let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
                let fence = &text[i..i + run];
                if let Some(close) = text[i + run..].find(fence) {
                    push(out, &text[plain_start..i], style);
                    let code = &text[i + run..i + run + close];
                    let code = if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') {
                        &code[1..code.len() - 1]
                    } else {
                        code
                    };
                    push(out, code, Style::Code);
                    i += run + close + run;
                    plain_start = i;
                } else {
                    i += run;
                }
            }
            b'*' | b'_' | b'~' => {
                let run = bytes[i..].iter().take_while(|b| **b == c).count();
                let marker_len = if c == b'~' {
                    if run < 2 {
                        i += run;
                        continue;
                    }
                    2
                } else {
                    run.min(3)
                };
                let marker = &text[i..i + marker_len];
                let after = text[i + marker_len..].chars().next();
                let before = text[..i].chars().next_back();
                let opens = after.is_some_and(|a| !a.is_whitespace())
                    && (c != b'_' || !before.is_some_and(char::is_alphanumeric));
                let close = opens
                    .then(|| closing(text, i + marker_len, marker, c))
                    .flatten();
                match close {
                    Some(end) => {
                        push(out, &text[plain_start..i], style);
                        let inner = &text[i + marker_len..end];
                        let inner_style = match (c, marker_len) {
                            (b'~', _) => style,
                            (_, 1) if style == Style::Plain => Style::Italic,
                            (_, 1) => style,
                            _ => Style::Bold,
                        };
                        parse_inline(inner, inner_style, out);
                        i = end + marker_len;
                        plain_start = i;
                    }
                    None => i += run,
                }
            }
            b'[' => {
                if let Some(close) = text[i + 1..].find("](")
                    && let Some(end) = text[i + 1 + close + 2..].find(')')
                {
                    push(out, &text[plain_start..i], style);
                    let label = &text[i + 1..i + 1 + close];
                    let mut inner = Vec::new();
                    parse_inline(label, Style::Link, &mut inner);
                    out.extend(inner.into_iter().map(|mut span| {
                        if span.style != Style::Code {
                            span.style = Style::Link;
                        }
                        span
                    }));
                    i = i + 1 + close + 2 + end + 1;
                    plain_start = i;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    push(out, &text[plain_start..], style);
}

/// Where the marker that closes one opened at `from` is: the end of a run
/// of the same character at least as long, not after whitespace, and for
/// `_` not inside a word. `**a *b***` closes its `**` with the run's last two.
fn closing(text: &str, from: usize, marker: &str, c: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] != c {
            i += 1;
            continue;
        }
        let run = bytes[i..].iter().take_while(|b| **b == c).count();
        let before = text[..i].chars().next_back();
        let after = text[i + run..].chars().next();
        if i > from
            && run >= marker.len()
            && before.is_some_and(|b| !b.is_whitespace())
            && (c != b'_' || !after.is_some_and(char::is_alphanumeric))
        {
            return Some(i + run - marker.len());
        }
        i += run;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
        }
    }

    #[test]
    fn inline_styles() {
        assert_eq!(
            inline("a **b** *c* `d` [e](http://x) f"),
            vec![
                span("a ", Style::Plain),
                span("b", Style::Bold),
                span(" ", Style::Plain),
                span("c", Style::Italic),
                span(" ", Style::Plain),
                span("d", Style::Code),
                span(" ", Style::Plain),
                span("e", Style::Link),
                span(" f", Style::Plain),
            ]
        );
        // Unclosed markers, snake_case and arithmetic stay text.
        assert_eq!(inline("2 * 3 = 6"), vec![span("2 * 3 = 6", Style::Plain)]);
        assert_eq!(inline("**open"), vec![span("**open", Style::Plain)]);
        assert_eq!(
            inline("snake_case_name"),
            vec![span("snake_case_name", Style::Plain)]
        );
        assert_eq!(
            inline("__b__ _i_"),
            vec![
                span("b", Style::Bold),
                span(" ", Style::Plain),
                span("i", Style::Italic)
            ]
        );
        assert_eq!(inline(r"\*not\*"), vec![span("*not*", Style::Plain)]);
        assert_eq!(inline("``a ` b``"), vec![span("a ` b", Style::Code)]);
        assert_eq!(inline("~~gone~~"), vec![span("gone", Style::Plain)]);
        assert_eq!(
            inline("**bold *and italic***"),
            vec![span("bold and italic", Style::Bold)]
        );
    }

    #[test]
    fn blocks() {
        let text = "# Title\nIntro line\nsecond line\n\n- one\n  - nested **x**\n3. three\n\n> quoted\n\n---\n```rust\nfn main() {}\n```\n| a | bb |\n|---|---|\n| ccc | d |\nafter";
        let blocks = parse(text);
        assert_eq!(
            blocks[0],
            MdBlock::Heading(1, vec![span("Title", Style::Plain)])
        );
        assert_eq!(
            blocks[1],
            MdBlock::Paragraph(vec![span("Intro line\nsecond line", Style::Plain)])
        );
        assert_eq!(
            blocks[2],
            MdBlock::Item(0, "•".into(), vec![span("one", Style::Plain)])
        );
        assert_eq!(
            blocks[3],
            MdBlock::Item(
                1,
                "•".into(),
                vec![span("nested ", Style::Plain), span("x", Style::Bold)]
            )
        );
        assert_eq!(
            blocks[4],
            MdBlock::Item(0, "3.".into(), vec![span("three", Style::Plain)])
        );
        assert_eq!(
            blocks[5],
            MdBlock::Quote(vec![span("quoted", Style::Plain)])
        );
        assert_eq!(blocks[6], MdBlock::Rule);
        assert_eq!(blocks[7], MdBlock::Code(vec!["fn main() {}".into()]));
        assert_eq!(
            blocks[8],
            MdBlock::Code(vec!["a   │ bb".into(), "────┼───".into(), "ccc │ d".into()])
        );
        assert_eq!(
            blocks[9],
            MdBlock::Paragraph(vec![span("after", Style::Plain)])
        );
    }

    #[test]
    fn streaming_text_is_tolerated() {
        // An unclosed fence is code to the end.
        assert_eq!(
            parse("Here:\n```\nlet x = 1;\nlet y"),
            vec![
                MdBlock::Paragraph(vec![span("Here:", Style::Plain)]),
                MdBlock::Code(vec!["let x = 1;".into(), "let y".into()])
            ]
        );
        assert_eq!(parse(""), vec![]);
        assert_eq!(parse("#"), vec![MdBlock::Heading(1, vec![])]);
        assert_eq!(strip_inline("**a** `b`"), "a b");
    }
}
