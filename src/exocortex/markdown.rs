//! The markdown the Exocortex app draws (GFM as the iPhone renders it,
//! PAR-135–138): paragraphs, headings, bullet, numbered and task lists
//! nested to 12 levels, block quotes, rules, fenced (``` and ~~~) and
//! indented code with its language, and tables with their alignment, with
//! bold, italic, strikethrough, inline code and link text inside. It is
//! tolerant of text still streaming in: an unclosed fence is code to the
//! end, an unclosed `**` is plain text, a table shows from its first row.
//! A newline inside a paragraph is a line break, as chat apps show replies.
#![allow(dead_code)]

use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Plain,
    Bold,
    Italic,
    Code,
    Link,
    /// `~~struck through~~`.
    Strike,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Span {
    pub text: String,
    pub style: Style,
    /// A link's target as written (`Style::Link` only): a web address or a
    /// local file (`link_kind`).
    pub link: Option<Rc<str>>,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
        }
    }
}

/// What a link points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// Opened in the Browser (http, https; `www.` made https).
    Web(String),
    /// A mail address or telephone number (`mailto:`, `tel:`): copied.
    Other(String),
    /// A path on the daemon's host (relative, `/abs`, `~/`, `file://`).
    File(String),
}

/// What a link target is (render.ts `safeHref` / `localFileTarget`); None:
/// not a link at all (its text shows plain).
pub fn link_kind(target: &str) -> Option<LinkKind> {
    let target = target.trim();
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") {
        return Some(LinkKind::Web(target.to_owned()));
    }
    if lower.starts_with("www.") {
        return Some(LinkKind::Web(format!("https://{target}")));
    }
    if lower.starts_with("mailto:") || lower.starts_with("tel:") || lower.starts_with("exocortex:")
    {
        return Some(LinkKind::Other(target.to_owned()));
    }
    local_file(target).map(LinkKind::File)
}

/// A local file link's path (shared/src/file-links.ts): relative, `/abs`,
/// `~/…` or `file://` (localhost) paths; no other schemes, network paths,
/// queries or fragments.
fn local_file(target: &str) -> Option<String> {
    let control = |s: &str| s.chars().any(char::is_control);
    if target.is_empty() || control(target) || target.starts_with('#') || target.starts_with("//") {
        return None;
    }
    let has_scheme = |s: &str| {
        let scheme: String = s.chars().take_while(|c| *c != ':').collect();
        s.len() > scheme.len()
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    };
    let path = if target.to_ascii_lowercase().starts_with("file:") {
        let rest = &target[5..];
        let rest = rest.strip_prefix("//").map_or(Some(rest), |after| {
            // file://host/path: only localhost (or no host).
            let host_end = after.find('/').unwrap_or(after.len());
            let host = &after[..host_end];
            (host.is_empty() || host.eq_ignore_ascii_case("localhost")).then(|| &after[host_end..])
        })?;
        if rest.contains('?') || rest.contains('#') {
            return None;
        }
        percent_decode(rest)?
    } else {
        if has_scheme(target) {
            return None;
        }
        percent_decode(target)?
    };
    if path.trim().is_empty() || control(&path) || path.starts_with("//") || has_scheme(&path) {
        return None;
    }
    Some(path)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A table column's alignment (from its delimiter row).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A table: its header cells, its rows' cells (inline markdown each) and
/// its columns' alignment. Rows may have fewer or more cells than the
/// header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Table {
    pub align: Vec<Align>,
    pub header: Vec<Vec<Span>>,
    pub rows: Vec<Vec<Vec<Span>>>,
}

impl Table {
    /// Its columns: the most cells any row has.
    pub fn columns(&self) -> usize {
        self.rows
            .iter()
            .map(Vec::len)
            .chain([self.header.len(), self.align.len()])
            .max()
            .unwrap_or(0)
    }

    /// As aligned monospaced text (the reader, copying, the bridge).
    pub fn text(&self) -> String {
        let columns = self.columns();
        let plain = |cells: &[Vec<Span>]| -> Vec<String> {
            (0..columns)
                .map(|index| {
                    cells
                        .get(index)
                        .map(|spans| spans.iter().map(|span| span.text.as_str()).collect())
                        .unwrap_or_default()
                })
                .collect()
        };
        let mut rows = vec![plain(&self.header)];
        rows.extend(self.rows.iter().map(|row| plain(row)));
        let mut widths = vec![0; columns];
        for row in &rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
        let line = |row: &[String]| -> String {
            row.iter()
                .enumerate()
                .map(|(index, cell)| {
                    let pad = widths[index].saturating_sub(cell.chars().count());
                    match self.align.get(index).copied().unwrap_or_default() {
                        Align::Right => format!("{}{cell}", " ".repeat(pad)),
                        Align::Center => {
                            format!("{}{cell}{}", " ".repeat(pad / 2), " ".repeat(pad - pad / 2))
                        }
                        Align::Left => format!("{cell}{}", " ".repeat(pad)),
                    }
                })
                .collect::<Vec<_>>()
                .join(" │ ")
                .trim_end()
                .to_owned()
        };
        let mut out = vec![line(&rows[0])];
        out.push(
            widths
                .iter()
                .map(|w| "─".repeat(*w))
                .collect::<Vec<_>>()
                .join("─┼─"),
        );
        out.extend(rows[1..].iter().map(|row| line(row)));
        out.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MdBlock {
    Paragraph(Vec<Span>),
    Heading(u8, Vec<Span>),
    /// A list item: nesting depth (0 to 11), its marker ("•", "3.", "☐",
    /// "☑"), and its text.
    Item(u8, String, Vec<Span>),
    Quote(Vec<Span>),
    /// Fenced or indented code: its language ("" when none given) and its
    /// lines (tabs as four spaces), never wrapped by words.
    Code {
        lang: String,
        lines: Vec<String>,
    },
    Table(Table),
    Rule,
}

/// Lists nest at most this deep.
pub const MAX_DEPTH: u8 = 11;
/// A table cell keeps at most this many characters (the iPhone's).
pub const CELL_MAX_CHARS: usize = 2000;

pub fn parse(text: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut quote: Vec<&str> = Vec::new();
    // The indents of the list items that are open, outermost first.
    let mut list: Vec<usize> = Vec::new();
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
        let indent = indent_of(line);
        if let Some((fence, run)) = fence_of(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            list.clear();
            let lang = trimmed[run..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c| c == '{' || c == '}' || c == '.')
                .to_owned();
            let strip = line.len() - trimmed.len();
            let mut code = Vec::new();
            for raw in lines.by_ref() {
                let inner = raw.trim_end();
                let inner_trimmed = inner.trim_start();
                if inner_trimmed.starts_with(&fence[..1].repeat(run))
                    && inner_trimmed
                        .trim_start_matches(&fence[..1])
                        .trim()
                        .is_empty()
                {
                    break;
                }
                // Drop the fence's own indentation from its lines.
                let leading = inner.len() - inner_trimmed.len();
                code.push(detab(&inner[leading.min(strip)..]));
            }
            blocks.push(MdBlock::Code { lang, lines: code });
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut quote);
            continue;
        }
        // Indented code: four spaces, not inside a list or a paragraph.
        if indent >= 4 && paragraph.is_empty() && quote.is_empty() && list.is_empty() {
            let mut code = vec![detab(line).chars().skip(4).collect::<String>()];
            while let Some(next) = lines.peek() {
                let next = next.trim_end();
                if !next.trim().is_empty() && indent_of(next) < 4 {
                    break;
                }
                code.push(detab(next).chars().skip(4).collect());
                lines.next();
            }
            while code.last().is_some_and(String::is_empty) {
                code.pop();
            }
            blocks.push(MdBlock::Code {
                lang: String::new(),
                lines: code,
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            if !paragraph.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut quote);
            }
            list.clear();
            quote.push(rest.strip_prefix(' ').unwrap_or(rest));
            continue;
        }
        if !quote.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut quote);
        }
        if is_rule(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            list.clear();
            blocks.push(MdBlock::Rule);
            continue;
        }
        if let Some((level, title)) = heading(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            list.clear();
            blocks.push(MdBlock::Heading(level, inline(title)));
            continue;
        }
        let delimiter_next = lines.peek().is_some_and(|next| is_delimiter_row(next));
        if (trimmed.starts_with('|') || (trimmed.contains('|') && delimiter_next))
            && paragraph.is_empty()
        {
            flush(&mut blocks, &mut paragraph, &mut quote);
            list.clear();
            let mut rows = vec![table_cells(trimmed)];
            let mut align = Vec::new();
            if delimiter_next {
                align = lines.next().map(delimiter_align).unwrap_or_default();
            }
            while let Some(next) = lines.peek() {
                let next = next.trim();
                if next.is_empty() || !next.contains('|') {
                    break;
                }
                if !next.starts_with('|') && align.is_empty() {
                    break;
                }
                rows.push(table_cells(next));
                lines.next();
            }
            let header = rows.remove(0);
            blocks.push(MdBlock::Table(Table {
                align,
                header,
                rows,
            }));
            continue;
        }
        if let Some((marker, rest)) = list_item(trimmed) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            while list.last().is_some_and(|open| *open > indent) {
                list.pop();
            }
            if list.last() != Some(&indent) {
                list.push(indent);
            }
            let depth = (list.len() - 1).min(usize::from(MAX_DEPTH)) as u8;
            blocks.push(MdBlock::Item(depth, marker, inline(rest)));
            continue;
        }
        // A wrapped list item's continuation line stays with it.
        if indent >= 2
            && paragraph.is_empty()
            && let Some(MdBlock::Item(_, _, spans)) = blocks.last_mut()
        {
            spans.push(Span::new("\n", Style::Plain));
            spans.extend(inline(trimmed));
            continue;
        }
        if indent == 0 {
            list.clear();
        }
        paragraph.push(trimmed);
    }
    flush(&mut blocks, &mut paragraph, &mut quote);
    blocks
}

/// Leading whitespace in columns (a tab is four).
fn indent_of(line: &str) -> usize {
    let mut columns = 0;
    for c in line.chars() {
        match c {
            ' ' => columns += 1,
            '\t' => columns += 4 - columns % 4,
            _ => break,
        }
    }
    columns
}

fn detab(line: &str) -> String {
    if line.contains('\t') {
        line.replace('\t', "    ")
    } else {
        line.to_owned()
    }
}

/// A fence's character and run length (three or more backticks or tildes).
fn fence_of(line: &str) -> Option<(&'static str, usize)> {
    for fence in ["`", "~"] {
        let run = line.chars().take_while(|c| c.to_string() == fence).count();
        if run >= 3 {
            // A backtick fence's info string has no backticks.
            if fence == "`" && line[run..].contains('`') {
                return None;
            }
            return Some((if fence == "`" { "```" } else { "~~~" }, run));
        }
    }
    None
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

/// `|---|:--:|--:|`: a table's delimiter row.
fn is_delimiter_row(line: &str) -> bool {
    let cells = split_cells(line.trim());
    !cells.is_empty()
        && line.contains('-')
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty()
                && cell.trim_matches(':').chars().all(|c| c == '-')
                && cell.contains('-')
        })
}

fn delimiter_align(line: &str) -> Vec<Align> {
    split_cells(line.trim())
        .iter()
        .map(|cell| {
            let cell = cell.trim();
            match (cell.starts_with(':'), cell.ends_with(':')) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            }
        })
        .collect()
}

/// A row's cells, split at pipes that are not escaped or in code spans.
fn split_cells(line: &str) -> Vec<String> {
    let inner = line.strip_prefix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut code = false;
    let mut escaped = false;
    for c in inner.chars() {
        match c {
            _ if escaped => {
                if c != '|' {
                    cell.push('\\');
                }
                cell.push(c);
                escaped = false;
            }
            '\\' => escaped = true,
            '`' => {
                code = !code;
                cell.push(c);
            }
            '|' if !code => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(c),
        }
    }
    if escaped {
        cell.push('\\');
    }
    if !cell.trim().is_empty() || cells.is_empty() {
        cells.push(cell);
    }
    cells
}

fn table_cells(line: &str) -> Vec<Vec<Span>> {
    split_cells(line)
        .iter()
        .map(|cell| {
            let cell = cell.trim();
            if cell.chars().count() > CELL_MAX_CHARS {
                let cut: String = cell.chars().take(CELL_MAX_CHARS).collect();
                vec![Span::new(format!("{cut}…"), Style::Plain)]
            } else {
                inline(cell)
            }
        })
        .collect()
}

/// The text of inline markdown without its markers (for previews and tables).
pub fn strip_inline(text: &str) -> String {
    inline(text).into_iter().map(|span| span.text).collect()
}

/// Inline markup: `**bold**`, `__bold__`, `*italic*`, `_italic_`,
/// `` `code` ``, `[link](url)`, `![alt](src)` (a link), `<https://…>`,
/// bare `https://…` and `www.…` addresses, `~~strike~~`, `\` escapes.
pub fn inline(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    parse_inline(text, Style::Plain, &mut out);
    merge(bare_links(out))
}

fn push(out: &mut Vec<Span>, text: &str, style: Style) {
    if !text.is_empty() {
        out.push(Span::new(text, style));
    }
}

fn merge(spans: Vec<Span>) -> Vec<Span> {
    let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            Some(last) if last.style == span.style && last.link == span.link => {
                last.text.push_str(&span.text)
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// Bare web addresses in plain text become links (GFM's autolink
/// literals): `http://`, `https://` or `www.` after a space or the start,
/// up to a space, without trailing punctuation or an unbalanced `)`.
fn bare_links(spans: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if span.style != Style::Plain || !(span.text.contains("://") || span.text.contains("www."))
        {
            out.push(span);
            continue;
        }
        let text = span.text.as_str();
        let mut plain_start = 0;
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            let starts = ["https://", "http://", "www."]
                .iter()
                .any(|p| rest.len() > p.len() && rest[..p.len()].eq_ignore_ascii_case(p));
            let at_word = text[..i].chars().next_back().is_none_or(|c| {
                c.is_whitespace() || matches!(c, '(' | '*' | '_' | '~' | '"' | '\'')
            });
            if starts && at_word {
                let mut end = rest
                    .find(|c: char| c.is_whitespace() || c == '<')
                    .unwrap_or(rest.len());
                // Trailing punctuation is not part of it; nor is a `)` it
                // does not open.
                loop {
                    let url = &rest[..end];
                    let Some(last) = url.chars().next_back() else {
                        break;
                    };
                    let unbalanced =
                        last == ')' && url.matches('(').count() < url.matches(')').count();
                    if matches!(
                        last,
                        '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"' | ';'
                    ) || unbalanced
                    {
                        end -= last.len_utf8();
                    } else {
                        break;
                    }
                }
                let url = &rest[..end];
                let domain = url.split("://").last().unwrap_or(url);
                if domain.len() > 4 && domain.contains('.') {
                    if i > plain_start {
                        out.push(Span::new(&text[plain_start..i], Style::Plain));
                    }
                    out.push(Span {
                        text: url.to_owned(),
                        style: Style::Link,
                        link: Some(Rc::from(url)),
                    });
                    i += end;
                    plain_start = i;
                    continue;
                }
            }
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
        if plain_start < text.len() {
            out.push(Span::new(&text[plain_start..], Style::Plain));
        }
    }
    out
}

/// The label and target of a link starting at `[` (`at`): `[label](dest)`
/// with balanced brackets and parentheses, the destination's title left
/// out; and where it ends.
fn link_at(text: &str, at: usize) -> Option<(&str, &str, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut i = at;
    let label_end = loop {
        match bytes.get(i)? {
            b'\\' => i += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break i;
                }
            }
            _ => {}
        }
        i += 1;
    };
    if bytes.get(label_end + 1) != Some(&b'(') {
        return None;
    }
    let mut depth = 0;
    let mut j = label_end + 1;
    let dest_end = loop {
        match bytes.get(j)? {
            b'\\' => j += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    break j;
                }
            }
            b'\n' => return None,
            _ => {}
        }
        j += 1;
    };
    let dest = text[label_end + 2..dest_end].trim();
    let dest = match dest.strip_prefix('<') {
        Some(rest) => rest.split('>').next().unwrap_or(""),
        None => dest.split_whitespace().next().unwrap_or(""),
    };
    Some((&text[at + 1..label_end], dest, dest_end + 1))
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
                            (b'~', _) if style == Style::Code => style,
                            (b'~', _) => Style::Strike,
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
                if let Some((label, dest, end)) = link_at(text, i) {
                    // `![alt](src)`: an image, shown as a link to it.
                    let image = i > plain_start && bytes[i - 1] == b'!';
                    let start = if image { i - 1 } else { i };
                    push(out, &text[plain_start..start], style);
                    let label = if image && label.trim().is_empty() {
                        "image"
                    } else {
                        label
                    };
                    let target = link_kind(dest).map(|_| Rc::<str>::from(dest));
                    let mut inner = Vec::new();
                    parse_inline(label, style, &mut inner);
                    out.extend(inner.into_iter().map(|mut span| {
                        // A target that is no link leaves its text plain.
                        if target.is_some() && span.style != Style::Code {
                            span.style = Style::Link;
                            span.link.clone_from(&target);
                        }
                        span
                    }));
                    i = end;
                    plain_start = i;
                } else {
                    i += 1;
                }
            }
            // `<https://…>`, `<mailto:…>`: an address as written.
            b'<' => {
                let close = text[i + 1..].find('>').map(|at| i + 1 + at);
                let dest = close.map(|end| &text[i + 1..end]);
                match dest.filter(|d| {
                    !d.contains(char::is_whitespace)
                        && matches!(link_kind(d), Some(LinkKind::Web(_) | LinkKind::Other(_)))
                        && d.contains(':')
                }) {
                    Some(dest) => {
                        push(out, &text[plain_start..i], style);
                        out.push(Span {
                            text: dest.to_owned(),
                            style: Style::Link,
                            link: Some(Rc::from(dest)),
                        });
                        i = close.unwrap_or(i) + 1;
                        plain_start = i;
                    }
                    None => i += 1,
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

    fn plain(text: &str) -> Vec<Span> {
        vec![span(text, Style::Plain)]
    }

    #[test]
    fn lists_nest_by_indentation_to_twelve_levels() {
        // Four-space nesting is one level per step, as two-space is.
        let blocks = parse("- a\n    - b\n        - c\n    - d\n- e");
        let depths: Vec<u8> = blocks
            .iter()
            .map(|b| match b {
                MdBlock::Item(depth, ..) => *depth,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(depths, [0, 1, 2, 1, 0]);
        let deep: String = (0..14)
            .map(|level| format!("{}- l{level}\n", "  ".repeat(level)))
            .collect();
        let depths: Vec<u8> = parse(&deep)
            .iter()
            .map(|b| match b {
                MdBlock::Item(depth, ..) => *depth,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(depths[11], 11);
        assert_eq!(depths[13], MAX_DEPTH);
        // Task items, and a numbered item nested in a bullet.
        assert_eq!(
            parse("- [x] done\n- [ ] todo\n  1. first"),
            vec![
                MdBlock::Item(0, "☑".into(), plain("done")),
                MdBlock::Item(0, "☐".into(), plain("todo")),
                MdBlock::Item(1, "1.".into(), plain("first")),
            ]
        );
    }

    #[test]
    fn tildes_indented_code_and_languages() {
        assert_eq!(
            parse("~~~python extra\nprint(1)\n  ~~~"),
            vec![MdBlock::Code {
                lang: "python".into(),
                lines: vec!["print(1)".into()]
            }]
        );
        assert_eq!(
            parse("Text\n\n    let a = 1;\n\n    let b;\n\nAfter"),
            vec![
                MdBlock::Paragraph(plain("Text")),
                MdBlock::Code {
                    lang: String::new(),
                    lines: vec!["let a = 1;".into(), "".into(), "let b;".into()]
                },
                MdBlock::Paragraph(plain("After")),
            ]
        );
        // Inside a list, indentation continues the item.
        assert_eq!(
            parse("- item\n\n    more"),
            vec![MdBlock::Item(
                0,
                "•".into(),
                vec![
                    span("item", Style::Plain),
                    span("\n", Style::Plain),
                    span("more", Style::Plain)
                ]
            )]
        );
        // A longer fence holds a shorter one.
        assert_eq!(
            parse("````md\n```\ninner\n```\n````"),
            vec![MdBlock::Code {
                lang: "md".into(),
                lines: vec!["```".into(), "inner".into(), "```".into()]
            }]
        );
    }

    #[test]
    fn tables_keep_alignment_and_inline_markdown() {
        let blocks = parse(
            "| Metric | Q2 | Q3 |\n|:--|--:|:-:|\n| **Rev** | 100 | `x\\|y` |\n| Costs | 80 |",
        );
        let MdBlock::Table(table) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(table.align, [Align::Left, Align::Right, Align::Center]);
        assert_eq!(table.header.len(), 3);
        assert_eq!(table.rows[0][0], vec![span("Rev", Style::Bold)]);
        assert_eq!(table.rows[0][2], vec![span("x|y", Style::Code)]);
        assert_eq!(table.rows[1].len(), 2);
        assert_eq!(table.columns(), 3);
        // Without leading pipes, but with a delimiter row.
        let blocks = parse("a | b\n--- | ---\n1 | 2\n\nnext");
        assert!(matches!(&blocks[0], MdBlock::Table(t) if t.rows.len() == 1));
        assert_eq!(blocks[1], MdBlock::Paragraph(plain("next")));
        // A pipe in a paragraph is just text.
        assert_eq!(parse("a | b"), vec![MdBlock::Paragraph(plain("a | b"))]);
    }

    #[test]
    fn the_iphones_rich_transcript() {
        let text = "## Summary\n\nThe report says revenue **grew 12%** and costs *fell*; see [the dashboard](https://example.com/q3) and [notes](./docs/q3-notes.md).\n\n- [x] Tests pass (104)\n- [ ] Gmail needs re-auth\n  - nested detail\n\n> Costs are seasonal.\n\nInline math $E = mc^2$ and money: $5 and $10.\n\n```ts\nexport function grow(rate: number): number {\n  // compound\n}\n```\n\n| Metric | Q2 | Q3 |\n|:--|--:|--:|\n| Revenue | 100 | 112 |\n| Costs | 80 | 76 |\n\n---\n\nDone.";
        let kinds: Vec<&str> = parse(text)
            .iter()
            .map(|b| match b {
                MdBlock::Paragraph(_) => "p",
                MdBlock::Heading(..) => "h",
                MdBlock::Item(..) => "li",
                MdBlock::Quote(_) => "quote",
                MdBlock::Code { .. } => "code",
                MdBlock::Table(_) => "table",
                MdBlock::Rule => "hr",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "h", "p", "li", "li", "li", "quote", "p", "code", "table", "hr", "p"
            ]
        );
    }

    /// Streaming (no raw-syntax flash): at every prefix of a reply, every
    /// block but the one still growing is already what it will be.
    #[test]
    fn a_streaming_prefix_only_changes_its_last_block() {
        let full = "## Summary\n\nRevenue **grew 12%**; see [it](https://x.y).\n\n- [x] Tests pass\n- [ ] Gmail\n  - nested\n\n> Costs are seasonal.\n\n```ts\nexport function grow() {\n  return 1;\n}\n```\n\n| Metric | Q2 |\n|:--|--:|\n| Revenue | 100 |\n\n~~~\nt\n~~~\n\n    indented\n\n---\n\nDone.";
        let done = parse(full);
        let cuts: Vec<usize> = full
            .char_indices()
            .map(|(at, _)| at)
            .chain([full.len()])
            .collect();
        for cut in cuts {
            let blocks = parse(&full[..cut]);
            let settled = blocks.len().saturating_sub(1);
            assert_eq!(
                blocks[..settled],
                done[..settled],
                "at {cut}: {:?}",
                &full[..cut]
            );
        }
    }

    fn span(text: &str, style: Style) -> Span {
        Span::new(text, style)
    }

    fn link(text: &str, target: &str) -> Span {
        Span {
            text: text.into(),
            style: Style::Link,
            link: Some(Rc::from(target)),
        }
    }

    #[test]
    fn links_bare_addresses_and_file_paths() {
        assert_eq!(
            inline("see [the dashboard](https://example.com/q3) and [notes](./docs/q3-notes.md)."),
            vec![
                span("see ", Style::Plain),
                link("the dashboard", "https://example.com/q3"),
                span(" and ", Style::Plain),
                link("notes", "./docs/q3-notes.md"),
                span(".", Style::Plain),
            ]
        );
        // Balanced parentheses in the address; a title is left out.
        assert_eq!(
            inline("[w](https://en.wikipedia.org/wiki/A_(b) \"T\")"),
            vec![link("w", "https://en.wikipedia.org/wiki/A_(b)")]
        );
        // Bare addresses, without trailing punctuation.
        assert_eq!(
            inline("Go to https://x.org/a, or www.rust-lang.org."),
            vec![
                span("Go to ", Style::Plain),
                link("https://x.org/a", "https://x.org/a"),
                span(", or ", Style::Plain),
                link("www.rust-lang.org", "www.rust-lang.org"),
                span(".", Style::Plain),
            ]
        );
        assert_eq!(
            inline("<https://a.b/c> (see https://a.b/d)"),
            vec![
                link("https://a.b/c", "https://a.b/c"),
                span(" (see ", Style::Plain),
                link("https://a.b/d", "https://a.b/d"),
                span(")", Style::Plain),
            ]
        );
        // An image is a link to it; a bad scheme is no link.
        assert_eq!(
            inline("![](https://a.b/i.png)"),
            vec![link("image", "https://a.b/i.png")]
        );
        assert_eq!(
            inline("[x](javascript:alert(1))"),
            vec![span("x", Style::Plain)]
        );
        assert_eq!(inline("`[a](b)`"), vec![span("[a](b)", Style::Code)]);
        // What each target is.
        assert_eq!(
            link_kind("www.a.org"),
            Some(LinkKind::Web("https://www.a.org".into()))
        );
        assert_eq!(
            link_kind("mailto:a@b.c"),
            Some(LinkKind::Other("mailto:a@b.c".into()))
        );
        assert_eq!(
            link_kind("~/x y.md"),
            Some(LinkKind::File("~/x y.md".into()))
        );
        assert_eq!(link_kind("/a/b%20c"), Some(LinkKind::File("/a/b c".into())));
        assert_eq!(
            link_kind("file:///etc/hosts"),
            Some(LinkKind::File("/etc/hosts".into()))
        );
        assert_eq!(link_kind("file://server/x"), None);
        assert_eq!(link_kind("ftp://x"), None);
        assert_eq!(link_kind("#anchor"), None);
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
                link("e", "http://x"),
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
        assert_eq!(inline("~~gone~~"), vec![span("gone", Style::Strike)]);
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
        assert_eq!(
            blocks[7],
            MdBlock::Code {
                lang: "rust".into(),
                lines: vec!["fn main() {}".into()]
            }
        );
        let MdBlock::Table(table) = &blocks[8] else {
            panic!("{:?}", blocks[8]);
        };
        assert_eq!(table.header, vec![plain("a"), plain("bb")]);
        assert_eq!(table.rows, vec![vec![plain("ccc"), plain("d")]]);
        assert_eq!(table.text(), "a   │ bb\n────┼───\nccc │ d");
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
                MdBlock::Code {
                    lang: String::new(),
                    lines: vec!["let x = 1;".into(), "let y".into()]
                }
            ]
        );
        // A table shows from its first row.
        assert!(matches!(&parse("| Metric | Q2 |")[0], MdBlock::Table(t) if t.rows.is_empty()));
        assert_eq!(parse(""), vec![]);
        assert_eq!(parse("#"), vec![MdBlock::Heading(1, vec![])]);
        assert_eq!(strip_inline("**a** `b`"), "a b");
    }
}
