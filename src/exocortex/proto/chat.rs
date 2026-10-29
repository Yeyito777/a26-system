//! A conversation's history and stream: blocks, entries, windows, and the
//! commands that load them.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{BACKFILL_TURNS, VIEWPORT};

/// One piece of an assistant message.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Block {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        text: String,
    },
    ToolCall {
        #[serde(default)]
        tool_call_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        summary: String,
    },
    ToolResult {
        #[serde(default)]
        tool_call_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        output: String,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Metadata {
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub model: String,
    /// Daemon-authored notices shown as system rows, not the user's words.
    pub system: bool,
    pub kind: Option<String>,
    pub automation: Option<Value>,
}

/// A history entry (`DisplayEntry`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    User {
        #[serde(default)]
        text: String,
        #[serde(default)]
        metadata: Option<Metadata>,
    },
    Ai {
        #[serde(default)]
        blocks: Vec<Block>,
        #[serde(default)]
        metadata: Option<Metadata>,
    },
    System {
        #[serde(default)]
        text: String,
        #[serde(default)]
        color: Option<String>,
    },
    SystemInstructions {
        #[serde(default)]
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolOutput {
    pub tool_call_id: String,
    pub output: String,
}

/// The live tail of a streaming conversation.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PendingAi {
    pub blocks: Vec<Block>,
    pub block_offset: u64,
}

/// A window of history: `conversation_loaded`, `conversation_history_loaded`
/// and `history_updated` all carry one.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Window {
    pub entries: Vec<Entry>,
    pub history_start_index: Option<u64>,
    pub history_start_block_index: Option<u64>,
    pub history_end_index: Option<u64>,
    pub history_end_block_index: Option<u64>,
    pub history_total_entries: Option<u64>,
    pub has_older_history: Option<bool>,
    pub reset_history_window: bool,
    /// The live tail: absent (`None`) leaves the client's own, `null`
    /// (`Some(None)`) ends it (history_updated, PAR-098).
    #[serde(rename = "pendingAI", deserialize_with = "super::present")]
    pub pending_ai: Option<Option<PendingAi>>,
    /// conversation_loaded: the older page asked for with `initialBackfill`
    /// follows, answering this reqId.
    pub initial_backfill_req_id: Option<String>,
}

/// Opens a conversation: a window of about two screens (byte-budgeted;
/// sending `turns` is what makes the daemon page at all, and pages its
/// history_updated too), then the older page `backfill` asks for, from the
/// same snapshot.
pub fn load_conversation(req_id: &str, conv_id: &str, backfill: &str) -> Value {
    json!({
        "type": "load_conversation",
        "reqId": req_id,
        "convId": conv_id,
        "turns": 10,
        "viewport": { "rows": VIEWPORT.0, "cols": VIEWPORT.1 },
        "initialBackfill": { "reqId": backfill, "turns": BACKFILL_TURNS },
    })
}

/// The page of history before `(entry, block)`. `entry` past the end asks
/// for the newest page.
pub fn load_history(req_id: &str, conv_id: &str, entry: u64, block: u64, turns: u32) -> Value {
    let mut command = json!({
        "type": "load_conversation_history",
        "reqId": req_id,
        "convId": conv_id,
        "beforeEntryIndex": entry,
        "turns": turns,
    });
    if block > 0 {
        command["beforeBlockIndex"] = json!(block);
    }
    command
}

/// Asks for the newest page without subscribing or marking it read (the
/// bridge reads the reply this way).
pub fn load_newest(req_id: &str, conv_id: &str, turns: u32) -> Value {
    load_history(req_id, conv_id, 1 << 40, 0, turns)
}

pub fn unsubscribe(conv_id: &str) -> Value {
    json!({ "type": "unsubscribe", "convId": conv_id })
}

pub fn load_tool_outputs(req_id: &str, conv_id: &str, ids: &[String]) -> Value {
    json!({ "type": "load_tool_outputs", "reqId": req_id, "convId": conv_id, "toolCallIds": ids })
}

/// The text a reply leads with: its text blocks, else the last tool's
/// summary.
pub fn reply_text(blocks: &[Block]) -> String {
    let text: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } if !text.trim().is_empty() => Some(text.trim()),
            _ => None,
        })
        .collect();
    text.join("\n\n")
}
