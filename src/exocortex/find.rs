//! Finding text (WP-C1, PAR-332, 331): case-insensitive matches of a query
//! in a text, at most `MAX_MATCHES`, as byte ranges. The reader's Find
//! uses it; Find in chat (E) searches loaded history with it.

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
}
