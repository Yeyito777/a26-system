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

/// `path_directory_entries`: a directory on the daemon's host, for path
/// completion (PAR-235).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PathEntries {
    pub req_id: Option<String>,
    pub listings: Vec<PathListing>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PathListing {
    /// As the prompt spells it (`~/src/`).
    pub directory: String,
    /// Every entry starts with it.
    pub prefix: String,
    pub entries: Vec<PathEntry>,
    /// A large directory, cut short.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PathEntry {
    pub name: String,
    /// `dir` or `file`.
    #[serde(rename = "type")]
    pub kind: String,
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

/// Queues `text` to join conversation `conv_id`'s running turn at `timing`
/// (`next-turn` or `message-end`); `queue_id` names it until it is
/// delivered (the daemon takes a resend with the same id once).
pub fn queue_message(
    req_id: &str,
    queue_id: &str,
    conv_id: &str,
    text: &str,
    timing: &str,
) -> Value {
    json!({
        "type": "queue_message",
        "reqId": req_id,
        "queueId": queue_id,
        "convId": conv_id,
        "text": text,
        "timing": timing,
    })
}

/// Takes a queued message out of the queue (PAR-259).
pub fn unqueue_message(req_id: &str, queue_id: &str) -> Value {
    json!({ "type": "unqueue_message", "reqId": req_id, "queueId": queue_id })
}

/// A new conversation with its first message, in `folder`, with only the
/// model choices the user made for it (`overrides`; none: the daemon's
/// defaults, PAR-277).
pub fn new_conversation(
    req_id: &str,
    conv_id: &str,
    overrides: Option<&Selection>,
    folder: Option<&str>,
    text: &str,
    started_at: u64,
) -> Value {
    let mut command = json!({
        "type": "new_conversation",
        "reqId": req_id,
        "convId": conv_id,
        "initialMessage": { "text": text, "startedAt": started_at },
    });
    if let Some(folder) = folder {
        command["folderId"] = json!(folder);
    }
    if let Some(selection) = overrides {
        for (key, value) in [
            ("provider", &selection.provider),
            ("model", &selection.model),
            ("effort", &selection.effort),
        ] {
            if !value.is_empty() {
                command[key] = json!(value);
            }
        }
        if !selection.fast_mode.is_null() {
            command["fastMode"] = selection.fast_mode.clone();
        }
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
