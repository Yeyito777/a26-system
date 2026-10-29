//! `/queue [target]` targets (PAR-227, PAR-262), a port of the TUI's
//! `tui/src/queuetargets.ts` (and the iPhone's `web/src/commands/queuetargets.ts`):
//! a delay (`5m`, `2h 30m`, `9d`), a conversation (newest first, by title or
//! id) or a 📁 folder (by path or name); absent means global idle.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::Item;
use super::search::is_space;
use crate::exocortex::proto::{Folder, Summary};

/// The list the targets come from.
pub struct Source<'a> {
    pub conversations: &'a HashMap<String, Summary>,
    pub folders: &'a HashMap<String, Folder>,
}

/// `14:05`-style durations: `parseDurationMs` (`500ms`, `30s`, `5m`, `2h`,
/// `9d`, combined as `8h3m1s`); None when invalid or zero.
pub fn parse_duration_ms(text: &str) -> Option<u64> {
    let duration = text.trim_matches(is_space);
    if duration.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut rest = duration;
    while !rest.is_empty() {
        let (amount, unit, after) = duration_part(rest)?;
        total += amount * unit;
        if !total.is_finite() {
            return None;
        }
        rest = after;
    }
    let result = total.round();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |now| now.as_millis() as f64);
    (result > 0.0 && result <= MAX_SAFE_INTEGER && result <= MAX_DATE_TIMESTAMP_MS - now)
        .then_some(result as u64)
}

/// The picker's choices after `/queue `: conversations newest first, then
/// folders (`📁 path`), with their ids and titles as aliases.
pub fn items(source: &Source) -> Vec<Item> {
    candidates(source)
        .into_iter()
        .map(|candidate| Item {
            name: candidate.completion_name,
            desc: candidate.desc,
            aliases: candidate.aliases,
            ..Item::default()
        })
        .collect()
}

/// The target spelled after `/queue` (which ends at byte `command_end`):
/// the `waitTarget` JSON (`{"type":"delay","delayMs":…,"label":…}`,
/// `{"type":"conversation","convId":…,"label":…}`,
/// `{"type":"folder","folderId":…,"label":…}`) and where it ends; None when
/// nothing follows that names one (global idle).
pub fn match_target(source: &Source, text: &str, command_end: usize) -> Option<(Value, usize)> {
    let after = text.get(command_end..)?;
    if !after.starts_with(is_space) {
        return None;
    }
    let arg = after.trim_start_matches(is_space);
    if arg.is_empty() {
        return None;
    }
    let arg_start = text.len() - arg.len();

    // A duration wins over a conversation whose title happens to look like one.
    if let Some((target, end)) = match_delay(arg) {
        return Some((target, arg_start + end));
    }

    // Conversations come first, newest to oldest: duplicate titles are common,
    // so the freshest one wins unless the folder icon is typed.
    let mut best: Option<(usize, Value)> = None;
    for candidate in candidates(source) {
        let mut forms = unique(
            [candidate.completion_name.as_str(), candidate.label.as_str()]
                .into_iter()
                .chain(candidate.aliases.iter().map(String::as_str)),
        );
        forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        let length = forms.iter().find_map(|form| {
            let length = prefix_len_ignoring_case(arg, form)?;
            arg[length..]
                .chars()
                .next()
                .is_none_or(is_space)
                .then_some(length)
        });
        if let Some(length) = length {
            if best.as_ref().is_none_or(|(best, _)| length > *best) {
                best = Some((length, candidate.wait_target));
            }
        }
    }
    best.map(|(length, target)| (target, arg_start + length))
}

/// A folder's path from the top ("Work/Clients").
pub fn folder_path(folders: &HashMap<String, Folder>, id: Option<&str>) -> String {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    let mut folder = id
        .filter(|id| !id.is_empty())
        .and_then(|id| folders.get(id));
    while let Some(current) = folder {
        if !seen.insert(current.id.as_str()) {
            break;
        }
        names.push(current.name.as_str());
        folder = current
            .parent_id
            .as_deref()
            .and_then(|parent| folders.get(parent));
    }
    names.reverse();
    names.join("/")
}

const FOLDER_ICON: &str = "📁";
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const MAX_DATE_TIMESTAMP_MS: f64 = 8_640_000_000_000_000.0;

/// One `\s*(\d+(?:\.\d+)?)\s*(ms|s|m|h|d)` part: its amount, unit in ms and the rest.
fn duration_part(text: &str) -> Option<(f64, f64, &str)> {
    let text = text.trim_start_matches(is_space);
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let mut end = digits(text);
    if end == 0 {
        return None;
    }
    if text[end..].starts_with('.') {
        let fraction = digits(&text[end + 1..]);
        if fraction > 0 {
            end += 1 + fraction;
        }
    }
    let amount: f64 = text[..end].parse().ok()?;
    let rest = text[end..].trim_start_matches(is_space);
    let unit_of = |len: usize| rest.get(..len).map(str::to_ascii_lowercase);
    let (unit, len) = match (unit_of(2).as_deref(), unit_of(1).as_deref()) {
        (Some("ms"), _) => (1.0, 2),
        (_, Some("s")) => (1_000.0, 1),
        (_, Some("m")) => (60_000.0, 1),
        (_, Some("h")) => (3_600_000.0, 1),
        (_, Some("d")) => (86_400_000.0, 1),
        _ => return None,
    };
    Some((amount, unit, &rest[len..]))
}

/// Match `8h3m1s`, or space-separated parts such as `2h 30m`, as a send delay.
/// Each word must be a complete duration so ordinary message text never joins it.
fn match_delay(text: &str) -> Option<(Value, usize)> {
    let mut delay: u64 = 0;
    let mut end = 0;
    let mut words = Vec::new();
    loop {
        let start = end + text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len();
        let word_len = text[start..].find(is_space).unwrap_or(text.len() - start);
        if word_len == 0 {
            break;
        }
        let word = &text[start..start + word_len];
        let Some(ms) = parse_duration_ms(word) else {
            break;
        };
        delay = delay.saturating_add(ms);
        end = start + word_len;
        words.push(word);
    }
    if words.is_empty() || delay as f64 > MAX_SAFE_INTEGER {
        return None;
    }
    Some((
        json!({ "type": "delay", "delayMs": delay, "label": words.join(" ") }),
        end,
    ))
}

/// The byte length of `text`'s prefix that equals `form` ignoring case.
fn prefix_len_ignoring_case(text: &str, form: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for expected in form.chars() {
        let (_, c) = chars.next()?;
        if c != expected && !c.to_lowercase().eq(expected.to_lowercase()) {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(at, _)| at))
}

struct Candidate {
    completion_name: String,
    label: String,
    aliases: Vec<String>,
    desc: String,
    wait_target: Value,
}

/// Trimmed, non-empty, case-insensitively unique.
fn unique<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim_matches(is_space))
        .filter(|value| !value.is_empty() && seen.insert(value.to_lowercase()))
        .map(str::to_owned)
        .collect()
}

/// Roughly `localeCompare`: case-insensitive first.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

fn non_empty(value: &str, fallback: &str) -> String {
    let trimmed = value.trim_matches(is_space);
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
    .to_owned()
}

fn descendant_count(source: &Source, folder_id: &str) -> usize {
    let mut ids = HashSet::from([folder_id]);
    let mut changed = true;
    while changed {
        changed = false;
        for folder in source.folders.values() {
            if folder
                .parent_id
                .as_deref()
                .is_some_and(|parent| ids.contains(parent))
                && ids.insert(folder.id.as_str())
            {
                changed = true;
            }
        }
    }
    source
        .conversations
        .values()
        .filter(|conv| conv.folder_id.as_deref().is_some_and(|id| ids.contains(id)))
        .count()
}

fn candidates(source: &Source) -> Vec<Candidate> {
    let mut conversations: Vec<&Summary> = source.conversations.values().collect();
    conversations.sort_by(|a, b| {
        b.updated_at
            .total_cmp(&a.updated_at)
            .then_with(|| b.created_at.total_cmp(&a.created_at))
            .then_with(|| locale_cmp(&a.id, &b.id))
    });
    let mut folders: Vec<&Folder> = source.folders.values().collect();
    folders.sort_by(|a, b| locale_cmp(&a.name, &b.name).then_with(|| locale_cmp(&a.id, &b.id)));

    let conversations = conversations.into_iter().map(|conv| {
        let title = non_empty(&conv.title, "Untitled");
        let location = folder_path(source.folders, conv.folder_id.as_deref());
        Candidate {
            completion_name: title.clone(),
            aliases: unique([title.as_str(), conv.id.as_str()]),
            desc: format!(
                "in {}",
                if location.is_empty() {
                    "top-level"
                } else {
                    &location
                }
            ),
            wait_target: json!({ "type": "conversation", "convId": conv.id, "label": title }),
            label: title,
        }
    });
    let folders = folders.into_iter().map(|folder| {
        let label = non_empty(
            &folder_path(source.folders, Some(&folder.id)),
            &non_empty(&folder.name, "Folder"),
        );
        let count = descendant_count(source, &folder.id);
        Candidate {
            completion_name: format!("{FOLDER_ICON} {label}"),
            aliases: unique([label.as_str(), folder.name.as_str(), folder.id.as_str()]),
            desc: format!("{count} conversation{}", if count == 1 { "" } else { "s" }),
            wait_target: json!({ "type": "folder", "folderId": folder.id, "label": label }),
            label,
        }
    });
    conversations.chain(folders).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str, title: &str, updated_at: f64, folder: Option<&str>) -> (String, Summary) {
        let summary = Summary {
            id: id.to_owned(),
            title: title.to_owned(),
            updated_at,
            created_at: 1.0,
            folder_id: folder.map(str::to_owned),
            ..Summary::default()
        };
        (id.to_owned(), summary)
    }

    fn folder(id: &str, name: &str, parent: Option<&str>) -> (String, Folder) {
        let folder = Folder {
            id: id.to_owned(),
            name: name.to_owned(),
            parent_id: parent.map(str::to_owned),
            ..Folder::default()
        };
        (id.to_owned(), folder)
    }

    struct Fixture {
        conversations: HashMap<String, Summary>,
        folders: HashMap<String, Folder>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                conversations: HashMap::from([
                    conv("c9", "Deploy notes", 5.0, None),
                    conv("c-fix", "Fix", 3.0, Some("f2")),
                    conv("c-login", "Fix login bug", 4.0, Some("f1")),
                    conv("c-old", "Notes", 1.0, None),
                    conv("c-new", "Notes", 9.0, Some("f2")),
                    conv("c-cafe", "Café ☕ plans", 2.0, None),
                    conv("c-5m", "5m", 0.5, None),
                    conv("c-blank", "  ", 0.25, None),
                ]),
                folders: HashMap::from([
                    folder("f1", "work", None),
                    folder("f2", "infra", Some("f1")),
                    folder("f3", "Ärger", None),
                ]),
            }
        }

        fn source(&self) -> Source<'_> {
            Source {
                conversations: &self.conversations,
                folders: &self.folders,
            }
        }

        /// The target after a leading `/queue`, and the text after it.
        fn target<'t>(&self, text: &'t str) -> Option<(Value, &'t str)> {
            let end = text.find("/queue").unwrap() + "/queue".len();
            match_target(&self.source(), text, end).map(|(target, end)| (target, &text[end..]))
        }
    }

    #[test]
    fn durations() {
        assert_eq!(
            parse_duration_ms("8h3m1s"),
            Some(8 * 3_600_000 + 3 * 60_000 + 1_000)
        );
        assert_eq!(parse_duration_ms("500ms"), Some(500));
        assert_eq!(parse_duration_ms("30s"), Some(30_000));
        assert_eq!(parse_duration_ms("5m"), Some(300_000));
        assert_eq!(parse_duration_ms("5M"), Some(300_000));
        assert_eq!(parse_duration_ms("2h"), Some(7_200_000));
        assert_eq!(parse_duration_ms("9d"), Some(9 * 86_400_000));
        assert_eq!(parse_duration_ms("1.5h"), Some(5_400_000));
        assert_eq!(parse_duration_ms(" 2h 30m "), Some(9_000_000));
        assert_eq!(parse_duration_ms("1MS"), Some(1));
        for invalid in [
            "5",
            "soon",
            "",
            "0s",
            "1.h",
            "5m later",
            "h",
            "-5m",
            "1e3s",
            "99999999999999999999d",
        ] {
            assert_eq!(parse_duration_ms(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn delays() {
        let fixture = Fixture::new();
        let (target, rest) = fixture.target("/queue 2h 30m ping me").unwrap();
        assert_eq!(
            target,
            json!({ "type": "delay", "delayMs": 9_000_000, "label": "2h 30m" })
        );
        assert_eq!(rest, " ping me");
        let (target, rest) = fixture.target("/queue\t5m").unwrap();
        assert_eq!(target["delayMs"], 300_000);
        assert_eq!(rest, "");
        // Each word is a whole duration; a duration beats a conversation titled like one.
        let (target, rest) = fixture.target("/queue 5m 2x").unwrap();
        assert_eq!(target["type"], "delay");
        assert_eq!(rest, " 2x");
        // The delay stops at a line break.
        let (target, rest) = fixture.target("/queue 1h\n5m").unwrap();
        assert_eq!(target["label"], "1h");
        assert_eq!(rest, "\n5m");
    }

    #[test]
    fn conversations_by_title_or_id() {
        let fixture = Fixture::new();
        let (target, rest) = fixture
            .target("/queue deploy notes then summarise")
            .unwrap();
        assert_eq!(
            target,
            json!({ "type": "conversation", "convId": "c9", "label": "Deploy notes" })
        );
        assert_eq!(rest, " then summarise");
        let (target, rest) = fixture.target("/queue C9 hi").unwrap();
        assert_eq!(target["convId"], "c9");
        assert_eq!(rest, " hi");
        // Duplicate titles: the newest conversation.
        assert_eq!(fixture.target("/queue notes").unwrap().0["convId"], "c-new");
        // Non-ASCII titles, case-insensitively, with byte offsets.
        let text = "🐋 /queue CAFÉ ☕ PLANS then";
        let (target, rest) = fixture.target(text).unwrap();
        assert_eq!(target["convId"], "c-cafe");
        assert_eq!(rest, " then");
    }

    #[test]
    fn longest_match_wins() {
        let fixture = Fixture::new();
        let (target, rest) = fixture.target("/queue fix login bug now").unwrap();
        assert_eq!(target["convId"], "c-login");
        assert_eq!(rest, " now");
        let (target, rest) = fixture.target("/queue fix login now").unwrap();
        assert_eq!(target["convId"], "c-fix");
        assert_eq!(rest, " login now");
        // A match must end at a word boundary.
        assert_eq!(fixture.target("/queue fixer"), None);
    }

    #[test]
    fn folders_by_path_or_name() {
        let fixture = Fixture::new();
        let (target, rest) = fixture.target("/queue 📁 work/infra deploy").unwrap();
        assert_eq!(
            target,
            json!({ "type": "folder", "folderId": "f2", "label": "work/infra" })
        );
        assert_eq!(rest, " deploy");
        assert_eq!(
            fixture.target("/queue WORK/infra x").unwrap().0["folderId"],
            "f2"
        );
        assert_eq!(
            fixture.target("/queue infra x").unwrap().0["folderId"],
            "f2"
        );
        assert_eq!(
            fixture.target("/queue 📁 work").unwrap().0["folderId"],
            "f1"
        );
        assert_eq!(fixture.target("/queue ärger").unwrap().0["folderId"], "f3");
    }

    #[test]
    fn nothing_means_global_idle() {
        let fixture = Fixture::new();
        for text in [
            "/queue",
            "/queue   ",
            "/queue hello world",
            "later /queue",
            "/queuex 5m",
        ] {
            assert_eq!(fixture.target(text), None, "{text}");
        }
        let empty = HashMap::new();
        let no_folders = HashMap::new();
        let source = Source {
            conversations: &empty,
            folders: &no_folders,
        };
        assert_eq!(match_target(&source, "/queue deploy", 6), None);
        assert_eq!(match_target(&source, "/queue", 60), None);
    }

    #[test]
    fn picker_items() {
        let fixture = Fixture::new();
        let items = items(&fixture.source());
        let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Notes",
                "Deploy notes",
                "Fix login bug",
                "Fix",
                "Café ☕ plans",
                "Notes",
                "5m",
                "Untitled",
                "📁 work/infra",
                "📁 work",
                "📁 Ärger"
            ]
        );
        let infra = &items[8];
        assert_eq!(infra.desc, "2 conversations");
        assert_eq!(infra.aliases, ["work/infra", "infra", "f2"]);
        assert_eq!(items[9].desc, "3 conversations");
        assert_eq!(items[10].desc, "0 conversations");
        assert_eq!(items[0].desc, "in work/infra");
        assert_eq!(items[1].desc, "in top-level");
        assert_eq!(items[1].aliases, ["Deploy notes", "c9"]);
    }

    #[test]
    fn folder_paths() {
        let fixture = Fixture::new();
        assert_eq!(folder_path(&fixture.folders, Some("f2")), "work/infra");
        assert_eq!(folder_path(&fixture.folders, Some("missing")), "");
        assert_eq!(folder_path(&fixture.folders, None), "");
        let cycle = HashMap::from([folder("a", "A", Some("b")), folder("b", "B", Some("a"))]);
        assert_eq!(folder_path(&cycle, Some("a")), "B/A");
    }
}
