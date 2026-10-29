//! Sending: messages, the queue, new conversations, stopping a turn.

use serde::Deserialize;
use serde_json::{Value, json};

use super::Selection;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Queued {
    pub id: String,
    pub conv_id: String,
    pub text: String,
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

pub fn abort(conv_id: &str, expected_started_at: Option<f64>) -> Value {
    let mut command = json!({ "type": "abort", "convId": conv_id, "reason": "user" });
    if let Some(at) = expected_started_at.filter(|at| *at > 0.0) {
        command["expectedStartedAt"] = json!(at as u64);
    }
    command
}
