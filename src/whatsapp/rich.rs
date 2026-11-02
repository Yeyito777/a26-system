//! WhatsApp's text: its markers (`*bold*`, `_italic_`, `~strike~`,
//! ```` ```mono``` ````, `` `code` ``) and links, laid out in lines that fit a
//! width, once per message (the chat keeps the layout), and drawn.

#![allow(dead_code)]

use crate::gfx::{Canvas, Rgb};
use crate::text::{self, Weight};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Bold,
    /// Inter has no italic: drawn plain, the markers gone (as WhatsApp does).
    Italic,
    Strike,
    Mono,
    Link,
}

impl Style {
    pub fn weight(self, base: Weight) -> Weight {
        match self {
            Style::Bold => Weight::SemiBold,
            Style::Mono => Weight::Mono,
            _ => base,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: Style,
    pub x: f32,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub runs: Vec<Run>,
    pub width: f32,
}

/// Text laid out at a size within a width.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Laid {
    pub lines: Vec<Line>,
    pub width: f32,
    pub size: f32,
    pub line_height: f32,
}

impl Laid {
    pub fn height(&self) -> f32 {
        self.lines.len() as f32 * self.line_height
    }

    pub fn last_width(&self) -> f32 {
        self.lines.last().map_or(0.0, |line| line.width)
    }

    /// Every URL in it, with where it is (line index, x range), for taps.
    pub fn links(&self) -> Vec<(usize, f32, f32, String)> {
        let mut out = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            for run in &line.runs {
                if run.style == Style::Link {
                    out.push((index, run.x, run.x + run.width, run.text.clone()));
                }
            }
        }
        out
    }
}

fn is_boundary(c: Option<char>) -> bool {
    c.is_none_or(|c| c.is_whitespace() || ",.;:!?()[]{}\"'-".contains(c))
}

/// Splits WhatsApp's markers into styled spans (the markers dropped).
pub fn spans(text: &str) -> Vec<(String, Style)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    let flush = |plain: &mut String, out: &mut Vec<(String, Style)>| {
        if !plain.is_empty() {
            out.extend(links(plain));
            plain.clear();
        }
    };
    while i < chars.len() {
        // ```mono```
        if chars[i] == '`'
            && chars.get(i + 1) == Some(&'`')
            && chars.get(i + 2) == Some(&'`')
            && let Some(end) = (i + 3..chars.len().saturating_sub(2))
                .find(|&j| chars[j] == '`' && chars[j + 1] == '`' && chars[j + 2] == '`')
            && end > i + 3
        {
            flush(&mut plain, &mut out);
            out.push((chars[i + 3..end].iter().collect(), Style::Mono));
            i = end + 3;
            continue;
        }
        let marker = match chars[i] {
            '*' => Some(Style::Bold),
            '_' => Some(Style::Italic),
            '~' => Some(Style::Strike),
            '`' => Some(Style::Mono),
            _ => None,
        };
        if let Some(style) = marker {
            let before = if i == 0 { None } else { Some(chars[i - 1]) };
            let next = chars.get(i + 1).copied();
            if is_boundary(before) && next.is_some_and(|c| !c.is_whitespace() && c != chars[i]) {
                let close = (i + 2..chars.len()).find(|&j| {
                    chars[j] == chars[i]
                        && !chars[j - 1].is_whitespace()
                        && is_boundary(chars.get(j + 1).copied())
                        && chars[i + 1..j].iter().all(|&c| c != '\n')
                });
                if let Some(close) = close {
                    flush(&mut plain, &mut out);
                    let inner: String = chars[i + 1..close].iter().collect();
                    out.push((inner, style));
                    i = close + 1;
                    continue;
                }
            }
        }
        plain.push(chars[i]);
        i += 1;
    }
    flush(&mut plain, &mut out);
    out
}

/// Plain text with its URLs as link spans.
fn links(text: &str) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = find_url(rest) {
        let end = rest[start..]
            .find(|c: char| c.is_whitespace())
            .map_or(rest.len(), |n| start + n);
        // Trailing punctuation is the sentence's.
        let mut end = end;
        while end > start && rest[..end].ends_with(['.', ',', ')', '!', '?', ';', ':', '"', '\'']) {
            end -= 1;
        }
        if start > 0 {
            out.push((rest[..start].to_owned(), Style::Plain));
        }
        out.push((rest[start..end].to_owned(), Style::Link));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        out.push((rest.to_owned(), Style::Plain));
    }
    out
}

fn find_url(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    ["https://", "http://", "www."]
        .iter()
        .filter_map(|prefix| lower.find(prefix))
        .filter(|&at| at == 0 || !text[..at].ends_with(|c: char| c.is_alphanumeric()))
        .min()
}

/// The URL a link run opens.
pub fn url_of(link: &str) -> String {
    if link.to_ascii_lowercase().starts_with("www.") {
        format!("https://{link}")
    } else {
        link.to_owned()
    }
}

/// Emoji as WhatsApp shows them, in colour: a pictograph standing alone
/// gets the emoji selector (the shared text drawing gives a lone one the
/// symbol fonts' outline when they have it: 👍 drew as a line drawing).
pub fn emojify(text_in: &str) -> std::borrow::Cow<'_, str> {
    let needs = |c: char, next: Option<char>| {
        let code = c as u32;
        ((0x1f300..=0x1faff).contains(&code) && !(0x1f3fb..=0x1f3ff).contains(&code)
            || emoji_presentation(code))
            && !next
                .is_some_and(|n| matches!(n as u32, 0xfe0e | 0xfe0f | 0x200d | 0x1f3fb..=0x1f3ff))
    };
    let mut chars = text_in.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        if needs(c, chars.peek().copied()) {
            any = true;
            break;
        }
    }
    if !any {
        return std::borrow::Cow::Borrowed(text_in);
    }
    let mut out = String::with_capacity(text_in.len() + 8);
    let mut chars = text_in.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if needs(c, chars.peek().copied()) {
            out.push('\u{fe0f}');
        }
    }
    std::borrow::Cow::Owned(out)
}

/// The symbols before U+1F300 that show as emoji by default (Unicode's
/// Emoji_Presentation), e.g. ☔ ☕ ⚡ ✨ ❌.
fn emoji_presentation(code: u32) -> bool {
    matches!(
        code,
        0x231a..=0x231b
            | 0x23e9..=0x23ec
            | 0x23f0
            | 0x23f3
            | 0x25fd..=0x25fe
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267f
            | 0x2693
            | 0x26a1
            | 0x26aa..=0x26ab
            | 0x26bd..=0x26be
            | 0x26c4..=0x26c5
            | 0x26ce
            | 0x26d4
            | 0x26ea
            | 0x26f2..=0x26f3
            | 0x26f5
            | 0x26fa
            | 0x26fd
            | 0x2705
            | 0x270a..=0x270b
            | 0x2728
            | 0x274c
            | 0x274e
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27b0
            | 0x27bf
            | 0x2b1b..=0x2b1c
            | 0x2b50
            | 0x2b55
    )
}

/// Lays `text` out in `width` at `size` (`base` weight for plain text).
pub fn layout(text_in: &str, width: f32, size: f32, line_height: f32, base: Weight) -> Laid {
    let emojified = emojify(text_in);
    let text_in: &str = &emojified;
    let mut lines: Vec<Line> = Vec::new();
    let mut line = Line::default();
    let push_line = |line: &mut Line, lines: &mut Vec<Line>| {
        lines.push(std::mem::take(line));
    };
    for (span, style) in spans(text_in) {
        let weight = style.weight(base);
        for (n, piece) in span.split('\n').enumerate() {
            if n > 0 {
                push_line(&mut line, &mut lines);
            }
            // Words with the spaces after them.
            let mut words: Vec<&str> = Vec::new();
            let mut start = 0;
            let bytes = piece.char_indices().collect::<Vec<_>>();
            for (index, &(at, c)) in bytes.iter().enumerate() {
                let next_is_word = bytes
                    .get(index + 1)
                    .is_some_and(|&(_, next)| !next.is_whitespace());
                if c.is_whitespace() && next_is_word {
                    let end = at + c.len_utf8();
                    words.push(&piece[start..end]);
                    start = end;
                }
            }
            if start < piece.len() {
                words.push(&piece[start..]);
            }
            for word in words {
                let trimmed = text::measure(word.trim_end(), weight, size);
                if line.width + trimmed > width && line.width > 0.0 {
                    push_line(&mut line, &mut lines);
                }
                if trimmed > width {
                    // A word wider than the line (a long link): by characters.
                    let mut chunk = String::new();
                    for c in word.chars() {
                        let candidate = format!("{chunk}{c}");
                        if line.width + text::measure(&candidate, weight, size) > width
                            && !chunk.is_empty()
                        {
                            add(&mut line, &chunk, style, weight, size);
                            push_line(&mut line, &mut lines);
                            chunk.clear();
                        }
                        chunk.push(c);
                    }
                    add(&mut line, &chunk, style, weight, size);
                    continue;
                }
                add(&mut line, word, style, weight, size);
            }
        }
    }
    push_line(&mut line, &mut lines);
    // Trailing spaces do not count toward a line's width.
    for line in &mut lines {
        if let Some(last) = line.runs.last_mut() {
            let trimmed = last.text.trim_end().to_owned();
            if trimmed.len() != last.text.len() {
                last.width = text::measure(&trimmed, last.style.weight(base), size);
                last.text = trimmed;
            }
            line.width = last.x + last.width;
        }
    }
    while lines.len() > 1 && lines.last().is_some_and(|line| line.runs.is_empty()) {
        lines.pop();
    }
    let max = lines.iter().map(|line| line.width).fold(0.0, f32::max);
    Laid {
        lines,
        width: max,
        size,
        line_height,
    }
}

fn add(line: &mut Line, text_in: &str, style: Style, weight: Weight, size: f32) {
    let width = text::measure(text_in, weight, size);
    if let Some(last) = line.runs.last_mut()
        && last.style == style
    {
        // Words are measured alone: Inter's pairs do not kern across a space.
        last.text.push_str(text_in);
        last.width += width;
        line.width = last.x + last.width;
        return;
    }
    line.runs.push(Run {
        text: text_in.to_owned(),
        style,
        x: line.width,
        width,
    });
    line.width += width;
}

/// Draws laid-out text with its first baseline at `baseline`.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &mut Canvas,
    laid: &Laid,
    x: f32,
    baseline: f32,
    base: Weight,
    color: Rgb,
    link: Rgb,
    clip: (f32, f32),
) {
    for (index, line) in laid.lines.iter().enumerate() {
        let y = baseline + index as f32 * laid.line_height;
        if y + laid.line_height < clip.0 || y - laid.line_height > clip.1 {
            continue;
        }
        for run in &line.runs {
            let weight = run.style.weight(base);
            let color = if run.style == Style::Link {
                link
            } else {
                color
            };
            text::draw(
                canvas,
                &run.text,
                x + run.x,
                y,
                weight,
                laid.size,
                color,
                1.0,
            );
            if run.style == Style::Strike {
                canvas.fill_rect(
                    (x + run.x) as i32,
                    (y - laid.size * 0.3) as i32,
                    run.width.ceil() as i32,
                    2,
                    color,
                    1.0,
                );
            }
            if run.style == Style::Link {
                canvas.fill_rect(
                    (x + run.x) as i32,
                    (y + laid.size * 0.12) as i32,
                    run.width.ceil() as i32,
                    2,
                    color,
                    0.6,
                );
            }
        }
    }
}

/// Whether the text is one to three emoji and nothing else (drawn big).
pub fn emoji_only(text_in: &str) -> bool {
    let trimmed = text_in.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 24 {
        return false;
    }
    let mut count = 0;
    let mut previous_joiner = false;
    for c in trimmed.chars() {
        let code = c as u32;
        if c.is_whitespace() {
            continue;
        }
        let modifier =
            matches!(code, 0xfe0f | 0x200d | 0x1f3fb..=0x1f3ff | 0x20e3 | 0xe0020..=0xe007f);
        let pictograph = matches!(code, 0x2600..=0x27bf | 0x1f000..=0x1faff | 0x2190..=0x21ff | 0x2b00..=0x2bff | 0x00a9 | 0x00ae | 0x203c | 0x2049 | 0x2122 | 0x2139 | 0x3030 | 0x303d | 0x3297 | 0x3299);
        if !(modifier || pictograph) {
            return false;
        }
        if pictograph && !previous_joiner {
            count += 1;
        }
        previous_joiner = code == 0x200d;
    }
    (1..=3).contains(&count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_become_styles() {
        assert_eq!(
            spans("that's *exactly* what I said"),
            vec![
                ("that's ".to_owned(), Style::Plain),
                ("exactly".to_owned(), Style::Bold),
                (" what I said".to_owned(), Style::Plain),
            ]
        );
        assert_eq!(
            spans("~cancelled~ moved")[0],
            ("cancelled".to_owned(), Style::Strike)
        );
        assert_eq!(spans("2*3*4 stays")[0].1, Style::Plain);
        assert_eq!(
            spans("```npm run build``` fails")[0],
            ("npm run build".to_owned(), Style::Mono)
        );
        let linked = spans("see https://example.com/a, ok");
        assert_eq!(linked[1], ("https://example.com/a".to_owned(), Style::Link));
        assert_eq!(linked[2].0, ", ok");
    }

    #[test]
    fn emoji_only_counts_sequences() {
        assert!(emoji_only("❤️❤️"));
        assert!(emoji_only("👍🏽"));
        assert!(emoji_only("👨‍👩‍👧"));
        assert!(!emoji_only("ok 👍"));
        assert!(!emoji_only("🔥🔥🔥🔥"));
    }
}
