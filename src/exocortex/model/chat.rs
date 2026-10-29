//! The open conversation: its history window (entries with stamps), the
//! reply streaming into it, older pages, and the conversations kept to
//! reopen at once.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::Value;

use super::super::proto::{self, Block, Entry, Event};
use super::{ANSWER_WITHIN, Model, Page};

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
    pub older_request: Option<String>,
    pub(crate) older_sent: Option<Instant>,
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
        self.older_request = None;
        self.older_sent = None;
        self.outputs_requested.clear();
        self.echoes.clear();
        self.error = None;
        self
    }
}

impl Model {
    pub(crate) fn open(&mut self, id: String) -> Vec<Value> {
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
    pub(crate) fn shelve(&mut self, chat: Chat) {
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

    pub(crate) fn unshelve(&mut self, id: &str) -> Option<Chat> {
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

    pub(crate) fn apply_stream(&mut self, event: Event) -> Vec<Value> {
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
    pub(crate) fn reload(&mut self) -> Vec<Value> {
        match self.chat.id.clone() {
            Some(id) if self.connected => {
                let request = self.request_id();
                let backfill = self.request_id();
                let now = Instant::now();
                self.load_request = Some(request.clone());
                self.load_sent = Some(now);
                self.chat.older_request = Some(backfill.clone());
                self.chat.older_sent = Some(now);
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
        self.chat.older_sent = Some(Instant::now());
        self.chat.touch();
        vec![proto::load_history(
            &request,
            &id,
            self.chat.start,
            self.chat.start_block,
            proto::OLDER_TURNS,
        )]
    }

    /// Gives up on what the daemon has not answered in `ANSWER_WITHIN`;
    /// returns whether that changed what shows.
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self
            .load_sent
            .is_some_and(|at| now.duration_since(at) >= ANSWER_WITHIN)
        {
            // load_request stays: a late answer still lands.
            self.load_sent = None;
            if self.load_request.is_some() {
                self.chat.loaded = true;
                self.chat.error =
                    Some("Exocortex did not answer. Open the conversation again to retry.".into());
                self.chat.touch();
                changed = true;
            }
        }
        if self
            .chat
            .older_sent
            .is_some_and(|at| now.duration_since(at) >= ANSWER_WITHIN)
        {
            self.chat.older_sent = None;
            self.chat.older_request = None;
            self.chat.touch();
            changed = true;
        }
        changed
    }

    /// When `expire` next has something to do.
    pub fn answer_deadline(&self) -> Option<Instant> {
        [self.load_sent, self.chat.older_sent]
            .into_iter()
            .flatten()
            .min()
            .map(|at| at + ANSWER_WITHIN)
    }
}
