//! Find in chat (WP-E; PAR-331; the iPhone's `ui/chat/search.tsx`): the
//! loaded history searched for a phrase (case- and accent-insensitive,
//! C1's find engine), matches listed with their context; tapping one scrolls
//! the chat to its message.

use serde_json::Value;

use super::super::find;
use super::super::model::{Action, Effect, Model, ops};
use super::super::proto::{Block, Entry};
use super::super::theme;
use super::super::ui::Haptic;
use super::super::ui::sheet::{Prompt, prompt_sheet};
use super::panel::{self, Btn, Built, Kind, Row, Style, Tap};

/// At most this many matches are listed.
const MAX: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    /// The phrase prompt.
    Ask,
    Search(String),
    /// The chat scrolled to a match's message.
    Open(usize),
}

fn act(a: Act) -> Action {
    Action::Ops(ops::Act::Find(a))
}

/// What an entry shows, as plain text, under its role (the iPhone's
/// `roleLabel`).
fn entry_text(entry: &Entry) -> Option<(&'static str, String)> {
    match entry {
        Entry::User { text, .. } => Some(("USER", text.clone())),
        Entry::Ai { blocks, .. } => {
            let mut parts = Vec::new();
            for block in blocks {
                match block {
                    Block::Text { text } => parts.push(text.clone()),
                    Block::ToolCall {
                        tool_name, summary, ..
                    } => parts.push(format!("{tool_name} {summary}")),
                    _ => {}
                }
            }
            let text = parts.join("\n");
            (!text.trim().is_empty()).then_some(("ASSISTANT", text))
        }
        Entry::System { text, .. } => Some(("SYSTEM", text.clone())),
        Entry::SystemInstructions { text } => Some(("SYSTEM INSTRUCTIONS", text.clone())),
        _ => None,
    }
}

/// The loaded entries as the search reads them: index, role, plain text.
fn messages(entries: &[Entry]) -> Vec<(usize, String, String)> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let (role, text) = entry_text(entry)?;
            Some((index, role.to_owned(), text))
        })
        .collect()
}

impl Model {
    pub(crate) fn handle_find(&mut self, a: Act) -> Vec<Value> {
        match a {
            Act::Ask => {
                self.ui.open(Box::new(prompt_sheet(Prompt::new(
                    "Find in chat",
                    "Searches the loaded part of the conversation",
                    self.ops.find.clone().unwrap_or_default(),
                    "Find",
                    Box::new(|text| act(Act::Search(text))),
                ))));
            }
            Act::Search(query) => {
                self.ops.find = Some(query);
                if let Some(conv) = self.chat.id.clone() {
                    self.open_panel(panel::Panel::Find(conv));
                }
            }
            Act::Open(index) => {
                if let Some(&stamp) = self.chat.stamps.get(index) {
                    self.ui.haptic(Haptic::Selection);
                    self.ui.effects.push(Effect::Reveal(stamp));
                }
            }
        }
        Vec::new()
    }
}

/// The Find panel's rows.
pub fn build(model: &Model, _conv: &str) -> Built {
    let p = theme::p();
    let query = model.ops.find.clone().unwrap_or_default();
    let hits = find::search_messages(&messages(&model.chat.entries), &query, find::MAX_MATCHES);
    let total = hits.len();
    let mut rows = vec![Row::Section {
        title: match total {
            0 => format!("No matches for “{query}”"),
            1 => format!("1 result for “{query}”"),
            n if n >= find::MAX_MATCHES => format!("500+ results for “{query}”"),
            n => format!("{n} results for “{query}”"),
        },
        button: Some(("Change".into(), Tap::Stay(act(Act::Ask)))),
    }];
    for hit in hits.iter().take(MAX) {
        rows.push(Row::Hit {
            role: hit.role.clone(),
            preview: hit.preview.clone(),
            start: hit.start,
            end: hit.end,
            tap: Tap::Close(act(Act::Open(hit.key))),
        });
    }
    if total > MAX {
        rows.push(panel::text(
            format!("… and {} more", total - MAX),
            Style::Meta,
            p.muted,
        ));
    }
    if total == 0 && !query.trim().is_empty() {
        rows.push(panel::note("No matches in the loaded history."));
    }
    if model.chat.has_older {
        rows.push(panel::note(
            "Only the loaded part is searched: scroll up in the chat to load earlier messages.",
        ));
    }
    rows.push(Row::Buttons(vec![Btn::new(
        "Find something else…",
        Kind::Secondary,
        Some(Tap::Stay(act(Act::Ask))),
    )]));
    Built {
        title: "Find in chat".into(),
        rows,
        wake: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_are_found_with_their_context() {
        let entries = vec![
            Entry::User {
                text: "Where is the Déploy script?".into(),
                images: Vec::new(),
                metadata: None,
            },
            Entry::Ai {
                blocks: vec![Block::Text {
                    text: "The deploy script is scripts/deploy.sh; deploy runs it.".into(),
                }],
                metadata: None,
            },
        ];
        let hits = find::search_messages(&messages(&entries), "deploy", 500);
        assert_eq!(hits.len(), 4, "accents and case folded");
        assert_eq!(hits[0].role, "USER");
        assert_eq!(hits[1].key, 1);
        let chars: Vec<char> = hits[0].preview.chars().collect();
        assert_eq!(
            chars[hits[0].start..hits[0].end].iter().collect::<String>(),
            "Déploy"
        );
    }

    #[test]
    fn finding_asks_then_lists_then_shows_the_message() {
        use crate::exocortex::model::Nav;
        let mut model = Model::new();
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        model.chat.push(Entry::User {
            text: "the deploy script".into(),
            images: Vec::new(),
            metadata: None,
        });
        model.handle(Action::Ops(ops::Act::OpenFind));
        assert_eq!(model.ui.opening.pop().unwrap().label(), "Find in chat");
        model.handle(Action::Ops(ops::Act::Find(Act::Search("deploy".into()))));
        assert_eq!(model.ui.opening.pop().unwrap().label(), "");
        let built = build(&model, "c");
        assert!(
            matches!(&built.rows[0], Row::Section { title, .. } if title == "1 result for “deploy”")
        );
        assert!(matches!(&built.rows[1], Row::Hit { role, .. } if role == "USER"));
        model.handle(Action::Ops(ops::Act::Find(Act::Open(0))));
        assert_eq!(
            model.ui.effects.last(),
            Some(&Effect::Reveal(model.chat.stamps[0]))
        );
    }

    #[test]
    fn a_match_is_scrolled_to() {
        use crate::exocortex::model::Nav;
        use crate::exocortex::view::ViewState;
        let mut model = Model::new();
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        for n in 0..40 {
            model.chat.push(Entry::User {
                text: format!("message {n}"),
                images: Vec::new(),
                metadata: None,
            });
        }
        let mut view = ViewState::new();
        let height = 2400.0;
        let max = view.chat_max(&model, height);
        assert!(max > 0.0);
        assert!(view.reveal(&model, model.chat.stamps[20], height));
        let middle = view.chat_scroll.offset;
        assert!(middle > 0.0 && middle < max, "{middle} of {max}");
        assert!(!view.follow);
        assert!(view.reveal(&model, model.chat.stamps[0], height));
        assert!(view.chat_scroll.offset < middle);
        assert!(view.reveal(&model, model.chat.stamps[39], height));
        assert_eq!(view.chat_scroll.offset, max);
        assert!(!view.reveal(&model, u64::MAX, height));
    }
}
