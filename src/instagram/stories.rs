//! The story viewer: one reel after another (the tray's order), each item
//! full screen with Instagram's progress segments, the owner's header and
//! the reply bar. Tap right for the next item, left for the one before,
//! hold to pause, swipe sideways for the next or previous person, down to
//! close. An item's clock starts when its picture is on screen; one shown
//! for a second is reported watched (`instagram.story.seen`: the bridge
//! keeps it locally, and sends Instagram's receipt only with writes on).
//! Videos show their poster for their length.

use std::time::{Duration, Instant};

use serde_json::json;

use super::app::Action;
use super::model::{Reel, StoryItem};
use super::paint::{self, Icon, Paint, Ring, BG, BODY_BOLD, DP, SMALL, STATUS, TEXT, TINY};
use super::store::{Ask, Store};
use crate::exocortex::gesture::{Axis, Gesture};
use crate::gfx::{Canvas, RectF};
use crate::social::media::Fit;

const BAR_Y: f32 = STATUS + 22.0;
const BAR_H: f32 = 7.0;
const PHOTO: Duration = Duration::from_secs(5);
const SEEN_AFTER: Duration = Duration::from_secs(1);
const PROGRESS_FRAME: Duration = Duration::from_millis(16);

pub struct StoryViewer {
    ids: Vec<String>,
    reel: usize,
    item: usize,
    /// When the shown item's picture appeared (its clock), less pauses.
    ready_at: Option<Instant>,
    paused_at: Option<Instant>,
    reported: bool,
    /// A vertical drag down (closing) or sideways (another person).
    drag: Option<(Axis, f32)>,
    pub closed: bool,
    asked: Vec<String>,
    /// When the last frame was drawn (the progress moves on from there).
    pub drawn_at: Option<Instant>,
}

impl StoryViewer {
    pub fn new(ids: Vec<String>, start: usize, _now: Instant) -> Self {
        Self {
            reel: start.min(ids.len().saturating_sub(1)),
            ids,
            item: usize::MAX,
            ready_at: None,
            paused_at: None,
            reported: false,
            drag: None,
            closed: false,
            asked: Vec::new(),
            drawn_at: None,
        }
    }

    fn current<'a>(&self, store: &'a Store) -> Option<&'a Reel> {
        self.ids.get(self.reel).and_then(|id| store.reels.get(id))
    }

    /// The reels shown now and next, when not loaded yet.
    pub fn load(&mut self, store: &Store) -> Action {
        let wanted: Vec<String> = self
            .ids
            .iter()
            .skip(self.reel)
            .take(3)
            .filter(|id| !store.reels.contains_key(*id) && !self.asked.contains(id))
            .cloned()
            .collect();
        if wanted.is_empty() {
            return Action::None;
        }
        self.asked.extend(wanted.iter().cloned());
        Action::request("instagram.reels", json!({"ids": wanted}), Ask::Reels)
    }

    fn item<'a>(&self, store: &'a Store) -> Option<&'a StoryItem> {
        self.current(store).and_then(|reel| reel.items.get(self.item))
    }

    /// The reel's first unseen item (or its first).
    fn start_item(&mut self, store: &Store) {
        if let Some(reel) = self.current(store) {
            self.item = reel
                .items
                .iter()
                .position(|item| item.ts > reel.seen_ts)
                .unwrap_or(0);
            self.ready_at = None;
            self.reported = false;
        }
    }

    fn duration(item: &StoryItem) -> Duration {
        if item.duration_ms > 0 {
            Duration::from_millis(item.duration_ms.min(60_000))
        } else {
            PHOTO
        }
    }

    fn elapsed(&self, now: Instant) -> Duration {
        match (self.ready_at, self.paused_at) {
            (Some(at), Some(paused)) => paused.saturating_duration_since(at),
            (Some(at), None) => now.saturating_duration_since(at),
            _ => Duration::ZERO,
        }
    }

    fn next(&mut self, store: &Store) -> Action {
        let count = self.current(store).map_or(0, |reel| reel.items.len());
        if self.item + 1 < count {
            self.item += 1;
            self.ready_at = None;
            self.reported = false;
            return Action::Redraw;
        }
        self.next_reel(store)
    }

    fn next_reel(&mut self, store: &Store) -> Action {
        if self.reel + 1 >= self.ids.len() {
            self.closed = true;
            return Action::None;
        }
        self.reel += 1;
        self.item = usize::MAX;
        self.start_item(store);
        self.load(store)
    }

    fn previous(&mut self, store: &Store) {
        if self.item > 0 && self.item != usize::MAX {
            self.item -= 1;
            self.ready_at = None;
            self.reported = false;
        } else if self.reel > 0 {
            self.reel -= 1;
            let count = self.current(store).map_or(0, |reel| reel.items.len());
            self.item = count.saturating_sub(1);
            self.ready_at = None;
            self.reported = false;
        } else {
            self.ready_at = None;
        }
    }

    pub fn tick(&mut self, store: &Store, now: Instant) -> Action {
        if self.closed {
            return Action::None;
        }
        if self.item == usize::MAX {
            if self.current(store).is_some() {
                self.start_item(store);
                return Action::Redraw;
            }
            if self.ids.get(self.reel).is_some_and(|id| self.asked.contains(id)) && !store.reels.is_empty() {
                // Asked and not there: the reel is gone; on to the next.
                if self.asked.iter().all(|id| store.reels.contains_key(id) || !self.ids[self.reel..].iter().take(3).any(|x| x == id)) {
                    return self.next_reel(store);
                }
            }
            return Action::None;
        }
        let Some(item) = self.item(store) else {
            return Action::None;
        };
        if self.paused_at.is_some() || self.ready_at.is_none() {
            return Action::None;
        }
        let elapsed = self.elapsed(now);
        let mut actions = Vec::new();
        if !self.reported && elapsed >= SEEN_AFTER {
            self.reported = true;
            if let Some(reel) = self.current(store) {
                actions.push(Action::request(
                    "instagram.story.seen",
                    json!({"reel": reel.id, "item": item.id, "ts": item.ts}),
                    Ask::Other,
                ));
            }
        }
        if elapsed >= Self::duration(item) {
            actions.push(self.next(store));
        }
        Action::Many(actions)
    }

    /// When the next frame is due (the progress moves), or None.
    pub fn next_frame(&self) -> Option<Instant> {
        if self.closed || self.paused_at.is_some() || self.ready_at.is_none() {
            return None;
        }
        Some(self.drawn_at.map_or_else(Instant::now, |at| at + PROGRESS_FRAME))
    }

    pub fn gesture(&mut self, store: &Store, gesture: &Gesture, width: f32, height: f32, now: Instant) -> Action {
        match *gesture {
            Gesture::Tap { x, y } => {
                if y < BAR_Y + 160.0 && x > width - 160.0 {
                    self.closed = true;
                    return Action::None;
                }
                if y > height - 200.0 {
                    return Action::Toast("Replies are off: writes are off.".into());
                }
                if x < width / 3.0 {
                    self.previous(store);
                    Action::Redraw
                } else {
                    self.next(store)
                }
            }
            Gesture::Down { .. } | Gesture::LongPress { .. } => {
                if self.paused_at.is_none() {
                    self.paused_at = Some(now);
                }
                Action::Redraw
            }
            Gesture::Lift { .. } => Action::None,
            Gesture::LongPressEnd { .. } | Gesture::Cancel => {
                self.resume(now);
                Action::Redraw
            }
            Gesture::DragStart { axis, x, y, start_x, start_y, .. } => {
                self.resume(now);
                let d = if axis == Axis::Vertical { y - start_y } else { x - start_x };
                self.drag = Some((axis, d));
                Action::Redraw
            }
            Gesture::Drag { axis, .. } => {
                if let Some((a, _)) = self.drag
                    && a == axis
                {
                    self.drag = Some((a, 0.0));
                }
                Action::None
            }
            Gesture::DragEnd { axis, x, y, vx, vy } => {
                self.drag = None;
                match axis {
                    Axis::Vertical if vy > 600.0 || y > height * 0.45 => {
                        self.closed = true;
                        Action::None
                    }
                    Axis::Horizontal if vx < -500.0 || x < width * 0.35 => self.next_reel(store),
                    Axis::Horizontal if vx > 500.0 || x > width * 0.65 => {
                        if self.reel > 0 {
                            self.reel -= 1;
                            self.item = usize::MAX;
                            self.start_item(store);
                        }
                        self.load(store)
                    }
                    _ => Action::Redraw,
                }
            }
            Gesture::Back => {
                self.closed = true;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn resume(&mut self, now: Instant) {
        if let (Some(paused), Some(at)) = (self.paused_at.take(), self.ready_at) {
            self.ready_at = Some(at + now.saturating_duration_since(paused));
        }
    }

    fn media_rect(width: f32) -> RectF {
        RectF::new(0.0, STATUS, width, width * 16.0 / 9.0)
    }

    pub fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, width: f32, height: f32) {
        canvas.clear(BG);
        let rect = Self::media_rect(width);
        let Some(reel) = self.current(store) else {
            super::direct::spinner(canvas, width / 2.0, height / 2.0);
            return;
        };
        if self.item == usize::MAX {
            self.start_item(store);
        }
        let Some(item) = reel.items.get(self.item) else {
            paint::draw_text_centered(canvas, "This story is no longer available", width / 2.0, height / 2.0, SMALL, TEXT);
            return;
        };
        let (w, h) = (rect.w.round() as u32, rect.h.round() as u32);
        match p.picture(&item.media.reference, w, h, Fit::Cover, rect.y) {
            Some(image) => {
                paint::blit(canvas, p.cache, &image, rect.x as i32, rect.y as i32);
                if self.ready_at.is_none() {
                    self.ready_at = Some(p.now);
                }
            }
            None => {
                canvas.fill_rect(rect.x as i32, rect.y as i32, rect.w as i32, rect.h as i32, 0x1a1a1a, 1.0);
                super::direct::spinner(canvas, width / 2.0, rect.y + rect.h / 2.0);
            }
        }
        paint::round_corners(canvas, rect, 10.0 * DP, BG);
        // The next item and the next person's first, ahead.
        if let Some(next) = reel.items.get(self.item + 1) {
            p.prefetch(&next.media.reference, w, h, Fit::Cover);
        }
        if let Some(next) = self.ids.get(self.reel + 1).and_then(|id| store.reels.get(id)).and_then(|r| r.items.first()) {
            p.prefetch(&next.media.reference, w, h, Fit::Cover);
        }
        // A shade under the header.
        for row in 0..260 {
            let a = 0.45 * (1.0 - row as f32 / 260.0);
            canvas.fill_rect(0, (rect.y + row as f32) as i32, width as i32, 1, 0x000000, a);
        }
        self.draw_progress(canvas, store, width, p.now, true);
        let cy = BAR_Y + 92.0;
        let d = 34.0 * DP;
        paint::avatar(canvas, p, reel.user.avatar_ref(), 40.0 + d / 2.0, cy, d, Ring::None);
        let x = 40.0 + d + 24.0;
        let name = reel.title.clone().unwrap_or_else(|| reel.user.handle().to_owned());
        let nw = paint::draw_text(canvas, &name, x, paint::baseline(cy, BODY_BOLD), BODY_BOLD, 0xffffff);
        let mut after = x + nw + 18.0;
        if reel.user.verified {
            paint::verified(canvas, p.cache, after - 6.0, cy, 34.0);
            after += 40.0;
        }
        if reel.title.is_none() {
            paint::draw_text(canvas, &paint::age_short(item.ts, paint::now_ms()), after, paint::baseline(cy, TINY), TINY, 0xdddddd);
        }
        if item.close_friends {
            let label = "Close Friends";
            let lw = paint::measure(label, TINY) + 30.0;
            canvas.fill_rrect(RectF::new(width - 180.0 - lw, cy - 26.0, lw, 52.0), 26.0, paint::GREEN, 1.0);
            paint::draw_text(canvas, label, width - 180.0 - lw + 15.0, paint::baseline(cy, TINY), TINY, 0xffffff);
        }
        paint::icon(canvas, p.cache, Icon::More, width - 160.0, cy - 25.0, 50.0, false, 0xffffff);
        paint::icon(canvas, p.cache, Icon::Close, width - 82.0, cy - 26.0, 52.0, false, 0xffffff);
        for sticker in &item.stickers {
            if sticker.kind == "mention" {
                let sx = sticker.x * rect.w;
                let sy = rect.y + sticker.y * rect.h;
                let lw = paint::measure(&sticker.label, BODY_BOLD) + 40.0;
                canvas.fill_rrect(RectF::new(sx - lw / 2.0, sy - 38.0, lw, 76.0), 16.0, 0xffffff, 1.0);
                paint::draw_text_centered(canvas, &sticker.label, sx, paint::baseline(sy, BODY_BOLD), BODY_BOLD, 0xd62976);
            }
        }
        if item.video {
            paint::draw_text_centered(canvas, "Video plays in Instagram", width / 2.0, rect.y + rect.h - 60.0, TINY, 0xdddddd);
        }
        // The reply bar.
        let by = rect.y + rect.h + 30.0;
        if !reel.own && reel.title.is_none() {
            let field = RectF::new(36.0, by, width - 36.0 - 260.0, 46.0 * DP);
            canvas.stroke_rrect(field, field.h / 2.0, 3.0, 0x8e8e8e, 1.0);
            paint::draw_text(canvas, "Send message", field.x + 44.0, paint::baseline(field.y + field.h / 2.0, SMALL), SMALL, 0xffffff);
            paint::icon(canvas, p.cache, Icon::Heart, width - 220.0, by + field.h / 2.0 - 36.0, 72.0, false, 0xffffff);
            paint::icon(canvas, p.cache, Icon::Share, width - 112.0, by + field.h / 2.0 - 36.0, 72.0, false, 0xffffff);
        }
    }

    /// The window rows the progress segments cover.
    pub fn progress_rows() -> (i32, i32) {
        ((BAR_Y - 2.0) as i32, (BAR_Y + BAR_H + 3.0) as i32)
    }

    /// The progress segments: all of them (`full`), or only the shown
    /// item's fill growing (the frames between items).
    pub fn draw_progress(&self, canvas: &mut Canvas, store: &Store, width: f32, now: Instant, full: bool) {
        let Some(reel) = self.current(store) else {
            return;
        };
        let count = reel.items.len().max(1);
        let gap = 8.0;
        let left = 24.0;
        let seg = (width - 2.0 * left - gap * (count - 1) as f32) / count as f32;
        let fraction = self
            .item(store)
            .map_or(0.0, |item| (self.elapsed(now).as_secs_f32() / Self::duration(item).as_secs_f32()).clamp(0.0, 1.0));
        for n in 0..count {
            if !full && n != self.item {
                continue;
            }
            let x = left + n as f32 * (seg + gap);
            if full {
                canvas.fill_rrect(RectF::new(x, BAR_Y, seg, BAR_H), BAR_H / 2.0, 0xffffff, 0.35);
            }
            let filled = if n < self.item {
                1.0
            } else if n == self.item {
                fraction
            } else {
                0.0
            };
            if filled > 0.0 {
                canvas.fill_rrect(RectF::new(x, BAR_Y, (seg * filled).max(BAR_H), BAR_H), BAR_H / 2.0, 0xffffff, 1.0);
            }
        }
    }
}
