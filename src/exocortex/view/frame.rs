//! Frames: what a frame showed (`Frame`), and how the next is drawn by
//! changing only what differs (roadmap §3.9). The page's own incremental
//! paths move and redraw its rows (`draw_chat_update`, `draw_list_update`);
//! overlays move or redraw their bodies' rows; anything else changed in
//! known rows is drawn again as a band — the whole frame drawn with the
//! canvas clipped to those rows. Every partial frame equals a full redraw
//! of the same state, pixel for pixel, and changes nothing outside the
//! rows it reports (`EXOCORTEX_APP_VERIFY=1` and the tests check it).
//! Opening or closing a modal overlay, a page change or the keyboard draw
//! everything (§3.9 rule 6).

use std::time::Instant;

use crate::gfx::Canvas;

use super::super::model::{Model, Page};
use super::super::screens::settings::{self, SettingsFrame};
use super::super::theme;
use super::super::ui::overlay::{Body, Cx, Scroller};
use super::{ChatFrame, Hit, ListFrame, ViewState, chat_frame, draw_chat_update, list};

/// Rows a frame changed: rows moved inside the presented copy (`y0, y1,
/// dy`, for `Presenter::scroll`), then rows drawn anew (for
/// `Presenter::present_rows`).
#[derive(Debug, Default, PartialEq)]
pub struct FrameDamage {
    pub moved: Option<(i32, i32, i32)>,
    pub rows: Vec<(i32, i32)>,
    /// Rectangles drawn anew (`x0, y0, x1, y1`) in rows that otherwise only
    /// moved: uploaded as they are (`Presenter::present_rows_and_boxes`).
    pub boxes: Vec<(i32, i32, i32, i32)>,
}

/// The page's part of a frame's record.
#[derive(Clone)]
pub enum PageFrame {
    Chat(ChatFrame),
    List(ListFrame),
    /// Settings (E): its scroll, switches and the rest (`SettingsFrame`).
    Settings(SettingsFrame),
    /// Something drawn whole every frame (a loading chat, the connection
    /// pill).
    Volatile,
}

/// What a frame showed.
#[derive(Clone)]
pub struct Frame {
    size: (i32, i32),
    /// The theme generation it was drawn in.
    generation: u64,
    page: Page,
    page_frame: PageFrame,
    modal: Vec<(u64, Body)>,
    floating: Vec<(u64, Body)>,
}

/// The overlays' context for a canvas.
fn cx<'a>(model: &'a Model, view: &ViewState, canvas_size: (i32, i32), now: Instant) -> Cx<'a> {
    view.cx(model, canvas_size.0 as f32, canvas_size.1 as f32, now)
}

/// The record of the frame `now` would draw (laying out what it needs and
/// asking for the frames animations need).
pub fn record(
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    size: (i32, i32),
    now: Instant,
) -> Frame {
    view.check_generation();
    let height = size.1 as f32;
    let page_frame = match model.page {
        Page::Chat => chat_frame(model, view, pressed, height, now)
            .map_or(PageFrame::Volatile, PageFrame::Chat),
        Page::List => list::list_frame(model, view, pressed, height, now)
            .map_or(PageFrame::Volatile, PageFrame::List),
        Page::Settings => PageFrame::Settings(settings::settings_frame(
            model,
            view,
            super::pressed_action(pressed),
            now,
        )),
    };
    super::animate(model, view, now);
    let mut overlays = std::mem::take(&mut view.overlays);
    let (modal, floating) = {
        let cx = cx(model, view, size, now);
        overlays.layout(&cx);
        overlays.animate(&cx, &mut view.animator);
        overlays.bodies(&cx)
    };
    view.overlays = overlays;
    Frame {
        size,
        generation: theme::generation(),
        page: model.page,
        page_frame,
        modal,
        floating,
    }
}

/// Draws everything: the page, the modal overlays, the floating ones.
pub fn draw_full(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
) {
    super::draw_page(canvas, model, view, pressed, now);
    let size = (canvas.width, canvas.height);
    let overlays = std::mem::take(&mut view.overlays);
    {
        let cx = cx(model, view, size, now);
        overlays.draw_modal(canvas, &cx);
        overlays.draw_floating(canvas, &cx);
    }
    view.overlays = overlays;
}

/// Draws rows `band` of the frame again, whole: the page, the overlays.
fn redraw_band(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
    (y0, y1): (i32, i32),
) {
    canvas.set_clip(0, y0, canvas.width, y1);
    draw_full(canvas, model, view, pressed, now);
    canvas.reset_clip();
}

/// Sorted, merged row ranges.
pub fn merge(mut rows: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    rows.retain(|(y0, y1)| y1 > y0);
    rows.sort_unstable();
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (y0, y1) in rows {
        match merged.last_mut() {
            Some(last) if y0 <= last.1 => last.1 = last.1.max(y1),
            _ => merged.push((y0, y1)),
        }
    }
    merged
}

/// `rows` without `cut`.
fn subtract(rows: Vec<(i32, i32)>, cut: (i32, i32)) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for (y0, y1) in rows {
        if cut.1 <= y0 || cut.0 >= y1 {
            out.push((y0, y1));
            continue;
        }
        if y0 < cut.0 {
            out.push((y0, cut.0));
        }
        if cut.1 < y1 {
            out.push((cut.1, y1));
        }
    }
    out
}

/// Moves rows `y0..y1` of the canvas by `dy` (content at y lands at y+dy).
fn move_rows(canvas: &mut Canvas, (y0, y1): (i32, i32), dy: i32) {
    let width = canvas.width as usize;
    let (y0, y1) = (y0.max(0), y1.min(canvas.height));
    let rows = y1 - y0 - dy.abs();
    if rows <= 0 || dy == 0 {
        return;
    }
    let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
    canvas.pixels.copy_within(
        src as usize * width..(src + rows) as usize * width,
        dst as usize * width,
    );
}

/// Rows `y0..y1` moved by `dy` (`Presenter::scroll`).
type Move = (i32, i32, i32);

/// How the top modal overlay's body changed: moved rows (the window, the
/// shift) and rows to draw again.
fn overlay_change(old: &Body, new: &Body, height: i32) -> (Option<Move>, Vec<(i32, i32)>) {
    let everything = vec![old.rows, new.rows];
    if old == new {
        return (None, Vec::new());
    }
    if old.key != new.key {
        return (None, everything);
    }
    // Only its content scrolled: move the scroller's rows.
    if old.rows == new.rows
        && old.solid == new.solid
        && let (Some(a), Some(b)) = (old.scroller, new.scroller)
        && (a.y0, a.y1) == (b.y0, b.y1)
    {
        let shift = b.offset - a.offset;
        let height = b.y1 - b.y0;
        if shift.abs() < height / 2 {
            let exposed = if shift > 0 {
                (b.y1 - shift, b.y1)
            } else {
                (b.y0, b.y0 - shift)
            };
            return (Some((b.y0, b.y1, -shift)), vec![exposed]);
        }
        return (None, vec![(b.y0, b.y1)]);
    }
    // Only moved (a sheet sliding, its panel perhaps partly below the
    // window): move its solid rows; what the move does not bring is drawn.
    if let (Some(a), Some(b)) = (old.solid, new.solid)
        && a.1 - a.0 == b.1 - b.0
        && scrolled_alike(old.scroller, new.scroller)
    {
        let dy = b.0 - a.0;
        let on_canvas = |(y0, y1): (i32, i32)| (y0.clamp(0, height), y1.clamp(0, height));
        let window = on_canvas((a.0.min(b.0), a.1.max(b.1)));
        if dy.abs() < (window.1 - window.0).max(1) {
            // Panel rows that were on the canvas, where they land.
            let shown = on_canvas(a);
            let valid = on_canvas((shown.0 + dy, shown.1 + dy));
            let redraw = subtract(merge(everything), valid);
            return (Some((window.0, window.1, dy)), redraw);
        }
    }
    (None, everything)
}

/// Two scrollers at the same offset (whatever their place).
fn scrolled_alike(a: Option<Scroller>, b: Option<Scroller>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.offset == b.offset,
        (None, None) => true,
        _ => false,
    }
}

/// Turns the frame on `canvas` (drawn for `old`) into `new`'s, drawing only
/// what changed; None when everything must be drawn.
pub fn update(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    old: &Frame,
    new: &Frame,
    now: Instant,
) -> Option<FrameDamage> {
    // Another size, page, theme or text size: everything.
    if old.size != new.size || old.page != new.page || old.generation != new.generation {
        return None;
    }
    let ids = |bodies: &[(u64, Body)]| bodies.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    // A modal overlay opened or closed: everything.
    if ids(&old.modal) != ids(&new.modal) {
        return None;
    }
    let mut damage = FrameDamage::default();
    let mut bands: Vec<(i32, i32)> = Vec::new();
    // Floating overlays redrawn in their own columns only (x0, y0, x1, y1).
    let mut boxes: Vec<(i32, i32, i32, i32)> = Vec::new();
    if new.modal.is_empty() {
        // The page's own path.
        let page = match (&old.page_frame, &new.page_frame) {
            (PageFrame::Chat(a), PageFrame::Chat(b)) => {
                draw_chat_update(canvas, model, view, a, b, now)?
            }
            (PageFrame::List(a), PageFrame::List(b)) => {
                list::draw_list_update(canvas, model, view, a, b, now)?
            }
            (PageFrame::Settings(a), PageFrame::Settings(b)) => {
                settings::draw_settings_update(canvas, model, view, a, b, now)?
            }
            _ => return None,
        };
        // Floating overlays sit on what the page drew: drawn again where it
        // drew (or moved) under them.
        let under = |rows: (i32, i32)| {
            page.moved
                .is_some_and(|(y0, y1, _)| rows.1 > y0 && rows.0 < y1)
                || page
                    .rows
                    .iter()
                    .any(|(y0, y1)| rows.1 > *y0 && rows.0 < *y1)
        };
        // Overlays that say which columns they draw in are redrawn there
        // only: the page drew the rest of their rows right.
        let mut redraw = |id: u64, rows: (i32, i32)| match view.overlays.columns(id) {
            Some((x0, x1)) => boxes.push((x0, rows.0, x1, rows.1)),
            None => bands.push(rows),
        };
        for (id, body) in &new.floating {
            if under(body.rows) {
                redraw(*id, body.rows);
            }
        }
        // A move carried floating overlays' pixels with the page: drawn
        // again where they landed.
        if let Some((y0, y1, dy)) = page.moved {
            for (id, body) in &old.floating {
                let (a, b) = body.rows;
                if b > a && b > y0 && a < y1 {
                    redraw(*id, ((a + dy).max(y0), (b + dy).min(y1)));
                }
            }
        }
        damage = page;
    } else {
        let count = new.modal.len();
        for index in 0..count - 1 {
            if old.modal[index].1 != new.modal[index].1 {
                return None;
            }
        }
        let (moved, redraw) = overlay_change(
            &old.modal[count - 1].1,
            &new.modal[count - 1].1,
            canvas.height,
        );
        if let Some((y0, y1, dy)) = moved {
            move_rows(canvas, (y0, y1), dy);
            damage.moved = Some((y0, y1, dy));
            // Floating overlays inside the moved rows went with them.
            for (_, body) in &new.floating {
                if body.rows.1 > y0 && body.rows.0 < y1 {
                    bands.push(body.rows);
                }
            }
        }
        bands.extend(redraw);
        // The page under the overlays changed (a pulsing dot, a streamed
        // line): its own path finds the rows (drawing only the page there,
        // after the overlay's move), and they are drawn again whole,
        // overlays included. A move of the page's would carry the overlays'
        // pixels with it: everything then.
        if !page_unchanged(&old.page_frame, &new.page_frame) {
            let page = match (&old.page_frame, &new.page_frame) {
                (PageFrame::Chat(a), PageFrame::Chat(b)) => {
                    draw_chat_update(canvas, model, view, a, b, now)?
                }
                (PageFrame::List(a), PageFrame::List(b)) => {
                    list::draw_list_update(canvas, model, view, a, b, now)?
                }
                (PageFrame::Settings(a), PageFrame::Settings(b)) => {
                    settings::draw_settings_update(canvas, model, view, a, b, now)?
                }
                _ => return None,
            };
            if page.moved.is_some() {
                return None;
            }
            bands.extend(page.rows);
        }
    }
    // Floating overlays that changed, came or went.
    for (id, body) in &new.floating {
        match old.floating.iter().find(|(old_id, _)| old_id == id) {
            Some((_, old_body)) if old_body == body => {}
            Some((_, old_body)) => bands.extend([old_body.rows, body.rows]),
            None => bands.push(body.rows),
        }
    }
    for (id, body) in &old.floating {
        if !new.floating.iter().any(|(new_id, _)| new_id == id) {
            bands.push(body.rows);
        }
    }
    let height = canvas.height;
    let bands: Vec<(i32, i32)> = merge(
        bands
            .into_iter()
            .map(|(y0, y1)| (y0.clamp(0, height), y1.clamp(0, height)))
            .collect(),
    );
    for &band in &bands {
        redraw_band(canvas, model, view, pressed, now, band);
    }
    for (x0, y0, x1, y1) in boxes {
        let (y0, y1) = (y0.clamp(0, height), y1.clamp(0, height));
        let (x0, x1) = (x0.clamp(0, canvas.width), x1.clamp(0, canvas.width));
        if y1 > y0 && x1 > x0 {
            canvas.set_clip(x0, y0, x1, y1);
            draw_full(canvas, model, view, pressed, now);
            canvas.reset_clip();
            damage.boxes.push((x0, y0, x1, y1));
        }
    }
    damage.rows = merge(damage.rows.into_iter().chain(bands).collect());
    Some(damage)
}

/// Whether two page records draw the same pixels.
fn page_unchanged(old: &PageFrame, new: &PageFrame) -> bool {
    match (old, new) {
        (PageFrame::Chat(a), PageFrame::Chat(b)) => a.same_as(b),
        (PageFrame::List(a), PageFrame::List(b)) => a == b,
        (PageFrame::Settings(a), PageFrame::Settings(b)) => a == b,
        _ => false,
    }
}

/// A frame drawn on `canvas` with the theme's background first (the
/// first frame, a resize).
pub fn clear(canvas: &mut Canvas) {
    canvas.clear(theme::p().app_background);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::exocortex::model::{Action, Nav, UiAct, sidebar};
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::ui::icons::Icon;
    use crate::exocortex::ui::menu::{AnchoredMenu, MenuItem, Tone};
    use crate::exocortex::ui::sheet::{ActionItem, ActionList, action_sheet};
    use crate::gfx::RectF;

    const SIZE: (i32, i32) = (1080, 2216);

    /// A connected model on the list of 30 conversations (five streaming,
    /// their dots pulsing, when `streaming`).
    fn list_model(streaming: bool) -> (Model, ViewState) {
        let now = Instant::now();
        let mut model = Model::new();
        model.on_connected();
        let rows: Vec<String> = (0..30)
            .map(|i| format!(r#"{{"id":"c{i}","provider":"anthropic","model":"m","title":"Conversation {i}","pinned":{},"sortOrder":{i},"streaming":{},"unread":{},"messageCount":{i}}}"#, i < 2, streaming && i % 7 == 0, i % 3 == 0))
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversations_list","conversations":[{}],"folders":[]}}"#,
                rows.join(",")
            )),
            now,
        );
        (model, ViewState::new())
    }

    fn full(model: &mut Model, view: &mut ViewState, now: Instant) -> Canvas {
        let mut canvas = Canvas::new(SIZE.0, SIZE.1);
        clear(&mut canvas);
        draw_full(&mut canvas, model, view, None, now);
        canvas
    }

    /// One frame drawn incrementally from `old`: it must equal a full
    /// redraw, and change nothing outside the rows it reports.
    fn step(
        canvas: &mut Canvas,
        model: &mut Model,
        view: &mut ViewState,
        old: &Frame,
        now: Instant,
    ) -> (Frame, FrameDamage) {
        let new = record(model, view, None, SIZE, now);
        let mut shadow = canvas.pixels.clone();
        let damage = update(canvas, model, view, None, old, &new, now).expect("incremental");
        let whole = full(model, view, now);
        let width = SIZE.0 as usize;
        let differing: Vec<usize> = (0..SIZE.1 as usize)
            .filter(|y| {
                whole.pixels[y * width..(y + 1) * width]
                    != canvas.pixels[y * width..(y + 1) * width]
            })
            .collect();
        assert!(
            differing.is_empty(),
            "frame differs from a full redraw in rows {:?}..{:?}: {damage:?}",
            differing.first(),
            differing.last()
        );
        if let Some((y0, y1, dy)) = damage.moved {
            move_rows_of(&mut shadow, width, (y0, y1), dy);
        }
        for y in 0..SIZE.1 {
            if damage.rows.iter().any(|(y0, y1)| (*y0..*y1).contains(&y)) {
                continue;
            }
            let row = y as usize * width..(y as usize + 1) * width;
            assert!(
                shadow[row.clone()] == canvas.pixels[row],
                "row {y} changed outside the damage"
            );
        }
        (new, damage)
    }

    fn move_rows_of(pixels: &mut [u32], width: usize, (y0, y1): (i32, i32), dy: i32) {
        let rows = y1 - y0 - dy.abs();
        let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
        pixels.copy_within(
            src as usize * width..(src + rows) as usize * width,
            dst as usize * width,
        );
    }

    fn drawn_rows(damage: &FrameDamage) -> i32 {
        damage.rows.iter().map(|(a, b)| b - a).sum()
    }

    /// Opens what the model asked for and draws the frame whole (an
    /// overlay opening is a full frame).
    fn open(model: &mut Model, view: &mut ViewState, now: Instant) -> (Canvas, Frame) {
        view.take_ui(model, now);
        let frame = record(model, view, None, SIZE, now);
        (full(model, view, now), frame)
    }

    #[test]
    fn a_sheet_slides_in_and_out_drawing_only_what_moves() {
        let (mut model, mut view) = list_model(false);
        let t0 = Instant::now();
        let items = (0..6)
            .map(|i| {
                ActionItem::new(format!("Item {i}"), Action::Ui(UiAct::Toast(i.to_string())))
                    .icon(Icon::Copy)
            })
            .collect();
        model.ui.open(Box::new(action_sheet(ActionList::new(
            "A test sheet",
            items,
        ))));
        let (mut canvas, mut frame) = open(&mut model, &mut view, t0);
        let mut moved = 0;
        for ms in (8..=320).step_by(8) {
            let now = t0 + Duration::from_millis(ms);
            let (next, damage) = step(&mut canvas, &mut model, &mut view, &frame, now);
            frame = next;
            if damage.moved.is_some() {
                moved += 1;
                // The panel moves; only the strip it uncovers and its
                // shadow are drawn.
                assert!(
                    drawn_rows(&damage) < 600,
                    "slide frame at {ms} ms: {damage:?}"
                );
            }
        }
        assert!(moved > 10, "the sheet slid as moved rows ({moved} frames)");
        // At rest: nothing to draw.
        let now = t0 + Duration::from_millis(400);
        let (next, damage) = step(&mut canvas, &mut model, &mut view, &frame, now);
        frame = next;
        assert_eq!(damage, FrameDamage::default());
        // Closing slides out the same way.
        model.handle(Action::Ui(UiAct::Dismiss));
        view.take_ui(&mut model, now);
        let mut moved = 0;
        for ms in (8..=200).step_by(8) {
            let at = now + Duration::from_millis(ms);
            let (next, damage) = step(&mut canvas, &mut model, &mut view, &frame, at);
            frame = next;
            moved += usize::from(damage.moved.is_some());
        }
        assert!(moved > 5);
        assert!(view.overlays.sweep(now + Duration::from_millis(400)));
    }

    #[test]
    fn the_anchored_menu_fades_in_over_its_own_rows() {
        let (mut model, mut view) = list_model(true);
        let t0 = Instant::now();
        let anchor = RectF::new(0.0, 300.0, 1080.0, 160.0);
        model.ui.open(Box::new(AnchoredMenu::new(
            anchor,
            vec![
                MenuItem::new(
                    Icon::Pin,
                    "Pin",
                    Tone::Accent,
                    Action::Nav(Nav::OpenSettings),
                ),
                MenuItem::new(
                    Icon::Copy,
                    "Copy id",
                    Tone::Plain,
                    Action::Nav(Nav::OpenSettings),
                ),
                MenuItem::new(
                    Icon::Trash,
                    "Delete",
                    Tone::Destructive,
                    Action::Nav(Nav::OpenSettings),
                ),
            ],
            true,
        )));
        let (mut canvas, mut frame) = open(&mut model, &mut view, t0);
        let menu = AnchoredMenu::place(anchor, 3, 1080.0, SIZE.1 as f32 - 170.0);
        for ms in (8..=200).step_by(8) {
            let now = t0 + Duration::from_millis(ms);
            let (next, damage) = step(&mut canvas, &mut model, &mut view, &frame, now);
            frame = next;
            for &(y0, y1) in &damage.rows {
                // The menu's rows, and the rows of the conversations that
                // pulse under the dim.
                let in_menu = y0 >= menu.y as i32 - 2 && y1 <= (menu.y + menu.h) as i32 + 2;
                assert!(in_menu || y1 - y0 <= 161, "{ms} ms: {damage:?}");
            }
        }
        // Tapping an item performs it; tapping outside closes.
        let cx = view.cx(
            &model,
            1080.0,
            SIZE.1 as f32,
            t0 + Duration::from_millis(300),
        );
        let mut overlays = std::mem::take(&mut view.overlays);
        let down = overlays.gesture(
            &crate::exocortex::gesture::Gesture::Down {
                x: menu.x + 100.0,
                y: menu.y + 10.0,
            },
            &cx,
        );
        let tap = overlays.gesture(
            &crate::exocortex::gesture::Gesture::Tap {
                x: menu.x + 100.0,
                y: menu.y + 10.0,
            },
            &cx,
        );
        assert_eq!(down, crate::exocortex::ui::overlay::Response::Handled);
        assert_eq!(
            tap,
            crate::exocortex::ui::overlay::Response::Action(Action::Nav(Nav::OpenSettings))
        );
        // ...and closes the menu (a Copy, say, closes nothing else).
        assert!(overlays.top().is_none(), "the menu is closing");
        assert!(overlays.sweep(t0 + Duration::from_millis(600)));
        assert!(overlays.is_empty());
        view.overlays = overlays;
    }

    #[test]
    fn a_theme_change_draws_everything() {
        let (mut model, mut view) = list_model(false);
        let t0 = Instant::now();
        let old = record(&mut model, &mut view, None, SIZE, t0);
        let mut canvas = full(&mut model, &mut view, t0);
        let same = record(&mut model, &mut view, None, SIZE, t0);
        assert!(update(&mut canvas, &mut model, &mut view, None, &old, &same, t0).is_some());
        theme::set_current(&theme::CERBERUS);
        let new = record(&mut model, &mut view, None, SIZE, t0);
        assert!(update(&mut canvas, &mut model, &mut view, None, &old, &new, t0).is_none());
        theme::set_current(&theme::WHALE);
    }

    #[test]
    fn a_toast_comes_and_goes_over_a_scrolling_list() {
        let (mut model, mut view) = list_model(true);
        let t0 = Instant::now();
        model.ui.toast("Copied");
        view.take_ui(&mut model, t0);
        let mut frame = record(&mut model, &mut view, None, SIZE, t0);
        let mut canvas = full(&mut model, &mut view, t0);
        for ms in (8..=4000).step_by(40) {
            let now = t0 + Duration::from_millis(ms);
            // The list scrolls under it meanwhile.
            view.list_scroll.offset = (ms / 40) as f32 * 3.0;
            let (next, _) = step(&mut canvas, &mut model, &mut view, &frame, now);
            frame = next;
        }
        assert!(view.overlays.sweep(t0 + Duration::from_millis(4000)));
        // A page under the list's ⋯ menu: the menu's action opens Settings.
        model.handle(Action::List(sidebar::Act::Menu(RectF::new(
            707.0, 0.0, 122.0, 122.0,
        ))));
        assert_eq!(model.ui.opening.len(), 1);
    }
}
