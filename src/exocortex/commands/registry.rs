//! The phone's slash commands (PAR-211–233; the iPhone's
//! `web/src/commands/builtin/*` and `ui/ops/commands.tsx`): what each is
//! called, says, takes and where it ranks in the picker. Running them is
//! `run.rs`; their dynamic argument lists (models, efforts, targets) are
//! `env.rs`.

use super::Item;

/// When a command's text is a prompt modifier rather than the command
/// ("/model p m and then a question", "/fast hello").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    /// Always the command.
    Never,
    /// A modifier when it has more than this many words.
    Beyond(usize),
    /// `/fast` and `/ultrafast`: the command only alone or with on/off.
    Speed,
    /// `/queue`: the command only alone.
    Bare,
}

#[derive(Debug, Clone, Copy)]
pub struct Command {
    /// Without the slash.
    pub name: &'static str,
    pub description: &'static str,
    /// "/rename [title]" (the hint footer, /help).
    pub usage: Option<&'static str>,
    /// What each argument position completes ("provider", "model").
    pub arg_names: &'static [&'static str],
    /// First-level choices, name and description.
    pub choices: &'static [(&'static str, &'static str)],
    pub needs_conversation: bool,
    /// Lower first in the picker.
    pub rank: u32,
    pub modifier: Modifier,
}

impl Command {
    const fn new(name: &'static str, description: &'static str, rank: u32) -> Self {
        Self {
            name,
            description,
            usage: None,
            arg_names: &[],
            choices: &[],
            needs_conversation: false,
            rank,
            modifier: Modifier::Never,
        }
    }

    const fn usage(mut self, usage: &'static str) -> Self {
        self.usage = Some(usage);
        self
    }

    const fn args(mut self, names: &'static [&'static str]) -> Self {
        self.arg_names = names;
        self
    }

    const fn choices(mut self, choices: &'static [(&'static str, &'static str)]) -> Self {
        self.choices = choices;
        self
    }

    const fn conversation(mut self) -> Self {
        self.needs_conversation = true;
        self
    }

    const fn modifier(mut self, modifier: Modifier) -> Self {
        self.modifier = modifier;
        self
    }

    /// "/name".
    pub fn slash(&self) -> String {
        format!("/{}", self.name)
    }

    /// Whether `text` uses this command's name as a prompt modifier.
    pub fn is_modifier(&self, text: &str) -> bool {
        let words: Vec<&str> = text.split_whitespace().collect();
        match self.modifier {
            Modifier::Never => false,
            Modifier::Beyond(n) => words.len() > n,
            Modifier::Speed => {
                !(words.len() == 1
                    || (words.len() == 2
                        && matches!(words[1].to_lowercase().as_str(), "on" | "off")))
            }
            Modifier::Bare => text.trim() != format!("/{}", self.name),
        }
    }

    /// The name of the argument completed after `typed` words, if it has one
    /// (`argNames`, else a `<name>`/`[name]` token of the usage).
    pub fn argument_name(&self, typed: &[&str]) -> Option<&'static str> {
        if let Some(name) = self.arg_names.get(typed.len()) {
            return Some(name);
        }
        if !self.arg_names.is_empty() {
            return None;
        }
        let usage = self.usage?;
        let first = usage.split(" · ").next()?;
        let token = first.split_whitespace().nth(typed.len() + 1)?;
        let inner = token
            .strip_prefix('<')
            .and_then(|t| t.strip_suffix('>'))
            .or_else(|| token.strip_prefix('[').and_then(|t| t.strip_suffix(']')))?;
        inner
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            .then_some(inner)
    }

    /// Its static first-level choices as items.
    pub fn choice_items(&self) -> Vec<Item> {
        self.choices
            .iter()
            .map(|(name, desc)| Item::new(*name, *desc))
            .collect()
    }
}

const SPEED: &[(&str, &str)] = &[
    ("on", "Enable this speed tier for this conversation"),
    ("off", "Return to standard speed"),
];

/// Every command, in registration order (the picker sorts by rank).
pub const COMMANDS: &[Command] = &[
    Command::new("help", "Commands and macros", 1),
    Command::new("model", "Set or show the current provider/model", 10)
        .usage("/model <provider> <model>")
        .args(&["provider", "model"])
        .modifier(Modifier::Beyond(3)),
    Command::new(
        "default-model",
        "Set or show defaults for new conversations",
        11,
    )
    .usage("/default-model [current|reset|<provider> <model> <effort> <fast|ultrafast|off|na>]")
    .args(&["defaults", "model", "effort", "speed"]),
    Command::new("effort", "Set or show reasoning effort level", 12)
        .usage("/effort <level>")
        .modifier(Modifier::Beyond(2)),
    Command::new("fast", "Toggle or set fast mode", 13)
        .usage("/fast [on|off]")
        .args(&["fast mode"])
        .choices(SPEED)
        .modifier(Modifier::Speed),
    Command::new("ultrafast", "Toggle or set ultrafast mode", 14)
        .usage("/ultrafast [on|off]")
        .args(&["ultrafast mode"])
        .choices(SPEED)
        .modifier(Modifier::Speed),
    Command::new("theme", "Show or change phone theme", 15)
        .usage("/theme whale|cerberus|tonikawa")
        .args(&["theme"])
        .choices(&[
            ("whale", "Whale theme"),
            ("cerberus", "Cerberus theme"),
            ("tonikawa", "Tonikawa theme"),
        ]),
    Command::new("new", "New chat; created on first message", 20),
    Command::new("stop", "Stop generation", 21).conversation(),
    Command::new("rename", "Rename chat; no title generates one", 30)
        .usage("/rename [title]")
        .conversation(),
    Command::new(
        "replay",
        "Replay now, or combine with /queue [target] to defer",
        31,
    )
    .usage("/replay")
    .conversation(),
    Command::new(
        "compact",
        "Compact now, or combine with /queue [target] to defer",
        32,
    )
    .usage("/compact")
    .conversation(),
    Command::new("trim", "Trim messages, thinking, or toolresults", 33)
        .usage("/trim messages|thinking|toolresults <count>")
        .args(&["mode", "count"])
        .choices(&[
            ("messages", "Trim oldest history entries first"),
            ("thinking", "Strip oldest assistant thinking blocks first"),
            ("toolresults", "Strip oldest tool result payloads first"),
        ])
        .conversation(),
    Command::new("instructions", "Show, set, or clear instructions", 34)
        .usage("/instructions [text|clear]")
        .args(&["instructions"])
        .choices(&[("clear", "Clear instructions")]),
    Command::new("system", "Show the effective system prompt", 35),
    Command::new(
        "goal",
        "Show, set, pause, resume, complete or clear a goal",
        36,
    )
    .usage("/goal [objective|max-time <duration> <objective>|pause|resume|complete|clear]")
    .args(&["action"])
    .choices(&[
        ("pause", "stop goal work until explicitly resumed"),
        ("resume", "resume a paused or blocked goal"),
        ("complete", "mark complete and retain the result"),
        ("clear", "remove the goal"),
        (
            "max-time",
            "optional active-time limit, e.g. 9h2m1s, 8h3m, 5m, 2d",
        ),
    ]),
    Command::new(
        "chrono",
        "Schedule a message or shell command, once or repeating",
        37,
    )
    .usage("/chrono <when> <message|!command> · /chrono list · /chrono cancel <id|all>")
    .conversation(),
    Command::new(
        "queue",
        "Queue a message or /replay, /compact until idle or after a delay",
        38,
    )
    .usage("/queue [conversation|📁 folder|duration] <message>")
    .args(&["target"])
    .modifier(Modifier::Bare),
    Command::new("reload", "Reload the conversation from the daemon", 40).conversation(),
    Command::new("usage", "Show remaining provider usage", 50).usage("/usage"),
    Command::new("ping", "Daemon round-trip time", 51).usage("/ping"),
    Command::new("time", "Show the timestamp of a recent chat message", 52)
        .usage("/time [n]")
        .conversation(),
    Command::new("convo", "Copy conversation info", 53).conversation(),
    Command::new(
        "accounts",
        "Every Claude and OpenAI account: sign-in, usage, conversations",
        54,
    ),
    Command::new("btw", "Ask a read-only aside", 60)
        .usage("/btw <query> · /btw close")
        .conversation(),
    Command::new("tokens", "Token statistics", 61),
];

/// Commands `/queue` may defer: the daemon runs them once the target is idle.
pub const QUEUEABLE: &[&str] = &["/replay", "/compact"];

/// TUI commands that do not apply on the phone, refused with a pointer.
pub fn not_on_phone(name: &str) -> Option<&'static str> {
    Some(match name {
        "/quit" => "The phone app has no /quit.",
        "/exit" => "The phone app has no /exit.",
        "/ssh" => "/ssh routes a terminal TUI; the phone always talks to its own daemon.",
        "/login" => "/login isn't supported on phone yet. Sign in from the TUI.",
        "/logout" => "/logout isn't supported on phone yet. Use the TUI.",
        "/account" => "/account isn't supported on phone yet. Use the TUI.",
        "/folders" => "Folder view settings live in the list on phone.",
        _ => return None,
    })
}

/// The command named `name` ("/model" or "model").
pub fn get(name: &str) -> Option<&'static Command> {
    let name = name.strip_prefix('/').unwrap_or(name);
    COMMANDS.iter().find(|c| c.name == name)
}

/// Commands in picker order (by rank, then registration).
pub fn ranked() -> Vec<&'static Command> {
    let mut commands: Vec<&Command> = COMMANDS.iter().collect();
    commands.sort_by_key(|c| c.rank);
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_and_argument_names() {
        let model = get("/model").unwrap();
        assert!(!model.is_modifier("/model anthropic claude-opus-5-5"));
        assert!(model.is_modifier("/model anthropic claude-opus-5-5 hello there"));
        let fast = get("fast").unwrap();
        assert!(!fast.is_modifier("/fast"));
        assert!(!fast.is_modifier("/fast ON"));
        assert!(fast.is_modifier("/fast hello"));
        assert!(get("queue").unwrap().is_modifier("/queue 5m hi"));
        assert!(!get("queue").unwrap().is_modifier(" /queue "));
        assert_eq!(model.argument_name(&[]), Some("provider"));
        assert_eq!(model.argument_name(&["anthropic"]), Some("model"));
        assert_eq!(model.argument_name(&["a", "b"]), None);
        assert_eq!(get("rename").unwrap().argument_name(&[]), Some("title"));
        assert_eq!(get("effort").unwrap().argument_name(&[]), Some("level"));
        assert!(ranked().first().is_some_and(|c| c.name == "help"));
    }
}
