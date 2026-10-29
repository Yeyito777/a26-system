//! The open conversation (WP-C2): its history window (entries with
//! stamps), the reply streaming into it, its live status (compaction,
//! waits, retries), tool outputs fetched on demand, older pages, the
//! message menu, and the conversations kept to reopen at once.
//!
//! Change `Chat::entries` only through `Chat`'s methods: they keep every
//! entry's stamp, which the view caches its layout by.
//!
//! The live reply has a stamp too, which the entry it becomes keeps: one
//! layout identity per turn from its first chunk through completion and
//! the daemon's own copy (PAR-155, 162), so the reader's place and the
//! pieces on screen survive all of it.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::Value;

use super::super::proto::{self, Block, Entry, Event, Metadata};
use super::super::requests::{self, Owner, Pending};
use super::super::ui::icons::Icon;
use super::super::ui::sheet::{ActionItem, ActionList, action_sheet};
use super::{Action, Model, Page, UiAct, composer, now_ms};

/// What the user does in the open conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Opens or folds a tool call, a run of them, a thinking block, a
    /// notice or a notification.
    ToggleExpanded(String),
    /// Asks for the conversation again after its load failed.
    Retry,
    /// Asks for the older page again after it failed.
    RetryOlder,
    /// The jump-to-latest button: back to the newest message, following it
    /// (the view stops any fling).
    JumpToLatest,
    /// The message menu of the entry (or live reply) with this stamp.
    MessageMenu(u64),
    /// The menu of a queued message (by its queue id).
    QueuedMenu(String),
    /// A tool call's full output in the reader, or copied.
    ReadOutput(String),
    CopyOutput(String),
    /// Asks for a tool's output again (missing or timed out).
    RetryOutput(String),
    /// The thinking with this key (of the entry with this stamp) in the
    /// reader.
    ReadThinking(u64, String),
    /// A notice's text copied (the entry with this stamp).
    CopyNotice(u64),
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
    /// Blocks of this turn already committed as segments (a message
    /// joined the turn): the canonical tail at completion is cut by them.
    pub committed: Vec<Block>,
    /// The blocks are the daemon's snapshot (the next one is adopted as
    /// is); false once local chunks grew them.
    pub hydrated: bool,
    /// What its footer shows: model, tokens, throughput, work timer.
    pub meta: Metadata,
    /// Its identity (0: none yet): the entry it is committed into keeps it.
    pub stamp: u64,
}

/// A retry the daemon is waiting out (`stream_retry`).
#[derive(Debug, Clone, PartialEq)]
pub struct Retry {
    pub message: String,
    pub attempt: u32,
    pub max_attempts: u32,
    /// When it retries (epoch ms).
    pub at: f64,
    /// A usage limit's reset it waits for (epoch ms).
    pub reset_at: Option<f64>,
}

/// A wait for the network or a usage limit (`connection_wait_status`).
#[derive(Debug, Clone, PartialEq)]
pub struct Wait {
    pub label: Option<String>,
    pub started_at: f64,
}

/// The live status lines under the reply (PAR-144, 158–160).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub retry: Option<Retry>,
    /// Compacting since (epoch ms).
    pub compaction: Option<f64>,
    pub wait: Option<Wait>,
}

impl Status {
    pub fn any(&self) -> bool {
        self.retry.is_some() || self.compaction.is_some() || self.wait.is_some()
    }
}

/// A new entry identity (`Chat::stamps`): unique in the process, so layouts
/// cached by stamp stay valid across conversations.
pub(crate) fn next_stamp() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Shown as a notice when a fetched output is not there.
pub const OUTPUT_MISSING: &str = "This result is no longer available. Tap to retry.";
pub const OUTPUT_TIMED_OUT: &str = "Tool result timed out. Tap to retry.";
/// The daemon's persisted compaction divider.
pub const COMPACTION_KIND: &str = "context_compaction_finished";
pub const COMPACTION_TEXT: &str = "--- Compaction finished ---";
/// Outputs prefetched near the screen per request.
const PREFETCH_BATCH: usize = 6;

/// Whether a block shows anything (text, thinking with words, a tool).
pub fn visible(block: &Block) -> bool {
    match block {
        Block::Text { text } | Block::Thinking { text, .. } => !text.trim().is_empty(),
        Block::Other => false,
        _ => true,
    }
}

fn any_visible(blocks: &[Block]) -> bool {
    blocks.iter().any(visible)
}

/// Whether two entries show the same: a reply by its blocks (its metadata
/// shows only in the footer, worked out apart), a message by its words.
fn same_content(a: &Entry, b: &Entry) -> bool {
    match (a, b) {
        (Entry::Ai { blocks: a, .. }, Entry::Ai { blocks: b, .. }) => a == b,
        (
            Entry::User {
                text: a_text,
                images: a_images,
                metadata: a_meta,
            },
            Entry::User {
                text: b_text,
                images: b_images,
                metadata: b_meta,
            },
        ) => {
            let kind = |m: &Option<Metadata>| {
                m.as_ref()
                    .map(|m| (m.system, m.automation.clone()))
                    .unwrap_or_default()
            };
            a_text == b_text && a_images.len() == b_images.len() && kind(a_meta) == kind(b_meta)
        }
        (a, b) => a == b,
    }
}

/// Whether `snapshot` strictly extends `local`: every local block maps in
/// order onto a snapshot block (text by prefix, tools by id) and the
/// snapshot has more blocks or longer text.
fn strict_extension(local: &[Block], snapshot: &[Block]) -> bool {
    if snapshot.len() < local.len() {
        return false;
    }
    let mut longer = snapshot.len() > local.len();
    for (l, s) in local.iter().zip(snapshot) {
        match (l, s) {
            (Block::Text { text: l }, Block::Text { text: s })
            | (Block::Thinking { text: l, .. }, Block::Thinking { text: s, .. }) => {
                if !s.starts_with(l.as_str()) {
                    return false;
                }
                longer |= s.len() > l.len();
            }
            (
                Block::ToolCall {
                    tool_call_id: l, ..
                },
                Block::ToolCall {
                    tool_call_id: s, ..
                },
            ) if l == s => {}
            (
                Block::ToolResult {
                    tool_call_id: l_id,
                    output: l_out,
                    is_error: l_error,
                    ..
                },
                Block::ToolResult {
                    tool_call_id: s_id,
                    output: s_out,
                    is_error: s_error,
                    ..
                },
            ) => {
                if l_id != s_id
                    || l_error != s_error
                    || !(s_out.starts_with(l_out.as_str()) || l_out.starts_with(s_out.as_str()))
                {
                    return false;
                }
                longer |= s_out.len() > l_out.len();
            }
            _ => return false,
        }
    }
    longer
}

/// The snapshot, keeping a fuller local tool output.
fn merge_snapshot(local: &[Block], snapshot: Vec<Block>) -> Vec<Block> {
    snapshot
        .into_iter()
        .enumerate()
        .map(|(index, block)| match (block, local.get(index)) {
            (
                Block::ToolResult {
                    tool_call_id,
                    tool_name,
                    output,
                    is_error,
                    images,
                },
                Some(Block::ToolResult {
                    tool_call_id: id,
                    output: fuller,
                    ..
                }),
            ) if *id == tool_call_id && fuller.len() > output.len() => Block::ToolResult {
                tool_call_id,
                tool_name,
                output: fuller.clone(),
                is_error,
                images,
            },
            (block, _) => block,
        })
        .collect()
}

/// A canonical turn tail without what was already committed as segments;
/// any mismatch keeps it all (never lose text).
fn subtract_committed(canonical: &[Block], committed: &[Block]) -> Vec<Block> {
    let mut out = canonical.to_vec();
    for done in committed {
        let Some(first) = out.first() else {
            return canonical.to_vec();
        };
        match (first, done) {
            (Block::Text { text }, Block::Text { text: done })
            | (Block::Thinking { text, .. }, Block::Thinking { text: done, .. }) => {
                let Some(rest) = text.strip_prefix(done.as_str()) else {
                    return canonical.to_vec();
                };
                if rest.is_empty() {
                    out.remove(0);
                } else {
                    let rest = rest.to_owned();
                    match &mut out[0] {
                        Block::Text { text } | Block::Thinking { text, .. } => *text = rest,
                        _ => {}
                    }
                }
            }
            (
                Block::ToolCall { tool_call_id, .. } | Block::ToolResult { tool_call_id, .. },
                Block::ToolCall {
                    tool_call_id: id, ..
                }
                | Block::ToolResult {
                    tool_call_id: id, ..
                },
            ) if std::mem::discriminant(first) == std::mem::discriminant(done)
                && tool_call_id == id =>
            {
                out.remove(0);
            }
            _ => return canonical.to_vec(),
        }
    }
    out
}

/// A conversation: a window of its history (entry `start` onward, from
/// block `start_block` of that entry) and what is streaming.
#[derive(Debug, Default)]
pub struct Chat {
    /// None while composing a new conversation.
    pub id: Option<String>,
    pub entries: Vec<Entry>,
    /// One per entry, changed exactly when what that entry shows changes (a
    /// window that resends an entry unchanged keeps its stamp; a reply's
    /// metadata alone does not change it): what the view caches an entry's
    /// layout by.
    pub stamps: Vec<u64>,
    pub start: u64,
    pub start_block: u64,
    pub has_older: bool,
    /// The newest window asked for (its answer replaces what shows; a late
    /// one still lands).
    pub load_request: Option<String>,
    /// The older page asked for.
    pub older_request: Option<String>,
    /// The older page failed: a Retry at the top, no asking on its own.
    pub older_error: Option<String>,
    /// The newest page asked for after a turn stopped, to settle on the
    /// daemon's copy.
    pub refresh_request: Option<String>,
    pub loaded: bool,
    pub live: Option<Live>,
    pub status: Status,
    /// The entry a durable sleep suspended (its footer in the warning
    /// colour), until the next turn starts.
    pub suspended: Option<u64>,
    pub echoes: Vec<Echo>,
    /// Disclosures (tool calls, thinking, notices) the user toggled away
    /// from their default (`Chat::is_open`).
    pub expanded: HashSet<String>,
    /// Tool outputs fetched on demand (history leaves them out).
    pub outputs: HashMap<String, String>,
    pub outputs_requested: HashSet<String>,
    /// Outputs that did not come (missing, timed out), with what to say.
    pub output_errors: HashMap<String, String>,
    /// Which tool ids each outputs request asked for.
    pub outputs_inflight: HashMap<String, Vec<String>>,
    /// Bumped on every change: whether the view lays the chat out again.
    pub version: u64,
    pub error: Option<String>,
}

/// What opens by default (Settings: Expand thinking, Expand tool calls).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Disclosure {
    pub thinking: bool,
    pub tools: bool,
}

impl Disclosure {
    /// As the settings say (read field by field: the layout asks often).
    pub fn of(persist: &super::super::store::Persist) -> Self {
        use super::super::store::SETTINGS;
        Self {
            thinking: persist.section(SETTINGS, "expandThinking"),
            tools: persist.section(SETTINGS, "expandToolCalls"),
        }
    }

    fn default_open(self, key: &str) -> bool {
        (self.thinking && key.starts_with("think:")) || (self.tools && key.starts_with("tool:"))
    }
}

impl Chat {
    /// Whether the disclosure at `key` is open: its default, unless the
    /// user toggled it.
    pub fn is_open(&self, key: &str, defaults: Disclosure) -> bool {
        defaults.default_open(key) != self.expanded.contains(key)
    }

    pub(crate) fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub(crate) fn end(&self) -> u64 {
        self.start + self.entries.len() as u64
    }

    pub(crate) fn push(&mut self, entry: Entry) {
        self.push_with(entry, next_stamp());
    }

    fn push_with(&mut self, entry: Entry, stamp: u64) {
        self.entries.push(entry);
        self.stamps.push(stamp);
    }

    /// The entry (or the live reply's blocks) with this stamp.
    pub fn by_stamp(&self, stamp: u64) -> Option<&Entry> {
        let index = self.stamps.iter().rposition(|s| *s == stamp)?;
        self.entries.get(index)
    }

    /// Replaces the entries (and where they start), keeping the stamp of
    /// every entry that shows the same as before at the same place.
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
                    Some(i) if same_content(&self.entries[i], entry) => self.stamps[i],
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
                        (old, new) => same_content(old, new),
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
            // resends showing the same keep their stamps.
            let old = self.entries.split_off(keep);
            let old_stamps = self.stamps.split_off(keep);
            for (index, entry) in incoming.into_iter().enumerate() {
                let stamp = match old.get(index) {
                    Some(before) if same_content(before, &entry) => old_stamps[index],
                    _ => next_stamp(),
                };
                self.push_with(entry, stamp);
            }
        }
        if let Some(Some(pending)) = window.pending_ai {
            self.adopt_pending(pending);
        }
        self.loaded = true;
        self.touch();
    }

    /// The daemon's live tail (a window's `pendingAI`): the reply as it
    /// is, its identity kept when it is the same turn.
    fn adopt_pending(&mut self, pending: proto::PendingAi) {
        let started = pending
            .metadata
            .as_ref()
            .map(|m| m.started_at)
            .or(self.live.as_ref().map(|live| live.started_at))
            .unwrap_or(0.0);
        let same = self
            .live
            .as_ref()
            .is_some_and(|live| live.started_at == started && live.stamp != 0);
        let previous = self.live.take().unwrap_or_default();
        let meta = pending.metadata.unwrap_or_else(|| Metadata {
            started_at: started,
            model: previous.meta.model.clone(),
            ..Metadata::default()
        });
        self.live = Some(Live {
            blocks: pending.blocks,
            started_at: started,
            offset: pending.block_offset as usize,
            committed: Vec::new(),
            hydrated: true,
            meta,
            stamp: if same { previous.stamp } else { next_stamp() },
        });
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
        self.older_error = None;
        self.touch();
        true
    }

    pub(crate) fn live_mut(&mut self) -> &mut Live {
        let live = self.live.get_or_insert_with(Live::default);
        if live.stamp == 0 {
            live.stamp = next_stamp();
            live.hydrated = live.blocks.is_empty();
            if live.meta.started_at == 0.0 {
                live.meta.started_at = live.started_at;
            }
        }
        live
    }

    fn commit(&mut self, blocks: Vec<Block>, meta: Metadata, stamp: u64) {
        if any_visible(&blocks) {
            let stamp = if stamp == 0 { next_stamp() } else { stamp };
            self.push_with(
                Entry::Ai {
                    blocks,
                    metadata: Some(meta),
                },
                stamp,
            );
        }
    }

    /// Something joins the running turn (a message, a notice): what the
    /// reply showed so far becomes its own segment, and the turn goes on
    /// in a fresh live reply.
    fn commit_segment(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if !any_visible(&live.blocks) {
            return;
        }
        let blocks = std::mem::take(&mut live.blocks);
        live.committed.extend(blocks.iter().cloned());
        let meta = live.meta.clone();
        let stamp = std::mem::replace(&mut live.stamp, next_stamp());
        self.commit(blocks, meta, stamp);
    }

    /// The turn's canonical blocks (`message_complete`, a stop's persisted
    /// blocks) without what this reply's segments already show.
    fn tail_of(live: &Live, canonical: &[Block]) -> Vec<Block> {
        if live.offset <= canonical.len() {
            subtract_committed(&canonical[live.offset..], &live.committed)
        } else {
            live.blocks.clone()
        }
    }

    /// The chat as kept while another is open: its history, without what
    /// only a live subscription keeps current.
    pub(crate) fn shelved(mut self) -> Self {
        self.live = None;
        self.status = Status::default();
        self.load_request = None;
        self.older_request = None;
        self.older_error = None;
        self.refresh_request = None;
        self.outputs_requested.clear();
        self.outputs_inflight.clear();
        self.output_errors.clear();
        self.echoes.clear();
        self.error = None;
        self
    }

    /// Where a tool's output is: in its result block, or fetched.
    pub fn output_of(&self, tool: &str) -> Option<&str> {
        fn in_blocks<'a>(blocks: &'a [Block], tool: &str) -> Option<&'a str> {
            blocks.iter().find_map(|block| match block {
                Block::ToolResult {
                    tool_call_id,
                    output,
                    ..
                } if tool_call_id == tool && !output.is_empty() => Some(output.as_str()),
                _ => None,
            })
        }
        self.live
            .as_ref()
            .and_then(|live| in_blocks(&live.blocks, tool))
            .or_else(|| {
                self.entries.iter().rev().find_map(|entry| match entry {
                    Entry::Ai { blocks, .. } => in_blocks(blocks, tool),
                    _ => None,
                })
            })
            .or_else(|| self.outputs.get(tool).map(String::as_str))
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

/// A message's words as text (Copy message, Read in full; PAR-142): a
/// reply's blocks joined by blank lines, tool calls as their lines, tool
/// outputs when known.
pub fn plain_text(chat: &Chat, entry: &Entry, label: &dyn Fn(&str) -> String) -> String {
    match entry {
        Entry::User { text, images, .. } => {
            let mut parts = vec![text.trim().to_owned()];
            if !images.is_empty() {
                let s = if images.len() == 1 { "" } else { "s" };
                parts.push(format!("{} image attachment{s}", images.len()));
            }
            parts.retain(|part| !part.is_empty());
            parts.join("\n\n")
        }
        Entry::Ai { blocks, .. } => blocks_text(chat, blocks, label),
        Entry::System { text, .. } | Entry::SystemInstructions { text } => text.clone(),
        Entry::Other => String::new(),
    }
}

pub fn blocks_text(chat: &Chat, blocks: &[Block], label: &dyn Fn(&str) -> String) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        let text = match block {
            Block::Text { text } | Block::Thinking { text, .. } => {
                text.trim_matches('\n').to_owned()
            }
            Block::ToolCall {
                tool_name,
                summary,
                presentation,
                ..
            } => {
                let detail = presentation
                    .as_ref()
                    .and_then(|p| p.detail.clone())
                    .unwrap_or_else(|| summary.clone());
                format!("{} {}", label(tool_name), detail).trim().to_owned()
            }
            Block::ToolResult {
                tool_call_id,
                output,
                ..
            } => {
                let output = if output.is_empty() {
                    chat.outputs.get(tool_call_id).cloned().unwrap_or_default()
                } else {
                    output.clone()
                };
                output.trim_end_matches('\n').to_owned()
            }
            Block::Other => String::new(),
        };
        if !text.trim().is_empty() {
            parts.push(text);
        }
    }
    parts.join("\n\n")
}

/// Merged thinking at `key` of `blocks` (the view's keys).
fn thinking_text(blocks: &[Block], key: &str) -> Option<String> {
    let mut index = 0;
    while index < blocks.len() {
        if matches!(blocks[index], Block::Thinking { .. }) {
            let (end, text) = merged_thinking(blocks, index);
            if thinking_key(blocks, index) == key {
                return Some(text);
            }
            index = end;
        } else {
            index += 1;
        }
    }
    None
}

/// The thinking starting at `from` merged with the thinking right after it
/// (PAR-121): where it ends, and its text without edge newlines.
pub fn merged_thinking(blocks: &[Block], from: usize) -> (usize, String) {
    let mut text = String::new();
    let mut end = from;
    while let Some(Block::Thinking { text: part, .. }) = blocks.get(end) {
        let part = part.trim_matches('\n');
        if !part.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(part);
        }
        end += 1;
    }
    (end, text)
}

/// A thinking block's key: stable within its turn however much of the turn
/// a window holds (the call before it and how far after it, or its index
/// from the start).
pub fn thinking_key(blocks: &[Block], index: usize) -> String {
    let anchor = blocks[..index]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, block)| match block {
            Block::ToolCall { tool_call_id, .. } => Some((tool_call_id.as_str(), at)),
            _ => None,
        });
    match anchor {
        Some((id, at)) => format!("think:{id}:{}", index - at),
        None => format!("think:start:{index}"),
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
                }
                let defaults = Disclosure::of(&self.persist);
                if self.chat.is_open(&key, defaults)
                    && let Some(tool) = key.strip_prefix("tool:")
                {
                    self.chat.output_errors.remove(tool);
                    if self.wants_output(tool) {
                        commands.extend(self.fetch_outputs(vec![tool.to_owned()]));
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
            Act::RetryOlder => {
                self.chat.older_error = None;
                self.chat.touch();
                self.load_older()
            }
            // The view snaps to the newest message (app.rs).
            Act::JumpToLatest => Vec::new(),
            Act::MessageMenu(stamp) => {
                self.message_menu(stamp);
                Vec::new()
            }
            Act::QueuedMenu(id) => {
                self.queued_menu(&id);
                Vec::new()
            }
            Act::ReadOutput(tool) => {
                if let Some(output) = self.chat.output_of(&tool) {
                    let title = self.tool_title(&tool);
                    let text = output.trim_end_matches('\n').to_owned();
                    self.handle_ui(UiAct::Read { title, text });
                }
                Vec::new()
            }
            Act::CopyOutput(tool) => {
                if let Some(output) = self.chat.output_of(&tool) {
                    let text = output.trim_end_matches('\n').to_owned();
                    self.handle_ui(UiAct::Copy(text));
                }
                Vec::new()
            }
            Act::RetryOutput(tool) => {
                self.chat.output_errors.remove(&tool);
                self.chat.outputs_requested.remove(&tool);
                self.chat.touch();
                self.fetch_outputs(vec![tool])
            }
            Act::ReadThinking(stamp, key) => {
                let blocks = self.blocks_of(stamp);
                if let Some(text) = blocks.and_then(|blocks| thinking_text(blocks, &key)) {
                    self.handle_ui(UiAct::Read {
                        title: "Thinking".into(),
                        text,
                    });
                }
                Vec::new()
            }
            Act::CopyNotice(stamp) => {
                if let Some(Entry::System { text, .. } | Entry::User { text, .. }) =
                    self.chat.by_stamp(stamp)
                {
                    let text = text.clone();
                    self.handle_ui(UiAct::Copy(text));
                }
                Vec::new()
            }
        }
    }

    /// The blocks of the entry (or live reply) with this stamp.
    fn blocks_of(&self, stamp: u64) -> Option<&[Block]> {
        match &self.chat.live {
            Some(live) if live.stamp == stamp => Some(&live.blocks),
            _ => match self.chat.by_stamp(stamp)? {
                Entry::Ai { blocks, .. } => Some(blocks),
                _ => None,
            },
        }
    }

    /// The reader's title for a tool's output: its label.
    fn tool_title(&self, tool: &str) -> String {
        let name = self
            .chat
            .live
            .iter()
            .flat_map(|live| live.blocks.iter())
            .chain(self.chat.entries.iter().flat_map(|entry| match entry {
                Entry::Ai { blocks, .. } => blocks.as_slice(),
                _ => &[],
            }))
            .find_map(|block| match block {
                Block::ToolCall {
                    tool_call_id,
                    tool_name,
                    ..
                } if tool_call_id == tool => Some(tool_name.as_str()),
                _ => None,
            });
        match name {
            Some(name) => self.tool_label(name),
            None => "Output".into(),
        }
    }

    /// A tool's label as its rows show it.
    pub fn tool_label(&self, name: &str) -> String {
        super::super::view::tools::tool_label(name, &self.ops.tool_styles, None).0
    }

    /// The message menu (PAR-142, `fake-10-message-actions`): Copy message,
    /// Edit in composer for the user's own words, Read in full.
    fn message_menu(&mut self, stamp: u64) {
        let label = |name: &str| self.tool_label(name);
        let (text, title, user) = match &self.chat.live {
            Some(live) if live.stamp == stamp => (
                blocks_text(&self.chat, &live.blocks, &label),
                "Response",
                false,
            ),
            _ => {
                let Some(entry) = self.chat.by_stamp(stamp) else {
                    return;
                };
                let (title, user) = match entry {
                    Entry::User { metadata, .. } => (
                        "Message",
                        !metadata
                            .as_ref()
                            .is_some_and(|m| m.system || m.automation.is_some()),
                    ),
                    Entry::Ai { .. } => ("Response", false),
                    _ => ("System notice", false),
                };
                (plain_text(&self.chat, entry, &label), title, user)
            }
        };
        if text.trim().is_empty() {
            return;
        }
        let mut items = vec![
            ActionItem::new("Copy message", Action::Ui(UiAct::Copy(text.clone()))).icon(Icon::Copy),
        ];
        if user {
            items.push(
                ActionItem::new(
                    "Edit in composer",
                    Action::Composer(composer::Act::Insert(text.clone())),
                )
                .icon(Icon::Pencil),
            );
        }
        items.push(
            ActionItem::new(
                "Read in full",
                Action::Ui(UiAct::Read {
                    title: title.into(),
                    text,
                }),
            )
            .icon(Icon::FileText),
        );
        self.ui
            .open(Box::new(action_sheet(ActionList::new("", items).scoped())));
    }

    /// A queued message's menu (D adds its queue actions here).
    fn queued_menu(&mut self, id: &str) {
        let Some(queued) = self.composer.queued.iter().find(|q| q.id == id) else {
            return;
        };
        let provider = self.open_summary().map(|s| s.provider.clone());
        let label = queue_label(queued, provider.as_deref(), now_ms() as f64);
        let items = vec![
            ActionItem::new("Copy text", Action::Ui(UiAct::Copy(queued.text.clone())))
                .icon(Icon::Copy),
        ];
        self.ui.open(Box::new(action_sheet(
            ActionList::new("Queued message", items)
                .subtitle(format!("Delivers {}", label.trim_start_matches("queued: ")))
                .scoped(),
        )));
    }

    /// A tool's output is not in the history the daemon sent, nor fetched
    /// or on its way.
    pub(crate) fn wants_output(&self, tool: &str) -> bool {
        if self.chat.outputs.contains_key(tool)
            || self.chat.outputs_requested.contains(tool)
            || self.chat.output_errors.contains_key(tool)
        {
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

    /// Asks for tools' outputs (deduplicated by the caller).
    fn fetch_outputs(&mut self, tools: Vec<String>) -> Vec<Value> {
        let (Some(id), true) = (self.chat.id.clone(), self.connected) else {
            return Vec::new();
        };
        if tools.is_empty() {
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Chat,
            "outputs",
            Some(&id),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        for tool in &tools {
            self.chat.outputs_requested.insert(tool.clone());
        }
        let command = proto::load_tool_outputs(&request, &id, &tools);
        self.chat.outputs_inflight.insert(request, tools);
        vec![command]
    }

    /// Fetches the outputs of tool rows near the screen (PAR-101): `near`
    /// are rows whose result came without its output. One batch at a time.
    pub fn prefetch_outputs(&mut self, near: &[&str]) -> Vec<Value> {
        if !self.chat.outputs_inflight.is_empty() {
            return Vec::new();
        }
        let tools: Vec<String> = near
            .iter()
            .filter(|tool| {
                !self.chat.outputs.contains_key(**tool)
                    && !self.chat.outputs_requested.contains(**tool)
                    && !self.chat.output_errors.contains_key(**tool)
            })
            .take(PREFETCH_BATCH)
            .map(|tool| (*tool).to_owned())
            .collect();
        self.fetch_outputs(tools)
    }

    /// The connection dropped: what was asked will not be answered. The
    /// live tail is unknown until the conversation is reloaded (a quiet
    /// reconnection keeps showing it until then).
    pub(super) fn chat_disconnected(&mut self, quiet: bool) {
        self.chat.load_request = None;
        self.chat.older_request = None;
        self.chat.refresh_request = None;
        self.chat.outputs_requested.clear();
        self.chat.outputs_inflight.clear();
        if !quiet {
            self.chat.live = None;
            self.chat.status = Status::default();
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
                    // The live reply comes again with the window (its
                    // identity kept when it is the same turn).
                    if !streaming {
                        self.chat.live = None;
                    }
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
                    self.chat.status = Status::default();
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
                        Some(None) => {
                            self.chat.live = None;
                            self.chat.status = Status::default();
                        }
                        None if had_live && !self.open_summary().is_some_and(|s| s.streaming) => {
                            self.chat.live = None;
                            self.chat.status = Status::default();
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
                req_id,
                conv_id,
                outputs,
            } => {
                let asked = req_id
                    .as_ref()
                    .and_then(|id| self.chat.outputs_inflight.remove(id));
                if self.chat.id.as_deref() == Some(&conv_id) {
                    for output in outputs {
                        self.chat.outputs_requested.remove(&output.tool_call_id);
                        self.chat.outputs.insert(output.tool_call_id, output.output);
                    }
                    // Asked for and not there: gone from the daemon.
                    for tool in asked.unwrap_or_default() {
                        if self.chat.outputs_requested.remove(&tool) {
                            self.chat.output_errors.insert(tool, OUTPUT_MISSING.into());
                        }
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
            | Event::TokensUpdate { conv_id, .. }
            | Event::MessageComplete { conv_id, .. }
            | Event::UserMessage { conv_id, .. }
            | Event::SystemMessage { conv_id, .. }
            | Event::StreamRetry { conv_id, .. }
            | Event::ContextCompactionStatus { conv_id, .. }
            | Event::ConnectionWaitStatus { conv_id, .. } => conv_id.clone(),
            _ => return Vec::new(),
        };
        if self.chat.id.as_deref() != Some(conv.as_str()) {
            return Vec::new();
        }
        let streaming = self.open_summary().is_some_and(|s| s.streaming);
        let chat = &mut self.chat;
        chat.touch();
        match event {
            Event::StreamingStarted {
                model,
                started_at,
                snapshot_kind,
                work_timer_started_at,
                blocks,
                block_offset,
                tokens,
                generation_throughput,
                compaction_started_at,
                connection_wait_started_at,
                connection_wait_label,
                ..
            } => {
                if let Some(at) = compaction_started_at {
                    chat.status.compaction = Some(at);
                }
                if let Some(at) = connection_wait_started_at {
                    chat.status.wait = Some(Wait {
                        label: connection_wait_label,
                        started_at: at,
                    });
                }
                let start = snapshot_kind.as_deref().is_none_or(|kind| kind == "start");
                if start {
                    chat.status.retry = None;
                    chat.suspended = None;
                }
                let fresh = chat.live.is_none();
                let live = chat.live_mut();
                if fresh {
                    live.hydrated = true;
                }
                // A new clock with nothing shown yet: a new identity (one
                // on screen never changes).
                if live.started_at != started_at
                    && !any_visible(&live.blocks)
                    && live.committed.is_empty()
                {
                    live.stamp = next_stamp();
                }
                live.started_at = started_at;
                live.meta.started_at = started_at;
                live.meta.ended_at = None;
                if !model.is_empty() {
                    live.meta.model = model;
                }
                if work_timer_started_at.is_some() {
                    live.meta.work_timer_started_at = work_timer_started_at;
                }
                if generation_throughput.is_some() {
                    live.meta.generation_throughput = generation_throughput;
                }
                if start {
                    live.offset = block_offset.unwrap_or(0) as usize;
                    live.committed.clear();
                }
                if let Some(blocks) = blocks
                    && (live.hydrated
                        || live.blocks.is_empty()
                        || strict_extension(&live.blocks, &blocks))
                {
                    live.blocks = merge_snapshot(&live.blocks, blocks);
                    live.hydrated = true;
                    if live.committed.is_empty()
                        && let Some(offset) = block_offset
                    {
                        live.offset = offset as usize;
                    }
                }
                if let Some(tokens) = tokens
                    && (live.hydrated || live.meta.tokens == 0 || tokens >= live.meta.tokens)
                {
                    live.meta.tokens = tokens;
                }
            }
            Event::BlockStart { block_type, .. } => {
                let block = if block_type == "thinking" {
                    Block::thinking("")
                } else {
                    Block::text("")
                };
                let live = chat.live_mut();
                live.hydrated = false;
                live.blocks.push(block);
            }
            Event::TextChunk { text, .. } => {
                chat.status.retry = None;
                let live = chat.live_mut();
                live.hydrated = false;
                match live.blocks.last_mut() {
                    Some(Block::Text { text: tail }) => tail.push_str(&text),
                    _ => live.blocks.push(Block::Text { text }),
                }
            }
            Event::ThinkingChunk { text, .. } => {
                let live = chat.live_mut();
                live.hydrated = false;
                match live.blocks.last_mut() {
                    Some(Block::Thinking { text: tail, .. }) => tail.push_str(&text),
                    _ => live.blocks.push(Block::thinking(text)),
                }
            }
            Event::StreamingSync { blocks, .. } => {
                let live = chat.live_mut();
                live.hydrated = false;
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
            } => {
                let live = chat.live_mut();
                live.hydrated = false;
                live.blocks.push(Block::ToolCall {
                    tool_call_id,
                    tool_name,
                    input,
                    summary,
                    presentation,
                    elided: None,
                });
            }
            Event::ToolResult {
                tool_call_id,
                tool_name,
                output,
                is_error,
                images,
                ..
            } => {
                let live = chat.live_mut();
                live.hydrated = false;
                live.blocks.push(Block::ToolResult {
                    tool_call_id,
                    tool_name,
                    output,
                    is_error,
                    images,
                });
            }
            Event::TokensUpdate {
                tokens,
                generation_throughput,
                ..
            } => {
                if let Some(live) = chat.live.as_mut() {
                    live.meta.tokens = tokens;
                    if generation_throughput.is_some() {
                        live.meta.generation_throughput = generation_throughput;
                    }
                }
            }
            Event::MessageComplete {
                blocks,
                ended_at,
                tokens,
                generation_throughput,
                ..
            } => {
                if let Some(mut live) = chat.live.take() {
                    let tail = Chat::tail_of(&live, &blocks);
                    live.meta.ended_at = ended_at.or(Some(now_ms() as f64));
                    live.meta.tokens = tokens;
                    if generation_throughput.is_some() {
                        live.meta.generation_throughput = generation_throughput;
                    }
                    chat.commit(tail, live.meta, live.stamp);
                }
                chat.status = Status::default();
            }
            Event::StreamingStopped {
                reason,
                persisted_blocks,
                ..
            } => {
                if let Some(mut live) = chat.live.take() {
                    let tail = match &persisted_blocks {
                        Some(persisted) => Chat::tail_of(&live, persisted),
                        None => live.blocks.clone(),
                    };
                    live.meta.ended_at = live.meta.ended_at.or(Some(now_ms() as f64));
                    let stamp = if live.stamp == 0 {
                        next_stamp()
                    } else {
                        live.stamp
                    };
                    let shown = any_visible(&tail);
                    chat.commit(tail, live.meta, stamp);
                    if shown && reason.as_deref() == Some("suspended") {
                        chat.suspended = Some(stamp);
                    }
                }
                chat.status = Status::default();
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
                queue_id,
                automation,
                ..
            } => {
                let started = started_at.unwrap_or(0.0);
                let mine = chat
                    .echoes
                    .iter()
                    .position(|echo| echo.started_at == started as u64 || echo.text == text);
                if let Some(index) = mine {
                    chat.echoes.remove(index);
                }
                // Already here (a window brought it first).
                let known = chat.entries.iter().any(|entry| {
                    matches!(entry, Entry::User { metadata: Some(m), .. }
                        if (started_at.is_some() && m.started_at == started)
                            || (queue_id.is_some() && m.queue_entry_id == queue_id))
                });
                if known {
                    return Vec::new();
                }
                // A message that joins a running turn splits the reply.
                chat.commit_segment();
                let model = chat
                    .live
                    .as_ref()
                    .map(|live| live.meta.model.clone())
                    .unwrap_or_default();
                let started = started_at.unwrap_or_else(|| now_ms() as f64);
                chat.push(Entry::User {
                    text,
                    images,
                    metadata: Some(Metadata {
                        started_at: started,
                        ended_at: Some(started),
                        model,
                        queue_entry_id: queue_id,
                        automation,
                        ..Metadata::default()
                    }),
                });
                // Sent from here, or the conversation works: a reply comes.
                if chat.live.is_none() && (mine.is_some() || streaming) {
                    chat.live_mut();
                }
            }
            Event::SystemMessage { text, color, .. } => {
                chat.commit_segment();
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
                delay_sec,
                reset_at,
                ..
            } => {
                // An unfinished provider block is not a completed round:
                // the reply keeps its completed tool rounds only.
                if let Some(live) = chat.live.as_mut() {
                    let last_tool = live
                        .blocks
                        .iter()
                        .rposition(|b| {
                            matches!(b, Block::ToolCall { .. } | Block::ToolResult { .. })
                        })
                        .map_or(0, |at| at + 1);
                    live.blocks.truncate(last_tool);
                }
                chat.commit_segment();
                chat.status.retry = Some(Retry {
                    message: error_message,
                    attempt,
                    max_attempts,
                    at: now_ms() as f64 + delay_sec * 1000.0,
                    reset_at,
                });
            }
            Event::ContextCompactionStatus {
                active,
                started_at,
                completed_at,
                ..
            } => {
                chat.status.compaction =
                    active.then(|| started_at.unwrap_or_else(|| now_ms() as f64));
                if let Some(at) = completed_at {
                    let exists = chat.entries.iter().any(|entry| {
                        matches!(entry, Entry::System { metadata: Some(m), .. }
                            if m.kind.as_deref() == Some(COMPACTION_KIND) && m.started_at == at)
                    });
                    if !exists {
                        chat.commit_segment();
                        chat.push(Entry::System {
                            text: COMPACTION_TEXT.into(),
                            color: None,
                            metadata: Some(Metadata {
                                started_at: at,
                                ended_at: Some(at),
                                system: true,
                                kind: Some(COMPACTION_KIND.into()),
                                ..Metadata::default()
                            }),
                        });
                    }
                }
            }
            Event::ConnectionWaitStatus {
                active,
                label,
                started_at,
                ..
            } => {
                chat.status.wait = active.then(|| Wait {
                    label,
                    started_at: started_at.unwrap_or_else(|| now_ms() as f64),
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
            || self.chat.older_error.is_some()
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
        } else if let Some(tools) = self.chat.outputs_inflight.remove(id) {
            for tool in tools {
                self.chat.outputs_requested.remove(&tool);
                self.chat.output_errors.insert(tool, message.to_owned());
            }
            self.chat.touch();
            true
        } else {
            false
        }
    }

    /// One of the chat's requests was not answered in time. The window's
    /// late answer still lands (its id stays); an older page can be asked
    /// for again from the top of the transcript.
    pub(super) fn chat_timeout(&mut self, id: &str, pending: &Pending) -> bool {
        if pending.conv != self.chat.id {
            return false;
        }
        match pending.kind {
            "load" if self.chat.load_request.as_deref() == Some(id) => {
                self.chat.loaded = true;
                self.chat.error = Some(
                    "The conversation did not respond. Retry to resync; your draft is safe.".into(),
                );
                self.chat.touch();
                true
            }
            "older" if self.chat.older_request.as_deref() == Some(id) => {
                self.chat.older_request = None;
                self.chat.older_error = Some("Earlier history timed out. Retry loading.".into());
                self.chat.touch();
                true
            }
            "outputs" => {
                for tool in self.chat.outputs_inflight.remove(id).unwrap_or_default() {
                    self.chat.outputs_requested.remove(&tool);
                    self.chat
                        .output_errors
                        .insert(tool, OUTPUT_TIMED_OUT.into());
                }
                self.chat.touch();
                true
            }
            _ => false,
        }
    }
}

/// How a queued message waits (PAR-143, the TUI's `queue.ts`): "queued:
/// next turn", "queued: steer", "queued: message end", "queued: after
/// <title>", "queued: at 14:05", "queued: global idle".
pub fn queue_label(queued: &proto::Queued, provider: Option<&str>, now: f64) -> String {
    if queued.source == "global-idle" {
        let target = queued.wait_target.as_ref();
        let kind = target
            .and_then(|t| t.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("global");
        let label = target
            .and_then(|t| t.get("label"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return match kind {
            "delay" => {
                let delay = target
                    .and_then(|t| t.get("delayMs"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let created = if queued.created_at > 0.0 {
                    queued.created_at
                } else {
                    now
                };
                format!(
                    "queued: at {}",
                    due_time(created + delay, now, delay < 3_600_000.0)
                )
            }
            "conversation" => format!("queued: after {label}"),
            "folder" => format!("queued: after folder {label}"),
            _ => "queued: global idle".into(),
        };
    }
    if queued.timing != "next-turn" {
        return "queued: message end".into();
    }
    if queued.automation.is_some() || provider == Some("anthropic") {
        "queued: next turn".into()
    } else {
        "queued: steer".into()
    }
}

/// `14:05` (with seconds when asked), or `Oct 10 14:05` when not today.
fn due_time(at: f64, now: f64, seconds: bool) -> String {
    // SAFETY: localtime_r fills the zeroed tm from a valid time_t.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = (at / 1000.0).floor() as _;
        libc::localtime_r(&time, &mut tm);
        tm
    };
    let time = if seconds {
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    } else {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    };
    if super::super::footer::local_day(at) == super::super::footer::local_day(now) {
        return time;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {time}",
        MONTHS[tm.tm_mon.clamp(0, 11) as usize],
        tm.tm_mday
    )
}

/// Display order of queued messages (PAR-143): next turn, message end,
/// a specific idle target, global idle; FIFO within each.
pub fn order_queued<'a>(list: &[&'a proto::Queued]) -> Vec<&'a proto::Queued> {
    let bucket = |q: &proto::Queued| {
        if q.source != "global-idle" {
            usize::from(q.timing != "next-turn")
        } else {
            let global = q
                .wait_target
                .as_ref()
                .and_then(|t| t.get("type"))
                .and_then(Value::as_str)
                .is_none_or(|kind| kind == "global");
            if global { 3 } else { 2 }
        }
    };
    let mut out: Vec<&proto::Queued> = list.to_vec();
    out.sort_by_key(|q| bucket(q));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;

    fn chat_with(entries: &str) -> Model {
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
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let request = commands
            .iter()
            .find(|c| c["type"] == "load_conversation")
            .unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{entries}],"historyStartIndex":0}}"#
            )),
            Instant::now(),
        );
        model
    }

    fn feed(model: &mut Model, lines: &[&str]) {
        for line in lines {
            model.apply(parse_event(line), Instant::now());
        }
    }

    #[test]
    fn a_completed_reply_keeps_its_identity_and_gets_its_metadata() {
        let mut model = chat_with(r#"{"type":"user","text":"q"}"#);
        feed(
            &mut model,
            &[
                r#"{"type":"streaming_started","convId":"c","provider":"anthropic","model":"claude-opus-5-5","startedAt":1000,"snapshotKind":"start","workTimerStartedAt":900}"#,
                r#"{"type":"text_chunk","convId":"c","text":"Hello"}"#,
                r#"{"type":"tokens_update","convId":"c","tokens":12,"generationThroughput":{"provider":"anthropic","model":"claude-opus-5-5","rates":[40]}}"#,
            ],
        );
        let live = model.chat.live.as_ref().unwrap();
        let stamp = live.stamp;
        assert_ne!(stamp, 0);
        assert_eq!(live.meta.tokens, 12);
        assert_eq!(live.meta.work_timer_started_at, Some(900.0));
        feed(
            &mut model,
            &[
                r#"{"type":"message_complete","convId":"c","blocks":[{"type":"text","text":"Hello"}],"endedAt":5000,"tokens":20}"#,
            ],
        );
        assert!(model.chat.live.is_none());
        assert_eq!(*model.chat.stamps.last().unwrap(), stamp);
        let Some(Entry::Ai {
            metadata: Some(meta),
            ..
        }) = model.chat.entries.last()
        else {
            panic!("committed");
        };
        assert_eq!(
            (meta.ended_at, meta.tokens, meta.started_at),
            (Some(5000.0), 20, 1000.0)
        );
        // The daemon's copy (same blocks, its own metadata) keeps the stamp.
        model.apply(
            parse_event(r#"{"type":"history_updated","convId":"c","entries":[{"type":"user","text":"q"},{"type":"ai","blocks":[{"type":"text","text":"Hello"}],"metadata":{"startedAt":1000,"endedAt":5001,"model":"claude-opus-5-5","tokens":20}}],"historyStartIndex":0}"#),
            Instant::now(),
        );
        assert_eq!(model.chat.stamps[1], stamp);
    }

    #[test]
    fn an_injected_message_splits_the_reply_and_completion_cuts_what_was_shown() {
        let mut model = chat_with(r#"{"type":"user","text":"q"}"#);
        feed(
            &mut model,
            &[
                r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":1000,"snapshotKind":"start"}"#,
                r#"{"type":"text_chunk","convId":"c","text":"Part one."}"#,
                r#"{"type":"user_message","convId":"c","text":"also this","startedAt":2000}"#,
            ],
        );
        assert_eq!(model.chat.entries.len(), 3);
        let first = model.chat.stamps[1];
        let live = model.chat.live.as_ref().unwrap();
        assert!(live.blocks.is_empty() && live.stamp != first);
        feed(
            &mut model,
            &[
                r#"{"type":"text_chunk","convId":"c","text":" Part two."}"#,
                r#"{"type":"message_complete","convId":"c","blocks":[{"type":"text","text":"Part one. Part two."}],"endedAt":3000}"#,
            ],
        );
        assert_eq!(model.chat.entries.len(), 4);
        assert!(matches!(&model.chat.entries[3], Entry::Ai { blocks, .. }
            if *blocks == vec![Block::text(" Part two.")]));
        assert_eq!(model.chat.stamps[1], first);
    }

    #[test]
    fn snapshots_are_adopted_only_as_compatible_extensions() {
        let local = vec![Block::text("Hel")];
        assert!(strict_extension(&local, &[Block::text("Hello")]));
        assert!(!strict_extension(&local, &[Block::text("Hex")]));
        assert!(!strict_extension(&local, &[Block::text("Hel")]));
        let mut model = chat_with("");
        feed(
            &mut model,
            &[
                r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":1,"snapshotKind":"start"}"#,
                r#"{"type":"text_chunk","convId":"c","text":"Hello there"}"#,
                // A stale heartbeat: not an extension of what streamed.
                r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":1,"snapshotKind":"heartbeat","blocks":[{"type":"text","text":"Hello"}]}"#,
            ],
        );
        assert_eq!(
            model.chat.live.as_ref().unwrap().blocks,
            vec![Block::text("Hello there")]
        );
        feed(
            &mut model,
            &[
                r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":1,"snapshotKind":"catchup","blocks":[{"type":"text","text":"Hello there, friend"}]}"#,
            ],
        );
        assert_eq!(
            model.chat.live.as_ref().unwrap().blocks,
            vec![Block::text("Hello there, friend")]
        );
    }

    #[test]
    fn live_status_follows_compaction_waits_and_retries() {
        let mut model = chat_with("");
        feed(
            &mut model,
            &[
                r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":1,"snapshotKind":"start"}"#,
                r#"{"type":"tool_call","convId":"c","toolCallId":"t1","toolName":"Bash","input":{},"summary":"ls"}"#,
                r#"{"type":"tool_result","convId":"c","toolCallId":"t1","toolName":"Bash","output":"a","isError":false}"#,
                r#"{"type":"text_chunk","convId":"c","text":"half a sen"}"#,
                r#"{"type":"stream_retry","convId":"c","attempt":2,"maxAttempts":5,"errorMessage":"overloaded","delaySec":12}"#,
            ],
        );
        // The completed tool round is committed; the half sentence goes.
        assert_eq!(model.chat.entries.len(), 1);
        let retry = model.chat.status.retry.as_ref().unwrap();
        assert_eq!((retry.attempt, retry.max_attempts), (2, 5));
        feed(
            &mut model,
            &[r#"{"type":"text_chunk","convId":"c","text":"Back."}"#],
        );
        assert!(model.chat.status.retry.is_none());
        feed(
            &mut model,
            &[
                r#"{"type":"context_compaction_status","convId":"c","active":true,"startedAt":5}"#,
                r#"{"type":"connection_wait_status","convId":"c","active":true,"label":"Waiting for connection…","startedAt":6}"#,
            ],
        );
        assert_eq!(model.chat.status.compaction, Some(5.0));
        assert_eq!(model.chat.status.wait.as_ref().unwrap().started_at, 6.0);
        feed(
            &mut model,
            &[
                r#"{"type":"context_compaction_status","convId":"c","active":false,"completedAt":9}"#,
                r#"{"type":"connection_wait_status","convId":"c","active":false}"#,
            ],
        );
        assert!(!model.chat.status.any());
        assert!(
            matches!(model.chat.entries.last(), Some(Entry::System { metadata: Some(m), .. })
            if m.kind.as_deref() == Some(COMPACTION_KIND))
        );
        // Once only.
        let count = model.chat.entries.len();
        feed(
            &mut model,
            &[
                r#"{"type":"context_compaction_status","convId":"c","active":false,"completedAt":9}"#,
            ],
        );
        assert_eq!(model.chat.entries.len(), count);
    }

    #[test]
    fn outputs_that_do_not_come_say_so_and_retry() {
        let mut model = chat_with(
            r#"{"type":"ai","blocks":[{"type":"tool_call","toolCallId":"t1","toolName":"Bash","input":{},"summary":"ls"},{"type":"tool_result","toolCallId":"t1","toolName":"","output":"","isError":false},{"type":"tool_call","toolCallId":"t2","toolName":"Bash","input":{},"summary":"ls"},{"type":"tool_result","toolCallId":"t2","toolName":"","output":"","isError":false}]}"#,
        );
        let commands = model.prefetch_outputs(&["t1", "t2"]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["toolCallIds"], serde_json::json!(["t1", "t2"]));
        assert!(
            model.prefetch_outputs(&["t1", "t2"]).is_empty(),
            "one batch at a time"
        );
        let request = commands[0]["reqId"].as_str().unwrap();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"tool_outputs_loaded","reqId":"{request}","convId":"c","outputs":[{{"toolCallId":"t1","output":"files"}}]}}"#
            )),
            Instant::now(),
        );
        assert_eq!(model.chat.output_of("t1"), Some("files"));
        assert_eq!(model.chat.output_errors["t2"], OUTPUT_MISSING);
        assert!(
            model.prefetch_outputs(&["t2"]).is_empty(),
            "not again on its own"
        );
        let commands = model.handle(Action::Chat(Act::RetryOutput("t2".into())));
        assert_eq!(commands[0]["toolCallIds"], serde_json::json!(["t2"]));
        // Timed out.
        let request = commands[0]["reqId"].as_str().unwrap().to_owned();
        let pending = model.requests.get(&request).unwrap().clone();
        assert!(model.chat_timeout(&request, &pending));
        assert_eq!(model.chat.output_errors["t2"], OUTPUT_TIMED_OUT);
    }

    #[test]
    fn the_message_menu_copies_and_reads_the_message() {
        let mut model = chat_with(
            r#"{"type":"user","text":"Please list"},{"type":"ai","blocks":[{"type":"thinking","text":"hm"},{"type":"tool_call","toolCallId":"t1","toolName":"Bash","input":{},"summary":"ls"},{"type":"tool_result","toolCallId":"t1","toolName":"Bash","output":"a b\n","isError":false},{"type":"text","text":"Done."}]}"#,
        );
        let stamp = model.chat.stamps[1];
        model.handle(Action::Chat(Act::MessageMenu(stamp)));
        assert_eq!(model.ui.opening.len(), 1);
        let text = plain_text(&model.chat, &model.chat.entries[1], &|name| name.to_owned());
        assert_eq!(text, "hm\n\nBash ls\n\na b\n\nDone.");
        let user = plain_text(&model.chat, &model.chat.entries[0], &|name| name.to_owned());
        assert_eq!(user, "Please list");
        model.ui.opening.clear();
        model.handle(Action::Chat(Act::ReadOutput("t1".into())));
        assert_eq!(model.ui.opening.len(), 1);
    }

    #[test]
    fn queued_labels_and_order_follow_the_tui() {
        let queued = |id: &str, timing: &str, source: &str, target: Option<Value>| proto::Queued {
            id: id.into(),
            timing: timing.into(),
            source: source.into(),
            wait_target: target,
            ..proto::Queued::default()
        };
        let next = queued("a", "next-turn", "daemon", None);
        let end = queued("b", "message-end", "daemon", None);
        let idle = queued("c", "message-end", "global-idle", None);
        let after = queued(
            "d",
            "message-end",
            "global-idle",
            Some(serde_json::json!({"type":"conversation","label":"Deploy"})),
        );
        assert_eq!(
            queue_label(&next, Some("anthropic"), 0.0),
            "queued: next turn"
        );
        assert_eq!(queue_label(&next, Some("openai"), 0.0), "queued: steer");
        assert_eq!(queue_label(&end, None, 0.0), "queued: message end");
        assert_eq!(queue_label(&idle, None, 0.0), "queued: global idle");
        assert_eq!(queue_label(&after, None, 0.0), "queued: after Deploy");
        let order: Vec<&str> = order_queued(&[&idle, &end, &after, &next])
            .iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(order, ["a", "b", "d", "c"]);
    }

    #[test]
    fn thinking_keys_survive_a_window_that_starts_later() {
        let blocks = vec![
            Block::thinking("a"),
            Block::ToolCall {
                tool_call_id: "t".into(),
                tool_name: "Bash".into(),
                input: Value::Null,
                summary: String::new(),
                presentation: None,
                elided: None,
            },
            Block::thinking("b"),
            Block::thinking("c"),
        ];
        assert_eq!(thinking_key(&blocks, 2), "think:t:1");
        assert_eq!(thinking_key(&blocks[1..], 1), "think:t:1");
        assert_eq!(merged_thinking(&blocks, 2), (4, "b\n\nc".into()));
        assert_eq!(
            thinking_text(&blocks, "think:t:1").as_deref(),
            Some("b\n\nc")
        );
    }
}
