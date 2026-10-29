//! The Exocortex app's state: the daemon's conversation list (folders,
//! pinned and recent order as the TUI shows them), the open conversation's
//! history window and live reply, the composer, the model choices, and what
//! the UI asks the daemon for. Events in, commands out; no I/O here, so all
//! of it is tested.
//!
//! Each work package owns a part (roadmap §4): the list is `sidebar.rs`
//! (B), the open conversation `chat.rs` (C2), the composer `composer.rs`
//! (D), models, settings and the operations panels `ops.rs` (E). Each
//! defines its state struct (a field of `Model`), its actions (a variant
//! of `Action`), its `handle_*` for them and its `apply_*` for daemon
//! events, so a package edits its own file. This file (A) holds the
//! connection, the pages, the fan-out and requests, and what the model
//! asks of the interface (`Ui`: overlays to open, effects such as haptics).

pub mod chat;
pub mod composer;
pub mod ops;
pub mod sidebar;

use std::time::{Duration, Instant};

use serde_json::Value;

use super::proto::{self, Event, Summary};
use super::requests::{self, Owner, Pending, Requests};
use super::store::Persist;
use super::ui::Haptic;
use super::ui::overlay::Overlay;
use crate::control::KeyboardPurpose;

pub use chat::{Chat, Echo, Live, Shelf};
pub use composer::Composer;
pub use ops::Ops;
pub use sidebar::{Row, Sidebar};

/// The three pages, left to right (PAR-001): Settings | List | Chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Settings,
    List,
    Chat,
}

/// Something the user did (or a screen asked for): one variant per area,
/// each area's own enum in its file.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Pages (A).
    Nav(Nav),
    /// The conversation list (B, `sidebar::Act`).
    List(sidebar::Act),
    /// The open conversation (C2, `chat::Act`).
    Chat(chat::Act),
    /// The composer (D, `composer::Act`).
    Composer(composer::Act),
    /// Models, settings and the operations panels (E, `ops::Act`).
    Ops(ops::Act),
    /// The interface itself (A).
    Ui(UiAct),
}

/// Moving between pages.
#[derive(Debug, Clone, PartialEq)]
pub enum Nav {
    /// The top bar's back, Moon's Back, the left-edge swipe: out of a chat,
    /// up a folder, or from the list to Settings.
    Back,
    OpenConversation(String),
    NewConversation,
    OpenSettings,
    /// From Settings back to the list.
    CloseSettings,
}

/// The interface's own actions.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Some are for the work packages (links: C1).
pub enum UiAct {
    /// Closes the topmost overlay.
    Dismiss,
    /// Copies text to the clipboard ("Copied", a selection haptic).
    Copy(String),
    /// Opens an http(s) link in Moon's Browser.
    OpenUrl(String),
    /// Opens the reader on a text (C1 completes the reader).
    Read { title: String, text: String },
    /// Shows a toast.
    Toast(String),
}

/// What the model asks the event loop to do for it (it does no I/O).
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // HideKeyboard is for the work packages.
pub enum Effect {
    Haptic(Haptic),
    Copy(String),
    OpenUrl(String),
    ShowKeyboard(KeyboardPurpose),
    HideKeyboard,
    /// Moon's status bar takes the theme's background again (E: a theme
    /// change).
    StatusBar,
}

/// What the model asks of the interface: overlays to open or close and
/// effects to perform; the event loop takes them after every change.
#[derive(Default)]
pub struct Ui {
    pub opening: Vec<Box<dyn Overlay>>,
    /// Close every modal overlay (a page changed).
    pub close_all: bool,
    /// Close the topmost modal overlay.
    pub close_top: bool,
    /// Close the conversation-scoped overlays (the conversation changed).
    pub close_scoped: bool,
    pub effects: Vec<Effect>,
}

impl Ui {
    pub fn open(&mut self, overlay: Box<dyn Overlay>) {
        self.opening.push(overlay);
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.open(Box::new(super::ui::toast::Toast::new(text.into(), false)));
    }

    pub fn error_toast(&mut self, text: impl Into<String>) {
        self.open(Box::new(super::ui::toast::Toast::new(text.into(), true)));
    }

    pub fn effect(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    pub fn haptic(&mut self, haptic: Haptic) {
        self.effects.push(Effect::Haptic(haptic));
    }
}

pub struct Model {
    pub connected: bool,
    /// The daemon answered the bootstrap's ping: sending and commands are
    /// allowed (PAR-020). Stop is not gated on it.
    pub ready: bool,
    /// "Reconnecting…" shows: disconnected, and not for a quiet reconnection
    /// that has not taken long yet.
    pub offline: bool,
    /// A reconnection the app asked for (back from the background) keeps
    /// what is on screen until then, while the new connection catches up.
    quiet_until: Option<Instant>,
    /// Ever connected: "Connecting…" is said differently the first time.
    pub was_connected: bool,
    /// The bootstrap ping's reqId, whose pong makes the connection ready.
    ping: Option<String>,
    pub page: Page,
    pub ui: Ui,
    pub requests: Requests,
    /// The kept files (settings.json, ui.json, drafts.json).
    pub persist: Persist,
    pub sidebar: Sidebar,
    pub chat: Chat,
    /// Conversations opened before, to show at once when reopened.
    pub shelf: Shelf,
    pub composer: Composer,
    pub ops: Ops,
    /// A conversation to open once connected (a notification's target).
    pub pending_open: Option<String>,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

/// A conversation's window, or an older page, that has not come in this
/// long is given up on: the chat says so (a late one still lands), and
/// scrolling up asks again.
#[allow(dead_code)]
pub const ANSWER_WITHIN: Duration = requests::LOAD_WITHIN;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Model {
    pub fn new() -> Self {
        Self {
            connected: false,
            ready: false,
            offline: true,
            quiet_until: None,
            was_connected: false,
            ping: None,
            page: Page::List,
            ui: Ui::default(),
            requests: Requests::default(),
            persist: Persist::default(),
            sidebar: Sidebar::default(),
            chat: Chat::default(),
            shelf: Shelf::default(),
            composer: Composer::default(),
            ops: Ops::default(),
            pending_open: None,
        }
    }

    pub fn current_folder(&self) -> Option<&str> {
        self.sidebar.current_folder()
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
        self.chat.load_request.is_some()
    }

    /// The conversation shown in the chat, if any, for the bridge.
    pub fn shown_conversation(&self) -> Option<&str> {
        match self.page {
            Page::Chat => self.chat.id.as_deref(),
            Page::List | Page::Settings => None,
        }
    }

    /// The bootstrap, sent on every (re)connection.
    pub fn on_connected(&mut self) -> Vec<Value> {
        self.connected = true;
        self.ready = false;
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
            Page::List | Page::Settings => None,
        });
        if let Some(id) = reopen {
            commands.extend(self.open(id));
        }
        let ping = format!("ping-{}", now_ms());
        self.requests.track(
            &ping,
            Owner::App,
            "ping",
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        commands.push(proto::ping(&ping));
        self.ping = Some(ping);
        commands
    }

    pub fn on_disconnected(&mut self) {
        self.connected = false;
        self.ready = false;
        self.ping = None;
        self.offline = self.quiet_until.is_none();
        // Nothing in flight will be answered.
        self.requests.clear();
        self.ops_disconnected();
        self.chat_disconnected(self.quiet_until.is_some());
    }

    /// The app is about to reconnect on purpose: what is on screen stays,
    /// without "Reconnecting…", for up to `grace`.
    pub fn reconnect_quietly(&mut self, now: Instant, grace: Duration) {
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
            Action::Nav(nav) => self.handle_nav(nav),
            Action::List(act) => self.handle_list(act),
            Action::Chat(act) => self.handle_chat(act),
            Action::Composer(act) => self.handle_composer(act),
            Action::Ops(act) => self.handle_ops(act),
            Action::Ui(act) => {
                self.handle_ui(act);
                Vec::new()
            }
        }
    }

    fn handle_nav(&mut self, nav: Nav) -> Vec<Value> {
        match nav {
            Nav::OpenConversation(id) => self.open(id),
            Nav::NewConversation => self.new_conversation(),
            Nav::OpenSettings => {
                self.ui.close_all = true;
                self.page = Page::Settings;
                Vec::new()
            }
            Nav::CloseSettings => {
                self.ui.close_all = true;
                self.page = Page::List;
                Vec::new()
            }
            Nav::Back => {
                self.ui.close_all = true;
                match self.page {
                    Page::Chat => self.leave_chat(),
                    Page::Settings => {
                        self.page = Page::List;
                        Vec::new()
                    }
                    // Up a folder; at the top, Settings (left of the list).
                    Page::List => {
                        if !self.sidebar.up() {
                            self.page = Page::Settings;
                        }
                        Vec::new()
                    }
                }
            }
        }
    }

    fn handle_ui(&mut self, act: UiAct) {
        match act {
            UiAct::Dismiss => self.ui.close_top = true,
            UiAct::Copy(text) => {
                self.ui.effect(Effect::Copy(text));
                self.ui.haptic(Haptic::Selection);
                self.ui.toast("Copied");
            }
            UiAct::OpenUrl(url) => {
                if super::ui::openable(&url) {
                    self.ui.effect(Effect::OpenUrl(url));
                } else {
                    self.ui.toast("Only web links open in the Browser.");
                }
            }
            UiAct::Read { title, text } => {
                self.ui
                    .open(Box::new(super::reader::Reader::new(title, text)));
            }
            UiAct::Toast(text) => self.ui.toast(text),
        }
    }

    /// Whether an event can change what is on screen (a chat is not
    /// redrawn for other conversations' sidebar traffic).
    pub fn affects_view(&self, event: &Event) -> bool {
        if self.page != Page::Chat {
            return true;
        }
        let open = self.chat.id.as_deref();
        match event {
            Event::ConversationUpdated { summary, .. }
            | Event::ConversationRestored { summary, .. } => Some(summary.id.as_str()) == open,
            Event::ConversationsList { .. }
            | Event::ConversationMoved { .. }
            | Event::SidebarItemsReordered { .. }
            | Event::SidebarStatePatched { .. }
            | Event::ConversationPinned { .. }
            | Event::ConversationMarked(_)
            | Event::ConnectivityStatus { .. }
            | Event::UsageUpdate(_)
            | Event::Ack { .. }
            | Event::Pong { .. } => false,
            Event::ConversationDeleted { conv_id, .. } => Some(conv_id.as_str()) == open,
            _ => true,
        }
    }

    /// Applies one daemon event; returns commands it calls for. Every area
    /// sees it: the list, the operations, the composer (by reference), and
    /// last the open conversation (by value: windows are big).
    pub fn apply(&mut self, event: Event, now: Instant) -> Vec<Value> {
        // An answer settles its request; its owner handles it below.
        let settled = event.req_id().and_then(|id| {
            self.requests
                .settle(id)
                .map(|pending| (id.to_owned(), pending))
        });
        let mut commands = Vec::new();
        match &event {
            Event::Pong { req_id, .. } if req_id.is_some() && *req_id == self.ping => {
                self.ready = true;
            }
            Event::Error {
                req_id,
                conv_id,
                message,
            } => {
                if !self.route_error(req_id.as_deref(), settled.as_ref(), message) {
                    // About the open conversation, or ours: say so.
                    let ours = settled.is_some();
                    if ours || (conv_id.is_some() && *conv_id == self.chat.id) {
                        self.ui.error_toast(message.clone());
                    }
                }
                return commands;
            }
            _ => {}
        }
        commands.extend(self.apply_list(&event, now));
        commands.extend(self.apply_ops(&event, now));
        commands.extend(self.apply_composer(&event, now));
        commands.extend(self.apply_chat(event, now));
        commands
    }

    /// An error answering a request: its owner's to handle. True when one
    /// did.
    fn route_error(
        &mut self,
        req_id: Option<&str>,
        settled: Option<&(String, Pending)>,
        message: &str,
    ) -> bool {
        let Some(id) = req_id else {
            return false;
        };
        match settled.map(|(_, pending)| pending.owner) {
            // Not waited on any more: a late answer the chat may still keep.
            None | Some(Owner::Chat) => self.chat_error(id, message),
            Some(Owner::Sidebar) => self.list_error(id, message),
            Some(Owner::Composer) => self.composer_error(id, message),
            Some(Owner::Ops) => self.ops_error(id, message),
            Some(Owner::App) => false,
        }
    }

    /// Gives up on what the daemon has not answered in time; returns
    /// whether that changed what shows.
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for (id, pending) in self.requests.expire(now) {
            changed |= match pending.owner {
                Owner::Chat => self.chat_timeout(&id, &pending),
                Owner::Sidebar => self.list_timeout(&id, &pending),
                Owner::Composer => self.composer_timeout(&id, &pending),
                Owner::Ops => self.ops_timeout(&id, &pending),
                // The connection answers nothing: it shows as connecting.
                Owner::App => false,
            };
        }
        changed
    }

    /// When `expire` next has something to do.
    pub fn answer_deadline(&self) -> Option<Instant> {
        self.requests.deadline()
    }

    /// A request of an area that did not handle its timeout itself: say so
    /// (the generic actionable error, PAR-027).
    pub(super) fn generic_timeout(&mut self) -> bool {
        self.ui.error_toast("Exocortex did not answer. Try again.");
        true
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

    /// Connected and ready (the daemon answered the ping).
    fn connected() -> Model {
        let mut model = Model::new();
        let commands = model.on_connected();
        let ping = commands.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            Instant::now(),
        );
        assert!(model.ready);
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
        let commands = model.handle(Action::Nav(Nav::OpenConversation(id.into())));
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
        model.composer.text = "Hello".into();
        let sent = model.handle(Action::Composer(composer::Act::Send));
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
        assert_eq!(
            model.handle(Action::Composer(composer::Act::Abort))[0]["expectedStartedAt"],
            7
        );
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
        model.sidebar.path.push("f1".into());
        model.handle(Action::Nav(Nav::NewConversation));
        assert_eq!(model.selection().model, "gpt");
        model.handle(Action::Ops(ops::Act::PickModel(
            "anthropic".into(),
            "claude-haiku-5-5".into(),
        )));
        model.handle(Action::Ops(ops::Act::PickEffort("low".into())));
        assert_eq!(model.model_label(&model.selection()), "Haiku");
        model.composer.text = "Hi".into();
        let commands = model.handle(Action::Composer(composer::Act::Send));
        assert_eq!(commands[0]["type"], "new_conversation");
        assert_eq!(commands[0]["model"], "claude-haiku-5-5");
        assert_eq!(commands[0]["effort"], "low");
        assert_eq!(commands[0]["folderId"], "f1");
        assert!(model.chat.id.is_some());
        assert!(model.chat.echoes.is_empty());
        assert!(matches!(&model.chat.entries[..], [Entry::User { text, .. }] if text == "Hi"));
        assert_eq!(model.shown_conversation(), model.chat.id.as_deref());
        // Leaving the chat unsubscribes.
        let back = model.handle(Action::Nav(Nav::Back));
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
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
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
        model.handle(Action::Nav(Nav::Back));
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
        model.handle(Action::Nav(Nav::Back));
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
        let commands = model.handle(Action::Chat(chat::Act::ToggleExpanded("tool:t1".into())));
        assert_eq!(commands[0]["type"], "load_tool_outputs");
        model.handle(Action::Chat(chat::Act::ToggleExpanded("tool:t1".into())));
        assert!(
            model
                .handle(Action::Chat(chat::Act::ToggleExpanded("tool:t1".into())))
                .is_empty()
        );
        model.apply(
            parse_event(r#"{"type":"tool_outputs_loaded","convId":"c","outputs":[{"toolCallId":"t1","output":"x"}]}"#),
            now,
        );
        assert_eq!(model.chat.outputs["t1"], "x");
    }

    /// What the model asked to open (toasts' texts, sheets' titles).
    fn opened(model: &mut Model) -> Vec<String> {
        model
            .ui
            .opening
            .drain(..)
            .map(|overlay| overlay.label())
            .collect()
    }

    #[test]
    fn sending_waits_for_the_pong_and_stop_does_not() {
        let now = Instant::now();
        let mut model = Model::new();
        let bootstrap = model.on_connected();
        let ping = bootstrap.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(!model.ready);
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_updated","summary":{}}}"#,
                summary("c", false, 0.0, None)
                    .replace(r#""streaming":false"#, r#""streaming":true"#)
            )),
            now,
        );
        open(&mut model, "c");
        model.composer.text = "Hi".into();
        assert!(
            model
                .handle(Action::Composer(composer::Act::Send))
                .is_empty(),
            "not ready: nothing is sent"
        );
        assert_eq!(model.composer.text, "Hi", "and nothing is lost");
        let stop = model.handle(Action::Composer(composer::Act::Abort));
        assert_eq!(stop[0]["type"], "abort", "Stop works before the pong");
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            now,
        );
        assert!(model.ready);
        assert_eq!(
            model.handle(Action::Composer(composer::Act::Send))[0]["type"],
            "queue_message",
            "a running turn: queued"
        );
    }

    #[test]
    fn an_unanswered_request_says_so_and_its_late_answer_is_stale() {
        let now = Instant::now();
        let mut model = connected();
        open(&mut model, "c");
        model.apply(
            parse_event(&window("conversation_loaded", "-", 0, 2, "a", "")),
            now,
        );
        // Stop: an ack within 12 s, else an actionable error.
        let stop = model.handle(Action::Composer(composer::Act::Abort));
        let id = stop[0]["reqId"].as_str().unwrap().to_owned();
        let deadline = model.answer_deadline().unwrap();
        assert!(deadline <= now + requests::ABORT_WITHIN + std::time::Duration::from_secs(1));
        opened(&mut model);
        assert!(model.expire(now + requests::ABORT_WITHIN + std::time::Duration::from_secs(1)));
        assert_eq!(
            opened(&mut model),
            vec!["Stop did not reach Exocortex. Try again."]
        );
        // Its ack, late, is answered by no one.
        assert!(!model.requests.is_pending(&id));
        model.apply(
            parse_event(&format!(r#"{{"type":"ack","reqId":"{id}","convId":"c"}}"#)),
            now,
        );
        assert!(opened(&mut model).is_empty());
        // A rejected command says why; an error for nobody stays quiet.
        let set = model.handle(Action::Ops(ops::Act::PickEffort("low".into())));
        let id = set[0]["reqId"].as_str().unwrap().to_owned();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"error","reqId":"{id}","message":"Unknown effort"}}"#
            )),
            now,
        );
        assert_eq!(opened(&mut model), vec!["Unknown effort"]);
        model.apply(
            parse_event(r#"{"type":"error","reqId":"app-999","message":"Not ours"}"#),
            now,
        );
        assert!(opened(&mut model).is_empty());
        // A dropped connection forgets everything in flight.
        model.handle(Action::Ops(ops::Act::PickEffort("high".into())));
        model.on_disconnected();
        assert!(model.requests.is_empty() && model.answer_deadline().is_none());
    }

    #[test]
    fn pages_go_settings_list_chat_and_back() {
        let mut model = connected();
        assert_eq!(model.page, Page::List);
        model.handle(Action::Nav(Nav::Back));
        assert_eq!(model.page, Page::Settings, "left of the list");
        model.handle(Action::Nav(Nav::CloseSettings));
        model.handle(Action::List(sidebar::Act::OpenFolder("f1".into())));
        model.handle(Action::Nav(Nav::Back));
        assert_eq!(model.page, Page::List, "up a folder first");
        assert!(model.sidebar.path.is_empty());
        open(&mut model, "c");
        assert!(model.ui.close_all, "leaving the list closes its menus");
        model.ui.close_all = false;
        model.handle(Action::Nav(Nav::Back));
        assert_eq!(model.page, Page::List);
        assert!(model.ui.close_all);
        // The list's menu opens Settings.
        model.handle(Action::List(sidebar::Act::Menu(crate::gfx::RectF::new(
            707.0, 0.0, 122.0, 122.0,
        ))));
        assert_eq!(opened(&mut model), vec!["menu: Settings"]);
    }
}
