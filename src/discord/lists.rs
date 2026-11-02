//! The plain lists: the Messages tab (DMs and group DMs, newest first, with
//! their last line), a forum's posts, and a channel's member list.

#![allow(dead_code)]

use std::time::Instant;

use crate::comms::Scroll;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::exocortex::ui::icons::{self, Icon};
use crate::gfx::{Canvas, RectF};
use crate::social::media::Media;
use crate::social::proto::Chat;
use crate::text::{self, Weight};

use super::Action;
use super::model::Model;
use super::theme::{self, dp};
use super::ui;

/// Vertical scrolling of a list between `top` and `bottom`.
pub struct ListScroll {
    pub scroll: Scroll,
}

impl ListScroll {
    pub fn new() -> Self {
        Self {
            scroll: Scroll::default(),
        }
    }

    /// Feeds a drag gesture; true when it was one.
    pub fn gesture(
        &mut self,
        gesture: &Gesture,
        max: f32,
        now: Instant,
        actions: &mut Vec<Action>,
    ) -> bool {
        match *gesture {
            Gesture::DragStart {
                axis: Axis::Vertical,
                y,
                start_y,
                ..
            } => {
                self.scroll.begin(start_y, now);
                self.scroll.moved(y, max, now);
                actions.push(Action::Redraw);
                true
            }
            Gesture::Drag {
                axis: Axis::Vertical,
                y,
                ..
            } => {
                self.scroll.moved(y, max, now);
                actions.push(Action::Redraw);
                true
            }
            Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            } => {
                self.scroll.end(max, now);
                actions.push(Action::Animate);
                true
            }
            _ => false,
        }
    }
}

impl Default for ListScroll {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Messages

const DM_ROW: f32 = dp(68.0);
const DM_HEADER: f32 = dp(64.0);

pub struct MessagesView {
    pub list: ListScroll,
    rows: Vec<String>,
    key: u64,
}

fn tab_bottom(height: f32) -> f32 {
    height - theme::SAFE_BOTTOM - theme::TAB_H
}

impl MessagesView {
    pub fn new() -> Self {
        Self {
            list: ListScroll::new(),
            rows: Vec::new(),
            key: u64::MAX,
        }
    }

    fn sync(&mut self, model: &Model) {
        if self.key == model.chats_revision {
            return;
        }
        self.key = model.chats_revision;
        let mut dms: Vec<&Chat> = model
            .chats
            .values()
            .filter(|chat| chat.kind == "dm" || chat.kind == "group")
            .collect();
        dms.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.ts.cmp(&a.ts))
                .then(a.id.cmp(&b.id))
        });
        self.rows = dms.into_iter().map(|chat| chat.id.clone()).collect();
    }

    fn max(&self, height: f32) -> f32 {
        (DM_HEADER + self.rows.len() as f32 * DM_ROW + dp(16.0) - (tab_bottom(height) - theme::TOP))
            .max(0.0)
    }

    pub fn tick(&mut self, model: &Model, height: f32, now: Instant) -> bool {
        self.sync(model);
        self.list.scroll.step(self.max(height), now)
    }

    pub fn gesture(
        &mut self,
        gesture: Gesture,
        model: &Model,
        height: f32,
        now: Instant,
    ) -> Vec<Action> {
        self.sync(model);
        let mut actions = Vec::new();
        if self
            .list
            .gesture(&gesture, self.max(height), now, &mut actions)
        {
            return actions;
        }
        if let Gesture::Tap { y, .. } = gesture
            && y > theme::TOP + DM_HEADER
            && y < tab_bottom(height)
        {
            let index =
                ((y - theme::TOP - DM_HEADER + self.list.scroll.value(now)) / DM_ROW).floor();
            if index >= 0.0
                && let Some(id) = self.rows.get(index as usize)
            {
                actions.push(Action::OpenChat(id.clone()));
            }
        }
        actions
    }

    pub fn draw(&mut self, canvas: &mut Canvas, model: &Model, media: &mut Media, now: Instant) {
        self.sync(model);
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        let bottom = tab_bottom(height);
        canvas.fill_rect(0, 0, width as i32, height as i32, theme::RAIL, 1.0);
        let offset = self.list.scroll.value(now);
        canvas.set_clip(0, theme::TOP as i32, width as i32, bottom as i32);
        text::draw(
            canvas,
            "Messages",
            theme::MARGIN,
            theme::TOP - offset + dp(40.0),
            Weight::Bold,
            theme::T_H2,
            theme::HEADER,
            1.0,
        );
        let first = ((offset - DM_HEADER) / DM_ROW).floor().max(0.0) as usize;
        for (i, id) in self.rows.iter().enumerate().skip(first) {
            let y = theme::TOP + DM_HEADER + i as f32 * DM_ROW - offset;
            if y > bottom {
                break;
            }
            let Some(chat) = model.chats.get(id) else {
                continue;
            };
            let cy = y + DM_ROW / 2.0;
            let size = dp(44.0);
            if chat.kind == "group" && chat.avatar.is_none() {
                canvas.fill_circle(
                    theme::MARGIN + size / 2.0,
                    cy,
                    size / 2.0,
                    theme::BLURPLE,
                    1.0,
                );
                icons::draw(
                    canvas,
                    Icon::Chat,
                    theme::MARGIN + size / 2.0,
                    cy,
                    dp(22.0),
                    theme::WHITE,
                    1.0,
                );
            } else {
                ui::avatar(
                    canvas,
                    media,
                    chat.avatar.as_ref(),
                    theme::MARGIN,
                    cy - size / 2.0,
                    size,
                    0x41434a,
                    now,
                );
            }
            let x = theme::MARGIN + size + dp(12.0);
            let unread = chat.unread > 0 && !chat.muted;
            let when = ui::short_time(chat.last.as_ref().map_or(chat.ts, |l| l.ts));
            let when_w = text::measure(&when, Weight::Regular, theme::T_TINY);
            let right = width - theme::MARGIN;
            let name_weight = if unread {
                Weight::Bold
            } else {
                Weight::SemiBold
            };
            let name = ui::fit(
                &chat.title,
                name_weight,
                theme::T_BODY,
                right - x - when_w - dp(12.0),
            );
            text::draw(
                canvas,
                &name,
                x,
                cy - dp(3.0),
                name_weight,
                theme::T_BODY,
                if unread || !chat.muted {
                    theme::HEADER
                } else {
                    theme::MUTED
                },
                1.0,
            );
            text::draw_right(
                canvas,
                &when,
                right,
                cy - dp(4.0),
                Weight::Regular,
                theme::T_TINY,
                theme::MUTED,
                1.0,
            );
            let preview = match &chat.last {
                Some(last) if last.mine => format!("You: {}", last.text),
                Some(last) if chat.kind == "group" => {
                    format!("{}: {}", last.author.clone().unwrap_or_default(), last.text)
                }
                Some(last) => last.text.clone(),
                None => match chat.members {
                    Some(n) => format!("{n} Members"),
                    None => String::new(),
                },
            };
            let badge_room = if unread { dp(34.0) } else { 0.0 };
            let preview = ui::fit(
                &preview.replace('\n', " "),
                Weight::Regular,
                theme::T_SMALL,
                right - x - badge_room,
            );
            text::draw(
                canvas,
                &preview,
                x,
                cy + dp(17.0),
                if unread {
                    Weight::Medium
                } else {
                    Weight::Regular
                },
                theme::T_SMALL,
                if unread { theme::TEXT } else { theme::MUTED },
                1.0,
            );
            if unread {
                ui::badge(canvas, chat.unread, right - dp(12.0), cy + dp(12.0), None);
            }
        }
        if self.rows.is_empty() {
            let note = if model.status.state == "online" {
                "No direct messages yet"
            } else {
                "Loading…"
            };
            text::draw_centered(
                canvas,
                note,
                width / 2.0,
                height / 2.0,
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            );
        }
        canvas.reset_clip();
    }
}

impl Default for MessagesView {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// A forum's posts, a channel's members

const POST_ROW: f32 = dp(78.0);
const MEMBER_ROW: f32 = dp(48.0);

pub struct ForumView {
    pub chat: String,
    pub list: ListScroll,
}

fn page_header(canvas: &mut Canvas, title: &str, kind: &str, channel_type: i64, width: f32) {
    let top = theme::TOP;
    canvas.fill_rect(
        0,
        0,
        width as i32,
        (top + theme::HEADER_H) as i32,
        theme::CHAT,
        1.0,
    );
    canvas.fill_rect(
        0,
        (top + theme::HEADER_H) as i32 - 2,
        width as i32,
        2,
        theme::HAIR,
        1.0,
    );
    let cy = top + theme::HEADER_H / 2.0;
    icons::draw(
        canvas,
        Icon::ChevronLeft,
        dp(24.0),
        cy,
        dp(26.0),
        theme::INTERACTIVE,
        1.0,
    );
    let mut x = dp(52.0);
    if !kind.is_empty() {
        super::chat::channel_icon(
            canvas,
            kind,
            channel_type,
            x + dp(10.0),
            cy,
            dp(20.0),
            theme::MUTED,
        );
        x += dp(26.0);
    }
    let name = ui::fit(title, Weight::Bold, theme::T_BODY, width - x - dp(24.0));
    text::draw(
        canvas,
        &name,
        x,
        text::centered_baseline(cy, Weight::Bold, theme::T_BODY),
        Weight::Bold,
        theme::T_BODY,
        theme::HEADER,
        1.0,
    );
}

impl ForumView {
    pub fn new(chat: String) -> Self {
        Self {
            chat,
            list: ListScroll::new(),
        }
    }

    fn top() -> f32 {
        theme::TOP + theme::HEADER_H
    }

    fn max(&self, model: &Model, height: f32) -> f32 {
        let count = model.threads.get(&self.chat).map_or(0, Vec::len);
        (count as f32 * POST_ROW + dp(16.0) - (height - theme::SAFE_BOTTOM - Self::top())).max(0.0)
    }

    pub fn gesture(
        &mut self,
        gesture: Gesture,
        model: &Model,
        width: f32,
        height: f32,
        now: Instant,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if self
            .list
            .gesture(&gesture, self.max(model, height), now, &mut actions)
        {
            return actions;
        }
        match gesture {
            Gesture::Tap { x, y } if y < Self::top() => {
                if x < dp(56.0) {
                    actions.push(Action::Back);
                }
            }
            Gesture::Tap { y, .. } => {
                let index =
                    ((y - Self::top() - dp(8.0) + self.list.scroll.value(now)) / POST_ROW).floor();
                if index >= 0.0
                    && let Some(post) = model
                        .threads
                        .get(&self.chat)
                        .and_then(|posts| posts.get(index as usize))
                {
                    actions.push(Action::OpenChat(post.id.clone()));
                }
            }
            Gesture::Back => actions.push(Action::Back),
            Gesture::DragEnd {
                axis: Axis::Horizontal,
                x,
                vx,
                ..
            } if x > width * 0.35 || vx > 1200.0 => actions.push(Action::Back),
            _ => {}
        }
        actions
    }

    pub fn tick(&mut self, model: &Model, height: f32, now: Instant) -> bool {
        self.list.scroll.step(self.max(model, height), now)
    }

    pub fn draw(&mut self, canvas: &mut Canvas, model: &Model, now: Instant) {
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        canvas.fill_rect(0, 0, width as i32, height as i32, theme::CHAT, 1.0);
        let offset = self.list.scroll.value(now);
        let top = Self::top();
        canvas.set_clip(0, top as i32, width as i32, height as i32);
        match model.threads.get(&self.chat) {
            None => text::draw_centered(
                canvas,
                "Loading posts…",
                width / 2.0,
                height / 2.0,
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            ),
            Some(posts) if posts.is_empty() => text::draw_centered(
                canvas,
                "No posts yet",
                width / 2.0,
                height / 2.0,
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            ),
            Some(posts) => {
                for (i, post) in posts.iter().enumerate() {
                    let y = top + dp(8.0) + i as f32 * POST_ROW - offset;
                    if y > height {
                        break;
                    }
                    if y + POST_ROW < top {
                        continue;
                    }
                    let card = RectF {
                        x: dp(12.0),
                        y: y + dp(4.0),
                        w: width - dp(24.0),
                        h: POST_ROW - dp(8.0),
                    };
                    canvas.fill_rrect(card, dp(10.0), theme::LIST, 1.0);
                    let unread = post.unread > 0;
                    let title = ui::fit(
                        &post.title,
                        Weight::SemiBold,
                        theme::T_BODY,
                        card.w - dp(32.0),
                    );
                    text::draw(
                        canvas,
                        &title,
                        card.x + dp(16.0),
                        card.y + dp(28.0),
                        Weight::SemiBold,
                        theme::T_BODY,
                        if unread { theme::HEADER } else { theme::TEXT },
                        1.0,
                    );
                    let count = model.ext(&post.id).and_then(|e| e.messages).or_else(|| {
                        post.ext
                            .get("messages")
                            .and_then(|v| v.as_u64())
                            .map(|n| n as u32)
                    });
                    let detail = match count {
                        Some(n) => format!("{n} messages · {}", ui::short_time(post.ts)),
                        None => ui::short_time(post.ts),
                    };
                    text::draw(
                        canvas,
                        &detail,
                        card.x + dp(16.0),
                        card.y + dp(52.0),
                        Weight::Regular,
                        theme::T_TINY,
                        theme::MUTED,
                        1.0,
                    );
                }
            }
        }
        canvas.reset_clip();
        let ext = model.ext(&self.chat);
        page_header(
            canvas,
            &model.title(&self.chat),
            "forum",
            ext.map_or(15, |e| e.channel_type),
            width,
        );
    }
}

pub struct MembersView {
    pub chat: String,
    pub list: ListScroll,
}

impl MembersView {
    pub fn new(chat: String) -> Self {
        Self {
            chat,
            list: ListScroll::new(),
        }
    }

    fn top() -> f32 {
        theme::TOP + theme::HEADER_H
    }

    fn max(&self, model: &Model, height: f32) -> f32 {
        let count = model
            .member_lists
            .get(&self.chat)
            .map_or(0, |l| l.items.len());
        (count as f32 * MEMBER_ROW + dp(16.0) - (height - theme::SAFE_BOTTOM - Self::top()))
            .max(0.0)
    }

    pub fn gesture(
        &mut self,
        gesture: Gesture,
        model: &Model,
        width: f32,
        height: f32,
        now: Instant,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if self
            .list
            .gesture(&gesture, self.max(model, height), now, &mut actions)
        {
            return actions;
        }
        match gesture {
            Gesture::Tap { x, y } if y < Self::top() && x < dp(56.0) => actions.push(Action::Back),
            Gesture::Back => actions.push(Action::Back),
            Gesture::DragEnd {
                axis: Axis::Horizontal,
                x,
                vx,
                ..
            } if x > width * 0.35 || vx > 1200.0 => actions.push(Action::Back),
            _ => {}
        }
        actions
    }

    pub fn tick(&mut self, model: &Model, height: f32, now: Instant) -> bool {
        self.list.scroll.step(self.max(model, height), now)
    }

    pub fn draw(&mut self, canvas: &mut Canvas, model: &Model, media: &mut Media, now: Instant) {
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        canvas.fill_rect(0, 0, width as i32, height as i32, theme::LIST, 1.0);
        let offset = self.list.scroll.value(now);
        let top = Self::top();
        canvas.set_clip(0, top as i32, width as i32, height as i32);
        match model.member_lists.get(&self.chat) {
            None => text::draw_centered(
                canvas,
                "Loading members…",
                width / 2.0,
                height / 2.0,
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            ),
            Some(list) => {
                let names: std::collections::HashMap<&str, (&str, u32)> = list
                    .groups
                    .iter()
                    .map(|g| (g.id.as_str(), (g.name.as_str(), g.count)))
                    .collect();
                for (i, item) in list.items.iter().enumerate() {
                    let y = top + dp(8.0) + i as f32 * MEMBER_ROW - offset;
                    if y > height {
                        break;
                    }
                    if y + MEMBER_ROW < top {
                        continue;
                    }
                    let cy = y + MEMBER_ROW / 2.0;
                    if let Some(group) = &item.group {
                        let (name, count) = names
                            .get(group.as_str())
                            .copied()
                            .unwrap_or((group.as_str(), item.count.unwrap_or(0)));
                        let label = format!("{} — {count}", name.to_uppercase());
                        text::draw_tracked(
                            canvas,
                            &label,
                            theme::MARGIN,
                            cy + dp(8.0),
                            Weight::SemiBold,
                            theme::T_TINY,
                            0.6,
                            theme::MUTED,
                            1.0,
                        );
                        continue;
                    }
                    let size = dp(34.0);
                    ui::avatar(
                        canvas,
                        media,
                        item.avatar.as_ref(),
                        theme::MARGIN,
                        cy - size / 2.0,
                        size,
                        0x41434a,
                        now,
                    );
                    let offline = item.status.as_deref() == Some("offline");
                    if let Some(status) = item.status.as_deref() {
                        let color = match status {
                            "online" => theme::GREEN,
                            "idle" => theme::YELLOW,
                            "dnd" => theme::RED,
                            _ => 0x80848e,
                        };
                        let (sx, sy) = (theme::MARGIN + size - dp(4.0), cy + size / 2.0 - dp(4.0));
                        canvas.fill_circle(sx, sy, dp(6.5), theme::LIST, 1.0);
                        canvas.fill_circle(sx, sy, dp(4.5), color, 1.0);
                    }
                    let name = ui::fit(
                        &item.name,
                        Weight::Medium,
                        theme::T_BODY,
                        width - theme::MARGIN * 2.0 - size - dp(12.0),
                    );
                    text::draw(
                        canvas,
                        &name,
                        theme::MARGIN + size + dp(12.0),
                        text::centered_baseline(cy, Weight::Medium, theme::T_BODY),
                        Weight::Medium,
                        theme::T_BODY,
                        theme::name_color(item.color),
                        if offline { 0.5 } else { 1.0 },
                    );
                }
            }
        }
        canvas.reset_clip();
        page_header(canvas, "Members", "", 0, width);
    }
}
