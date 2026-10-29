//! The Exocortex app's state: the daemon's conversation list (folders,
//! pinned and recent order as the TUI shows them), the open conversation's
//! history window and live reply, the model choices, and what the UI asks the
//! daemon for. Events in, commands out; no I/O here, so all of it is tested.

pub mod chat;
pub mod composer;
pub mod ops;
pub mod sidebar;

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;

use super::proto::{self, Event, Provider, Queued, Selection, Summary};

pub use chat::{Chat, Echo, Live};
pub use sidebar::{Row, Sidebar};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    List,
    Chat,
}

/// Something the user did.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    OpenFolder(String),
    OpenConversation(String),
    NewConversation,
    Back,
    Send,
    Abort,
    ToggleExpanded(String),
    ShowPicker,
    HidePicker,
    PickModel(String, String),
    PickEffort(String),
}

pub struct Model {
    pub connected: bool,
    /// "Reconnecting…" shows: disconnected, and not for a quiet reconnection
    /// that has not taken long yet.
    pub offline: bool,
    /// A reconnection the app asked for (back from the background) keeps
    /// what is on screen until then, while the new connection catches up.
    quiet_until: Option<Instant>,
    /// Ever connected: "Connecting…" is said differently the first time.
    pub was_connected: bool,
    pub sidebar: Sidebar,
    pub page: Page,
    /// The folder path of the list, innermost last.
    pub folders: Vec<String>,
    pub chat: Chat,
    pub providers: Vec<Provider>,
    pub tool_styles: HashMap<String, proto::ToolStyle>,
    pub auth: HashMap<String, bool>,
    pub defaults: Selection,
    /// The model a new conversation will use.
    pub draft_selection: Option<Selection>,
    pub queued: Vec<Queued>,
    pub composer: String,
    pub drafts: HashMap<String, String>,
    pub picker: bool,
    next_request: u64,
    /// Requests whose answer opens the chat (load) or replaces it.
    load_request: Option<String>,
    /// When it was sent (it is given up on after `ANSWER_WITHIN`).
    load_sent: Option<Instant>,
    refresh_request: Option<String>,
    /// Conversations opened before, most recent last: reopening one shows
    /// it at once while the daemon's fresh window is on its way.
    shelf: Vec<Chat>,
    /// Bumped when tool styles change (they label tool rows).
    pub styles_version: u64,
    /// A conversation to open once connected (a notification's target).
    pub pending_open: Option<String>,
    pub notice: Option<(String, Instant)>,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

/// A conversation's window, or an older page, that has not come in this
/// long is given up on: the chat says so (a late one still lands), and
/// scrolling up asks again.
pub const ANSWER_WITHIN: std::time::Duration = std::time::Duration::from_secs(15);

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Model {
    pub fn new() -> Self {
        Self {
            connected: false,
            offline: true,
            quiet_until: None,
            was_connected: false,
            sidebar: Sidebar::default(),
            page: Page::List,
            folders: Vec::new(),
            chat: Chat::default(),
            providers: Vec::new(),
            tool_styles: HashMap::new(),
            auth: HashMap::new(),
            defaults: Selection::default(),
            draft_selection: None,
            queued: Vec::new(),
            composer: String::new(),
            drafts: HashMap::new(),
            picker: false,
            next_request: 0,
            load_request: None,
            load_sent: None,
            refresh_request: None,
            shelf: Vec::new(),
            styles_version: 0,
            pending_open: None,
            notice: None,
        }
    }

    fn request_id(&mut self) -> String {
        self.next_request += 1;
        format!("app-{}", self.next_request)
    }

    pub fn current_folder(&self) -> Option<&str> {
        self.folders.last().map(String::as_str)
    }

    pub fn open_summary(&self) -> Option<&Summary> {
        self.chat
            .id
            .as_ref()
            .and_then(|id| self.sidebar.conversations.get(id))
    }

    /// The open conversation is working (streaming, or a handoff between
    /// queued turns).
    pub fn streaming(&self) -> bool {
        self.open_summary().is_some_and(|s| s.streaming) || self.chat.live.is_some()
    }

    /// The open conversation's window is still on its way (what shows is
    /// what was kept from before).
    pub fn chat_pending(&self) -> bool {
        self.load_request.is_some()
    }

    /// The conversation shown in the chat, if any, for the bridge.
    pub fn shown_conversation(&self) -> Option<&str> {
        match self.page {
            Page::Chat => self.chat.id.as_deref(),
            Page::List => None,
        }
    }

    /// The bootstrap, sent on every (re)connection.
    pub fn on_connected(&mut self) -> Vec<Value> {
        self.connected = true;
        self.offline = false;
        self.quiet_until = None;
        self.was_connected = true;
        let mut commands = vec![proto::client_capabilities(
            &proto::APP_CAPABILITIES,
            Some(24),
        )];
        // The open conversation comes back first, then the list (PAR-018),
        // all in the same write.
        let reopen = self.pending_open.take().or_else(|| match self.page {
            Page::Chat => self.chat.id.clone(),
            Page::List => None,
        });
        if let Some(id) = reopen {
            commands.extend(self.open(id));
        }
        commands.push(proto::ping(&format!("ping-{}", now_ms())));
        commands
    }

    pub fn on_disconnected(&mut self) {
        self.connected = false;
        self.offline = self.quiet_until.is_none();
        self.load_request = None;
        self.load_sent = None;
        self.refresh_request = None;
        self.chat.older_request = None;
        self.chat.older_sent = None;
        self.chat.outputs_requested.clear();
        // The live tail is unknown until the conversation is reloaded (a
        // quiet reconnection keeps showing it until then).
        if self.quiet_until.is_none() {
            self.chat.live = None;
        }
    }

    /// The app is about to reconnect on purpose: what is on screen stays,
    /// without "Reconnecting…", for up to `grace`.
    pub fn reconnect_quietly(&mut self, now: Instant, grace: std::time::Duration) {
        self.quiet_until = Some(now + grace);
    }

    /// When a quiet reconnection stops being quiet.
    pub fn quiet_deadline(&self) -> Option<Instant> {
        self.quiet_until.filter(|_| !self.connected)
    }

    /// Updates `offline` for the time; returns whether it changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let offline = !self.connected && self.quiet_until.is_none_or(|until| now >= until);
        let changed = offline != self.offline;
        self.offline = offline;
        changed
    }

    pub fn handle(&mut self, action: Action) -> Vec<Value> {
        match action {
            Action::OpenFolder(id) => {
                self.folders.push(id);
                Vec::new()
            }
            Action::OpenConversation(id) => self.open(id),
            Action::NewConversation => {
                self.save_draft();
                let previous = self.chat.id.clone();
                let draft = Chat {
                    loaded: true,
                    ..Chat::default()
                };
                let old = std::mem::replace(&mut self.chat, draft);
                self.shelve(old);
                self.load_request = None;
                self.draft_selection = None;
                self.page = Page::Chat;
                previous
                    .map(|id| proto::unsubscribe(&id))
                    .into_iter()
                    .collect()
            }
            Action::Back => {
                self.picker = false;
                match self.page {
                    Page::Chat => {
                        self.save_draft();
                        self.page = Page::List;
                        let id = self.chat.id.clone();
                        let old = std::mem::take(&mut self.chat);
                        self.shelve(old);
                        self.load_request = None;
                        // Not viewing it any more: the daemon may mark it unread.
                        id.filter(|_| self.connected)
                            .map(|id| proto::unsubscribe(&id))
                            .into_iter()
                            .collect()
                    }
                    Page::List => {
                        self.folders.pop();
                        Vec::new()
                    }
                }
            }
            Action::Send => self.send(),
            Action::Abort => match (&self.chat.id, self.connected) {
                (Some(id), true) => {
                    let started = self.chat.live.as_ref().map(|live| live.started_at);
                    vec![proto::abort(id, started)]
                }
                _ => Vec::new(),
            },
            Action::ToggleExpanded(key) => {
                let mut commands = Vec::new();
                if !self.chat.expanded.remove(&key) {
                    self.chat.expanded.insert(key.clone());
                    if let Some(tool) = key.strip_prefix("tool:")
                        && self.wants_output(tool)
                        && let (Some(id), true) = (self.chat.id.clone(), self.connected)
                    {
                        self.chat.outputs_requested.insert(tool.to_owned());
                        let request = self.request_id();
                        commands.push(proto::load_tool_outputs(&request, &id, &[tool.to_owned()]));
                    }
                }
                self.chat.touch();
                commands
            }
            Action::ShowPicker => {
                self.picker = true;
                Vec::new()
            }
            Action::HidePicker => {
                self.picker = false;
                Vec::new()
            }
            Action::PickModel(provider, model) => {
                self.picker = false;
                let effort = self
                    .providers
                    .iter()
                    .find(|p| p.id == provider)
                    .and_then(|p| p.models.iter().find(|m| m.id == model))
                    .map(|m| m.default_effort.clone())
                    .unwrap_or_default();
                match (&self.chat.id, self.connected) {
                    (Some(id), true) => vec![proto::set_model(id, &provider, &model)],
                    (Some(_), false) => Vec::new(),
                    (None, _) => {
                        self.draft_selection = Some(Selection {
                            provider,
                            model,
                            effort,
                        });
                        Vec::new()
                    }
                }
            }
            Action::PickEffort(effort) => match (&self.chat.id, self.connected) {
                (Some(id), true) => vec![proto::set_effort(id, &effort)],
                (Some(_), false) => Vec::new(),
                (None, _) => {
                    let mut selection = self.selection();
                    selection.effort = effort;
                    self.draft_selection = Some(selection);
                    Vec::new()
                }
            },
        }
    }

    /// Whether an event can change what is on screen (a chat is not
    /// redrawn for other conversations' sidebar traffic).
    pub fn affects_view(&self, event: &Event) -> bool {
        if self.page == Page::List || self.picker {
            return true;
        }
        let open = self.chat.id.as_deref();
        match event {
            Event::ConversationUpdated { summary, .. }
            | Event::ConversationRestored { summary } => Some(summary.id.as_str()) == open,
            Event::ConversationsList { .. }
            | Event::ConversationMoved { .. }
            | Event::SidebarItemsReordered { .. }
            | Event::SidebarStatePatched { .. }
            | Event::ConversationPinned { .. }
            | Event::ConnectivityStatus { .. }
            | Event::Pong { .. } => false,
            Event::ConversationDeleted { conv_id } => Some(conv_id.as_str()) == open,
            _ => true,
        }
    }

    /// Applies one daemon event; returns commands it calls for.
    pub fn apply(&mut self, event: Event, now: Instant) -> Vec<Value> {
        self.sidebar.apply(&event);
        match event {
            Event::ToolsAvailable {
                providers,
                tools,
                auth_by_provider,
                conversation_defaults,
            } => {
                let styles: HashMap<String, proto::ToolStyle> = tools
                    .into_iter()
                    .map(|tool| (tool.name.to_ascii_lowercase(), tool))
                    .collect();
                if styles != self.tool_styles {
                    self.tool_styles = styles;
                    self.styles_version += 1;
                    self.chat.touch();
                }
                self.providers = providers;
                self.auth = auth_by_provider;
                if let Some(snapshot) = conversation_defaults {
                    self.defaults = snapshot.defaults;
                }
                Vec::new()
            }
            Event::ConversationDefaults { defaults } => {
                self.defaults = defaults;
                Vec::new()
            }
            Event::QueueUpdated { messages } => {
                self.queued = messages;
                self.chat.touch();
                Vec::new()
            }
            Event::ConversationDeleted { conv_id } => {
                self.shelf
                    .retain(|chat| chat.id.as_deref() != Some(&conv_id));
                if self.chat.id.as_deref() == Some(&conv_id) {
                    self.page = Page::List;
                    self.chat = Chat::default();
                }
                Vec::new()
            }
            Event::ConversationLoaded {
                req_id,
                conv_id,
                window,
                ..
            } => {
                if self.chat.id.as_deref() != Some(&conv_id) {
                    return Vec::new();
                }
                let backfill = window.initial_backfill_req_id.clone();
                let streaming = matches!(window.pending_ai, Some(Some(_)));
                let fresh = req_id.is_some() && req_id == self.load_request;
                if fresh {
                    self.load_request = None;
                    self.load_sent = None;
                    self.chat.live = None;
                    self.chat.echoes.clear();
                    self.chat.error = None;
                }
                let before = (self.chat.start, self.chat.start_block);
                self.chat.splice(window, fresh);
                if fresh {
                    // The older page follows, unless the history from before
                    // was kept: it would not join on, so it is not waited for.
                    let kept = (self.chat.start, self.chat.start_block) == before;
                    self.chat.older_request = if kept { None } else { backfill };
                    if self.chat.older_request.is_none() {
                        self.chat.older_sent = None;
                    }
                }
                if !streaming && !self.open_summary().is_some_and(|s| s.streaming) {
                    self.chat.live = None;
                }
                Vec::new()
            }
            Event::ConversationHistoryLoaded {
                req_id,
                conv_id,
                window,
            } => {
                if self.chat.id.as_deref() != Some(&conv_id) {
                    return Vec::new();
                }
                if req_id.is_some() && req_id == self.chat.older_request {
                    self.chat.older_request = None;
                    self.chat.older_sent = None;
                    if !self.chat.prepend(window) {
                        // The history moved under the page: start over.
                        return self.reload();
                    }
                } else if req_id.is_some() && req_id == self.refresh_request {
                    self.refresh_request = None;
                    let live = self.chat.live.take();
                    self.chat.splice(window, false);
                    if self.open_summary().is_some_and(|s| s.streaming) {
                        self.chat.live = live;
                    }
                }
                Vec::new()
            }
            Event::HistoryUpdated { conv_id, window } => {
                if self.chat.id.as_deref() == Some(&conv_id) && self.chat.loaded {
                    let had_live = self.chat.live.is_some();
                    let pending = window.pending_ai.clone();
                    self.chat.splice(window, false);
                    match pending {
                        // The daemon says the turn is over.
                        Some(None) => self.chat.live = None,
                        None if had_live && !self.open_summary().is_some_and(|s| s.streaming) => {
                            self.chat.live = None;
                        }
                        _ => {}
                    }
                }
                Vec::new()
            }
            Event::ConversationUnwound { conv_id, .. } => {
                self.shelf
                    .retain(|chat| chat.id.as_deref() != Some(&conv_id));
                if self.chat.id.as_deref() == Some(&conv_id) {
                    self.reload()
                } else {
                    Vec::new()
                }
            }
            Event::ToolOutputsLoaded { conv_id, outputs } => {
                if self.chat.id.as_deref() == Some(&conv_id) {
                    for output in outputs {
                        self.chat.outputs.insert(output.tool_call_id, output.output);
                    }
                    self.chat.touch();
                }
                Vec::new()
            }
            Event::Error {
                req_id,
                conv_id,
                message,
            } => {
                if req_id.is_some() && req_id == self.load_request {
                    self.load_request = None;
                    self.load_sent = None;
                    self.chat.error = Some(message.clone());
                    self.chat.loaded = true;
                    self.chat.touch();
                } else if req_id.is_some() && req_id == self.chat.older_request {
                    self.chat.older_request = None;
                    self.chat.older_sent = None;
                    self.chat.has_older = false;
                } else if conv_id.is_some() && conv_id == self.chat.id {
                    self.notice = Some((message, now));
                }
                Vec::new()
            }
            event => self.apply_stream(event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::proto::{Block, Entry};

    fn summary(id: &str, pinned: bool, sort: f64, folder: Option<&str>) -> String {
        format!(
            r#"{{"id":"{id}","provider":"anthropic","model":"m","effort":"low","title":"{id}","pinned":{pinned},"sortOrder":{sort},"folderId":{},"streaming":false,"unread":false}}"#,
            folder.map_or("null".into(), |f| format!("\"{f}\""))
        )
    }

    fn connected() -> Model {
        let mut model = Model::new();
        model.on_connected();
        model
    }

    #[test]
    fn the_list_orders_pinned_then_sort_order_with_folders() {
        let mut model = connected();
        let list = format!(
            r#"{{"type":"conversations_list","conversations":[{},{},{},{}],"folders":[{{"id":"f1","name":"Work","parentId":null,"pinned":false,"sortOrder":0}}]}}"#,
            summary("a", false, 2.0, None),
            summary("b", true, 5.0, None),
            summary("c", false, -1.0, None),
            summary("d", false, 0.0, Some("f1")),
        );
        model.apply(parse_event(&list), Instant::now());
        assert_eq!(
            model.sidebar.rows(None),
            &[
                Row::Conversation("b".into()),
                Row::Conversation("c".into()),
                Row::Folder {
                    id: "f1".into(),
                    name: "Work".into(),
                    count: 1,
                    pinned: false
                },
                Row::Conversation("a".into()),
            ]
        );
        assert_eq!(
            model.sidebar.rows(Some("f1")),
            &[Row::Conversation("d".into())]
        );
        // A reorder delta and a deletion.
        model.apply(
            parse_event(r#"{"type":"sidebar_items_reordered","updates":[{"item":{"type":"conversation","id":"a"},"sortOrder":-9}]}"#),
            Instant::now(),
        );
        model.apply(
            parse_event(r#"{"type":"conversation_deleted","convId":"c"}"#),
            Instant::now(),
        );
        assert_eq!(model.sidebar.rows(None)[1], Row::Conversation("a".into()));
        assert_eq!(model.sidebar.rows(None).len(), 3);
    }

    #[test]
    fn an_incremental_list_is_completed_by_its_rest() {
        let mut model = connected();
        let first = format!(
            r#"{{"type":"conversations_list","conversations":[{}],"folders":[],"sidebarPartial":{{"remaining":1,"folderConversationCounts":{{}}}}}}"#,
            summary("a", false, 1.0, None)
        );
        let rest = format!(
            r#"{{"type":"conversations_list","conversations":[{}],"sidebarRest":true}}"#,
            summary("b", false, 0.0, None)
        );
        model.apply(parse_event(&first), Instant::now());
        assert!(!model.sidebar.complete);
        model.apply(parse_event(&rest), Instant::now());
        assert!(model.sidebar.complete);
        assert_eq!(model.sidebar.rows(None).len(), 2);
    }

    fn open(model: &mut Model, id: &str) -> String {
        let commands = model.handle(Action::OpenConversation(id.into()));
        let load = commands
            .iter()
            .find(|c| c["type"] == "load_conversation")
            .unwrap();
        load["reqId"].as_str().unwrap().to_owned()
    }

    #[test]
    fn a_conversation_streams_completes_and_settles() {
        let now = Instant::now();
        let mut model = connected();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_updated","summary":{}}}"#,
                summary("c", false, 0.0, None)
            )),
            now,
        );
        let request = open(&mut model, "c");
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"user","text":"old"}},{{"type":"ai","blocks":[{{"type":"text","text":"reply"}}]}}],"historyStartIndex":4,"historyTotalEntries":6,"hasOlderHistory":true}}"#
            )),
            now,
        );
        assert_eq!(model.chat.start, 4);
        assert_eq!(model.chat.entries.len(), 2);
        model.composer = "Hello".into();
        let sent = model.handle(Action::Send);
        assert_eq!(sent[0]["type"], "send_message");
        assert_eq!(model.chat.echoes.len(), 1);
        let started = sent[0]["startedAt"].as_u64().unwrap();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"user_message","convId":"c","text":"Hello","startedAt":{started}}}"#
            )),
            now,
        );
        assert!(model.chat.echoes.is_empty());
        for line in [
            r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":7}"#,
            r#"{"type":"block_start","convId":"c","blockType":"thinking"}"#,
            r#"{"type":"thinking_chunk","convId":"c","text":"hm"}"#,
            r#"{"type":"tool_call","convId":"c","toolCallId":"t1","toolName":"Bash","input":{},"summary":"ls"}"#,
            r#"{"type":"tool_result","convId":"c","toolCallId":"t1","toolName":"Bash","output":"a b","isError":false}"#,
            r#"{"type":"text_chunk","convId":"c","text":"Hi "}"#,
            r#"{"type":"text_chunk","convId":"c","text":"there"}"#,
        ] {
            model.apply(parse_event(line), now);
        }
        let live = model.chat.live.as_ref().unwrap();
        assert_eq!(live.blocks.len(), 4);
        assert_eq!(
            live.blocks[3],
            Block::Text {
                text: "Hi there".into()
            }
        );
        assert_eq!(model.handle(Action::Abort)[0]["expectedStartedAt"], 7);
        model.apply(
            parse_event(r#"{"type":"streaming_sync","convId":"c","blocks":[{"type":"text","text":"Hi there!"}]}"#),
            now,
        );
        assert_eq!(model.chat.live.as_ref().unwrap().blocks.len(), 4);
        model.apply(
            parse_event(r#"{"type":"message_complete","convId":"c","blocks":[{"type":"text","text":"Final"}],"endedAt":9,"tokens":3}"#),
            now,
        );
        assert!(model.chat.live.is_none());
        assert_eq!(model.chat.entries.len(), 4);
        let refresh = model.apply(
            parse_event(r#"{"type":"streaming_stopped","convId":"c"}"#),
            now,
        );
        assert_eq!(refresh[0]["type"], "load_conversation_history");
        let request = refresh[0]["reqId"].as_str().unwrap();
        // The daemon's newest page replaces the tail from its start.
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_history_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"user","text":"Hello"}},{{"type":"ai","blocks":[{{"type":"text","text":"Final!"}}]}}],"historyStartIndex":6,"historyStartUserIndex":3,"historyEndIndex":8,"historyTotalEntries":8,"hasOlderHistory":true}}"#
            )),
            now,
        );
        assert_eq!(model.chat.entries.len(), 4);
        assert_eq!(
            model.chat.entries[3],
            Entry::Ai {
                blocks: vec![Block::Text {
                    text: "Final!".into()
                }],
                metadata: None
            }
        );
    }

    #[test]
    fn older_pages_join_a_split_entry() {
        let now = Instant::now();
        let mut model = connected();
        let request = open(&mut model, "c");
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"ai","blocks":[{{"type":"text","text":"second half"}}]}}],"historyStartIndex":3,"historyStartBlockIndex":2,"hasOlderHistory":true}}"#
            )),
            now,
        );
        let older = model.load_older();
        assert_eq!(older[0]["beforeEntryIndex"], 3);
        assert_eq!(older[0]["beforeBlockIndex"], 2);
        assert!(model.load_older().is_empty(), "one page at a time");
        let request = older[0]["reqId"].as_str().unwrap();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_history_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"user","text":"q"}},{{"type":"ai","blocks":[{{"type":"text","text":"a"}},{{"type":"text","text":"b"}}]}}],"historyStartIndex":2,"historyStartUserIndex":2,"historyEndIndex":3,"historyEndBlockIndex":2,"historyTotalEntries":4,"hasOlderHistory":false}}"#
            )),
            now,
        );
        assert_eq!(model.chat.start, 2);
        assert!(!model.chat.has_older);
        assert_eq!(model.chat.entries.len(), 2);
        let Entry::Ai { blocks, .. } = &model.chat.entries[1] else {
            panic!()
        };
        assert_eq!(blocks.len(), 3);
    }

    #[test]
    fn a_new_conversation_is_created_with_the_chosen_model() {
        let now = Instant::now();
        let mut model = connected();
        model.apply(
            parse_event(r#"{"type":"tools_available","providers":[{"id":"anthropic","label":"Anthropic","defaultModel":"o","models":[{"id":"claude-haiku-5-5","label":"Haiku","defaultEffort":"high","supportedEfforts":[{"effort":"low"}]}]}],"authByProvider":{"anthropic":true},"conversationDefaults":{"defaults":{"provider":"openai","model":"gpt","effort":"low"}}}"#),
            now,
        );
        model.folders.push("f1".into());
        model.handle(Action::NewConversation);
        assert_eq!(model.selection().model, "gpt");
        model.handle(Action::PickModel(
            "anthropic".into(),
            "claude-haiku-5-5".into(),
        ));
        model.handle(Action::PickEffort("low".into()));
        assert_eq!(model.model_label(&model.selection()), "Haiku");
        model.composer = "Hi".into();
        let commands = model.handle(Action::Send);
        assert_eq!(commands[0]["type"], "new_conversation");
        assert_eq!(commands[0]["model"], "claude-haiku-5-5");
        assert_eq!(commands[0]["effort"], "low");
        assert_eq!(commands[0]["folderId"], "f1");
        assert!(model.chat.id.is_some());
        assert!(model.chat.echoes.is_empty());
        assert!(matches!(&model.chat.entries[..], [Entry::User { text, .. }] if text == "Hi"));
        assert_eq!(model.shown_conversation(), model.chat.id.as_deref());
        // Leaving the chat unsubscribes.
        let back = model.handle(Action::Back);
        assert_eq!(back[0]["type"], "unsubscribe");
        assert_eq!(model.shown_conversation(), None);
    }

    /// A window of `count` user/ai pairs from entry `start`, the ai replies
    /// saying `reply`.
    fn window(
        kind: &str,
        request: &str,
        start: u64,
        count: u64,
        reply: &str,
        extra: &str,
    ) -> String {
        let entries: Vec<String> = (start..start + count)
            .map(|i| {
                if i % 2 == 0 {
                    format!(r#"{{"type":"user","text":"q{i}"}}"#)
                } else {
                    format!(r#"{{"type":"ai","blocks":[{{"type":"text","text":"{reply} {i}"}}]}}"#)
                }
            })
            .collect();
        format!(
            r#"{{"type":"{kind}","reqId":"{request}","convId":"c","entries":[{}],"historyStartIndex":{start},"historyEndIndex":{},"historyTotalEntries":{},"hasOlderHistory":{}{extra}}}"#,
            entries.join(","),
            start + count,
            start + count,
            start > 0
        )
    }

    #[test]
    fn an_open_pages_its_window_and_the_backfill_joins_on() {
        let now = Instant::now();
        let mut model = connected();
        let commands = model.handle(Action::OpenConversation("c".into()));
        let load = &commands[0];
        assert_eq!(load["type"], "load_conversation");
        // `turns` is what makes the daemon page at all (and page its
        // history_updated): without it the whole history comes, every time.
        assert!(load["turns"].is_u64());
        let request = load["reqId"].as_str().unwrap().to_owned();
        let backfill = load["initialBackfill"]["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        model.apply(
            parse_event(&window(
                "conversation_loaded",
                &request,
                20,
                4,
                "a",
                &format!(r#","initialBackfillReqId":"{backfill}""#),
            )),
            now,
        );
        assert!(model.chat.loaded);
        assert_eq!(model.chat.older_request.as_deref(), Some(backfill.as_str()));
        assert!(model.load_older().is_empty(), "the backfill is that page");
        model.apply(
            parse_event(&window(
                "conversation_history_loaded",
                &backfill,
                10,
                10,
                "a",
                "",
            )),
            now,
        );
        assert_eq!((model.chat.start, model.chat.entries.len()), (10, 14));
        assert_eq!(model.chat.stamps.len(), 14);
        let older = model.load_older();
        assert_eq!(older[0]["beforeEntryIndex"], 10);
        assert_eq!(older[0]["turns"], proto::OLDER_TURNS);
    }

    #[test]
    fn stamps_change_only_with_their_entries() {
        let now = Instant::now();
        let mut model = connected();
        let request = open(&mut model, "c");
        model.apply(
            parse_event(&window("conversation_loaded", &request, 0, 6, "a", "")),
            now,
        );
        let stamps = model.chat.stamps.clone();
        assert_eq!(stamps.len(), 6);
        // The daemon's newest window again, its last reply changed.
        let mut updated = window("history_updated", "-", 2, 4, "a", "");
        updated = updated.replace("a 5", "a 5, longer");
        model.apply(parse_event(&updated), now);
        assert_eq!(model.chat.entries.len(), 6);
        assert_eq!(model.chat.stamps[..5], stamps[..5]);
        assert_ne!(model.chat.stamps[5], stamps[5]);
        // A message and a reply are new entries with new stamps.
        model.apply(
            parse_event(r#"{"type":"system_message","convId":"c","text":"note"}"#),
            now,
        );
        assert_eq!(model.chat.stamps.len(), 7);
        assert!(!stamps.contains(&model.chat.stamps[6]));
    }

    #[test]
    fn a_reopened_conversation_shows_at_once_and_keeps_what_agrees() {
        let now = Instant::now();
        let mut model = connected();
        let request = open(&mut model, "c");
        model.apply(
            parse_event(&window("conversation_loaded", &request, 10, 6, "a", "")),
            now,
        );
        let stamps = model.chat.stamps.clone();
        model.handle(Action::Back);
        assert!(model.chat.id.is_none());
        // Reopened: the kept history shows while the fresh window comes.
        let request = open(&mut model, "c");
        assert!(model.chat.loaded && model.chat_pending());
        assert_eq!(model.chat.stamps, stamps);
        // The fresh window starts inside what is kept and agrees there (its
        // last reply grew): the older entries stay, with their stamps.
        let fresh = window("conversation_loaded", &request, 14, 2, "a", "")
            .replace("a 15", "a 15 and more");
        model.apply(parse_event(&fresh), now);
        assert!(!model.chat_pending());
        assert_eq!((model.chat.start, model.chat.entries.len()), (10, 6));
        assert_eq!(model.chat.stamps[..5], stamps[..5]);
        assert!(model.chat.older_request.is_none());
        // One that does not agree where it begins replaces everything.
        model.handle(Action::Back);
        let request = open(&mut model, "c");
        let rewritten = window("conversation_loaded", &request, 12, 4, "rewritten", "");
        model.apply(parse_event(&rewritten), now);
        assert_eq!((model.chat.start, model.chat.entries.len()), (12, 4));
    }

    #[test]
    fn a_null_live_tail_ends_the_reply() {
        let now = Instant::now();
        let mut model = connected();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_updated","summary":{}}}"#,
                summary("c", false, 0.0, None)
                    .replace(r#""streaming":false"#, r#""streaming":true"#)
            )),
            now,
        );
        let request = open(&mut model, "c");
        model.apply(
            parse_event(&window("conversation_loaded", &request, 0, 2, "a", "")),
            now,
        );
        model.apply(
            parse_event(r#"{"type":"text_chunk","convId":"c","text":"partial"}"#),
            now,
        );
        assert!(model.chat.live.is_some());
        // Absent: the live tail stays (an older daemon says nothing).
        model.apply(
            parse_event(&window("history_updated", "-", 0, 2, "a", "")),
            now,
        );
        assert!(model.chat.live.is_some());
        model.apply(
            parse_event(&window(
                "history_updated",
                "-",
                0,
                2,
                "a",
                r#","pendingAI":null"#,
            )),
            now,
        );
        assert!(model.chat.live.is_none());
    }

    #[test]
    fn a_quiet_reconnection_keeps_the_screen_for_a_while() {
        let now = Instant::now();
        let mut model = connected();
        let request = open(&mut model, "c");
        model.apply(
            parse_event(&window("conversation_loaded", &request, 0, 2, "a", "")),
            now,
        );
        model.apply(
            parse_event(r#"{"type":"text_chunk","convId":"c","text":"partial"}"#),
            now,
        );
        model.reconnect_quietly(now, std::time::Duration::from_secs(1));
        model.on_disconnected();
        assert!(!model.offline && model.chat.live.is_some());
        assert!(!model.tick(now));
        assert!(model.tick(now + std::time::Duration::from_secs(2)));
        assert!(model.offline);
        let commands = model.on_connected();
        assert!(!model.offline);
        assert!(commands.iter().any(|c| c["type"] == "load_conversation"));
    }

    #[test]
    fn an_unanswered_load_is_given_up_on_and_a_late_one_still_lands() {
        let now = Instant::now();
        let mut model = connected();
        let request = open(&mut model, "c");
        assert!(!model.expire(now));
        let late = model.answer_deadline().expect("a deadline");
        assert!(model.expire(late));
        assert!(model.chat.loaded && model.chat.error.is_some());
        assert!(
            model.chat.older_request.is_none(),
            "scrolling up asks again"
        );
        model.apply(
            parse_event(&window("conversation_loaded", &request, 0, 2, "a", "")),
            now,
        );
        assert!(model.chat.error.is_none());
        assert_eq!(model.chat.entries.len(), 2);
        assert!(model.answer_deadline().is_none());
    }

    #[test]
    fn reconnecting_reopens_and_a_target_waits_for_the_connection() {
        let mut model = Model::new();
        assert!(model.open_target("c9").is_empty());
        assert_eq!(model.page, Page::Chat);
        let commands = model.on_connected();
        assert!(
            commands
                .iter()
                .any(|c| c["type"] == "load_conversation" && c["convId"] == "c9")
        );
        model.on_disconnected();
        let commands = model.on_connected();
        assert!(
            commands
                .iter()
                .any(|c| c["type"] == "load_conversation" && c["convId"] == "c9")
        );
        assert_eq!(commands[0]["type"], "client_capabilities");
    }

    #[test]
    fn expanding_a_tool_fetches_its_output_once() {
        let now = Instant::now();
        let mut model = connected();
        let request = open(&mut model, "c");
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"ai","blocks":[{{"type":"tool_call","toolCallId":"t1","toolName":"Bash","input":{{}},"summary":"ls"}},{{"type":"tool_result","toolCallId":"t1","toolName":"","output":"","isError":false}}]}}],"historyStartIndex":0}}"#
            )),
            now,
        );
        let commands = model.handle(Action::ToggleExpanded("tool:t1".into()));
        assert_eq!(commands[0]["type"], "load_tool_outputs");
        model.handle(Action::ToggleExpanded("tool:t1".into()));
        assert!(
            model
                .handle(Action::ToggleExpanded("tool:t1".into()))
                .is_empty()
        );
        model.apply(
            parse_event(r#"{"type":"tool_outputs_loaded","convId":"c","outputs":[{"toolCallId":"t1","output":"x"}]}"#),
            now,
        );
        assert_eq!(model.chat.outputs["t1"], "x");
    }
}
