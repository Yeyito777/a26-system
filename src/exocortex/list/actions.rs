//! The list's commands (PAR-064–079): pin, mark, mute, rename and delete a
//! conversation (with the undo snackbar), undo and redo deletions, create,
//! rename, pin, mute and delete folders, and the menus and prompts that
//! lead to them.
//!
//! Most list commands answer nothing and fail silently, so each is followed
//! on the same connection by a `ping` with its reqId: the daemon handles a
//! client's commands in order, so by the `pong` every broadcast the command
//! caused has arrived, and a change the list still does not show was not
//! made (PAR-074): the user is told and the list asked for again. Commands
//! that ack (renames, a new folder) are through at their ack; undo and redo
//! at the restored conversation or the state patch they cause.

use std::collections::HashSet;
use std::time::Instant;

use serde_json::Value;

use super::super::model::sidebar::{Act, Row, Sidebar};
use super::super::model::{Action, Model, Nav, UiAct};
use super::super::proto::{self, Event};
use super::super::requests::{self, Owner, Pending};
use super::super::ui::Haptic;
use super::super::ui::icons::Icon;
use super::super::ui::menu::{AnchoredMenu, Confirm, MenuItem, Tone};
use super::super::ui::sheet::{self, ActionItem, ActionList, Prompt};
use super::super::ui::toast::Snackbar;
use super::activity;
use crate::gfx::RectF;

/// The daemon did not make a change it was asked for.
pub const NOT_APPLIED: &str =
    "The daemon did not apply this sidebar change. The sidebar has been refreshed.";
/// A change was not answered in time.
pub const NO_ACK: &str = "No acknowledgement received. Check the conversation before retrying; the command may have been accepted.";

/// A list command sent and not yet seen through.
#[derive(Debug, Clone, PartialEq)]
pub enum Mutation {
    Pin {
        conv: String,
        pinned: bool,
    },
    Mark {
        conv: String,
        marked: bool,
    },
    Mute {
        conv: String,
        muted: bool,
    },
    Rename {
        conv: String,
    },
    Delete {
        conv: String,
    },
    Undo,
    Redo,
    CreateFolder {
        name: String,
        parent: Option<String>,
        /// The folders there were before.
        before: HashSet<String>,
    },
    RenameFolder {
        folder: String,
    },
    PinFolder {
        folder: String,
        pinned: bool,
    },
    MuteFolder {
        folder: String,
        muted: bool,
    },
    DeleteFolder {
        folder: String,
        name: String,
        recursive: bool,
    },
    /// The list asked for again.
    Refresh,
}

impl Mutation {
    fn kind(&self) -> &'static str {
        match self {
            Mutation::Pin { .. } | Mutation::PinFolder { .. } => "pin",
            Mutation::Mark { .. } => "mark",
            Mutation::Mute { .. } | Mutation::MuteFolder { .. } => "mute",
            Mutation::Rename { .. } | Mutation::RenameFolder { .. } => "rename",
            Mutation::Delete { .. } | Mutation::DeleteFolder { .. } => "delete",
            Mutation::Undo => "undo",
            Mutation::Redo => "redo",
            Mutation::CreateFolder { .. } => "folder",
            Mutation::Refresh => "list",
        }
    }

    /// What failing at it is called.
    fn failure(&self) -> &'static str {
        match self {
            Mutation::Pin { pinned: true, .. } | Mutation::PinFolder { pinned: true, .. } => {
                "Could not pin"
            }
            Mutation::Pin { .. } | Mutation::PinFolder { .. } => "Could not unpin",
            Mutation::Mark { marked: true, .. } => "Could not mark",
            Mutation::Mark { .. } => "Could not unmark",
            Mutation::Mute { muted: true, .. } | Mutation::MuteFolder { muted: true, .. } => {
                "Could not mute"
            }
            Mutation::Mute { .. } | Mutation::MuteFolder { .. } => "Could not unmute",
            Mutation::Rename { .. } => "Could not rename",
            Mutation::RenameFolder { .. } => "Could not rename the folder",
            Mutation::Delete { .. } => "Could not delete",
            Mutation::DeleteFolder { .. } => "Could not delete the folder",
            Mutation::Undo => "Could not undo",
            Mutation::Redo => "Could not redo",
            Mutation::CreateFolder { .. } => "Could not create the folder",
            Mutation::Refresh => "Could not load the chats",
        }
    }

    /// Through at its ack (else at the barrier's pong, or, for undo, redo
    /// and the list, at what they answer with).
    fn acked(&self) -> bool {
        matches!(
            self,
            Mutation::Rename { .. } | Mutation::RenameFolder { .. } | Mutation::CreateFolder { .. }
        )
    }

    fn barrier(&self) -> bool {
        !self.acked() && !matches!(self, Mutation::Undo | Mutation::Redo | Mutation::Refresh)
    }

    /// Whether the list shows it done.
    fn applied(&self, sidebar: &Sidebar) -> bool {
        let conv = |id: &str| sidebar.conversations.get(id);
        let folder = |id: &str| sidebar.folders.get(id);
        match self {
            Mutation::Pin { conv: id, pinned } => conv(id).is_some_and(|c| c.pinned == *pinned),
            Mutation::Mark { conv: id, marked } => conv(id).is_some_and(|c| c.marked == *marked),
            Mutation::Mute { conv: id, muted } => conv(id).is_some_and(|c| c.muted == *muted),
            Mutation::Delete { conv: id } => conv(id).is_none(),
            Mutation::PinFolder { folder: id, pinned } => {
                folder(id).is_some_and(|f| f.pinned == *pinned)
            }
            Mutation::MuteFolder { folder: id, muted } => {
                folder(id).is_some_and(|f| f.muted == *muted)
            }
            Mutation::DeleteFolder { folder: id, .. } => folder(id).is_none(),
            _ => true,
        }
    }
}

/// The conversation's title for messages: “Title”.
fn quoted(sidebar: &Sidebar, conv: &str) -> String {
    let title = sidebar
        .conversations
        .get(conv)
        .and_then(activity::row_title)
        .unwrap_or("Untitled chat");
    format!("“{title}”")
}

fn list(act: Act) -> Action {
    Action::List(act)
}

impl Model {
    /// Sends a list command (and its barrier) and waits on it.
    fn mutate(&mut self, mutation: Mutation, command: impl FnOnce(&str) -> Value) -> Vec<Value> {
        if !self.ready {
            self.list_failed(&format!(
                "{}: not connected to the daemon.",
                mutation.failure()
            ));
            return Vec::new();
        }
        let within = match mutation {
            Mutation::CreateFolder { .. } | Mutation::RenameFolder { .. } => {
                requests::FOLDER_CREATE_WITHIN
            }
            _ => requests::DEFAULT_WITHIN,
        };
        let id = self.requests.issue(
            Owner::Sidebar,
            mutation.kind(),
            None,
            within,
            Instant::now(),
        );
        let mut commands = vec![command(&id)];
        if mutation.barrier() {
            commands.push(proto::barrier(&id));
        }
        self.sidebar.mutations.insert(id, mutation);
        commands
    }

    fn list_failed(&mut self, message: &str) {
        self.ui.haptic(Haptic::Error);
        self.ui.error_toast(message);
    }

    fn snackbar(&mut self, text: &str, button: &str, act: Act) {
        self.ui
            .open(Box::new(Snackbar::new(text, button, list(act), 0.0)));
    }

    /// The list again (the error state's Retry, a change not made).
    pub(crate) fn refresh_list(&mut self) -> Vec<Value> {
        if self.requests.of(Owner::Sidebar, "list").next().is_some() {
            return Vec::new();
        }
        self.mutate(Mutation::Refresh, proto::list_conversations)
    }

    pub(crate) fn list_pin(&mut self, conv: String, pinned: bool) -> Vec<Value> {
        if self
            .sidebar
            .conversations
            .get(&conv)
            .is_none_or(|c| c.pinned == pinned)
        {
            return Vec::new();
        }
        self.mutate(
            Mutation::Pin {
                conv: conv.clone(),
                pinned,
            },
            |id| proto::pin_conversation(id, &conv, pinned),
        )
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
        self.mutate(
            Mutation::Mark {
                conv: conv.clone(),
                marked,
            },
            |id| proto::mark_conversation(id, &conv, marked),
        )
    }

    pub(crate) fn list_mute(&mut self, conv: String, muted: bool) -> Vec<Value> {
        if self
            .sidebar
            .conversations
            .get(&conv)
            .is_none_or(|c| c.muted == muted)
        {
            return Vec::new();
        }
        self.mutate(
            Mutation::Mute {
                conv: conv.clone(),
                muted,
            },
            |id| proto::mute_conversation(id, &conv, muted),
        )
    }

    pub(crate) fn list_set_title(&mut self, conv: String, title: String) -> Vec<Value> {
        let title = title.trim().to_owned();
        if title.is_empty()
            || self
                .sidebar
                .conversations
                .get(&conv)
                .is_none_or(|c| c.title == title)
        {
            return Vec::new();
        }
        self.mutate(Mutation::Rename { conv: conv.clone() }, |id| {
            proto::rename_conversation(id, &conv, &title)
        })
    }

    pub(crate) fn list_delete(&mut self, conv: String) -> Vec<Value> {
        if !self.sidebar.conversations.contains_key(&conv) {
            return Vec::new();
        }
        self.mutate(Mutation::Delete { conv: conv.clone() }, |id| {
            proto::delete_conversation(id, &conv)
        })
    }

    pub(crate) fn list_undo(&mut self, redo: bool) -> Vec<Value> {
        if redo {
            self.mutate(Mutation::Redo, proto::redo_delete)
        } else {
            self.mutate(Mutation::Undo, proto::undo_delete)
        }
    }

    pub(crate) fn list_create_folder(
        &mut self,
        name: String,
        parent: Option<String>,
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
        };
        self.mutate(mutation, |id| {
            proto::create_folder(id, &name, parent.as_deref())
        })
    }

    pub(crate) fn list_set_folder_name(&mut self, folder: String, name: String) -> Vec<Value> {
        let name = name.trim().to_owned();
        if name.is_empty()
            || self
                .sidebar
                .folders
                .get(&folder)
                .is_none_or(|f| f.name == name)
        {
            return Vec::new();
        }
        self.mutate(
            Mutation::RenameFolder {
                folder: folder.clone(),
            },
            |id| proto::rename_folder(id, &folder, &name),
        )
    }

    pub(crate) fn list_pin_folder(&mut self, folder: String, pinned: bool) -> Vec<Value> {
        if self
            .sidebar
            .folders
            .get(&folder)
            .is_none_or(|f| f.pinned == pinned)
        {
            return Vec::new();
        }
        self.mutate(
            Mutation::PinFolder {
                folder: folder.clone(),
                pinned,
            },
            |id| proto::pin_folder(id, &folder, pinned),
        )
    }

    pub(crate) fn list_mute_folder(&mut self, folder: String, muted: bool) -> Vec<Value> {
        if self
            .sidebar
            .folders
            .get(&folder)
            .is_none_or(|f| f.muted == muted)
        {
            return Vec::new();
        }
        self.mutate(
            Mutation::MuteFolder {
                folder: folder.clone(),
                muted,
            },
            |id| proto::mute_folder(id, &folder, muted),
        )
    }

    pub(crate) fn list_delete_folder(&mut self, folder: String, recursive: bool) -> Vec<Value> {
        let Some(name) = self.sidebar.folders.get(&folder).map(|f| f.name.clone()) else {
            return Vec::new();
        };
        let mutation = Mutation::DeleteFolder {
            folder: folder.clone(),
            name,
            recursive,
        };
        self.mutate(mutation, |id| proto::delete_folder(id, &folder, recursive))
    }

    /// A list command seen through.
    fn list_done(&mut self, mutation: Mutation, restored: Option<String>) {
        match mutation {
            Mutation::Delete { conv } => {
                if self.sidebar.selected.as_deref() == Some(conv.as_str()) {
                    self.sidebar.selected = None;
                }
                self.snackbar("Moved to trash", "Undo", Act::Undo);
            }
            Mutation::Undo => {
                self.ui.haptic(Haptic::Light);
                if let Some(id) = restored {
                    self.sidebar.reveal_row(Row::Conversation { id, depth: 0 });
                    self.keep_list();
                }
                self.snackbar("Restored from trash", "Redo", Act::Redo);
            }
            Mutation::Redo => {
                self.ui.haptic(Haptic::Light);
                self.snackbar("Moved to trash again", "Undo", Act::Undo);
            }
            Mutation::CreateFolder {
                name,
                parent,
                before,
            } => {
                self.ui.haptic(Haptic::Light);
                // The state patch with the folder follows the ack.
                self.sidebar.created = Some((name, parent, before));
            }
            Mutation::DeleteFolder {
                name, recursive, ..
            } => {
                if recursive {
                    self.snackbar(&format!("Moved “{name}” to trash"), "Undo", Act::Undo);
                } else {
                    self.ui.toast(format!("Removed folder “{name}”"));
                }
            }
            _ => {}
        }
    }

    /// What the daemon said about the list's commands; returns commands
    /// (the list asked for again after a change that was not made).
    pub(crate) fn settle_list(&mut self, event: &Event, _now: Instant) -> Vec<Value> {
        let mut commands = Vec::new();
        match event {
            Event::Pong {
                req_id: Some(id), ..
            }
            | Event::Ack {
                req_id: Some(id), ..
            } => {
                let ack = matches!(event, Event::Ack { .. });
                let ours = self
                    .sidebar
                    .mutations
                    .get(id)
                    .is_some_and(|m| if ack { m.acked() } else { m.barrier() });
                if ours && let Some(mutation) = self.sidebar.mutations.remove(id) {
                    if mutation.applied(&self.sidebar) {
                        self.list_done(mutation, None);
                    } else {
                        self.list_failed(NOT_APPLIED);
                        commands.extend(self.refresh_list());
                    }
                }
            }
            Event::ConversationRestored {
                req_id: Some(id),
                summary,
            } => {
                if let Some(mutation) = self.sidebar.mutations.remove(id) {
                    self.list_done(mutation, Some(summary.id.clone()));
                }
            }
            Event::ConversationsList {
                req_id: Some(id), ..
            } => {
                self.sidebar.mutations.remove(id);
            }
            // Folder undos answer with a state patch alone.
            Event::SidebarStatePatched { .. } => {
                let undo = self
                    .sidebar
                    .mutations
                    .iter()
                    .find(|(_, m)| matches!(m, Mutation::Undo | Mutation::Redo))
                    .map(|(id, _)| id.clone());
                if let Some(id) = undo
                    && let Some(mutation) = self.sidebar.mutations.remove(&id)
                {
                    self.requests.forget(&id);
                    self.list_done(mutation, None);
                }
            }
            _ => {}
        }
        // A folder made here shows once the daemon lists it.
        if let Some((name, parent, before)) = &self.sidebar.created {
            let made = self
                .sidebar
                .folders
                .values()
                .find(|f| !before.contains(&f.id) && &f.name == name && &f.parent_id == parent)
                .map(|f| f.id.clone());
            if let Some(id) = made {
                let parent = parent.clone();
                self.sidebar.created = None;
                if let Some(parent) = parent {
                    self.sidebar.expanded.insert(parent);
                }
                self.sidebar.reveal_row(Row::Folder {
                    id,
                    depth: 0,
                    open: false,
                });
                self.keep_list();
            }
        }
        commands
    }

    /// An error answering one of the list's requests; true when handled.
    pub(crate) fn list_error(&mut self, id: &str, message: &str) -> bool {
        let Some(mutation) = self.sidebar.mutations.remove(id) else {
            return false;
        };
        if mutation == Mutation::Refresh && !self.sidebar.loaded {
            self.sidebar.error = Some(message.to_owned());
            self.sidebar.changed();
        }
        self.list_failed(&format!("{}: {message}", mutation.failure()));
        true
    }

    /// One of the list's requests was not answered in time; true when
    /// handled.
    pub(crate) fn list_timeout(&mut self, id: &str, _pending: &Pending) -> bool {
        let Some(mutation) = self.sidebar.mutations.remove(id) else {
            return false;
        };
        if mutation == Mutation::Refresh {
            if !self.sidebar.loaded {
                self.sidebar.error = Some("Exocortex did not send the list.".into());
                self.sidebar.changed();
            }
            self.list_failed("Exocortex did not send the list. Try again.");
            return true;
        }
        self.list_failed(&format!("{}. {NO_ACK}", mutation.failure()));
        true
    }

    // ----- Menus and prompts ----------------------------------------------------

    /// The header's ⋯ (`real-03-sidebar-menu`).
    pub(crate) fn list_menu(&mut self, anchor: RectF) {
        let items = vec![
            MenuItem::new(
                Icon::DownloadCircle,
                "Next unread",
                Tone::Success,
                list(Act::NextUnread),
            ),
            MenuItem::new(
                Icon::Undo,
                "Undo last deletion",
                Tone::Accent,
                list(Act::Undo),
            ),
            MenuItem::new(Icon::Redo, "Redo deletion", Tone::Accent, list(Act::Redo)),
            MenuItem::new(
                Icon::Gear,
                "Settings",
                Tone::Muted,
                Action::Nav(Nav::OpenSettings),
            ),
        ];
        self.ui
            .open(Box::new(AnchoredMenu::new(anchor, items, false)));
    }

    /// A conversation row's long press (PAR-067, 068).
    pub(crate) fn conversation_menu(&mut self, id: String, rect: RectF) {
        let Some(c) = self.sidebar.conversations.get(&id) else {
            return;
        };
        let (pinned, marked, muted) = (c.pinned, c.marked, c.muted);
        let items = vec![
            MenuItem::new(
                if pinned { Icon::PinOff } else { Icon::Pin },
                if pinned { "Unpin" } else { "Pin" },
                Tone::Accent,
                list(Act::Pin(id.clone(), !pinned)),
            ),
            MenuItem::new(
                if marked {
                    Icon::StarOff
                } else {
                    Icon::StarLine
                },
                if marked { "Unmark" } else { "Mark" },
                Tone::Warning,
                list(Act::Mark(id.clone(), !marked)),
            ),
            MenuItem::new(
                if muted { Icon::Bell } else { Icon::BellOff },
                if muted { "Unmute" } else { "Mute" },
                Tone::Plain,
                list(Act::Mute(id.clone(), !muted)),
            ),
            MenuItem::new(
                Icon::Pencil,
                "Rename…",
                Tone::Accent,
                list(Act::Rename(id.clone())),
            ),
            MenuItem::new(
                Icon::Hash,
                "Copy id",
                Tone::Muted,
                Action::Ui(UiAct::Copy(id.clone())),
            ),
            MenuItem::new(
                Icon::Trash,
                "Delete",
                Tone::Destructive,
                list(Act::ConfirmDelete(id)),
            ),
        ];
        self.ui.open(Box::new(AnchoredMenu::new(rect, items, true)));
    }

    /// A folder row's long press (PAR-078's quick actions).
    pub(crate) fn folder_menu(&mut self, id: String, rect: RectF) {
        let Some(f) = self.sidebar.folders.get(&id) else {
            return;
        };
        let (pinned, muted) = (f.pinned, f.muted);
        let items = vec![
            MenuItem::new(
                if pinned { Icon::PinOff } else { Icon::Pin },
                if pinned { "Unpin" } else { "Pin" },
                Tone::Accent,
                list(Act::PinFolder(id.clone(), !pinned)),
            ),
            MenuItem::new(
                if muted { Icon::Bell } else { Icon::BellOff },
                if muted { "Unmute" } else { "Mute" },
                Tone::Plain,
                list(Act::MuteFolder(id.clone(), !muted)),
            ),
            MenuItem::new(
                Icon::Plus,
                "New chat",
                Tone::Accent,
                list(Act::NewChatIn(id.clone())),
            ),
            MenuItem::new(
                Icon::Pencil,
                "Rename…",
                Tone::Accent,
                list(Act::RenameFolder(id.clone())),
            ),
            MenuItem::new(
                Icon::FolderPlus,
                "New subfolder…",
                Tone::Accent,
                list(Act::NewFolder(Some(id.clone()))),
            ),
            MenuItem::new(
                Icon::Trash,
                "Delete…",
                Tone::Destructive,
                list(Act::ConfirmDeleteFolder(id)),
            ),
        ];
        self.ui.open(Box::new(AnchoredMenu::new(rect, items, true)));
    }

    pub(crate) fn rename_prompt(&mut self, id: String) {
        let Some(c) = self.sidebar.conversations.get(&id) else {
            return;
        };
        let title = activity::first_line(&c.title).to_owned();
        let prompt = Prompt::new(
            "Rename conversation",
            "",
            title,
            "Rename",
            Box::new(move |title| list(Act::SetTitle(id.clone(), title))),
        );
        self.ui.open(Box::new(sheet::prompt_sheet(prompt)));
    }

    pub(crate) fn new_folder_prompt(&mut self, parent: Option<String>) {
        // One line (the prompt's message does not wrap).
        let message = match &parent {
            Some(id) => format!("Inside “{}”.", self.sidebar.folder_path(Some(id))),
            None => "At the top level of the list.".into(),
        };
        let title = if parent.is_some() {
            "New subfolder"
        } else {
            "New folder"
        };
        let prompt = Prompt::new(
            title,
            message,
            "",
            "Create",
            Box::new(move |name| {
                list(Act::CreateFolder {
                    name,
                    parent: parent.clone(),
                })
            }),
        );
        self.ui.open(Box::new(sheet::prompt_sheet(prompt)));
    }

    pub(crate) fn rename_folder_prompt(&mut self, id: String) {
        let Some(f) = self.sidebar.folders.get(&id) else {
            return;
        };
        let prompt = Prompt::new(
            "Rename folder",
            "",
            f.name.clone(),
            "Rename",
            Box::new(move |name| list(Act::SetFolderName(id.clone(), name))),
        );
        self.ui.open(Box::new(sheet::prompt_sheet(prompt)));
    }

    pub(crate) fn confirm_delete(&mut self, id: String) {
        if !self.sidebar.conversations.contains_key(&id) {
            return;
        }
        let title = format!("Delete {}?", quoted(&self.sidebar, &id));
        self.ui.open(Box::new(Confirm::new(
            title,
            "The conversation will be moved to Exocortex’s undoable trash.",
            "Delete",
            list(Act::Delete(id)),
        )));
    }

    pub(crate) fn confirm_delete_folder(&mut self, id: String) {
        let Some(f) = self.sidebar.folders.get(&id) else {
            return;
        };
        let title = format!("Delete “{}”", f.name);
        let count = self
            .sidebar
            .aggregates(&self.composer.queued)
            .folders
            .get(&id)
            .map_or(0, |a| a.count);
        let message = match count {
            0 => "The folder is empty.".to_owned(),
            1 => "It contains 1 chat.".to_owned(),
            n => format!("It contains {} chats.", activity::thousands(n as u64)),
        };
        let items = vec![
            ActionItem::new(
                "Remove folder shell; keep children",
                list(Act::DeleteFolder {
                    id: id.clone(),
                    recursive: false,
                }),
            )
            .icon(Icon::Tray)
            .detail("Its chats and subfolders move up one level"),
            ActionItem::new(
                "Move folder and children to trash",
                list(Act::DeleteFolder {
                    id,
                    recursive: true,
                }),
            )
            .icon(Icon::Trash)
            .detail("Undoable from the trash")
            .tone(Tone::Destructive),
        ];
        self.ui.open(Box::new(sheet::action_sheet(
            ActionList::new(title, items).subtitle(message),
        )));
    }
}
