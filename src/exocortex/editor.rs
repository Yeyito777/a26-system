//! The composer's editor (WP-D; PAR-181, PAR-196): a caret and a
//! selection in the composer's text, its editing (typing, Backspace,
//! Return as a line break, paste, cut, select a word or all), and the text
//! wrapped into lines exactly as the view draws it, so a touch maps to the
//! glyph under it and the caret is drawn where the next character goes.
//!
//! Indices are byte offsets into the text, always on a cluster boundary (an
//! emoji sequence or a letter with its combining marks is one step).
//! Wrapping follows a `<textarea>`'s `white-space: pre-wrap;
//! overflow-wrap: anywhere`: a newline breaks the line, lines break after
//! a run of spaces (which may hang past the edge), and a word longer than a
//! line breaks between characters. Pure: no drawing, no I/O.

use crate::text::{self, Weight};

/// Moon's paste key and the clipboard never bring more than this; a
/// draft is capped there too (PAR-188's 32 MB).
pub const MAX_TEXT: usize = 32 << 20;

/// A tab becomes this many spaces (Moon's paste key does the same).
const TAB: &str = "    ";

/// Characters that join the cluster before them: combining marks,
/// variation selectors, skin tones, emoji tags, a zero-width non-joiner.
fn extends(character: char) -> bool {
    matches!(
        u32::from(character),
        0x300..=0x36f
            | 0x483..=0x489
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x200c
            | 0x20d0..=0x20ff
            | 0xfe00..=0xfe0f
            | 0xfe20..=0xfe2f
            | 0x1f3fb..=0x1f3ff
            | 0xe0020..=0xe007f
    )
}

fn regional(character: char) -> bool {
    (0x1f1e6..=0x1f1ff).contains(&u32::from(character))
}

/// Where the cluster starting at `start` ends (`start` itself at the end).
pub fn cluster_end(text: &str, start: usize) -> usize {
    let mut characters = text[start..].char_indices().peekable();
    let Some((_, first)) = characters.next() else {
        return start;
    };
    let mut end = start + first.len_utf8();
    if first == '\n' {
        return end;
    }
    if regional(first) {
        if let Some(&(at, next)) = characters.peek()
            && regional(next)
        {
            return start + at + next.len_utf8();
        }
        return end;
    }
    while let Some(&(at, next)) = characters.peek() {
        if extends(next) {
            characters.next();
            end = start + at + next.len_utf8();
        } else if next == '\u{200d}' {
            characters.next();
            end = start + at + next.len_utf8();
            if let Some((at, joined)) = characters.next() {
                end = start + at + joined.len_utf8();
            }
        } else {
            break;
        }
    }
    end
}

/// Where the scan for the cluster boundaries around `index` may start: the
/// line's start, or far enough back that no cluster spans it.
fn scan_start(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    let floor = index.saturating_sub(256);
    let mut start = text[..index].rfind('\n').map_or(0, |at| at + 1).max(floor);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

/// `index` moved back onto a cluster boundary (and into the text).
pub fn snap(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    let mut at = scan_start(text, index);
    loop {
        let end = cluster_end(text, at);
        if end > index || end == at {
            return at;
        }
        at = end;
    }
}

/// The boundary before `index` (0 at the start).
pub fn prev_cluster(text: &str, index: usize) -> usize {
    let index = snap(text, index);
    if index == 0 {
        return 0;
    }
    let mut at = scan_start(text, index - 1);
    loop {
        let end = cluster_end(text, at);
        if end >= index || end == at {
            return at;
        }
        at = end;
    }
}

/// Text as the editor keeps it: line breaks as `\n`, tabs as spaces, no
/// other control characters.
pub fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str(TAB),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A caret and a selection in a text: `caret` is where typing goes,
/// `anchor` the selection's other end (equal: no selection).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Cursor {
    pub caret: usize,
    pub anchor: usize,
}

impl Cursor {
    pub fn at(index: usize) -> Self {
        Self {
            caret: index,
            anchor: index,
        }
    }

    /// The selection, start before end, if there is one.
    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor)
            .then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    /// Both ends on cluster boundaries inside `text`.
    pub fn clamped(&self, text: &str) -> Self {
        Self {
            caret: snap(text, self.caret),
            anchor: snap(text, self.anchor),
        }
    }

    /// The selected text.
    pub fn selected<'a>(&self, text: &'a str) -> &'a str {
        match self.clamped(text).selection() {
            Some((a, b)) => &text[a..b],
            None => "",
        }
    }

    /// Types `input` over the selection (or at the caret). Returns whether
    /// the text changed.
    pub fn insert(&mut self, text: &mut String, input: &str) -> bool {
        let input = sanitize(input);
        let this = self.clamped(text);
        let (a, b) = this.selection().unwrap_or((this.caret, this.caret));
        let room = MAX_TEXT.saturating_sub(text.len() - (b - a));
        let mut take = input.len().min(room);
        while !input.is_char_boundary(take) {
            take -= 1;
        }
        if take == 0 && a == b {
            return false;
        }
        text.replace_range(a..b, &input[..take]);
        *self = Self::at(a + take);
        true
    }

    /// Backspace: the selection, else the cluster before the caret.
    pub fn backspace(&mut self, text: &mut String) -> bool {
        let this = self.clamped(text);
        let (a, b) = match this.selection() {
            Some(range) => range,
            None if this.caret == 0 => return false,
            None => (prev_cluster(text, this.caret), this.caret),
        };
        text.replace_range(a..b, "");
        *self = Self::at(a);
        true
    }

    /// Removes the selection; its text.
    pub fn cut(&mut self, text: &mut String) -> Option<String> {
        let (a, b) = self.clamped(text).selection()?;
        let cut = text[a..b].to_owned();
        text.replace_range(a..b, "");
        *self = Self::at(a);
        Some(cut)
    }

    pub fn select_all(text: &str) -> Self {
        Self {
            anchor: 0,
            caret: text.len(),
        }
    }

    /// The word at `index` selected (or the run of spaces or punctuation
    /// there; at a word's end, that word).
    pub fn word(text: &str, index: usize) -> Self {
        let index = snap(text, index);
        #[derive(PartialEq)]
        enum Class {
            Word,
            Space,
            Break,
            Other,
        }
        let class = |c: char| {
            if c == '\n' {
                Class::Break
            } else if c.is_whitespace() {
                Class::Space
            } else if c.is_alphanumeric() || c == '_' || extends(c) || c == '\u{200d}' {
                Class::Word
            } else {
                Class::Other
            }
        };
        let after = text[index..].chars().next();
        let before = text[..index].chars().next_back();
        // At a word's end (or before a space or a line break), the word
        // behind the finger.
        let pick = match (before, after) {
            (Some(b), Some(a)) if class(b) == Class::Word && class(a) != Class::Word => b,
            (Some(b), None) => b,
            (_, Some(a)) => a,
            (None, None) => return Self::at(0),
        };
        let kind = class(pick);
        if kind == Class::Break {
            return Self::at(index);
        }
        let mut start = if after.is_some_and(|a| class(a) == kind) {
            index
        } else {
            index - before.map_or(0, char::len_utf8)
        };
        while let Some(c) = text[..start].chars().next_back()
            && class(c) == kind
        {
            start -= c.len_utf8();
        }
        let mut end = start;
        while let Some(c) = text[end..].chars().next()
            && class(c) == kind
        {
            end += c.len_utf8();
        }
        Self {
            anchor: snap(text, start),
            caret: next_boundary_at_or_after(text, end),
        }
    }
}

fn next_boundary_at_or_after(text: &str, index: usize) -> usize {
    let snapped = snap(text, index);
    if snapped == index {
        index
    } else {
        cluster_end(text, snapped)
    }
}

/// One wrapped line: the bytes it shows (its line break excluded) and how
/// wide they draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    pub width: f32,
}

/// Widths of clusters in the editor's face: printable ASCII is one mono
/// advance (JetBrains Mono), anything else is measured.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub weight: Weight,
    pub size: f32,
    mono: f32,
}

impl Metrics {
    pub fn new(weight: Weight, size: f32) -> Self {
        Self {
            weight,
            size,
            mono: text::measure("0", weight, size),
        }
    }

    pub fn width(&self, cluster: &str) -> f32 {
        match cluster.as_bytes() {
            [byte] if (0x20..0x7f).contains(byte) => self.mono,
            _ => text::measure(cluster, self.weight, self.size),
        }
    }
}

/// `text` wrapped to `width`.
pub fn wrap(text: &str, width: f32, metrics: &Metrics) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut paragraph = 0;
    loop {
        let end = text[paragraph..]
            .find('\n')
            .map_or(text.len(), |at| paragraph + at);
        wrap_paragraph(text, paragraph, end, width, metrics, &mut lines);
        if end == text.len() {
            break;
        }
        paragraph = end + 1;
    }
    lines
}

fn wrap_paragraph(
    text: &str,
    start: usize,
    end: usize,
    width: f32,
    metrics: &Metrics,
    lines: &mut Vec<Line>,
) {
    let mut line_start = start;
    let mut x = 0.0_f32;
    // After the last run of spaces on this line: where it may break, and
    // the width up to there.
    let mut breakable: Option<(usize, f32)> = None;
    let mut at = start;
    while at < end {
        let next = cluster_end(text, at).min(end);
        let cluster = &text[at..next];
        let w = metrics.width(cluster);
        if cluster == " " {
            x += w;
            breakable = Some((next, x));
            at = next;
            continue;
        }
        if x + w > width && at > line_start {
            match breakable.take() {
                Some((point, before)) if point > line_start => {
                    lines.push(Line {
                        start: line_start,
                        end: point,
                        width: before,
                    });
                    line_start = point;
                    x -= before;
                    if x + w > width && at > line_start {
                        lines.push(Line {
                            start: line_start,
                            end: at,
                            width: x,
                        });
                        line_start = at;
                        x = 0.0;
                    }
                }
                _ => {
                    lines.push(Line {
                        start: line_start,
                        end: at,
                        width: x,
                    });
                    line_start = at;
                    x = 0.0;
                }
            }
        }
        x += w;
        at = next;
    }
    lines.push(Line {
        start: line_start,
        end,
        width: x,
    });
}

/// The line `index` is on: at a soft break, the line after it.
pub fn line_of(lines: &[Line], index: usize) -> usize {
    lines
        .partition_point(|line| line.start <= index)
        .saturating_sub(1)
}

/// How far into its line `index` is drawn.
pub fn x_of(text: &str, line: &Line, index: usize, metrics: &Metrics) -> f32 {
    let index = index.clamp(line.start, line.end);
    let mut x = 0.0;
    let mut at = line.start;
    while at < index {
        let next = cluster_end(text, at).min(line.end);
        x += metrics.width(&text[at..next]);
        at = next;
    }
    x
}

/// The boundary in `line` nearest to `x`.
pub fn index_at(text: &str, line: &Line, x: f32, metrics: &Metrics) -> usize {
    let mut pen = 0.0;
    let mut at = line.start;
    while at < line.end {
        let next = cluster_end(text, at).min(line.end);
        let w = metrics.width(&text[at..next]);
        if x < pen + w / 2.0 {
            return at;
        }
        pen += w;
        at = next;
    }
    line.end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics::new(Weight::Mono, 46.0)
    }

    #[test]
    fn clusters_keep_emoji_sequences_and_marks_whole() {
        let family = "a👨\u{200d}👩\u{200d}👧b";
        assert_eq!(cluster_end(family, 1), family.len() - 1);
        assert_eq!(prev_cluster(family, family.len() - 1), 1);
        assert_eq!(snap(family, 5), 1, "inside the sequence: its start");
        let flag = "🇫🇷!";
        assert_eq!(cluster_end(flag, 0), 8);
        let accented = "e\u{301}x";
        assert_eq!(cluster_end(accented, 0), 3);
        let thumbs = "👍🏽";
        assert_eq!(cluster_end(thumbs, 0), thumbs.len());
    }

    #[test]
    fn typing_backspace_and_selections() {
        let mut text = String::new();
        let mut cursor = Cursor::default();
        assert!(cursor.insert(&mut text, "hello\r\nworld\tx"));
        assert_eq!(text, "hello\nworld    x");
        assert_eq!(cursor, Cursor::at(text.len()));
        cursor.backspace(&mut text);
        assert_eq!(text, "hello\nworld    ");
        // A word, cut, typed over.
        cursor = Cursor::word(&text, 2);
        assert_eq!(cursor.selected(&text), "hello");
        assert_eq!(cursor.cut(&mut text).as_deref(), Some("hello"));
        assert_eq!(text, "\nworld    ");
        cursor = Cursor::word(&text, 6);
        assert_eq!(
            cursor.selected(&text),
            "world",
            "at a word's end: that word"
        );
        cursor.insert(&mut text, "earth");
        assert_eq!(text, "\nearth    ");
        cursor = Cursor::select_all(&text);
        cursor.backspace(&mut text);
        assert!(text.is_empty() && cursor == Cursor::at(0));
        assert!(!cursor.backspace(&mut text));
        // An emoji goes in one Backspace.
        cursor.insert(&mut text, "ok 👍🏽");
        cursor.backspace(&mut text);
        assert_eq!(text, "ok ");
        // A caret left past the end (the text changed under it) is clamped.
        let mut stale = Cursor::at(99);
        stale.insert(&mut text, "!");
        assert_eq!(text, "ok !");
    }

    #[test]
    fn wrapping_breaks_after_spaces_and_inside_long_words() {
        let m = metrics();
        let cell = m.width("a");
        let width = cell * 10.0 + 1.0;
        // Ten cells a line.
        let text = "hello world foo\n\nabcdefghijklmnopqrstuvwxyz";
        let lines = wrap(text, width, &m);
        let shown: Vec<&str> = lines.iter().map(|l| &text[l.start..l.end]).collect();
        assert_eq!(
            shown,
            vec![
                "hello ",
                "world foo",
                "",
                "abcdefghij",
                "klmnopqrst",
                "uvwxyz"
            ]
        );
        // Spaces hang past the edge rather than starting a line.
        let spaces = "abcdefghij   k";
        let lines = wrap(spaces, width, &m);
        assert_eq!(&spaces[lines[1].start..lines[1].end], "k");
        // An empty text and a trailing break are lines too.
        assert_eq!(wrap("", width, &m).len(), 1);
        assert_eq!(wrap("a\n", width, &m).len(), 2);
    }

    #[test]
    fn touches_map_to_glyphs_and_back() {
        let m = metrics();
        let cell = m.width("a");
        let text = "one two three four";
        let lines = wrap(text, cell * 8.0 + 1.0, &m);
        // "one two ", "three ", "four"
        assert_eq!(lines.len(), 3);
        let second = lines[1];
        assert_eq!(
            line_of(&lines, second.start),
            1,
            "a soft break: the next line"
        );
        assert_eq!(index_at(text, &second, cell * 2.4, &m), second.start + 2);
        assert_eq!(index_at(text, &second, cell * 2.6, &m), second.start + 3);
        assert_eq!(index_at(text, &second, 9999.0, &m), second.end);
        assert_eq!(x_of(text, &second, second.start + 3, &m), cell * 3.0);
        // An emoji measures as one glyph, wider or not.
        let emoji = "a😀b";
        let line = wrap(emoji, 9999.0, &m)[0];
        let smiley = m.width("😀");
        assert_eq!(x_of(emoji, &line, 5, &m), cell + smiley);
        assert_eq!(index_at(emoji, &line, cell + smiley * 0.4, &m), 1);
        assert_eq!(index_at(emoji, &line, cell + smiley * 0.6, &m), 5);
    }

    #[test]
    fn a_wrapped_line_draws_as_wide_as_measured() {
        // The view draws each line with one text call: its width is the
        // sum of the clusters' widths the editor measures.
        let m = metrics();
        let text = "Can you also check the logs from yesterday?";
        for line in wrap(text, 700.0, &m) {
            let drawn = text::measure(&text[line.start..line.end], m.weight, m.size);
            assert!(
                (drawn - line.width).abs() < 0.01,
                "{drawn} vs {}",
                line.width
            );
        }
    }
}
