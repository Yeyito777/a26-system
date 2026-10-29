//! A conversation's history and stream (WP-C1 and WP-C2): blocks, entries,
//! windows, images and file links, and the commands that load them.

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
        /// Left out of a folded history page (`load_display_blocks`).
        #[serde(default)]
        elided: Option<Elided>,
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
        #[serde(default)]
        presentation: Option<Presentation>,
        #[serde(default)]
        elided: Option<Elided>,
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
        /// Images the call attached to the message (the attach tool).
        #[serde(default)]
        images: Vec<ImageRef>,
    },
    #[serde(other)]
    Other,
}

impl Block {
    pub fn text(text: impl Into<String>) -> Self {
        Block::Text { text: text.into() }
    }

    pub fn thinking(text: impl Into<String>) -> Self {
        Block::Thinking {
            text: text.into(),
            elided: None,
        }
    }
}

/// Where an elided block's content lives (absolute entry, block index).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Elided {
    pub entry: u64,
    pub block: u64,
}

/// A tool call's invocation-time display details (`ToolCallPresentation`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Presentation {
    /// Bash command styles found beside the executables it ran.
    pub bash_styles: Vec<super::ops::ExternalToolStyle>,
    /// A conversation-scoped tool's own style.
    pub tool_style: Option<super::ops::ToolStyle>,
    /// Shown instead of the input's summary.
    pub detail: Option<String>,
}

/// A daemon-stored image (`ImageRef`): fetched with `get_image`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageRef {
    pub id: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub name: Option<String>,
}

/// An image attached to a user message (`ImageAttachment`); history sends
/// its `image_id` with the bytes compacted away.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageAttachment {
    pub media_type: String,
    pub base64: String,
    pub size_bytes: u64,
    pub image_id: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Output-throughput samples of a message (`GenerationThroughput`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Throughput {
    pub provider: String,
    pub model: String,
    /// Tokens per second of its last ten API rounds.
    pub rates: Vec<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Metadata {
    pub started_at: f64,
    pub ended_at: Option<f64>,
    /// Start of the run's work timer (turns handed off within it share it).
    pub work_timer_started_at: Option<f64>,
    pub model: String,
    /// Output tokens.
    pub tokens: u64,
    pub generation_throughput: Option<Throughput>,
    /// Daemon-authored notices shown as system rows, not the user's words.
    pub system: bool,
    pub kind: Option<String>,
    /// `{"kind": "chrono", "sourceId": …}`: an automated prompt.
    pub automation: Option<Value>,
    pub queue_entry_id: Option<String>,
}

/// A history entry (`DisplayEntry`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    User {
        #[serde(default)]
        text: String,
        #[serde(default)]
        images: Vec<ImageAttachment>,
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
        #[serde(default)]
        metadata: Option<Metadata>,
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
    pub metadata: Option<Metadata>,
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
    /// The conversation's last known input tokens (loaded, updated).
    pub context_tokens: Option<u64>,
    /// Whether tool results carry their outputs.
    pub tool_outputs_included: bool,
}

/// `display_blocks_loaded`: blocks a folded page left out, by entry.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayBlocks {
    pub req_id: Option<String>,
    pub conv_id: String,
    pub ranges: Vec<BlockRange>,
    /// Later ranges are missing: ask again.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BlockRange {
    pub entry: u64,
    pub start: u64,
    pub blocks: Vec<Block>,
}

/// `image_data`: an image's bounded preview.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageData {
    pub req_id: Option<String>,
    pub image_id: String,
    /// `png` or `rgba`.
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub base64: String,
}

/// `file_link_resolved`: a local path in a reply, on the daemon's host.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileLink {
    pub req_id: Option<String>,
    pub conv_id: String,
    pub path: String,
    /// `file` or `directory`.
    pub kind: String,
    pub size: u64,
}

/// `system_instructions_updated`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemInstructions {
    pub conv_id: String,
    /// Empty: cleared.
    pub text: String,
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

/// A stored image as a PNG that fits `width` × `height` (never upscaled);
/// `thumbnail` lets the daemon quantize it (history previews).
pub fn get_image(
    req_id: &str,
    conv_id: Option<&str>,
    image_id: &str,
    (width, height): (u32, u32),
    thumbnail: bool,
) -> Value {
    let mut command = json!({
        "type": "get_image",
        "reqId": req_id,
        "imageId": image_id,
        "format": "png",
        "width": width,
        "height": height,
    });
    if let Some(conv_id) = conv_id {
        command["convId"] = json!(conv_id);
    }
    if thumbnail {
        command["purpose"] = json!("thumbnail");
    }
    command
}

/// Blocks a folded history page left out (`load_display_blocks`).
pub fn load_display_blocks(req_id: &str, conv_id: &str, ranges: &[(u64, u64, u64)]) -> Value {
    let ranges: Vec<Value> = ranges
        .iter()
        .map(|(entry, start, end)| json!({ "entry": entry, "start": start, "end": end }))
        .collect();
    json!({
        "type": "load_display_blocks",
        "reqId": req_id,
        "convId": conv_id,
        "ranges": ranges,
    })
}

/// Where a local path in a reply is (`resolve_file_link`).
pub fn resolve_file_link(req_id: &str, conv_id: &str, target: &str) -> Value {
    json!({ "type": "resolve_file_link", "reqId": req_id, "convId": conv_id, "target": target })
}
