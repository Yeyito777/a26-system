//! Ask aside, BTW (WP-E; PAR-305, 306; the iPhone's `ui/ops/btw` and
//! `state/ops/btw.ts`): a side question answered from the conversation's
//! context that never enters the transcript; follow-ups, then "End aside".
//! The answers show as plain text (C2's block renderer is the iPhone's).
//! A start, follow-up or close the daemon has not confirmed is sent again
//! after a reconnection.

use std::collections::HashMap;

use serde_json::Value;

use super::super::model::{Action, Model, now_ms, ops};
use super::super::proto::{self, Block, Btw, Event};
use super::super::theme;
use super::super::ui::menu::Confirm;
use super::super::ui::textarea::{self, TextArea};
use super::panel::{self, Btn, Built, Kind, Row, Style, Tap};

pub const BUSY: &str = "Wait for the current /btw answer before asking a follow-up.";
pub const EMPTY: &str = "No answer was produced.";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    pub id: String,
    pub query: String,
    /// `running`, `complete`, `error`.
    pub phase: String,
    pub text: String,
    pub status: String,
}

/// A conversation's aside.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Aside {
    pub session: String,
    pub provider: String,
    pub model: String,
    /// `starting`, `running`, `complete`, `error`.
    pub phase: String,
    pub turns: Vec<Turn>,
}

impl Aside {
    pub fn busy(&self) -> bool {
        matches!(self.phase.as_str(), "starting" | "running")
    }

    fn turn(&mut self, id: &str, change: impl FnOnce(&mut Turn)) {
        let Some(index) = self.turns.iter().position(|t| t.id == id) else {
            return;
        };
        change(&mut self.turns[index]);
        if index + 1 == self.turns.len() {
            self.phase = self.turns[index].phase.clone();
        }
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub asides: HashMap<String, Aside>,
    /// Writes the daemon has not confirmed, by conversation.
    pub unsettled: HashMap<String, Value>,
    pub errors: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    /// The question prompt.
    Ask,
    Query(String),
    End,
    EndConfirmed,
}

fn act(a: Act) -> Action {
    Action::Ops(ops::Act::Btw(a))
}

/// A client id (sessions, turns): unique enough.
fn fresh_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(now_ms());
    format!("{:016x}{:08x}", hasher.finish(), now_ms() as u32)
}

fn text_of(blocks: &[Block]) -> String {
    proto::reply_text(blocks)
}

/// The daemon's durable aside (`conversation_loaded.btw`, `btw_snapshot`).
fn project(value: &Value) -> Option<Aside> {
    if value.is_null() {
        return None;
    }
    let str_of = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_owned();
    let turn_of = |v: &Value, id: String| {
        let blocks: Vec<Block> = serde_json::from_value(v["blocks"].clone()).unwrap_or_default();
        let text = if blocks.is_empty() {
            str_of(v, "text")
        } else {
            text_of(&blocks)
        };
        Turn {
            id,
            query: str_of(v, "query"),
            phase: str_of(v, "phase"),
            text,
            status: str_of(v, "status"),
        }
    };
    let session = str_of(value, "sessionId");
    let turns = match value["turns"].as_array() {
        Some(turns) if !turns.is_empty() => {
            turns.iter().map(|t| turn_of(t, str_of(t, "id"))).collect()
        }
        _ => vec![turn_of(value, session.clone())],
    };
    Some(Aside {
        session,
        provider: str_of(value, "provider"),
        model: str_of(value, "model"),
        phase: str_of(value, "phase"),
        turns,
    })
}

impl Model {
    pub fn aside(&self, conv: &str) -> Option<&Aside> {
        self.ops.btw.asides.get(conv)
    }

    pub(crate) fn handle_btw(&mut self, a: Act) -> Vec<Value> {
        let Some(conv) = self.chat.id.clone() else {
            return Vec::new();
        };
        match a {
            Act::Ask => {
                if self.aside(&conv).is_some_and(Aside::busy) {
                    self.ui.toast(BUSY);
                    return Vec::new();
                }
                let follow = self.aside(&conv).is_some();
                self.ui.open(Box::new(textarea::sheet(TextArea::new(
                    if follow { "Follow up" } else { "Ask aside" },
                    "Answered from this chat's context; it never enters the transcript",
                    "",
                    "Ask",
                    Box::new(|text| act(Act::Query(text))),
                ))));
                Vec::new()
            }
            Act::Query(query) => self.ask_aside(&conv, &query),
            Act::End => {
                self.ui.open(Box::new(Confirm::new(
                    "End this aside?",
                    "Its questions and answers are discarded.",
                    "End aside",
                    act(Act::EndConfirmed),
                )));
                Vec::new()
            }
            Act::EndConfirmed => {
                let Some(aside) = self.ops.btw.asides.remove(&conv) else {
                    return Vec::new();
                };
                let request = self.requests.id();
                let command = proto::btw_close(&request, &conv, &aside.session);
                self.ops.btw.unsettled.insert(conv, command.clone());
                vec![command]
            }
        }
    }

    /// Asks (a new aside, or a follow-up in the open one); D's `/btw` too.
    pub fn ask_aside(&mut self, conv: &str, query: &str) -> Vec<Value> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        if !self.ready {
            self.ui.error_toast("Not connected to Exocortex.");
            return Vec::new();
        }
        if self.aside(conv).is_some_and(Aside::busy) {
            self.ops.btw.errors.insert(conv.to_owned(), BUSY.into());
            return Vec::new();
        }
        self.ops.btw.errors.remove(conv);
        let now = now_ms();
        let request = self.requests.id();
        let turn = Turn {
            id: String::new(),
            query: query.to_owned(),
            phase: "running".into(),
            text: String::new(),
            status: "Starting…".into(),
        };
        let command = match self.ops.btw.asides.get_mut(conv) {
            Some(aside) => {
                let id = fresh_id();
                aside.phase = "starting".into();
                aside.turns.push(Turn {
                    id: id.clone(),
                    ..turn
                });
                proto::btw_followup(&request, &aside.session, &id, conv, query, now)
            }
            None => {
                let session = fresh_id();
                let r = self.resolve();
                self.ops.btw.asides.insert(
                    conv.to_owned(),
                    Aside {
                        session: session.clone(),
                        provider: r.provider,
                        model: r.model,
                        phase: "starting".into(),
                        turns: vec![Turn {
                            id: session.clone(),
                            ..turn
                        }],
                    },
                );
                proto::btw_query(&request, &session, conv, query, now)
            }
        };
        self.ops
            .btw
            .unsettled
            .insert(conv.to_owned(), command.clone());
        self.ui.haptic(super::super::ui::Haptic::Light);
        vec![command]
    }

    pub(crate) fn apply_btw(&mut self, event: &Event) -> Vec<Value> {
        let state = &mut self.ops.btw;
        let e: &Btw = match event {
            Event::ConversationLoaded { conv_id, btw, .. } => {
                match btw.as_ref().and_then(project) {
                    Some(aside) => {
                        state.asides.insert(conv_id.clone(), aside);
                    }
                    None => {
                        if btw.is_some() {
                            state.asides.remove(conv_id);
                        }
                    }
                }
                return Vec::new();
            }
            Event::BtwSnapshot(s) => {
                match s.btw.as_ref().and_then(project) {
                    Some(aside) => {
                        state.asides.insert(s.conv_id.clone(), aside);
                    }
                    None => {
                        state.asides.remove(&s.conv_id);
                    }
                }
                return Vec::new();
            }
            // A reconnection is ready: what the daemon has not confirmed
            // goes again.
            Event::Pong { .. } => return state.unsettled.values().cloned().collect(),
            Event::BtwStarted(e)
            | Event::BtwFollowupStarted(e)
            | Event::BtwBlockStart(e)
            | Event::BtwTextChunk(e)
            | Event::BtwThinkingChunk(e)
            | Event::BtwContent(e)
            | Event::BtwToolCall(e)
            | Event::BtwToolResult(e)
            | Event::BtwStatus(e)
            | Event::BtwFinished(e)
            | Event::BtwError(e)
            | Event::BtwClosed(e)
            | Event::BtwMutationSettled(e) => e,
            _ => return Vec::new(),
        };
        // Settled: by the confirmation, or by the session's own events.
        if let Some(command) = state.unsettled.get(&e.conv_id) {
            let kind = command["type"].as_str().unwrap_or("");
            let session = command["sessionId"].as_str();
            let same_session = session.is_none_or(|s| s == e.session_id);
            let settled = match event {
                Event::BtwMutationSettled(_) => same_session,
                Event::BtwClosed(_) => kind == "btw_close" && same_session,
                _ => {
                    kind != "btw_close"
                        && same_session
                        && (kind != "btw_followup"
                            || command["turnId"].as_str() == e.turn_id.as_deref())
                }
            };
            if settled {
                state.unsettled.remove(&e.conv_id);
            }
        }
        if let Event::BtwStarted(e) = event {
            state.asides.insert(
                e.conv_id.clone(),
                Aside {
                    session: e.session_id.clone(),
                    provider: e.provider.clone().unwrap_or_default(),
                    model: e.model.clone().unwrap_or_default(),
                    phase: "running".into(),
                    turns: vec![Turn {
                        id: e.session_id.clone(),
                        query: e.query.clone().unwrap_or_default(),
                        phase: "running".into(),
                        text: String::new(),
                        status: "Thinking…".into(),
                    }],
                },
            );
            return Vec::new();
        }
        if let Event::BtwClosed(e) = event {
            if state
                .asides
                .get(&e.conv_id)
                .is_some_and(|a| a.session == e.session_id)
            {
                state.asides.remove(&e.conv_id);
            }
            return Vec::new();
        }
        let Some(aside) = state.asides.get_mut(&e.conv_id) else {
            return Vec::new();
        };
        if aside.session != e.session_id {
            return Vec::new();
        }
        let id = e.turn_id.clone().unwrap_or_else(|| e.session_id.clone());
        match event {
            Event::BtwFollowupStarted(_) => {
                let fresh = Turn {
                    id: id.clone(),
                    query: e.query.clone().unwrap_or_default(),
                    phase: "running".into(),
                    text: String::new(),
                    status: "Thinking…".into(),
                };
                match aside.turns.iter_mut().find(|t| t.id == id) {
                    Some(turn) => *turn = fresh,
                    None => aside.turns.push(fresh),
                }
                aside.phase = "running".into();
            }
            Event::BtwBlockStart(_) | Event::BtwToolCall(_) | Event::BtwToolResult(_) => {
                aside.turn(&id, |t| t.phase = "running".into());
            }
            Event::BtwTextChunk(_) => {
                let text = e.text.clone().unwrap_or_default();
                aside.turn(&id, |t| {
                    t.text.push_str(&text);
                    t.phase = "running".into();
                });
            }
            Event::BtwThinkingChunk(_) => aside.turn(&id, |t| t.phase = "running".into()),
            Event::BtwContent(_) => {
                let text = match &e.blocks {
                    Some(blocks) if !blocks.is_empty() => text_of(blocks),
                    _ => e.text.clone().unwrap_or_default(),
                };
                aside.turn(&id, |t| t.text = text);
            }
            Event::BtwStatus(_) => {
                let status = e.status.clone().unwrap_or_default();
                aside.turn(&id, |t| t.status = status);
            }
            Event::BtwFinished(_) => aside.turn(&id, |t| {
                t.phase = "complete".into();
                t.status = "Complete".into();
            }),
            Event::BtwError(_) => {
                let message = e.message.clone().unwrap_or_default();
                aside.turn(&id, |t| {
                    t.phase = "error".into();
                    t.status = message;
                });
            }
            _ => {}
        }
        Vec::new()
    }
}

/// The Ask aside panel's rows.
pub fn build(model: &Model, conv: &str) -> Built {
    let p = theme::p();
    let c = theme::c();
    let aside = model.aside(conv);
    let mut rows = Vec::new();
    match aside {
        Some(aside) => {
            let label = model.model_label(&proto::Selection {
                provider: aside.provider.clone(),
                model: aside.model.clone(),
                ..Default::default()
            });
            rows.push(Row::Section {
                title: format!("{label} · read-only aside"),
                button: Some(("End aside".into(), Tap::Stay(act(Act::End)))),
            });
            for turn in &aside.turns {
                rows.push(Row::Gap(22));
                rows.push(Row::Rail {
                    color: p.secondary_accent,
                    lines: vec![(turn.query.clone(), Style::Body, p.text)],
                });
                if !turn.text.trim().is_empty() {
                    rows.push(panel::text(turn.text.clone(), Style::Body, p.text));
                }
                match turn.phase.as_str() {
                    "running" => rows.push(panel::text(
                        if turn.status.is_empty() {
                            "Thinking…".into()
                        } else {
                            turn.status.clone()
                        },
                        Style::Caption,
                        c.text_dim,
                    )),
                    "error" => rows.push(panel::text(
                        if turn.status.is_empty() {
                            EMPTY.into()
                        } else {
                            turn.status.clone()
                        },
                        Style::Caption,
                        p.error,
                    )),
                    "complete" if turn.text.trim().is_empty() => {
                        rows.push(panel::text(EMPTY, Style::Caption, p.muted));
                    }
                    _ => {}
                }
            }
        }
        None => {
            let title = model
                .sidebar
                .conversations
                .get(conv)
                .map(|s| s.display_title().to_owned());
            rows.push(panel::note(format!(
                "A side question answered from the context of {}. It never enters the transcript.",
                title.map_or_else(|| "this chat".to_owned(), |t| format!("“{t}”"))
            )));
        }
    }
    let busy = aside.is_some_and(Aside::busy);
    if let Some(error) = model.ops.btw.errors.get(conv) {
        rows.push(panel::text(error.clone(), Style::Caption, p.error));
    } else if busy {
        rows.push(panel::text(BUSY, Style::Caption, p.muted));
    }
    rows.push(Row::Buttons(vec![Btn::new(
        if aside.is_some() {
            "Follow up…"
        } else {
            "Ask aside…"
        },
        Kind::Primary,
        (!busy && model.ready).then_some(Tap::Stay(act(Act::Ask))),
    )]));
    Built {
        title: "BTW".into(),
        rows,
        wake: None,
    }
}

/// The chat menu's line for the aside.
pub fn menu_detail(model: &Model, conv: &str) -> String {
    match model.aside(conv) {
        Some(aside) if aside.busy() => "Answering…".into(),
        Some(aside) => format!("{} answered", aside.turns.len()),
        None => "A side question that stays out of the chat".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;
    use std::time::Instant;

    #[test]
    fn an_aside_streams_follows_up_and_ends() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let sent = model.ask_aside("c", "why?");
        assert_eq!(sent[0]["type"], "btw_query");
        let session = sent[0]["sessionId"].as_str().unwrap().to_owned();
        let now = Instant::now();
        for line in [
            format!(
                r#"{{"type":"btw_started","convId":"c","sessionId":"{session}","query":"why?","provider":"anthropic","model":"m","startedAt":1}}"#
            ),
            format!(
                r#"{{"type":"btw_text_chunk","convId":"c","sessionId":"{session}","text":"Because "}}"#
            ),
            format!(
                r#"{{"type":"btw_text_chunk","convId":"c","sessionId":"{session}","text":"so."}}"#
            ),
            format!(
                r#"{{"type":"btw_finished","convId":"c","sessionId":"{session}","endedAt":2}}"#
            ),
            r#"{"type":"btw_text_chunk","convId":"c","sessionId":"stale","text":"no"}"#.into(),
        ] {
            model.apply(parse_event(&line), now);
        }
        let aside = model.aside("c").unwrap();
        assert_eq!(aside.turns[0].text, "Because so.");
        assert!(!aside.busy());
        assert!(
            model.ops.btw.unsettled.is_empty(),
            "the session's events settle the start"
        );
        let follow = model.ask_aside("c", "and?");
        assert_eq!(follow[0]["type"], "btw_followup");
        assert!(model.aside("c").unwrap().busy());
        assert!(model.ask_aside("c", "again?").is_empty(), "one at a time");
        assert_eq!(
            model.ops.btw.errors.get("c").map(String::as_str),
            Some(BUSY)
        );
        // Unconfirmed, it goes again once a new connection is ready.
        let again = model.apply(parse_event(r#"{"type":"pong","reqId":"x"}"#), now);
        assert!(again.iter().any(|c| c["type"] == "btw_followup"));
        let close = model.handle(Action::Ops(ops::Act::Btw(Act::EndConfirmed)));
        assert_eq!(close[0]["type"], "btw_close");
        assert!(model.aside("c").is_none());
    }
}
