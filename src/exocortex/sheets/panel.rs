//! Panels (WP-E): the operations screens (Goal, Activity, Chrono, Usage,
//! Accounts, System prompt, Ask aside, Connection) as sheets made of rows,
//! in the iPhone's `ui/ops` style: accent section titles over a rule,
//! key/value rows, navigation rows, task rows, rail cards, bars and
//! buttons. A panel's rows are a function of the model (`Kind::build`), so
//! it redraws whenever what it shows changes; taps answer with actions.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::gfx::{Canvas, RectF, Rgb};
use crate::text::Weight;

use super::super::model::{Action, Model};
use super::super::theme::{self, HAIRLINE, RAIL, TARGET, Type, W};
use super::super::ui::icons::{self, Icon};
use super::super::ui::overlay::Cx;
use super::super::ui::sheet::{Reply, Sheet, SheetContent};
use super::super::ui::widgets::{self, Button, ButtonKind};

/// What a tap does: perform and stay, or close the panel first.
#[derive(Debug, Clone, PartialEq)]
pub enum Tap {
    Stay(Action),
    Close(Action),
}

/// A type token, by name (tokens hold floats; rows are hashed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Body,
    Note,
    Caption,
    CaptionBold,
    Meta,
    Small,
}

impl Style {
    pub fn ty(self) -> Type {
        match self {
            Style::Body => theme::T_BODY,
            Style::Note => theme::T_NOTE,
            Style::Caption => theme::T_CAPTION,
            Style::CaptionBold => theme::T_CAPTION.weight(Weight::MonoBold),
            Style::Meta => theme::T_META,
            Style::Small => theme::T_SMALL,
        }
    }
}

/// A button of a `Row::Buttons`; no tap: disabled.
#[derive(Debug, Clone, PartialEq)]
pub struct Btn {
    pub label: String,
    pub kind: Kind,
    pub tap: Option<Tap>,
}

/// A button's look (`ButtonKind`, hashable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Primary,
    Secondary,
    Destructive,
}

impl Kind {
    pub fn button(self) -> ButtonKind {
        match self {
            Kind::Primary => ButtonKind::Primary,
            Kind::Secondary => ButtonKind::Secondary,
            Kind::Destructive => ButtonKind::Destructive,
        }
    }
}

impl Btn {
    pub fn new(label: impl Into<String>, kind: Kind, tap: Option<Tap>) -> Self {
        Self {
            label: label.into(),
            kind,
            tap,
        }
    }
}

/// Something in a panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// An accent title over a rule, with an optional text button right.
    Section {
        title: String,
        button: Option<(String, Tap)>,
    },
    /// A muted eyebrow.
    Sub(String),
    /// Text wrapped to the width.
    Text {
        text: String,
        style: Style,
        color: Rgb,
    },
    /// A key (muted, 38 %) and its value (wrapped).
    Kv {
        key: String,
        value: String,
        color: Rgb,
    },
    /// A row that opens something: label, detail, chevron.
    Nav {
        icon: Option<Icon>,
        label: String,
        detail: Option<String>,
        tap: Tap,
    },
    /// Buttons side by side (one fills the width).
    Buttons(Vec<Btn>),
    /// A card with a coloured rail on its left: lines in their styles.
    Rail {
        color: Rgb,
        lines: Vec<(String, Style, Rgb)>,
    },
    /// An activity row: a coloured label, a title, a time, and ⋯.
    Task {
        label: String,
        color: Rgb,
        title: String,
        time: String,
        tap: Option<Tap>,
        more: Option<Tap>,
    },
    /// Two texts on one line, the right one coloured.
    Pair {
        left: String,
        right: String,
        style: Style,
        color: Rgb,
    },
    /// A 3 pt bar filled to `permille`.
    Bar {
        permille: u16,
        color: Rgb,
    },
    Gap(u16),
    /// A search hit: its role, a preview with the match (char range)
    /// highlighted, wrapped to at most `HIT_LINES` lines.
    Hit {
        role: String,
        preview: String,
        start: usize,
        end: usize,
        tap: Tap,
    },
}

/// A hit's preview shows at most this many lines.
const HIT_LINES: usize = 4;

/// `text` wrapped at spaces to `width` in `ty`, as char ranges (the space a
/// line breaks at belongs to neither line; a word longer than a line
/// breaks anywhere).
pub fn wrap_ranges(text: &str, ty: Type, width: f32) -> Vec<(usize, usize)> {
    let metrics = super::super::editor::Metrics::new(ty.weight, ty.px());
    let mut buffer = [0u8; 4];
    let widths: Vec<f32> = text
        .chars()
        .map(|c| metrics.width(c.encode_utf8(&mut buffer)))
        .collect();
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let (mut end, mut x) = (start, 0.0);
        let mut last_space = None;
        while end < chars.len() && x + widths[end] <= width {
            if chars[end] == ' ' {
                last_space = Some(end);
            }
            x += widths[end];
            end += 1;
        }
        if end >= chars.len() {
            lines.push((start, chars.len()));
            break;
        }
        match last_space.filter(|space| *space > start) {
            Some(space) => {
                lines.push((start, space));
                start = space + 1;
            }
            None => {
                let end = end.max(start + 1);
                lines.push((start, end));
                start = end;
            }
        }
    }
    if lines.is_empty() {
        lines.push((0, 0));
    }
    lines
}

const SECTION: f32 = 110.0;
const SECTION_LINE: f32 = 108.0;
const SUB: f32 = 90.0;
const NAV_MIN: f32 = 144.0;
const TASK: f32 = 133.0;
const BUTTON_GAP: f32 = 22.0;
const PAD_Y: f32 = 20.0;
const RAIL_PAD: (f32, f32) = (28.0, 34.0);
const BAR: f32 = 8.0;

/// Words wrapped to `width` in `ty`, words longer than a line broken
/// (paths, ids).
pub fn wrap(text: &str, ty: Type, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for line in widgets::wrap_plain(text, ty, width) {
        if widgets::measure(&line, ty) <= width {
            lines.push(line);
            continue;
        }
        let mut part = String::new();
        for c in line.chars() {
            part.push(c);
            if widgets::measure(&part, ty) > width {
                part.pop();
                lines.push(std::mem::take(&mut part));
                part.push(c);
            }
        }
        lines.push(part);
    }
    lines
}

/// A row laid out: its rows of text wrapped, its height.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    pub row: Row,
    pub top: f32,
    pub height: f32,
    lines: Vec<String>,
    /// Kv: the key's lines.
    keys: Vec<String>,
    /// Rail: each line's wrapped parts.
    parts: Vec<Vec<String>>,
}

fn kv_widths(width: f32) -> (f32, f32) {
    let key = (width * 0.38).floor();
    (key, width - key - 34.0)
}

/// Lays `row` out at `top` for `width`.
pub fn lay(row: Row, top: f32, width: f32) -> Laid {
    let mut lines = Vec::new();
    let mut keys = Vec::new();
    let mut parts = Vec::new();
    let height = match &row {
        Row::Section { .. } => SECTION,
        Row::Sub(_) => SUB,
        Row::Text { text, style, .. } => {
            lines = wrap(text, style.ty(), width);
            11.0 + lines.len() as f32 * style.ty().line() + 11.0
        }
        Row::Kv { key, value, .. } => {
            let (kw, vw) = kv_widths(width);
            let ty = Style::Caption.ty();
            keys = wrap(key, ty, kw);
            lines = wrap(value, ty, vw);
            2.0 * PAD_Y + keys.len().max(lines.len()) as f32 * ty.line()
        }
        Row::Nav { detail, icon, .. } => {
            let text_w = width - 60.0 - if icon.is_some() { 104.0 } else { 0.0 };
            if let Some(detail) = detail {
                lines = wrap(detail, Style::Meta.ty(), text_w);
            }
            (2.0 * 22.0 + Style::Body.ty().line() + lines.len() as f32 * Style::Meta.ty().line())
                .max(NAV_MIN)
        }
        Row::Buttons(buttons) => {
            let rows = button_rects(buttons, width, 0.0)
                .last()
                .map_or(0.0, |r| r.y);
            BUTTON_GAP + rows + TARGET
        }
        Row::Rail { lines: items, .. } => {
            let inner = width - RAIL - 2.0 * RAIL_PAD.1;
            parts = items
                .iter()
                .map(|(text, style, _)| wrap(text, style.ty(), inner))
                .collect();
            let text_h: f32 = parts
                .iter()
                .zip(items)
                .map(|(p, (_, style, _))| p.len() as f32 * style.ty().line())
                .sum();
            2.0 * RAIL_PAD.0 + text_h
        }
        Row::Task { .. } => TASK,
        Row::Pair { style, .. } => style.ty().line() + 8.0,
        Row::Bar { .. } => BAR + 14.0,
        Row::Gap(px) => *px as f32,
        Row::Hit { preview, .. } => {
            let lines = wrap_ranges(preview, Style::Caption.ty(), width)
                .len()
                .min(HIT_LINES);
            PAD_Y + theme::T_EYEBROW.line() + lines as f32 * Style::Caption.ty().line() + PAD_Y
        }
    };
    Laid {
        row,
        top,
        height: height.ceil(),
        lines,
        keys,
        parts,
    }
}

/// The rects of a button row's buttons (from the row's top): each as wide
/// as its label needs, wrapping to more lines; a lone one fills the width.
fn button_rects(buttons: &[Btn], width: f32, top: f32) -> Vec<RectF> {
    if buttons.len() == 1 {
        return vec![RectF::new(0.0, top + BUTTON_GAP, width, TARGET)];
    }
    let mut rects = Vec::new();
    let (mut x, mut y) = (0.0, top + BUTTON_GAP);
    for button in buttons {
        let w = Button::new(&button.label, button.kind.button())
            .width()
            .min(width);
        if x > 0.0 && x + w > width {
            x = 0.0;
            y += TARGET + BUTTON_GAP;
        }
        rects.push(RectF::new(x, y, w, TARGET));
        x += w + BUTTON_GAP;
    }
    rects
}

/// What a panel shows, as its builder made it.
pub struct Built {
    pub title: String,
    pub rows: Vec<Row>,
    /// When it next changes by itself (a ticking time).
    pub wake: Option<Instant>,
}

/// Which panel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Panel {
    Goal(String),
    Activity(String),
    Chrono(String),
    Usage,
    Accounts(Option<String>),
    SystemPrompt(Option<String>),
    Btw(String),
    Connection,
    Tokens,
    Find(String),
}

impl Panel {
    fn build(&self, model: &Model, width: f32, now: Instant) -> Built {
        match self {
            Panel::Goal(conv) => super::goal::build(model, conv, width, now),
            Panel::Activity(conv) => super::activity::build(model, conv, now),
            Panel::Chrono(conv) => super::chrono::build(model, conv, now),
            Panel::Usage => super::usage::build(model, now),
            Panel::Accounts(conv) => super::usage::accounts(model, conv.as_deref(), now),
            Panel::SystemPrompt(conv) => super::system::build(model, conv.as_deref()),
            Panel::Btw(conv) => super::btw::build(model, conv),
            Panel::Connection => super::usage::connection(model, now),
            Panel::Tokens => super::usage::tokens(model),
            Panel::Find(conv) => super::find::build(model, conv),
        }
    }

    /// Closes when the conversation changes.
    fn scoped(&self) -> bool {
        matches!(
            self,
            Panel::Goal(_)
                | Panel::Activity(_)
                | Panel::Chrono(_)
                | Panel::Btw(_)
                | Panel::Find(_)
                | Panel::SystemPrompt(Some(_))
        )
    }

    /// What to do once it has gone.
    fn farewell(&self) -> Option<Action> {
        match self {
            Panel::Accounts(_) => Some(Action::Ops(super::super::model::ops::Act::AccountsClosed)),
            _ => None,
        }
    }
}

/// A panel's content.
pub struct Content {
    panel: Panel,
    title: String,
    laid: Vec<Laid>,
    width: f32,
    wake: Option<Instant>,
}

/// The sheet showing `panel`.
pub fn sheet(panel: Panel) -> Sheet<Content> {
    Sheet::new(Content {
        panel,
        title: String::new(),
        laid: Vec::new(),
        width: W - 88.0,
        wake: None,
    })
}

impl Content {
    /// Tappable parts in order: rect (content coordinates) and tap.
    fn targets(&self) -> Vec<(RectF, Tap)> {
        self.laid
            .iter()
            .flat_map(|laid| targets(laid, self.width))
            .collect()
    }
}

/// A laid row's tappable parts (from its own top, at `laid.top`).
pub fn targets(laid: &Laid, width: f32) -> Vec<(RectF, Tap)> {
    let mut targets = Vec::new();
    {
        {
            let top = laid.top;
            match &laid.row {
                Row::Section {
                    button: Some((label, tap)),
                    ..
                } => {
                    let w = (widgets::measure(label, Style::Caption.ty()) + 56.0).max(TARGET);
                    targets.push((RectF::new(width - w + 28.0, top, w, SECTION), tap.clone()));
                }
                Row::Nav { tap, .. } => {
                    targets.push((RectF::new(0.0, top, width, laid.height), tap.clone()));
                }
                Row::Buttons(buttons) => {
                    for (button, rect) in buttons.iter().zip(button_rects(buttons, width, top)) {
                        if let Some(tap) = &button.tap {
                            targets.push((rect, tap.clone()));
                        }
                    }
                }
                Row::Hit { tap, .. } => {
                    targets.push((RectF::new(0.0, top, width, laid.height), tap.clone()));
                }
                Row::Task { tap, more, .. } => {
                    let more_w = if more.is_some() { TARGET } else { 0.0 };
                    if let Some(tap) = tap {
                        targets.push((RectF::new(0.0, top, width - more_w, TASK), tap.clone()));
                    }
                    if let Some(more) = more {
                        targets.push((
                            RectF::new(width - more_w + 34.0, top, more_w, TASK),
                            more.clone(),
                        ));
                    }
                }
                _ => {}
            }
        }
        targets
    }
}

impl SheetContent for Content {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn layout(&mut self, cx: &Cx, width: f32) -> f32 {
        self.width = width;
        let built = self.panel.build(cx.model, width, cx.now);
        self.title = built.title;
        self.wake = built.wake;
        let mut top = 0.0;
        self.laid = built
            .rows
            .into_iter()
            .map(|row| {
                let laid = lay(row, top, width);
                top += laid.height;
                laid
            })
            .collect();
        top + 34.0
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let pressed = pressed
            .and_then(|index| self.targets().into_iter().nth(index))
            .map(|(_, tap)| tap_action(&tap).clone());
        for laid in &self.laid {
            let y = top + laid.top;
            if widgets::shows(canvas, y, y + laid.height) {
                draw_row(canvas, laid, x, y, self.width, pressed.as_ref());
            }
        }
    }

    fn item_at(&self, _cx: &Cx, x: f32, y: f32) -> Option<usize> {
        self.targets()
            .iter()
            .position(|(rect, _)| rect.contains(x, y))
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        match self.targets().into_iter().nth(index) {
            Some((_, Tap::Stay(action))) => Reply::Action(action),
            Some((_, Tap::Close(action))) => Reply::CloseWith(action),
            None => Reply::None,
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.title, theme::generation()).hash(&mut hasher);
        for laid in &self.laid {
            (laid.top as i32, format!("{:?}", laid.row)).hash(&mut hasher);
        }
        hasher.finish()
    }

    fn scoped(&self) -> bool {
        self.panel.scoped()
    }

    fn wake(&self, _cx: &Cx) -> Option<Instant> {
        self.wake
    }

    fn farewell(&mut self) -> Option<Action> {
        self.panel.farewell()
    }
}

/// The action a tap performs.
pub fn tap_action(tap: &Tap) -> &Action {
    match tap {
        Tap::Stay(action) | Tap::Close(action) => action,
    }
}

/// Draws a laid row with its top at `y`; `pressed` is the action under the
/// finger.
pub fn draw_row(
    canvas: &mut Canvas,
    laid: &Laid,
    x: f32,
    y: f32,
    width: f32,
    pressed: Option<&Action>,
) {
    let down = |tap: &Tap| pressed == Some(tap_action(tap));
    let p = theme::p();
    let c = theme::c();
    let right = x + width;
    let background = p.sidebar_background;
    let row_rule = theme::mix(p.border, 25, background);
    match &laid.row {
        Row::Section { title, button } => {
            let ty = theme::T_SECTION;
            let baseline = ty.baseline(y + 24.0 + (SECTION_LINE - 24.0 - ty.line()) / 2.0);
            widgets::text(canvas, title, x, baseline, ty, p.accent, 1.0);
            if let Some((label, tap)) = button {
                let ty = Style::Caption.ty();
                let alpha = if down(tap) { 0.5 } else { 1.0 };
                widgets::text_right(
                    canvas,
                    label,
                    right,
                    ty.baseline(y + 24.0 + (SECTION_LINE - 24.0 - ty.line()) / 2.0),
                    ty,
                    p.secondary_accent,
                    alpha,
                );
            }
            widgets::hairline(canvas, x, right, y + SECTION_LINE, c.rule_sheet_row);
        }
        Row::Sub(text) => {
            let ty = theme::T_EYEBROW;
            widgets::text(canvas, text, x, ty.baseline(y + 44.0), ty, p.muted, 1.0);
        }
        Row::Text { style, color, .. } => {
            let ty = style.ty();
            for (index, line) in laid.lines.iter().enumerate() {
                widgets::text(
                    canvas,
                    line,
                    x,
                    ty.baseline(y + 11.0 + index as f32 * ty.line()),
                    ty,
                    *color,
                    1.0,
                );
            }
        }
        Row::Kv { color, .. } => {
            let ty = Style::Caption.ty();
            let (kw, _) = kv_widths(width);
            for (index, line) in laid.keys.iter().enumerate() {
                let baseline = ty.baseline(y + PAD_Y + index as f32 * ty.line());
                widgets::text(canvas, line, x, baseline, ty, p.muted, 1.0);
            }
            for (index, line) in laid.lines.iter().enumerate() {
                let baseline = ty.baseline(y + PAD_Y + index as f32 * ty.line());
                widgets::text(canvas, line, x + kw + 34.0, baseline, ty, *color, 1.0);
            }
            widgets::hairline(
                canvas,
                x,
                right,
                y + laid.height - HAIRLINE,
                theme::mix(p.border, 22, background),
            );
        }
        Row::Nav {
            icon, label, tap, ..
        } => {
            if down(tap) {
                widgets::fill(
                    canvas,
                    RectF::new(0.0, y, W, laid.height - HAIRLINE),
                    p.selection_background,
                );
            }
            let mut text_x = x;
            if let Some(icon) = icon {
                icons::draw(
                    canvas,
                    *icon,
                    x + 30.0,
                    y + laid.height / 2.0,
                    60.0,
                    p.secondary_accent,
                    1.0,
                );
                text_x += 104.0;
            }
            let body = Style::Body.ty();
            let meta = Style::Meta.ty();
            let text_h = body.line() + laid.lines.len() as f32 * meta.line();
            let text_top = y + ((laid.height - text_h) / 2.0).round();
            let label = widgets::fit(label, body, right - 60.0 - text_x);
            widgets::text(
                canvas,
                &label,
                text_x,
                body.baseline(text_top),
                body,
                p.text,
                1.0,
            );
            for (index, line) in laid.lines.iter().enumerate() {
                widgets::text(
                    canvas,
                    line,
                    text_x,
                    meta.baseline(text_top + body.line() + index as f32 * meta.line()),
                    meta,
                    p.muted,
                    1.0,
                );
            }
            icons::draw(
                canvas,
                Icon::ChevronRight,
                right - 16.0,
                y + laid.height / 2.0,
                39.0,
                p.muted,
                1.0,
            );
            widgets::hairline(canvas, x, right, y + laid.height - HAIRLINE, row_rule);
        }
        Row::Buttons(buttons) => {
            for (button, rect) in buttons.iter().zip(button_rects(buttons, width, 0.0)) {
                let rect = RectF::new(rect.x + x, rect.y + y, rect.w, rect.h);
                let pressed = button.tap.as_ref().is_some_and(down);
                widgets::button(
                    canvas,
                    rect,
                    &Button::new(&button.label, button.kind.button())
                        .pressed(pressed)
                        .disabled(button.tap.is_none()),
                    background,
                );
            }
        }
        Row::Rail { color, lines } => {
            let rect = RectF::new(x, y, width, laid.height);
            widgets::fill(canvas, rect, theme::mix(*color, 4, background));
            widgets::fill(canvas, RectF::new(x, y, RAIL, laid.height), *color);
            let mut ty_top = y + RAIL_PAD.0;
            for ((_, style, color), parts) in lines.iter().zip(&laid.parts) {
                let ty = style.ty();
                for part in parts {
                    widgets::text(
                        canvas,
                        part,
                        x + RAIL + RAIL_PAD.1,
                        ty.baseline(ty_top),
                        ty,
                        *color,
                        1.0,
                    );
                    ty_top += ty.line();
                }
            }
        }
        Row::Task {
            label,
            color,
            title,
            time,
            tap,
            more,
        } => {
            if tap.as_ref().is_some_and(down) {
                widgets::fill(
                    canvas,
                    RectF::new(0.0, y, W - if more.is_some() { TARGET } else { 0.0 }, TASK),
                    p.selection_background,
                );
            }
            let ty = Style::Caption.ty();
            let bold = Style::CaptionBold.ty();
            let baseline = ty.baseline(y + (TASK - ty.line()) / 2.0);
            let label_w = widgets::text(canvas, label, x, baseline, bold, *color, 1.0);
            let time_right = right - if more.is_some() { TARGET - 34.0 } else { 0.0 };
            let time_w = widgets::text_right(
                canvas,
                time,
                time_right,
                baseline,
                Style::Meta.ty(),
                p.muted,
                1.0,
            );
            let title_x = x + label_w + 28.0;
            let title = widgets::fit(title, ty, time_right - time_w - 28.0 - title_x);
            widgets::text(canvas, &title, title_x, baseline, ty, p.text, 1.0);
            if let Some(more) = more {
                let alpha = if down(more) { 0.5 } else { 1.0 };
                widgets::ellipsis(canvas, right - 30.0, y + TASK / 2.0, p.muted, alpha);
            }
            widgets::hairline(canvas, x, right, y + TASK - HAIRLINE, row_rule);
        }
        Row::Pair {
            left,
            right: value,
            style,
            color,
        } => {
            let ty = style.ty();
            let baseline = ty.baseline(y + 4.0);
            widgets::text(canvas, left, x, baseline, ty, p.text, 1.0);
            let bold = ty.weight(Weight::MonoBold);
            widgets::text_right(canvas, value, right, baseline, bold, *color, 1.0);
        }
        Row::Bar { permille, color } => {
            let track = RectF::new(x, y + 14.0, width, BAR);
            widgets::fill(canvas, track, c.switch_off);
            let w = (width * (*permille).min(1000) as f32 / 1000.0).round();
            if w > 0.0 {
                widgets::fill(canvas, RectF::new(x, y + 14.0, w, BAR), *color);
            }
        }
        Row::Gap(_) => {}
        Row::Hit {
            role,
            preview,
            start,
            end,
            tap,
        } => {
            if down(tap) {
                widgets::fill(
                    canvas,
                    RectF::new(0.0, y, W, laid.height - HAIRLINE),
                    p.selection_background,
                );
            }
            let eyebrow = theme::T_EYEBROW;
            widgets::text(
                canvas,
                role,
                x,
                eyebrow.baseline(y + PAD_Y),
                eyebrow,
                p.muted,
                1.0,
            );
            let ty = Style::Caption.ty();
            let chars: Vec<char> = preview.chars().collect();
            let piece = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
            let lines = wrap_ranges(preview, ty, width);
            let top = y + PAD_Y + eyebrow.line();
            for (row, &(a, b)) in lines.iter().take(HIT_LINES).enumerate() {
                let line_top = top + row as f32 * ty.line();
                let baseline = ty.baseline(line_top);
                // The match's part of this line, on its highlight.
                let (s, e) = ((*start).clamp(a, b), (*end).clamp(a, b));
                if e > s {
                    let x0 = x + widgets::measure(&piece(a, s), ty);
                    let w = widgets::measure(&piece(s, e), ty);
                    widgets::fill(
                        canvas,
                        RectF::new(x0.round(), line_top, w.round(), ty.line()),
                        p.search_background,
                    );
                }
                let mut pen = x;
                for (from, to, color) in
                    [(a, s, p.text), (s, e, p.search_foreground), (e, b, p.text)]
                {
                    if to > from {
                        pen +=
                            widgets::text(canvas, &piece(from, to), pen, baseline, ty, color, 1.0);
                    }
                }
            }
            widgets::hairline(canvas, x, right, y + laid.height - HAIRLINE, row_rule);
        }
    }
}

/// A section's rows, as builders write them.
pub fn section(title: impl Into<String>) -> Row {
    Row::Section {
        title: title.into(),
        button: None,
    }
}

pub fn text(text: impl Into<String>, style: Style, color: Rgb) -> Row {
    Row::Text {
        text: text.into(),
        style,
        color,
    }
}

/// A muted note in `T_NOTE`.
pub fn note(text: impl Into<String>) -> Row {
    Row::Text {
        text: text.into(),
        style: Style::Note,
        color: theme::p().muted,
    }
}

pub fn kv(key: impl Into<String>, value: impl Into<String>) -> Row {
    Row::Kv {
        key: key.into(),
        value: value.into(),
        color: theme::p().text,
    }
}

pub fn nav(label: impl Into<String>, detail: Option<String>, icon: Option<Icon>, tap: Tap) -> Row {
    Row::Nav {
        icon,
        label: label.into(),
        detail,
        tap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::ops;
    use crate::exocortex::ui::overlay::{Overlay, OverlayStack};

    #[test]
    fn the_accounts_panel_says_farewell_once_gone() {
        let mut stack = OverlayStack::default();
        stack.push(Box::new(sheet(Panel::Accounts(None))));
        let now = Instant::now();
        assert!(stack.close_top(now));
        assert!(stack.sweep(now + std::time::Duration::from_secs(1)));
        assert_eq!(
            stack.take_farewells(),
            vec![Action::Ops(ops::Act::AccountsClosed)]
        );
        let mut usage = sheet(Panel::Usage);
        assert_eq!(usage.farewell(), None);
    }

    #[test]
    fn long_words_break_and_buttons_wrap() {
        let ty = Style::Caption.ty();
        let lines = wrap(&"x".repeat(100), ty, 400.0);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| widgets::measure(l, ty) <= 400.0));
        let buttons: Vec<Btn> = ["Pause", "Complete", "Clear goal", "Another one", "More"]
            .iter()
            .map(|l| Btn::new(*l, Kind::Secondary, None))
            .collect();
        let rects = button_rects(&buttons, 992.0, 0.0);
        assert!(rects.iter().all(|r| r.x + r.w <= 992.0));
        assert!(
            rects.last().unwrap().y > rects[0].y,
            "wrapped to a second line"
        );
        assert_eq!(
            button_rects(&buttons[..1], 992.0, 0.0)[0].w,
            992.0,
            "a lone button fills the row"
        );
    }
}
