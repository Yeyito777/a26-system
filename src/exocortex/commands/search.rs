//! Fuzzy slash search (PAR-201, PAR-236), a port of the TUI's
//! `tui/src/slashsearch.ts` (and the iPhone's `web/src/commands/slashsearch.ts`):
//! words are case-insensitive substrings matched anywhere in a command or
//! argument name, later words search nested arguments ("/model opus" finds
//! "/model anthropic claude-opus-5-5"), shallower paths rank first, then
//! better matches. Submission expands a typed shorthand to the first result.
//!
//! All offsets are byte offsets into the text.

use std::collections::HashSet;

use super::{ArgRegistry, Item};

/// Ranked completions for a slash input (`raw` starts with "/"): the roots
/// (commands and macros) and, below each, its nested arguments from
/// `args_for(root name)`. A result's `insert_text` spells out the whole
/// input; `matches` are the matched ranges of its `name`; `exact` marks one
/// that passes through a destructive argument `raw` does not type out.
pub fn search_slash(
    raw: &str,
    roots: &[Item],
    args_for: &dyn Fn(&str) -> ArgRegistry,
) -> Vec<Item> {
    search(raw, roots, args_for)
        .into_iter()
        .map(|c| {
            let exact = !c.typed_out(raw);
            Item { exact, ..c.item }
        })
        .collect()
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
    /// Leading-position search (commands and macros): `search_slash`, whose
    /// `exact` results are never expanded to.
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
    if !text.contains('/') {
        return Vec::new();
    }
    let words = words_in(text);
    let Some(first) = words.first() else {
        return Vec::new();
    };
    if let Some(shorthand) = prompt_shorthand(text, first, sources) {
        return vec![shorthand];
    }

    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let root = words[i].word;
        if root != "/queue" && sources.inline_roots.contains(&root) {
            if let Some(shorthand) = inline_shorthand(text, &words, i, &(sources.inline_args)(root))
            {
                while i + 1 < words.len() && words[i + 1].start < shorthand.end {
                    i += 1;
                }
                out.push(shorthand);
            }
        }
        i += 1;
    }
    out
}

/// `text` with `shorthands` (from `find_shorthands`) replaced.
pub fn apply_shorthands(text: &str, shorthands: &[Shorthand]) -> String {
    let mut out = text.to_owned();
    for s in shorthands.iter().rev() {
        out.replace_range(s.start..s.end, &s.replacement);
    }
    out
}

/// JavaScript's `\s`, which the TUI splits and trims with.
pub(crate) fn is_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

// ── Query parsing ───────────────────────────────────────────────────

struct Query {
    /// Lowercased whitespace-separated words.
    words: Vec<String>,
    /// The query ends in whitespace, so its final word must be followed by a space.
    trailing_space: bool,
}

fn parse_query(text: &str) -> Query {
    Query {
        words: text
            .to_lowercase()
            .split(is_space)
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect(),
        trailing_space: text.chars().next_back().is_some_and(is_space),
    }
}

// ── Path matching ───────────────────────────────────────────────────

/// The strings an item is found by: its name, insert text and aliases.
fn forms(item: &Item) -> impl Iterator<Item = &str> {
    let insert = item.insert_text.as_deref().filter(|text| !text.is_empty());
    std::iter::once(item.name.as_str())
        .chain(insert)
        .chain(item.aliases.iter().map(String::as_str))
}

/// Root names match without their slash so "/del" can find "/model".
fn normalize(form: &str, is_root: bool) -> String {
    let form = if is_root {
        form.char_indices().nth(1).map_or("", |(at, _)| &form[at..])
    } else {
        form
    };
    form.to_lowercase()
}

/// Lowercased strings a segment can match.
fn segment_forms(item: &Item, is_root: bool) -> Vec<String> {
    forms(item).map(|form| normalize(form, is_root)).collect()
}

/// 0 = the whole segment, 1 = starts the segment, 2 = starts after punctuation or a space, 3 = mid-word.
fn word_match_quality(
    text: &str,
    at: usize,
    end: usize,
    segment_start: usize,
    segment_end: usize,
) -> u32 {
    if at == segment_start {
        return if end == segment_end { 0 } else { 1 };
    }
    if text[..at]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
    {
        3
    } else {
        2
    }
}

struct PathMatch {
    /// Sum of word match qualities; lower is better.
    score: u32,
    /// Matched word ranges in the space-joined segment text.
    ranges: Vec<(usize, usize)>,
}

/// Query words matched against a path's space-joined, lowercased segments.
struct PathText<'a> {
    text: String,
    starts: Vec<usize>,
    lens: Vec<usize>,
    query: &'a Query,
    first_word_segment: Option<usize>,
}

impl PathText<'_> {
    fn segment_at(&self, index: usize) -> usize {
        let mut segment = 0;
        while segment + 1 < self.starts.len() && self.starts[segment + 1] <= index {
            segment += 1;
        }
        segment
    }

    fn search(
        &self,
        word_index: usize,
        prev_end: usize,
        prev_segment: usize,
        ranges: &mut [(usize, usize)],
    ) -> Option<u32> {
        let word = self.query.words[word_index].as_str();
        let text = self.text.as_str();
        let bytes = text.as_bytes();
        let last_segment = self.starts.len() - 1;
        let mut from = prev_end;
        while let Some(found) = text.get(from..).and_then(|rest| rest.find(word)) {
            let at = from + found;
            from = at + text[at..].chars().next().map_or(1, char::len_utf8);
            let segment = self.segment_at(at);
            if word_index == 0 {
                if self
                    .first_word_segment
                    .is_some_and(|first| first != segment)
                {
                    continue;
                }
            } else if segment <= prev_segment
                && !(at == prev_end + 1 && bytes.get(prev_end) == Some(&b' '))
            {
                continue;
            }

            let end = at + word.len();
            let start = self.starts[segment];
            let quality = word_match_quality(text, at, end, start, start + self.lens[segment]);

            if word_index == self.query.words.len() - 1 {
                let lands_in_last = if self.query.trailing_space {
                    bytes.get(end) == Some(&b' ')
                        && end + 1 < text.len()
                        && self.segment_at(end + 1) == last_segment
                } else {
                    segment == last_segment
                };
                if !lands_in_last {
                    continue;
                }
                ranges[word_index] = (at, end);
                return Some(quality);
            }

            if let Some(rest) = self.search(word_index + 1, end, segment, ranges) {
                ranges[word_index] = (at, end);
                return Some(quality + rest);
            }
        }
        None
    }
}

/// Match query words against a completion path (lowercased segments).
///
/// Each word is an exact substring. A later word either continues the previous
/// one after a single space (multi-word names such as conversation titles) or
/// starts in a later segment, so "opus" can skip the provider segment and land
/// in "claude-opus-5-5". The final word must land in the final segment, or with
/// a trailing space be followed by a space that leads into it.
fn match_completion_path(
    segments: &[&str],
    query: &Query,
    first_word_segment: Option<usize>,
) -> Option<PathMatch> {
    if query.words.is_empty() || segments.is_empty() {
        return None;
    }
    let mut starts = Vec::with_capacity(segments.len());
    let mut offset = 0;
    for segment in segments {
        starts.push(offset);
        offset += segment.len() + 1;
    }
    let path = PathText {
        text: segments.join(" "),
        starts,
        lens: segments.iter().map(|segment| segment.len()).collect(),
        query,
        first_word_segment,
    };
    let mut ranges = vec![(0, 0); query.words.len()];
    let score = path.search(0, 0, 0, &mut ranges)?;
    Some(PathMatch { score, ranges })
}

/// Best match across the final segment's name, insert text, and aliases; ranges always come from the name.
fn match_path(path: &[&Item], query: &Query, includes_root: bool) -> Option<PathMatch> {
    let last_word = query.words.last()?;
    let (last, parents) = path.split_last()?;
    let last_forms = segment_forms(last, includes_root && parents.is_empty());
    // Cheap reject: without a trailing space the final word must sit inside the final segment.
    if !query.trailing_space
        && !last_forms
            .iter()
            .any(|form| form.contains(last_word.as_str()))
    {
        return None;
    }
    let parents: Vec<String> = parents
        .iter()
        .enumerate()
        .map(|(i, item)| normalize(&item.name, includes_root && i == 0))
        .collect();
    let mut segments: Vec<&str> = parents.iter().map(String::as_str).collect();
    segments.push("");

    let mut best: Option<PathMatch> = None;
    let mut name_ranges = Vec::new();
    for (i, form) in last_forms.iter().enumerate() {
        if !query.trailing_space && !form.contains(last_word.as_str()) {
            continue;
        }
        *segments.last_mut()? = form.as_str();
        let Some(found) = match_completion_path(&segments, query, includes_root.then_some(0))
        else {
            continue;
        };
        if i == 0 {
            name_ranges.clone_from(&found.ranges);
        }
        if best.as_ref().is_none_or(|best| found.score < best.score) {
            best = Some(found);
        }
    }
    best.map(|best| PathMatch {
        score: best.score,
        ranges: name_ranges,
    })
}

// ── Ranked search ───────────────────────────────────────────────────

/// A result and the spellings submission requires before expanding to it.
struct Completion {
    item: Item,
    /// Spellings of each path segment from the first `exact` one on;
    /// submission expands to it only when every one was typed as a whole word.
    exact_forms: Option<Vec<Vec<String>>>,
}

impl Completion {
    /// Whether submission may expand `typed` into it: its destructive segments are typed out.
    fn typed_out(&self, typed: &str) -> bool {
        let Some(exact_forms) = &self.exact_forms else {
            return true;
        };
        let typed = typed.to_lowercase();
        let words: HashSet<&str> = typed.split(is_space).filter(|w| !w.is_empty()).collect();
        exact_forms
            .iter()
            .all(|forms| forms.iter().any(|form| words.contains(form.as_str())))
    }
}

struct Ranked {
    completion: Completion,
    depth: usize,
    score: u32,
}

/// Depth-first walk of every completion path below `key`.
fn completion_paths<'a>(
    registry: &'a ArgRegistry,
    key: &str,
    parents: &[&'a Item],
    out: &mut Vec<Vec<&'a Item>>,
) {
    for item in registry.get(key).into_iter().flatten() {
        let mut path = parents.to_vec();
        path.push(item);
        out.push(path.clone());
        completion_paths(registry, &format!("{key} {}", item.name), &path, out);
    }
}

/// Whether lowercasing keeps every byte offset of `text`.
fn lowercase_keeps_offsets(text: &str) -> bool {
    text.chars()
        .all(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() == c.len_utf8())
}

/// One popup row for a path; the insert text always spells out the full slash input.
fn path_completion(
    path: &[&Item],
    typed_prefix: &str,
    text_ranges: &[(usize, usize)],
    below_exact: bool,
) -> Completion {
    let last = path[path.len() - 1];
    let name = path
        .iter()
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    // Root segments match without their slash; skip ranges if lowercasing changed lengths.
    let offset = usize::from(name.starts_with('/'));
    let matches = if lowercase_keeps_offsets(&name) {
        text_ranges
            .iter()
            .map(|&(start, end)| (start + offset, end + offset))
            .collect()
    } else {
        Vec::new()
    };
    let first_exact = if below_exact {
        Some(0)
    } else {
        path.iter().position(|item| item.exact)
    };
    let exact_forms = first_exact.map(|first| {
        path[first..]
            .iter()
            .map(|item| forms(item).map(str::to_lowercase).collect())
            .collect()
    });

    let item = if path.len() == 1 && typed_prefix.is_empty() {
        Item {
            matches,
            ..last.clone()
        }
    } else {
        let insert = path
            .iter()
            .map(|item| item.insert())
            .collect::<Vec<_>>()
            .join(" ");
        Item {
            name,
            desc: last.desc.clone(),
            insert_text: Some(format!("{typed_prefix}{insert}")),
            matches,
            ..Item::default()
        }
    };
    Completion { item, exact_forms }
}

/// Shallow paths first, then stronger matches; ties keep registry order.
fn rank_paths(
    paths: &[Vec<&Item>],
    query: &Query,
    includes_root: bool,
    typed_prefix: &str,
    below_exact: bool,
) -> Vec<Ranked> {
    let mut ranked: Vec<Ranked> = paths
        .iter()
        .filter_map(|path| {
            let found = match_path(path, query, includes_root)?;
            Some(Ranked {
                completion: path_completion(path, typed_prefix, &found.ranges, below_exact),
                depth: path.len(),
                score: found.score,
            })
        })
        .collect();
    ranked.sort_by_key(|ranked| (ranked.depth, ranked.score));
    ranked
}

/// Whether a spelled-out key such as "/chrono cancel" passes through a destructive argument.
fn key_below_exact(registry: &ArgRegistry, key: &str) -> bool {
    let words: Vec<&str> = key.split(' ').collect();
    (1..words.len()).any(|i| {
        let parent = words[..i].join(" ");
        registry
            .get(&parent)
            .is_some_and(|items| items.iter().any(|item| item.exact && item.name == words[i]))
    })
}

/// Search below the deepest registry key the input spells out exactly, e.g.
/// "/model " or "/model openai ". Returns None when no key is spelled out.
fn search_anchored_args(raw: &str, registry: &ArgRegistry) -> Option<Vec<Ranked>> {
    let mut keys: Vec<&String> = registry.keys().collect();
    keys.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    for key in keys {
        let Some(head) = raw.get(..key.len()) else {
            continue;
        };
        if head.to_lowercase() != key.to_lowercase() || !raw[key.len()..].starts_with(is_space) {
            continue;
        }
        let rest = raw[key.len()..].trim_start_matches(is_space);
        let typed_prefix = &raw[..raw.len() - rest.len()];
        let query = parse_query(rest);
        let below_exact = key_below_exact(registry, key);
        if query.words.is_empty() {
            let items = registry.get(key).into_iter().flatten();
            return Some(
                items
                    .map(|item| Ranked {
                        completion: path_completion(&[item], typed_prefix, &[], below_exact),
                        depth: 1,
                        score: 0,
                    })
                    .collect(),
            );
        }
        let mut paths = Vec::new();
        completion_paths(registry, key, &[], &mut paths);
        return Some(rank_paths(&paths, &query, false, typed_prefix, below_exact));
    }
    None
}

/// Search slash roots by substring. The first word must match the root name;
/// any further words search that root's nested arguments.
fn search_roots(
    raw: &str,
    roots: &[Item],
    args_for: &dyn Fn(&str) -> ArgRegistry,
) -> Vec<Completion> {
    let body = &raw[1..];
    if body.is_empty() {
        return roots
            .iter()
            .map(|root| Completion {
                item: root.clone(),
                exact_forms: None,
            })
            .collect();
    }
    if body.starts_with(is_space) {
        return Vec::new();
    }

    let query = parse_query(body);
    let root_word = query.words[0].as_str();
    let search_args = query.words.len() > 1 || query.trailing_space;
    let matching: Vec<&Item> = roots
        .iter()
        .filter(|root| normalize(&root.name, true).contains(root_word))
        .collect();
    let registries: Vec<ArgRegistry> = if search_args {
        matching.iter().map(|root| args_for(&root.name)).collect()
    } else {
        Vec::new()
    };
    let mut paths = Vec::new();
    for (i, &root) in matching.iter().enumerate() {
        paths.push(vec![root]);
        if search_args {
            completion_paths(&registries[i], &root.name, &[root], &mut paths);
        }
    }
    rank_paths(&paths, &query, true, "", false)
        .into_iter()
        .map(|ranked| ranked.completion)
        .collect()
}

/// A fully typed command path anchors the search; otherwise roots are searched by substring.
fn search(raw: &str, roots: &[Item], args_for: &dyn Fn(&str) -> ArgRegistry) -> Vec<Completion> {
    if !raw.starts_with('/') {
        return Vec::new();
    }
    if raw.contains(is_space) {
        let base = raw.split(is_space).next().unwrap_or(raw);
        if let Some(anchored) = search_anchored_args(raw, &args_for(base)) {
            return anchored
                .into_iter()
                .map(|ranked| ranked.completion)
                .collect();
        }
    }
    search_roots(raw, roots, args_for)
}

// ── Shorthand resolution ────────────────────────────────────────────

struct WordPosition<'a> {
    word: &'a str,
    start: usize,
    end: usize,
}

fn words_in(text: &str) -> Vec<WordPosition<'_>> {
    let mut words = Vec::new();
    let mut start = None;
    for (at, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (start, is_space(c)) {
            (None, false) => start = Some(at),
            (Some(from), true) => {
                words.push(WordPosition {
                    word: &text[from..at],
                    start: from,
                    end: at,
                });
                start = None;
            }
            _ => {}
        }
    }
    words
}

fn normalized_command_key(text: &str) -> String {
    text.to_lowercase()
        .split(is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A /queue target is followed by free message text that a substring match could swallow.
fn rewrites_queue_target(replacement: &str) -> bool {
    replacement
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("/queue"))
        && replacement[6..].starts_with(is_space)
}

/// A single-line prompt starting with "/" sends the popup's first option for the same text.
fn prompt_shorthand(
    text: &str,
    first: &WordPosition,
    sources: &ShorthandSources,
) -> Option<Shorthand> {
    let line = text[first.start..].trim_end_matches(is_space);
    if line.contains('\n') {
        return None;
    }
    let best = (sources.prompt)(line).into_iter().next()?;
    // A destructive argument is never guessed: unless typed out, the text goes as typed.
    if best.exact {
        return None;
    }
    let replacement = best.insert();
    if rewrites_queue_target(replacement) {
        return None;
    }
    Some(Shorthand {
        start: first.start,
        end: first.start + line.len(),
        replacement: replacement.to_owned(),
    })
}

/// Resolve the longest run of words after an inline modifier to its first
/// complete argument. Prose follows, so "please /model op fix" names a model
/// rather than stopping at the "openai" provider and reading "fix" as a model id.
fn inline_shorthand(
    text: &str,
    words: &[WordPosition],
    root_index: usize,
    registry: &ArgRegistry,
) -> Option<Shorthand> {
    let root = &words[root_index];
    let parent_keys: HashSet<String> = registry
        .iter()
        .filter(|(_, items)| !items.is_empty())
        .map(|(key, _)| normalized_command_key(key))
        .collect();
    let mut shorthand = None;
    for j in root_index + 1..words.len() {
        if words[j].word.starts_with('/') || text[words[j - 1].end..words[j].start].contains('\n') {
            break;
        }
        let typed = &text[root.start..words[j].end];
        // Matches only shrink as words are added, so the first miss ends the run.
        let Some(ranked) =
            search_anchored_args(typed, registry).filter(|ranked| !ranked.is_empty())
        else {
            break;
        };
        let best = ranked.iter().find(|ranked| {
            let completion = &ranked.completion;
            !parent_keys.contains(&normalized_command_key(completion.item.insert()))
                && completion.typed_out(typed)
        });
        if let Some(best) = best {
            shorthand = Some(Shorthand {
                start: root.start,
                end: words[j].end,
                replacement: best.completion.item.insert().to_owned(),
            });
        }
    }
    shorthand
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::macros;

    fn items(names: &[&str]) -> Vec<Item> {
        names.iter().map(|name| Item::new(*name, "")).collect()
    }

    fn exact(name: &str) -> Item {
        Item {
            exact: true,
            ..Item::new(name, "")
        }
    }

    fn aliased(name: &str, aliases: &[&str]) -> Item {
        Item {
            aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
            ..Item::new(name, "")
        }
    }

    fn registry(entries: Vec<(&str, Vec<Item>)>) -> ArgRegistry {
        entries
            .into_iter()
            .map(|(key, list)| (key.to_owned(), list))
            .collect()
    }

    /// The TUI fixture's providers: `/model` → providers → models.
    fn models(root: &str) -> ArgRegistry {
        let openai = format!("{root} openai");
        let anthropic = format!("{root} anthropic");
        registry(vec![
            (root, items(&["openai", "anthropic"])),
            (&openai, items(&["gpt-5.4", "gpt-5.4-mini"])),
            (&anthropic, items(&["claude-opus-5-5", "claude-sonnet-5-5"])),
        ])
    }

    const COMMANDS: &[&str] = &[
        "/help",
        "/model",
        "/default-model",
        "/effort",
        "/fast",
        "/ultrafast",
        "/new",
        "/rename",
        "/goal",
        "/queue",
        "/theme",
        "/instructions",
        "/usage",
        "/login",
        "/account",
        "/chrono",
    ];

    fn command_args(root: &str) -> ArgRegistry {
        match root {
            "/model" | "/default-model" => models(root),
            "/effort" => registry(vec![(
                "/effort",
                items(&["low", "medium", "high", "xhigh"]),
            )]),
            "/goal" => registry(vec![(
                "/goal",
                vec![
                    Item::new("pause", ""),
                    Item::new("resume", ""),
                    Item::new("complete", ""),
                    exact("clear"),
                    Item::new("max-time", ""),
                ],
            )]),
            "/theme" => registry(vec![(
                "/theme",
                items(&["whale", "cerberus", "日本語テーマ", "Ünïcode", "İstanbul"]),
            )]),
            "/instructions" => registry(vec![(
                "/instructions",
                vec![
                    aliased("conversation", &["convo", "c"]),
                    aliased("folder", &["f"]),
                    aliased("global", &["g"]),
                    aliased("tools", &["t"]),
                    exact("clear"),
                ],
            )]),
            "/usage" => registry(vec![("/usage", vec![exact("reset")])]),
            "/login" => registry(vec![
                ("/login", items(&["openai", "anthropic"])),
                (
                    "/login openai",
                    vec![
                        Item::new("browser", ""),
                        Item::new("code", ""),
                        Item::new("key", ""),
                        Item::new("add", ""),
                        exact("remove"),
                    ],
                ),
                ("/login openai add", items(&["browser", "code"])),
            ]),
            "/account" => registry(vec![
                ("/account", items(&["claude"])),
                (
                    "/account claude",
                    vec![Item::new("add", ""), exact("remove")],
                ),
                (
                    "/account anthropic",
                    vec![Item::new("add", ""), exact("remove")],
                ),
            ]),
            "/chrono" => registry(vec![
                (
                    "/chrono",
                    vec![
                        Item::new("list", ""),
                        exact("cancel"),
                        Item::new("in", ""),
                        Item::new("every", ""),
                    ],
                ),
                ("/chrono cancel", items(&["standup-0123", "all"])),
            ]),
            "/queue" => registry(vec![(
                "/queue",
                vec![aliased("Fix login bug", &["Fix login bug", "conv-fix"])],
            )]),
            _ => ArgRegistry::new(),
        }
    }

    fn inline_command_args(root: &str) -> ArgRegistry {
        match root {
            "/model" => models(root),
            "/effort" => registry(vec![(
                "/effort",
                items(&["low", "medium", "high", "xhigh"]),
            )]),
            "/fast" | "/ultrafast" => registry(vec![(root, items(&["on", "off"]))]),
            _ => ArgRegistry::new(),
        }
    }

    /// Macro arguments merged with a command registry (`slashArgRegistry`).
    fn with_macros(args: ArgRegistry) -> ArgRegistry {
        let mut merged = macros::catalog(None).args.clone();
        merged.extend(args);
        merged
    }

    fn roots() -> Vec<Item> {
        let mut roots = items(COMMANDS);
        roots.extend(macros::catalog(None).items.iter().cloned());
        roots
    }

    fn inline_roots() -> Vec<Item> {
        let mut roots = macros::catalog(None).items.clone();
        roots.extend(items(&[
            "/model",
            "/effort",
            "/fast",
            "/ultrafast",
            "/queue",
        ]));
        roots
    }

    fn completions(raw: &str) -> Vec<Item> {
        search_slash(raw, &roots(), &|root| with_macros(command_args(root)))
    }

    fn inline_completions(raw: &str) -> Vec<Item> {
        search_slash(raw, &inline_roots(), &|root| {
            with_macros(inline_command_args(root))
        })
    }

    fn shorthands(text: &str) -> Vec<Shorthand> {
        let sources = ShorthandSources {
            prompt: &completions,
            inline_roots: &["/model", "/effort", "/fast", "/ultrafast"],
            inline_args: &|root| with_macros(inline_command_args(root)),
        };
        find_shorthands(text, &sources)
    }

    fn resolve(text: &str) -> String {
        apply_shorthands(text, &shorthands(text))
    }

    #[test]
    fn resolves_a_unique_nested_argument() {
        assert_eq!(resolve("/model opus"), "/model anthropic claude-opus-5-5");
        assert_eq!(
            resolve("/model anthropic son"),
            "/model anthropic claude-sonnet-5-5"
        );
        assert_eq!(
            resolve("/default-model opus"),
            "/default-model anthropic claude-opus-5-5"
        );
    }

    #[test]
    fn sends_the_first_option_when_several_match() {
        assert_eq!(resolve("/model claude"), "/model anthropic claude-opus-5-5");
        assert_eq!(resolve("/model pus"), "/model anthropic claude-opus-5-5");
        assert_eq!(resolve("/mod opus"), "/model anthropic claude-opus-5-5");
        assert_eq!(resolve("/mod"), "/model");
        assert_eq!(resolve("/model op"), "/model openai");
        assert_eq!(resolve("/theme cer"), "/theme cerberus");
    }

    #[test]
    fn leaves_unmatched_and_custom_arguments_unchanged() {
        for text in [
            "/model openai my-custom-model",
            "/login deepseek sk-test",
            "/home/me/notes.txt what is this",
            "/rename My chat",
            "/goal fix the tests",
            "/notacommand at all",
            "no slash here",
        ] {
            assert_eq!(resolve(text), text);
        }
    }

    #[test]
    fn a_trailing_space_does_not_pick_a_child() {
        assert_eq!(resolve("/model"), "/model");
        assert_eq!(resolve("/model anthropic"), "/model anthropic");
        // Submission trims; the shorthand ends before the space.
        assert_eq!(resolve("/model "), "/model ");
    }

    #[test]
    fn prefers_an_exactly_typed_argument() {
        assert_eq!(resolve("/model openai gpt-5.4"), "/model openai gpt-5.4");
        assert_eq!(resolve("/model mini"), "/model openai gpt-5.4-mini");
        assert_eq!(resolve("/effort h"), "/effort high");
    }

    #[test]
    fn prompt_commands_and_macros_resolve_only_when_the_whole_input_does() {
        assert_eq!(resolve("/goal pa"), "/goal pause");
        assert_eq!(
            resolve("/goal use the new parser"),
            "/goal use the new parser"
        );
        assert_eq!(resolve("/exocortex tui"), "/exocortex tui-quality");
        assert_eq!(resolve("/exocortex tui is slow"), "/exocortex tui is slow");
        // A multi-line prompt is not a command.
        assert_eq!(resolve("/mod opus\nmore"), "/mod opus\nmore");
    }

    #[test]
    fn inline_commands_resolve_up_to_the_prose() {
        assert_eq!(
            resolve("please /model opus fix this"),
            "please /model anthropic claude-opus-5-5 fix this"
        );
        assert_eq!(
            resolve("please /model claude fix this"),
            "please /model anthropic claude-opus-5-5 fix this"
        );
        // A provider alone is incomplete inline, so the prose is never read as a model id.
        assert_eq!(
            resolve("please /model op fix it"),
            "please /model anthropic claude-opus-5-5 fix it"
        );
        assert_eq!(
            resolve("please /model anthropic fix it"),
            "please /model anthropic fix it"
        );
        assert_eq!(
            resolve("/model opus /effort h go"),
            "/model anthropic claude-opus-5-5 /effort high go"
        );
        assert_eq!(resolve("explain /effort levels"), "explain /effort levels");
        assert_eq!(resolve("hi /effort hi there"), "hi /effort high there");
        assert_eq!(resolve("go /fast of\nnow"), "go /fast off\nnow");
        // A newline ends the run.
        assert_eq!(resolve("try /model\nopus"), "try /model\nopus");
    }

    #[test]
    fn never_rewrites_queue_targets() {
        assert_eq!(resolve("/queue fix"), "/queue fix");
        assert_eq!(resolve("/que fix"), "/que fix");
        assert_eq!(resolve("do it /queue fix"), "do it /queue fix");
        // The TUI still resolves inline modifiers after a /queue prompt.
        assert_eq!(resolve("/queue fix /effort h"), "/queue fix /effort high");
    }

    #[test]
    fn match_ranges_mark_matched_words() {
        let anchored = &completions("/model opus")[0];
        assert_eq!(anchored.name, "anthropic claude-opus-5-5");
        assert_eq!(anchored.insert(), "/model anthropic claude-opus-5-5");
        assert_eq!(anchored.matches, [(17, 21)]);

        let rooted = &completions("/mod opus")[0];
        assert_eq!(rooted.name, "/model anthropic claude-opus-5-5");
        assert_eq!(rooted.matches, [(1, 4), (24, 28)]);

        let inline = &inline_completions("/model son")[0];
        assert_eq!(inline.name, "anthropic claude-sonnet-5-5");
        assert_eq!(inline.matches, [(17, 20)]);

        let listed = completions("/model ");
        assert_eq!(
            listed
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["openai", "anthropic"]
        );
        assert_eq!(listed[1].insert(), "/model anthropic");
        assert!(listed.iter().all(|item| item.matches.is_empty()));
    }

    #[test]
    fn roots_commands_then_macros() {
        let all = completions("/");
        assert_eq!(all[0].name, "/help");
        assert!(all.iter().any(|item| item.name == "/consider"));
        assert_eq!(completions("/model")[0].name, "/model");
        let inline: Vec<String> = inline_completions("/mo")
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert!(inline.contains(&"/model".to_owned()) && !inline.contains(&"/rename".to_owned()));
        assert!(inline_completions("/goa").is_empty());
        assert!(completions("/ model").is_empty());
        assert!(completions("model").is_empty());
    }

    #[test]
    fn raw_search_api() {
        let roots = items(&["/alpha", "/beta"]);
        let found = search_slash("/lph", &roots, &|_| ArgRegistry::new());
        assert_eq!(
            found
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["/alpha"]
        );
        assert_eq!(found[0].matches, [(2, 5)]);
        let prompt = |raw: &str| search_slash(raw, &roots, &|_| ArgRegistry::new());
        let sources = ShorthandSources {
            prompt: &prompt,
            inline_roots: &[],
            inline_args: &|_| ArgRegistry::new(),
        };
        assert_eq!(
            apply_shorthands("/bet", &find_shorthands("/bet", &sources)),
            "/beta"
        );
    }

    #[test]
    fn aliases_find_items_but_ranges_come_from_the_name() {
        let found = completions("/instructions convo");
        assert_eq!(found[0].name, "conversation");
        assert!(found[0].matches.is_empty());
        assert_eq!(completions("/instructions conv")[0].matches, [(0, 4)]);
        assert_eq!(resolve("/instructions g"), "/instructions global");
    }

    #[test]
    fn destructive_fragments_are_sent_as_typed() {
        for text in [
            "/instructions lear",
            "/instructions cle",
            "/goal clea",
            "/usage res",
            "/usage rese",
            "/login openai rem",
            "/account claude rem",
            "/account anthropic emo",
            "/chrono canc",
            "/worktree cle",
            "/worktree lean",
            "/tool uninst",
        ] {
            assert_eq!(resolve(text), text, "{text}");
        }
        assert_eq!(resolve("/instructions clear"), "/instructions clear");
        assert_eq!(resolve("/instr clear"), "/instructions clear");
        assert_eq!(resolve("/goal CLEAR"), "/goal clear");
        assert_eq!(resolve("/usa reset"), "/usage reset");
        assert_eq!(resolve("/acc claude remove"), "/account claude remove");
    }

    #[test]
    fn the_popup_still_offers_destructive_arguments() {
        let first = &completions("/instructions lear")[0];
        assert_eq!(first.name, "clear");
        assert!(first.exact);
        assert_eq!(completions("/worktree cle")[0].name, "clean");
        assert!(!completions("/instructions clear")[0].exact);
        assert_eq!(resolve("/instructions to"), "/instructions tools");
        assert_eq!(resolve("/goal pau"), "/goal pause");
        assert_eq!(resolve("/worktree mer"), "/worktree merge");
        assert_eq!(resolve("/chrono lis"), "/chrono list");
    }

    #[test]
    fn below_a_destructive_argument_every_word_is_typed_out() {
        let names: Vec<String> = completions("/chrono cancel ")
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert_eq!(names, ["standup-0123", "all"]);
        for text in [
            "/chrono cancel sta",
            "/chrono cancel al",
            "/chrono cancel standup-0123",
            "/chrono cancel all",
        ] {
            assert_eq!(resolve(text), text);
        }
        assert!(completions("/chrono cancel al")[0].exact);
        assert!(!completions("/chrono cancel all")[0].exact);
    }

    #[test]
    fn offsets_are_bytes_in_non_ascii_text() {
        // Shorthands after multi-byte prose.
        let text = "🐋 naïve /model opus fix";
        let found = shorthands(text);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].start..found[0].end], "/model opus");
        assert_eq!(
            resolve(text),
            "🐋 naïve /model anthropic claude-opus-5-5 fix"
        );
        assert_eq!(
            resolve("  /mod opus  "),
            "  /model anthropic claude-opus-5-5  "
        );

        // Match ranges in non-ASCII names, case-insensitively.
        let found = &completions("/theme テーマ")[0];
        assert_eq!(found.name, "日本語テーマ");
        assert_eq!(found.matches, [(9, 18)]);
        assert_eq!(&found.name[9..18], "テーマ");
        let found = &completions("/theme ÏCO")[0];
        assert_eq!(found.name, "Ünïcode");
        assert_eq!(&found.name[found.matches[0].0..found.matches[0].1], "ïco");
        assert_eq!(resolve("/theme ÜNÏ"), "/theme Ünïcode");
        let rooted = &completions("/them 本語")[0];
        assert_eq!(rooted.name, "/theme 日本語テーマ");
        assert_eq!(
            &rooted.name[rooted.matches[1].0..rooted.matches[1].1],
            "本語"
        );
        // Lowercasing "İ" changes its length, so no ranges rather than wrong ones.
        let found = &completions("/theme stan")[0];
        assert_eq!(found.name, "İstanbul");
        assert!(found.matches.is_empty());
        // Words split on Unicode spaces too.
        assert_eq!(
            resolve("/model\u{3000}opus"),
            "/model\u{3000}anthropic claude-opus-5-5"
        );
    }
}
