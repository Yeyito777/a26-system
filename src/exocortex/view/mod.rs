//! The Exocortex app drawn in the System and Messages apps' iOS dark
//! style: the conversation list (folders, pinned first), a conversation with
//! the user's messages in blue bubbles and replies as full-width markdown,
//! tool calls folded to one row each (tap to open), a composer above Moon's
//! keyboard, and a model sheet. Layout is cached per history entry, so a
//! streaming reply re-lays out only itself.

pub mod blocks;
pub mod chat_header;
pub mod composer;
pub mod list;
pub mod markdown_layout;
pub mod ops;
pub mod tools;
pub mod transcript;

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;

use crate::comms::Scroll;
use crate::gfx::{Canvas, RectF, Rgb};
use crate::text::{self, Weight};
use crate::ui::{BG, MARGIN, RED, SECONDARY, SECONDARY_ALPHA, WHITE};

use super::model::{Action, Model, Page, Row};
use super::sheets::inspector::{
    SHEET_HEADER, SHEET_ROW, SHEET_TOP, SheetItem, draw_sheet, effort_chips, sheet_content,
};
use chat_header::draw_nav;
use composer::{SEND_R, composer, draw_composer};
use list::{ROW_H, draw_list, list_bottom, list_top};
use ops::{Laid, op_rows};
use transcript::{CachedPiece, CachedPlan, ChatLayout, draw_transcript};

pub use super::sheets::inspector::sheet_max;
pub use list::{ListFrame, draw_list_update, list_frame, list_max};

pub const W: f32 = 1080.0;
/// Moon owns the bottom of the window for its swipe-up gesture.
const GESTURE_RESERVE: f32 = 170.0;
const NAV_H: f32 = 150.0;
const ROW_PRESSED: Rgb = 0x2c2c2e;

/// Something on screen a touch is on.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Action(Action),
    /// The composer's text field.
    Composer,
    /// The transcript, away from anything tappable.
    Transcript,
    /// Outside the model sheet (closes it).
    SheetBackdrop,
}

/// The view's own state: scroll positions, focus and layout caches.
#[derive(Default)]
pub struct ViewState {
    pub list_scroll: Scroll,
    pub chat_scroll: Scroll,
    pub sheet_scroll: Scroll,
    /// The composer has focus and Moon's keyboard is up.
    pub keyboard: bool,
    /// The chat follows its newest message.
    pub follow: bool,
    layout: ChatLayout,
    /// Pieces by what they show (`piece_key`): an entry that comes back
    /// unchanged (a window resent, a reply committed, a chat reopened) is
    /// not laid out again.
    pieces: HashMap<u64, CachedPiece>,
    /// Entries' plans by stamp.
    plans: HashMap<u64, CachedPlan>,
    pass: u64,
    /// Phase of the "working" dots.
    pub dots: u8,
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            follow: true,
            ..Self::default()
        }
    }
}

fn transcript_bounds(model: &Model, height: f32, keyboard: bool) -> (f32, f32) {
    (NAV_H, composer(model, height, keyboard).field.y - 20.0)
}

pub fn draw(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
) {
    let height = canvas.height as f32;
    match model.page {
        Page::List => draw_list(canvas, model, view, pressed, height, now),
        Page::Chat => draw_chat(canvas, model, view, pressed, height, now),
    }
    if model.picker {
        draw_sheet(canvas, model, view, pressed, height, now);
    }
}

pub(crate) fn pressed_action(pressed: Option<&Hit>) -> Option<&Action> {
    match pressed {
        Some(Hit::Action(action)) => Some(action),
        _ => None,
    }
}

/// What a chat frame showed: enough to draw the next one by changing only
/// what differs (`draw_chat_update`).
#[derive(Clone)]
pub struct ChatFrame {
    bounds: (i32, i32),
    height: i32,
    offset: f32,
    items: Vec<(f32, Rc<Laid>)>,
    /// The working dots' phase and their center in content coordinates.
    dots: Option<(u8, f32)>,
    pressed: Option<Action>,
    nav: u64,
    composer: u64,
}

/// Rows a frame changed: rows moved inside the presented copy (`y0, y1,
/// dy`, for `Presenter::scroll`), then rows drawn anew (for
/// `Presenter::present_rows`).
#[derive(Debug, Default, PartialEq)]
pub struct FrameDamage {
    pub moved: Option<(i32, i32, i32)>,
    pub rows: Vec<(i32, i32)>,
}

/// The current chat frame's record, or None when it shows something the
/// incremental path does not draw (another page, the model sheet, a notice
/// or connection pill, a loading or empty chat).
pub fn chat_frame(
    model: &Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) -> Option<ChatFrame> {
    if model.page != Page::Chat
        || model.picker
        || model.offline
        || model.notice.is_some()
        || !model.chat.loaded
        || model.chat.id.is_none()
    {
        return None;
    }
    view.layout(model);
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    let mut nav = DefaultHasher::new();
    let selection = model.selection();
    (selection.provider, selection.model, selection.effort).hash(&mut nav);
    model
        .open_summary()
        .map(|s| s.display_title().to_owned())
        .hash(&mut nav);
    let mut composer = DefaultHasher::new();
    (&model.composer, view.keyboard, model.streaming()).hash(&mut composer);
    Some(ChatFrame {
        bounds: (top as i32, bottom as i32),
        height: height as i32,
        offset: view.chat_scroll.value(now).round(),
        items: view
            .layout
            .items
            .iter()
            .map(|item| (item.y, item.laid.clone()))
            .collect(),
        dots: model
            .streaming()
            .then_some((view.dots % 3, view.layout.height - 34.0)),
        pressed: pressed_action(pressed).cloned(),
        nav: nav.finish(),
        composer: composer.finish(),
    })
}

/// Where two layouts of one piece start to differ (relative to it), if
/// anywhere: the top of every op after the first that differs.
fn first_change(old: &Laid, new: &Laid) -> Option<f32> {
    let shared = old.ops.len().min(new.ops.len());
    let first = (0..shared)
        .find(|&index| old.ops[index] != new.ops[index])
        .unwrap_or(shared);
    if first == old.ops.len() && first == new.ops.len() && old.h == new.h {
        return None;
    }
    let top = old.ops[first..]
        .iter()
        .chain(&new.ops[first..])
        .map(|op| op_rows(op).0)
        .fold(old.h.min(new.h), f32::min);
    Some(top)
}

/// Turns the frame on `canvas` (drawn for `old`) into the one for `new` by
/// moving the transcript's rows as far as it scrolled and drawing only what
/// changed: the uncovered strip, the transcript from the first change down,
/// the dots, the bar or the composer. None when a full redraw is needed.
/// The result is pixel-identical to a full redraw (tested).
pub fn draw_chat_update(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    old: &ChatFrame,
    new: &ChatFrame,
) -> Option<FrameDamage> {
    if old.bounds != new.bounds || old.height != new.height {
        return None;
    }
    let (top, bottom) = new.bounds;
    let shift = (new.offset - old.offset) as i32;
    if shift.abs() > (bottom - top) / 2 {
        return None;
    }
    // Content (in transcript coordinates) from which the frames differ.
    let mut change = f32::INFINITY;
    for index in 0..old.items.len().max(new.items.len()) {
        match (old.items.get(index), new.items.get(index)) {
            (Some((old_y, old_laid)), Some((new_y, new_laid))) if old_y == new_y => {
                if Rc::ptr_eq(old_laid, new_laid) {
                    continue;
                }
                if let Some(at) = first_change(old_laid, new_laid) {
                    change = new_y + at;
                    break;
                }
            }
            (old_item, new_item) => {
                change = old_item
                    .map(|(y, _)| *y)
                    .into_iter()
                    .chain(new_item.map(|(y, _)| *y))
                    .fold(f32::INFINITY, f32::min);
                break;
            }
        }
    }
    if old.dots != new.dots {
        for (_, y) in old.dots.iter().chain(&new.dots) {
            change = change.min(y - 16.0);
        }
    }
    let mut damage = FrameDamage::default();
    let mut redraw: Vec<(i32, i32)> = Vec::new();
    if shift != 0 {
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
    if change.is_finite() {
        let from = (NAV_H + change - new.offset).floor() as i32;
        if from < bottom {
            redraw.push((from.max(top), bottom));
        }
    }
    // A row pressed or let go: its highlight's rows.
    let mut nav_pressed = false;
    let mut composer_pressed = false;
    if old.pressed != new.pressed {
        for action in old.pressed.iter().chain(&new.pressed) {
            match action {
                Action::Back | Action::ShowPicker => nav_pressed = true,
                Action::Send | Action::Abort => composer_pressed = true,
                _ => {}
            }
            for (y, laid) in &new.items {
                for (rect, hit) in &laid.hits {
                    if hit == action {
                        let y0 = (NAV_H + y + rect.y - new.offset).floor() as i32 - 1;
                        let y1 = (NAV_H + y + rect.y + rect.h - new.offset).ceil() as i32 + 1;
                        if y1 > top && y0 < bottom {
                            redraw.push((y0.max(top), y1.min(bottom)));
                        }
                    }
                }
            }
        }
    }
    redraw.sort_unstable();
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (y0, y1) in redraw {
        match merged.last_mut() {
            Some(last) if y0 <= last.1 => last.1 = last.1.max(y1),
            _ => merged.push((y0, y1)),
        }
    }
    for &(y0, y1) in &merged {
        canvas.fill_rect(0, y0, W as i32, y1 - y0, BG, 1.0);
        draw_transcript(
            canvas,
            model,
            view,
            new.pressed.as_ref(),
            (y0 as f32, y1 as f32),
            new.offset,
        );
    }
    damage.rows = merged;
    if old.nav != new.nav || nav_pressed {
        draw_nav(canvas, model, new.pressed.as_ref());
        damage.rows.push((0, NAV_H as i32));
    }
    if old.composer != new.composer || composer_pressed {
        draw_composer(canvas, model, view, new.pressed.as_ref(), new.height as f32);
        damage.rows.push((bottom, new.height));
    }
    Some(damage)
}

fn draw_chat(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    view.layout(model);
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    // Whole pixels, so a scrolled frame equals a shifted one (scroll_chat).
    let offset = view.chat_scroll.value(now).round();
    let pressed_action = pressed_action(pressed).cloned();
    draw_transcript(
        canvas,
        model,
        view,
        pressed_action.as_ref(),
        (top, bottom),
        offset,
    );
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    if !model.chat.loaded && model.chat.id.is_some() {
        text::draw_centered(
            canvas,
            "Loading…",
            W / 2.0,
            (top + bottom) / 2.0,
            Weight::Regular,
            40.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    } else if model.chat.id.is_none() && model.chat.echoes.is_empty() {
        let selection = model.selection();
        let label = model.model_label(&selection);
        let cy = (top + bottom) / 2.0 - 60.0;
        text::draw_centered(
            canvas,
            "New Conversation",
            W / 2.0,
            cy,
            Weight::SemiBold,
            58.0,
            WHITE,
            0.9,
        );
        let detail = if label.is_empty() {
            String::new()
        } else {
            format!("with {label}")
        };
        text::draw_centered(
            canvas,
            &detail,
            W / 2.0,
            cy + 70.0,
            Weight::Regular,
            40.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
    canvas.reset_clip();
    draw_nav(canvas, model, pressed_action.as_ref());
    if model.offline {
        let message = if model.was_connected {
            "Reconnecting…"
        } else {
            "Connecting…"
        };
        let w = text::measure(message, Weight::Medium, 34.0) + 60.0;
        let pill = RectF::new((W - w) / 2.0, NAV_H + 20.0, w, 64.0);
        canvas.fill_rrect(pill, 32.0, 0x2c2c2e, 0.95);
        text::draw_centered(
            canvas,
            message,
            W / 2.0,
            NAV_H + 64.0,
            Weight::Medium,
            34.0,
            WHITE,
            0.9,
        );
    }
    if let Some((notice, _)) = &model.notice {
        let notice = text::fit(notice, Weight::Medium, 34.0, W - 2.0 * MARGIN - 60.0);
        let w = text::measure(&notice, Weight::Medium, 34.0) + 60.0;
        let pill = RectF::new((W - w) / 2.0, NAV_H + 20.0, w, 64.0);
        canvas.fill_rrect(pill, 32.0, 0x3a1d1d, 0.95);
        text::draw_centered(
            canvas,
            &notice,
            W / 2.0,
            NAV_H + 64.0,
            Weight::Medium,
            34.0,
            RED,
            1.0,
        );
    }
    draw_composer(canvas, model, view, pressed_action.as_ref(), height);
}

// ----- Hit testing -------------------------------------------------------------

pub enum ScrollArea {
    List,
    Chat,
    Sheet,
}

/// The list a touch at `y` may scroll.
pub fn scroll_area(model: &Model, view: &ViewState, height: f32, y: f32) -> Option<ScrollArea> {
    if model.picker {
        return (y >= SHEET_TOP + SHEET_HEADER).then_some(ScrollArea::Sheet);
    }
    match model.page {
        Page::List => (y >= list_top(model) && y < list_bottom(height)).then_some(ScrollArea::List),
        Page::Chat => {
            let (top, bottom) = transcript_bounds(model, height, view.keyboard);
            (y >= top && y < bottom).then_some(ScrollArea::Chat)
        }
    }
}

pub fn hit(
    model: &mut Model,
    view: &mut ViewState,
    height: f32,
    x: i16,
    y: i16,
    now: Instant,
) -> Option<Hit> {
    let (x, y) = (f32::from(x), f32::from(y));
    if model.picker {
        if y < SHEET_TOP {
            return Some(Hit::SheetBackdrop);
        }
        if y < SHEET_TOP + SHEET_HEADER {
            return (x > W - 300.0).then_some(Hit::Action(Action::HidePicker));
        }
        let top = SHEET_TOP + SHEET_HEADER;
        let offset = view.sheet_scroll.value(now);
        for (item_y, item) in sheet_content(model) {
            let iy = top + item_y - offset;
            match item {
                SheetItem::Efforts(efforts) => {
                    for (rect, effort) in effort_chips(&efforts, iy) {
                        if rect.inset(-10.0).contains(x, y) {
                            return Some(Hit::Action(Action::PickEffort(effort)));
                        }
                    }
                }
                SheetItem::Model { provider, id, .. } if y >= iy && y < iy + SHEET_ROW => {
                    return Some(Hit::Action(Action::PickModel(provider, id)));
                }
                _ => {}
            }
        }
        return None;
    }
    match model.page {
        Page::List => {
            if y < list_top(model) {
                if x > W - 200.0
                    && y < if model.folders.is_empty() {
                        170.0
                    } else {
                        NAV_H
                    }
                {
                    return Some(Hit::Action(Action::NewConversation));
                }
                if !model.folders.is_empty() && y < NAV_H && x < 420.0 {
                    return Some(Hit::Action(Action::Back));
                }
                return None;
            }
            if y >= list_bottom(height) {
                return None;
            }
            let offset = view.list_scroll.value(now);
            let index = ((y - list_top(model) + offset) / ROW_H).floor();
            if index < 0.0 {
                return None;
            }
            let folder = model.current_folder().map(str::to_owned);
            let rows = model.sidebar.rows(folder.as_deref());
            match rows.get(index as usize)? {
                Row::Folder { id, .. } => Some(Hit::Action(Action::OpenFolder(id.clone()))),
                Row::Conversation(id) => Some(Hit::Action(Action::OpenConversation(id.clone()))),
            }
        }
        Page::Chat => {
            if y < NAV_H {
                if x < 300.0 {
                    return Some(Hit::Action(Action::Back));
                }
                if y > 80.0 && (W / 2.0 - 300.0..W / 2.0 + 380.0).contains(&x) {
                    return Some(Hit::Action(Action::ShowPicker));
                }
                return (x > 300.0 && x < W - 160.0).then_some(Hit::Action(Action::ShowPicker));
            }
            let geometry = composer(model, height, view.keyboard);
            let (sx, sy) = geometry.send;
            if (x - sx).hypot(y - sy) < SEND_R + 30.0 {
                let typed = !model.composer.trim().is_empty();
                return Some(Hit::Action(if model.streaming() && !typed {
                    Action::Abort
                } else {
                    Action::Send
                }));
            }
            if geometry.field.inset(-16.0).contains(x, y) {
                return Some(Hit::Composer);
            }
            let (top, bottom) = transcript_bounds(model, height, view.keyboard);
            if y < top || y >= bottom {
                return None;
            }
            view.layout(model);
            let offset = view.chat_scroll.value(now);
            for item in &view.layout.items {
                let iy = top + item.y - offset;
                if y < iy || y >= iy + item.laid.h {
                    continue;
                }
                for (rect, action) in &item.laid.hits {
                    if y >= iy + rect.y && y < iy + rect.y + rect.h {
                        return Some(Hit::Action(action.clone()));
                    }
                }
            }
            Some(Hit::Transcript)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::markdown::{Span, Style};
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::sheets::inspector::SHEET_SECTION;
    use crate::ui::BLUE;
    use markdown_layout::{BODY, lay_markdown, line_width, plain, wrap};
    use ops::Op;
    use tools::TOOL_ROW;

    #[test]
    fn wrapping_fits_width_breaks_lines_and_long_words() {
        let spans = vec![
            Span {
                text: "Hello world, this is ".into(),
                style: Style::Plain,
            },
            Span {
                text: "bold".into(),
                style: Style::Bold,
            },
            Span {
                text: " text that wraps across several lines\nand breaks".into(),
                style: Style::Plain,
            },
        ];
        let lines = wrap(&spans, 400.0, BODY);
        assert!(lines.len() >= 4);
        for line in &lines {
            assert!(line_width(line) <= 400.5, "{line:?}");
            assert!(!line.first().is_some_and(|run| run.text.starts_with(' ')));
        }
        assert!(
            lines
                .iter()
                .any(|line| line.iter().any(|run| run.style == Style::Bold))
        );
        let last: String = lines
            .last()
            .unwrap()
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(last, "and breaks");
        let long = wrap(&plain(&"x".repeat(200)), 300.0, BODY);
        assert!(long.len() > 3);
        assert!(long.iter().all(|line| line_width(line) <= 300.5));
        assert_eq!(wrap(&plain(""), 300.0, BODY).len(), 1);
    }

    #[test]
    fn markdown_lays_out_top_to_bottom() {
        let mut ops = Vec::new();
        let bottom = lay_markdown(
            &mut ops,
            "# Title\nText **bold**\n\n- a\n- b\n\n```\ncode\n```",
            MARGIN,
            900.0,
            0.0,
        );
        let baselines: Vec<f32> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Text { baseline, .. } => Some(*baseline),
                _ => None,
            })
            .collect();
        assert!(
            baselines.windows(2).all(|w| w[1] >= w[0] - 0.1),
            "{baselines:?}"
        );
        assert!(bottom > *baselines.last().unwrap());
        assert!(ops.iter().any(|op| matches!(
            op,
            Op::Text {
                weight: Weight::Mono,
                ..
            }
        )));
        assert!(ops.iter().any(|op| matches!(
            op,
            Op::Text {
                weight: Weight::SemiBold,
                ..
            }
        )));
    }

    fn chat_model() -> Model {
        let mut model = Model::new();
        model.on_connected();
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","provider":"anthropic","model":"m","effort":"low","title":"Hi","pinned":false,"sortOrder":0,"streaming":false,"unread":false}}"#),
            Instant::now(),
        );
        let commands = model.handle(Action::OpenConversation("c".into()));
        let request = commands[0]["reqId"].as_str().unwrap().to_owned();
        let calls: Vec<String> = (0..4)
            .map(|i| format!(r#"{{"type":"tool_call","toolCallId":"t{i}","toolName":"Bash","input":{{"command":"ls {i}"}},"summary":"ls {i}"}},{{"type":"tool_result","toolCallId":"t{i}","toolName":"Bash","output":"ok","isError":false}}"#))
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"user","text":"Please list"}},{{"type":"ai","blocks":[{},{{"type":"text","text":"Done."}}]}}],"historyStartIndex":0}}"#,
                calls.join(",")
            )),
            Instant::now(),
        );
        model
    }

    #[test]
    fn tool_runs_fold_and_expand_on_tap() {
        let now = Instant::now();
        let mut model = chat_model();
        let mut view = ViewState::default();
        view.layout(&model);
        let height = 2216.0;
        // A bubble, then one folded row for four calls, then the text.
        let group = view.layout.items[1].laid.hits.clone();
        assert_eq!(group.len(), 1);
        assert_eq!(group[0].1, Action::ToggleExpanded("group:t0".into()));
        let (top, _) = transcript_bounds(&model, height, false);
        let y = top + view.layout.items[1].y + group[0].0.y + 10.0;
        let hit_action = hit(&mut model, &mut view, height, 300, y as i16, now);
        assert_eq!(
            hit_action,
            Some(Hit::Action(Action::ToggleExpanded("group:t0".into())))
        );
        // The reply's pieces: the folded row, then the text.
        assert_eq!(view.layout.items.len(), 3);
        let before = view.layout.height;
        let text = view.layout.items[2].laid.clone();
        model.handle(Action::ToggleExpanded("group:t0".into()));
        view.layout(&model);
        // Opened: the row, a row per call, and the same text piece below.
        let hits: Vec<&Action> = view.layout.items[1..]
            .iter()
            .flat_map(|item| item.laid.hits.iter().map(|(_, action)| action))
            .collect();
        assert_eq!(hits.len(), 5);
        assert_eq!(hits[1], &Action::ToggleExpanded("tool:t0".into()));
        assert!(view.layout.height > before + 3.0 * TOOL_ROW);
        assert!(Rc::ptr_eq(&view.layout.items[6].laid, &text));
        // The composer and send button are hit; the transcript elsewhere.
        let geometry = composer(&model, height, false);
        assert_eq!(
            hit(
                &mut model,
                &mut view,
                height,
                300,
                (geometry.field.y + 20.0) as i16,
                now
            ),
            Some(Hit::Composer)
        );
        model.composer = "x".into();
        assert_eq!(
            hit(
                &mut model,
                &mut view,
                height,
                geometry.send.0 as i16,
                geometry.send.1 as i16,
                now
            ),
            Some(Hit::Action(Action::Send))
        );
        let mut canvas = Canvas::new(1080, height as i32);
        draw(&mut canvas, &mut model, &mut view, None, now);
        assert!(canvas.pixels.contains(&BLUE));
    }

    #[test]
    fn a_streaming_reply_lays_out_only_what_grew() {
        let now = Instant::now();
        let mut model = chat_model();
        let mut view = ViewState::new();
        for line in [
            r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":7}"#,
            r#"{"type":"text_chunk","convId":"c","text":"First paragraph.\n\nSecond"}"#,
        ] {
            model.apply(parse_event(line), now);
        }
        view.layout(&model);
        let before: Vec<Rc<Laid>> = view.layout.items.iter().map(|i| i.laid.clone()).collect();
        model.apply(
            parse_event(r#"{"type":"text_chunk","convId":"c","text":" grows"}"#),
            now,
        );
        view.layout(&model);
        let after: Vec<Rc<Laid>> = view.layout.items.iter().map(|i| i.laid.clone()).collect();
        assert_eq!(before.len(), after.len());
        let last = after.len() - 1;
        // Everything but the growing paragraph is the same piece.
        for (index, (old, new)) in before.iter().zip(&after).enumerate() {
            assert_eq!(Rc::ptr_eq(old, new), index != last, "piece {index}");
        }
        // Committed, the reply is the same pieces: nothing is laid out again.
        model.apply(
            parse_event(r#"{"type":"message_complete","convId":"c","blocks":[{"type":"text","text":"First paragraph.\n\nSecond grows"}]}"#),
            now,
        );
        view.layout(&model);
        let committed: Vec<Rc<Laid>> = view.layout.items.iter().map(|i| i.laid.clone()).collect();
        assert!(after.iter().zip(&committed).all(|(a, b)| Rc::ptr_eq(a, b)));
    }

    #[test]
    fn the_reader_stays_put_when_older_history_arrives_mid_drag() {
        let now = Instant::now();
        let height = 900.0;
        let mut model = Model::new();
        model.on_connected();
        let commands = model.handle(Action::OpenConversation("c".into()));
        let request = commands[0]["reqId"].as_str().unwrap().to_owned();
        let page = |kind: &str, request: &str, from: u64, to: u64| {
            let entries: Vec<String> = (from..to)
                .map(|i| format!(r#"{{"type":"user","text":"Message number {i}"}},{{"type":"ai","blocks":[{{"type":"text","text":"Reply {i}"}}]}}"#))
                .collect();
            format!(
                r#"{{"type":"{kind}","reqId":"{request}","convId":"c","entries":[{}],"historyStartIndex":{},"historyEndIndex":{},"hasOlderHistory":{}}}"#,
                entries.join(","),
                from * 2,
                to * 2,
                from > 0
            )
        };
        model.apply(
            parse_event(&page("conversation_loaded", &request, 10, 20)),
            now,
        );
        // Asked for (its Loading… row shows above).
        let older = model.load_older();
        let request = older[0]["reqId"].as_str().unwrap().to_owned();
        let mut view = ViewState::new();
        view.follow = false;
        view.chat_scroll.offset = 700.0;
        view.layout(&model);
        let first = view
            .layout
            .items
            .iter()
            .find(|item| item.y >= 700.0)
            .unwrap();
        let (laid, on_screen) = (first.laid.clone(), first.y - 700.0);
        // One pass of the loop: the finger moves 20 px down (toward older
        // messages) and the older page lands.
        let before = view.before(&model, now);
        view.chat_scroll.begin(500.0, now);
        view.chat_scroll.moved(520.0, 5000.0, now);
        model.apply(
            parse_event(&page("conversation_history_loaded", &request, 0, 10)),
            now,
        );
        view.settle(&model, before, height, now);
        let item = view
            .layout
            .items
            .iter()
            .find(|item| Rc::ptr_eq(&item.laid, &laid))
            .unwrap();
        assert!(item.y > 700.0, "older history above it");
        // Where it was, moved with the finger.
        assert_eq!(item.y - view.chat_scroll.value(now), on_screen + 20.0);
    }

    #[test]
    fn opening_a_row_above_keeps_the_reader_on_the_same_piece() {
        let now = Instant::now();
        let mut model = chat_model();
        let mut view = ViewState::new();
        view.layout(&model);
        // The reply's text, after its folded run of calls, at the top.
        let text = view.layout.items[2].laid.clone();
        view.follow = false;
        view.chat_scroll.offset = view.layout.items[2].y;
        let before = view.before(&model, now);
        model.handle(Action::ToggleExpanded("group:t0".into()));
        view.settle(&model, before, 900.0, now);
        let item = view
            .layout
            .items
            .iter()
            .find(|item| Rc::ptr_eq(&item.laid, &text))
            .unwrap();
        assert_eq!(item.y, view.chat_scroll.value(now));
    }

    /// A real finger: motion after motion, each pass of the event loop
    /// handling a few (and now and then a streamed chunk) before its frame.
    /// Every frame must move the transcript with the finger.
    #[test]
    fn a_sustained_drag_scrolls_every_frame() {
        let height = 900.0;
        let (mut model, mut view) = long_chat(height);
        let now = Instant::now();
        model.apply(
            parse_event(r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":7}"#),
            now,
        );
        view.follow = false;
        let max = view.chat_max(&model, height);
        view.chat_scroll.offset = (max / 2.0).round();
        let start = view.chat_scroll.offset;
        view.chat_scroll.begin(600.0, now);
        for pass in 1..=30 {
            let damage = update_matches(&mut model, &mut view, height, |model, view| {
                let before = view.before(model, now);
                for motion in 1..=3 {
                    let y = 600.0 + ((pass - 1) * 3 + motion) as f32 * 1.5;
                    view.chat_scroll.moved(y, max, now);
                    view.follow = false;
                }
                if pass % 4 == 0 {
                    model.apply(
                        parse_event(r#"{"type":"text_chunk","convId":"c","text":"more words "}"#),
                        now,
                    );
                }
                view.settle(model, before, height, now);
            });
            assert!(damage.moved.is_some(), "pass {pass} did not scroll");
            assert_eq!(view.chat_scroll.value(now), start - pass as f32 * 4.5);
        }
    }

    /// Draws the frame, applies `change`, updates the frame incrementally,
    /// and checks it against a full redraw and that nothing outside the
    /// damage changed (what the presenter skips comparing).
    fn update_matches(
        model: &mut Model,
        view: &mut ViewState,
        height: f32,
        change: impl FnOnce(&mut Model, &mut ViewState),
    ) -> FrameDamage {
        update_matches_with(model, view, height, None, None, change)
    }

    fn update_matches_with(
        model: &mut Model,
        view: &mut ViewState,
        height: f32,
        before: Option<&Hit>,
        after: Option<&Hit>,
        change: impl FnOnce(&mut Model, &mut ViewState),
    ) -> FrameDamage {
        let now = Instant::now();
        let mut canvas = Canvas::new(1080, height as i32);
        canvas.clear(BG);
        draw(&mut canvas, model, view, before, now);
        let old = chat_frame(model, view, before, height, now).expect("a chat frame");
        let mut shadow = canvas.pixels.clone();
        change(model, view);
        let new = chat_frame(model, view, after, height, now).expect("a chat frame");
        let damage = draw_chat_update(&mut canvas, model, view, &old, &new).expect("incremental");
        let mut full = Canvas::new(1080, height as i32);
        full.clear(BG);
        draw(&mut full, model, view, after, now);
        assert!(
            full.pixels == canvas.pixels,
            "incremental frame differs: {damage:?}"
        );
        let width = 1080;
        if let Some((y0, y1, dy)) = damage.moved {
            let rows = y1 - y0 - dy.abs();
            let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
            shadow.copy_within(
                (src * width) as usize..((src + rows) * width) as usize,
                (dst * width) as usize,
            );
        }
        for y in 0..height as i32 {
            if damage.rows.iter().any(|(y0, y1)| (*y0..*y1).contains(&y)) {
                continue;
            }
            let row = (y * width) as usize..((y + 1) * width) as usize;
            assert!(
                shadow[row.clone()] == canvas.pixels[row],
                "row {y} changed outside the damage"
            );
        }
        damage
    }

    fn long_chat(height: f32) -> (Model, ViewState) {
        let mut model = chat_model();
        model.handle(Action::ToggleExpanded("group:t0".into()));
        model.handle(Action::ToggleExpanded("tool:t1".into()));
        model.handle(Action::ToggleExpanded("tool:t2".into()));
        let mut view = ViewState::new();
        let max = view.chat_max(&model, height);
        assert!(max > 300.0, "the transcript must scroll: {max}");
        view.chat_scroll.offset = 120.0;
        (model, view)
    }

    #[test]
    fn a_scrolled_frame_equals_a_redrawn_one() {
        let height = 900.0;
        let (mut model, mut view) = long_chat(height);
        for to in [177.0, 140.0, 260.0, 200.0, 20.0] {
            let damage = update_matches(&mut model, &mut view, height, |_, view| {
                view.chat_scroll.offset = to;
            });
            assert!(damage.moved.is_some());
            let redrawn: i32 = damage.rows.iter().map(|(a, b)| b - a).sum();
            assert!(redrawn <= 200, "only the uncovered strip: {damage:?}");
        }
        // Unchanged: nothing to draw.
        let damage = update_matches(&mut model, &mut view, height, |_, _| {});
        assert_eq!(damage, FrameDamage::default());
    }

    #[test]
    fn streaming_typing_and_the_bar_update_only_what_changed() {
        let height = 900.0;
        let (mut model, mut view) = long_chat(height);
        let now = Instant::now();
        for line in [
            r#"{"type":"streaming_started","convId":"c","provider":"a","model":"m","startedAt":7}"#,
            r#"{"type":"text_chunk","convId":"c","text":"A reply that **grows**, word by word, "}"#,
        ] {
            model.apply(parse_event(line), now);
        }
        view.chat_scroll.offset = view.chat_max(&model, height);
        for chunk in [
            "and wraps onto",
            " a second line with `code`,",
            " then\n\n- a list",
        ] {
            let damage = update_matches(&mut model, &mut view, height, |model, view| {
                model.apply(
                    parse_event(&format!(
                        r#"{{"type":"text_chunk","convId":"c","text":"{}"}}"#,
                        chunk.replace('\n', "\\n")
                    )),
                    now,
                );
                // Following the newest line, as the app does.
                view.chat_scroll.offset = view.chat_max(model, height);
            });
            let redrawn: i32 = damage.rows.iter().map(|(a, b)| b - a).sum();
            assert!(redrawn < 400, "only the reply's end: {damage:?}");
        }
        update_matches(&mut model, &mut view, height, |_, view| view.dots += 1);
        // Pressing a tool row, the back button and the stop button.
        let mut previous: Option<Hit> = None;
        for pressed in [
            Some(Hit::Action(Action::ToggleExpanded("tool:t1".into()))),
            Some(Hit::Action(Action::Back)),
            Some(Hit::Action(Action::Abort)),
            None,
        ] {
            let damage = update_matches_with(
                &mut model,
                &mut view,
                height,
                previous.as_ref(),
                pressed.as_ref(),
                |_, _| {},
            );
            if pressed == Some(Hit::Action(Action::Back)) {
                assert!(damage.rows.contains(&(0, NAV_H as i32)));
            }
            previous = pressed;
        }
        let damage = update_matches(&mut model, &mut view, height, |model, _| {
            model.composer.push('h');
        });
        assert!(damage.moved.is_none() && damage.rows.len() == 1);
        update_matches(&mut model, &mut view, height, |model, _| {
            model.apply(
                parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","provider":"anthropic","model":"m","effort":"high","title":"Renamed","pinned":false,"sortOrder":0,"streaming":true,"unread":false}}"#),
                now,
            );
        });
    }

    #[test]
    fn a_scrolled_list_equals_a_redrawn_one() {
        let now = Instant::now();
        let mut model = Model::new();
        model.on_connected();
        let rows: Vec<String> = (0..30)
            .map(|i| format!(r#"{{"id":"c{i}","provider":"anthropic","model":"m","title":"Conversation {i}","pinned":{},"sortOrder":{i},"streaming":{},"unread":{}}}"#, i < 2, i % 7 == 0, i % 3 == 0))
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversations_list","conversations":[{}],"folders":[]}}"#,
                rows.join(",")
            )),
            now,
        );
        let mut view = ViewState::new();
        let height = 2216.0;
        let mut canvas = Canvas::new(1080, height as i32);
        canvas.clear(BG);
        view.list_scroll.offset = 100.0;
        draw(&mut canvas, &mut model, &mut view, None, now);
        for to in [161.0, 140.0, 900.0, 600.0] {
            let old = list_frame(&model, &view, None, height, now).unwrap();
            view.list_scroll.offset = to;
            let new = list_frame(&model, &view, None, height, now).unwrap();
            let damage =
                draw_list_update(&mut canvas, &mut model, &old, &new).expect("incremental");
            assert_eq!(damage.rows.len(), 1);
            let mut full = Canvas::new(1080, height as i32);
            full.clear(BG);
            draw(&mut full, &mut model, &mut view, None, now);
            assert!(full.pixels == canvas.pixels, "list frame at {to} differs");
        }
        // Pressing a row and letting go redraws that row alone.
        let press = Hit::Action(Action::OpenConversation("c5".into()));
        for (before, after) in [(None, Some(&press)), (Some(&press), None)] {
            let old = list_frame(&model, &view, before, height, now).unwrap();
            let new = list_frame(&model, &view, after, height, now).unwrap();
            let damage =
                draw_list_update(&mut canvas, &mut model, &old, &new).expect("incremental");
            assert_eq!(damage.rows.len(), 1);
            assert!(damage.rows[0].1 - damage.rows[0].0 <= ROW_H as i32 + 1);
            let mut full = Canvas::new(1080, height as i32);
            full.clear(BG);
            draw(&mut full, &mut model, &mut view, after, now);
            assert!(full.pixels == canvas.pixels, "pressed list frame differs");
        }
    }

    #[test]
    fn list_rows_and_compose_are_hit() {
        let now = Instant::now();
        let mut model = chat_model();
        model.handle(Action::Back);
        let mut view = ViewState::default();
        let y = (list_top(&model) + ROW_H / 2.0) as i16;
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 400, y, now),
            Some(Hit::Action(Action::OpenConversation("c".into())))
        );
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 1020, 90, now),
            Some(Hit::Action(Action::NewConversation))
        );
        let mut canvas = Canvas::new(1080, 2216);
        draw(&mut canvas, &mut model, &mut view, None, now);
    }

    #[test]
    fn the_model_sheet_lists_signed_in_providers() {
        let now = Instant::now();
        let mut model = chat_model();
        model.apply(
            parse_event(r#"{"type":"tools_available","providers":[{"id":"anthropic","label":"Anthropic","defaultModel":"m","models":[{"id":"m","label":"Haiku","defaultEffort":"high","supportedEfforts":[{"effort":"low"},{"effort":"high"}]}]},{"id":"openai","label":"OpenAI","defaultModel":"g","models":[{"id":"g","label":"GPT","defaultEffort":"low","supportedEfforts":[]}]}],"authByProvider":{"anthropic":true,"openai":false},"tools":[]}"#),
            now,
        );
        model.handle(Action::ShowPicker);
        let items = sheet_content(&model);
        assert!(
            items
                .iter()
                .any(|(_, item)| matches!(item, SheetItem::Model { id, .. } if id == "m"))
        );
        assert!(
            !items
                .iter()
                .any(|(_, item)| matches!(item, SheetItem::Model { id, .. } if id == "g"))
        );
        let mut view = ViewState::default();
        let chips = effort_chips(
            &["low".into(), "high".into()],
            SHEET_TOP + SHEET_HEADER + SHEET_SECTION,
        );
        let (cx, cy) = chips[1].0.center();
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, cx as i16, cy as i16, now),
            Some(Hit::Action(Action::PickEffort("high".into())))
        );
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 500, 100, now),
            Some(Hit::SheetBackdrop)
        );
    }
}
