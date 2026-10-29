//! Sheets (roadmap §3.6.17): a panel rising from the bottom over a dimmed
//! page, with a grabber and an eyebrow title, its content scrolling inside;
//! closed by tapping the backdrop, dragging the grabber down, or Back. It
//! slides in over 280 ms and out over 220 ms as a moving block of rows (the
//! dim is drawn once), so a frame of the slide draws only the strip it
//! uncovers.
//!
//! What a sheet shows is a `SheetContent`: an action list (`ActionList`),
//! a text prompt (`Prompt`), or a screen of an area's own (the model
//! inspector, E's panels).

#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::comms::Scroll;
use crate::control::KeyboardPurpose;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;

use super::super::anim::{Animator, Tween};
use super::super::gesture::{Axis, Gesture};
use super::super::model::Action;
use super::super::theme::{
    self, ACTION_ICON, ACTION_ITEM, ACTION_PAD, GRABBER, GRABBER_TOP, HAIRLINE, S2, SHEET_MAX,
    T_EYEBROW, T_META, T_NOTE, T_ROW, TARGET, W,
};
use super::icons::{self, Icon};
use super::overlay::{Body, Cx, Overlay, Response, Scroller};
use super::widgets::{self, Button, ButtonKind};

const OPEN_FOR: Duration = Duration::from_millis(280);
const CLOSE_FOR: Duration = Duration::from_millis(220);
/// The shadow cast upwards from the panel's top.
const SHADOW: f32 = 110.0;
/// The grabber dragged this far down closes the sheet.
const DRAG_CLOSES: f32 = 150.0;
/// The header: grabber and title.
pub const HEADER: f32 = 122.0;

/// The header at the current text size (its title grows).
pub fn header() -> f32 {
    theme::grown(HEADER, 40.0)
}

/// An action list's item: a label and a detail line (100 px of text in
/// 144), growing with the text.
fn item_h() -> f32 {
    theme::grown(ACTION_ITEM, 100.0)
}

/// What a sheet's content answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    None,
    /// Draw again.
    Redraw,
    Action(Action),
    Close,
    /// Close, then perform.
    CloseWith(Action),
}

/// What a sheet shows.
pub trait SheetContent {
    /// The eyebrow title (may be empty).
    fn title(&self) -> String;

    /// Lays out at `width` for this frame; returns the content's height.
    fn layout(&mut self, cx: &Cx, width: f32) -> f32;

    /// Draws the content with its top left at (`x`, `top`), scrolled
    /// already; `pressed` is the item under the finger.
    fn draw(&self, canvas: &mut Canvas, cx: &Cx, x: f32, top: f32, pressed: Option<usize>);

    /// The item at (`x`, `y`) from the content's top left.
    fn item_at(&self, cx: &Cx, x: f32, y: f32) -> Option<usize>;

    /// Item `index` was tapped.
    fn activate(&mut self, cx: &Cx, index: usize) -> Reply;

    /// Everything its pixels depend on but the pressed item and the scroll.
    fn key(&self, cx: &Cx) -> u64;

    fn scoped(&self) -> bool {
        false
    }

    /// A key from Moon's keyboard (a prompt's field).
    fn key_input(&mut self, _input: &KeyInput, _cx: &Cx) -> Reply {
        Reply::None
    }

    fn keyboard(&self) -> Option<KeyboardPurpose> {
        None
    }

    /// When its content next changes by itself (a ticking time).
    fn wake(&self, _cx: &Cx) -> Option<Instant> {
        None
    }

    /// What to do once the sheet has gone (`Overlay::farewell`).
    fn farewell(&mut self) -> Option<Action> {
        None
    }
}

/// A sheet showing `C`.
pub struct Sheet<C: SheetContent> {
    pub content: C,
    opened: Option<Instant>,
    closed: Option<Instant>,
    scroll: Scroll,
    pressed: Option<usize>,
    /// The grabber dragged down by this much.
    drag: Option<(f32, f32)>,
    /// Laid out: content height, window size.
    content_h: f32,
    size: (f32, f32, bool),
    /// The vertical drag in progress scrolls the content.
    scrolling: bool,
}

impl<C: SheetContent> Sheet<C> {
    pub fn new(content: C) -> Self {
        Self {
            content,
            opened: None,
            closed: None,
            scroll: Scroll::default(),
            pressed: None,
            drag: None,
            content_h: 0.0,
            size: (W, 0.0, false),
            scrolling: false,
        }
    }

    /// The panel's height at rest.
    fn panel_h(&self, cx: &Cx) -> f32 {
        let bottom_pad = cx.height - cx.bottom() + S2;
        (header() + self.content_h + bottom_pad)
            .min((cx.height * SHEET_MAX).round())
            .round()
    }

    /// Where the content scrolls: below the header, above the gesture
    /// reserve (or the keyboard); it moves with the panel.
    fn viewport(&self, cx: &Cx, top: f32) -> (f32, f32) {
        let panel_h = self.panel_h(cx);
        (
            top + header(),
            top + panel_h - (cx.height - cx.bottom()) - S2,
        )
    }

    fn max_scroll(&self, cx: &Cx) -> f32 {
        let rest = cx.height - self.panel_h(cx);
        let (y0, y1) = self.viewport(cx, rest);
        (self.content_h - (y1 - y0)).max(0.0)
    }

    /// The panel's top at `now`: sliding in, at rest (dragged), sliding out.
    fn top(&self, cx: &Cx) -> f32 {
        let rest = cx.height - self.panel_h(cx);
        let travel = cx.height - rest;
        let shown = match (self.opened, self.closed) {
            (_, Some(closed)) => 1.0 - Tween::new(closed, CLOSE_FOR, 0.0, 1.0).value(cx.now),
            (Some(opened), None) => Tween::new(opened, OPEN_FOR, 0.0, 1.0).value(cx.now),
            (None, None) => 0.0,
        };
        let drag = self.drag.map_or(0.0, |(_, d)| d.max(0.0));
        (rest + travel * (1.0 - shown) + drag).round()
    }

    fn offset(&self, now: Instant) -> f32 {
        self.scroll.value(now).round()
    }

    /// At rest (not sliding or dragged): its shadow shows. A shadow is
    /// drawn once, not per animation frame (§3.5), so a sliding panel has
    /// none and moves as a block.
    fn at_rest(&self, cx: &Cx) -> bool {
        self.closed.is_none()
            && self.drag.is_none()
            && self
                .opened
                .is_some_and(|opened| Tween::new(opened, OPEN_FOR, 0.0, 1.0).done(cx.now))
    }
}

impl<C: SheetContent> Overlay for Sheet<C> {
    fn label(&self) -> String {
        self.content.title()
    }

    fn scoped(&self) -> bool {
        self.content.scoped()
    }

    fn layout(&mut self, cx: &Cx) {
        if self.opened.is_none() {
            self.opened = Some(cx.now);
        }
        self.content_h = self.content.layout(cx, W - 2.0 * ACTION_PAD).ceil();
        self.size = (cx.width, cx.height, cx.keyboard);
    }

    fn draw_backdrop(&self, canvas: &mut Canvas, cx: &Cx) {
        canvas.fill_rect(0, 0, cx.width as i32, cx.height as i32, 0x000000, 0.55);
    }

    fn body(&self, cx: &Cx) -> Body {
        let top = self.top(cx) as i32;
        let height = cx.height as i32;
        let (y0, y1) = self.viewport(cx, top as f32);
        let mut key = DefaultHasher::new();
        (
            self.content.key(cx),
            self.content.title(),
            self.pressed,
            self.content_h as i32,
            height,
        )
            .hash(&mut key);
        Body {
            rows: (
                (top - if self.at_rest(cx) { SHADOW as i32 } else { 0 }).max(0),
                height,
            ),
            // The whole panel, though its bottom may be below the window
            // while it slides: it moves as one block.
            solid: Some((top, top + self.panel_h(cx) as i32)),
            scroller: Some(Scroller {
                y0: y0 as i32,
                y1: y1 as i32,
                offset: self.offset(cx.now) as i32,
            }),
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        let c = theme::c();
        let top = self.top(cx);
        if top >= cx.height {
            return;
        }
        if self.at_rest(cx) {
            widgets::shadow_above(canvas, top, SHADOW, 0.45);
        }
        widgets::fill(
            canvas,
            RectF::new(0.0, top, cx.width, cx.height - top),
            p.sidebar_background,
        );
        widgets::hairline(canvas, 0.0, cx.width, top, c.rule_sheet_top);
        let (gw, gh) = GRABBER;
        canvas.fill_rrect(
            RectF::new(((cx.width - gw) / 2.0).round(), top + GRABBER_TOP, gw, gh),
            6.0,
            c.grabber,
            1.0,
        );
        let title = self.content.title();
        if !title.is_empty() {
            let title = widgets::fit(&title, T_EYEBROW, cx.width - 2.0 * ACTION_PAD);
            widgets::text(
                canvas,
                &title,
                ACTION_PAD,
                T_EYEBROW.baseline(top + 55.0),
                T_EYEBROW,
                p.muted,
                1.0,
            );
        }
        let (y0, y1) = self.viewport(cx, top);
        let offset = self.offset(cx.now);
        widgets::clipped(canvas, RectF::new(0.0, y0, cx.width, y1 - y0), |canvas| {
            self.content
                .draw(canvas, cx, ACTION_PAD, y0 - offset, self.pressed);
        });
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        if self.closed.is_some() {
            return Response::Handled;
        }
        let top = self.top(cx);
        let (y0, y1) = self.viewport(cx, top);
        let offset = self.offset(cx.now);
        let item_at = |sheet: &Self, x: f32, y: f32| {
            (y >= y0 && y < y1)
                .then(|| sheet.content.item_at(cx, x - ACTION_PAD, y - y0 + offset))
                .flatten()
        };
        match *gesture {
            Gesture::Down { x, y } => {
                if y >= top && y < top + header() {
                    self.drag = Some((y, 0.0));
                }
                self.pressed = item_at(self, x, y);
                Response::Handled
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                start_y,
                y,
                ..
            } => {
                self.pressed = None;
                if let Some((from, _)) = self.drag {
                    self.drag = Some((from, y - from));
                } else if start_y >= y0 {
                    self.scrolling = true;
                    self.scroll.begin(start_y, cx.now);
                    self.scroll.moved(y, self.max_scroll(cx), cx.now);
                }
                Response::Handled
            }
            Gesture::Drag { y, .. } => {
                if let Some((from, _)) = self.drag {
                    self.drag = Some((from, y - from));
                } else if self.scrolling {
                    self.scroll.moved(y, self.max_scroll(cx), cx.now);
                }
                Response::Handled
            }
            Gesture::DragEnd { .. } => {
                if let Some((_, dragged)) = self.drag.take() {
                    if dragged > DRAG_CLOSES {
                        return Response::Action(Action::Ui(super::super::model::UiAct::Dismiss));
                    }
                } else if self.scrolling {
                    self.scrolling = false;
                    self.scroll.end(self.max_scroll(cx), cx.now);
                }
                Response::Handled
            }
            Gesture::Tap { x, y } => {
                self.pressed = None;
                self.drag = None;
                if y < top {
                    return Response::Action(Action::Ui(super::super::model::UiAct::Dismiss));
                }
                let Some(index) = item_at(self, x, y) else {
                    return Response::Handled;
                };
                match self.content.activate(cx, index) {
                    Reply::None | Reply::Redraw => Response::Handled,
                    Reply::Action(action) => Response::Action(action),
                    Reply::Close => {
                        Response::Action(Action::Ui(super::super::model::UiAct::Dismiss))
                    }
                    Reply::CloseWith(action) => {
                        self.close(cx.now);
                        Response::Action(action)
                    }
                }
            }
            Gesture::Back => Response::Action(Action::Ui(super::super::model::UiAct::Dismiss)),
            Gesture::Cancel | Gesture::DragStart { .. } => {
                self.pressed = None;
                self.drag = None;
                Response::Handled
            }
            _ => Response::Handled,
        }
    }

    fn key(&mut self, input: &KeyInput, cx: &Cx) -> Response {
        match self.content.key_input(input, cx) {
            Reply::None => Response::Pass,
            Reply::Redraw => Response::Handled,
            Reply::Action(action) => Response::Action(action),
            Reply::Close => Response::Action(Action::Ui(super::super::model::UiAct::Dismiss)),
            Reply::CloseWith(action) => {
                self.close(cx.now);
                Response::Action(action)
            }
        }
    }

    fn keyboard(&self) -> Option<KeyboardPurpose> {
        self.content.keyboard()
    }

    fn close(&mut self, now: Instant) {
        if self.closed.is_none() {
            // Out from where it is (a sheet closed mid-slide goes back from
            // there): the exit starts as far along as the entrance got.
            let shown = self.opened.map_or(1.0, |opened| {
                Tween::new(opened, OPEN_FOR, 0.0, 1.0).value(now)
            });
            let backdate = CLOSE_FOR.mul_f32((1.0 - shown).clamp(0.0, 1.0));
            self.closed = Some(now.checked_sub(backdate).unwrap_or(now));
            self.pressed = None;
        }
    }

    fn closing(&self) -> bool {
        self.closed.is_some()
    }

    fn finished(&self, now: Instant) -> bool {
        self.closed.is_some_and(|closed| now >= closed + CLOSE_FOR)
    }

    fn animate(&self, cx: &Cx, animator: &mut Animator) {
        match (self.opened, self.closed) {
            (_, Some(closed)) => animator.follow(&Tween::new(closed, CLOSE_FOR, 0.0, 1.0), cx.now),
            (Some(opened), None) => {
                animator.follow(&Tween::new(opened, OPEN_FOR, 0.0, 1.0), cx.now)
            }
            (None, None) => animator.frame(cx.now),
        }
        if self.closed.is_none()
            && let Some(at) = self.content.wake(cx)
        {
            animator.request(at);
        }
    }

    fn farewell(&mut self) -> Option<Action> {
        self.content.farewell()
    }

    fn step(&mut self, cx: &Cx) -> bool {
        let max = self.max_scroll(cx);
        self.scroll.step(max, cx.now)
    }
}

// ----- Action lists -------------------------------------------------------------

/// How an action item is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Icon in the secondary accent, label in text.
    Plain,
    Accent,
    Warning,
    /// Icon in the success colour (Next unread).
    Success,
    /// Icon in the muted colour (Settings, Copy id).
    Muted,
    /// Destructive: icon and label in the error colour.
    Destructive,
}

impl Tone {
    fn icon(self) -> crate::gfx::Rgb {
        let p = theme::p();
        match self {
            Tone::Plain => p.secondary_accent,
            Tone::Accent => p.accent,
            Tone::Warning => p.warning,
            Tone::Success => p.success,
            Tone::Muted => p.muted,
            Tone::Destructive => p.error,
        }
    }
}

/// An item of an action list.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionItem {
    pub icon: Option<Icon>,
    pub label: String,
    pub detail: Option<String>,
    pub tone: Tone,
    /// Checked (a choice that is current).
    pub checked: bool,
    pub action: Action,
}

impl ActionItem {
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        Self {
            icon: None,
            label: label.into(),
            detail: None,
            tone: Tone::Plain,
            checked: false,
            action,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// An action list (`fake-09-chat-menu`): a title, an optional line under
/// it, items with an icon, a label and a detail, and Cancel.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionList {
    pub title: String,
    pub subtitle: Option<String>,
    pub items: Vec<ActionItem>,
    pub cancel: bool,
    pub scoped: bool,
}

impl ActionList {
    pub fn new(title: impl Into<String>, items: Vec<ActionItem>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            items,
            cancel: true,
            scoped: false,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Closes when the conversation changes (a chat's menu).
    pub fn scoped(mut self) -> Self {
        self.scoped = true;
        self
    }

    fn head(&self) -> f32 {
        if self.subtitle.is_some() {
            theme::grown(100.0, 60.0)
        } else {
            0.0
        }
    }

    /// The index the Cancel row answers to.
    fn cancel_index(&self) -> usize {
        self.items.len()
    }
}

/// An action list in a sheet.
pub fn action_sheet(list: ActionList) -> Sheet<ActionList> {
    Sheet::new(list)
}

impl SheetContent for ActionList {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn layout(&mut self, _cx: &Cx, _width: f32) -> f32 {
        self.head() + self.items.len() as f32 * item_h() + if self.cancel { item_h() } else { 0.0 }
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        let width = W - 2.0 * x;
        if let Some(subtitle) = &self.subtitle {
            let line = widgets::fit(subtitle, theme::T_BODY, width);
            widgets::text(
                canvas,
                &line,
                x,
                theme::T_BODY.baseline(top + 8.0),
                theme::T_BODY,
                p.muted,
                1.0,
            );
        }
        let mut y = top + self.head();
        let rows = self.items.len() + usize::from(self.cancel);
        for index in 0..rows {
            widgets::hairline(canvas, 0.0, W, y, c.rule_sheet_row);
            if pressed == Some(index) {
                widgets::fill(
                    canvas,
                    RectF::new(0.0, y + HAIRLINE, W, item_h() - HAIRLINE),
                    p.selection_background,
                );
            }
            let Some(item) = self.items.get(index) else {
                let label = "Cancel";
                widgets::text_centered(
                    canvas,
                    label,
                    W / 2.0,
                    T_ROW.baseline(y + (item_h() - T_ROW.line()) / 2.0),
                    T_ROW,
                    p.muted,
                    1.0,
                );
                y += item_h();
                continue;
            };
            let mut text_x = x;
            if let Some(icon) = item.icon {
                icons::draw(
                    canvas,
                    icon,
                    x + ACTION_ICON / 2.0,
                    y + item_h() / 2.0,
                    ACTION_ICON,
                    item.tone.icon(),
                    1.0,
                );
                text_x = x + ACTION_ICON + 44.0;
            }
            let label_color = if item.tone == Tone::Destructive {
                p.error
            } else {
                p.text
            };
            let right = W - x - if item.checked { 76.0 } else { 0.0 };
            let (label_top, detail) = match &item.detail {
                Some(detail) => (
                    y + (item_h() - T_ROW.line() - T_META.line()) / 2.0,
                    Some(detail),
                ),
                None => (y + (item_h() - T_ROW.line()) / 2.0, None),
            };
            let label = widgets::fit(&item.label, T_ROW, right - text_x);
            widgets::text(
                canvas,
                &label,
                text_x,
                T_ROW.baseline(label_top),
                T_ROW,
                label_color,
                1.0,
            );
            if let Some(detail) = detail {
                let detail = widgets::fit(detail, T_META, right - text_x);
                widgets::text(
                    canvas,
                    &detail,
                    text_x,
                    T_META.baseline(label_top + T_ROW.line()),
                    T_META,
                    p.muted,
                    1.0,
                );
            }
            if item.checked {
                icons::draw(
                    canvas,
                    Icon::Check,
                    W - x - 30.0,
                    y + item_h() / 2.0,
                    52.0,
                    p.accent,
                    1.0,
                );
            }
            y += item_h();
        }
    }

    fn item_at(&self, _cx: &Cx, _x: f32, y: f32) -> Option<usize> {
        let y = y - self.head();
        if y < 0.0 {
            return None;
        }
        let index = (y / item_h()).floor() as usize;
        (index < self.items.len() + usize::from(self.cancel)).then_some(index)
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        if index == self.cancel_index() {
            return Reply::Close;
        }
        match self.items.get(index) {
            Some(item) => Reply::CloseWith(item.action.clone()),
            None => Reply::None,
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        format!("{:?}", self.items).hash(&mut hasher);
        (&self.title, &self.subtitle, self.cancel).hash(&mut hasher);
        hasher.finish()
    }

    fn scoped(&self) -> bool {
        self.scoped
    }
}

// ----- Prompts ------------------------------------------------------------------

/// A text prompt (rename, new folder): a message, a field typed into with
/// Moon's keyboard, Cancel and a confirming button; submitting performs the
/// action its `submit` makes of the text.
pub struct Prompt {
    pub title: String,
    pub message: String,
    pub text: String,
    pub confirm: String,
    pub submit: Box<dyn Fn(String) -> Action>,
    width: f32,
}

impl Prompt {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        text: impl Into<String>,
        confirm: impl Into<String>,
        submit: Box<dyn Fn(String) -> Action>,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            text: text.into(),
            confirm: confirm.into(),
            submit,
            width: W,
        }
    }

    const FIELD: f32 = 127.0;

    fn field_top(&self) -> f32 {
        if self.message.is_empty() {
            0.0
        } else {
            T_NOTE.line() + 22.0
        }
    }

    fn buttons_top(&self) -> f32 {
        self.field_top() + Self::FIELD + 34.0
    }

    /// Cancel's and the confirming button's rects, from the content's left.
    fn buttons(&self) -> (RectF, RectF) {
        let gap = 22.0;
        let w = ((self.width - gap) / 2.0).floor();
        let y = self.buttons_top();
        (
            RectF::new(0.0, y, w, TARGET),
            RectF::new(w + gap, y, w, TARGET),
        )
    }

    fn submit(&self) -> Reply {
        let text = self.text.trim().to_owned();
        if text.is_empty() {
            return Reply::None;
        }
        Reply::CloseWith((self.submit)(text))
    }
}

/// A prompt in a sheet.
pub fn prompt_sheet(prompt: Prompt) -> Sheet<Prompt> {
    Sheet::new(prompt)
}

impl SheetContent for Prompt {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn layout(&mut self, _cx: &Cx, width: f32) -> f32 {
        self.width = width;
        self.buttons_top() + TARGET
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        if !self.message.is_empty() {
            widgets::text(
                canvas,
                &widgets::fit(&self.message, T_NOTE, self.width),
                x,
                T_NOTE.baseline(top),
                T_NOTE,
                c.text_dim,
                1.0,
            );
        }
        let field = RectF::new(x, top + self.field_top(), self.width, Self::FIELD);
        widgets::boxed(canvas, field, p.surface_background, c.field_focused);
        let ty = T_ROW.weight(theme::T_BODY.weight);
        let shown = if self.text.is_empty() { "" } else { &self.text };
        let inner = self.width - 56.0;
        // The end of what is typed stays in view.
        let mut visible = shown.to_owned();
        while widgets::measure(&visible, ty) > inner - 10.0 && !visible.is_empty() {
            visible.remove(0);
        }
        let baseline = ty.baseline(field.y + (Self::FIELD - ty.line()) / 2.0);
        let w = widgets::text(canvas, &visible, field.x + 28.0, baseline, ty, p.text, 1.0);
        widgets::fill(
            canvas,
            RectF::new(
                (field.x + 28.0 + w + 2.0).round(),
                field.y + (Self::FIELD - 52.0) / 2.0,
                5.0,
                52.0,
            ),
            p.accent,
        );
        let (cancel, confirm) = self.buttons();
        let offset = |r: RectF| RectF::new(r.x + x, r.y + top, r.w, r.h);
        widgets::button(
            canvas,
            offset(cancel),
            &Button::new("Cancel", ButtonKind::Secondary).pressed(pressed == Some(0)),
            p.sidebar_background,
        );
        widgets::button(
            canvas,
            offset(confirm),
            &Button::new(&self.confirm, ButtonKind::Primary)
                .pressed(pressed == Some(1))
                .disabled(self.text.trim().is_empty()),
            p.sidebar_background,
        );
    }

    fn item_at(&self, _cx: &Cx, x: f32, y: f32) -> Option<usize> {
        let (cancel, confirm) = self.buttons();
        if cancel.contains(x, y) {
            Some(0)
        } else if confirm.contains(x, y) {
            Some(1)
        } else {
            None
        }
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        match index {
            0 => Reply::Close,
            _ => self.submit(),
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.title, &self.message, &self.text, &self.confirm).hash(&mut hasher);
        hasher.finish()
    }

    fn key_input(&mut self, input: &KeyInput, _cx: &Cx) -> Reply {
        match input {
            KeyInput::Character(character) => {
                self.text.push(*character);
                Reply::Redraw
            }
            KeyInput::Backspace => {
                self.text.pop();
                Reply::Redraw
            }
            KeyInput::Enter => self.submit(),
        }
    }

    fn keyboard(&self) -> Option<KeyboardPurpose> {
        Some(KeyboardPurpose::Text)
    }
}
