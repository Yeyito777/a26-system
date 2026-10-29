//! The conversation list (WP-B) as the daemon keeps it: summaries and
//! folders, the rows the list shows — pinned first under a PINNED header,
//! folders expanding in place (PAR-052, 053), or flat search results
//! (PAR-061) — the activity added up per folder, and what the user does
//! with it (`list/actions.rs`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::list::activity::{self, FolderActivity, QueueInfo};
use super::super::list::search::{self, Search};
use super::super::proto::{self, Event, Folder, Queued, Summary};
use super::super::store;
use super::super::theme::{self, DIVIDER_ROW};
use super::super::ui::Haptic;
use super::{Action, Effect, Model, Nav};
use crate::control::KeyboardPurpose;
use crate::gfx::RectF;
use crate::keys::KeyInput;

pub use super::super::list::actions::Sent;
pub use super::super::list::pending::Key;
use super::super::list::pending::Pending;
use super::super::list::sheets::{FolderSheet, InstructionsEditor};
use super::super::requests::Owner;
use super::super::ui::sheet::Sheet;

/// A row held after its long press, perhaps dragged (PAR-070, 071).
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    /// What is dragged.
    pub key: Key,
    /// Where the long press was, and where the finger is (window).
    pub start: (f32, f32),
    pub at: (f32, f32),
    /// Moved far enough: the menu gave way to the preview.
    pub active: bool,
    pub target: Option<DropTarget>,
    /// The edge auto-scroll's speed (px/s) and its last step.
    pub speed: f32,
    pub stepped: Option<Instant>,
}

/// Where a dragged item would land.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DropTarget {
    /// Into this folder.
    Folder(String),
    /// Next to this conversation (before, or `after` it), in its level.
    Reorder { key: Key, after: bool },
}

/// A folder's own instructions (its AGENTS.md).
#[derive(Debug, Clone, PartialEq)]
pub enum Instructions {
    /// Asked for with this reqId.
    Loading(String),
    Loaded(String),
    Failed(String),
}

/// How long the list counts as in use after it was last shown, touched,
/// scrolled or typed into.
pub const IN_USE: std::time::Duration = std::time::Duration::from_secs(10);

/// Select mode's bar under the rows (PAR-077): the count, then Move, Pin,
/// Group and Delete.
pub const SELECT_BAR: f32 = 200.0;

/// How long a folder's instructions may take (they can queue behind a big
/// list).
const INSTRUCTIONS_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

/// What the user does with the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// A conversation row tapped.
    Open(String),
    /// A folder row tapped: it opens or closes in place.
    Toggle(String),
    /// A folder found by search tapped: shown where it lives.
    ShowFolder(String),
    /// The header's "+": a new chat at the top level.
    NewChat,
    /// A folder's "+": a new chat in it.
    NewChatIn(String),
    /// The header's search field tapped.
    Search,
    /// The search field's ✕.
    ClearSearch,
    /// A key from Moon's keyboard while the search field has it.
    Key(KeyInput),
    /// The header's ⋯: its menu, under the button.
    Menu(RectF),
    /// A long press on a conversation row: its menu, anchored to the row.
    RowMenu {
        id: String,
        rect: RectF,
    },
    /// A long press on a folder row.
    FolderMenu {
        id: String,
        rect: RectF,
    },
    /// The list again from the daemon (the error state's Retry).
    Retry,
    /// The next conversation that is unread or working (PAR-084).
    NextUnread,
    Pin(Vec<Key>, bool),
    Mark(String, bool),
    Mute(Key, bool),
    /// Rename…: the prompt.
    Rename(Key),
    /// A conversation's title or a folder's name.
    SetName(Key, String),
    /// Move to folder…: the picker.
    MovePicker(Vec<Key>),
    MoveTo {
        keys: Vec<Key>,
        parent: Option<String>,
    },
    Clone(String),
    /// Delete: the confirmation.
    ConfirmDelete(Vec<Key>),
    /// To the trash (folders with what is in them, or only their shells).
    Delete {
        keys: Vec<Key>,
        recursive: bool,
    },
    Undo,
    Redo,
    /// New folder…: the prompt (in a folder: a subfolder; with items:
    /// they move into it).
    NewFolder {
        parent: Option<String>,
        items: Vec<Key>,
    },
    CreateFolder {
        name: String,
        parent: Option<String>,
        items: Vec<Key>,
    },
    /// Select mode (PAR-077), with an item selected or none.
    Select(Option<Key>),
    /// A row tapped in select mode.
    ToggleSelect(Key),
    EndSelect,
    /// The select bar's actions on what is selected.
    SelectMove,
    SelectPin,
    SelectGroup,
    SelectDelete,
    /// A conversation's ◆ tapped: its subagent, or a choice of them
    /// (PAR-090).
    Subagents(String),
    /// Emoji mark…: the TUI's marks (PAR-083).
    MarkEmoji(String),
    /// The folder inspector (PAR-078).
    FolderInfo(String),
    EditInstructions(String),
    SaveInstructions {
        folder: String,
        text: String,
    },
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Row {
    /// The PINNED header.
    Pinned,
    /// "SEARCH RESULTS // N".
    Results(usize),
    /// Between a level's pinned rows and the rest.
    Divider {
        depth: usize,
    },
    Folder {
        id: String,
        depth: usize,
        open: bool,
    },
    Conversation {
        id: String,
        depth: usize,
    },
}

impl Row {
    pub fn height(&self) -> f32 {
        match self {
            Row::Pinned | Row::Results(_) => theme::header_row(),
            Row::Divider { .. } => DIVIDER_ROW,
            Row::Folder { .. } | Row::Conversation { .. } => theme::list_row(),
        }
    }
}

/// The rows and where each starts (from the list's top, px).
#[derive(Debug, Default)]
pub struct Layout {
    pub rows: Vec<Row>,
    pub tops: Vec<f32>,
    pub height: f32,
}

impl Layout {
    fn new(rows: Vec<Row>) -> Self {
        let mut tops = Vec::with_capacity(rows.len());
        let mut y = 0.0;
        for row in &rows {
            tops.push(y);
            y += row.height();
        }
        Self {
            rows,
            tops,
            height: y,
        }
    }

    /// The row at `y` (from the list's top).
    pub fn at(&self, y: f32) -> Option<usize> {
        if y < 0.0 || y >= self.height {
            return None;
        }
        Some(self.tops.partition_point(|top| *top <= y) - 1)
    }

    /// The first row reaching below `y`.
    pub fn first_below(&self, y: f32) -> usize {
        self.tops.partition_point(|top| *top <= y).saturating_sub(1)
    }

    pub fn index_of(&self, row: impl Fn(&Row) -> bool) -> Option<usize> {
        self.rows.iter().position(row)
    }
}

/// Folder activity, mutes and counts, added up from every conversation.
#[derive(Debug, Default)]
pub struct Aggregates {
    pub folders: HashMap<String, FolderActivity>,
    /// Folders muted themselves or by a folder above.
    pub muted: HashSet<String>,
    /// Each conversation's queue entries.
    pub queues: HashMap<String, QueueInfo>,
}

/// The list's part of `ui.json` (PAR-053).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Kept {
    expanded: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Sidebar {
    pub conversations: HashMap<String, Summary>,
    pub folders: HashMap<String, Folder>,
    /// The whole list has arrived (not just the bootstrap's first page).
    pub complete: bool,
    pub loaded: bool,
    /// The first page's per-folder counts, until the rest arrives.
    partial_counts: HashMap<String, usize>,
    /// Bumped on every change that shows (the view's redraw key).
    pub version: u64,
    /// Bumped when rows may come, go or move.
    structure: u64,
    /// Open folders (kept in `ui.json`).
    pub expanded: HashSet<String>,
    /// `expanded` was read from `ui.json`.
    restored: bool,
    pub search: Search,
    /// The conversation open last: its row shows selected.
    pub selected: Option<String>,
    /// The folder a new chat goes to (a folder's "+").
    pub draft_folder: Option<String>,
    /// A row to bring into view (`view::list::settle`).
    pub reveal: Option<Row>,
    /// Commands sent and not yet seen through, by reqId.
    pub mutations: HashMap<String, Sent>,
    /// A folder created here, to show once the daemon lists it: its name,
    /// parent, the folders there were before, and whether it opens.
    pub created: Option<(String, Option<String>, HashSet<String>, bool)>,
    /// Changes shown before the daemon answers (`list/pending.rs`).
    pub pending: Pending,
    /// New conversations shown before the daemon lists them: their edit,
    /// the request making them, and since when that request is over.
    pub creating: Vec<(u64, String, Option<Instant>)>,
    /// Select mode: what is selected.
    pub selection: Option<BTreeSet<Key>>,
    /// Folders' own instructions, as loaded.
    pub instructions: HashMap<String, Instructions>,
    /// Instructions being saved: folder by reqId.
    pub saving: HashMap<String, String>,
    /// A pull past the top far enough to refresh, the finger still down.
    pub pull_armed: bool,
    /// Whether a conversation opened from the list was unread just before
    /// (PAR-088; the transcript's reading position).
    pub unread_at_open: HashMap<String, bool>,
    /// The wall clock (ms) at an instant: frames read the time from their
    /// `now` through it, so a frame and its redraw agree whatever the real
    /// clock does meanwhile (and tests are deterministic).
    pub clock: Option<(Instant, u64)>,
    /// A held row being dragged (PAR-070).
    pub drag: Option<Drag>,
    /// The daemon's update status (`update_status`, a pong's
    /// `updateStatus`): `update_available`, `restart_needed`, … (PAR-086).
    pub update_status: Option<String>,
    /// The list could not be loaded (the state's Retry asks again).
    pub error: Option<String>,
    /// A row on screen has a pulsing dot (the view's frames).
    pub pulsing: bool,
    /// The search's results changed or it ended: the list goes to the
    /// results' top, or back to where the tree was (`tree_scroll`).
    pub scroll_top: bool,
    /// The tree's scroll while a search shows.
    pub tree_scroll: Option<f32>,
    /// The folder opened or closed last, and when (its chevron turns).
    pub toggled: Option<(String, Instant)>,
    /// Until when the list counts as in use (shown, touched, scrolled,
    /// typed into): streaming dots pulse only then, so a list nobody
    /// touches draws nothing (battery).
    pub in_use_until: Option<Instant>,
    /// The scroll offset the last pass saw, and whether the list showed:
    /// what says it moved or came up.
    pub seen: Option<f32>,
    layout: Option<((u64, String, u64), Rc<Layout>)>,
    aggregates: Option<(u64, Rc<Aggregates>)>,
}

/// An item of a level: pinned, sort order, a conversation (else a folder),
/// its id.
type Item<'a> = (bool, f64, bool, &'a str);

/// Each level's items, by parent folder (None: the top).
type Levels<'a> = HashMap<Option<&'a str>, Vec<Item<'a>>>;

/// Sidebar order: pinned first, then the daemon's sort order; conversations
/// before folders at a tie, then by id (the iPhone's `compareItems`).
fn order(a: Item, b: Item) -> std::cmp::Ordering {
    b.0.cmp(&a.0)
        .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .then(b.2.cmp(&a.2))
        .then(a.3.cmp(b.3))
}

impl Sidebar {
    pub(crate) fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Rows may have come, gone or moved.
    pub(crate) fn restructured(&mut self) {
        self.structure = self.structure.wrapping_add(1);
        self.changed();
    }

    /// A folder id, or None when it is not a folder the list has.
    fn parent<'a>(&self, id: Option<&'a str>) -> Option<&'a str> {
        id.filter(|id| self.folders.contains_key(*id))
    }

    fn upsert(&mut self, summary: &Summary) {
        let moved = self.conversations.get(&summary.id).is_none_or(|old| {
            old.pinned != summary.pinned
                || old.sort_order != summary.sort_order
                || old.folder_id != summary.folder_id
                || old.title != summary.title
        });
        self.conversations
            .insert(summary.id.clone(), summary.clone());
        if moved {
            self.restructured();
        }
    }

    /// Applies a daemon event: to the daemon's own values of what edits
    /// show changed, the edits then shown over them again.
    pub fn apply(&mut self, event: &Event, now: Instant) -> bool {
        let edited = !self.pending.is_empty();
        if edited {
            self.unproject();
        }
        let applied = self.apply_event(event);
        if edited {
            self.reproject(event, now);
        }
        if let Event::ConversationDeleted { conv_id, .. } = event
            && self.selected.as_ref() == Some(conv_id)
        {
            self.selected = None;
        }
        applied
    }

    fn apply_event(&mut self, event: &Event) -> bool {
        match event {
            Event::ConversationsList {
                conversations,
                folders,
                sidebar_partial,
                sidebar_rest,
                ..
            } => {
                if sidebar_rest == &Some(true) {
                    // The rest of an incremental bootstrap: add to the first page.
                    for summary in conversations {
                        self.conversations
                            .insert(summary.id.clone(), summary.clone());
                    }
                    self.complete = true;
                    self.partial_counts.clear();
                } else {
                    self.conversations = conversations
                        .iter()
                        .map(|summary| (summary.id.clone(), summary.clone()))
                        .collect();
                    self.complete = sidebar_partial.is_none();
                    self.partial_counts = sidebar_partial
                        .as_ref()
                        .and_then(|partial| partial.get("folderConversationCounts"))
                        .and_then(Value::as_object)
                        .map(|counts| {
                            counts
                                .iter()
                                .filter_map(|(id, n)| Some((id.clone(), n.as_u64()? as usize)))
                                .collect()
                        })
                        .unwrap_or_default();
                }
                if let Some(folders) = folders {
                    self.folders = folders.iter().map(|f| (f.id.clone(), f.clone())).collect();
                }
                self.loaded = true;
                self.error = None;
                self.restructured();
            }
            // A whole list on clients without state patches.
            Event::ConversationMoved {
                conversations,
                folders,
            } => {
                self.conversations = conversations
                    .iter()
                    .map(|summary| (summary.id.clone(), summary.clone()))
                    .collect();
                if let Some(folders) = folders {
                    self.folders = folders.iter().map(|f| (f.id.clone(), f.clone())).collect();
                }
                self.restructured();
            }
            Event::ConversationUpdated { summary, .. }
            | Event::ConversationRestored { summary, .. }
            | Event::ConversationUnwound { summary, .. } => {
                self.upsert(summary);
                self.changed();
            }
            Event::ConversationDeleted { conv_id, .. } => {
                if self.conversations.remove(conv_id).is_some() {
                    self.restructured();
                }
            }
            Event::ConversationMarked(marked) => {
                if let Some(summary) = self.conversations.get_mut(&marked.conv_id) {
                    summary.marked = marked.marked;
                    self.changed();
                }
            }
            Event::ConversationPinned { conv_id, pinned } => {
                if let Some(summary) = self.conversations.get_mut(conv_id) {
                    summary.pinned = *pinned;
                    self.restructured();
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
                self.restructured();
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
                    self.expanded.remove(id);
                }
                self.restructured();
            }
            // Queued counts and global-idle waits show on rows.
            Event::QueueUpdated { .. } => self.changed(),
            Event::UpdateStatus { status }
            | Event::Pong {
                update_status: Some(status),
                ..
            } => {
                let status = status.as_str().map(str::to_owned);
                if status.is_none() || status == self.update_status {
                    return false;
                }
                self.update_status = status;
                self.changed();
            }
            _ => return false,
        }
        true
    }

    /// The rows the list shows now (cached until something they depend on
    /// changes).
    pub fn layout(&mut self) -> Rc<Layout> {
        // Row heights follow the text size (its generation).
        let key = (
            self.structure,
            self.search.query().to_owned(),
            theme::generation(),
        );
        if let Some((cached, layout)) = &self.layout
            && *cached == key
        {
            return layout.clone();
        }
        let rows = if self.search.active() {
            self.search_rows(self.search.query())
        } else {
            self.tree_rows()
        };
        let layout = Rc::new(Layout::new(rows));
        self.layout = Some((key, layout.clone()));
        layout
    }

    /// Each level's folders and conversations in sidebar order.
    fn children(&self) -> Levels<'_> {
        let mut children: Levels = HashMap::new();
        for f in self.folders.values() {
            let parent = self.parent(f.parent_id.as_deref());
            children
                .entry(parent)
                .or_default()
                .push((f.pinned, f.sort_order, false, &f.id));
        }
        for c in self.conversations.values() {
            let parent = self.parent(c.folder_id.as_deref());
            children
                .entry(parent)
                .or_default()
                .push((c.pinned, c.sort_order, true, &c.id));
        }
        for level in children.values_mut() {
            level.sort_by(|a, b| order(*a, *b));
        }
        children
    }

    /// The tree: the top level, open folders' rows under them, a PINNED
    /// header over the top level's pinned rows and a divider after each
    /// level's pinned rows.
    fn tree_rows(&self) -> Vec<Row> {
        let children = self.children();
        let mut rows = Vec::new();
        let mut listed = HashSet::new();
        self.walk(&children, None, 0, &mut rows, &mut listed);
        rows
    }

    fn walk<'a>(
        &self,
        children: &Levels<'a>,
        parent: Option<&'a str>,
        depth: usize,
        rows: &mut Vec<Row>,
        listed: &mut HashSet<&'a str>,
    ) {
        let Some(items) = children.get(&parent) else {
            return;
        };
        let pinned = items.iter().filter(|item| item.0).count();
        if depth == 0 && pinned > 0 {
            rows.push(Row::Pinned);
        }
        for (index, &(_, _, conversation, id)) in items.iter().enumerate() {
            if pinned > 0 && index == pinned {
                rows.push(Row::Divider { depth });
            }
            if conversation {
                rows.push(Row::Conversation {
                    id: id.to_owned(),
                    depth,
                });
            } else {
                let open = self.expanded.contains(id);
                rows.push(Row::Folder {
                    id: id.to_owned(),
                    depth,
                    open,
                });
                if open && listed.insert(id) {
                    self.walk(children, Some(id), depth + 1, rows, listed);
                }
            }
        }
    }

    /// Search results: conversation titles (marks stripped) and folder
    /// names, flat, in sidebar order, under a "SEARCH RESULTS // N" header.
    fn search_rows(&self, query: &str) -> Vec<Row> {
        let query = query.to_lowercase();
        let mut hits: Vec<Item> = Vec::new();
        for f in self.folders.values() {
            let name = if f.name.is_empty() { "Folder" } else { &f.name };
            if Search::find(name, &query).is_some() {
                hits.push((f.pinned, f.sort_order, false, &f.id));
            }
        }
        for c in self.conversations.values() {
            if Search::find(activity::search_title(c), &query).is_some() {
                hits.push((c.pinned, c.sort_order, true, &c.id));
            }
        }
        hits.sort_by(|a, b| order(*a, *b));
        let mut rows = vec![Row::Results(hits.len())];
        rows.extend(
            hits.iter()
                .take(search::LIMIT)
                .map(|&(_, _, conversation, id)| {
                    if conversation {
                        Row::Conversation {
                            id: id.to_owned(),
                            depth: 0,
                        }
                    } else {
                        Row::Folder {
                            id: id.to_owned(),
                            depth: 0,
                            open: false,
                        }
                    }
                }),
        );
        rows
    }

    /// Folder activity and counts, folder mutes and the queue per
    /// conversation, added up once per change.
    pub fn aggregates(&mut self, queued: &[Queued]) -> Rc<Aggregates> {
        if let Some((version, aggregates)) = &self.aggregates
            && *version == self.version
        {
            return aggregates.clone();
        }
        let mut queues: HashMap<String, QueueInfo> = HashMap::new();
        for entry in queued {
            let info = queues.entry(entry.conv_id.clone()).or_default();
            info.count += 1;
            info.idle |= entry.source == "global-idle";
        }
        let mut muted = HashSet::new();
        for id in self.folders.keys() {
            let mut seen = HashSet::new();
            let mut at = Some(id.as_str());
            while let Some(folder) = at.and_then(|id| self.folders.get(id)) {
                if folder.muted {
                    muted.insert(id.clone());
                    break;
                }
                if !seen.insert(folder.id.as_str()) {
                    break;
                }
                at = folder.parent_id.as_deref();
            }
        }
        let mut folders: HashMap<String, FolderActivity> = self
            .folders
            .keys()
            .map(|id| (id.clone(), FolderActivity::default()))
            .collect();
        for c in self.conversations.values() {
            let queue = queues.get(&c.id).copied().unwrap_or_default();
            let mut seen = HashSet::new();
            let mut at = c.folder_id.as_deref();
            while let Some(id) = at {
                let Some(aggregate) = folders.get_mut(id) else {
                    break;
                };
                if !seen.insert(id) {
                    break;
                }
                aggregate.add(c, queue, false);
                at = self.folders.get(id).and_then(|f| f.parent_id.as_deref());
            }
        }
        // Until the whole list is here, the first page's counts.
        if !self.complete {
            for (id, count) in &self.partial_counts {
                if let Some(aggregate) = folders.get_mut(id) {
                    aggregate.count = aggregate.count.max(*count);
                }
            }
        }
        let aggregates = Rc::new(Aggregates {
            folders,
            muted,
            queues,
        });
        self.aggregates = Some((self.version, aggregates.clone()));
        aggregates
    }

    /// Folders above `folder` (nearest first), `folder` itself first when
    /// `inclusive`.
    pub fn ancestors(&self, folder: Option<&str>, inclusive: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = if inclusive {
            folder
        } else {
            folder
                .and_then(|id| self.folders.get(id))
                .and_then(|f| f.parent_id.as_deref())
        };
        while let Some(f) = at.and_then(|id| self.folders.get(id)) {
            if out.contains(&f.id) {
                break;
            }
            out.push(f.id.clone());
            at = f.parent_id.as_deref();
        }
        out
    }

    /// "Parent / Child", or "Top level".
    pub fn folder_path(&self, folder: Option<&str>) -> String {
        let mut names: Vec<&str> = self
            .ancestors(folder, true)
            .iter()
            .filter_map(|id| self.folders.get(id))
            .map(|f| {
                if f.name.is_empty() {
                    "Folder"
                } else {
                    f.name.as_str()
                }
            })
            .collect();
        if names.is_empty() {
            return "Top level".into();
        }
        names.reverse();
        names.join(" / ")
    }

    /// Opens every folder above a row so it is in the tree, and asks the
    /// view to bring it into view.
    pub fn reveal_row(&mut self, row: Row) {
        let above = match &row {
            Row::Conversation { id, .. } => {
                let folder = self.conversations.get(id).and_then(|c| c.folder_id.clone());
                self.ancestors(folder.as_deref(), true)
            }
            Row::Folder { id, .. } => self.ancestors(Some(id), false),
            _ => Vec::new(),
        };
        let mut opened = false;
        for id in above {
            opened |= self.expanded.insert(id);
        }
        if opened {
            self.restructured();
        }
        self.reveal = Some(row);
    }

    /// Back on the list: ends select mode or a search first. False when
    /// there was neither.
    pub fn up(&mut self) -> bool {
        if self.selection.take().is_some() {
            self.changed();
            return true;
        }
        if self.search.focused || !self.search.text.is_empty() || self.search.active() {
            self.search.clear();
            self.search.focused = false;
            self.searched();
            return true;
        }
        false
    }

    /// The list is in use: its dots pulse for a while.
    pub fn touch_list(&mut self, now: Instant) {
        self.in_use_until = Some(now + IN_USE);
    }

    /// Whether the dots pulse at `now`.
    pub fn pulsing_now(&self, now: Instant) -> bool {
        self.in_use_until.is_some_and(|until| now < until)
    }

    /// The wall clock (ms since the epoch) at `now`.
    pub fn wall_ms(&mut self, now: Instant) -> f64 {
        let (at, ms) = *self.clock.get_or_insert_with(|| (now, super::now_ms()));
        if now >= at {
            ms as f64 + now.duration_since(at).as_millis() as f64
        } else {
            ms as f64 - at.duration_since(now).as_millis() as f64
        }
    }

    /// What is selected that is still there, in the list's order.
    pub fn selected_keys(&self) -> Vec<Key> {
        let Some(selection) = &self.selection else {
            return Vec::new();
        };
        let mut keys: Vec<(usize, Key)> = selection
            .iter()
            .filter(|key| match key {
                Key::Conv(id) => self.conversations.contains_key(id),
                Key::Folder(id) => self.folders.contains_key(id),
            })
            .map(|key| {
                let at = self
                    .layout
                    .as_ref()
                    .and_then(|(_, layout)| {
                        layout.index_of(|row| match (row, key) {
                            (Row::Conversation { id, .. }, Key::Conv(other))
                            | (Row::Folder { id, .. }, Key::Folder(other)) => id == other,
                            _ => false,
                        })
                    })
                    .unwrap_or(usize::MAX);
                (at, key.clone())
            })
            .collect();
        keys.sort();
        keys.into_iter().map(|(_, key)| key).collect()
    }

    /// Items gone (deleted, moved into a new folder) leave the selection; an
    /// emptied selection stays open.
    pub fn leave_selection(&mut self, keys: &[Key]) {
        if let Some(selection) = &mut self.selection {
            for key in keys {
                selection.remove(key);
            }
            self.changed();
        }
    }

    /// Folders in the tree's order, with their depth (the move picker).
    pub fn folder_tree(&self) -> Vec<(String, usize)> {
        fn walk(
            children: &Levels,
            parent: Option<&str>,
            depth: usize,
            out: &mut Vec<(String, usize)>,
        ) {
            for &(_, _, conversation, id) in children.get(&parent).map_or(&[][..], |v| v) {
                if conversation || out.iter().any(|(seen, _)| seen == id) {
                    continue;
                }
                out.push((id.to_owned(), depth));
                walk(children, Some(id), depth + 1, out);
            }
        }
        let children = self.children();
        let mut out = Vec::new();
        walk(&children, None, 0, &mut out);
        out
    }

    /// The folder aggregates last added up, if any.
    pub fn aggregates_cached(&self) -> Option<&Aggregates> {
        self.aggregates
            .as_ref()
            .map(|(_, aggregates)| aggregates.as_ref())
    }

    /// What the search shows changed.
    pub(crate) fn searched(&mut self) {
        self.scroll_top = true;
        self.restructured();
    }

    /// The folder a new chat goes to.
    pub fn current_folder(&self) -> Option<&str> {
        self.draft_folder.as_deref()
    }

    /// The `sidebarView` of the bootstrap: the tree with the open folders,
    /// so the daemon's first page holds the rows on screen (PAR-018).
    pub fn view_hint(&self) -> Value {
        let mut expanded: Vec<&String> = self.expanded.iter().collect();
        expanded.sort();
        serde_json::json!({
            "folderView": "tree",
            "currentFolderId": null,
            "expandedFolderIds": expanded,
            "scrollOffset": 0,
            "rows": 24,
            "conversationIds": self.selected.iter().collect::<Vec<_>>(),
        })
    }

    fn ids_by_tree_order(&self) -> Vec<String> {
        // Depth first, in order: each level, descending into folders.
        fn visit<'a>(
            children: &Levels<'a>,
            parent: Option<&'a str>,
            out: &mut Vec<String>,
            seen: &mut HashSet<&'a str>,
        ) {
            for &(_, _, conversation, id) in children.get(&parent).map_or(&[][..], |v| v) {
                if conversation {
                    out.push(id.to_owned());
                } else if seen.insert(id) {
                    visit(children, Some(id), out, seen);
                }
            }
        }
        let children = self.children();
        let mut out = Vec::new();
        visit(&children, None, &mut out, &mut HashSet::new());
        out
    }

    /// The next conversation after `from`, in the list's order (folders
    /// open or not), that is unread or working; wrapping (PAR-084).
    pub fn next_attention(&mut self, from: Option<&str>, queued: &[Queued]) -> Option<String> {
        let order = self.ids_by_tree_order();
        if order.is_empty() {
            return None;
        }
        let aggregates = self.aggregates(queued);
        let start = from
            .and_then(|id| order.iter().position(|o| o == id))
            .map_or(order.len() - 1, |i| i);
        (1..=order.len())
            .map(|n| &order[(start + n) % order.len()])
            .find(|id| {
                Some(id.as_str()) != from
                    && self.conversations.get(*id).is_some_and(|c| {
                        let folder_muted = c
                            .folder_id
                            .as_ref()
                            .is_some_and(|f| aggregates.muted.contains(f));
                        let queue = aggregates.queues.get(&c.id).copied().unwrap_or_default();
                        matches!(
                            activity::conversation(c, queue, false, folder_muted).kind,
                            Some(activity::Kind::Unread | activity::Kind::Streaming)
                        )
                    })
            })
            .cloned()
    }
}

impl Model {
    /// The list's kept state, read once (the store loads after the model
    /// is made).
    pub(crate) fn restore_list(&mut self) {
        if self.sidebar.restored {
            return;
        }
        self.sidebar.restored = true;
        let kept: Kept = self.persist.section(store::UI, "list");
        self.sidebar.expanded.extend(kept.expanded);
        self.sidebar.restructured();
    }

    pub(crate) fn keep_list(&mut self) {
        let mut expanded: Vec<String> = self.sidebar.expanded.iter().cloned().collect();
        expanded.sort();
        self.persist
            .set_section(store::UI, "list", &Kept { expanded });
    }

    /// The bootstrap's `sidebarView` (tree, open folders).
    pub fn list_view_hint(&mut self) -> Value {
        self.restore_list();
        self.sidebar.view_hint()
    }

    /// The search field keeps Moon's keyboard on the list.
    pub fn list_keeps_keyboard(&self) -> bool {
        self.page == super::Page::List && self.sidebar.search.focused
    }

    /// The debounced search follows the typing, unanswered edits expire,
    /// a new conversation's row goes when its request failed; true when
    /// the list changed.
    pub fn list_tick(&mut self, now: Instant) -> bool {
        // The wall clock relative times read, set again each pass of the
        // event loop (a deep suspend stops `Instant`, not the wall clock).
        self.sidebar.clock = Some((now, super::now_ms()));
        let mut changed = self.settle_created(now);
        changed |= self.sidebar.expire_edits(now);
        if self.sidebar.search.tick(now) {
            self.sidebar.searched();
            changed = true;
        }
        changed
    }

    /// Opens a conversation from the list: its row shows selected, and
    /// whether it was unread is kept (the daemon clears it on open;
    /// PAR-088, for the reading position).
    fn open_from_list(&mut self, id: String) -> Vec<Value> {
        let unread = self
            .sidebar
            .conversations
            .get(&id)
            .is_some_and(|c| c.unread);
        if self.sidebar.unread_at_open.len() >= 50 {
            self.sidebar.unread_at_open.clear();
        }
        self.sidebar.unread_at_open.insert(id.clone(), unread);
        self.sidebar.selected = Some(id.clone());
        self.handle(Action::Nav(Nav::OpenConversation(id)))
    }

    /// The search field gives up Moon's keyboard.
    fn unfocus_search(&mut self) {
        if self.sidebar.search.focused {
            self.sidebar.search.focused = false;
            self.ui.effect(Effect::HideKeyboard);
        }
    }

    pub(super) fn handle_list(&mut self, act: Act) -> Vec<Value> {
        self.restore_list();
        let now = Instant::now();
        match act {
            Act::Open(id) => {
                self.unfocus_search();
                return self.open_from_list(id);
            }
            Act::Subagents(id) => {
                let children: Vec<(String, String)> = self
                    .sidebar
                    .conversations
                    .get(&id)
                    .map(|c| {
                        c.tasks
                            .iter()
                            .filter(|t| t.kind == "subagent")
                            .map(|t| (t.id.clone(), t.title.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                match &children[..] {
                    [] => {}
                    [(child, _)] => return self.open_from_list(child.clone()),
                    _ => self.subagents_sheet(children),
                }
            }
            Act::MarkEmoji(id) => self.emoji_sheet(id),
            Act::Toggle(id) => {
                let open = !self.sidebar.expanded.remove(&id);
                if open {
                    self.sidebar.expanded.insert(id.clone());
                }
                self.sidebar.toggled = Some((id, now));
                self.ui.haptic(Haptic::Selection);
                self.sidebar.restructured();
                self.keep_list();
            }
            Act::ShowFolder(id) => {
                self.unfocus_search();
                self.sidebar.search.clear();
                self.sidebar.searched();
                self.sidebar.expanded.insert(id.clone());
                self.sidebar.reveal_row(Row::Folder {
                    id,
                    depth: 0,
                    open: true,
                });
                self.sidebar.restructured();
                self.keep_list();
            }
            Act::NewChat | Act::NewChatIn(_) => {
                self.unfocus_search();
                self.sidebar.draft_folder = match act {
                    Act::NewChatIn(folder) => Some(folder),
                    _ => None,
                };
                return self.handle(Action::Nav(Nav::NewConversation));
            }
            Act::Search => {
                self.sidebar.search.focused = true;
                self.ui
                    .effect(Effect::ShowKeyboard(KeyboardPurpose::Search));
                self.sidebar.changed();
            }
            // Clears and gives up the keyboard, as the iPhone's ✕ does.
            Act::ClearSearch => {
                self.sidebar.search.clear();
                self.unfocus_search();
                self.sidebar.searched();
            }
            Act::Key(KeyInput::Enter) => {
                self.unfocus_search();
                self.sidebar.changed();
            }
            Act::Key(input) => {
                self.sidebar.touch_list(now);
                if self.sidebar.search.key(input, now) {
                    self.sidebar.changed();
                }
            }
            Act::Menu(anchor) => self.list_menu(anchor),
            Act::RowMenu { id, rect } => self.conversation_menu(id, rect),
            Act::FolderMenu { id, rect } => self.folder_menu(id, rect),
            Act::Retry => {
                self.sidebar.error = None;
                self.sidebar.changed();
                return self.refresh_list();
            }
            Act::NextUnread => {
                let from = self.sidebar.selected.clone();
                match self
                    .sidebar
                    .next_attention(from.as_deref(), &self.composer.queued)
                {
                    Some(id) => {
                        self.sidebar.reveal_row(Row::Conversation {
                            id: id.clone(),
                            depth: 0,
                        });
                        self.keep_list();
                        return self.open_from_list(id);
                    }
                    None => self.ui.toast("No unread chats."),
                }
            }
            Act::Pin(keys, pinned) => return self.list_pin(keys, pinned),
            Act::Mark(id, marked) => return self.list_mark(id, marked),
            Act::Mute(key, muted) => return self.list_mute(key, muted),
            Act::Rename(key) => self.rename_prompt(key),
            Act::SetName(key, name) => return self.list_rename(key, name),
            Act::MovePicker(keys) => self.move_picker(keys),
            Act::MoveTo { keys, parent } => {
                self.ui.haptic(Haptic::Light);
                return self.list_move(keys, parent);
            }
            Act::Clone(id) => return self.list_clone(id),
            Act::ConfirmDelete(keys) => self.confirm_delete(keys),
            Act::Delete { keys, recursive } => return self.list_delete(keys, recursive),
            Act::Undo => return self.list_undo(false),
            Act::Redo => return self.list_undo(true),
            Act::NewFolder { parent, items } => self.new_folder_prompt(parent, items),
            Act::CreateFolder {
                name,
                parent,
                items,
            } => return self.list_create_folder(name, parent, items),
            Act::Select(first) => {
                self.sidebar.selection = Some(first.into_iter().collect());
                self.ui.haptic(Haptic::Light);
                self.sidebar.changed();
            }
            Act::ToggleSelect(key) => {
                if let Some(selection) = &mut self.sidebar.selection
                    && !selection.remove(&key)
                {
                    selection.insert(key);
                }
                self.ui.haptic(Haptic::Selection);
                self.sidebar.changed();
            }
            Act::EndSelect => {
                self.sidebar.selection = None;
                self.sidebar.changed();
            }
            Act::SelectMove => {
                let keys = self.sidebar.selected_keys();
                self.move_picker(keys);
            }
            Act::SelectPin => {
                let keys = self.sidebar.selected_keys();
                let all = !keys.is_empty()
                    && keys
                        .iter()
                        .all(|key| self.sidebar.pinned_of(key) == Some(true));
                return self.list_pin(keys, !all);
            }
            Act::SelectGroup => {
                let keys = self.sidebar.selected_keys();
                if !keys.is_empty() {
                    self.new_folder_prompt(None, keys);
                }
            }
            Act::SelectDelete => {
                let keys = self.sidebar.selected_keys();
                self.confirm_delete(keys);
            }
            Act::FolderInfo(id) => return self.folder_info(id),
            Act::EditInstructions(id) => {
                if let Some(Instructions::Loaded(text)) = self.sidebar.instructions.get(&id) {
                    let editor = InstructionsEditor::new(id, text.clone());
                    self.ui.open(Box::new(Sheet::new(editor)));
                }
            }
            Act::SaveInstructions { folder, text } => {
                let id = self.requests.issue(
                    Owner::Sidebar,
                    "instructions",
                    None,
                    INSTRUCTIONS_WITHIN,
                    now,
                );
                self.sidebar.saving.insert(id.clone(), folder.clone());
                return vec![proto::set_folder_instructions(&id, &folder, &text)];
            }
        }
        Vec::new()
    }

    pub(super) fn apply_list(&mut self, event: &Event, now: Instant) -> Vec<Value> {
        self.restore_list();
        let open = self.sidebar.expanded.len();
        self.sidebar.apply(event, now);
        // Folders that went away are not kept open.
        if self.sidebar.expanded.len() != open {
            self.keep_list();
        }
        if let Event::FolderInstructionsLoaded(loaded) | Event::FolderInstructionsUpdated(loaded) =
            event
        {
            let saved = loaded
                .req_id
                .as_ref()
                .and_then(|id| self.sidebar.saving.remove(id))
                .is_some();
            if saved {
                self.ui.toast("Instructions saved");
            }
            self.sidebar.instructions.insert(
                loaded.folder_id.clone(),
                Instructions::Loaded(loaded.text.clone()),
            );
            self.sidebar.changed();
        }
        self.settle_list(event, now)
    }

    /// The folder inspector, its instructions asked for.
    fn folder_info(&mut self, id: String) -> Vec<Value> {
        if !self.sidebar.folders.contains_key(&id) {
            return Vec::new();
        }
        self.ui
            .open(Box::new(Sheet::new(FolderSheet::new(id.clone()))));
        if matches!(
            self.sidebar.instructions.get(&id),
            Some(Instructions::Loading(_))
        ) || !self.ready
        {
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Sidebar,
            "instructions",
            None,
            INSTRUCTIONS_WITHIN,
            Instant::now(),
        );
        self.sidebar
            .instructions
            .insert(id.clone(), Instructions::Loading(request.clone()));
        vec![proto::load_folder_instructions(&request, &id)]
    }

    /// An instructions request failed (`message`); true when it was one.
    pub(crate) fn instructions_failed(&mut self, id: &str, message: &str) -> bool {
        if let Some(folder) = self.sidebar.saving.remove(id) {
            let _ = folder;
            self.list_failed(&format!("Could not save the instructions: {message}"));
            return true;
        }
        let loading = self
            .sidebar
            .instructions
            .iter()
            .find(|(_, state)| **state == Instructions::Loading(id.to_owned()))
            .map(|(folder, _)| folder.clone());
        if let Some(folder) = loading {
            self.sidebar
                .instructions
                .insert(folder, Instructions::Failed(message.to_owned()));
            self.sidebar.changed();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::{Action, Effect, Model, Page};
    use super::*;
    use crate::exocortex::proto::parse_event;

    /// `base` with `extra` (JSON object fields) over it.
    fn object(base: Value, extra: &str) -> String {
        let mut value = base;
        let extra: Value = serde_json::from_str(&format!("{{{extra}}}")).unwrap();
        for (key, field) in extra.as_object().unwrap() {
            value[key] = field.clone();
        }
        value.to_string()
    }

    /// A conversation summary with `extra` fields.
    fn conv(id: &str, extra: &str) -> String {
        object(
            serde_json::json!({ "id": id, "title": id, "sortOrder": 0 }),
            extra,
        )
    }

    fn folder(id: &str, extra: &str) -> String {
        object(
            serde_json::json!({ "id": id, "name": id, "sortOrder": 0 }),
            extra,
        )
    }

    /// Connected, ready, with this list.
    fn model(conversations: &[String], folders: &[String]) -> Model {
        let mut model = Model::new();
        let bootstrap = model.on_connected();
        let ping = bootstrap.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        let now = Instant::now();
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            now,
        );
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversations_list","conversations":[{}],"folders":[{}]}}"#,
                conversations.join(","),
                folders.join(",")
            )),
            now,
        );
        model
    }

    fn keys(model: &mut Model) -> Vec<String> {
        model
            .sidebar
            .layout()
            .rows
            .iter()
            .map(|row| match row {
                Row::Pinned => "h:pinned".to_owned(),
                Row::Results(n) => format!("h:search:{n}"),
                Row::Divider { depth } => format!("d:{depth}"),
                Row::Folder { id, depth, .. } => format!("f:{id}:{depth}"),
                Row::Conversation { id, depth } => format!("c:{id}:{depth}"),
            })
            .collect()
    }

    fn list(model: &mut Model, act: Act) -> Vec<Value> {
        model.handle(Action::List(act))
    }

    fn event(model: &mut Model, json: &str) -> Vec<Value> {
        model.apply(parse_event(json), Instant::now())
    }

    /// What the model asked to open (labels), taken.
    fn opened(model: &mut Model) -> Vec<String> {
        model
            .ui
            .opening
            .drain(..)
            .map(|overlay| overlay.label())
            .collect()
    }

    // The iPhone's test/sidebar.test.ts, "SidebarStore".
    #[test]
    fn the_tree_has_a_pinned_header_nested_dividers_and_open_folders() {
        let mut model = model(
            &[
                conv("root1", r#""sortOrder":2"#),
                conv("pin", r#""pinned":true,"sortOrder":9"#),
                conv(
                    "inner",
                    r#""folderId":"f2","streaming":true,"subagentCount":2"#,
                ),
                conv("inner2", r#""folderId":"f2","unread":true"#),
                conv("pinnedInner", r#""folderId":"f1","pinned":true"#),
            ],
            &[
                folder("f1", r#""sortOrder":1"#),
                folder("f2", r#""parentId":"f1""#),
            ],
        );
        assert_eq!(
            keys(&mut model),
            ["h:pinned", "c:pin:0", "d:0", "f:f1:0", "c:root1:0"]
        );
        list(&mut model, Act::Toggle("f1".into()));
        list(&mut model, Act::Toggle("f2".into()));
        assert_eq!(
            keys(&mut model),
            [
                "h:pinned",
                "c:pin:0",
                "d:0",
                "f:f1:0",
                "c:pinnedInner:1",
                "d:1",
                "f:f2:1",
                "c:inner:2",
                "c:inner2:2",
                "c:root1:0"
            ]
        );
        let aggregates = model.sidebar.aggregates(&[]);
        let f1 = aggregates.folders["f1"];
        assert_eq!(
            (f1.count, f1.streaming, f1.warning, f1.unread, f1.agents),
            (3, 1, false, 1, 2)
        );
        // Open folders are kept, and the bootstrap names them.
        let kept: Kept = model.persist.section(store::UI, "list");
        assert_eq!(kept.expanded, ["f1", "f2"]);
        let hint = model.list_view_hint();
        assert_eq!(hint["folderView"], "tree");
        assert_eq!(hint["expandedFolderIds"], serde_json::json!(["f1", "f2"]));
        // Closing one forgets it.
        list(&mut model, Act::Toggle("f1".into()));
        assert_eq!(keys(&mut model).len(), 5);
        let kept: Kept = model.persist.section(store::UI, "list");
        assert_eq!(kept.expanded, ["f2"]);
    }

    #[test]
    fn open_folders_are_restored_from_ui_json() {
        let mut model = Model::new();
        model.persist.set_section(
            store::UI,
            "list",
            &Kept {
                expanded: vec!["f".into()],
            },
        );
        let bootstrap = model.on_connected();
        assert_eq!(
            bootstrap[0]["sidebarView"]["expandedFolderIds"],
            serde_json::json!(["f"])
        );
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversations_list","conversations":[{}],"folders":[{}]}}"#,
                conv("a", r#""folderId":"f""#),
                folder("f", "")
            ),
        );
        assert_eq!(keys(&mut model), ["f:f:0", "c:a:1"]);
    }

    #[test]
    fn a_muted_folder_mutes_what_is_in_it() {
        let mut model = model(
            &[
                conv(
                    "w",
                    r#""folderId":"f","streaming":true,"providerWaiting":true"#,
                ),
                conv("u", r#""folderId":"g","unread":true"#),
            ],
            &[
                folder("f", r#""muted":true"#),
                folder("g", r#""parentId":"f""#),
            ],
        );
        let aggregates = model.sidebar.aggregates(&[]);
        assert!(aggregates.muted.contains("g"), "muted by the folder above");
        let f = aggregates.folders["f"];
        assert!(!f.warning, "a provider wait is not added up");
        assert_eq!(f.indicator(true), Some((activity::Kind::Streaming, 1)));
    }

    #[test]
    fn the_first_pages_counts_stand_until_the_rest_arrives() {
        let mut model = Model::new();
        model.on_connected();
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversations_list","conversations":[{}],"folders":[{}],"sidebarPartial":{{"remaining":40,"folderConversationCounts":{{"f":41}}}}}}"#,
                conv("a", r#""folderId":"f""#),
                folder("f", "")
            ),
        );
        assert_eq!(model.sidebar.aggregates(&[]).folders["f"].count, 41);
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversations_list","conversations":[{}],"sidebarRest":true}}"#,
                conv("b", r#""folderId":"f""#)
            ),
        );
        assert_eq!(model.sidebar.aggregates(&[]).folders["f"].count, 2);
    }

    #[test]
    fn search_is_flat_debounced_and_ends_with_back() {
        let mut model = model(
            &[
                conv("a", r#""title":"Whale facts","sortOrder":5"#),
                conv(
                    "b",
                    r#""title":"🐛 whale songs","sortOrder":1,"folderId":"f""#,
                ),
                conv("p", r#""title":"whale pinned","pinned":true,"sortOrder":9"#),
                conv("c", r#""title":"krill""#),
            ],
            &[folder("f", r#""name":"Whales","sortOrder":3"#)],
        );
        list(&mut model, Act::Search);
        assert!(model.sidebar.search.focused && model.list_keeps_keyboard());
        assert!(
            model
                .ui
                .effects
                .contains(&Effect::ShowKeyboard(KeyboardPurpose::Search))
        );
        let t = Instant::now();
        for c in "WHALE".chars() {
            list(&mut model, Act::Key(KeyInput::Character(c)));
        }
        // Not until the typing pauses.
        assert_eq!(keys(&mut model).len(), 6);
        assert!(model.tick(t + Duration::from_millis(400)));
        assert_eq!(
            keys(&mut model),
            ["h:search:4", "c:p:0", "c:b:0", "f:f:0", "c:a:0"]
        );
        // Return puts the keyboard away and keeps the results.
        model.ui.effects.clear();
        list(&mut model, Act::Key(KeyInput::Enter));
        assert_eq!(model.ui.effects, [Effect::HideKeyboard]);
        assert!(!model.list_keeps_keyboard());
        assert_eq!(keys(&mut model).len(), 5);
        // A folder found opens where it lives.
        list(&mut model, Act::ShowFolder("f".into()));
        assert_eq!(keys(&mut model)[..4], ["h:pinned", "c:p:0", "d:0", "c:c:0"]);
        assert!(keys(&mut model).contains(&"c:b:1".to_owned()));
        assert!(matches!(model.sidebar.reveal, Some(Row::Folder { .. })));
        // Back ends a search before leaving the list.
        for c in "zz".chars() {
            list(&mut model, Act::Key(KeyInput::Character(c)));
        }
        model.tick(t + Duration::from_secs(5));
        assert_eq!(keys(&mut model), ["h:search:0"]);
        model.handle(Action::Nav(Nav::Back));
        assert_eq!(model.page, Page::List);
        assert!(!model.sidebar.search.active());
        model.handle(Action::Nav(Nav::Back));
        assert_eq!(model.page, Page::Settings);
    }

    #[test]
    fn opening_a_row_selects_it_and_the_folder_plus_makes_a_chat_there() {
        let mut model = model(&[conv("a", "")], &[folder("f", "")]);
        let commands = list(&mut model, Act::Open("a".into()));
        assert_eq!(commands[0]["type"], "load_conversation");
        assert_eq!(model.page, Page::Chat);
        assert_eq!(model.sidebar.selected.as_deref(), Some("a"));
        model.handle(Action::Nav(Nav::Back));
        list(&mut model, Act::NewChatIn("f".into()));
        assert_eq!(model.page, Page::Chat);
        assert_eq!(model.current_folder(), Some("f"));
        model.handle(Action::Nav(Nav::Back));
        list(&mut model, Act::NewChat);
        assert_eq!(model.current_folder(), None);
    }

    #[test]
    fn a_pin_is_verified_by_the_barrier_and_refreshed_when_not_made() {
        let mut model = model(&[conv("a", ""), conv("b", "")], &[]);
        let commands = list(&mut model, Act::Pin(vec![Key::Conv("a".into())], true));
        assert_eq!(commands[0]["type"], "pin_conversation");
        assert_eq!(commands[0]["pinned"], true);
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        assert_eq!(commands[1]["type"], "ping");
        assert_eq!(commands[1]["reqId"], id.as_str());
        assert_eq!(commands[1]["updateStatusOnly"], true);
        // The daemon's patch, then the barrier: done, quietly.
        event(
            &mut model,
            &format!(
                r#"{{"type":"sidebar_state_patched","conversations":[{}]}}"#,
                conv("a", r#""pinned":true"#)
            ),
        );
        assert!(event(&mut model, &format!(r#"{{"type":"pong","reqId":"{id}"}}"#)).is_empty());
        assert!(opened(&mut model).is_empty());
        assert!(model.sidebar.mutations.is_empty() && model.requests.is_empty());
        assert_eq!(keys(&mut model)[..2], ["h:pinned", "c:a:0"]);
        // A mark the daemon does not make: said, and the list asked for.
        let commands = list(&mut model, Act::Mark("b".into(), true));
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        let after = event(&mut model, &format!(r#"{{"type":"pong","reqId":"{id}"}}"#));
        assert_eq!(after[0]["type"], "list_conversations");
        assert_eq!(
            opened(&mut model),
            [super::super::super::list::actions::NOT_APPLIED]
        );
    }

    #[test]
    fn errors_and_timeouts_are_said() {
        let mut model = model(&[conv("a", "")], &[]);
        let commands = list(&mut model, Act::Mute(Key::Conv("a".into()), true));
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{id}","message":"No such thing"}}"#),
        );
        assert_eq!(opened(&mut model), ["Could not mute: No such thing"]);
        list(
            &mut model,
            Act::SetName(Key::Conv("a".into()), "New name".into()),
        );
        let late = model.answer_deadline().unwrap() + Duration::from_secs(1);
        assert!(model.expire(late));
        assert_eq!(
            opened(&mut model),
            [format!(
                "Could not rename. {}",
                super::super::super::list::actions::NO_ACK
            )]
        );
        // Not ready: nothing is sent.
        model.on_disconnected();
        assert!(list(&mut model, Act::Pin(vec![Key::Conv("a".into())], true)).is_empty());
        assert_eq!(
            opened(&mut model),
            ["Could not pin: not connected to the daemon."]
        );
    }

    #[test]
    fn delete_confirms_then_offers_undo_and_redo() {
        let mut model = model(&[conv("a", r#""title":"Old chat""#)], &[]);
        model.sidebar.selected = Some("a".into());
        list(&mut model, Act::ConfirmDelete(vec![Key::Conv("a".into())]));
        assert_eq!(opened(&mut model), ["Delete “Old chat”?"]);
        let commands = list(
            &mut model,
            Act::Delete {
                keys: vec![Key::Conv("a".into())],
                recursive: true,
            },
        );
        assert_eq!(commands[0]["type"], "delete_conversation");
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            r#"{"type":"conversation_deleted","convId":"a"}"#,
        );
        event(&mut model, &format!(r#"{{"type":"pong","reqId":"{id}"}}"#));
        assert_eq!(opened(&mut model), ["Moved to trash · Undo"]);
        assert_eq!(model.sidebar.selected, None);
        let commands = list(&mut model, Act::Undo);
        assert_eq!(commands.len(), 1, "undo has no barrier");
        assert_eq!(commands[0]["type"], "undo_delete");
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversation_restored","reqId":"{id}","summary":{}}}"#,
                conv("a", "")
            ),
        );
        assert_eq!(opened(&mut model), ["Restored from trash · Redo"]);
        assert!(matches!(&model.sidebar.reveal, Some(Row::Conversation { id, .. }) if id == "a"));
        // A folder's undo answers with a patch alone.
        let commands = list(&mut model, Act::Redo);
        event(
            &mut model,
            r#"{"type":"sidebar_state_patched","removedConversationIds":["a"]}"#,
        );
        assert_eq!(opened(&mut model), ["Moved to trash again · Undo"]);
        let id = commands[0]["reqId"].as_str().unwrap();
        assert!(!model.requests.is_pending(id));
        // Nothing to undo: the daemon says so.
        let commands = list(&mut model, Act::Undo);
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{id}","message":"Nothing to undo"}}"#),
        );
        assert_eq!(opened(&mut model), ["Could not undo: Nothing to undo"]);
    }

    #[test]
    fn a_new_folder_is_acked_then_shown_in_its_parent() {
        let mut model = model(&[], &[folder("p", "")]);
        list(
            &mut model,
            Act::NewFolder {
                parent: Some("p".into()),
                items: Vec::new(),
            },
        );
        assert_eq!(opened(&mut model), ["New subfolder"]);
        let commands = list(
            &mut model,
            Act::CreateFolder {
                name: " Ideas ".into(),
                parent: Some("p".into()),
                items: Vec::new(),
            },
        );
        assert_eq!(commands.len(), 1, "acked: no barrier");
        assert_eq!(commands[0]["type"], "create_folder");
        assert_eq!(commands[0]["name"], "Ideas");
        assert_eq!(commands[0]["parentId"], "p");
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        let deadline = model.answer_deadline().unwrap();
        assert!(deadline <= Instant::now() + requests_within());
        event(&mut model, &format!(r#"{{"type":"ack","reqId":"{id}"}}"#));
        event(
            &mut model,
            &format!(
                r#"{{"type":"sidebar_state_patched","folders":[{}]}}"#,
                folder("n", r#""name":"Ideas","parentId":"p""#)
            ),
        );
        assert!(matches!(&model.sidebar.reveal, Some(Row::Folder { id, .. }) if id == "n"));
        assert_eq!(keys(&mut model), ["f:p:0", "f:n:1"]);
    }

    fn requests_within() -> Duration {
        super::super::super::requests::FOLDER_CREATE_WITHIN
    }

    #[test]
    fn next_unread_wraps_through_the_tree_and_opens_folders_on_the_way() {
        let mut model = model(
            &[
                conv("a", r#""sortOrder":1,"unread":true"#),
                conv("b", r#""sortOrder":2"#),
                conv("c", r#""folderId":"f","streaming":true"#),
            ],
            &[folder("f", r#""sortOrder":3"#)],
        );
        model.sidebar.selected = Some("a".into());
        list(&mut model, Act::NextUnread);
        assert_eq!(model.chat.id.as_deref(), Some("c"));
        assert!(model.sidebar.expanded.contains("f"));
        model.handle(Action::Nav(Nav::Back));
        assert_eq!(model.sidebar.selected.as_deref(), Some("c"));
        list(&mut model, Act::NextUnread);
        assert_eq!(model.chat.id.as_deref(), Some("a"), "wrapping");
    }

    #[test]
    fn a_pin_shows_at_once_survives_other_traffic_and_goes_back_when_not_made() {
        let mut model = model(
            &[conv("a", r#""sortOrder":3"#), conv("b", r#""sortOrder":1"#)],
            &[],
        );
        let commands = list(&mut model, Act::Pin(vec![Key::Conv("a".into())], true));
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        // At once, before the daemon says anything.
        assert_eq!(keys(&mut model)[..2], ["h:pinned", "c:a:0"]);
        // A list from before the pin does not flicker it back.
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversations_list","conversations":[{},{}],"folders":[]}}"#,
                conv("a", r#""sortOrder":3"#),
                conv("b", r#""sortOrder":1"#)
            ),
        );
        assert_eq!(keys(&mut model)[..2], ["h:pinned", "c:a:0"]);
        // Never made: back where it was, said, and the list asked for.
        let after = event(&mut model, &format!(r#"{{"type":"pong","reqId":"{id}"}}"#));
        assert_eq!(after[0]["type"], "list_conversations");
        assert_eq!(keys(&mut model), ["c:b:0", "c:a:0"]);
        assert_eq!(
            opened(&mut model),
            [super::super::super::list::actions::NOT_APPLIED]
        );
        assert!(model.sidebar.pending.is_empty());
    }

    #[test]
    fn the_daemons_answer_wins_over_the_projection() {
        let mut model = model(
            &[
                conv("a", r#""sortOrder":3"#),
                conv("p", r#""pinned":true,"sortOrder":4"#),
            ],
            &[],
        );
        let commands = list(&mut model, Act::Pin(vec![Key::Conv("a".into())], true));
        assert_eq!(
            model.sidebar.conversations["a"].sort_order, 5.0,
            "the bottom of the pinned"
        );
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        // The daemon put it elsewhere: its patch is the truth.
        event(
            &mut model,
            &format!(
                r#"{{"type":"sidebar_state_patched","conversations":[{}]}}"#,
                conv("a", r#""pinned":true,"sortOrder":9"#)
            ),
        );
        assert_eq!(model.sidebar.conversations["a"].sort_order, 9.0);
        event(&mut model, &format!(r#"{{"type":"pong","reqId":"{id}"}}"#));
        assert!(opened(&mut model).is_empty());
        assert!(model.sidebar.pending.is_empty());
    }

    #[test]
    fn edits_expire_when_never_answered() {
        let mut model = model(&[conv("a", "")], &[]);
        list(&mut model, Act::Mark("a".into(), true));
        assert!(model.sidebar.conversations["a"].marked);
        assert!(model.tick(Instant::now() + Duration::from_secs(11)));
        assert!(!model.sidebar.conversations["a"].marked);
    }

    #[test]
    fn a_move_lands_at_the_top_of_the_folder_and_a_refusal_restores_it() {
        let mut model = model(
            &[
                conv("a", r#""pinned":true,"sortOrder":3"#),
                conv("b", r#""folderId":"f","sortOrder":-2"#),
                conv("c", r#""folderId":"f","pinned":true"#),
            ],
            &[folder("f", "")],
        );
        list(&mut model, Act::MovePicker(vec![Key::Conv("a".into())]));
        assert_eq!(opened(&mut model), ["Move “a”"]);
        let commands = list(
            &mut model,
            Act::MoveTo {
                keys: vec![Key::Conv("a".into())],
                parent: Some("f".into()),
            },
        );
        assert_eq!(commands[0]["type"], "move_sidebar_items");
        assert_eq!(commands[0]["parentId"], "f");
        assert_eq!(commands[0]["items"][0]["type"], "conversation");
        // The iPhone's projection: unpinned, at the top of the unpinned.
        let a = &model.sidebar.conversations["a"];
        assert_eq!(
            (a.folder_id.as_deref(), a.pinned, a.sort_order),
            (Some("f"), false, -4.0)
        );
        assert!(model.sidebar.expanded.contains("f"), "it opens to show it");
        assert_eq!(
            keys(&mut model),
            ["f:f:0", "c:c:1", "d:1", "c:a:1", "c:b:1"]
        );
        // Refused: the daemon's list with its reqId, and the move undone.
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversations_list","reqId":"{id}","conversations":[{},{},{}],"folders":[{}]}}"#,
                conv("a", r#""pinned":true,"sortOrder":3"#),
                conv("b", r#""folderId":"f","sortOrder":-2"#),
                conv("c", r#""folderId":"f","pinned":true"#),
                folder("f", "")
            ),
        );
        assert_eq!(model.sidebar.conversations["a"].folder_id, None);
        assert_eq!(
            opened(&mut model),
            ["Could not move “a”; the sidebar was restored."]
        );
    }

    #[test]
    fn sending_bumps_and_a_new_conversation_shows_until_its_request_fails() {
        let mut model = model(
            &[conv("a", r#""sortOrder":4"#), conv("b", r#""sortOrder":1"#)],
            &[],
        );
        model.list_observe(&[
            serde_json::json!({"type": "send_message", "reqId": "x", "convId": "a"}),
        ]);
        assert_eq!(keys(&mut model), ["c:a:0", "c:b:0"]);
        // The daemon's update with its new order settles it.
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversation_updated","summary":{}}}"#,
                conv("a", r#""sortOrder":-7"#)
            ),
        );
        assert!(model.sidebar.pending.is_empty());
        assert_eq!(model.sidebar.conversations["a"].sort_order, -7.0);
        // A new conversation: shown at once, at the top, while it is made.
        let request = model.requests.issue(
            Owner::Composer,
            "new",
            None,
            std::time::Duration::from_secs(20),
            Instant::now(),
        );
        model.list_observe(&[serde_json::json!({
            "type": "new_conversation", "reqId": request, "convId": "n",
            "initialMessage": {"text": "hi", "startedAt": 5}
        })]);
        assert_eq!(keys(&mut model)[0], "c:n:0");
        assert!(model.sidebar.conversations["n"].streaming);
        // A list from before it does not drop it.
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversations_list","conversations":[{},{}],"folders":[]}}"#,
                conv("a", r#""sortOrder":-7"#),
                conv("b", r#""sortOrder":1"#)
            ),
        );
        assert!(model.sidebar.conversations.contains_key("n"));
        // Refused: the row goes (after a moment for a late listing).
        event(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{request}","message":"Not authenticated"}}"#),
        );
        let now = Instant::now();
        model.list_tick(now);
        assert!(model.sidebar.conversations.contains_key("n"));
        model.list_tick(now + Duration::from_secs(2));
        assert!(!model.sidebar.conversations.contains_key("n"));
    }

    #[test]
    fn select_mode_pins_moves_groups_and_deletes_whats_selected() {
        let mut model = model(
            &[
                conv("a", r#""sortOrder":1"#),
                conv("b", r#""sortOrder":2"#),
                conv("c", r#""sortOrder":3"#),
            ],
            &[folder("f", r#""sortOrder":9"#)],
        );
        list(&mut model, Act::Select(Some(Key::Conv("a".into()))));
        list(&mut model, Act::ToggleSelect(Key::Conv("c".into())));
        list(&mut model, Act::ToggleSelect(Key::Folder("f".into())));
        list(&mut model, Act::ToggleSelect(Key::Folder("f".into())));
        assert_eq!(
            model.sidebar.selected_keys(),
            [Key::Conv("a".into()), Key::Conv("c".into())]
        );
        let commands = list(&mut model, Act::SelectPin);
        assert_eq!(commands[0]["type"], "pin_sidebar_items");
        assert_eq!(commands[0]["pins"].as_array().unwrap().len(), 2);
        // Pinned as a block, in their order.
        assert_eq!(keys(&mut model)[..3], ["h:pinned", "c:a:0", "c:c:0"]);
        list(&mut model, Act::SelectGroup);
        assert_eq!(opened(&mut model), ["New folder"]);
        let items = model.sidebar.selected_keys();
        let commands = list(
            &mut model,
            Act::CreateFolder {
                name: "Pair".into(),
                parent: None,
                items,
            },
        );
        assert_eq!(commands[0]["items"].as_array().unwrap().len(), 2);
        list(&mut model, Act::SelectDelete);
        assert_eq!(opened(&mut model), ["Delete 2 conversations?"]);
        let keys_selected = model.sidebar.selected_keys();
        let commands = list(
            &mut model,
            Act::Delete {
                keys: keys_selected,
                recursive: true,
            },
        );
        assert_eq!(commands[0]["type"], "delete_conversations");
        assert_eq!(keys(&mut model), ["c:b:0", "f:f:0"], "gone at once");
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            r#"{"type":"conversation_deleted","convId":"a"}"#,
        );
        event(
            &mut model,
            r#"{"type":"conversation_deleted","convId":"c"}"#,
        );
        event(&mut model, &format!(r#"{{"type":"pong","reqId":"{id}"}}"#));
        assert!(opened(&mut model).contains(&"Moved 2 chats to trash · Undo".to_owned()));
        assert_eq!(
            model.sidebar.selection,
            Some(BTreeSet::new()),
            "an emptied selection stays"
        );
        // Back ends select mode before anything else.
        model.handle(Action::Nav(Nav::Back));
        assert_eq!(model.page, Page::List);
        assert_eq!(model.sidebar.selection, None);
    }

    #[test]
    fn a_folders_instructions_load_and_save() {
        let mut model = model(&[], &[folder("f", "")]);
        let commands = list(&mut model, Act::FolderInfo("f".into()));
        assert_eq!(opened(&mut model), ["Folder"]);
        assert_eq!(commands[0]["type"], "load_folder_instructions");
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(
                r#"{{"type":"folder_instructions_loaded","reqId":"{id}","folderId":"f","text":"Be brief."}}"#
            ),
        );
        assert_eq!(
            model.sidebar.instructions["f"],
            Instructions::Loaded("Be brief.".into())
        );
        list(&mut model, Act::EditInstructions("f".into()));
        assert_eq!(opened(&mut model), ["Folder instructions"]);
        let commands = list(
            &mut model,
            Act::SaveInstructions {
                folder: "f".into(),
                text: "Be briefer.".into(),
            },
        );
        assert_eq!(commands[0]["type"], "set_folder_instructions");
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(
                r#"{{"type":"folder_instructions_updated","reqId":"{id}","folderId":"f","text":"Be briefer."}}"#
            ),
        );
        assert_eq!(opened(&mut model), ["Instructions saved"]);
        // A failed load says why in the sheet.
        model.sidebar.instructions.clear();
        let commands = list(&mut model, Act::FolderInfo("f".into()));
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{id}","message":"No folder"}}"#),
        );
        assert_eq!(
            model.sidebar.instructions["f"],
            Instructions::Failed("No folder".into())
        );
    }

    #[test]
    fn clone_reveals_the_copy() {
        let mut model = model(&[conv("a", "")], &[]);
        let commands = list(&mut model, Act::Clone("a".into()));
        assert_eq!(commands.len(), 1, "no barrier");
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(
                r#"{{"type":"conversation_restored","reqId":"{id}","summary":{}}}"#,
                conv("a2", "")
            ),
        );
        assert_eq!(opened(&mut model), ["Cloned"]);
        assert!(matches!(&model.sidebar.reveal, Some(Row::Conversation { id, .. }) if id == "a2"));
    }

    #[test]
    fn emoji_marks_prefix_the_title_and_the_same_one_takes_it_off() {
        let mut model = model(&[conv("a", r#""title":"🔥 Fix the build""#)], &[]);
        list(&mut model, Act::MarkEmoji("a".into()));
        assert_eq!(opened(&mut model), ["Emoji mark"]);
        let commands = list(
            &mut model,
            Act::SetName(Key::Conv("a".into()), "🐛 Fix the build".into()),
        );
        assert_eq!(commands[0]["type"], "rename_conversation");
        assert_eq!(commands[0]["title"], "🐛 Fix the build");
        // Shown at once.
        assert_eq!(model.sidebar.conversations["a"].title, "🐛 Fix the build");
    }

    #[test]
    fn the_subagent_badge_opens_the_child_and_opening_keeps_unread() {
        let mut model = model(
            &[
                conv(
                    "p",
                    r#""unread":true,"subagentCount":1,"tasks":[{"id":"k","kind":"subagent","title":"child","startedAt":1}]"#,
                ),
                conv("k", r#""parentConversationId":"p""#),
            ],
            &[],
        );
        let commands = list(&mut model, Act::Subagents("p".into()));
        assert_eq!(commands[0]["convId"], "k");
        assert_eq!(model.page, Page::Chat);
        model.handle(Action::Nav(Nav::Back));
        list(&mut model, Act::Open("p".into()));
        assert_eq!(model.sidebar.unread_at_open.get("p"), Some(&true));
        assert_eq!(model.sidebar.unread_at_open.get("k"), Some(&false));
    }

    #[test]
    fn the_daemons_update_status_is_kept_for_the_footer() {
        let mut model = model(&[conv("a", "")], &[]);
        assert!(
            model.on_connected()[0]["capabilities"]
                .as_array()
                .unwrap()
                .contains(&"update-status-push".into())
        );
        event(
            &mut model,
            r#"{"type":"update_status","status":"update_available"}"#,
        );
        assert_eq!(
            model.sidebar.update_status.as_deref(),
            Some("update_available")
        );
        // A barrier's pong carries it too.
        event(
            &mut model,
            r#"{"type":"pong","reqId":"x","updateStatus":"restart_needed"}"#,
        );
        assert_eq!(
            model.sidebar.update_status.as_deref(),
            Some("restart_needed")
        );
    }

    #[test]
    fn menus_offer_what_applies() {
        let mut model = model(&[conv("a", r#""pinned":true"#)], &[folder("f", "")]);
        let rect = RectF::new(0.0, 300.0, 1080.0, 160.0);
        list(
            &mut model,
            Act::RowMenu {
                id: "a".into(),
                rect,
            },
        );
        assert_eq!(
            opened(&mut model),
            [
                "menu: Unpin, Mark, Mute, Rename…, Emoji mark…, Move to folder…, Clone, Select, Copy id, Delete"
            ]
        );
        list(
            &mut model,
            Act::FolderMenu {
                id: "f".into(),
                rect,
            },
        );
        assert_eq!(
            opened(&mut model),
            [
                "menu: Pin, Mute, New chat, Rename…, New subfolder…, Move to folder…, Instructions & info, Delete…"
            ]
        );
        list(
            &mut model,
            Act::ConfirmDelete(vec![Key::Folder("f".into())]),
        );
        assert_eq!(opened(&mut model), ["Delete “f”"]);
        let commands = list(
            &mut model,
            Act::Delete {
                keys: vec![Key::Folder("f".into())],
                recursive: false,
            },
        );
        assert_eq!(commands[0]["mode"], "unwrap");
    }
}
