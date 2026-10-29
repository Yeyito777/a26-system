//! The list's search field (PAR-061): what is typed, and the query the list
//! shows results for, which follows the typing 160 ms after the last key
//! (the iPhone's debounce), so a burst of keys costs one search.

use std::time::{Duration, Instant};

use crate::keys::KeyInput;

/// How long after the last key the results follow.
pub const DEBOUNCE: Duration = Duration::from_millis(160);

/// Results shown at most (the iPhone's cap: the list is virtualised, but
/// sorting thousands per key is wasted work).
pub const LIMIT: usize = 500;

#[derive(Debug, Default, Clone)]
pub struct Search {
    /// What is typed.
    pub text: String,
    /// The query the results are for.
    query: String,
    /// When the typed text takes over, if it has not yet.
    due: Option<Instant>,
    /// The field has Moon's keyboard.
    pub focused: bool,
}

impl Search {
    /// The query results show (trimmed; empty when not searching).
    pub fn query(&self) -> &str {
        self.query.trim()
    }

    /// Searching: the list shows results instead of the tree.
    pub fn active(&self) -> bool {
        !self.query().is_empty()
    }

    /// When the typed text becomes the query, if it is waiting to.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// A key typed into the field; returns whether the text changed.
    pub fn key(&mut self, input: KeyInput, now: Instant) -> bool {
        match input {
            KeyInput::Character(character) if !character.is_control() => {
                self.text.push(character);
            }
            KeyInput::Backspace if !self.text.is_empty() => {
                self.text.pop();
            }
            _ => return false,
        }
        self.due = Some(now + DEBOUNCE);
        true
    }

    /// The typed text becomes the query once the debounce is over; true
    /// when the query changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.due {
            Some(due) if now >= due => {
                self.due = None;
                let changed = self.query.trim() != self.text.trim();
                self.query = self.text.clone();
                changed
            }
            _ => false,
        }
    }

    /// Empties the field and the results at once.
    pub fn clear(&mut self) {
        self.text.clear();
        self.query.clear();
        self.due = None;
    }

    /// Whether `haystack` matches `query` (lower-cased): a case-insensitive
    /// substring, and where it starts in `haystack` (bytes), for the
    /// highlight.
    pub fn find(haystack: &str, query: &str) -> Option<(usize, usize)> {
        if query.is_empty() {
            return None;
        }
        if haystack.is_ascii() && query.is_ascii() {
            let (h, q) = (haystack.as_bytes(), query.as_bytes());
            return (0..=h.len().checked_sub(q.len())?)
                .find(|&at| h[at..at + q.len()].eq_ignore_ascii_case(q))
                .map(|at| (at, at + q.len()));
        }
        // Unicode: compare lower-cased characters, mapping back to bytes.
        let chars: Vec<(usize, char)> = haystack.char_indices().collect();
        let needle: Vec<char> = query.chars().collect();
        let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
        (0..chars.len()).find_map(|start| {
            let mut end = start;
            for &n in &needle {
                let (_, c) = *chars.get(end)?;
                if lower(c) != n {
                    return None;
                }
                end += 1;
            }
            let from = chars[start].0;
            let to = chars.get(end).map_or(haystack.len(), |(at, _)| *at);
            Some((from, to))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_follows_typing_after_the_debounce() {
        let t = Instant::now();
        let mut search = Search::default();
        for (i, c) in "wha".chars().enumerate() {
            assert!(search.key(
                KeyInput::Character(c),
                t + Duration::from_millis(i as u64 * 50)
            ));
        }
        assert!(!search.active());
        assert!(!search.tick(t + Duration::from_millis(200)));
        assert!(search.tick(t + Duration::from_millis(260)));
        assert_eq!(search.query(), "wha");
        assert!(search.due().is_none());
        search.key(KeyInput::Backspace, t);
        search.clear();
        assert!(!search.active() && search.text.is_empty());
        assert!(!search.key(KeyInput::Backspace, t));
    }

    #[test]
    fn matching_ignores_case_and_finds_where() {
        assert_eq!(Search::find("Whale facts", "whale"), Some((0, 5)));
        assert_eq!(Search::find("blue WHALE", "whale"), Some((5, 10)));
        assert_eq!(Search::find("krill", "whale"), None);
        assert_eq!(Search::find("Über Wale", "über"), Some((0, 5)));
        assert_eq!(Search::find("a 🐛 crash", "🐛 c"), Some((2, 8)));
    }
}
