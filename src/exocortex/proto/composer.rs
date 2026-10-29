//! Sending (WP-D): messages, the queue, new conversations, stopping a
//! turn, dictation.

use serde::Deserialize;
use serde_json::{Value, json};

use super::Selection;

/// A queued message (`QueuedMessageInfo`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Queued {
    pub id: String,
    pub conv_id: String,
    pub text: String,
    /// `next-turn` or `message-end`.
    pub timing: String,
    /// `daemon` or `global-idle`.
    pub source: String,
    /// A queued command (`{"name": "/compact"}`).
    pub command: Option<Value>,
    pub automation: Option<Value>,
    /// `conversation` or `new-conversation`.
    pub target: Option<String>,
    pub wait_target: Option<Value>,
    pub folder_id: Option<String>,
    pub created_at: f64,
}

/// `queue_updated`'s ids settled for this client.
pub type SettledIds = Vec<String>;

/// `queue_notice`: a queued message was dropped or delivered elsewhere.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueNotice {
    pub queue_id: String,
    pub conv_id: Option<String>,
    pub message: String,
    /// `warning` or `error`.
    pub level: String,
}

/// `transcription_result`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Transcription {
    pub req_id: Option<String>,
    pub text: String,
}

/// `user_macros`: the daemon's user macros (the picker's to read).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserMacros {
    pub req_id: Option<String>,
    pub macros: Vec<Value>,
    pub change: Option<Value>,
}

pub fn send_message(req_id: &str, conv_id: &str, text: &str, started_at: u64) -> Value {
    json!({
        "type": "send_message",
        "reqId": req_id,
        "convId": conv_id,
        "text": text,
        "startedAt": started_at,
    })
}

pub fn queue_message(req_id: &str, queue_id: &str, conv_id: &str, text: &str) -> Value {
    json!({
        "type": "queue_message",
        "reqId": req_id,
        "queueId": queue_id,
        "convId": conv_id,
        "text": text,
        "timing": "message-end",
    })
}

pub fn new_conversation(
    req_id: &str,
    conv_id: &str,
    selection: &Selection,
    text: &str,
    started_at: u64,
) -> Value {
    let mut command = json!({
        "type": "new_conversation",
        "reqId": req_id,
        "convId": conv_id,
        "provider": selection.provider,
        "model": selection.model,
        "initialMessage": { "text": text, "startedAt": started_at },
    });
    if !selection.effort.is_empty() {
        command["effort"] = json!(selection.effort);
    }
    command
}

/// Stops the turn that started at `expected_started_at` (the daemon ignores
/// it when that one already ended and a queued turn began).
pub fn abort(req_id: &str, conv_id: &str, expected_started_at: Option<f64>) -> Value {
    let mut command =
        json!({ "type": "abort", "reqId": req_id, "convId": conv_id, "reason": "user" });
    if let Some(at) = expected_started_at.filter(|at| *at > 0.0) {
        command["expectedStartedAt"] = json!(at as u64);
    }
    command
}
