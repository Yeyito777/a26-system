//! What the user writes: the composer's text, drafts per conversation,
//! sending (or queueing while a turn runs) and new conversations.

use serde_json::Value;

use super::super::proto::{self, Entry, Queued};
use super::{Chat, Echo, Live, Model, now_ms};

impl Model {
    pub(crate) fn save_draft(&mut self) {
        let text = std::mem::take(&mut self.composer);
        match &self.chat.id {
            Some(id) if !text.trim().is_empty() => {
                self.drafts.insert(id.clone(), text);
            }
            Some(id) => {
                self.drafts.remove(id);
            }
            None => {}
        }
    }

    pub(crate) fn send(&mut self) -> Vec<Value> {
        let text = self.composer.trim().to_owned();
        if text.is_empty() || !self.connected {
            return Vec::new();
        }
        self.composer.clear();
        let started_at = now_ms();
        let request = self.request_id();
        match self.chat.id.clone() {
            Some(id) => {
                self.drafts.remove(&id);
                if self.streaming() {
                    // Joins the running turn, as the TUI's Enter does.
                    let queue_id = format!("phone-{started_at}");
                    self.queued.push(Queued {
                        id: queue_id.clone(),
                        conv_id: id.clone(),
                        text: text.clone(),
                    });
                    self.chat.touch();
                    return vec![proto::queue_message(&request, &queue_id, &id, &text)];
                }
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
                let selection = self.selection();
                let mut command =
                    proto::new_conversation(&request, &id, &selection, &text, started_at);
                if let Some(folder) = self.current_folder() {
                    command["folderId"] = Value::String(folder.to_owned());
                }
                self.chat = Chat {
                    id: Some(id),
                    loaded: true,
                    ..Chat::default()
                };
                // The daemon echoes no user_message for a conversation's
                // first message: it is the conversation's first entry.
                self.chat.push(Entry::User {
                    text,
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
        self.queued
            .iter()
            .filter(|q| Some(q.conv_id.as_str()) == self.chat.id.as_deref())
            .collect()
    }
}
