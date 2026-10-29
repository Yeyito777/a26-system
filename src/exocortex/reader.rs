//! The reader (PAR-332): a full-screen page over the app showing a text in
//! full, monospaced, as written (lines kept, long ones wrapped at spaces or
//! anywhere), with its own top bar: Done, the title and how many
//! characters, Find and Copy. Find opens a bar with a field (Moon's
//! keyboard), "i/N" and ‹ ›: every match is marked, the current one in
//! `search_background`, scrolled to a third of the way down. A long press
//! selects a word (PAR-141); moving on, or dragging either handle, extends
//! it; COPY over it copies it, a tap elsewhere lets it go.
//!
//! Open one with `reader::open(&mut model.ui, title, text)` or the
//! `UiAct::Read` action.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::comms::Scroll;
use crate::control::KeyboardPurpose;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::text::{self, Weight};

use super::find;
use super::gesture::{Axis, Gesture};
use super::model::{Action, Ui, UiAct};
use super::theme::{self, HAIRLINE, T_BODY, T_META, T_NOTE, T_SUBTITLE, T_TITLE, TARGET, TOP_BAR};
use super::ui::Haptic;
use super::ui::icons::{self, Icon};
use super::ui::overlay::{Body, Cx, Overlay, Response, Scroller};
use super::ui::widgets;

/// Opens the reader on `text`.
pub fn open(ui: &mut Ui, title: impl Into<String>, text: impl Into<String>) {
    ui.open(Box::new(Reader::new(title.into(), text.into())));
}

/// The find bar's height (a 122 px field and its padding).
const FIND_BAR: f32 = 144.0;
/// Text padding: top, sides, bottom (12 / 16 / 24 pt).
const PAD: (f32, f32, f32) = (34.0, 44.0, 66.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Control {
    Done,
    Find,
    Copy,
    Field,
    Previous,
    Next,
    /// COPY over a selection.
    CopySelection,
    /// A selection's start or end handle.
    Handle(bool),
}

/// A handle's knob and how far from it a touch still takes it.
const KNOB: f32 = 22.0;
const HANDLE_REACH: f32 = 70.0;

pub struct Reader {
    title: String,
    text: String,
    /// Visual lines as byte ranges of `text`, for `wrapped_for`.
    lines: Vec<(usize, usize)>,
    wrapped_for: f32,
    scroll: Scroll,
    scrolling: bool,
    pressed: Option<Control>,
    closed: bool,
    find: Option<Find>,
    /// Widths of characters other than ASCII (fallback glyphs, wide ones).
    widths: RefCell<HashMap<char, f32>>,
    /// Selected text (byte range), and which end a drag moves.
    selection: Option<(usize, usize)>,
    dragging: Option<bool>,
    /// A haptic for the event loop (a long press selected a word).
    haptic: Option<Haptic>,
}

#[derive(Debug, Default)]
struct Find {
    query: String,
    matches: Vec<(usize, usize)>,
    current: usize,
    /// The field has the keyboard.
    focused: bool,
}

impl Reader {
    pub fn new(title: String, text: String) -> Self {
        Self {
            title,
            text,
            lines: Vec::new(),
            wrapped_for: 0.0,
            scroll: Scroll::default(),
            scrolling: false,
            pressed: None,
            closed: false,
            find: None,
            widths: RefCell::new(HashMap::new()),
            selection: None,
            dragging: None,
            haptic: None,
        }
    }

    /// The reader opened with Find on `query` (Find in chat, E).
    pub fn with_query(title: String, text: String, query: &str) -> Self {
        let mut reader = Self::new(title, text);
        reader.find = Some(Find {
            query: query.to_owned(),
            ..Find::default()
        });
        reader.search();
        reader
    }

    fn size() -> f32 {
        T_NOTE.px()
    }

    fn line_h() -> f32 {
        T_NOTE.line()
    }

    /// A character's advance.
    fn width_of(&self, c: char) -> f32 {
        let size = Self::size();
        if c.is_ascii() {
            return text::measure("M", Weight::Mono, size);
        }
        *self
            .widths
            .borrow_mut()
            .entry(c)
            .or_insert_with(|| text::measure(c.encode_utf8(&mut [0; 4]), Weight::Mono, size))
    }

    fn width_between(&self, from: usize, to: usize) -> f32 {
        self.text[from..to].chars().map(|c| self.width_of(c)).sum()
    }

    /// Wraps the text to `width` as written: every newline breaks a line,
    /// a line too long breaks after its last space that fits, else
    /// anywhere.
    fn wrap(&mut self, width: f32) {
        let mut lines = Vec::new();
        let mut start = 0;
        for line in self.text.split('\n') {
            let end = start + line.len();
            let mut from = start;
            let mut x = 0.0;
            let mut space: Option<usize> = None;
            let mut index = start;
            for c in line.chars() {
                let w = self.width_of(c);
                if x + w > width && index > from {
                    let cut = match space {
                        Some(after) if after > from => after,
                        _ => index,
                    };
                    lines.push((from, cut));
                    x = self.width_between(cut, index);
                    from = cut;
                    space = None;
                }
                x += w;
                index += c.len_utf8();
                if c == ' ' {
                    space = Some(index);
                }
            }
            lines.push((from, end));
            start = end + 1;
        }
        self.lines = lines;
        self.wrapped_for = width;
    }

    fn find_open(&self) -> bool {
        self.find.is_some()
    }

    /// Where the text area is: under the bars, above the bottom.
    fn area(&self, cx: &Cx) -> (f32, f32) {
        let top = TOP_BAR
            + HAIRLINE
            + if self.find_open() {
                FIND_BAR + HAIRLINE
            } else {
                0.0
            };
        (top, cx.bottom())
    }

    fn content_h(&self) -> f32 {
        self.lines.len() as f32 * Self::line_h() + PAD.0 + PAD.2
    }

    fn max_scroll(&self, cx: &Cx) -> f32 {
        let (y0, y1) = self.area(cx);
        (self.content_h() - (y1 - y0)).max(0.0)
    }

    fn control_at(&self, cx: &Cx, x: f32, y: f32) -> Option<Control> {
        if let Some(copy) = self.copy_rect(cx)
            && copy.contains(x, y)
        {
            return Some(Control::CopySelection);
        }
        if let Some(handle) = self.handle_at(cx, x, y) {
            return Some(Control::Handle(handle));
        }
        if y < TOP_BAR {
            return if x < 240.0 {
                Some(Control::Done)
            } else if x >= cx.width - TARGET {
                Some(Control::Copy)
            } else if x >= cx.width - 2.0 * TARGET {
                Some(Control::Find)
            } else {
                None
            };
        }
        if self.find_open() && y < TOP_BAR + HAIRLINE + FIND_BAR {
            return if x >= cx.width - TARGET {
                Some(Control::Next)
            } else if x >= cx.width - 2.0 * TARGET {
                Some(Control::Previous)
            } else if x < self.field_rect(cx).x + self.field_rect(cx).w {
                Some(Control::Field)
            } else {
                None
            };
        }
        None
    }

    fn field_rect(&self, cx: &Cx) -> RectF {
        let top = TOP_BAR + HAIRLINE + 11.0;
        RectF::new(34.0, top, cx.width - 34.0 - 2.0 * TARGET - 200.0, TARGET)
    }

    /// Finds the query again; the current match is the first.
    fn search(&mut self) {
        if let Some(find) = &mut self.find {
            find.matches = find::matches(&self.text, &find.query, find::MAX_MATCHES);
            find.current = 0;
        }
    }

    /// Scrolls the current match to a third of the way down.
    fn reveal(&mut self, cx: &Cx) {
        let Some(find) = &self.find else { return };
        let Some(&(start, _)) = find.matches.get(find.current) else {
            return;
        };
        let line = self.lines.partition_point(|(_, end)| *end < start);
        let line = line.min(self.lines.len().saturating_sub(1));
        let (y0, y1) = self.area(cx);
        let y = PAD.0 + line as f32 * Self::line_h();
        let offset = (y - (y1 - y0) / 3.0)
            .clamp(0.0, self.max_scroll(cx))
            .round();
        self.scroll.reset();
        self.scroll.offset = offset;
    }

    fn step(&mut self, cx: &Cx, by: isize) {
        if let Some(find) = &mut self.find
            && !find.matches.is_empty()
        {
            let n = find.matches.len() as isize;
            find.current = ((find.current as isize + by).rem_euclid(n)) as usize;
            self.reveal(cx);
        }
    }

    /// Moves the dragged end of the selection to (`x`, `y`) (whole words
    /// while it grows from a long press, as text selection does).
    fn extend(&mut self, cx: &Cx, x: f32, y: f32) {
        let (Some(start_end), Some((a, b))) = (self.dragging, self.selection) else {
            return;
        };
        let at = self.offset_at(cx, x, y);
        self.selection = Some(if start_end {
            (at.min(b.saturating_sub(1)), b)
        } else {
            (a, at.max(a + 1).min(self.text.len()))
        });
    }

    fn draw_text(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        let (y0, y1) = self.area(cx);
        let offset = self.scroll.value(cx.now).round();
        let size = Self::size();
        let line_h = Self::line_h();
        let match_fill = theme::mix(p.search_background, 40, p.app_background);
        let (matches, current) = match &self.find {
            Some(find) => (
                find.matches.as_slice(),
                find.matches.get(find.current).copied(),
            ),
            None => (&[][..], None),
        };
        widgets::clipped(canvas, RectF::new(0.0, y0, cx.width, y1 - y0), |canvas| {
            let top = y0 + PAD.0 - offset;
            let first = (((y0 - top) / line_h).floor().max(0.0)) as usize;
            for (index, &(start, end)) in self.lines.iter().enumerate().skip(first) {
                let y = top + index as f32 * line_h;
                if y > y1 {
                    break;
                }
                let baseline = T_NOTE.baseline(y);
                // Matches on this line: marked behind the text.
                let on_line = matches
                    .iter()
                    .skip(matches.partition_point(|m| m.1 <= start))
                    .take_while(|m| m.0 < end);
                let mut current_span = None;
                for &(m0, m1) in on_line {
                    let (a, b) = (m0.max(start), m1.min(end));
                    if a >= b {
                        continue;
                    }
                    let x = PAD.1 + self.width_between(start, a);
                    let w = self.width_between(a, b);
                    let is_current = current == Some((m0, m1));
                    if is_current {
                        current_span = Some((a, b));
                    }
                    widgets::fill(
                        canvas,
                        RectF::new(x.round(), y + 2.0, w.round(), line_h - 4.0),
                        if is_current {
                            p.search_background
                        } else {
                            match_fill
                        },
                    );
                }
                if let Some((s0, s1)) = self.selection {
                    let (a, b) = (s0.max(start), s1.min(end));
                    let to_end = s1 > end && end < self.text.len();
                    if a < b || (to_end && a <= b && s0 <= end) {
                        let x = PAD.1 + self.width_between(start, a);
                        let mut w = self.width_between(a, b);
                        if to_end {
                            w += self.width_of(' ');
                        }
                        widgets::fill(
                            canvas,
                            RectF::new(x.round(), y, w.round(), line_h),
                            p.text_selection,
                        );
                    }
                }
                let segment = |canvas: &mut Canvas, a: usize, b: usize, color| {
                    if a < b {
                        text::draw_tracked(
                            canvas,
                            &self.text[a..b],
                            PAD.1 + self.width_between(start, a),
                            baseline,
                            Weight::Mono,
                            size,
                            0.0,
                            color,
                            1.0,
                        );
                    }
                };
                match current_span {
                    Some((a, b)) => {
                        segment(canvas, start, a, p.text);
                        segment(canvas, a, b, p.search_foreground);
                        segment(canvas, b, end, p.text);
                    }
                    None => segment(canvas, start, end, p.text),
                }
            }
        });
    }

    /// The text's top on screen (scrolled).
    fn text_top(&self, cx: &Cx) -> f32 {
        self.area(cx).0 + PAD.0 - self.scroll.value(cx.now).round()
    }

    /// The byte offset nearest (`x`, `y`) on screen.
    fn offset_at(&self, cx: &Cx, x: f32, y: f32) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let line = ((y - self.text_top(cx)) / Self::line_h()).floor();
        let line = (line.max(0.0) as usize).min(self.lines.len() - 1);
        let (start, end) = self.lines[line];
        let mut at = start;
        let mut left = PAD.1;
        for (index, c) in self.text[start..end].char_indices() {
            let w = self.width_of(c);
            if x < left + w / 2.0 {
                return start + index;
            }
            left += w;
            at = start + index + c.len_utf8();
        }
        at
    }

    /// Where byte offset `at` is on screen: its x and its line's top.
    fn point_of(&self, cx: &Cx, at: usize) -> (f32, f32) {
        let line = self
            .lines
            .partition_point(|(_, end)| *end < at)
            .min(self.lines.len().saturating_sub(1));
        let start = self.lines.get(line).map_or(0, |l| l.0);
        (
            PAD.1 + self.width_between(start, at.max(start)),
            self.text_top(cx) + line as f32 * Self::line_h(),
        )
    }

    /// The word at byte offset `at` (letters, digits and `_`; else the
    /// character itself).
    fn word_at(&self, at: usize) -> (usize, usize) {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        // On a space just after a word: that word.
        let at = match (
            self.text[at..].chars().next(),
            self.text[..at].chars().next_back(),
        ) {
            (Some(c), Some(before)) if c.is_whitespace() && word(before) => at - before.len_utf8(),
            _ => at,
        };
        let Some(c) = self.text[at..].chars().next() else {
            let before = self.text[..at].char_indices().next_back();
            return before.map_or((at, at), |(i, _)| (i, at));
        };
        if !word(c) {
            return (at, at + c.len_utf8());
        }
        let start = self.text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| word(*c))
            .last()
            .map_or(at, |(i, _)| i);
        let end = self.text[at..]
            .char_indices()
            .find(|(_, c)| !word(*c))
            .map_or(self.text.len(), |(i, _)| at + i);
        (start, end)
    }

    /// Which handle (true: the start) is under (`x`, `y`).
    fn handle_at(&self, cx: &Cx, x: f32, y: f32) -> Option<bool> {
        let (start, end) = self.selection?;
        let line_h = Self::line_h();
        let (sx, sy) = self.point_of(cx, start);
        let (ex, ey) = self.point_of(cx, end);
        let near = |hx: f32, hy: f32| (x - hx).hypot(y - hy) < HANDLE_REACH;
        if near(ex, ey + line_h + KNOB) {
            Some(false)
        } else if near(sx, sy - KNOB) {
            Some(true)
        } else {
            None
        }
    }

    /// The COPY pill over the selection.
    fn copy_rect(&self, cx: &Cx) -> Option<RectF> {
        let (start, end) = self.selection?;
        let (sx, sy) = self.point_of(cx, start);
        let w = 200.0;
        let h = TARGET - 22.0;
        // Above the selection, or under it when there is no room above.
        let above = sy - KNOB - 22.0 - h;
        let top = if above >= self.area(cx).0 + 8.0 {
            above
        } else {
            self.point_of(cx, end).1 + Self::line_h() + KNOB + 22.0
        };
        let x = (sx - w / 2.0).clamp(22.0, cx.width - w - 22.0);
        Some(RectF::new(x.round(), top.round(), w, h))
    }

    fn draw_selection(&self, canvas: &mut Canvas, cx: &Cx) {
        let Some((start, end)) = self.selection else {
            return;
        };
        let p = theme::p();
        let c = theme::c();
        let (y0, y1) = self.area(cx);
        let line_h = Self::line_h();
        widgets::clipped(canvas, RectF::new(0.0, y0, cx.width, y1 - y0), |canvas| {
            let (sx, sy) = self.point_of(cx, start);
            let (ex, ey) = self.point_of(cx, end);
            // Knobs: a bar on each end, a dot above the start, below the end.
            let bar = |canvas: &mut Canvas, x: f32, y: f32| {
                widgets::fill(
                    canvas,
                    RectF::new(x.round() - 2.0, y, 4.0, line_h),
                    p.accent,
                );
            };
            bar(canvas, sx, sy);
            bar(canvas, ex, ey);
            canvas.fill_circle(sx.round(), sy - KNOB / 2.0, KNOB / 2.0, p.accent, 1.0);
            canvas.fill_circle(
                ex.round(),
                ey + line_h + KNOB / 2.0,
                KNOB / 2.0,
                p.accent,
                1.0,
            );
            if let Some(copy) = self.copy_rect(cx) {
                widgets::fill(canvas, copy, p.surface_background);
                widgets::border(canvas, copy, HAIRLINE, c.menu_border);
                let ty = theme::T_MENU;
                widgets::text_centered(
                    canvas,
                    "Copy",
                    copy.x + copy.w / 2.0,
                    ty.baseline(copy.y + (copy.h - ty.line()) / 2.0),
                    ty,
                    p.text,
                    if self.pressed == Some(Control::CopySelection) {
                        0.5
                    } else {
                        1.0
                    },
                );
            }
        });
    }

    fn draw_find_bar(&self, canvas: &mut Canvas, cx: &Cx, find: &Find) {
        let p = theme::p();
        let c = theme::c();
        let top = TOP_BAR + HAIRLINE;
        widgets::fill(
            canvas,
            RectF::new(0.0, top, cx.width, FIND_BAR),
            p.app_background,
        );
        widgets::hairline(canvas, 0.0, cx.width, top + FIND_BAR, c.rule_topbar);
        let field = self.field_rect(cx);
        widgets::fill(canvas, field, p.surface_background);
        widgets::border(
            canvas,
            field,
            HAIRLINE,
            if find.focused {
                c.field_focused
            } else {
                theme::mix(p.border, 50, p.surface_background)
            },
        );
        let ty = T_BODY;
        let baseline = ty.baseline(field.y + (field.h - ty.line()) / 2.0);
        let room = field.w - 2.0 * 28.0;
        let caret_x = if find.query.is_empty() {
            let placeholder = widgets::fit(&format!("Find in {}", self.title), ty, room);
            widgets::text(
                canvas,
                &placeholder,
                field.x + 28.0,
                baseline,
                ty,
                c.placeholder,
                1.0,
            );
            field.x + 28.0
        } else {
            // The end of the query shows (it scrolls left as it grows).
            let mut shown: String = find.query.clone();
            while widgets::measure(&shown, ty) > room && !shown.is_empty() {
                shown.remove(0);
            }
            let w = widgets::text(canvas, &shown, field.x + 28.0, baseline, ty, p.text, 1.0);
            field.x + 28.0 + w
        };
        if find.focused {
            widgets::fill(
                canvas,
                RectF::new(caret_x.round() + 2.0, field.y + 31.0, 4.0, field.h - 62.0),
                p.accent,
            );
        }
        let status = find::status(&find.query, find.matches.len(), find.current);
        widgets::text_right(
            canvas,
            &status,
            cx.width - 2.0 * TARGET - 11.0,
            T_META.baseline(field.y + (field.h - T_META.line()) / 2.0),
            T_META,
            p.muted,
            1.0,
        );
        let cy = field.y + field.h / 2.0;
        let enabled = !find.matches.is_empty();
        for (control, icon, cx_) in [
            (Control::Previous, Icon::ChevronUp, cx.width - 1.5 * TARGET),
            (Control::Next, Icon::ChevronDown, cx.width - 0.5 * TARGET),
        ] {
            let alpha = if !enabled {
                0.3
            } else if self.pressed == Some(control) {
                0.5
            } else {
                1.0
            };
            icons::draw(canvas, icon, cx_, cy, 52.0, p.secondary_accent, alpha);
        }
    }
}

impl Overlay for Reader {
    fn label(&self) -> String {
        format!("reader: {}", self.title)
    }

    fn layout(&mut self, cx: &Cx) {
        let width = cx.width - 2.0 * PAD.1;
        if self.wrapped_for != width {
            self.wrap(width);
        }
    }

    fn body(&self, cx: &Cx) -> Body {
        let (y0, y1) = self.area(cx);
        let mut key = DefaultHasher::new();
        (
            &self.title,
            self.text.len(),
            self.pressed,
            self.lines.len(),
            self.find
                .as_ref()
                .map(|find| (&find.query, find.current, find.matches.len(), find.focused)),
            cx.width.to_bits(),
            cx.height.to_bits(),
            self.selection,
        )
            .hash(&mut key);
        Body {
            rows: (0, cx.height as i32),
            // A selection's knobs and COPY do not scroll with the text.
            solid: None,
            scroller: self.selection.is_none().then_some(Scroller {
                y0: y0 as i32,
                y1: y1 as i32,
                offset: self.scroll.value(cx.now).round() as i32,
            }),
            key: if self.selection.is_some() {
                key.finish() ^ self.scroll.value(cx.now).round().to_bits() as u64
            } else {
                key.finish()
            },
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        widgets::fill(
            canvas,
            RectF::new(0.0, 0.0, cx.width, cx.height),
            p.app_background,
        );
        self.draw_text(canvas, cx);
        widgets::top_bar(canvas, p.app_background);
        let label_ty = T_NOTE.weight(Weight::MonoSemiBold);
        let cy = TOP_BAR / 2.0;
        let alpha = |control| {
            if self.pressed == Some(control) {
                0.5
            } else {
                1.0
            }
        };
        widgets::text(
            canvas,
            "Done",
            44.0,
            label_ty.baseline(cy - label_ty.line() / 2.0),
            label_ty,
            p.accent,
            alpha(Control::Done),
        );
        let title = widgets::fit(&self.title, T_TITLE, cx.width - 2.0 * 260.0);
        widgets::text_centered(canvas, &title, cx.width / 2.0, 47.0, T_TITLE, p.text, 1.0);
        let count = self.text.chars().count();
        let meta = format!(
            "{} character{}",
            group(count),
            if count == 1 { "" } else { "s" }
        );
        widgets::text_centered(
            canvas,
            &meta,
            cx.width / 2.0,
            96.0,
            T_SUBTITLE,
            theme::c().text_dim,
            1.0,
        );
        let find_color = if self.find_open() {
            p.accent
        } else {
            p.secondary_accent
        };
        icons::draw(
            canvas,
            Icon::Search,
            cx.width - 1.5 * TARGET,
            cy,
            theme::ICON,
            find_color,
            alpha(Control::Find),
        );
        icons::draw(
            canvas,
            Icon::Copy,
            cx.width - 0.5 * TARGET,
            cy,
            theme::ICON,
            p.secondary_accent,
            alpha(Control::Copy),
        );
        if let Some(find) = &self.find {
            self.draw_find_bar(canvas, cx, find);
        }
        self.draw_selection(canvas, cx);
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } => {
                self.pressed = self.control_at(cx, x, y);
                Response::Handled
            }
            Gesture::DragStart { x, y, .. } if matches!(self.pressed, Some(Control::Handle(_))) => {
                if let Some(Control::Handle(start)) = self.pressed.take() {
                    self.dragging = Some(start);
                    self.extend(cx, x, y);
                }
                Response::Handled
            }
            Gesture::Drag { x, y, .. } if self.dragging.is_some() => {
                self.extend(cx, x, y);
                Response::Handled
            }
            Gesture::DragEnd { .. } if self.dragging.is_some() => {
                self.dragging = None;
                Response::Handled
            }
            Gesture::LongPress { x, y } => {
                self.pressed = None;
                if y >= self.area(cx).0 {
                    let at = self.offset_at(cx, x, y);
                    self.selection = Some(self.word_at(at));
                    self.dragging = Some(false);
                    // A long press recognised (roadmap §3.8).
                    self.haptic = Some(Haptic::Medium);
                }
                Response::Handled
            }
            Gesture::LongPressMove { x, y } => {
                if self.dragging.is_some() {
                    self.extend(cx, x, y);
                }
                Response::Handled
            }
            Gesture::LongPressEnd { .. } => {
                self.dragging = None;
                Response::Handled
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                start_y,
                y,
                ..
            } => {
                self.pressed = None;
                self.scrolling = true;
                self.scroll.begin(start_y, cx.now);
                self.scroll.moved(y, self.max_scroll(cx), cx.now);
                Response::Handled
            }
            Gesture::Drag { y, .. } if self.scrolling => {
                self.scroll.moved(y, self.max_scroll(cx), cx.now);
                Response::Handled
            }
            Gesture::DragEnd { .. } if self.scrolling => {
                self.scrolling = false;
                self.scroll.end(self.max_scroll(cx), cx.now);
                Response::Handled
            }
            Gesture::Tap { x, y } => {
                self.pressed = None;
                match self.control_at(cx, x, y) {
                    Some(Control::Done) => Response::Action(Action::Ui(UiAct::Dismiss)),
                    Some(Control::Copy) => {
                        Response::Action(Action::Ui(UiAct::Copy(self.text.clone())))
                    }
                    Some(Control::Find) => {
                        if self.find.take().is_none() {
                            self.find = Some(Find {
                                focused: true,
                                ..Find::default()
                            });
                        }
                        Response::Handled
                    }
                    Some(Control::Field) => {
                        if let Some(find) = &mut self.find {
                            find.focused = true;
                        }
                        Response::Handled
                    }
                    Some(Control::Previous) => {
                        self.step(cx, -1);
                        Response::Handled
                    }
                    Some(Control::Next) => {
                        self.step(cx, 1);
                        Response::Handled
                    }
                    Some(Control::CopySelection) => {
                        let copied = self
                            .selection
                            .take()
                            .map(|(a, b)| self.text[a..b].to_owned())
                            .unwrap_or_default();
                        Response::Action(Action::Ui(UiAct::Copy(copied)))
                    }
                    Some(Control::Handle(_)) => Response::Handled,
                    None => {
                        // A tap on the text lets a selection go and puts
                        // the keyboard away.
                        self.selection = None;
                        if let Some(find) = &mut self.find {
                            find.focused = false;
                        }
                        Response::Handled
                    }
                }
            }
            Gesture::Back => Response::Action(Action::Ui(UiAct::Dismiss)),
            _ => {
                self.pressed = None;
                Response::Handled
            }
        }
    }

    fn key(&mut self, input: &KeyInput, cx: &Cx) -> Response {
        let Some(find) = &mut self.find else {
            return Response::Pass;
        };
        match input {
            KeyInput::Character(c) => find.query.push(*c),
            KeyInput::Backspace => {
                find.query.pop();
            }
            KeyInput::Enter => {
                // Moon puts its keyboard away on Return: the next match.
                find.focused = false;
                self.step(cx, 1);
                return Response::Handled;
            }
        }
        self.search();
        self.reveal(cx);
        Response::Handled
    }

    fn take_haptic(&mut self) -> Option<Haptic> {
        self.haptic.take()
    }

    fn keyboard(&self) -> Option<KeyboardPurpose> {
        self.find
            .as_ref()
            .filter(|find| find.focused)
            .map(|_| KeyboardPurpose::Search)
    }

    fn close(&mut self, _now: Instant) {
        self.closed = true;
    }

    fn closing(&self) -> bool {
        self.closed
    }

    fn finished(&self, _now: Instant) -> bool {
        self.closed
    }

    fn step(&mut self, cx: &Cx) -> bool {
        let max = self.max_scroll(cx);
        self.scroll.step(max, cx.now)
    }
}

/// 12345 as "12,345".
fn group(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Model;

    fn cx(model: &Model) -> Cx<'_> {
        Cx {
            model,
            width: 1080.0,
            height: 2216.0,
            keyboard: false,
            now: Instant::now(),
        }
    }

    #[test]
    fn text_keeps_its_lines_and_wraps_long_ones() {
        let model = Model::new();
        let text = format!("short\n\n  indented line\n{}\nend", "word ".repeat(60));
        let mut reader = Reader::new("T".into(), text.clone());
        reader.layout(&cx(&model));
        let lines: Vec<&str> = reader.lines.iter().map(|(a, b)| &text[*a..*b]).collect();
        assert_eq!(lines[0], "short");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "  indented line");
        // 300 characters at 41 per line: broken after spaces.
        let advance = text::measure("M", Weight::Mono, Reader::size());
        let per_line = ((1080.0 - 2.0 * PAD.1) / advance).floor() as usize;
        assert!(lines[3].len() <= per_line && lines[3].ends_with(' '));
        assert_eq!(*lines.last().unwrap(), "end");
        // A word longer than a line breaks anywhere.
        let long = "x".repeat(100);
        let mut reader = Reader::new("T".into(), long.clone());
        reader.layout(&cx(&model));
        assert_eq!(reader.lines.len(), 100usize.div_ceil(per_line));
    }

    #[test]
    fn a_long_press_selects_a_word_handles_extend_and_copy_copies() {
        let model = Model::new();
        let c = cx(&model);
        let mut reader = Reader::new("T".into(), "alpha beta_gamma delta\nsecond line".into());
        reader.layout(&c);
        let advance = text::measure("M", Weight::Mono, Reader::size());
        let top = reader.text_top(&c);
        let at = |col: f32, line: f32| {
            (
                PAD.1 + col * advance + 2.0,
                top + line * Reader::line_h() + 10.0,
            )
        };
        let (x, y) = at(8.0, 0.0);
        reader.gesture(&Gesture::LongPress { x, y }, &c);
        assert_eq!(reader.selection, Some((6, 16)), "beta_gamma");
        assert_eq!(reader.take_haptic(), Some(Haptic::Medium));
        assert_eq!(reader.take_haptic(), None, "once");
        // Moving on extends it to where the finger is.
        let (x, y) = at(4.0, 1.0);
        reader.gesture(&Gesture::LongPressMove { x, y }, &c);
        reader.gesture(&Gesture::LongPressEnd { x, y }, &c);
        let (a, b) = reader.selection.unwrap();
        assert_eq!(&reader.text[a..b], "beta_gamma delta\nseco");
        // The start handle drags back to the start.
        let (hx, hy) = reader.point_of(&c, a);
        reader.gesture(
            &Gesture::Down {
                x: hx,
                y: hy - KNOB,
            },
            &c,
        );
        let (x, y) = at(0.0, 0.0);
        reader.gesture(
            &Gesture::DragStart {
                axis: Axis::Horizontal,
                x,
                y,
                start_x: hx,
                start_y: hy - KNOB,
                edge: false,
            },
            &c,
        );
        reader.gesture(
            &Gesture::DragEnd {
                axis: Axis::Horizontal,
                x,
                y,
                vx: 0.0,
                vy: 0.0,
            },
            &c,
        );
        let (a, b) = reader.selection.unwrap();
        assert_eq!(&reader.text[a..b], "alpha beta_gamma delta\nseco");
        // COPY copies it and lets it go.
        let copy = reader.copy_rect(&c).unwrap();
        let (x, y) = (copy.x + 10.0, copy.y + 10.0);
        reader.gesture(&Gesture::Down { x, y }, &c);
        assert_eq!(
            reader.gesture(&Gesture::Tap { x, y }, &c),
            Response::Action(Action::Ui(UiAct::Copy(
                "alpha beta_gamma delta\nseco".into()
            )))
        );
        assert_eq!(reader.selection, None);
    }

    #[test]
    fn find_counts_steps_and_scrolls_to_the_match() {
        let model = Model::new();
        let c = cx(&model);
        let text: String = (0..300)
            .map(|i| format!("line {i}{}\n", if i % 100 == 50 { " needle" } else { "" }))
            .collect();
        let mut reader = Reader::new("Big".into(), text);
        reader.layout(&c);
        assert_eq!(reader.keyboard(), None);
        // The search icon opens Find with the keyboard.
        reader.gesture(
            &Gesture::Tap {
                x: 1080.0 - 1.5 * TARGET,
                y: 60.0,
            },
            &c,
        );
        assert_eq!(reader.keyboard(), Some(KeyboardPurpose::Search));
        for ch in "NEEDLE".chars() {
            reader.key(&KeyInput::Character(ch), &c);
        }
        let find = reader.find.as_ref().unwrap();
        assert_eq!(find.matches.len(), 3);
        assert_eq!(find::status(&find.query, 3, find.current), "1/3");
        let first = reader.scroll.offset;
        assert!(first > 0.0);
        // Next, next, and round to the first again.
        reader.gesture(
            &Gesture::Tap {
                x: 1080.0 - 0.5 * TARGET,
                y: TOP_BAR + 60.0,
            },
            &c,
        );
        assert_eq!(reader.find.as_ref().unwrap().current, 1);
        assert!(reader.scroll.offset > first);
        reader.key(&KeyInput::Enter, &c);
        assert_eq!(reader.find.as_ref().unwrap().current, 2);
        assert_eq!(reader.keyboard(), None, "Return puts the keyboard away");
        reader.gesture(
            &Gesture::Tap {
                x: 1080.0 - 0.5 * TARGET,
                y: TOP_BAR + 60.0,
            },
            &c,
        );
        assert_eq!(reader.find.as_ref().unwrap().current, 0);
        // Copy takes the whole text; Done closes.
        assert!(matches!(
            reader.gesture(&Gesture::Tap { x: 1080.0 - 0.5 * TARGET, y: 60.0 }, &c),
            Response::Action(Action::Ui(UiAct::Copy(text))) if text.len() > 2000
        ));
        assert_eq!(
            reader.gesture(&Gesture::Tap { x: 60.0, y: 60.0 }, &c),
            Response::Action(Action::Ui(UiAct::Dismiss))
        );
        assert_eq!(group(1234567), "1,234,567");
    }
}
