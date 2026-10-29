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

/// Every event the app, its packages and the bridge use (protocol.ts
/// `Event`, the roadmap's §2): parsed leniently, everything else is
/// `Other`. Events with many fields carry a payload struct from the area
/// that owns them (`proto/{sidebar,chat,composer,ops}.rs`), so an area adds
/// fields to its own file.
// Windows are big and events are moved once, from the link thread to
// their handler: boxing them would cost an allocation for nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    // ----- Connection (A) --------------------------------------------------
    /// The answer to the bootstrap `ping`: the connection is ready.
    Pong {
        #[serde(default)]
        req_id: Option<String>,
        #[serde(default)]
        update_status: Option<Value>,
    },
    /// The daemon's update status (with `update-status-push`).
    UpdateStatus {
        #[serde(default)]
        status: Value,
    },
    /// A graceful shutdown follows (`stop` or `restart`).
    DaemonShutdown {
        #[serde(default)]
        mode: Option<String>,
    },
    Ack {
        #[serde(default)]
        req_id: Option<String>,
        #[serde(default)]
        conv_id: Option<String>,
    },
    /// The daemon's network monitor (one per daemon).
    ConnectivityStatus {
        #[serde(default)]
        online: bool,
        #[serde(default)]
        offline_since: Option<f64>,
        #[serde(default)]
        next_probe_at: Option<f64>,
    },
    Error {
        #[serde(default)]
        req_id: Option<String>,
        #[serde(default)]
        conv_id: Option<String>,
        #[serde(default)]
        message: String,
    },

    // ----- Models, defaults, usage, accounts (E) --------------------------
    ToolsAvailable {
        #[serde(default)]
        providers: Vec<Provider>,
        #[serde(default)]
        tools: Vec<ToolStyle>,
        #[serde(default)]
        auth_by_provider: HashMap<String, bool>,
        #[serde(default)]
        auth_info_by_provider: AuthByProvider,
        #[serde(default)]
        external_tool_styles: Vec<ExternalToolStyle>,
        /// The daemon host's paths for macro expansion (D).
        #[serde(default)]
        macro_environment: Option<Value>,
        #[serde(default)]
        conversation_defaults: Option<DefaultsSnapshot>,
    },
    ConversationDefaults {
        #[serde(default)]
        req_id: Option<String>,
        #[serde(default)]
        defaults: Selection,
        #[serde(default)]
        configured: bool,
        /// A mutation's confirmation (to the requesting client only).
        #[serde(default)]
        message: Option<String>,
    },
    UsageUpdate(UsageUpdate),
    Accounts(Accounts),
    UsageResetResult(UsageReset),
    TokenStats(TokenStats),
    GoalUpdated(GoalUpdated),
    ChronoResult(ChronoResult),
    TasksList(TasksList),
    TaskStopped(TaskStopped),
    BtwStarted(Btw),
    BtwFollowupStarted(Btw),
    BtwBlockStart(Btw),
    BtwTextChunk(Btw),
    BtwThinkingChunk(Btw),
    BtwContent(Btw),
    BtwToolCall(Btw),
    BtwToolResult(Btw),
    BtwStatus(Btw),
    BtwFinished(Btw),
    BtwError(Btw),
    BtwClosed(Btw),
    BtwMutationSettled(Btw),
    BtwSnapshot(BtwSnapshot),
    InstructionsLoaded(InstructionsLoaded),
    GlobalInstructionsUpdated(GlobalInstructions),
    SystemPrompt(SystemPrompt),
    AuthStatus(AuthStatus),

    // ----- The conversation list (B) ---------------------------------------
    ConversationsList {
        #[serde(default)]
        req_id: Option<String>,
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
        #[serde(default)]
        req_id: Option<String>,
        summary: Summary,
    },
    ConversationUnwound {
        #[serde(default)]
        req_id: Option<String>,
        conv_id: String,
        summary: Summary,
        /// The user message index the unwind removed.
        #[serde(default)]
        user_message_index: u64,
        #[serde(default)]
        history_total_entries: u64,
        #[serde(default)]
        context_tokens: Option<u64>,
    },
    ConversationDeleted {
        #[serde(default)]
        req_id: Option<String>,
        conv_id: String,
    },
    ConversationMarked(Marked),
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
        #[serde(default)]
        fast_mode: Value,
    },
    FolderInstructionsLoaded(FolderInstructions),
    FolderInstructionsUpdated(FolderInstructions),

    // ----- A conversation: history and stream (C1, C2) ----------------------
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
        #[serde(default)]
        fast_mode: Value,
        /// Messages queued for it (shadows).
        #[serde(default)]
        queued_messages: Vec<Queued>,
        #[serde(default)]
        goal: Option<Goal>,
        #[serde(default)]
        btw: Option<Value>,
        #[serde(flatten)]
        window: Window,
    },
    ConversationHistoryLoaded {
        #[serde(default)]
        req_id: Option<String>,
        conv_id: String,
        /// `initial-backfill` or `viewport`.
        #[serde(default)]
        request_source: Option<String>,
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
        provider: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        started_at: f64,
        /// `start`, `catchup` or `heartbeat`.
        #[serde(default)]
        snapshot_kind: Option<String>,
        #[serde(default)]
        work_timer_started_at: Option<f64>,
        #[serde(default)]
        blocks: Option<Vec<Block>>,
        #[serde(default)]
        block_offset: Option<u64>,
        #[serde(default)]
        tokens: Option<u64>,
        #[serde(default)]
        generation_throughput: Option<Throughput>,
        #[serde(default)]
        compaction_started_at: Option<f64>,
        #[serde(default)]
        connection_wait_started_at: Option<f64>,
        #[serde(default)]
        connection_wait_label: Option<String>,
    },
    StreamingStopped {
        conv_id: String,
        /// `daemon-restart`, `handoff`, `unwind` or `suspended`.
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
        #[serde(default)]
        presentation: Option<Presentation>,
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
        #[serde(default)]
        images: Vec<ImageRef>,
    },
    /// The live reply's output tokens so far.
    TokensUpdate {
        conv_id: String,
        #[serde(default)]
        tokens: u64,
        #[serde(default)]
        generation_throughput: Option<Throughput>,
    },
    ContextUpdate {
        conv_id: String,
        #[serde(default)]
        context_tokens: u64,
    },
    MessageComplete {
        conv_id: String,
        #[serde(default)]
        blocks: Vec<Block>,
        #[serde(default)]
        ended_at: Option<f64>,
        #[serde(default)]
        tokens: u64,
        #[serde(default)]
        generation_throughput: Option<Throughput>,
    },
    UserMessage {
        conv_id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        started_at: Option<f64>,
        #[serde(default)]
        images: Vec<ImageAttachment>,
        #[serde(default)]
        queue_id: Option<String>,
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
        #[serde(default)]
        delay_sec: f64,
        /// `transient` or `usage_limit_reset`.
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        reset_at: Option<f64>,
    },
    ContextCompactionStatus {
        conv_id: String,
        #[serde(default)]
        active: bool,
        #[serde(default)]
        started_at: Option<f64>,
        /// Finished: clients keep a divider.
        #[serde(default)]
        completed_at: Option<f64>,
    },
    ConnectionWaitStatus {
        conv_id: String,
        #[serde(default)]
        active: bool,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        started_at: Option<f64>,
    },
    ToolOutputsLoaded {
        #[serde(default)]
        req_id: Option<String>,
        conv_id: String,
        #[serde(default)]
        outputs: Vec<ToolOutput>,
    },
    DisplayBlocksLoaded(DisplayBlocks),
    ImageData(ImageData),
    FileLinkResolved(FileLink),
    SystemInstructionsUpdated(SystemInstructions),

    // ----- Sending and the queue (D) ----------------------------------------
    QueueUpdated {
        #[serde(default)]
        messages: Vec<Queued>,
        #[serde(default)]
        settled_queue_ids: SettledIds,
    },
    QueueNotice(QueueNotice),
    TranscriptionResult(Transcription),
    UserMacros(UserMacros),
    /// `path_directory_entries` (D: path completion).
    PathDirectoryEntries(PathEntries),

    #[serde(other)]
    Other,
}

impl Event {
    /// The request this event answers, if it says.
    pub fn req_id(&self) -> Option<&str> {
        let id = match self {
            Event::Pong { req_id, .. }
            | Event::Ack { req_id, .. }
            | Event::Error { req_id, .. }
            | Event::ConversationDefaults { req_id, .. }
            | Event::ConversationsList { req_id, .. }
            | Event::ConversationRestored { req_id, .. }
            | Event::ConversationUnwound { req_id, .. }
            | Event::ConversationDeleted { req_id, .. }
            | Event::ConversationCreated { req_id, .. }
            | Event::ConversationLoaded { req_id, .. }
            | Event::ConversationHistoryLoaded { req_id, .. }
            | Event::ToolOutputsLoaded { req_id, .. } => req_id,
            Event::Accounts(e) => &e.req_id,
            Event::UsageResetResult(e) => &e.req_id,
            Event::TokenStats(e) => &e.req_id,
            Event::GoalUpdated(e) => &e.req_id,
            Event::ChronoResult(e) => &e.req_id,
            Event::TasksList(e) => &e.req_id,
            Event::TaskStopped(e) => &e.req_id,
            Event::InstructionsLoaded(e) => &e.req_id,
            Event::GlobalInstructionsUpdated(e) => &e.req_id,
            Event::SystemPrompt(e) => &e.req_id,
            Event::AuthStatus(e) => &e.req_id,
            Event::FolderInstructionsLoaded(e) | Event::FolderInstructionsUpdated(e) => &e.req_id,
            Event::DisplayBlocksLoaded(e) => &e.req_id,
            Event::ImageData(e) => &e.req_id,
            Event::FileLinkResolved(e) => &e.req_id,
            Event::TranscriptionResult(e) => &e.req_id,
            Event::UserMacros(e) => &e.req_id,
            Event::PathDirectoryEntries(e) => &e.req_id,
            _ => return None,
        };
        id.as_deref()
    }

    /// The conversation this event is about, if one.
    pub fn conv_id(&self) -> Option<&str> {
        match self {
            Event::ConversationUpdated { summary, .. }
            | Event::ConversationRestored { summary, .. } => Some(&summary.id),
            Event::ConversationUnwound { conv_id, .. }
            | Event::ConversationDeleted { conv_id, .. }
            | Event::ConversationPinned { conv_id, .. }
            | Event::ConversationCreated { conv_id, .. }
            | Event::ConversationLoaded { conv_id, .. }
            | Event::ConversationHistoryLoaded { conv_id, .. }
            | Event::HistoryUpdated { conv_id, .. }
            | Event::StreamingStarted { conv_id, .. }
            | Event::StreamingStopped { conv_id, .. }
            | Event::BlockStart { conv_id, .. }
            | Event::TextChunk { conv_id, .. }
            | Event::ThinkingChunk { conv_id, .. }
            | Event::StreamingSync { conv_id, .. }
            | Event::ToolCall { conv_id, .. }
            | Event::ToolResult { conv_id, .. }
            | Event::TokensUpdate { conv_id, .. }
            | Event::ContextUpdate { conv_id, .. }
            | Event::MessageComplete { conv_id, .. }
            | Event::UserMessage { conv_id, .. }
            | Event::SystemMessage { conv_id, .. }
            | Event::StreamRetry { conv_id, .. }
            | Event::ContextCompactionStatus { conv_id, .. }
            | Event::ConnectionWaitStatus { conv_id, .. }
            | Event::ToolOutputsLoaded { conv_id, .. } => Some(conv_id),
            Event::Ack { conv_id, .. } | Event::Error { conv_id, .. } => conv_id.as_deref(),
            Event::ConversationMarked(e) => Some(&e.conv_id),
            Event::GoalUpdated(e) => Some(&e.conv_id),
            Event::ChronoResult(e) => Some(&e.conv_id),
            Event::TaskStopped(e) => Some(&e.conv_id),
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
            | Event::BtwMutationSettled(e) => Some(&e.conv_id),
            Event::BtwSnapshot(e) => Some(&e.conv_id),
            Event::DisplayBlocksLoaded(e) => Some(&e.conv_id),
            Event::FileLinkResolved(e) => Some(&e.conv_id),
            Event::SystemInstructionsUpdated(e) => Some(&e.conv_id),
            Event::QueueNotice(e) => e.conv_id.as_deref(),
            _ => None,
        }
    }
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
            parse_event(r#"{"type":"provider_proxies","proxies":[]}"#),
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
            ..Selection::default()
        };
        let created = new_conversation("r", "c", Some(&selection), Some("f1"), "Hi", 9);
        assert_eq!(created["initialMessage"]["text"], "Hi");
        assert_eq!(created["effort"], "low");
        assert_eq!(created["folderId"], "f1");
        // A blank draft sends no model choices: the daemon's defaults apply.
        let blank = new_conversation("r", "c", None, None, "Hi", 9);
        for key in ["provider", "model", "effort", "fastMode", "folderId"] {
            assert!(blank.get(key).is_none(), "{key}");
        }
        assert_eq!(abort("r", "c", Some(12.0))["expectedStartedAt"], 12);
        assert!(abort("r", "c", None).get("expectedStartedAt").is_none());
        let load = load_conversation("r", "c", "b");
        assert_eq!(load["initialBackfill"]["reqId"], "b");
        assert!(load["turns"].is_u64() && load["viewport"]["rows"].is_u64());
        let caps = client_capabilities(&APP_CAPABILITIES, Some(20));
        assert_eq!(caps["sidebarView"]["folderView"], "navigate");
    }

    /// Every event this client parses, one per line as protocol.ts shapes
    /// it (`fixtures.jsonl`): each parses as itself, not `Other`, with its
    /// fields.
    #[test]
    fn every_event_of_the_surface_parses() {
        let mut seen = Vec::new();
        for line in include_str!("fixtures.jsonl").lines() {
            let event = parse_event(line);
            assert_ne!(event, Event::Other, "{line}");
            let name = format!("{event:?}");
            let name = name
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap()
                .to_owned();
            seen.push(name);
        }
        for variant in [
            "Pong",
            "UpdateStatus",
            "DaemonShutdown",
            "Ack",
            "ConnectivityStatus",
            "Error",
            "ToolsAvailable",
            "ConversationDefaults",
            "UsageUpdate",
            "Accounts",
            "UsageResetResult",
            "TokenStats",
            "GoalUpdated",
            "ChronoResult",
            "TasksList",
            "TaskStopped",
            "BtwStarted",
            "BtwFollowupStarted",
            "BtwBlockStart",
            "BtwTextChunk",
            "BtwThinkingChunk",
            "BtwContent",
            "BtwToolCall",
            "BtwToolResult",
            "BtwStatus",
            "BtwFinished",
            "BtwError",
            "BtwClosed",
            "BtwMutationSettled",
            "BtwSnapshot",
            "InstructionsLoaded",
            "GlobalInstructionsUpdated",
            "SystemPrompt",
            "AuthStatus",
            "ConversationsList",
            "ConversationUpdated",
            "ConversationRestored",
            "ConversationUnwound",
            "ConversationDeleted",
            "ConversationMarked",
            "ConversationPinned",
            "ConversationMoved",
            "SidebarItemsReordered",
            "SidebarStatePatched",
            "ConversationCreated",
            "FolderInstructionsLoaded",
            "FolderInstructionsUpdated",
            "ConversationLoaded",
            "ConversationHistoryLoaded",
            "HistoryUpdated",
            "StreamingStarted",
            "StreamingStopped",
            "BlockStart",
            "TextChunk",
            "ThinkingChunk",
            "StreamingSync",
            "ToolCall",
            "ToolResult",
            "TokensUpdate",
            "ContextUpdate",
            "MessageComplete",
            "UserMessage",
            "SystemMessage",
            "StreamRetry",
            "ContextCompactionStatus",
            "ConnectionWaitStatus",
            "ToolOutputsLoaded",
            "DisplayBlocksLoaded",
            "ImageData",
            "FileLinkResolved",
            "SystemInstructionsUpdated",
            "QueueUpdated",
            "QueueNotice",
            "TranscriptionResult",
            "UserMacros",
            "PathDirectoryEntries",
        ] {
            assert!(
                seen.iter().any(|name| name == variant),
                "no fixture for {variant}"
            );
        }
        // Spot checks of what the packages read.
        let parse = |kind: &str| {
            include_str!("fixtures.jsonl")
                .lines()
                .find(|line| event_type(line) == Some(kind))
                .map(parse_event)
                .unwrap()
        };
        let Event::UsageUpdate(usage) = parse("usage_update") else {
            panic!()
        };
        let usage = usage.usage.unwrap();
        assert_eq!(usage.capacity, Some(200.0));
        assert_eq!(usage.five_hour.unwrap().resets.len(), 2);
        let Event::ConversationLoaded {
            window,
            queued_messages,
            ..
        } = parse("conversation_loaded")
        else {
            panic!()
        };
        assert_eq!(window.context_tokens, Some(1234));
        assert_eq!(queued_messages[0].timing, "next-turn");
        let Entry::Ai { blocks, metadata } = &window.entries[1] else {
            panic!()
        };
        assert_eq!(metadata.as_ref().unwrap().tokens, 42);
        assert!(matches!(&blocks[0], Block::Thinking { elided: Some(e), .. } if e.entry == 1));
        assert!(
            matches!(&blocks[1], Block::ToolCall { presentation: Some(p), .. } if p.bash_styles[0].label == "Gmail")
        );
        assert!(
            matches!(&blocks[2], Block::ToolResult { images, .. } if images[0].name.as_deref() == Some("shot.png"))
        );
        let Entry::User {
            images, metadata, ..
        } = &window.entries[0]
        else {
            panic!()
        };
        assert_eq!(images[0].image_id.as_deref(), Some("img1"));
        assert!(metadata.as_ref().unwrap().automation.is_some());
        let Event::ConversationsList { conversations, .. } = parse("conversations_list") else {
            panic!()
        };
        let summary = &conversations[0];
        assert_eq!(summary.fast_mode, serde_json::json!("ultrafast"));
        assert!(summary.provider_waiting && summary.marked);
        assert_eq!(summary.tasks[0].chrono_mode.as_deref(), Some("wake"));
        let Event::StreamRetry { kind, reset_at, .. } = parse("stream_retry") else {
            panic!()
        };
        assert_eq!(kind.as_deref(), Some("usage_limit_reset"));
        assert!(reset_at.is_some());
        let Event::ToolsAvailable {
            auth_info_by_provider,
            external_tool_styles,
            ..
        } = parse("tools_available")
        else {
            panic!()
        };
        assert_eq!(auth_info_by_provider["anthropic"].status, "logged_in");
        assert_eq!(external_tool_styles[0].cmd, "gmail");
        // Who an answer is for, and what it is about.
        assert_eq!(parse("ack").req_id(), Some("app-7"));
        assert_eq!(parse("goal_updated").conv_id(), Some("c1"));
        assert_eq!(parse("btw_text_chunk").conv_id(), Some("c1"));
        assert_eq!(parse("image_data").req_id(), Some("app-32"));
    }
}
