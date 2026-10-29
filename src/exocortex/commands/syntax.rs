//! Prompt syntax (PAR-198–210; the iPhone's `web/src/commands/syntax.ts`):
//! what the caret is completing, the `//` escape, which command a prompt
//! invokes, and the picker's heading. Pure functions over byte offsets.

use super::Item;
use super::registry;

/// What a collapsed caret is completing: the slash word from `start`
/// (through the word under the caret, to `end`), typed up to the caret as
/// `query`; `leading` when it starts the prompt (commands are offered then).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub start: usize,
    pub end: usize,
    pub query: String,
    pub leading: bool,
    /// A path on the daemon's host (`commands/paths.rs`), not a slash word.
    pub path: bool,
}

/// A leading `//` sends one literal slash and skips commands, modifiers
/// and macros (PAR-203): the text without that slash.
pub fn unescape_literal(text: &str) -> Option<String> {
    let at = text.find(|c: char| !c.is_whitespace())?;
    text[at..]
        .starts_with("//")
        .then(|| format!("{}{}", &text[..at], &text[at + 1..]))
}

/// The command a prompt invokes: a leading "/name" that is not `//`, not
/// a macro and not a path (a second "/" or a "." in the name). Unknown
/// names are returned too, so they are refused rather than sent (PAR-204).
pub fn invocation<'a>(
    text: &'a str,
    is_macro: &dyn Fn(&str) -> bool,
) -> Option<(&'a str, &'a str)> {
    let t = text.trim();
    if !t.starts_with('/') || t.starts_with("//") {
        return None;
    }
    let name_end = t.find(char::is_whitespace).unwrap_or(t.len());
    let name = &t[..name_end];
    if name.len() < 2 || name[1..].contains('/') || name.contains('.') || is_macro(name) {
        return None;
    }
    Some((name, t[name_end..].trim()))
}

/// A path-like word (`~`, `~/…`, `./…`, `../…`, `/a/…`): not a slash
/// completion.
fn path_word(word: &str) -> bool {
    word == "~"
        || word.starts_with("~/")
        || word.starts_with("./")
        || word.starts_with("../")
        || (word.starts_with('/') && !word.starts_with("//") && word[1..].contains('/'))
}

/// What the caret at `caret` (no selection) is completing, if a slash word
/// (`completionContext`): the nearest slash word on the caret's line that
/// starts after whitespace; not a `//` escape, a path or a URL.
pub fn completion(text: &str, caret: usize) -> Option<Completion> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let prefix = &text[..caret];
    // The word under the caret runs on to its end.
    let mut end = caret;
    if prefix
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace())
    {
        end += text[caret..]
            .find(char::is_whitespace)
            .unwrap_or(text.len() - caret);
    }
    let word_start = prefix.rfind(char::is_whitespace).map_or(0, |at| {
        at + prefix[at..].chars().next().map_or(1, char::len_utf8)
    });
    if path_word(&prefix[word_start..]) {
        return Some(Completion {
            start: word_start,
            end,
            query: prefix[word_start..].to_owned(),
            leading: text[..word_start].trim().is_empty(),
            path: true,
        });
    }
    let line_start = prefix.rfind(['\n', '\t']).map_or(0, |at| at + 1);
    // The last "/" on the line that starts a word.
    let mut start = None;
    for (at, _) in prefix[line_start..].match_indices('/') {
        let at = line_start + at;
        let after_space = at == 0
            || prefix[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if after_space {
            start = Some(at);
        }
    }
    let start = start?;
    let query = &prefix[start..];
    let base = query.split(char::is_whitespace).next().unwrap_or("");
    if base.starts_with("//") || base[1..].contains('/') || base.contains('.') {
        return None;
    }
    Some(Completion {
        start,
        end,
        query: query.to_owned(),
        leading: text[..start].trim().is_empty(),
        path: false,
    })
}

/// The picker's heading for a completion: "Commands" or "Macros" for the
/// first word, else the argument's name ("Provider" after `/model `) and
/// the words already typed before it (shown once, not on every row).
pub fn heading(
    query: &str,
    commands: usize,
    macros: usize,
    is_macro: &dyn Fn(&str) -> bool,
) -> (String, String) {
    let parts: Vec<&str> = query.split_whitespace().collect();
    let trailing = query.ends_with(char::is_whitespace);
    if parts.len() <= 1 && !trailing {
        let title = if macros > 0 && commands == 0 {
            "Macros"
        } else {
            "Commands"
        };
        return (title.into(), String::new());
    }
    let root = parts.first().copied().unwrap_or("");
    let typed: Vec<&str> = if trailing {
        parts[1..].to_vec()
    } else {
        parts[1..parts.len() - 1].to_vec()
    };
    let name = if is_macro(root) {
        None
    } else {
        registry::get(root).and_then(|c| c.argument_name(&typed))
    };
    let title = match name {
        Some(name) => {
            let mut chars = name.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
        None => "Options".into(),
    };
    let mut prefix = vec![root];
    prefix.extend(typed);
    (title, prefix.join(" "))
}

/// Whether a suggestion belongs on the Commands tab: its root is a command,
/// not a macro.
pub fn is_command_suggestion(item: &Item, is_macro: &dyn Fn(&str) -> bool) -> bool {
    let root = item.insert().split_whitespace().next().unwrap_or("");
    !is_macro(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_macros(_: &str) -> bool {
        false
    }

    #[test]
    fn escapes_and_invocations() {
        assert_eq!(
            unescape_literal("  //etc/hosts").as_deref(),
            Some("  /etc/hosts")
        );
        assert_eq!(unescape_literal("/model"), None);
        assert_eq!(
            invocation(" /model a b ", &no_macros),
            Some(("/model", "a b"))
        );
        assert_eq!(invocation("//x", &no_macros), None);
        assert_eq!(invocation("/usr/bin is a path", &no_macros), None);
        assert_eq!(invocation("/file.txt", &no_macros), None);
        assert_eq!(
            invocation("/go now", &|n: &str| n == "/go"),
            None,
            "a macro"
        );
        assert_eq!(invocation("/nope", &no_macros), Some(("/nope", "")));
        assert_eq!(invocation("hello /model", &no_macros), None);
    }

    #[test]
    fn the_caret_completes_the_nearest_slash_word() {
        let c = completion("/mo", 3).unwrap();
        assert_eq!(
            (c.start, c.end, c.query.as_str(), c.leading),
            (0, 3, "/mo", true)
        );
        // Mid-word: the range runs through the word's end.
        let c = completion("/model anth more", 9).unwrap();
        assert_eq!((c.start, c.end, c.query.as_str()), (0, 11, "/model an"));
        // Mid-message: inline (macros and modifiers).
        let c = completion("please /co", 10).unwrap();
        assert!(!c.leading);
        assert_eq!(c.start, 7);
        // Paths, URLs, escapes and other lines are not completions.
        let c = completion("see /usr/lo", 11).unwrap();
        assert!(c.path && c.start == 4 && c.query == "/usr/lo", "a path");
        assert!(completion("~/src", 5).is_some_and(|c| c.path));
        assert_eq!(completion("http://x.org/a", 14), None);
        assert_eq!(completion("//x", 3), None);
        assert_eq!(completion("/go\nnext", 8), None);
        assert_eq!(completion("é /mo", 6).map(|c| c.start), Some(3));
    }

    #[test]
    fn headings_name_the_argument() {
        assert_eq!(
            heading("/mo", 3, 0, &no_macros),
            ("Commands".into(), String::new())
        );
        assert_eq!(
            heading("/co", 0, 2, &no_macros),
            ("Macros".into(), String::new())
        );
        assert_eq!(
            heading("/model ", 1, 0, &no_macros),
            ("Provider".into(), "/model".into())
        );
        assert_eq!(
            heading("/model anthropic cl", 1, 0, &no_macros),
            ("Model".into(), "/model anthropic".into())
        );
    }
}
