//! A chat: its header, the messages (bubbles with tails, group names,
//! forwards, quoted replies, photos, videos, GIFs, stickers, voice notes,
//! documents, deleted messages, reactions, times and ticks), day chips,
//! group notices and the unread bar, bottom-anchored over the composer. A
//! message's layout is made once and kept; scrolling draws only what comes
//! into view; older pages are asked for before the top is reached.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use serde_json::{Value, json};

use super::band::{self, Band};
use super::icons::{self, Glyph};
use super::rich::{self, Laid};
use super::{Ask, WhatsApp, avatar, composer, list, theme, time, viewer};
use crate::comms::Scroll;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::exocortex::ui::Haptic;
use crate::exocortex::ui::icons::Icon;
use crate::gfx::{Canvas, RectF, Rgb};
use crate::social::media::{Fit, Media};
use crate::social::proto::{Chat, Message};
use crate::social::shell::{Cx, DrawCx};
use crate::text::{self, Weight};

/// Older pages are asked for this far before the top.
const PREFETCH: f32 = 2400.0;
const SCROLL_BUTTON: f32 = 116.0;
const MEDIA_MAX: f32 = 700.0;
const STICKER: f32 = 400.0;
const QUOTE_H: f32 = 122.0;
const NAME_H: f32 = 48.0;
const FORWARD_H: f32 = 44.0;
const META_SIZE: f32 = 29.0;
const REACTION_H: f32 = 62.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Text,
    Media,
    Sticker,
    Audio,
    File,
    Deleted,
    System,
}

#[derive(Debug, Clone)]
pub struct MediaBox {
    pub rect: RectF,
    pub reference: String,
    pub thumb: Option<String>,
    pub kind: String,
    pub label: Option<String>,
}

/// One message, laid out (relative to its bubble's top left).
#[derive(Debug, Clone)]
pub struct Bubble {
    pub body: Body,
    pub mine: bool,
    pub w: f32,
    pub h: f32,
    /// Room under the bubble (its reactions).
    pub below: f32,
    pub name: Option<(String, Rgb, Option<String>)>,
    pub forwarded: bool,
    pub quote: Option<(String, Rgb, String, f32)>,
    pub media: Option<MediaBox>,
    pub text: Option<Laid>,
    pub text_y: f32,
    pub text_weight: Weight,
    pub meta: String,
    pub status: Option<String>,
    pub meta_x: f32,
    pub meta_y: f32,
    pub meta_on_media: bool,
    pub reactions: Option<(String, f32)>,
    pub audio: Option<(String, u64)>,
    pub file: Option<(String, String, String)>,
}

#[derive(Debug, Clone)]
enum Item {
    Day(String),
    Unread(u32),
    Message { index: usize, bubble: Rc<Bubble>, tail: bool },
    Loading,
}

pub struct ChatView {
    pub id: String,
    pub messages: Vec<Message>,
    pub more: bool,
    pub loading_older: bool,
    pub requested: bool,
    pub loaded: bool,
    pub scroll: Scroll,
    unread: u32,
    unread_marker: Option<String>,
    jump_to_unread: bool,
    layouts: HashMap<String, (u64, Rc<Bubble>)>,
    items: Vec<Item>,
    tops: Vec<f32>,
    total: f32,
    dirty: bool,
    version: u64,
    band: Band,
    pressed: Option<String>,
    width: f32,
    group: bool,
    members: Option<u32>,
    title: String,
    avatar: Option<crate::social::proto::Media>,
    push_name: Option<String>,
    /// The scroll button and what it covered last frame.
    button_shown: bool,
    /// Pictures the last frames drew without (`avatar::arrived`).
    waiting: Vec<(String, u32, u32, Fit)>,
    /// The header's picture had not come when it was drawn.
    header_waiting: Vec<(String, u32, u32, Fit)>,
}

/// What a message shows, as a number: its layout is made again when it changes.
fn hash_message(message: &Message) -> u64 {
    let mut parts: Vec<u64> = vec![
        message.ts as u64,
        u64::from(message.mine),
        u64::from(message.deleted),
        u64::from(message.system),
        u64::from(message.forwarded),
        message.edited_ts.unwrap_or(0) as u64,
    ];
    fn text_hash(parts: &mut Vec<u64>, text: &str) {
        parts.push(text.bytes().fold(text.len() as u64, |h, b| h.wrapping_mul(1_099_511_628_211).wrapping_add(u64::from(b))));
    }
    text_hash(&mut parts, &message.text);
    text_hash(&mut parts, &message.author.name);
    text_hash(&mut parts, message.status.as_deref().unwrap_or(""));
    text_hash(&mut parts, message.ext["push_name"].as_str().unwrap_or(""));
    for media in &message.media {
        text_hash(&mut parts, &media.reference);
        text_hash(&mut parts, media.thumb.as_deref().unwrap_or(""));
    }
    if let Some(reply) = &message.reply_to {
        text_hash(&mut parts, &reply.text);
        text_hash(&mut parts, reply.author.as_deref().unwrap_or(""));
    }
    for reaction in &message.reactions {
        text_hash(&mut parts, &reaction.emoji);
        parts.push(u64::from(reaction.count) * 2 + u64::from(reaction.mine));
    }
    band::key_of(&parts)
}

impl ChatView {
    pub fn new(id: String, unread: u32) -> Self {
        let mut scroll = Scroll::default();
        scroll.smooth = true;
        Self {
            id,
            messages: Vec::new(),
            more: false,
            loading_older: false,
            requested: false,
            loaded: false,
            scroll,
            unread,
            unread_marker: None,
            jump_to_unread: unread > 0,
            layouts: HashMap::new(),
            items: Vec::new(),
            tops: Vec::new(),
            total: 0.0,
            dirty: true,
            version: 0,
            band: Band::default(),
            pressed: None,
            width: 0.0,
            group: false,
            members: None,
            title: String::new(),
            avatar: None,
            push_name: None,
            button_shown: false,
            waiting: Vec::new(),
            header_waiting: Vec::new(),
        }
    }

    pub fn chat_changed(&mut self, chat: &Chat) {
        self.group = chat.is_group();
        self.members = chat.members;
        self.title = chat.title.clone();
        self.avatar = chat.avatar.clone();
        self.push_name = chat.ext["push_name"].as_str().map(str::to_owned);
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.version += 1;
    }

    pub fn page_arrived(&mut self, value: &Value, older: bool, now: Instant) {
        let page: Vec<Message> = serde_json::from_value(value["messages"].clone()).unwrap_or_default();
        let more = value["more"].as_bool().unwrap_or(false);
        if older {
            self.loading_older = false;
            self.more = more;
            let known: std::collections::HashSet<&str> = self.messages.iter().map(|m| m.id.as_str()).collect();
            let mut merged: Vec<Message> = page.into_iter().filter(|m| !known.contains(m.id.as_str())).collect();
            merged.append(&mut self.messages);
            self.messages = merged;
        } else {
            self.loaded = true;
            self.more = more;
            // Messages that came live meanwhile stay.
            let ids: std::collections::HashSet<String> = page.iter().map(|m| m.id.clone()).collect();
            let newest = page.last().map_or(i64::MIN, |m| m.ts);
            let mut extra: Vec<Message> = self
                .messages
                .drain(..)
                .filter(|m| !ids.contains(&m.id) && m.ts > newest)
                .collect();
            self.messages = page;
            self.messages.append(&mut extra);
            if self.unread > 0 && self.unread_marker.is_none() {
                let mut count = 0;
                for message in self.messages.iter().rev() {
                    if !message.mine && !message.system {
                        count += 1;
                        if count == self.unread {
                            self.unread_marker = Some(message.id.clone());
                            break;
                        }
                    }
                }
            }
        }
        let _ = now;
        self.changed();
    }

    pub fn page_failed(&mut self, older: bool) {
        if older {
            self.loading_older = false;
        } else {
            self.loaded = true;
        }
        self.changed();
    }

    pub fn upsert(&mut self, message: Message, live: bool, _now: Instant) {
        if let Some(known) = self.messages.iter_mut().find(|m| m.id == message.id) {
            *known = message;
            self.changed();
            return;
        }
        if let Some(client_id) = &message.client_id
            && let Some(pending) = self.messages.iter_mut().find(|m| m.id == *client_id)
        {
            *pending = message;
            self.changed();
            return;
        }
        if !self.loaded && !live {
            return;
        }
        let at_bottom = self.scroll.value(Instant::now()) < 40.0;
        let index = self.messages.partition_point(|m| m.ts <= message.ts);
        let newest = index == self.messages.len();
        self.messages.insert(index, message);
        let before = self.total;
        self.changed();
        if newest && !at_bottom {
            // Keep what is on screen where it is.
            self.relayout_if_needed();
            self.scroll.shift(self.total - before);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.messages.retain(|m| m.id != id);
        self.changed();
    }

    pub fn add_pending(&mut self, client_id: &str, text_in: &str, reply: Option<&str>) {
        let reply_to = reply.and_then(|id| self.messages.iter().find(|m| m.id == id)).map(|m| {
            crate::social::proto::ReplyTo {
                id: m.id.clone(),
                author: Some(if m.mine { "You".to_owned() } else { m.author.name.clone() }),
                text: m.text.clone(),
            }
        });
        let message = Message {
            id: client_id.to_owned(),
            chat: self.id.clone(),
            ts: time::now_secs() * 1000,
            mine: true,
            text: text_in.to_owned(),
            format: "whatsapp".into(),
            status: Some("pending".into()),
            client_id: Some(client_id.to_owned()),
            reply_to,
            ..Message::default()
        };
        self.messages.push(message);
        self.scroll.reset();
        self.changed();
    }

    pub fn fail_pending(&mut self, client_id: &str) {
        if let Some(message) = self.messages.iter_mut().find(|m| m.id == client_id) {
            message.status = Some("failed".into());
            self.changed();
        }
    }

    fn relayout_if_needed(&mut self) {
        if self.dirty && self.width > 0.0 {
            self.build(self.width);
        }
    }

    /// Lays the items out for `width` (each message's layout kept while it
    /// does not change).
    fn build(&mut self, width: f32) {
        if self.width != width {
            self.layouts.clear();
        }
        self.width = width;
        self.dirty = false;
        let mut items = Vec::with_capacity(self.messages.len() + 16);
        if self.loading_older || (self.more && !self.messages.is_empty()) {
            items.push(Item::Loading);
        }
        let mut last_day = i64::MIN;
        let mut previous: Option<&Message> = None;
        let mut seen = std::collections::HashSet::new();
        for (index, message) in self.messages.iter().enumerate() {
            seen.insert(message.id.clone());
            let day = time::day_of(message.ts);
            let new_day = day != last_day;
            if new_day {
                items.push(Item::Day(time::day_chip(message.ts)));
                last_day = day;
            }
            let marker = self.unread_marker.as_deref() == Some(message.id.as_str());
            if marker {
                items.push(Item::Unread(self.unread));
            }
            let hash = hash_message(message);
            let bubble = match self.layouts.get(&message.id) {
                Some((known, bubble)) if *known == hash => bubble.clone(),
                _ => {
                    let bubble = Rc::new(layout(message, self.group, width));
                    self.layouts.insert(message.id.clone(), (hash, bubble.clone()));
                    bubble
                }
            };
            let tail = message.system
                || new_day
                || marker
                || previous.is_none_or(|p| p.mine != message.mine || p.system || p.author.id != message.author.id);
            items.push(Item::Message { index, bubble, tail });
            previous = Some(message);
        }
        self.layouts.retain(|id, _| seen.contains(id));
        let mut tops = Vec::with_capacity(items.len());
        let mut y = 16.0;
        for (n, item) in items.iter().enumerate() {
            let gap = match item {
                Item::Message { tail, .. } if n > 0 => {
                    if *tail { theme::GAP_OTHER } else { theme::GAP_SAME }
                }
                _ => 14.0,
            };
            y += gap;
            tops.push(y);
            y += item_height(item);
        }
        self.items = items;
        self.tops = tops;
        self.total = y + 24.0;
    }

    fn max_scroll(&self, band_h: f32) -> f32 {
        (self.total - band_h).max(0.0)
    }
}

fn item_height(item: &Item) -> f32 {
    match item {
        Item::Day(_) => 66.0,
        Item::Unread(_) => 80.0,
        Item::Loading => 80.0,
        Item::Message { bubble, .. } => bubble.h + bubble.below,
    }
}

fn meta_text(message: &Message) -> String {
    let time_text = time::clock(message.ts);
    if message.edited_ts.is_some() && !message.deleted {
        format!("Edited {time_text}")
    } else {
        time_text
    }
}

fn meta_width(meta: &str, mine: bool, status: Option<&str>) -> f32 {
    text::measure(meta, Weight::Regular, META_SIZE) + if mine && status.is_some() { 52.0 } else { 0.0 }
}

/// Lays a message out for a chat `width` wide.
pub fn layout(message: &Message, group: bool, width: f32) -> Bubble {
    let max_w = (width - 2.0 * theme::SIDE - 150.0).min(900.0);
    let inner_max = max_w - 2.0 * theme::BUBBLE_PAD_X;
    let mine = message.mine;
    let status = if mine { message.status.clone() } else { None };
    let meta = meta_text(message);
    let meta_w = meta_width(&meta, mine, status.as_deref());
    let mut bubble = Bubble {
        body: Body::Text,
        mine,
        w: 0.0,
        h: 0.0,
        below: 0.0,
        name: None,
        forwarded: message.forwarded && !message.deleted,
        quote: None,
        media: None,
        text: None,
        text_y: 0.0,
        text_weight: Weight::Regular,
        meta,
        status,
        meta_x: 0.0,
        meta_y: 0.0,
        meta_on_media: false,
        reactions: None,
        audio: None,
        file: None,
    };
    if message.system {
        let laid = rich::layout(&message.text, (width - 260.0).min(820.0), 34.0, 46.0, Weight::Regular);
        bubble.body = Body::System;
        bubble.w = laid.width + 52.0;
        bubble.h = laid.height() + 30.0;
        bubble.text = Some(laid);
        return bubble;
    }
    let media = message.media.first().filter(|_| !message.deleted);
    bubble.body = if message.deleted {
        Body::Deleted
    } else {
        match media.map(|m| m.kind.as_str()) {
            Some("image" | "video" | "gif") => Body::Media,
            Some("sticker") => Body::Sticker,
            Some("audio") => Body::Audio,
            Some("file") => Body::File,
            _ => Body::Text,
        }
    };
    if bubble.body == Body::Sticker {
        let m = media.unwrap();
        let aspect = m.aspect().unwrap_or(1.0);
        let (w, h) = if aspect >= 1.0 { (STICKER, STICKER / aspect) } else { (STICKER * aspect, STICKER) };
        bubble.media = Some(MediaBox {
            rect: RectF { x: 0.0, y: 0.0, w, h },
            reference: m.reference.clone(),
            thumb: m.thumb.clone(),
            kind: m.kind.clone(),
            label: None,
        });
        bubble.w = w;
        bubble.h = h + 54.0;
        bubble.meta_x = w - meta_w - 22.0;
        bubble.meta_y = h + 40.0;
        add_reactions(&mut bubble, message);
        return bubble;
    }
    let mut y = theme::BUBBLE_PAD_Y;
    let mut inner_w: f32 = 0.0;
    if group && !mine {
        let push = message.ext["push_name"].as_str().map(|p| format!("~ {p}"));
        let name_w = text::measure(&message.author.name, Weight::SemiBold, theme::T_NAME)
            + push.as_deref().map_or(0.0, |p| 30.0 + text::measure(p, Weight::Regular, 31.0));
        inner_w = inner_w.max(name_w.min(inner_max));
        bubble.name = Some((message.author.name.clone(), theme::name_color(&message.author.id), push));
        y += NAME_H;
    }
    if bubble.forwarded {
        inner_w = inner_w.max(text::measure("Forwarded", Weight::Regular, 32.0) + 52.0);
        y += FORWARD_H;
    }
    if let Some(reply) = &message.reply_to
        && !message.deleted
    {
        let author = reply.author.clone().unwrap_or_default();
        let color = if author == "You" { theme::TEAL } else { theme::name_color(&author) };
        let text_line = if reply.text.is_empty() { "Message".to_owned() } else { reply.text.replace('\n', " ") };
        let w = text::measure(&author, Weight::SemiBold, 34.0).max(text::measure(&text_line, Weight::Regular, 36.0)) + 60.0;
        inner_w = inner_w.max(w.clamp(420.0, inner_max));
        bubble.quote = Some((author, color, text_line, 0.0));
        y += QUOTE_H + 8.0;
    }
    match bubble.body {
        Body::Media => {
            let m = media.unwrap();
            let mw = MEDIA_MAX.min(max_w - 16.0);
            let aspect = m.aspect().unwrap_or(1.0);
            let mh = (mw / aspect).clamp(mw * 0.55, mw * 1.35).round();
            let pad = 8.0;
            let top = if bubble.name.is_some() || bubble.quote.is_some() || bubble.forwarded { y } else { pad };
            let label = match m.kind.as_str() {
                "video" => m.duration_ms.map(time::duration),
                "gif" => Some("GIF".to_owned()),
                _ => None,
            };
            bubble.media = Some(MediaBox {
                rect: RectF { x: pad, y: top, w: mw, h: mh },
                reference: m.reference.clone(),
                thumb: m.thumb.clone(),
                kind: m.kind.clone(),
                label,
            });
            bubble.w = mw + 2.0 * pad;
            y = top + mh;
            if message.text.trim().is_empty() {
                bubble.meta_on_media = true;
                bubble.h = y + pad;
                bubble.meta_x = pad + mw - meta_w - 20.0;
                bubble.meta_y = y - 22.0;
                add_reactions(&mut bubble, message);
                return bubble;
            }
            y += 8.0;
            inner_w = inner_w.max(mw - 2.0 * theme::BUBBLE_PAD_X + 16.0);
        }
        Body::Audio => {
            let m = media.unwrap();
            inner_w = inner_w.max(640.0f32.min(inner_max));
            let seed = m.reference.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
            bubble.audio = Some((m.duration_ms.map(time::duration).unwrap_or_else(|| "0:00".into()), seed));
            y += 112.0;
            bubble.w = inner_w + 2.0 * theme::BUBBLE_PAD_X;
            bubble.meta_x = bubble.w - theme::BUBBLE_PAD_X - meta_w;
            bubble.meta_y = y + 18.0;
            bubble.h = y + 36.0;
            add_reactions(&mut bubble, message);
            return bubble;
        }
        Body::File => {
            let m = media.unwrap();
            inner_w = inner_w.max(660.0f32.min(inner_max));
            let name = m.name.clone().unwrap_or_else(|| "Document".into());
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_uppercase()).filter(|e| e.len() <= 5).unwrap_or_else(|| "FILE".into());
            let mut detail = Vec::new();
            if let Some(size) = m.size {
                detail.push(time::size(size));
            }
            detail.push(ext.clone());
            bubble.file = Some((name, detail.join(" • "), ext));
            y += 136.0;
        }
        _ => {}
    }
    // The text, and the time beside its last line when it fits.
    let shown = if message.deleted {
        if mine { "You deleted this message".to_owned() } else { "This message was deleted".to_owned() }
    } else {
        message.text.clone()
    };
    let jumbo = bubble.body == Body::Text && rich::emoji_only(&shown);
    let (size, line) = if jumbo { (96.0, 112.0) } else { (theme::T_BODY, theme::LINE) };
    if !shown.is_empty() {
        let indent = if message.deleted { 52.0 } else { 0.0 };
        let laid = rich::layout(&shown, inner_max - indent, size, line, Weight::Regular);
        bubble.text_y = y + size * 0.98;
        let last_w = laid.last_width() + indent;
        let text_w = laid.width + indent;
        let lines = laid.lines.len();
        y += laid.height();
        if last_w + 22.0 + meta_w <= inner_max && !jumbo {
            inner_w = inner_w.max(text_w).max(last_w + 22.0 + meta_w);
            bubble.meta_y = bubble.text_y + (lines as f32 - 1.0) * line + 8.0;
            y += 6.0;
        } else {
            inner_w = inner_w.max(text_w).max(meta_w);
            bubble.meta_y = y + 26.0;
            y += 36.0;
        }
        bubble.text = Some(laid);
    } else {
        inner_w = inner_w.max(meta_w);
        bubble.meta_y = y + 28.0;
        y += 36.0;
    }
    let pad_x = theme::BUBBLE_PAD_X;
    bubble.w = bubble.w.max(inner_w + 2.0 * pad_x);
    bubble.meta_x = bubble.w - pad_x - meta_w;
    bubble.h = y + 10.0;
    if let Some(quote) = bubble.quote.as_mut() {
        quote.3 = bubble.w - 2.0 * 10.0;
    }
    add_reactions(&mut bubble, message);
    bubble
}

fn add_reactions(bubble: &mut Bubble, message: &Message) {
    if message.reactions.is_empty() || message.deleted {
        return;
    }
    let mut label: String = message.reactions.iter().take(3).map(|r| r.emoji.as_str()).collect();
    let total: u32 = message.reactions.iter().map(|r| r.count).sum();
    if total > 1 {
        label.push_str(&format!(" {total}"));
    }
    let w = text::measure(&label, Weight::Medium, 32.0) + 36.0;
    bubble.reactions = Some((label, w));
    bubble.below = REACTION_H - 18.0;
}

/// The band the messages scroll in: under the header, over the composer.
fn band_of(app: &WhatsApp, height: f32) -> (f32, f32) {
    (theme::TOP + theme::HEADER, height - composer::height(app, height))
}

pub fn tick(app: &mut WhatsApp, cx: &mut Cx) {
    let (top, bottom) = band_of(app, cx.height);
    let width = cx.width;
    let connected = cx.connected;
    let Some(view) = app.chat.as_mut() else {
        return;
    };
    if view.dirty {
        view.build(width);
    }
    let max = view.max_scroll(bottom - top);
    if view.jump_to_unread && view.loaded && !view.items.is_empty() {
        view.jump_to_unread = false;
        if let Some(n) = view.items.iter().position(|item| matches!(item, Item::Unread(_))) {
            let content_y = view.tops[n];
            let s = top + 30.0 - bottom + view.total - content_y;
            view.scroll.reset();
            view.scroll.shift(s.clamp(0.0, max));
        }
    }
    if view.scroll.step(max, cx.now) {
        cx.animate();
    }
    let offset = view.scroll.value(cx.now);
    if view.more && !view.loading_older && connected && view.loaded && offset > max - PREFETCH
        && let Some(first) = view.messages.first()
    {
        let chat = view.id.clone();
        let before = first.id.clone();
        view.loading_older = true;
        view.changed();
        let request = cx.request("history", json!({"chat": chat, "before": before, "limit": 60}));
        app.asked.insert(request, Ask::History { chat, older: true });
    }
}

#[derive(Debug, Clone)]
enum Hit {
    Back,
    Header,
    Bubble { id: String, media: bool, link: Option<String> },
    ScrollButton,
    Composer(composer::Hit),
}

fn hit(app: &mut WhatsApp, x: f32, y: f32, width: f32, height: f32, now: Instant) -> Option<Hit> {
    let (top, bottom) = band_of(app, height);
    if y < top {
        if y < theme::TOP - 20.0 {
            return None;
        }
        return Some(if x < 140.0 { Hit::Back } else { Hit::Header });
    }
    if y >= bottom {
        return composer::hit(app, x, y, width, height).map(Hit::Composer);
    }
    let view = app.chat.as_mut()?;
    if view.dirty {
        view.build(width);
    }
    let offset = view.scroll.value(now).round();
    if offset > 600.0 {
        let (bx, by) = scroll_button_center(width, bottom);
        if (x - bx).hypot(y - by) < SCROLL_BUTTON * 0.7 {
            return Some(Hit::ScrollButton);
        }
    }
    let content_y = y - bottom + view.total - offset;
    let n = view.tops.partition_point(|&t| t <= content_y).checked_sub(1)?;
    let Item::Message { index, bubble, tail } = &view.items[n] else {
        return None;
    };
    let message = &view.messages[*index];
    let bx = bubble_x(bubble, *tail, width);
    let by = view.tops[n];
    let (lx, ly) = (x - bx, content_y - by);
    if lx < 0.0 || lx > bubble.w || ly > bubble.h {
        return None;
    }
    let media = bubble.media.as_ref().is_some_and(|m| {
        lx >= m.rect.x && lx < m.rect.x + m.rect.w && ly >= m.rect.y && ly < m.rect.y + m.rect.h
    });
    let mut link = None;
    if let Some(laid) = &bubble.text {
        let line = ((ly - (bubble.text_y - laid.size * 0.98)) / laid.line_height).floor();
        if line >= 0.0 {
            for (index, x0, x1, url) in laid.links() {
                if index == line as usize && lx >= theme::BUBBLE_PAD_X + x0 - 10.0 && lx <= theme::BUBBLE_PAD_X + x1 + 10.0 {
                    link = Some(rich::url_of(&url));
                }
            }
        }
    }
    Some(Hit::Bubble { id: message.id.clone(), media, link })
}

fn scroll_button_center(width: f32, bottom: f32) -> (f32, f32) {
    (width - 40.0 - SCROLL_BUTTON / 2.0, bottom - 40.0 - SCROLL_BUTTON / 2.0)
}

fn bubble_x(bubble: &Bubble, _tail: bool, width: f32) -> f32 {
    match bubble.body {
        Body::System => (width - bubble.w) / 2.0,
        _ if bubble.mine => width - theme::SIDE - bubble.w,
        _ => theme::SIDE,
    }
}

pub fn gesture(app: &mut WhatsApp, cx: &mut Cx, gesture: Gesture) {
    let (width, height, now) = (cx.width, cx.height, cx.now);
    let (top, bottom) = band_of(app, height);
    if let Some(view) = app.chat.as_mut() {
        let max = view.max_scroll(bottom - top);
        match gesture {
            Gesture::DragStart { axis: Axis::Vertical, y, start_y, .. } => {
                view.pressed = None;
                // The chat scrolls from the bottom: dragging down shows older.
                view.scroll.begin(-start_y, now);
                view.scroll.moved(-y, max, now);
                cx.animate();
                return;
            }
            Gesture::Drag { axis: Axis::Vertical, y, .. } => {
                view.scroll.moved(-y, max, now);
                cx.animate();
                return;
            }
            Gesture::DragEnd { axis: Axis::Vertical, .. } => {
                view.scroll.end(max, now);
                cx.animate();
                return;
            }
            Gesture::DragEnd { axis: Axis::Horizontal, x, .. } if x > width * 0.4 => {
                app.back(cx);
                return;
            }
            _ => {}
        }
    }
    match gesture {
        Gesture::Down { x, y } => {
            if let Some(Hit::Bubble { id, .. }) = hit(app, x, y, width, height, now)
                && let Some(view) = app.chat.as_mut()
            {
                view.pressed = Some(id);
                view.version += 1;
                cx.redraw();
            }
        }
        Gesture::Cancel | Gesture::LongPressEnd { .. } => {
            if let Some(view) = app.chat.as_mut()
                && view.pressed.take().is_some()
            {
                view.version += 1;
                cx.redraw();
            }
        }
        Gesture::Tap { x, y } => {
            if let Some(view) = app.chat.as_mut()
                && view.pressed.take().is_some()
            {
                view.version += 1;
            }
            match hit(app, x, y, width, height, now) {
                Some(Hit::Back) => app.back(cx),
                Some(Hit::Bubble { link: Some(url), .. }) => {
                    if !cx.open_url(&url) {
                        app.toast(cx, "Can't open this link");
                    }
                }
                Some(Hit::Bubble { id, media: true, .. }) => open_viewer(app, cx, &id),
                Some(Hit::ScrollButton) => {
                    if let Some(view) = app.chat.as_mut() {
                        view.scroll.reset();
                    }
                }
                Some(Hit::Composer(hit)) => composer::tap(app, cx, hit),
                _ => {}
            }
            cx.redraw();
        }
        Gesture::LongPress { x, y } => {
            if let Some(Hit::Bubble { id, .. }) = hit(app, x, y, width, height, now)
                && let Some(view) = app.chat.as_mut()
                && let Some(message) = view.messages.iter().find(|m| m.id == id)
            {
                if app.settings.writes && !message.system && !message.deleted {
                    let reply = composer::Reply {
                        id: message.id.clone(),
                        author: if message.mine { "You".into() } else { message.author.name.clone() },
                        text: if message.text.is_empty() { "Media".into() } else { message.text.replace('\n', " ") },
                    };
                    app.composer.reply = Some(reply);
                    cx.haptic(Haptic::Medium);
                    if !cx.keyboard_shown() {
                        cx.show_keyboard(crate::control::KeyboardPurpose::Multiline);
                    }
                } else if !message.text.is_empty() {
                    cx.copy(&message.text);
                    cx.haptic(Haptic::Medium);
                    let text = "Message copied";
                    app.toast(cx, text);
                }
                cx.redraw();
            }
        }
        _ => {}
    }
}

fn open_viewer(app: &mut WhatsApp, cx: &mut Cx, id: &str) {
    let Some(view) = app.chat.as_ref() else {
        return;
    };
    let Some(message) = view.messages.iter().find(|m| m.id == id) else {
        return;
    };
    let Some(media) = message.media.first() else {
        return;
    };
    let title = if message.mine { "You".to_owned() } else { message.author.name.clone() };
    app.viewer = Some(viewer::Viewer::new(media.clone(), title, message.ts, message.text.clone()));
    cx.redraw();
}

pub fn draw(app: &mut WhatsApp, canvas: &mut Canvas, cx: &mut DrawCx, full: bool) {
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    let (top, bottom) = band_of(app, height);
    let now = cx.now;
    let writes = app.settings.writes;
    let composer_key = composer::key(app);
    let Some(view) = app.chat.as_mut() else {
        return;
    };
    if view.dirty || view.width != width {
        view.build(width);
    }
    let offset = view.scroll.value(now).round();
    let max = view.max_scroll(bottom - top);
    let offset = offset.clamp(-200.0, max + 200.0);
    if full {
        canvas.fill_rect(0, 0, canvas.width, top as i32, theme::CHAT_BAR, 1.0);
    }
    let key = band::key_of(&[
        view.version,
        width as u64,
        height as u64,
        composer_key,
        u64::from(writes),
        crate::control::clock_24h() as u64,
    ]);
    let total = view.total;
    let items = &view.items;
    let tops = &view.tops;
    let messages = &view.messages;
    let pressed = view.pressed.clone();
    let rounded = &mut app.rounded;
    let arrived = avatar::arrived(cx.media, &mut view.waiting);
    let mut pics = avatar::Pics { media: &mut *cx.media, waiting: &mut view.waiting };
    let pics = &mut pics;
    let shift = view.band.draw(canvas, full || arrived, (top as i32, bottom as i32), offset as i32, key, |canvas, (y0, y1)| {
        canvas.fill_rect(0, y0, canvas.width, y1 - y0, theme::BG, 1.0);
        let base = bottom - total + offset;
        let first = tops.partition_point(|&t| base + t < y0 as f32 - 900.0);
        for n in first..items.len() {
            let y = base + tops[n];
            if y > y1 as f32 {
                break;
            }
            if y + item_height(&items[n]) + 40.0 < y0 as f32 {
                continue;
            }
            draw_item(canvas, pics, rounded, &items[n], messages, y, width, pressed.as_deref(), now);
        }
    });
    // The scroll button floats over the band: drawn again (with what is
    // under it) every frame it shows, and where the band moved its copy.
    let show_button = offset > 600.0;
    let (bx, by) = scroll_button_center(width, bottom);
    let area = |dy: f32| {
        (
            ((by - SCROLL_BUTTON / 2.0 - 8.0 + dy) as i32).max(top as i32),
            ((by + SCROLL_BUTTON / 2.0 + 8.0 + dy) as i32).min(bottom as i32),
        )
    };
    if show_button || view.button_shown {
        let mut areas = vec![area(0.0)];
        if let Some(shift) = shift
            && shift != 0
        {
            areas.push(area(shift as f32));
        }
        if shift.is_some() {
            for (y0, y1) in areas {
                if y1 <= y0 {
                    continue;
                }
                canvas.set_clip(0, y0, canvas.width, y1);
                canvas.fill_rect(0, y0, canvas.width, y1 - y0, theme::BG, 1.0);
                let base = bottom - total + offset;
                for n in 0..items.len() {
                    let y = base + tops[n];
                    if y > y1 as f32 || y + item_height(&items[n]) + 40.0 < y0 as f32 {
                        continue;
                    }
                    draw_item(canvas, pics, rounded, &items[n], messages, y, width, pressed.as_deref(), now);
                }
                canvas.reset_clip();
            }
        }
        if show_button {
            canvas.set_clip(0, top as i32, canvas.width, bottom as i32);
            canvas.shadow_rrect(
                RectF { x: bx - SCROLL_BUTTON / 2.0, y: by - SCROLL_BUTTON / 2.0, w: SCROLL_BUTTON, h: SCROLL_BUTTON },
                SCROLL_BUTTON / 2.0,
                10.0,
                0x000000,
                0.35,
            );
            canvas.fill_circle(bx, by, SCROLL_BUTTON / 2.0, theme::SURFACE, 1.0);
            icons::shared(canvas, Icon::ChevronDown, bx, by + 2.0, 56.0, theme::SECONDARY);
            canvas.reset_clip();
        }
    }
    view.button_shown = show_button;
    if full || avatar::arrived(pics.media, &mut view.header_waiting) {
        let header = Header {
            title: &view.title,
            avatar: view.avatar.as_ref(),
            group: view.group,
            members: view.members,
            push_name: view.push_name.as_deref(),
        };
        let mut header_pics = avatar::Pics { media: &mut *pics.media, waiting: &mut view.header_waiting };
        draw_header(canvas, &mut header_pics, rounded, &header, width, now);
    }
    if full {
        composer::draw(app, canvas, width, height);
    }
    // Pictures about to scroll in.
    if let Some(view) = app.chat.as_ref() {
        let base = bottom - view.total + offset;
        for (n, item) in view.items.iter().enumerate() {
            let y = base + view.tops[n];
            if y > bottom + 1600.0 || y + item_height(item) < top - 1600.0 {
                continue;
            }
            if let Item::Message { bubble, .. } = item
                && let Some(m) = &bubble.media
                && (y > bottom || y + bubble.h < top)
            {
                let (w, h) = (m.rect.w.round() as u32, m.rect.h.round() as u32);
                let _ = cx.media.get(&m.reference, w, h, Fit::Cover, now);
            }
        }
    }
}

struct Header<'a> {
    title: &'a str,
    avatar: Option<&'a crate::social::proto::Media>,
    group: bool,
    members: Option<u32>,
    push_name: Option<&'a str>,
}

fn draw_header(
    canvas: &mut Canvas,
    pics: &mut avatar::Pics,
    rounded: &mut avatar::Rounded,
    view: &Header,
    width: f32,
    now: Instant,
) {
    canvas.fill_rect(0, 0, canvas.width, (theme::TOP + theme::HEADER) as i32, theme::CHAT_BAR, 1.0);
    let cy = theme::TOP + theme::HEADER / 2.0;
    icons::draw(canvas, Glyph::Back, 56.0, cy, 62.0, theme::TEXT, 1.0);
    let size = 104.0;
    avatar::draw(canvas, pics, rounded, view.avatar, view.group, 104.0, cy - size / 2.0, size, now);
    let x = 104.0 + size + 30.0;
    let title = text::fit(view.title, Weight::Medium, 48.0, width - x - 50.0);
    let subtitle = if view.group {
        view.members.map(|n| format!("{n} members")).or_else(|| Some("tap here for group info".into()))
    } else {
        view.push_name.map(|p| format!("~ {p}"))
    };
    match subtitle {
        Some(sub) => {
            text::draw(canvas, &title, x, cy - 4.0, Weight::Medium, 48.0, theme::TEXT, 1.0);
            let sub = text::fit(&sub, Weight::Regular, 33.0, width - x - 50.0);
            text::draw(canvas, &sub, x, cy + 42.0, Weight::Regular, 33.0, theme::SECONDARY, 1.0);
        }
        None => {
            text::draw(canvas, &title, x, text::centered_baseline(cy, Weight::Medium, 48.0), Weight::Medium, 48.0, theme::TEXT, 1.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_item(
    canvas: &mut Canvas,
    pics: &mut avatar::Pics,
    rounded: &mut avatar::Rounded,
    item: &Item,
    messages: &[Message],
    y: f32,
    width: f32,
    pressed: Option<&str>,
    now: Instant,
) {
    match item {
        Item::Day(label) => chip(canvas, label, width / 2.0, y, 66.0, theme::SECONDARY),
        Item::Loading => {
            text::draw_centered(canvas, "Loading…", width / 2.0, y + 52.0, Weight::Regular, 32.0, theme::SECONDARY, 1.0);
        }
        Item::Unread(count) => {
            canvas.fill_rect(0, (y + 8.0) as i32, canvas.width, 64, theme::CHIP, 0.9);
            let label = if *count == 1 { "1 unread message".to_owned() } else { format!("{count} unread messages") };
            text::draw_centered(canvas, &label, width / 2.0, y + 52.0, Weight::Medium, 32.0, theme::TEXT, 1.0);
        }
        Item::Message { index, bubble, tail } => {
            let message = &messages[*index];
            let x = bubble_x(bubble, *tail, width);
            draw_bubble(canvas, pics, rounded, message, bubble, *tail, x, y, pressed == Some(message.id.as_str()), now);
        }
    }
}

fn chip(canvas: &mut Canvas, label: &str, cx: f32, y: f32, h: f32, color: Rgb) {
    let w = text::measure(label, Weight::Medium, 32.0) + 52.0;
    canvas.fill_rrect(RectF { x: cx - w / 2.0, y: y + 4.0, w, h: h - 8.0 }, 18.0, theme::CHIP, 1.0);
    text::draw_centered(canvas, label, cx, y + h / 2.0 + 12.0, Weight::Medium, 32.0, color, 1.0);
}

#[allow(clippy::too_many_arguments)]
fn draw_bubble(
    canvas: &mut Canvas,
    pics: &mut avatar::Pics,
    rounded: &mut avatar::Rounded,
    _message: &Message,
    bubble: &Bubble,
    tail: bool,
    x: f32,
    y: f32,
    pressed: bool,
    now: Instant,
) {
    if bubble.body == Body::System {
        let rect = RectF { x, y, w: bubble.w, h: bubble.h };
        canvas.fill_rrect(rect, 18.0, theme::CHIP, 1.0);
        if let Some(laid) = &bubble.text {
            for (n, line) in laid.lines.iter().enumerate() {
                let text_line: String = line.runs.iter().map(|r| r.text.as_str()).collect();
                text::draw_centered(canvas, &text_line, x + bubble.w / 2.0, y + 48.0 + n as f32 * laid.line_height, Weight::Regular, laid.size, theme::SECONDARY, 1.0);
            }
        }
        return;
    }
    let mine = bubble.mine;
    let fill = if mine { theme::OUT } else { theme::IN };
    let secondary = if mine { theme::OUT_SECONDARY } else { theme::SECONDARY };
    let rect = RectF { x, y, w: bubble.w, h: bubble.h };
    if bubble.body == Body::Sticker {
        if let Some(m) = &bubble.media {
            let (w, h) = (m.rect.w.round() as u32, m.rect.h.round() as u32);
            match pics.get(&m.reference, w, h, Fit::Contain, now) {
                Some(image) => {
                    let dx = (m.rect.w - image.width as f32) / 2.0;
                    let dy = (m.rect.h - image.height as f32) / 2.0;
                    canvas.draw_image(&image, (x + dx) as i32, (y + dy) as i32, 1.0);
                }
                None => canvas.fill_rrect(RectF { x, y, w: m.rect.w, h: m.rect.h }, 30.0, theme::SURFACE, 0.5),
            }
        }
        let meta_w = meta_width(&bubble.meta, mine, bubble.status.as_deref());
        let pill = RectF { x: x + bubble.meta_x - 14.0, y: y + bubble.meta_y - 32.0, w: meta_w + 28.0, h: 44.0 };
        canvas.fill_rrect(pill, 22.0, theme::CHIP, 0.9);
        draw_meta(canvas, bubble, x + bubble.meta_x, y + bubble.meta_y, theme::SECONDARY);
        draw_reactions(canvas, bubble, x, y);
        return;
    }
    canvas.fill_rrect(rect, theme::BUBBLE_RADIUS, fill, 1.0);
    if tail {
        let corner = theme::BUBBLE_RADIUS;
        if mine {
            canvas.fill_rect((x + bubble.w - corner) as i32, y as i32, corner as i32, corner as i32, fill, 1.0);
            let mut path = crate::gfx::Path::new();
            let r = x + bubble.w;
            path.polygon(&[(r - 1.0, y), (r + theme::TAIL, y), (r - 1.0, y + theme::TAIL * 1.3)]);
            canvas.fill_path(&path, fill, 1.0);
        } else {
            canvas.fill_rect(x as i32, y as i32, corner as i32, corner as i32, fill, 1.0);
            let mut path = crate::gfx::Path::new();
            path.polygon(&[(x + 1.0, y), (x - theme::TAIL, y), (x + 1.0, y + theme::TAIL * 1.3)]);
            canvas.fill_path(&path, fill, 1.0);
        }
    }
    let pad = theme::BUBBLE_PAD_X;
    let mut line_y = y + theme::BUBBLE_PAD_Y;
    if let Some((name, color, push)) = &bubble.name {
        let name_fit = text::fit(name, Weight::SemiBold, theme::T_NAME, bubble.w - 2.0 * pad);
        text::draw(canvas, &name_fit, x + pad, line_y + 36.0, Weight::SemiBold, theme::T_NAME, *color, 1.0);
        if let Some(push) = push {
            let nw = text::measure(&name_fit, Weight::SemiBold, theme::T_NAME);
            let room = bubble.w - 2.0 * pad - nw - 30.0;
            if room > 80.0 {
                let push = text::fit(push, Weight::Regular, 31.0, room);
                text::draw_right(canvas, &push, x + bubble.w - pad, line_y + 36.0, Weight::Regular, 31.0, theme::SECONDARY, 1.0);
            }
        }
        line_y += NAME_H;
    }
    if bubble.forwarded {
        icons::draw(canvas, Glyph::Forward, x + pad + 18.0, line_y + 22.0, 36.0, secondary, 1.0);
        text::draw(canvas, "Forwarded", x + pad + 46.0, line_y + 34.0, Weight::Regular, 32.0, secondary, 1.0);
        line_y += FORWARD_H;
    }
    if let Some((author, color, quoted, w)) = &bubble.quote {
        let q = RectF { x: x + 10.0, y: line_y, w: *w, h: QUOTE_H };
        canvas.fill_rrect(q, 14.0, 0x000000, 0.22);
        canvas.fill_rrect(RectF { x: q.x, y: q.y, w: 9.0, h: q.h }, 4.0, *color, 1.0);
        let author = text::fit(author, Weight::SemiBold, 34.0, q.w - 50.0);
        text::draw(canvas, &author, q.x + 30.0, q.y + 46.0, Weight::SemiBold, 34.0, *color, 1.0);
        let quoted = text::fit(quoted, Weight::Regular, 36.0, q.w - 50.0);
        text::draw(canvas, &quoted, q.x + 30.0, q.y + 98.0, Weight::Regular, 36.0, secondary, 1.0);
    }
    if let Some(m) = &bubble.media {
        draw_media(canvas, pics, rounded, m, x, y, now);
    }
    if let Some((duration, seed)) = &bubble.audio {
        let cy = y + theme::BUBBLE_PAD_Y + 56.0;
        icons::draw(canvas, Glyph::PlayFill, x + pad + 40.0, cy, 64.0, secondary, 1.0);
        let bars = 30;
        let bx = x + pad + 100.0;
        let span = bubble.w - pad - 100.0 - pad - 20.0;
        let mut state = *seed | 1;
        for n in 0..bars {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let level = 0.25 + (state % 1000) as f32 / 1000.0 * 0.75;
            let h = 10.0 + level * 54.0;
            let px = bx + n as f32 * (span / bars as f32);
            canvas.fill_rrect(RectF { x: px, y: cy - h / 2.0, w: 9.0, h }, 4.5, secondary, 0.85);
        }
        text::draw(canvas, duration, bx, y + bubble.meta_y, Weight::Regular, META_SIZE, secondary, 1.0);
    }
    if let Some((name, detail, ext)) = &bubble.file {
        let fy = y + theme::BUBBLE_PAD_Y + if bubble.quote.is_some() { QUOTE_H + 8.0 } else { 0.0 };
        let row = RectF { x: x + 10.0, y: fy, w: bubble.w - 20.0, h: 126.0 };
        canvas.fill_rrect(row, 14.0, 0x000000, 0.18);
        let icon = RectF { x: row.x + 22.0, y: row.y + 18.0, w: 72.0, h: 90.0 };
        let color = match ext.as_str() {
            "PDF" => 0xe5534b,
            "XLSX" | "XLS" | "CSV" => 0x1f9d55,
            "DOCX" | "DOC" => 0x3b82f6,
            _ => 0x8696a0,
        };
        canvas.fill_rrect(icon, 10.0, color, 1.0);
        let label = text::fit(ext, Weight::Bold, 22.0, 66.0);
        text::draw_centered(canvas, &label, icon.x + 36.0, icon.y + 78.0, Weight::Bold, 22.0, 0xffffff, 1.0);
        let name = text::fit(name, Weight::Medium, 38.0, row.w - 140.0);
        text::draw(canvas, &name, row.x + 120.0, row.y + 54.0, Weight::Medium, 38.0, theme::TEXT, 1.0);
        let detail = text::fit(detail, Weight::Regular, 30.0, row.w - 140.0);
        text::draw(canvas, &detail, row.x + 120.0, row.y + 100.0, Weight::Regular, 30.0, secondary, 1.0);
    }
    if let Some(laid) = &bubble.text {
        let deleted = bubble.body == Body::Deleted;
        let indent = if deleted { 52.0 } else { 0.0 };
        if deleted {
            icons::draw(canvas, Glyph::Blocked, x + pad + 20.0, y + bubble.text_y - 14.0, 40.0, secondary, 1.0);
        }
        let color = if deleted { secondary } else { theme::TEXT };
        let clip = canvas.clip();
        rich::draw(canvas, laid, x + pad + indent, y + bubble.text_y, Weight::Regular, color, theme::LINK, (clip.1 as f32, clip.3 as f32));
    }
    let meta_color = if bubble.meta_on_media { 0xffffff } else { secondary };
    if bubble.meta_on_media {
        let meta_w = meta_width(&bubble.meta, mine, bubble.status.as_deref());
        let shade = RectF { x: x + bubble.meta_x - 16.0, y: y + bubble.meta_y - 34.0, w: meta_w + 32.0, h: 46.0 };
        canvas.fill_rrect(shade, 23.0, 0x000000, 0.35);
    }
    draw_meta(canvas, bubble, x + bubble.meta_x, y + bubble.meta_y, meta_color);
    if pressed {
        canvas.fill_rrect(rect, theme::BUBBLE_RADIUS, 0xffffff, 0.06);
    }
    draw_reactions(canvas, bubble, x, y);
}

fn draw_meta(canvas: &mut Canvas, bubble: &Bubble, x: f32, baseline: f32, color: Rgb) {
    text::draw(canvas, &bubble.meta, x, baseline, Weight::Regular, META_SIZE, color, 1.0);
    if let Some(status) = &bubble.status {
        let w = text::measure(&bubble.meta, Weight::Regular, META_SIZE);
        list::draw_ticks(canvas, status, x + w + 8.0, baseline - 10.0, 40.0, color);
    }
}

fn draw_reactions(canvas: &mut Canvas, bubble: &Bubble, x: f32, y: f32) {
    let Some((label, w)) = &bubble.reactions else {
        return;
    };
    let rx = if bubble.mine { x + bubble.w - w - 26.0 } else { x + 26.0 };
    let rect = RectF { x: rx, y: y + bubble.h - 16.0, w: *w, h: REACTION_H };
    canvas.fill_rrect(RectF { x: rect.x - 4.0, y: rect.y - 4.0, w: rect.w + 8.0, h: rect.h + 8.0 }, (REACTION_H + 8.0) / 2.0, theme::BG, 1.0);
    canvas.fill_rrect(rect, REACTION_H / 2.0, theme::SURFACE, 1.0);
    text::draw(canvas, label, rect.x + 18.0, rect.y + 44.0, Weight::Medium, 32.0, theme::TEXT, 1.0);
}

fn draw_media(
    canvas: &mut Canvas,
    pics: &mut avatar::Pics,
    rounded: &mut avatar::Rounded,
    m: &MediaBox,
    x: f32,
    y: f32,
    now: Instant,
) {
    let r = RectF { x: x + m.rect.x, y: y + m.rect.y, w: m.rect.w, h: m.rect.h };
    let (w, h) = (r.w.round() as u32, r.h.round() as u32);
    // A video shows its poster (the thumbnail); a photo itself, its
    // thumbnail until it has come.
    let full = if m.kind == "image" { pics.get(&m.reference, w, h, Fit::Cover, now) } else { None };
    let picture = full.or_else(|| m.thumb.as_ref().and_then(|thumb| pics.get(thumb, w, h, Fit::Cover, now)));
    match picture {
        Some(image) => {
            let round = rounded.get(&image, 16.0);
            let dx = (r.w - round.width as f32) / 2.0;
            let dy = (r.h - round.height as f32) / 2.0;
            canvas.draw_image(&round, (r.x + dx).round() as i32, (r.y + dy).round() as i32, 1.0);
        }
        None => canvas.fill_rrect(r, 16.0, 0x000000, 0.25),
    }
    if m.kind == "video" || m.kind == "gif" {
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        canvas.fill_circle(cx, cy, 64.0, 0x000000, 0.5);
        if m.kind == "gif" {
            text::draw_centered(canvas, "GIF", cx, cy + 14.0, Weight::Bold, 40.0, 0xffffff, 1.0);
        } else {
            icons::draw(canvas, Glyph::PlayFill, cx + 4.0, cy, 70.0, 0xffffff, 1.0);
        }
        if m.kind == "video"
            && let Some(label) = &m.label
        {
            icons::shared(canvas, Icon::Camera, r.x + 40.0, r.y + r.h - 34.0, 38.0, 0xffffff);
            text::draw(canvas, label, r.x + 70.0, r.y + r.h - 22.0, Weight::Medium, 30.0, 0xffffff, 1.0);
        }
    }
}
