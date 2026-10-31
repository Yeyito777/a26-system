//! The anchored menu and the confirm dialog (roadmap §3.6.17).
//!
//! The anchored menu (a list row's long press, the list's ⋯): the page
//! dimmed to the app background at 64 % except the row it belongs to,
//! a 676 px menu 25 px below it (above when it does not fit), its shadow,
//! items with an icon and an uppercase label. It fades in over 170 ms while
//! growing from 94 % (drawn inset), out over 160 ms; the dim and the shadow
//! are drawn once, so a frame of it redraws only the menu's rows.
//!
//! The confirm dialog: a centred box with a title, a message, a
//! destructive button and Cancel, fading in the same way.

#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::gfx::{Canvas, RectF};

use super::super::anim::{Animator, Tween, ease_out};
use super::super::gesture::Gesture;
use super::super::model::{Action, UiAct};
use super::super::theme::{
    self, HAIRLINE, MENU_CLAMP, MENU_ITEM, MENU_OFFSET, MENU_QUICK, MENU_W, T_MENU, T_NOTE, T_ROW,
    TARGET,
};
use super::icons::{self, Icon};
use super::overlay::{Body, Cx, Overlay, Response};
use super::widgets::{self, Button, ButtonKind};

/// A menu row and the quick row at the current text size.
fn menu_item() -> f32 {
    theme::grown(MENU_ITEM, 44.0)
}

fn menu_quick() -> f32 {
    theme::grown(MENU_QUICK, 44.0)
}

pub use super::sheet::Tone;

const IN_FOR: Duration = Duration::from_millis(170);
/// Where an item's label (and the rule above it) starts.
const LABEL_X: f32 = 141.0;
const OUT_FOR: Duration = Duration::from_millis(160);
const SHADOW: f32 = 78.0;
const SHADOW_DY: f32 = 19.0;

/// An item of an anchored menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub icon: Icon,
    pub label: String,
    pub tone: Tone,
    pub action: Action,
}

impl MenuItem {
    pub fn new(icon: Icon, label: impl Into<String>, tone: Tone, action: Action) -> Self {
        Self {
            icon,
            label: label.into(),
            tone,
            action,
        }
    }

    fn colors(&self) -> (crate::gfx::Rgb, crate::gfx::Rgb) {
        let p = theme::p();
        match self.tone {
            Tone::Plain => (p.secondary_accent, p.text),
            Tone::Accent => (p.accent, p.text),
            Tone::Warning => (p.warning, p.text),
            Tone::Success => (p.success, p.text),
            Tone::Muted => (p.muted, p.text),
            Tone::Destructive => (p.error, p.error),
        }
    }
}

/// How far an entrance or exit is (0 hidden, 1 shown) at `now`.
fn shown(opened: Option<Instant>, closed: Option<Instant>, now: Instant) -> f32 {
    match (opened, closed) {
        (_, Some(closed)) => 1.0 - Tween::new(closed, OUT_FOR, 0.0, 1.0).value(now),
        (Some(opened), None) => Tween::new(opened, IN_FOR, 0.0, 1.0)
            .with_curve(ease_out)
            .value(now),
        (None, None) => 0.0,
    }
}

/// Quantized so frames of the same state agree (and a key can name it).
fn steps(shown: f32) -> u32 {
    (shown * 64.0).round() as u32
}

pub struct AnchoredMenu {
    anchor: RectF,
    /// The quick row's items (icon over label, side by side), then the rest.
    quick: Vec<MenuItem>,
    items: Vec<MenuItem>,
    /// Keep the anchor undimmed (a list row stays visible, lifted).
    cut_out: bool,
    rect: RectF,
    opened: Option<Instant>,
    closed: Option<Instant>,
    pressed: Option<usize>,
}

impl AnchoredMenu {
    /// A menu for what is at `anchor` (window coordinates).
    pub fn new(anchor: RectF, items: Vec<MenuItem>, cut_out: bool) -> Self {
        Self {
            anchor,
            quick: Vec::new(),
            items,
            cut_out,
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
            opened: None,
            closed: None,
            pressed: None,
        }
    }

    /// A quick row on top (§3.4: 172 px): the most used items side by side,
    /// each an icon over its label.
    pub fn quick(mut self, quick: Vec<MenuItem>) -> Self {
        self.quick = quick;
        self
    }

    fn height(&self) -> f32 {
        let quick = if self.quick.is_empty() {
            0.0
        } else {
            menu_quick()
        };
        quick + self.items.len() as f32 * menu_item()
    }

    /// Where the menu goes for an anchor in a window `width` × `bottom`.
    pub fn place(anchor: RectF, count: usize, width: f32, bottom: f32) -> RectF {
        Self::place_height(anchor, count as f32 * MENU_ITEM, width, bottom)
    }

    /// `place` for a menu `h` tall.
    pub fn place_height(anchor: RectF, h: f32, width: f32, bottom: f32) -> RectF {
        Self::place_box(anchor, MENU_W, h, width, bottom)
    }

    /// `place` for a menu `w` × `h`.
    fn place_box(anchor: RectF, w: f32, h: f32, width: f32, bottom: f32) -> RectF {
        // Under a wide anchor (a row) from its content's left; under a
        // button, its right edges with the button's.
        let x = if anchor.w > w {
            anchor.x + theme::LIST_PAD
        } else {
            anchor.x + anchor.w - w
        };
        let x = x.clamp(MENU_CLAMP, width - MENU_CLAMP - w);
        let below = anchor.y + anchor.h + MENU_OFFSET;
        let y = if below + h <= bottom - MENU_CLAMP {
            below
        } else {
            (anchor.y - MENU_OFFSET - h).max(MENU_CLAMP)
        };
        RectF::new(x.round(), y.round(), w, h)
    }

    /// The menu's width: 676, wider when large text needs it for its
    /// labels (up to the window less its margins).
    fn width(&self, window: f32) -> f32 {
        let longest = self
            .items
            .iter()
            .map(|item| widgets::measure(&item.label, T_MENU))
            .fold(0.0, f32::max);
        (longest + LABEL_X + 34.0)
            .ceil()
            .clamp(MENU_W, (window - 2.0 * MENU_CLAMP).max(MENU_W))
    }

    fn shown(&self, now: Instant) -> f32 {
        shown(self.opened, self.closed, now)
    }

    /// The menu's box as drawn at `shown` (growing from 94 %).
    fn drawn_rect(&self, shown: f32) -> RectF {
        let inset_x = ((1.0 - shown) * 0.06 * self.rect.w / 2.0).round();
        let inset_y = ((1.0 - shown) * 0.06 * self.rect.h / 2.0).round();
        RectF::new(
            self.rect.x + inset_x,
            self.rect.y + inset_y,
            self.rect.w - 2.0 * inset_x,
            self.rect.h - 2.0 * inset_y,
        )
    }

    /// The item at (`x`, `y`): quick ones first, then the rest.
    fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let quick = self.quick.len();
        let mut top = self.rect.y;
        if quick > 0 {
            if y < top + menu_quick() {
                let cell = self.rect.w / quick as f32;
                return Some((((x - self.rect.x) / cell).floor() as usize).min(quick - 1));
            }
            top += menu_quick();
        }
        if self.items.is_empty() {
            return None;
        }
        Some(quick + (((y - top) / menu_item()).floor() as usize).min(self.items.len() - 1))
    }

    fn item(&self, index: usize) -> &MenuItem {
        match index.checked_sub(self.quick.len()) {
            Some(index) => &self.items[index],
            None => &self.quick[index],
        }
    }
}

impl Overlay for AnchoredMenu {
    fn label(&self) -> String {
        let labels: Vec<&str> = self
            .quick
            .iter()
            .chain(&self.items)
            .map(|item| item.label.as_str())
            .collect();
        format!("menu: {}", labels.join(", "))
    }

    fn scoped(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &Cx) {
        if self.opened.is_none() {
            self.opened = Some(cx.now);
        }
        let w = self.width(cx.width);
        self.rect = Self::place_box(self.anchor, w, self.height(), cx.width, cx.safe_bottom());
    }

    fn draw_backdrop(&self, canvas: &mut Canvas, cx: &Cx) {
        let dim = theme::p().app_background;
        let (w, h) = (cx.width as i32, cx.height as i32);
        if self.cut_out {
            let top = self.anchor.y.round() as i32;
            let bottom = (self.anchor.y + self.anchor.h).round() as i32;
            canvas.fill_rect(0, 0, w, top, dim, 0.64);
            canvas.fill_rect(0, bottom, w, h - bottom, dim, 0.64);
        } else {
            canvas.fill_rect(0, 0, w, h, dim, 0.64);
        }
        widgets::shadow(canvas, self.rect, SHADOW, SHADOW_DY, 0.42);
    }

    fn body(&self, cx: &Cx) -> Body {
        let mut key = DefaultHasher::new();
        (
            steps(self.shown(cx.now)),
            self.pressed,
            format!("{:?}{:?}", self.quick, self.items),
        )
            .hash(&mut key);
        Body {
            rows: (
                self.rect.y.floor() as i32 - 1,
                (self.rect.y + self.rect.h).ceil() as i32 + 1,
            ),
            solid: None,
            scroller: None,
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        let c = theme::c();
        let shown = steps(self.shown(cx.now)) as f32 / 64.0;
        if shown <= 0.0 {
            return;
        }
        let rect = self.drawn_rect(shown);
        let alpha = shown;
        let fill = |canvas: &mut Canvas, r: RectF, color| {
            canvas.fill_rect(
                r.x.round() as i32,
                r.y.round() as i32,
                r.w.round() as i32,
                r.h.round() as i32,
                color,
                alpha,
            );
        };
        fill(canvas, rect, p.surface_background);
        widgets::clipped(canvas, rect, |canvas| {
            let quick = self.quick.len();
            let mut top = self.rect.y;
            if quick > 0 {
                let cell = (self.rect.w / quick as f32).floor();
                for (index, item) in self.quick.iter().enumerate() {
                    let x = self.rect.x + index as f32 * cell;
                    let w = if index + 1 == quick {
                        self.rect.x + self.rect.w - x
                    } else {
                        cell
                    };
                    if self.pressed == Some(index) {
                        fill(
                            canvas,
                            RectF::new(x, top, w, menu_quick()),
                            c.menu_item_pressed,
                        );
                    }
                    if index > 0 {
                        fill(
                            canvas,
                            RectF::new(x, top, HAIRLINE, menu_quick()),
                            c.menu_border,
                        );
                    }
                    let (icon_color, label_color) = item.colors();
                    let cx = x + w / 2.0;
                    icons::draw(canvas, item.icon, cx, top + 62.0, 56.0, icon_color, alpha);
                    let ty = theme::T_FOOTER.weight(crate::text::Weight::MonoSemiBold);
                    let label = widgets::fit(&item.label, ty, w - 22.0);
                    widgets::text_centered(
                        canvas,
                        &label,
                        cx,
                        ty.baseline(top + 106.0),
                        ty,
                        label_color,
                        alpha,
                    );
                }
                top += menu_quick();
            }
            for (index, item) in self.items.iter().enumerate() {
                let y = top + index as f32 * menu_item();
                let index = index + quick;
                if self.pressed == Some(index) {
                    fill(
                        canvas,
                        RectF::new(self.rect.x, y, self.rect.w, menu_item()),
                        c.menu_item_pressed,
                    );
                }
                if index == quick && quick > 0 {
                    fill(
                        canvas,
                        RectF::new(self.rect.x, y, self.rect.w, HAIRLINE),
                        c.menu_border,
                    );
                } else if index > 0 {
                    fill(
                        canvas,
                        RectF::new(self.rect.x + LABEL_X, y, self.rect.w - LABEL_X, HAIRLINE),
                        c.menu_border,
                    );
                }
                let (icon_color, label_color) = item.colors();
                icons::draw(
                    canvas,
                    item.icon,
                    self.rect.x + 76.0,
                    y + menu_item() / 2.0,
                    56.0,
                    icon_color,
                    alpha,
                );
                let label = widgets::fit(&item.label, T_MENU, self.rect.w - LABEL_X - 34.0);
                widgets::text(
                    canvas,
                    &label,
                    self.rect.x + LABEL_X,
                    T_MENU.baseline(y + (menu_item() - T_MENU.line()) / 2.0),
                    T_MENU,
                    label_color,
                    alpha,
                );
            }
        });
        for r in [
            RectF::new(rect.x, rect.y, rect.w, HAIRLINE),
            RectF::new(rect.x, rect.y + rect.h - HAIRLINE, rect.w, HAIRLINE),
            RectF::new(rect.x, rect.y, HAIRLINE, rect.h),
            RectF::new(rect.x + rect.w - HAIRLINE, rect.y, HAIRLINE, rect.h),
        ] {
            fill(canvas, r, c.menu_border);
        }
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        if self.closed.is_some() {
            return Response::Handled;
        }
        match *gesture {
            Gesture::Down { x, y } => {
                self.pressed = self.item_at(x, y);
                Response::Handled
            }
            Gesture::Tap { x, y } => {
                self.pressed = None;
                match self.item_at(x, y) {
                    Some(index) => {
                        self.close(cx.now);
                        Response::Action(self.item(index).action.clone())
                    }
                    None => Response::Action(Action::Ui(UiAct::Dismiss)),
                }
            }
            Gesture::Back => Response::Action(Action::Ui(UiAct::Dismiss)),
            Gesture::DragStart { .. } | Gesture::Cancel | Gesture::LongPressEnd { .. } => {
                self.pressed = None;
                Response::Handled
            }
            _ => Response::Handled,
        }
    }

    fn close(&mut self, now: Instant) {
        if self.closed.is_none() {
            let shown = self.shown(now);
            let backdate = OUT_FOR.mul_f32((1.0 - shown).clamp(0.0, 1.0));
            self.closed = Some(now.checked_sub(backdate).unwrap_or(now));
            self.pressed = None;
        }
    }

    fn closing(&self) -> bool {
        self.closed.is_some()
    }

    fn finished(&self, now: Instant) -> bool {
        self.closed.is_some_and(|closed| now >= closed + OUT_FOR)
    }

    fn animate(&self, cx: &Cx, animator: &mut Animator) {
        match (self.opened, self.closed) {
            (_, Some(closed)) => animator.follow(&Tween::new(closed, OUT_FOR, 0.0, 1.0), cx.now),
            (Some(opened), None) => animator.follow(&Tween::new(opened, IN_FOR, 0.0, 1.0), cx.now),
            (None, None) => animator.frame(cx.now),
        }
    }
}

// ----- Confirm dialog -----------------------------------------------------------

/// "Delete this conversation?": a centred box, a destructive button and
/// Cancel.
pub struct Confirm {
    title: String,
    message: String,
    confirm: String,
    action: Action,
    rect: RectF,
    opened: Option<Instant>,
    closed: Option<Instant>,
    pressed: Option<usize>,
}

impl Confirm {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        confirm: impl Into<String>,
        action: Action,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            confirm: confirm.into(),
            action,
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
            opened: None,
            closed: None,
            pressed: None,
        }
    }

    const WIDTH: f32 = 900.0;
    const PAD: f32 = 44.0;

    fn lines(&self) -> Vec<String> {
        widgets::wrap_plain(&self.message, T_NOTE, Self::WIDTH - 2.0 * Self::PAD)
    }

    /// Cancel's and the confirming button's rects.
    fn buttons(&self) -> (RectF, RectF) {
        let gap = 22.0;
        let w = ((self.rect.w - 2.0 * Self::PAD - gap) / 2.0).floor();
        let y = self.rect.y + self.rect.h - Self::PAD - TARGET;
        (
            RectF::new(self.rect.x + Self::PAD, y, w, TARGET),
            RectF::new(self.rect.x + Self::PAD + w + gap, y, w, TARGET),
        )
    }
}

impl Overlay for Confirm {
    fn label(&self) -> String {
        self.title.clone()
    }

    fn scoped(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &Cx) {
        if self.opened.is_none() {
            self.opened = Some(cx.now);
        }
        let title_ty = T_ROW.weight(crate::text::Weight::MonoBold);
        let h = Self::PAD
            + title_ty.line()
            + 17.0
            + self.lines().len() as f32 * T_NOTE.line()
            + 34.0
            + TARGET
            + Self::PAD;
        self.rect = RectF::new(
            ((cx.width - Self::WIDTH) / 2.0).round(),
            ((cx.bottom() - h) / 2.0).round(),
            Self::WIDTH,
            h.round(),
        );
    }

    fn draw_backdrop(&self, canvas: &mut Canvas, cx: &Cx) {
        canvas.fill_rect(0, 0, cx.width as i32, cx.height as i32, 0x000000, 0.55);
        widgets::shadow(canvas, self.rect, SHADOW, SHADOW_DY, 0.42);
    }

    fn body(&self, cx: &Cx) -> Body {
        let mut key = DefaultHasher::new();
        (
            steps(shown(self.opened, self.closed, cx.now)),
            self.pressed,
            &self.title,
            &self.message,
        )
            .hash(&mut key);
        Body {
            rows: (
                self.rect.y.floor() as i32 - 1,
                (self.rect.y + self.rect.h).ceil() as i32 + 1,
            ),
            solid: None,
            scroller: None,
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        let c = theme::c();
        let alpha = steps(shown(self.opened, self.closed, cx.now)) as f32 / 64.0;
        if alpha <= 0.0 {
            return;
        }
        canvas.fill_rect(
            self.rect.x as i32,
            self.rect.y as i32,
            self.rect.w as i32,
            self.rect.h as i32,
            p.surface_background,
            alpha,
        );
        let title_ty = T_ROW.weight(crate::text::Weight::MonoBold);
        let mut y = self.rect.y + Self::PAD;
        let x = self.rect.x + Self::PAD;
        widgets::text(
            canvas,
            &widgets::fit(&self.title, title_ty, self.rect.w - 2.0 * Self::PAD),
            x,
            title_ty.baseline(y),
            title_ty,
            p.text,
            alpha,
        );
        y += title_ty.line() + 17.0;
        for line in self.lines() {
            widgets::text(
                canvas,
                &line,
                x,
                T_NOTE.baseline(y),
                T_NOTE,
                c.text_dim,
                alpha,
            );
            y += T_NOTE.line();
        }
        if alpha >= 1.0 {
            let (cancel, confirm) = self.buttons();
            widgets::button(
                canvas,
                cancel,
                &Button::new("Cancel", ButtonKind::Secondary).pressed(self.pressed == Some(0)),
                p.surface_background,
            );
            widgets::button(
                canvas,
                confirm,
                &Button::new(&self.confirm, ButtonKind::Destructive)
                    .pressed(self.pressed == Some(1)),
                p.surface_background,
            );
        }
        widgets::border(canvas, self.rect, HAIRLINE, c.menu_border);
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        if self.closed.is_some() {
            return Response::Handled;
        }
        let (cancel, confirm) = self.buttons();
        let at = |x: f32, y: f32| {
            if cancel.contains(x, y) {
                Some(0)
            } else if confirm.contains(x, y) {
                Some(1)
            } else {
                None
            }
        };
        match *gesture {
            Gesture::Down { x, y } => {
                self.pressed = at(x, y);
                Response::Handled
            }
            Gesture::Tap { x, y } => {
                self.pressed = None;
                match at(x, y) {
                    Some(1) => {
                        self.close(cx.now);
                        Response::Action(self.action.clone())
                    }
                    Some(_) => Response::Action(Action::Ui(UiAct::Dismiss)),
                    None if !self.rect.contains(x, y) => {
                        Response::Action(Action::Ui(UiAct::Dismiss))
                    }
                    None => Response::Handled,
                }
            }
            Gesture::Back => Response::Action(Action::Ui(UiAct::Dismiss)),
            _ => {
                self.pressed = None;
                Response::Handled
            }
        }
    }

    fn close(&mut self, now: Instant) {
        if self.closed.is_none() {
            self.closed = Some(now);
            self.pressed = None;
        }
    }

    fn closing(&self) -> bool {
        self.closed.is_some()
    }

    fn finished(&self, now: Instant) -> bool {
        self.closed.is_some_and(|closed| now >= closed + OUT_FOR)
    }

    fn animate(&self, cx: &Cx, animator: &mut Animator) {
        match (self.opened, self.closed) {
            (_, Some(closed)) => animator.follow(&Tween::new(closed, OUT_FOR, 0.0, 1.0), cx.now),
            (Some(opened), None) => animator.follow(&Tween::new(opened, IN_FOR, 0.0, 1.0), cx.now),
            (None, None) => animator.frame(cx.now),
        }
    }
}
