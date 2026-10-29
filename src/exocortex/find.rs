//! Finding text (WP-C1, PAR-332, 331): case-insensitive matches of a query
//! in a text, at most `MAX_MATCHES`, as byte ranges. The reader's Find
//! uses it; Find in chat (E) searches loaded history with it.

#![allow(dead_code)] // search_messages is for E's Find in chat sheet.

/// At most this many matches are found (the iPhone's).
pub const MAX_MATCHES: usize = 500;

/// Where `query` occurs in `text` (case-insensitive, not overlapping), as
/// byte ranges of `text`, at most `max`.
pub fn matches(text: &str, query: &str, max: usize) -> Vec<(usize, usize)> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() || max == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // ASCII: compare bytes, ignoring case.
    if query.iter().all(char::is_ascii) {
        let needle: Vec<u8> = query.iter().map(|c| *c as u8).collect();
        let hay = text.as_bytes();
        let mut at = 0;
        while at + needle.len() <= hay.len() && out.len() < max {
            if hay[at..at + needle.len()]
                .iter()
                .zip(&needle)
                .all(|(h, n)| h.to_ascii_lowercase() == *n)
            {
                out.push((at, at + needle.len()));
                at += needle.len();
            } else {
                at += 1;
            }
        }
        return out;
    }
    // Otherwise character by character (lowercasing may change lengths).
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut index = 0;
    while index < chars.len() && out.len() < max {
        let mut matched = 0;
        let mut taken = 0;
        while matched < query.len() && index + taken < chars.len() {
            let lower: Vec<char> = chars[index + taken].1.to_lowercase().collect();
            if query[matched..].starts_with(&lower) {
                matched += lower.len();
                taken += 1;
            } else {
                break;
            }
        }
        if matched == query.len() {
            let start = chars[index].0;
            let end = chars.get(index + taken).map_or(text.len(), |(at, _)| *at);
            out.push((start, end));
            index += taken.max(1);
        } else {
            index += 1;
        }
    }
    out
}

// ----- Find in chat (PAR-331, E's sheet) ----------------------------------------

/// A preview starts this many characters before its match…
pub const PREVIEW_BEFORE: usize = 80;
/// …and is at most this long (the iPhone's).
pub const PREVIEW_MAX: usize = 350;

/// A match in a message: which message (`key`), its role label, a preview
/// of the text around it (whitespace collapsed) and where the match is in
/// the preview (character offsets).
#[derive(Debug, Clone, PartialEq)]
pub struct Hit<K> {
    pub key: K,
    pub role: String,
    pub preview: String,
    pub start: usize,
    pub end: usize,
}

/// `text` folded for searching (lower case, accents removed) and, for every
/// character of the result, the byte offset in `text` it came from (one
/// more for the end).
pub fn fold(text: &str) -> (Vec<char>, Vec<usize>) {
    let mut folded = Vec::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len() + 1);
    for (at, c) in text.char_indices() {
        for f in c.to_lowercase() {
            folded.push(unaccent(f));
            map.push(at);
        }
    }
    map.push(text.len());
    (folded, map)
}

/// A Latin letter without its accent (what NFD and dropping marks give
/// for the common ones).
fn unaccent(c: char) -> char {
    match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' => 'd',
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' => 'h',
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò'..='ö' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' => 't',
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// Every match of `query` in the messages (key, role label, plain text),
/// in order, at most `max`: case- and accent-insensitive, each with a
/// preview from 80 characters before it, at most 350, whitespace
/// collapsed (the iPhone's `searchMessages`).
pub fn search_messages<K: Clone>(
    messages: &[(K, String, String)],
    query: &str,
    max: usize,
) -> Vec<Hit<K>> {
    let (needle, _) = fold(query.trim());
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (key, role, text) in messages {
        let (hay, map) = fold(text);
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut at = 0;
        while at + needle.len() <= hay.len() {
            if hay[at..at + needle.len()] != needle[..] {
                at += 1;
                continue;
            }
            let (s, e) = (map[at], map[at + needle.len()]);
            let s_char = chars.partition_point(|(i, _)| *i < s);
            let e_char = chars.partition_point(|(i, _)| *i < e);
            let from = s_char.saturating_sub(PREVIEW_BEFORE);
            let to = chars.len().min(from + PREVIEW_MAX);
            let collapse = |range: &[(usize, char)]| -> String {
                let mut out = String::new();
                let mut space = false;
                for (_, c) in range {
                    if c.is_whitespace() {
                        space = true;
                    } else {
                        if space {
                            out.push(' ');
                        }
                        space = false;
                        out.push(*c);
                    }
                }
                if space {
                    out.push(' ');
                }
                out
            };
            let lead = if from > 0 { "…" } else { "" };
            let preview = format!(
                "{lead}{}{}",
                collapse(&chars[from..to]),
                if to < chars.len() { "…" } else { "" }
            );
            let before = format!("{lead}{}", collapse(&chars[from..s_char]))
                .chars()
                .count();
            let length = collapse(&chars[s_char..e_char]).chars().count();
            hits.push(Hit {
                key: key.clone(),
                role: role.clone(),
                start: before,
                end: (before + length).min(preview.chars().count()),
                preview,
            });
            if hits.len() >= max {
                return hits;
            }
            at += needle.len();
        }
    }
    hits
}

/// "2/14", "500+" when capped, "No matches"; empty without a query.
pub fn status(query: &str, found: usize, current: usize) -> String {
    if query.is_empty() {
        String::new()
    } else if found == 0 {
        "No matches".into()
    } else {
        format!(
            "{}/{}{}",
            current + 1,
            found,
            if found >= MAX_MATCHES { "+" } else { "" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_case_insensitively_without_overlap() {
        assert_eq!(matches("Abc abc ABC", "abc", 10), [(0, 3), (4, 7), (8, 11)]);
        assert_eq!(matches("aaaa", "aa", 10), [(0, 2), (2, 4)]);
        assert_eq!(matches("abc", "", 10), []);
        assert_eq!(matches("abcabc", "abc", 1), [(0, 3)]);
        // Byte ranges in text with wide characters.
        assert_eq!(matches("é Café CAFÉ", "café", 10), [(3, 8), (9, 14)]);
        assert_eq!(matches("日本語のテキスト", "テキ", 10), [(12, 18)]);
        assert_eq!(status("x", 0, 0), "No matches");
        assert_eq!(status("x", 3, 1), "2/3");
        assert_eq!(status("x", MAX_MATCHES, 0), "1/500+");
        assert_eq!(status("", 0, 0), "");
    }

    #[test]
    fn finds_in_messages_with_previews() {
        let long = format!("{} Café needle {}", "a ".repeat(60), "b ".repeat(300));
        let messages = vec![
            (
                1_u64,
                "USER".to_owned(),
                "Where is the CAFE?\n\nNear\tthe café.".to_owned(),
            ),
            (2, "ASSISTANT".to_owned(), long.clone()),
        ];
        let hits = search_messages(&messages, "cafe", 10);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].key, 1);
        assert_eq!(hits[0].preview, "Where is the CAFE? Near the café.");
        let shown = |hit: &Hit<u64>| {
            hit.preview
                .chars()
                .skip(hit.start)
                .take(hit.end - hit.start)
                .collect::<String>()
        };
        assert_eq!(shown(&hits[0]), "CAFE");
        assert_eq!(shown(&hits[1]), "café");
        // Far into a message: the preview starts 80 characters before, with
        // an ellipsis each side, at most 350 characters.
        let hit = &hits[2];
        assert_eq!(hit.role, "ASSISTANT");
        assert!(hit.preview.starts_with('…') && hit.preview.ends_with('…'));
        assert_eq!(shown(hit), "Café");
        assert!(hit.preview.chars().count() <= PREVIEW_MAX + 2);
        assert!(search_messages(&messages, "  ", 10).is_empty());
        assert_eq!(search_messages(&messages, "a", 5).len(), 5);
    }
}
