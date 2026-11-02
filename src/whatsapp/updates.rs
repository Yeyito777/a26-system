//! Updates: the status updates of the last day (WhatsApp's "Status"), who
//! posted, with a ring for the unseen ones, and the full-screen viewer that
//! plays each person's updates in turn (photos, text on colour), with the
//! bars across the top. Seeing a status here is noted here only: no
//! receipt is ever sent (the bridge has no such command).

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::icons::{self, Glyph};
use super::{Ask, Page, WhatsApp, avatar, list, rich, theme, time};
use crate::exocortex::gesture::{Axis, Gesture};
use crate::gfx::{Canvas, RectF};
use crate::social::media::Fit;
use crate::social::proto::{Media, Message, User};
use crate::social::shell::{Cx, DrawCx};
use crate::text::{self, Weight};

const ROW: f32 = 186.0;
const AVATAR: f32 = 126.0;
const HEADER_H: f32 = 110.0;
/// A photo shows this long; text a little longer with more of it.
const PHOTO_TIME: Duration = Duration::from_millis(5000);

#[derive(Debug, Clone, Default)]
pub struct Update {
    pub user: User,
    pub ts: i64,
    pub seen: bool,
    pub items: Vec<Message>,
}

#[derive(Default)]
pub struct UpdatesView {
    pub updates: Vec<Update>,
    pub loaded: bool,
    asked: bool,
}

pub struct StatusViewer {
    updates: Vec<Update>,
    user: usize,
    item: usize,
    started: Instant,
    /// Held down: the bars stop.
    paused: Option<Instant>,
    drag: f32,
    start_y: f32,
    /// What the last frame drew whole (user, item).
    drawn: Option<(usize, usize)>,
}

pub fn parse(value: &Value) -> Vec<Update> {
    value["updates"]
        .as_array()
        .map(|updates| {
            updates
                .iter()
                .map(|u| Update {
                    user: serde_json::from_value(u["user"].clone()).unwrap_or_default(),
                    ts: u["ts"].as_i64().unwrap_or(0),
                    seen: u["seen"].as_bool().unwrap_or(false),
                    items: serde_json::from_value(u["items"].clone()).unwrap_or_default(),
                })
                .filter(|u| !u.items.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Asks the bridge for the updates (the page opened, a new one came).
pub fn refresh(app: &mut WhatsApp, cx: &mut Cx) {
    if !app.hello.capabilities.iter().any(|c| c == "whatsapp.qr") {
        return;
    }
    app.updates.asked = true;
    app.ask(cx, "whatsapp.status.list", json!({}), Ask::Updates);
}

fn rows(view: &UpdatesView) -> Vec<(Option<&'static str>, Option<usize>)> {
    // (A section's title, or an update.)
    let mut out = Vec::new();
    let mine: Vec<usize> = (0..view.updates.len())
        .filter(|&i| view.updates[i].user.id == "me")
        .collect();
    let recent: Vec<usize> = (0..view.updates.len())
        .filter(|&i| view.updates[i].user.id != "me" && !view.updates[i].seen)
        .collect();
    let seen: Vec<usize> = (0..view.updates.len())
        .filter(|&i| view.updates[i].user.id != "me" && view.updates[i].seen)
        .collect();
    for i in mine {
        out.push((None, Some(i)));
    }
    if !recent.is_empty() {
        out.push((Some("Recent updates"), None));
        out.extend(recent.into_iter().map(|i| (None, Some(i))));
    }
    if !seen.is_empty() {
        out.push((Some("Viewed updates"), None));
        out.extend(seen.into_iter().map(|i| (None, Some(i))));
    }
    out
}

fn row_height(row: &(Option<&'static str>, Option<usize>)) -> f32 {
    if row.0.is_some() { 96.0 } else { ROW }
}

pub fn gesture(app: &mut WhatsApp, cx: &mut Cx, gesture: Gesture) {
    let Gesture::Tap { x, y } = gesture else {
        return;
    };
    let height = cx.height;
    if y >= height - theme::TAB_BAR {
        let page = list::tab_at(x, cx.width);
        if page != Page::Updates {
            app.go(cx, page);
        }
        return;
    }
    if y > theme::TOP + theme::APP_BAR - 20.0 && y < theme::TOP + theme::APP_BAR + HEADER_H {
        return;
    }
    let mut top = theme::TOP + theme::APP_BAR + HEADER_H;
    for row in rows(&app.updates) {
        let h = row_height(&row);
        if y >= top && y < top + h {
            if let (None, Some(index)) = row {
                open(app, cx, index);
            }
            return;
        }
        top += h;
    }
}

fn open(app: &mut WhatsApp, cx: &mut Cx, index: usize) {
    let mut updates = app.updates.updates.clone();
    // Seen ones play from their start, the others from the first unseen.
    let item = updates[index]
        .items
        .iter()
        .position(|m| !m.ext["seen"].as_bool().unwrap_or(false))
        .unwrap_or(0);
    for update in &mut updates {
        update.seen = update
            .items
            .iter()
            .all(|m| m.ext["seen"].as_bool().unwrap_or(false));
    }
    let mut viewer = StatusViewer {
        updates,
        user: index,
        item,
        started: cx.now,
        paused: None,
        drag: 0.0,
        start_y: 0.0,
        drawn: None,
    };
    seen(app, cx, &mut viewer);
    app.status_viewer = Some(viewer);
    app.full = true;
    cx.wake_at(cx.now + Duration::from_millis(100));
    cx.redraw();
}

/// The update on screen is seen (here only).
fn seen(app: &mut WhatsApp, cx: &mut Cx, viewer: &mut StatusViewer) {
    let Some(message) = viewer
        .updates
        .get_mut(viewer.user)
        .and_then(|u| u.items.get_mut(viewer.item))
    else {
        return;
    };
    if message.ext["seen"].as_bool().unwrap_or(false) {
        return;
    }
    message.ext["seen"] = json!(true);
    let id = message.id.clone();
    app.ask(
        cx,
        "whatsapp.status.seen",
        json!({"ids": [id]}),
        Ask::Other("note the status as seen"),
    );
}

fn duration(message: &Message) -> Duration {
    if message.media.is_empty() {
        Duration::from_millis(3500 + 40 * message.text.chars().count().min(200) as u64)
    } else {
        PHOTO_TIME
    }
}

impl StatusViewer {
    fn current(&self) -> Option<&Message> {
        self.updates.get(self.user)?.items.get(self.item)
    }

    /// Moves by one update (across people); false when past either end.
    fn step(&mut self, forward: bool, now: Instant) -> bool {
        let count = self.updates[self.user].items.len();
        if forward {
            if self.item + 1 < count {
                self.item += 1;
            } else if self.user + 1 < self.updates.len() {
                self.user += 1;
                self.item = 0;
            } else {
                return false;
            }
        } else if self.item > 0 {
            self.item -= 1;
        } else if self.user > 0 {
            self.user -= 1;
            self.item = self.updates[self.user].items.len().saturating_sub(1);
        } else {
            self.started = now;
            return true;
        }
        self.started = now;
        true
    }

    fn progress(&self, now: Instant) -> f32 {
        let Some(message) = self.current() else {
            return 0.0;
        };
        let at = self.paused.unwrap_or(now);
        (at.duration_since(self.started).as_secs_f32() / duration(message).as_secs_f32())
            .clamp(0.0, 1.0)
    }
}

/// The viewer's gestures; true when it closes.
pub fn viewer_gesture(app: &mut WhatsApp, cx: &mut Cx, gesture: Gesture) -> bool {
    let Some(mut viewer) = app.status_viewer.take() else {
        return true;
    };
    let close = match gesture {
        Gesture::Back => true,
        Gesture::Lift { .. } | Gesture::LongPress { .. } => {
            viewer.paused.get_or_insert(cx.now);
            false
        }
        Gesture::LongPressEnd { .. } | Gesture::Cancel => {
            resume(&mut viewer, cx.now);
            false
        }
        Gesture::Tap { x, y } => {
            if y < theme::TOP + 260.0 && x < 140.0 {
                true
            } else {
                resume(&mut viewer, cx.now);
                let moved = viewer.step(x > cx.width * 0.3, cx.now);
                if moved {
                    seen(app, cx, &mut viewer);
                }
                !moved
            }
        }
        Gesture::DragStart {
            axis: Axis::Vertical,
            y,
            start_y,
            ..
        } => {
            viewer.paused.get_or_insert(cx.now);
            viewer.start_y = start_y;
            viewer.drag = (y - start_y).max(0.0);
            false
        }
        Gesture::Drag {
            axis: Axis::Vertical,
            y,
            ..
        } => {
            viewer.drag = (y - viewer.start_y).max(0.0);
            false
        }
        Gesture::DragEnd {
            axis: Axis::Vertical,
            vy,
            ..
        } => {
            let close = viewer.drag > 300.0 || vy > 1500.0;
            viewer.drag = 0.0;
            resume(&mut viewer, cx.now);
            close
        }
        _ => false,
    };
    viewer.drawn = None;
    if !close {
        app.status_viewer = Some(viewer);
    } else {
        refresh(app, cx);
    }
    app.full = true;
    cx.redraw();
    close
}

fn resume(viewer: &mut StatusViewer, now: Instant) {
    if let Some(paused) = viewer.paused.take() {
        viewer.started += now.duration_since(paused);
    }
}

/// Advances the bars, and the updates when one has run its time.
pub fn tick(app: &mut WhatsApp, cx: &mut Cx) {
    let Some(mut viewer) = app.status_viewer.take() else {
        return;
    };
    if viewer.paused.is_none() && viewer.progress(cx.now) >= 1.0 {
        if viewer.step(true, cx.now) {
            seen(app, cx, &mut viewer);
            viewer.drawn = None;
        } else {
            app.full = true;
            refresh(app, cx);
            cx.redraw();
            return;
        }
    }
    if viewer.paused.is_none() {
        // The bars move a few pixels a frame: 20 frames a second is smooth enough.
        cx.wake_at(cx.now + Duration::from_millis(50));
        cx.redraw();
    }
    app.status_viewer = Some(viewer);
}

pub fn draw(app: &mut WhatsApp, canvas: &mut Canvas, cx: &mut DrawCx, full: bool) {
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    canvas.clear(theme::BG);
    let _ = full;
    // The bar: the title.
    canvas.fill_rect(
        0,
        0,
        canvas.width,
        (theme::TOP + theme::APP_BAR) as i32,
        theme::BAR,
        1.0,
    );
    let cy = theme::TOP + theme::APP_BAR / 2.0;
    text::draw(
        canvas,
        "Updates",
        theme::MARGIN,
        text::centered_baseline(cy, Weight::SemiBold, theme::T_APP_TITLE),
        Weight::SemiBold,
        theme::T_APP_TITLE,
        theme::TEXT,
        1.0,
    );
    let top = theme::TOP + theme::APP_BAR;
    text::draw(
        canvas,
        "Status",
        theme::MARGIN,
        top + 74.0,
        Weight::SemiBold,
        46.0,
        theme::TEXT,
        1.0,
    );
    let mut y = top + HEADER_H;
    let mut waiting = Vec::new();
    let mut pics = avatar::Pics::new(&mut *cx.media, &mut waiting, 0.0);
    let view = &app.updates;
    let rows = rows(view);
    for row in &rows {
        let h = row_height(row);
        if y > height - theme::TAB_BAR {
            break;
        }
        match row {
            (Some(title), _) => {
                text::draw(
                    canvas,
                    title,
                    theme::MARGIN,
                    y + 66.0,
                    Weight::Medium,
                    34.0,
                    theme::SECONDARY,
                    1.0,
                );
            }
            (None, Some(index)) => {
                let update = &view.updates[*index];
                draw_row(
                    canvas,
                    &mut pics,
                    &mut app.rounded,
                    update,
                    y,
                    width,
                    cx.now,
                );
            }
            _ => {}
        }
        y += h;
    }
    if rows.is_empty() {
        let note = if app.updates.loaded {
            "No status updates in the last 24 hours"
        } else {
            "Loading…"
        };
        text::draw_centered(
            canvas,
            note,
            width / 2.0,
            top + 360.0,
            Weight::Regular,
            theme::T_PREVIEW,
            theme::SECONDARY,
            1.0,
        );
    }
    list::draw_tabs(canvas, width, height, Page::Updates);
}

fn draw_row(
    canvas: &mut Canvas,
    pics: &mut avatar::Pics,
    rounded: &mut avatar::Rounded,
    update: &Update,
    y: f32,
    width: f32,
    now: Instant,
) {
    let cy = y + ROW / 2.0;
    let x = theme::MARGIN + 8.0;
    // The ring: a segment per update, green while unseen.
    let count = update.items.len().max(1);
    let gap = if count > 1 { 0.18 } else { 0.0 };
    for (n, item) in update.items.iter().enumerate() {
        let seen = item.ext["seen"].as_bool().unwrap_or(false);
        let span = std::f32::consts::TAU / count as f32;
        let start = n as f32 * span + gap / 2.0;
        canvas.ring_arc(
            x + AVATAR / 2.0,
            cy,
            AVATAR / 2.0 + 10.0,
            6.0,
            start,
            start + span - gap,
            if seen { theme::SECONDARY } else { theme::GREEN },
            1.0,
        );
    }
    avatar::draw(
        canvas,
        pics,
        rounded,
        update.user.avatar.as_ref(),
        false,
        x,
        cy - AVATAR / 2.0,
        AVATAR,
        now,
    );
    let tx = x + AVATAR + 48.0;
    let name = text::fit(
        &rich::emojify(&update.user.name),
        Weight::Medium,
        theme::T_TITLE,
        width - tx - theme::MARGIN,
    );
    text::draw(
        canvas,
        &name,
        tx,
        cy - 8.0,
        Weight::Medium,
        theme::T_TITLE,
        theme::TEXT,
        1.0,
    );
    let when = format!("{}, {}", time::day_chip(update.ts), time::clock(update.ts));
    text::draw(
        canvas,
        &when,
        tx,
        cy + 46.0,
        Weight::Regular,
        theme::T_TIME + 2.0,
        theme::SECONDARY,
        1.0,
    );
}

/// The viewer, whole when its update changed, else just the bars.
pub fn draw_viewer(
    app: &mut WhatsApp,
    canvas: &mut Canvas,
    cx: &mut DrawCx,
    full: bool,
) -> Option<(i32, i32)> {
    let viewer = app.status_viewer.as_mut()?;
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    let bars = (theme::TOP + 24.0, theme::TOP + 34.0);
    let whole = full || viewer.drawn != Some((viewer.user, viewer.item)) || viewer.drag > 0.0;
    // A picture not there yet: whole frames until it is.
    let mut missing = false;
    let update = viewer.updates.get(viewer.user)?.clone();
    let message = update.items.get(viewer.item)?.clone();
    let progress = viewer.progress(cx.now);
    if whole {
        let background = message.ext["background"]
            .as_str()
            .and_then(|hex| u32::from_str_radix(hex.trim_start_matches('#'), 16).ok())
            .unwrap_or(0x000000);
        canvas.clear(if message.media.is_empty() {
            background
        } else {
            0x000000
        });
        let dy = viewer.drag;
        if let Some(media) = message.media.first() {
            missing |= !draw_media(canvas, cx, media, width, height, dy);
        } else {
            let laid = rich::layout(&message.text, width - 160.0, 64.0, 86.0, Weight::Medium);
            let block = laid.height();
            let mut y = (height - block) / 2.0 + 64.0 + dy;
            for line in &laid.lines {
                let text_line: String = line.runs.iter().map(|r| r.text.as_str()).collect();
                text::draw_centered(
                    canvas,
                    &text_line,
                    width / 2.0,
                    y,
                    Weight::Medium,
                    64.0,
                    0xffffff,
                    1.0,
                );
                y += laid.line_height;
            }
        }
        if !message.media.is_empty() && !message.text.trim().is_empty() {
            let laid = rich::layout(&message.text, width - 120.0, 42.0, 56.0, Weight::Regular);
            let block = laid.height() + 70.0;
            let top = height - theme::GESTURE_RESERVE - block;
            canvas.fill_rect(0, top as i32, canvas.width, block as i32, 0x000000, 0.5);
            let clip = (0.0, height);
            rich::draw(
                canvas,
                &laid,
                60.0,
                top + 64.0,
                Weight::Regular,
                0xffffff,
                theme::LINK,
                clip,
            );
        }
        // Who and when.
        let cy = theme::TOP + 120.0;
        icons::draw(canvas, Glyph::Back, 56.0, cy, 60.0, 0xffffff, 1.0);
        let mut waiting = Vec::new();
        let mut pics = avatar::Pics::new(&mut *cx.media, &mut waiting, 0.0);
        avatar::draw(
            canvas,
            &mut pics,
            &mut app.rounded,
            update.user.avatar.as_ref(),
            false,
            110.0,
            cy - 50.0,
            100.0,
            cx.now,
        );
        missing |= !waiting.is_empty();
        let name = text::fit(
            &rich::emojify(&update.user.name),
            Weight::Medium,
            44.0,
            width - 300.0,
        );
        text::draw(
            canvas,
            &name,
            236.0,
            cy - 6.0,
            Weight::Medium,
            44.0,
            0xffffff,
            1.0,
        );
        let when = format!(
            "{}, {}",
            time::day_chip(message.ts),
            time::clock(message.ts)
        );
        text::draw(
            canvas,
            &when,
            236.0,
            cy + 40.0,
            Weight::Regular,
            32.0,
            0xdddddd,
            1.0,
        );
    }
    // The bars: one per update of this person, the current one filling.
    let count = update.items.len();
    let gap = 10.0;
    let w = (width - 40.0 - gap * (count as f32 - 1.0)) / count as f32;
    if !whole {
        // Only the bars' rows change: under them, what the update shows.
        let background = message.ext["background"]
            .as_str()
            .and_then(|hex| u32::from_str_radix(hex.trim_start_matches('#'), 16).ok())
            .filter(|_| message.media.is_empty())
            .unwrap_or(0x000000);
        canvas.fill_rect(
            0,
            bars.0 as i32,
            canvas.width,
            (bars.1 - bars.0) as i32,
            background,
            1.0,
        );
    }
    for n in 0..count {
        let x = 20.0 + n as f32 * (w + gap);
        let rect = RectF {
            x,
            y: bars.0,
            w,
            h: 6.0,
        };
        canvas.fill_rrect(rect, 3.0, 0xffffff, 0.35);
        let fill = if n < viewer.item {
            1.0
        } else if n == viewer.item {
            progress
        } else {
            0.0
        };
        if fill > 0.0 {
            canvas.fill_rrect(
                RectF {
                    w: (w * fill).max(6.0),
                    ..rect
                },
                3.0,
                0xffffff,
                1.0,
            );
        }
    }
    viewer.drawn = (!missing).then_some((viewer.user, viewer.item));
    if whole {
        None
    } else {
        Some((bars.0 as i32, bars.1 as i32))
    }
}

/// Draws the update's picture; false while it has not come.
fn draw_media(
    canvas: &mut Canvas,
    cx: &mut DrawCx,
    media: &Media,
    width: f32,
    height: f32,
    dy: f32,
) -> bool {
    let video = media.kind != "image";
    let (w, h) = (width as u32, (height - 2.0 * theme::TOP - 200.0) as u32);
    let reference = if video {
        media.thumb.as_deref()
    } else {
        Some(media.reference.as_str())
    };
    let picture = reference
        .and_then(|r| cx.media.get(r, w, h, Fit::Contain, cx.now))
        .or_else(|| {
            media
                .thumb
                .as_deref()
                .and_then(|t| cx.media.get(t, w, h, Fit::Contain, cx.now))
        });
    match picture {
        Some(image) => {
            let (iw, ih) = (image.width as f32, image.height as f32);
            let scale = (width / iw).min(h as f32 / ih);
            let rect = RectF {
                x: (width - iw * scale) / 2.0,
                y: (height - ih * scale) / 2.0 + dy,
                w: iw * scale,
                h: ih * scale,
            };
            if (scale - 1.0).abs() < 0.01 {
                canvas.draw_image(&image, rect.x.round() as i32, rect.y.round() as i32, 1.0);
            } else {
                canvas.draw_image_scaled(&image, rect, 1.0);
            }
            true
        }
        None => {
            text::draw_centered(
                canvas,
                "Loading…",
                width / 2.0,
                height / 2.0,
                Weight::Regular,
                40.0,
                theme::SECONDARY,
                1.0,
            );
            false
        }
    }
}
