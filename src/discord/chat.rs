//! A channel, thread or DM open: its messages grouped by author as
//! Discord's mobile app shows them (avatar and name at a group's start, day
//! dividers), newest at the bottom; older pages load as the top nears. The
//! composer under them sends (with the bridge's writes on), replies, and
//! shows who is typing. Long press opens a message's actions; a tap opens a
//! link, a picture, a spoiler or a thread.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use crate::comms::Scroll;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::exocortex::ui::icons::{self, Icon};
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::social::media::Media;
use crate::text::{self, Weight};

use super::Action;
use super::layout::{self, DrawEnv, Hit, MessageLayout};
use super::model::{Entry, Model};
use super::theme::{self, dp};
use super::ui;
use crate::social::shell::Damage;

/// Messages by one author closer than this share a group.
const GROUP_MS: i64 = 7 * 60 * 1000;
const LOAD_AHEAD: f32 = 2400.0;
const COMPOSER_LINES: usize = 6;
const QUICK_REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🔥"];

#[derive(Debug, Clone)]
enum Row {
    /// The channel's first message is loaded: "Welcome to #name!".
    Start,
    Day(String),
    /// Where the unread messages begin (Discord's red "NEW" line).
    New,
    Message {
        index: usize,
        header: bool,
    },
}

/// A message's long-press menu.
pub struct Sheet {
    pub message: String,
    pub opened: Instant,
}

/// What the last frame showed, for drawing the next one in part.
#[derive(Debug, Clone, PartialEq)]
struct Frame {
    offset: i32,
    key: (i32, i32, i32, i32),
    /// The bar's words, if it showed.
    bar: Option<String>,
    /// An older page was loading (the header says so).
    loading: bool,
}

/// The bar over the bottom of the messages while scrolled up.
const BAR_H: f32 = dp(34.0);

/// Exact layouts are kept this many screens beyond the view (above it,
/// where older messages are); further up, rows carry an estimate until
/// they come near (a page of fifty costs a frame no longer).
const REACH_SCREENS: f32 = 1.0;
/// Rows laid out ahead of the view per pass of the loop.
const LAYOUTS_PER_TICK: usize = 4;
/// What a missed picture in the header (a DM's avatar) redraws: the frame.
const HEADER_TAG: u64 = 6;

pub struct ChatView {
    pub chat: String,
    /// Unread messages (a DM's count) when the chat opened, until the NEW line is placed.
    pub unread_at_open: u32,
    new_marker: Option<String>,
    last_frame: Option<Frame>,
    /// Something besides the scroll offset changed: the next frame is drawn whole.
    pub dirty: bool,
    pub scroll: Scroll,
    rows: Vec<Row>,
    /// The entries `rows` index (the history as of the last `sync`).
    entries: Vec<Rc<Entry>>,
    /// Rows whose height is an estimate (no layout yet).
    estimated: Vec<bool>,
    heights: Vec<f32>,
    /// Distance from the content's bottom to each row's bottom.
    bottoms: Vec<f32>,
    total: f32,
    built: (u64, u32),
    layouts: HashMap<String, (u64, bool, u32, Rc<MessageLayout>)>,
    pub text: String,
    caret: crate::exocortex::editor::Cursor,
    pub focused: bool,
    pub reply: Option<(String, String)>,
    revealed: HashSet<(String, u16)>,
    pub sheet: Option<Sheet>,
    /// Arrived while scrolled up.
    new_below: u32,
    last_newest: Option<String>,
    pressed: Option<String>,
    client_seq: u64,
}

fn profile() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("SOCIAL_APP_PROFILE").is_some()
            || std::env::var_os("EXOCORTEX_APP_PROFILE").is_some()
    })
}

pub fn channel_icon(
    canvas: &mut Canvas,
    kind: &str,
    channel_type: i64,
    cx: f32,
    cy: f32,
    size: f32,
    color: u32,
) {
    match (kind, channel_type) {
        ("voice", _) => ui::glyph(canvas, ui::Glyph::Speaker, cx, cy, size, color, 1.0),
        ("forum", _) => ui::glyph(canvas, ui::Glyph::Forum, cx, cy, size, color, 1.0),
        (_, 5) => ui::glyph(canvas, ui::Glyph::Megaphone, cx, cy, size, color, 1.0),
        ("thread", _) => icons::draw(canvas, Icon::MessageSquare, cx, cy, size * 0.85, color, 1.0),
        _ => {
            let weight = Weight::Medium;
            let s = size * 1.05;
            text::draw_centered(
                canvas,
                "#",
                cx,
                text::centered_baseline(cy, weight, s),
                weight,
                s,
                color,
                1.0,
            );
        }
    }
}

impl ChatView {
    pub fn new(chat: String) -> Self {
        Self {
            chat,
            unread_at_open: 0,
            new_marker: None,
            last_frame: None,
            dirty: true,
            scroll: Scroll::default(),
            rows: Vec::new(),
            entries: Vec::new(),
            estimated: Vec::new(),
            heights: Vec::new(),
            bottoms: Vec::new(),
            total: 0.0,
            built: (u64::MAX, 0),
            layouts: HashMap::new(),
            text: String::new(),
            caret: Default::default(),
            focused: false,
            reply: None,
            revealed: HashSet::new(),
            sheet: None,
            new_below: 0,
            last_newest: None,
            pressed: None,
            client_seq: 0,
        }
    }

    fn content_top() -> f32 {
        theme::TOP + theme::HEADER_H
    }

    /// The composer's height (its field grows with the text).
    fn composer_h(&self, width: f32) -> f32 {
        let lines = self.composer_lines(width).len().clamp(1, COMPOSER_LINES);
        let field = dp(40.0) + (lines as f32 - 1.0) * theme::L_BODY;
        let reply = if self.reply.is_some() { dp(34.0) } else { 0.0 };
        field + dp(16.0) + reply
    }

    fn bottom_inset(height: f32) -> f32 {
        // With the keyboard up the window is shorter and its bottom is the keyboard's top.
        if height < crate::REQUESTED_HEIGHT as f32 - 300.0 {
            dp(4.0)
        } else {
            theme::SAFE_BOTTOM
        }
    }

    fn content_bottom(&self, width: f32, height: f32) -> f32 {
        height - Self::bottom_inset(height) - self.composer_h(width) - dp(18.0)
    }

    fn visible(&self, width: f32, height: f32) -> f32 {
        (self.content_bottom(width, height) - Self::content_top()).max(1.0)
    }

    pub fn max_offset(&self, width: f32, height: f32) -> f32 {
        (self.total - self.visible(width, height)).max(0.0)
    }

    fn text_width(width: f32) -> f32 {
        width - theme::CONTENT_X - theme::MARGIN
    }

    fn layout_for(
        &mut self,
        entry: &Entry,
        header: bool,
        width: f32,
        model: &Model,
    ) -> Rc<MessageLayout> {
        let w = Self::text_width(width);
        let key = w.round() as u32;
        if let Some((revision, h, wk, layout)) = self.layouts.get(&entry.message.id)
            && *revision == entry.revision
            && *h == header
            && *wk == key
        {
            return layout.clone();
        }
        let channel = |id: &str| model.chats.get(id).map(|chat| chat.title.clone());
        let layout = Rc::new(layout::message(entry, w, header, &channel));
        self.layouts.insert(
            entry.message.id.clone(),
            (entry.revision, header, key, layout.clone()),
        );
        layout
    }

    fn row_height(layout: &MessageLayout) -> f32 {
        let pad = if layout.header { dp(16.0) } else { dp(2.0) };
        let min = if layout.header && !layout.system {
            theme::AVATAR
        } else {
            0.0
        };
        pad + layout.height.max(min) + dp(2.0)
    }

    /// The layout already made for this entry as it is now, if any.
    fn cached(&self, entry: &Entry, header: bool, width: f32) -> Option<Rc<MessageLayout>> {
        let key = Self::text_width(width).round() as u32;
        match self.layouts.get(&entry.message.id) {
            Some((revision, h, wk, layout))
                if *revision == entry.revision && *h == header && *wk == key =>
            {
                Some(layout.clone())
            }
            _ => None,
        }
    }

    /// A row's height before its layout is made: near enough that the
    /// rows above it settle unseen when it is laid out.
    fn estimate(entry: &Entry, header: bool, text_width: f32) -> f32 {
        let message = &entry.message;
        let per_line = (text_width / (theme::T_BODY * 0.5)).max(10.0);
        let lines: f32 = message
            .text
            .split('\n')
            .map(|line| (line.chars().count() as f32 / per_line).ceil().max(1.0))
            .sum();
        let mut h = if message.text.trim().is_empty() {
            0.0
        } else {
            lines * theme::L_BODY
        };
        if header && !message.system {
            h += layout::HEADER_LINE + dp(2.0);
        }
        if message.reply_to.is_some() {
            h += layout::REPLY_H + dp(4.0);
        }
        let pictures: Vec<_> = message
            .media
            .iter()
            .filter(|m| matches!(m.kind.as_str(), "image" | "gif" | "video"))
            .collect();
        let max_w = text_width.min(dp(330.0));
        match pictures.as_slice() {
            [] => {}
            [one] => h += layout::fit_box(one.w, one.h, max_w, dp(330.0)).1 + dp(4.0),
            many => {
                let cell = (max_w - dp(4.0)) / 2.0;
                let rows = many.len().div_ceil(2) as f32;
                h += rows * cell + (rows - 1.0) * dp(4.0) + dp(4.0);
            }
        }
        h += entry.ext.embeds.len() as f32 * dp(140.0);
        if !message.reactions.is_empty() {
            h += dp(34.0);
        }
        let pad = if header { dp(16.0) } else { dp(2.0) };
        let min = if header && !message.system {
            theme::AVATAR
        } else {
            0.0
        };
        pad + h.max(min) + dp(2.0)
    }

    /// What is in view at `offset`: each row's identity, where it sits and how tall it is.
    fn in_view(&self, offset: f32, visible: f32) -> Vec<(u64, i32, i32)> {
        let mut out = Vec::new();
        for i in (0..self.rows.len()).rev() {
            if self.bottoms[i] >= offset + visible {
                break;
            }
            if self.bottoms[i] + self.heights[i] <= offset {
                continue;
            }
            let identity = match &self.rows[i] {
                Row::Start => 1,
                Row::New => 2,
                Row::Day(label) => ui::tag_of(label),
                Row::Message { index, header } => {
                    let entry = &self.entries[*index];
                    ui::tag_of(&entry.message.id)
                        ^ entry.revision.wrapping_mul(0x9e37_79b9_7f4a_7c15)
                        ^ u64::from(*header) << 1
                }
            };
            out.push((
                identity,
                (self.bottoms[i] - offset).round() as i32,
                self.heights[i].round() as i32,
            ));
        }
        out
    }

    /// Lays out estimated rows from the bottom up as far as `reach` above
    /// the view's bottom, at most `budget` of them. A row in view that
    /// changes height makes the frame whole; one above the view moves only
    /// rows above it, unseen.
    fn lay_out_ahead(
        &mut self,
        model: &Model,
        width: f32,
        height: f32,
        offset: f32,
        reach: f32,
        budget: usize,
    ) {
        if !self.estimated.iter().any(|e| *e) {
            return;
        }
        let visible = self.visible(width, height);
        let mut changed = false;
        let mut done = 0;
        for i in (0..self.rows.len()).rev() {
            if self.bottoms[i] > offset + reach || done >= budget {
                break;
            }
            if !self.estimated[i] {
                continue;
            }
            let Row::Message { index, header } = self.rows[i] else {
                continue;
            };
            let entry = self.entries[index].clone();
            let layout = self.layout_for(&entry, header, width, model);
            let h = Self::row_height(&layout);
            self.estimated[i] = false;
            done += 1;
            if (h - self.heights[i]).abs() >= 0.5 {
                self.heights[i] = h;
                changed = true;
                if self.bottoms[i] < offset + visible {
                    self.dirty = true;
                }
            }
        }
        if changed {
            let mut acc = 0.0;
            for i in (0..self.rows.len()).rev() {
                self.bottoms[i] = acc;
                acc += self.heights[i];
            }
            self.total = acc + dp(8.0);
        }
    }

    /// Rebuilds the rows when the history changed (or the width did).
    pub fn sync(&mut self, model: &Model, width: f32, height: f32) {
        let Some(history) = model.histories.get(&self.chat) else {
            return;
        };
        let key = (history.revision, width.round() as u32);
        if key == self.built {
            return;
        }
        let old_total = self.total;
        let old_newest = self.last_newest.clone();
        self.built = key;
        let now = Instant::now();
        let started = now;
        let visible = self.visible(width, height);
        let before = self.in_view(self.scroll.value(now), visible);
        let laid_before = self.layouts.len();
        let entries = history.entries.clone();
        let mut rows = Vec::with_capacity(entries.len() + 8);
        if history.loaded && !history.more {
            rows.push(Row::Start);
        }
        // The unread messages counted when the chat opened: the line goes above the first.
        if self.new_marker.is_none()
            && self.unread_at_open > 0
            && history.loaded
            && !entries.is_empty()
        {
            let mut left = self.unread_at_open;
            let mut marker = None;
            for entry in entries.iter().rev() {
                if entry.message.mine {
                    continue;
                }
                marker = Some(entry.message.id.clone());
                left -= 1;
                if left == 0 {
                    break;
                }
            }
            self.new_marker = marker;
            self.unread_at_open = 0;
        }
        let mut previous: Option<&Rc<Entry>> = None;
        for (index, entry) in entries.iter().enumerate() {
            let message = &entry.message;
            let new_day = previous.is_none_or(|p| !ui::same_day(p.message.ts, message.ts));
            if new_day {
                rows.push(Row::Day(ui::day_label(message.ts)));
            }
            if self.new_marker.as_deref() == Some(message.id.as_str()) {
                rows.push(Row::New);
            }
            let header = new_day
                || message.system
                || message.reply_to.is_some()
                || previous.is_none_or(|p| {
                    p.message.author.id != message.author.id
                        || p.message.system
                        || message.ts - p.message.ts > GROUP_MS
                });
            rows.push(Row::Message { index, header });
            previous = Some(entry);
        }
        // From the newest up: exact within reach of the view, estimated beyond.
        let reach = self.scroll.value(now) + visible * (1.0 + REACH_SCREENS);
        let mut heights = vec![0.0; rows.len()];
        let mut estimated = vec![false; rows.len()];
        let mut acc = 0.0;
        for i in (0..rows.len()).rev() {
            heights[i] = match &rows[i] {
                Row::Start => dp(150.0),
                Row::Day(_) => dp(36.0),
                Row::New => dp(20.0),
                Row::Message { index, header } => {
                    let entry = &entries[*index];
                    if acc <= reach || self.cached(entry, *header, width).is_some() {
                        let layout = self.layout_for(entry, *header, width, model);
                        Self::row_height(&layout)
                    } else {
                        estimated[i] = true;
                        Self::estimate(entry, *header, Self::text_width(width))
                    }
                }
            };
            acc += heights[i];
        }
        let mut bottoms = vec![0.0; rows.len()];
        let mut acc = 0.0;
        for i in (0..rows.len()).rev() {
            bottoms[i] = acc;
            acc += heights[i];
        }
        self.rows = rows;
        self.entries = entries.clone();
        self.estimated = estimated;
        self.heights = heights;
        self.bottoms = bottoms;
        self.total = acc + dp(8.0);
        // Messages that came at the bottom while scrolled up keep the view still.
        let newest = entries.last().map(|entry| entry.message.id.clone());
        let grew_at_bottom = newest != old_newest && old_newest.is_some();
        if grew_at_bottom && self.scroll.value(Instant::now()) > 1.0 {
            let added = self.total - old_total;
            if added > 0.0 {
                let below_old = old_newest
                    .as_ref()
                    .and_then(|id| entries.iter().rposition(|entry| &entry.message.id == id));
                let count = below_old.map_or(0, |at| entries.len() - 1 - at);
                // Only what was added below moves the view; older pages add above.
                let below: f32 = self.rows.iter().zip(&self.heights).filter(|(row, _)| matches!(row, Row::Message { index, .. } if below_old.is_some_and(|at| *index > at))).map(|(_, h)| h).sum();
                self.scroll.shift(below);
                self.new_below += count as u32;
            }
        }
        if self.scroll.value(Instant::now()) <= 1.0 {
            self.new_below = 0;
        }
        self.last_newest = newest;
        // Only a change to what is in view needs the frame drawn whole (an
        // older page above it, messages below it while scrolled up, do not).
        if self.in_view(self.scroll.value(now), visible) != before {
            self.dirty = true;
        }
        if profile() {
            eprintln!(
                "discord: chat rows {} laid out {} in {} us",
                self.rows.len(),
                self.layouts.len().saturating_sub(laid_before),
                started.elapsed().as_micros()
            );
        }
        // Old layouts of messages no longer here go.
        if self.layouts.len() > entries.len() + 200 {
            let ids: HashSet<&str> = entries
                .iter()
                .map(|entry| entry.message.id.as_str())
                .collect();
            self.layouts.retain(|id, _| ids.contains(id.as_str()));
        }
        let max = self.max_offset(width, height);
        if self.scroll.offset > max && !self.scroll.dragging() {
            self.scroll.offset = max;
        }
    }

    /// Asks for older messages when the top is near; returns the request to make.
    pub fn wants_older(
        &self,
        model: &Model,
        width: f32,
        height: f32,
        now: Instant,
    ) -> Option<String> {
        let history = model.histories.get(&self.chat)?;
        if !history.loaded || !history.more || history.loading_older || history.loading {
            return None;
        }
        if self.scroll.value(now) < self.max_offset(width, height) - LOAD_AHEAD {
            return None;
        }
        history
            .entries
            .first()
            .map(|entry| entry.message.id.clone())
    }

    // ---------------------------------------------------------------------
    // The composer

    fn composer_width(width: f32) -> f32 {
        width - dp(12.0) - dp(40.0) - dp(8.0) - dp(12.0) - dp(40.0) - dp(12.0) - dp(28.0)
    }

    /// The composer's text in lines (byte ranges), wrapped at its width.
    fn composer_lines(&self, width: f32) -> Vec<(usize, usize)> {
        let max = Self::composer_width(width);
        let mut lines = Vec::new();
        let mut start = 0;
        for paragraph in self.text.split('\n') {
            let base = start;
            let mut line_start = 0;
            let mut last_break = None;
            let mut x = 0.0;
            for (at, c) in paragraph.char_indices() {
                let w = text::measure(c.encode_utf8(&mut [0; 4]), Weight::Regular, theme::T_BODY);
                if c == ' ' {
                    last_break = Some(at + 1);
                }
                if x + w > max && at > line_start {
                    let cut = last_break.filter(|b| *b > line_start).unwrap_or(at);
                    lines.push((base + line_start, base + cut));
                    x = text::measure(&paragraph[cut..at], Weight::Regular, theme::T_BODY);
                    line_start = cut;
                    last_break = None;
                }
                x += w;
            }
            lines.push((base + line_start, base + paragraph.len()));
            start += paragraph.len() + 1;
        }
        lines
    }

    pub fn key(&mut self, key: KeyInput) -> bool {
        if !self.focused {
            return false;
        }
        self.dirty = true;
        match key {
            KeyInput::Character(c) => {
                let mut buffer = [0; 4];
                self.caret
                    .insert(&mut self.text, c.encode_utf8(&mut buffer))
            }
            KeyInput::Backspace => self.caret.backspace(&mut self.text),
            KeyInput::Enter => self.caret.insert(&mut self.text, "\n"),
        }
    }

    pub fn paste(&mut self, value: &str) {
        self.caret = crate::exocortex::editor::Cursor::at(self.text.len());
        self.caret.insert(&mut self.text, value);
    }

    fn take_send(&mut self) -> Option<Action> {
        let text = self.text.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        self.client_seq += 1;
        let client_id = format!("moon-{}-{}", super::model::now_ms(), self.client_seq);
        let reply = self.reply.take().map(|(id, _)| id);
        self.text.clear();
        self.caret = Default::default();
        Some(Action::Send {
            chat: self.chat.clone(),
            text,
            reply_to: reply,
            client_id,
        })
    }

    // ---------------------------------------------------------------------
    // Gestures

    fn message_at(
        &mut self,
        model: &Model,
        width: f32,
        height: f32,
        y: f32,
        now: Instant,
    ) -> Option<(Rc<Entry>, Rc<MessageLayout>, f32)> {
        let offset = self.scroll.value(now);
        let bottom = self.content_bottom(width, height);
        let _ = model;
        for (i, row) in self.rows.iter().enumerate() {
            let top = bottom - (self.bottoms[i] + self.heights[i] - offset);
            if y < top || y > top + self.heights[i] {
                continue;
            }
            if let Row::Message { index, header } = row {
                let entry = self.entries.get(*index)?.clone();
                let layout = self.layouts.get(&entry.message.id).map(|l| l.3.clone())?;
                let pad = if *header { dp(16.0) } else { dp(2.0) };
                return Some((entry, layout, top + pad));
            }
        }
        None
    }

    pub fn gesture(
        &mut self,
        gesture: Gesture,
        model: &Model,
        width: f32,
        height: f32,
        now: Instant,
    ) -> Vec<Action> {
        if !matches!(
            gesture,
            Gesture::DragStart {
                axis: Axis::Vertical,
                ..
            } | Gesture::Drag {
                axis: Axis::Vertical,
                ..
            } | Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            } | Gesture::Down { .. }
                | Gesture::Lift { .. }
        ) {
            self.dirty = true;
        }
        let mut actions = Vec::new();
        let max = self.max_offset(width, height);
        let composer_top = height - Self::bottom_inset(height) - self.composer_h(width);
        if let Some(sheet) = &self.sheet {
            if let Gesture::Tap { x, y } = gesture {
                let message = sheet.message.clone();
                actions.extend(self.sheet_tap(model, &message, x, y, width, height));
            } else if matches!(gesture, Gesture::Back) {
                self.sheet = None;
            }
            return actions;
        }
        match gesture {
            Gesture::Tap { x, y } => {
                if y < Self::content_top() {
                    if x < dp(56.0) {
                        actions.push(Action::Back);
                    } else if x > width - dp(56.0) {
                        actions.push(Action::Members(self.chat.clone()));
                    }
                    return actions;
                }
                if y >= composer_top - dp(8.0) {
                    actions.extend(self.composer_tap(model, x, y, width, height));
                    return actions;
                }
                // The bar: Jump to Present.
                let bottom = self.content_bottom(width, height);
                if self.bar(height, self.scroll.value(now).round()).is_some()
                    && y >= bottom - BAR_H
                    && y < bottom
                {
                    self.scroll.reset();
                    self.new_below = 0;
                    self.dirty = true;
                    return actions;
                }
                if self.focused {
                    self.focused = false;
                    actions.push(Action::HideKeyboard);
                }
                if let Some((entry, layout, top)) = self.message_at(model, width, height, y, now) {
                    let lx = x - theme::CONTENT_X;
                    let ly = y - top;
                    match layout::hit(&layout, lx, ly) {
                        Some(Hit::Link(url)) | Some(Hit::EmbedImage(url)) => {
                            actions.push(Action::OpenUrl(url))
                        }
                        Some(Hit::Spoiler(id)) => {
                            self.revealed.insert((entry.message.id.clone(), id));
                            actions.push(Action::Redraw);
                        }
                        Some(Hit::Picture(index)) => actions.push(Action::Viewer {
                            chat: self.chat.clone(),
                            message: entry.message.id.clone(),
                            index,
                        }),
                        Some(Hit::File(index)) => {
                            if let Some(Some(url)) = entry.ext.urls.get(index) {
                                actions.push(Action::OpenUrl(url.clone()));
                            }
                        }
                        Some(Hit::Reaction(emoji)) => {
                            let mine = entry
                                .message
                                .reactions
                                .iter()
                                .any(|r| r.emoji == emoji && r.mine);
                            actions.push(Action::React {
                                chat: self.chat.clone(),
                                message: entry.message.id.clone(),
                                emoji,
                                remove: mine,
                            });
                        }
                        Some(Hit::Thread(id)) => actions.push(Action::OpenChat(id)),
                        None => {}
                    }
                }
            }
            Gesture::LongPress { y, .. } => {
                if y > Self::content_top()
                    && y < composer_top
                    && let Some((entry, _, _)) = self.message_at(model, width, height, y, now)
                    && !entry.message.system
                {
                    self.sheet = Some(Sheet {
                        message: entry.message.id.clone(),
                        opened: now,
                    });
                    actions.push(Action::Haptic);
                }
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                y,
                start_y,
                ..
            } => {
                self.scroll.begin(-start_y, now);
                self.scroll.moved(-y, max, now);
                actions.push(Action::Redraw);
            }
            Gesture::Drag {
                axis: Axis::Vertical,
                y,
                ..
            } => {
                self.scroll.moved(-y, max, now);
                actions.push(Action::Redraw);
            }
            Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            } => {
                self.scroll.end(max, now);
                actions.push(Action::Animate);
            }
            Gesture::DragEnd {
                axis: Axis::Horizontal,
                x,
                vx,
                ..
            } if x > width * 0.35 || vx > 1200.0 => actions.push(Action::Back),
            Gesture::Back => actions.push(Action::Back),
            _ => {}
        }
        actions
    }

    fn composer_tap(
        &mut self,
        model: &Model,
        x: f32,
        _y: f32,
        width: f32,
        _height: f32,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if !model.settings.writes {
            actions.push(Action::Toast(
                "Sending is off. Turn it on in You › Allow sending.".into(),
            ));
            return actions;
        }
        if self.reply.is_some()
            && x > width - dp(48.0)
            && _y < _height - Self::bottom_inset(_height) - self.composer_h(width) + dp(34.0)
        {
            self.reply = None;
            actions.push(Action::Redraw);
            return actions;
        }
        if x > width - dp(60.0) && !self.text.trim().is_empty() {
            if let Some(send) = self.take_send() {
                actions.push(send);
            }
            return actions;
        }
        if x < dp(60.0) {
            actions.push(Action::Toast(
                "Attaching files is not available yet.".into(),
            ));
            return actions;
        }
        if !self.focused {
            self.focused = true;
            self.caret = crate::exocortex::editor::Cursor::at(self.text.len());
            actions.push(Action::ShowKeyboard);
        }
        actions.push(Action::Redraw);
        actions
    }

    fn sheet_rows(entry: &Entry, writes: bool) -> Vec<(&'static str, Icon, bool, bool)> {
        // (label, icon, enabled, destructive)
        let mut rows = vec![
            ("Reply", Icon::Undo, writes, false),
            (
                "Copy Text",
                Icon::Copy,
                !entry.message.text.is_empty(),
                false,
            ),
        ];
        if entry.message.mine {
            rows.push(("Edit Message", Icon::Pencil, writes, false));
            rows.push(("Delete Message", Icon::Trash, writes, true));
        }
        rows.push(("Copy Message ID", Icon::Clone, true, false));
        rows
    }

    fn sheet_geometry(rows: usize, width: f32, height: f32) -> (RectF, f32) {
        let row_h = dp(52.0);
        let h = dp(24.0) + dp(56.0) + dp(12.0) + rows as f32 * row_h + theme::SAFE_BOTTOM;
        (
            RectF {
                x: 0.0,
                y: height - h,
                w: width,
                h,
            },
            row_h,
        )
    }

    fn sheet_tap(
        &mut self,
        model: &Model,
        message: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Vec<Action> {
        let mut actions = vec![Action::Redraw];
        let Some(entry) = model
            .histories
            .get(&self.chat)
            .and_then(|h| h.find(message).map(|i| h.entries[i].clone()))
        else {
            self.sheet = None;
            return actions;
        };
        let rows = Self::sheet_rows(&entry, model.settings.writes);
        let (rect, row_h) = Self::sheet_geometry(rows.len(), width, height);
        if y < rect.y {
            self.sheet = None;
            return actions;
        }
        let reactions_y = rect.y + dp(24.0);
        if y >= reactions_y && y < reactions_y + dp(56.0) {
            let slot = (width - 2.0 * theme::MARGIN) / QUICK_REACTIONS.len() as f32;
            let index = ((x - theme::MARGIN) / slot).floor();
            if index >= 0.0 && (index as usize) < QUICK_REACTIONS.len() {
                if !model.settings.writes {
                    actions.push(Action::Toast(
                        "Sending is off. Turn it on in You › Allow sending.".into(),
                    ));
                } else {
                    let emoji = QUICK_REACTIONS[index as usize].to_owned();
                    let mine = entry
                        .message
                        .reactions
                        .iter()
                        .any(|r| r.emoji == emoji && r.mine);
                    actions.push(Action::React {
                        chat: self.chat.clone(),
                        message: message.to_owned(),
                        emoji,
                        remove: mine,
                    });
                }
                self.sheet = None;
            }
            return actions;
        }
        let list_y = reactions_y + dp(56.0) + dp(12.0);
        let index = ((y - list_y) / row_h).floor();
        if index < 0.0 || index as usize >= rows.len() {
            return actions;
        }
        let (label, _, enabled, _) = rows[index as usize];
        if !enabled {
            if label != "Copy Text" {
                actions.push(Action::Toast(
                    "Sending is off. Turn it on in You › Allow sending.".into(),
                ));
            }
            return actions;
        }
        self.sheet = None;
        match label {
            "Reply" => {
                self.reply = Some((entry.message.id.clone(), entry.message.author.name.clone()));
                self.focused = true;
                actions.push(Action::ShowKeyboard);
            }
            "Copy Text" => actions.push(Action::Copy(entry.message.text.clone())),
            "Edit Message" => {
                self.text = entry.message.text.clone();
                self.caret = crate::exocortex::editor::Cursor::at(self.text.len());
                self.focused = true;
                actions.push(Action::EditStart(entry.message.id.clone()));
                actions.push(Action::ShowKeyboard);
            }
            "Delete Message" => actions.push(Action::Delete {
                chat: self.chat.clone(),
                message: entry.message.id.clone(),
            }),
            "Copy Message ID" => actions.push(Action::Copy(entry.message.id.clone())),
            _ => {}
        }
        actions
    }

    pub fn tick(&mut self, model: &Model, width: f32, height: f32, now: Instant) -> bool {
        let max = self.max_offset(width, height);
        let moving = self.scroll.step(max, now);
        // Older rows laid out ahead of the view, a few a pass.
        let visible = self.visible(width, height);
        self.lay_out_ahead(
            model,
            width,
            height,
            self.scroll.value(now),
            visible * (1.0 + 2.0 * REACH_SCREENS),
            LAYOUTS_PER_TICK,
        );
        if self.scroll.value(now) <= 1.0 {
            self.new_below = 0;
        }
        moving
    }

    // ---------------------------------------------------------------------
    // Drawing

    /// The bar over the bottom of the messages while scrolled up (Discord's
    /// "viewing older messages, Jump to Present"), or new messages under the
    /// view: its words. It sits outside what scrolls, so a scroll frame
    /// never draws it.
    fn bar(&self, height: f32, offset: f32) -> Option<String> {
        if offset <= 1.0 {
            return None;
        }
        if self.new_below > 0 {
            Some(format!(
                "{} new message{}",
                self.new_below,
                if self.new_below == 1 { "" } else { "s" }
            ))
        } else if offset > height {
            Some("You're viewing older messages".to_owned())
        } else {
            None
        }
    }

    /// Where the rows of the messages tagged `tags` are now (their pictures
    /// came): rows sit on `bottom`, bands are cut to `top..limit`.
    fn row_bands(
        &self,
        tags: &HashSet<u64>,
        offset: f32,
        top: f32,
        bottom: f32,
        limit: f32,
    ) -> Vec<(i32, i32)> {
        let mut bands = Vec::new();
        for i in (0..self.rows.len()).rev() {
            let row_bottom = bottom - (self.bottoms[i] - offset);
            let row_top = row_bottom - self.heights[i];
            if row_bottom < top {
                break;
            }
            if row_top > limit {
                continue;
            }
            if let Row::Message { index, .. } = self.rows[i]
                && tags.contains(&ui::tag_of(&self.entries[index].message.id))
            {
                bands.push((
                    row_top.max(top).floor() as i32,
                    row_bottom.min(limit).ceil() as i32,
                ));
            }
        }
        bands
    }

    /// Draws the frame. While only the messages scrolled since the last
    /// one, they are moved and only what came into view (and rows whose
    /// pictures came) is drawn: the damage says so.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        model: &Model,
        media: &mut Media,
        now: Instant,
        full: bool,
    ) -> Option<Damage> {
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        let offset = self.scroll.value(now).round();
        let top = Self::content_top();
        let bottom = self.content_bottom(width, height).round();
        let bar = self.bar(height, offset);
        // What scrolls: down to the bar when it shows.
        let scrolls = if bar.is_some() {
            bottom - BAR_H
        } else {
            bottom
        };
        let (top_i, bottom_i, scrolls_i) = (top as i32, bottom as i32, scrolls as i32);
        let key = (canvas.width, canvas.height, bottom_i, scrolls_i);
        let loading = model
            .histories
            .get(&self.chat)
            .is_some_and(|h| h.loading_older);
        // Whatever comes into view is laid out exactly before it is drawn.
        let visible = self.visible(width, height);
        self.lay_out_ahead(model, width, height, offset, visible, usize::MAX);
        let ready = ui::take_ready(media);
        if ready.contains(&0) || ready.contains(&HEADER_TAG) {
            self.dirty = true;
        }
        if let Some(last) = &self.last_frame
            && !full
            && !self.dirty
            && self.sheet.is_none()
            && last.key == key
        {
            let dy = offset as i32 - last.offset;
            let header = last.loading != loading;
            let bar_changed = last.bar != bar;
            // Rows whose pictures came are drawn again where they are now.
            let pictures: Vec<(i32, i32)> = if ready.is_empty() {
                Vec::new()
            } else {
                self.row_bands(&ready, offset, top, bottom, scrolls)
            };
            if dy == 0 && pictures.is_empty() && !header && !bar_changed {
                return Some(Damage::default());
            }
            if dy.abs() < (scrolls_i - top_i) / 2 {
                let mut bands: Vec<(i32, i32)> = pictures;
                if dy != 0 {
                    canvas.move_rows(top_i, scrolls_i, dy);
                    bands.push(if dy > 0 {
                        (top_i, top_i + dy)
                    } else {
                        (scrolls_i + dy, scrolls_i)
                    });
                }
                bands.retain(|band| band.1 > band.0);
                bands.sort();
                let mut merged: Vec<(i32, i32)> = Vec::new();
                for band in bands {
                    match merged.last_mut() {
                        Some(last) if band.0 <= last.1 => last.1 = last.1.max(band.1),
                        _ => merged.push(band),
                    }
                }
                for &(y0, y1) in &merged {
                    canvas.set_clip(0, y0, canvas.width, y1);
                    canvas.fill_rect(0, y0, canvas.width, y1 - y0, theme::CHAT, 1.0);
                    self.draw_rows(canvas, model, media, now, offset, top, bottom, width);
                }
                if bar_changed && let Some(label) = &bar {
                    canvas.set_clip(0, scrolls_i, canvas.width, bottom_i);
                    self.draw_bar(canvas, label, scrolls, bottom, width);
                    merged.push((scrolls_i, bottom_i));
                }
                if header {
                    canvas.set_clip(0, 0, canvas.width, top_i);
                    self.draw_header(canvas, model, media, width, now);
                    merged.insert(0, (0, top_i));
                }
                canvas.reset_clip();
                if profile() {
                    eprintln!(
                        "discord: chat moved {dy} redrew {} rows",
                        merged.iter().map(|b| b.1 - b.0).sum::<i32>()
                    );
                }
                self.last_frame = Some(Frame {
                    offset: offset as i32,
                    key,
                    bar,
                    loading,
                });
                return Some(Damage {
                    moved: (dy != 0).then_some((top_i, scrolls_i, dy)),
                    rows: merged,
                });
            }
        }
        // The whole frame.
        if profile() {
            eprintln!("discord: chat whole");
        }
        ui::clear_misses();
        self.dirty = false;
        canvas.fill_rect(0, 0, width as i32, height as i32, theme::CHAT, 1.0);
        canvas.set_clip(0, top_i, canvas.width, scrolls_i);
        self.draw_rows(canvas, model, media, now, offset, top, bottom, width);
        canvas.reset_clip();
        if let Some(label) = &bar {
            self.draw_bar(canvas, label, scrolls, bottom, width);
        }
        self.draw_header(canvas, model, media, width, now);
        self.draw_composer(canvas, model, width, height, now);
        if self.sheet.is_some() {
            self.draw_sheet(canvas, model, media, width, height, now);
        }
        self.last_frame = Some(Frame {
            offset: offset as i32,
            key,
            bar,
            loading,
        });
        None
    }

    /// The bar over the bottom of the messages: its words, and Jump to Present.
    fn draw_bar(&self, canvas: &mut Canvas, label: &str, y: f32, bottom: f32, width: f32) {
        let fresh = self.new_below > 0;
        let fill = if fresh { theme::BLURPLE } else { theme::LIST };
        canvas.fill_rect(
            0,
            y as i32,
            width as i32,
            (bottom - y).ceil() as i32,
            fill,
            1.0,
        );
        if !fresh {
            canvas.fill_rect(0, y as i32, width as i32, 2, theme::HAIR, 1.0);
        }
        let cy = y + BAR_H / 2.0;
        let jump = "Jump to Present";
        let jump_w = text::measure(jump, Weight::SemiBold, theme::T_SMALL);
        let words = ui::fit(
            label,
            Weight::Medium,
            theme::T_SMALL,
            width - jump_w - theme::MARGIN * 3.0,
        );
        let color = if fresh { theme::WHITE } else { theme::MUTED };
        text::draw(
            canvas,
            &words,
            theme::MARGIN,
            text::centered_baseline(cy, Weight::Medium, theme::T_SMALL),
            Weight::Medium,
            theme::T_SMALL,
            color,
            1.0,
        );
        text::draw_right(
            canvas,
            jump,
            width - theme::MARGIN,
            text::centered_baseline(cy, Weight::SemiBold, theme::T_SMALL),
            Weight::SemiBold,
            theme::T_SMALL,
            if fresh { theme::WHITE } else { theme::LINK },
            1.0,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_rows(
        &self,
        canvas: &mut Canvas,
        model: &Model,
        media: &mut Media,
        now: Instant,
        offset: f32,
        top: f32,
        bottom: f32,
        width: f32,
    ) {
        let Some(history) = model.histories.get(&self.chat) else {
            return;
        };
        let (clip_top, clip_bottom) = (canvas.clip().1 as f32, canvas.clip().3 as f32);
        let entries = &self.entries;
        for i in (0..self.rows.len()).rev() {
            let row_bottom = bottom - (self.bottoms[i] - offset);
            let row_top = row_bottom - self.heights[i];
            if row_top > clip_bottom {
                continue;
            }
            if row_bottom < clip_top {
                break;
            }
            match &self.rows[i] {
                Row::Start => self.draw_start(canvas, model, row_top, width),
                Row::New => {
                    let cy = row_top + self.heights[i] / 2.0;
                    let tag_w = dp(34.0);
                    canvas.fill_rect(
                        theme::MARGIN as i32,
                        cy as i32,
                        (width - 2.0 * theme::MARGIN - tag_w) as i32,
                        2,
                        theme::RED,
                        1.0,
                    );
                    let tag = RectF {
                        x: width - theme::MARGIN - tag_w,
                        y: cy - dp(7.0),
                        w: tag_w,
                        h: dp(14.0),
                    };
                    canvas.fill_rrect(tag, dp(3.0), theme::RED, 1.0);
                    text::draw_centered(
                        canvas,
                        "NEW",
                        tag.x + tag.w / 2.0,
                        text::centered_baseline(cy, Weight::Bold, dp(10.0)),
                        Weight::Bold,
                        dp(10.0),
                        theme::WHITE,
                        1.0,
                    );
                }
                Row::Day(label) => {
                    let cy = row_top + self.heights[i] / 2.0;
                    let w = text::measure(label, Weight::SemiBold, theme::T_TINY);
                    let line = 0x3f4147;
                    let side = ((width - w) / 2.0 - dp(8.0) - theme::MARGIN) as i32;
                    canvas.fill_rect(theme::MARGIN as i32, cy as i32, side, 2, line, 1.0);
                    canvas.fill_rect(
                        ((width + w) / 2.0 + dp(8.0)) as i32,
                        cy as i32,
                        side,
                        2,
                        line,
                        1.0,
                    );
                    text::draw_centered(
                        canvas,
                        label,
                        width / 2.0,
                        text::centered_baseline(cy, Weight::SemiBold, theme::T_TINY),
                        Weight::SemiBold,
                        theme::T_TINY,
                        theme::MUTED,
                        1.0,
                    );
                }
                Row::Message { index, header } => {
                    let Some(entry) = entries.get(*index) else {
                        continue;
                    };
                    let Some((_, _, _, layout)) = self.layouts.get(&entry.message.id) else {
                        continue;
                    };
                    let pad = if *header { dp(16.0) } else { dp(2.0) };
                    let layout = layout.clone();
                    ui::set_tag(ui::tag_of(&entry.message.id));
                    self.draw_message(
                        canvas,
                        entry,
                        &layout,
                        row_top,
                        pad,
                        self.heights[i],
                        media,
                        now,
                        width,
                    );
                    ui::set_tag(0);
                }
            }
        }
        if !history.loaded {
            text::draw_centered(
                canvas,
                "Loading messages…",
                width / 2.0,
                (top + bottom) / 2.0,
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            );
        }
        if let Some(error) = &history.error
            && history.entries.is_empty()
        {
            text::draw_centered(
                canvas,
                &ui::fit(error, Weight::Medium, theme::T_SMALL, width - dp(40.0)),
                width / 2.0,
                (top + bottom) / 2.0 + dp(30.0),
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            );
        }
    }

    fn draw_start(&self, canvas: &mut Canvas, model: &Model, top: f32, width: f32) {
        let chat = model.chats.get(&self.chat);
        let title = chat.map(|c| c.title.clone()).unwrap_or_default();
        let dm = chat.is_some_and(|c| c.kind == "dm" || c.kind == "group");
        let x = theme::MARGIN;
        let circle = dp(64.0);
        canvas.fill_circle(
            x + circle / 2.0,
            top + dp(16.0) + circle / 2.0,
            circle / 2.0,
            0x41434a,
            1.0,
        );
        if dm {
            icons::draw(
                canvas,
                Icon::Chat,
                x + circle / 2.0,
                top + dp(16.0) + circle / 2.0,
                dp(30.0),
                theme::HEADER,
                1.0,
            );
        } else {
            text::draw_centered(
                canvas,
                "#",
                x + circle / 2.0,
                text::centered_baseline(top + dp(16.0) + circle / 2.0, Weight::Medium, dp(40.0)),
                Weight::Medium,
                dp(40.0),
                theme::HEADER,
                1.0,
            );
        }
        let heading = if dm {
            title.clone()
        } else {
            format!("Welcome to #{title}!")
        };
        let heading = ui::fit(&heading, Weight::Bold, theme::T_H2, width - 2.0 * x);
        text::draw(
            canvas,
            &heading,
            x,
            top + dp(16.0) + circle + dp(30.0),
            Weight::Bold,
            theme::T_H2,
            theme::HEADER,
            1.0,
        );
        let note = if dm {
            format!("This is the beginning of your direct message history with {title}.")
        } else {
            format!("This is the start of the #{title} channel.")
        };
        let note = ui::fit(&note, Weight::Regular, theme::T_SMALL, width - 2.0 * x);
        text::draw(
            canvas,
            &note,
            x,
            top + dp(16.0) + circle + dp(54.0),
            Weight::Regular,
            theme::T_SMALL,
            theme::MUTED,
            1.0,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_message(
        &self,
        canvas: &mut Canvas,
        entry: &Entry,
        layout: &MessageLayout,
        row_top: f32,
        pad: f32,
        row_h: f32,
        media: &mut Media,
        now: Instant,
        width: f32,
    ) {
        let top = row_top + pad;
        let message = &entry.message;
        let mut background = theme::CHAT;
        if layout.mentioned {
            background = theme::MENTION_ROW;
            canvas.fill_rect(
                0,
                (row_top + pad - dp(2.0)) as i32,
                width as i32,
                (row_h - pad + dp(2.0)) as i32,
                theme::MENTION_ROW,
                1.0,
            );
            canvas.fill_rect(
                0,
                (row_top + pad - dp(2.0)) as i32,
                dp(2.0) as i32,
                (row_h - pad + dp(2.0)) as i32,
                theme::MENTION_BAR,
                1.0,
            );
        }
        if self
            .sheet
            .as_ref()
            .is_some_and(|sheet| sheet.message == message.id)
        {
            background = theme::HOVER;
            canvas.fill_rect(
                0,
                (row_top + pad - dp(2.0)) as i32,
                width as i32,
                (row_h - pad + dp(2.0)) as i32,
                theme::HOVER,
                1.0,
            );
        }
        let x = theme::CONTENT_X;
        let mut y = top;
        if let Some((author, summary, color)) = &layout.reply {
            // The spine from the avatar column to the replied message.
            let sx = theme::GUTTER + theme::AVATAR / 2.0;
            let sy = y + layout::REPLY_H / 2.0;
            canvas.fill_rect(
                sx as i32,
                sy as i32,
                (x - sx - dp(6.0)) as i32,
                dp(2.0) as i32,
                0x4e5058,
                1.0,
            );
            canvas.fill_rect(
                sx as i32,
                sy as i32,
                dp(2.0) as i32,
                (layout::REPLY_H / 2.0 + dp(6.0)) as i32,
                0x4e5058,
                1.0,
            );
            let name = format!("@{author}");
            let nw = text::draw(
                canvas,
                &ui::fit(&name, Weight::SemiBold, theme::T_SMALL, dp(150.0)),
                x,
                text::centered_baseline(sy, Weight::SemiBold, theme::T_SMALL),
                Weight::SemiBold,
                theme::T_SMALL,
                *color,
                1.0,
            );
            let rest = ui::fit(
                summary,
                Weight::Regular,
                theme::T_SMALL,
                width - x - nw - dp(8.0) - theme::MARGIN,
            );
            text::draw(
                canvas,
                &rest,
                x + nw + dp(6.0),
                text::centered_baseline(sy, Weight::Regular, theme::T_SMALL),
                Weight::Regular,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            );
            y += layout::REPLY_H + dp(4.0);
        }
        if layout.system {
            let icon_y = y + theme::L_BODY / 2.0;
            let kind = entry.ext.kind;
            let (icon, color) = match kind {
                7 => (Icon::ArrowUp, theme::GREEN),
                6 => (Icon::Pin, theme::MUTED),
                18 | 21 => (Icon::MessageSquare, theme::MUTED),
                _ => (Icon::InfoCircle, theme::MUTED),
            };
            icons::draw(
                canvas,
                icon,
                theme::GUTTER + theme::AVATAR / 2.0,
                icon_y,
                dp(18.0),
                color,
                1.0,
            );
        } else if layout.header {
            ui::avatar(
                canvas,
                media,
                message.author.avatar.as_ref(),
                theme::GUTTER,
                y,
                theme::AVATAR,
                0x41434a,
                now,
            );
            let baseline = text::centered_baseline(
                y + layout::HEADER_LINE / 2.0,
                Weight::SemiBold,
                theme::T_BODY,
            );
            let name = ui::fit(
                &layout.name,
                Weight::SemiBold,
                theme::T_BODY,
                width - x - dp(150.0),
            );
            let w = text::draw(
                canvas,
                &name,
                x,
                baseline,
                Weight::SemiBold,
                theme::T_BODY,
                layout.name_color,
                1.0,
            );
            let mut tx = x + w + dp(6.0);
            if layout.bot {
                let tag = RectF {
                    x: tx,
                    y: baseline - dp(13.0),
                    w: dp(30.0),
                    h: dp(16.0),
                };
                canvas.fill_rrect(tag, dp(3.0), theme::BLURPLE, 1.0);
                text::draw_centered(
                    canvas,
                    "APP",
                    tag.x + tag.w / 2.0,
                    text::centered_baseline(tag.y + tag.h / 2.0, Weight::Bold, dp(10.0)),
                    Weight::Bold,
                    dp(10.0),
                    theme::WHITE,
                    1.0,
                );
                tx += tag.w + dp(6.0);
            }
            text::draw(
                canvas,
                &layout.time,
                tx,
                baseline,
                Weight::Regular,
                theme::T_TINY,
                theme::MUTED,
                1.0,
            );
        }
        let revealed = |id: u16| self.revealed.contains(&(message.id.clone(), id));
        let mut env = DrawEnv {
            media,
            now,
            background,
            revealed: &revealed,
        };
        let body_top = top + layout.body_y;
        let alpha_pending = message.status.as_deref() == Some("pending");
        layout::draw_flow(canvas, &layout.body, x, body_top, &mut env);
        if alpha_pending {
            // A message on its way: dimmed, as Discord shows it.
            canvas.fill_rect(
                x as i32,
                body_top as i32,
                (width - x) as i32,
                layout.body.height as i32,
                background,
                0.45,
            );
        }
        if let Some((ex, baseline)) = layout.edited {
            text::draw(
                canvas,
                "(edited)",
                x + ex,
                body_top + baseline,
                Weight::Regular,
                theme::T_TINY,
                theme::FAINT,
                1.0,
            );
        }
        layout::draw_pieces(canvas, layout, x, top, &mut env);
        if message.status.as_deref() == Some("failed") {
            text::draw(
                canvas,
                "Failed to send",
                x,
                top + layout.height + dp(14.0),
                Weight::Medium,
                theme::T_TINY,
                theme::RED,
                1.0,
            );
        }
    }

    fn draw_header(
        &self,
        canvas: &mut Canvas,
        model: &Model,
        media: &mut Media,
        width: f32,
        now: Instant,
    ) {
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
        let chat = model.chats.get(&self.chat);
        let title = chat.map(|c| c.title.clone()).unwrap_or_default();
        let kind = chat.map(|c| c.kind.as_str()).unwrap_or("channel");
        let ext = model.ext(&self.chat);
        let mut x = dp(52.0);
        match kind {
            "dm" | "group" => {
                let size = dp(28.0);
                ui::set_tag(HEADER_TAG);
                ui::avatar(
                    canvas,
                    media,
                    chat.and_then(|c| c.avatar.as_ref()),
                    x,
                    cy - size / 2.0,
                    size,
                    0x41434a,
                    now,
                );
                ui::set_tag(0);
                x += size + dp(10.0);
            }
            _ => {
                channel_icon(
                    canvas,
                    kind,
                    ext.map_or(0, |e| e.channel_type),
                    x + dp(10.0),
                    cy,
                    dp(20.0),
                    theme::MUTED,
                );
                x += dp(26.0);
            }
        }
        // An older page on its way says so here, outside what scrolls.
        let loading = model
            .histories
            .get(&self.chat)
            .is_some_and(|h| h.loading_older);
        let note = "Loading…";
        let room = if loading {
            text::measure(note, Weight::Medium, theme::T_SMALL) + dp(12.0)
        } else {
            0.0
        };
        let name = ui::fit(
            &title,
            Weight::Bold,
            theme::T_BODY,
            width - x - dp(70.0) - room,
        );
        if loading {
            text::draw_right(
                canvas,
                note,
                width - dp(52.0),
                text::centered_baseline(cy, Weight::Medium, theme::T_SMALL),
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            );
        }
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
        ui::glyph(
            canvas,
            ui::Glyph::Members,
            width - dp(28.0),
            cy,
            dp(24.0),
            theme::INTERACTIVE,
            1.0,
        );
    }

    fn draw_composer(
        &self,
        canvas: &mut Canvas,
        model: &Model,
        width: f32,
        height: f32,
        now: Instant,
    ) {
        let h = self.composer_h(width);
        let bottom = height - Self::bottom_inset(height);
        let top = bottom - h;
        canvas.fill_rect(
            0,
            (top - dp(18.0)) as i32,
            width as i32,
            (height - top + dp(18.0)) as i32,
            theme::CHAT,
            1.0,
        );
        // Who is typing, just above.
        let typing = model.typing_in(&self.chat);
        if !typing.is_empty() {
            let names: Vec<&str> = typing.iter().map(|t| t.name.as_str()).collect();
            let label = match names.len() {
                1 => format!("{} is typing…", names[0]),
                2 => format!("{} and {} are typing…", names[0], names[1]),
                _ => "Several people are typing…".to_owned(),
            };
            let phase = (now.elapsed().as_millis() / 400 % 3) as usize;
            let _ = phase;
            for dot in 0..3 {
                let alpha = 0.4
                    + 0.6 * ((super::model::now_ms() / 300 + dot as i64) % 3 == 0) as i32 as f32;
                canvas.fill_circle(
                    theme::MARGIN + dp(4.0) + dot as f32 * dp(7.0),
                    top - dp(8.0),
                    dp(2.5),
                    theme::HEADER,
                    alpha,
                );
            }
            let label = ui::fit(&label, Weight::Medium, theme::T_TINY, width - dp(60.0));
            text::draw(
                canvas,
                &label,
                theme::MARGIN + dp(26.0),
                top - dp(3.0),
                Weight::Medium,
                theme::T_TINY,
                theme::INTERACTIVE,
                1.0,
            );
        }
        let mut y = top + dp(8.0);
        if let Some((_, name)) = &self.reply {
            let bar = RectF {
                x: dp(12.0),
                y: y - dp(4.0),
                w: width - dp(24.0),
                h: dp(30.0),
            };
            canvas.fill_rrect(bar, dp(8.0), theme::LIST, 1.0);
            let label = format!("Replying to {name}");
            text::draw(
                canvas,
                &ui::fit(&label, Weight::Medium, theme::T_SMALL, bar.w - dp(50.0)),
                bar.x + dp(12.0),
                text::centered_baseline(bar.y + bar.h / 2.0, Weight::Medium, theme::T_SMALL),
                Weight::Medium,
                theme::T_SMALL,
                theme::INTERACTIVE,
                1.0,
            );
            icons::draw(
                canvas,
                Icon::X,
                bar.x + bar.w - dp(18.0),
                bar.y + bar.h / 2.0,
                dp(16.0),
                theme::INTERACTIVE,
                1.0,
            );
            y += dp(34.0);
        }
        let lines = self.composer_lines(width);
        let shown = lines.len().clamp(1, COMPOSER_LINES);
        let field_h = dp(40.0) + (shown as f32 - 1.0) * theme::L_BODY;
        let writes = model.settings.writes;
        // [+]
        let plus_c = (dp(12.0) + dp(20.0), y + field_h - dp(20.0));
        canvas.fill_circle(plus_c.0, plus_c.1, dp(20.0), theme::FIELD, 1.0);
        icons::draw(
            canvas,
            Icon::Plus,
            plus_c.0,
            plus_c.1,
            dp(22.0),
            if writes {
                theme::INTERACTIVE
            } else {
                theme::DIM
            },
            1.0,
        );
        let field = RectF {
            x: dp(12.0) + dp(40.0) + dp(8.0),
            y,
            w: width - dp(12.0) - dp(40.0) - dp(8.0) - dp(12.0) - dp(48.0),
            h: field_h,
        };
        canvas.fill_rrect(field, dp(20.0), theme::FIELD, 1.0);
        let tx = field.x + dp(14.0);
        if self.text.is_empty() {
            let chat = model.chats.get(&self.chat);
            let placeholder = match chat.map(|c| c.kind.as_str()) {
                _ if !writes => "Sending is off".to_owned(),
                Some("dm") => format!("Message @{}", chat.map(|c| c.title.as_str()).unwrap_or("")),
                Some("group") => {
                    format!("Message {}", chat.map(|c| c.title.as_str()).unwrap_or(""))
                }
                Some("thread") => {
                    format!("Message {}", chat.map(|c| c.title.as_str()).unwrap_or(""))
                }
                _ => format!("Message #{}", chat.map(|c| c.title.as_str()).unwrap_or("")),
            };
            let placeholder = ui::fit(
                &placeholder,
                Weight::Regular,
                theme::T_BODY,
                field.w - dp(28.0),
            );
            text::draw(
                canvas,
                &placeholder,
                tx,
                text::centered_baseline(field.y + dp(20.0), Weight::Regular, theme::T_BODY),
                Weight::Regular,
                theme::T_BODY,
                theme::FAINT,
                1.0,
            );
            if self.focused {
                canvas.fill_rect(
                    tx as i32,
                    (field.y + dp(10.0)) as i32,
                    4,
                    dp(20.0) as i32,
                    theme::HEADER,
                    1.0,
                );
            }
        } else {
            let first = lines.len().saturating_sub(COMPOSER_LINES);
            for (n, (a, b)) in lines[first..].iter().enumerate() {
                let line_cy = field.y + dp(20.0) + n as f32 * theme::L_BODY;
                text::draw(
                    canvas,
                    &self.text[*a..*b],
                    tx,
                    text::centered_baseline(line_cy, Weight::Regular, theme::T_BODY),
                    Weight::Regular,
                    theme::T_BODY,
                    theme::TEXT,
                    1.0,
                );
            }
            if self.focused {
                let (a, b) = *lines.last().unwrap();
                let cx =
                    tx + text::measure(&self.text[a..b], Weight::Regular, theme::T_BODY) + dp(1.0);
                let line_cy = field.y + dp(20.0) + (shown as f32 - 1.0) * theme::L_BODY;
                canvas.fill_rect(
                    cx as i32,
                    (line_cy - dp(10.0)) as i32,
                    4,
                    dp(20.0) as i32,
                    theme::HEADER,
                    1.0,
                );
            }
        }
        // Send, or the emoji face when empty.
        let send_c = (width - dp(12.0) - dp(20.0), y + field_h - dp(20.0));
        if !self.text.trim().is_empty() && writes {
            canvas.fill_circle(send_c.0, send_c.1, dp(20.0), theme::BLURPLE, 1.0);
            ui::glyph(
                canvas,
                ui::Glyph::Send,
                send_c.0 + dp(1.0),
                send_c.1,
                dp(20.0),
                theme::WHITE,
                1.0,
            );
        } else {
            canvas.fill_circle(send_c.0, send_c.1, dp(20.0), theme::FIELD, 1.0);
            icons::draw(
                canvas,
                Icon::Mic,
                send_c.0,
                send_c.1,
                dp(20.0),
                if writes {
                    theme::INTERACTIVE
                } else {
                    theme::DIM
                },
                1.0,
            );
        }
    }

    fn draw_sheet(
        &self,
        canvas: &mut Canvas,
        model: &Model,
        media: &mut Media,
        width: f32,
        height: f32,
        now: Instant,
    ) {
        let Some(sheet) = &self.sheet else { return };
        let Some(entry) = model
            .histories
            .get(&self.chat)
            .and_then(|h| h.find(&sheet.message).map(|i| h.entries[i].clone()))
        else {
            return;
        };
        let writes = model.settings.writes;
        let rows = Self::sheet_rows(&entry, writes);
        let (rect, row_h) = Self::sheet_geometry(rows.len(), width, height);
        let _ = media;
        let _ = now;
        canvas.fill_rect(0, 0, width as i32, height as i32, 0x000000, 0.55);
        canvas.fill_rrect(
            RectF {
                h: rect.h + dp(20.0),
                ..rect
            },
            dp(16.0),
            theme::LIST,
            1.0,
        );
        canvas.fill_rrect(
            RectF {
                x: width / 2.0 - dp(18.0),
                y: rect.y + dp(8.0),
                w: dp(36.0),
                h: dp(4.0),
            },
            dp(2.0),
            theme::DIM,
            1.0,
        );
        let reactions_y = rect.y + dp(24.0);
        let slot = (width - 2.0 * theme::MARGIN) / QUICK_REACTIONS.len() as f32;
        for (i, emoji) in QUICK_REACTIONS.iter().enumerate() {
            let cx = theme::MARGIN + slot * (i as f32 + 0.5);
            let cy = reactions_y + dp(28.0);
            canvas.fill_circle(
                cx,
                cy,
                dp(24.0),
                theme::FIELD,
                if writes { 1.0 } else { 0.5 },
            );
            let size = dp(24.0);
            text::draw_centered(
                canvas,
                &ui::emoji_presentation(emoji),
                cx,
                text::centered_baseline(cy, Weight::Regular, size),
                Weight::Regular,
                size,
                theme::TEXT,
                if writes { 1.0 } else { 0.4 },
            );
        }
        let list_y = reactions_y + dp(56.0) + dp(12.0);
        let card = RectF {
            x: dp(12.0),
            y: list_y,
            w: width - dp(24.0),
            h: rows.len() as f32 * row_h,
        };
        canvas.fill_rrect(card, dp(12.0), theme::FIELD, 1.0);
        for (i, (label, icon, enabled, destructive)) in rows.iter().enumerate() {
            let cy = list_y + row_h * (i as f32 + 0.5);
            let color = if !enabled {
                theme::DIM
            } else if *destructive {
                theme::RED
            } else {
                theme::HEADER
            };
            icons::draw(canvas, *icon, card.x + dp(24.0), cy, dp(20.0), color, 1.0);
            text::draw(
                canvas,
                label,
                card.x + dp(52.0),
                text::centered_baseline(cy, Weight::Medium, theme::T_BODY),
                Weight::Medium,
                theme::T_BODY,
                color,
                1.0,
            );
            if i + 1 < rows.len() {
                canvas.fill_rect(
                    (card.x + dp(52.0)) as i32,
                    (list_y + row_h * (i as f32 + 1.0)) as i32,
                    (card.w - dp(52.0)) as i32,
                    2,
                    theme::DIVIDER,
                    1.0,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::proto::{Author, Message};

    fn chat_with_messages(writes: bool) -> (Model, ChatView) {
        let mut model = Model::default();
        model.settings.writes = writes;
        let messages = (0..12)
            .map(|n| Message {
                id: format!("m{n}"),
                chat: "c".into(),
                ts: 1_792_000_000_000 + n * 600_000,
                author: Author {
                    id: format!("u{}", n % 2),
                    name: format!("Person {}", n % 2),
                    avatar: None,
                },
                text: format!("message number {n}"),
                format: "discord".into(),
                ..Message::default()
            })
            .collect();
        model.page("c", messages, false, false);
        let mut view = ChatView::new("c".into());
        view.sync(&model, 1080.0, 2216.0);
        (model, view)
    }

    #[test]
    fn a_long_press_opens_the_sheet_and_its_reactions_react() {
        let (model, mut view) = chat_with_messages(true);
        let now = Instant::now();
        // The newest message sits just above the composer.
        let y = view.content_bottom(1080.0, 2216.0) - dp(20.0);
        let actions = view.gesture(
            Gesture::LongPress { x: 400.0, y },
            &model,
            1080.0,
            2216.0,
            now,
        );
        assert!(matches!(actions.as_slice(), [Action::Haptic]));
        assert_eq!(view.sheet.as_ref().map(|s| s.message.as_str()), Some("m11"));
        // The quick reactions row: the first is 👍.
        let entry = model.histories["c"].entries[11].clone();
        let (rect, _) =
            ChatView::sheet_geometry(ChatView::sheet_rows(&entry, true).len(), 1080.0, 2216.0);
        let tap = Gesture::Tap {
            x: theme::MARGIN + dp(20.0),
            y: rect.y + dp(24.0) + dp(28.0),
        };
        let actions = view.gesture(tap, &model, 1080.0, 2216.0, now);
        assert!(actions.iter().any(|a| matches!(a, Action::React { message, emoji, remove: false, .. } if message == "m11" && emoji == "👍")));
        assert!(view.sheet.is_none());
    }

    #[test]
    fn with_writes_off_the_sheet_only_copies() {
        let (model, mut view) = chat_with_messages(false);
        let now = Instant::now();
        let y = view.content_bottom(1080.0, 2216.0) - dp(20.0);
        view.gesture(
            Gesture::LongPress { x: 400.0, y },
            &model,
            1080.0,
            2216.0,
            now,
        );
        let entry = model.histories["c"].entries[11].clone();
        let rows = ChatView::sheet_rows(&entry, false);
        assert!(
            rows.iter()
                .any(|(label, _, enabled, _)| *label == "Copy Text" && *enabled)
        );
        assert!(
            rows.iter()
                .any(|(label, _, enabled, _)| *label == "Reply" && !*enabled)
        );
        let (rect, row_h) = ChatView::sheet_geometry(rows.len(), 1080.0, 2216.0);
        let list_y = rect.y + dp(24.0) + dp(56.0) + dp(12.0);
        // Reply: refused with the reason, the sheet stays.
        let actions = view.gesture(
            Gesture::Tap {
                x: 400.0,
                y: list_y + row_h / 2.0,
            },
            &model,
            1080.0,
            2216.0,
            now,
        );
        assert!(actions.iter().any(|a| matches!(a, Action::Toast(_))));
        assert!(view.sheet.is_some());
        // Copy Text copies and closes.
        let actions = view.gesture(
            Gesture::Tap {
                x: 400.0,
                y: list_y + row_h * 1.5,
            },
            &model,
            1080.0,
            2216.0,
            now,
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::Copy(text) if text == "message number 11"))
        );
        assert!(view.sheet.is_none());
    }

    #[test]
    fn messages_group_by_author_within_seven_minutes() {
        let mut model = Model::default();
        let at = |minutes: i64| 1_792_000_000_000 + minutes * 60_000;
        let message = |id: &str, author: &str, ts: i64| Message {
            id: id.into(),
            chat: "c".into(),
            ts,
            author: Author {
                id: author.into(),
                name: author.into(),
                avatar: None,
            },
            text: "hi".into(),
            format: "discord".into(),
            ..Message::default()
        };
        // a, a (2 min later), a (12 min later: a new group), b.
        model.page(
            "c",
            vec![
                message("1", "a", at(0)),
                message("2", "a", at(2)),
                message("3", "a", at(14)),
                message("4", "b", at(15)),
            ],
            false,
            false,
        );
        let mut view = ChatView::new("c".into());
        view.sync(&model, 1080.0, 2216.0);
        let headers: Vec<bool> = view
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Message { header, .. } => Some(*header),
                _ => None,
            })
            .collect();
        assert_eq!(headers, [true, false, true, true]);
        assert!(matches!(view.rows[0], Row::Start));
        assert!(matches!(view.rows[1], Row::Day(_)));
    }
}
