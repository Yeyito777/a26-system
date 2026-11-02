//! A photo, sticker or video poster on black, whole: who sent it and when
//! above, its caption below. A tap on the arrow, Back, or a swipe down
//! closes it.

use std::time::Instant;

use super::avatar;
use super::icons::{self, Glyph};
use super::{theme, time};
use crate::exocortex::gesture::{Axis, Gesture};
use crate::gfx::{Canvas, RectF};
use crate::social::media::Fit;
use crate::social::proto;
use crate::social::shell::{Cx, DrawCx};
use crate::text::{self, Weight};

pub struct Viewer {
    media: proto::Media,
    title: String,
    ts: i64,
    caption: String,
    /// A swipe down in progress: where it started, how far it went.
    start: f32,
    drag: f32,
}

impl Viewer {
    pub fn new(media: proto::Media, title: String, ts: i64, caption: String) -> Self {
        Self {
            media,
            title,
            ts,
            caption,
            start: 0.0,
            drag: 0.0,
        }
    }
}

/// True when the viewer closes.
pub fn gesture(viewer: &mut Viewer, cx: &mut Cx, gesture: Gesture) -> bool {
    match gesture {
        Gesture::Back => true,
        Gesture::Tap { x, y } => y < theme::TOP + theme::HEADER && x < 160.0,
        Gesture::DragStart {
            axis: Axis::Vertical,
            y,
            start_y,
            ..
        } => {
            viewer.start = start_y;
            viewer.drag = (y - start_y).max(0.0);
            cx.redraw();
            false
        }
        Gesture::Drag {
            axis: Axis::Vertical,
            y,
            ..
        } => {
            viewer.drag = (y - viewer.start).max(0.0);
            cx.redraw();
            false
        }
        Gesture::DragEnd {
            axis: Axis::Vertical,
            vy,
            ..
        } => {
            let close = viewer.drag > 300.0 || vy > 1500.0;
            viewer.drag = 0.0;
            cx.redraw();
            close
        }
        Gesture::DragEnd {
            axis: Axis::Horizontal,
            x,
            ..
        } => x > cx.width * 0.4,
        _ => false,
    }
}

pub fn tick(_viewer: &mut Viewer, _cx: &mut Cx) {}

pub fn draw(viewer: &mut Viewer, canvas: &mut Canvas, cx: &mut DrawCx) {
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    canvas.clear(0x000000);
    let now: Instant = cx.now;
    let video = viewer.media.kind == "video" || viewer.media.kind == "gif";
    let (w, h) = (
        width as u32,
        (height - 2.0 * (theme::TOP + theme::HEADER)) as u32,
    );
    let reference = if video {
        viewer.media.thumb.as_deref()
    } else {
        Some(viewer.media.reference.as_str())
    };
    let picture = reference.and_then(|r| cx.media.get(r, w, h, Fit::Contain, now));
    let area_top = theme::TOP + theme::HEADER;
    match picture {
        Some(image) => {
            let (iw, ih) = (image.width as f32, image.height as f32);
            // A poster is small: shown scaled up to the width.
            let scale = if video {
                (width / iw).min(h as f32 / ih)
            } else {
                1.0
            };
            let rect = RectF {
                x: (width - iw * scale) / 2.0,
                y: area_top + (h as f32 - ih * scale) / 2.0 + viewer.drag,
                w: iw * scale,
                h: ih * scale,
            };
            if scale == 1.0 {
                avatar::blit(canvas, &image, rect.x.round() as i32, rect.y.round() as i32);
            } else {
                canvas.draw_image_scaled(&image, rect, 1.0);
            }
        }
        None => {
            let note = if reference.is_some() {
                "Loading…"
            } else {
                "No preview"
            };
            text::draw_centered(
                canvas,
                note,
                width / 2.0,
                height / 2.0,
                Weight::Regular,
                40.0,
                theme::SECONDARY,
                1.0,
            );
        }
    }
    if video {
        canvas.fill_circle(width / 2.0, height / 2.0, 90.0, 0x000000, 0.5);
        icons::draw(
            canvas,
            Glyph::PlayFill,
            width / 2.0 + 6.0,
            height / 2.0,
            100.0,
            0xffffff,
            1.0,
        );
        text::draw_centered(
            canvas,
            "Videos can't play on this phone yet",
            width / 2.0,
            height / 2.0 + 170.0,
            Weight::Regular,
            36.0,
            0xffffff,
            0.85,
        );
    }
    // The bar over it.
    canvas.fill_rect(
        0,
        0,
        canvas.width,
        (theme::TOP + theme::HEADER) as i32,
        0x000000,
        0.55,
    );
    let cy = theme::TOP + theme::HEADER / 2.0;
    icons::draw(canvas, Glyph::Back, 70.0, cy, 66.0, 0xffffff, 1.0);
    let title = text::fit(&viewer.title, Weight::Medium, 46.0, width - 220.0);
    text::draw(
        canvas,
        &title,
        150.0,
        cy - 4.0,
        Weight::Medium,
        46.0,
        0xffffff,
        1.0,
    );
    let when = format!("{}, {}", time::day_chip(viewer.ts), time::clock(viewer.ts));
    text::draw(
        canvas,
        &when,
        150.0,
        cy + 42.0,
        Weight::Regular,
        32.0,
        0xdddddd,
        1.0,
    );
    if !viewer.caption.trim().is_empty() {
        let laid = super::rich::layout(&viewer.caption, width - 120.0, 40.0, 54.0, Weight::Regular);
        let block = laid.height() + 60.0;
        let top = height - theme::GESTURE_RESERVE - block;
        canvas.fill_rect(0, top as i32, canvas.width, block as i32, 0x000000, 0.55);
        super::rich::draw(
            canvas,
            &laid,
            60.0,
            top + 60.0,
            Weight::Regular,
            0xffffff,
            theme::LINK,
            (0.0, height),
        );
    }
}
