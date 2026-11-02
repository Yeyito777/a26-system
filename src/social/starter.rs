//! A plain chat client on the shared pieces: the chat list and a chat's
//! bubbles, from any bridge. It is where each service's app starts from
//! (and what proves the bridge, the link, the pictures and the shell work
//! together); the apps replace it with their service's own look.

#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Instant;

use serde_json::{Value, json};

use super::media::Fit;
use super::proto::{BridgeError, Chat, Event, Hello, Message, Status};
use super::shell::{self, App, Cx, DrawCx};
use crate::comms::Scroll;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::gfx::{Canvas, RectF, Rgb};
use crate::text::{self, Weight};

/// The colours a service's starter is drawn in.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub background: Rgb,
    pub bar: Rgb,
    pub text: Rgb,
    pub secondary: Rgb,
    pub accent: Rgb,
    pub mine: Rgb,
    pub theirs: Rgb,
    pub badge: Rgb,
}

const TOP: f32 = 124.0;
const BAR_H: f32 = 150.0;
const ROW_H: f32 = 190.0;
const AVATAR: f32 = 130.0;
const RESERVE: f32 = 170.0;
const BUBBLE_TEXT: f32 = 44.0;
const BUBBLE_LINE: f32 = 58.0;
const BUBBLE_MAX: f32 = 760.0;
const PHOTO_MAX: f32 = 640.0;
const MARGIN: f32 = 40.0;

#[derive(Debug, Clone, PartialEq)]
enum Page {
    List,
    Chat(String),
}

pub struct Starter {
    id: &'static str,
    theme: Theme,
    status: Status,
    chats: Vec<Chat>,
    messages: Vec<Message>,
    more: bool,
    page: Page,
    list_scroll: Scroll,
    chat_scroll: Scroll,
    /// Requests in flight and what they were for.
    asked: HashMap<u64, Ask>,
    loading_older: bool,
    /// The chat page's content height and where it sat (laid out at draw).
    chat_height: f32,
}

enum Ask {
    List,
    History { chat: String, older: bool },
}

impl Starter {
    pub fn new(id: &'static str, theme: Theme) -> Self {
        Self {
            id,
            theme,
            status: Status::default(),
            chats: Vec::new(),
            messages: Vec::new(),
            more: false,
            page: Page::List,
            list_scroll: Scroll::default(),
            chat_scroll: Scroll::default(),
            asked: HashMap::new(),
            loading_older: false,
            chat_height: 0.0,
        }
    }

    fn list_top() -> f32 {
        TOP + BAR_H
    }

    fn list_max(&self, height: f32) -> f32 {
        let visible = height - Self::list_top() - RESERVE;
        (self.chats.len() as f32 * ROW_H - visible).max(0.0)
    }

    fn chat_max(&self, height: f32) -> f32 {
        let visible = height - Self::list_top() - RESERVE;
        (self.chat_height - visible).max(0.0)
    }

    fn sort(&mut self) {
        self.chats.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.ts.cmp(&a.ts))
                .then(a.id.cmp(&b.id))
        });
    }

    fn open(&mut self, cx: &mut Cx, chat: String) {
        cx.transition(true);
        self.page = Page::Chat(chat.clone());
        self.messages.clear();
        self.more = false;
        self.chat_scroll.reset();
        let id = cx.request("history", json!({"chat": chat, "limit": 40}));
        self.asked.insert(id, Ask::History { chat, older: false });
    }

    fn back(&mut self, cx: &mut Cx) {
        if self.page != Page::List {
            cx.transition(false);
            self.page = Page::List;
            cx.redraw();
        }
    }

    fn row_at(&self, y: f32, now: Instant) -> Option<usize> {
        let top = Self::list_top();
        if y < top {
            return None;
        }
        let index = ((y - top + self.list_scroll.value(now)) / ROW_H) as usize;
        (index < self.chats.len()).then_some(index)
    }
}

impl App for Starter {
    fn id(&self) -> &'static str {
        self.id
    }

    fn status_bar(&self) -> (String, &'static str) {
        (format!("#{:06x}", self.theme.bar), "light")
    }

    fn connected(&mut self, cx: &mut Cx, hello: &Hello) {
        self.status = hello.status.clone();
        let id = cx.request("chats.list", json!({"limit": 200}));
        self.asked.insert(id, Ask::List);
        if let Page::Chat(chat) = &self.page {
            let chat = chat.clone();
            let id = cx.request("history", json!({"chat": chat, "limit": 40}));
            self.asked.insert(id, Ask::History { chat, older: false });
        }
        cx.redraw();
    }

    fn disconnected(&mut self, cx: &mut Cx) {
        self.status.state = "connecting".into();
        self.loading_older = false;
        cx.redraw();
    }

    fn event(&mut self, cx: &mut Cx, event: Event) {
        match event {
            Event::Status(status) => self.status = status,
            Event::Chat(chat) => {
                match self.chats.iter_mut().find(|known| known.id == chat.id) {
                    Some(known) => *known = *chat,
                    None => self.chats.push(*chat),
                }
                self.sort();
            }
            Event::ChatRemoved(id) => self.chats.retain(|chat| chat.id != id),
            Event::Message { message, .. } => {
                if self.page == Page::Chat(message.chat.clone()) {
                    match self
                        .messages
                        .iter_mut()
                        .find(|known| known.id == message.id)
                    {
                        Some(known) => *known = *message,
                        None => {
                            self.messages.push(*message);
                            self.messages.sort_by_key(|m| m.ts);
                        }
                    }
                }
            }
            Event::MessageRemoved { id, .. } => self.messages.retain(|m| m.id != id),
            _ => return,
        }
        cx.redraw();
    }

    fn answer(&mut self, cx: &mut Cx, id: u64, result: Result<Value, BridgeError>) {
        let Some(ask) = self.asked.remove(&id) else {
            return;
        };
        let Ok(value) = result else {
            if let Ask::History { older: true, .. } = ask {
                self.loading_older = false;
            }
            return;
        };
        match ask {
            Ask::List => {
                if let Ok(chats) = serde_json::from_value::<Vec<Chat>>(value["chats"].clone()) {
                    self.chats = chats;
                    self.sort();
                }
            }
            Ask::History { chat, older } => {
                if self.page != Page::Chat(chat) {
                    return;
                }
                let page: Vec<Message> =
                    serde_json::from_value(value["messages"].clone()).unwrap_or_default();
                self.more = value["more"].as_bool().unwrap_or(false);
                if older {
                    self.loading_older = false;
                    let mut merged = page;
                    merged.retain(|m| !self.messages.iter().any(|k| k.id == m.id));
                    merged.append(&mut self.messages);
                    self.messages = merged;
                } else {
                    self.messages = page;
                }
            }
        }
        cx.redraw();
    }

    fn gesture(&mut self, cx: &mut Cx, gesture: Gesture) {
        let now = cx.now;
        let height = cx.height;
        match gesture {
            Gesture::Tap { x, y } => {
                if y < TOP + BAR_H && x < 300.0 && self.page != Page::List {
                    self.back(cx);
                } else if self.page == Page::List
                    && let Some(index) = self.row_at(y, now)
                {
                    let chat = self.chats[index].id.clone();
                    self.open(cx, chat);
                }
            }
            Gesture::Back => self.back(cx),
            Gesture::DragStart {
                axis: Axis::Vertical,
                y,
                start_y,
                ..
            } => {
                let max_list = self.list_max(height);
                let max_chat = self.chat_max(height);
                match self.page {
                    Page::List => {
                        self.list_scroll.begin(start_y, now);
                        self.list_scroll.moved(y, max_list, now);
                    }
                    Page::Chat(_) => {
                        // The chat scrolls from the bottom: dragging down shows older.
                        self.chat_scroll.begin(-start_y, now);
                        self.chat_scroll.moved(-y, max_chat, now);
                    }
                }
                cx.redraw();
            }
            Gesture::Drag {
                axis: Axis::Vertical,
                y,
                ..
            } => {
                match self.page {
                    Page::List => self.list_scroll.moved(y, self.list_max(height), now),
                    Page::Chat(_) => self.chat_scroll.moved(-y, self.chat_max(height), now),
                }
                cx.redraw();
            }
            Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            } => {
                match self.page {
                    Page::List => self.list_scroll.end(self.list_max(height), now),
                    Page::Chat(_) => self.chat_scroll.end(self.chat_max(height), now),
                }
                cx.animate();
            }
            Gesture::DragEnd {
                axis: Axis::Horizontal,
                x,
                ..
            } if x > cx.width * 0.4 => self.back(cx),
            _ => {}
        }
    }

    fn open_target(&mut self, cx: &mut Cx, target: &str) {
        self.open(cx, target.to_owned());
    }

    fn tick(&mut self, cx: &mut Cx) {
        let (now, height) = (cx.now, cx.height);
        let moving = match self.page {
            Page::List => self.list_scroll.step(self.list_max(height), now),
            Page::Chat(_) => self.chat_scroll.step(self.chat_max(height), now),
        };
        if moving {
            cx.animate();
        }
        // Older history once the top is near.
        if let Page::Chat(chat) = &self.page
            && self.more
            && !self.loading_older
            && cx.connected
            && self.chat_scroll.value(now) > self.chat_max(height) - 1200.0
            && let Some(first) = self.messages.first()
        {
            let chat = chat.clone();
            let id = cx.request(
                "history",
                json!({"chat": chat, "before": first.id, "limit": 40}),
            );
            self.asked.insert(id, Ask::History { chat, older: true });
            self.loading_older = true;
        }
    }

    fn viewing(&self) -> Option<String> {
        match &self.page {
            Page::Chat(chat) => Some(chat.clone()),
            Page::List => None,
        }
    }

    fn draw(&mut self, canvas: &mut Canvas, cx: &mut DrawCx) {
        let theme = self.theme;
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        canvas.clear(theme.background);
        match self.page.clone() {
            Page::List => self.draw_list(canvas, cx, width, height),
            Page::Chat(chat) => self.draw_chat(canvas, cx, &chat, width, height),
        }
        // The bar over the content.
        canvas.fill_rect(0, 0, width as i32, (TOP + BAR_H) as i32, theme.bar, 1.0);
        let title = match &self.page {
            Page::List => theme.name.to_owned(),
            Page::Chat(id) => self
                .chats
                .iter()
                .find(|chat| &chat.id == id)
                .map_or_else(String::new, |chat| chat.title.clone()),
        };
        let baseline = TOP + BAR_H / 2.0 + 18.0;
        if self.page != Page::List {
            text::draw(
                canvas,
                "‹",
                MARGIN,
                baseline + 6.0,
                Weight::Regular,
                80.0,
                theme.accent,
                1.0,
            );
        }
        let title = text::fit(&title, Weight::SemiBold, 52.0, width - 360.0);
        text::draw_centered(
            canvas,
            &title,
            width / 2.0,
            baseline,
            Weight::SemiBold,
            52.0,
            theme.text,
            1.0,
        );
        if !self.status.online() {
            let note = match self.status.state.as_str() {
                "signed_out" => "Not signed in",
                "offline" => "Offline",
                "error" => "Needs attention",
                _ => "Connecting…",
            };
            text::draw_centered(
                canvas,
                note,
                width / 2.0,
                TOP + BAR_H - 14.0,
                Weight::Regular,
                32.0,
                theme.secondary,
                1.0,
            );
        }
    }
}

impl Starter {
    fn draw_list(&self, canvas: &mut Canvas, cx: &mut DrawCx, width: f32, height: f32) {
        let theme = self.theme;
        let offset = self.list_scroll.value(cx.now);
        let top = Self::list_top();
        let first = (offset / ROW_H).floor().max(0.0) as usize;
        for (index, chat) in self.chats.iter().enumerate().skip(first) {
            let y = top + index as f32 * ROW_H - offset;
            if y > height {
                break;
            }
            let cy = y + ROW_H / 2.0;
            let size = AVATAR as u32;
            let picture = chat.avatar.as_ref().and_then(|avatar| {
                cx.media
                    .get(&avatar.reference, size, size, Fit::Cover, cx.now)
            });
            match picture {
                Some(image) => {
                    canvas.draw_image(&image, MARGIN as i32, (cy - AVATAR / 2.0) as i32, 1.0)
                }
                None => {
                    canvas.fill_circle(MARGIN + AVATAR / 2.0, cy, AVATAR / 2.0, theme.theirs, 1.0)
                }
            }
            let x = MARGIN + AVATAR + 32.0;
            let right = width - MARGIN;
            let title = text::fit(&chat.title, Weight::SemiBold, 46.0, right - x - 200.0);
            text::draw(
                canvas,
                &title,
                x,
                cy - 12.0,
                Weight::SemiBold,
                46.0,
                theme.text,
                1.0,
            );
            let when = time_text(chat.ts);
            text::draw_right(
                canvas,
                &when,
                right,
                cy - 12.0,
                Weight::Regular,
                34.0,
                theme.secondary,
                1.0,
            );
            let preview = chat.last.as_ref().map_or(String::new(), |last| {
                if chat.is_group() && !last.mine {
                    format!("{}: {}", last.author.clone().unwrap_or_default(), last.text)
                } else if last.mine {
                    format!("You: {}", last.text)
                } else {
                    last.text.clone()
                }
            });
            let preview = text::fit(
                &preview.replace('\n', " "),
                Weight::Regular,
                38.0,
                right - x - 110.0,
            );
            text::draw(
                canvas,
                &preview,
                x,
                cy + 46.0,
                Weight::Regular,
                38.0,
                theme.secondary,
                1.0,
            );
            if chat.unread > 0 {
                let label = if chat.unread > 99 {
                    "99+".to_owned()
                } else {
                    chat.unread.to_string()
                };
                let w = text::measure(&label, Weight::SemiBold, 30.0).max(28.0) + 28.0;
                let rect = RectF {
                    x: right - w,
                    y: cy + 14.0,
                    w,
                    h: 50.0,
                };
                let color = if chat.muted {
                    theme.secondary
                } else {
                    theme.badge
                };
                canvas.fill_rrect(rect, 25.0, color, 1.0);
                text::draw_centered(
                    canvas,
                    &label,
                    right - w / 2.0,
                    cy + 50.0,
                    Weight::SemiBold,
                    30.0,
                    0xffffff,
                    1.0,
                );
            }
        }
        if self.chats.is_empty() {
            let note = if self.status.signed_out() {
                "Sign in on the bridge to see your chats."
            } else {
                "No chats yet"
            };
            text::draw_centered(
                canvas,
                note,
                width / 2.0,
                height / 2.0,
                Weight::Regular,
                40.0,
                theme.secondary,
                1.0,
            );
        }
    }

    fn draw_chat(
        &mut self,
        canvas: &mut Canvas,
        cx: &mut DrawCx,
        chat: &str,
        width: f32,
        height: f32,
    ) {
        let theme = self.theme;
        let group = self
            .chats
            .iter()
            .find(|c| c.id == chat)
            .is_some_and(Chat::is_group);
        // Lay out from the newest (bottom) up.
        let bottom = height - RESERVE;
        let offset = self.chat_scroll.value(cx.now);
        let mut y = bottom + offset;
        let mut total = 0.0;
        for message in self.messages.iter().rev() {
            let lines = wrap(&message.text, BUBBLE_MAX - 64.0);
            let photo = message
                .media
                .iter()
                .find(|m| m.kind == "image" || m.kind == "sticker");
            let (pw, ph) = photo.map_or((0.0, 0.0), |m| {
                let aspect = m.aspect().unwrap_or(1.0);
                if aspect >= 1.0 {
                    (PHOTO_MAX, PHOTO_MAX / aspect)
                } else {
                    (PHOTO_MAX * aspect, PHOTO_MAX)
                }
            });
            let name_h = if group && !message.mine { 44.0 } else { 0.0 };
            let text_h = if message.text.is_empty() {
                0.0
            } else {
                lines.len() as f32 * BUBBLE_LINE
            };
            let bubble_h = name_h + text_h + ph + 40.0;
            let text_w = lines
                .iter()
                .map(|l| text::measure(l, Weight::Regular, BUBBLE_TEXT))
                .fold(0.0, f32::max);
            let bubble_w = (text_w + 64.0).max(pw + 24.0).clamp(160.0, BUBBLE_MAX);
            y -= bubble_h + 16.0;
            total += bubble_h + 16.0;
            if y > bottom || y + bubble_h < Self::list_top() {
                continue;
            }
            let x = if message.mine {
                width - MARGIN - bubble_w
            } else {
                MARGIN
            };
            let rect = RectF {
                x,
                y,
                w: bubble_w,
                h: bubble_h,
            };
            canvas.fill_rrect(
                rect,
                36.0,
                if message.mine {
                    theme.mine
                } else {
                    theme.theirs
                },
                1.0,
            );
            let mut line_y = y + 20.0;
            if name_h > 0.0 {
                text::draw(
                    canvas,
                    &message.author.name,
                    x + 32.0,
                    line_y + 32.0,
                    Weight::SemiBold,
                    34.0,
                    theme.accent,
                    1.0,
                );
                line_y += name_h;
            }
            if let Some(photo) = photo {
                let (w, h) = (pw.round() as u32, ph.round() as u32);
                match cx.media.get(&photo.reference, w, h, Fit::Contain, cx.now) {
                    Some(image) => canvas.draw_image(&image, (x + 12.0) as i32, line_y as i32, 1.0),
                    None => canvas.fill_rect(
                        (x + 12.0) as i32,
                        line_y as i32,
                        w as i32,
                        h as i32,
                        theme.background,
                        0.5,
                    ),
                }
                line_y += ph;
            }
            for line in &lines {
                if message.text.is_empty() {
                    break;
                }
                text::draw(
                    canvas,
                    line,
                    x + 32.0,
                    line_y + 44.0,
                    Weight::Regular,
                    BUBBLE_TEXT,
                    theme.text,
                    1.0,
                );
                line_y += BUBBLE_LINE;
            }
        }
        self.chat_height = total + 40.0;
        if self.messages.is_empty() {
            text::draw_centered(
                canvas,
                "Loading…",
                width / 2.0,
                height / 2.0,
                Weight::Regular,
                40.0,
                theme.secondary,
                1.0,
            );
        }
    }
}

fn wrap(text_in: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text_in.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if line.is_empty() || text::measure(&candidate, Weight::Regular, BUBBLE_TEXT) <= width {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_owned();
            }
            if text::measure(&line, Weight::Regular, BUBBLE_TEXT) > width {
                lines.push(text::fit(&line, Weight::Regular, BUBBLE_TEXT, width));
                line.clear();
            }
        }
        lines.push(line);
    }
    while lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// `14:05` today, else the day.
fn time_text(ts_ms: i64) -> String {
    let t = ts_ms / 1000;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let tm = local(t);
    if now - t < 20 * 3600 && local(now).tm_yday == tm.tm_yday {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        format!("{}/{}", tm.tm_mon + 1, tm.tm_mday)
    }
}

fn local(t: i64) -> libc::tm {
    // SAFETY: localtime_r writes into a zeroed tm it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = t as _;
        libc::localtime_r(&time, &mut tm);
        tm
    }
}

/// Runs the starter as `id`'s app.
pub fn run(id: &'static str, theme: Theme) -> Result<(), Box<dyn std::error::Error>> {
    shell::run(Starter::new(id, theme))
}
