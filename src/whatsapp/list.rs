//! The chat list (and the archive): WhatsApp's title bar, the search field
//! and filter chips that scroll with the list, the "Archived" row, a row per
//! chat (picture, name, time, the last message with its ticks, unread
//! count, pinned and muted marks), and the tab bar. Scrolling draws only
//! the rows that come into view.

use std::time::Instant;

use super::band::{self, Band};
use super::icons::{self, Glyph};
use super::{Page, WhatsApp, avatar, theme, time};
use crate::comms::Scroll;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::exocortex::ui::icons::Icon;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::social::proto::Chat;
use crate::social::shell::{Cx, DrawCx};
use crate::text::{self, Weight};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    All,
    Unread,
    Groups,
}

const FILTERS: [(Filter, &str); 3] = [
    (Filter::All, "All"),
    (Filter::Unread, "Unread"),
    (Filter::Groups, "Groups"),
];
const ARCHIVED_ROW: f32 = 150.0;
const SEARCH_TOP: f32 = 14.0;
const CHIPS_TOP: f32 = SEARCH_TOP + theme::SEARCH_H + 34.0;

pub struct ListView {
    pub folder: &'static str,
    pub scroll: Scroll,
    pub filter: Filter,
    /// Searching: the query typed so far.
    search: Option<String>,
    pressed: Option<Pressed>,
    band: Band,
    /// The rows shown (ids), for the chats' version they were made from.
    rows: Vec<String>,
    rows_for: Option<(u64, Filter, String)>,
    /// Pictures the last frames drew without (`avatar::arrived`).
    pub waiting: Vec<avatar::Waiting>,
    /// The pressed row as last drawn (a press redraws its row only).
    drawn_pressed: Option<Pressed>,
}

#[derive(Debug, Clone, PartialEq)]
enum Pressed {
    Row(String),
    Archived,
}

impl ListView {
    pub fn new(folder: &'static str) -> Self {
        let mut scroll = Scroll::default();
        scroll.smooth = true;
        Self {
            folder,
            scroll,
            filter: Filter::All,
            search: None,
            pressed: None,
            band: Band::default(),
            rows: Vec::new(),
            rows_for: None,
            waiting: Vec::new(),
            drawn_pressed: None,
        }
    }

    pub fn searching(&self) -> bool {
        self.search.is_some()
    }

    pub fn end_search(&mut self) {
        self.search = None;
        self.rows_for = None;
    }

    pub fn search_key(&mut self, key: KeyInput) {
        let Some(query) = self.search.as_mut() else {
            return;
        };
        match key {
            KeyInput::Character(c) => query.push(c),
            KeyInput::Backspace => {
                query.pop();
            }
            KeyInput::Enter => {}
        }
        self.rows_for = None;
        self.scroll.reset();
    }

    fn archive(&self) -> bool {
        self.folder == "archived"
    }

    /// The rows this list shows now.
    fn refresh_rows(&mut self, chats: &mut super::Chats) {
        chats.sort();
        let query = self.search.clone().unwrap_or_default().to_lowercase();
        let key = (chats.version, self.filter, query.clone());
        if self.rows_for.as_ref() == Some(&key) {
            return;
        }
        let source = if self.archive() {
            &chats.archived
        } else {
            &chats.inbox
        };
        self.rows = source
            .iter()
            .filter(|id| {
                let Some(chat) = chats.get(id) else {
                    return false;
                };
                let filtered = match self.filter {
                    Filter::All => true,
                    Filter::Unread => chat.unread > 0,
                    Filter::Groups => chat.is_group(),
                };
                filtered
                    && (query.is_empty()
                        || chat.title.to_lowercase().contains(&query)
                        || chat
                            .last
                            .as_ref()
                            .is_some_and(|last| last.text.to_lowercase().contains(&query)))
            })
            .cloned()
            .collect();
        self.rows_for = Some(key);
    }

    /// Where the rows start in the content (under the search field, the
    /// chips and the archived row).
    fn rows_top(&self, archived: usize) -> f32 {
        if self.archive() || self.searching() {
            return 20.0;
        }
        let mut top = CHIPS_TOP + theme::CHIPS_H + 26.0;
        if archived > 0 && self.filter == Filter::All {
            top += ARCHIVED_ROW;
        }
        top
    }

    fn content_height(&self, archived: usize) -> f32 {
        self.rows_top(archived) + self.rows.len() as f32 * theme::ROW + 60.0
    }
}

fn band_of(list: &ListView, height: f32) -> (f32, f32) {
    let top = theme::TOP + theme::APP_BAR;
    let bottom = if list.archive() || list.searching() {
        height
    } else {
        height - theme::TAB_BAR
    };
    (top, bottom)
}

fn max_scroll(list: &ListView, archived: usize, height: f32) -> f32 {
    let (top, bottom) = band_of(list, height);
    (list.content_height(archived) - (bottom - top)).max(0.0)
}

pub fn tick(list: &mut ListView, chats: &mut super::Chats, cx: &mut Cx) {
    chats.sort();
    let max = max_scroll(list, chats.archived.len(), cx.height);
    if list.scroll.step(max, cx.now) {
        cx.animate();
    }
}

fn view(app: &mut WhatsApp) -> &mut ListView {
    if app.page == Page::Archived {
        &mut app.archived
    } else {
        &mut app.list
    }
}

pub fn gesture(app: &mut WhatsApp, cx: &mut Cx, gesture: Gesture) {
    let height = cx.height;
    let now = cx.now;
    // (Sorted first: the archive's size moves the rows.)
    app.chats.sort();
    let archived_count = app.chats.archived.len();
    let page = app.page;
    {
        let list = view(app);
        let max = max_scroll(list, archived_count, height);
        match gesture {
            Gesture::DragStart {
                axis: Axis::Vertical,
                y,
                start_y,
                ..
            } => {
                list.pressed = None;
                list.scroll.begin(start_y, now);
                list.scroll.moved(y, max, now);
                cx.animate();
                return;
            }
            Gesture::Drag {
                axis: Axis::Vertical,
                y,
                ..
            } => {
                list.scroll.moved(y, max, now);
                cx.animate();
                return;
            }
            Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            } => {
                list.scroll.end(max, now);
                cx.animate();
                return;
            }
            Gesture::DragEnd {
                axis: Axis::Horizontal,
                x,
                ..
            } if page == Page::Archived && x > cx.width * 0.4 => {
                app.back(cx);
                return;
            }
            _ => {}
        }
    }
    match gesture {
        Gesture::Down { x, y } => {
            let hit = hit(app, x, y, height, now);
            if let Some(Hit::Row(id)) = &hit {
                app.prefetch(cx, id);
            }
            let list = view(app);
            list.pressed = match hit {
                Some(Hit::Row(id)) => Some(Pressed::Row(id)),
                Some(Hit::Archived) => Some(Pressed::Archived),
                _ => None,
            };
            if list.pressed.is_some() {
                cx.redraw();
            }
        }
        Gesture::Cancel | Gesture::LongPressEnd { .. } => {
            view(app).pressed = None;
            cx.redraw();
        }
        Gesture::Tap { x, y } => {
            view(app).pressed = None;
            let hit = hit(app, x, y, height, now);
            match hit {
                Some(Hit::Row(id)) => app.open_chat(cx, &id),
                Some(Hit::Archived) => {
                    app.archived.scroll.reset();
                    app.go(cx, Page::Archived);
                }
                Some(Hit::Back) => app.back(cx),
                Some(Hit::More) | Some(Hit::Tab(Page::Settings)) => app.go(cx, Page::Settings),
                Some(Hit::Tab(_)) => {
                    let list = view(app);
                    list.scroll.reset();
                    app.full = true;
                }
                Some(Hit::Search) => {
                    let list = view(app);
                    list.search = Some(String::new());
                    list.rows_for = None;
                    list.scroll.reset();
                    cx.show_keyboard(crate::control::KeyboardPurpose::Search);
                    app.full = true;
                }
                Some(Hit::Filter(filter)) => {
                    let list = view(app);
                    list.filter = filter;
                    list.rows_for = None;
                    app.full = true;
                }
                None => {}
            }
            cx.redraw();
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Hit {
    Row(String),
    Archived,
    Back,
    More,
    Search,
    Filter(Filter),
    Tab(Page),
}

fn hit(app: &mut WhatsApp, x: f32, y: f32, height: f32, now: Instant) -> Option<Hit> {
    // (Sorted first: the archive's size moves the rows.)
    app.chats.sort();
    let archived_count = app.chats.archived.len();
    let page = app.page;
    let list = if page == Page::Archived {
        &mut app.archived
    } else {
        &mut app.list
    };
    list.refresh_rows(&mut app.chats);
    let (top, bottom) = band_of(list, height);
    if y < top {
        if y < theme::TOP - 20.0 {
            return None;
        }
        if page == Page::Archived || list.searching() {
            return (x < 150.0).then_some(Hit::Back);
        }
        return (x > app_width() - 150.0).then_some(Hit::More);
    }
    if y >= bottom {
        if page == Page::List && !list.searching() {
            return Some(Hit::Tab(if x < app_width() / 2.0 {
                Page::List
            } else {
                Page::Settings
            }));
        }
        return None;
    }
    let content_y = y - top + list.scroll.value(now).round();
    if !list.archive() && !list.searching() {
        if (SEARCH_TOP..SEARCH_TOP + theme::SEARCH_H).contains(&content_y) {
            return Some(Hit::Search);
        }
        if (CHIPS_TOP..CHIPS_TOP + theme::CHIPS_H).contains(&content_y) {
            let mut cx = theme::MARGIN;
            for (filter, label) in FILTERS {
                let w = chip_width(label, filter, &app.chats);
                if x >= cx && x < cx + w {
                    return Some(Hit::Filter(filter));
                }
                cx += w + 22.0;
            }
            return None;
        }
        let rows_top = list.rows_top(archived_count);
        if archived_count > 0
            && list.filter == Filter::All
            && (rows_top - ARCHIVED_ROW..rows_top).contains(&content_y)
        {
            return Some(Hit::Archived);
        }
    }
    let index = ((content_y - list.rows_top(archived_count)) / theme::ROW).floor();
    if index < 0.0 {
        return None;
    }
    list.rows.get(index as usize).cloned().map(Hit::Row)
}

fn app_width() -> f32 {
    f32::from(crate::REQUESTED_WIDTH)
}

fn chip_label(label: &str, filter: Filter, chats: &super::Chats) -> String {
    if filter == Filter::Unread {
        let count = chats
            .inbox
            .iter()
            .filter(|id| chats.get(id).is_some_and(|chat| chat.unread > 0))
            .count();
        if count > 0 {
            return format!("{label} {count}");
        }
    }
    label.to_owned()
}

fn chip_width(label: &str, filter: Filter, chats: &super::Chats) -> f32 {
    text::measure(
        &chip_label(label, filter, chats),
        Weight::Medium,
        theme::T_CHIP,
    ) + 70.0
}

pub fn draw(app: &mut WhatsApp, canvas: &mut Canvas, cx: &mut DrawCx, full: bool, archive: bool) {
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    let now = cx.now;
    // (Sorted first: the archive's size moves the rows.)
    app.chats.sort();
    let archived_count = app.chats.archived.len();
    let status = app.status.clone();
    let page = app.page;
    {
        let list = if archive {
            &mut app.archived
        } else {
            &mut app.list
        };
        list.refresh_rows(&mut app.chats);
    }
    let list = if archive { &app.archived } else { &app.list };
    let (top, bottom) = band_of(list, height);
    let offset = list.scroll.value(now).round();
    let searching = list.searching();

    if full {
        canvas.clear(theme::BG);
        draw_app_bar(canvas, &status, list, archive, width);
        if !archive && !searching {
            draw_tabs(canvas, width, height);
        }
    }
    let key = band::key_of(&[
        app.chats.version,
        list.filter as u64,
        hash_str(list.search.as_deref().unwrap_or("")),
        width as u64,
        height as u64,
        crate::control::clock_24h() as u64,
        u64::from(page == Page::Archived),
    ]);
    let rows_top = list.rows_top(archived_count);
    let filter = list.filter;
    let pressed = list.pressed.clone();
    let chats = &app.chats;
    let rounded = &mut app.rounded;
    let view = if archive {
        &mut app.archived
    } else {
        &mut app.list
    };
    // Lent to the drawing, given back below.
    let rows = std::mem::take(&mut view.rows);
    let ready = avatar::arrived(cx.media, &mut view.waiting);
    let translate = top - offset;
    let mut pics = avatar::Pics::new(&mut *cx.media, &mut view.waiting, translate);
    let mut draw_rows = |canvas: &mut Canvas, (y0, y1): (i32, i32)| {
        canvas.fill_rect(0, y0, canvas.width, y1 - y0, theme::BG, 1.0);
        let base = translate;
        if !archive && !searching {
            draw_search_pill(canvas, base + SEARCH_TOP, width);
            draw_chips(canvas, base + CHIPS_TOP, filter, chats);
            if archived_count > 0 && filter == Filter::All {
                let y = base + rows_top - ARCHIVED_ROW;
                if y + ARCHIVED_ROW > y0 as f32 && y < y1 as f32 {
                    draw_archived_row(canvas, y, chats, pressed == Some(Pressed::Archived), width);
                }
            }
        }
        let first = (((y0 as f32 - base - rows_top) / theme::ROW)
            .floor()
            .max(0.0)) as usize;
        for (index, id) in rows.iter().enumerate().skip(first) {
            let y = base + rows_top + index as f32 * theme::ROW;
            if y > y1 as f32 {
                break;
            }
            let Some(chat) = chats.get(id) else {
                continue;
            };
            let is_pressed = pressed.as_ref() == Some(&Pressed::Row(id.clone()));
            pics.span = (y, y + theme::ROW);
            draw_row(canvas, &mut pics, rounded, chat, y, width, is_pressed, now);
        }
        if rows.is_empty() {
            let note = if searching {
                "No chats found"
            } else if filter == Filter::Unread {
                "No unread chats"
            } else if filter == Filter::Groups {
                "No groups"
            } else if !status.online() && chats.by_id.is_empty() {
                "Loading your chats…"
            } else {
                "No chats yet"
            };
            text::draw_centered(
                canvas,
                note,
                width / 2.0,
                base + rows_top + 160.0,
                Weight::Regular,
                theme::T_PREVIEW,
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
    // A press or its end: the rows it changes.
    let mut again = ready;
    if view.drawn_pressed != pressed {
        if drawn.is_some() {
            for target in [&view.drawn_pressed, &pressed] {
                let index = match target {
                    Some(Pressed::Row(id)) => rows.iter().position(|row| row == id),
                    _ => None,
                };
                let row_y = match (target, index) {
                    (Some(Pressed::Archived), _) => Some(rows_top - ARCHIVED_ROW),
                    (_, Some(index)) => Some(rows_top + index as f32 * theme::ROW),
                    _ => None,
                };
                if let Some(row_y) = row_y {
                    let h = if matches!(target, Some(Pressed::Archived)) {
                        ARCHIVED_ROW
                    } else {
                        theme::ROW
                    };
                    again.push((row_y, row_y + h));
                }
            }
        }
        view.drawn_pressed = pressed.clone();
    }
    // Pictures that came, a press: only their rows again.
    if drawn.is_some() {
        for (c0, c1) in again {
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
    view.rows = rows;
    // Prefetch the pictures just below and above.
    let list = if archive { &app.archived } else { &app.list };
    let visible_first = ((offset - rows_top) / theme::ROW).floor().max(0.0) as usize;
    let visible = ((bottom - top) / theme::ROW).ceil() as usize + 1;
    for id in list
        .rows
        .iter()
        .skip(visible_first.saturating_sub(3))
        .take(visible + 6)
    {
        if let Some(avatar) = app.chats.get(id).and_then(|chat| chat.avatar.as_ref()) {
            let px = theme::AVATAR.round() as u32;
            let _ = cx.media.get(
                &avatar.reference,
                px,
                px,
                crate::social::media::Fit::Cover,
                now,
            );
        }
    }
}

fn hash_str(text_in: &str) -> u64 {
    band::key_of(&text_in.bytes().map(u64::from).collect::<Vec<_>>())
}

fn draw_app_bar(
    canvas: &mut Canvas,
    status: &crate::social::proto::Status,
    list: &ListView,
    archive: bool,
    width: f32,
) {
    canvas.fill_rect(
        0,
        0,
        canvas.width,
        (theme::TOP + theme::APP_BAR) as i32,
        theme::BAR,
        1.0,
    );
    let cy = theme::TOP + theme::APP_BAR / 2.0;
    if archive {
        icons::draw(canvas, Glyph::Back, 70.0, cy, 66.0, theme::TEXT, 1.0);
        text::draw(
            canvas,
            "Archived",
            150.0,
            text::centered_baseline(cy, Weight::Medium, theme::T_HEADER),
            Weight::Medium,
            theme::T_HEADER,
            theme::TEXT,
            1.0,
        );
        return;
    }
    if let Some(query) = &list.search {
        icons::draw(canvas, Glyph::Back, 70.0, cy, 66.0, theme::TEXT, 1.0);
        let baseline = text::centered_baseline(cy, Weight::Regular, theme::T_TITLE);
        if query.is_empty() {
            text::draw(
                canvas,
                "Search…",
                150.0,
                baseline,
                Weight::Regular,
                theme::T_TITLE,
                theme::SECONDARY,
                1.0,
            );
            canvas.fill_rect(150, (cy - 30.0) as i32, 3, 60, theme::GREEN, 1.0);
        } else {
            let shown = text::fit(query, Weight::Regular, theme::T_TITLE, width - 220.0);
            text::draw(
                canvas,
                &shown,
                150.0,
                baseline,
                Weight::Regular,
                theme::T_TITLE,
                theme::TEXT,
                1.0,
            );
            let w = text::measure(&shown, Weight::Regular, theme::T_TITLE);
            canvas.fill_rect(
                (152.0 + w) as i32,
                (cy - 30.0) as i32,
                3,
                60,
                theme::GREEN,
                1.0,
            );
        }
        return;
    }
    let (title, weight, size, color) = match status.state.as_str() {
        "online" => (
            "WhatsApp",
            Weight::SemiBold,
            theme::T_APP_TITLE,
            theme::TEXT,
        ),
        "offline" => (
            "Waiting for network…",
            Weight::Medium,
            theme::T_HEADER,
            theme::TEXT,
        ),
        "error" => (
            "WhatsApp",
            Weight::SemiBold,
            theme::T_APP_TITLE,
            theme::TEXT,
        ),
        _ => ("Connecting…", Weight::Medium, theme::T_HEADER, theme::TEXT),
    };
    text::draw(
        canvas,
        title,
        theme::MARGIN,
        text::centered_baseline(cy, weight, size),
        weight,
        size,
        color,
        1.0,
    );
    icons::draw(
        canvas,
        Glyph::More,
        width - 70.0,
        cy,
        62.0,
        theme::TEXT,
        1.0,
    );
    if status.state == "error"
        && let Some(detail) = &status.detail
    {
        let detail = text::fit(
            detail,
            Weight::Regular,
            30.0,
            width - 2.0 * theme::MARGIN - 120.0,
        );
        text::draw(
            canvas,
            &detail,
            theme::MARGIN,
            cy + 58.0,
            Weight::Regular,
            30.0,
            theme::DANGER,
            1.0,
        );
    }
}

fn draw_search_pill(canvas: &mut Canvas, y: f32, width: f32) {
    let rect = RectF {
        x: theme::MARGIN,
        y,
        w: width - 2.0 * theme::MARGIN,
        h: theme::SEARCH_H,
    };
    canvas.fill_rrect(rect, theme::SEARCH_H / 2.0, theme::SURFACE, 1.0);
    icons::shared(
        canvas,
        Icon::Search,
        rect.x + 75.0,
        y + theme::SEARCH_H / 2.0,
        54.0,
        theme::SECONDARY,
    );
    text::draw(
        canvas,
        "Search",
        rect.x + 140.0,
        text::centered_baseline(y + theme::SEARCH_H / 2.0, Weight::Regular, theme::T_PREVIEW),
        Weight::Regular,
        theme::T_PREVIEW,
        theme::SECONDARY,
        1.0,
    );
}

fn draw_chips(canvas: &mut Canvas, y: f32, active: Filter, chats: &super::Chats) {
    let mut x = theme::MARGIN;
    for (filter, label) in FILTERS {
        let label = chip_label(label, filter, chats);
        let w = text::measure(&label, Weight::Medium, theme::T_CHIP) + 70.0;
        let rect = RectF {
            x,
            y,
            w,
            h: theme::CHIPS_H,
        };
        let selected = filter == active;
        let (fill, color) = if selected {
            (theme::GREEN_DARK, theme::GREEN_TEXT)
        } else {
            (theme::SURFACE, theme::SECONDARY)
        };
        canvas.fill_rrect(rect, theme::CHIPS_H / 2.0, fill, 1.0);
        text::draw_centered(
            canvas,
            &label,
            x + w / 2.0,
            text::centered_baseline(y + theme::CHIPS_H / 2.0, Weight::Medium, theme::T_CHIP),
            Weight::Medium,
            theme::T_CHIP,
            color,
            1.0,
        );
        x += w + 22.0;
    }
}

fn draw_archived_row(canvas: &mut Canvas, y: f32, chats: &super::Chats, pressed: bool, width: f32) {
    if pressed {
        canvas.fill_rect(
            0,
            y as i32,
            canvas.width,
            ARCHIVED_ROW as i32,
            theme::PRESSED,
            1.0,
        );
    }
    let cy = y + ARCHIVED_ROW / 2.0;
    icons::draw(
        canvas,
        Glyph::Archive,
        theme::MARGIN + theme::AVATAR / 2.0,
        cy,
        62.0,
        theme::SECONDARY,
        1.0,
    );
    text::draw(
        canvas,
        "Archived",
        theme::ROW_TEXT_X,
        text::centered_baseline(cy, Weight::Medium, theme::T_TITLE),
        Weight::Medium,
        theme::T_TITLE,
        theme::TEXT,
        1.0,
    );
    let unread = chats
        .archived
        .iter()
        .filter(|id| chats.get(id).is_some_and(|chat| chat.unread > 0))
        .count();
    if unread > 0 {
        text::draw_right(
            canvas,
            &unread.to_string(),
            width - theme::MARGIN,
            text::centered_baseline(cy, Weight::Medium, theme::T_TIME),
            Weight::Medium,
            theme::T_TIME,
            theme::GREEN,
            1.0,
        );
    }
}

/// The ticks for one of the account's messages, at (x, cy); returns their width.
pub fn draw_ticks(
    canvas: &mut Canvas,
    status: &str,
    x: f32,
    cy: f32,
    size: f32,
    color: crate::gfx::Rgb,
) -> f32 {
    match status {
        "pending" => {
            icons::shared(canvas, Icon::Clock, x + size * 0.45, cy, size * 0.82, color);
            size * 0.95
        }
        "sent" => {
            icons::draw(canvas, Glyph::Tick, x + size * 0.45, cy, size, color, 1.0);
            size * 0.9
        }
        "delivered" => {
            icons::draw(
                canvas,
                Glyph::DoubleTick,
                x + size * 0.55,
                cy,
                size * 1.1,
                color,
                1.0,
            );
            size * 1.15
        }
        "read" => {
            icons::draw(
                canvas,
                Glyph::DoubleTick,
                x + size * 0.55,
                cy,
                size * 1.1,
                theme::TICK_READ,
                1.0,
            );
            size * 1.15
        }
        "failed" => {
            canvas.fill_circle(x + size * 0.45, cy, size * 0.4, theme::DANGER, 1.0);
            text::draw_centered(
                canvas,
                "!",
                x + size * 0.45,
                cy + size * 0.22,
                Weight::Bold,
                size * 0.6,
                0xffffff,
                1.0,
            );
            size * 0.95
        }
        _ => 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    canvas: &mut Canvas,
    pics: &mut avatar::Pics,
    rounded: &mut avatar::Rounded,
    chat: &Chat,
    y: f32,
    width: f32,
    pressed: bool,
    now: Instant,
) {
    if pressed {
        canvas.fill_rect(
            0,
            y as i32,
            canvas.width,
            theme::ROW as i32,
            theme::PRESSED,
            1.0,
        );
    }
    let cy = y + theme::ROW / 2.0;
    avatar::draw(
        canvas,
        pics,
        rounded,
        chat.avatar.as_ref(),
        chat.is_group(),
        theme::MARGIN,
        cy - theme::AVATAR / 2.0,
        theme::AVATAR,
        now,
    );
    let x = theme::ROW_TEXT_X;
    let right = width - theme::MARGIN;
    let unread = chat.unread > 0 || chat.ext["marked_unread"].as_bool().unwrap_or(false);
    let when = time::list_time(chat.ts);
    let when_w = text::measure(&when, Weight::Regular, theme::T_TIME);
    text::draw_right(
        canvas,
        &when,
        right,
        cy - 18.0,
        Weight::Regular,
        theme::T_TIME,
        if unread {
            theme::GREEN
        } else {
            theme::SECONDARY
        },
        1.0,
    );
    let title = text::fit(
        &super::rich::emojify(&chat.title),
        Weight::Medium,
        theme::T_TITLE,
        right - x - when_w - 30.0,
    );
    text::draw(
        canvas,
        &title,
        x,
        cy - 16.0,
        Weight::Medium,
        theme::T_TITLE,
        theme::TEXT,
        1.0,
    );

    // The marks on the right of the preview: unread, muted, pinned.
    let line = cy + 50.0;
    let mut marks_right = right;
    if chat.unread > 0 {
        let label = if chat.unread > 999 {
            "999+".to_owned()
        } else {
            chat.unread.to_string()
        };
        let w = (text::measure(&label, Weight::SemiBold, 30.0) + 30.0).max(58.0);
        let rect = RectF {
            x: marks_right - w,
            y: line - 43.0,
            w,
            h: 58.0,
        };
        canvas.fill_rrect(
            rect,
            29.0,
            if chat.muted {
                theme::SECONDARY
            } else {
                theme::GREEN
            },
            1.0,
        );
        text::draw_centered(
            canvas,
            &label,
            rect.x + w / 2.0,
            line - 3.0,
            Weight::SemiBold,
            30.0,
            theme::ON_GREEN,
            1.0,
        );
        marks_right -= w + 16.0;
    } else if unread {
        canvas.fill_circle(marks_right - 22.0, line - 14.0, 22.0, theme::GREEN, 1.0);
        marks_right -= 60.0;
    }
    if chat.muted {
        icons::shared(
            canvas,
            Icon::BellOff,
            marks_right - 24.0,
            line - 14.0,
            44.0,
            theme::SECONDARY,
        );
        marks_right -= 56.0;
    }
    if chat.pinned {
        icons::shared(
            canvas,
            Icon::Pin,
            marks_right - 24.0,
            line - 14.0,
            44.0,
            theme::SECONDARY,
        );
        marks_right -= 56.0;
    }
    let mut px = x;
    let Some(last) = &chat.last else {
        return;
    };
    let status = chat.ext["status"].as_str();
    if last.mine
        && let Some(status) = status
    {
        px += draw_ticks(canvas, status, px, line - 13.0, 42.0, theme::SECONDARY) + 8.0;
    }
    let deleted = chat.ext["deleted"].as_bool().unwrap_or(false);
    if deleted {
        icons::draw(
            canvas,
            Glyph::Blocked,
            px + 20.0,
            line - 13.0,
            38.0,
            theme::SECONDARY,
            1.0,
        );
        px += 50.0;
    }
    let preview = if chat.is_group() && !last.mine && !chat.ext["notice"].as_bool().unwrap_or(false)
    {
        match &last.author {
            Some(author) if !author.is_empty() => format!("{author}: {}", last.text),
            _ => last.text.clone(),
        }
    } else {
        last.text.clone()
    };
    let preview = text::fit(
        &super::rich::emojify(&preview.replace('\n', " ")),
        Weight::Regular,
        theme::T_PREVIEW,
        marks_right - px - 16.0,
    );
    text::draw(
        canvas,
        &preview,
        px,
        line,
        Weight::Regular,
        theme::T_PREVIEW,
        theme::SECONDARY,
        1.0,
    );
}

fn draw_tabs(canvas: &mut Canvas, width: f32, height: f32) {
    let top = height - theme::TAB_BAR;
    canvas.fill_rect(
        0,
        top as i32,
        canvas.width,
        theme::TAB_BAR as i32,
        theme::BAR,
        1.0,
    );
    canvas.fill_rect(0, top as i32, canvas.width, 2, theme::DIVIDER, 1.0);
    let tabs = [
        (Glyph::Chats, "Chats", true),
        (Glyph::Settings, "Settings", false),
    ];
    let slot = width / tabs.len() as f32;
    for (index, (glyph, label, active)) in tabs.into_iter().enumerate() {
        let cx = slot * (index as f32 + 0.5);
        let icon_y = top + 62.0;
        if active {
            canvas.fill_rrect(
                RectF {
                    x: cx - 88.0,
                    y: icon_y - 44.0,
                    w: 176.0,
                    h: 88.0,
                },
                44.0,
                theme::GREEN_DARK,
                1.0,
            );
        }
        icons::draw(
            canvas,
            glyph,
            cx,
            icon_y,
            60.0,
            if active {
                theme::GREEN_TEXT
            } else {
                theme::TEXT
            },
            1.0,
        );
        text::draw_centered(
            canvas,
            label,
            cx,
            top + 152.0,
            if active {
                Weight::SemiBold
            } else {
                Weight::Medium
            },
            32.0,
            theme::TEXT,
            1.0,
        );
    }
}
