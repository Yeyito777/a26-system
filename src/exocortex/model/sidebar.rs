//! The conversation list (WP-B) as the daemon keeps it: summaries and
//! folders, each level's rows in the TUI's order, and what the user does
//! with it.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::Value;

use super::super::proto::{Event, Folder, Summary};
use super::super::requests::Pending;
use super::super::ui::icons::Icon;
use super::super::ui::menu::{AnchoredMenu, MenuItem, Tone};
use super::{Action, Model, Nav, UiAct};
use crate::gfx::RectF;

/// What the user does with the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    OpenFolder(String),
    /// Up a folder level.
    Up,
    /// The header's search field (B: search).
    Search,
    /// The header's folder-plus (B: new folder).
    NewFolder,
    /// The header's ⋯: its menu, under the button.
    Menu(RectF),
    /// A long press on a conversation row: its menu, anchored to the row.
    RowMenu {
        id: String,
        rect: RectF,
    },
}

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
    /// The folder path shown, innermost last (drill-down; B replaces it
    /// with an expanding tree).
    pub path: Vec<String>,
}

pub(crate) fn order(pinned: bool, sort: f64) -> (bool, f64) {
    (!pinned, sort)
}

impl Sidebar {
    pub(crate) fn changed(&mut self) {
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
                ..
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
            | Event::ConversationRestored { summary, .. }
            | Event::ConversationUnwound { summary, .. } => {
                self.conversations
                    .insert(summary.id.clone(), summary.clone());
            }
            Event::ConversationDeleted { conv_id, .. } => {
                self.conversations.remove(conv_id);
            }
            Event::ConversationMarked(marked) => {
                if let Some(summary) = self.conversations.get_mut(&marked.conv_id) {
                    summary.marked = marked.marked;
                }
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

    pub(crate) fn build_rows(&self, folder: Option<&str>) -> Vec<Row> {
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
    pub(crate) fn folder_counts(&self) -> HashMap<&str, usize> {
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

impl Sidebar {
    /// The folder the list shows.
    pub fn current_folder(&self) -> Option<&str> {
        self.path.last().map(String::as_str)
    }

    /// Up a folder level; false at the top.
    pub fn up(&mut self) -> bool {
        self.path.pop().is_some()
    }
}

impl Model {
    pub(super) fn handle_list(&mut self, act: Act) -> Vec<Value> {
        match act {
            Act::OpenFolder(id) => self.sidebar.path.push(id),
            Act::Up => {
                self.sidebar.up();
            }
            Act::Search | Act::NewFolder => {}
            Act::Menu(anchor) => {
                let items = vec![MenuItem::new(
                    Icon::Gear,
                    "Settings",
                    Tone::Plain,
                    Action::Nav(Nav::OpenSettings),
                )];
                self.ui
                    .open(Box::new(AnchoredMenu::new(anchor, items, false)));
            }
            Act::RowMenu { id, rect } => {
                let items = vec![MenuItem::new(
                    Icon::Copy,
                    "Copy id",
                    Tone::Plain,
                    Action::Ui(UiAct::Copy(id)),
                )];
                self.ui.open(Box::new(AnchoredMenu::new(rect, items, true)));
            }
        }
        Vec::new()
    }

    pub(super) fn apply_list(&mut self, event: &Event, _now: Instant) -> Vec<Value> {
        self.sidebar.apply(event);
        Vec::new()
    }

    /// An error answering one of the list's requests; true when handled.
    pub(super) fn list_error(&mut self, _id: &str, _message: &str) -> bool {
        false
    }

    /// One of the list's requests was not answered in time.
    pub(super) fn list_timeout(&mut self, _id: &str, _pending: &Pending) -> bool {
        self.generic_timeout()
    }
}
