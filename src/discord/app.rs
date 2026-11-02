//! The Discord app on the social shell: the bridge's answers and events
//! into the model, gestures and keys to the view in front, the views'
//! actions into requests and navigation, and drawing. `DISCORD_APP_TRACE`
//! (a file) logs each frame's drawing time, for measuring.

use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::control::KeyboardPurpose;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::exocortex::ui::Haptic;
use crate::exocortex::ui::icons::{self, Icon};
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::social::media::{Fit, Media};
use crate::social::proto::{Author, BridgeError, Chat, Event, Hello, Message};
use crate::social::shell::{App, Cx, DrawCx};
use crate::text::{self, Weight};

use super::Action;
use super::chat::ChatView;
use super::lists::{ForumView, MembersView, MessagesView};
use super::model::{Model, Settings, Typing, now_ms};
use super::servers::ServersView;
use super::theme::{self, dp};
use super::ui;
use super::you::YouView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Servers,
    Messages,
    You,
}

enum Page {
    Chat(Box<ChatView>),
    Forum(ForumView),
    Members(MembersView),
}

enum Ask {
    Chats,
    Guilds,
    Settings,
    History { chat: String, older: bool },
    Threads(String),
    MemberList(String),
    Send { chat: String, client_id: String },
    Write,
    Login,
}

struct Viewer {
    chat: String,
    message: String,
    index: usize,
}

struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    started: Instant,
    lines: u32,
}

pub struct DiscordApp {
    model: Model,
    tab: Tab,
    stack: Vec<Page>,
    servers: ServersView,
    messages: MessagesView,
    you: YouView,
    asked: HashMap<u64, Ask>,
    viewer: Option<Viewer>,
    toast: Option<(String, Instant)>,
    editing: Option<(String, String)>,
    status_color: String,
    trace: Option<Trace>,
    /// A notification's chat, opened once the chats are known.
    pending_target: Option<String>,
    last_tab_bar: Option<(Tab, u32, u32)>,
}

const TOAST_FOR: Duration = Duration::from_millis(2600);

fn status_bar_send(color: &str) {
    let command = format!("statusbar color {color} light");
    let _ = std::thread::Builder::new()
        .name("discord-moon".into())
        .spawn(move || {
            crate::control::send(&command);
        });
}

impl DiscordApp {
    pub fn new() -> Self {
        let trace = std::env::var_os("DISCORD_APP_TRACE").and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(|file| Trace {
                    out: std::io::BufWriter::new(file),
                    started: Instant::now(),
                    lines: 0,
                })
        });
        Self {
            model: Model::default(),
            tab: Tab::Servers,
            stack: Vec::new(),
            servers: ServersView::new(),
            messages: MessagesView::new(),
            you: YouView::new(),
            asked: HashMap::new(),
            viewer: None,
            toast: None,
            editing: None,
            status_color: format!("#{:06x}", theme::RAIL),
            trace,
            pending_target: None,
            last_tab_bar: None,
        }
    }

    fn page_color(&self) -> String {
        let color = match self.stack.last() {
            Some(Page::Chat(_)) | Some(Page::Forum(_)) => theme::CHAT,
            Some(Page::Members(_)) => theme::CHAT,
            None => theme::RAIL,
        };
        format!("#{color:06x}")
    }

    fn follow_status_bar(&mut self) {
        let color = self.page_color();
        if color != self.status_color {
            self.status_color = color.clone();
            if !cfg!(test) {
                status_bar_send(&color);
            }
        }
    }

    fn ask(&mut self, cx: &Cx, cmd: &str, args: Value, ask: Ask) {
        let id = cx.request(cmd, args);
        self.asked.insert(id, ask);
    }

    fn load_history(&mut self, cx: &mut Cx, chat: &str) {
        if !cx.connected {
            return;
        }
        let history = self.model.histories.entry(chat.to_owned()).or_default();
        history.loading = true;
        self.ask(
            cx,
            "history",
            json!({"chat": chat, "limit": 50}),
            Ask::History {
                chat: chat.to_owned(),
                older: false,
            },
        );
    }

    fn open_chat(&mut self, cx: &mut Cx, chat: String, replace: bool) {
        if let Some(Page::Chat(view)) = self.stack.last()
            && view.chat == chat
        {
            return;
        }
        cx.transition(true);
        if replace {
            self.stack.clear();
        }
        self.stack
            .push(Page::Chat(Box::new(ChatView::new(chat.clone()))));
        let fresh = self.model.histories.get(&chat).is_none_or(|h| !h.loaded)
            || self.model.stale.remove(&chat);
        // Opened before: shown at once, and the newest page asked for again.
        if fresh || cx.connected {
            self.load_history(cx, &chat);
        }
        self.follow_status_bar();
        cx.redraw();
    }

    fn back(&mut self, cx: &mut Cx) {
        if self.viewer.take().is_some() {
            self.dirty();
            cx.redraw();
            return;
        }
        if let Some(Page::Chat(view)) = self.stack.last_mut()
            && view.sheet.take().is_some()
        {
            cx.redraw();
            return;
        }
        if self.stack.pop().is_some() {
            self.dirty();
            cx.hide_keyboard();
            cx.transition(false);
            self.editing = None;
            self.follow_status_bar();
            cx.redraw();
        }
    }

    fn toast(&mut self, cx: &mut Cx, text: String) {
        self.dirty();
        self.toast = Some((text, cx.now));
        cx.wake_at(cx.now + TOAST_FOR);
        cx.redraw();
    }

    fn act(&mut self, cx: &mut Cx, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Redraw => cx.redraw(),
                Action::Animate => cx.animate(),
                Action::Haptic => cx.haptic(Haptic::Medium),
                Action::Back => self.back(cx),
                Action::OpenChat(chat) => self.open_chat(cx, chat, false),
                Action::OpenForum(chat) => {
                    cx.transition(true);
                    self.stack.push(Page::Forum(ForumView::new(chat.clone())));
                    self.ask(
                        cx,
                        "discord.threads",
                        json!({"chat": chat}),
                        Ask::Threads(chat),
                    );
                    self.follow_status_bar();
                    cx.redraw();
                }
                Action::Members(chat) => {
                    cx.transition(true);
                    self.stack
                        .push(Page::Members(MembersView::new(chat.clone())));
                    self.ask(
                        cx,
                        "discord.member_list",
                        json!({"chat": chat}),
                        Ask::MemberList(chat),
                    );
                    self.follow_status_bar();
                    cx.redraw();
                }
                Action::OpenUrl(url) => {
                    if !cx.open_url(&url) {
                        self.toast(cx, "No browser to open the link".into());
                    }
                }
                Action::Viewer {
                    chat,
                    message,
                    index,
                } => {
                    self.viewer = Some(Viewer {
                        chat,
                        message,
                        index,
                    });
                    cx.redraw();
                }
                Action::Send {
                    chat,
                    text,
                    reply_to,
                    client_id,
                } => self.send(cx, chat, text, reply_to, client_id),
                Action::React {
                    chat,
                    message,
                    emoji,
                    remove,
                } => {
                    if !self.model.settings.writes {
                        self.toast(
                            cx,
                            "Sending is off. Turn it on in You › Allow sending.".into(),
                        );
                        continue;
                    }
                    self.ask(
                        cx,
                        "react",
                        json!({"chat": chat, "message": message, "emoji": emoji, "remove": remove}),
                        Ask::Write,
                    );
                }
                Action::Delete { chat, message } => self.ask(
                    cx,
                    "delete",
                    json!({"chat": chat, "message": message}),
                    Ask::Write,
                ),
                Action::EditStart(message) => {
                    if let Some(Page::Chat(view)) = self.stack.last() {
                        self.editing = Some((view.chat.clone(), message));
                    }
                }
                Action::Copy(text) => {
                    cx.copy(&text);
                    cx.haptic(Haptic::Selection);
                    self.toast(cx, "Copied".into());
                }
                Action::Toast(text) => self.toast(cx, text),
                Action::ShowKeyboard => {
                    cx.show_keyboard(KeyboardPurpose::Multiline);
                    cx.redraw();
                }
                Action::HideKeyboard => {
                    cx.hide_keyboard();
                    cx.redraw();
                }
                Action::SetWrites(on) => {
                    self.ask(cx, "settings.set", json!({"writes": on}), Ask::Settings)
                }
                Action::SetNotifications(on) => self.ask(
                    cx,
                    "settings.set",
                    json!({"notifications": on}),
                    Ask::Settings,
                ),
                Action::SetPreviews(on) => {
                    self.ask(cx, "settings.set", json!({"previews": on}), Ask::Settings)
                }
                Action::LoginFromClipboard => match crate::exocortex::clipboard::paste() {
                    Some(token) if !token.trim().is_empty() => {
                        self.you.login = Some("Checking the token with Discord…".into());
                        self.ask(
                            cx,
                            "login.start",
                            json!({"method": "token", "token": token.trim()}),
                            Ask::Login,
                        );
                        cx.redraw();
                    }
                    _ => self.toast(cx, "The clipboard is empty".into()),
                },
                Action::Logout => self.ask(cx, "logout", json!({}), Ask::Write),
            }
        }
    }

    fn send(
        &mut self,
        cx: &mut Cx,
        chat: String,
        text: String,
        reply_to: Option<String>,
        client_id: String,
    ) {
        if let Some((edit_chat, message)) = self.editing.take()
            && edit_chat == chat
        {
            self.ask(
                cx,
                "edit",
                json!({"chat": chat, "message": message, "text": text}),
                Ask::Write,
            );
            return;
        }
        // The bubble shows at once, dimmed, until the bridge's echo replaces it.
        let account = self.model.status.account.clone().unwrap_or_default();
        let pending = Message {
            id: client_id.clone(),
            chat: chat.clone(),
            ts: now_ms(),
            author: Author {
                id: account.id,
                name: account.name,
                avatar: account.avatar,
            },
            mine: true,
            text: text.clone(),
            format: "discord".into(),
            status: Some("pending".into()),
            client_id: Some(client_id.clone()),
            ..Message::default()
        };
        self.model.upsert_message(pending);
        let mut args = json!({"chat": chat, "text": text, "client_id": client_id});
        if let Some(reply) = reply_to {
            args["reply_to"] = json!(reply);
        }
        self.ask(cx, "send", args, Ask::Send { chat, client_id });
        cx.redraw();
    }

    fn fail_pending(&mut self, chat: &str, client_id: &str) {
        let revision = self.model.revision();
        if let Some(history) = self.model.histories.get_mut(chat)
            && let Some(index) = history
                .entries
                .iter()
                .position(|e| e.message.client_id.as_deref() == Some(client_id))
        {
            let mut entry = (*history.entries[index]).clone();
            entry.message.status = Some("failed".into());
            entry.revision = revision;
            history.entries[index] = std::rc::Rc::new(entry);
            history.revision = revision;
        }
    }

    /// The page in front will be drawn whole next (something besides scrolling changed).
    fn dirty(&mut self) {
        if let Some(Page::Chat(view)) = self.stack.last_mut() {
            view.dirty = true;
        }
        self.messages.partial.dirty = true;
        self.servers.dirty = true;
    }

    fn chat_view(&mut self) -> Option<&mut ChatView> {
        match self.stack.last_mut() {
            Some(Page::Chat(view)) => Some(view),
            _ => None,
        }
    }

    fn tab_bar(&self, canvas: &mut Canvas, media: &mut Media, now: Instant) {
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        let top = height - theme::SAFE_BOTTOM - theme::TAB_H;
        canvas.fill_rect(
            0,
            top as i32,
            width as i32,
            (height - top) as i32,
            theme::RAIL,
            1.0,
        );
        canvas.fill_rect(0, top as i32, width as i32, 2, theme::HAIR, 1.0);
        let tabs = [
            (Tab::Servers, "Servers"),
            (Tab::Messages, "Messages"),
            (Tab::You, "You"),
        ];
        let slot = width / tabs.len() as f32;
        for (i, (tab, label)) in tabs.iter().enumerate() {
            let cx = slot * (i as f32 + 0.5);
            let cy = top + dp(20.0);
            let selected = *tab == self.tab;
            let color = if selected {
                theme::HEADER
            } else {
                theme::MUTED
            };
            match tab {
                Tab::Servers => {
                    ui::glyph(canvas, ui::Glyph::Logo, cx, cy, dp(24.0), color, 1.0);
                    canvas.fill_circle(cx - dp(3.6), cy + dp(1.5), dp(1.8), theme::RAIL, 1.0);
                    canvas.fill_circle(cx + dp(3.6), cy + dp(1.5), dp(1.8), theme::RAIL, 1.0);
                    let unread: u32 = self.model.guilds.iter().map(|g| g.mentions).sum();
                    if unread > 0 {
                        ui::badge(
                            canvas,
                            unread,
                            cx + dp(14.0),
                            cy - dp(8.0),
                            Some(theme::RAIL),
                        );
                    }
                }
                Tab::Messages => {
                    icons::draw(canvas, Icon::MessageSquare, cx, cy, dp(24.0), color, 1.0);
                    let unread = self.model.dm_unread();
                    if unread > 0 {
                        ui::badge(
                            canvas,
                            unread,
                            cx + dp(14.0),
                            cy - dp(8.0),
                            Some(theme::RAIL),
                        );
                    }
                }
                Tab::You => {
                    let size = dp(24.0);
                    let account = self
                        .model
                        .status
                        .account
                        .as_ref()
                        .and_then(|a| a.avatar.as_ref());
                    if selected {
                        canvas.fill_circle(cx, cy, size / 2.0 + dp(2.0), theme::HEADER, 1.0);
                    }
                    ui::avatar(
                        canvas,
                        media,
                        account,
                        cx - size / 2.0,
                        cy - size / 2.0,
                        size,
                        0x41434a,
                        now,
                    );
                }
            }
            text::draw_centered(
                canvas,
                label,
                cx,
                top + dp(46.0),
                Weight::Medium,
                theme::T_TINY * 0.95,
                color,
                1.0,
            );
        }
    }

    fn draw_viewer(&self, canvas: &mut Canvas, media: &mut Media, now: Instant) {
        let Some(viewer) = &self.viewer else { return };
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        canvas.fill_rect(0, 0, width as i32, height as i32, 0x000000, 1.0);
        let entry = self
            .model
            .histories
            .get(&viewer.chat)
            .and_then(|h| h.find(&viewer.message).map(|i| h.entries[i].clone()));
        let Some(entry) = entry else { return };
        let pictures: Vec<usize> = entry
            .message
            .media
            .iter()
            .enumerate()
            .filter(|(_, m)| matches!(m.kind.as_str(), "image" | "gif" | "video" | "sticker"))
            .map(|(i, _)| i)
            .collect();
        let Some(picture) = entry.message.media.get(viewer.index) else {
            return;
        };
        let shown = if picture.kind == "video" {
            picture.thumb.clone().unwrap_or_default()
        } else {
            picture.reference.clone()
        };
        let box_h = height - theme::TOP - theme::SAFE_BOTTOM - dp(60.0);
        let (w, h) = (width.round() as u32, box_h.round() as u32);
        match media.get(&shown, w, h, Fit::Contain, now) {
            Some(image) => {
                let x = (width - image.width as f32) / 2.0;
                let y = theme::TOP + (box_h - image.height as f32) / 2.0;
                canvas.draw_image(&image, x.round() as i32, y.round() as i32, 1.0);
            }
            None => text::draw_centered(
                canvas,
                "Loading…",
                width / 2.0,
                height / 2.0,
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            ),
        }
        icons::draw(
            canvas,
            Icon::X,
            dp(28.0),
            theme::TOP + dp(24.0),
            dp(24.0),
            theme::WHITE,
            1.0,
        );
        if pictures.len() > 1 {
            let at = pictures
                .iter()
                .position(|i| *i == viewer.index)
                .unwrap_or(0);
            text::draw_centered(
                canvas,
                &format!("{} / {}", at + 1, pictures.len()),
                width / 2.0,
                height - theme::SAFE_BOTTOM - dp(16.0),
                Weight::Medium,
                theme::T_SMALL,
                theme::WHITE,
                1.0,
            );
        }
        let name = format!(
            "{} · {}",
            entry.message.author.name,
            ui::message_time(entry.message.ts)
        );
        text::draw_centered(
            canvas,
            &ui::fit(&name, Weight::Medium, theme::T_SMALL, width - dp(120.0)),
            width / 2.0,
            theme::TOP + dp(30.0),
            Weight::Medium,
            theme::T_SMALL,
            theme::WHITE,
            1.0,
        );
        if picture.kind == "video" {
            text::draw_centered(
                canvas,
                "Videos open in the browser: tap",
                width / 2.0,
                height - theme::SAFE_BOTTOM - dp(40.0),
                Weight::Regular,
                theme::T_TINY,
                theme::MUTED,
                1.0,
            );
        }
    }

    fn viewer_gesture(&mut self, cx: &mut Cx, gesture: Gesture) {
        let Some(viewer) = &mut self.viewer else {
            return;
        };
        let entry = self
            .model
            .histories
            .get(&viewer.chat)
            .and_then(|h| h.find(&viewer.message).map(|i| h.entries[i].clone()));
        let Some(entry) = entry else {
            self.viewer = None;
            cx.redraw();
            return;
        };
        let pictures: Vec<usize> = entry
            .message
            .media
            .iter()
            .enumerate()
            .filter(|(_, m)| matches!(m.kind.as_str(), "image" | "gif" | "video" | "sticker"))
            .map(|(i, _)| i)
            .collect();
        let at = pictures
            .iter()
            .position(|i| *i == viewer.index)
            .unwrap_or(0);
        match gesture {
            Gesture::DragEnd {
                axis: Axis::Horizontal,
                vx,
                x,
                ..
            } => {
                let next = if vx < -300.0 || x < cx.width * 0.3 {
                    at + 1
                } else {
                    at.saturating_sub(1)
                };
                if let Some(index) = pictures.get(next.min(pictures.len().saturating_sub(1))) {
                    viewer.index = *index;
                }
                cx.redraw();
            }
            Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            }
            | Gesture::Back => {
                self.viewer = None;
                cx.redraw();
            }
            Gesture::Tap { .. } => {
                let media = &entry.message.media[viewer.index];
                if media.kind == "video"
                    && let Some(Some(url)) = entry.ext.urls.get(viewer.index)
                {
                    let url = url.clone();
                    if !cx.open_url(&url) {
                        self.toast(cx, "No browser to open the video".into());
                    }
                    return;
                }
                self.viewer = None;
                cx.redraw();
            }
            _ => {}
        }
    }

    fn tick_typing(&mut self, cx: &mut Cx) {
        let now = now_ms();
        let mut next: Option<i64> = None;
        let mut changed = false;
        for list in self.model.typing.values_mut() {
            let before = list.len();
            list.retain(|t| t.until_ms > now);
            changed |= list.len() != before;
            for t in list.iter() {
                next = Some(next.map_or(t.until_ms, |n| n.min(t.until_ms)));
            }
        }
        self.model.typing.retain(|_, list| !list.is_empty());
        if changed {
            self.dirty();
            cx.redraw();
        }
        // The dots move a few times a second while someone types in the open chat.
        if let Some(Page::Chat(view)) = self.stack.last_mut()
            && self.model.typing.contains_key(&view.chat)
        {
            view.dirty = true;
            cx.redraw();
            cx.wake_at(cx.now + Duration::from_millis(300));
        } else if let Some(at) = next {
            cx.wake_at(cx.now + Duration::from_millis((at - now).max(10) as u64));
        }
    }
}

impl App for DiscordApp {
    fn id(&self) -> &'static str {
        "discord"
    }

    fn status_bar(&self) -> (String, &'static str) {
        (self.page_color(), "light")
    }

    fn connected(&mut self, cx: &mut Cx, hello: &Hello) {
        self.model.connected = true;
        self.model.status = hello.status.clone();
        self.model.fixtures = hello.fixtures;
        self.ask(cx, "settings.get", json!({}), Ask::Settings);
        self.ask(cx, "chats.list", json!({"limit": 500}), Ask::Chats);
        self.ask(cx, "discord.guilds", json!({}), Ask::Guilds);
        // Everything open is asked again: events were missed while away.
        for chat in self.model.histories.keys().cloned().collect::<Vec<_>>() {
            self.model.stale.insert(chat);
        }
        let open: Vec<(u8, String)> = self
            .stack
            .iter()
            .map(|page| match page {
                Page::Chat(view) => (0, view.chat.clone()),
                Page::Forum(view) => (1, view.chat.clone()),
                Page::Members(view) => (2, view.chat.clone()),
            })
            .collect();
        for (kind, chat) in open {
            match kind {
                0 => {
                    self.model.stale.remove(&chat);
                    self.load_history(cx, &chat);
                }
                1 => self.ask(
                    cx,
                    "discord.threads",
                    json!({"chat": chat}),
                    Ask::Threads(chat),
                ),
                _ => self.ask(
                    cx,
                    "discord.member_list",
                    json!({"chat": chat}),
                    Ask::MemberList(chat),
                ),
            }
        }
        cx.redraw();
    }

    fn disconnected(&mut self, cx: &mut Cx) {
        self.model.connected = false;
        self.model.status.state = "connecting".into();
        for history in self.model.histories.values_mut() {
            history.loading = false;
            history.loading_older = false;
        }
        cx.redraw();
    }

    fn event(&mut self, cx: &mut Cx, event: Event) {
        self.dirty();
        match event {
            Event::Status(status) => self.model.status = status,
            Event::Chat(chat) => self.model.put_chat(*chat),
            Event::ChatRemoved(id) => self.model.remove_chat(&id),
            Event::Message { message, .. } => {
                if !self.model.upsert_message(*message) {
                    return;
                }
            }
            Event::MessageRemoved { chat, id } => {
                if !self.model.remove_message(&chat, &id) {
                    return;
                }
            }
            Event::Typing { chat, user, until } => {
                let list = self.model.typing.entry(chat).or_default();
                list.retain(|t| t.user != user.id);
                list.push(Typing {
                    user: user.id,
                    name: user.name,
                    until_ms: until,
                });
                cx.wake_at(cx.now);
            }
            Event::Login(value) => {
                let state = value.get("state").and_then(Value::as_str).unwrap_or("");
                self.you.login = Some(match state {
                    "waiting" => "Checking the token with Discord…".into(),
                    "done" => "Signed in".into(),
                    "failed" => value
                        .get("detail")
                        .and_then(Value::as_str)
                        .unwrap_or("Signing in failed")
                        .to_owned(),
                    other => other.to_owned(),
                });
            }
            Event::Other(name, value) => match name.as_str() {
                "discord.guilds" => self.model.set_guilds(&value["guilds"]),
                "discord.guild" => self.model.put_guild(&value["guild"]),
                "discord.guild_removed" => {
                    if let Some(id) = value["id"].as_str() {
                        self.model.remove_guild(id);
                    }
                }
                "discord.member_list" => {
                    if let Ok(list) =
                        serde_json::from_value::<super::model::MemberList>(value.clone())
                    {
                        self.model.member_lists.insert(list.chat.clone(), list);
                    }
                }
                _ => return,
            },
            Event::Read { .. } => return,
        }
        if let Some(target) = self.pending_target.take() {
            if self.model.chats.contains_key(&target) {
                self.open_chat(cx, target, true);
            } else {
                self.pending_target = Some(target);
            }
        }
        cx.redraw();
    }

    fn answer(&mut self, cx: &mut Cx, id: u64, result: Result<Value, BridgeError>) {
        let Some(ask) = self.asked.remove(&id) else {
            return;
        };
        self.dirty();
        match (ask, result) {
            (Ask::Chats, Ok(value)) => {
                let chats: Vec<Chat> =
                    serde_json::from_value(value["chats"].clone()).unwrap_or_default();
                let first_page = !self.asked.values().any(|a| matches!(a, Ask::Chats));
                let _ = first_page;
                for chat in chats {
                    self.model.put_chat(chat);
                }
                if let Some(cursor) = value["cursor"].as_str() {
                    self.ask(
                        cx,
                        "chats.list",
                        json!({"limit": 500, "cursor": cursor}),
                        Ask::Chats,
                    );
                }
                if let Some(target) = self.pending_target.take() {
                    if self.model.chats.contains_key(&target) {
                        self.open_chat(cx, target, true);
                    } else {
                        self.pending_target = Some(target);
                    }
                }
            }
            (Ask::Guilds, Ok(value)) => self.model.set_guilds(&value["guilds"]),
            (Ask::Settings, Ok(value)) => {
                self.model.settings = Settings {
                    writes: value["writes"].as_bool().unwrap_or(false),
                    notifications: value["notifications"].as_bool().unwrap_or(true),
                    previews: value["previews"].as_bool().unwrap_or(true),
                };
            }
            (Ask::Settings, Err(error)) => {
                let message = if error.code == "read_only" {
                    "This bridge runs read-only"
                } else {
                    "Couldn't change the setting"
                };
                self.toast(cx, message.into());
            }
            (Ask::History { chat, older }, Ok(value)) => {
                let messages: Vec<Message> =
                    serde_json::from_value(value["messages"].clone()).unwrap_or_default();
                let more = value["more"].as_bool().unwrap_or(false);
                self.model.page(&chat, messages, more, older);
            }
            (Ask::History { chat, older }, Err(error)) => {
                if let Some(history) = self.model.histories.get_mut(&chat) {
                    history.loading = false;
                    if older {
                        history.loading_older = false;
                    }
                    if !history.loaded && error.code != "disconnected" {
                        history.error = Some(match error.code.as_str() {
                            "not_found" => "You can't see this channel's messages.".into(),
                            "rate_limited" => {
                                "Discord asked to slow down; try again in a moment.".into()
                            }
                            _ => "Couldn't load messages.".into(),
                        });
                        history.loaded = true;
                    }
                }
            }
            (Ask::Threads(chat), Ok(value)) => {
                let threads: Vec<Chat> =
                    serde_json::from_value(value["threads"].clone()).unwrap_or_default();
                for thread in &threads {
                    if !self.model.chats.contains_key(&thread.id) {
                        // Known for the chat page's title; not one of the server's rows (not joined).
                        self.model
                            .chat_ext
                            .entry(thread.id.clone())
                            .or_insert_with(|| {
                                serde_json::from_value(thread.ext.clone()).unwrap_or_default()
                            });
                    }
                }
                self.model.threads.insert(chat, threads);
            }
            (Ask::MemberList(chat), Ok(value)) => {
                if let Ok(list) = serde_json::from_value::<super::model::MemberList>(value) {
                    self.model.member_lists.insert(chat, list);
                }
            }
            (Ask::Send { chat, client_id }, Ok(value)) => {
                if let Ok(message) = serde_json::from_value::<Message>(value["message"].clone()) {
                    let mut message = message;
                    message.client_id = Some(client_id);
                    if message.chat.is_empty() {
                        message.chat = chat;
                    }
                    self.model.upsert_message(message);
                }
            }
            (Ask::Send { chat, client_id }, Err(error)) => {
                self.fail_pending(&chat, &client_id);
                let message = match error.code.as_str() {
                    "read_only" => "Sending is off. Turn it on in You › Allow sending.",
                    "uncertain" => "It may not have been sent; check before trying again.",
                    _ => "Couldn't send the message.",
                };
                self.toast(cx, message.into());
            }
            (Ask::Write, Err(error)) => {
                let message = if error.code == "read_only" {
                    "Sending is off. Turn it on in You › Allow sending."
                } else {
                    "Discord didn't take that."
                };
                self.toast(cx, message.into());
            }
            (Ask::Login, Err(_)) => {
                if self.you.login.as_deref() == Some("Checking the token with Discord…") {
                    self.you.login = Some("Signing in failed".into());
                }
            }
            _ => {}
        }
        cx.redraw();
    }

    fn gesture(&mut self, cx: &mut Cx, gesture: Gesture) {
        let (width, height, now) = (cx.width, cx.height, cx.now);
        if self.viewer.is_some() {
            self.viewer_gesture(cx, gesture);
            if self.viewer.is_none() {
                self.dirty();
            }
            return;
        }
        let actions = match self.stack.last_mut() {
            Some(Page::Chat(view)) => view.gesture(gesture, &self.model, width, height, now),
            Some(Page::Forum(view)) => view.gesture(gesture, &self.model, width, height, now),
            Some(Page::Members(view)) => view.gesture(gesture, &self.model, width, height, now),
            None => {
                // The tab bar.
                let tab_top = height - theme::SAFE_BOTTOM - theme::TAB_H;
                if let Gesture::Tap { x, y } = gesture
                    && y >= tab_top
                    && !self.you.confirm_writes
                    && !self.you.confirm_logout
                {
                    let slot = width / 3.0;
                    let tab = match (x / slot) as usize {
                        0 => Tab::Servers,
                        1 => Tab::Messages,
                        _ => Tab::You,
                    };
                    if tab != self.tab {
                        self.tab = tab;
                        self.dirty();
                        cx.haptic(Haptic::Light);
                    }
                    cx.redraw();
                    return;
                }
                match self.tab {
                    Tab::Servers => self
                        .servers
                        .gesture(gesture, &self.model, width, height, now),
                    Tab::Messages => self.messages.gesture(gesture, &self.model, height, now),
                    Tab::You => self.you.gesture(gesture, &self.model, width, height),
                }
            }
        };
        self.act(cx, actions);
    }

    fn key(&mut self, cx: &mut Cx, key: KeyInput) {
        if let Some(view) = self.chat_view()
            && view.key(key)
        {
            cx.redraw();
        }
    }

    fn open_target(&mut self, cx: &mut Cx, target: &str) {
        self.viewer = None;
        let dm = self
            .model
            .chats
            .get(target)
            .map(|c| c.kind == "dm" || c.kind == "group");
        match dm {
            Some(true) => self.tab = Tab::Messages,
            Some(false) => {
                self.tab = Tab::Servers;
                if let Some(guild) = self.model.ext(target).and_then(|e| e.guild.clone()) {
                    self.servers.selected = Some(guild.clone());
                    self.servers.last_channel.insert(guild, target.to_owned());
                }
            }
            None => {
                self.pending_target = Some(target.to_owned());
                return;
            }
        }
        self.open_chat(cx, target.to_owned(), true);
    }

    fn shown(&mut self, cx: &mut Cx, shown: bool) {
        if shown {
            self.status_color = String::new();
            self.follow_status_bar();
            cx.redraw();
        }
    }

    fn tick(&mut self, cx: &mut Cx) {
        let (width, height, now) = (cx.width, cx.height, cx.now);
        let moving = match self.stack.last_mut() {
            Some(Page::Chat(view)) => {
                view.sync(&self.model, width, height);
                let moving = view.tick(width, height, now);
                if let Some(before) = view.wants_older(&self.model, width, height, now) {
                    let chat = view.chat.clone();
                    if let Some(history) = self.model.histories.get_mut(&chat) {
                        history.loading_older = true;
                    }
                    self.ask(
                        cx,
                        "history",
                        json!({"chat": chat, "before": before, "limit": 50}),
                        Ask::History { chat, older: true },
                    );
                }
                moving
            }
            Some(Page::Forum(view)) => view.tick(&self.model, height, now),
            Some(Page::Members(view)) => view.tick(&self.model, height, now),
            None => match self.tab {
                Tab::Servers => self.servers.tick(&self.model, height, now),
                Tab::Messages => self.messages.tick(&self.model, height, now),
                Tab::You => false,
            },
        };
        if moving {
            cx.animate();
        }
        if let Some((_, at)) = &self.toast
            && now.duration_since(*at) >= TOAST_FOR
        {
            self.toast = None;
            self.dirty();
            cx.redraw();
        }
        self.tick_typing(cx);
    }

    fn viewing(&self) -> Option<String> {
        match self.stack.last() {
            Some(Page::Chat(view)) => Some(view.chat.clone()),
            _ => None,
        }
    }

    fn draw(&mut self, canvas: &mut Canvas, cx: &mut DrawCx) {
        let started = Instant::now();
        let now = cx.now;
        let page = match self.stack.last_mut() {
            Some(Page::Chat(view)) => {
                view.sync(&self.model, canvas.width as f32, canvas.height as f32);
                // Overlays over the messages (a toast, the viewer) need whole frames.
                let full = cx.full || self.toast.is_some() || self.viewer.is_some();
                cx.damage = view.draw(canvas, &self.model, cx.media, now, full);
                "chat"
            }
            Some(Page::Forum(view)) => {
                view.draw(canvas, &self.model, now);
                "forum"
            }
            Some(Page::Members(view)) => {
                view.draw(canvas, &self.model, cx.media, now);
                "members"
            }
            None => {
                let mut whole = true;
                let name = match self.tab {
                    Tab::Servers => {
                        let full = cx.full || self.toast.is_some() || self.viewer.is_some();
                        whole = self.servers.draw(canvas, &self.model, cx.media, now, full);
                        "servers"
                    }
                    Tab::Messages => {
                        let full = cx.full || self.toast.is_some() || self.viewer.is_some();
                        cx.damage = self.messages.draw(canvas, &self.model, cx.media, now, full);
                        whole = cx.damage.is_none();
                        "messages"
                    }
                    Tab::You => {
                        self.you.draw(canvas, &self.model, cx.media, now);
                        "you"
                    }
                };
                // The tab bar, when the page was drawn whole or a badge changed.
                let key = (
                    self.tab,
                    self.model.dm_unread(),
                    self.model.guilds.iter().map(|g| g.mentions).sum::<u32>(),
                );
                if whole || self.last_tab_bar != Some(key) {
                    self.last_tab_bar = Some(key);
                    self.tab_bar(canvas, cx.media, now);
                }
                name
            }
        };
        if self.model.status.state != "online"
            && self.model.status.state != "signed_out"
            && self.viewer.is_none()
        {
            let label = match self.model.status.state.as_str() {
                "offline" => "Offline: waiting for the network",
                "error" => "The Discord bridge needs attention (You)",
                _ => {
                    if self.model.connected {
                        "Connecting to Discord…"
                    } else {
                        "Connecting to the bridge…"
                    }
                }
            };
            let w = canvas.width as f32;
            let bar = RectF {
                x: 0.0,
                y: theme::TOP,
                w,
                h: dp(24.0),
            };
            canvas.fill_rect(
                0,
                bar.y as i32,
                w as i32,
                bar.h as i32,
                if self.model.status.state == "error" {
                    theme::RED
                } else {
                    0x4e5058
                },
                1.0,
            );
            text::draw_centered(
                canvas,
                label,
                w / 2.0,
                text::centered_baseline(bar.y + bar.h / 2.0, Weight::Medium, theme::T_TINY),
                Weight::Medium,
                theme::T_TINY,
                theme::WHITE,
                1.0,
            );
        }
        self.draw_viewer(canvas, cx.media, now);
        if let Some((message, _)) = &self.toast {
            let (width, height) = (canvas.width as f32, canvas.height as f32);
            let w = (text::measure(message, Weight::Medium, theme::T_SMALL) + dp(32.0))
                .min(width - dp(24.0));
            let rect = RectF {
                x: (width - w) / 2.0,
                y: height - theme::SAFE_BOTTOM - theme::TAB_H - dp(56.0),
                w,
                h: dp(40.0),
            };
            canvas.fill_rrect(rect.inset(-dp(1.5)), dp(11.5), 0x000000, 0.35);
            canvas.fill_rrect(rect, dp(10.0), 0x111214, 1.0);
            let label = ui::fit(message, Weight::Medium, theme::T_SMALL, w - dp(24.0));
            text::draw_centered(
                canvas,
                &label,
                width / 2.0,
                text::centered_baseline(rect.y + rect.h / 2.0, Weight::Medium, theme::T_SMALL),
                Weight::Medium,
                theme::T_SMALL,
                theme::HEADER,
                1.0,
            );
        }
        if let Some(trace) = &mut self.trace {
            let us = started.elapsed().as_micros();
            let _ = writeln!(
                trace.out,
                "{:.1} draw_us={us} page={page}",
                trace.started.elapsed().as_secs_f64() * 1000.0
            );
            trace.lines += 1;
            if trace.lines % 20 == 0 {
                let _ = trace.out.flush();
            }
        }
    }
}
