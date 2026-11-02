//! The WhatsApp app: `a26-system --app whatsapp`, a client of the WhatsApp
//! bridge (docs/social-protocol.md §5), in WhatsApp's own dark look: the
//! chat list (filters, search, archived chats), a chat (bubbles, replies,
//! reactions, photos, stickers, voice notes, documents, notices), the photo
//! viewer, the composer (or, with writes off, why there is none), settings
//! with the writes switch, and linking with a QR code.
//!
//! It never waits on the bridge: every request is answered later
//! (`App::answer`), the list is drawn from what the bridge last said, and a
//! chat's pages come as they come. Scrolling moves what is drawn and draws
//! only what comes into view (`band.rs`).

mod avatar;
mod band;
mod chat;
mod composer;
mod icons;
mod info;
mod list;
mod login;
mod rich;
mod settings;
mod theme;
mod time;
mod updates;
mod viewer;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::exocortex::gesture::Gesture;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::social::proto::{BridgeError, Chat, Event, Hello, Status};
use crate::social::shell::{self, App, Cx, DrawCx};
use crate::text::{self, Weight};

use self::avatar::Rounded;

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    shell::run(WhatsApp::new())
}

/// How long a notice at the bottom stays.
const TOAST: Duration = Duration::from_millis(2600);
/// A first page asked for on a press is used by the open within this long.
const PREFETCH_FRESH: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    List,
    Archived,
    Chat,
    Settings,
    Updates,
    Info,
}

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub writes: bool,
    pub notifications: bool,
    pub previews: bool,
}

/// What a request was for.
enum Ask {
    List { folder: &'static str },
    History { chat: String, older: bool },
    Settings,
    Send { chat: String, client_id: String },
    Login,
    Updates,
    Info,
    InfoMedia,
    Other(&'static str),
}

/// The chats the bridge said, by id, and the list's order.
#[derive(Default)]
pub struct Chats {
    pub by_id: HashMap<String, Chat>,
    pub inbox: Vec<String>,
    pub archived: Vec<String>,
    /// Bumped on every change (what the list shows depends on it).
    pub version: u64,
    sorted: bool,
}

impl Chats {
    pub fn upsert(&mut self, chat: Chat) {
        self.by_id.insert(chat.id.clone(), chat);
        self.sorted = false;
        self.version += 1;
    }

    pub fn remove(&mut self, id: &str) {
        if self.by_id.remove(id).is_some() {
            self.sorted = false;
            self.version += 1;
        }
    }

    /// The list's order: pinned first, then newest first.
    pub fn sort(&mut self) {
        if self.sorted {
            return;
        }
        let mut all: Vec<&Chat> = self.by_id.values().collect();
        all.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.ts.cmp(&a.ts))
                .then(a.id.cmp(&b.id))
        });
        self.inbox = all
            .iter()
            .filter(|chat| !chat.archived)
            .map(|chat| chat.id.clone())
            .collect();
        self.archived = all
            .iter()
            .filter(|chat| chat.archived)
            .map(|chat| chat.id.clone())
            .collect();
        self.sorted = true;
    }

    pub fn get(&self, id: &str) -> Option<&Chat> {
        self.by_id.get(id)
    }
}

pub struct WhatsApp {
    pub status: Status,
    pub hello: Hello,
    pub settings: Settings,
    pub chats: Chats,
    pub page: Page,
    pub list: list::ListView,
    pub archived: list::ListView,
    pub chat: Option<chat::ChatView>,
    pub composer: composer::Composer,
    pub settings_view: settings::SettingsView,
    pub login: login::LoginView,
    pub viewer: Option<viewer::Viewer>,
    pub updates: updates::UpdatesView,
    pub info: Option<info::InfoView>,
    pub status_viewer: Option<updates::StatusViewer>,
    pub rounded: Rounded,
    asked: HashMap<u64, Ask>,
    toast: Option<(String, Instant)>,
    /// The bridge has answered `hello` at least once.
    pub greeted: bool,
    /// The whole canvas must be drawn next frame.
    pub full: bool,
    last_size: (f32, f32),
    last_page: Option<Page>,
    /// A chat opened at this time, its messages not drawn yet (profiling).
    opening: Option<Instant>,
    /// A chat's first page asked for as a finger came down on its row
    /// (the bridge answers while the finger lifts): which, when, the answer.
    prefetch: Option<(String, Instant, Option<Value>)>,
    /// What this frame changed (the shell presents only that), unless the
    /// whole canvas was drawn.
    pub damage: shell::Damage,
    pub damage_full: bool,
}

impl WhatsApp {
    pub fn new() -> Self {
        Self {
            status: Status {
                state: "connecting".into(),
                ..Status::default()
            },
            hello: Hello::default(),
            settings: Settings {
                writes: false,
                notifications: true,
                previews: true,
            },
            chats: Chats::default(),
            page: Page::List,
            list: list::ListView::new("inbox"),
            archived: list::ListView::new("archived"),
            chat: None,
            composer: composer::Composer::default(),
            settings_view: settings::SettingsView::default(),
            login: login::LoginView::default(),
            viewer: None,
            updates: updates::UpdatesView::default(),
            info: None,
            status_viewer: None,
            rounded: Rounded::default(),
            asked: HashMap::new(),
            toast: None,
            greeted: false,
            full: true,
            last_size: (0.0, 0.0),
            last_page: None,
            opening: None,
            prefetch: None,
            damage: shell::Damage::default(),
            damage_full: false,
        }
    }

    pub fn signed_out(&self) -> bool {
        self.greeted && self.status.signed_out()
    }

    fn ask(&mut self, cx: &mut Cx, cmd: &str, args: Value, ask: Ask) -> u64 {
        let id = cx.request(cmd, args);
        self.asked.insert(id, ask);
        id
    }

    pub fn toast(&mut self, cx: &mut Cx, text: impl Into<String>) {
        self.toast = Some((text.into(), cx.now));
        cx.wake_at(cx.now + TOAST);
        cx.redraw();
    }

    fn request_lists(&mut self, cx: &mut Cx) {
        for folder in ["inbox", "archived"] {
            self.ask(
                cx,
                "chats.list",
                json!({"folder": folder, "limit": 500}),
                Ask::List { folder },
            );
        }
    }

    /// Opens a chat (from the list, a notification, the archive).
    pub fn open_chat(&mut self, cx: &mut Cx, id: &str) {
        if self.page == Page::Chat && self.chat.as_ref().is_some_and(|chat| chat.id == id) {
            return;
        }
        cx.hide_keyboard();
        cx.transition(true);
        let unread = self.chats.get(id).map_or(0, |chat| chat.unread);
        let mut view = chat::ChatView::new(id.to_owned(), unread);
        if let Some(chat) = self.chats.get(id) {
            view.chat_changed(chat);
        }
        match self.prefetch.take() {
            // Asked for as the finger came down: answered, or about to be.
            Some((chat, at, answer))
                if chat == id && cx.now.duration_since(at) < PREFETCH_FRESH =>
            {
                if let Some(answer) = answer {
                    view.page_arrived(&answer, false, cx.now);
                }
            }
            _ => self.ask_first_page(cx, id),
        }
        view.requested = true;
        self.chat = Some(view);
        self.opening = Some(cx.now);
        self.composer.clear();
        self.page = Page::Chat;
        self.full = true;
        cx.redraw();
    }

    fn ask_first_page(&mut self, cx: &mut Cx, id: &str) {
        let unread = self.chats.get(id).map_or(0, |chat| chat.unread);
        let limit = (unread as usize + 30).clamp(50, 100);
        let request = cx.request("history", json!({"chat": id, "limit": limit}));
        self.asked.insert(
            request,
            Ask::History {
                chat: id.to_owned(),
                older: false,
            },
        );
    }

    /// A finger came down on a chat's row: its first page is asked for now,
    /// so it is there (or nearly) when the tap opens the chat.
    pub fn prefetch(&mut self, cx: &mut Cx, id: &str) {
        if !cx.connected
            || self.prefetch.as_ref().is_some_and(|(chat, at, _)| {
                chat == id && cx.now.duration_since(*at) < PREFETCH_FRESH
            })
        {
            return;
        }
        self.prefetch = Some((id.to_owned(), cx.now, None));
        self.ask_first_page(cx, id);
    }

    pub fn go(&mut self, cx: &mut Cx, page: Page) {
        if self.page == page {
            return;
        }
        cx.hide_keyboard();
        // Tabs change in place; pages slide.
        let tabs = matches!(
            (self.page, page),
            (Page::List | Page::Updates, Page::List | Page::Updates)
        );
        if !tabs {
            let push = matches!(
                (self.page, page),
                (Page::List | Page::Updates, _)
                    | (Page::Archived, Page::Chat)
                    | (_, Page::Settings)
            ) && page != Page::List;
            cx.transition(push);
        }
        if page != Page::Chat {
            self.chat = None;
        }
        if page == Page::Updates {
            updates::refresh(self, cx);
        }
        self.page = page;
        self.full = true;
        cx.redraw();
    }

    /// Back: the viewer, the search, then the page under this one.
    pub fn back(&mut self, cx: &mut Cx) {
        if self.viewer.take().is_some() || self.status_viewer.take().is_some() {
            self.full = true;
            cx.redraw();
            return;
        }
        if self.settings_view.dialog.take().is_some() {
            cx.redraw();
            return;
        }
        if let Some(chat) = self.chat.as_mut()
            && chat.selected.take().is_some()
        {
            self.full = true;
            cx.redraw();
            return;
        }
        match self.page {
            Page::Chat => {
                let to = if self
                    .chat
                    .as_ref()
                    .and_then(|chat| self.chats.get(&chat.id))
                    .is_some_and(|chat| chat.archived)
                {
                    Page::Archived
                } else {
                    Page::List
                };
                self.go(cx, to);
            }
            Page::Archived | Page::Settings | Page::Updates => self.go(cx, Page::List),
            Page::Info => {
                cx.transition(false);
                self.info = None;
                self.page = Page::Chat;
                self.full = true;
                cx.redraw();
            }
            Page::List => {
                if self.list.searching() {
                    self.list.end_search();
                    cx.hide_keyboard();
                    self.full = true;
                    cx.redraw();
                }
            }
        }
    }

    pub fn set_setting(&mut self, cx: &mut Cx, field: &str, value: bool) {
        self.ask(cx, "settings.set", json!({ field: value }), Ask::Settings);
    }

    fn apply_settings(&mut self, value: &Value) {
        self.settings.writes = value["writes"].as_bool().unwrap_or(self.settings.writes);
        self.settings.notifications = value["notifications"]
            .as_bool()
            .unwrap_or(self.settings.notifications);
        self.settings.previews = value["previews"]
            .as_bool()
            .unwrap_or(self.settings.previews);
    }

    /// Sends what the composer holds (writes on).
    pub fn send(&mut self, cx: &mut Cx) {
        let Some(chat) = self.chat.as_mut() else {
            return;
        };
        let text = self.composer.text.trim_end().to_owned();
        if text.trim().is_empty() {
            return;
        }
        let client_id = format!("c{}", time::now_secs() * 1000 + self.asked.len() as i64);
        let reply = self.composer.reply.as_ref().map(|reply| reply.id.clone());
        chat.add_pending(&client_id, &text, reply.as_deref());
        let chat_id = chat.id.clone();
        self.composer.clear();
        let mut args = json!({"chat": chat_id, "text": text, "client_id": client_id});
        if let Some(reply) = reply {
            args["reply_to"] = json!(reply);
        }
        self.ask(
            cx,
            "send",
            args,
            Ask::Send {
                chat: chat_id,
                client_id,
            },
        );
        cx.redraw();
    }

    fn draw_toast(&self, canvas: &mut Canvas, now: Instant) {
        let Some((text_in, at)) = &self.toast else {
            return;
        };
        if now.duration_since(*at) > TOAST {
            return;
        }
        let width = canvas.width as f32;
        let bottom = canvas.height as f32 - theme::GESTURE_RESERVE - 60.0;
        let size = 38.0;
        let laid = text::fit(text_in, Weight::Regular, size, width - 200.0);
        let w = text::measure(&laid, Weight::Regular, size) + 80.0;
        let rect = RectF {
            x: (width - w) / 2.0,
            y: bottom - 110.0,
            w,
            h: 110.0,
        };
        canvas.shadow_rrect(rect, 24.0, 24.0, 0x000000, 0.4);
        canvas.fill_rrect(rect, 24.0, theme::SURFACE_HI, 1.0);
        text::draw_centered(
            canvas,
            &laid,
            width / 2.0,
            rect.y + 68.0,
            Weight::Regular,
            size,
            theme::TEXT,
            1.0,
        );
    }
}

impl App for WhatsApp {
    fn id(&self) -> &'static str {
        "whatsapp"
    }

    fn status_bar(&self) -> (String, &'static str) {
        let color = if self.viewer.is_some() || self.status_viewer.is_some() {
            0x000000
        } else if self.page == Page::Chat {
            theme::CHAT_BAR
        } else {
            theme::BAR
        };
        (format!("#{color:06x}"), "light")
    }

    fn connected(&mut self, cx: &mut Cx, hello: &Hello) {
        self.greeted = true;
        self.hello = hello.clone();
        self.status = hello.status.clone();
        self.settings.writes = hello.status.writes;
        self.request_lists(cx);
        self.ask(cx, "settings.get", json!({}), Ask::Settings);
        if let Some(chat) = self.chat.as_mut() {
            let id = chat.id.clone();
            // The page as it is now replaces the one shown.
            chat.loaded = false;
            let request = cx.request("history", json!({"chat": id, "limit": 60}));
            self.asked.insert(
                request,
                Ask::History {
                    chat: id,
                    older: false,
                },
            );
            chat.loading_older = false;
        }
        if self.status.signed_out() && self.login.wanted() {
            login::start(self, cx);
        }
        self.full = true;
        cx.redraw();
    }

    fn disconnected(&mut self, cx: &mut Cx) {
        self.status.state = "connecting".into();
        if let Some(chat) = self.chat.as_mut() {
            chat.loading_older = false;
        }
        self.full = true;
        cx.redraw();
    }

    fn event(&mut self, cx: &mut Cx, event: Event) {
        match event {
            Event::Status(status) => {
                let was_signed_out = self.status.signed_out();
                self.settings.writes = status.writes;
                self.status = status;
                if was_signed_out && self.status.online() {
                    self.request_lists(cx);
                    self.go(cx, Page::List);
                }
                if self.status.signed_out() && !was_signed_out {
                    self.chats = Chats::default();
                    self.chat = None;
                    self.page = Page::List;
                }
                self.full = true;
            }
            Event::Chat(mut chat) => {
                rich::colour_chat(&mut chat);
                if let Some(view) = self.chat.as_mut()
                    && view.id == chat.id
                {
                    view.chat_changed(&chat);
                }
                self.chats.upsert(*chat);
            }
            Event::ChatRemoved(id) => self.chats.remove(&id),
            Event::Message { mut message, live } => {
                rich::colour_message(&mut message);
                if let Some(view) = self.chat.as_mut()
                    && view.id == message.chat
                {
                    view.upsert(*message, live, cx.now);
                }
            }
            Event::MessageRemoved { chat, id } => {
                if let Some(view) = self.chat.as_mut()
                    && view.id == chat
                {
                    view.remove(&id);
                }
            }
            Event::Login(value) => login::event(self, cx, &value),
            Event::Other(name, _) if name == "whatsapp.status" => {
                if self.page == Page::Updates && self.status_viewer.is_none() {
                    updates::refresh(self, cx);
                }
                return;
            }
            _ => return,
        }
        cx.redraw();
    }

    fn answer(&mut self, cx: &mut Cx, id: u64, result: Result<Value, BridgeError>) {
        let Some(ask) = self.asked.remove(&id) else {
            return;
        };
        match (ask, result) {
            (Ask::List { folder }, Ok(value)) => {
                if let Ok(chats) = serde_json::from_value::<Vec<Chat>>(value["chats"].clone()) {
                    // The answer is the folder as it is now: what it no longer
                    // holds goes.
                    let archived = folder == "archived";
                    let keep: std::collections::HashSet<String> =
                        chats.iter().map(|chat| chat.id.clone()).collect();
                    let stale: Vec<String> = self
                        .chats
                        .by_id
                        .values()
                        .filter(|chat| chat.archived == archived && !keep.contains(&chat.id))
                        .map(|chat| chat.id.clone())
                        .collect();
                    for id in stale {
                        self.chats.remove(&id);
                    }
                    for mut chat in chats {
                        rich::colour_chat(&mut chat);
                        self.chats.upsert(chat);
                    }
                }
            }
            (Ask::History { chat, older }, result) => {
                if let Some((prefetched, _, answer)) = self.prefetch.as_mut()
                    && *prefetched == chat
                    && !older
                    && let Ok(value) = &result
                {
                    *answer = Some(value.clone());
                }
                if let Some(view) = self.chat.as_mut()
                    && view.id == chat
                    && (older || !view.loaded)
                {
                    match result {
                        Ok(value) => view.page_arrived(&value, older, cx.now),
                        Err(_) => view.page_failed(older),
                    }
                }
            }
            (Ask::Settings, Ok(value)) => self.apply_settings(&value),
            (Ask::Settings, Err(error)) => {
                let text = if error.code == "read_only" {
                    "This bridge runs read-only: writes stay off"
                } else {
                    "Couldn't change the setting"
                };
                self.toast(cx, text);
            }
            (Ask::Send { chat, client_id }, result) => {
                if let Some(view) = self.chat.as_mut()
                    && view.id == chat
                {
                    match result {
                        Ok(value) => {
                            if let Ok(mut message) =
                                serde_json::from_value(value["message"].clone())
                            {
                                rich::colour_message(&mut message);
                                view.upsert(message, false, cx.now);
                            }
                        }
                        Err(error) => {
                            view.fail_pending(&client_id);
                            let text = match error.code.as_str() {
                                "read_only" => "Writes are off: turn them on in Settings",
                                "uncertain" => "The message may not have been sent",
                                _ => "Couldn't send the message",
                            };
                            self.toast(cx, text);
                        }
                    }
                }
            }
            (Ask::Login, Err(error)) => login::failed(self, cx, &error),
            (Ask::Info, Ok(value)) => {
                if let Some(view) = self.info.as_mut() {
                    view.info_arrived(value);
                }
            }
            (Ask::InfoMedia, Ok(value)) => {
                if let Some(view) = self.info.as_mut() {
                    view.media_arrived(&value);
                }
            }
            (Ask::Updates, Ok(value)) => {
                self.updates.updates = updates::parse(&value);
                self.updates.loaded = true;
            }
            (Ask::Other(what), Err(error)) => {
                let text = match error.code.as_str() {
                    "read_only" => "Writes are off: turn them on in Settings".to_owned(),
                    _ => format!("Couldn't {what}"),
                };
                self.toast(cx, text);
            }
            _ => {}
        }
        self.full |= self.page != Page::Chat;
        cx.redraw();
    }

    fn gesture(&mut self, cx: &mut Cx, gesture: Gesture) {
        if let Some(viewer) = self.viewer.as_mut() {
            if viewer::gesture(viewer, cx, gesture) {
                self.viewer = None;
                self.full = true;
            }
            cx.redraw();
            return;
        }
        if self.status_viewer.is_some() {
            updates::viewer_gesture(self, cx, gesture);
            return;
        }
        if matches!(gesture, Gesture::Back) {
            self.back(cx);
            return;
        }
        if self.signed_out() {
            login::gesture(self, cx, gesture);
            return;
        }
        if self.settings_view.dialog.is_some() || self.page == Page::Settings {
            settings::gesture(self, cx, gesture);
            return;
        }
        match self.page {
            Page::List | Page::Archived => list::gesture(self, cx, gesture),
            Page::Chat => chat::gesture(self, cx, gesture),
            Page::Updates => updates::gesture(self, cx, gesture),
            Page::Info => info::gesture(self, cx, gesture),
            Page::Settings => {}
        }
    }

    fn key(&mut self, cx: &mut Cx, key: KeyInput) {
        if self.page == Page::Chat {
            if self.composer.key(key) {
                self.send(cx);
            }
            cx.redraw();
        } else if self.page == Page::List && self.list.searching() {
            self.list.search_key(key);
            self.full = true;
            cx.redraw();
        }
    }

    fn open_target(&mut self, cx: &mut Cx, target: &str) {
        self.viewer = None;
        self.open_chat(cx, target);
    }

    fn shown(&mut self, cx: &mut Cx, shown: bool) {
        if shown {
            self.full = true;
            cx.redraw();
        } else {
            cx.hide_keyboard();
            self.list.end_search();
        }
    }

    fn tick(&mut self, cx: &mut Cx) {
        if self.signed_out() {
            login::tick(self, cx);
            return;
        }
        match self.page {
            Page::List => list::tick(&mut self.list, &mut self.chats, cx),
            Page::Archived => list::tick(&mut self.archived, &mut self.chats, cx),
            Page::Chat => chat::tick(self, cx),
            Page::Info => info::tick(self, cx),
            Page::Settings | Page::Updates => {}
        }
        if let Some(viewer) = self.viewer.as_mut() {
            viewer::tick(viewer, cx);
        }
        updates::tick(self, cx);
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, at)| cx.now.duration_since(*at) > TOAST)
        {
            self.toast = None;
            self.full = true;
            cx.redraw();
        }
    }

    fn viewing(&self) -> Option<String> {
        match self.page {
            Page::Chat if !self.signed_out() => self.chat.as_ref().map(|chat| chat.id.clone()),
            _ => None,
        }
    }

    fn draw(&mut self, canvas: &mut Canvas, cx: &mut DrawCx) {
        let size = (canvas.width as f32, canvas.height as f32);
        let mut full = cx.full || self.full || self.last_size != size;
        if self.last_page != Some(self.page) {
            full = true;
            self.last_page = Some(self.page);
        }
        self.last_size = size;
        self.full = false;
        self.damage = shell::Damage::default();
        self.damage_full = full;
        if let Some(viewer) = self.viewer.as_mut() {
            viewer::draw(viewer, canvas, cx);
            // What is under the viewer is drawn whole when it closes.
            self.full = true;
            return;
        }
        if self.status_viewer.is_some() {
            // The bars move every frame; the rest when the update changes.
            cx.damage = updates::draw_viewer(self, canvas, cx, full).map(|rows| shell::Damage {
                moved: None,
                rows: vec![rows],
            });
            self.full = true;
            return;
        }
        if self.signed_out() {
            self.damage_full = true;
            login::draw(self, canvas, cx);
        } else {
            match self.page {
                Page::List => list::draw(self, canvas, cx, full, false),
                Page::Archived => list::draw(self, canvas, cx, full, true),
                Page::Chat => chat::draw(self, canvas, cx, full),
                Page::Settings => {
                    self.damage_full = true;
                    settings::draw(self, canvas, cx);
                }
                Page::Updates => {
                    self.damage_full = true;
                    updates::draw(self, canvas, cx, full);
                }
                Page::Info => info::draw(self, canvas, cx, full),
            }
            if !full && verify() {
                self.verify(canvas, cx);
            }
            if self.page == Page::Chat
                && let Some(at) = self.opening
                && let Some(chat) = &self.chat
                && chat.loaded
            {
                self.opening = None;
                if profile() {
                    eprintln!(
                        "whatsapp: opened {} in {:.1} ms ({} messages)",
                        if chat.messages.is_empty() {
                            "empty"
                        } else {
                            "chat"
                        },
                        cx.now.duration_since(at).as_secs_f64() * 1000.0,
                        chat.messages.len()
                    );
                }
            }
        }
        if self.toast.is_some() {
            self.draw_toast(canvas, cx.now);
            // The toast's rows are drawn over: the next frame draws them whole.
            self.full = true;
            self.damage_full = true;
        }
        if profile() {
            let rows: i32 = self.damage.rows.iter().map(|(a, b)| b - a).sum();
            eprintln!(
                "whatsapp: damage {} moved {} rows {}",
                if self.damage_full { "full" } else { "part" },
                self.damage.moved.map_or(0, |m| m.2),
                rows
            );
        }
        cx.damage = (!self.damage_full).then(|| std::mem::take(&mut self.damage));
    }
}

/// Measurement (`SOCIAL_APP_PROFILE`, the bench's `EXOCORTEX_APP_PROFILE`).
pub(crate) fn profile() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("SOCIAL_APP_PROFILE").is_some()
            || std::env::var_os("EXOCORTEX_APP_PROFILE").is_some()
    })
}

/// Development check (`WHATSAPP_APP_VERIFY`): a frame drawn the cheap way
/// equals the same frame drawn whole.
fn verify() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WHATSAPP_APP_VERIFY").is_some())
}

impl WhatsApp {
    fn verify(&mut self, canvas: &Canvas, cx: &mut DrawCx) {
        if !matches!(self.page, Page::List | Page::Archived | Page::Chat) {
            return;
        }
        let mut whole = Canvas::new(canvas.width, canvas.height);
        // What the real frames still owe (pictures to draw when they come)
        // stays theirs.
        let saved = (
            self.list.waiting.clone(),
            self.archived.waiting.clone(),
            self.chat
                .as_ref()
                .map(|chat| (chat.waiting.clone(), chat.header_waiting.clone())),
            std::mem::take(&mut self.damage),
        );
        match self.page {
            Page::List => list::draw(self, &mut whole, cx, true, false),
            Page::Archived => list::draw(self, &mut whole, cx, true, true),
            Page::Chat => chat::draw(self, &mut whole, cx, true),
            Page::Settings | Page::Updates | Page::Info => {}
        }
        self.list.waiting = saved.0;
        self.archived.waiting = saved.1;
        if let (Some(chat), Some((waiting, header))) = (self.chat.as_mut(), saved.2) {
            chat.waiting = waiting;
            chat.header_waiting = header;
        }
        self.damage = saved.3;
        let width = canvas.width as usize;
        let mut differ = 0;
        let mut first = None;
        for (index, (a, b)) in canvas.pixels.iter().zip(&whole.pixels).enumerate() {
            if a != b {
                differ += 1;
                first.get_or_insert(index / width);
            }
        }
        if differ > 0 && std::env::var_os("WHATSAPP_APP_VERIFY_DUMP").is_some() {
            for (name, c) in [("real", canvas), ("whole", &whole)] {
                let mut out = format!("P6\n{} {}\n255\n", c.width, c.height).into_bytes();
                for p in &c.pixels {
                    out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
                }
                let _ = std::fs::write(format!("/tmp/verify-{name}.ppm"), out);
            }
        }
        if differ > 0 {
            eprintln!(
                "whatsapp: verify: {differ} pixels differ from a whole frame, the first in row {}",
                first.unwrap_or(0)
            );
        }
    }
}

/// A sub-page's bar: back arrow, title (and subtitle).
pub fn draw_bar(canvas: &mut Canvas, title: &str, subtitle: Option<&str>, color: crate::gfx::Rgb) {
    let width = canvas.width as f32;
    canvas.fill_rect(
        0,
        0,
        canvas.width,
        (theme::TOP + theme::HEADER) as i32,
        color,
        1.0,
    );
    let cy = theme::TOP + theme::HEADER / 2.0;
    icons::draw(canvas, icons::Glyph::Back, 70.0, cy, 66.0, theme::TEXT, 1.0);
    let x = 150.0;
    let title = text::fit(title, Weight::Medium, theme::T_HEADER, width - x - 60.0);
    match subtitle {
        Some(sub) => {
            text::draw(
                canvas,
                &title,
                x,
                cy - 6.0,
                Weight::Medium,
                theme::T_HEADER,
                theme::TEXT,
                1.0,
            );
            let sub = text::fit(sub, Weight::Regular, theme::T_SUB, width - x - 60.0);
            text::draw(
                canvas,
                &sub,
                x,
                cy + 42.0,
                Weight::Regular,
                theme::T_SUB,
                theme::SECONDARY,
                1.0,
            );
        }
        None => {
            text::draw(
                canvas,
                &title,
                x,
                text::centered_baseline(cy, Weight::Medium, theme::T_HEADER),
                Weight::Medium,
                theme::T_HEADER,
                theme::TEXT,
                1.0,
            );
        }
    }
}

/// Whether a tap at (x, y) is on a sub-page bar's back arrow.
pub fn on_back(x: f32, y: f32) -> bool {
    y < theme::TOP + theme::HEADER && y > theme::TOP - 20.0 && x < 140.0
}
