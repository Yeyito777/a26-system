//! The file browser (WP-D; PAR-237): a sheet over a folder of the phone's
//! files. Its path on top, then Up (to the folder holding it), the folders
//! and the files with their size and date, and a row showing or hiding
//! dotfiles. A folder opens in place (the sheet goes back to its top), a
//! file is attached to the draft the sheet was opened for and the sheet
//! closes. Only the rows on screen are drawn: a folder may hold thousands.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use crate::gfx::{Canvas, RectF};

use super::super::attachments;
use super::super::model::attach::{self, Browser};
use super::super::model::composer::Act;
use super::super::model::{Action, Model};
use super::super::theme::{self, ACTION_ICON, ACTION_ITEM, HAIRLINE, T_META, T_ROW, W};

/// A row: its label and its line under it grow with the text size.
fn row_h() -> f32 {
    theme::grown(ACTION_ITEM, 100.0)
}
use super::super::ui::icons::{self, Icon};
use super::super::ui::overlay::Cx;
use super::super::ui::sheet::{Reply, Sheet, SheetContent};
use super::super::ui::widgets;

/// The path line over the rows.
const HEAD: f32 = 84.0;

#[derive(Debug, Clone, PartialEq)]
enum Row {
    Up(PathBuf),
    Folder(PathBuf, String),
    File {
        path: PathBuf,
        name: String,
        detail: String,
    },
    /// Show (true) or hide the dotfiles, and how many there are.
    Hidden(bool, usize),
    /// Loading, empty, or why the folder cannot be read.
    Note(String),
}

/// What the sheet shows of the model's browser.
pub struct FileSheet {
    rows: Vec<Row>,
    dir: String,
    /// The browser's state these rows were made from.
    made: Option<u64>,
    moves: u64,
    reset: bool,
}

pub fn file_sheet() -> Sheet<FileSheet> {
    Sheet::new(FileSheet {
        rows: Vec::new(),
        dir: String::new(),
        made: None,
        moves: 0,
        reset: false,
    })
}

fn state_key(browser: &Browser) -> u64 {
    let mut hasher = DefaultHasher::new();
    (
        &browser.dir,
        browser.listed,
        browser.listing.is_some(),
        browser.hidden,
    )
        .hash(&mut hasher);
    hasher.finish()
}

fn image_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|e| lower.ends_with(e))
}

fn rows(model: &Model) -> Vec<Row> {
    let browser = &model.composer.browser;
    let mut rows = Vec::new();
    if let Some(parent) = attach::parent(&browser.dir) {
        rows.push(Row::Up(parent));
    }
    match &browser.listing {
        None => rows.push(Row::Note("Loading…".into())),
        Some(Err(why)) => rows.push(Row::Note(why.clone())),
        Some(Ok(_)) => {
            let (shown, hidden) = browser.shown();
            for entry in &shown {
                let path = browser.dir.join(&entry.name);
                rows.push(if entry.dir {
                    Row::Folder(path, entry.name.clone())
                } else {
                    let date = if entry.modified > 0 {
                        format!(" · {}", crate::clock::local_date_time_text(entry.modified))
                    } else {
                        String::new()
                    };
                    Row::File {
                        path,
                        name: entry.name.clone(),
                        detail: format!("{}{date}", attachments::size_text(entry.size)),
                    }
                });
            }
            if shown.is_empty() {
                rows.push(Row::Note("This folder is empty.".into()));
            }
            if hidden > 0 {
                rows.push(Row::Hidden(!browser.hidden, hidden));
            }
            if shown.len() >= super::super::files::MAX_ENTRIES {
                rows.push(Row::Note(format!(
                    "Only the first {} entries are shown.",
                    super::super::files::MAX_ENTRIES
                )));
            }
        }
    }
    rows
}

/// `path` fitting `width`: its last parts, after "…/" if cut.
fn fit_path(path: &str, width: f32) -> String {
    if widgets::measure(path, T_META) <= width {
        return path.to_owned();
    }
    let mut rest = path;
    while let Some((_, tail)) = rest.trim_start_matches('/').split_once('/') {
        rest = tail;
        let candidate = format!("…/{rest}");
        if widgets::measure(&candidate, T_META) <= width {
            return candidate;
        }
    }
    widgets::fit(path, T_META, width)
}

impl SheetContent for FileSheet {
    fn title(&self) -> String {
        "Browse files".into()
    }

    fn layout(&mut self, cx: &Cx, _width: f32) -> f32 {
        let browser = &cx.model.composer.browser;
        let key = state_key(browser);
        if self.made != Some(key) {
            self.rows = rows(cx.model);
            self.dir = browser.dir.to_string_lossy().into_owned();
            self.made = Some(key);
        }
        if browser.moves != self.moves {
            self.moves = browser.moves;
            self.reset = true;
        }
        HEAD + self.rows.len() as f32 * row_h()
    }

    fn scroll_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset)
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        let width = W - 2.0 * x;
        if top + HEAD > 0.0 {
            widgets::text(
                canvas,
                &fit_path(&self.dir, width),
                x,
                T_META.baseline(top + 8.0),
                T_META,
                p.muted,
                1.0,
            );
        }
        let first = ((-top - HEAD) / row_h()).floor().max(0.0) as usize;
        for (index, row) in self.rows.iter().enumerate().skip(first) {
            let y = top + HEAD + index as f32 * row_h();
            if y > cx.height {
                break;
            }
            widgets::hairline(canvas, 0.0, W, y, c.rule_sheet_row);
            if pressed == Some(index) && !matches!(row, Row::Note(_)) {
                widgets::fill(
                    canvas,
                    RectF::new(0.0, y + HAIRLINE, W, row_h() - HAIRLINE),
                    p.selection_background,
                );
            }
            let (icon, label, detail, muted): (Option<Icon>, &str, Option<String>, bool) = match row
            {
                Row::Up(parent) => (
                    Some(Icon::ArrowUp),
                    "Up",
                    Some(parent.to_string_lossy().into_owned()),
                    false,
                ),
                Row::Folder(_, name) => (Some(Icon::Folder), name, None, false),
                Row::File { name, detail, .. } => (
                    Some(if image_name(name) {
                        Icon::Image
                    } else {
                        Icon::FileText
                    }),
                    name,
                    Some(detail.clone()),
                    false,
                ),
                Row::Hidden(show, count) => (
                    None,
                    if *show {
                        "Show hidden files"
                    } else {
                        "Hide hidden files"
                    },
                    Some(format!(
                        "{count} name{} starting with a dot",
                        if *count == 1 { "" } else { "s" }
                    )),
                    true,
                ),
                Row::Note(text) => (None, text, None, true),
            };
            let mut text_x = x;
            if let Some(icon) = icon {
                icons::draw(
                    canvas,
                    icon,
                    x + ACTION_ICON / 2.0,
                    y + row_h() / 2.0,
                    ACTION_ICON,
                    if matches!(row, Row::Folder(..)) {
                        p.accent
                    } else {
                        p.secondary_accent
                    },
                    1.0,
                );
                text_x = x + ACTION_ICON + 44.0;
            }
            let right = W - x;
            let label_top = match &detail {
                Some(_) => y + (row_h() - T_ROW.line() - T_META.line()) / 2.0,
                None => y + (row_h() - T_ROW.line()) / 2.0,
            };
            widgets::text(
                canvas,
                &widgets::fit(label, T_ROW, right - text_x),
                text_x,
                T_ROW.baseline(label_top),
                T_ROW,
                if muted { p.muted } else { p.text },
                1.0,
            );
            if let Some(detail) = detail {
                widgets::text(
                    canvas,
                    &widgets::fit(&detail, T_META, right - text_x),
                    text_x,
                    T_META.baseline(label_top + T_ROW.line()),
                    T_META,
                    p.muted,
                    1.0,
                );
            }
        }
    }

    fn item_at(&self, _cx: &Cx, _x: f32, y: f32) -> Option<usize> {
        let y = y - HEAD;
        if y < 0.0 {
            return None;
        }
        let index = (y / row_h()).floor() as usize;
        (index < self.rows.len()).then_some(index)
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        match self.rows.get(index) {
            Some(Row::Up(dir) | Row::Folder(dir, _)) => {
                Reply::Action(Action::Composer(Act::Browse(Some(dir.clone()))))
            }
            Some(Row::File { path, .. }) => {
                Reply::CloseWith(Action::Composer(Act::AttachFile(path.clone())))
            }
            Some(Row::Hidden(..)) => Reply::Action(Action::Composer(Act::BrowseHidden)),
            Some(Row::Note(_)) | None => Reply::None,
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (self.made, theme::generation()).hash(&mut hasher);
        hasher.finish()
    }
}
