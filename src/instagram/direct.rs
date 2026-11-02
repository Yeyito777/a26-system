//! Messages: the inbox (Instagram's list of chats, requests apart) and a
//! chat (bubbles grouped by sender, time separators, replies, reactions,
//! shared posts, reels and stories as cards, photos, voice notes, calls,
//! the Seen line, the composer). A chat is laid out once per message and
//! drawn from the bottom up, so it opens at the newest message.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::{Value, json};

use super::app::{Action, Area, Screen};
use super::model;
use super::paint::{
    self, BG, BLUE, BODY, BODY_BOLD, DP, ELEVATED, FIELD, Font, Icon, Line, MARGIN, MESSAGE, Paint,
    RED, Ring, SAFE_BOTTOM, SECONDARY, SEPARATOR, SMALL, SMALL_BOLD, STATUS, TAB_BAR, TERTIARY,
    TEXT, TINY, TOP_BAR,
};
use super::profile::ProfileScreen;
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::control::KeyboardPurpose;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::social::media::Fit;
use crate::social::proto::{Chat, Message};
use crate::text::Weight;

/// Own messages' bubble (Instagram's purple-blue).
const MINE: u32 = 0x5851db;
const ROW: f32 = 189.0;
const AVATAR: f32 = 56.0 * DP;

fn top_of_content() -> f32 {
    STATUS + TOP_BAR
}

// ----- The inbox ----------------------------------------------------------------

pub struct InboxScreen {
    scroll: Scroll,
    requests: bool,
}

const SEARCH_H: f32 = 95.0;
const HEADER_H: f32 = SEARCH_H + 34.0 + 120.0;

impl InboxScreen {
    pub fn new() -> Self {
        Self {
            scroll: Scroll::default(),
            requests: false,
        }
    }

    pub fn requests() -> Self {
        Self {
            scroll: Scroll::default(),
            requests: true,
        }
    }

    fn rows<'a>(&self, store: &'a Store) -> Vec<&'a Chat> {
        if self.requests {
            store.requests().collect()
        } else {
            store.inbox().collect()
        }
    }

    fn header(&self) -> f32 {
        if self.requests { 24.0 } else { HEADER_H }
    }
}

/// Instagram's second line for a chat row.
pub fn preview(chat: &Chat, now: i64) -> (String, bool) {
    let unread = chat.unread > 0;
    let Some(last) = &chat.last else {
        return (String::new(), false);
    };
    let age = paint::age_short(last.ts, now);
    if unread {
        let text = if chat.unread >= 2 {
            format!(
                "{} new messages",
                if chat.unread > 4 {
                    "4+".to_owned()
                } else {
                    chat.unread.to_string()
                }
            )
        } else if chat.is_group() {
            format!("{}: {}", last.author.clone().unwrap_or_default(), last.text)
        } else {
            last.text.clone()
        };
        return (format!("{text} · {age}"), true);
    }
    if last.mine {
        let seen = chat
            .ext
            .get("seen")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if seen && !chat.is_group() {
            return (format!("Seen {age} ago"), false);
        }
        return (format!("You: {} · {age}", last.text), false);
    }
    let text = if chat.is_group() {
        format!("{}: {}", last.author.clone().unwrap_or_default(), last.text)
    } else {
        last.text.clone()
    };
    (format!("{text} · {age}"), false)
}

fn chat_ring(chat: &Chat) -> Ring {
    if chat.is_group() {
        return Ring::None;
    }
    let users = model::chat_users(&chat.ext);
    Ring::from_story(users.first().and_then(|user| user.story.as_deref()))
}

fn draw_chat_avatar(canvas: &mut Canvas, p: &mut Paint, chat: &Chat, cx: f32, cy: f32, d: f32) {
    let users = model::chat_users(&chat.ext);
    if chat.is_group() && users.len() >= 2 {
        // Two faces, the second over the first's corner.
        let small = d * 0.72;
        paint::avatar(
            canvas,
            p,
            users[1].avatar_ref(),
            cx - d * 0.14,
            cy - d * 0.14,
            small,
            Ring::None,
        );
        canvas.fill_circle(cx + d * 0.14, cy + d * 0.14, small / 2.0 + 5.0, BG, 1.0);
        paint::avatar(
            canvas,
            p,
            users[0].avatar_ref(),
            cx + d * 0.14,
            cy + d * 0.14,
            small,
            Ring::None,
        );
    } else {
        let reference = chat.avatar.as_ref().map(|m| m.reference.as_str());
        paint::avatar(canvas, p, reference, cx, cy, d, chat_ring(chat));
    }
}

impl Screen for InboxScreen {
    fn viewport(&self, height: f32) -> (f32, f32) {
        let bottom = if self.requests {
            height - SAFE_BOTTOM
        } else {
            height - SAFE_BOTTOM - TAB_BAR
        };
        (top_of_content(), bottom)
    }

    fn scroll(&mut self) -> Option<&mut Scroll> {
        Some(&mut self.scroll)
    }

    fn layout(&mut self, store: &Store, _width: f32) -> f32 {
        self.header() + self.rows(store).len() as f32 * ROW + 40.0
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        if super::signin::wanted(store) && store.chats.is_empty() {
            super::signin::draw(canvas, p, store, area);
            return;
        }
        let now = paint::now_ms();
        let y0 = area.top - area.offset;
        if !self.requests && area.visible(y0, y0 + HEADER_H) {
            let field = RectF::new(MARGIN, y0 + 16.0, area.width - 2.0 * MARGIN, SEARCH_H);
            canvas.fill_rrect(field, 10.0 * DP, FIELD, 1.0);
            paint::icon(
                canvas,
                p.cache,
                Icon::Search,
                field.x + 30.0,
                field.y + (SEARCH_H - 44.0) / 2.0,
                44.0,
                false,
                SECONDARY,
            );
            paint::draw_text(
                canvas,
                "Search",
                field.x + 96.0,
                paint::baseline(field.y + SEARCH_H / 2.0, BODY),
                BODY,
                SECONDARY,
            );
            let hy = y0 + SEARCH_H + 34.0 + 60.0;
            paint::draw_text(
                canvas,
                "Messages",
                MARGIN,
                paint::baseline(hy, paint::HEADLINE),
                paint::HEADLINE,
                TEXT,
            );
            let requests = store.requests().count();
            let label = if requests > 0 {
                format!("Requests ({requests})")
            } else {
                "Requests".to_owned()
            };
            paint::draw_text_right(
                canvas,
                &label,
                area.width - MARGIN,
                paint::baseline(hy, BODY_BOLD),
                BODY_BOLD,
                BLUE,
            );
        }
        let rows = self.rows(store);
        let start = y0 + self.header();
        let first = (((area.band.0 - start) / ROW).floor().max(0.0)) as usize;
        for (index, chat) in rows.iter().enumerate().skip(first) {
            let y = start + index as f32 * ROW;
            if y > area.band.1 {
                break;
            }
            draw_row(canvas, p, chat, y, area.width, now);
        }
        if rows.is_empty() {
            let note = if store.status.signed_out() {
                "Sign in to see your messages"
            } else if !store.chats_loaded {
                ""
            } else if self.requests {
                "No message requests"
            } else {
                "No messages yet"
            };
            paint::draw_text_centered(
                canvas,
                note,
                area.width / 2.0,
                start + 300.0,
                BODY,
                SECONDARY,
            );
        }
        // The next rows' avatars, ahead of the scroll.
        let last = ((area.bottom - start) / ROW).ceil().max(0.0) as usize;
        for chat in rows.iter().skip(last).take(6) {
            if let Some(avatar) = &chat.avatar {
                let size = (AVATAR - 2.0 * (2.5 * DP + 3.0 * DP)).round() as u32;
                p.prefetch(&avatar.reference, size, size, Fit::Cover);
            }
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        let cy = STATUS + TOP_BAR / 2.0;
        if self.requests {
            paint::icon(
                canvas,
                p.cache,
                Icon::Back,
                MARGIN - 10.0,
                cy - 36.0,
                72.0,
                false,
                TEXT,
            );
            paint::draw_text(
                canvas,
                "Message requests",
                MARGIN + 92.0,
                paint::baseline(cy, paint::TITLE),
                paint::TITLE,
                TEXT,
            );
            return;
        }
        let name = store
            .account()
            .map(|account| {
                account
                    .username
                    .clone()
                    .unwrap_or_else(|| account.name.clone())
            })
            .unwrap_or_else(|| "Messages".to_owned());
        let font = paint::TITLE;
        let w = paint::draw_text(canvas, &name, MARGIN, paint::baseline(cy, font), font, TEXT);
        paint::icon(
            canvas,
            p.cache,
            Icon::ChevronDown,
            MARGIN + w + 10.0,
            cy - 18.0,
            40.0,
            false,
            TEXT,
        );
        paint::icon(
            canvas,
            p.cache,
            Icon::Compose,
            area.width - MARGIN - 66.0,
            cy - 33.0,
            66.0,
            false,
            TEXT,
        );
        if !store.status.online() && store.status.state != "connecting" {
            let note = status_note(store);
            paint::draw_text_right(
                canvas,
                &note,
                area.width - MARGIN - 100.0,
                paint::baseline(cy, TINY),
                TINY,
                SECONDARY,
            );
        }
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            if self.requests && x < 300.0 {
                return Action::Pop;
            }
            return Action::None;
        }
        if super::signin::wanted(store) && store.chats.is_empty() {
            return super::signin::tap(area, x, y);
        }
        let content_y = y - area.top + area.offset;
        if !self.requests && content_y < HEADER_H {
            if content_y > SEARCH_H + 34.0 && x > area.width * 0.6 {
                return Action::Push(Box::new(InboxScreen::requests()));
            }
            return Action::Tab(super::app::Tab::Search);
        }
        let index = ((content_y - self.header()) / ROW).floor();
        if index < 0.0 {
            return Action::None;
        }
        let rows = self.rows(store);
        match rows.get(index as usize) {
            Some(chat) if x < MARGIN + AVATAR + 10.0 && chat_ring(chat) == Ring::Unseen => {
                let users = model::chat_users(&chat.ext);
                match users.first() {
                    Some(user) => Action::Stories {
                        ids: vec![user.id.clone()],
                        start: 0,
                    },
                    None => Action::None,
                }
            }
            Some(chat) => Action::Push(Box::new(ThreadScreen::new(chat.id.clone()))),
            None => Action::None,
        }
    }

    fn shown(&mut self, _store: &Store) -> Action {
        Action::request(
            "instagram.tray",
            json!({"cached": true}),
            Ask::Tray { cached: true },
        )
    }

    fn tab_bar(&self) -> bool {
        !self.requests
    }
}

pub fn status_note(store: &Store) -> String {
    match store.status.state.as_str() {
        "signed_out" => "Not signed in".into(),
        "offline" => "Offline".into(),
        "error" => "Needs attention".into(),
        "connecting" => "Connecting…".into(),
        _ => String::new(),
    }
}

fn draw_row(canvas: &mut Canvas, p: &mut Paint, chat: &Chat, y: f32, width: f32, now: i64) {
    let cy = y + ROW / 2.0;
    draw_chat_avatar(canvas, p, chat, MARGIN + AVATAR / 2.0, cy, AVATAR);
    let x = MARGIN + AVATAR + 32.0;
    let unread = chat.unread > 0;
    let right = width - MARGIN - if unread { 60.0 } else { 90.0 };
    let name_font = if unread { BODY_BOLD } else { BODY };
    let title = paint::fit(&chat.title, name_font, right - x);
    paint::draw_text(canvas, &title, x, cy - 10.0, name_font, TEXT);
    let (line, bold) = preview(chat, now);
    let font = if bold { SMALL_BOLD } else { SMALL };
    let color = if bold { TEXT } else { SECONDARY };
    let line = paint::fit(&line.replace('\n', " "), font, right - x);
    paint::draw_text(canvas, &line, x, cy + 40.0, font, color);
    if unread {
        canvas.fill_circle(width - MARGIN - 12.0, cy, 4.0 * DP, BLUE, 1.0);
    } else if chat.muted {
        paint::draw_text_right(canvas, "Muted", width - MARGIN, cy + 10.0, TINY, TERTIARY);
    } else {
        paint::icon(
            canvas,
            p.cache,
            Icon::Camera,
            width - MARGIN - 64.0,
            cy - 32.0,
            64.0,
            false,
            SECONDARY,
        );
    }
}

// ----- A chat ----------------------------------------------------------------------

const BUBBLE_MAX: f32 = 0.70 * 1080.0;
const PAD_X: f32 = 32.0;
const PAD_Y: f32 = 21.0;
const LINE_H: f32 = 52.0;
const RADIUS: f32 = 22.0 * DP;
const SMALL_R: f32 = 5.0 * DP;
const THEIR_X: f32 = MARGIN + 28.0 * DP + 16.0;
const MEDIA_W: f32 = 640.0;
const COMPOSER_H: f32 = 116.0;

/// What a message is drawn as.
#[derive(Debug, Clone)]
enum Body {
    Text(Vec<Line>),
    Jumbo(String),
    Heart,
    Pictures(Vec<(String, f32, f32, bool)>),
    Sticker(String, f32, f32),
    Voice(u64),
    Card(Card),
    Portrait {
        preview: Option<String>,
        label: String,
        author: String,
        expired: bool,
        text: Vec<Line>,
    },
    Notice(String, bool),
    Outline(String),
}

#[derive(Debug, Clone)]
struct Card {
    author: String,
    preview: Option<(String, f32)>,
    lines: Vec<Line>,
    title: String,
    note: Vec<Line>,
}

#[derive(Debug, Clone)]
struct Laid {
    signature: u64,
    body: Body,
    /// The bubble (or picture) size.
    w: f32,
    h: f32,
    reply: Option<(String, Vec<Line>)>,
    forwarded: bool,
    reactions: Option<String>,
}

#[derive(Debug, Clone)]
struct Placed {
    index: usize,
    /// Distance from the content's bottom to this block's bottom.
    from_bottom: f32,
    height: f32,
    separator: Option<String>,
    name: Option<String>,
    first: bool,
    last: bool,
    avatar: bool,
    seen: Option<&'static str>,
}

pub struct ThreadScreen {
    chat: String,
    scroll: Scroll,
    laid: HashMap<String, Laid>,
    placed: Vec<Placed>,
    generation: u64,
    height: f32,
    composer: String,
    focused: bool,
    composer_lines: Vec<Line>,
    /// Laid out again since the last frame, and whether everything already
    /// laid out stayed (then: the content's old height).
    relaid: bool,
    stable_from: Option<f32>,
}

fn is_jumbo(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && t.chars().count() <= 8
        && t.chars()
            .all(|c| !c.is_alphanumeric() && !c.is_ascii_punctuation() && !c.is_whitespace())
}

impl ThreadScreen {
    pub fn new(chat: String) -> Self {
        Self {
            chat,
            scroll: Scroll::default(),
            laid: HashMap::new(),
            placed: Vec::new(),
            generation: u64::MAX,
            height: 0.0,
            composer: String::new(),
            focused: false,
            composer_lines: Vec::new(),
            relaid: false,
            stable_from: None,
        }
    }

    fn composer_height(&self) -> f32 {
        COMPOSER_H.max(self.composer_lines.len().min(5) as f32 * 52.0 + 40.0) + 24.0
    }

    /// What a message's layout depends on: laid out again only when it changes.
    fn signature(message: &Message, group: bool) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        message.text.hash(&mut h);
        message.deleted.hash(&mut h);
        message.forwarded.hash(&mut h);
        message.status.hash(&mut h);
        group.hash(&mut h);
        for media in &message.media {
            media.reference.hash(&mut h);
            media.thumb.hash(&mut h);
        }
        for reaction in &message.reactions {
            reaction.emoji.hash(&mut h);
            reaction.count.hash(&mut h);
        }
        if let Some(reply) = &message.reply_to {
            reply.text.hash(&mut h);
            reply.author.hash(&mut h);
        }
        if !message.ext.is_null() && message.ext.as_object().is_none_or(|ext| !ext.is_empty()) {
            message.ext.to_string().hash(&mut h);
        }
        h.finish()
    }

    fn lay_message(message: &Message, group: bool) -> Laid {
        let signature = Self::signature(message, group);
        let ext = &message.ext;
        let max_text = BUBBLE_MAX - 2.0 * PAD_X;
        let text_lines = |s: &str| paint::wrap_plain(s, MESSAGE, TEXT, max_text, 200).0;
        let lines_w = |lines: &[Line]| lines.iter().map(paint::line_width).fold(0.0, f32::max);
        let (body, w, h) = if message.system {
            let call = ext.get("call").is_some();
            (Body::Notice(message.text.clone(), call), 0.0, 60.0)
        } else if message.deleted {
            let label = "Message unavailable".to_owned();
            let w = paint::measure(&label, MESSAGE) + 2.0 * PAD_X;
            (Body::Outline(label), w, LINE_H + 2.0 * PAD_Y)
        } else if ext.get("heart").is_some() {
            (Body::Heart, 46.0 * DP, 46.0 * DP)
        } else if let Some(share) = ext.get("share") {
            Self::lay_share(share, &message.text, lines_w)
        } else if let Some(once) = ext.get("view_once") {
            let video = once["video"].as_bool().unwrap_or(false);
            let label = if video { "Video" } else { "Photo" }.to_owned();
            let w = paint::measure(&label, MESSAGE) + 2.0 * PAD_X + 70.0;
            (Body::Outline(label), w, LINE_H + 2.0 * PAD_Y)
        } else if let Some(voice) = ext.get("voice") {
            let ms = voice["duration_ms"].as_u64().unwrap_or(0);
            (Body::Voice(ms), 560.0, 44.0 * DP)
        } else if let Some(first) = message.media.first() {
            match first.kind.as_str() {
                "sticker" | "gif" => {
                    let aspect = first.aspect().unwrap_or(1.0);
                    let w = 300.0_f32.min(300.0 * aspect);
                    let h = w / aspect;
                    (Body::Sticker(first.reference.clone(), w, h), w, h)
                }
                _ => {
                    let items: Vec<(String, f32, f32, bool)> = message
                        .media
                        .iter()
                        .take(4)
                        .map(|m| {
                            let reference = m.thumb.clone().unwrap_or_else(|| m.reference.clone());
                            (
                                reference,
                                m.w.unwrap_or(1) as f32,
                                m.h.unwrap_or(1) as f32,
                                m.kind == "video",
                            )
                        })
                        .collect();
                    if items.len() == 1 {
                        let (_, iw, ih, _) = &items[0];
                        let aspect = (iw / ih).clamp(0.5, 2.0);
                        let (w, h) = if aspect >= 1.0 {
                            (MEDIA_W, MEDIA_W / aspect)
                        } else {
                            ((MEDIA_W * 1.3 * aspect).min(MEDIA_W), MEDIA_W * 1.3)
                        };
                        (Body::Pictures(items), w, h)
                    } else {
                        let cell = (MEDIA_W - 6.0) / 2.0;
                        let rows = items.len().div_ceil(2) as f32;
                        (
                            Body::Pictures(items),
                            MEDIA_W,
                            rows * cell + (rows - 1.0) * 6.0,
                        )
                    }
                }
            }
        } else if is_jumbo(&message.text) {
            let size = 110.0;
            let w = crate::text::measure(message.text.trim(), Weight::Regular, size);
            (Body::Jumbo(message.text.trim().to_owned()), w, size * 1.3)
        } else {
            let lines = text_lines(&message.text);
            let w = (lines_w(&lines) + 2.0 * PAD_X).max(2.0 * PAD_X + 20.0);
            let h = lines.len().max(1) as f32 * LINE_H + 2.0 * PAD_Y;
            (Body::Text(lines), w, h)
        };
        let reply = message.reply_to.as_ref().map(|reply| {
            let who = match (&reply.author, message.mine) {
                (Some(author), true) if author == "You" => "You replied to yourself".to_owned(),
                (Some(author), true) => format!("You replied to {author}"),
                (Some(author), false) if author == "You" => format!(
                    "{} replied to you",
                    message.author.name.split(' ').next().unwrap_or("")
                ),
                (Some(author), false) => format!(
                    "{} replied to {author}",
                    message.author.name.split(' ').next().unwrap_or("")
                ),
                (None, _) => "Replied".to_owned(),
            };
            (
                who,
                paint::wrap_plain(&reply.text, SMALL, SECONDARY, BUBBLE_MAX - 2.0 * PAD_X, 2).0,
            )
        });
        let reactions = if message.reactions.is_empty() {
            None
        } else {
            let mut label: String = message
                .reactions
                .iter()
                .take(3)
                .map(|r| r.emoji.as_str())
                .collect();
            let total: u32 = message.reactions.iter().map(|r| r.count).sum();
            if total > 1 {
                label.push_str(&format!(" {total}"));
            }
            Some(label)
        };
        Laid {
            signature,
            body,
            w,
            h,
            reply,
            forwarded: message.forwarded,
            reactions,
        }
    }

    fn lay_share(share: &Value, text: &str, lines_w: impl Fn(&[Line]) -> f32) -> (Body, f32, f32) {
        let kind = share["kind"].as_str().unwrap_or("");
        let preview = share.get("preview").and_then(|p| {
            let reference = p["ref"].as_str()?.to_owned();
            let w = p["w"].as_f64().unwrap_or(1.0) as f32;
            let h = p["h"].as_f64().unwrap_or(1.0) as f32;
            Some((reference, (w / h.max(1.0)).clamp(0.5, 1.91)))
        });
        let author = share["author"].as_str().unwrap_or("").to_owned();
        let note = if text.is_empty() {
            Vec::new()
        } else {
            paint::wrap_plain(text, MESSAGE, TEXT, BUBBLE_MAX - 2.0 * PAD_X, 50).0
        };
        let note_h = if note.is_empty() {
            0.0
        } else {
            note.len() as f32 * LINE_H + 2.0 * PAD_Y + 8.0
        };
        match kind {
            "reel" | "story" => {
                let expired = share["expired"].as_bool().unwrap_or(false);
                let context = share["context"].as_str().unwrap_or("");
                // A story without its picture is gone (expired or removed).
                let expired = expired || preview.is_none();
                let label = match (kind, context) {
                    ("story", "reply") => "Replied to a story",
                    ("story", "reaction") => "Reacted to a story",
                    ("story", "mention") => "Mentioned in a story",
                    ("story", _) => "Shared a story",
                    _ => "",
                }
                .to_owned();
                let w = 380.0;
                let h = if expired { 120.0 } else { w * 16.0 / 9.0 };
                let label_h = if label.is_empty() { 0.0 } else { 50.0 };
                let _ = &lines_w;
                (
                    Body::Portrait {
                        preview: if expired { None } else { preview.map(|p| p.0) },
                        label,
                        author,
                        expired,
                        text: note,
                    },
                    w,
                    h + label_h + note_h,
                )
            }
            _ => {
                let caption = share["caption"].as_str().unwrap_or("");
                let title = share["title"].as_str().unwrap_or("").to_owned();
                let width = MEDIA_W;
                let lines = if caption.is_empty() {
                    Vec::new()
                } else {
                    let mut words = vec![(author.clone(), paint::SMALL_BOLD, TEXT)];
                    words.extend(paint::rich_words(caption, SMALL, TEXT));
                    paint::wrap_runs(&words, width - 2.0 * PAD_X, 2).0
                };
                let header = if author.is_empty() { 0.0 } else { 100.0 };
                let picture = preview
                    .as_ref()
                    .map_or(0.0, |(_, aspect)| width / aspect.max(0.8));
                let unavailable = kind == "unavailable" && preview.is_none();
                let title = if unavailable && title.is_empty() {
                    "This content isn't available".to_owned()
                } else {
                    title
                };
                let title_h = if title.is_empty() { 0.0 } else { 70.0 };
                let h = header + picture + lines.len() as f32 * 44.0 + title_h + 28.0;
                (
                    Body::Card(Card {
                        author,
                        preview,
                        lines,
                        title,
                        note,
                    }),
                    width,
                    h + note_h,
                )
            }
        }
    }

    fn relayout(&mut self, store: &Store) {
        // What was drawn before: if all of it stays where it was, only what
        // came above it (older messages) changed on screen.
        let before: Vec<(usize, f32, f32, u64)> = self
            .placed
            .iter()
            .map(|p| (p.index, p.from_bottom, p.height, self.laid_signature(store, p.index)))
            .collect();
        let old_height = self.height;
        self.relayout_all(store);
        let thread = store.threads.get(&self.chat);
        let stable = !before.is_empty()
            && thread.is_some_and(|thread| {
                let shift = thread.messages.len() - self.placed.len().min(thread.messages.len());
                let _ = shift;
                before.iter().all(|&(index, from_bottom, height, signature)| {
                    let added = self.placed.len().saturating_sub(before.len());
                    self.placed.iter().any(|p| {
                        p.index == index + added && p.from_bottom == from_bottom && p.height == height
                            && self.laid_signature(store, p.index) == signature
                    })
                })
            });
        self.stable_from = stable.then_some(old_height);
        self.relaid = true;
    }

    /// The signature of the message at `index` as laid out now (0: none).
    fn laid_signature(&self, store: &Store, index: usize) -> u64 {
        store
            .threads
            .get(&self.chat)
            .and_then(|thread| thread.messages.get(index))
            .and_then(|message| self.laid.get(&message.id))
            .map_or(0, |laid| laid.signature)
    }

    fn relayout_all(&mut self, store: &Store) {
        let Some(thread) = store.threads.get(&self.chat) else {
            self.placed.clear();
            self.height = 0.0;
            return;
        };
        let group = store.chat(&self.chat).is_some_and(Chat::is_group);
        let messages = &thread.messages;
        let mut laid = std::mem::take(&mut self.laid);
        let mut next: HashMap<String, Laid> = HashMap::with_capacity(messages.len());
        for message in messages {
            let signature = Self::signature(message, group);
            let entry = match laid.remove(&message.id) {
                Some(old) if old.signature == signature => old,
                _ => Self::lay_message(message, group),
            };
            next.insert(message.id.clone(), entry);
        }
        self.laid = next;
        // Place from the newest (bottom) up.
        let newest_mine = messages.iter().rposition(|m| m.mine && !m.system);
        let last_index = messages.len().saturating_sub(1);
        let mut placed = Vec::with_capacity(messages.len());
        let mut from_bottom = 24.0;
        for index in (0..messages.len()).rev() {
            let message = &messages[index];
            let laid = &self.laid[&message.id];
            let prev = index.checked_sub(1).map(|i| &messages[i]);
            let next = messages.get(index + 1);
            let same = |a: &Message, b: &Message| {
                a.author.id == b.author.id
                    && !a.system
                    && !b.system
                    && (a.ts - b.ts).abs() <= 3_600_000
            };
            let first = prev.is_none_or(|prev| !same(prev, message));
            let last = next.is_none_or(|next| !same(message, next));
            let separator = match prev {
                None => Some(paint::separator_time(message.ts, paint::now_ms())),
                Some(prev) if message.ts - prev.ts > 3_600_000 => {
                    Some(paint::separator_time(message.ts, paint::now_ms()))
                }
                _ => None,
            };
            let name = (group && !message.mine && first && !message.system)
                .then(|| message.author.name.clone());
            let seen = (Some(index) == newest_mine && index == last_index).then_some({
                match message.status.as_deref() {
                    Some("read") => "Seen",
                    Some("pending") => "Sending…",
                    Some("failed") => "Not sent",
                    _ => "Sent",
                }
            });
            let mut h = laid.h;
            if matches!(laid.body, Body::Notice(..)) {
                h = 70.0;
            }
            if let Some((_, lines)) = &laid.reply {
                h += 44.0 + lines.len() as f32 * 44.0 + 2.0 * PAD_Y + 6.0;
            }
            if laid.forwarded {
                h += 44.0;
            }
            if laid.reactions.is_some() {
                h += 40.0;
            }
            if name.is_some() {
                h += 44.0;
            }
            if separator.is_some() {
                h += 110.0;
            }
            if seen.is_some() {
                h += 50.0;
            }
            let gap = if last { 18.0 } else { 5.0 };
            placed.push(Placed {
                index,
                from_bottom,
                height: h,
                separator,
                name,
                first,
                last,
                avatar: !message.mine && last && !message.system,
                seen,
            });
            from_bottom += h + gap;
        }
        let more = if thread.more { 160.0 } else { 0.0 };
        self.height = from_bottom + more + 40.0;
        self.placed = placed;
    }

    fn draw_message(
        &self,
        canvas: &mut Canvas,
        p: &mut Paint,
        message: &Message,
        place: &Placed,
        bottom_y: f32,
        width: f32,
    ) {
        let laid = &self.laid[&message.id];
        let mut y = bottom_y - place.height;
        if let Some(sep) = &place.separator {
            paint::draw_text_centered(canvas, sep, width / 2.0, y + 70.0, TINY, SECONDARY);
            y += 110.0;
        }
        if let Body::Notice(text, call) = &laid.body {
            let font = TINY;
            let line = paint::fit(text, font, width - 200.0);
            if *call {
                let w = paint::measure(&line, font) + 60.0;
                let x = width / 2.0 - w / 2.0;
                paint::icon(
                    canvas,
                    p.cache,
                    Icon::Video,
                    x,
                    y + 14.0,
                    44.0,
                    false,
                    if text.starts_with("Missed") {
                        RED
                    } else {
                        SECONDARY
                    },
                );
                paint::draw_text(canvas, &line, x + 60.0, y + 48.0, font, SECONDARY);
            } else {
                paint::draw_text_centered(canvas, &line, width / 2.0, y + 48.0, font, SECONDARY);
            }
            return;
        }
        let mine = message.mine;
        let right = width - MARGIN;
        if let Some(name) = &place.name {
            paint::draw_text(canvas, name, THEIR_X + PAD_X, y + 32.0, TINY, SECONDARY);
            y += 44.0;
        }
        if laid.forwarded {
            let label = "Forwarded";
            if mine {
                paint::draw_text_right(canvas, label, right - 10.0, y + 32.0, TINY, SECONDARY);
            } else {
                paint::draw_text(canvas, label, THEIR_X + 10.0, y + 32.0, TINY, SECONDARY);
            }
            y += 44.0;
        }
        if let Some((who, lines)) = &laid.reply {
            if mine {
                paint::draw_text_right(canvas, who, right - 10.0, y + 32.0, TINY, SECONDARY);
            } else {
                paint::draw_text(canvas, who, THEIR_X + 10.0, y + 32.0, TINY, SECONDARY);
            }
            y += 44.0;
            let w = lines.iter().map(paint::line_width).fold(0.0, f32::max) + 2.0 * PAD_X;
            let h = lines.len() as f32 * 44.0 + 2.0 * PAD_Y;
            let x = if mine { right - w } else { THEIR_X };
            canvas.fill_rrect(RectF::new(x, y, w, h), RADIUS * 0.8, 0x1e1e1e, 1.0);
            for (n, line) in lines.iter().enumerate() {
                paint::draw_line(canvas, line, x + PAD_X, y + PAD_Y + 34.0 + n as f32 * 44.0);
            }
            y += h + 6.0;
        }
        let x = if mine { right - laid.w } else { THEIR_X };
        let rect = RectF::new(x, y, laid.w, laid.h);
        let radii = {
            let (top, bottom) = (
                if place.first { RADIUS } else { SMALL_R },
                if place.last { RADIUS } else { SMALL_R },
            );
            if mine {
                [RADIUS, top, bottom, RADIUS]
            } else {
                [top, RADIUS, RADIUS, bottom]
            }
        };
        let fill = if mine { MINE } else { ELEVATED };
        match &laid.body {
            Body::Text(lines) => {
                paint::fill_rrect4(canvas, rect, radii, &|_| fill);
                for (n, line) in lines.iter().enumerate() {
                    paint::draw_line(
                        canvas,
                        line,
                        x + PAD_X,
                        y + PAD_Y + 40.0 + n as f32 * LINE_H,
                    );
                }
            }
            Body::Jumbo(text) => {
                crate::text::draw(
                    canvas,
                    text,
                    x,
                    y + laid.h * 0.8,
                    Weight::Regular,
                    110.0,
                    TEXT,
                    1.0,
                );
            }
            Body::Heart => paint::icon(canvas, p.cache, Icon::Heart, x, y, laid.w, true, RED),
            Body::Pictures(items) => {
                if items.len() == 1 {
                    let (reference, _, _, video) = &items[0];
                    paint::picture(canvas, p, reference, rect, 0x1a1a1a);
                    paint::round_corners(canvas, rect, RADIUS, BG);
                    if *video {
                        play_badge(canvas, p, rect);
                    }
                } else {
                    let cell = (MEDIA_W - 6.0) / 2.0;
                    for (n, (reference, _, _, video)) in items.iter().enumerate() {
                        let cr = RectF::new(
                            x + (n % 2) as f32 * (cell + 6.0),
                            y + (n / 2) as f32 * (cell + 6.0),
                            cell,
                            cell,
                        );
                        paint::picture(canvas, p, reference, cr, 0x1a1a1a);
                        paint::round_corners(canvas, cr, 14.0 * DP, BG);
                        if *video {
                            play_badge(canvas, p, cr);
                        }
                    }
                }
            }
            Body::Sticker(reference, w, h) => {
                if let Some(image) = p.picture(reference, *w as u32, *h as u32, Fit::Contain, y) {
                    canvas.draw_image(&image, x as i32, y as i32, 1.0);
                }
            }
            Body::Voice(ms) => {
                paint::fill_rrect4(canvas, rect, radii, &|_| fill);
                paint::icon(
                    canvas,
                    p.cache,
                    Icon::Play,
                    x + 30.0,
                    y + rect.h / 2.0 - 24.0,
                    48.0,
                    true,
                    TEXT,
                );
                let bars = 26;
                for n in 0..bars {
                    let amp = ((n as f32 * 1.7).sin().abs() * 0.7 + 0.3) * (rect.h * 0.5);
                    let bx = x + 100.0 + n as f32 * 13.0;
                    canvas.fill_rrect(
                        RectF::new(bx, y + rect.h / 2.0 - amp / 2.0, 6.0, amp),
                        3.0,
                        TEXT,
                        0.8,
                    );
                }
                let label = format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60);
                paint::draw_text_right(
                    canvas,
                    &label,
                    x + rect.w - PAD_X,
                    paint::baseline(y + rect.h / 2.0, TINY),
                    TINY,
                    TEXT,
                );
            }
            Body::Card(card) => {
                let mut cy = y;
                let card_h = laid.h
                    - if card.note.is_empty() {
                        0.0
                    } else {
                        card.note.len() as f32 * LINE_H + 2.0 * PAD_Y + 8.0
                    };
                canvas.fill_rrect(RectF::new(x, cy, laid.w, card_h), RADIUS, 0x1c1c1c, 1.0);
                if !card.author.is_empty() {
                    canvas.fill_circle(x + PAD_X + 30.0, cy + 50.0, 30.0, 0x363636, 1.0);
                    paint::draw_text(
                        canvas,
                        &card.author,
                        x + PAD_X + 76.0,
                        cy + 62.0,
                        SMALL_BOLD,
                        TEXT,
                    );
                    cy += 100.0;
                }
                if let Some((reference, aspect)) = &card.preview {
                    let ph = laid.w / aspect.max(0.8);
                    let pr = RectF::new(x, cy, laid.w, ph);
                    paint::picture(canvas, p, reference, pr, 0x1a1a1a);
                    if card.author.is_empty() {
                        paint::round_corners(
                            canvas,
                            RectF::new(x, cy, laid.w, ph + 30.0),
                            RADIUS,
                            BG,
                        );
                    }
                    cy += ph;
                }
                for (n, line) in card.lines.iter().enumerate() {
                    paint::draw_line(canvas, line, x + PAD_X, cy + 48.0 + n as f32 * 44.0);
                }
                cy += card.lines.len() as f32 * 44.0;
                if !card.title.is_empty() {
                    paint::draw_text(
                        canvas,
                        &paint::fit(&card.title, SMALL, laid.w - 2.0 * PAD_X),
                        x + PAD_X,
                        cy + 52.0,
                        SMALL,
                        SECONDARY,
                    );
                }
                if !card.note.is_empty() {
                    draw_note(canvas, &card.note, mine, x, y + card_h + 8.0, right, fill);
                }
            }
            Body::Portrait {
                preview,
                label,
                author,
                expired,
                text,
            } => {
                let mut cy = y;
                if !label.is_empty() {
                    if mine {
                        paint::draw_text_right(
                            canvas,
                            label,
                            right - 10.0,
                            cy + 34.0,
                            TINY,
                            SECONDARY,
                        );
                    } else {
                        paint::draw_text(canvas, label, x + 10.0, cy + 34.0, TINY, SECONDARY);
                    }
                    cy += 50.0;
                }
                let w = 380.0;
                let px = if mine { right - w } else { x };
                if *expired {
                    let r = RectF::new(px, cy, w, 120.0);
                    canvas.stroke_rrect(r, 30.0, 3.0, SEPARATOR, 1.0);
                    paint::draw_text_centered(
                        canvas,
                        "Story unavailable",
                        px + w / 2.0,
                        cy + 72.0,
                        SMALL,
                        SECONDARY,
                    );
                    cy += 120.0;
                } else {
                    let r = RectF::new(px, cy, w, w * 16.0 / 9.0);
                    match preview {
                        Some(reference) => {
                            paint::picture(canvas, p, reference, r, 0x1a1a1a);
                        }
                        None => canvas.fill_rect(
                            r.x as i32, r.y as i32, r.w as i32, r.h as i32, 0x1a1a1a, 1.0,
                        ),
                    }
                    paint::round_corners(canvas, r, 18.0 * DP, BG);
                    if !author.is_empty() {
                        canvas.fill_rect(r.x as i32 + 20, r.y as i32 + 20, 0, 0, BG, 0.0);
                        paint::draw_text(
                            canvas,
                            &paint::fit(author, SMALL_BOLD, w - 60.0),
                            r.x + 28.0,
                            r.y + 60.0,
                            SMALL_BOLD,
                            TEXT,
                        );
                    }
                    cy += r.h;
                }
                if !text.is_empty() {
                    draw_note(canvas, text, mine, x, cy + 8.0, right, fill);
                }
            }
            Body::Outline(label) => {
                canvas.stroke_rrect(rect, RADIUS, 3.0, 0x363636, 1.0);
                paint::draw_text(
                    canvas,
                    label,
                    x + PAD_X,
                    y + PAD_Y + 40.0,
                    MESSAGE,
                    SECONDARY,
                );
            }
            Body::Notice(..) => {}
        }
        let mut below = y + laid.h;
        if let Some(reactions) = &laid.reactions {
            let w = paint::measure(reactions, SMALL) + 36.0;
            let rx = if mine { right - w - 20.0 } else { x + 20.0 };
            let pill = RectF::new(rx, below - 14.0, w, 52.0);
            canvas.fill_rrect(
                RectF::new(pill.x - 4.0, pill.y - 4.0, pill.w + 8.0, pill.h + 8.0),
                30.0,
                BG,
                1.0,
            );
            canvas.fill_rrect(pill, 26.0, ELEVATED, 1.0);
            paint::draw_text(canvas, reactions, rx + 18.0, below + 24.0, SMALL, TEXT);
            below += 40.0;
        }
        if place.avatar {
            let d = 28.0 * DP;
            paint::avatar(
                canvas,
                p,
                message.author.avatar.as_ref().map(|m| m.reference.as_str()),
                MARGIN + d / 2.0,
                y + laid.h - d / 2.0,
                d,
                Ring::None,
            );
        }
        if let Some(seen) = place.seen {
            let color = if seen == "Not sent" { RED } else { SECONDARY };
            paint::draw_text_right(canvas, seen, right - 8.0, below + 40.0, TINY, color);
        }
    }
}

fn draw_note(
    canvas: &mut Canvas,
    lines: &[Line],
    mine: bool,
    x: f32,
    y: f32,
    right: f32,
    fill: u32,
) {
    let w = lines.iter().map(paint::line_width).fold(0.0, f32::max) + 2.0 * PAD_X;
    let h = lines.len() as f32 * LINE_H + 2.0 * PAD_Y;
    let bx = if mine { right - w } else { x };
    canvas.fill_rrect(RectF::new(bx, y, w, h), RADIUS, fill, 1.0);
    for (n, line) in lines.iter().enumerate() {
        paint::draw_line(
            canvas,
            line,
            bx + PAD_X,
            y + PAD_Y + 40.0 + n as f32 * LINE_H,
        );
    }
}

fn play_badge(canvas: &mut Canvas, p: &mut Paint, rect: RectF) {
    let (cx, cy) = rect.center();
    canvas.fill_circle(cx, cy, 52.0, 0x000000, 0.45);
    paint::icon(
        canvas,
        p.cache,
        Icon::Play,
        cx - 26.0,
        cy - 30.0,
        60.0,
        true,
        0xffffff,
    );
}

impl Screen for ThreadScreen {
    fn chat(&self) -> Option<&str> {
        Some(&self.chat)
    }

    fn viewport(&self, height: f32) -> (f32, f32) {
        let bottom = height - if self.focused { 0.0 } else { SAFE_BOTTOM } - self.composer_height();
        (top_of_content(), bottom)
    }

    fn scroll(&mut self) -> Option<&mut Scroll> {
        Some(&mut self.scroll)
    }

    fn inverted(&self) -> bool {
        true
    }

    fn layout(&mut self, store: &Store, _width: f32) -> f32 {
        if self.generation != store.generation {
            self.generation = store.generation;
            self.relayout(store);
        }
        self.height
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let Some(thread) = store.threads.get(&self.chat) else {
            paint::draw_text_centered(
                canvas,
                "Loading…",
                area.width / 2.0,
                (area.top + area.bottom) / 2.0,
                BODY,
                SECONDARY,
            );
            return;
        };
        if thread.messages.is_empty() {
            let note = thread.failed.as_deref().unwrap_or(if thread.loaded {
                "No messages yet"
            } else {
                ""
            });
            paint::draw_text_centered(
                canvas,
                note,
                area.width / 2.0,
                (area.top + area.bottom) / 2.0,
                BODY,
                SECONDARY,
            );
        }
        let base = area.bottom + area.offset;
        for place in &self.placed {
            let bottom_y = base - place.from_bottom;
            let top_y = bottom_y - place.height;
            if bottom_y < area.band.0 {
                break;
            }
            if top_y > area.band.1 {
                continue;
            }
            if let Some(message) = thread.messages.get(place.index) {
                self.draw_message(canvas, p, message, place, bottom_y, area.width);
            }
        }
        if thread.more {
            let y = base - self.height + 80.0;
            if area.visible(y - 40.0, y + 40.0) {
                spinner(canvas, area.width / 2.0, y);
            }
        } else if let Some(chat) = store.chat(&self.chat) {
            // The chat's head: who it is with, above the first message.
            let y = base - self.height + 40.0;
            if area.visible(y - 400.0, y) && !self.placed.is_empty() {
                let _ = chat;
            }
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        canvas.fill_rect(0, area.top as i32 - 1, canvas.width, 1, SEPARATOR, 1.0);
        let cy = STATUS + TOP_BAR / 2.0;
        paint::icon(
            canvas,
            p.cache,
            Icon::Back,
            MARGIN - 10.0,
            cy - 36.0,
            72.0,
            false,
            TEXT,
        );
        let chat = store.chat(&self.chat);
        let d = 34.0 * DP;
        let ax = MARGIN + 90.0 + d / 2.0;
        if let Some(chat) = chat {
            draw_chat_avatar(canvas, p, chat, ax, cy, d);
            let x = ax + d / 2.0 + 28.0;
            let title = paint::fit(&chat.title, BODY_BOLD, area.width - x - 260.0);
            paint::draw_text(canvas, &title, x, cy - 6.0, BODY_BOLD, TEXT);
            let users = model::chat_users(&chat.ext);
            let sub = if chat.is_group() {
                format!("{} members", chat.members.unwrap_or(users.len() as u32 + 1))
            } else {
                users
                    .first()
                    .and_then(|u| u.username.clone())
                    .unwrap_or_default()
            };
            paint::draw_text(
                canvas,
                &paint::fit(&sub, TINY, area.width - x - 260.0),
                x,
                cy + 40.0,
                TINY,
                SECONDARY,
            );
        }
        paint::icon(
            canvas,
            p.cache,
            Icon::Phone,
            area.width - MARGIN - 190.0,
            cy - 30.0,
            60.0,
            false,
            TEXT,
        );
        paint::icon(
            canvas,
            p.cache,
            Icon::Video,
            area.width - MARGIN - 70.0,
            cy - 33.0,
            70.0,
            false,
            TEXT,
        );
        // The composer.
        let bottom = area.bottom;
        canvas.fill_rect(
            0,
            bottom as i32,
            canvas.width,
            canvas.height - bottom as i32,
            BG,
            1.0,
        );
        let h = self.composer_height() - 24.0;
        let field = RectF::new(MARGIN * 0.6, bottom + 12.0, area.width - MARGIN * 1.2, h);
        canvas.fill_rrect(field, (COMPOSER_H / 2.0).min(h / 2.0), FIELD, 1.0);
        let cam = 36.0 * DP;
        canvas.fill_circle(
            field.x + 14.0 + cam / 2.0,
            field.y + h - COMPOSER_H / 2.0,
            cam / 2.0,
            BLUE,
            1.0,
        );
        paint::icon(
            canvas,
            p.cache,
            Icon::Camera,
            field.x + 14.0 + cam / 2.0 - 30.0,
            field.y + h - COMPOSER_H / 2.0 - 30.0,
            60.0,
            false,
            0xffffff,
        );
        let tx = field.x + 14.0 + cam + 26.0;
        let read_only = chat.is_some_and(|chat| chat.ext.get("read_only").is_some());
        if self.composer.is_empty() {
            let hint = if read_only {
                "You can't reply to this conversation"
            } else {
                "Message…"
            };
            paint::draw_text(
                canvas,
                hint,
                tx,
                paint::baseline(field.y + h - COMPOSER_H / 2.0, BODY),
                BODY,
                SECONDARY,
            );
            let ix = field.x + field.w - 40.0;
            for (n, icon) in [Icon::Photo, Icon::Mic].iter().enumerate() {
                paint::icon(
                    canvas,
                    p.cache,
                    *icon,
                    ix - (n as f32 + 1.0) * 84.0,
                    field.y + h - COMPOSER_H / 2.0 - 30.0,
                    60.0,
                    false,
                    TEXT,
                );
            }
        } else {
            let lines = &self.composer_lines;
            let shown = lines.len().min(5);
            for (n, line) in lines[lines.len() - shown..].iter().enumerate() {
                paint::draw_line(canvas, line, tx, field.y + 24.0 + 40.0 + n as f32 * 52.0);
            }
            paint::draw_text_right(
                canvas,
                "Send",
                field.x + field.w - 34.0,
                paint::baseline(field.y + h - COMPOSER_H / 2.0, BODY_BOLD),
                BODY_BOLD,
                BLUE,
            );
        }
        if self.focused {
            let caret_line = self.composer_lines.len().clamp(1, 5) - 1;
            let last_w = self.composer_lines.last().map_or(0.0, paint::line_width);
            let cx = tx + last_w + 4.0;
            let cy0 = field.y + 18.0 + caret_line as f32 * 52.0;
            canvas.fill_rect(cx as i32, cy0 as i32 + 6, 3, 50, BLUE, 1.0);
        }
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            if x < MARGIN + 100.0 {
                return Action::Pop;
            }
            let chat = store.chat(&self.chat);
            if let Some(user) = chat
                .filter(|c| !c.is_group())
                .and_then(|c| model::chat_users(&c.ext).into_iter().next())
            {
                return Action::Push(Box::new(ProfileScreen::user(
                    user.id.clone(),
                    user.username.clone(),
                )));
            }
            return Action::None;
        }
        if y >= area.bottom {
            // The composer: Send, or focus.
            if !self.composer.trim().is_empty() && x > area.width - 260.0 {
                let text = std::mem::take(&mut self.composer);
                self.composer_lines.clear();
                self.scroll.reset();
                return Action::request(
                    "send",
                    json!({"chat": self.chat, "text": text.trim_end()}),
                    Ask::Send {
                        chat: self.chat.clone(),
                        client_id: String::new(),
                    },
                );
            }
            self.focused = true;
            return Action::Keyboard(Some(KeyboardPurpose::Multiline));
        }
        // A message: a shared post or profile opens it; a story opens its owner's.
        let base = area.bottom + area.offset;
        let Some(thread) = store.threads.get(&self.chat) else {
            return Action::None;
        };
        for place in &self.placed {
            let bottom_y = base - place.from_bottom;
            if y > bottom_y || y < bottom_y - place.height {
                continue;
            }
            let Some(message) = thread.messages.get(place.index) else {
                break;
            };
            if let Some(share) = message.ext.get("share") {
                let kind = share["kind"].as_str().unwrap_or("");
                let username = share["username"]
                    .as_str()
                    .or_else(|| share["author"].as_str())
                    .unwrap_or("");
                match kind {
                    "post" | "reel" => {
                        if let Some(code) = share["code"].as_str().filter(|code| !code.is_empty()) {
                            return Action::Push(Box::new(super::post::PostScreen::by_code(
                                code.to_owned(),
                            )));
                        }
                        if !username.is_empty() {
                            return Action::Push(Box::new(ProfileScreen::user(
                                String::new(),
                                Some(username.to_owned()),
                            )));
                        }
                    }
                    "profile" | "story" if !username.is_empty() => {
                        return Action::Push(Box::new(ProfileScreen::user(
                            String::new(),
                            Some(username.to_owned()),
                        )));
                    }
                    "link" => {
                        if let Some(url) = share["url"].as_str() {
                            return Action::OpenUrl(url.to_owned());
                        }
                    }
                    _ => {}
                }
            }
            break;
        }
        if self.focused {
            self.focused = false;
            return Action::Keyboard(None);
        }
        Action::None
    }

    fn key(&mut self, _store: &Store, key: KeyInput) -> Action {
        match key {
            KeyInput::Character(c) => self.composer.push(c),
            KeyInput::Backspace => {
                self.composer.pop();
            }
            KeyInput::Enter => self.composer.push('\n'),
        }
        let words: Vec<(String, Font, u32)> = self
            .composer
            .split(' ')
            .map(|word| (word.to_owned(), BODY, TEXT))
            .collect();
        self.composer_lines =
            paint::wrap_runs(&words, 1080.0 - 2.0 * MARGIN - 36.0 * DP - 220.0, 50).0;
        Action::Redraw
    }

    fn keyboard_hidden(&mut self) {
        self.focused = false;
    }

    fn tick(&mut self, store: &Store, area: Area, _now: Instant) -> Action {
        let Some(thread) = store.threads.get(&self.chat) else {
            return Action::None;
        };
        let max = (self.height - (area.bottom - area.top)).max(0.0);
        if thread.more
            && !thread.loading_older
            && area.offset > max - 1500.0
            && let Some(first) = thread
                .messages
                .iter()
                .find(|m| m.status.as_deref() != Some("pending"))
        {
            return Action::request(
                "history",
                json!({"chat": self.chat, "before": first.id, "limit": 40}),
                Ask::History {
                    chat: self.chat.clone(),
                    older: true,
                },
            );
        }
        Action::None
    }

    fn shown(&mut self, _store: &Store) -> Action {
        Action::request(
            "history",
            json!({"chat": self.chat, "limit": 40}),
            Ask::History {
                chat: self.chat.clone(),
                older: false,
            },
        )
    }

    fn blit_ok(&self) -> bool {
        true
    }

    fn changed_rows(&mut self, area: Area) -> Option<Vec<(i32, i32)>> {
        if !std::mem::take(&mut self.relaid) {
            return Some(Vec::new());
        }
        // Older messages came in above what was drawn: the rows from the
        // top to just under where the old content began (its spinner).
        let old = self.stable_from.take()?;
        let old_top = area.bottom + area.offset - old;
        let end = (old_top + 200.0).ceil() as i32;
        Some(if end > area.top as i32 { vec![(area.top as i32, end)] } else { Vec::new() })
    }

    fn tab_bar(&self) -> bool {
        false
    }
}

pub fn spinner(canvas: &mut Canvas, cx: f32, cy: f32) {
    for n in 0..8 {
        let a = n as f32 * std::f32::consts::TAU / 8.0;
        let alpha = 0.25 + 0.75 * (n as f32 / 8.0);
        canvas.fill_circle(
            cx + a.cos() * 26.0,
            cy + a.sin() * 26.0,
            6.0,
            SECONDARY,
            alpha,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::proto::ChatLast;

    fn chat(unread: u32, mine: bool, seen: bool) -> Chat {
        Chat {
            id: "1".into(),
            kind: "dm".into(),
            title: "Ana".into(),
            unread,
            last: Some(ChatLast {
                id: "m".into(),
                ts: 1_800_000_000_000 - 120_000,
                author: Some("Ana".into()),
                text: "hi there".into(),
                mine,
            }),
            ext: serde_json::json!({"seen": seen}),
            ..Default::default()
        }
    }

    #[test]
    fn chat_rows_say_what_instagram_says() {
        let now = 1_800_000_000_000;
        assert_eq!(
            preview(&chat(1, false, false), now),
            ("hi there · 2m".to_owned(), true)
        );
        assert_eq!(
            preview(&chat(3, false, false), now),
            ("3 new messages · 2m".to_owned(), true)
        );
        assert_eq!(
            preview(&chat(9, false, false), now),
            ("4+ new messages · 2m".to_owned(), true)
        );
        assert_eq!(
            preview(&chat(0, true, true), now),
            ("Seen 2m ago".to_owned(), false)
        );
        assert_eq!(
            preview(&chat(0, true, false), now),
            ("You: hi there · 2m".to_owned(), false)
        );
    }

    #[test]
    fn only_a_few_emoji_are_jumbo() {
        assert!(is_jumbo("😂😂"));
        assert!(!is_jumbo("ok 😂"));
        assert!(!is_jumbo(""));
    }
}
