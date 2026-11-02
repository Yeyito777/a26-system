//! A chat's page (a tap on its header): the picture, large; the name, the
//! number or the group's size; how many messages this phone keeps; and the
//! chat's photos and videos in WhatsApp's grid, each opening in the viewer.

use serde_json::{Value, json};

use super::band::{self, Band};
use super::icons::{self, Glyph};
use super::{Ask, Page, WhatsApp, avatar, rich, theme, viewer};
use crate::comms::Scroll;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::gfx::Canvas;
use crate::social::media::Fit;
use crate::social::proto::Message;
use crate::social::shell::{Cx, DrawCx};
use crate::text::{self, Weight};

const PICTURE: f32 = 380.0;
const GAP: f32 = 6.0;
const COLUMNS: usize = 3;

pub struct InfoView {
    pub chat: String,
    info: Option<Value>,
    media: Vec<Message>,
    more: bool,
    loading: bool,
    scroll: Scroll,
    band: Band,
    waiting: Vec<avatar::Waiting>,
    version: u64,
}

impl InfoView {
    pub fn new(chat: String) -> Self {
        let mut scroll = Scroll::default();
        scroll.smooth = true;
        Self {
            chat,
            info: None,
            media: Vec::new(),
            more: false,
            loading: false,
            scroll,
            band: Band::default(),
            waiting: Vec::new(),
            version: 0,
        }
    }

    pub fn info_arrived(&mut self, value: Value) {
        self.info = Some(value);
        self.version += 1;
    }

    pub fn media_arrived(&mut self, value: &Value) {
        let page: Vec<Message> =
            serde::Deserialize::deserialize(&value["messages"]).unwrap_or_default();
        self.media.extend(page);
        self.more = value["more"].as_bool().unwrap_or(false);
        self.loading = false;
        self.version += 1;
    }

    fn cell() -> f32 {
        ((f32::from(crate::REQUESTED_WIDTH) - GAP * (COLUMNS as f32 - 1.0)) / COLUMNS as f32)
            .floor()
    }

    /// Where the grid starts in the content.
    fn grid_top() -> f32 {
        PICTURE + 420.0
    }

    fn content_height(&self) -> f32 {
        let rows = self.media.len().div_ceil(COLUMNS) as f32;
        Self::grid_top() + rows * (Self::cell() + GAP) + 120.0
    }
}

pub fn open(app: &mut WhatsApp, cx: &mut Cx) {
    let Some(chat) = app.chat.as_ref().map(|chat| chat.id.clone()) else {
        return;
    };
    app.ask(cx, "whatsapp.chat_info", json!({"chat": chat}), Ask::Info);
    app.ask(
        cx,
        "whatsapp.chat_media",
        json!({"chat": chat, "limit": 90}),
        Ask::InfoMedia,
    );
    let mut view = InfoView::new(chat);
    view.loading = true;
    app.info = Some(view);
    cx.hide_keyboard();
    cx.transition(true);
    app.page = Page::Info;
    app.full = true;
    cx.redraw();
}

fn band_of(height: f32) -> (f32, f32) {
    (theme::TOP + theme::HEADER, height)
}

pub fn tick(app: &mut WhatsApp, cx: &mut Cx) {
    let (top, bottom) = band_of(cx.height);
    let Some(view) = app.info.as_mut() else {
        return;
    };
    let max = (view.content_height() - (bottom - top)).max(0.0);
    if view.scroll.step(max, cx.now) {
        cx.animate();
    }
    let offset = view.scroll.value(cx.now);
    if view.more
        && !view.loading
        && offset > max - 1500.0
        && cx.connected
        && let Some(last) = view.media.last()
    {
        let (chat, before) = (view.chat.clone(), last.id.clone());
        view.loading = true;
        app.ask(
            cx,
            "whatsapp.chat_media",
            json!({"chat": chat, "before": before, "limit": 90}),
            Ask::InfoMedia,
        );
    }
}

pub fn gesture(app: &mut WhatsApp, cx: &mut Cx, gesture: Gesture) {
    let (top, bottom) = band_of(cx.height);
    let now = cx.now;
    let Some(view) = app.info.as_mut() else {
        return;
    };
    let max = (view.content_height() - (bottom - top)).max(0.0);
    match gesture {
        Gesture::DragStart {
            axis: Axis::Vertical,
            y,
            start_y,
            ..
        } => {
            view.scroll.begin(start_y, now);
            view.scroll.moved(y, max, now);
            cx.animate();
        }
        Gesture::Drag {
            axis: Axis::Vertical,
            y,
            ..
        } => {
            view.scroll.moved(y, max, now);
            cx.animate();
        }
        Gesture::DragEnd {
            axis: Axis::Vertical,
            ..
        } => {
            view.scroll.end(max, now);
            cx.animate();
        }
        Gesture::DragEnd {
            axis: Axis::Horizontal,
            x,
            ..
        } if x > cx.width * 0.4 => app.back(cx),
        Gesture::Tap { x, y } => {
            if super::on_back(x, y) {
                app.back(cx);
                return;
            }
            let content_y = y - top + view.scroll.value(now).round() - InfoView::grid_top();
            if y < top || content_y < 0.0 {
                return;
            }
            let cell = InfoView::cell();
            let (row, column) = (
                (content_y / (cell + GAP)) as usize,
                ((x / (cell + GAP)) as usize).min(COLUMNS - 1),
            );
            if let Some(message) = view.media.get(row * COLUMNS + column)
                && let Some(media) = message.media.first()
            {
                let title = if message.mine {
                    "You".to_owned()
                } else {
                    message.author.name.clone()
                };
                app.viewer = Some(viewer::Viewer::new(
                    media.clone(),
                    title,
                    message.ts,
                    message.text.clone(),
                ));
                cx.redraw();
            }
        }
        _ => {}
    }
}

pub fn draw(app: &mut WhatsApp, canvas: &mut Canvas, cx: &mut DrawCx, full: bool) {
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    let (top, bottom) = band_of(height);
    let now = cx.now;
    let (title, avatar_media, group) =
        match app.info.as_ref().and_then(|view| app.chats.get(&view.chat)) {
            Some(chat) => (chat.title.clone(), chat.avatar.clone(), chat.is_group()),
            None => (String::new(), None, false),
        };
    let Some(view) = app.info.as_mut() else {
        return;
    };
    if full {
        canvas.clear(theme::BG);
        super::draw_bar(canvas, "", None, theme::BAR);
    }
    let offset = view.scroll.value(now).round();
    let translate = top - offset;
    let key = band::key_of(&[view.version, width as u64, height as u64]);
    let info = view.info.clone().unwrap_or(Value::Null);
    let media = &view.media;
    let rounded = &mut app.rounded;
    let ready = avatar::arrived(cx.media, &mut view.waiting, now);
    let mut pics = avatar::Pics::new(&mut *cx.media, &mut view.waiting, translate);
    let cell = InfoView::cell();
    let mut draw_rows = |canvas: &mut Canvas, (y0, y1): (i32, i32)| {
        canvas.fill_rect(0, y0, canvas.width, y1 - y0, theme::BG, 1.0);
        let base = translate;
        // The picture, the name, the number.
        if base + InfoView::grid_top() > y0 as f32 {
            let x = (width - PICTURE) / 2.0;
            pics.span = (base + 40.0, base + 40.0 + PICTURE);
            let px = PICTURE.round() as u32;
            match avatar_media
                .as_ref()
                .and_then(|a| pics.get(&a.reference, px, px, Fit::Cover, now))
            {
                Some(image) => {
                    let round = rounded.get(&image, PICTURE / 2.0);
                    canvas.draw_image(&round, x as i32, (base + 40.0) as i32, 1.0);
                }
                None => avatar::silhouette(canvas, group, x, base + 40.0, PICTURE),
            }
            let name = text::fit(
                &rich::emojify(&title),
                Weight::SemiBold,
                58.0,
                width - 120.0,
            );
            text::draw_centered(
                canvas,
                &name,
                width / 2.0,
                base + PICTURE + 130.0,
                Weight::SemiBold,
                58.0,
                theme::TEXT,
                1.0,
            );
            let subtitle = if group {
                info["members"]
                    .as_u64()
                    .map_or_else(|| "Group".to_owned(), |n| format!("Group · {n} members"))
            } else {
                info["phone"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_default()
            };
            text::draw_centered(
                canvas,
                &subtitle,
                width / 2.0,
                base + PICTURE + 196.0,
                Weight::Regular,
                40.0,
                theme::SECONDARY,
                1.0,
            );
            if let Some(push) = info["push_name"].as_str().filter(|p| !p.is_empty()) {
                let push = text::fit(
                    &format!("~ {}", rich::emojify(push)),
                    Weight::Regular,
                    36.0,
                    width - 120.0,
                );
                text::draw_centered(
                    canvas,
                    &push,
                    width / 2.0,
                    base + PICTURE + 250.0,
                    Weight::Regular,
                    36.0,
                    theme::SECONDARY,
                    1.0,
                );
            }
            let kept = info["messages"].as_u64().unwrap_or(0);
            let muted = info["muted"].as_bool().unwrap_or(false);
            let line = format!(
                "{kept} message{} on this phone{}",
                if kept == 1 { "" } else { "s" },
                if muted { " · muted" } else { "" }
            );
            text::draw_centered(
                canvas,
                &line,
                width / 2.0,
                base + PICTURE + 310.0,
                Weight::Regular,
                32.0,
                theme::SECONDARY,
                1.0,
            );
            text::draw(
                canvas,
                "Media",
                theme::MARGIN,
                base + InfoView::grid_top() - 30.0,
                Weight::Medium,
                36.0,
                theme::SECONDARY,
                1.0,
            );
        }
        // The grid, row by row.
        let first = (((y0 as f32 - base - InfoView::grid_top()) / (cell + GAP))
            .floor()
            .max(0.0)) as usize;
        let rows = media.len().div_ceil(COLUMNS);
        for row in first..rows {
            let y = base + InfoView::grid_top() + row as f32 * (cell + GAP);
            if y > y1 as f32 {
                break;
            }
            pics.span = (y, y + cell);
            for column in 0..COLUMNS {
                let Some(message) = media.get(row * COLUMNS + column) else {
                    break;
                };
                let Some(item) = message.media.first() else {
                    continue;
                };
                let x = column as f32 * (cell + GAP);
                let px = cell as u32;
                let reference = if item.kind == "image" {
                    Some(&item.reference)
                } else {
                    item.thumb.as_ref()
                };
                let picture = reference
                    .and_then(|r| pics.get(r, px, px, Fit::Cover, now))
                    .or_else(|| {
                        item.thumb
                            .as_ref()
                            .and_then(|t| pics.get(t, px, px, Fit::Cover, now))
                    });
                match picture {
                    Some(image) => canvas.draw_image(&image, x as i32, y as i32, 1.0),
                    None => canvas.fill_rect(
                        x as i32,
                        y as i32,
                        px as i32,
                        px as i32,
                        theme::SURFACE,
                        1.0,
                    ),
                }
                if item.kind != "image" {
                    icons::draw(
                        canvas,
                        Glyph::PlayFill,
                        x + 46.0,
                        y + cell - 44.0,
                        44.0,
                        0xffffff,
                        1.0,
                    );
                }
            }
        }
        if media.is_empty() {
            let note = if info.is_null() {
                "Loading…"
            } else {
                "No photos or videos on this phone"
            };
            text::draw_centered(
                canvas,
                note,
                width / 2.0,
                base + InfoView::grid_top() + 80.0,
                Weight::Regular,
                36.0,
                theme::SECONDARY,
                1.0,
            );
        }
    };
    let drawn = view.band.draw(
        canvas,
        &mut app.damage,
        full,
        (top as i32, bottom as i32),
        -(offset as i32),
        key,
        &mut draw_rows,
    );
    if drawn.is_some() {
        for (c0, c1) in ready {
            let (y0, y1) = (
                ((c0 + translate) as i32).max(top as i32),
                ((c1 + translate) as i32).min(bottom as i32),
            );
            if y1 > y0 {
                canvas.set_clip(0, y0, canvas.width, y1);
                draw_rows(canvas, (y0, y1));
                canvas.reset_clip();
                app.damage.rows.push((y0, y1));
            }
        }
    }
}
