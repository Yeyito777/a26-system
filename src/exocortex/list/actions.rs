//! The list's commands (PAR-064–079): pin, mark, mute, rename, move, clone
//! and delete conversations and folders (with the undo snackbar), undo and
//! redo deletions, create folders, ask for the list again.
//!
//! A change shows at once (`list/pending.rs`) and is then verified: most
//! list commands answer nothing and fail silently, so each is followed on
//! the same connection by a `ping` with its reqId. The daemon handles a
//! client's commands in order, so by the `pong` every broadcast the command
//! caused has arrived, and a change the daemon's state still does not show
//! was not made (PAR-074): it is taken back, the user told, and the list
//! asked for again. Commands that ack (renames, a new folder) are through at
//! their ack; undo, redo and clone at the conversation they restore or the
//! state patch they cause; a move the daemon refuses comes back as a list
//! with its reqId.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::model::Model;
use super::super::model::sidebar::{Act, Row, Sidebar};
use super::super::proto::{self, Event, Summary};
use super::super::requests::{self, Owner, Pending};
use super::super::ui::Haptic;
use super::super::ui::toast::Snackbar;
use super::activity;
use super::pending::{Answer, Expect, Key, top_unpinned};

/// The daemon did not make a change it was asked for.
pub const NOT_APPLIED: &str =
    "The daemon did not apply this sidebar change. The sidebar has been refreshed.";
/// A change was not answered in time.
pub const NO_ACK: &str = "No acknowledgement received. Check the conversation before retrying; the command may have been accepted.";

/// How long a new conversation's row stays after its request went without
/// the daemon listing it (its `conversation_updated` follows the answer).
const CREATED_GRACE: Duration = Duration::from_millis(1500);

/// A list command sent and not yet seen through.
#[derive(Debug, Clone, PartialEq)]
pub enum Mutation {
    Pin {
        keys: Vec<Key>,
        pinned: bool,
    },
    Mark {
        conv: String,
        marked: bool,
    },
    Mute {
        key: Key,
        muted: bool,
    },
    Rename {
        key: Key,
    },
    Move {
        keys: Vec<Key>,
        label: String,
    },
    Delete {
        convs: Vec<String>,
        /// Folders (id, name).
        folders: Vec<(String, String)>,
        recursive: bool,
    },
    Undo,
    Redo,
    Clone,
    CreateFolder {
        name: String,
        parent: Option<String>,
        /// The folders there were before.
        before: HashSet<String>,
        /// Moved into it.
        items: Vec<Key>,
    },
    /// The list asked for again.
    Refresh,
}

/// A command sent, and the change it shows meanwhile.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    pub mutation: Mutation,
    pub edit: Option<u64>,
}

impl Mutation {
    fn kind(&self) -> &'static str {
        match self {
            Mutation::Pin { .. } => "pin",
            Mutation::Mark { .. } => "mark",
            Mutation::Mute { .. } => "mute",
            Mutation::Rename { .. } => "rename",
            Mutation::Move { .. } => "move",
            Mutation::Delete { .. } => "delete",
            Mutation::Undo => "undo",
            Mutation::Redo => "redo",
            Mutation::Clone => "clone",
            Mutation::CreateFolder { .. } => "folder",
            Mutation::Refresh => "list",
        }
    }

    /// What failing at it is called.
    fn failure(&self) -> String {
        match self {
            Mutation::Pin { pinned: true, .. } => "Could not pin".into(),
            Mutation::Pin { .. } => "Could not unpin".into(),
            Mutation::Mark { marked: true, .. } => "Could not mark".into(),
            Mutation::Mark { .. } => "Could not unmark".into(),
            Mutation::Mute { muted: true, .. } => "Could not mute".into(),
            Mutation::Mute { .. } => "Could not unmute".into(),
            Mutation::Rename {
                key: Key::Folder(_),
            } => "Could not rename the folder".into(),
            Mutation::Rename { .. } => "Could not rename".into(),
            Mutation::Move { label, .. } => format!("Could not move {label}"),
            Mutation::Delete { convs, folders, .. } if convs.is_empty() && folders.len() == 1 => {
                "Could not delete the folder".into()
            }
            Mutation::Delete { .. } => "Could not delete".into(),
            Mutation::Undo => "Could not undo".into(),
            Mutation::Redo => "Could not redo".into(),
            Mutation::Clone => "Could not clone".into(),
            Mutation::CreateFolder { .. } => "Could not create the folder".into(),
            Mutation::Refresh => "Could not load the chats".into(),
        }
    }

    /// Through at its ack.
    fn acked(&self) -> bool {
        matches!(
            self,
            Mutation::Rename { .. } | Mutation::CreateFolder { .. }
        )
    }

    /// Verified by a barrier ping.
    fn barrier(&self) -> bool {
        matches!(
            self,
            Mutation::Pin { .. }
                | Mutation::Mark { .. }
                | Mutation::Mute { .. }
                | Mutation::Move { .. }
                | Mutation::Delete { .. }
        )
    }
}

/// “Title”, “Folder” or "N items" (the TUI's `sidebarMoveLabel`).
pub fn label_for(sidebar: &Sidebar, keys: &[Key]) -> String {
    match keys {
        [Key::Conv(id)] => {
            let title = sidebar
                .conversations
                .get(id)
                .and_then(activity::row_title)
                .unwrap_or("Untitled chat");
            format!("“{title}”")
        }
        [Key::Folder(id)] => format!(
            "“{}”",
            sidebar
                .folders
                .get(id)
                .map_or("folder", |f| f.name.as_str())
        ),
        _ => format!("{} items", keys.len()),
    }
}

impl Model {
    /// Shows `edit` at once, sends the commands `commands` makes of a
    /// reqId (and the barrier) and waits on them.
    fn mutate(
        &mut self,
        mutation: Mutation,
        edit: Option<(Vec<(Key, Expect)>, Answer)>,
        commands: impl FnOnce(&str) -> Vec<Value>,
    ) -> Vec<Value> {
        if !self.ready {
            self.list_failed(&format!(
                "{}: not connected to the daemon.",
                mutation.failure()
            ));
            return Vec::new();
        }
        let within = match mutation {
            Mutation::CreateFolder { .. }
            | Mutation::Rename {
                key: Key::Folder(_),
            } => requests::FOLDER_CREATE_WITHIN,
            _ => requests::DEFAULT_WITHIN,
        };
        let now = Instant::now();
        let id = self
            .requests
            .issue(Owner::Sidebar, mutation.kind(), None, within, now);
        let edit = edit.map(|(expected, answer)| self.sidebar.add_edit(expected, answer, now));
        let mut sent = commands(&id);
        if mutation.barrier() {
            sent.push(proto::barrier(&id));
        }
        self.sidebar.mutations.insert(id, Sent { mutation, edit });
        sent
    }

    pub(crate) fn list_failed(&mut self, message: &str) {
        self.ui.haptic(Haptic::Error);
        self.ui.error_toast(message);
    }

    fn snackbar(&mut self, text: &str, button: &str, act: Act) {
        self.ui.open(Box::new(Snackbar::new(
            text,
            button,
            super::super::model::Action::List(act),
            0.0,
        )));
    }

    /// The list again (the error state's Retry, pull to refresh, a change
    /// not made).
    pub(crate) fn refresh_list(&mut self) -> Vec<Value> {
        if self.requests.of(Owner::Sidebar, "list").next().is_some() {
            return Vec::new();
        }
        self.mutate(Mutation::Refresh, None, |id| {
            vec![proto::list_conversations(id)]
        })
    }

    /// Whether the list is being asked for again.
    pub fn list_refreshing(&self) -> bool {
        self.requests.of(Owner::Sidebar, "list").next().is_some()
    }

    /// Pins or unpins items (PAR-075): pinned ones go to the bottom of the
    /// pinned section, unpinned ones to the top of the unpinned section,
    /// keeping their order.
    pub(crate) fn list_pin(&mut self, keys: Vec<Key>, pinned: bool) -> Vec<Value> {
        let keys: Vec<Key> = keys
            .into_iter()
            .filter(|key| self.sidebar.pinned_of(key).is_some_and(|p| p != pinned))
            .collect();
        if keys.is_empty() {
            return Vec::new();
        }
        let expected = self.sidebar.pin_placements(&keys, pinned);
        let answer = Answer::Patch(keys.clone());
        let mutation = Mutation::Pin {
            keys: keys.clone(),
            pinned,
        };
        self.mutate(mutation, Some((expected, answer)), |id| match &keys[..] {
            [Key::Conv(conv)] => vec![proto::pin_conversation(id, conv, pinned)],
            [Key::Folder(folder)] => vec![proto::pin_folder(id, folder, pinned)],
            _ => vec![proto::pin_sidebar_items(
                id,
                keys.iter().map(|key| (key.item_ref(), pinned)).collect(),
            )],
        })
    }

    pub(crate) fn list_mark(&mut self, conv: String, marked: bool) -> Vec<Value> {
        if self
            .sidebar
            .conversations
            .get(&conv)
            .is_none_or(|c| c.marked == marked)
        {
            return Vec::new();
        }
        let expect = Expect {
            marked: Some(marked),
            ..Expect::default()
        };
        let edit = (
            vec![(Key::Conv(conv.clone()), expect)],
            Answer::Marked(conv.clone()),
        );
        let mutation = Mutation::Mark {
            conv: conv.clone(),
            marked,
        };
        self.mutate(mutation, Some(edit), |id| {
            vec![proto::mark_conversation(id, &conv, marked)]
        })
    }

    pub(crate) fn list_mute(&mut self, key: Key, muted: bool) -> Vec<Value> {
        let current = match &key {
            Key::Conv(id) => self.sidebar.conversations.get(id).map(|c| c.muted),
            Key::Folder(id) => self.sidebar.folders.get(id).map(|f| f.muted),
        };
        if current.is_none_or(|m| m == muted) {
            return Vec::new();
        }
        let expect = Expect {
            muted: Some(muted),
            ..Expect::default()
        };
        let answer = match key {
            Key::Folder(_) => Answer::Patch(vec![key.clone()]),
            Key::Conv(_) => Answer::None,
        };
        let edit = (vec![(key.clone(), expect)], answer);
        let mutation = Mutation::Mute {
            key: key.clone(),
            muted,
        };
        self.mutate(mutation, Some(edit), |id| match &key {
            Key::Conv(conv) => vec![proto::mute_conversation(id, conv, muted)],
            Key::Folder(folder) => vec![proto::mute_folder(id, folder, muted)],
        })
    }

    /// A conversation's title or a folder's name.
    pub(crate) fn list_rename(&mut self, key: Key, name: String) -> Vec<Value> {
        let name = name.trim().to_owned();
        let current = match &key {
            Key::Conv(id) => self.sidebar.conversations.get(id).map(|c| c.title.clone()),
            Key::Folder(id) => self.sidebar.folders.get(id).map(|f| f.name.clone()),
        };
        if name.is_empty() || current.is_none_or(|c| c == name) {
            return Vec::new();
        }
        let expect = Expect {
            title: Some(name.clone()),
            ..Expect::default()
        };
        let answer = match key {
            Key::Folder(_) => Answer::Patch(vec![key.clone()]),
            Key::Conv(_) => Answer::None,
        };
        let edit = (vec![(key.clone(), expect)], answer);
        self.mutate(
            Mutation::Rename { key: key.clone() },
            Some(edit),
            |id| match &key {
                Key::Conv(conv) => vec![proto::rename_conversation(id, conv, &name)],
                Key::Folder(folder) => vec![proto::rename_folder(id, folder, &name)],
            },
        )
    }

    /// Items into a folder (or the top level), at the top of its unpinned
    /// section (PAR-073).
    pub(crate) fn list_move(&mut self, keys: Vec<Key>, parent: Option<String>) -> Vec<Value> {
        let Some(expected) = self.sidebar.project_move(&keys, parent.as_deref()) else {
            return Vec::new();
        };
        // Nothing to do when every item already sits there.
        let changed = expected.iter().any(|(key, expect)| {
            self.sidebar.parent_of(key) != expect.parent
                || self.sidebar.pinned_of(key) != expect.pinned
        });
        if !changed {
            return Vec::new();
        }
        let moving: Vec<Key> = expected.iter().map(|(key, _)| key.clone()).collect();
        let label = label_for(&self.sidebar, &moving);
        if let Some(parent) = &parent {
            self.sidebar.expanded.insert(parent.clone());
            self.keep_list();
        }
        let answer = Answer::Patch(moving.clone());
        let mutation = Mutation::Move {
            keys: moving.clone(),
            label,
        };
        self.mutate(mutation, Some((expected, answer)), |id| {
            vec![proto::move_sidebar_items(
                id,
                moving.iter().map(Key::item_ref).collect(),
                parent.as_deref(),
            )]
        })
    }

    /// Conversations and folders to the trash (folders `recursive`ly, or
    /// only their shells).
    pub(crate) fn list_delete(&mut self, keys: Vec<Key>, recursive: bool) -> Vec<Value> {
        let convs: Vec<String> = keys
            .iter()
            .filter_map(|key| match key {
                Key::Conv(id) if self.sidebar.conversations.contains_key(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let folders: Vec<(String, String)> = keys
            .iter()
            .filter_map(|key| match key {
                Key::Folder(id) => self
                    .sidebar
                    .folders
                    .get(id)
                    .map(|f| (id.clone(), f.name.clone())),
                _ => None,
            })
            .collect();
        if convs.is_empty() && folders.is_empty() {
            return Vec::new();
        }
        let mut expected: Vec<(Key, Expect)> = Vec::new();
        let gone = || Expect {
            removed: true,
            ..Expect::default()
        };
        for (folder, _) in &folders {
            if recursive {
                let below = self.sidebar.descendant_folders(folder);
                for c in self.sidebar.conversations.values() {
                    if c.folder_id.as_ref().is_some_and(|f| below.contains(f)) {
                        expected.push((Key::Conv(c.id.clone()), gone()));
                    }
                }
                expected.extend(below.into_iter().map(|id| (Key::Folder(id), gone())));
            } else {
                expected.push((Key::Folder(folder.clone()), gone()));
            }
        }
        for conv in &convs {
            expected.push((Key::Conv(conv.clone()), gone()));
        }
        let mutation = Mutation::Delete {
            convs: convs.clone(),
            folders: folders.clone(),
            recursive,
        };
        self.mutate(mutation, Some((expected, Answer::None)), |id| {
            let mut commands: Vec<Value> = folders
                .iter()
                .map(|(folder, _)| proto::delete_folder(id, folder, recursive))
                .collect();
            match &convs[..] {
                [] => {}
                [conv] => commands.push(proto::delete_conversation(id, conv)),
                _ => commands.push(proto::delete_conversations(id, &convs)),
            }
            commands
        })
    }

    pub(crate) fn list_undo(&mut self, redo: bool) -> Vec<Value> {
        if redo {
            self.mutate(Mutation::Redo, None, |id| vec![proto::redo_delete(id)])
        } else {
            self.mutate(Mutation::Undo, None, |id| vec![proto::undo_delete(id)])
        }
    }

    pub(crate) fn list_clone(&mut self, conv: String) -> Vec<Value> {
        if !self.sidebar.conversations.contains_key(&conv) {
            return Vec::new();
        }
        self.mutate(Mutation::Clone, None, |id| {
            vec![proto::clone_conversation(id, &conv)]
        })
    }

    /// A folder (`items` moved into it).
    pub(crate) fn list_create_folder(
        &mut self,
        name: String,
        parent: Option<String>,
        items: Vec<Key>,
    ) -> Vec<Value> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Vec::new();
        }
        let before = self.sidebar.folders.keys().cloned().collect();
        let mutation = Mutation::CreateFolder {
            name: name.clone(),
            parent: parent.clone(),
            before,
            items: items.clone(),
        };
        self.mutate(mutation, None, |id| {
            vec![proto::create_folder(
                id,
                &name,
                parent.as_deref(),
                items.iter().map(Key::item_ref).collect(),
            )]
        })
    }

    /// Commands the rest of the app sent: a message bumps its unpinned
    /// conversation to the top of its folder (PAR-076); a new conversation
    /// shows at once at the top of its folder (PAR-082).
    pub fn list_observe(&mut self, commands: &[Value]) {
        let now = Instant::now();
        for command in commands {
            let conv = command["convId"].as_str().unwrap_or_default();
            match command["type"].as_str() {
                Some("send_message") => {
                    let key = Key::Conv(conv.to_owned());
                    let Some(old) = self
                        .sidebar
                        .conversations
                        .get(conv)
                        .filter(|c| !c.pinned)
                        .map(|c| c.sort_order)
                    else {
                        continue;
                    };
                    let top = self.sidebar.pinned_sort_order(&key, false);
                    if top < old {
                        let expect = Expect {
                            sort_order: Some(top),
                            ..Expect::default()
                        };
                        self.sidebar.add_edit(
                            vec![(key, expect)],
                            Answer::Moved(conv.to_owned(), old),
                            now,
                        );
                    }
                }
                Some("new_conversation") if !self.sidebar.conversations.contains_key(conv) => {
                    let folder = self.sidebar.parent_of_id(command["folderId"].as_str());
                    let entries = self.sidebar.placement_entries(folder.as_deref());
                    let started = command["initialMessage"]["startedAt"]
                        .as_f64()
                        .unwrap_or(now_ms());
                    let summary = Summary {
                        id: conv.to_owned(),
                        provider: command["provider"].as_str().unwrap_or_default().into(),
                        model: command["model"].as_str().unwrap_or_default().into(),
                        title: "pending".into(),
                        created_at: started,
                        updated_at: started,
                        message_count: 1,
                        streaming: true,
                        sort_order: top_unpinned(entries.iter().map(|(_, p, s)| (*p, *s))),
                        folder_id: folder,
                        ..Summary::default()
                    };
                    let expect = Expect {
                        created: Some(Box::new(summary)),
                        ..Expect::default()
                    };
                    let edit = self.sidebar.add_edit(
                        vec![(Key::Conv(conv.to_owned()), expect)],
                        Answer::None,
                        now,
                    );
                    let request = command["reqId"].as_str().unwrap_or_default().to_owned();
                    self.sidebar.creating.push((edit, request, None));
                }
                _ => {}
            }
        }
    }

    /// A new conversation's row goes when its request ended without the
    /// daemon listing it (it refused, or did not answer).
    pub(crate) fn settle_created(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let mut creating = std::mem::take(&mut self.sidebar.creating);
        creating.retain_mut(|(edit, request, orphaned)| {
            if self.sidebar.edit_settled(*edit) {
                return false;
            }
            if self.requests.is_pending(request) {
                return true;
            }
            match orphaned {
                None => {
                    *orphaned = Some(now);
                    true
                }
                Some(at) if now.duration_since(*at) < CREATED_GRACE => true,
                Some(_) => {
                    self.sidebar.drop_edit(*edit, now);
                    changed = true;
                    false
                }
            }
        });
        self.sidebar.creating = creating;
        changed
    }

    /// A list command seen through.
    fn list_done(&mut self, sent: Sent, restored: Option<String>) {
        match sent.mutation {
            Mutation::Delete {
                convs,
                folders,
                recursive,
            } => {
                if self
                    .sidebar
                    .selected
                    .as_ref()
                    .is_some_and(|s| convs.contains(s))
                {
                    self.sidebar.selected = None;
                }
                let keys: Vec<Key> = convs
                    .iter()
                    .map(|c| Key::Conv(c.clone()))
                    .chain(folders.iter().map(|(f, _)| Key::Folder(f.clone())))
                    .collect();
                self.sidebar.leave_selection(&keys);
                match (&convs[..], &folders[..]) {
                    (_, [(_, name)]) if convs.is_empty() && !recursive => {
                        self.ui.toast(format!("Removed folder “{name}”"));
                    }
                    (_, [(_, name)]) if convs.is_empty() => {
                        self.snackbar(&format!("Moved “{name}” to trash"), "Undo", Act::Undo);
                    }
                    ([_], []) => self.snackbar("Moved to trash", "Undo", Act::Undo),
                    _ if folders.is_empty() => self.snackbar(
                        &format!("Moved {} chats to trash", convs.len()),
                        "Undo",
                        Act::Undo,
                    ),
                    _ => self.snackbar(
                        &format!("Moved {} items to trash", keys.len()),
                        "Undo",
                        Act::Undo,
                    ),
                }
            }
            Mutation::Undo => {
                self.ui.haptic(Haptic::Light);
                if let Some(id) = restored {
                    self.reveal_conversation(id);
                }
                self.snackbar("Restored from trash", "Redo", Act::Redo);
            }
            Mutation::Redo => {
                self.ui.haptic(Haptic::Light);
                self.snackbar("Moved to trash again", "Undo", Act::Undo);
            }
            Mutation::Clone => {
                if let Some(id) = restored {
                    self.reveal_conversation(id);
                }
                self.ui.toast("Cloned");
            }
            Mutation::Move { keys, .. } => {
                if let Some(first) = keys.first() {
                    let row = match first {
                        Key::Conv(id) => Row::Conversation {
                            id: id.clone(),
                            depth: 0,
                        },
                        Key::Folder(id) => Row::Folder {
                            id: id.clone(),
                            depth: 0,
                            open: false,
                        },
                    };
                    self.sidebar.reveal_row(row);
                    self.keep_list();
                }
            }
            Mutation::CreateFolder {
                name,
                parent,
                before,
                items,
            } => {
                self.ui.haptic(Haptic::Light);
                self.sidebar.leave_selection(&items);
                // The state patch with the folder follows the ack.
                self.sidebar.created = Some((name, parent, before, !items.is_empty()));
            }
            _ => {}
        }
    }

    fn reveal_conversation(&mut self, id: String) {
        self.sidebar.reveal_row(Row::Conversation { id, depth: 0 });
        self.keep_list();
    }

    /// What the daemon said about the list's commands; returns commands
    /// (the list asked for again after a change that was not made).
    pub(crate) fn settle_list(&mut self, event: &Event, now: Instant) -> Vec<Value> {
        let mut commands = Vec::new();
        match event {
            Event::Pong {
                req_id: Some(id), ..
            }
            | Event::Ack {
                req_id: Some(id), ..
            } => {
                let ack = matches!(event, Event::Ack { .. });
                let ours = self.sidebar.mutations.get(id).is_some_and(|sent| {
                    if ack {
                        sent.mutation.acked()
                    } else {
                        sent.mutation.barrier()
                    }
                });
                if ours && let Some(sent) = self.sidebar.mutations.remove(id) {
                    // An acked command's broadcast follows its ack: its edit
                    // settles on its own.
                    let made = ack || sent.edit.is_none_or(|edit| self.sidebar.edit_settled(edit));
                    if made {
                        self.list_done(sent, None);
                    } else {
                        if let Some(edit) = sent.edit {
                            self.sidebar.drop_edit(edit, now);
                        }
                        self.list_failed(NOT_APPLIED);
                        commands.extend(self.refresh_list());
                    }
                }
            }
            Event::ConversationRestored {
                req_id: Some(id),
                summary,
            } => {
                if let Some(sent) = self.sidebar.mutations.remove(id) {
                    self.list_done(sent, Some(summary.id.clone()));
                }
            }
            Event::ConversationsList {
                req_id: Some(id), ..
            } => {
                if let Some(sent) = self.sidebar.mutations.remove(id)
                    && let Mutation::Move { label, .. } = &sent.mutation
                {
                    // Refused: the daemon's whole list instead.
                    if let Some(edit) = sent.edit {
                        self.sidebar.drop_edit(edit, now);
                    }
                    self.list_failed(&format!(
                        "Could not move {label}; the sidebar was restored."
                    ));
                }
            }
            // Folder undos answer with a state patch alone.
            Event::SidebarStatePatched { .. } => {
                let undo = self
                    .sidebar
                    .mutations
                    .iter()
                    .find(|(_, sent)| matches!(sent.mutation, Mutation::Undo | Mutation::Redo))
                    .map(|(id, _)| id.clone());
                if let Some(id) = undo
                    && let Some(sent) = self.sidebar.mutations.remove(&id)
                {
                    self.requests.forget(&id);
                    self.list_done(sent, None);
                }
            }
            _ => {}
        }
        // A folder made here shows once the daemon lists it.
        if let Some((name, parent, before, open)) = &self.sidebar.created {
            let made = self
                .sidebar
                .folders
                .values()
                .find(|f| !before.contains(&f.id) && &f.name == name && &f.parent_id == parent)
                .map(|f| f.id.clone());
            if let Some(id) = made {
                let (parent, open) = (parent.clone(), *open);
                self.sidebar.created = None;
                if let Some(parent) = parent {
                    self.sidebar.expanded.insert(parent);
                }
                if open {
                    self.sidebar.expanded.insert(id.clone());
                }
                self.sidebar.reveal_row(Row::Folder { id, depth: 0, open });
                self.keep_list();
            }
        }
        commands
    }

    /// An error answering one of the list's requests; true when handled.
    pub(crate) fn list_error(&mut self, id: &str, message: &str) -> bool {
        let Some(sent) = self.sidebar.mutations.remove(id) else {
            return self.instructions_failed(id, message);
        };
        if let Some(edit) = sent.edit {
            self.sidebar.drop_edit(edit, Instant::now());
        }
        if sent.mutation == Mutation::Refresh && !self.sidebar.loaded {
            self.sidebar.error = Some(message.to_owned());
            self.sidebar.changed();
        }
        self.list_failed(&format!("{}: {message}", sent.mutation.failure()));
        true
    }

    /// One of the list's requests was not answered in time; true when
    /// handled.
    pub(crate) fn list_timeout(&mut self, id: &str, _pending: &Pending) -> bool {
        let Some(sent) = self.sidebar.mutations.remove(id) else {
            return self.instructions_failed(id, "Exocortex did not answer. Try again.");
        };
        if let Some(edit) = sent.edit {
            self.sidebar.drop_edit(edit, Instant::now());
        }
        if sent.mutation == Mutation::Refresh {
            if !self.sidebar.loaded {
                self.sidebar.error = Some("Exocortex did not send the list.".into());
                self.sidebar.changed();
            }
            self.list_failed("Exocortex did not send the list. Try again.");
            return true;
        }
        self.list_failed(&format!("{}. {NO_ACK}", sent.mutation.failure()));
        true
    }
}

impl Sidebar {
    /// Where pinning or unpinning `keys` puts each (in their list order):
    /// pinned after the pinned section's last, unpinned before the unpinned
    /// section's first, as a block.
    pub fn pin_placements(&self, keys: &[Key], pinned: bool) -> Vec<(Key, Expect)> {
        let mut out = Vec::new();
        let mut parents: Vec<Option<String>> = Vec::new();
        for key in keys {
            let parent = self.parent_of(key).flatten();
            if !parents.contains(&parent) {
                parents.push(parent);
            }
        }
        for parent in parents {
            let entries = self.placement_entries(parent.as_deref());
            let mine: Vec<&Key> = entries
                .iter()
                .map(|(key, _, _)| key)
                .filter(|key| keys.contains(key))
                .collect();
            let others = entries.iter().filter(|(key, _, _)| !keys.contains(key));
            let orders: Vec<f64> = if pinned {
                let max = others
                    .filter(|(_, p, _)| *p)
                    .map(|(_, _, s)| *s)
                    .fold(f64::NEG_INFINITY, f64::max);
                let first = if max == f64::NEG_INFINITY {
                    0.0
                } else {
                    max + 1.0
                };
                (0..mine.len()).map(|i| first + i as f64).collect()
            } else {
                let top = top_unpinned(others.map(|(_, p, s)| (*p, *s)));
                let n = mine.len() as f64;
                (0..mine.len()).map(|i| top - n + 1.0 + i as f64).collect()
            };
            for (key, order) in mine.into_iter().zip(orders) {
                let expect = Expect {
                    pinned: Some(pinned),
                    sort_order: Some(order),
                    ..Expect::default()
                };
                out.push((key.clone(), expect));
            }
        }
        out
    }
}

fn now_ms() -> f64 {
    super::super::model::now_ms() as f64
}
