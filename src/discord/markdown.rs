//! Discord's markdown, as its clients parse it: code blocks, quotes (`>`
//! and `>>>`), headings (`#`, `##`, `###`), subtext (`-#`), lists, and
//! inline bold, italic, underline, strikethrough, spoilers, inline code,
//! links (bare, `<…>` and `[masked](…)`), mentions of users, roles and
//! channels, `@everyone`/`@here`, custom emoji, timestamps and slash
//! commands. Parsed into blocks of spans; `layout.rs` lays them out.

#![allow(dead_code)]

use std::rc::Rc;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style(pub u8);

impl Style {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 2;
    pub const UNDERLINE: u8 = 4;
    pub const STRIKE: u8 = 8;

    pub fn has(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn with(self, flag: u8) -> Self {
        Self(self.0 | flag)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Text,
    Code,
    Link(Rc<str>),
    User(Rc<str>),
    Role(Rc<str>),
    Channel(Rc<str>),
    /// `@everyone` or `@here`.
    Everyone,
    Emoji {
        id: Rc<str>,
        animated: bool,
    },
    /// `<t:UNIX:STYLE>`.
    Time(i64, char),
    /// `</name:id>`.
    Command,
    /// A line break inside a paragraph.
    Break,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub kind: Kind,
    /// Which spoiler it is in (1-based), 0 for none.
    pub spoiler: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Span>),
    Heading(u8, Vec<Span>),
    Subtext(Vec<Span>),
    Quote(Vec<Block>),
    Code {
        lang: Option<String>,
        text: String,
    },
    /// One item; consecutive items make a list.
    Item {
        depth: u8,
        /// `Some(n)` for `n.`, None for a bullet.
        number: Option<u32>,
        spans: Vec<Span>,
    },
}

/// Parses a whole message.
pub fn parse(text: &str) -> Vec<Block> {
    let mut spoilers = 0;
    parse_blocks(text, true, &mut spoilers)
}

fn parse_blocks(text: &str, quotes: bool, spoilers: &mut u16) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut quote: Vec<&str> = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut index = 0;
    let flush_para = |para: &mut Vec<&str>, blocks: &mut Vec<Block>, spoilers: &mut u16| {
        if para.is_empty() {
            return;
        }
        let joined = para.join("\n");
        para.clear();
        let spans = parse_inline(&joined, spoilers);
        if !spans.is_empty() {
            blocks.push(Block::Para(spans));
        }
    };
    let flush_quote = |quote: &mut Vec<&str>, blocks: &mut Vec<Block>, spoilers: &mut u16| {
        if quote.is_empty() {
            return;
        }
        let joined = quote.join("\n");
        quote.clear();
        blocks.push(Block::Quote(parse_blocks(&joined, false, spoilers)));
    };
    while index < lines.len() {
        let line = lines[index];
        // A fenced code block.
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            let mut body = String::new();
            let mut lang = None;
            let mut closed = false;
            if let Some(end) = rest.find("```") {
                // ```one line```
                body.push_str(&rest[..end]);
                let after = &rest[end + 3..];
                flush_para(&mut para, &mut blocks, spoilers);
                flush_quote(&mut quote, &mut blocks, spoilers);
                blocks.push(Block::Code {
                    lang: None,
                    text: body.trim_matches('\n').to_owned(),
                });
                if !after.trim().is_empty() {
                    para.push(after);
                }
                index += 1;
                continue;
            }
            let first = rest.trim();
            if !first.is_empty()
                && first
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-_#.".contains(c))
            {
                lang = Some(first.to_owned());
            } else if !rest.is_empty() {
                body.push_str(rest);
                body.push('\n');
            }
            let mut cursor = index + 1;
            let mut trailing = None;
            while cursor < lines.len() {
                if let Some(end) = lines[cursor].find("```") {
                    body.push_str(&lines[cursor][..end]);
                    trailing = Some(&lines[cursor][end + 3..]);
                    closed = true;
                    break;
                }
                body.push_str(lines[cursor]);
                body.push('\n');
                cursor += 1;
            }
            if closed {
                flush_para(&mut para, &mut blocks, spoilers);
                flush_quote(&mut quote, &mut blocks, spoilers);
                blocks.push(Block::Code {
                    lang,
                    text: body.trim_end_matches('\n').to_owned(),
                });
                if let Some(after) = trailing
                    && !after.trim().is_empty()
                {
                    para.push(after);
                }
                index = cursor + 1;
                continue;
            }
            // Unclosed: plain text.
        }
        if quotes && let Some(rest) = line.strip_prefix(">>> ") {
            flush_para(&mut para, &mut blocks, spoilers);
            flush_quote(&mut quote, &mut blocks, spoilers);
            let mut all = vec![rest];
            all.extend_from_slice(&lines[index + 1..]);
            blocks.push(Block::Quote(parse_blocks(&all.join("\n"), false, spoilers)));
            return blocks;
        }
        if quotes && (line.starts_with("> ") || line == ">") {
            flush_para(&mut para, &mut blocks, spoilers);
            quote.push(line.get(2..).unwrap_or(""));
            index += 1;
            continue;
        }
        flush_quote(&mut quote, &mut blocks, spoilers);
        let heading = [("### ", 3), ("## ", 2), ("# ", 1)]
            .into_iter()
            .find_map(|(prefix, level)| line.strip_prefix(prefix).map(|rest| (rest, level)));
        if let Some((rest, level)) = heading
            && !rest.trim().is_empty()
        {
            flush_para(&mut para, &mut blocks, spoilers);
            blocks.push(Block::Heading(level, parse_inline(rest.trim(), spoilers)));
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("-# ")
            && !rest.trim().is_empty()
        {
            flush_para(&mut para, &mut blocks, spoilers);
            blocks.push(Block::Subtext(parse_inline(rest.trim(), spoilers)));
            index += 1;
            continue;
        }
        if let Some((depth, number, rest)) = list_item(line) {
            flush_para(&mut para, &mut blocks, spoilers);
            blocks.push(Block::Item {
                depth,
                number,
                spans: parse_inline(rest, spoilers),
            });
            index += 1;
            continue;
        }
        para.push(line);
        index += 1;
    }
    flush_para(&mut para, &mut blocks, spoilers);
    flush_quote(&mut quote, &mut blocks, spoilers);
    blocks
}

fn list_item(line: &str) -> Option<(u8, Option<u32>, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let depth = (indent / 2).min(3) as u8;
    if let Some(item) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* "))
        && !item.trim().is_empty()
    {
        return Some((depth, None, item));
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits)
        && rest[digits..].starts_with(". ")
        && !rest[digits + 2..].trim().is_empty()
    {
        return Some((depth, rest[..digits].parse().ok(), &rest[digits + 2..]));
    }
    None
}

struct Inline<'a> {
    text: &'a str,
    spans: Vec<Span>,
    spoilers: &'a mut u16,
}

/// Parses inline markdown.
pub fn parse_inline(text: &str, spoilers: &mut u16) -> Vec<Span> {
    let mut inline = Inline {
        text,
        spans: Vec::new(),
        spoilers,
    };
    inline.run(0, text.len(), Style::default(), 0, false);
    merge(inline.spans)
}

/// Adjacent plain spans of one style become one.
fn merge(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(last) = out.last_mut()
            && last.kind == Kind::Text
            && span.kind == Kind::Text
            && last.style == span.style
            && last.spoiler == span.spoiler
        {
            last.text.push_str(&span.text);
            continue;
        }
        if span.text.is_empty() && span.kind == Kind::Text {
            continue;
        }
        out.push(span);
    }
    out
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
}

impl Inline<'_> {
    fn push(&mut self, text: &str, style: Style, kind: Kind, spoiler: u16) {
        self.spans.push(Span {
            text: text.to_owned(),
            style,
            kind,
            spoiler,
        });
    }

    /// Parses `start..end` with `style`; `in_link` stops nested links.
    fn run(&mut self, start: usize, end: usize, style: Style, spoiler: u16, in_link: bool) {
        let text = self.text;
        let mut pos = start;
        let mut plain = start;
        while pos < end {
            let rest = &text[pos..end];
            let c = rest.chars().next().unwrap_or(' ');
            let consumed = self.rule(pos, end, style, spoiler, in_link);
            match consumed {
                Some(next) => {
                    if plain < pos {
                        // Text before the rule was pushed by `rule`'s caller order: flush here.
                    }
                    pos = next;
                    plain = pos;
                }
                None => {
                    // Plain text up to the next character that may start a rule.
                    let mut next = pos + c.len_utf8();
                    while next < end {
                        let d = text[next..].chars().next().unwrap_or(' ');
                        if "\\<[*_~|`@\n".contains(d)
                            || (d == 'h' && text[next..].starts_with("http"))
                        {
                            break;
                        }
                        next += d.len_utf8();
                    }
                    self.push(&text[pos..next], style, Kind::Text, spoiler);
                    pos = next;
                    plain = pos;
                }
            }
        }
        let _ = plain;
    }

    /// Tries every rule at `pos`; returns where it ended.
    fn rule(
        &mut self,
        pos: usize,
        end: usize,
        style: Style,
        spoiler: u16,
        in_link: bool,
    ) -> Option<usize> {
        let text = self.text;
        let rest = &text[pos..end];
        let first = rest.as_bytes()[0];
        match first {
            b'\n' => {
                self.push("\n", style, Kind::Break, spoiler);
                Some(pos + 1)
            }
            b'\\' => {
                let next = rest[1..].chars().next()?;
                if is_punct(next) {
                    self.push(&rest[1..1 + next.len_utf8()], style, Kind::Text, spoiler);
                    Some(pos + 1 + next.len_utf8())
                } else {
                    None
                }
            }
            b'<' => self.angle(pos, end, style, spoiler, in_link),
            b'[' if !in_link => {
                // [text](https://…)
                let close = find_matching(rest, b'[', b']')?;
                let after = &rest[close + 1..];
                if !after.starts_with('(') {
                    return None;
                }
                let paren = after.find(')')?;
                let url = after[1..paren]
                    .trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>');
                if !(url.starts_with("https://") || url.starts_with("http://")) || close <= 1 {
                    return None;
                }
                let url: Rc<str> = url.into();
                let inner_start = pos + 1;
                let inner_end = pos + close;
                let before = self.spans.len();
                self.run(inner_start, inner_end, style, spoiler, true);
                for span in &mut self.spans[before..] {
                    if span.kind == Kind::Text {
                        span.kind = Kind::Link(url.clone());
                    }
                }
                Some(pos + close + 1 + paren + 1)
            }
            b'h' if !in_link => {
                if !(rest.starts_with("https://") || rest.starts_with("http://")) {
                    return None;
                }
                let mut stop = rest
                    .find(|c: char| c.is_whitespace() || c == '<')
                    .unwrap_or(rest.len());
                // Trailing punctuation is not part of a bare link.
                while stop > 0 && ".,:;\"')]!?".contains(rest[..stop].chars().last()?) {
                    let last = rest[..stop].chars().last()?;
                    if last == ')' && rest[..stop].contains('(') {
                        break;
                    }
                    stop -= last.len_utf8();
                }
                if stop <= 8 {
                    return None;
                }
                let url: Rc<str> = rest[..stop].into();
                self.push(&rest[..stop], style, Kind::Link(url), spoiler);
                Some(pos + stop)
            }
            b'@' => {
                for word in ["@everyone", "@here"] {
                    if rest.starts_with(word) {
                        self.push(word, style, Kind::Everyone, spoiler);
                        return Some(pos + word.len());
                    }
                }
                None
            }
            b'`' => {
                let ticks = rest.bytes().take_while(|&b| b == b'`').count();
                let fence = &rest[..ticks];
                let close = rest[ticks..].find(fence)?;
                let body = &rest[ticks..ticks + close];
                if body.trim().is_empty() {
                    return None;
                }
                let body = if ticks > 1 { body.trim() } else { body };
                self.push(body, style, Kind::Code, spoiler);
                Some(pos + ticks + close + ticks)
            }
            b'|' if rest.starts_with("||") => {
                let close = rest[2..].find("||")?;
                if close == 0 {
                    return None;
                }
                *self.spoilers += 1;
                let id = *self.spoilers;
                self.run(pos + 2, pos + 2 + close, style, id, in_link);
                Some(pos + 2 + close + 2)
            }
            b'*' | b'_' | b'~' => self.emphasis(pos, end, style, spoiler, in_link),
            _ => None,
        }
    }

    fn emphasis(
        &mut self,
        pos: usize,
        end: usize,
        style: Style,
        spoiler: u16,
        in_link: bool,
    ) -> Option<usize> {
        let text = self.text;
        let rest = &text[pos..end];
        let tries: [(&str, u8); 6] = [
            ("***", Style::BOLD | Style::ITALIC),
            ("**", Style::BOLD),
            ("__", Style::UNDERLINE),
            ("~~", Style::STRIKE),
            ("*", Style::ITALIC),
            ("_", Style::ITALIC),
        ];
        for (marker, flags) in tries {
            if !rest.starts_with(marker) {
                continue;
            }
            let body_start = marker.len();
            let inner = &rest[body_start..];
            if inner.is_empty() || inner.starts_with(char::is_whitespace) {
                continue;
            }
            // The closing marker: not after whitespace, and (as Discord's parser
            // has it) not followed by the marker's character; a single `*`
            // passes over `**` pairs in the body.
            let mc = marker.as_bytes()[0];
            let mut search = 0;
            let close = loop {
                let Some(found) = inner[search..].find(marker).map(|at| at + search) else {
                    break None;
                };
                let next = inner.as_bytes().get(found + marker.len()).copied();
                let after_space = found == 0 || inner[..found].ends_with(char::is_whitespace);
                if marker.len() == 1 && mc == b'*' && next == Some(b'*') {
                    search = found + 2;
                } else if next != Some(mc) && !after_space {
                    break Some(found);
                } else {
                    search = found + 1;
                }
                if search >= inner.len() {
                    break None;
                }
            };
            let Some(close) = close else {
                continue;
            };
            // `_italic_` only at word boundaries.
            if marker == "_" {
                let after = inner[close + 1..].chars().next();
                if after.is_some_and(char::is_alphanumeric) {
                    continue;
                }
                let before = text[..pos].chars().last();
                if before.is_some_and(char::is_alphanumeric) {
                    continue;
                }
            }
            let inner_start = pos + body_start;
            self.run(
                inner_start,
                inner_start + close,
                style.with(flags),
                spoiler,
                in_link,
            );
            return Some(inner_start + close + marker.len());
        }
        None
    }

    fn angle(
        &mut self,
        pos: usize,
        end: usize,
        style: Style,
        spoiler: u16,
        in_link: bool,
    ) -> Option<usize> {
        let text = self.text;
        let rest = &text[pos..end];
        let close = rest.find('>')?;
        let inner = &rest[1..close];
        let next = pos + close + 1;
        let digits = |value: &str| {
            !value.is_empty() && value.len() <= 22 && value.bytes().all(|b| b.is_ascii_digit())
        };
        if !in_link
            && (inner.starts_with("https://") || inner.starts_with("http://"))
            && !inner.contains(' ')
        {
            self.push(inner, style, Kind::Link(inner.into()), spoiler);
            return Some(next);
        }
        if let Some(id) = inner.strip_prefix("@&")
            && digits(id)
        {
            self.push(inner, style, Kind::Role(id.into()), spoiler);
            return Some(next);
        }
        if let Some(id) = inner.strip_prefix("@!").or_else(|| inner.strip_prefix('@'))
            && digits(id)
        {
            self.push(inner, style, Kind::User(id.into()), spoiler);
            return Some(next);
        }
        if let Some(id) = inner.strip_prefix('#')
            && digits(id)
        {
            self.push(inner, style, Kind::Channel(id.into()), spoiler);
            return Some(next);
        }
        let animated = inner.starts_with("a:");
        let emoji = if animated {
            &inner[2..]
        } else {
            inner.strip_prefix(':').unwrap_or("")
        };
        if let Some((name, id)) = emoji.split_once(':')
            && !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'~')
            && digits(id)
        {
            self.push(
                &format!(":{name}:"),
                style,
                Kind::Emoji {
                    id: id.into(),
                    animated,
                },
                spoiler,
            );
            return Some(next);
        }
        if let Some(body) = inner.strip_prefix("t:") {
            let (number, format) = body.split_once(':').unwrap_or((body, "f"));
            if let Ok(seconds) = number.parse::<i64>() {
                let format = format
                    .chars()
                    .next()
                    .filter(|c| "tTdDfFR".contains(*c))
                    .unwrap_or('f');
                self.push(inner, style, Kind::Time(seconds, format), spoiler);
                return Some(next);
            }
        }
        if let Some(body) = inner.strip_prefix('/')
            && let Some((name, id)) = body.rsplit_once(':')
            && digits(id)
            && !name.is_empty()
        {
            self.push(&format!("/{name}"), style, Kind::Command, spoiler);
            return Some(next);
        }
        None
    }
}

/// The index of the bracket closing the one at 0.
fn find_matching(text: &str, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        } else if byte == b'\n' {
            return None;
        }
    }
    None
}

/// Is the message only emoji (up to 30), and no other text? Then they draw big.
pub fn only_emoji(blocks: &[Block]) -> bool {
    let [Block::Para(spans)] = blocks else {
        return false;
    };
    let mut count = 0;
    for span in spans {
        match &span.kind {
            Kind::Emoji { .. } => count += 1,
            Kind::Text => {
                for word in span.text.split_whitespace() {
                    if !word.chars().all(is_emoji_char) {
                        return false;
                    }
                    count += word
                        .chars()
                        .filter(|c| !is_joiner(*c))
                        .count()
                        .div_ceil(2)
                        .max(1);
                }
            }
            Kind::Break => {}
            _ => return false,
        }
    }
    count > 0 && count <= 30
}

fn is_joiner(c: char) -> bool {
    matches!(c as u32, 0x200d | 0xfe0f | 0x1f3fb..=0x1f3ff | 0x20e3)
}

fn is_emoji_char(c: char) -> bool {
    let n = c as u32;
    is_joiner(c)
        || (0x1f000..=0x1faff).contains(&n)
        || (0x2600..=0x27bf).contains(&n)
        || (0x2190..=0x21ff).contains(&n)
        || (0x2b00..=0x2bff).contains(&n)
        || (0x1f1e6..=0x1f1ff).contains(&n)
        || (0xe0020..=0xe007f).contains(&n)
        || n == 0x00a9
        || n == 0x00ae
        || n == 0x203c
        || n == 0x2049
        || n == 0x2122
        || n == 0x2139
        || (0x231a..=0x23ff).contains(&n)
}

/// Text without markdown, for a one-line preview (a reply, a notification).
pub fn plain(text: &str, names: &dyn Fn(&Kind) -> Option<String>) -> String {
    let mut out = String::new();
    for block in parse(text) {
        let spans = match block {
            Block::Para(spans)
            | Block::Heading(_, spans)
            | Block::Subtext(spans)
            | Block::Item { spans, .. } => spans,
            Block::Code { text, .. } => {
                out.push_str(&text);
                out.push(' ');
                continue;
            }
            Block::Quote(_) => continue,
        };
        for span in spans {
            match &span.kind {
                Kind::Break => out.push(' '),
                Kind::Text | Kind::Code | Kind::Link(_) | Kind::Everyone | Kind::Command => {
                    if span.spoiler > 0 {
                        out.push_str("(spoiler)");
                    } else {
                        out.push_str(&span.text);
                    }
                }
                other => out.push_str(&names(other).unwrap_or_else(|| span.text.clone())),
            }
        }
        out.push(' ');
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, u8, String)> {
        let blocks = parse(text);
        let Block::Para(spans) = &blocks[0] else {
            panic!("a paragraph: {blocks:?}")
        };
        spans
            .iter()
            .map(|s| {
                let kind = match &s.kind {
                    Kind::Text => "text".to_owned(),
                    Kind::Code => "code".to_owned(),
                    Kind::Link(url) => format!("link {url}"),
                    Kind::User(id) => format!("user {id}"),
                    Kind::Role(id) => format!("role {id}"),
                    Kind::Channel(id) => format!("channel {id}"),
                    Kind::Everyone => "everyone".to_owned(),
                    Kind::Emoji { id, animated } => format!("emoji {id} {animated}"),
                    Kind::Time(t, f) => format!("time {t} {f}"),
                    Kind::Command => "command".to_owned(),
                    Kind::Break => "break".to_owned(),
                };
                (s.text.clone(), s.style.0, kind)
            })
            .collect()
    }

    #[test]
    fn inline_styles_nest() {
        let spans = kinds("a **bold *both*** and __under__ ~~gone~~ `x*y*`");
        assert_eq!(spans[0], ("a ".into(), 0, "text".into()));
        assert_eq!(spans[1], ("bold ".into(), Style::BOLD, "text".into()));
        assert_eq!(
            spans[2],
            ("both".into(), Style::BOLD | Style::ITALIC, "text".into())
        );
        assert!(spans.contains(&("under".into(), Style::UNDERLINE, "text".into())));
        assert!(spans.contains(&("gone".into(), Style::STRIKE, "text".into())));
        assert_eq!(spans.last().unwrap(), &("x*y*".into(), 0, "code".into()));
    }

    #[test]
    fn tokens_links_and_spoilers() {
        let spans = kinds(
            "hi <@123> in <#456> for <@&789> <:blob:42> <t:1700000000:R> see https://x.com/a. ||secret||",
        );
        let found: Vec<&str> = spans.iter().map(|s| s.2.as_str()).collect();
        assert!(found.contains(&"user 123"));
        assert!(found.contains(&"channel 456"));
        assert!(found.contains(&"role 789"));
        assert!(found.contains(&"emoji 42 false"));
        assert!(found.contains(&"time 1700000000 R"));
        assert!(found.contains(&"link https://x.com/a"));
        let blocks = parse("||a|| b ||c||");
        let Block::Para(spans) = &blocks[0] else {
            panic!()
        };
        assert_eq!(
            spans.iter().map(|s| s.spoiler).collect::<Vec<_>>(),
            vec![1, 0, 2]
        );
        let masked = kinds("[the docs](https://docs.rs) here");
        assert_eq!(
            masked[0],
            ("the docs".into(), 0, "link https://docs.rs".into())
        );
        assert_eq!(kinds("snake_case_name")[0].0, "snake_case_name");
        assert_eq!(kinds("2 * 3 * 4")[0].0, "2 * 3 * 4");
    }

    #[test]
    fn blocks() {
        let blocks = parse(
            "# Title\nsome text\n> quoted\n> more\n```rs\nfn x() {}\n```\n- one\n  - two\n1. first\n-# small",
        );
        assert!(matches!(&blocks[0], Block::Heading(1, _)));
        assert!(matches!(&blocks[1], Block::Para(_)));
        assert!(matches!(&blocks[2], Block::Quote(inner) if inner.len() == 1));
        assert!(
            matches!(&blocks[3], Block::Code { lang: Some(l), text } if l == "rs" && text == "fn x() {}")
        );
        assert!(matches!(
            &blocks[4],
            Block::Item {
                depth: 0,
                number: None,
                ..
            }
        ));
        assert!(matches!(
            &blocks[5],
            Block::Item {
                depth: 1,
                number: None,
                ..
            }
        ));
        assert!(matches!(
            &blocks[6],
            Block::Item {
                number: Some(1),
                ..
            }
        ));
        assert!(matches!(&blocks[7], Block::Subtext(_)));
        let rest = parse("before\n>>> all\nof this");
        assert!(
            matches!(&rest[1], Block::Quote(inner) if matches!(&inner[0], Block::Para(s) if s.len() == 3))
        );
        assert!(only_emoji(&parse("😀 <:a:1>")));
        assert!(!only_emoji(&parse("ok 😀")));
    }
}
