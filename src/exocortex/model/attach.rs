//! The composer's attachments (WP-D; PAR-237–242): the "+" sheet, the file
//! browser's folder, files and pasted images prepared off the UI thread
//! (`files.rs`) into the draft they were chosen for even if the user moved
//! on meanwhile (PAR-189), the tray's ✕, and what a send takes along. A
//! draft's attachments are kept in memory, not across restarts.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::super::attachments::{self, Attachment, MAX_ITEMS, TOO_MANY};
use super::super::files::{Done, Entry, Job};
use super::super::ui::icons::Icon;
use super::super::ui::sheet::{self, ActionItem, ActionList, Tone};
use super::composer::{Act, ComposerError};
use super::{Action, Effect, Model, UiAct};

/// The file browser: its folder and what it holds.
#[derive(Debug, Default)]
pub struct Browser {
    /// The folder shown (empty: not opened yet).
    pub dir: PathBuf,
    /// Its entries; None while they are read.
    pub listing: Option<Result<Vec<Entry>, String>>,
    /// Dotfiles show.
    pub hidden: bool,
    /// The draft a chosen file goes to.
    pub key: String,
    /// Counts folder changes (the sheet goes back to its top).
    pub moves: u64,
    /// Counts listings received.
    pub listed: u64,
}

impl Browser {
    /// The entries shown: without dotfiles unless asked; and how many
    /// dotfiles there are.
    pub fn shown(&self) -> (Vec<&Entry>, usize) {
        let Some(Ok(entries)) = &self.listing else {
            return (Vec::new(), 0);
        };
        let hidden = entries.iter().filter(|e| e.name.starts_with('.')).count();
        let shown = entries
            .iter()
            .filter(|e| self.hidden || !e.name.starts_with('.'))
            .collect();
        (shown, hidden)
    }
}

/// Where the browser opens first: the home folder.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/root"))
}

impl Model {
    /// Draft `key`'s attachments.
    pub fn attachments_of(&self, key: &str) -> &[Attachment] {
        self.composer
            .attachments
            .get(key)
            .map_or(&[][..], Vec::as_slice)
    }

    /// The shown draft's attachments, and how many are being prepared.
    pub fn shown_attachments(&self) -> (&[Attachment], usize) {
        let key = self.shown_draft_key();
        let importing = self.composer.importing.get(&key).copied().unwrap_or(0);
        (self.attachments_of(&key), importing)
    }

    /// Room left in draft `key`, counting those being prepared.
    fn room(&self, key: &str) -> usize {
        let importing = self.composer.importing.get(key).copied().unwrap_or(0);
        MAX_ITEMS.saturating_sub(self.attachments_of(key).len() + importing)
    }

    fn attach_error(&mut self, key: &str, text: &str) {
        self.composer.errors.insert(
            key.to_owned(),
            ComposerError {
                text: text.to_owned(),
                since: Instant::now(),
            },
        );
        self.ui.haptic(super::super::ui::Haptic::Error);
    }

    /// "+": Browse files, Paste image, Record voice.
    pub(super) fn attach_menu(&mut self) {
        let full = self.room(&self.shown_draft_key()) == 0;
        let guarded = |item: ActionItem| {
            if full {
                ActionItem {
                    action: Action::Ui(UiAct::Toast(TOO_MANY.into())),
                    tone: Tone::Muted,
                    ..item
                }
            } else {
                item
            }
        };
        let items = vec![
            guarded(
                ActionItem::new("Browse files", Action::Composer(Act::Browse(None)))
                    .icon(Icon::FileText)
                    .detail("Images, or text up to 120,000 characters"),
            ),
            guarded(
                ActionItem::new("Paste image", Action::Composer(Act::PasteImage)).icon(Icon::Paste),
            ),
            ActionItem::new("Record voice", Action::Composer(Act::Mic)).icon(Icon::Mic),
        ];
        let mut list = ActionList::new("Add attachment or voice", items).scoped();
        if full {
            list = list.subtitle(TOO_MANY);
        }
        self.ui.haptic(super::super::ui::Haptic::Light);
        self.ui.open(Box::new(sheet::action_sheet(list)));
    }

    /// The file browser at `dir` (None: where it was last, or home).
    pub(super) fn browse(&mut self, dir: Option<PathBuf>) {
        let open = dir.is_none();
        if open {
            self.composer.browser.key = self.shown_draft_key();
        }
        let browser = &mut self.composer.browser;
        let dir = dir.unwrap_or_else(|| {
            if browser.dir.as_os_str().is_empty() {
                home()
            } else {
                browser.dir.clone()
            }
        });
        if browser.dir != dir {
            browser.moves += 1;
        }
        browser.dir = dir.clone();
        browser.listing = None;
        self.ui.effect(Effect::Files(Job::List { dir }));
        if open {
            self.ui
                .open(Box::new(super::super::view::files::file_sheet()));
        }
    }

    pub(super) fn browse_hidden(&mut self) {
        self.composer.browser.hidden = !self.composer.browser.hidden;
    }

    /// A file chosen in the browser: read into the draft it was opened for.
    pub(super) fn attach_file(&mut self, path: PathBuf) {
        let key = self.composer.browser.key.clone();
        if self.room(&key) == 0 {
            self.attach_error(&key, TOO_MANY);
            return;
        }
        *self.composer.importing.entry(key.clone()).or_default() += 1;
        self.ui.effect(Effect::Files(Job::Read { key, path }));
    }

    /// The clipboard's image, into the shown draft.
    pub(super) fn paste_image(&mut self) {
        let key = self.shown_draft_key();
        if self.room(&key) == 0 {
            self.attach_error(&key, TOO_MANY);
            return;
        }
        let importing = self.composer.importing.entry(key.clone()).or_default();
        *importing += 1;
        let name = format!(
            "Pasted image {}",
            self.attachments_of(&key).len() + self.composer.importing[&key]
        );
        self.ui.effect(Effect::Files(Job::Paste { key, name }));
    }

    pub(super) fn remove_attachment(&mut self, id: u64) {
        let key = self.shown_draft_key();
        if let Some(list) = self.composer.attachments.get_mut(&key) {
            list.retain(|a| a.id != id);
            if list.is_empty() {
                self.composer.attachments.remove(&key);
            }
        }
        self.composer.errors.remove(&key);
        self.ui.haptic(super::super::ui::Haptic::Light);
    }

    /// A folder listed, or a file or image prepared.
    pub fn files_done(&mut self, done: Done) {
        match done {
            Done::Listed { dir, result } => {
                if self.composer.browser.dir == dir {
                    self.composer.browser.listing = Some(result);
                    self.composer.browser.listed += 1;
                }
            }
            Done::Prepared { key, result } => {
                if let Some(n) = self.composer.importing.get_mut(&key) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        self.composer.importing.remove(&key);
                    }
                }
                match result {
                    Ok(attachment) if self.attachments_of(&key).len() < MAX_ITEMS => {
                        self.add_attachments(&key, vec![attachment]);
                        // The tray shows the newest.
                        self.composer.tray_scroll.set(f32::MAX);
                    }
                    Ok(_) => self.attach_error(&key, TOO_MANY),
                    Err(why) => self.attach_error(&key, &why),
                }
                self.chat.touch();
            }
        }
    }

    /// Puts `items` into draft `key` after what is there.
    fn add_attachments(&mut self, key: &str, items: Vec<Attachment>) {
        let list = self.composer.attachments.entry(key.to_owned()).or_default();
        for mut item in items {
            self.composer.attachment_serial += 1;
            item.id = self.composer.attachment_serial;
            list.push(item);
        }
    }

    /// Takes draft `key`'s attachments (they go with its message).
    pub(crate) fn take_attachments(&mut self, key: &str) -> Vec<Attachment> {
        self.composer.attachments.remove(key).unwrap_or_default()
    }

    /// Puts attachments back in front of draft `key`'s (a send that did not
    /// arrive, a rewind, a queued message pulled back).
    pub(crate) fn restore_attachments(&mut self, key: &str, items: Vec<Attachment>) {
        if items.is_empty() {
            return;
        }
        let after = self.take_attachments(key);
        self.add_attachments(key, items);
        self.add_attachments(key, after);
    }

    /// The draft's attachments, checked for a send: still being prepared,
    /// over the budget with `text`, or images to a model without vision.
    pub(crate) fn attachments_problem(
        &self,
        key: &str,
        text: &str,
        provider: &str,
        model: &str,
    ) -> Option<String> {
        if self.composer.importing.get(key).copied().unwrap_or(0) > 0 {
            return Some(attachments::PREPARING.into());
        }
        let items = self.attachments_of(key);
        if attachments::weight(text, items) > attachments::BUDGET {
            return Some(attachments::OVER_BUDGET.into());
        }
        let vision = super::ops::model_info(&self.ops.providers, provider, model)
            .and_then(|m| m.supports_images)
            .unwrap_or(true);
        (!vision && items.iter().any(Attachment::is_image)).then(|| {
            format!("Image inputs are not supported by {provider}/{model}. Remove the attachment or switch to a vision-capable model.")
        })
    }
}

/// A path's parent, unless it is the root.
pub fn parent(dir: &Path) -> Option<PathBuf> {
    dir.parent().map(Path::to_path_buf)
}
