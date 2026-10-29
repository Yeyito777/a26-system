//! The folder inspector (PAR-078) and its instructions editor: a folder's
//! name and place, Rename, Pin, Mute, Create subfolder, its own AGENTS.md
//! document (`load_folder_instructions`, `set_folder_instructions`;
//! inherited by the conversations below it), and Delete folder….
//!
//! The sheets read the folder and its instructions from the model each
//! frame, so a rename, a pin or the document arriving shows in them.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::control::KeyboardPurpose;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::text::Weight;

use super::super::editor::{self, Metrics};
use super::super::model::sidebar::{Act, Instructions};
use super::super::model::{Action, Model};
use super::super::theme::{
    self, ACTION_ICON, ACTION_ITEM, ACTION_PAD, FOLDER_ICON, T_CAPTION, T_META, T_NOTE, T_ROW,
    T_SECTION, T_SMALL, TARGET,
};
use super::super::ui::icons::{self, Icon};
use super::super::ui::overlay::Cx;
use super::super::ui::sheet::{Reply, SheetContent};
use super::super::ui::widgets::{self, Button, ButtonKind};
use super::activity;
use super::pending::Key;

/// The instructions shown at most before "Edit" (the rest: in the editor).
const PREVIEW_LINES: usize = 40;
/// The editor's text area: lines shown.
const EDITOR_LINES: usize = 10;

#[derive(Debug, Clone, PartialEq)]
enum Item {
    /// Icon, name, where it is and its count.
    Head {
        name: String,
        meta: String,
    },
    Row {
        icon: Icon,
        label: String,
        destructive: bool,
        act: Act,
    },
    Section(&'static str),
    /// Lines of the document (`T_SMALL`), or a note (`T_CAPTION` muted).
    Lines {
        lines: Vec<String>,
        note: bool,
    },
}

/// A row: a label and a detail line, growing with the text.
fn row_h() -> f32 {
    theme::grown(ACTION_ITEM, 100.0)
}

impl Item {
    fn height(&self) -> f32 {
        match self {
            // Their text grows with the text size (`theme::grown`).
            Item::Head { .. } => theme::grown(150.0, 100.0),
            Item::Row { .. } => row_h(),
            Item::Section(_) => theme::grown(101.0, 40.0),
            Item::Lines { lines, note: false } => 28.0 + lines.len() as f32 * T_SMALL.line() + 28.0,
            Item::Lines { lines, note: true } => {
                11.0 + lines.len() as f32 * T_CAPTION.line() + 11.0
            }
        }
    }
}

/// Wrapped lines of `text` (its line breaks kept).
fn lines(text: &str, width: f32, ty: theme::Type) -> Vec<String> {
    let metrics = Metrics::new(ty.weight, ty.px());
    editor::wrap(text, width, &metrics)
        .iter()
        .map(|line| text[line.start..line.end].to_owned())
        .collect()
}

/// The folder inspector.
pub struct FolderSheet {
    folder: String,
    width: f32,
    items: Vec<(f32, Item)>,
}

impl FolderSheet {
    pub fn new(folder: String) -> Self {
        Self {
            folder,
            width: theme::W,
            items: Vec::new(),
        }
    }

    fn items(&self, model: &Model) -> Vec<Item> {
        let sidebar = &model.sidebar;
        let Some(f) = sidebar.folders.get(&self.folder) else {
            return vec![Item::Lines {
                lines: vec!["This folder is gone.".into()],
                note: true,
            }];
        };
        let key = Key::Folder(self.folder.clone());
        let count = sidebar
            .aggregates_cached()
            .and_then(|a| a.folders.get(&self.folder).map(|a| a.count))
            .unwrap_or(0);
        let meta = format!(
            "{} · {} {}",
            sidebar.folder_path(f.parent_id.as_deref()),
            activity::thousands(count as u64),
            if count == 1 { "chat" } else { "chats" }
        );
        let row = |icon, label: &str, act| Item::Row {
            icon,
            label: label.into(),
            destructive: false,
            act,
        };
        let mut items = vec![
            Item::Head {
                name: f.name.clone(),
                meta,
            },
            row(Icon::Pencil, "Rename…", Act::Rename(key.clone())),
            row(
                if f.pinned { Icon::PinOff } else { Icon::Pin },
                if f.pinned { "Unpin" } else { "Pin" },
                Act::Pin(vec![key.clone()], !f.pinned),
            ),
            row(
                if f.muted { Icon::Bell } else { Icon::BellOff },
                if f.muted { "Unmute" } else { "Mute" },
                Act::Mute(key.clone(), !f.muted),
            ),
            row(
                Icon::FolderPlus,
                "Create subfolder…",
                Act::NewFolder {
                    parent: Some(self.folder.clone()),
                    items: Vec::new(),
                },
            ),
            Item::Section("Folder instructions"),
        ];
        let note = |text: &str| Item::Lines {
            lines: lines(text, self.width, T_CAPTION),
            note: true,
        };
        match sidebar.instructions.get(&self.folder) {
            None | Some(Instructions::Loading(_)) => items.push(note("Loading…")),
            Some(Instructions::Failed(error)) => items.push(note(error)),
            Some(Instructions::Loaded(text)) if text.trim().is_empty() => {
                items.push(note("No instructions of its own."));
            }
            Some(Instructions::Loaded(text)) => {
                let mut shown = lines(text.trim_end(), self.width - 44.0, T_SMALL);
                if shown.len() > PREVIEW_LINES {
                    let more = shown.len() - PREVIEW_LINES;
                    shown.truncate(PREVIEW_LINES);
                    shown.push(format!("… +{more} more lines"));
                }
                items.push(Item::Lines {
                    lines: shown,
                    note: false,
                });
            }
        }
        if matches!(
            sidebar.instructions.get(&self.folder),
            Some(Instructions::Loaded(_))
        ) {
            items.push(row(
                Icon::Pencil,
                "Edit instructions",
                Act::EditInstructions(self.folder.clone()),
            ));
        }
        items.push(note(
            "Instructions are inherited by conversations below this folder. Clearing removes only this folder’s document.",
        ));
        items.push(Item::Row {
            icon: Icon::Trash,
            label: "Delete folder…".into(),
            destructive: true,
            act: Act::ConfirmDelete(vec![key]),
        });
        items
    }

    fn rows(&self) -> impl Iterator<Item = (usize, f32, &Act)> {
        self.items
            .iter()
            .filter_map(|(y, item)| match item {
                Item::Row { act, .. } => Some((*y, act)),
                _ => None,
            })
            .enumerate()
            .map(|(index, (y, act))| (index, y, act))
    }
}

impl SheetContent for FolderSheet {
    fn title(&self) -> String {
        "Folder".into()
    }

    fn layout(&mut self, cx: &Cx, width: f32) -> f32 {
        self.width = width;
        let mut y = 0.0;
        self.items = self
            .items(cx.model)
            .into_iter()
            .map(|item| {
                let at = y;
                y += item.height();
                (at, item)
            })
            .collect();
        y
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        let mut row = 0;
        for (y, item) in &self.items {
            let y = top + y;
            let h = item.height();
            if !widgets::shows(canvas, y, y + h) {
                if matches!(item, Item::Row { .. }) {
                    row += 1;
                }
                continue;
            }
            match item {
                Item::Head { name, meta } => {
                    let icon = RectF::new(
                        x,
                        y + ((h - FOLDER_ICON) / 2.0).floor(),
                        FOLDER_ICON,
                        FOLDER_ICON,
                    );
                    widgets::boxed(canvas, icon, c.folder_icon_bg, c.folder_icon_border);
                    let (icx, icy) = icon.center();
                    icons::draw(
                        canvas,
                        Icon::FolderFill,
                        icx,
                        icy,
                        47.0,
                        p.secondary_accent,
                        1.0,
                    );
                    let tx = x + FOLDER_ICON + 28.0;
                    let ty = T_ROW.weight(Weight::MonoBold);
                    let room = self.width - FOLDER_ICON - 28.0;
                    widgets::text(
                        canvas,
                        &widgets::fit(name, ty, room),
                        tx,
                        y + 66.0,
                        ty,
                        p.text,
                        1.0,
                    );
                    widgets::text(
                        canvas,
                        &widgets::fit(meta, T_META, room),
                        tx,
                        y + 112.0,
                        T_META,
                        p.muted,
                        1.0,
                    );
                }
                Item::Row {
                    icon,
                    label,
                    destructive,
                    ..
                } => {
                    let full = RectF::new(x - ACTION_PAD, y, self.width + 2.0 * ACTION_PAD, h);
                    if pressed == Some(row) {
                        widgets::fill(canvas, full, p.selection_background);
                    }
                    widgets::hairline(canvas, full.x, full.x + full.w, y, c.rule_sheet_row);
                    let color = if *destructive { p.error } else { p.accent };
                    icons::draw(
                        canvas,
                        *icon,
                        x + ACTION_ICON / 2.0,
                        y + h / 2.0,
                        ACTION_ICON,
                        color,
                        1.0,
                    );
                    let label_color = if *destructive { p.error } else { p.text };
                    widgets::text(
                        canvas,
                        label,
                        x + ACTION_ICON + 34.0,
                        T_ROW.baseline(y + (h - T_ROW.line()) / 2.0),
                        T_ROW,
                        label_color,
                        1.0,
                    );
                    row += 1;
                }
                Item::Section(title) => {
                    widgets::text(
                        canvas,
                        title,
                        x,
                        T_SECTION.baseline(y + 44.0),
                        T_SECTION,
                        p.accent,
                        1.0,
                    );
                }
                Item::Lines { lines, note } => {
                    let (ty, color, mut ly) = if *note {
                        (T_CAPTION, p.muted, y + 11.0)
                    } else {
                        let box_ = RectF::new(x, y + 11.0, self.width, h - 22.0);
                        widgets::boxed(canvas, box_, p.surface_background, c.rule_settings);
                        (T_SMALL, p.text, y + 28.0)
                    };
                    let lx = if *note { x } else { x + 22.0 };
                    for line in lines {
                        widgets::text(canvas, line, lx, ty.baseline(ly), ty, color, 1.0);
                        ly += ty.line();
                    }
                }
            }
        }
    }

    fn item_at(&self, _cx: &Cx, _x: f32, y: f32) -> Option<usize> {
        self.rows()
            .find(|(_, top, _)| y >= *top && y < top + row_h())
            .map(|(index, _, _)| index)
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        let Some((_, _, act)) = self.rows().find(|(i, _, _)| *i == index) else {
            return Reply::None;
        };
        let act = act.clone();
        match act {
            // A pin or a mute shows here at once; the rest open over it.
            Act::Pin(..) | Act::Mute(..) => Reply::Action(Action::List(act)),
            Act::ConfirmDelete(_) => Reply::CloseWith(Action::List(act)),
            _ => Reply::Action(Action::List(act)),
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.folder, format!("{:?}", self.items)).hash(&mut hasher);
        hasher.finish()
    }

    fn scoped(&self) -> bool {
        false
    }
}

/// The instructions editor: the document in a box, typed into at its end
/// with Moon's keyboard (Return is a line break), Cancel and Save.
pub struct InstructionsEditor {
    folder: String,
    text: String,
    width: f32,
}

impl InstructionsEditor {
    pub fn new(folder: String, text: String) -> Self {
        Self {
            folder,
            text,
            width: theme::W,
        }
    }

    fn area_h(&self) -> f32 {
        EDITOR_LINES as f32 * T_NOTE.line() + 2.0 * 22.0
    }

    fn buttons(&self) -> (RectF, RectF) {
        let gap = 22.0;
        let w = ((self.width - gap) / 2.0).floor();
        let y = self.area_h() + 34.0;
        (
            RectF::new(0.0, y, w, TARGET),
            RectF::new(w + gap, y, w, TARGET),
        )
    }
}

impl SheetContent for InstructionsEditor {
    fn title(&self) -> String {
        "Folder instructions".into()
    }

    fn layout(&mut self, _cx: &Cx, width: f32) -> f32 {
        self.width = width;
        self.buttons().0.y + TARGET
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        let area = RectF::new(x, top, self.width, self.area_h());
        widgets::boxed(canvas, area, p.surface_background, c.field_focused);
        let metrics = Metrics::new(T_NOTE.weight, T_NOTE.px());
        let inner = self.width - 56.0;
        let lines = editor::wrap(&self.text, inner, &metrics);
        // The end (where typing goes) stays in view.
        let first = lines.len().saturating_sub(EDITOR_LINES);
        let mut y = top + 22.0;
        let mut caret = (x + 28.0, y);
        for line in &lines[first..] {
            widgets::text(
                canvas,
                &self.text[line.start..line.end],
                x + 28.0,
                T_NOTE.baseline(y),
                T_NOTE,
                p.text,
                1.0,
            );
            caret = (x + 28.0 + line.width, y);
            y += T_NOTE.line();
        }
        if self.text.is_empty() {
            widgets::text(
                canvas,
                "AGENTS.md for this folder",
                x + 28.0,
                T_NOTE.baseline(top + 22.0),
                T_NOTE,
                c.placeholder,
                1.0,
            );
        }
        widgets::fill(
            canvas,
            RectF::new(
                (caret.0 + 2.0).round(),
                caret.1 + 4.0,
                4.0,
                T_NOTE.line() - 8.0,
            ),
            p.accent,
        );
        let (cancel, save) = self.buttons();
        let at = |r: RectF| RectF::new(r.x + x, r.y + top, r.w, r.h);
        widgets::button(
            canvas,
            at(cancel),
            &Button::new("Cancel", ButtonKind::Secondary).pressed(pressed == Some(0)),
            p.sidebar_background,
        );
        widgets::button(
            canvas,
            at(save),
            &Button::new("Save", ButtonKind::Primary).pressed(pressed == Some(1)),
            p.sidebar_background,
        );
    }

    fn item_at(&self, _cx: &Cx, x: f32, y: f32) -> Option<usize> {
        let (cancel, save) = self.buttons();
        if cancel.contains(x, y) {
            Some(0)
        } else if save.contains(x, y) {
            Some(1)
        } else {
            None
        }
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        match index {
            0 => Reply::Close,
            _ => Reply::CloseWith(Action::List(Act::SaveInstructions {
                folder: self.folder.clone(),
                text: self.text.clone(),
            })),
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.folder, &self.text).hash(&mut hasher);
        hasher.finish()
    }

    fn key_input(&mut self, input: &KeyInput, _cx: &Cx) -> Reply {
        match input {
            KeyInput::Character(character) => self.text.push(*character),
            KeyInput::Enter => self.text.push('\n'),
            KeyInput::Backspace => {
                self.text.pop();
            }
        }
        Reply::Redraw
    }

    fn keyboard(&self) -> Option<KeyboardPurpose> {
        Some(KeyboardPurpose::Multiline)
    }
}
