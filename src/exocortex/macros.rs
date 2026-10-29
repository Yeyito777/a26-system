//! Macros (WP-D; PAR-207, PAR-208): the TUI's text shortcuts (`/consider`,
//! `/commit`, `/go`, …), expanded here on the phone before a message is sent.
//! The catalog is `macros.json`, generated from Exocortex `tui/src/macros.ts`
//! by `scripts/sync-exocortex-macros.ts`; the daemon's paths come from
//! `tools_available.macroEnvironment` (without it, the TUI's `<daemon-…>`
//! placeholders and its resolve-on-daemon prefix). Expansion is one
//! right-to-left pass, longest nested prefix wins, trailing words kept;
//! URLs and paths are untouched (the iPhone's `web/src/commands/macros.ts`).

#![allow(dead_code)]

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

use super::commands::Item;
use super::commands::search::is_space;

/// `tools_available.macroEnvironment`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MacroEnv {
    pub repo_root: String,
    pub storage_dir: String,
    pub external_tools_dir: String,
    pub external_tools_trash_dir: String,
    pub installed_tool_dirs: Vec<String>,
    /// `pathStyle: "win32"`: joined paths use backslashes.
    pub windows: bool,
}

impl MacroEnv {
    /// From the event's JSON (None when it is absent or not an object).
    pub fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let text = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let dirs = object.get("installedToolDirs").and_then(Value::as_array);
        Some(Self {
            repo_root: text("repoRoot"),
            storage_dir: text("storageDir"),
            external_tools_dir: text("externalToolsDir"),
            external_tools_trash_dir: text("externalToolsTrashDir"),
            installed_tool_dirs: dirs
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            windows: object.get("pathStyle").and_then(Value::as_str) == Some("win32"),
        })
    }
}

/// The macros for a daemon's environment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    /// The picker's macros ("/consider", "Am I right or wrong?").
    pub items: Vec<Item>,
    /// Nested arguments, keyed "/name" and "/name arg …".
    pub args: HashMap<String, Vec<Item>>,
    /// What each macro (with its arguments) expands to, by its words.
    pub expansions: HashMap<String, String>,
}

/// The catalog for `env` (built once per environment and kept).
pub fn catalog(env: Option<&MacroEnv>) -> std::rc::Rc<Catalog> {
    thread_local! {
        static CACHE: RefCell<Vec<(Option<MacroEnv>, Rc<Catalog>)>> = const { RefCell::new(Vec::new()) };
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, catalog)) = cache.iter().find(|(key, _)| key.as_ref() == env) {
            return catalog.clone();
        }
        let catalog = Rc::new(build(env));
        if cache.len() >= 4 {
            cache.remove(0);
        }
        cache.push((env.cloned(), catalog.clone()));
        catalog
    })
}

/// `text` with its macros expanded (`expandMacros`).
pub fn expand(text: &str, env: Option<&MacroEnv>) -> String {
    if !text.contains('/') {
        return text.to_owned();
    }
    let catalog = catalog(env);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while let Some(found) = text[at..].find('/') {
        let start = at + found;
        // Only at a word boundary: the start of a line or after whitespace.
        let boundary = text[..start].chars().next_back().is_none_or(is_space);
        match macro_end(text, start).filter(|_| boundary) {
            Some(end) => {
                out.push_str(&text[copied..start]);
                out.push_str(&expand_words(&text[start..end], &catalog.expansions));
                copied = end;
                at = end;
            }
            None => at = start + 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

// ── The generated catalog ────────────────────────────────────────────

#[derive(Deserialize)]
struct Template {
    items: Vec<TemplateItem>,
    args: HashMap<String, Vec<TemplateItem>>,
    expansions: HashMap<String, String>,
    /// The expansions whose joined paths differ on a Windows daemon.
    #[serde(default)]
    win32: HashMap<String, String>,
}

#[derive(Deserialize)]
struct TemplateItem {
    name: String,
    desc: String,
    #[serde(default)]
    exact: bool,
}

fn template() -> &'static Template {
    static TEMPLATE: OnceLock<Template> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        serde_json::from_str(include_str!("macros.json"))
            .expect("macros.json is the generated catalog")
    })
}

const RESOLVE_PREFIX: &str = "Resolve the <daemon-...> path placeholders on the connected daemon host before acting; they are not literal paths. Locate its Exocortex checkout, configured storage, external tools and tool trash directories there, asking if necessary. Never substitute paths from the local TUI host. ";

/// `/tool uninstall`'s choices, and the template each installed tool fills.
const UNINSTALL_ARGS: &str = "/tool uninstall";
const UNINSTALL_KEY: &str = "/tool uninstall {{toolDir}}";

/// A placeholder's value: the daemon's path, or the TUI's `<daemon-…>` without one.
fn placeholder<'a>(
    key: &str,
    env: Option<&'a MacroEnv>,
    tool_dir: Option<&'a str>,
) -> Option<&'a str> {
    Some(match key {
        "repoRoot" => env.map_or("<daemon-repo>", |env| &env.repo_root),
        "storageDir" => env.map_or("<daemon-storage>", |env| &env.storage_dir),
        "externalToolsDir" => env.map_or("<daemon-external-tools>", |env| &env.external_tools_dir),
        "externalToolsTrashDir" => {
            env.map_or("<daemon-tools-trash>", |env| &env.external_tools_trash_dir)
        }
        "toolDir" => tool_dir?,
        _ => return None,
    })
}

/// `text` with its `{{repoRoot}}`-style placeholders filled.
fn fill(text: &str, env: Option<&MacroEnv>, tool_dir: Option<&str>) -> String {
    let windows = env.is_some_and(|env| env.windows);
    let separator = |c: char| c == '/' || (windows && c == '\\');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let Some(close) = rest[open..].find("}}").map(|close| open + close) else {
            break;
        };
        out.push_str(&rest[..open]);
        let tail = &rest[close + 2..];
        match placeholder(&rest[open + 2..close], env, tool_dir) {
            // A joined path ("{{repoRoot}}/scripts") keeps one separator, like path.join.
            Some(value) if tail.starts_with(separator) => {
                out.push_str(value.trim_end_matches(separator))
            }
            Some(value) => out.push_str(value),
            None => out.push_str(&rest[open..close + 2]),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn tool_short_name(dir: &str) -> &str {
    dir.strip_suffix("-cli").unwrap_or(dir)
}

/// Roughly `localeCompare`: case-insensitive first.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

fn build(env: Option<&MacroEnv>) -> Catalog {
    let template = template();
    let windows = env.is_some_and(|env| env.windows);
    let item = |t: &TemplateItem| Item {
        name: t.name.clone(),
        desc: fill(&t.desc, env, None),
        exact: t.exact,
        ..Item::default()
    };
    let text_for = |key: &str| {
        if windows {
            template.win32.get(key)
        } else {
            None
        }
        .or_else(|| template.expansions.get(key))
    };

    let mut args: HashMap<String, Vec<Item>> = template
        .args
        .iter()
        .filter(|(key, _)| key.as_str() != UNINSTALL_ARGS)
        .map(|(key, list)| (key.clone(), list.iter().map(item).collect()))
        .collect();
    let mut expansions: HashMap<String, String> = template
        .expansions
        .keys()
        .filter(|key| key.as_str() != UNINSTALL_KEY)
        .filter_map(|key| {
            let filled = fill(text_for(key)?, env, None);
            let resolve = env.is_none() && filled.contains("<daemon-");
            Some((
                key.clone(),
                if resolve {
                    format!("{RESOLVE_PREFIX}{filled}")
                } else {
                    filled
                },
            ))
        })
        .collect();

    // Uninstall choices come from the daemon's installed tools, never the phone.
    let mut dirs: Vec<&str> = env
        .into_iter()
        .flat_map(|env| env.installed_tool_dirs.iter().map(String::as_str))
        .collect();
    dirs.sort_by(|a, b| locale_cmp(tool_short_name(a), tool_short_name(b)));
    if !dirs.is_empty() {
        args.insert(
            UNINSTALL_ARGS.to_owned(),
            dirs.iter()
                .map(|dir| Item::new(tool_short_name(dir), *dir))
                .collect(),
        );
    }
    if let Some(uninstall) = text_for(UNINSTALL_KEY) {
        for dir in dirs {
            expansions.insert(
                format!("{UNINSTALL_ARGS} {}", tool_short_name(dir)),
                fill(uninstall, env, Some(dir)),
            );
        }
    }

    Catalog {
        items: template.items.iter().map(item).collect(),
        args,
        expansions,
    }
}

// ── Expansion ─────────────────────────────────────────────────────────

/// The end of `(\/[\w-]+(?:[ \t]+[\w-]+)*)` at `start`: a slash word and any
/// words following it on the line.
fn macro_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let word_end = |mut at: usize| {
        while bytes
            .get(at)
            .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            at += 1;
        }
        at
    };
    if bytes.get(start) != Some(&b'/') {
        return None;
    }
    let mut end = word_end(start + 1);
    if end == start + 1 {
        return None;
    }
    loop {
        let mut gap = end;
        while matches!(bytes.get(gap), Some(b' ' | b'\t')) {
            gap += 1;
        }
        let next = word_end(gap);
        if gap == end || next == gap {
            return Some(end);
        }
        end = next;
    }
}

/// A matched slash phrase expanded by its longest macro prefix; the
/// remaining words follow the expansion.
fn expand_words(full: &str, expansions: &HashMap<String, String>) -> String {
    let words: Vec<&str> = full
        .split([' ', '\t'])
        .filter(|word| !word.is_empty())
        .collect();
    let expansion = |key: &str| expansions.get(key).filter(|text| !text.is_empty());
    let with_rest = |expansion: &str, rest: &[&str]| {
        if rest.is_empty() {
            expansion.to_owned()
        } else {
            format!("{expansion} {}", rest.join(" "))
        }
    };
    // Older daemons have no catalog, and a freshly installed tool can precede
    // its refresh event. Preserve an explicit uninstall request without guessing
    // a directory name or silently turning it into the explanatory base macro.
    if words.len() > 2
        && words[0] == "/tool"
        && words[1] == "uninstall"
        && expansion(&words[..3].join(" ")).is_none()
    {
        let text = format!(
            "Uninstall the {} external tool on the connected daemon host. Locate its exact installed directory under that daemon's external-tools directory; verify the tool identity rather than guessing its path. Move it into that daemon's external-tools trash directory (create it if needed and use a timestamp suffix to avoid overwriting). Do not delete it outright. Check its README for extra cleanup steps and walk me through them if needed.",
            words[2]
        );
        return with_rest(&text, &words[3..]);
    }
    for len in (1..=words.len()).rev() {
        if let Some(text) = expansion(&words[..len].join(" ")) {
            return with_rest(text, &words[len..]);
        }
    }
    full.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> MacroEnv {
        MacroEnv {
            repo_root: "/srv/remote exocortex".into(),
            storage_dir: "/var/lib/remote config/storage".into(),
            external_tools_dir: "/srv/remote exocortex/external-tools".into(),
            external_tools_trash_dir: "/var/lib/remote config/data/trash/external-tools".into(),
            installed_tool_dirs: vec!["remote-only-cli".into()],
            windows: false,
        }
    }

    fn windows() -> MacroEnv {
        MacroEnv {
            repo_root: "D:\\Remote Exocortex".into(),
            storage_dir: "E:\\Remote Config\\storage".into(),
            external_tools_dir: "D:\\Remote Exocortex\\external-tools".into(),
            external_tools_trash_dir: "E:\\Remote Config\\data\\trash\\external-tools".into(),
            installed_tool_dirs: vec!["remote-only-cli".into()],
            windows: true,
        }
    }

    fn paths() -> MacroEnv {
        MacroEnv {
            repo_root: "/srv/exo".into(),
            storage_dir: "/srv/store".into(),
            external_tools_dir: "/srv/tools".into(),
            external_tools_trash_dir: "/srv/trash".into(),
            installed_tool_dirs: vec!["gmail-cli".into(), "discord-cli".into()],
            windows: false,
        }
    }

    fn arg_names(env: Option<&MacroEnv>, key: &str) -> Vec<String> {
        catalog(env)
            .args
            .get(key)
            .into_iter()
            .flatten()
            .map(|item| item.name.clone())
            .collect()
    }

    fn expand_remote(text: &str) -> String {
        expand(text, Some(&remote()))
    }

    #[test]
    fn simple_macros() {
        let env = remote();
        assert!(
            !catalog(Some(&env))
                .items
                .iter()
                .any(|item| item.name == "/xenv")
        );
        assert_eq!(expand_remote("/xenv"), "/xenv");
        assert_eq!(
            expand_remote("/todo"),
            "Make a <name>-todo.md for this with items - [ ] and finish it sequentially."
        );
        assert_eq!(arg_names(Some(&env), "/todo"), ["wait"]);
        assert_eq!(
            expand_remote("/todo wait"),
            "Make a <name>-todo.md for this with items - [ ] and tell me the abs path of it so I can review before you go and finish it sequentially."
        );
        assert_eq!(
            expand_remote("/subagents"),
            "Use subagents when parallel work would materially improve speed or quality."
        );
        assert_eq!(expand_remote("/go"), "Go ahead and implement that");
        let consider = catalog(None)
            .items
            .iter()
            .find(|item| item.name == "/consider")
            .cloned()
            .unwrap();
        assert_eq!(consider.desc, "Am I right or wrong?");
    }

    #[test]
    fn exocortex_quality_macros() {
        assert_eq!(
            arg_names(None, "/exocortex"),
            ["tui-quality", "daemon-quality"]
        );
        let tui = expand_remote("/exocortex tui-quality");
        let daemon = expand_remote("/exocortex daemon-quality");
        assert!(tui.contains("Check the code quality of exocortex's tui."));
        assert!(daemon.contains("Check the code quality of exocortex's daemon."));
        assert!(tui.contains("./scripts/dev/create-worktree <name>"));
        assert!(daemon.contains("./scripts/dev/clean-worktree <name-or-path>"));
        assert!(tui.contains("Once done, test end to end with xenv to make sure nothing broke."));
        assert!(daemon.contains("Check exo-cli -h first to see how to test in worktree."));
    }

    #[test]
    fn worktree_macros() {
        assert_eq!(arg_names(None, "/worktree"), ["setup", "merge", "clean"]);
        let exact: Vec<bool> = catalog(None).args["/worktree"]
            .iter()
            .map(|item| item.exact)
            .collect();
        assert_eq!(exact, [false, false, true]);

        let merge = expand_remote("/worktree merge");
        assert!(merge.starts_with("First merge local main into the worktree branch"));
        assert!(
            merge.contains("resolve any merge conflicts")
                && merge.contains("Merge it back into main")
        );

        let clean = expand_remote("/worktree clean hosted-web-run");
        for part in [
            "The work in this worktree is rejected.",
            "Do not merge it",
            "do not preserve the changes",
            "verify it is a linked worktree and not main",
            "./scripts/dev/clean-worktree <name-or-path>",
            "force-remove the worktree/branch",
        ] {
            assert!(clean.contains(part), "{part}");
        }
        assert!(clean.ends_with(" hosted-web-run"));

        let setup = expand_remote("/worktree setup \"project with spaces\"");
        assert!(setup.contains("If the project is not already a git repo, initialize git first."));
        for file in [
            "scripts/dev/create-worktree",
            "scripts/dev/clean-worktree",
            "scripts/dev/worktree-common.sh",
            ".gitignore",
            ".githooks/post-checkout",
            "scripts/dev/exotest",
        ] {
            assert!(
                setup.contains(&format!("`/srv/remote exocortex/{file}`")),
                "{file}"
            );
        }
        for part in [
            "Verify these files exist",
            "ask for its location rather than guessing a path",
            "host OS",
            "smoke-test create + clean",
        ] {
            assert!(setup.contains(part), "{part}");
        }
        assert!(setup.ends_with("Project: \"project with spaces\""));
    }

    #[test]
    fn saved_output_macros_use_the_storage_directory() {
        for name in ["/html", "/plan other", "/autoresearch stop"] {
            let expanded = expand_remote(name);
            assert!(
                expanded.contains("`/var/lib/remote config/storage/playground`"),
                "{name}"
            );
            assert!(
                expanded.contains("create the directory if needed"),
                "{name}"
            );
        }
        for expanded in catalog(Some(&remote())).expansions.values() {
            assert!(
                !expanded.contains("~/Workspace/")
                    && !expanded.contains("~/.config/exocortex/")
                    && !expanded.contains("~/Desktop/")
            );
            assert!(!expanded.contains("{{"), "{expanded}");
        }
    }

    #[test]
    fn autoresearch_macros() {
        let expanded = expand_remote("/autoresearch improve benchmark quality");
        for part in [
            "You're going to autoresearch.",
            "objective the user can start with /goal",
            "use Chrono for recurring monitoring",
            "autoresearch/<topic>",
            "Never force-add or commit this directory",
            "you must create the benchmark first",
            "Make sure to not use subagents.",
            "propose the goal AFTER the user has answered the five questions",
        ] {
            assert!(expanded.contains(part), "{part}");
        }
        assert!(expanded.ends_with(" improve benchmark quality"));
        assert_eq!(arg_names(None, "/autoresearch"), ["stop"]);
        let stop = expand_remote("/autoresearch stop");
        assert!(stop.contains("You're going to stop autoresearching."));
        assert!(stop.contains("Save it to a file in `/var/lib/remote config/storage/playground`"));
    }

    #[test]
    fn tool_macros() {
        assert_eq!(arg_names(None, "/tool"), ["install", "uninstall"]);
        assert!(catalog(None).args["/tool"][1].exact);
        assert_eq!(
            expand_remote("/tool"),
            "Explain to me how the external tools system works in Exocortex."
        );
        assert_eq!(
            expand_remote("/tool install"),
            "Explain to me how the installation process for a tool looks in Exocortex."
        );
        assert_eq!(
            expand_remote("/tool uninstall"),
            "Explain to me how the uninstallation process for a tool looks in Exocortex."
        );
        let install: Vec<(String, String)> = catalog(None).args["/tool install"]
            .iter()
            .map(|item| (item.name.clone(), item.desc.clone()))
            .collect();
        let expected = [
            ("discord", "Discord messages and calls"),
            ("exo", "Debug other Exocortex instances"),
            ("gmail", "Read, search, and send email"),
            ("google", "Search the web"),
            ("image", "Generate AI images"),
            ("linkedin", "LinkedIn profiles and connections"),
            ("transcribe", "Transcribe audio files"),
            ("twitter", "Read and post on X"),
            ("whatsapp", "Read and send WhatsApp messages"),
        ];
        assert_eq!(
            install,
            expected.map(|(name, desc)| (name.to_owned(), desc.to_owned()))
        );
    }

    #[test]
    fn tool_uninstall_comes_from_the_daemon() {
        let env = paths();
        let catalog = catalog(Some(&env));
        assert_eq!(
            arg_names(Some(&env), "/tool uninstall"),
            ["discord", "gmail"]
        );
        assert_eq!(catalog.args["/tool uninstall"][1].desc, "gmail-cli");
        assert!(catalog.expansions["/tool uninstall gmail"].contains("/srv/tools/gmail-cli"));
        assert!(
            !catalog
                .expansions
                .contains_key("/tool uninstall {{toolDir}}")
        );

        let expanded =
            expand_remote("Please /tool uninstall remote-only after checking the README");
        assert!(expanded.starts_with("Please Uninstall the remote-only-cli tool for yourself."));
        assert!(expanded.contains("/srv/remote exocortex/external-tools/remote-only-cli"));
        assert!(expanded.contains("/var/lib/remote config/data/trash/external-tools"));
        assert!(
            expanded.contains("timestamp suffix")
                && expanded.contains("Do not delete it outright.")
        );
        assert!(expanded.ends_with(" after checking the README"));

        // A tool the catalog does not know keeps the explicit request.
        let unknown = expand("/tool uninstall twitter now", Some(&env));
        assert!(
            unknown
                .starts_with("Uninstall the twitter external tool on the connected daemon host.")
        );
        assert!(unknown.ends_with(" now"));
    }

    #[test]
    fn daemon_host_paths() {
        for (env, sep) in [(remote(), "/"), (windows(), "\\")] {
            let join = |root: &str, rest: &str| format!("{root}{sep}{}", rest.replace('/', sep));
            assert!(
                expand("/worktree setup", Some(&env))
                    .contains(&join(&env.repo_root, "scripts/dev/create-worktree"))
            );
            assert!(expand("/commit exocortex", Some(&env)).contains(&env.repo_root));
            assert!(
                catalog(Some(&env)).args["/commit"][0]
                    .desc
                    .contains(&env.repo_root)
            );
            for name in ["/html", "/plan other", "/autoresearch stop"] {
                assert!(
                    expand(name, Some(&env)).contains(&join(&env.storage_dir, "playground")),
                    "{name}"
                );
            }
            assert!(
                expand("/tool install google", Some(&env))
                    .contains(&join(&env.external_tools_dir, "google-cli"))
            );
            let uninstall = expand("/tool uninstall remote-only", Some(&env));
            assert!(
                uninstall.contains(&join(&env.external_tools_dir, "remote-only-cli")),
                "{uninstall}"
            );
            assert!(uninstall.contains(&env.external_tools_trash_dir));
            assert_eq!(
                catalog(Some(&env)).args["/tool uninstall"],
                [Item::new("remote-only", "remote-only-cli")]
            );
        }
        // path.join keeps one separator.
        let env = MacroEnv {
            repo_root: "/srv/exo/".into(),
            ..paths()
        };
        assert!(
            expand("/worktree setup", Some(&env))
                .contains("`/srv/exo/scripts/dev/create-worktree`")
        );
        let env = MacroEnv {
            storage_dir: "/".into(),
            ..paths()
        };
        assert!(expand("/html", Some(&env)).contains("`/playground`"));
    }

    #[test]
    fn missing_environment_uses_placeholders() {
        for name in [
            "/worktree setup",
            "/commit exocortex",
            "/html",
            "/plan other",
            "/autoresearch stop",
            "/tool install google",
        ] {
            let expanded = expand(name, None);
            assert!(
                expanded.starts_with(
                    "Resolve the <daemon-...> path placeholders on the connected daemon host"
                ),
                "{name}"
            );
            assert!(!expanded.contains("{{"), "{name}");
        }
        assert!(expand("/html", None).contains("<daemon-storage>/playground"));
        assert!(
            expand("/tool install google", None).contains("<daemon-external-tools>/google-cli")
        );
        assert_eq!(
            catalog(None).args["/commit"][0].desc,
            "Commit <daemon-repo>"
        );
        assert!(!catalog(None).args.contains_key("/tool uninstall"));
        let uninstall = expand("/tool uninstall discord after checking README", None);
        assert!(
            uninstall
                .starts_with("Uninstall the discord external tool on the connected daemon host")
        );
        assert!(uninstall.contains("verify the tool identity rather than guessing"));
        assert!(uninstall.ends_with(" after checking README"));
        assert_eq!(expand("/go", None), "Go ahead and implement that");
        assert_eq!(expand("/go", None), expand_remote("/go"));
    }

    #[test]
    fn expansion_anywhere_longest_prefix_once() {
        let env = paths();
        let plan = expand("ok /plan other now", Some(&env));
        assert!(plan.starts_with("ok Draft a plan for this as a prompt for another instance."));
        assert!(plan.ends_with(" now"));
        assert_eq!(
            expand("/plan\tother", Some(&env)),
            expand("/plan other", Some(&env))
        );
        assert_eq!(
            expand("a\n/fix\nb /go", Some(&env)),
            "a\nGo ahead and fix it\nb Go ahead and implement that"
        );
        assert_eq!(
            expand("see https://x.com/go and a/go", Some(&env)),
            "see https://x.com/go and a/go"
        );
        assert_eq!(expand("/usr/bin/env", Some(&env)), "/usr/bin/env");
        assert_eq!(
            expand("/unknownmacro stays", Some(&env)),
            "/unknownmacro stays"
        );
        assert_eq!(
            expand("/go, then /fix.", Some(&env)),
            "Go ahead and implement that, then Go ahead and fix it."
        );
        assert_eq!(expand("//go / go", Some(&env)), "//go / go");
        // Trailing words are kept, with their spacing collapsed like the TUI's.
        assert_eq!(
            expand("/go  now\tplease", Some(&env)),
            "Go ahead and implement that now please"
        );
        // Expansion text containing a slash word is not expanded again.
        assert_eq!(
            expand("/todo", Some(&env)),
            "Make a <name>-todo.md for this with items - [ ] and finish it sequentially."
        );
        assert!(expand("/autoresearch", Some(&env)).contains("start with /goal."));
        // Non-ASCII around macros; JavaScript's \s includes U+00A0.
        assert_eq!(
            expand("naïve 🐋 /go ✓", Some(&env)),
            "naïve 🐋 Go ahead and implement that ✓"
        );
        assert_eq!(
            expand("日本\u{a0}/fix", Some(&env)),
            "日本\u{a0}Go ahead and fix it"
        );
        assert_eq!(expand("¿/go?", Some(&env)), "¿/go?");
        assert_eq!(expand("no macros", Some(&env)), "no macros");
    }

    #[test]
    fn environment_from_json_and_cache() {
        let value = serde_json::json!({
            "repoRoot": "/srv/exo", "storageDir": "/srv/store", "externalToolsDir": "/srv/tools",
            "externalToolsTrashDir": "/srv/trash", "pathStyle": "posix", "installedToolDirs": ["gmail-cli", "discord-cli"],
        });
        let env = MacroEnv::from_json(&value).unwrap();
        assert_eq!(env, paths());
        let mut win = value.clone();
        win["pathStyle"] = "win32".into();
        assert!(MacroEnv::from_json(&win).unwrap().windows);
        assert_eq!(MacroEnv::from_json(&Value::Null), None);
        assert_eq!(MacroEnv::from_json(&serde_json::json!("x")), None);

        assert!(Rc::ptr_eq(&catalog(Some(&env)), &catalog(Some(&paths()))));
        assert!(!Rc::ptr_eq(&catalog(Some(&env)), &catalog(None)));
        assert!(Rc::ptr_eq(&catalog(None), &catalog(None)));
    }
}
