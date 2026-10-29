//! The Exocortex app's state: the daemon's conversation list (folders,
//! pinned and recent order as the TUI shows them), the open conversation's
//! history window and live reply, the model choices, and what the UI asks the
//! daemon for. Events in, commands out; no I/O here, so all of it is tested.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::Value;

use super::proto::{self, Block, Entry, Event, Folder, Provider, Queued, Selection, Summary};

/// One row of a conversation list level.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Folder {
        id: String,
        name: String,
        count: usize,
        pinned: bool,
    },
    Conversation(String),
}

#[derive(Debug, Default)]
pub struct Sidebar {
    pub conversations: HashMap<String, Summary>,
    pub folders: HashMap<String, Folder>,
    /// The whole list has arrived (not just the bootstrap's first page).
    pub complete: bool,
    pub loaded: bool,
    /// Rows by folder, rebuilt after any change.
    rows: HashMap<Option<String>, Vec<Row>>,
    /// Bumped on every change (the list view's redraw key).
    pub version: u64,
}

fn order(pinned: bool, sort: f64) -> (bool, f64) {
    (!pinned, sort)
}

impl Sidebar {
    fn changed(&mut self) {
        self.rows.clear();
        self.version = self.version.wrapping_add(1);
    }

    pub fn apply(&mut self, event: &Event) -> bool {
        match event {
            Event::ConversationsList {
                conversations,
                folders,
                sidebar_partial,
                sidebar_rest,
            } => {
                if sidebar_rest == &Some(true) {
                    // The rest of an incremental bootstrap: add to the first page.
                    for summary in conversations {
                        self.conversations
                            .insert(summary.id.clone(), summary.clone());
                    }
                    self.complete = true;
                } else {
                    self.conversations = conversations
                        .iter()
                        .map(|summary| (summary.id.clone(), summary.clone()))
                        .collect();
                    self.complete = sidebar_partial.is_none();
                }
                if let Some(folders) = folders {
                    self.folders = folders.iter().map(|f| (f.id.clone(), f.clone())).collect();
                }
                self.loaded = true;
            }
            Event::ConversationUpdated { summary, .. }
            | Event::ConversationRestored { summary }
            | Event::ConversationUnwound { summary, .. } => {
                self.conversations
                    .insert(summary.id.clone(), summary.clone());
            }
            Event::ConversationDeleted { conv_id } => {
                self.conversations.remove(conv_id);
            }
            Event::ConversationPinned { conv_id, pinned } => {
                if let Some(summary) = self.conversations.get_mut(conv_id) {
                    summary.pinned = *pinned;
                }
            }
            Event::ConversationMoved {
                conversations,
                folders,
            } => {
                for summary in conversations {
                    self.conversations
                        .insert(summary.id.clone(), summary.clone());
                }
                if let Some(folders) = folders {
                    self.folders = folders.iter().map(|f| (f.id.clone(), f.clone())).collect();
                }
            }
            Event::SidebarItemsReordered { updates } => {
                for update in updates {
                    match update.item.kind.as_str() {
                        "conversation" => {
                            if let Some(summary) = self.conversations.get_mut(&update.item.id) {
                                summary.sort_order = update.sort_order;
                            }
                        }
                        "folder" => {
                            if let Some(folder) = self.folders.get_mut(&update.item.id) {
                                folder.sort_order = update.sort_order;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::SidebarStatePatched {
                conversations,
                folders,
                removed_conversation_ids,
                removed_folder_ids,
            } => {
                for summary in conversations {
                    self.conversations
                        .insert(summary.id.clone(), summary.clone());
                }
                for folder in folders {
                    self.folders.insert(folder.id.clone(), folder.clone());
                }
                for id in removed_conversation_ids {
                    self.conversations.remove(id);
                }
                for id in removed_folder_ids {
                    self.folders.remove(id);
                }
            }
            _ => return false,
        }
        self.changed();
        true
    }

    /// A level of the list: its folders and conversations together, pinned
    /// first, then by the daemon's sort order (sidebar-first-page.ts).
    pub fn rows(&mut self, folder: Option<&str>) -> &[Row] {
        let key = folder.map(str::to_owned);
        if !self.rows.contains_key(&key) {
            let rows = self.build_rows(folder);
            self.rows.insert(key.clone(), rows);
        }
        &self.rows[&key]
    }

    fn build_rows(&self, folder: Option<&str>) -> Vec<Row> {
        let mut entries: Vec<((bool, f64), Row)> = Vec::new();
        let counts = self.folder_counts();
        for f in self.folders.values() {
            if f.parent_id.as_deref() == folder {
                entries.push((
                    order(f.pinned, f.sort_order),
                    Row::Folder {
                        id: f.id.clone(),
                        name: f.name.clone(),
                        count: counts.get(f.id.as_str()).copied().unwrap_or(0),
                        pinned: f.pinned,
                    },
                ));
            }
        }
        for summary in self.conversations.values() {
            let parent = summary
                .folder_id
                .as_deref()
                .filter(|id| self.folders.contains_key(*id));
            if parent == folder {
                entries.push((
                    order(summary.pinned, summary.sort_order),
                    Row::Conversation(summary.id.clone()),
                ));
            }
        }
        entries.sort_by(|a, b| {
            a.0.0.cmp(&b.0.0).then(
                a.0.1
                    .partial_cmp(&b.0.1)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        entries.into_iter().map(|(_, row)| row).collect()
    }

    /// Conversations in each folder, its subfolders included.
    fn folder_counts(&self) -> HashMap<&str, usize> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for summary in self.conversations.values() {
            let mut seen = HashSet::new();
            let mut folder = summary.folder_id.as_deref();
            while let Some(id) = folder {
                let Some(f) = self.folders.get(id) else { break };
                if !seen.insert(id) {
                    break;
                }
                *counts.entry(f.id.as_str()).or_default() += 1;
                folder = f.parent_id.as_deref();
            }
        }
        counts
    }
}

/// A user message sent from here that the daemon has not echoed yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Echo {
    pub text: String,
    pub started_at: u64,
}

/// The reply being streamed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub blocks: Vec<Block>,
    pub started_at: f64,
    /// Turn blocks already in history before this tail (`blockOffset`).
    pub offset: usize,
}

/// A new entry identity (`Chat::stamps`): unique in the process, so layouts
/// cached by stamp stay valid across conversations.
fn next_stamp() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A conversation: a window of its history (entry `start` onward, from
/// block `start_block` of that entry) and what is streaming.
#[derive(Debug, Default)]
pub struct Chat {
    /// None while composing a new conversation.
    pub id: Option<String>,
    pub entries: Vec<Entry>,
    /// One per entry, changed exactly when that entry's content changes (a
    /// window that resends an entry unchanged keeps its stamp): what the
    /// view caches an entry's layout by.
    pub stamps: Vec<u64>,
    pub start: u64,
    pub start_block: u64,
    pub has_older: bool,
    pub older_request: Option<String>,
    pub loaded: bool,
    pub live: Option<Live>,
    pub echoes: Vec<Echo>,
    /// Tool calls and thinking blocks the user opened.
    pub expanded: HashSet<String>,
    /// Tool outputs fetched on demand (history leaves them out).
    pub outputs: HashMap<String, String>,
    pub outputs_requested: HashSet<String>,
    /// Bumped on every change: whether the view lays the chat out again.
    pub version: u64,
    pub error: Option<String>,
}

impl Chat {
    fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    fn end(&self) -> u64 {
        self.start + self.entries.len() as u64
    }

    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        self.stamps.push(next_stamp());
    }

    /// Replaces the entries (and where they start), keeping the stamp of
    /// every entry that is the same as before at the same place.
    fn set_entries(&mut self, entries: Vec<Entry>, start: u64, start_block: u64) {
        let same_start_block = start_block == self.start_block;
        let stamps = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let at = start + index as u64;
                let old = at
                    .checked_sub(self.start)
                    .map(|i| i as usize)
                    .filter(|i| *i < self.entries.len());
                match old {
                    // The first entry may begin at another block.
                    Some(0) if !same_start_block => next_stamp(),
                    Some(i) if self.entries[i] == *entry => self.stamps[i],
                    _ => next_stamp(),
                }
            })
            .collect();
        self.entries = entries;
        self.stamps = stamps;
        self.start = start;
        self.start_block = start_block;
    }

    /// The window the daemon sent replaces what it covers. Messages sent
    /// from here that it now holds are no longer echoes. `fresh`: a newly
    /// loaded window (an open, a reconnection), which keeps the older
    /// history already here only when the two agree where they meet.
    fn splice(&mut self, window: proto::Window, fresh: bool) {
        self.echoes.retain(|echo| {
            !window
                .entries
                .iter()
                .any(|entry| matches!(entry, Entry::User { text, .. } if text.trim() == echo.text.trim()))
        });
        let entries: Vec<Entry> = window
            .entries
            .into_iter()
            .filter(|entry| !matches!(entry, Entry::SystemInstructions { .. }))
            .collect();
        let start = window.history_start_index.unwrap_or(0);
        let start_block = window.history_start_block_index.unwrap_or(0);
        let mut covers_all = window.reset_history_window
            || !self.loaded
            || window.history_start_index.is_none()
            || start < self.start
            || (start == self.start && start_block <= self.start_block)
            || start > self.end();
        if fresh && !covers_all {
            // Where the new window begins must be what is here: the history
            // before it is then the same (an unwind only cuts the end).
            let keep = (start - self.start) as usize;
            let overlap = (self.entries.len() - keep).min(entries.len());
            let agrees = overlap > 0
                && (0..overlap).all(|at| {
                    let (old, new) = (&self.entries[keep + at], &entries[at]);
                    match (old, new) {
                        // The newest here may have grown or settled since.
                        _ if keep + at + 1 == self.entries.len() => true,
                        // The window may begin inside an entry.
                        (Entry::Ai { blocks: old, .. }, Entry::Ai { blocks: new, .. })
                            if at == 0 =>
                        {
                            let own = if keep == 0 { self.start_block } else { 0 };
                            let from = start_block.saturating_sub(own) as usize;
                            old.get(from..) == Some(new.as_slice())
                        }
                        (old, new) => old == new,
                    }
                });
            covers_all = !agrees;
        }
        if covers_all {
            self.set_entries(entries, start, start_block);
            self.has_older = window
                .has_older_history
                .unwrap_or(start > 0 || start_block > 0);
        } else {
            let keep = (start - self.start) as usize;
            let mut incoming = entries;
            if start_block > 0
                && let Some(Entry::Ai { blocks: old, .. }) = self.entries.get(keep)
                && let Some(Entry::Ai { blocks: new, .. }) = incoming.first_mut()
            {
                let own_offset = if keep == 0 { self.start_block } else { 0 };
                let take = (start_block.saturating_sub(own_offset) as usize).min(old.len());
                let mut joined = old[..take].to_vec();
                joined.append(new);
                *new = joined;
            }
            // The entries before the window stay as they are; those it
            // resends unchanged keep their stamps.
            let old = self.entries.split_off(keep);
            let old_stamps = self.stamps.split_off(keep);
            for (index, entry) in incoming.into_iter().enumerate() {
                let stamp = match old.get(index) {
                    Some(before) if *before == entry => old_stamps[index],
                    _ => next_stamp(),
                };
                self.entries.push(entry);
                self.stamps.push(stamp);
            }
        }
        if let Some(Some(pending)) = window.pending_ai {
            let live = self.live.get_or_insert_with(Live::default);
            live.blocks = pending.blocks;
            live.offset = pending.block_offset as usize;
        }
        self.loaded = true;
        self.touch();
    }

    /// An older page, which ends where the window begins.
    fn prepend(&mut self, window: proto::Window) -> bool {
        let end = window.history_end_index.unwrap_or(0);
        let end_block = window.history_end_block_index.unwrap_or(0);
        if end != self.start || end_block != self.start_block {
            return false;
        }
        let mut entries: Vec<Entry> = window
            .entries
            .into_iter()
            .filter(|entry| !matches!(entry, Entry::SystemInstructions { .. }))
            .collect();
        let mut stamps: Vec<u64> = entries.iter().map(|_| next_stamp()).collect();
        let mut old_stamps = std::mem::take(&mut self.stamps);
        if end_block > 0
            && let Some(Entry::Ai { blocks: tail, .. }) = entries.last_mut()
            && let Some(Entry::Ai { blocks: head, .. }) = self.entries.first_mut()
        {
            // The page ends inside the first entry here: one entry again.
            tail.append(head);
            self.entries.remove(0);
            old_stamps.remove(0);
        }
        entries.append(&mut self.entries);
        stamps.append(&mut old_stamps);
        self.entries = entries;
        self.stamps = stamps;
        self.start = window.history_start_index.unwrap_or(0);
        self.start_block = window.history_start_block_index.unwrap_or(0);
        self.has_older = window
            .has_older_history
            .unwrap_or(self.start > 0 || self.start_block > 0);
        self.touch();
        true
    }

    fn live_mut(&mut self) -> &mut Live {
        self.live.get_or_insert_with(Live::default)
    }

    fn commit_live(&mut self) {
        if let Some(live) = self.live.take()
            && !live.blocks.is_empty()
        {
            self.push(Entry::Ai {
                blocks: live.blocks,
                metadata: None,
            });
        }
    }

    /// The chat as kept while another is open: its history, without what
    /// only a live subscription keeps current.
    fn shelved(mut self) -> Self {
        self.live = None;
        self.older_request = None;
        self.outputs_requested.clear();
        self.echoes.clear();
        self.error = None;
        self
    }
}

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

    /// Provider and model of the chat (or the new conversation's).
    pub fn selection(&self) -> Selection {
        if let Some(summary) = self.open_summary() {
            return Selection {
                provider: summary.provider.clone(),
                model: summary.model.clone(),
                effort: summary.effort.clone(),
            };
        }
        self.draft_selection
            .clone()
            .unwrap_or_else(|| self.defaults.clone())
    }

    pub fn model_label(&self, selection: &Selection) -> String {
        self.providers
            .iter()
            .find(|p| p.id == selection.provider)
            .and_then(|p| p.models.iter().find(|m| m.id == selection.model))
            .map_or_else(|| selection.model.clone(), |m| m.label.clone())
    }

    /// Providers the daemon can use (signed in, with models).
    pub fn usable_providers(&self) -> Vec<&Provider> {
        self.providers
            .iter()
            .filter(|p| !p.models.is_empty() && self.auth.get(&p.id).copied().unwrap_or(false))
            .collect()
    }

    /// The bootstrap, sent on every (re)connection.
    pub fn on_connected(&mut self) -> Vec<Value> {
        self.connected = true;
        self.offline = false;
        self.quiet_until = None;
        self.was_connected = true;
        let mut commands = vec![
            proto::client_capabilities(&proto::APP_CAPABILITIES, Some(24)),
            proto::ping(&format!("ping-{}", now_ms())),
        ];
        // The open conversation comes back with the list, in the same write.
        let reopen = self.pending_open.take().or_else(|| match self.page {
            Page::Chat => self.chat.id.clone(),
            Page::List => None,
        });
        if let Some(id) = reopen {
            commands.extend(self.open(id));
        }
        commands
    }

    pub fn on_disconnected(&mut self) {
        self.connected = false;
        self.offline = self.quiet_until.is_none();
        self.load_request = None;
        self.refresh_request = None;
        self.chat.older_request = None;
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

    fn open(&mut self, id: String) -> Vec<Value> {
        self.save_draft();
        let changed = self.chat.id.as_deref() != Some(id.as_str());
        let previous = self.chat.id.clone();
        if changed {
            let chat = self.unshelve(&id).unwrap_or_else(|| Chat {
                id: Some(id.clone()),
                ..Chat::default()
            });
            let old = std::mem::replace(&mut self.chat, chat);
            self.shelve(old);
            self.composer = self.drafts.get(&id).cloned().unwrap_or_default();
        }
        self.page = Page::Chat;
        self.picker = false;
        let mut commands = Vec::new();
        if let Some(previous) = previous.filter(|_| changed) {
            commands.push(proto::unsubscribe(&previous));
        }
        if !self.connected {
            return commands;
        }
        commands.extend(self.reload());
        commands
    }

    /// Keeps a conversation that is no longer open, to show it at once
    /// when it opens again.
    fn shelve(&mut self, chat: Chat) {
        const KEEP: usize = 8;
        if chat.id.is_none() || !chat.loaded || chat.error.is_some() {
            return;
        }
        self.shelf.retain(|kept| kept.id != chat.id);
        self.shelf.push(chat.shelved());
        if self.shelf.len() > KEEP {
            self.shelf.remove(0);
        }
    }

    fn unshelve(&mut self, id: &str) -> Option<Chat> {
        let index = self
            .shelf
            .iter()
            .position(|chat| chat.id.as_deref() == Some(id))?;
        let mut chat = self.shelf.remove(index);
        chat.touch();
        Some(chat)
    }

    /// A tapped notification (or a conversation the app was asked to show).
    pub fn open_target(&mut self, id: &str) -> Vec<Value> {
        if !self.connected {
            self.pending_open = Some(id.to_owned());
            self.save_draft();
            self.page = Page::Chat;
            if self.chat.id.as_deref() != Some(id) {
                let chat = self.unshelve(id).unwrap_or_else(|| Chat {
                    id: Some(id.to_owned()),
                    ..Chat::default()
                });
                let old = std::mem::replace(&mut self.chat, chat);
                self.shelve(old);
            }
            return Vec::new();
        }
        self.open(id.to_owned())
    }

    fn save_draft(&mut self) {
        let text = std::mem::take(&mut self.composer);
        match &self.chat.id {
            Some(id) if !text.trim().is_empty() => {
                self.drafts.insert(id.clone(), text);
            }
            Some(id) => {
                self.drafts.remove(id);
            }
            None => {}
        }
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

    /// A tool's output is not in the history the daemon sent.
    fn wants_output(&self, tool: &str) -> bool {
        if self.chat.outputs.contains_key(tool) || self.chat.outputs_requested.contains(tool) {
            return false;
        }
        let known = |blocks: &[Block]| {
            blocks.iter().any(|block| {
                matches!(block, Block::ToolResult { tool_call_id, output, .. }
                    if tool_call_id == tool && !output.is_empty())
            })
        };
        let in_history = self.chat.entries.iter().any(|entry| match entry {
            Entry::Ai { blocks, .. } => known(blocks),
            _ => false,
        });
        !in_history
            && !self
                .chat
                .live
                .as_ref()
                .is_some_and(|live| known(&live.blocks))
    }

    fn send(&mut self) -> Vec<Value> {
        let text = self.composer.trim().to_owned();
        if text.is_empty() || !self.connected {
            return Vec::new();
        }
        self.composer.clear();
        let started_at = now_ms();
        let request = self.request_id();
        match self.chat.id.clone() {
            Some(id) => {
                self.drafts.remove(&id);
                if self.streaming() {
                    // Joins the running turn, as the TUI's Enter does.
                    let queue_id = format!("phone-{started_at}");
                    self.queued.push(Queued {
                        id: queue_id.clone(),
                        conv_id: id.clone(),
                        text: text.clone(),
                    });
                    self.chat.touch();
                    return vec![proto::queue_message(&request, &queue_id, &id, &text)];
                }
                self.chat.echoes.push(Echo {
                    text: text.clone(),
                    started_at,
                });
                self.chat.touch();
                vec![proto::send_message(&request, &id, &text, started_at)]
            }
            None => {
                let id = format!(
                    "{started_at}-{:06x}",
                    started_at.wrapping_mul(2654435761) & 0xff_ffff
                );
                let selection = self.selection();
                let mut command =
                    proto::new_conversation(&request, &id, &selection, &text, started_at);
                if let Some(folder) = self.current_folder() {
                    command["folderId"] = Value::String(folder.to_owned());
                }
                self.chat = Chat {
                    id: Some(id),
                    loaded: true,
                    ..Chat::default()
                };
                // The daemon echoes no user_message for a conversation's
                // first message: it is the conversation's first entry.
                self.chat.push(Entry::User {
                    text,
                    metadata: None,
                });
                self.chat.live = Some(Live {
                    started_at: started_at as f64,
                    ..Live::default()
                });
                vec![command]
            }
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
                    self.chat.error = Some(message.clone());
                    self.chat.loaded = true;
                    self.chat.touch();
                } else if req_id.is_some() && req_id == self.chat.older_request {
                    self.chat.older_request = None;
                    self.chat.has_older = false;
                } else if conv_id.is_some() && conv_id == self.chat.id {
                    self.notice = Some((message, now));
                }
                Vec::new()
            }
            event => self.apply_stream(event),
        }
    }

    fn apply_stream(&mut self, event: Event) -> Vec<Value> {
        let conv = match &event {
            Event::StreamingStarted { conv_id, .. }
            | Event::StreamingStopped { conv_id, .. }
            | Event::BlockStart { conv_id, .. }
            | Event::TextChunk { conv_id, .. }
            | Event::ThinkingChunk { conv_id, .. }
            | Event::StreamingSync { conv_id, .. }
            | Event::ToolCall { conv_id, .. }
            | Event::ToolResult { conv_id, .. }
            | Event::MessageComplete { conv_id, .. }
            | Event::UserMessage { conv_id, .. }
            | Event::SystemMessage { conv_id, .. }
            | Event::StreamRetry { conv_id, .. } => conv_id.clone(),
            _ => return Vec::new(),
        };
        if self.chat.id.as_deref() != Some(conv.as_str()) {
            return Vec::new();
        }
        let chat = &mut self.chat;
        chat.touch();
        match event {
            Event::StreamingStarted {
                started_at,
                blocks,
                block_offset,
                ..
            } => {
                let live = chat.live_mut();
                live.started_at = started_at;
                if let Some(offset) = block_offset {
                    live.offset = offset as usize;
                }
                if let Some(blocks) = blocks {
                    live.blocks = blocks;
                }
            }
            Event::BlockStart { block_type, .. } => {
                let block = if block_type == "thinking" {
                    Block::Thinking {
                        text: String::new(),
                    }
                } else {
                    Block::Text {
                        text: String::new(),
                    }
                };
                chat.live_mut().blocks.push(block);
            }
            Event::TextChunk { text, .. } => {
                let blocks = &mut chat.live_mut().blocks;
                match blocks.last_mut() {
                    Some(Block::Text { text: tail }) => tail.push_str(&text),
                    _ => blocks.push(Block::Text { text }),
                }
            }
            Event::ThinkingChunk { text, .. } => {
                let blocks = &mut chat.live_mut().blocks;
                match blocks.last_mut() {
                    Some(Block::Thinking { text: tail }) => tail.push_str(&text),
                    _ => blocks.push(Block::Thinking { text }),
                }
            }
            Event::StreamingSync { blocks, .. } => {
                let live = chat.live_mut();
                let cut = live
                    .blocks
                    .iter()
                    .rposition(|b| !matches!(b, Block::Text { .. } | Block::Thinking { .. }))
                    .map_or(0, |i| i + 1);
                live.blocks.truncate(cut);
                live.blocks.extend(blocks);
            }
            Event::ToolCall {
                tool_call_id,
                tool_name,
                input,
                summary,
                ..
            } => chat.live_mut().blocks.push(Block::ToolCall {
                tool_call_id,
                tool_name,
                input,
                summary,
            }),
            Event::ToolResult {
                tool_call_id,
                tool_name,
                output,
                is_error,
                ..
            } => chat.live_mut().blocks.push(Block::ToolResult {
                tool_call_id,
                tool_name,
                output,
                is_error,
            }),
            Event::MessageComplete { blocks, .. } => {
                let live = chat.live_mut();
                if live.offset <= blocks.len() {
                    live.blocks = blocks[live.offset..].to_vec();
                }
                chat.commit_live();
            }
            Event::StreamingStopped {
                persisted_blocks, ..
            } => {
                if let Some(live) = chat.live.as_mut()
                    && let Some(persisted) = persisted_blocks
                    && live.offset <= persisted.len()
                {
                    live.blocks = persisted[live.offset..].to_vec();
                }
                chat.commit_live();
                // Settle on the daemon's own copy of the newest turns.
                if self.connected {
                    let request = self.request_id();
                    self.refresh_request = Some(request.clone());
                    return vec![proto::load_history(&request, &conv, 1 << 40, 0, 3)];
                }
            }
            Event::UserMessage {
                text, started_at, ..
            } => {
                let started = started_at.unwrap_or(0.0) as u64;
                if let Some(index) = chat
                    .echoes
                    .iter()
                    .position(|echo| echo.started_at == started || echo.text == text)
                {
                    chat.echoes.remove(index);
                }
                // A message that joins a running turn splits the reply.
                chat.commit_live();
                chat.push(Entry::User {
                    text,
                    metadata: None,
                });
                if self.open_summary().is_some_and(|s| s.streaming) {
                    let chat = &mut self.chat;
                    chat.live = Some(Live::default());
                }
            }
            Event::SystemMessage { text, color, .. } => {
                chat.push(Entry::System { text, color });
            }
            Event::StreamRetry {
                attempt,
                max_attempts,
                error_message,
                ..
            } => {
                chat.push(Entry::System {
                    text: format!("Retrying ({attempt}/{max_attempts}): {error_message}"),
                    color: Some("warning".into()),
                });
            }
            _ => {}
        }
        Vec::new()
    }

    /// Asks for the open conversation's newest window (which subscribes to
    /// it and marks it read), and the older page after it.
    fn reload(&mut self) -> Vec<Value> {
        match self.chat.id.clone() {
            Some(id) if self.connected => {
                let request = self.request_id();
                let backfill = self.request_id();
                self.load_request = Some(request.clone());
                self.chat.older_request = Some(backfill.clone());
                vec![proto::load_conversation(&request, &id, &backfill)]
            }
            _ => Vec::new(),
        }
    }

    /// The user scrolled near the top of what is loaded.
    pub fn load_older(&mut self) -> Vec<Value> {
        if !self.connected
            || !self.chat.has_older
            || self.chat.older_request.is_some()
            || self.load_request.is_some()
            || !self.chat.loaded
        {
            return Vec::new();
        }
        let Some(id) = self.chat.id.clone() else {
            return Vec::new();
        };
        let request = self.request_id();
        self.chat.older_request = Some(request.clone());
        self.chat.touch();
        vec![proto::load_history(
            &request,
            &id,
            self.chat.start,
            self.chat.start_block,
            proto::OLDER_TURNS,
        )]
    }

    /// Messages waiting to join the open conversation.
    pub fn queued_here(&self) -> Vec<&Queued> {
        self.queued
            .iter()
            .filter(|q| Some(q.conv_id.as_str()) == self.chat.id.as_deref())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;

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
