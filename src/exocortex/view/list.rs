//! The conversation list: a level of folders and conversations, pinned
//! first, drawn as rows; scrolled by moving rows.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::gfx::{Canvas, Path, RectF, Rgb};
use crate::text::{self, Weight};
use crate::ui::{
    BG, BLUE, MARGIN, SECONDARY, SECONDARY_ALPHA, SEPARATOR, TERTIARY_ALPHA, WHITE, back_chevron,
    baseline, chevron, large_title,
};

use super::super::model::{Action, Model, Page, Row};
use super::{FrameDamage, GESTURE_RESERVE, Hit, NAV_H, ROW_PRESSED, ViewState, W, pressed_action};

pub(crate) const TITLE_BASELINE: f32 = 132.0;
pub(crate) const LIST_TOP: f32 = 190.0;
pub(crate) const ROW_H: f32 = 168.0;
pub(crate) const INSET: f32 = MARGIN + 16.0;
pub(crate) const ORANGE: Rgb = 0xff9f0a;

pub(crate) fn list_top(model: &Model) -> f32 {
    let base = if model.folders.is_empty() {
        LIST_TOP
    } else {
        NAV_H + 10.0
    };
    if model.offline { base + 70.0 } else { base }
}

pub(crate) fn list_bottom(height: f32) -> f32 {
    height - GESTURE_RESERVE
}

pub fn list_max(model: &mut Model, height: f32) -> f32 {
    let top = list_top(model);
    let folder = model.current_folder().map(str::to_owned);
    let rows = model.sidebar.rows(folder.as_deref()).len();
    (rows as f32 * ROW_H + 40.0 - (list_bottom(height) - top)).max(0.0)
}

pub(crate) fn compose_glyph(canvas: &mut Canvas, cx: f32, cy: f32, alpha: f32) {
    // A square with a pencil across its corner, as iOS's compose button.
    let mut square = Path::new();
    square.stroke_polyline(
        &[
            (cx + 2.0, cy - 26.0),
            (cx - 30.0, cy - 26.0),
            (cx - 30.0, cy + 30.0),
            (cx + 26.0, cy + 30.0),
            (cx + 26.0, cy - 2.0),
        ],
        6.0,
    );
    canvas.fill_path(&square, BLUE, alpha);
    let mut pencil = Path::new();
    pencil.stroke_segment((cx - 6.0, cy + 8.0), (cx + 30.0, cy - 28.0), 9.0);
    canvas.fill_path(&pencil, BLUE, alpha);
}

pub(crate) fn folder_glyph(canvas: &mut Canvas, cx: f32, cy: f32) {
    canvas.fill_rrect(RectF::new(cx - 40.0, cy - 30.0, 36.0, 16.0), 6.0, BLUE, 1.0);
    canvas.fill_rrect(RectF::new(cx - 40.0, cy - 22.0, 80.0, 54.0), 9.0, BLUE, 1.0);
    canvas.fill_rect((cx - 40.0) as i32, (cy - 12.0) as i32, 80, 3, BG, 0.35);
}

pub(crate) fn pin_glyph(canvas: &mut Canvas, cx: f32, cy: f32) {
    let mut pin = Path::new();
    pin.polygon(&[
        (cx - 12.0, cy - 16.0),
        (cx + 12.0, cy - 16.0),
        (cx + 8.0, cy + 2.0),
        (cx - 8.0, cy + 2.0),
    ]);
    pin.stroke_segment((cx, cy + 2.0), (cx, cy + 20.0), 4.0);
    canvas.fill_path(&pin, ORANGE, 0.9);
}

pub(crate) fn status_line(canvas: &mut Canvas, model: &Model, y: f32) {
    if !model.offline {
        return;
    }
    let message = if model.was_connected {
        "Reconnecting to Exocortex…"
    } else {
        "Connecting to Exocortex…"
    };
    text::draw(
        canvas,
        message,
        MARGIN,
        y,
        Weight::Regular,
        38.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}

/// The list's rows that fall in rows `clip`, scrolled to `offset`.
pub(crate) fn draw_list_rows(
    canvas: &mut Canvas,
    model: &Model,
    rows: &[Row],
    pressed: Option<&Action>,
    clip: (f32, f32),
    offset: f32,
) {
    let list_top = list_top(model);
    canvas.set_clip(0, clip.0 as i32, W as i32, clip.1 as i32);
    let today = crate::comms_ui::local_day(crate::clock::unix_now());
    for (index, row) in rows.iter().enumerate() {
        let y = list_top + index as f32 * ROW_H - offset;
        if y + ROW_H < clip.0 || y > clip.1 {
            continue;
        }
        match row {
            Row::Folder {
                id,
                name,
                count,
                pinned,
            } => {
                if pressed == Some(&Action::OpenFolder(id.clone())) {
                    canvas.fill_rect(0, y as i32, W as i32, ROW_H as i32, ROW_PRESSED, 1.0);
                }
                folder_glyph(canvas, INSET + 44.0, y + ROW_H / 2.0);
                let x = INSET + 120.0;
                let right = W - MARGIN - 20.0;
                let count_text = count.to_string();
                text::draw_right(
                    canvas,
                    &count_text,
                    right - 34.0,
                    baseline(y + ROW_H / 2.0, Weight::Regular, 42.0),
                    Weight::Regular,
                    42.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
                if *pinned {
                    pin_glyph(
                        canvas,
                        right - 34.0 - text::measure(&count_text, Weight::Regular, 42.0) - 30.0,
                        y + ROW_H / 2.0,
                    );
                }
                chevron(
                    canvas,
                    right - 6.0,
                    y + ROW_H / 2.0,
                    SECONDARY,
                    TERTIARY_ALPHA,
                );
                let name = text::fit(name, Weight::SemiBold, 46.0, right - 160.0 - x);
                text::draw(
                    canvas,
                    &name,
                    x,
                    baseline(y + ROW_H / 2.0, Weight::SemiBold, 46.0),
                    Weight::SemiBold,
                    46.0,
                    WHITE,
                    1.0,
                );
                canvas.fill_rect(
                    x as i32,
                    (y + ROW_H - 2.0) as i32,
                    (W - x) as i32,
                    2,
                    SEPARATOR,
                    1.0,
                );
            }
            Row::Conversation(id) => {
                let Some(summary) = model.sidebar.conversations.get(id) else {
                    continue;
                };
                if pressed == Some(&Action::OpenConversation(id.clone())) {
                    canvas.fill_rect(0, y as i32, W as i32, ROW_H as i32, ROW_PRESSED, 1.0);
                }
                if summary.unread {
                    canvas.fill_circle(30.0, y + 66.0, 11.0, BLUE, 1.0);
                }
                let x = INSET;
                let right = W - MARGIN - 20.0;
                let when = crate::comms_ui::when_text((summary.updated_at / 1000.0) as i64, today);
                let when_w = text::measure(&when, Weight::Regular, 38.0);
                text::draw_right(
                    canvas,
                    &when,
                    right - 34.0,
                    y + 78.0,
                    Weight::Regular,
                    38.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
                chevron(canvas, right - 6.0, y + 66.0, SECONDARY, TERTIARY_ALPHA);
                let mut title_right = right - 70.0 - when_w;
                if summary.pinned {
                    pin_glyph(canvas, title_right + 14.0, y + 64.0);
                    title_right -= 44.0;
                }
                let title = text::fit(
                    summary.display_title(),
                    Weight::SemiBold,
                    46.0,
                    title_right - x,
                );
                text::draw(
                    canvas,
                    &title,
                    x,
                    y + 80.0,
                    Weight::SemiBold,
                    46.0,
                    WHITE,
                    1.0,
                );
                let selection = super::super::proto::Selection {
                    provider: summary.provider.clone(),
                    model: summary.model.clone(),
                    effort: summary.effort.clone(),
                };
                let (detail, color, alpha) = if summary.streaming {
                    ("Working…".to_owned(), BLUE, 1.0)
                } else {
                    (model.model_label(&selection), SECONDARY, SECONDARY_ALPHA)
                };
                let detail = text::fit(&detail, Weight::Regular, 40.0, right - x);
                text::draw(
                    canvas,
                    &detail,
                    x,
                    y + 136.0,
                    Weight::Regular,
                    40.0,
                    color,
                    alpha,
                );
                canvas.fill_rect(
                    x as i32,
                    (y + ROW_H - 2.0) as i32,
                    (W - x) as i32,
                    2,
                    SEPARATOR,
                    1.0,
                );
            }
        }
    }
    canvas.reset_clip();
}

/// What a list frame showed, so a scroll moves its rows (`draw_list_update`).
#[derive(Clone, PartialEq)]
pub struct ListFrame {
    pub(crate) bounds: (i32, i32),
    pub(crate) offset: f32,
    pub(crate) key: u64,
    pub(crate) pressed: Option<Action>,
}

/// The current list frame's record, or None on other pages and the sheet.
pub fn list_frame(
    model: &Model,
    view: &ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) -> Option<ListFrame> {
    if model.page != Page::List || model.picker {
        return None;
    }
    let mut key = DefaultHasher::new();
    // A pressed row is redrawn alone; the header's buttons are not rows.
    let header = matches!(
        pressed_action(pressed),
        Some(Action::NewConversation | Action::Back)
    );
    (
        model.sidebar.version,
        model.sidebar.loaded,
        &model.folders,
        model.offline,
        model.was_connected,
        height as i32,
        header,
    )
        .hash(&mut key);
    Some(ListFrame {
        bounds: (list_top(model) as i32, list_bottom(height) as i32),
        offset: view.list_scroll.value(now).round(),
        key: key.finish(),
        pressed: pressed_action(pressed).cloned(),
    })
}

/// The list frame on `canvas` (drawn for `old`) scrolled to `new`'s offset:
/// its rows moved, the uncovered strip drawn. None when more changed.
pub fn draw_list_update(
    canvas: &mut Canvas,
    model: &mut Model,
    old: &ListFrame,
    new: &ListFrame,
) -> Option<FrameDamage> {
    if old.key != new.key || old.bounds != new.bounds {
        return None;
    }
    let (top, bottom) = new.bounds;
    let shift = (new.offset - old.offset) as i32;
    let folder = model.current_folder().map(str::to_owned);
    let rows: Vec<Row> = model.sidebar.rows(folder.as_deref()).to_vec();
    if shift == 0 {
        if old.pressed == new.pressed {
            return Some(FrameDamage::default());
        }
        // A row pressed or let go: only its rows.
        let list_top = list_top(model);
        let mut damage = FrameDamage::default();
        for action in old.pressed.iter().chain(&new.pressed) {
            let Some(index) = rows.iter().position(|row| match (row, action) {
                (Row::Folder { id, .. }, Action::OpenFolder(open)) => id == open,
                (Row::Conversation(id), Action::OpenConversation(open)) => id == open,
                _ => false,
            }) else {
                continue;
            };
            let y = list_top + index as f32 * ROW_H - new.offset;
            let strip = ((y as i32).max(top), ((y + ROW_H) as i32 + 1).min(bottom));
            if strip.0 >= strip.1 {
                continue;
            }
            canvas.fill_rect(0, strip.0, W as i32, strip.1 - strip.0, BG, 1.0);
            draw_list_rows(
                canvas,
                model,
                &rows,
                new.pressed.as_ref(),
                (strip.0 as f32, strip.1 as f32),
                new.offset,
            );
            damage.rows.push(strip);
        }
        return Some(damage);
    }
    if shift.abs() > (bottom - top) / 2 || old.pressed != new.pressed {
        return None;
    }
    let width = canvas.width as usize;
    let strip = if shift > 0 {
        canvas.pixels.copy_within(
            (top + shift) as usize * width..bottom as usize * width,
            top as usize * width,
        );
        (bottom - shift, bottom)
    } else {
        canvas.pixels.copy_within(
            top as usize * width..(bottom + shift) as usize * width,
            (top - shift) as usize * width,
        );
        (top, top - shift)
    };
    canvas.fill_rect(0, strip.0, W as i32, strip.1 - strip.0, BG, 1.0);
    draw_list_rows(
        canvas,
        model,
        &rows,
        new.pressed.as_ref(),
        (strip.0 as f32, strip.1 as f32),
        new.offset,
    );
    Some(FrameDamage {
        moved: Some((top, bottom, -shift)),
        rows: vec![strip],
    })
}

pub(crate) fn draw_list(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    let pressed = pressed_action(pressed).cloned();
    let top = list_top(model);
    let bottom = list_bottom(height);
    let folder = model.current_folder().map(str::to_owned);
    let rows: Vec<Row> = model.sidebar.rows(folder.as_deref()).to_vec();
    // Whole pixels, so a scrolled frame equals a shifted one.
    let offset = view.list_scroll.value(now).round();
    draw_list_rows(
        canvas,
        model,
        &rows,
        pressed.as_ref(),
        (top, bottom),
        offset,
    );
    if rows.is_empty() {
        let (title, detail) = if !model.sidebar.loaded {
            ("", "")
        } else if folder.is_some() {
            ("Empty Folder", "")
        } else {
            ("No Conversations", "Tap the compose button to start one.")
        };
        if !title.is_empty() {
            let cy = (top + bottom) / 2.0;
            text::draw_centered(
                canvas,
                title,
                W / 2.0,
                cy,
                Weight::SemiBold,
                58.0,
                WHITE,
                0.9,
            );
            if !detail.is_empty() {
                text::draw_centered(
                    canvas,
                    detail,
                    W / 2.0,
                    cy + 70.0,
                    Weight::Regular,
                    40.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
            }
        }
    }
    // Header.
    canvas.fill_rect(0, 0, W as i32, top as i32 - 1, BG, 1.0);
    let compose_alpha = if pressed == Some(Action::NewConversation) {
        0.4
    } else {
        1.0
    };
    match &folder {
        None => {
            large_title(canvas, "Exocortex", TITLE_BASELINE);
            compose_glyph(canvas, W - MARGIN - 40.0, 92.0, compose_alpha);
            status_line(canvas, model, LIST_TOP + 34.0);
        }
        Some(id) => {
            let parent = model
                .folders
                .len()
                .checked_sub(2)
                .and_then(|i| model.sidebar.folders.get(&model.folders[i]))
                .map_or("Exocortex".to_owned(), |f| f.name.clone());
            let back_alpha = if pressed == Some(Action::Back) {
                0.4
            } else {
                1.0
            };
            back_chevron(canvas, MARGIN + 6.0, 72.0);
            let parent = text::fit(&parent, Weight::Regular, 47.0, 300.0);
            text::draw(
                canvas,
                &parent,
                MARGIN + 36.0,
                baseline(72.0, Weight::Regular, 47.0),
                Weight::Regular,
                47.0,
                BLUE,
                back_alpha,
            );
            let name = model
                .sidebar
                .folders
                .get(id)
                .map_or("Folder", |f| f.name.as_str());
            let name = text::fit(name, Weight::SemiBold, 47.0, 400.0);
            text::draw_centered(
                canvas,
                &name,
                W / 2.0 + 60.0,
                baseline(72.0, Weight::SemiBold, 47.0),
                Weight::SemiBold,
                47.0,
                WHITE,
                1.0,
            );
            compose_glyph(canvas, W - MARGIN - 40.0, 72.0, compose_alpha);
            canvas.fill_rect(0, NAV_H as i32 - 2, W as i32, 2, SEPARATOR, 1.0);
            status_line(canvas, model, NAV_H + 56.0);
        }
    }
}
