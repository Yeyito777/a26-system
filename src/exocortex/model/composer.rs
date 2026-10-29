//! What the user writes (WP-D): the composer's text with its caret and
//! selection (`editor.rs`), drafts per conversation and per new chat kept
//! across restarts (`drafts.rs`), sending (or queueing at the next turn or
//! the message's end while a turn runs), new conversations with only the
//! choices the user made, stopping a turn, and the queue the daemon keeps.
//!
//! Sending is optimistic (PAR-185): the text leaves the composer at once
//! and shows in the transcript; a refusal or a deadline puts it back into
//! the draft it came from, wherever the user is by then (PAR-189), and
//! says why above the composer.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::attachments::{self, Attachment};
use super::super::dictation::{self, Do, Line};
use super::super::drafts::Drafts;
use super::super::editor::{self, Cursor};
use super::super::proto::{self, Entry, Event, Queued};
use super::super::requests::{self, Owner, Pending};
use super::super::ui::Haptic;
use super::super::ui::sheet::{self, ActionItem, ActionList, Tone};
use super::attach::Browser;
use super::{Action, Chat, Echo, Effect, Live, Model, Page, UiAct, now_ms};
use crate::control::KeyboardPurpose;
use crate::keys::KeyInput;

/// A message and its attachments may not be larger (PAR-191).
#[cfg(test)]
pub const BUDGET: usize = attachments::BUDGET;
/// Typing is written into `drafts.json` this long after it pauses (the
/// store then waits its own 500 ms).
const SAVE_AFTER: Duration = Duration::from_millis(300);

/// When a message joins a turn that is running (PAR-183).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timing {
    /// Before the next tool round ("Steer now" on providers that steer).
    NextTurn,
    /// After the response finishes.
    MessageEnd,
}

impl Timing {
    pub fn wire(self) -> &'static str {
        match self {
            Timing::NextTurn => "next-turn",
            Timing::MessageEnd => "message-end",
        }
    }
}

/// An end of the selection a finger moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum End {
    /// The caret (with no selection, the caret itself).
    Caret,
    Anchor,
}

/// Dictation stops by itself before the hook's capture limit (120 s).
const RECORD_FOR: Duration = Duration::from_secs(115);
/// Its transcription is given up on after this (PAR-247).
const TRANSCRIBE_WITHIN: Duration = requests::TRANSCRIPTION_WITHIN;
/// Levels kept for the waveform (newest last).
pub const LEVELS: usize = 64;

/// Where the composer's microphone is (PAR-243–250).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum VoicePhase {
    #[default]
    Idle,
    Recording,
    Transcribing,
}

/// The composer's microphone: one recording at a time, for the draft it
/// started in (its words go there, wherever the user is by then).
#[derive(Debug, Default)]
pub struct Voice {
    pub phase: VoicePhase,
    pub key: Option<String>,
    /// When the phase began (the recording's clock, the transcription's
    /// deadline).
    pub since: Option<Instant>,
    /// Recent loudness, 0..1, newest last.
    pub levels: VecDeque<f32>,
    /// Bumped with every level (what the strip's frame shows).
    pub version: u64,
}

impl Voice {
    pub fn active(&self) -> bool {
        self.phase != VoicePhase::Idle
    }
}

/// An error shown above the composer for one draft (PAR-186).
#[derive(Debug, Clone, PartialEq)]
pub struct ComposerError {
    pub text: String,
    pub since: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Send,
    Queue,
    New,
}

/// A message on its way: what to put back if it does not arrive.
#[derive(Debug, Clone)]
struct Submission {
    kind: Kind,
    conv: String,
    /// The draft it came from.
    key: String,
    text: String,
    started_at: u64,
    queue_id: Option<String>,
    /// Its attachments (back into the draft with its text).
    attachments: Vec<Attachment>,
}

/// The composer's state.
#[derive(Debug, Default)]
pub struct Composer {
    /// What is typed: the shown draft's text.
    pub text: String,
    /// The caret and the selection in it.
    pub cursor: Cursor,
    /// The draft `text` belongs to (a conversation id or a new chat's
    /// `draft:<folder>`); None before any.
    pub key: Option<String>,
    /// How far the editor's text is scrolled up (px), and whether the next
    /// frame scrolls the caret into view. The view resolves both (it knows
    /// the wrap and the editor's height), hence cells.
    pub scroll: Cell<f32>,
    pub reveal: Cell<bool>,
    /// When the caret last moved (it shows solid for a blink after).
    pub since: Option<Instant>,
    /// The edit menu (Copy, Paste, …) shown for this serial, if any.
    pub menu: Option<u64>,
    serial: u64,
    /// A long press is selecting: moving the finger extends it.
    pub selecting: bool,
    /// The kept drafts.
    pub drafts: Drafts,
    /// Typed since the drafts were last written into the kept file.
    unsaved: Option<Instant>,
    /// Errors per draft key.
    pub errors: HashMap<String, ComposerError>,
    /// Messages sent or queued and not yet confirmed, by request id.
    inflight: HashMap<String, Submission>,
    /// The daemon's queue, every conversation's (`queue_updated`).
    pub queued: Vec<Queued>,
    /// Queued from here, not in the daemon's queue yet (request id).
    pending: Vec<(String, Queued)>,
    /// Removals asked for and not yet acknowledged: queue id → request id
    /// (and whether its text goes back into the draft).
    removing: HashMap<String, (String, bool)>,
    /// A queued message's last error (a removal refused).
    queue_errors: HashMap<String, String>,
    /// Stops on their way: conversation → request id.
    stopping: HashMap<String, String>,
    /// The microphone.
    pub voice: Voice,
    /// The daemon's paths for macros (`tools_available.macroEnvironment`).
    pub macro_env: Option<super::super::macros::MacroEnv>,
    /// The command and macro picker shown (its serial) and what it shows.
    pub picker: Option<u64>,
    pub picker_state: Option<super::super::commands::run::PickerState>,
    /// The Macros tab is chosen.
    pub picker_macros: bool,
    /// The picker was dismissed for exactly this text (PAR-198).
    pub picker_dismissed: Option<String>,
    /// `/ping` asked (its reqId) and when.
    pub ping: Option<(String, Instant)>,
    /// Listings of the daemon host's directories (path completion).
    pub paths: super::super::commands::paths::Paths,
    /// What is typed is a command (the trailing button says Run).
    pub is_command: bool,
    /// The text's valid commands, macros and arguments (byte ranges), drawn
    /// in the command colour.
    pub highlights: Vec<(usize, usize)>,
    /// User messages before each window start seen, by conversation: entry
    /// index → user message index (a rewind's `userMessageIndex`).
    user_index: HashMap<String, std::collections::BTreeMap<u64, u64>>,
    /// A rewind on its way: its request and conversation.
    rewinding: Option<(String, String)>,
    /// Each draft's attachments (by draft key), kept in memory.
    pub attachments: HashMap<String, Vec<Attachment>>,
    /// Attachments being prepared, by draft key (the tray's "Preparing…").
    pub importing: HashMap<String, usize>,
    /// The last attachment's id.
    pub attachment_serial: u64,
    /// The file browser.
    pub browser: Browser,
    /// How far the tray is scrolled sideways (px; the view clamps it).
    pub tray_scroll: Cell<f32>,
}

impl Composer {
    /// Forgets messages on their way (tests send several in a row).
    #[cfg(test)]
    pub fn inflight_clear_for_tests(&mut self) {
        self.inflight.clear();
    }

    #[cfg(test)]
    pub fn rewinding_for_tests(&self) -> Option<&(String, String)> {
        self.rewinding.as_ref()
    }

    /// The caret and selection, inside the text.
    pub fn cursor(&self) -> Cursor {
        self.cursor.clamped(&self.text)
    }

    /// Whether a message from draft `key` is on its way.
    pub fn submitting(&self, key: &str) -> bool {
        self.inflight.values().any(|s| s.key == key)
    }

    /// The queued message `id` is not confirmed by the daemon yet.
    pub fn optimistic(&self, id: &str) -> bool {
        self.pending.iter().any(|(_, q)| q.id == id)
    }

    /// The queued message `id` is being removed.
    pub fn removing(&self, id: &str) -> bool {
        self.removing.contains_key(id)
    }

    pub fn queue_error(&self, id: &str) -> Option<&str> {
        self.queue_errors.get(id).map(String::as_str)
    }

    /// The queue: the daemon's, then what was queued from here and is not
    /// in it yet.
    pub fn all_queued(&self) -> impl Iterator<Item = &Queued> {
        self.queued.iter().chain(
            self.pending
                .iter()
                .map(|(_, q)| q)
                .filter(|q| !self.queued.iter().any(|d| d.id == q.id)),
        )
    }

    fn find_queued(&self, id: &str) -> Option<&Queued> {
        self.all_queued().find(|q| q.id == id)
    }

    /// The caret moved or the text changed: the caret shows, scrolled into
    /// view; a menu for the old state goes.
    fn touched(&mut self, now: Instant) {
        self.since = Some(now);
        self.reveal.set(true);
        self.menu = None;
    }

    fn edited(&mut self, now: Instant) {
        self.touched(now);
        self.unsaved = Some(now);
        if let Some(key) = &self.key {
            self.errors.remove(key);
        }
    }

    fn set_text(&mut self, text: String) {
        self.cursor = Cursor::at(text.len());
        self.text = text;
        self.scroll.set(0.0);
        self.reveal.set(true);
        self.menu = None;
        self.selecting = false;
    }
}

/// The label under a queued bubble, as the TUI words it (PAR-143):
/// "queued: next turn", "queued: steer", "queued: message end", or the idle
/// queue's "queued: after …" / "queued: at 14:05" / "queued: global idle".
pub fn queue_label(q: &Queued, provider: Option<&str>) -> String {
    if q.source == "global-idle" {
        let wait = q.wait_target.as_ref();
        let field = |name: &str| {
            wait.and_then(|w| w.get(name))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        return match field("type").as_str() {
            "delay" => {
                let delay = wait
                    .and_then(|w| w.get("delayMs"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                format!("queued: at {}", clock((q.created_at + delay) / 1000.0))
            }
            "conversation" => format!("queued: after {}", field("label")),
            "folder" => format!("queued: after folder {}", field("label")),
            _ => "queued: global idle".into(),
        };
    }
    if q.timing != "next-turn" {
        return "queued: message end".into();
    }
    if q.automation.is_some() || provider.is_none_or(|p| p == "anthropic") {
        "queued: next turn".into()
    } else {
        "queued: steer".into()
    }
}

/// `14:05` in local time.
fn clock(seconds: f64) -> String {
    // SAFETY: localtime_r fills the zeroed tm from a valid time_t.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = seconds as i64 as _;
        libc::localtime_r(&time, &mut tm);
        tm
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// The order the TUI shows queued messages in: next turn, message end, an
/// idle queue's targets, the global idle queue; the daemon's order within.
pub fn queue_order(q: &Queued) -> u8 {
    if q.source != "global-idle" {
        return if q.timing == "next-turn" { 0 } else { 1 };
    }
    match q
        .wait_target
        .as_ref()
        .and_then(|w| w.get("type"))
        .and_then(Value::as_str)
    {
        None | Some("global") => 3,
        _ => 2,
    }
}

/// What the user does with the composer; other areas use these too (C2's
/// message menu, E's commands): `Insert`, `RestoreDraft`, `Focus`.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Insert, RestoreDraft and Focus are for C2 and E.
pub enum Act {
    /// The trailing button: Send, or (while a turn runs) the choice of
    /// when the message joins it.
    Send,
    /// The choice made: queue for the next turn or the message's end (sent
    /// at once if the turn ended meanwhile, PAR-257).
    Deliver(Timing),
    /// Stop the turn (not gated on readiness, PAR-197).
    Abort,
    /// Give the composer the keyboard.
    Focus,
    /// Insert text where the caret is.
    Insert(String),
    /// Put text back into a conversation's draft (`None`: the new
    /// conversation's): a rewind, a failed send.
    RestoreDraft {
        conv: Option<String>,
        text: String,
    },
    /// The "+" button: attachments and voice.
    Attach,
    /// The file browser at a folder (None: where it was last, or home).
    Browse(Option<std::path::PathBuf>),
    /// Dotfiles shown in it, or not.
    BrowseHidden,
    /// A file chosen in it.
    AttachFile(std::path::PathBuf),
    /// The clipboard's image attached.
    PasteImage,
    /// The tray's ✕ on an attachment.
    RemoveAttachment(u64),
    /// The tray dragged sideways to this scroll.
    TrayScroll(f32),
    /// A tap in the editor at text `index`; `focused`: the keyboard was up.
    Place {
        index: usize,
        focused: bool,
    },
    /// A long press at `index`: its word selected, the menu.
    SelectWord(usize),
    /// A finger moving an end of the selection (or the caret) to `index`.
    MoveEnd {
        end: End,
        index: usize,
    },
    /// That finger let go: the menu for what is selected.
    EndSelect,
    /// The editor's text dragged to this scroll.
    Scroll(f32),
    Copy,
    Cut,
    Paste,
    SelectAll,
    /// Select the word at the caret.
    Select,
    /// The error label's ✕.
    DismissError,
    /// A queued message tapped: its actions.
    Queued(String),
    /// Remove a queued message (PAR-259).
    Unqueue(String),
    /// Take a queued message back into the composer (PAR-261).
    PullBack(String),
    /// The mic: start dictating into this draft.
    Mic,
    /// The recording strip's ✓: stop and transcribe.
    MicDone,
    /// Its ✕: drop the recording (or the transcription).
    MicCancel,
    /// The picker's suggestion chosen: this text replaces the word at the
    /// caret (PAR-199).
    Complete(String),
    /// The picker's tabs: macros or commands.
    PickerTab(bool),
    /// The picker's ✕.
    PickerDismiss,
    /// A queued message's text, in a prompt (PAR-260).
    QueueEdit(String),
    /// …saved.
    QueueSave(String, String),
    /// A queued message delivered at the other timing.
    QueueTiming(String, Timing),
    /// A queued message moved earlier (true) or later.
    QueueMove(String, bool),
    /// Edit / rewind a sent message (PAR-266, PAR-267; C2's message menu
    /// with the entry's stamp): asks first.
    Rewind(u64),
    /// …confirmed: its text back into the draft, everything from it on
    /// removed from the conversation.
    RewindConfirmed(u64),
    /// "Edit…" (PAR-268, the TUI's Ctrl+W; E's chat menu): queued
    /// messages, recent sent messages, the system instructions.
    EditPicker,
    /// Discard the shown draft (PAR-190; E's chat menu): asks first.
    Discard,
    DiscardConfirmed,
}

impl Model {
    /// Something to send: text, or attachments.
    pub fn has_draft(&self) -> bool {
        !self.composer.text.trim().is_empty() || !self.shown_attachments().0.is_empty()
    }

    /// The draft key of what the composer shows now: the open
    /// conversation's, or the new chat's (`draft_key`, E).
    pub fn shown_draft_key(&self) -> String {
        match &self.chat.id {
            Some(id) => id.clone(),
            None => self.draft_key(),
        }
    }

    /// The composer's error, if the shown draft has one.
    pub fn composer_error_label(&self) -> Option<&ComposerError> {
        self.composer
            .key
            .as_ref()
            .and_then(|key| self.composer.errors.get(key))
    }

    /// A message from the shown draft is on its way.
    pub fn submitting(&self) -> bool {
        self.composer
            .key
            .as_ref()
            .is_some_and(|key| self.composer.submitting(key))
    }

    /// A Stop for the open conversation is on its way.
    pub fn stopping(&self) -> bool {
        self.chat
            .id
            .as_ref()
            .is_some_and(|id| self.composer.stopping.contains_key(id))
    }

    /// The provider of the open conversation (or the new chat's choice).
    fn provider(&self) -> String {
        self.selection().provider
    }

    /// Keeps what is typed as its draft (the composer keeps showing it).
    pub(super) fn save_draft(&mut self) {
        self.composer.drafts.load(&self.persist);
        if let Some(key) = self.composer.key.clone()
            && self
                .composer
                .drafts
                .set(&key, &self.composer.text, now_ms())
        {
            self.composer.unsaved = Some(Instant::now());
        }
        self.write_drafts();
    }

    /// Shows the draft of what is open now (a conversation, or a new chat
    /// in the list's folder).
    pub(super) fn restore_draft(&mut self) {
        let key = self.shown_draft_key();
        if self.composer.key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.composer.drafts.load(&self.persist);
        let text = self
            .composer
            .drafts
            .get(&key)
            .unwrap_or_default()
            .to_owned();
        self.composer.set_text(text);
        self.composer.key = Some(key);
        self.composer.picker_dismissed = None;
        let _ = self.composer_changed();
    }

    /// Writes the drafts into the kept file now.
    fn write_drafts(&mut self) {
        if self.composer.unsaved.take().is_some() {
            if let Some(key) = self.composer.key.clone() {
                let text = self.composer.text.clone();
                self.composer.drafts.set(&key, &text, now_ms());
            }
            self.composer.drafts.save(&mut self.persist);
        }
    }

    /// Writes typing into the kept drafts once it pauses; whether anything
    /// shows differently (never).
    pub(super) fn composer_tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let voice = &self.composer.voice;
        match (voice.phase, voice.since) {
            // A recording ends with its chat: no microphone left open
            // behind another page.
            (VoicePhase::Recording, _)
                if self.page != Page::Chat || self.composer.key != voice.key =>
            {
                self.cancel_dictation();
                changed = true;
            }
            (VoicePhase::Recording, Some(since)) if now >= since + RECORD_FOR => {
                changed = true;
                self.handle_composer(Act::MicDone);
            }
            (VoicePhase::Transcribing, Some(since)) if now >= since + TRANSCRIBE_WITHIN => {
                let key = voice.key.clone().unwrap_or_default();
                self.cancel_dictation();
                self.voice_error(&key, "Voice transcription took too long. Try again.");
                changed = true;
            }
            _ => {}
        }
        if self
            .composer
            .unsaved
            .is_some_and(|at| now >= at + SAVE_AFTER)
        {
            self.composer.drafts.load(&self.persist);
            self.write_drafts();
        }
        changed
    }

    /// When `composer_tick` next has something to do.
    pub(super) fn composer_deadline(&self) -> Option<Instant> {
        let voice = &self.composer.voice;
        let voice = voice.since.and_then(|since| match voice.phase {
            VoicePhase::Recording => Some(since + RECORD_FOR),
            VoicePhase::Transcribing => Some(since + TRANSCRIBE_WITHIN),
            VoicePhase::Idle => None,
        });
        let unsaved = self.composer.unsaved.map(|at| at + SAVE_AFTER);
        match (voice, unsaved) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Writes everything typed into the kept drafts (the app is closing).
    #[allow(dead_code)]
    pub fn flush_drafts(&mut self) {
        self.composer.drafts.load(&self.persist);
        self.write_drafts();
    }

    /// A key from Moon's keyboard (`keyboard show multiline`: Return is a
    /// line break). Returns whether the composer changed.
    pub fn composer_key(&mut self, input: KeyInput) -> Option<Vec<Value>> {
        if self.page != Page::Chat {
            return None;
        }
        let now = Instant::now();
        let mut cursor = self.composer.cursor;
        let changed = match input {
            KeyInput::Character(character) => {
                cursor.insert(&mut self.composer.text, character.encode_utf8(&mut [0; 4]))
            }
            KeyInput::Enter => cursor.insert(&mut self.composer.text, "\n"),
            KeyInput::Backspace => cursor.backspace(&mut self.composer.text),
        };
        self.composer.cursor = cursor;
        self.composer.selecting = false;
        if changed {
            self.composer.edited(now);
        } else {
            self.composer.touched(now);
        }
        Some(self.composer_changed())
    }

    /// Text from the clipboard, pasted at the caret (`Effect::Paste`'s
    /// answer).
    pub fn paste_text(&mut self, text: Option<String>) {
        match text.filter(|text| !text.is_empty()) {
            Some(text) => {
                let mut cursor = self.composer.cursor;
                if cursor.insert(&mut self.composer.text, &text) {
                    self.composer.cursor = cursor;
                    self.composer.edited(Instant::now());
                }
            }
            // No text: an image, if the clipboard holds one (PAR-242).
            None => self.paste_image(true),
        }
        let _ = self.composer_changed();
    }

    pub(super) fn handle_composer(&mut self, act: Act) -> Vec<Value> {
        let mut commands = self.handle_composer_act(act);
        commands.extend(self.composer_changed());
        commands
    }

    /// The picker follows what is typed and where the caret is: open when it
    /// has something to offer, closed when not. A path at the caret asks the
    /// daemon for its directory (the commands to send).
    pub(crate) fn composer_changed(&mut self) -> Vec<Value> {
        let mut commands = Vec::new();
        let caret = self.composer.cursor();
        if caret.selection().is_none()
            && self.ready
            && let Some(c) =
                super::super::commands::syntax::completion(&self.composer.text, caret.caret)
                    .filter(|c| c.path)
            && let Some((directory, prefix)) = self.composer.paths.wanted(&c.query, Instant::now())
        {
            let request = self.requests.issue(
                Owner::Composer,
                "paths",
                None,
                super::super::commands::paths::INFLIGHT,
                Instant::now(),
            );
            self.composer.paths.asked(
                request.clone(),
                directory.clone(),
                prefix.clone(),
                Instant::now(),
            );
            commands.push(serde_json::json!({
                "type": "list_path_directory",
                "reqId": request,
                "directory": directory,
                "prefix": prefix,
            }));
        }
        let state = self.picker();
        if state.is_none() {
            self.composer.picker_macros = false;
        }
        match (&state, self.composer.picker) {
            (Some(_), None) => {
                self.composer.serial += 1;
                let serial = self.composer.serial;
                self.composer.picker = Some(serial);
                self.ui
                    .open(Box::new(crate::exocortex::view::picker::Picker::new(
                        serial,
                    )));
            }
            (None, Some(_)) => self.composer.picker = None,
            _ => {}
        }
        self.composer.picker_state = state;
        self.composer.is_command = self.is_command_text();
        self.composer.highlights = if self.composer.text.contains('/') {
            super::super::commands::env::SlashEnv::new(self).highlights(&self.composer.text)
        } else {
            Vec::new()
        };
        commands
    }

    /// Text put back into draft `key` after a command refused it.
    pub(crate) fn restore_typed(&mut self, key: &str, text: &str) {
        self.restore_into(key, text);
    }

    fn handle_composer_act(&mut self, act: Act) -> Vec<Value> {
        let now = Instant::now();
        match act {
            Act::Send => {
                // Disabled until the daemon is ready (PAR-184, PAR-197).
                if !self.ready || !self.has_draft() {
                    Vec::new()
                } else if self.chat.id.is_some() && self.streaming() && !self.is_command_text() {
                    self.deliver_menu();
                    Vec::new()
                } else {
                    self.submit_prompt(None)
                }
            }
            Act::Deliver(timing) => self.submit_prompt(Some(timing)),
            Act::Abort => self.abort(),
            Act::Focus => {
                self.ui
                    .effect(Effect::ShowKeyboard(KeyboardPurpose::Multiline));
                self.composer.touched(now);
                Vec::new()
            }
            Act::Insert(text) => {
                let mut cursor = self.composer.cursor;
                if cursor.insert(&mut self.composer.text, &text) {
                    self.composer.cursor = cursor;
                    self.composer.edited(now);
                }
                Vec::new()
            }
            Act::RestoreDraft { conv, text } => {
                let key = conv.unwrap_or_else(|| self.draft_key());
                self.restore_into(&key, &text);
                Vec::new()
            }
            Act::Attach => {
                self.attach_menu();
                Vec::new()
            }
            Act::Browse(dir) => {
                self.browse(dir);
                Vec::new()
            }
            Act::BrowseHidden => {
                self.browse_hidden();
                Vec::new()
            }
            Act::AttachFile(path) => {
                self.attach_file(path);
                Vec::new()
            }
            Act::PasteImage => {
                self.paste_image(false);
                Vec::new()
            }
            Act::RemoveAttachment(id) => {
                self.remove_attachment(id);
                Vec::new()
            }
            Act::TrayScroll(to) => {
                self.composer.tray_scroll.set(to);
                Vec::new()
            }
            Act::Place { index, focused } => {
                let cursor = self.composer.cursor();
                let index = editor::snap(&self.composer.text, index);
                if !focused {
                    self.ui
                        .effect(Effect::ShowKeyboard(KeyboardPurpose::Multiline));
                    self.ui.haptic(Haptic::Light);
                }
                let on_selection = cursor
                    .selection()
                    .is_some_and(|(a, b)| index >= a && index <= b);
                let at_caret = cursor.selection().is_none() && index == cursor.caret;
                if focused && (on_selection || at_caret) {
                    // A second tap where the caret is, or on the selection:
                    // the menu, or away with it.
                    if self.composer.menu.is_some() {
                        self.composer.menu = None;
                    } else {
                        self.open_edit_menu();
                    }
                    return Vec::new();
                }
                self.composer.cursor = Cursor::at(index);
                self.composer.selecting = false;
                self.composer.touched(now);
                Vec::new()
            }
            Act::SelectWord(index) => {
                self.ui
                    .effect(Effect::ShowKeyboard(KeyboardPurpose::Multiline));
                self.composer.cursor = Cursor::word(&self.composer.text, index);
                self.composer.selecting = true;
                self.composer.touched(now);
                Vec::new()
            }
            Act::MoveEnd { end, index } => {
                let index = editor::snap(&self.composer.text, index);
                let mut cursor = self.composer.cursor();
                match (end, cursor.selection().is_some()) {
                    (End::Caret, false) if !self.composer.selecting => {
                        cursor = Cursor::at(index);
                    }
                    (End::Anchor, _) => cursor.anchor = index,
                    (End::Caret, _) => cursor.caret = index,
                }
                if cursor != self.composer.cursor {
                    self.composer.cursor = cursor;
                    self.composer.touched(now);
                }
                Vec::new()
            }
            Act::EndSelect => {
                self.composer.selecting = false;
                if self.composer.cursor().selection().is_some() {
                    self.open_edit_menu();
                }
                Vec::new()
            }
            Act::Scroll(to) => {
                self.composer.scroll.set(to);
                self.composer.reveal.set(false);
                Vec::new()
            }
            Act::Copy => {
                let selected = self
                    .composer
                    .cursor()
                    .selected(&self.composer.text)
                    .to_owned();
                if !selected.is_empty() {
                    self.ui.effect(Effect::Copy(selected));
                    self.ui.haptic(Haptic::Selection);
                }
                self.composer.menu = None;
                Vec::new()
            }
            Act::Cut => {
                let mut cursor = self.composer.cursor;
                if let Some(cut) = cursor.cut(&mut self.composer.text) {
                    self.composer.cursor = cursor;
                    self.ui.effect(Effect::Copy(cut));
                    self.ui.haptic(Haptic::Selection);
                    self.composer.edited(now);
                }
                self.composer.menu = None;
                Vec::new()
            }
            Act::Paste => {
                self.composer.menu = None;
                self.ui.effect(Effect::Paste);
                Vec::new()
            }
            Act::SelectAll => {
                self.composer.cursor = Cursor::select_all(&self.composer.text);
                self.composer.touched(now);
                self.open_edit_menu();
                Vec::new()
            }
            Act::Select => {
                let caret = self.composer.cursor().caret;
                self.composer.cursor = Cursor::word(&self.composer.text, caret);
                self.composer.touched(now);
                self.open_edit_menu();
                Vec::new()
            }
            Act::DismissError => {
                if let Some(key) = &self.composer.key {
                    self.composer.errors.remove(key);
                }
                Vec::new()
            }
            Act::Queued(id) => {
                self.queued_menu(&id);
                Vec::new()
            }
            Act::Unqueue(id) => self.unqueue(&id, false),
            Act::PullBack(id) => self.unqueue(&id, true),
            Act::Mic => {
                self.start_dictation(now);
                Vec::new()
            }
            Act::MicDone => {
                if self.composer.voice.phase == VoicePhase::Recording {
                    self.ui.effect(Effect::Dictate(Do::Stop));
                    self.composer.voice.phase = VoicePhase::Transcribing;
                    self.composer.voice.since = Some(now);
                }
                Vec::new()
            }
            Act::MicCancel => {
                self.cancel_dictation();
                Vec::new()
            }
            Act::Complete(insert) => {
                self.complete(&insert);
                Vec::new()
            }
            Act::PickerTab(macros) => {
                self.composer.picker_macros = macros;
                self.ui.haptic(Haptic::Selection);
                Vec::new()
            }
            Act::PickerDismiss => {
                self.composer.picker_dismissed = Some(self.composer.text.clone());
                self.ui.haptic(Haptic::Selection);
                Vec::new()
            }
            Act::QueueEdit(id) => {
                if let Some(q) = self.composer.find_queued(&id).cloned() {
                    let target = id.clone();
                    self.ui
                        .open(Box::new(sheet::prompt_sheet(sheet::Prompt::new(
                            "Edit queued message",
                            if q.command.is_some() {
                                "Runs its command when ready."
                            } else {
                                "It stays in the queue, delivered as before."
                            },
                            q.text.clone(),
                            "Save",
                            Box::new(move |text| {
                                Action::Composer(Act::QueueSave(target.clone(), text))
                            }),
                        ))));
                }
                Vec::new()
            }
            Act::QueueSave(id, text) => {
                let Some(q) = self.composer.find_queued(&id).cloned() else {
                    self.ui.error_toast(
                        "This entry was already delivered or removed, so the edit was not saved.",
                    );
                    return Vec::new();
                };
                if text.trim().is_empty() {
                    return Vec::new();
                }
                let timing = if q.source == "global-idle" {
                    "message-end".to_owned()
                } else {
                    q.timing.clone()
                };
                self.update_queued(&q.conv_id, &id, &text, &timing)
            }
            Act::QueueTiming(id, timing) => match self.composer.find_queued(&id).cloned() {
                Some(q) => self.update_queued(&q.conv_id, &id, &q.text, timing.wire()),
                None => Vec::new(),
            },
            Act::Rewind(stamp) => {
                match self.can_rewind(stamp) {
                    Ok(_) => self.ui.open(Box::new(super::super::ui::menu::Confirm::new(
                        "Edit sent message",
                        "Rewind removes this message and everything after it from canonical history. Your edited message returns to this chat’s draft; it is not sent automatically.",
                        "Rewind",
                        Action::Composer(Act::RewindConfirmed(stamp)),
                    ))),
                    Err(why) => self.ui.error_toast(why),
                }
                Vec::new()
            }
            Act::RewindConfirmed(stamp) => self.rewind(stamp),
            Act::EditPicker => {
                self.edit_picker();
                Vec::new()
            }
            Act::Discard => {
                if !self.composer.text.trim().is_empty() && !self.submitting() {
                    self.ui.open(Box::new(super::super::ui::menu::Confirm::new(
                        "Discard draft",
                        "Discard this chat’s unsent text?",
                        "Discard draft",
                        Action::Composer(Act::DiscardConfirmed),
                    )));
                }
                Vec::new()
            }
            Act::DiscardConfirmed => {
                self.take_draft();
                self.cancel_dictation();
                Vec::new()
            }
            Act::QueueMove(id, up) => {
                let request = self.requests.issue(
                    Owner::Composer,
                    "queue-edit",
                    None,
                    requests::DEFAULT_WITHIN,
                    now,
                );
                vec![serde_json::json!({
                    "type": "move_queued_message",
                    "reqId": request,
                    "queueId": id,
                    "direction": if up { "up" } else { "down" },
                })]
            }
        }
    }

    fn start_dictation(&mut self, now: Instant) {
        if self.composer.voice.active() || !dictation::available() {
            return;
        }
        let Some(key) = self.composer.key.clone() else {
            return;
        };
        self.composer.errors.remove(&key);
        self.composer.menu = None;
        self.composer.selecting = false;
        self.composer.voice = Voice {
            phase: VoicePhase::Recording,
            key: Some(key),
            since: Some(now),
            levels: VecDeque::new(),
            version: self.composer.voice.version.wrapping_add(1),
        };
        // The strip takes the editor's row: the keyboard goes.
        self.ui.effect(Effect::HideKeyboard);
        self.ui.effect(Effect::Dictate(Do::Start));
    }

    fn cancel_dictation(&mut self) {
        if self.composer.voice.active() {
            self.ui.effect(Effect::Dictate(Do::Cancel));
        }
        self.composer.voice.phase = VoicePhase::Idle;
        self.composer.voice.since = None;
    }

    /// The voice hook said something (`dictation::take`).
    pub fn dictation_line(&mut self, line: Line) {
        let voice = &mut self.composer.voice;
        let Some(key) = voice.key.clone() else {
            return;
        };
        match line {
            Line::Level(level) => {
                if voice.phase == VoicePhase::Recording {
                    voice.levels.push_back(level);
                    while voice.levels.len() > LEVELS {
                        voice.levels.pop_front();
                    }
                    voice.version = voice.version.wrapping_add(1);
                }
            }
            Line::Text(text) => {
                if voice.phase != VoicePhase::Transcribing {
                    return;
                }
                voice.phase = VoicePhase::Idle;
                let text = text.trim().to_owned();
                if text.is_empty() {
                    self.voice_error(&key, "No speech was detected in the recording.");
                    return;
                }
                self.insert_transcript(&key, &text);
            }
            Line::Error(message) => {
                if voice.active() {
                    voice.phase = VoicePhase::Idle;
                    self.voice_error(&key, &message);
                }
            }
            Line::Failed(why) => {
                if voice.active() {
                    voice.phase = VoicePhase::Idle;
                    eprintln!("exocortex: dictation did not start: {why}");
                    self.voice_error(&key, "The microphone recording could not start.");
                }
            }
            Line::Exited => {
                // Gone without its answer (it failed, or was cancelled).
                if voice.active() {
                    let recording = voice.phase == VoicePhase::Recording;
                    voice.phase = VoicePhase::Idle;
                    self.voice_error(
                        &key,
                        if recording {
                            "The recording stopped."
                        } else {
                            "The transcription did not come back. Try again."
                        },
                    );
                }
            }
        }
    }

    fn voice_error(&mut self, key: &str, message: &str) {
        self.composer.errors.insert(
            key.to_owned(),
            ComposerError {
                text: message.to_owned(),
                since: Instant::now(),
            },
        );
        if self.composer.key.as_deref() != Some(key) || self.page != Page::Chat {
            self.ui.error_toast(message.to_owned());
        }
    }

    /// Words dictated into draft `key`: at the caret when it shows (spaced
    /// from what is around them), else after the draft's text; the editor
    /// gets the keyboard back.
    fn insert_transcript(&mut self, key: &str, text: &str) {
        if self.composer.key.as_deref() == Some(key) {
            let cursor = self.composer.cursor();
            let (a, b) = cursor.selection().unwrap_or((cursor.caret, cursor.caret));
            let before = self.composer.text[..a].chars().next_back();
            let after = self.composer.text[b..].chars().next();
            let mut words = String::new();
            if before.is_some_and(|c| !c.is_whitespace()) {
                words.push(' ');
            }
            words.push_str(text);
            if after.is_some_and(|c| !c.is_whitespace()) {
                words.push(' ');
            }
            let mut cursor = self.composer.cursor;
            cursor.insert(&mut self.composer.text, &words);
            self.composer.cursor = cursor;
            self.composer.edited(Instant::now());
            let _ = self.composer_changed();
            if self.page == Page::Chat {
                self.ui
                    .effect(Effect::ShowKeyboard(KeyboardPurpose::Multiline));
            }
        } else {
            self.composer.drafts.load(&self.persist);
            let current = self.composer.drafts.get(key).unwrap_or_default().to_owned();
            let next = if current.is_empty() {
                text.to_owned()
            } else if current.ends_with(char::is_whitespace) {
                format!("{current}{text}")
            } else {
                format!("{current} {text}")
            };
            self.composer.drafts.set(key, &next, now_ms());
            self.composer.drafts.save(&mut self.persist);
        }
    }

    /// The edit menu over the caret or the selection (Cut, Copy, Paste,
    /// Select, Select all).
    fn open_edit_menu(&mut self) {
        self.composer.serial += 1;
        let serial = self.composer.serial;
        self.composer.menu = Some(serial);
        self.ui
            .open(Box::new(crate::exocortex::view::composer::EditMenu::new(
                serial,
            )));
    }

    /// "Deliver message" (PAR-183): the next turn (or Steer now), or the
    /// message's end.
    fn deliver_menu(&mut self) {
        if self.composer.text.trim().is_empty() {
            return;
        }
        let steer = self.provider() != "anthropic";
        let items = vec![
            ActionItem::new(
                if steer { "Steer now" } else { "Next turn" },
                Action::Composer(Act::Deliver(Timing::NextTurn)),
            )
            .icon(super::super::ui::icons::Icon::Bolt)
            .detail("Before the next tool round")
            .tone(Tone::Warning),
            ActionItem::new(
                "Message end",
                Action::Composer(Act::Deliver(Timing::MessageEnd)),
            )
            .icon(super::super::ui::icons::Icon::Clock)
            .detail("After this response finishes"),
        ];
        self.ui.open(Box::new(sheet::action_sheet(
            ActionList::new("Deliver message", items).scoped(),
        )));
    }

    /// A queued message's actions (PAR-259, PAR-261).
    fn queued_menu(&mut self, id: &str) {
        let Some(q) = self.composer.find_queued(id).cloned() else {
            return;
        };
        let provider = self
            .sidebar
            .conversations
            .get(&q.conv_id)
            .map(|s| s.provider.as_str());
        let label = queue_label(&q, provider);
        let optimistic = self.composer.optimistic(id);
        let mut items = Vec::new();
        let editable = !optimistic && q.automation.is_none();
        use super::super::ui::icons::Icon;
        if editable {
            items.push(
                ActionItem::new("Edit…", Action::Composer(Act::QueueEdit(id.to_owned())))
                    .icon(Icon::Pencil)
                    .detail(if q.command.is_some() {
                        "Arguments of the queued command"
                    } else {
                        "Its text"
                    }),
            );
        }
        if editable && q.source != "global-idle" && q.command.is_none() {
            let (label, timing, icon) = if q.timing == "next-turn" {
                ("Deliver at message end", Timing::MessageEnd, Icon::Clock)
            } else {
                (
                    if provider.is_none_or(|p| p == "anthropic") {
                        "Deliver at next turn"
                    } else {
                        "Steer now"
                    },
                    Timing::NextTurn,
                    Icon::Bolt,
                )
            };
            items.push(
                ActionItem::new(
                    label,
                    Action::Composer(Act::QueueTiming(id.to_owned(), timing)),
                )
                .icon(icon),
            );
        }
        if editable && q.command.is_none() {
            items.push(
                ActionItem::new(
                    "Edit in composer",
                    Action::Composer(Act::PullBack(id.to_owned())),
                )
                .icon(Icon::Rewind)
                .detail("Remove from the queue and put it back in the draft"),
            );
        }
        // Its neighbours in the daemon's order (one conversation's, or the
        // idle queue's).
        let peers: Vec<&Queued> = self
            .composer
            .queued
            .iter()
            .filter(|m| {
                if q.source == "global-idle" {
                    m.source == "global-idle"
                } else {
                    m.source != "global-idle" && m.conv_id == q.conv_id
                }
            })
            .collect();
        let at = peers.iter().position(|m| m.id == q.id);
        if !optimistic && let Some(at) = at {
            if at > 0 {
                items.push(
                    ActionItem::new(
                        "Move earlier",
                        Action::Composer(Act::QueueMove(id.to_owned(), true)),
                    )
                    .icon(Icon::ArrowUp),
                );
            }
            if at + 1 < peers.len() {
                items.push(
                    ActionItem::new(
                        "Move later",
                        Action::Composer(Act::QueueMove(id.to_owned(), false)),
                    )
                    .icon(Icon::ArrowDown),
                );
            }
        }
        items.push(
            ActionItem::new("Copy text", Action::Ui(UiAct::Copy(q.text.clone()))).icon(Icon::Copy),
        );
        items.push(
            ActionItem::new(
                "Remove from queue",
                Action::Composer(Act::Unqueue(id.to_owned())),
            )
            .icon(Icon::Trash)
            .tone(Tone::Destructive),
        );
        let subtitle = format!(
            "Delivers {}{}",
            label.trim_start_matches("queued: "),
            if optimistic {
                " · waiting for Exocortex…"
            } else {
                ""
            }
        );
        self.ui.open(Box::new(sheet::action_sheet(
            ActionList::new("Queued message", items)
                .subtitle(subtitle)
                .scoped(),
        )));
    }

    /// A rewind of the open conversation is on its way.
    pub fn rewinding(&self) -> bool {
        self.composer
            .rewinding
            .as_ref()
            .is_some_and(|(_, conv)| Some(conv) == self.chat.id.as_ref())
    }

    /// Whether the sent message with `stamp` can be rewound now
    /// (PAR-266): its user message index, or why not.
    pub fn can_rewind(&self, stamp: u64) -> Result<u64, String> {
        let Some(id) = &self.chat.id else {
            return Err("This message cannot safely be rewound.".into());
        };
        let Some(at) = self.chat.stamps.iter().position(|s| *s == stamp) else {
            return Err("This message cannot safely be rewound.".into());
        };
        let Some(Entry::User {
            images, metadata, ..
        }) = self.chat.entries.get(at)
        else {
            return Err("Only your own messages can be rewound.".into());
        };
        if metadata.as_ref().is_none_or(|m| m.started_at <= 0.0) {
            return Err(
                "Not synced with history yet. Reload the conversation, then try again.".into(),
            );
        }
        if images.iter().any(|image| image.base64.is_empty()) {
            return Err("Original image bytes are unavailable; rewind was not performed.".into());
        }
        if self.streaming() {
            return Err("Stop the response before rewinding.".into());
        }
        if self.chat_pending() || !self.chat.loaded {
            return Err("The conversation is still syncing.".into());
        }
        if self.composer.rewinding.is_some() {
            return Err("Finishing conversation rewind; try again in a moment.".into());
        }
        let base = self
            .composer
            .user_index
            .get(id)
            .and_then(|starts| starts.get(&self.chat.start))
            .ok_or("Not synced with history yet. Reload the conversation, then try again.")?;
        let before = self.chat.entries[..at]
            .iter()
            .filter(|e| matches!(e, Entry::User { .. }))
            .count() as u64;
        Ok(base + before)
    }

    /// PAR-267: the message's text goes into the draft (kept first), then
    /// the irreversible `unwind_conversation` goes out.
    fn rewind(&mut self, stamp: u64) -> Vec<Value> {
        let index = match self.can_rewind(stamp) {
            Ok(index) => index,
            Err(why) => {
                self.ui.error_toast(why);
                return Vec::new();
            }
        };
        let Some(id) = self.chat.id.clone() else {
            return Vec::new();
        };
        let at = self.chat.stamps.iter().position(|s| *s == stamp);
        let Some(Entry::User {
            text,
            images,
            metadata,
        }) = at.and_then(|at| self.chat.entries.get(at)).cloned()
        else {
            return Vec::new();
        };
        self.composer.key = Some(self.shown_draft_key());
        let (text, attachments) = attachments::editable(&text, &images);
        self.restore_into(&id, &text);
        self.restore_attachments(&id, attachments);
        self.composer.drafts.load(&self.persist);
        self.write_drafts();
        let request = self.requests.issue(
            Owner::Composer,
            "unwind",
            Some(&id),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.composer.rewinding = Some((request.clone(), id.clone()));
        let mut command = serde_json::json!({
            "type": "unwind_conversation",
            "reqId": request,
            "operationId": format!("phone-unwind-{}-{request}", now_ms()),
            "convId": id,
            "userMessageIndex": index,
        });
        if let Some(started) = metadata.map(|m| m.started_at) {
            command["expectedStartedAt"] = Value::from(started as u64);
        }
        vec![command]
    }

    /// "Edit…" (PAR-268): this conversation's queued messages (newest
    /// first) back into the composer, its last sent messages to rewind, and
    /// its instructions as an `/instructions` command to edit.
    fn edit_picker(&mut self) {
        use super::super::ui::icons::Icon;
        let Some(conv) = self.chat.id.clone() else {
            return;
        };
        let snippet = |text: &str| {
            let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if line.chars().count() > 60 {
                format!("{}…", line.chars().take(59).collect::<String>())
            } else {
                line
            }
        };
        let mut items = Vec::new();
        let provider = self
            .sidebar
            .conversations
            .get(&conv)
            .map(|s| s.provider.clone());
        let mut queued: Vec<Queued> = self.queued_here().into_iter().cloned().collect();
        queued.reverse();
        for q in queued
            .iter()
            .filter(|q| q.command.is_none() && q.automation.is_none())
        {
            items.push(
                ActionItem::new(
                    snippet(&q.text),
                    Action::Composer(Act::PullBack(q.id.clone())),
                )
                .icon(Icon::Pencil)
                .detail(format!(
                    "Queued · {} · back to the composer",
                    queue_label(q, provider.as_deref()).trim_start_matches("queued: ")
                )),
            );
        }
        let sent: Vec<(u64, String, Option<f64>)> = self
            .chat
            .entries
            .iter()
            .zip(&self.chat.stamps)
            .filter_map(|(entry, stamp)| match entry {
                Entry::User { text, metadata, .. }
                    if !metadata
                        .as_ref()
                        .is_some_and(|m| m.system || m.automation.is_some()) =>
                {
                    Some((
                        *stamp,
                        text.clone(),
                        metadata.as_ref().map(|m| m.started_at),
                    ))
                }
                _ => None,
            })
            .collect();
        for (stamp, text, started) in sent.iter().rev().take(12) {
            let (action, detail) = match self.can_rewind(*stamp) {
                Ok(_) => (
                    Action::Composer(Act::Rewind(*stamp)),
                    format!(
                        "Sent {} · rewind and edit",
                        started
                            .filter(|t| *t > 0.0)
                            .map(|t| crate::clock::local_date_time_text((t / 1000.0) as i64))
                            .unwrap_or_default()
                    ),
                ),
                Err(why) => (Action::Ui(UiAct::Toast(why.clone())), why),
            };
            items.push(
                ActionItem::new(snippet(text), action)
                    .icon(Icon::Rewind)
                    .detail(detail),
            );
        }
        let instructions = self.chat.entries.iter().find_map(|e| match e {
            Entry::SystemInstructions { text } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        });
        if let Some(text) = instructions {
            let empty = self.composer.text.trim().is_empty();
            let action = if empty {
                Action::Composer(Act::Insert(format!("/instructions {text}")))
            } else {
                Action::Ui(UiAct::Toast(
                    "Clear the draft first to edit the instructions.".into(),
                ))
            };
            items.push(
                ActionItem::new("System instructions", action)
                    .icon(Icon::Pencil)
                    .detail(if empty {
                        snippet(&text)
                    } else {
                        "Clear the draft first to edit the instructions".into()
                    }),
            );
        }
        if items.is_empty() {
            self.ui.toast("Nothing to edit in this chat yet.");
            return;
        }
        self.ui.open(Box::new(sheet::action_sheet(
            ActionList::new("Edit…", items).scoped(),
        )));
    }

    /// `update_queued_message`: its text and timing (PAR-260).
    fn update_queued(&mut self, conv: &str, id: &str, text: &str, timing: &str) -> Vec<Value> {
        let request = self.requests.issue(
            Owner::Composer,
            "queue-edit",
            Some(conv),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        vec![serde_json::json!({
            "type": "update_queued_message",
            "reqId": request,
            "queueId": id,
            "text": text,
            "timing": timing,
        })]
    }

    fn unqueue(&mut self, id: &str, pull_back: bool) -> Vec<Value> {
        if self.composer.removing.contains_key(id) || !self.ready {
            return Vec::new();
        }
        if self.composer.optimistic(id) {
            self.ui
                .toast("Exocortex has not confirmed this message yet.");
            return Vec::new();
        }
        let conv = self.composer.find_queued(id).map(|q| q.conv_id.clone());
        let request = self.requests.issue(
            Owner::Composer,
            "unqueue",
            conv.as_deref(),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.composer
            .removing
            .insert(id.to_owned(), (request.clone(), pull_back));
        self.composer.queue_errors.remove(id);
        self.chat.touch();
        vec![proto::unqueue_message(&request, id)]
    }

    /// Puts `text` back into draft `key` without losing what is there.
    fn restore_into(&mut self, key: &str, text: &str) {
        if self.composer.key.as_deref() == Some(key) {
            let current = std::mem::take(&mut self.composer.text);
            let merged = if current.trim().is_empty() || current == text {
                text.to_owned()
            } else {
                format!("{text}\n{current}")
            };
            self.composer.set_text(merged);
            self.composer.since = Some(Instant::now());
            self.composer.unsaved = Some(Instant::now());
        } else {
            self.composer.drafts.load(&self.persist);
            self.composer.drafts.restore(key, text, now_ms());
            self.composer.drafts.save(&mut self.persist);
        }
    }

    fn abort(&mut self) -> Vec<Value> {
        let Some(id) = self.chat.id.clone() else {
            return Vec::new();
        };
        if !self.connected {
            return Vec::new();
        }
        if self.composer.stopping.contains_key(&id) {
            self.ui
                .toast("Exocortex is already stopping this response.");
            return Vec::new();
        }
        let started = self.chat.live.as_ref().map(|live| live.started_at);
        let request = self.requests.issue(
            Owner::Composer,
            "abort",
            Some(&id),
            requests::ABORT_WITHIN,
            Instant::now(),
        );
        self.composer.stopping.insert(id.clone(), request.clone());
        vec![proto::abort(&request, &id, started)]
    }

    /// Sends what is typed: to the open conversation (queued at `timing`
    /// while it works, at the message's end by default), or as a new
    /// conversation's first message.
    /// Takes what is typed off the composer (it is on its way, or a
    /// command runs): the draft's key. A failure puts it back.
    pub(crate) fn take_draft(&mut self) -> String {
        let key = self.shown_draft_key();
        self.composer.set_text(String::new());
        self.composer.key = Some(key.clone());
        self.composer.errors.remove(&key);
        self.composer.drafts.load(&self.persist);
        if self.composer.drafts.remove(&key) {
            self.composer.drafts.save(&mut self.persist);
        }
        self.composer.unsaved = None;
        self.composer.menu = None;
        key
    }

    /// Sends `body` from draft `key` (`original` is what was typed, put back
    /// if it does not arrive): to the open conversation (queued at `timing`
    /// while it works, at the message's end by default), as a new
    /// conversation's first message, or onto the daemon's idle queue for
    /// `idle` (a `/queue` wait target).
    pub(crate) fn deliver(
        &mut self,
        key: String,
        body: String,
        original: String,
        timing: Option<Timing>,
        idle: Option<Value>,
    ) -> Vec<Value> {
        let now = Instant::now();
        let started_at = now_ms();
        let fresh_id = || {
            format!(
                "{started_at}-{:06x}",
                started_at.wrapping_mul(2654435761) & 0xff_ffff
            )
        };
        // The draft's attachments go along: documents as envelopes after
        // the text, images beside it.
        let attachments = self.take_attachments(&key);
        let text = if attachments.iter().any(|a| !a.is_image()) {
            attachments::resolve_text(&body, &attachments)
        } else {
            body
        };
        let images: Vec<Value> = attachments
            .iter()
            .filter_map(Attachment::wire_image)
            .collect();
        let shown: Vec<proto::ImageAttachment> = attachments
            .iter()
            .filter_map(Attachment::shown_image)
            .collect();
        let with_images = |mut command: Value, field: &[&str]| {
            if !images.is_empty() {
                let mut at = &mut command;
                for name in field {
                    at = &mut at[*name];
                }
                at["images"] = Value::from(images.clone());
            }
            command
        };
        if let Some(wait) = idle {
            // The daemon's idle queue (PAR-262): a new chat is created with
            // its entry, with the draft's model choices.
            let conv = self.chat.id.clone();
            let id = conv.clone().unwrap_or_else(fresh_id);
            let request = self.requests.issue(
                Owner::Composer,
                "queue",
                Some(&id),
                requests::DEFAULT_WITHIN,
                now,
            );
            let queue_id = format!("phone-{}", fresh_id());
            let mut command = with_images(
                proto::queue_message(&request, &queue_id, &id, &text, "message-end"),
                &[],
            );
            command["source"] = Value::from("global-idle");
            command["waitTarget"] = wait.clone();
            if conv.is_none() {
                command["target"] = Value::from("new-conversation");
                if let Some(folder) = self.current_folder() {
                    command["folderId"] = Value::from(folder);
                }
                self.new_conversation_fields(&mut command);
            } else {
                command["target"] = Value::from("conversation");
            }
            self.composer.pending.push((
                request.clone(),
                Queued {
                    id: queue_id.clone(),
                    conv_id: id.clone(),
                    text: text.clone(),
                    timing: "message-end".into(),
                    source: "global-idle".into(),
                    wait_target: Some(wait),
                    created_at: started_at as f64,
                    images: shown,
                    ..Queued::default()
                },
            ));
            self.composer.inflight.insert(
                request,
                Submission {
                    kind: Kind::Queue,
                    conv: id,
                    key,
                    text: original,
                    started_at,
                    queue_id: Some(queue_id),
                    attachments,
                },
            );
            self.chat.touch();
            return vec![command];
        }
        match self.chat.id.clone() {
            Some(id) if self.streaming() => {
                let timing = timing.unwrap_or(Timing::MessageEnd);
                let request = self.requests.issue(
                    Owner::Composer,
                    "queue",
                    Some(&id),
                    requests::DEFAULT_WITHIN,
                    now,
                );
                let queue_id = format!("phone-{}", fresh_id());
                self.composer.pending.push((
                    request.clone(),
                    Queued {
                        id: queue_id.clone(),
                        conv_id: id.clone(),
                        text: text.clone(),
                        timing: timing.wire().into(),
                        source: "daemon".into(),
                        created_at: started_at as f64,
                        images: shown,
                        ..Queued::default()
                    },
                ));
                self.composer.inflight.insert(
                    request.clone(),
                    Submission {
                        kind: Kind::Queue,
                        conv: id.clone(),
                        key,
                        text: original,
                        started_at,
                        queue_id: Some(queue_id.clone()),
                        attachments,
                    },
                );
                self.chat.touch();
                vec![with_images(
                    proto::queue_message(&request, &queue_id, &id, &text, timing.wire()),
                    &[],
                )]
            }
            Some(id) => {
                let request = self.requests.issue(
                    Owner::Composer,
                    "send",
                    Some(&id),
                    requests::DEFAULT_WITHIN,
                    now,
                );
                self.chat.echoes.push(Echo {
                    text: text.clone(),
                    started_at,
                });
                self.chat.touch();
                self.composer.inflight.insert(
                    request.clone(),
                    Submission {
                        kind: Kind::Send,
                        conv: id.clone(),
                        key,
                        text: original,
                        started_at,
                        queue_id: None,
                        attachments,
                    },
                );
                vec![with_images(
                    proto::send_message(&request, &id, &text, started_at),
                    &[],
                )]
            }
            None => {
                let id = fresh_id();
                let request = self.requests.issue(
                    Owner::Composer,
                    "new",
                    Some(&id),
                    requests::DEFAULT_WITHIN,
                    now,
                );
                // Only what the user chose for this draft: the daemon applies
                // its defaults to the rest (E's overrides, PAR-277).
                let mut command = with_images(
                    proto::new_conversation(
                        &request,
                        &id,
                        None,
                        self.current_folder(),
                        &text,
                        started_at,
                    ),
                    &["initialMessage"],
                );
                self.new_conversation_fields(&mut command);
                self.composer.inflight.insert(
                    request,
                    Submission {
                        kind: Kind::New,
                        conv: id.clone(),
                        key,
                        text: original,
                        started_at,
                        queue_id: None,
                        attachments,
                    },
                );
                self.chat = Chat {
                    id: Some(id.clone()),
                    loaded: true,
                    ..Chat::default()
                };
                self.composer.key = Some(id);
                // The daemon echoes no user_message for a conversation's
                // first message: it is the conversation's first entry.
                self.chat.push(Entry::User {
                    text,
                    images: shown,
                    metadata: None,
                });
                self.chat.live = Some(Live {
                    started_at: started_at as f64,
                    ..Live::default()
                });
                vec![command]
            }
        }
    }

    /// Messages waiting to join the open conversation, in the TUI's order.
    pub fn queued_here(&self) -> Vec<&Queued> {
        let mut queued: Vec<&Queued> = self
            .composer
            .all_queued()
            .filter(|q| Some(q.conv_id.as_str()) == self.chat.id.as_deref())
            .collect();
        queued.sort_by_key(|q| queue_order(q));
        queued
    }

    /// The caption under a queued bubble: when it is delivered, or that
    /// it is being removed, or why it was not.
    pub fn queued_caption(&self, q: &Queued) -> String {
        if self.composer.removing(&q.id) {
            return "Removing…".into();
        }
        if let Some(error) = self.composer.queue_error(&q.id) {
            return error.to_owned();
        }
        let provider = self
            .sidebar
            .conversations
            .get(&q.conv_id)
            .map(|s| s.provider.as_str());
        queue_label(q, provider)
    }

    /// A message from here arrived: no longer on its way.
    fn arrived(&mut self, request: &str) {
        if let Some(submission) = self.composer.inflight.remove(request)
            && let Some(queue_id) = submission.queue_id
        {
            self.composer.pending.retain(|(_, q)| q.id != queue_id);
        }
        self.requests.forget(request);
    }

    pub(super) fn apply_composer(&mut self, event: &Event, _now: Instant) -> Vec<Value> {
        match event {
            Event::QueueUpdated {
                messages,
                settled_queue_ids,
            } => {
                self.composer.queued = messages.clone();
                // Queued from here: confirmed once the daemon lists or
                // settles it.
                let confirmed: Vec<String> = self
                    .composer
                    .pending
                    .iter()
                    .filter(|(_, q)| {
                        messages.iter().any(|m| m.id == q.id) || settled_queue_ids.contains(&q.id)
                    })
                    .map(|(request, _)| request.clone())
                    .collect();
                for request in confirmed {
                    self.arrived(&request);
                }
                // Removed: gone from the queue it was settled in.
                let removed: Vec<String> = self
                    .composer
                    .removing
                    .keys()
                    .filter(|id| {
                        settled_queue_ids.contains(id) && !messages.iter().any(|m| &m.id == *id)
                    })
                    .cloned()
                    .collect();
                for id in removed {
                    self.unqueued(&id);
                }
                self.chat.touch();
            }
            Event::Ack {
                req_id: Some(req_id),
                ..
            } => {
                if self.composer.inflight.contains_key(req_id) {
                    self.arrived(req_id);
                    self.chat.touch();
                }
                let removed = self
                    .composer
                    .removing
                    .iter()
                    .find(|(_, (request, _))| request == req_id)
                    .map(|(id, _)| id.clone());
                if let Some(id) = removed {
                    self.unqueued(&id);
                    self.chat.touch();
                }
                self.composer
                    .stopping
                    .retain(|_, request| request != req_id);
            }
            Event::ConversationCreated {
                req_id: Some(req_id),
                ..
            } => {
                if self.composer.inflight.contains_key(req_id) {
                    self.arrived(req_id);
                }
            }
            // A send is accepted by its echo or by the turn it starts.
            Event::UserMessage {
                conv_id,
                started_at: Some(started_at),
                ..
            } => self.echoed(conv_id, *started_at),
            Event::StreamingStarted {
                conv_id,
                started_at,
                ..
            } => self.echoed(conv_id, *started_at),
            Event::MessageComplete { conv_id, .. } | Event::StreamingStopped { conv_id, .. } => {
                self.composer.stopping.remove(conv_id);
            }
            Event::ConversationLoaded {
                conv_id, window, ..
            }
            | Event::ConversationHistoryLoaded {
                conv_id, window, ..
            }
            | Event::HistoryUpdated { conv_id, window } => {
                if let (Some(start), Some(users)) =
                    (window.history_start_index, window.history_start_user_index)
                {
                    self.composer
                        .user_index
                        .entry(conv_id.clone())
                        .or_default()
                        .insert(start, users);
                }
            }
            Event::ConversationUnwound { conv_id, .. } => {
                self.composer.user_index.remove(conv_id);
                if self
                    .composer
                    .rewinding
                    .as_ref()
                    .is_some_and(|(_, conv)| conv == conv_id)
                {
                    self.composer.rewinding = None;
                }
            }
            Event::Pong {
                req_id: Some(req_id),
                ..
            } if self
                .composer
                .ping
                .as_ref()
                .is_some_and(|(id, _)| id == req_id) =>
            {
                if let Some((_, at)) = self.composer.ping.take() {
                    let ms = at.elapsed().as_millis();
                    self.notice(
                        &format!("Daemon round trip: {ms} ms"),
                        super::super::commands::run::Level::Info,
                        None,
                    );
                }
            }
            Event::PathDirectoryEntries(e) => {
                if let Some(id) = &e.req_id
                    && self.composer.paths.is_ours(id)
                {
                    self.composer
                        .paths
                        .answered(id, Some(e.listings.clone()), Instant::now());
                    return self.composer_changed();
                }
            }
            Event::ToolsAvailable {
                macro_environment: Some(env),
                ..
            } => {
                self.composer.macro_env = super::super::macros::MacroEnv::from_json(env);
            }
            Event::QueueNotice(notice) => {
                // PAR-263: said above the composer of its conversation.
                if let Some(conv) = &notice.conv_id {
                    self.composer.errors.insert(
                        conv.clone(),
                        ComposerError {
                            text: notice.message.clone(),
                            since: Instant::now(),
                        },
                    );
                    if self.chat.id.as_ref() != Some(conv) || self.page != Page::Chat {
                        self.ui.error_toast(notice.message.clone());
                    }
                } else {
                    self.ui.error_toast(notice.message.clone());
                }
            }
            Event::ConversationDeleted { conv_id, .. } => {
                self.composer.drafts.load(&self.persist);
                if self.composer.drafts.remove(conv_id) {
                    self.composer.drafts.save(&mut self.persist);
                }
                if self.composer.key.as_ref() == Some(conv_id) {
                    self.composer.set_text(String::new());
                    self.composer.key = None;
                    self.composer.unsaved = None;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn echoed(&mut self, conv: &str, started_at: f64) {
        let started = started_at as u64;
        let request = self
            .composer
            .inflight
            .iter()
            .find(|(_, s)| s.kind == Kind::Send && s.conv == conv && s.started_at == started)
            .map(|(request, _)| request.clone());
        if let Some(request) = request {
            self.arrived(&request);
        }
    }

    /// The daemon removed queued message `id` as asked.
    fn unqueued(&mut self, id: &str) {
        let Some((request, pull_back)) = self.composer.removing.remove(id) else {
            return;
        };
        self.requests.forget(&request);
        let removed = self.composer.find_queued(id).cloned();
        self.composer.queued.retain(|q| q.id != id);
        if let Some(q) = removed
            && pull_back
        {
            let (text, attachments) = attachments::editable(&q.text, &q.images);
            self.restore_into(&q.conv_id, &text);
            self.restore_attachments(&q.conv_id, attachments);
        }
    }

    /// A message did not arrive: its text back into the draft it came from,
    /// and why, there.
    fn fail_send(&mut self, mut submission: Submission, why: &str) {
        let message = match submission.kind {
            Kind::Send => format!("Not sent: {why}"),
            Kind::Queue => format!("Not queued: {why}"),
            Kind::New => format!("Could not start the chat: {why}"),
        };
        match submission.kind {
            Kind::Send => {
                if self.chat.id.as_deref() == Some(submission.conv.as_str()) {
                    self.chat
                        .echoes
                        .retain(|echo| echo.started_at != submission.started_at);
                    self.chat.touch();
                }
            }
            Kind::Queue => {
                if let Some(queue_id) = &submission.queue_id {
                    self.composer.pending.retain(|(_, q)| &q.id != queue_id);
                }
                self.chat.touch();
            }
            Kind::New => {
                // Back to the new chat it was written in, if still there.
                if self.chat.id.as_deref() == Some(submission.conv.as_str()) {
                    self.chat = Chat {
                        loaded: true,
                        ..Chat::default()
                    };
                    self.composer.key = Some(self.shown_draft_key());
                }
            }
        }
        self.restore_into(&submission.key, &submission.text);
        let attachments = std::mem::take(&mut submission.attachments);
        self.restore_attachments(&submission.key, attachments);
        let shown = self.page == Page::Chat
            && self.composer.key.as_deref() == Some(submission.key.as_str());
        self.composer.errors.insert(
            submission.key,
            ComposerError {
                text: message.clone(),
                since: Instant::now(),
            },
        );
        if !shown {
            self.ui.error_toast(message);
        }
    }

    /// The connection dropped: nothing on its way will be answered.
    pub(super) fn composer_disconnected(&mut self) {
        let inflight: Vec<Submission> = self.composer.inflight.drain().map(|(_, s)| s).collect();
        for submission in inflight {
            self.fail_send(
                submission,
                "the connection dropped before Exocortex confirmed it.",
            );
        }
        self.composer.pending.clear();
        self.composer.removing.clear();
        self.composer.stopping.clear();
    }

    /// An error answering one of the composer's requests; true when
    /// handled.
    pub(super) fn composer_error(&mut self, id: &str, message: &str) -> bool {
        // An old daemon's "Unknown command": no listing, never the phone's
        // own files.
        if self.composer.paths.is_ours(id) {
            self.composer.paths.answered(id, None, Instant::now());
            let _ = self.composer_changed();
            return true;
        }
        if let Some(submission) = self.composer.inflight.remove(id) {
            self.fail_send(submission, message);
            return true;
        }
        let refused = self
            .composer
            .removing
            .iter()
            .find(|(_, (request, _))| request == id)
            .map(|(queue_id, _)| queue_id.clone());
        if let Some(queue_id) = refused {
            self.composer.removing.remove(&queue_id);
            self.composer
                .queue_errors
                .insert(queue_id, message.to_owned());
            self.ui.error_toast(format!("Not removed: {message}"));
            self.chat.touch();
            return true;
        }
        self.composer.stopping.retain(|_, request| request != id);
        if self
            .composer
            .rewinding
            .as_ref()
            .is_some_and(|(request, _)| request == id)
        {
            self.composer.rewinding = None;
            self.ui.error_toast(format!("Not rewound: {message}"));
            return true;
        }
        if message.contains("not found") {
            self.ui.error_toast(
                "This entry was already delivered or removed, so the change was not saved.",
            );
            return true;
        }
        false
    }

    /// One of the composer's requests was not answered in time.
    pub(super) fn composer_timeout(&mut self, id: &str, pending: &Pending) -> bool {
        match pending.kind {
            "send" | "new" | "queue" => {
                if let Some(submission) = self.composer.inflight.remove(id) {
                    self.fail_send(submission, "Exocortex did not confirm it in time.");
                }
                true
            }
            "abort" => {
                self.composer.stopping.retain(|_, request| request != id);
                self.ui
                    .error_toast("Stop did not reach Exocortex. Try again.");
                true
            }
            "unqueue" | "unwind" => self.composer_error(id, "Exocortex did not answer. Try again."),
            // A listing that did not come: the picker keeps what it had.
            "paths" => false,
            _ => self.generic_timeout(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::files::{Done, Job};
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::store::DRAFTS;

    fn summary(id: &str, streaming: bool) -> String {
        format!(
            r#"{{"type":"conversation_updated","summary":{{"id":"{id}","provider":"anthropic","model":"m","effort":"low","title":"{id}","pinned":false,"sortOrder":0,"streaming":{streaming},"unread":false}}}}"#
        )
    }

    /// Ready, with conversations `c` and `d` (`c` working when `streaming`),
    /// `c` open.
    fn ready(streaming: bool) -> Model {
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
        model.apply(parse_event(&summary("c", streaming)), now);
        model.apply(parse_event(&summary("d", false)), now);
        open(&mut model, "c");
        model
    }

    fn open(model: &mut Model, id: &str) {
        let commands = model.handle(Action::Nav(Nav::OpenConversation(id.into())));
        let load = commands
            .iter()
            .find(|c| c["type"] == "load_conversation")
            .unwrap();
        let request = load["reqId"].as_str().unwrap().to_owned();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"{id}","entries":[{{"type":"user","text":"q"}},{{"type":"ai","blocks":[{{"type":"text","text":"a"}}]}}],"historyStartIndex":0}}"#
            )),
            Instant::now(),
        );
    }

    fn type_text(model: &mut Model, text: &str) {
        for character in text.chars() {
            model.composer_key(if character == '\n' {
                KeyInput::Enter
            } else {
                KeyInput::Character(character)
            });
        }
    }

    fn opened(model: &mut Model) -> Vec<String> {
        model
            .ui
            .opening
            .drain(..)
            .map(|overlay| overlay.label())
            .collect()
    }

    fn apply(model: &mut Model, line: &str) -> Vec<Value> {
        model.apply(parse_event(line), Instant::now())
    }

    #[test]
    fn typing_return_and_backspace_edit_at_the_caret() {
        let mut model = ready(false);
        type_text(&mut model, "helo\nworld");
        assert_eq!(model.composer.text, "helo\nworld", "Return is a line break");
        model.handle(Action::Composer(Act::Place {
            index: 3,
            focused: true,
        }));
        type_text(&mut model, "l");
        assert_eq!(model.composer.text, "hello\nworld");
        model.composer_key(KeyInput::Backspace);
        model.composer_key(KeyInput::Backspace);
        assert_eq!(model.composer.text, "heo\nworld");
        // A second tap at the caret opens the edit menu; Select all, Cut.
        let caret = model.composer.cursor().caret;
        model.handle(Action::Composer(Act::Place {
            index: caret,
            focused: true,
        }));
        assert_eq!(opened(&mut model), vec!["edit menu: "]);
        model.handle(Action::Composer(Act::SelectAll));
        model.handle(Action::Composer(Act::Cut));
        assert!(model.composer.text.is_empty());
        assert!(
            model
                .ui
                .effects
                .contains(&Effect::Copy("heo\nworld".into()))
        );
        model.ui.effects.clear();
        model.handle(Action::Composer(Act::Paste));
        assert_eq!(model.ui.effects, vec![Effect::Paste]);
        model.paste_text(Some("pasted\r\ntext".into()));
        assert_eq!(model.composer.text, "pasted\ntext");
        // No text on the clipboard: its image, if any (PAR-242).
        model.ui.effects.clear();
        model.paste_text(None);
        assert_eq!(
            model.ui.effects,
            vec![Effect::Files(Job::Paste {
                key: "c".into(),
                name: "Pasted image 1".into(),
                menu: true,
            })]
        );
        model.files_done(Done::Prepared {
            key: "c".into(),
            result: Err(crate::exocortex::files::NO_IMAGE.into()),
        });
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some(crate::exocortex::files::NO_IMAGE)
        );
        assert!(model.shown_attachments() == (&[][..], 0));
    }

    #[test]
    fn a_send_is_confirmed_by_its_echo() {
        let mut model = ready(false);
        type_text(&mut model, "Hello");
        let sent = model.handle(Action::Composer(Act::Send));
        assert_eq!(sent[0]["type"], "send_message");
        assert!(model.composer.text.is_empty() && model.submitting());
        let started = sent[0]["startedAt"].as_u64().unwrap();
        apply(
            &mut model,
            &format!(
                r#"{{"type":"user_message","convId":"c","text":"Hello","startedAt":{started}}}"#
            ),
        );
        assert!(!model.submitting());
        assert!(model.requests.is_empty(), "nothing waited on");
    }

    #[test]
    fn a_refused_send_goes_back_into_its_draft_after_navigating_away() {
        let mut model = ready(false);
        type_text(&mut model, "Hello");
        let sent = model.handle(Action::Composer(Act::Send));
        let request = sent[0]["reqId"].as_str().unwrap().to_owned();
        assert_eq!(model.chat.echoes.len(), 1);
        open(&mut model, "d");
        type_text(&mut model, "other");
        opened(&mut model);
        apply(
            &mut model,
            &format!(
                r#"{{"type":"error","reqId":"{request}","convId":"c","message":"Already streaming"}}"#
            ),
        );
        // Said where the user is; the text waits in c's draft.
        assert_eq!(opened(&mut model), vec!["Not sent: Already streaming"]);
        assert_eq!(model.composer.text, "other");
        open(&mut model, "c");
        assert_eq!(model.composer.text, "Hello");
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some("Not sent: Already streaming")
        );
        assert!(model.chat.echoes.is_empty());
        // Typing clears the error.
        type_text(&mut model, "!");
        assert!(model.composer_error_label().is_none());
    }

    #[test]
    fn an_unconfirmed_send_is_restored_when_its_deadline_passes() {
        let mut model = ready(false);
        type_text(&mut model, "Hello");
        model.handle(Action::Composer(Act::Send));
        type_text(&mut model, "typed since");
        model.expire(Instant::now() + requests::DEFAULT_WITHIN + Duration::from_secs(1));
        assert_eq!(model.composer.text, "Hello\ntyped since");
        assert!(
            model
                .composer_error_label()
                .unwrap()
                .text
                .starts_with("Not sent:")
        );
    }

    #[test]
    fn a_failed_new_chat_comes_back_as_its_folders_draft() {
        let mut model = ready(false);
        model.sidebar.draft_folder = Some("f1".into());
        model.handle(Action::Nav(Nav::NewConversation));
        assert_eq!(model.composer.key.as_deref(), Some("draft:f1"));
        type_text(&mut model, "First message");
        let sent = model.handle(Action::Composer(Act::Send));
        assert_eq!(sent[0]["type"], "new_conversation");
        assert_eq!(sent[0]["folderId"], "f1");
        // A blank draft: the daemon's defaults (PAR-277).
        for key in ["provider", "model", "effort"] {
            assert!(sent[0].get(key).is_none(), "{key}");
        }
        let request = sent[0]["reqId"].as_str().unwrap().to_owned();
        assert!(model.chat.id.is_some());
        apply(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{request}","message":"Not authenticated"}}"#),
        );
        assert!(model.chat.id.is_none(), "back to the new chat");
        assert_eq!(model.composer.text, "First message");
        assert_eq!(model.composer.key.as_deref(), Some("draft:f1"));
        assert!(model.composer_error_label().is_some());
    }

    #[test]
    fn deliver_message_queues_at_the_chosen_timing_or_sends_once_the_turn_ended() {
        let mut model = ready(true);
        type_text(&mut model, "steer");
        assert!(model.handle(Action::Composer(Act::Send)).is_empty());
        assert_eq!(opened(&mut model), vec!["Deliver message"]);
        let queued = model.handle(Action::Composer(Act::Deliver(Timing::NextTurn)));
        assert_eq!(queued[0]["type"], "queue_message");
        assert_eq!(queued[0]["timing"], "next-turn");
        let id = queued[0]["queueId"].as_str().unwrap().to_owned();
        // Shown at once, confirmed by the daemon's queue.
        let shown = model.queued_here();
        assert_eq!(shown.len(), 1);
        assert!(model.composer.optimistic(&id));
        assert_eq!(model.queued_caption(shown[0]), "queued: next turn");
        apply(
            &mut model,
            &format!(
                r#"{{"type":"queue_updated","messages":[{{"id":"{id}","convId":"c","text":"steer","timing":"next-turn","source":"daemon"}}],"settledQueueIds":["{id}"]}}"#
            ),
        );
        assert!(!model.composer.optimistic(&id));
        assert_eq!(model.queued_here().len(), 1);
        // Message end.
        type_text(&mut model, "later");
        model.handle(Action::Composer(Act::Send));
        let queued = model.handle(Action::Composer(Act::Deliver(Timing::MessageEnd)));
        assert_eq!(queued[0]["timing"], "message-end");
        // One message of a draft at a time: the next waits for the ack.
        type_text(&mut model, "more");
        assert!(
            model
                .handle(Action::Composer(Act::Deliver(Timing::MessageEnd)))
                .is_empty()
        );
        let request = queued[0]["reqId"].as_str().unwrap();
        apply(
            &mut model,
            &format!(r#"{{"type":"ack","reqId":"{request}"}}"#),
        );
        model.composer.text.clear();
        // The turn ends while the choice is open: sent at once (PAR-257).
        type_text(&mut model, "after");
        model.handle(Action::Composer(Act::Send));
        apply(&mut model, &summary("c", false));
        let sent = model.handle(Action::Composer(Act::Deliver(Timing::NextTurn)));
        assert_eq!(sent[0]["type"], "send_message");
    }

    #[test]
    fn a_refused_queue_entry_goes_back_into_the_draft() {
        let mut model = ready(true);
        type_text(&mut model, "later");
        model.handle(Action::Composer(Act::Send));
        let queued = model.handle(Action::Composer(Act::Deliver(Timing::MessageEnd)));
        let request = queued[0]["reqId"].as_str().unwrap().to_owned();
        apply(
            &mut model,
            &format!(
                r#"{{"type":"error","reqId":"{request}","convId":"c","message":"Invalid queue timing"}}"#
            ),
        );
        assert!(model.queued_here().is_empty());
        assert_eq!(model.composer.text, "later");
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some("Not queued: Invalid queue timing")
        );
    }

    #[test]
    fn a_removed_queue_entry_stays_until_the_daemon_says_so() {
        let mut model = ready(true);
        let entry = r#"{"id":"q1","convId":"c","text":"queued text","timing":"message-end","source":"daemon"}"#;
        apply(
            &mut model,
            &format!(r#"{{"type":"queue_updated","messages":[{entry}]}}"#),
        );
        model.handle(Action::Composer(Act::Queued("q1".into())));
        assert_eq!(opened(&mut model), vec!["Queued message"]);
        let remove = model.handle(Action::Composer(Act::Unqueue("q1".into())));
        assert_eq!(remove[0]["type"], "unqueue_message");
        assert_eq!(remove[0]["queueId"], "q1");
        assert_eq!(model.queued_caption(model.queued_here()[0]), "Removing…");
        assert!(
            model
                .handle(Action::Composer(Act::Unqueue("q1".into())))
                .is_empty(),
            "once"
        );
        apply(
            &mut model,
            r#"{"type":"queue_updated","messages":[],"settledQueueIds":["q1"]}"#,
        );
        assert!(model.queued_here().is_empty());
        // Refused: it stays, saying why.
        apply(
            &mut model,
            &format!(r#"{{"type":"queue_updated","messages":[{entry}]}}"#),
        );
        let remove = model.handle(Action::Composer(Act::Unqueue("q1".into())));
        let request = remove[0]["reqId"].as_str().unwrap().to_owned();
        apply(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{request}","message":"Queue locked"}}"#),
        );
        assert_eq!(model.queued_caption(model.queued_here()[0]), "Queue locked");
        // Pulled back: the text returns to the conversation's draft.
        let pull = model.handle(Action::Composer(Act::PullBack("q1".into())));
        let request = pull[0]["reqId"].as_str().unwrap().to_owned();
        apply(
            &mut model,
            &format!(r#"{{"type":"ack","reqId":"{request}"}}"#),
        );
        assert!(model.queued_here().is_empty());
        assert_eq!(model.composer.text, "queued text");
    }

    #[test]
    fn stop_goes_once_and_works_before_the_daemon_is_ready() {
        let mut model = ready(true);
        let stop = model.handle(Action::Composer(Act::Abort));
        assert_eq!(stop[0]["type"], "abort");
        assert!(model.stopping());
        opened(&mut model);
        assert!(model.handle(Action::Composer(Act::Abort)).is_empty());
        assert_eq!(
            opened(&mut model),
            vec!["Exocortex is already stopping this response."]
        );
        apply(&mut model, r#"{"type":"streaming_stopped","convId":"c"}"#);
        assert!(!model.stopping());
        model.ready = false;
        assert_eq!(
            model.handle(Action::Composer(Act::Abort))[0]["type"],
            "abort"
        );
    }

    #[test]
    fn drafts_follow_their_conversations_and_are_kept() {
        let mut model = ready(false);
        type_text(&mut model, "for c");
        open(&mut model, "d");
        assert!(model.composer.text.is_empty());
        type_text(&mut model, "for d");
        model.handle(Action::Nav(Nav::Back));
        model.handle(Action::Nav(Nav::NewConversation));
        assert!(model.composer.text.is_empty(), "a new chat's own draft");
        open(&mut model, "c");
        assert_eq!(model.composer.text, "for c");
        open(&mut model, "d");
        assert_eq!(model.composer.text, "for d");
        // Written into drafts.json once typing pauses.
        type_text(&mut model, "!");
        let due = model.answer_deadline().unwrap();
        model.tick(due);
        let kept: serde_json::Map<String, Value> = model.persist.section(DRAFTS, "drafts");
        assert_eq!(kept["c"]["text"], "for c");
        assert_eq!(kept["d"]["text"], "for d!");
        // Sent: gone from the kept drafts.
        model.handle(Action::Composer(Act::Send));
        let kept: serde_json::Map<String, Value> = model.persist.section(DRAFTS, "drafts");
        assert!(!kept.contains_key("d"));
    }

    #[test]
    fn dictation_records_transcribes_and_inserts_at_the_caret() {
        let mut model = ready(false);
        type_text(&mut model, "Hello world");
        model.handle(Action::Composer(Act::Place {
            index: 5,
            focused: true,
        }));
        model.ui.effects.clear();
        model.handle(Action::Composer(Act::Mic));
        assert_eq!(model.composer.voice.phase, VoicePhase::Recording);
        assert_eq!(
            model.ui.effects,
            vec![Effect::HideKeyboard, Effect::Dictate(Do::Start)]
        );
        for level in [0.0, 0.4, 0.9] {
            model.dictation_line(Line::Level(level));
        }
        assert_eq!(model.composer.voice.levels, [0.0, 0.4, 0.9]);
        model.ui.effects.clear();
        model.handle(Action::Composer(Act::MicDone));
        assert_eq!(model.ui.effects, vec![Effect::Dictate(Do::Stop)]);
        assert_eq!(model.composer.voice.phase, VoicePhase::Transcribing);
        model.dictation_line(Line::Level(0.5));
        assert_eq!(model.composer.voice.levels.len(), 3, "no levels after stop");
        model.dictation_line(Line::Text("  dear  ".into()));
        assert_eq!(model.composer.text, "Hello dear world");
        assert_eq!(model.composer.voice.phase, VoicePhase::Idle);
        assert!(
            model
                .ui
                .effects
                .contains(&Effect::ShowKeyboard(KeyboardPurpose::Multiline))
        );
        // Its exit after the answer says nothing more.
        model.dictation_line(Line::Exited);
        assert!(model.composer_error_label().is_none());
    }

    #[test]
    fn dictation_problems_are_said_and_nothing_is_lost() {
        let mut model = ready(false);
        type_text(&mut model, "keep me");
        model.handle(Action::Composer(Act::Mic));
        model.handle(Action::Composer(Act::MicDone));
        model.dictation_line(Line::Text(String::new()));
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some("No speech was detected in the recording.")
        );
        assert_eq!(model.composer.text, "keep me");
        model.handle(Action::Composer(Act::Mic));
        assert!(
            model.composer_error_label().is_none(),
            "a new try clears it"
        );
        model.handle(Action::Composer(Act::MicDone));
        model.dictation_line(Line::Error("Transcription failed: quota".into()));
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some("Transcription failed: quota")
        );
        // The hook dying mid-recording, and a transcription that never comes.
        model.handle(Action::Composer(Act::Mic));
        model.dictation_line(Line::Exited);
        assert_eq!(model.composer.voice.phase, VoicePhase::Idle);
        assert!(model.composer_error_label().is_some());
        model.handle(Action::Composer(Act::Mic));
        model.handle(Action::Composer(Act::MicDone));
        let since = model.composer.voice.since.unwrap();
        model.ui.effects.clear();
        model.tick(since + TRANSCRIBE_WITHIN);
        assert_eq!(model.ui.effects, vec![Effect::Dictate(Do::Cancel)]);
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some("Voice transcription took too long. Try again.")
        );
        // A late answer after that is no one's.
        model.dictation_line(Line::Text("late".into()));
        assert_eq!(model.composer.text, "keep me");
    }

    #[test]
    fn a_recording_ends_with_its_chat_and_a_transcript_finds_its_draft() {
        let mut model = ready(false);
        model.handle(Action::Composer(Act::Mic));
        open(&mut model, "d");
        model.ui.effects.clear();
        model.tick(Instant::now());
        assert_eq!(model.ui.effects, vec![Effect::Dictate(Do::Cancel)]);
        assert_eq!(model.composer.voice.phase, VoicePhase::Idle);
        // Transcribing, the words go to the draft they were spoken for.
        open(&mut model, "c");
        model.handle(Action::Composer(Act::Mic));
        model.handle(Action::Composer(Act::MicDone));
        open(&mut model, "d");
        model.tick(Instant::now());
        assert_eq!(model.composer.voice.phase, VoicePhase::Transcribing);
        model.dictation_line(Line::Text("for c".into()));
        assert!(model.composer.text.is_empty());
        open(&mut model, "c");
        assert_eq!(model.composer.text, "for c");
        // A recording stops by itself before the capture's limit.
        model.composer.text.clear();
        model.handle(Action::Composer(Act::Mic));
        let since = model.composer.voice.since.unwrap();
        model.tick(since + RECORD_FOR);
        assert_eq!(model.composer.voice.phase, VoicePhase::Transcribing);
    }

    #[test]
    fn a_message_over_the_budget_is_not_sent() {
        let mut model = ready(false);
        model.composer.text = "x".repeat(BUDGET + 1);
        assert!(model.handle(Action::Composer(Act::Send)).is_empty());
        assert!(model.composer_error_label().unwrap().text.contains("12 MB"));
        assert_eq!(model.composer.text.len(), BUDGET + 1, "nothing lost");
    }

    const GIF: &[u8] = b"GIF89a\x02\x00\x01\x00\x00\x00\x00;";

    /// Draft `key` gets a document and an image, as `files.rs` would.
    fn attach_two(model: &mut Model, key: &str) {
        for (name, bytes) in [("notes.md", &b"# hi"[..]), ("dot.gif", GIF)] {
            model.files_done(Done::Prepared {
                key: key.into(),
                result: attachments::prepare(name, bytes),
            });
        }
    }

    #[test]
    fn files_are_browsed_and_attached_to_the_draft_they_were_chosen_for() {
        let mut model = ready(false);
        model.handle(Action::Composer(Act::Attach));
        assert_eq!(opened(&mut model), vec!["Add attachment or voice"]);
        model.handle(Action::Composer(Act::Browse(None)));
        assert_eq!(opened(&mut model), vec!["Browse files"]);
        let Some(dir) = model.ui.effects.iter().find_map(|e| match e {
            Effect::Files(Job::List { dir }) => Some(dir.clone()),
            _ => None,
        }) else {
            panic!("a listing asked for");
        };
        assert!(model.ui.effects.contains(&Effect::HideKeyboard));
        assert!(dir.is_absolute());
        let entry = |name: &str, dir: bool| crate::exocortex::files::Entry {
            name: name.into(),
            dir,
            size: 4,
            modified: 0,
        };
        model.files_done(Done::Listed {
            dir: dir.clone(),
            result: Ok(vec![
                entry("sub", true),
                entry(".hidden", false),
                entry("notes.md", false),
            ]),
        });
        let (shown, hidden) = model.composer.browser.shown();
        assert_eq!((shown.len(), hidden), (2, 1));
        // Into a folder: listed afresh; a stale answer is ignored.
        model.handle(Action::Composer(Act::Browse(Some(dir.join("sub")))));
        model.files_done(Done::Listed {
            dir: dir.clone(),
            result: Ok(Vec::new()),
        });
        assert!(model.composer.browser.listing.is_none());
        // A file chosen, then the user moves to another chat: it lands in c.
        model.handle(Action::Composer(Act::AttachFile(dir.join("sub/notes.md"))));
        assert_eq!(
            model.ui.effects.last(),
            Some(&Effect::Files(Job::Read {
                key: "c".into(),
                path: dir.join("sub/notes.md")
            }))
        );
        open(&mut model, "d");
        attach_two(&mut model, "c");
        assert!(model.shown_attachments().0.is_empty());
        open(&mut model, "c");
        let (items, importing) = model.shown_attachments();
        assert_eq!(importing, 0);
        assert_eq!(
            items.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["notes.md", "dot.gif"]
        );
        // The tray's ✕.
        let id = items[0].id;
        model.handle(Action::Composer(Act::RemoveAttachment(id)));
        assert_eq!(model.shown_attachments().0.len(), 1);
        // At most six.
        for _ in 0..6 {
            model.files_done(Done::Prepared {
                key: "c".into(),
                result: attachments::prepare("x.gif", GIF),
            });
        }
        assert_eq!(model.shown_attachments().0.len(), attachments::MAX_ITEMS);
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some(attachments::TOO_MANY)
        );
    }

    #[test]
    fn attachments_go_with_the_message_and_come_back_if_it_does_not_arrive() {
        let mut model = ready(false);
        attach_two(&mut model, "c");
        // Attachments alone can be sent.
        assert!(matches!(
            crate::exocortex::view::composer::trailing(&model).primary,
            crate::exocortex::view::composer::Primary::Send { .. }
        ));
        // Not while one is still being prepared.
        model.handle(Action::Composer(Act::PasteImage));
        type_text(&mut model, "look");
        assert!(model.handle(Action::Composer(Act::Send)).is_empty());
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some(attachments::PREPARING)
        );
        model.files_done(Done::Prepared {
            key: "c".into(),
            result: Err(crate::exocortex::files::NO_IMAGE.into()),
        });
        // Commands carry none.
        model.composer.text = "/rename x".into();
        assert!(model.handle(Action::Composer(Act::Send)).is_empty());
        assert_eq!(
            model.composer_error_label().map(|e| e.text.as_str()),
            Some(attachments::COMMAND)
        );
        model.composer.text = "look".into();
        let sent = model.handle(Action::Composer(Act::Send));
        let send = sent.iter().find(|c| c["type"] == "send_message").unwrap();
        let text = send["text"].as_str().unwrap();
        assert!(
            text.starts_with("look\n\n<exocortex_file name_base64="),
            "{text}"
        );
        assert!(text.contains("[Attached file: notes.md]\n# hi\n</exocortex_file>"));
        assert_eq!(send["images"][0]["mediaType"], "image/gif");
        assert_eq!(send["images"][0]["width"], 2);
        assert!(model.shown_attachments().0.is_empty(), "they left with it");
        // Refused: text and attachments back into the draft.
        let request = send["reqId"].as_str().unwrap().to_owned();
        apply(
            &mut model,
            &format!(r#"{{"type":"error","reqId":"{request}","convId":"c","message":"No"}}"#),
        );
        assert_eq!(model.composer.text, "look");
        assert_eq!(model.shown_attachments().0.len(), 2);
        // A model without vision takes no images.
        model.ops.providers = vec![crate::exocortex::proto::Provider {
            id: "anthropic".into(),
            models: vec![crate::exocortex::proto::Model {
                id: "m".into(),
                supports_images: Some(false),
                ..Default::default()
            }],
            ..Default::default()
        }];
        assert!(model.handle(Action::Composer(Act::Send)).is_empty());
        assert!(
            model
                .composer_error_label()
                .unwrap()
                .text
                .starts_with("Image inputs are not supported by anthropic/m.")
        );
    }

    #[test]
    fn a_new_chat_carries_its_images_and_a_rewind_brings_them_back() {
        let mut model = ready(false);
        model.handle(Action::Nav(Nav::NewConversation));
        let key = model.shown_draft_key();
        attach_two(&mut model, &key);
        let sent = model.handle(Action::Composer(Act::Send));
        let new = sent
            .iter()
            .find(|c| c["type"] == "new_conversation")
            .unwrap();
        assert_eq!(new["initialMessage"]["images"][0]["mediaType"], "image/gif");
        assert!(
            new["initialMessage"]["text"]
                .as_str()
                .unwrap()
                .starts_with("<exocortex_file")
        );
        // The transcript shows the image by its id until the daemon has it.
        let Some(Entry::User { images, .. }) = model.chat.entries.last() else {
            panic!("the first message shown");
        };
        assert!(images[0].base64.is_empty() && images[0].image_id.is_some());
        // A sent message's documents and images back into a draft.
        let image = proto::ImageAttachment {
            media_type: "image/gif".into(),
            base64: attachments::base64(GIF),
            size_bytes: GIF.len() as u64,
            ..Default::default()
        };
        let (text, back) =
            attachments::editable(new["initialMessage"]["text"].as_str().unwrap(), &[image]);
        assert_eq!(text, "");
        assert_eq!(
            back.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["notes.md", "Image 1"]
        );
    }
}
