//! The open conversation (WP-C2): its history window (entries with
//! stamps), the reply streaming into it, older pages, and the
//! conversations kept to reopen at once.
//!
//! Change `Chat::entries` only through `Chat`'s methods: they keep every
//! entry's stamp, which the view caches its layout by.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::Value;

use super::super::proto::{self, Block, Entry, Event};
use super::super::requests::{self, Owner, Pending};
use super::{Model, Page};

/// What the user does in the open conversation.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Retry is C2's (the load error's button).
pub enum Act {
    /// Opens or folds a tool call, a run of them, or a thinking block.
    ToggleExpanded(String),
    /// Asks for the conversation again after its load failed.
    Retry,
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
pub(crate) fn next_stamp() -> u64 {
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
    /// The newest window asked for (its answer replaces what shows; a late
    /// one still lands).
    pub load_request: Option<String>,
    /// The older page asked for.
    pub older_request: Option<String>,
    /// The newest page asked for after a turn stopped, to settle on the
    /// daemon's copy.
    pub refresh_request: Option<String>,
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
    pub(crate) fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub(crate) fn end(&self) -> u64 {
        self.start + self.entries.len() as u64
    }

    pub(crate) fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        self.stamps.push(next_stamp());
    }

    /// Replaces the entries (and where they start), keeping the stamp of
    /// every entry that is the same as before at the same place.
    pub(crate) fn set_entries(&mut self, entries: Vec<Entry>, start: u64, start_block: u64) {
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
    pub(crate) fn splice(&mut self, window: proto::Window, fresh: bool) {
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
    pub(crate) fn prepend(&mut self, window: proto::Window) -> bool {
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

    pub(crate) fn live_mut(&mut self) -> &mut Live {
        self.live.get_or_insert_with(Live::default)
    }

    pub(crate) fn commit_live(&mut self) {
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
    pub(crate) fn shelved(mut self) -> Self {
        self.live = None;
        self.load_request = None;
        self.older_request = None;
        self.refresh_request = None;
        self.outputs_requested.clear();
        self.echoes.clear();
        self.error = None;
        self
    }
}

/// Conversations opened before, most recent last: reopening one shows it
/// at once while the daemon's fresh window is on its way.
#[derive(Debug, Default)]
pub struct Shelf {
    chats: Vec<Chat>,
}

impl Shelf {
    const KEEP: usize = 8;

    /// Keeps a conversation that is no longer open.
    pub fn keep(&mut self, chat: Chat) {
        if chat.id.is_none() || !chat.loaded || chat.error.is_some() {
            return;
        }
        self.chats.retain(|kept| kept.id != chat.id);
        self.chats.push(chat.shelved());
        if self.chats.len() > Self::KEEP {
            self.chats.remove(0);
        }
    }

    pub fn take(&mut self, id: &str) -> Option<Chat> {
        let index = self
            .chats
            .iter()
            .position(|chat| chat.id.as_deref() == Some(id))?;
        let mut chat = self.chats.remove(index);
        chat.touch();
        Some(chat)
    }

    pub fn forget(&mut self, id: &str) {
        self.chats.retain(|chat| chat.id.as_deref() != Some(id));
    }
}

impl Model {
    /// Shows conversation `id`, and asks for its newest window.
    pub(super) fn open(&mut self, id: String) -> Vec<Value> {
        self.save_draft();
        let changed = self.chat.id.as_deref() != Some(id.as_str());
        let previous = self.chat.id.clone();
        if changed {
            let chat = self.shelf.take(&id).unwrap_or_else(|| Chat {
                id: Some(id.clone()),
                ..Chat::default()
            });
            let old = std::mem::replace(&mut self.chat, chat);
            self.shelf.keep(old);
            self.restore_draft();
            self.ui.close_scoped = true;
        }
        if self.page != Page::Chat {
            self.ui.close_all = true;
        }
        self.page = Page::Chat;
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

    /// A new conversation, composed in the chat page (in the folder the
    /// list shows).
    pub(super) fn new_conversation(&mut self) -> Vec<Value> {
        self.save_draft();
        let previous = self.chat.id.clone();
        let draft = Chat {
            loaded: true,
            ..Chat::default()
        };
        let old = std::mem::replace(&mut self.chat, draft);
        self.shelf.keep(old);
        self.ops.draft_selection = None;
        self.ui.close_all = true;
        self.page = Page::Chat;
        previous
            .map(|id| proto::unsubscribe(&id))
            .into_iter()
            .collect()
    }

    /// Back from the chat to the list.
    pub(super) fn leave_chat(&mut self) -> Vec<Value> {
        self.save_draft();
        self.page = Page::List;
        let id = self.chat.id.clone();
        let old = std::mem::take(&mut self.chat);
        self.shelf.keep(old);
        // Not viewing it any more: the daemon may mark it unread.
        id.filter(|_| self.connected)
            .map(|id| proto::unsubscribe(&id))
            .into_iter()
            .collect()
    }

    /// A tapped notification (or a conversation the app was asked to show).
    pub fn open_target(&mut self, id: &str) -> Vec<Value> {
        if !self.connected {
            self.pending_open = Some(id.to_owned());
            self.save_draft();
            self.ui.close_all = true;
            self.page = Page::Chat;
            if self.chat.id.as_deref() != Some(id) {
                let chat = self.shelf.take(id).unwrap_or_else(|| Chat {
                    id: Some(id.to_owned()),
                    ..Chat::default()
                });
                let old = std::mem::replace(&mut self.chat, chat);
                self.shelf.keep(old);
            }
            return Vec::new();
        }
        self.open(id.to_owned())
    }

    pub(super) fn handle_chat(&mut self, act: Act) -> Vec<Value> {
        match act {
            Act::ToggleExpanded(key) => {
                let mut commands = Vec::new();
                if !self.chat.expanded.remove(&key) {
                    self.chat.expanded.insert(key.clone());
                    if let Some(tool) = key.strip_prefix("tool:")
                        && self.wants_output(tool)
                        && let (Some(id), true) = (self.chat.id.clone(), self.connected)
                    {
                        self.chat.outputs_requested.insert(tool.to_owned());
                        let request = self.requests.issue(
                            Owner::Chat,
                            "outputs",
                            Some(&id),
                            requests::DEFAULT_WITHIN,
                            Instant::now(),
                        );
                        commands.push(proto::load_tool_outputs(&request, &id, &[tool.to_owned()]));
                    }
                }
                self.chat.touch();
                commands
            }
            Act::Retry => {
                self.chat.error = None;
                self.chat.touch();
                self.reload()
            }
        }
    }

    /// A tool's output is not in the history the daemon sent.
    pub(crate) fn wants_output(&self, tool: &str) -> bool {
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

    /// The connection dropped: what was asked will not be answered. The
    /// live tail is unknown until the conversation is reloaded (a quiet
    /// reconnection keeps showing it until then).
    pub(super) fn chat_disconnected(&mut self, quiet: bool) {
        self.chat.load_request = None;
        self.chat.older_request = None;
        self.chat.refresh_request = None;
        self.chat.outputs_requested.clear();
        if !quiet {
            self.chat.live = None;
        }
    }

    /// The open conversation's events (and deletions of any).
    pub(super) fn apply_chat(&mut self, event: Event, _now: Instant) -> Vec<Value> {
        match event {
            Event::ConversationDeleted { conv_id, .. } => {
                self.shelf.forget(&conv_id);
                if self.chat.id.as_deref() == Some(&conv_id) {
                    self.ui.close_all = true;
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
                let fresh = req_id.is_some() && req_id == self.chat.load_request;
                if fresh {
                    self.chat.load_request = None;
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
                    let asked = self.chat.older_request.take();
                    self.chat.older_request = if kept { None } else { backfill };
                    if let Some(asked) = asked
                        && self.chat.older_request.as_ref() != Some(&asked)
                    {
                        self.requests.forget(&asked);
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
                ..
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
                } else if req_id.is_some() && req_id == self.chat.refresh_request {
                    self.chat.refresh_request = None;
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
                self.shelf.forget(&conv_id);
                if self.chat.id.as_deref() == Some(&conv_id) {
                    self.reload()
                } else {
                    Vec::new()
                }
            }
            Event::ToolOutputsLoaded {
                conv_id, outputs, ..
            } => {
                if self.chat.id.as_deref() == Some(&conv_id) {
                    for output in outputs {
                        self.chat.outputs.insert(output.tool_call_id, output.output);
                    }
                    self.chat.touch();
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
                    Block::thinking("")
                } else {
                    Block::text("")
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
                    Some(Block::Thinking { text: tail, .. }) => tail.push_str(&text),
                    _ => blocks.push(Block::thinking(text)),
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
                presentation,
                ..
            } => chat.live_mut().blocks.push(Block::ToolCall {
                tool_call_id,
                tool_name,
                input,
                summary,
                presentation,
                elided: None,
            }),
            Event::ToolResult {
                tool_call_id,
                tool_name,
                output,
                is_error,
                images,
                ..
            } => chat.live_mut().blocks.push(Block::ToolResult {
                tool_call_id,
                tool_name,
                output,
                is_error,
                images,
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
                    let request = self.requests.issue(
                        Owner::Chat,
                        "refresh",
                        Some(&conv),
                        requests::DEFAULT_WITHIN,
                        Instant::now(),
                    );
                    self.chat.refresh_request = Some(request.clone());
                    return vec![proto::load_history(&request, &conv, 1 << 40, 0, 3)];
                }
            }
            Event::UserMessage {
                text,
                started_at,
                images,
                ..
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
                    images,
                    metadata: None,
                });
                if self.open_summary().is_some_and(|s| s.streaming) {
                    let chat = &mut self.chat;
                    chat.live = Some(Live::default());
                }
            }
            Event::SystemMessage { text, color, .. } => {
                chat.push(Entry::System {
                    text,
                    color,
                    metadata: None,
                });
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
                    metadata: None,
                });
            }
            _ => {}
        }
        Vec::new()
    }

    /// Asks for the open conversation's newest window (which subscribes to
    /// it and marks it read), and the older page after it.
    pub(super) fn reload(&mut self) -> Vec<Value> {
        match self.chat.id.clone() {
            Some(id) if self.connected => {
                let now = Instant::now();
                let request =
                    self.requests
                        .issue(Owner::Chat, "load", Some(&id), requests::LOAD_WITHIN, now);
                let backfill = self.requests.issue(
                    Owner::Chat,
                    "older",
                    Some(&id),
                    requests::LOAD_WITHIN,
                    now,
                );
                self.chat.load_request = Some(request.clone());
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
            || self.chat.load_request.is_some()
            || !self.chat.loaded
        {
            return Vec::new();
        }
        let Some(id) = self.chat.id.clone() else {
            return Vec::new();
        };
        let request = self.requests.issue(
            Owner::Chat,
            "older",
            Some(&id),
            requests::LOAD_WITHIN,
            Instant::now(),
        );
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

    /// An error answering request `id`: the chat's when it is its window or
    /// older page (late ones included). True when handled.
    pub(super) fn chat_error(&mut self, id: &str, message: &str) -> bool {
        if self.chat.load_request.as_deref() == Some(id) {
            self.chat.load_request = None;
            self.chat.error = Some(message.to_owned());
            self.chat.loaded = true;
            self.chat.touch();
            true
        } else if self.chat.older_request.as_deref() == Some(id) {
            self.chat.older_request = None;
            self.chat.has_older = false;
            self.chat.touch();
            true
        } else {
            false
        }
    }

    /// One of the chat's requests was not answered in time. The window's
    /// late answer still lands (its id stays); an older page is asked for
    /// again on the next scroll up.
    pub(super) fn chat_timeout(&mut self, id: &str, pending: &Pending) -> bool {
        if pending.conv != self.chat.id {
            return false;
        }
        match pending.kind {
            "load" if self.chat.load_request.as_deref() == Some(id) => {
                self.chat.loaded = true;
                self.chat.error =
                    Some("Exocortex did not answer. Open the conversation again to retry.".into());
                self.chat.touch();
                true
            }
            "older" if self.chat.older_request.as_deref() == Some(id) => {
                self.chat.older_request = None;
                self.chat.touch();
                true
            }
            "outputs" => {
                self.chat.outputs_requested.clear();
                self.chat.touch();
                true
            }
            _ => false,
        }
    }
}
