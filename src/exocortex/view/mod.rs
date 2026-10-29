//! The Exocortex app's view (roadmap §3, §4): the three pages in Whale —
//! Settings, the conversation list, a conversation — and the overlays over
//! them, drawn on a CPU canvas a frame at a time, only what changed
//! (`frame.rs`).
//!
//! Who owns what: the frame pipeline, page composition, hit testing and
//! pieces' drawing (`ops.rs`) are A's; the list is B's (`list.rs`);
//! markdown is C1's (`markdown_layout.rs`); the transcript is C2's
//! (`transcript.rs`, `blocks.rs`, `tools.rs`); the composer is D's
//! (`composer.rs`); the chat header is E's (`chat_header.rs`), as is the
//! Settings page (`screens/settings.rs`). Layout is cached per piece, keyed
//! by what it shows and `theme::generation()`, so a streaming reply lays
//! out only the paragraph that grew.

pub mod blocks;
pub mod chat_header;
pub mod composer;
pub mod frame;
pub mod list;
pub mod markdown_layout;
pub mod media;
pub mod ops;
pub mod tools;
pub mod transcript;

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;

use crate::comms::Scroll;
use crate::gfx::{Canvas, RectF};

use super::anim::Animator;
use super::model::{Action, Model, Page, ops as ops_model};
use super::screens::settings;
use super::theme::{self, HAIRLINE, T_CAPTION, T_EYEBROW, TARGET, TOP_BAR};
use super::ui::overlay::{Cx, OverlayStack};
use super::ui::widgets;
use ops::{Laid, op_rows};
use transcript::{CachedPiece, CachedPlan, ChatLayout, draw_transcript};

pub use frame::{Frame, FrameDamage};
pub use list::{ListFrame, list_max};

pub use theme::W;

/// Where the chat's transcript starts: under its top bar and hairline.
pub const CHAT_TOP: f32 = TOP_BAR + HAIRLINE;

/// Something on screen a touch is on.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Action(Action),
    /// The composer's editor.
    Composer,
    /// The transcript, away from anything tappable.
    Transcript,
}

/// The view's own state: scroll positions, focus, the overlays, layout
/// caches.
#[derive(Default)]
pub struct ViewState {
    pub list_scroll: Scroll,
    pub chat_scroll: Scroll,
    /// The Settings page's (E).
    pub settings_scroll: Scroll,
    /// The composer has focus and Moon's keyboard is up.
    pub keyboard: bool,
    /// The chat follows its newest message.
    pub follow: bool,
    /// The overlays on screen.
    pub overlays: OverlayStack,
    /// What a press held 150 ms lifted (a list row; B draws it).
    pub lifted: Option<Action>,
    /// Horizontal scroll offsets of code blocks, tables and display maths,
    /// by their key (C1), and a counter bumped when one changes.
    pub xscroll: HashMap<u64, f32>,
    pub xscroll_version: u64,
    /// What wants the next frame, gathered while a frame is made.
    pub animator: Animator,
    layout: ChatLayout,
    /// Pieces by what they show (`piece_key`): an entry that comes back
    /// unchanged (a window resent, a reply committed, a chat reopened) is
    /// not laid out again.
    pieces: HashMap<u64, CachedPiece>,
    /// Entries' plans by stamp.
    plans: HashMap<u64, CachedPlan>,
    pass: u64,
    /// The theme generation the caches were laid out in.
    generation: u64,
    /// Phase of the "working" dots.
    pub dots: u8,
}

/// A hash of an action (for frame keys; actions carry rects, which do not
/// hash).
pub fn action_key(action: Option<&Action>) -> u64 {
    action.map_or(0, |action| hash_of(format!("{action:?}")))
}

pub(crate) fn hash_of(value: impl Hash) -> u64 {
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

    /// Forgets every cached layout when the theme or the text size changed
    /// (pieces bake their colours and sizes in).
    pub(crate) fn check_generation(&mut self) {
        let generation = theme::generation();
        if generation != self.generation {
            self.generation = generation;
            self.pieces.clear();
            self.plans.clear();
            self.layout = ChatLayout::default();
        }
    }

    /// What overlays see this frame.
    pub fn cx<'a>(&self, model: &'a Model, width: f32, height: f32, now: Instant) -> Cx<'a> {
        Cx {
            model,
            width,
            height,
            keyboard: self.keyboard,
            now,
        }
    }

    /// Takes what the model asked of the interface: overlays to open or
    /// close (it also closes them when the model changed page or
    /// conversation). Returns whether anything changed.
    pub fn take_ui(&mut self, model: &mut Model, now: Instant) -> bool {
        let ui = &mut model.ui;
        let mut changed = false;
        if std::mem::take(&mut ui.close_all) {
            self.overlays.close_all(now);
            changed = true;
        }
        if std::mem::take(&mut ui.close_scoped) {
            self.overlays.close_scoped(now);
            changed = true;
        }
        if std::mem::take(&mut ui.close_top) {
            changed |= self.overlays.close_top(now);
        }
        for overlay in ui.opening.drain(..) {
            self.overlays.push(overlay);
            changed = true;
        }
        changed
    }
}

/// Where the transcript is: under the chat's top bar, above the composer.
pub fn transcript_bounds(model: &Model, height: f32, keyboard: bool) -> (f32, f32) {
    (
        CHAT_TOP,
        composer::geometry(model, height, keyboard).band_top(),
    )
}

/// Draws the page (honouring the canvas's clip: a band of it can be drawn
/// again alone).
pub fn draw_page(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
) {
    let height = canvas.height as f32;
    match model.page {
        Page::List => list::draw_list(canvas, model, view, pressed, height, now),
        Page::Chat => draw_chat(canvas, model, view, pressed, height, now),
        Page::Settings => settings::draw(canvas, model, view, pressed_action(pressed), now),
    }
}

/// Draws the whole frame: the page, then the overlays.
#[allow(dead_code)]
pub fn draw(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
) {
    frame::draw_full(canvas, model, view, pressed, now);
}

pub(crate) fn pressed_action(pressed: Option<&Hit>) -> Option<&Action> {
    match pressed {
        Some(Hit::Action(action)) => Some(action),
        _ => None,
    }
}

/// Asks for the frames what is on the page needs (pulses, spinners, the
/// caret): every area's own `animate`.
pub fn animate(model: &Model, view: &mut ViewState, now: Instant) {
    match model.page {
        Page::Chat => {
            chat_header::animate(model, now, &mut view.animator);
            composer::animate(model, view.keyboard, now, &mut view.animator);
        }
        Page::List => list::animate(model, now, &mut view.animator),
        Page::Settings => settings::animate(model, now, &mut view.animator),
    }
}

// ----- The chat page ------------------------------------------------------------

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
    header: u64,
    composer: u64,
    xscroll: u64,
}

impl ChatFrame {
    /// Whether the two frames draw the same pixels.
    pub fn same_as(&self, other: &ChatFrame) -> bool {
        self.bounds == other.bounds
            && self.height == other.height
            && self.offset == other.offset
            && self.dots == other.dots
            && self.pressed == other.pressed
            && self.header == other.header
            && self.composer == other.composer
            && self.xscroll == other.xscroll
            && self.items.len() == other.items.len()
            && self
                .items
                .iter()
                .zip(&other.items)
                .all(|((a_y, a), (b_y, b))| a_y == b_y && Rc::ptr_eq(a, b))
    }
}

/// The current chat frame's record, or None when it shows something the
/// incremental path does not draw (another page, the connection pill, a
/// loading or empty chat).
pub fn chat_frame(
    model: &Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) -> Option<ChatFrame> {
    if model.page != Page::Chat || model.offline || !model.chat.loaded || model.chat.id.is_none() {
        return None;
    }
    view.layout(model);
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    let pressed = pressed_action(pressed).cloned();
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
        header: chat_header::frame_key(model, pressed.as_ref(), now),
        composer: composer::frame_key(model, view.keyboard, pressed.as_ref(), now),
        xscroll: view.xscroll_version,
        pressed,
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
    now: Instant,
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
        let from = (CHAT_TOP + change - new.offset).floor() as i32;
        if from < bottom {
            redraw.push((from.max(top), bottom));
        }
    }
    // Horizontal scrollers moved: their pieces' rows.
    if old.xscroll != new.xscroll {
        for (y, laid) in &new.items {
            if laid.xkeys.is_empty() {
                continue;
            }
            let y0 = (CHAT_TOP + y + laid.ink.0 - new.offset).floor() as i32;
            let y1 = (CHAT_TOP + y + laid.ink.1 - new.offset).ceil() as i32;
            if y1 > top && y0 < bottom {
                redraw.push((y0.max(top), y1.min(bottom)));
            }
        }
    }
    // A row pressed or let go: its highlight's rows.
    let mut header_pressed = false;
    let mut composer_pressed = false;
    if old.pressed != new.pressed {
        for action in old.pressed.iter().chain(&new.pressed) {
            header_pressed |= chat_header::owns(action);
            composer_pressed |= composer::owns(action);
            for (y, laid) in &new.items {
                for (rect, hit) in &laid.hits {
                    if hit == action {
                        let y0 = (CHAT_TOP + y + rect.y - new.offset).floor() as i32 - 1;
                        let y1 = (CHAT_TOP + y + rect.y + rect.h - new.offset).ceil() as i32 + 1;
                        if y1 > top && y0 < bottom {
                            redraw.push((y0.max(top), y1.min(bottom)));
                        }
                    }
                }
            }
        }
    }
    let merged = frame::merge(redraw);
    let background = theme::p().app_background;
    for &(y0, y1) in &merged {
        canvas.fill_rect(0, y0, W as i32, y1 - y0, background, 1.0);
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
    if old.header != new.header || header_pressed {
        chat_header::draw(canvas, model, new.pressed.as_ref(), now);
        damage.rows.push(widgets::TOP_BAR_ROWS);
    }
    if old.composer != new.composer || composer_pressed {
        composer::draw(
            canvas,
            model,
            view.keyboard,
            new.pressed.as_ref(),
            new.height as f32,
            now,
        );
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
    let p = theme::p();
    widgets::fill(canvas, RectF::new(0.0, 0.0, W, height), p.app_background);
    view.layout(model);
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    // Whole pixels, so a scrolled frame equals a shifted one.
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
    widgets::clipped(canvas, RectF::new(0.0, top, W, bottom - top), |canvas| {
        if !model.chat.loaded && model.chat.id.is_some() {
            widgets::empty_state(
                canvas,
                RectF::new(0.0, top, W, bottom - top),
                "",
                "Loading…",
                None,
                p.app_background,
            );
        } else if model.chat.id.is_none() && model.chat.echoes.is_empty() {
            new_chat_state(canvas, model, (top, bottom), pressed_action.as_ref());
        }
    });
    chat_header::draw(canvas, model, pressed_action.as_ref(), now);
    if model.offline {
        let message = if model.was_connected {
            "Reconnecting…"
        } else {
            "Connecting…"
        };
        widgets::pill(
            canvas,
            W / 2.0,
            top + 22.0,
            message,
            true,
            p.app_background,
            now,
        );
    }
    composer::draw(
        canvas,
        model,
        view.keyboard,
        pressed_action.as_ref(),
        height,
        now,
    );
}

/// The new-chat state (`real-04-new-chat`): "NEW CHAT" over a box naming
/// the model, which opens the model sheet.
fn new_chat_state(
    canvas: &mut Canvas,
    model: &Model,
    (top, bottom): (f32, f32),
    pressed: Option<&Action>,
) {
    let p = theme::p();
    let c = theme::c();
    let rect = new_chat_box(model, (top, bottom));
    let label_ty = T_EYEBROW.tracked(3.3);
    widgets::text_centered(
        canvas,
        "New chat",
        W / 2.0,
        label_ty.baseline(rect.y - 34.0 - label_ty.line()),
        label_ty,
        p.accent,
        1.0,
    );
    let down = pressed == Some(&Action::Ops(ops_model::Act::OpenInspector));
    if down {
        widgets::fill(canvas, rect, c.row_pressed);
    }
    widgets::border(canvas, rect, HAIRLINE, c.rule_box);
    widgets::text_centered(
        canvas,
        &new_chat_label(model),
        W / 2.0,
        T_CAPTION.baseline(rect.y + (rect.h - T_CAPTION.line()) / 2.0),
        T_CAPTION,
        p.muted,
        1.0,
    );
}

fn new_chat_label(model: &Model) -> String {
    model.new_chat_label()
}

/// The new-chat state's model box.
fn new_chat_box(model: &Model, (top, bottom): (f32, f32)) -> RectF {
    let w = (widgets::measure(&new_chat_label(model), T_CAPTION) + 2.0 * theme::S4).round();
    let cy = ((top + bottom) / 2.0).round();
    RectF::new(((W - w) / 2.0).round(), cy - TARGET / 2.0 + 40.0, w, TARGET)
}

// ----- Touches --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollArea {
    List,
    Chat,
    Settings,
}

/// The list a touch at `y` may scroll.
pub fn scroll_area(model: &Model, view: &ViewState, height: f32, y: f32) -> Option<ScrollArea> {
    match model.page {
        Page::List => {
            let (top, bottom) = list::bounds(model, height);
            (y >= top && y < bottom).then_some(ScrollArea::List)
        }
        Page::Chat => {
            let (top, bottom) = transcript_bounds(model, height, view.keyboard);
            (y >= top && y < bottom).then_some(ScrollArea::Chat)
        }
        Page::Settings => (y >= CHAT_TOP).then_some(ScrollArea::Settings),
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
    match model.page {
        Page::List => list::hit(model, view, height, x, y, now),
        Page::Settings => settings::hit(model, view, x, y, now).map(Hit::Action),
        Page::Chat => {
            if y < CHAT_TOP {
                return chat_header::hit(model, x, y).map(Hit::Action);
            }
            if let Some(hit) = composer::hit(model, height, view.keyboard, x, y) {
                return Some(hit);
            }
            let (top, bottom) = transcript_bounds(model, height, view.keyboard);
            if y < top || y >= bottom {
                return None;
            }
            if model.chat.id.is_none()
                && model.chat.echoes.is_empty()
                && new_chat_box(model, (top, bottom)).contains(x, y)
            {
                return Some(Hit::Action(Action::Ops(ops_model::Act::OpenInspector)));
            }
            view.layout(model);
            let offset = view.chat_scroll.value(now);
            for item in &view.layout.items {
                let iy = top + item.y - offset;
                if y < iy || y >= iy + item.laid.h {
                    continue;
                }
                for (rect, action) in &item.laid.hits {
                    if y >= iy + rect.y
                        && y < iy + rect.y + rect.h
                        && x >= rect.x
                        && x < rect.x + rect.w
                    {
                        return Some(Hit::Action(action.clone()));
                    }
                }
            }
            Some(Hit::Transcript)
        }
    }
}

/// What a press held 150 ms lifts (a list row), if anything.
pub fn liftable(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Action> {
    match model.page {
        Page::List => list::liftable(model, view, height, x, y, now),
        _ => None,
    }
}

/// What a long press at (`x`, `y`) does (a list row's menu), if anything.
pub fn long_press(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Action> {
    match model.page {
        Page::List => list::long_press(model, view, height, x, y, now),
        _ => None,
    }
}

/// A horizontal drag starting at (`x`, `y`): the scroller it moves (a code
/// block's or a table's key), if any (C1 fills pieces' `xkeys`).
pub fn horizontal_target(
    model: &Model,
    view: &mut ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<u64> {
    if model.page != Page::Chat {
        return None;
    }
    let _ = x;
    view.layout(model);
    let (top, _) = transcript_bounds(model, height, view.keyboard);
    let offset = view.chat_scroll.value(now);
    view.layout.items.iter().find_map(|item| {
        let iy = top + item.y - offset;
        (y >= iy && y < iy + item.laid.h)
            .then(|| item.laid.xkeys.first().copied())
            .flatten()
    })
}

/// Moves horizontal scroller `key` by `dx` (C1 clamps it to its content).
pub fn horizontal_drag(view: &mut ViewState, key: u64, dx: f32) {
    let offset = view.xscroll.entry(key).or_insert(0.0);
    let next = (*offset - dx).max(0.0).round();
    if next != *offset {
        *offset = next;
        view.xscroll_version = view.xscroll_version.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::markdown::{Span, Style};
    use crate::exocortex::model::{Nav, chat, composer as composer_act};
    use crate::exocortex::proto::parse_event;
    use crate::text::Weight;
    use list::{ROW_H, draw_list_update, list_frame, list_top};
    use markdown_layout::{BODY, lay_markdown, line_width, plain, wrap};
    use ops::Op;
    use theme::TRANSCRIPT_PAD as MARGIN;
    use tools::TOOL_ROW;

    const BG: crate::gfx::Rgb = 0x000000;

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
    fn markdown_lays_out_as_one_piece() {
        let laid = markdown_layout::lay(
            "Hello **world**\n\n- a",
            MARGIN,
            900.0,
            0.0,
            markdown_layout::MdStyle::Body,
        );
        assert!(laid.h > 0.0);
        assert!(laid.ops.len() >= 3);
        assert!(laid.ink.0 <= 0.0 && laid.ink.1 >= laid.h);
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
                weight: Weight::MonoBold,
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
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
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
        assert_eq!(
            group[0].1,
            Action::Chat(chat::Act::ToggleExpanded("group:t0".into()))
        );
        let (top, _) = transcript_bounds(&model, height, false);
        let y = top + view.layout.items[1].y + group[0].0.y + 10.0;
        let hit_action = hit(&mut model, &mut view, height, 300, y as i16, now);
        assert_eq!(
            hit_action,
            Some(Hit::Action(Action::Chat(chat::Act::ToggleExpanded(
                "group:t0".into()
            ))))
        );
        // The reply's pieces: the folded row, then the text.
        assert_eq!(view.layout.items.len(), 3);
        let before = view.layout.height;
        let text = view.layout.items[2].laid.clone();
        model.handle(Action::Chat(chat::Act::ToggleExpanded("group:t0".into())));
        view.layout(&model);
        // Opened: the row, a row per call, and the same text piece below.
        let hits: Vec<&Action> = view.layout.items[1..]
            .iter()
            .flat_map(|item| item.laid.hits.iter().map(|(_, action)| action))
            .collect();
        assert_eq!(hits.len(), 5);
        assert_eq!(
            hits[1],
            &Action::Chat(chat::Act::ToggleExpanded("tool:t0".into()))
        );
        assert!(view.layout.height > before + 3.0 * TOOL_ROW);
        assert!(Rc::ptr_eq(&view.layout.items[6].laid, &text));
        // The composer and send button are hit; the transcript elsewhere.
        let geometry = composer::geometry(&model, height, false);
        assert_eq!(
            hit(
                &mut model,
                &mut view,
                height,
                300,
                (geometry.island.y + 20.0) as i16,
                now
            ),
            Some(Hit::Composer)
        );
        model.composer.text = "x".into();
        assert_eq!(
            hit(
                &mut model,
                &mut view,
                height,
                geometry.send.0 as i16,
                geometry.send.1 as i16,
                now
            ),
            Some(Hit::Action(Action::Composer(composer_act::Act::Send)))
        );
        let mut canvas = Canvas::new(1080, height as i32);
        draw(&mut canvas, &mut model, &mut view, None, now);
        assert!(canvas.pixels.contains(&theme::p().accent));
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
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
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
        model.handle(Action::Chat(chat::Act::ToggleExpanded("group:t0".into())));
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
        let damage =
            draw_chat_update(&mut canvas, model, view, &old, &new, now).expect("incremental");
        let mut full = Canvas::new(1080, height as i32);
        full.clear(BG);
        draw(&mut full, model, view, after, now);
        let differing: Vec<usize> = (0..height as usize)
            .filter(|y| {
                full.pixels[y * 1080..(y + 1) * 1080] != canvas.pixels[y * 1080..(y + 1) * 1080]
            })
            .collect();
        let first_x = differing.first().and_then(|y| {
            (0..1080).find(|x| full.pixels[y * 1080 + x] != canvas.pixels[y * 1080 + x])
        });
        assert!(
            differing.is_empty(),
            "incremental frame differs in rows {:?}..{:?} ({} rows) from x {:?}: {damage:?}",
            differing.first(),
            differing.last(),
            differing.len(),
            first_x.map(|x| (
                x,
                full.pixels[differing[0] * 1080 + x],
                canvas.pixels[differing[0] * 1080 + x]
            )),
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
        model.handle(Action::Chat(chat::Act::ToggleExpanded("group:t0".into())));
        model.handle(Action::Chat(chat::Act::ToggleExpanded("tool:t1".into())));
        model.handle(Action::Chat(chat::Act::ToggleExpanded("tool:t2".into())));
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
            Some(Hit::Action(Action::Chat(chat::Act::ToggleExpanded(
                "tool:t1".into(),
            )))),
            Some(Hit::Action(Action::Nav(Nav::Back))),
            Some(Hit::Action(Action::Composer(composer_act::Act::Abort))),
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
            if pressed == Some(Hit::Action(Action::Nav(Nav::Back))) {
                assert!(damage.rows.contains(&widgets::TOP_BAR_ROWS));
            }
            previous = pressed;
        }
        let damage = update_matches(&mut model, &mut view, height, |model, _| {
            model.composer.text.push('h');
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
            let damage = draw_list_update(&mut canvas, &mut model, &mut view, &old, &new, now)
                .expect("incremental");
            assert_eq!(damage.rows.len(), 1);
            let mut full = Canvas::new(1080, height as i32);
            full.clear(BG);
            draw(&mut full, &mut model, &mut view, None, now);
            assert!(full.pixels == canvas.pixels, "list frame at {to} differs");
        }
        // Pressing a row and letting go redraws that row alone.
        let press = Hit::Action(Action::Nav(Nav::OpenConversation("c5".into())));
        for (before, after) in [(None, Some(&press)), (Some(&press), None)] {
            let old = list_frame(&model, &view, before, height, now).unwrap();
            let new = list_frame(&model, &view, after, height, now).unwrap();
            let damage = draw_list_update(&mut canvas, &mut model, &mut view, &old, &new, now)
                .expect("incremental");
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
        model.handle(Action::Nav(Nav::Back));
        let mut view = ViewState::default();
        let y = (list_top(&model) + ROW_H / 2.0) as i16;
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 400, y, now),
            Some(Hit::Action(Action::Nav(Nav::OpenConversation("c".into()))))
        );
        assert_eq!(
            hit(&mut model, &mut view, 2216.0, 1020, 90, now),
            Some(Hit::Action(Action::Nav(Nav::NewConversation)))
        );
        let mut canvas = Canvas::new(1080, 2216);
        draw(&mut canvas, &mut model, &mut view, None, now);
    }
}
