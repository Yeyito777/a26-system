//! Whale widgets (roadmap §3.6), drawn straight onto a canvas in the current
//! theme: text in a type token, hairlines, top bars and the page rail,
//! buttons, the segmented control, the switch, spinners, activity dots,
//! badges, eyebrows, empty states, pills and the shadows of floating things.
//!
//! Everything lands on whole pixels or is anti-aliased from exact
//! positions, so a widget drawn again in a partial frame gives the same
//! pixels; animated ones take `now` and nothing else.

#![allow(dead_code)]

use std::borrow::Cow;
use std::time::Instant;

use crate::gfx::{Canvas, RectF, Rgb};
use crate::text;

use super::super::anim;
use super::super::theme::{
    self, HAIRLINE, RAIL, SPINNER, SPINNER_STROKE, T_CAPTION, T_EYEBROW, T_META, T_NOTE, T_ROW,
    TARGET, Type, W,
};
use super::icons::{self, Icon};

// ----- Text in a type token ---------------------------------------------------

/// The text as a token shows it (uppercase tokens upper-case it).
pub fn shown<'a>(text: &'a str, ty: &Type) -> Cow<'a, str> {
    if ty.upper {
        Cow::Owned(text.to_uppercase())
    } else {
        Cow::Borrowed(text)
    }
}

pub fn measure(text: &str, ty: Type) -> f32 {
    text::measure_tracked(&shown(text, &ty), ty.weight, ty.px(), ty.tracking())
}

/// Draws `text` in `ty` with its baseline at `baseline`; returns its width.
pub fn text(
    canvas: &mut Canvas,
    text: &str,
    x: f32,
    baseline: f32,
    ty: Type,
    color: Rgb,
    alpha: f32,
) -> f32 {
    text::draw_tracked(
        canvas,
        &shown(text, &ty),
        x,
        baseline,
        ty.weight,
        ty.px(),
        ty.tracking(),
        color,
        alpha,
    )
}

pub fn text_right(
    canvas: &mut Canvas,
    s: &str,
    right: f32,
    baseline: f32,
    ty: Type,
    color: Rgb,
    alpha: f32,
) -> f32 {
    let width = measure(s, ty);
    text(canvas, s, right - width, baseline, ty, color, alpha)
}

pub fn text_centered(
    canvas: &mut Canvas,
    s: &str,
    cx: f32,
    baseline: f32,
    ty: Type,
    color: Rgb,
    alpha: f32,
) -> f32 {
    let width = measure(s, ty);
    text(
        canvas,
        s,
        (cx - width / 2.0).round(),
        baseline,
        ty,
        color,
        alpha,
    )
}

/// `text` shortened with an ellipsis to fit `width` in `ty`.
pub fn fit(s: &str, ty: Type, width: f32) -> String {
    let shown = shown(s, &ty);
    if ty.tracking() == 0.0 {
        return text::fit(&shown, ty.weight, ty.px(), width);
    }
    if text::measure_tracked(&shown, ty.weight, ty.px(), ty.tracking()) <= width {
        return shown.into_owned();
    }
    let mut out: String = shown.into_owned();
    while !out.is_empty() {
        out.pop();
        let candidate = format!("{}…", out.trim_end());
        if text::measure_tracked(&candidate, ty.weight, ty.px(), ty.tracking()) <= width {
            return candidate;
        }
    }
    "…".into()
}

/// An eyebrow label (THINKING, PINNED, a sheet's title); returns its width.
pub fn eyebrow(canvas: &mut Canvas, label: &str, x: f32, baseline: f32, color: Rgb) -> f32 {
    text(canvas, label, x, baseline, T_EYEBROW, color, 1.0)
}

/// Whether anything drawn in rows `y0..y1` can show: a band drawn again
/// alone (clipped) skips what lies outside it.
pub fn shows(canvas: &Canvas, y0: f32, y1: f32) -> bool {
    let (_, top, _, bottom) = canvas.clip();
    y1 > top as f32 && y0 < bottom as f32
}

/// Runs `draw` with drawing limited to `rect` within the current clip,
/// then puts the clip back.
pub fn clipped(canvas: &mut Canvas, rect: RectF, draw: impl FnOnce(&mut Canvas)) {
    let saved = canvas.clip();
    let x0 = (rect.x.floor() as i32).max(saved.0);
    let y0 = (rect.y.floor() as i32).max(saved.1);
    let x1 = ((rect.x + rect.w).ceil() as i32).min(saved.2);
    let y1 = ((rect.y + rect.h).ceil() as i32).min(saved.3);
    if x0 < x1 && y0 < y1 {
        canvas.set_clip(x0, y0, x1, y1);
        draw(canvas);
    }
    canvas.set_clip(saved.0, saved.1, saved.2, saved.3);
}

// ----- Lines and boxes ----------------------------------------------------------

/// An opaque rectangle at whole pixels.
pub fn fill(canvas: &mut Canvas, rect: RectF, color: Rgb) {
    let (x0, y0) = (rect.x.round() as i32, rect.y.round() as i32);
    let (x1, y1) = (
        (rect.x + rect.w).round() as i32,
        (rect.y + rect.h).round() as i32,
    );
    canvas.fill_rect(x0, y0, x1 - x0, y1 - y0, color, 1.0);
}

/// A horizontal hairline from `x0` to `x1` whose top is at `y`.
pub fn hairline(canvas: &mut Canvas, x0: f32, x1: f32, y: f32, color: Rgb) {
    fill(canvas, RectF::new(x0, y, x1 - x0, HAIRLINE), color);
}

/// A vertical line `width` wide from `y0` to `y1` whose left is at `x`.
pub fn vline(canvas: &mut Canvas, x: f32, y0: f32, y1: f32, width: f32, color: Rgb) {
    fill(canvas, RectF::new(x, y0, width, y1 - y0), color);
}

/// A square border `width` wide inside `rect`.
pub fn border(canvas: &mut Canvas, rect: RectF, width: f32, color: Rgb) {
    let RectF { x, y, w, h } = rect;
    fill(canvas, RectF::new(x, y, w, width), color);
    fill(canvas, RectF::new(x, y + h - width, w, width), color);
    fill(
        canvas,
        RectF::new(x, y + width, width, h - 2.0 * width),
        color,
    );
    fill(
        canvas,
        RectF::new(x + w - width, y + width, width, h - 2.0 * width),
        color,
    );
}

/// A filled box with a hairline border (the Whale box: code, notices,
/// tool rows, menus).
pub fn boxed(canvas: &mut Canvas, rect: RectF, fill_color: Rgb, border_color: Rgb) {
    fill(canvas, rect, fill_color);
    border(canvas, rect, HAIRLINE, border_color);
}

// ----- Top bars -----------------------------------------------------------------

/// Which of the three pages a page rail lights (Settings | List | Chat).
pub const RAIL_SETTINGS: usize = 0;
pub const RAIL_LIST: usize = 1;
pub const RAIL_CHAT: usize = 2;

/// A top bar's background (window y 0 to 122, taller with large text:
/// `theme::top_bar()`) and its hairline (§3.6.1).
pub fn top_bar(canvas: &mut Canvas, background: Rgb) {
    let bar = theme::top_bar();
    fill(canvas, RectF::new(0.0, 0.0, W, bar), background);
    hairline(canvas, 0.0, W, bar, theme::c().rule_topbar);
}

/// The page rail: the current page's third of the bar's bottom edge in the
/// accent, over the hairline (y 118 to 124).
pub fn page_rail(canvas: &mut Canvas, page: usize) {
    let segment = (W - 16.0) / 3.0;
    let x = (page as f32 * (W + 8.0) / 3.0).round();
    fill(
        canvas,
        RectF::new(x, theme::top_bar() - 4.0, segment.round(), RAIL),
        theme::p().accent,
    );
}

/// Rows a top bar covers, its hairline and rail included.
pub fn top_bar_rows() -> (i32, i32) {
    (0, (theme::top_bar() + HAIRLINE) as i32)
}

/// ⋯: three 6.5 px dots 15 px apart (as `real-04-new-chat` draws them;
/// the roadmap's 9 and 18 are larger).
pub fn ellipsis(canvas: &mut Canvas, cx: f32, cy: f32, color: Rgb, alpha: f32) {
    for index in -1..=1 {
        canvas.fill_circle(cx + index as f32 * 15.0, cy, 3.25, color, alpha);
    }
}

/// An icon in a 122 px target; pressed, it is drawn at half opacity.
pub fn icon_button(
    canvas: &mut Canvas,
    icon: Icon,
    cx: f32,
    cy: f32,
    size: f32,
    color: Rgb,
    pressed: bool,
) {
    icons::draw(
        canvas,
        icon,
        cx,
        cy,
        size,
        color,
        if pressed { 0.5 } else { 1.0 },
    );
}

/// The 122 px target around a control centred at (`cx`, `cy`).
pub fn target(cx: f32, cy: f32) -> RectF {
    RectF::new(cx - TARGET / 2.0, cy - TARGET / 2.0, TARGET, TARGET)
}

// ----- Buttons ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The one confirming action: accent fill.
    Primary,
    /// Any other action: accent outline.
    Secondary,
    /// Deletes, discards, stops: error outline.
    Destructive,
    /// Text only.
    Plain,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button<'a> {
    pub label: &'a str,
    pub kind: ButtonKind,
    pub icon: Option<Icon>,
    pub pressed: bool,
    pub disabled: bool,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, kind: ButtonKind) -> Self {
        Self {
            label,
            kind,
            icon: None,
            pressed: false,
            disabled: false,
        }
    }

    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The width it needs (padding 44 each side, at least the target).
    pub fn width(&self) -> f32 {
        let label = measure(self.label, T_NOTE.weight(text::Weight::MonoSemiBold));
        let icon = if self.icon.is_some() {
            44.0 + 17.0
        } else {
            0.0
        };
        (label + icon + 88.0).max(TARGET).round()
    }
}

/// A button filling `rect` (at least 122 high) on `background`.
pub fn button(canvas: &mut Canvas, rect: RectF, button: &Button, background: Rgb) {
    let p = theme::p();
    let (color, fill_color, border_color) = match button.kind {
        ButtonKind::Primary => (p.on_accent, Some(p.accent), Some(p.accent)),
        ButtonKind::Secondary => (p.accent, None, Some(theme::mix(p.accent, 50, background))),
        ButtonKind::Destructive => (p.error, None, Some(theme::mix(p.error, 50, background))),
        ButtonKind::Plain => (p.accent, None, None),
    };
    let alpha = if button.disabled { 0.35 } else { 1.0 };
    let face = match fill_color {
        Some(fill_color) => fill_color,
        None if button.pressed && !button.disabled => theme::mix(color, 12, background),
        None => background,
    };
    let face = if button.pressed && fill_color.is_some() && !button.disabled {
        theme::mix(0x000000, 12, face)
    } else {
        face
    };
    if face != background || fill_color.is_some() {
        canvas.fill_rect(
            rect.x.round() as i32,
            rect.y.round() as i32,
            rect.w.round() as i32,
            rect.h.round() as i32,
            face,
            alpha,
        );
    }
    if let Some(border_color) = border_color {
        let (x, y, w, h) = (
            rect.x.round(),
            rect.y.round(),
            rect.w.round(),
            rect.h.round(),
        );
        for r in [
            RectF::new(x, y, w, HAIRLINE),
            RectF::new(x, y + h - HAIRLINE, w, HAIRLINE),
            RectF::new(x, y, HAIRLINE, h),
            RectF::new(x + w - HAIRLINE, y, HAIRLINE, h),
        ] {
            canvas.fill_rect(
                r.x as i32,
                r.y as i32,
                r.w as i32,
                r.h as i32,
                border_color,
                alpha,
            );
        }
    }
    let ty = T_NOTE.weight(text::Weight::MonoSemiBold);
    let label_w = measure(button.label, ty);
    let icon_w = if button.icon.is_some() {
        44.0 + 17.0
    } else {
        0.0
    };
    let left = (rect.x + (rect.w - label_w - icon_w) / 2.0).round();
    let cy = rect.y + rect.h / 2.0;
    if let Some(icon) = button.icon {
        icons::draw(canvas, icon, left + 22.0, cy, 44.0, color, alpha);
    }
    text(
        canvas,
        button.label,
        left + icon_w,
        ty.baseline(cy - ty.line() / 2.0),
        ty,
        color,
        alpha,
    );
}

// ----- Segmented control and switch ---------------------------------------------

/// A segmented control (§3.6.18) filling `rect`: equal segments, the
/// selected one filled with an accent underline.
pub fn segmented(
    canvas: &mut Canvas,
    rect: RectF,
    labels: &[&str],
    selected: usize,
    pressed: Option<usize>,
) {
    let p = theme::p();
    let c = theme::c();
    if labels.is_empty() {
        return;
    }
    border(canvas, rect, HAIRLINE, c.rule_strong);
    let width = (rect.w - 2.0 * HAIRLINE) / labels.len() as f32;
    for (index, label) in labels.iter().enumerate() {
        let x0 = (rect.x + HAIRLINE + index as f32 * width).round();
        let x1 = (rect.x + HAIRLINE + (index + 1) as f32 * width).round();
        let segment = RectF::new(x0, rect.y + HAIRLINE, x1 - x0, rect.h - 2.0 * HAIRLINE);
        let on = index == selected;
        if on {
            fill(canvas, segment, p.selection_background);
            fill(
                canvas,
                RectF::new(x0, segment.y + segment.h - RAIL, x1 - x0, RAIL),
                p.accent,
            );
        } else if pressed == Some(index) {
            fill(canvas, segment, c.row_pressed);
        }
        let ty = if on {
            T_NOTE.weight(text::Weight::MonoBold)
        } else {
            T_NOTE
        };
        let label = fit(label, ty, segment.w - 16.0);
        text_centered(
            canvas,
            &label,
            segment.x + segment.w / 2.0,
            ty.baseline(segment.y + (segment.h - ty.line()) / 2.0),
            ty,
            if on { p.text } else { p.muted },
            1.0,
        );
    }
}

/// Which segment of a control filling `rect` with `count` segments `x` is
/// on.
pub fn segment_at(rect: RectF, count: usize, x: f32, y: f32) -> Option<usize> {
    if count == 0 || !rect.contains(x, y) {
        return None;
    }
    let index = ((x - rect.x) / (rect.w / count as f32)).floor() as usize;
    Some(index.min(count - 1))
}

/// A switch (128 × 78) with its top left at (`x`, `y`); `on` from 0 (off)
/// to 1 (on) as it slides.
pub fn switch(canvas: &mut Canvas, x: f32, y: f32, on: f32, background: Rgb) {
    let p = theme::p();
    let c = theme::c();
    let (w, h) = theme::SWITCH;
    let on = on.clamp(0.0, 1.0);
    let track = crate::gfx::mix(c.switch_off, p.success, on);
    let rect = RectF::new(x.round(), y.round(), w, h);
    canvas.fill_rrect(rect, h / 2.0, background, 1.0);
    canvas.fill_rrect(rect, h / 2.0, track, 1.0);
    let knob = theme::KNOB;
    let inset = (h - knob) / 2.0;
    let kx = rect.x + inset + (w - knob - 2.0 * inset) * on;
    canvas.fill_circle(
        kx + knob / 2.0,
        rect.y + h / 2.0 + 2.0,
        knob / 2.0,
        0x000000,
        0.25,
    );
    canvas.fill_circle(kx + knob / 2.0, rect.y + h / 2.0, knob / 2.0, 0xffffff, 1.0);
}

// ----- Spinners, dots, badges ---------------------------------------------------

/// The spinner (§3.6.20): a ring in `text` at 25 % with a 90° arc in
/// `color`, one turn per 700 ms. Its rect is `spinner_rect`.
pub fn spinner(
    canvas: &mut Canvas,
    cx: f32,
    cy: f32,
    diameter: f32,
    color: Rgb,
    background: Rgb,
    now: Instant,
) {
    let stroke = SPINNER_STROKE * diameter / SPINNER;
    let radius = (diameter - stroke) / 2.0;
    let track = theme::mix(theme::p().text, 25, background);
    let tau = std::f32::consts::TAU;
    canvas.ring_arc(cx, cy, radius, stroke, 0.0, tau, track, 1.0);
    let start = anim::phase(now, 700) * tau;
    canvas.ring_arc(cx, cy, radius, stroke, start, start + tau / 4.0, color, 1.0);
}

/// The rows a spinner of `diameter` at `cy` touches.
pub fn spinner_rows(cy: f32, diameter: f32) -> (i32, i32) {
    (
        (cy - diameter / 2.0 - 2.0).floor() as i32,
        (cy + diameter / 2.0 + 2.0).ceil() as i32,
    )
}

/// The activity dot (§3.6.5): a 25 px ring, a gap in `background`, and a
/// filled centre; `alpha` for the streaming pulse (`anim::pulse`).
pub fn activity_dot(
    canvas: &mut Canvas,
    cx: f32,
    cy: f32,
    color: Rgb,
    background: Rgb,
    alpha: f32,
) {
    let shade = theme::mix(
        color,
        (alpha.clamp(0.0, 1.0) * 100.0).round() as u32,
        background,
    );
    canvas.fill_circle(cx, cy, 12.5, shade, 1.0);
    canvas.fill_circle(cx, cy, 7.0, background, 1.0);
    canvas.fill_circle(cx, cy, 2.8, shade, 1.0);
}

/// A count as badges show it ("99+").
pub fn count_text(count: usize) -> String {
    if count > 99 {
        "99+".into()
    } else {
        count.to_string()
    }
}

/// A filled badge (an unread count): at least 55 × 50, its right edge at
/// `right`, centred on `cy`; returns its width.
pub fn badge(
    canvas: &mut Canvas,
    label: &str,
    right: f32,
    cy: f32,
    fill_color: Rgb,
    color: Rgb,
) -> f32 {
    let ty = T_META.weight(text::Weight::MonoBold);
    let w = (measure(label, ty) + 22.0).max(55.0).round();
    let rect = RectF::new((right - w).round(), (cy - 25.0).round(), w, 50.0);
    fill(canvas, rect, fill_color);
    text_centered(
        canvas,
        label,
        rect.x + w / 2.0,
        ty.baseline(rect.y + (50.0 - ty.line()) / 2.0),
        ty,
        color,
        1.0,
    );
    w
}

/// An inline glyph-and-number badge (`◆2`, `$3`, `◷21`) in `T_CAPTION`
/// Bold, its right edge at `right`; returns its width.
pub fn mark(canvas: &mut Canvas, label: &str, right: f32, baseline: f32, color: Rgb) -> f32 {
    let ty = T_CAPTION.weight(text::Weight::MonoBold);
    text_right(canvas, label, right, baseline, ty, color, 1.0)
}

// ----- Empty states and pills ---------------------------------------------------

/// A centred state block (§3.6.20) in `rect`: a title, a body and an
/// optional button; returns the button's rect.
pub fn empty_state(
    canvas: &mut Canvas,
    rect: RectF,
    title: &str,
    body: &str,
    action: Option<&Button>,
    background: Rgb,
) -> Option<RectF> {
    let p = theme::p();
    let title_ty = T_ROW.weight(text::Weight::MonoSemiBold);
    let body_lines: Vec<String> = if body.is_empty() {
        Vec::new()
    } else {
        wrap_plain(body, T_NOTE, rect.w - 88.0)
    };
    let button_h = if action.is_some() { TARGET + 34.0 } else { 0.0 };
    let height = title_ty.line() + body_lines.len() as f32 * T_NOTE.line() + 11.0 + button_h;
    let mut y = (rect.y + (rect.h - height) / 2.0).round();
    let cx = rect.x + rect.w / 2.0;
    if !title.is_empty() {
        text_centered(
            canvas,
            title,
            cx,
            title_ty.baseline(y),
            title_ty,
            p.text,
            1.0,
        );
    }
    y += title_ty.line() + 11.0;
    for line in &body_lines {
        text_centered(canvas, line, cx, T_NOTE.baseline(y), T_NOTE, p.muted, 1.0);
        y += T_NOTE.line();
    }
    action.map(|action| {
        let w = action.width();
        let r = RectF::new((cx - w / 2.0).round(), y + 34.0, w, TARGET);
        button(canvas, r, action, background);
        r
    })
}

/// Words of `text` wrapped to `width` in `ty` (plain text, for widgets).
pub fn wrap_plain(text: &str, ty: Type, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if measure(&candidate, ty) <= width || line.is_empty() {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_owned();
            }
            // A word wider than a line breaks anywhere (large text).
            while measure(&line, ty) > width && line.chars().count() > 1 {
                let mut cut = line.len();
                while cut > 0 && measure(&line[..cut], ty) > width {
                    cut = line[..cut].char_indices().last().map_or(0, |(i, _)| i);
                }
                let cut = if cut == 0 {
                    line.chars().next().map_or(1, char::len_utf8)
                } else {
                    cut
                };
                lines.push(line[..cut].to_owned());
                line = line[cut..].to_owned();
            }
        }
        lines.push(line);
    }
    lines
}

/// A pill (the connection pill, §3.6.4): `surface` with a border, an
/// optional spinner, a caption; centred on `cx`, its top at `top`.
/// Returns its rect.
pub fn pill(
    canvas: &mut Canvas,
    cx: f32,
    top: f32,
    label: &str,
    spinning: bool,
    background: Rgb,
    now: Instant,
) -> RectF {
    let p = theme::p();
    let c = theme::c();
    let label_w = measure(label, T_CAPTION);
    let spin_w = if spinning { SPINNER + 22.0 } else { 0.0 };
    let w = (label_w + spin_w + 78.0).round();
    let rect = RectF::new((cx - w / 2.0).round(), top.round(), w, 88.0);
    let face = theme::mix(p.surface_background, 92, background);
    boxed(canvas, rect, face, c.toast_border);
    let mut x = rect.x + 39.0;
    let cy = rect.y + rect.h / 2.0;
    if spinning {
        spinner(canvas, x + SPINNER / 2.0, cy, SPINNER, p.accent, face, now);
        x += spin_w;
    }
    text(
        canvas,
        label,
        x,
        T_CAPTION.baseline(cy - T_CAPTION.line() / 2.0),
        T_CAPTION,
        p.text,
        1.0,
    );
    rect
}

// ----- Shadows --------------------------------------------------------------------

/// How much of a shadow `distance` px outside its shape is left (the
/// smoothstep fall-off of `Canvas::shadow_rrect`), 0.5 at the edge.
fn falloff(distance: f32, blur: f32) -> f32 {
    let t = ((distance + blur) / (2.0 * blur)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// A soft black shadow around a square `rect` (offset `dy` down), drawn only
/// outside `rect`: the floating thing drawn over it covers the rest. Cheap
/// (rows and columns of constant shade), so it can be drawn every frame.
pub fn shadow(canvas: &mut Canvas, rect: RectF, blur: f32, dy: f32, alpha: f32) {
    if blur <= 0.0 || alpha <= 0.0 {
        return;
    }
    let (x0, x1) = (rect.x.round() as i32, (rect.x + rect.w).round() as i32);
    let (y0, y1) = (rect.y.round() as i32, (rect.y + rect.h).round() as i32);
    let (s0, s1) = (
        (rect.y + dy).round() as i32,
        (rect.y + rect.h + dy).round() as i32,
    );
    let b = blur.ceil() as i32;
    let shade = |dx: i32, dy: i32| -> f32 {
        // Distances outside the shadow's own rect (0 inside it).
        let fx = falloff(dx as f32, blur);
        let fy = falloff(dy as f32, blur);
        (2.0 * fx * fy).min(1.0) * alpha
    };
    let outside_x = |x: i32| {
        if x < x0 {
            x0 - x
        } else if x >= x1 {
            x - x1 + 1
        } else {
            0
        }
    };
    let outside_y = |y: i32| {
        if y < s0 {
            s0 - y
        } else if y >= s1 {
            y - s1 + 1
        } else {
            0
        }
    };
    for y in (s0 - b)..(s1 + b) {
        let dy = outside_y(y);
        let inside_rows = y >= y0 && y < y1;
        // Above or below the rect: one span across it, corners per pixel.
        if !inside_rows {
            let a = shade(0, dy);
            if a > 0.002 {
                canvas.fill_rect(x0, y, x1 - x0, 1, 0x000000, a);
            }
        }
        for x in (x0 - b)..x0 {
            let a = shade(outside_x(x), dy);
            if a > 0.002 {
                canvas.fill_rect(x, y, 1, 1, 0x000000, a);
            }
        }
        for x in x1..(x1 + b) {
            let a = shade(outside_x(x), dy);
            if a > 0.002 {
                canvas.fill_rect(x, y, 1, 1, 0x000000, a);
            }
        }
    }
}

/// The shadow above a full-width panel whose top is at `top` (a sheet's,
/// cast upwards over `blur` px).
pub fn shadow_above(canvas: &mut Canvas, top: f32, blur: f32, alpha: f32) {
    let top = top.round() as i32;
    for d in 1..=(blur.ceil() as i32) {
        let a = falloff(d as f32, blur) * alpha;
        if a > 0.002 {
            canvas.fill_rect(0, top - d, W as i32, 1, 0x000000, a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_wraps_at_spaces_and_breaks_words_wider_than_a_line() {
        let w = measure("lavender", T_META);
        assert_eq!(
            wrap_plain("Rose and lavender", T_META, w),
            ["Rose and", "lavender"]
        );
        let lines = wrap_plain("lavender", T_META, w / 2.0);
        assert_eq!(lines.concat(), "lavender");
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|line| measure(line, T_META) <= w / 2.0));
    }

    #[test]
    fn the_page_rail_lights_one_third_of_the_bar() {
        let mut canvas = Canvas::new(1080, 200);
        top_bar(&mut canvas, theme::p().app_background);
        page_rail(&mut canvas, RAIL_CHAT);
        let accent = theme::p().accent;
        let row = 120 * 1080;
        let lit: Vec<usize> = (0..1080)
            .filter(|x| canvas.pixels[row + x] == accent)
            .collect();
        assert_eq!(lit.first(), Some(&725));
        assert_eq!(lit.last(), Some(&1079));
        // The hairline under the bar.
        assert_eq!(canvas.pixels[123 * 1080 + 10], theme::c().rule_topbar);
        assert_eq!(canvas.pixels[60 * 1080 + 10], theme::p().app_background);
    }

    #[test]
    fn widgets_draw_the_same_pixels_twice() {
        let now = Instant::now();
        let draw = |canvas: &mut Canvas| {
            let bg = theme::p().sidebar_background;
            canvas.clear(bg);
            button(
                canvas,
                RectF::new(40.0, 40.0, 400.0, 122.0),
                &Button::new("Retry", ButtonKind::Secondary).pressed(true),
                bg,
            );
            segmented(
                canvas,
                RectF::new(40.0, 200.0, 900.0, 122.0),
                &["Standard", "Fast", "Ultrafast"],
                1,
                Some(2),
            );
            switch(canvas, 40.0, 360.0, 0.5, bg);
            spinner(canvas, 300.0, 400.0, 39.0, theme::p().accent, bg, now);
            activity_dot(canvas, 400.0, 400.0, theme::p().success, bg, 1.0);
            badge(canvas, &count_text(120), 600.0, 400.0, 0xc62828, 0xffffff);
            shadow(
                canvas,
                RectF::new(200.0, 500.0, 600.0, 200.0),
                60.0,
                10.0,
                0.4,
            );
            empty_state(
                canvas,
                RectF::new(0.0, 800.0, 1080.0, 500.0),
                "No conversations",
                "Start one with the plus button.",
                Some(&Button::new("New chat", ButtonKind::Primary)),
                bg,
            );
        };
        let mut a = Canvas::new(1080, 1400);
        let mut b = Canvas::new(1080, 1400);
        draw(&mut a);
        draw(&mut b);
        assert!(a.pixels == b.pixels);
        assert_eq!(count_text(7), "7");
        assert!(fit("A very long title that does not fit", T_EYEBROW, 300.0).ends_with('…'));
        assert_eq!(
            segment_at(RectF::new(0.0, 0.0, 300.0, 100.0), 3, 250.0, 50.0),
            Some(2)
        );
    }
}
