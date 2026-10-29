//! The list's menus, prompts and pickers: the header's ⋯ (PAR-064), a
//! row's long press (PAR-067, 068, the folder's of PAR-078), the rename and
//! new-folder prompts, the delete confirmations, and the move picker.

use std::collections::HashSet;

use super::super::model::sidebar::Act;
use super::super::model::{Action, Model, Nav, UiAct};
use super::super::ui::icons::Icon;
use super::super::ui::menu::{AnchoredMenu, Confirm, MenuItem, Tone};
use super::super::ui::sheet::{self, ActionItem, ActionList, Prompt};
use super::actions::label_for;
use super::activity;
use super::pending::Key;
use crate::gfx::RectF;

fn list(act: Act) -> Action {
    Action::List(act)
}

impl Model {
    /// The header's ⋯ (`real-03-sidebar-menu`).
    pub(crate) fn list_menu(&mut self, anchor: RectF) {
        let items = vec![
            MenuItem::new(
                Icon::ListChecks,
                "Select chats",
                Tone::Accent,
                list(Act::Select(None)),
            ),
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

    /// A conversation row's long press (PAR-067, 068): pin, mark and mute
    /// side by side, then the rest.
    pub(crate) fn conversation_menu(&mut self, id: String, rect: RectF) {
        let Some(c) = self.sidebar.conversations.get(&id) else {
            return;
        };
        let (pinned, marked, muted) = (c.pinned, c.marked, c.muted);
        let key = Key::Conv(id.clone());
        let quick = vec![
            MenuItem::new(
                if pinned { Icon::PinOff } else { Icon::Pin },
                if pinned { "Unpin" } else { "Pin" },
                Tone::Accent,
                list(Act::Pin(vec![key.clone()], !pinned)),
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
                list(Act::Mute(key.clone(), !muted)),
            ),
        ];
        let items = vec![
            MenuItem::new(
                Icon::Pencil,
                "Rename…",
                Tone::Accent,
                list(Act::Rename(key.clone())),
            ),
            MenuItem::new(
                Icon::FolderMove,
                "Move to folder…",
                Tone::Accent,
                list(Act::MovePicker(vec![key.clone()])),
            ),
            MenuItem::new(
                Icon::Clone,
                "Clone",
                Tone::Accent,
                list(Act::Clone(id.clone())),
            ),
            MenuItem::new(
                Icon::ListChecks,
                "Select",
                Tone::Accent,
                list(Act::Select(Some(key.clone()))),
            ),
            MenuItem::new(
                Icon::Hash,
                "Copy id",
                Tone::Muted,
                Action::Ui(UiAct::Copy(id)),
            ),
            MenuItem::new(
                Icon::Trash,
                "Delete",
                Tone::Destructive,
                list(Act::ConfirmDelete(vec![key])),
            ),
        ];
        self.ui
            .open(Box::new(AnchoredMenu::new(rect, items, true).quick(quick)));
    }

    /// A folder row's long press (PAR-078's quick actions).
    pub(crate) fn folder_menu(&mut self, id: String, rect: RectF) {
        let Some(f) = self.sidebar.folders.get(&id) else {
            return;
        };
        let (pinned, muted) = (f.pinned, f.muted);
        let key = Key::Folder(id.clone());
        let quick = vec![
            MenuItem::new(
                if pinned { Icon::PinOff } else { Icon::Pin },
                if pinned { "Unpin" } else { "Pin" },
                Tone::Accent,
                list(Act::Pin(vec![key.clone()], !pinned)),
            ),
            MenuItem::new(
                if muted { Icon::Bell } else { Icon::BellOff },
                if muted { "Unmute" } else { "Mute" },
                Tone::Plain,
                list(Act::Mute(key.clone(), !muted)),
            ),
            MenuItem::new(
                Icon::Plus,
                "New chat",
                Tone::Accent,
                list(Act::NewChatIn(id.clone())),
            ),
        ];
        let items = vec![
            MenuItem::new(
                Icon::Pencil,
                "Rename…",
                Tone::Accent,
                list(Act::Rename(key.clone())),
            ),
            MenuItem::new(
                Icon::FolderPlus,
                "New subfolder…",
                Tone::Accent,
                list(Act::NewFolder {
                    parent: Some(id.clone()),
                    items: Vec::new(),
                }),
            ),
            MenuItem::new(
                Icon::FolderMove,
                "Move to folder…",
                Tone::Accent,
                list(Act::MovePicker(vec![key.clone()])),
            ),
            MenuItem::new(
                Icon::FileText,
                "Instructions & info",
                Tone::Accent,
                list(Act::FolderInfo(id.clone())),
            ),
            MenuItem::new(
                Icon::Trash,
                "Delete…",
                Tone::Destructive,
                list(Act::ConfirmDelete(vec![key])),
            ),
        ];
        self.ui
            .open(Box::new(AnchoredMenu::new(rect, items, true).quick(quick)));
    }

    /// Rename…: a conversation's first title line, or a folder's name.
    pub(crate) fn rename_prompt(&mut self, key: Key) {
        let (title, name) = match &key {
            Key::Conv(id) => match self.sidebar.conversations.get(id) {
                Some(c) => (
                    "Rename conversation",
                    activity::first_line(&c.title).to_owned(),
                ),
                None => return,
            },
            Key::Folder(id) => match self.sidebar.folders.get(id) {
                Some(f) => ("Rename folder", f.name.clone()),
                None => return,
            },
        };
        let prompt = Prompt::new(
            title,
            "",
            name,
            "Rename",
            Box::new(move |name| list(Act::SetName(key.clone(), name))),
        );
        self.ui.open(Box::new(sheet::prompt_sheet(prompt)));
    }

    /// New folder… (in `parent`; `items` move into it: the select bar's
    /// Group).
    pub(crate) fn new_folder_prompt(&mut self, parent: Option<String>, items: Vec<Key>) {
        // One line (the prompt's message does not wrap).
        let message = match (&parent, items.len()) {
            (_, 1) => "The selected item moves into it.".to_owned(),
            (_, n) if n > 1 => format!("The {n} selected items move into it."),
            (Some(id), _) => format!("Inside “{}”.", self.sidebar.folder_path(Some(id))),
            (None, _) => "At the top level of the list.".into(),
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
                    items: items.clone(),
                })
            }),
        );
        self.ui.open(Box::new(sheet::prompt_sheet(prompt)));
    }

    /// Delete: one conversation, several, or folders (whose children go
    /// too, or move up a level).
    pub(crate) fn confirm_delete(&mut self, keys: Vec<Key>) {
        let keys: Vec<Key> = keys
            .into_iter()
            .filter(|key| match key {
                Key::Conv(id) => self.sidebar.conversations.contains_key(id),
                Key::Folder(id) => self.sidebar.folders.contains_key(id),
            })
            .collect();
        let convs = keys.iter().filter(|k| matches!(k, Key::Conv(_))).count();
        let folders = keys.len() - convs;
        match (convs, folders) {
            (0, 0) => {}
            (1, 0) => {
                let title = format!("Delete {}?", label_for(&self.sidebar, &keys));
                self.ui.open(Box::new(Confirm::new(
                    title,
                    "The conversation will be moved to Exocortex’s undoable trash.",
                    "Delete",
                    list(Act::Delete {
                        keys,
                        recursive: true,
                    }),
                )));
            }
            (n, 0) => {
                self.ui.open(Box::new(Confirm::new(
                    format!("Move {n} conversations to undoable trash?"),
                    "Undo restores them.",
                    "Move to trash",
                    list(Act::Delete {
                        keys,
                        recursive: true,
                    }),
                )));
            }
            (0, 1) => {
                let Key::Folder(id) = &keys[0] else {
                    return;
                };
                let name = self.sidebar.folders[id].name.clone();
                let count = self
                    .sidebar
                    .aggregates(&self.composer.queued)
                    .folders
                    .get(id)
                    .map_or(0, |a| a.count);
                let message = match count {
                    0 => "The folder is empty.".to_owned(),
                    1 => "It contains 1 chat.".to_owned(),
                    n => format!("It contains {} chats.", activity::thousands(n as u64)),
                };
                let items = vec![
                    ActionItem::new(
                        "Remove folder shell; keep children",
                        list(Act::Delete {
                            keys: keys.clone(),
                            recursive: false,
                        }),
                    )
                    .icon(Icon::Tray)
                    .detail("Its chats and subfolders move up one level"),
                    ActionItem::new(
                        "Move folder and children to trash",
                        list(Act::Delete {
                            keys,
                            recursive: true,
                        }),
                    )
                    .icon(Icon::Trash)
                    .detail("Undoable from the trash")
                    .tone(Tone::Destructive),
                ];
                self.ui.open(Box::new(sheet::action_sheet(
                    ActionList::new(format!("Delete “{name}”"), items).subtitle(message),
                )));
            }
            (n, m) => {
                let mut title = String::from("Delete ");
                if n > 0 {
                    title.push_str(&format!(
                        "{n} {} and ",
                        if n == 1 {
                            "conversation"
                        } else {
                            "conversations"
                        }
                    ));
                }
                title.push_str(&format!(
                    "{m} {}?",
                    if m == 1 { "folder" } else { "folders" }
                ));
                let items = vec![
                    ActionItem::new(
                        "Move to trash",
                        list(Act::Delete {
                            keys,
                            recursive: true,
                        }),
                    )
                    .icon(Icon::Trash)
                    .tone(Tone::Destructive),
                ];
                self.ui.open(Box::new(sheet::action_sheet(
                    ActionList::new(title, items)
                        .subtitle("Everything inside the folders moves to the trash too."),
                )));
            }
        }
    }

    /// Move to folder… (PAR-068): the top level and every folder in tree
    /// order; where the items are is checked, a folder never takes itself.
    pub(crate) fn move_picker(&mut self, keys: Vec<Key>) {
        if keys.is_empty() {
            return;
        }
        let parents: HashSet<Option<String>> = keys
            .iter()
            .filter_map(|key| self.sidebar.parent_of(key))
            .collect();
        let here =
            |id: Option<&str>| parents.len() == 1 && parents.contains(&id.map(str::to_owned));
        let mut blocked: HashSet<String> = HashSet::new();
        for key in &keys {
            if let Key::Folder(id) = key {
                blocked.extend(self.sidebar.descendant_folders(id));
            }
        }
        let mut items = vec![
            ActionItem::new(
                "Top level",
                list(Act::MoveTo {
                    keys: keys.clone(),
                    parent: None,
                }),
            )
            .icon(Icon::Tray)
            .checked(here(None)),
        ];
        for (id, depth) in self.sidebar.folder_tree() {
            if blocked.contains(&id) {
                continue;
            }
            let name = &self.sidebar.folders[&id].name;
            let mut item = ActionItem::new(
                format!("{}{name}", "  ".repeat(depth)),
                list(Act::MoveTo {
                    keys: keys.clone(),
                    parent: Some(id.clone()),
                }),
            )
            .icon(Icon::FolderFill)
            .checked(here(Some(&id)));
            if depth > 0 {
                item = item.detail(self.sidebar.folder_path(Some(&id)));
            }
            items.push(item);
        }
        let title = format!("Move {}", label_for(&self.sidebar, &keys));
        self.ui
            .open(Box::new(sheet::action_sheet(ActionList::new(title, items))));
    }
}
