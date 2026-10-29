//! Fuzzy slash search (PAR-201, PAR-236), a port of the TUI's
//! `tui/src/slashsearch.ts` (and the iPhone's `web/src/commands/slashsearch.ts`):
//! words are case-insensitive substrings matched anywhere in a command or
//! argument name, later words search nested arguments ("/model opus" finds
//! "/model anthropic claude-opus-5-5"), shallower paths rank first, then
//! better matches. Submission expands a typed shorthand to the first result.
//!
//! All offsets are byte offsets into the text.

use super::{ArgRegistry, Item};

/// Ranked completions for a slash input (`raw` starts with "/"): the roots
/// (commands and macros) and, below each, its nested arguments from
/// `args_for(root name)`. A result's `insert_text` spells out the whole
/// input; `matches` are the matched ranges of its `name`.
pub fn search_slash(raw: &str, roots: &[Item], args_for: &dyn Fn(&str) -> ArgRegistry) -> Vec<Item> {
    let _ = (raw, roots, args_for);
    unimplemented!("WP-D logic subagent")
}

/// A typed shorthand that submission replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shorthand {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// Where shorthands are resolved from.
pub struct ShorthandSources<'a> {
    /// Leading-position search (commands and macros).
    pub prompt: &'a dyn Fn(&str) -> Vec<Item>,
    /// Inline modifier roots (`/model`, `/effort`, `/fast`, `/ultrafast`;
    /// not `/queue`).
    pub inline_roots: &'a [&'a str],
    pub inline_args: &'a dyn Fn(&str) -> ArgRegistry,
}

/// The shorthands submission expands (`findSlashShorthands`): a single-line
/// "/…" prompt resolves to its first suggestion; inline modifiers resolve
/// their arguments; input matching nothing and `/queue` targets are left
/// alone.
pub fn find_shorthands(text: &str, sources: &ShorthandSources) -> Vec<Shorthand> {
    let _ = (text, sources);
    unimplemented!("WP-D logic subagent")
}

/// `text` with `shorthands` (from `find_shorthands`) replaced.
pub fn apply_shorthands(text: &str, shorthands: &[Shorthand]) -> String {
    let mut out = text.to_owned();
    for s in shorthands.iter().rev() {
        out.replace_range(s.start..s.end, &s.replacement);
    }
    out
}
