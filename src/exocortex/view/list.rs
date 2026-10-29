//! The conversation list (WP-B; roadmap §3.6.1–3.6.5): the Whale top bar
//! (search field, ⋯, new folder, new chat, the page rail), then a level of
//! folders and conversations, pinned first, as 160 px rows; scrolled by
//! moving rows. WP-A's version: drill-down folders (with a path row) and
//! rows recoloured to the palette; B brings the tree, the PINNED header,
//! badges, search, menus and the rest.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::gfx::{Canvas, RectF};

use super::super::anim::{self, Animator};
use super::super::model::{Action, Model, Nav, Page, Row, sidebar};
use super::super::theme::{
    self, FOLDER_ICON, GESTURE_RESERVE, HAIRLINE, HEADER_ROW, LIST_PAD, LIST_ROW, T_CAPTION,
    T_EYEBROW, T_META, T_ROW, TOP_BAR, W,
};
use super::super::ui::icons::{self, Icon};
use super::super::ui::widgets::{self, Button, ButtonKind, RAIL_LIST};
use super::{FrameDamage, Hit, ViewState, pressed_action};

pub(crate) const ROW_H: f32 = LIST_ROW;
/// The header's controls' centres.
const MORE_X: f32 = 768.0;
const FOLDER_PLUS_X: f32 = 888.0;
const PLUS_X: f32 = 1008.0;
const ROW_Y: f32 = 61.0;
/// The search field (`real-01-sidebar`: x 44 to 692; the roadmap's 17 to
/// 680 is not where the iPhone draws it).
const SEARCH: RectF = RectF::new(44.0, 14.0, 648.0, 94.0);

fn in_folder(model: &Model) -> bool {
    !model.sidebar.path.is_empty()
}

/// Where the list's rows are: under the bar (and the folder path row),
/// above Moon's home gesture.
pub fn bounds(model: &Model, height: f32) -> (f32, f32) {
    let top = TOP_BAR + HAIRLINE + if in_folder(model) { HEADER_ROW } else { 0.0 };
    (top, height - GESTURE_RESERVE)
}

pub(crate) fn list_top(model: &Model) -> f32 {
    bounds(model, 0.0).0 + theme::S1
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

/// "now", "5 min. ago", "2 hr. ago", "3 days ago", "1 wk. ago", else a date.
pub fn relative_time(updated_ms: f64, now_ms: f64) -> String {
    let seconds = ((now_ms - updated_ms) / 1000.0).max(0.0) as i64;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{} min. ago", seconds / 60),
        3600..86_400 => format!("{} hr. ago", seconds / 3600),
        86_400..604_800 => match seconds / 86_400 {
            1 => "yesterday".into(),
            days => format!("{days} days ago"),
        },
        _ => format!("{} wk. ago", seconds / 604_800),
    }
}

fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The list's rows that fall in rows `clip` (within the canvas's clip),
/// scrolled to `offset`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_list_rows(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    rows: &[Row],
    pressed: Option<&Action>,
    clip: (f32, f32),
    offset: f32,
    now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let list_top = list_top(model);
    let outer = canvas.clip();
    let clip = (clip.0.max(outer.1 as f32), clip.1.min(outer.3 as f32));
    if clip.0 >= clip.1 {
        return;
    }
    canvas.set_clip(outer.0, clip.0 as i32, outer.2, clip.1 as i32);
    let now_ms = crate::clock::unix_now() as f64 * 1000.0;
    let x = LIST_PAD;
    let right = W - LIST_PAD;
    for (index, row) in rows.iter().enumerate() {
        let y = list_top + index as f32 * ROW_H - offset;
        if y + ROW_H < clip.0 || y > clip.1 {
            continue;
        }
        let action = row_action(row);
        let background = if pressed == Some(&action) {
            Some(c.row_pressed)
        } else if view.lifted.as_ref() == Some(&action) {
            Some(c.row_lifted)
        } else {
            None
        };
        if let Some(background) = background {
            widgets::fill(canvas, RectF::new(0.0, y, W, ROW_H), background);
        }
        match row {
            Row::Folder { name, count, .. } => {
                icons::draw(
                    canvas,
                    Icon::ChevronRight,
                    x + 20.0,
                    y + ROW_H / 2.0,
                    39.0,
                    p.muted,
                    1.0,
                );
                let icon = RectF::new(
                    x + 60.0,
                    y + (ROW_H - FOLDER_ICON) / 2.0,
                    FOLDER_ICON,
                    FOLDER_ICON,
                );
                widgets::boxed(canvas, icon, c.folder_icon_bg, c.folder_icon_border);
                let (cx, cy) = icon.center();
                icons::draw(
                    canvas,
                    Icon::FolderFill,
                    cx,
                    cy,
                    47.0,
                    p.secondary_accent,
                    1.0,
                );
                let text_x = x + 172.0;
                let name_ty = T_ROW.weight(crate::text::Weight::MonoSemiBold);
                let name = widgets::fit(name, name_ty, right - text_x);
                widgets::text(canvas, &name, text_x, y + 72.0, name_ty, p.text, 1.0);
                let meta_ty = T_META.tracked(1.2);
                let meta = format!("{} chats", thousands(*count as u64)).to_uppercase();
                widgets::text(canvas, &meta, text_x, y + 124.0, meta_ty, p.muted, 1.0);
            }
            Row::Conversation(id) => {
                let Some(summary) = model.sidebar.conversations.get(id) else {
                    continue;
                };
                let mut title_x = x;
                if summary.marked {
                    icons::draw(canvas, Icon::Star, x + 18.0, y + 60.0, 36.0, p.warning, 1.0);
                    title_x += 56.0;
                }
                // Trailing: the activity dot.
                let dot = if summary.streaming {
                    Some((p.accent, anim::pulse(now, 1600)))
                } else if summary.unread {
                    Some((p.success, 1.0))
                } else {
                    None
                };
                let trailing = if dot.is_some() { 60.0 } else { 0.0 };
                if let Some((color, alpha)) = dot {
                    let bg = background.unwrap_or(p.sidebar_background);
                    widgets::activity_dot(canvas, right - 12.5, y + 60.0, color, bg, alpha);
                }
                let title =
                    widgets::fit(summary.display_title(), T_ROW, right - trailing - title_x);
                widgets::text(canvas, &title, title_x, y + 72.0, T_ROW, p.text, 1.0);
                let when = relative_time(summary.updated_at, now_ms);
                let meta = if summary.message_count > 0 {
                    format!("{when} · {} messages", thousands(summary.message_count))
                } else {
                    when
                };
                let meta = widgets::fit(&meta, T_META, right - x);
                widgets::text(canvas, &meta, x, y + 124.0, T_META, p.muted, 1.0);
            }
        }
    }
    canvas.set_clip(outer.0, outer.1, outer.2, outer.3);
}

fn row_action(row: &Row) -> Action {
    match row {
        Row::Folder { id, .. } => Action::List(sidebar::Act::OpenFolder(id.clone())),
        Row::Conversation(id) => Action::Nav(Nav::OpenConversation(id.clone())),
    }
}

/// The header's control at (`x`, `y`).
fn header_hit(x: f32, y: f32) -> Option<Action> {
    if y >= TOP_BAR {
        return None;
    }
    let near = |cx: f32| (x - cx).abs() < 60.0;
    if near(PLUS_X) {
        Some(Action::Nav(Nav::NewConversation))
    } else if near(FOLDER_PLUS_X) {
        Some(Action::List(sidebar::Act::NewFolder))
    } else if near(MORE_X) {
        Some(Action::List(sidebar::Act::Menu(widgets::target(
            MORE_X, ROW_Y,
        ))))
    } else if SEARCH.contains(x, y) || x < SEARCH.x + SEARCH.w {
        Some(Action::List(sidebar::Act::Search))
    } else {
        None
    }
}

fn header_owns(action: &Action) -> bool {
    matches!(
        action,
        Action::Nav(Nav::NewConversation)
            | Action::List(sidebar::Act::NewFolder | sidebar::Act::Menu(_) | sidebar::Act::Search)
            | Action::List(sidebar::Act::Up)
    )
}

/// The top bar: the search field, ⋯, new folder, new chat; inside a folder,
/// the path row under it.
fn draw_header(canvas: &mut Canvas, model: &Model, pressed: Option<&Action>) {
    let bottom = bounds(model, 0.0).0;
    if !widgets::shows(canvas, 0.0, bottom) {
        return;
    }
    let p = theme::p();
    let c = theme::c();
    widgets::top_bar(canvas, p.sidebar_background);
    widgets::page_rail(canvas, RAIL_LIST);
    widgets::boxed(canvas, SEARCH, c.search_box, c.search_field_border);
    let search_down = pressed == Some(&Action::List(sidebar::Act::Search));
    let alpha = if search_down { 0.5 } else { 1.0 };
    icons::draw(
        canvas,
        Icon::Search,
        SEARCH.x + 49.0,
        ROW_Y,
        44.0,
        p.muted,
        alpha,
    );
    let placeholder = T_CAPTION.tracked(2.2);
    widgets::text(
        canvas,
        "SEARCH CHATS",
        SEARCH.x + 97.0,
        placeholder.baseline(ROW_Y - placeholder.line() / 2.0),
        placeholder,
        p.muted,
        alpha,
    );
    let down = |action: &Action| pressed == Some(action);
    widgets::ellipsis(
        canvas,
        MORE_X,
        ROW_Y,
        p.text,
        if matches!(pressed, Some(Action::List(sidebar::Act::Menu(_)))) {
            0.5
        } else {
            1.0
        },
    );
    widgets::icon_button(
        canvas,
        Icon::FolderPlus,
        FOLDER_PLUS_X,
        ROW_Y,
        60.0,
        p.text,
        down(&Action::List(sidebar::Act::NewFolder)),
    );
    widgets::icon_button(
        canvas,
        Icon::Plus,
        PLUS_X,
        ROW_Y,
        60.0,
        p.text,
        down(&Action::Nav(Nav::NewConversation)),
    );
    if in_folder(model) {
        let top = TOP_BAR + HAIRLINE;
        let row = RectF::new(0.0, top, W, HEADER_ROW);
        let up_down = down(&Action::List(sidebar::Act::Up));
        widgets::fill(
            canvas,
            row,
            if up_down {
                c.row_pressed
            } else {
                p.sidebar_background
            },
        );
        icons::draw(
            canvas,
            Icon::ChevronLeft,
            LIST_PAD + 20.0,
            top + HEADER_ROW / 2.0,
            39.0,
            p.muted,
            1.0,
        );
        let path: Vec<&str> = model
            .sidebar
            .path
            .iter()
            .map(|id| {
                model
                    .sidebar
                    .folders
                    .get(id)
                    .map_or("Folder", |folder| folder.name.as_str())
            })
            .collect();
        let label = widgets::fit(&path.join(" / "), T_EYEBROW, W - 2.0 * LIST_PAD - 60.0);
        widgets::text(
            canvas,
            &label,
            LIST_PAD + 60.0,
            T_EYEBROW.baseline(top + (HEADER_ROW - T_EYEBROW.line()) / 2.0),
            T_EYEBROW,
            p.muted,
            1.0,
        );
        widgets::hairline(canvas, 0.0, W, top + HEADER_ROW - HAIRLINE, c.rule_list);
    }
}

/// The connection pill's top (bottom centre, above the home gesture).
fn pill_top(height: f32) -> f32 {
    height - GESTURE_RESERVE - 44.0 - 88.0
}

/// What a list frame showed, so a scroll moves its rows
/// (`draw_list_update`).
#[derive(Clone, PartialEq)]
pub struct ListFrame {
    pub(crate) bounds: (i32, i32),
    pub(crate) offset: f32,
    pub(crate) key: u64,
    pub(crate) pressed: Option<Action>,
    pub(crate) lifted: Option<Action>,
    /// Rows that animate (activity dots), their pulse step.
    pub(crate) pulse: u32,
    /// The connection pill's spinner step, when it shows.
    pub(crate) pill: Option<u32>,
}

/// The current list frame's record.
pub fn list_frame(
    model: &Model,
    view: &ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) -> Option<ListFrame> {
    if model.page != Page::List {
        return None;
    }
    let pressed = pressed_action(pressed).cloned();
    let mut key = DefaultHasher::new();
    (
        model.sidebar.version,
        model.sidebar.loaded,
        &model.sidebar.path,
        model.offline,
        model.was_connected,
        height as i32,
        // The header's controls are drawn with the header.
        super::action_key(pressed.as_ref().filter(|action| header_owns(action))),
        // Relative times move on each minute.
        crate::clock::unix_now() / 60,
        theme::generation(),
    )
        .hash(&mut key);
    let (top, bottom) = bounds(model, height);
    let streaming = model.sidebar.conversations.values().any(|s| s.streaming);
    Some(ListFrame {
        bounds: (top as i32, bottom as i32),
        offset: view.list_scroll.value(now).round(),
        key: key.finish(),
        pressed,
        lifted: view.lifted.clone(),
        pulse: if streaming {
            anim::pulse_percent(now, 1600)
        } else {
            0
        },
        pill: model.offline.then(|| (anim::phase(now, 700) * 60.0) as u32),
    })
}

/// Asks for frames while something on the list animates.
pub fn animate(model: &Model, now: Instant, animator: &mut Animator) {
    if model.offline || model.sidebar.conversations.values().any(|s| s.streaming) {
        animator.slow_frame(now);
    }
}

/// The rows of row `action` on screen.
fn row_rows(model: &mut Model, frame: &ListFrame, action: &Action) -> Option<(i32, i32)> {
    let folder = model.current_folder().map(str::to_owned);
    let index = model
        .sidebar
        .rows(folder.as_deref())
        .iter()
        .position(|row| &row_action(row) == action)?;
    let y = list_top(model) + index as f32 * ROW_H - frame.offset;
    let (top, bottom) = frame.bounds;
    let strip = ((y as i32).max(top), ((y + ROW_H) as i32 + 1).min(bottom));
    (strip.0 < strip.1).then_some(strip)
}

/// The list frame on `canvas` (drawn for `old`) turned into `new`'s: rows
/// moved by a scroll and the uncovered strip drawn, or a pressed, lifted or
/// pulsing row drawn again. None when more changed.
pub fn draw_list_update(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    old: &ListFrame,
    new: &ListFrame,
    now: Instant,
) -> Option<FrameDamage> {
    if old.key != new.key || old.bounds != new.bounds {
        return None;
    }
    let (top, bottom) = new.bounds;
    let shift = (new.offset - old.offset) as i32;
    let mut damage = FrameDamage::default();
    let mut redraw: Vec<(i32, i32)> = Vec::new();
    if shift != 0 {
        if shift.abs() > (bottom - top) / 2 {
            return None;
        }
        let width = canvas.width as usize;
        if shift > 0 {
            canvas.pixels.copy_within(
                (top + shift) as usize * width..bottom as usize * width,
                top as usize * width,
            );
            redraw.push((bottom - shift, bottom));
        } else {
            canvas.pixels.copy_within(
                top as usize * width..(bottom + shift) as usize * width,
                (top - shift) as usize * width,
            );
            redraw.push((top, top - shift));
        }
        damage.moved = Some((top, bottom, -shift));
    }
    for action in [&old.pressed, &new.pressed, &old.lifted, &new.lifted]
        .into_iter()
        .flatten()
    {
        let changed = old.pressed != new.pressed || old.lifted != new.lifted;
        if changed && let Some(rows) = row_rows(model, new, action) {
            redraw.push(rows);
        }
    }
    // Pulsing dots: the rows of conversations that stream.
    if old.pulse != new.pulse {
        let folder = model.current_folder().map(str::to_owned);
        let rows = model.sidebar.rows(folder.as_deref()).to_vec();
        let streaming: Vec<Action> = rows
            .iter()
            .filter(|row| match row {
                Row::Conversation(id) => model
                    .sidebar
                    .conversations
                    .get(id)
                    .is_some_and(|s| s.streaming),
                Row::Folder { .. } => false,
            })
            .map(row_action)
            .collect();
        for action in streaming {
            if let Some(rows) = row_rows(model, new, &action) {
                redraw.push(rows);
            }
        }
    }
    let pill_rows = |height: i32| {
        let top = pill_top(height as f32) as i32 - 2;
        (top, top + 92)
    };
    if new.pill.is_some() && (old.pill != new.pill || shift != 0) {
        redraw.push(pill_rows(canvas.height));
    }
    let merged = super::frame::merge(redraw);
    let pressed = new.pressed.clone().map(Hit::Action);
    for &(y0, y1) in &merged {
        canvas.set_clip(0, y0, canvas.width, y1);
        draw_list(
            canvas,
            model,
            view,
            pressed.as_ref(),
            canvas.height as f32,
            now,
        );
        canvas.reset_clip();
    }
    damage.rows = merged;
    Some(damage)
}

pub(crate) fn draw_list(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    let p = theme::p();
    let pressed = pressed_action(pressed).cloned();
    widgets::fill(
        canvas,
        RectF::new(0.0, 0.0, W, height),
        p.sidebar_background,
    );
    let (top, bottom) = bounds(model, height);
    let folder = model.current_folder().map(str::to_owned);
    let rows: Vec<Row> = model.sidebar.rows(folder.as_deref()).to_vec();
    // Whole pixels, so a scrolled frame equals a shifted one.
    let offset = view.list_scroll.value(now).round();
    draw_list_rows(
        canvas,
        model,
        view,
        &rows,
        pressed.as_ref(),
        (top, bottom),
        offset,
        now,
    );
    if rows.is_empty() {
        let area = RectF::new(0.0, top, W, bottom - top);
        if !model.sidebar.loaded {
            if model.connected {
                widgets::empty_state(
                    canvas,
                    area,
                    "",
                    "Loading chats…",
                    None,
                    p.sidebar_background,
                );
            }
        } else if folder.is_some() {
            widgets::empty_state(canvas, area, "Empty folder", "", None, p.sidebar_background);
        } else {
            widgets::empty_state(
                canvas,
                area,
                "No conversations",
                "",
                Some(
                    &Button::new("New chat", ButtonKind::Primary)
                        .pressed(pressed == Some(Action::Nav(Nav::NewConversation))),
                ),
                p.sidebar_background,
            );
        }
    }
    draw_header(canvas, model, pressed.as_ref());
    if model.offline {
        let message = if model.was_connected {
            "Reconnecting…"
        } else {
            "Connecting…"
        };
        widgets::pill(
            canvas,
            W / 2.0,
            pill_top(height),
            message,
            true,
            p.sidebar_background,
            now,
        );
    }
}

/// The list's target at (`x`, `y`).
pub fn hit(
    model: &mut Model,
    view: &mut ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Hit> {
    if y < TOP_BAR {
        return header_hit(x, y).map(Hit::Action);
    }
    let (top, bottom) = bounds(model, height);
    if y < top {
        return in_folder(model).then_some(Hit::Action(Action::List(sidebar::Act::Up)));
    }
    if y >= bottom {
        return None;
    }
    let offset = view.list_scroll.value(now);
    let index = ((y - list_top(model) + offset) / ROW_H).floor();
    if index < 0.0 {
        return None;
    }
    let folder = model.current_folder().map(str::to_owned);
    let loaded = model.sidebar.loaded;
    let rows = model.sidebar.rows(folder.as_deref());
    match rows.get(index as usize) {
        Some(row) => Some(Hit::Action(row_action(row))),
        None if rows.is_empty() && loaded && folder.is_none() => {
            Some(Hit::Action(Action::Nav(Nav::NewConversation)))
        }
        None => None,
    }
}

/// The row a press held at (`x`, `y`) lifts.
pub fn liftable(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Action> {
    let (top, bottom) = bounds(model, height);
    if y < top || y >= bottom {
        return None;
    }
    let _ = x;
    let offset = view.list_scroll.value(now);
    let index = ((y - list_top(model) + offset) / ROW_H).floor();
    if index < 0.0 {
        return None;
    }
    let folder = model.current_folder().map(str::to_owned);
    match model.sidebar.rows(folder.as_deref()).get(index as usize)? {
        row @ Row::Conversation(_) => Some(row_action(row)),
        Row::Folder { .. } => None,
    }
}

/// What a long press at (`x`, `y`) opens: a conversation row's menu,
/// anchored to the row.
pub fn long_press(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Action> {
    let Action::Nav(Nav::OpenConversation(id)) = liftable(model, view, height, x, y, now)? else {
        return None;
    };
    let offset = view.list_scroll.value(now);
    let index = ((y - list_top(model) + offset) / ROW_H).floor();
    let row_y = list_top(model) + index * ROW_H - offset;
    Some(Action::List(sidebar::Act::RowMenu {
        id,
        rect: RectF::new(0.0, row_y.round(), W, ROW_H),
    }))
}
