//! Submitting and running commands (the iPhone's
//! `web/src/commands/submit.ts`, `run.ts` and its built-ins): the pipeline
//! a Send or Run goes through, the commands themselves, their result
//! notices (PAR-206), and the picker's state.
//!
//! The pipeline: `//` escape → slash shorthands → a command (Run) or a
//! refusal for an unknown one → inline modifiers (all valid or none) →
//! `/replay` or `/compact` deferred with `/queue` → macro expansion → the
//! daemon's idle queue (`/queue`) or a send / queue / new chat. Errors go
//! to the draft's error label and the typed text stays (PAR-204, 211).

use serde_json::{Value, json};

use super::Item;
use super::env::SlashEnv;
use super::inline::{self, Generation};
use super::registry::{self, Command, QUEUEABLE};
use super::syntax::{self, Completion};
use crate::exocortex::macros;
use crate::exocortex::model::composer::{BUDGET, ComposerError, Timing, VoicePhase};
use crate::exocortex::model::ops::{self, model_info, provider_info, supports_speed};
use crate::exocortex::model::{Action, Effect, Model, Nav, Page, chat};
use crate::exocortex::proto::{Entry, Speed};
use crate::exocortex::requests::{self, Owner};
use crate::exocortex::ui::Haptic;

/// Notices longer than this many lines open in the reader.
const READER_LINES: usize = 12;

/// How a notice reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// The picker over the composer (PAR-198): what the caret completes, the
/// suggestions split into commands and macros, the heading, and the hint
/// footer when there are none.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerState {
    pub completion: Completion,
    pub commands: Vec<Item>,
    pub macros: Vec<Item>,
    pub title: String,
    pub prefix: String,
    /// The hint footer: usage and description, or a warning.
    pub hint: Option<(String, bool)>,
}

impl PickerState {
    /// The rows of the tab shown.
    pub fn rows(&self, macros: bool) -> &[Item] {
        if macros || self.commands.is_empty() {
            &self.macros
        } else {
            &self.commands
        }
    }
}

type Outcome = Result<Vec<Value>, String>;

impl Model {
    /// The picker's state, if it shows (PAR-198, PAR-202).
    pub fn picker(&self) -> Option<PickerState> {
        if self.page != Page::Chat || self.composer.voice.phase != VoicePhase::Idle {
            return None;
        }
        let text = &self.composer.text;
        if self.composer.picker_dismissed.as_deref() == Some(text.as_str()) {
            return None;
        }
        let cursor = self.composer.cursor();
        if cursor.selection().is_some() {
            return None;
        }
        let completion = syntax::completion(text, cursor.caret);
        let env = SlashEnv::new(self);
        let items = match &completion {
            Some(c) if c.leading => env.leading(&c.query),
            Some(c) => env.inline(&c.query),
            None => Vec::new(),
        };
        let is_macro = |name: &str| env.is_macro(name);
        let (commands, macros): (Vec<Item>, Vec<Item>) = items
            .into_iter()
            .partition(|item| syntax::is_command_suggestion(item, &is_macro));
        let hint = if commands.is_empty() && macros.is_empty() {
            self.command_hint(&env)
        } else {
            None
        };
        if commands.is_empty() && macros.is_empty() && hint.is_none() {
            return None;
        }
        let completion = completion.unwrap_or(Completion {
            start: cursor.caret,
            end: cursor.caret,
            query: String::new(),
            leading: false,
        });
        let (title, prefix) =
            syntax::heading(&completion.query, commands.len(), macros.len(), &is_macro);
        Some(PickerState {
            completion,
            commands,
            macros,
            title,
            prefix,
            hint,
        })
    }

    /// The hint footer for a command being typed (PAR-202).
    fn command_hint(&self, env: &SlashEnv) -> Option<(String, bool)> {
        if self.composer_error_label().is_some() {
            return None;
        }
        let text = &self.composer.text;
        let (name, _) = syntax::invocation(text, &|n: &str| env.is_macro(n))?;
        match registry::get(name) {
            Some(command) if command.is_modifier(text) => None,
            Some(command) => Some((
                format!(
                    "{}\n{} · Tap Run to execute.",
                    command.usage.map_or_else(|| command.slash(), str::to_owned),
                    command.description
                ),
                false,
            )),
            None if inline::INLINE_NAMES.contains(&name) => None,
            None => Some((
                "Unknown command. Use /help, or start with // to send a literal slash.".into(),
                true,
            )),
        }
    }

    /// The text is a command Run would run (the trailing button says Run).
    pub fn is_command_text(&self) -> bool {
        let text = &self.composer.text;
        let env = SlashEnv::new(self);
        match syntax::invocation(text, &|n: &str| env.is_macro(n)) {
            Some((name, _)) => match registry::get(name) {
                Some(command) => !command.is_modifier(text),
                None => !inline::INLINE_NAMES.contains(&name),
            },
            None => false,
        }
    }

    /// A suggestion chosen: it replaces the word at the caret (with a space
    /// after it), and never runs or sends (PAR-199).
    pub(crate) fn complete(&mut self, insert: &str) {
        let text = self.composer.text.clone();
        let caret = self.composer.cursor().caret;
        let Some(c) = syntax::completion(&text, caret) else {
            return;
        };
        let tail = &text[c.end..];
        let spaced = tail.starts_with([' ', '\t']);
        let insertion = if spaced {
            insert.to_owned()
        } else {
            format!("{insert} ")
        };
        let mut next = text.clone();
        next.replace_range(c.start..c.end, &insertion);
        let caret = c.start + insertion.len() + usize::from(spaced);
        self.composer.text = next;
        self.composer.cursor = crate::exocortex::editor::Cursor::at(caret);
        self.composer.picker_dismissed = None;
        self.ui.haptic(Haptic::Selection);
        self.composer_changed();
        // A finished command or macro needs no exact-match menu.
        if self
            .picker()
            .is_none_or(|p| p.commands.is_empty() && p.macros.is_empty())
        {
            self.composer.picker_dismissed = Some(self.composer.text.clone());
        }
    }

    /// A local notice (PAR-206): in the open transcript, long ones (or
    /// listings) in the reader; a new chat, which has no transcript, as a
    /// toast.
    pub(crate) fn notice(&mut self, text: &str, level: Level, reader: Option<&str>) {
        let long = text.lines().count() > READER_LINES;
        if let Some(title) = reader.filter(|_| true).or(long.then_some("Result")) {
            self.ui.open(Box::new(crate::exocortex::reader::Reader::new(
                title.to_owned(),
                text.to_owned(),
            )));
            return;
        }
        if self.chat.id.is_some() && self.page == Page::Chat {
            self.chat.push(Entry::System {
                text: text.to_owned(),
                color: match level {
                    Level::Info => None,
                    Level::Warning => Some("warning".into()),
                    Level::Error => Some("error".into()),
                },
                metadata: None,
            });
            self.chat.touch();
        } else if level == Level::Info {
            self.ui.toast(text);
        } else {
            self.ui.error_toast(text);
        }
    }

    /// An error for the composer's label (the typed text stays).
    fn refuse(&mut self, key: &str, message: &str) {
        self.composer.errors.insert(
            key.to_owned(),
            ComposerError {
                text: message.to_owned(),
                since: std::time::Instant::now(),
            },
        );
    }

    /// Send or Run (PAR-185, PAR-199–209).
    pub(crate) fn submit_prompt(&mut self, timing: Option<Timing>) -> Vec<Value> {
        let original = self.composer.text.clone();
        let key = self.shown_draft_key();
        if original.trim().is_empty() || !self.ready || self.composer.submitting(&key) {
            return Vec::new();
        }
        if original.len() > BUDGET {
            self.refuse(
                &key,
                "Message exceeds the 12 MB attachment budget. Remove an item before sending.",
            );
            return Vec::new();
        }
        self.composer.errors.remove(&key);
        // `//`: one literal slash, nothing else.
        if let Some(literal) = syntax::unescape_literal(&original) {
            let key = self.take_draft();
            return self.deliver(key, literal.trim().to_owned(), original, timing, None);
        }
        let env = SlashEnv::new(self);
        let resolved = env.resolve_shorthands(original.trim());
        let is_macro = |n: &str| env.is_macro(n);
        if let Some((name, args)) = syntax::invocation(&resolved, &is_macro) {
            let command = registry::get(name);
            let deferred =
                QUEUEABLE.contains(&name) && resolved.split_whitespace().any(|w| w == "/queue");
            if let Some(command) = command
                && !deferred
                && !command.is_modifier(&resolved)
            {
                let args = args.to_owned();
                drop(env);
                return self.run(command, &args, &original);
            }
            if command.is_none() && !inline::INLINE_NAMES.contains(&name) {
                let message = registry::not_on_phone(name).map_or_else(
                    || format!("{name} isn't supported on phone yet. Use /help for supported commands, or start with // to send a literal slash."),
                    str::to_owned,
                );
                drop(env);
                self.refuse(&key, &message);
                return Vec::new();
            }
        }
        // Inline modifiers: all valid, or nothing changes.
        let parsed = inline::parse(&resolved, &env.inline_env());
        let macro_env = self.composer.macro_env.clone();
        drop(env);
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                self.refuse(&key, &error);
                return Vec::new();
            }
        };
        let primary = parsed.text.trim().to_owned();
        // "/replay /queue 5m": the command deferred to the daemon's queue.
        if let Some(wait) = &parsed.queue
            && QUEUEABLE.contains(&primary.as_str())
        {
            if self.chat.id.is_none() {
                self.refuse(&key, &format!("{primary} needs an existing conversation."));
                return Vec::new();
            }
            let key = self.take_draft();
            let mut commands =
                self.deliver(key, primary.clone(), original, None, Some(wait.clone()));
            if let Some(command) = commands.first_mut() {
                command["command"] = json!({ "name": primary });
                command["text"] = Value::from("");
            }
            return commands;
        }
        let mut commands = Vec::new();
        if let Some(selection) = &parsed.selection {
            commands.extend(self.apply_selection(selection, &parsed));
            for notice in parsed.notices.clone() {
                self.notice(&notice, Level::Info, None);
            }
        }
        let body = parsed.text.trim().to_owned();
        if body.is_empty() {
            let key = self.take_draft();
            let _ = key;
            if parsed.queue.is_some() {
                self.notice("Nothing to queue.", Level::Warning, None);
            }
            return commands;
        }
        let body = macros::expand(&body, macro_env.as_ref());
        let key = self.take_draft();
        commands.extend(self.deliver(key, body, original, timing, parsed.queue.clone()));
        commands
    }

    /// The selection the modifiers chose, applied: a conversation's on the
    /// daemon (model, then effort, then speed), a draft's locally.
    fn apply_selection(&mut self, s: &Generation, parsed: &inline::Inline) -> Vec<Value> {
        let mut commands = Vec::new();
        if parsed.model_changed {
            commands.extend(self.handle(Action::Ops(ops::Act::PickModel(
                s.provider.clone(),
                s.model.clone(),
            ))));
        }
        if parsed.model_changed || !parsed.efforts.is_empty() {
            commands.extend(self.handle(Action::Ops(ops::Act::PickEffort(s.effort.clone()))));
        }
        if parsed.model_changed || !parsed.speeds.is_empty() {
            commands.extend(self.handle(Action::Ops(ops::Act::PickSpeed(s.speed))));
        }
        commands
    }

    /// Runs `command`: what is typed leaves the composer, and comes back
    /// with the error if the command refuses.
    fn run(&mut self, command: &Command, args: &str, original: &str) -> Vec<Value> {
        let key = self.take_draft();
        let name = command.slash();
        let outcome = if command.needs_conversation && self.chat.id.is_none() {
            Err(format!("{name} needs an existing conversation."))
        } else {
            self.run_command(command, args)
        };
        match outcome {
            Ok(commands) => commands,
            Err(message) => {
                self.restore_typed(&key, original);
                self.refuse(&key, &message);
                Vec::new()
            }
        }
    }

    fn require_idle(&self, name: &str) -> Result<(), String> {
        if self.chat.id.is_some() && self.streaming() {
            return Err(format!("Stop generation before using {name}."));
        }
        Ok(())
    }

    fn generation_now(&self) -> Generation {
        let r = self.resolve();
        Generation {
            provider: r.provider,
            model: r.model,
            effort: r.effort,
            speed: r.speed,
        }
    }

    fn run_command(&mut self, command: &Command, args: &str) -> Outcome {
        let name = command.slash();
        let argv: Vec<String> = args.split_whitespace().map(str::to_owned).collect();
        let no_args = |argv: &[String]| -> Result<(), String> {
            if argv.is_empty() {
                Ok(())
            } else {
                Err(format!("Usage: {name}"))
            }
        };
        match command.name {
            "help" => {
                no_args(&argv)?;
                self.help();
                Ok(Vec::new())
            }
            "new" => {
                no_args(&argv)?;
                Ok(self.handle(Action::Nav(Nav::NewConversation)))
            }
            "stop" => {
                no_args(&argv)?;
                if !self.streaming() {
                    return Err("Nothing is generating.".into());
                }
                Ok(self.handle(Action::Composer(
                    crate::exocortex::model::composer::Act::Abort,
                )))
            }
            "model" => self.model_command(&argv),
            "effort" => self.effort_command(&argv),
            "fast" | "ultrafast" => self.speed_command(command.name, &argv),
            "default-model" => self.default_model_command(&argv),
            "theme" => {
                let current = self.persist.settings().theme;
                match argv.as_slice() {
                    [] => {
                        self.notice(
                            &format!("Theme: {current}\nAvailable: whale, cerberus, tonikawa"),
                            Level::Info,
                            None,
                        );
                        Ok(Vec::new())
                    }
                    [theme] => {
                        let id: &'static str = match theme.to_lowercase().as_str() {
                            "whale" => "whale",
                            "cerberus" => "cerberus",
                            "tonikawa" => "tonikawa",
                            _ => return Err("Usage: /theme whale|cerberus|tonikawa".into()),
                        };
                        Ok(self.handle(Action::Ops(ops::Act::SetTheme(id))))
                    }
                    _ => Err("Usage: /theme whale|cerberus|tonikawa".into()),
                }
            }
            "rename" => {
                let id = self.chat.id.clone().ok_or("No conversation to rename.")?;
                let title = args.trim();
                let request = self.requests.issue(
                    Owner::Composer,
                    "command",
                    Some(&id),
                    requests::DEFAULT_WITHIN,
                    std::time::Instant::now(),
                );
                if title.is_empty() {
                    self.notice("Generating a title…", Level::Info, None);
                    return Ok(vec![
                        json!({ "type": "generate_title", "reqId": request, "convId": id }),
                    ]);
                }
                // A rename keeps the chat's mark (an emoji before the title).
                let old = self
                    .sidebar
                    .conversations
                    .get(&id)
                    .map(|s| s.title.clone())
                    .unwrap_or_default();
                let mark = old.split_once(' ').map(|(first, _)| first).filter(|first| {
                    !first.is_empty() && first.chars().all(|c| !c.is_alphanumeric())
                });
                let title = match mark {
                    Some(mark) => format!("{mark} {title}"),
                    None => title.to_owned(),
                };
                Ok(vec![
                    json!({ "type": "rename_conversation", "reqId": request, "convId": id, "title": title }),
                ])
            }
            "replay" | "compact" => {
                no_args(&argv)?;
                let id = self
                    .chat
                    .id
                    .clone()
                    .ok_or(format!("No active conversation to {}.", command.name))?;
                if self.streaming() {
                    return Err(format!(
                        "Cannot {} the conversation while it is streaming.",
                        command.name
                    ));
                }
                let has_history = self
                    .chat
                    .entries
                    .iter()
                    .any(|e| matches!(e, Entry::User { .. } | Entry::Ai { .. }))
                    || self
                        .sidebar
                        .conversations
                        .get(&id)
                        .is_some_and(|s| s.message_count > 0);
                if !has_history {
                    return Err(format!("No conversation history to {}.", command.name));
                }
                let request = self.requests.issue(
                    Owner::Composer,
                    "command",
                    Some(&id),
                    requests::DEFAULT_WITHIN,
                    std::time::Instant::now(),
                );
                Ok(vec![json!({
                    "type": format!("{}_conversation", command.name),
                    "reqId": request,
                    "convId": id,
                    "startedAt": crate::exocortex::model::now_ms(),
                })])
            }
            "trim" => self.trim_command(&argv),
            "instructions" => self.instructions_command(args.trim()),
            "system" => {
                no_args(&argv)?;
                Ok(self.handle(Action::Ops(ops::Act::OpenSystemPrompt)))
            }
            "goal" => self.goal_command(args.trim()),
            "chrono" => Ok(self.handle(Action::Ops(ops::Act::OpenChrono))),
            "queue" => {
                self.notice(
                    "Usage: include /queue in a message, or combine it with a compatible command such as /replay or /compact, to run after all conversations and queued turns are idle. Use /queue <conversation-or-folder> to wait for one conversation, or a 📁 folder, instead, or /queue <duration> (such as 30s, 5h, 8h3m1s, or 9d) to send after that delay once this conversation is idle.",
                    Level::Info,
                    None,
                );
                Ok(Vec::new())
            }
            "reload" => {
                no_args(&argv)?;
                Ok(self.handle(Action::Chat(chat::Act::Retry)))
            }
            "usage" => {
                if argv.first().is_some_and(|a| a == "reset") {
                    return Err("Usage resets are not used from the phone yet. Run /usage reset in the TUI.".into());
                }
                no_args(&argv)?;
                Ok(self.handle(Action::Ops(ops::Act::OpenSettings)))
            }
            "ping" => {
                no_args(&argv)?;
                let id = format!("ping-cmd-{}", crate::exocortex::model::now_ms());
                self.requests.track(
                    &id,
                    Owner::Composer,
                    "ping",
                    requests::DEFAULT_WITHIN,
                    std::time::Instant::now(),
                );
                self.composer.ping = Some((id.clone(), std::time::Instant::now()));
                Ok(vec![
                    json!({ "type": "ping", "reqId": id, "updateStatusOnly": true }),
                ])
            }
            "time" => {
                let n = match argv.as_slice() {
                    [] => 0,
                    [n] => n
                        .parse::<usize>()
                        .map_err(|_| "Usage: /time [n]".to_string())?,
                    _ => return Err("Usage: /time [n]".into()),
                };
                let times: Vec<Option<f64>> = self
                    .chat
                    .entries
                    .iter()
                    .filter_map(|e| match e {
                        Entry::User { metadata, .. } | Entry::Ai { metadata, .. } => {
                            Some(metadata.as_ref().map(|m| m.started_at).filter(|t| *t > 0.0))
                        }
                        _ => None,
                    })
                    .collect();
                let text = if times.is_empty() {
                    "No chat messages yet.".to_owned()
                } else if n >= times.len() {
                    format!(
                        "Only {} chat message{} loaded.",
                        times.len(),
                        if times.len() == 1 { "" } else { "s" }
                    )
                } else {
                    match times[times.len() - 1 - n] {
                        Some(at) => crate::clock::local_date_time_text((at / 1000.0) as i64),
                        None => "No timestamp available for that message.".to_owned(),
                    }
                };
                self.notice(&text, Level::Info, None);
                Ok(Vec::new())
            }
            "convo" => {
                no_args(&argv)?;
                let id = self.chat.id.clone().unwrap_or_default();
                let g = self.generation_now();
                let summary = self.sidebar.conversations.get(&id);
                let lines = [
                    format!(
                        "Title:    {}",
                        summary.map_or("(untitled)", |s| if s.title.is_empty() {
                            "(untitled)"
                        } else {
                            &s.title
                        })
                    ),
                    format!("ID:       {id}"),
                    format!("Provider: {}", g.provider),
                    format!("Model:    {}", g.model),
                    format!("Effort:   {}", g.effort),
                    format!(
                        "Fast:     {}",
                        match g.speed {
                            Speed::Standard => "off",
                            Speed::Fast => "on",
                            Speed::Ultrafast => "ultrafast",
                        }
                    ),
                    format!(
                        "Messages: {}",
                        summary.map_or("?".into(), |s| s.message_count.to_string())
                    ),
                ];
                self.ui.effect(Effect::Copy(lines.join("\n")));
                self.ui.haptic(Haptic::Selection);
                self.notice("Conversation info copied to clipboard.", Level::Info, None);
                Ok(Vec::new())
            }
            "accounts" => {
                no_args(&argv)?;
                Ok(self.handle(Action::Ops(ops::Act::OpenAccounts)))
            }
            "btw" => {
                if args.trim().is_empty() {
                    return Err("Usage: /btw <query>\n       /btw close".into());
                }
                Ok(self.handle(Action::Ops(ops::Act::OpenBtw)))
            }
            "tokens" => {
                no_args(&argv)?;
                Ok(self.handle(Action::Ops(ops::Act::OpenSettings)))
            }
            _ => Err(format!("{name} isn't supported on phone yet.")),
        }
    }

    fn help(&mut self) {
        let mut lines = vec!["PHONE COMMANDS".to_owned()];
        for c in registry::ranked() {
            lines.push(format!("/{} — {}", c.name, c.description));
            if let Some(usage) = c.usage.filter(|u| *u != c.slash()) {
                lines.push(format!("    {usage}"));
            }
        }
        lines.push(String::new());
        lines.push("MACROS".into());
        let catalog = macros::catalog(self.composer.macro_env.as_ref());
        for m in &catalog.items {
            lines.push(format!("{} — {}", m.name, m.desc));
        }
        lines.push(String::new());
        lines.push("Macros expand inline anywhere in a message, including nested arguments. Tap a suggestion to insert it; completion never runs or sends. Tap Run for commands, Send for messages.".into());
        lines.push("Inline modifiers anywhere in a message: /model <provider> <model>, /effort <level>, /fast [on|off], /ultrafast [on|off], /queue [target].".into());
        lines.push("Start with // to send a literal slash. Unknown commands are refused, never sent to the model.".into());
        self.notice(&lines.join("\n"), Level::Info, Some("Help"));
    }

    fn model_command(&mut self, argv: &[String]) -> Outcome {
        let providers = self.ops.providers.clone();
        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        let current = self.generation_now();
        match argv {
            [] => {
                let mut text = format!(
                    "Current: {}/{}\nAvailable:\n",
                    current.provider, current.model
                );
                for p in &providers {
                    let models: Vec<&str> = p.models.iter().map(|m| m.id.as_str()).collect();
                    text.push_str(&format!(
                        "{}: {}{}\n",
                        p.id,
                        if models.is_empty() {
                            "(waiting for daemon)".to_owned()
                        } else {
                            models.join(", ")
                        },
                        if p.allows_custom_models {
                            " (custom ids allowed)"
                        } else {
                            ""
                        }
                    ));
                }
                text.push_str("Usage: /model <provider> <model>");
                self.notice(&text, Level::Info, Some("Models"));
                Ok(Vec::new())
            }
            [p] => {
                let Some(info) = provider_info(&providers, p) else {
                    return Err(format!(
                        "Unknown provider: {p}. Available: {}",
                        ids.join(", ")
                    ));
                };
                let model = if *p == current.provider {
                    current.model.clone()
                } else {
                    info.default_model.clone()
                };
                let efforts: Vec<String> = inline::supported_efforts(&providers, p, &model)
                    .into_iter()
                    .map(|(e, _)| e)
                    .collect();
                let models: Vec<&str> = info.models.iter().map(|m| m.id.as_str()).collect();
                self.notice(
                    &format!(
                        "Current: {model}\nAvailable: {}\nEffort: {}{}",
                        if models.is_empty() {
                            "(waiting for daemon)".to_owned()
                        } else {
                            models.join(", ")
                        },
                        if efforts.is_empty() {
                            "not supported".to_owned()
                        } else {
                            efforts.join(", ")
                        },
                        if info.allows_custom_models {
                            "\nThis provider also accepts custom model ids."
                        } else {
                            ""
                        }
                    ),
                    Level::Info,
                    None,
                );
                Ok(Vec::new())
            }
            [p, m] => {
                if let Some(error) = inline::selection_error(&providers, p, m) {
                    return Err(error);
                }
                if self.chat.id.is_some() && self.streaming() {
                    return Err(
                        "Cannot switch provider/model while this conversation is streaming.".into(),
                    );
                }
                let effort = ops::normalize_effort(&providers, p, m, &current.effort);
                let speed = if supports_speed(&providers, p, m, current.speed) {
                    current.speed
                } else {
                    Speed::Standard
                };
                let commands = self.handle(Action::Ops(ops::Act::PickModel(p.clone(), m.clone())));
                self.notice(
                    &format!(
                        "Model set to {p}/{m}{}{}",
                        if effort != current.effort {
                            format!(" (effort {effort})")
                        } else {
                            String::new()
                        },
                        if current.speed != Speed::Standard && speed == Speed::Standard {
                            " (fast off)"
                        } else {
                            ""
                        }
                    ),
                    Level::Info,
                    None,
                );
                if model_info(&providers, p, m).is_some_and(|i| i.supports_tools == Some(false)) {
                    self.notice(
                        "This endpoint is chat-only: tools and external actions are unavailable.",
                        Level::Warning,
                        None,
                    );
                }
                Ok(commands)
            }
            _ => Err("Usage: /model <provider> <model>".into()),
        }
    }

    fn effort_command(&mut self, argv: &[String]) -> Outcome {
        let providers = self.ops.providers.clone();
        let current = self.generation_now();
        let supported = inline::supported_efforts(&providers, &current.provider, &current.model);
        if let [arg] = argv
            && supported.iter().any(|(e, _)| e == arg)
        {
            let commands = self.handle(Action::Ops(ops::Act::PickEffort(arg.clone())));
            self.notice(&format!("Effort set to {arg}"), Level::Info, None);
            return Ok(commands);
        }
        let default = model_info(&providers, &current.provider, &current.model)
            .map(|m| m.default_effort.clone())
            .unwrap_or_default();
        let choices: Vec<String> = supported
            .iter()
            .map(|(e, _)| {
                let tags: Vec<&str> = [
                    (*e == current.effort).then_some("current"),
                    (*e == default).then_some("default"),
                ]
                .into_iter()
                .flatten()
                .collect();
                if tags.is_empty() {
                    e.clone()
                } else {
                    format!("{e} ({})", tags.join(", "))
                }
            })
            .collect();
        let details: Vec<String> = supported.iter().map(|(e, d)| format!("{e}: {d}")).collect();
        self.notice(
            &format!(
                "Current: {}. Available: {}\n{}",
                current.effort,
                choices.join(", "),
                details.join("\n")
            ),
            Level::Info,
            None,
        );
        Ok(Vec::new())
    }

    fn speed_command(&mut self, name: &str, argv: &[String]) -> Outcome {
        let tier = if name == "ultrafast" {
            Speed::Ultrafast
        } else {
            Speed::Fast
        };
        let label = if tier == Speed::Ultrafast {
            "Ultrafast"
        } else {
            "Fast"
        };
        let arg = argv.first().map(|a| a.to_lowercase());
        if argv.len() > 1 || arg.as_deref().is_some_and(|a| a != "on" && a != "off") {
            return Err(format!("Usage: /{name} [on|off]"));
        }
        let current = self.generation_now();
        let speed = inline::speed_for(tier, arg.as_deref(), current.speed);
        if speed != Speed::Standard
            && !supports_speed(
                &self.ops.providers,
                &current.provider,
                &current.model,
                speed,
            )
        {
            return Err(if tier == Speed::Ultrafast {
                "Ultrafast is not advertised for this model/account.".into()
            } else {
                format!(
                    "Fast mode is only available for {} conversations that support it.",
                    current.provider
                )
            });
        }
        if speed == current.speed {
            self.notice(
                &format!(
                    "{label} mode already {}.",
                    if speed == Speed::Standard {
                        "off"
                    } else {
                        "on"
                    }
                ),
                Level::Info,
                None,
            );
            return Ok(Vec::new());
        }
        let commands = self.handle(Action::Ops(ops::Act::PickSpeed(speed)));
        self.notice(
            &format!(
                "{label} mode {}.",
                if speed == Speed::Standard {
                    "disabled"
                } else {
                    "enabled"
                }
            ),
            Level::Info,
            None,
        );
        Ok(commands)
    }

    fn default_model_command(&mut self, argv: &[String]) -> Outcome {
        const USAGE: &str = "Usage:\n  /default-model\n  /default-model current\n  /default-model reset\n  /default-model <provider> <model> <effort> <fast|ultrafast|off|na>";
        let providers = self.ops.providers.clone();
        let request = |model: &mut Model| {
            model.requests.issue(
                Owner::Composer,
                "command",
                None,
                requests::DEFAULT_WITHIN,
                std::time::Instant::now(),
            )
        };
        let Some(first) = argv.first().map(|a| a.to_lowercase()) else {
            let text = match self.ops.defaults.as_ref() {
                Some(d) => format!(
                    "Defaults for new conversations: {}/{} · {} · {}{}\n\n{USAGE}",
                    d.defaults.provider,
                    d.defaults.model,
                    d.defaults.effort,
                    d.defaults.speed().label(),
                    if d.configured { "" } else { " (app default)" }
                ),
                None => "Waiting for conversation defaults from the connected daemon.".into(),
            };
            self.notice(&text, Level::Info, None);
            return Ok(Vec::new());
        };
        if matches!(first.as_str(), "reset" | "clear" | "product" | "default") && argv.len() == 1 {
            let id = request(self);
            self.notice(
                "Resetting the defaults for new conversations…",
                Level::Info,
                None,
            );
            return Ok(vec![
                json!({ "type": "reset_conversation_defaults", "reqId": id }),
            ]);
        }
        let (provider, model, mut effort, mut speed) = if first == "current" && argv.len() == 1 {
            let g = self.generation_now();
            (g.provider, g.model, Some(g.effort), Some(g.speed))
        } else {
            let (provider, model, rest) = if provider_info(&providers, &first).is_some() {
                let model = argv.get(1).ok_or(USAGE)?;
                (first.clone(), model.clone(), &argv[2..])
            } else if let Some((p, m)) = first.split_once('/') {
                (p.to_owned(), m.to_owned(), &argv[1..])
            } else {
                (self.generation_now().provider, argv[0].clone(), &argv[1..])
            };
            let mut effort = None;
            let mut speed = None;
            for raw in rest {
                let a = raw.to_lowercase();
                if inline::EFFORT_LEVELS.contains(&a.as_str()) {
                    if effort.is_some() {
                        return Err(format!("Effort specified twice: {a}"));
                    }
                    effort = Some(a);
                    continue;
                }
                let s = match a.as_str() {
                    "ultrafast" => Speed::Ultrafast,
                    "fast" | "on" | "true" | "yes" => Speed::Fast,
                    "off" | "na" | "n/a" | "standard" | "normal" | "false" | "no" => {
                        Speed::Standard
                    }
                    _ => return Err(format!("Unknown default-model option: {raw}\n\n{USAGE}")),
                };
                if speed.is_some() {
                    return Err("Fast mode specified twice.".into());
                }
                speed = Some(s);
            }
            (provider, model, effort, speed)
        };
        if let Some(error) = inline::selection_error(&providers, &provider, &model) {
            return Err(error);
        }
        let levels: Vec<String> = inline::supported_efforts(&providers, &provider, &model)
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        if let Some(e) = &effort
            && !levels.contains(e)
        {
            return Err(format!(
                "Invalid effort for {provider}/{model}: {e}. Valid: {}",
                levels.join(", ")
            ));
        }
        let speed_now = speed.take().unwrap_or(Speed::Standard);
        if speed_now != Speed::Standard && !supports_speed(&providers, &provider, &model, speed_now)
        {
            return Err(if speed_now == Speed::Ultrafast {
                "Ultrafast is not advertised for this model/account.".into()
            } else {
                format!("Fast mode is only available for {provider} conversations that support it.")
            });
        }
        let effort = ops::normalize_effort(
            &providers,
            &provider,
            &model,
            &effort.take().unwrap_or_default(),
        );
        let id = request(self);
        self.notice(
            &format!(
                "Default for new conversations: {provider}/{model} · {effort} · {}",
                speed_now.label()
            ),
            Level::Info,
            None,
        );
        Ok(vec![json!({
            "type": "set_conversation_defaults",
            "reqId": id,
            "defaults": {
                "provider": provider,
                "model": model,
                "effort": effort,
                "fastMode": speed_now.value(),
            },
        })])
    }

    fn trim_command(&mut self, argv: &[String]) -> Outcome {
        let id = self
            .chat
            .id
            .clone()
            .ok_or("/trim needs an existing conversation.")?;
        let usage = "Usage: /trim messages|thinking|toolresults <count>";
        let Some(mode) = argv.first() else {
            let g = self.generation_now();
            let max = model_info(&self.ops.providers, &g.provider, &g.model).map(|m| m.max_context);
            let tokens = self.context_tokens();
            let fmt = |n: Option<u64>| n.map_or("unknown".to_owned(), |n| n.to_string());
            let mut text = format!(
                "Current context: {} / {} tokens\nUsage:\n  /trim messages <n>\n  /trim thinking <n>\n  /trim toolresults <n>\n\nmessages    Removes the oldest history entries first.\nthinking    Removes thinking blocks from the oldest assistant turns first.\ntoolresults Replaces the oldest tool result payloads with placeholders first.",
                fmt(tokens),
                fmt(max)
            );
            if g.provider == "anthropic" {
                text.push_str("\n\nClaude Code resumes its own session, which keeps trimmed content; /compact shrinks its context.");
            }
            self.notice(&text, Level::Info, Some("Trim"));
            return Ok(Vec::new());
        };
        if !matches!(mode.as_str(), "messages" | "thinking" | "toolresults") {
            return Err(format!("Unknown trim mode: {mode}. {usage}"));
        }
        if argv.len() != 2 {
            return Err(usage.into());
        }
        let count: u64 = argv[1]
            .parse()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("Trim count must be a positive integer.")?;
        self.require_idle("/trim")?;
        let request = self.requests.issue(
            Owner::Composer,
            "command",
            Some(&id),
            requests::DEFAULT_WITHIN,
            std::time::Instant::now(),
        );
        Ok(vec![
            json!({ "type": "trim_conversation", "reqId": request, "convId": id, "mode": mode, "count": count }),
        ])
    }

    fn instructions_command(&mut self, text: &str) -> Outcome {
        let current = self.chat.entries.iter().find_map(|e| match e {
            Entry::SystemInstructions { text } => Some(text.trim().to_owned()),
            _ => None,
        });
        if text.is_empty() {
            let shown = match current.filter(|t| !t.is_empty()) {
                Some(body) => format!("Current instructions:\n{body}"),
                None => "No system instructions set for this conversation.".into(),
            };
            self.notice(&shown, Level::Info, None);
            return Ok(Vec::new());
        }
        let Some(id) = self.chat.id.clone() else {
            return Err("Send a first message, then set this conversation's instructions.".into());
        };
        let clear = text == "clear";
        let request = self.requests.issue(
            Owner::Composer,
            "command",
            Some(&id),
            requests::DEFAULT_WITHIN,
            std::time::Instant::now(),
        );
        self.notice(
            if clear {
                "Instructions cleared."
            } else {
                "Instructions set."
            },
            Level::Info,
            None,
        );
        Ok(vec![json!({
            "type": "set_system_instructions",
            "reqId": request,
            "convId": id,
            "text": if clear { "" } else { text },
        })])
    }

    fn goal_command(&mut self, args: &str) -> Outcome {
        let Some(id) = self.chat.id.clone() else {
            return Err("Create or open a conversation before using /goal.".into());
        };
        if args.is_empty() {
            return Ok(self.handle(Action::Ops(ops::Act::OpenGoal)));
        }
        let mut command = json!({ "type": "set_goal", "convId": id });
        match args {
            "pause" | "resume" | "complete" | "clear" => command["action"] = Value::from(args),
            _ => {
                let mut objective = args.to_owned();
                if objective == "max-time" || objective.starts_with("max-time ") {
                    let mut parts = objective["max-time".len()..]
                        .trim()
                        .splitn(2, char::is_whitespace);
                    let duration = parts.next().unwrap_or("");
                    let rest = parts.next().unwrap_or("").trim();
                    let ms = super::queue_targets::parse_duration_ms(duration)
                        .filter(|_| !rest.is_empty())
                        .ok_or("Usage: /goal max-time <duration> <objective>, with a duration such as 9h2m1s, 8h3m, 1h, 5m or 2d")?;
                    command["maxTimeMs"] = Value::from(ms);
                    objective = rest.to_owned();
                }
                if objective.starts_with("--max-turns") {
                    return Err(
                        "--max-turns was removed. Use /goal max-time <duration> <objective>."
                            .into(),
                    );
                }
                command["action"] = Value::from("set");
                command["objective"] = Value::from(objective);
            }
        }
        let request = self.requests.issue(
            Owner::Composer,
            "command",
            Some(&id),
            requests::DEFAULT_WITHIN,
            std::time::Instant::now(),
        );
        command["reqId"] = Value::from(request);
        Ok(vec![command])
    }
}
