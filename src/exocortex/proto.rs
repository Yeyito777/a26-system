//! The Exocortex daemon's wire protocol, as far as the app and the
//! notification bridge use it (exocortex `shared/src/protocol.ts` is the
//! spec): newline-delimited JSON over a Unix socket, commands from the
//! client, events from the daemon. Parsing is lenient: unknown events and
//! fields are ignored, missing optional fields take their defaults, so a newer
//! daemon never breaks an older phone.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

/// Where the phone's daemon (root, in the chroot, checkout `/opt/exocortex`)
/// listens. `EXOCORTEX_SOCKET` overrides it.
pub const DEFAULT_SOCKET: &str = "/opt/exocortex/config/runtime/exocortexd.sock";

pub fn socket_path() -> PathBuf {
    std::env::var_os("EXOCORTEX_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))
}

/// A sidebar row (`ConversationSummary`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Summary {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub title: String,
    pub created_at: f64,
    pub updated_at: f64,
    pub message_count: u64,
    pub pinned: bool,
    pub marked: bool,
    pub muted: bool,
    pub streaming: bool,
    pub unread: bool,
    pub notifications_muted: bool,
    pub sort_order: f64,
    pub folder_id: Option<String>,
    pub parent_conversation_id: Option<String>,
    pub goal: Option<Goal>,
}

impl Summary {
    /// The title as the user should read it: the daemon names a
    /// conversation "pending" until its title is generated.
    pub fn display_title(&self) -> &str {
        match self.title.trim() {
            "" | "pending" => "New Conversation",
            title => title,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Goal {
    pub objective: String,
    pub status: String,
    pub paused_by: Option<String>,
    pub pause_reason: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub pinned: bool,
    pub muted: bool,
    pub sort_order: f64,
}

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
pub struct Effort {
    pub effort: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub default_effort: String,
    pub supported_efforts: Vec<Effort>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub default_model: String,
    pub models: Vec<Model>,
}

/// How a tool's calls are labelled (`ToolDisplayInfo`): "$" for bash.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolStyle {
    pub name: String,
    pub label: String,
    pub color: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    pub provider: String,
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultsSnapshot {
    pub defaults: Selection,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Queued {
    pub id: String,
    pub conv_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolOutput {
    pub tool_call_id: String,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRef {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderUpdate {
    pub item: ItemRef,
    pub sort_order: f64,
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
    #[serde(rename = "pendingAI", deserialize_with = "present")]
    pub pending_ai: Option<Option<PendingAi>>,
    /// conversation_loaded: the older page asked for with `initialBackfill`
    /// follows, answering this reqId.
    pub initial_backfill_req_id: Option<String>,
}

/// A field that may be `null`: present (even as null) is `Some`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// The events the app and the bridge act on. Everything else is `Other`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    Pong {
        #[serde(default)]
        req_id: Option<String>,
    },
    ToolsAvailable {
        #[serde(default)]
        providers: Vec<Provider>,
        #[serde(default)]
        tools: Vec<ToolStyle>,
        #[serde(default)]
        auth_by_provider: HashMap<String, bool>,
        #[serde(default)]
        conversation_defaults: Option<DefaultsSnapshot>,
    },
    ConversationDefaults {
        #[serde(default)]
        defaults: Selection,
    },
    ConversationsList {
        #[serde(default)]
        conversations: Vec<Summary>,
        #[serde(default)]
        folders: Option<Vec<Folder>>,
        #[serde(default)]
        sidebar_partial: Option<Value>,
        #[serde(default)]
        sidebar_rest: Option<bool>,
    },
    ConversationUpdated {
        summary: Summary,
        #[serde(default)]
        stream_stop_reason: Option<String>,
        #[serde(default)]
        owner_turn_finished: bool,
    },
    ConversationRestored {
        summary: Summary,
    },
    ConversationUnwound {
        conv_id: String,
        summary: Summary,
    },
    ConversationDeleted {
        conv_id: String,
    },
    ConversationPinned {
        conv_id: String,
        pinned: bool,
    },
    ConversationMoved {
        #[serde(default)]
        conversations: Vec<Summary>,
        #[serde(default)]
        folders: Option<Vec<Folder>>,
    },
    SidebarItemsReordered {
        #[serde(default)]
        updates: Vec<OrderUpdate>,
    },
    SidebarStatePatched {
        #[serde(default)]
        conversations: Vec<Summary>,
        #[serde(default)]
        folders: Vec<Folder>,
        #[serde(default)]
        removed_conversation_ids: Vec<String>,
        #[serde(default)]
        removed_folder_ids: Vec<String>,
    },
    ConversationCreated {
        #[serde(default)]
        req_id: Option<String>,
        conv_id: String,
        #[serde(default)]
        provider: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        effort: String,
    },
    ConversationLoaded {
        #[serde(default)]
        req_id: Option<String>,
        conv_id: String,
        #[serde(default)]
        provider: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        effort: String,
        #[serde(flatten)]
        window: Window,
    },
    ConversationHistoryLoaded {
        #[serde(default)]
        req_id: Option<String>,
        conv_id: String,
        #[serde(flatten)]
        window: Window,
    },
    HistoryUpdated {
        conv_id: String,
        #[serde(flatten)]
        window: Window,
    },
    StreamingStarted {
        conv_id: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        started_at: f64,
        #[serde(default)]
        blocks: Option<Vec<Block>>,
        #[serde(default)]
        block_offset: Option<u64>,
    },
    StreamingStopped {
        conv_id: String,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        persisted_blocks: Option<Vec<Block>>,
    },
    BlockStart {
        conv_id: String,
        #[serde(default)]
        block_type: String,
    },
    TextChunk {
        conv_id: String,
        #[serde(default)]
        text: String,
    },
    ThinkingChunk {
        conv_id: String,
        #[serde(default)]
        text: String,
    },
    StreamingSync {
        conv_id: String,
        #[serde(default)]
        blocks: Vec<Block>,
    },
    ToolCall {
        conv_id: String,
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
        conv_id: String,
        #[serde(default)]
        tool_call_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        output: String,
        #[serde(default)]
        is_error: bool,
    },
    MessageComplete {
        conv_id: String,
        #[serde(default)]
        blocks: Vec<Block>,
    },
    UserMessage {
        conv_id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        started_at: Option<f64>,
        #[serde(default)]
        automation: Option<Value>,
    },
    SystemMessage {
        conv_id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        color: Option<String>,
    },
    StreamRetry {
        conv_id: String,
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        max_attempts: u32,
        #[serde(default)]
        error_message: String,
    },
    QueueUpdated {
        #[serde(default)]
        messages: Vec<Queued>,
    },
    ToolOutputsLoaded {
        conv_id: String,
        #[serde(default)]
        outputs: Vec<ToolOutput>,
    },
    ConnectivityStatus {
        #[serde(default)]
        online: bool,
    },
    DaemonShutdown {},
    Error {
        #[serde(default)]
        req_id: Option<String>,
        #[serde(default)]
        conv_id: Option<String>,
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Other,
}

/// Parses one line from the daemon. A line that is not an event this
/// client understands (or not JSON) is `Other`.
pub fn parse_event(line: &str) -> Event {
    match serde_json::from_str::<Event>(line) {
        Ok(event) => event,
        Err(error) => {
            if cfg!(debug_assertions) {
                let kind = serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned));
                eprintln!("exocortex: unparsed event {kind:?}: {error}");
            }
            Event::Other
        }
    }
}

/// The cheap first look the bridge takes at a line before parsing it: the
/// event's `type`, which the daemon writes first.
pub fn event_type(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("{\"type\":\"")?;
    rest.split('"').next()
}

// ----- Commands --------------------------------------------------------

/// Capabilities this client understands (protocol.ts `ClientCapability`):
/// compact sidebar deltas, byte-budgeted history windows and token stats
/// only on request.
pub const APP_CAPABILITIES: [&str; 6] = [
    "targeted-unwind",
    "sidebar-reorder-delta",
    "sidebar-state-patch",
    "history-block-pagination",
    "token-stats-on-demand",
    "sidebar-incremental",
];

/// The phone's history viewport in terminal-cell terms (the daemon sizes the
/// opening window to fill two of these).
pub const VIEWPORT: (u32, u32) = (40, 44);

/// Turns in the older page that follows an opening window, and in each page
/// fetched as the user scrolls up (all within the daemon's byte budget).
pub const BACKFILL_TURNS: u32 = 10;
pub const OLDER_TURNS: u32 = 15;

pub fn client_capabilities(capabilities: &[&str], incremental_rows: Option<u32>) -> Value {
    let mut command = json!({ "type": "client_capabilities", "capabilities": capabilities });
    if let Some(rows) = incremental_rows {
        command["sidebarView"] = json!({
            "folderView": "navigate",
            "currentFolderId": null,
            "expandedFolderIds": [],
            "scrollOffset": 0,
            "rows": rows,
            "conversationIds": [],
        });
    }
    command
}

pub fn ping(req_id: &str) -> Value {
    json!({ "type": "ping", "reqId": req_id })
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

pub fn set_model(conv_id: &str, provider: &str, model: &str) -> Value {
    json!({ "type": "set_model", "convId": conv_id, "provider": provider, "model": model })
}

pub fn set_effort(conv_id: &str, effort: &str) -> Value {
    json!({ "type": "set_effort", "convId": conv_id, "effort": effort })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_summary_update_with_unknown_fields() {
        let line = r#"{"type":"conversation_updated","summary":{"id":"c1","provider":"anthropic","model":"claude-haiku-5-5","effort":"low","fastMode":false,"createdAt":1,"updatedAt":2,"messageCount":4,"title":"pending","goal":null,"marked":false,"pinned":true,"sortOrder":-3,"folderId":null,"streaming":false,"unread":true,"tasks":[],"integrations":[],"brandNew":7},"ownerTurnFinished":true}"#;
        let Event::ConversationUpdated {
            summary,
            owner_turn_finished,
            stream_stop_reason,
        } = parse_event(line)
        else {
            panic!("not an update");
        };
        assert!(owner_turn_finished);
        assert_eq!(stream_stop_reason, None);
        assert_eq!(summary.id, "c1");
        assert!(summary.pinned && summary.unread);
        assert_eq!(summary.sort_order, -3.0);
        assert_eq!(summary.display_title(), "New Conversation");
        assert_eq!(event_type(line), Some("conversation_updated"));
    }

    #[test]
    fn parses_history_windows_and_blocks() {
        let line = r#"{"type":"conversation_loaded","reqId":"l","convId":"c1","provider":"anthropic","model":"m","effort":"low","fastMode":false,"entries":[{"type":"system_instructions","text":"x"},{"type":"user","text":"Hi","metadata":{"startedAt":5,"endedAt":5,"model":"m","tokens":0}},{"type":"ai","blocks":[{"type":"thinking","text":"hm"},{"type":"tool_call","toolCallId":"t1","toolName":"Bash","input":{"command":"ls"},"summary":"ls"},{"type":"tool_result","toolCallId":"t1","toolName":"","output":"","isError":false},{"type":"text","text":"**Done**"},{"type":"image","id":"z"}],"metadata":null},{"type":"system","text":"Error","color":"error"},{"type":"future"}],"historyStartIndex":3,"historyStartBlockIndex":2,"historyTotalEntries":9,"hasOlderHistory":true,"pendingAI":{"blocks":[{"type":"text","text":"so"}],"metadata":null},"contextTokens":10,"toolOutputsIncluded":false}"#;
        let Event::ConversationLoaded {
            conv_id, window, ..
        } = parse_event(line)
        else {
            panic!("not loaded");
        };
        assert_eq!(conv_id, "c1");
        assert_eq!(window.entries.len(), 5);
        assert_eq!(window.history_start_index, Some(3));
        assert_eq!(window.history_start_block_index, Some(2));
        assert_eq!(window.has_older_history, Some(true));
        let Entry::Ai { blocks, .. } = &window.entries[2] else {
            panic!("not ai");
        };
        assert_eq!(blocks.len(), 5);
        assert!(
            matches!(&blocks[1], Block::ToolCall { tool_name, summary, .. } if tool_name == "Bash" && summary == "ls")
        );
        assert_eq!(blocks[4], Block::Other);
        assert_eq!(window.entries[4], Entry::Other);
        assert_eq!(window.pending_ai.unwrap().unwrap().blocks.len(), 1);
        // A null live tail is not an absent one.
        let Event::HistoryUpdated { window, .. } = parse_event(
            r#"{"type":"history_updated","convId":"c1","entries":[],"pendingAI":null}"#,
        ) else {
            panic!("not updated");
        };
        assert_eq!(window.pending_ai, Some(None));
        let Event::HistoryUpdated { window, .. } =
            parse_event(r#"{"type":"history_updated","convId":"c1","entries":[]}"#)
        else {
            panic!("not updated");
        };
        assert_eq!(window.pending_ai, None);
        assert_eq!(reply_text(blocks), "**Done**");
    }

    #[test]
    fn streaming_events_and_unknown_types() {
        assert_eq!(
            parse_event(r#"{"type":"text_chunk","convId":"c","streamSeq":3,"text":"ab"}"#),
            Event::TextChunk {
                conv_id: "c".into(),
                text: "ab".into()
            }
        );
        assert_eq!(
            parse_event(r#"{"type":"usage_update","provider":"x","usage":null}"#),
            Event::Other
        );
        assert_eq!(parse_event("not json"), Event::Other);
        // A known type with a malformed required field is dropped, not fatal.
        assert_eq!(parse_event(r#"{"type":"text_chunk"}"#), Event::Other);
        let Event::ToolsAvailable {
            providers,
            auth_by_provider,
            conversation_defaults,
            ..
        } = parse_event(
            r#"{"type":"tools_available","providers":[{"id":"anthropic","label":"Anthropic","defaultModel":"o","allowsCustomModels":false,"supportsFastMode":true,"models":[{"id":"o","label":"Opus","maxContext":1,"supportedEfforts":[{"effort":"low","description":"d"}],"defaultEffort":"high"}]}],"tools":[],"authByProvider":{"anthropic":true},"authInfoByProvider":{},"conversationDefaults":{"defaults":{"provider":"openai","model":"g","effort":"low","fastMode":false},"configured":false}}"#,
        )
        else {
            panic!("not tools");
        };
        assert_eq!(providers[0].models[0].supported_efforts[0].effort, "low");
        assert_eq!(auth_by_provider.get("anthropic"), Some(&true));
        assert_eq!(conversation_defaults.unwrap().defaults.model, "g");
    }

    #[test]
    fn commands_carry_the_documented_fields() {
        let command = load_newest("r", "c", 1);
        assert_eq!(command["type"], "load_conversation_history");
        assert!(command["beforeEntryIndex"].as_u64().unwrap() > 1 << 30);
        assert!(command.get("beforeBlockIndex").is_none());
        assert_eq!(load_history("r", "c", 5, 2, 3)["beforeBlockIndex"], 2);
        let selection = Selection {
            provider: "anthropic".into(),
            model: "claude-haiku-5-5".into(),
            effort: "low".into(),
        };
        let created = new_conversation("r", "c", &selection, "Hi", 9);
        assert_eq!(created["initialMessage"]["text"], "Hi");
        assert_eq!(created["effort"], "low");
        assert_eq!(abort("c", Some(12.0))["expectedStartedAt"], 12);
        assert!(abort("c", None).get("expectedStartedAt").is_none());
        let load = load_conversation("r", "c", "b");
        assert_eq!(load["initialBackfill"]["reqId"], "b");
        assert!(load["turns"].is_u64() && load["viewport"]["rows"].is_u64());
        let caps = client_capabilities(&APP_CAPABILITIES, Some(20));
        assert_eq!(caps["sidebarView"]["folderView"], "navigate");
    }
}
