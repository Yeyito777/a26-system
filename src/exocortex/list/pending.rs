//! Optimistic list changes (PAR-073, 075, 076, 082; the iPhone's
//! `state/sidebar-pending.ts`, the TUI's `sidebar/pendingmoves.ts`): a pin,
//! mark, mute, rename, move or deletion shows at once, before the daemon
//! answers, and stays over every update the daemon sends meanwhile, so
//! unrelated traffic cannot flicker it back.
//!
//! An edit names the fields it expects per item. The list's maps
//! (`Sidebar::conversations`, `folders`) are what shows; for an item an edit
//! is about, what the daemon last said is kept apart (`Pending::shadow`).
//! Each daemon event is applied to the daemon's values, then the edits still
//! unsettled are projected over them again. An edit settles when the
//! daemon's state shows it, or when the daemon's own answer for those items
//! arrives (its values then win); it is dropped when refused, when the
//! barrier finds it not made, or after 10 s.
//!
//! The placement rules mirror the daemon's (`conversations.moveSidebarItems`,
//! the iPhone's `sidebar-model.ts`), so an optimistic move lands where the
//! daemon will put it.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::super::model::sidebar::Sidebar;
use super::super::proto::{Event, Folder, Summary};

/// How long an unanswered edit shows.
pub const TTL: Duration = Duration::from_secs(10);

/// A list item.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Conv(String),
    Folder(String),
}

impl Key {
    /// The daemon's `SidebarItemRef`.
    pub fn item_ref(&self) -> serde_json::Value {
        match self {
            Key::Conv(id) => serde_json::json!({ "type": "conversation", "id": id }),
            Key::Folder(id) => serde_json::json!({ "type": "folder", "id": id }),
        }
    }
}

/// An item as the daemon said it (or as shown).
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Conv(Box<Summary>),
    Folder(Folder),
}

/// The fields an edit expects of one item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Expect {
    pub sort_order: Option<f64>,
    pub parent: Option<Option<String>>,
    pub pinned: Option<bool>,
    pub marked: Option<bool>,
    pub muted: Option<bool>,
    /// A conversation's title, a folder's name.
    pub title: Option<String>,
    /// The item is gone.
    pub removed: bool,
    /// A conversation the daemon is making, shown until it lists it.
    pub created: Option<Box<Summary>>,
}

impl Expect {
    /// `other`'s fields over these (a later edit wins).
    fn merge(&mut self, other: &Expect) {
        if other.sort_order.is_some() {
            self.sort_order = other.sort_order;
        }
        if other.parent.is_some() {
            self.parent.clone_from(&other.parent);
        }
        if other.pinned.is_some() {
            self.pinned = other.pinned;
        }
        if other.marked.is_some() {
            self.marked = other.marked;
        }
        if other.muted.is_some() {
            self.muted = other.muted;
        }
        if other.title.is_some() {
            self.title.clone_from(&other.title);
        }
        self.removed |= other.removed;
        if other.created.is_some() {
            self.created.clone_from(&other.created);
        }
    }

    /// Whether `item` shows these fields.
    fn matches(&self, item: Option<&Item>) -> bool {
        if self.removed {
            return item.is_none();
        }
        let Some(item) = item else {
            return false;
        };
        let (sort_order, parent, pinned, muted) = match item {
            Item::Conv(c) => (c.sort_order, c.folder_id.as_deref(), c.pinned, c.muted),
            Item::Folder(f) => (f.sort_order, f.parent_id.as_deref(), f.pinned, f.muted),
        };
        self.sort_order.is_none_or(|s| s == sort_order)
            && self.parent.as_ref().is_none_or(|p| p.as_deref() == parent)
            && self.pinned.is_none_or(|p| p == pinned)
            && self.muted.is_none_or(|m| m == muted)
            && match item {
                Item::Conv(c) => {
                    self.marked.is_none_or(|m| m == c.marked)
                        && self.title.as_ref().is_none_or(|t| *t == c.title)
                }
                Item::Folder(f) => self.title.as_ref().is_none_or(|t| *t == f.name),
            }
    }

    /// `item` with these fields (None: removed, or nothing to show).
    fn apply(&self, item: Option<&Item>) -> Option<Item> {
        if self.removed {
            return None;
        }
        let mut item = match item {
            Some(item) => item.clone(),
            None => Item::Conv(self.created.clone()?),
        };
        match &mut item {
            Item::Conv(c) => {
                if let Some(s) = self.sort_order {
                    c.sort_order = s;
                }
                if let Some(p) = &self.parent {
                    c.folder_id.clone_from(p);
                }
                if let Some(p) = self.pinned {
                    c.pinned = p;
                }
                if let Some(m) = self.marked {
                    c.marked = m;
                }
                if let Some(m) = self.muted {
                    c.muted = m;
                }
                if let Some(t) = &self.title {
                    c.title.clone_from(t);
                }
            }
            Item::Folder(f) => {
                if let Some(s) = self.sort_order {
                    f.sort_order = s;
                }
                if let Some(p) = &self.parent {
                    f.parent_id.clone_from(p);
                }
                if let Some(p) = self.pinned {
                    f.pinned = p;
                }
                if let Some(m) = self.muted {
                    f.muted = m;
                }
                if let Some(t) = &self.title {
                    f.name.clone_from(t);
                }
            }
        }
        Some(item)
    }
}

/// The daemon's own answer for an edit's items: once it arrives, its values
/// show, whatever the projection guessed (a pin's exact sort order).
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    None,
    /// A state patch carrying every one of these.
    Patch(Vec<Key>),
    Marked(String),
    /// The conversation's update with a sort order other than this.
    Moved(String, f64),
}

impl Answer {
    fn answered_by(&self, event: &Event) -> bool {
        match (self, event) {
            (
                Answer::Patch(keys),
                Event::SidebarStatePatched {
                    conversations,
                    folders,
                    removed_conversation_ids,
                    removed_folder_ids,
                },
            ) => keys.iter().all(|key| match key {
                Key::Conv(id) => {
                    conversations.iter().any(|c| &c.id == id)
                        || removed_conversation_ids.contains(id)
                }
                Key::Folder(id) => {
                    folders.iter().any(|f| &f.id == id) || removed_folder_ids.contains(id)
                }
            }),
            (Answer::Marked(id), Event::ConversationMarked(marked)) => &marked.conv_id == id,
            (Answer::Moved(id, old), Event::ConversationUpdated { summary, .. }) => {
                &summary.id == id && summary.sort_order != *old
            }
            (Answer::Moved(id, _), _) => {
                Answer::Patch(vec![Key::Conv(id.clone())]).answered_by(event)
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
struct Edit {
    id: u64,
    expected: Vec<(Key, Expect)>,
    answer: Answer,
    acknowledged: bool,
    expires: Instant,
}

#[derive(Debug, Default)]
pub struct Pending {
    edits: Vec<Edit>,
    next: u64,
    /// What the daemon last said of the items edits show changed (None:
    /// it has no such item).
    shadow: HashMap<Key, Option<Item>>,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.shadow.is_empty()
    }

    /// Shown by the daemon, or already gone.
    pub fn is_settled(&self, id: u64) -> bool {
        self.edits
            .iter()
            .find(|edit| edit.id == id)
            .is_none_or(|edit| edit.acknowledged)
    }

    /// The edits still showing, merged per item (in order: later wins).
    fn merged(&self) -> HashMap<Key, Expect> {
        let mut merged: HashMap<Key, Expect> = HashMap::new();
        for edit in self.edits.iter().filter(|edit| !edit.acknowledged) {
            for (key, expect) in &edit.expected {
                merged.entry(key.clone()).or_default().merge(expect);
            }
        }
        merged
    }

    /// Forgets expired edits and, once every earlier one settled, the
    /// settled ones; true when any went.
    fn prune(&mut self, now: Instant) -> bool {
        let before = self.edits.len();
        self.edits.retain(|edit| edit.expires > now);
        while self.edits.first().is_some_and(|edit| edit.acknowledged) {
            self.edits.remove(0);
        }
        self.edits.len() != before
    }
}

impl Sidebar {
    fn item(&self, key: &Key) -> Option<Item> {
        match key {
            Key::Conv(id) => self
                .conversations
                .get(id)
                .map(|c| Item::Conv(Box::new(c.clone()))),
            Key::Folder(id) => self.folders.get(id).map(|f| Item::Folder(f.clone())),
        }
    }

    fn set_item(&mut self, key: &Key, item: Option<Item>) {
        match (key, item) {
            (Key::Conv(id), Some(Item::Conv(c))) => {
                self.conversations.insert(id.clone(), *c);
            }
            (Key::Folder(id), Some(Item::Folder(f))) => {
                self.folders.insert(id.clone(), f);
            }
            (Key::Conv(id), _) => {
                self.conversations.remove(id);
            }
            (Key::Folder(id), _) => {
                self.folders.remove(id);
            }
        }
    }

    /// Shows an edit at once; returns its id (for `drop_edit`,
    /// `edit_settled`).
    pub fn add_edit(&mut self, expected: Vec<(Key, Expect)>, answer: Answer, now: Instant) -> u64 {
        for (key, _) in &expected {
            if !self.pending.shadow.contains_key(key) {
                let item = self.item(key);
                self.pending.shadow.insert(key.clone(), item);
            }
        }
        self.pending.next += 1;
        let id = self.pending.next;
        self.pending.edits.push(Edit {
            id,
            expected,
            answer,
            acknowledged: false,
            expires: now + TTL,
        });
        self.project(now);
        id
    }

    /// The edit was refused or not made: what the daemon says shows again.
    pub fn drop_edit(&mut self, id: u64, now: Instant) {
        self.pending.edits.retain(|edit| edit.id != id);
        self.project(now);
    }

    pub fn edit_settled(&self, id: u64) -> bool {
        self.pending.is_settled(id)
    }

    /// Before a daemon event: the daemon's own values for the edited items,
    /// so the event lands on them.
    pub(crate) fn unproject(&mut self) {
        let shadow: Vec<(Key, Option<Item>)> = self
            .pending
            .shadow
            .iter()
            .map(|(key, item)| (key.clone(), item.clone()))
            .collect();
        for (key, item) in shadow {
            self.set_item(&key, item);
        }
    }

    /// After a daemon event (applied over `unproject`'s values): it is
    /// what the daemon says; edits it answers or shows settle; the rest
    /// show over it again.
    pub(crate) fn reproject(&mut self, event: &Event, now: Instant) {
        let keys: Vec<Key> = self.pending.shadow.keys().cloned().collect();
        for key in keys {
            let item = self.item(&key);
            self.pending.shadow.insert(key, item);
        }
        let shadow = &self.pending.shadow;
        for edit in self
            .pending
            .edits
            .iter_mut()
            .filter(|edit| !edit.acknowledged)
        {
            edit.acknowledged = edit.answer.answered_by(event)
                || edit
                    .expected
                    .iter()
                    .all(|(key, expect)| expect.matches(shadow.get(key).and_then(Option::as_ref)));
        }
        self.project(now);
    }

    /// Expired edits go (from the event loop's tick); true when the list
    /// changed.
    pub(crate) fn expire_edits(&mut self, now: Instant) -> bool {
        if self.pending.edits.iter().any(|edit| edit.expires <= now) {
            self.project(now);
            return true;
        }
        false
    }

    /// Shows the daemon's values with the unsettled edits over them.
    fn project(&mut self, now: Instant) {
        self.pending.prune(now);
        let merged = self.pending.merged();
        let keys: Vec<Key> = self.pending.shadow.keys().cloned().collect();
        for key in keys {
            let base = self.pending.shadow.get(&key).cloned().flatten();
            match merged.get(&key) {
                Some(expect) => {
                    let shown = expect.apply(base.as_ref());
                    self.set_item(&key, shown);
                }
                None => {
                    self.set_item(&key, base);
                    self.pending.shadow.remove(&key);
                }
            }
        }
        self.restructured();
    }

    // ----- Placement (the daemon's rules) ----------------------------------------

    /// A folder id the list has, else the top level.
    pub fn parent_of_id(&self, id: Option<&str>) -> Option<String> {
        id.filter(|id| self.folders.contains_key(*id))
            .map(str::to_owned)
    }

    pub fn parent_of(&self, key: &Key) -> Option<Option<String>> {
        match key {
            Key::Conv(id) => self
                .conversations
                .get(id)
                .map(|c| self.parent_of_id(c.folder_id.as_deref())),
            Key::Folder(id) => self
                .folders
                .get(id)
                .map(|f| self.parent_of_id(f.parent_id.as_deref())),
        }
    }

    pub fn sort_order_of(&self, key: &Key) -> Option<f64> {
        match key {
            Key::Conv(id) => self.conversations.get(id).map(|c| c.sort_order),
            Key::Folder(id) => self.folders.get(id).map(|f| f.sort_order),
        }
    }

    pub fn pinned_of(&self, key: &Key) -> Option<bool> {
        match key {
            Key::Conv(id) => self.conversations.get(id).map(|c| c.pinned),
            Key::Folder(id) => self.folders.get(id).map(|f| f.pinned),
        }
    }

    /// A level's items (key, pinned, sort order), pinned first, then by
    /// sort order.
    pub fn placement_entries(&self, parent: Option<&str>) -> Vec<(Key, bool, f64)> {
        let mut out: Vec<(Key, bool, f64)> = Vec::new();
        for c in self.conversations.values() {
            if self.parent_of_id(c.folder_id.as_deref()).as_deref() == parent {
                out.push((Key::Conv(c.id.clone()), c.pinned, c.sort_order));
            }
        }
        for f in self.folders.values() {
            if self.parent_of_id(f.parent_id.as_deref()).as_deref() == parent {
                out.push((Key::Folder(f.id.clone()), f.pinned, f.sort_order));
            }
        }
        out.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
        });
        out
    }

    /// Folder `id` and every folder below it.
    pub fn descendant_folders(&self, id: &str) -> HashSet<String> {
        let mut out = HashSet::from([id.to_owned()]);
        let mut grew = true;
        while grew {
            grew = false;
            for f in self.folders.values() {
                if f.parent_id.as_ref().is_some_and(|p| out.contains(p)) && !out.contains(&f.id) {
                    out.insert(f.id.clone());
                    grew = true;
                }
            }
        }
        out
    }

    /// The sort order after a pin toggle: the bottom of the pinned section,
    /// or the top of the unpinned one.
    pub fn pinned_sort_order(&self, key: &Key, pinned: bool) -> f64 {
        let parent = self.parent_of(key).flatten();
        let entries = self.placement_entries(parent.as_deref());
        let others = entries.iter().filter(|(k, _, _)| k != key);
        if pinned {
            let max = others
                .filter(|(_, p, _)| *p)
                .map(|(_, _, s)| *s)
                .fold(f64::NEG_INFINITY, f64::max);
            if max == f64::NEG_INFINITY {
                0.0
            } else {
                max + 1.0
            }
        } else {
            top_unpinned(others.map(|(_, p, s)| (*p, *s)))
        }
    }

    /// Where `move_sidebar_items {items, parentId, before?, preservePinned?,
    /// placement?}` puts each item (the TUI's `projectSidebarItemsMove`):
    /// without an anchor, the top of the destination's unpinned section,
    /// unpinned; before an anchor, between it and the one above it (in the
    /// same pinned group when pins are kept); or at the group's bottom.
    /// None when nothing can move.
    pub fn project_move(
        &self,
        items: &[Key],
        parent: Option<&str>,
        place: Place,
    ) -> Option<Vec<(Key, Expect)>> {
        let parent = self.parent_of_id(parent);
        let mut movable: Vec<Key> = Vec::new();
        for key in items {
            if movable.contains(key) {
                continue;
            }
            match key {
                Key::Conv(id) if self.conversations.contains_key(id) => {}
                Key::Folder(id) if self.folders.contains_key(id) => {
                    // Never into itself or below itself.
                    if parent
                        .as_ref()
                        .is_some_and(|p| self.descendant_folders(id).contains(p))
                    {
                        continue;
                    }
                }
                _ => continue,
            }
            movable.push(key.clone());
        }
        if movable.is_empty() {
            return None;
        }
        let n = movable.len() as f64;
        let all = self.placement_entries(parent.as_deref());
        let dest: Vec<&(Key, bool, f64)> = all
            .iter()
            .filter(|(k, _, _)| !movable.contains(k))
            .collect();
        let keep = place.keep_pinned();
        let preserved = keep.then(|| self.pinned_of(&movable[0])).flatten();
        let homogeneous =
            preserved.is_some_and(|p| movable.iter().all(|k| self.pinned_of(k) == Some(p)));
        let anchors: Vec<&(Key, bool, f64)> = if homogeneous {
            dest.iter()
                .copied()
                .filter(|(_, p, _)| Some(*p) == preserved)
                .collect()
        } else {
            dest.clone()
        };
        let (start, step) = match &place {
            Place::Before(before) => {
                match anchors.iter().position(|(k, _, _)| k == before) {
                    Some(index) => {
                        let anchor = anchors[index].2;
                        match index.checked_sub(1).map(|i| anchors[i].2) {
                            Some(prev) => {
                                let step = (anchor - prev) / (n + 1.0);
                                (prev + step, step)
                            }
                            None => (anchor - n, 1.0),
                        }
                    }
                    // An anchor elsewhere: the top, as without one.
                    None => (top_unpinned(all.iter().map(|(_, p, s)| (*p, *s))) - n, 1.0),
                }
            }
            Place::Bottom => {
                let pool = if homogeneous { &anchors } else { &dest };
                let max = pool
                    .iter()
                    .map(|(_, _, s)| *s)
                    .fold(f64::NEG_INFINITY, f64::max);
                (
                    if max == f64::NEG_INFINITY {
                        0.0
                    } else {
                        max + 1.0
                    },
                    1.0,
                )
            }
            Place::Top => (top_unpinned(all.iter().map(|(_, p, s)| (*p, *s))) - n, 1.0),
        };
        Some(
            movable
                .into_iter()
                .enumerate()
                .map(|(index, key)| {
                    let pinned = keep && self.pinned_of(&key).unwrap_or(false);
                    let expect = Expect {
                        sort_order: Some(start + step * index as f64),
                        parent: Some(parent.clone()),
                        pinned: Some(pinned),
                        ..Expect::default()
                    };
                    (key, expect)
                })
                .collect(),
        )
    }
}

/// Where a move puts its items in their new level.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    /// The top of the unpinned section, unpinned (into a folder).
    Top,
    /// Before this item, pins kept (a reorder).
    Before(Key),
    /// The bottom of the items' pinned group, pins kept.
    Bottom,
}

impl Place {
    fn keep_pinned(&self) -> bool {
        !matches!(self, Place::Top)
    }
}

/// The sort order that puts an item at the top of an unpinned section.
pub fn top_unpinned(items: impl Iterator<Item = (bool, f64)>) -> f64 {
    let min = items
        .filter(|(pinned, _)| !pinned)
        .map(|(_, s)| s)
        .fold(0.0, f64::min);
    min - 1.0
}
