//! What the user writes (WP-D): the composer's text, drafts per
//! conversation, sending (or queueing while a turn runs), new
//! conversations, stopping a turn, and the queue the daemon keeps.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;

use super::super::proto::{self, Entry, Event, Queued};
use super::super::requests::{self, Owner, Pending};
use super::{Chat, Echo, Effect, Live, Model, now_ms};
use crate::control::KeyboardPurpose;

/// The composer's state.
#[derive(Debug, Default)]
pub struct Composer {
    /// What is typed.
    pub text: String,
    /// Drafts of conversations not open, by id.
    pub drafts: HashMap<String, String>,
    /// The daemon's queue, every conversation's (`queue_updated`), plus
    /// what was queued from here and not yet confirmed. The transcript
    /// reads it (queued bubbles).
    pub queued: Vec<Queued>,
}

/// What the user does with the composer; other areas use these too (C2's
/// message menu, E's commands): `Insert`, `RestoreDraft`, `Focus`.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Insert, RestoreDraft and Focus are for C2, D and E.
pub enum Act {
    Send,
    /// Stop the turn (not gated on readiness, PAR-197).
    Abort,
    /// Give the composer the keyboard.
    Focus,
    /// Insert text where the caret is (D: at the caret; for now at the end).
    Insert(String),
    /// Put text back into a conversation's draft (`None`: the new
    /// conversation's): a rewind, a failed send.
    RestoreDraft {
        conv: Option<String>,
        text: String,
    },
    /// The "+" button (D: attachments, P2).
    Attach,
}

impl Model {
    /// Keeps what is typed as the open conversation's draft.
    pub(super) fn save_draft(&mut self) {
        let text = std::mem::take(&mut self.composer.text);
        match &self.chat.id {
            Some(id) if !text.trim().is_empty() => {
                self.composer.drafts.insert(id.clone(), text);
            }
            Some(id) => {
                self.composer.drafts.remove(id);
            }
            None => {}
        }
    }

    /// Shows the open conversation's draft in the composer.
    pub(super) fn restore_draft(&mut self) {
        self.composer.text = self
            .chat
            .id
            .as_ref()
            .and_then(|id| self.composer.drafts.get(id).cloned())
            .unwrap_or_default();
    }

    pub(super) fn handle_composer(&mut self, act: Act) -> Vec<Value> {
        match act {
            Act::Send => self.send(),
            Act::Abort => self.abort(),
            Act::Focus => {
                self.ui.effect(Effect::ShowKeyboard(KeyboardPurpose::Text));
                Vec::new()
            }
            Act::Insert(text) => {
                self.composer.text.push_str(&text);
                Vec::new()
            }
            Act::RestoreDraft { conv, text } => {
                if conv == self.chat.id {
                    self.composer.text = text;
                } else if let Some(conv) = conv {
                    self.composer.drafts.insert(conv, text);
                }
                Vec::new()
            }
            Act::Attach => Vec::new(),
        }
    }

    fn abort(&mut self) -> Vec<Value> {
        match (self.chat.id.clone(), self.connected) {
            (Some(id), true) => {
                let started = self.chat.live.as_ref().map(|live| live.started_at);
                let request = self.requests.issue(
                    Owner::Composer,
                    "abort",
                    Some(&id),
                    requests::ABORT_WITHIN,
                    Instant::now(),
                );
                vec![proto::abort(&request, &id, started)]
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn send(&mut self) -> Vec<Value> {
        let text = self.composer.text.trim().to_owned();
        if text.is_empty() || !self.ready {
            return Vec::new();
        }
        self.composer.text.clear();
        let started_at = now_ms();
        let now = Instant::now();
        match self.chat.id.clone() {
            Some(id) => {
                self.composer.drafts.remove(&id);
                if self.streaming() {
                    // Joins the running turn, as the TUI's Enter does.
                    let request = self.requests.id();
                    let queue_id = format!("phone-{started_at}");
                    self.composer.queued.push(Queued {
                        id: queue_id.clone(),
                        conv_id: id.clone(),
                        text: text.clone(),
                        timing: "message-end".into(),
                        ..Queued::default()
                    });
                    self.chat.touch();
                    return vec![proto::queue_message(&request, &queue_id, &id, &text)];
                }
                let request = self.requests.issue(
                    Owner::Composer,
                    "send",
                    Some(&id),
                    requests::DEFAULT_WITHIN,
                    now,
                );
                self.chat.echoes.push(Echo {
                    text: text.clone(),
                    started_at,
                });
                self.chat.touch();
                vec![proto::send_message(&request, &id, &text, started_at)]
            }
            None => {
                let id = format!(
                    "{started_at}-{:06x}",
                    started_at.wrapping_mul(2654435761) & 0xff_ffff
                );
                let request = self.requests.issue(
                    Owner::Composer,
                    "new",
                    Some(&id),
                    requests::DEFAULT_WITHIN,
                    now,
                );
                let selection = self.selection();
                let mut command =
                    proto::new_conversation(&request, &id, &selection, &text, started_at);
                if let Some(folder) = self.current_folder() {
                    command["folderId"] = Value::String(folder.to_owned());
                }
                // Only the draft's explicit model overrides (E, PAR-277).
                self.new_conversation_fields(&mut command);
                self.chat = Chat {
                    id: Some(id),
                    loaded: true,
                    ..Chat::default()
                };
                // The daemon echoes no user_message for a conversation's
                // first message: it is the conversation's first entry.
                self.chat.push(Entry::User {
                    text,
                    images: Vec::new(),
                    metadata: None,
                });
                self.chat.live = Some(Live {
                    started_at: started_at as f64,
                    ..Live::default()
                });
                vec![command]
            }
        }
    }

    /// Messages waiting to join the open conversation.
    pub fn queued_here(&self) -> Vec<&Queued> {
        self.composer
            .queued
            .iter()
            .filter(|q| Some(q.conv_id.as_str()) == self.chat.id.as_deref())
            .collect()
    }

    pub(super) fn apply_composer(&mut self, event: &Event, _now: Instant) -> Vec<Value> {
        if let Event::QueueUpdated { messages, .. } = event {
            self.composer.queued = messages.clone();
            self.chat.touch();
        }
        Vec::new()
    }

    /// An error answering one of the composer's requests; true when
    /// handled (D: restore a failed send into its draft, PAR-185).
    pub(super) fn composer_error(&mut self, _id: &str, _message: &str) -> bool {
        false
    }

    /// One of the composer's requests was not answered in time.
    pub(super) fn composer_timeout(&mut self, _id: &str, pending: &Pending) -> bool {
        let message = match pending.kind {
            "abort" => "Stop did not reach Exocortex. Try again.",
            "send" | "new" => "Exocortex did not confirm the message.",
            _ => return self.generic_timeout(),
        };
        self.ui.error_toast(message);
        true
    }
}
