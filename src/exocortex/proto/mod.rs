//! The Exocortex daemon's wire protocol, as far as the app and the
//! notification bridge use it (exocortex `shared/src/protocol.ts` is the
//! spec): newline-delimited JSON over a Unix socket, commands from the
//! client, events from the daemon. Parsing is lenient: unknown events and
//! fields are ignored, missing optional fields take their defaults, so a newer
//! daemon never breaks an older phone.
#![allow(dead_code)]

pub mod chat;
pub mod composer;
pub mod ops;
pub mod sidebar;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

pub use chat::*;
pub use composer::*;
pub use ops::*;
pub use sidebar::*;

/// Where the phone's daemon (root, in the chroot, checkout `/opt/exocortex`)
/// listens. `EXOCORTEX_SOCKET` overrides it.
pub const DEFAULT_SOCKET: &str = "/opt/exocortex/config/runtime/exocortexd.sock";

pub fn socket_path() -> PathBuf {
    std::env::var_os("EXOCORTEX_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))
}

/// A field that may be `null`: present (even as null) is `Some`.
pub(crate) fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
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
