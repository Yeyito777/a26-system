//! The conversation list (WP-B) as the daemon keeps it: summaries and
//! folders, the rows the list shows — pinned first under a PINNED header,
//! folders expanding in place (PAR-052, 053), or flat search results
//! (PAR-061) — the activity added up per folder, and what the user does
//! with it (`list/actions.rs`).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::list::activity::{self, FolderActivity, QueueInfo};
use super::super::list::search::{self, Search};
use super::super::proto::{Event, Folder, Queued, Summary};
use super::super::store;
use super::super::theme::{DIVIDER_ROW, HEADER_ROW, LIST_ROW};
use super::super::ui::Haptic;
use super::{Action, Effect, Model, Nav};
use crate::control::KeyboardPurpose;
use crate::gfx::RectF;
use crate::keys::KeyInput;

pub use super::super::list::actions::Mutation;

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
    Pin(String, bool),
    Mark(String, bool),
    Mute(String, bool),
    /// Rename…: the prompt.
    Rename(String),
    SetTitle(String, String),
    /// Delete: the confirmation.
    ConfirmDelete(String),
    Delete(String),
    Undo,
    Redo,
    /// New folder…: the prompt (in a folder: a subfolder).
    NewFolder(Option<String>),
    CreateFolder {
        name: String,
        parent: Option<String>,
    },
    PinFolder(String, bool),
    MuteFolder(String, bool),
    RenameFolder(String),
    SetFolderName(String, String),
    /// Delete…: the choice of keeping its children or not.
    ConfirmDeleteFolder(String),
    DeleteFolder {
        id: String,
        recursive: bool,
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
            Row::Pinned | Row::Results(_) => HEADER_ROW,
            Row::Divider { .. } => DIVIDER_ROW,
            Row::Folder { .. } | Row::Conversation { .. } => LIST_ROW,
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
    pub mutations: HashMap<String, Mutation>,
    /// A folder created here, to show once the daemon lists it: its name,
    /// parent, and the folders there were before.
    pub created: Option<(String, Option<String>, HashSet<String>)>,
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
    layout: Option<((u64, String), Rc<Layout>)>,
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

    pub fn apply(&mut self, event: &Event) -> bool {
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
            _ => return false,
        }
        true
    }

    /// The rows the list shows now (cached until something they depend on
    /// changes).
    pub fn layout(&mut self) -> Rc<Layout> {
        let key = (self.structure, self.search.query().to_owned());
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

    /// Back on the list: ends a search first. False when there was none.
    pub fn up(&mut self) -> bool {
        if self.search.focused || !self.search.text.is_empty() || self.search.active() {
            self.search.clear();
            self.search.focused = false;
            self.searched();
            return true;
        }
        false
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

    /// The debounced search follows the typing; true when the list changed.
    pub fn list_tick(&mut self, now: Instant) -> bool {
        if self.sidebar.search.tick(now) {
            self.sidebar.searched();
            return true;
        }
        false
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
                self.sidebar.selected = Some(id.clone());
                return self.handle(Action::Nav(Nav::OpenConversation(id)));
            }
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
            Act::ClearSearch => {
                self.sidebar.search.clear();
                self.sidebar.searched();
            }
            Act::Key(KeyInput::Enter) => {
                self.unfocus_search();
                self.sidebar.changed();
            }
            Act::Key(input) => {
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
                        self.sidebar.selected = Some(id.clone());
                        return self.handle(Action::Nav(Nav::OpenConversation(id)));
                    }
                    None => self.ui.toast("No unread chats."),
                }
            }
            Act::Pin(id, pinned) => return self.list_pin(id, pinned),
            Act::Mark(id, marked) => return self.list_mark(id, marked),
            Act::Mute(id, muted) => return self.list_mute(id, muted),
            Act::Rename(id) => self.rename_prompt(id),
            Act::SetTitle(id, title) => return self.list_set_title(id, title),
            Act::ConfirmDelete(id) => self.confirm_delete(id),
            Act::Delete(id) => return self.list_delete(id),
            Act::Undo => return self.list_undo(false),
            Act::Redo => return self.list_undo(true),
            Act::NewFolder(parent) => self.new_folder_prompt(parent),
            Act::CreateFolder { name, parent } => return self.list_create_folder(name, parent),
            Act::PinFolder(id, pinned) => return self.list_pin_folder(id, pinned),
            Act::MuteFolder(id, muted) => return self.list_mute_folder(id, muted),
            Act::RenameFolder(id) => self.rename_folder_prompt(id),
            Act::SetFolderName(id, name) => return self.list_set_folder_name(id, name),
            Act::ConfirmDeleteFolder(id) => self.confirm_delete_folder(id),
            Act::DeleteFolder { id, recursive } => return self.list_delete_folder(id, recursive),
        }
        Vec::new()
    }

    pub(super) fn apply_list(&mut self, event: &Event, now: Instant) -> Vec<Value> {
        self.restore_list();
        let open = self.sidebar.expanded.len();
        self.sidebar.apply(event);
        // Folders that went away are not kept open.
        if self.sidebar.expanded.len() != open {
            self.keep_list();
        }
        self.settle_list(event, now)
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
        let commands = list(&mut model, Act::Pin("a".into(), true));
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
        let commands = list(&mut model, Act::Mute("a".into(), true));
        let id = commands[0]["reqId"].as_str().unwrap().to_owned();
        event(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{id}","message":"No such thing"}}"#),
        );
        assert_eq!(opened(&mut model), ["Could not mute: No such thing"]);
        list(&mut model, Act::SetTitle("a".into(), "New name".into()));
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
        assert!(list(&mut model, Act::Pin("a".into(), true)).is_empty());
        assert_eq!(
            opened(&mut model),
            ["Could not pin: not connected to the daemon."]
        );
    }

    #[test]
    fn delete_confirms_then_offers_undo_and_redo() {
        let mut model = model(&[conv("a", r#""title":"Old chat""#)], &[]);
        model.sidebar.selected = Some("a".into());
        list(&mut model, Act::ConfirmDelete("a".into()));
        assert_eq!(opened(&mut model), ["Delete “Old chat”?"]);
        let commands = list(&mut model, Act::Delete("a".into()));
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
        list(&mut model, Act::NewFolder(Some("p".into())));
        assert_eq!(opened(&mut model), ["New subfolder"]);
        let commands = list(
            &mut model,
            Act::CreateFolder {
                name: " Ideas ".into(),
                parent: Some("p".into()),
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
            ["menu: Unpin, Mark, Mute, Rename…, Copy id, Delete"]
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
            ["menu: Pin, Mute, New chat, Rename…, New subfolder…, Delete…"]
        );
        list(&mut model, Act::ConfirmDeleteFolder("f".into()));
        assert_eq!(opened(&mut model), ["Delete “f”"]);
        let commands = list(
            &mut model,
            Act::DeleteFolder {
                id: "f".into(),
                recursive: false,
            },
        );
        assert_eq!(commands[0]["mode"], "unwrap");
    }
}
