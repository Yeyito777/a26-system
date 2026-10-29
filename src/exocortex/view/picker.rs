//! The command and macro picker (WP-D; PAR-198–202, roadmap §3.6.16): a
//! panel directly above the composer (and its error label) while the caret
//! completes a slash word. Commands and Macros tabs with counts (or what
//! the argument is, "Provider", and the words already typed), rows with the
//! name in the accent and its description, a ✕, and the hint footer
//! (usage, or "Unknown command…" in the warning colour). A row inserts its
//! text and never runs or sends. Floating: touches elsewhere go on to the
//! page; it closes when the composer says so.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::gfx::{Canvas, RectF};

use super::super::anim::{self, Animator, Tween};
use super::super::commands::Item;
use super::super::commands::run::PickerState;
use super::super::gesture::{Axis, Gesture};
use super::super::model::composer::Act;
use super::super::model::{Action, Page};
use super::super::theme::{
    self, COMPOSER_MARGIN, HAIRLINE, S1, S2, S3, T_CAPTION, T_ROW, TARGET, W,
};
use super::super::ui::icons::{self, Icon, Style};
use super::super::ui::overlay::{Body, Cx, Overlay, Response};
use super::super::ui::widgets;
use super::chat_top;
use super::composer;

const IN_FOR: Duration = Duration::from_millis(160);
const OUT_FOR: Duration = Duration::from_millis(120);
/// The head (tabs or title, ✕) and a row.
const HEAD: f32 = TARGET;
const ROW: f32 = 155.0;
/// At most this tall, and not shown when there is less room than this.
const MAX_H: f32 = 664.0;
const MIN_H: f32 = 244.0;
/// Gap above the island (or its error label).
const GAP: f32 = 17.0;
const SHADOW: f32 = 61.0;
const PAD: f32 = 34.0;

/// What a touch on the picker is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Target {
    Tab(bool),
    Close,
    Row(usize),
}

pub struct Picker {
    serial: u64,
    rect: RectF,
    /// The list's scroll, px.
    offset: f32,
    drag: Option<(f32, f32)>,
    pressed: Option<Target>,
    opened: Option<Instant>,
    closed: Option<Instant>,
    state: Option<PickerState>,
    /// A hash of `state` (what the rows show).
    state_key: u64,
    /// The hint footer, wrapped.
    hint: Vec<String>,
    macros: bool,
}

impl Picker {
    pub fn new(serial: u64) -> Self {
        Self {
            serial,
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
            offset: 0.0,
            drag: None,
            pressed: None,
            opened: None,
            closed: None,
            state: None,
            state_key: 0,
            hint: Vec::new(),
            macros: false,
        }
    }

    fn shown(&self, now: Instant) -> f32 {
        let value = match (self.opened, self.closed) {
            (_, Some(closed)) => 1.0 - Tween::new(closed, OUT_FOR, 0.0, 1.0).value(now),
            (Some(opened), None) => Tween::new(opened, IN_FOR, 0.0, 1.0)
                .with_curve(anim::ease_out)
                .value(now),
            (None, None) => 0.0,
        };
        (value * 32.0).round() / 32.0
    }

    fn tabs(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| !s.commands.is_empty() && !s.macros.is_empty())
    }

    fn rows(&self) -> &[Item] {
        self.state.as_ref().map_or(&[], |s| s.rows(self.macros))
    }

    fn hint_h(&self) -> f32 {
        if self.hint.is_empty() {
            0.0
        } else {
            self.hint.len() as f32 * T_CAPTION.line() + 2.0 * S2
        }
    }

    fn content_h(&self) -> f32 {
        self.rows().len() as f32 * ROW + self.hint_h()
    }

    fn max_offset(&self) -> f32 {
        (self.content_h() - (self.rect.h - HEAD)).max(0.0)
    }

    fn target_at(&self, x: f32, y: f32) -> Option<Target> {
        let r = self.rect;
        if !r.contains(x, y) {
            return None;
        }
        if y < r.y + HEAD {
            if x > r.x + r.w - TARGET {
                return Some(Target::Close);
            }
            if self.tabs() {
                let split = r.x + PAD + self.tab_width(false) + S2;
                return Some(Target::Tab(x >= split));
            }
            return None;
        }
        let index = ((y - r.y - HEAD + self.offset) / ROW).floor();
        (index >= 0.0 && (index as usize) < self.rows().len())
            .then_some(Target::Row(index as usize))
    }

    fn tab_label(&self, macros: bool) -> (String, String) {
        let state = self.state.as_ref();
        let count = state.map_or(0, |s| {
            if macros {
                s.macros.len()
            } else {
                s.commands.len()
            }
        });
        (
            if macros { "Macros" } else { "Commands" }.to_owned(),
            count.to_string(),
        )
    }

    fn tab_width(&self, macros: bool) -> f32 {
        let (label, count) = self.tab_label(macros);
        let ty = T_CAPTION.weight(crate::text::Weight::MonoSemiBold);
        widgets::measure(&label, ty) + S1 + widgets::measure(&count, T_CAPTION)
    }
}

impl Overlay for Picker {
    fn label(&self) -> String {
        let names: Vec<&str> = self.rows().iter().map(|i| i.name.as_str()).collect();
        format!("picker: {}", names.join(", "))
    }

    fn scoped(&self) -> bool {
        true
    }

    fn modal(&self) -> bool {
        false
    }

    fn layout(&mut self, cx: &Cx) {
        if self.opened.is_none() {
            self.opened = Some(cx.now);
        }
        let model = cx.model;
        if (model.composer.picker != Some(self.serial) || model.page != Page::Chat)
            && self.closed.is_none()
        {
            self.closed = Some(cx.now);
        }
        if self.closed.is_some() && self.state.is_some() {
            return;
        }
        if model.composer.picker_state != self.state {
            let state = model.composer.picker_state.clone();
            if state.as_ref().map(|s| &s.completion.query)
                != self.state.as_ref().map(|s| &s.completion.query)
            {
                self.offset = 0.0;
            }
            let mut key = DefaultHasher::new();
            format!("{state:?}").hash(&mut key);
            self.state_key = key.finish();
            self.hint = state
                .as_ref()
                .and_then(|s| s.hint.as_ref())
                .map(|(text, _)| {
                    widgets::wrap_plain(text, T_CAPTION, W - 2.0 * COMPOSER_MARGIN - 2.0 * PAD)
                })
                .unwrap_or_default();
            self.state = state;
        }
        self.macros = model.composer.picker_macros;
        // Only with the keyboard up for the composer (PAR-198).
        if !cx.keyboard {
            self.rect = RectF::new(0.0, 0.0, 0.0, 0.0);
            return;
        }
        let g = composer::geometry(model, cx.height, cx.keyboard);
        let bottom = g.band_top() + 17.0 - GAP;
        let room = bottom - chat_top() - S3;
        let wanted = HEAD + self.rows().len().min(3) as f32 * ROW + self.hint_h();
        let h = wanted.min(room).min(MAX_H).round();
        if h < MIN_H.min(wanted) {
            self.rect = RectF::new(0.0, 0.0, 0.0, 0.0);
            return;
        }
        self.rect = RectF::new(
            COMPOSER_MARGIN,
            (bottom - h).round(),
            W - 2.0 * COMPOSER_MARGIN,
            h,
        );
        self.offset = self.offset.clamp(0.0, self.max_offset());
    }

    fn body(&self, cx: &Cx) -> Body {
        if self.rect.h <= 0.0 {
            return Body::empty();
        }
        let mut key = DefaultHasher::new();
        (
            self.serial,
            self.state_key,
            self.macros,
            self.offset.to_bits(),
            self.pressed,
            self.rect.y.to_bits(),
            self.rect.h.to_bits(),
            (self.shown(cx.now) * 32.0) as u32,
            theme::generation(),
        )
            .hash(&mut key);
        Body {
            rows: (
                (self.rect.y - SHADOW).floor() as i32,
                (self.rect.y + self.rect.h).ceil() as i32,
            ),
            solid: None,
            scroller: None,
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let alpha = self.shown(cx.now);
        let Some(state) = &self.state else {
            return;
        };
        if alpha <= 0.0 || self.rect.h <= 0.0 {
            return;
        }
        let p = theme::p();
        let c = theme::c();
        let r = self.rect;
        widgets::shadow_above(canvas, r.y, SHADOW, 0.38 * alpha);
        canvas.fill_rect(
            r.x as i32,
            r.y as i32,
            r.w as i32,
            r.h as i32,
            p.surface_background,
            alpha,
        );
        // The head: tabs, or what is being completed.
        let baseline = T_CAPTION.baseline(r.y + (HEAD - T_CAPTION.line()) / 2.0);
        let semi = T_CAPTION.weight(crate::text::Weight::MonoSemiBold);
        let dim = |percent| theme::mix(p.text, percent, p.surface_background);
        if self.tabs() {
            let mut x = r.x + PAD;
            for macros in [false, true] {
                let (label, count) = self.tab_label(macros);
                let on = macros == self.macros;
                let w = widgets::text(
                    canvas,
                    &label,
                    x,
                    baseline,
                    semi,
                    if on { p.accent } else { dim(50) },
                    alpha,
                );
                widgets::text(
                    canvas,
                    &count,
                    x + w + S1,
                    baseline,
                    T_CAPTION,
                    dim(40),
                    alpha,
                );
                x += self.tab_width(macros) + S2 * 2.0;
            }
        } else {
            let mut x = r.x + PAD;
            x += widgets::text(canvas, &state.title, x, baseline, semi, dim(70), alpha) + S1;
            let rows = self.rows();
            if !rows.is_empty() {
                x += widgets::text(
                    canvas,
                    &rows.len().to_string(),
                    x,
                    baseline,
                    T_CAPTION,
                    dim(40),
                    alpha,
                ) + S2;
            }
            if !state.prefix.is_empty() {
                let room = r.x + r.w - TARGET - x;
                let prefix = widgets::fit(&state.prefix, T_CAPTION, room);
                widgets::text(
                    canvas,
                    &prefix,
                    x,
                    baseline,
                    T_CAPTION,
                    theme::mix(p.accent, 70, p.surface_background),
                    alpha,
                );
            }
        }
        let close_down = self.pressed == Some(Target::Close);
        icons::draw_styled(
            canvas,
            Icon::X,
            r.x + r.w - TARGET / 2.0,
            r.y + HEAD / 2.0,
            Style::new(40.0, dim(60))
                .stroke(5.0)
                .alpha(alpha * if close_down { 0.5 } else { 1.0 }),
        );
        let rule = theme::mix(p.border, 40, p.surface_background);
        canvas.fill_rect(
            r.x as i32,
            (r.y + HEAD) as i32,
            r.w as i32,
            HAIRLINE as i32,
            rule,
            alpha,
        );
        // The rows, scrolled, clipped to the list.
        let list = RectF::new(r.x, r.y + HEAD + HAIRLINE, r.w, r.h - HEAD - HAIRLINE);
        let row_rule = theme::mix(p.border, 28, p.surface_background);
        widgets::clipped(canvas, list, |canvas| {
            let top = r.y + HEAD - self.offset;
            for (index, item) in self.rows().iter().enumerate() {
                let y = top + index as f32 * ROW;
                if y + ROW < list.y || y > list.y + list.h {
                    continue;
                }
                if self.pressed == Some(Target::Row(index)) {
                    canvas.fill_rect(
                        r.x as i32,
                        y as i32,
                        r.w as i32,
                        ROW as i32,
                        p.selection_background,
                        alpha,
                    );
                }
                let name = label(&item.name, &state.prefix);
                let room = r.w - 2.0 * PAD - 60.0;
                let name = widgets::fit(&name, T_ROW, room);
                widgets::text(
                    canvas,
                    &name,
                    r.x + PAD,
                    T_ROW.baseline(y + 22.0),
                    T_ROW,
                    p.accent,
                    alpha,
                );
                if !item.desc.is_empty() {
                    let desc = widgets::fit(&item.desc, T_CAPTION, room);
                    widgets::text(
                        canvas,
                        &desc,
                        r.x + PAD,
                        T_CAPTION.baseline(y + 22.0 + T_ROW.line() + 6.0),
                        T_CAPTION,
                        theme::c().text_dim,
                        alpha,
                    );
                }
                widgets::text_right(
                    canvas,
                    "↖",
                    r.x + r.w - PAD,
                    T_ROW.baseline(y + (ROW - T_ROW.line()) / 2.0),
                    T_CAPTION,
                    dim(35),
                    alpha,
                );
                canvas.fill_rect(
                    r.x as i32,
                    (y + ROW - HAIRLINE) as i32,
                    r.w as i32,
                    HAIRLINE as i32,
                    row_rule,
                    alpha,
                );
            }
            if let Some((_, warning)) = &state.hint {
                let mut y = top + self.rows().len() as f32 * ROW + S2;
                for line in &self.hint {
                    widgets::text(
                        canvas,
                        line,
                        r.x + PAD,
                        T_CAPTION.baseline(y),
                        T_CAPTION,
                        if *warning { p.warning } else { c.text_dim },
                        alpha,
                    );
                    y += T_CAPTION.line();
                }
            }
        });
        widgets::border(canvas, r, HAIRLINE, c.menu_border);
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } => {
                if !self.rect.contains(x, y) {
                    return Response::Pass;
                }
                self.pressed = self.target_at(x, y);
                Response::Handled
            }
            Gesture::Tap { x, y } => {
                self.pressed = None;
                match self.target_at(x, y) {
                    Some(Target::Close) => Response::Action(Action::Composer(Act::PickerDismiss)),
                    Some(Target::Tab(macros)) => {
                        Response::Action(Action::Composer(Act::PickerTab(macros)))
                    }
                    Some(Target::Row(index)) => {
                        let insert = self.rows()[index].insert().to_owned();
                        Response::Action(Action::Composer(Act::Complete(insert)))
                    }
                    None => Response::Handled,
                }
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                y,
                ..
            } => {
                self.pressed = None;
                self.drag = Some((y, self.offset));
                Response::Handled
            }
            Gesture::Drag { y, .. } => {
                if let Some((start, from)) = self.drag {
                    self.offset = (from - (y - start)).clamp(0.0, self.max_offset()).round();
                }
                Response::Handled
            }
            Gesture::DragEnd { .. } | Gesture::Cancel | Gesture::LongPressEnd { .. } => {
                self.drag = None;
                self.pressed = None;
                Response::Handled
            }
            _ => {
                let _ = cx;
                Response::Handled
            }
        }
    }

    fn close(&mut self, now: Instant) {
        if self.closed.is_none() {
            self.closed = Some(now);
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

/// A row's name without the words the head already shows.
fn label(name: &str, prefix: &str) -> String {
    if !prefix.is_empty()
        && name.len() > prefix.len()
        && name[..prefix.len()].eq_ignore_ascii_case(prefix)
        && name.as_bytes()[prefix.len()] == b' '
    {
        name[prefix.len() + 1..].to_owned()
    } else {
        name.to_owned()
    }
}
