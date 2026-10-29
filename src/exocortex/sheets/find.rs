//! Find in chat (WP-E; PAR-331; the iPhone's `ui/chat/search.tsx`): the
//! loaded history searched for a phrase (case-insensitive, C1's find
//! engine), matches listed with their context; one opens its message in the
//! reader.

use serde_json::Value;

use super::super::find;
use super::super::model::{Action, Model, UiAct, ops};
use super::super::proto::{Block, Entry};
use super::super::theme;
use super::super::ui::sheet::{Prompt, prompt_sheet};
use super::panel::{self, Btn, Built, Kind, Row, Style, Tap};

/// At most this many matches are listed.
const MAX: usize = 60;
/// Characters of context on each side of a match.
const CONTEXT: usize = 28;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    /// The phrase prompt.
    Ask,
    Search(String),
    /// A match's message, in the reader.
    Open(usize),
}

fn act(a: Act) -> Action {
    Action::Ops(ops::Act::Find(a))
}

/// What an entry shows, as plain text.
fn entry_text(entry: &Entry) -> Option<(&'static str, String)> {
    match entry {
        Entry::User { text, .. } => Some(("You", text.clone())),
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
            (!text.trim().is_empty()).then_some(("Reply", text))
        }
        Entry::System { text, .. } => Some(("System", text.clone())),
        _ => None,
    }
}

/// A match: the entry, who, and the line around it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub entry: usize,
    pub who: &'static str,
    pub snippet: String,
}

/// Every place `query` occurs in the loaded entries (C1's
/// `find::matches`), the first `MAX` with their context; and how many.
pub fn search(entries: &[Entry], query: &str) -> (Vec<Hit>, usize) {
    let query = query.trim();
    let mut hits = Vec::new();
    let mut total = 0;
    if query.is_empty() {
        return (hits, 0);
    }
    for (index, entry) in entries.iter().enumerate() {
        let Some((who, text)) = entry_text(entry) else {
            continue;
        };
        for (start, end) in find::matches(&text, query, find::MAX_MATCHES) {
            total += 1;
            if hits.len() >= MAX {
                continue;
            }
            let before: String = {
                let head: Vec<char> = text[..start].chars().collect();
                head[head.len().saturating_sub(CONTEXT)..].iter().collect()
            };
            let after: String = text[end..].chars().take(CONTEXT).collect();
            let mut snippet = format!("{before}{}{after}", &text[start..end]);
            snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
            if before.len() < start {
                snippet.insert(0, '…');
            }
            if end + after.len() < text.len() {
                snippet.push('…');
            }
            hits.push(Hit {
                entry: index,
                who,
                snippet,
            });
        }
    }
    (hits, total)
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
                if let Some((who, text)) = self.chat.entries.get(index).and_then(entry_text) {
                    self.handle(Action::Ui(UiAct::Read {
                        title: if who == "You" {
                            "Your message".into()
                        } else {
                            who.into()
                        },
                        text,
                    }));
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
    let (hits, total) = search(&model.chat.entries, &query);
    let mut rows = vec![Row::Section {
        title: match total {
            0 => format!("No matches for “{query}”"),
            1 => format!("1 match for “{query}”"),
            n => format!("{n} matches for “{query}”"),
        },
        button: Some(("Change".into(), Tap::Stay(act(Act::Ask)))),
    }];
    for hit in &hits {
        rows.push(Row::Task {
            label: hit.who.into(),
            color: if hit.who == "You" {
                p.accent
            } else {
                p.secondary_accent
            },
            title: hit.snippet.clone(),
            time: String::new(),
            tap: Some(Tap::Close(act(Act::Open(hit.entry)))),
            more: None,
        });
    }
    if total > hits.len() {
        rows.push(panel::text(
            format!("… and {} more", total - hits.len()),
            Style::Meta,
            p.muted,
        ));
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
                text: "Where is the Deploy script?".into(),
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
        let (hits, total) = search(&entries, "deploy");
        assert_eq!(total, 4);
        assert_eq!(hits[0].who, "You");
        assert!(hits[0].snippet.contains("Deploy script"), "{:?}", hits[0]);
        assert_eq!(hits[1].entry, 1);
        assert_eq!(search(&entries, "  ").1, 0);
        assert_eq!(search(&entries, "nothing").1, 0);
    }

    #[test]
    fn finding_asks_then_lists_then_reads() {
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
            matches!(&built.rows[0], Row::Section { title, .. } if title == "1 match for “deploy”")
        );
        model.handle(Action::Ops(ops::Act::Find(Act::Open(0))));
        assert_eq!(
            model.ui.opening.pop().unwrap().label(),
            "reader: Your message"
        );
    }
}
