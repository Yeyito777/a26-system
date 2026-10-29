//! A multi-line text sheet (WP-E; the iPhone's `ops-textarea`): a title, a
//! message, a field of several lines typed into with Moon's keyboard
//! (Return types a line break; the keyboard's multiline purpose keeps it
//! up), Paste and Clear, then Cancel and a confirming button. The goal's
//! objective, a chat's instructions and an aside's question are written in
//! it. Editing and wrapping are the composer's (`editor.rs`), so a tap
//! places the caret on the glyph under it.

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::control::KeyboardPurpose;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;

use super::super::clipboard;
use super::super::editor::{self, Cursor, Line, Metrics};
use super::super::model::Action;
use super::super::theme::{self, HAIRLINE, T_BODY, T_NOTE, T_SMALL, TARGET};
use super::overlay::Cx;
use super::sheet::{Reply, Sheet, SheetContent};
use super::widgets::{self, Button, ButtonKind};

/// Lines the field shows at least and at most (then it scrolls inside).
const MIN_LINES: usize = 3;
const MAX_LINES: usize = 7;
/// The field's padding and the caret.
const PAD: (f32, f32) = (20.0, 28.0);
const CARET: (f32, f32) = (5.0, 52.0);
/// The tool row (Paste, Clear).
const TOOLS: f32 = 100.0;
const GAP: f32 = 22.0;

/// What a tap is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Field,
    Paste,
    Clear,
    Cancel,
    Confirm,
}

const PARTS: [Part; 5] = [
    Part::Field,
    Part::Paste,
    Part::Clear,
    Part::Cancel,
    Part::Confirm,
];

pub struct TextArea {
    pub title: String,
    pub message: String,
    pub text: String,
    cursor: Cursor,
    pub confirm: String,
    /// The confirming button works with nothing typed (clearing).
    pub allow_empty: bool,
    pub submit: Box<dyn Fn(String) -> Action>,
    width: f32,
    message_lines: Vec<String>,
    lines: Vec<Line>,
    /// The first line the field shows.
    first: usize,
    /// Where the last `item_at` looked (a tap's place, for the caret).
    probe: Cell<(f32, f32)>,
}

impl TextArea {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        text: impl Into<String>,
        confirm: impl Into<String>,
        submit: Box<dyn Fn(String) -> Action>,
    ) -> Self {
        let text = text.into();
        Self {
            title: title.into(),
            message: message.into(),
            cursor: Cursor::at(text.len()),
            text,
            confirm: confirm.into(),
            allow_empty: false,
            submit,
            width: theme::W,
            message_lines: Vec::new(),
            lines: Vec::new(),
            first: 0,
            probe: Cell::new((0.0, 0.0)),
        }
    }

    /// The confirming button also works with nothing typed.
    pub fn allow_empty(mut self) -> Self {
        self.allow_empty = true;
        self
    }

    fn metrics() -> Metrics {
        let ty = T_BODY;
        Metrics::new(ty.weight, ty.px())
    }

    fn inner_width(&self) -> f32 {
        self.width - 2.0 * PAD.1
    }

    fn message_h(&self) -> f32 {
        if self.message_lines.is_empty() {
            0.0
        } else {
            self.message_lines.len() as f32 * T_NOTE.line() + GAP
        }
    }

    fn visible(&self) -> usize {
        self.lines.len().clamp(MIN_LINES, MAX_LINES)
    }

    fn field(&self) -> RectF {
        let h = 2.0 * PAD.0 + self.visible() as f32 * T_BODY.line();
        RectF::new(0.0, self.message_h(), self.width, h)
    }

    fn tools(&self) -> (RectF, RectF) {
        let field = self.field();
        let y = field.y + field.h;
        let w = 200.0;
        (RectF::new(0.0, y, w, TOOLS), RectF::new(w, y, w, TOOLS))
    }

    fn buttons(&self) -> (RectF, RectF) {
        let (paste, _) = self.tools();
        let y = paste.y + TOOLS + GAP;
        let w = ((self.width - GAP) / 2.0).floor();
        (
            RectF::new(0.0, y, w, TARGET),
            RectF::new(w + GAP, y, w, TARGET),
        )
    }

    fn rect(&self, part: Part) -> RectF {
        match part {
            Part::Field => self.field(),
            Part::Paste => self.tools().0,
            Part::Clear => self.tools().1,
            Part::Cancel => self.buttons().0,
            Part::Confirm => self.buttons().1,
        }
    }

    fn can_submit(&self) -> bool {
        self.allow_empty || !self.text.trim().is_empty()
    }

    /// Wraps the text again and scrolls the caret's line into the field.
    fn relayout(&mut self) {
        let metrics = Self::metrics();
        self.lines = editor::wrap(&self.text, self.inner_width(), &metrics);
        let caret = editor::line_of(&self.lines, self.cursor.caret);
        let visible = self.visible();
        if caret < self.first {
            self.first = caret;
        } else if caret >= self.first + visible {
            self.first = caret + 1 - visible;
        }
        self.first = self.first.min(self.lines.len().saturating_sub(visible));
    }

    /// The text index under (`x`, `y`) in the field.
    fn index_at(&self, x: f32, y: f32) -> usize {
        let field = self.field();
        let row = ((y - field.y - PAD.0) / T_BODY.line()).floor().max(0.0) as usize + self.first;
        let Some(line) = self.lines.get(row.min(self.lines.len().saturating_sub(1))) else {
            return self.text.len();
        };
        editor::index_at(&self.text, line, x - field.x - PAD.1, &Self::metrics())
    }

    fn edit(&mut self, change: impl FnOnce(&mut Cursor, &mut String) -> bool) -> Reply {
        let changed = change(&mut self.cursor, &mut self.text);
        self.relayout();
        if changed { Reply::Redraw } else { Reply::None }
    }

    fn submit(&self) -> Reply {
        if !self.can_submit() {
            return Reply::None;
        }
        Reply::CloseWith((self.submit)(self.text.trim_end().to_owned()))
    }
}

/// A text sheet.
pub fn sheet(area: TextArea) -> Sheet<TextArea> {
    Sheet::new(area)
}

impl SheetContent for TextArea {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn layout(&mut self, _cx: &Cx, width: f32) -> f32 {
        if (width - self.width).abs() > 0.5
            || self.message_lines.is_empty() && !self.message.is_empty()
        {
            self.width = width;
            self.message_lines = widgets::wrap_plain(&self.message, T_NOTE, width);
            self.relayout();
        } else if self.lines.is_empty() {
            self.relayout();
        }
        let (_, confirm) = self.buttons();
        confirm.y + confirm.h
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        let at = |r: RectF| RectF::new(r.x + x, r.y + top, r.w, r.h);
        for (index, line) in self.message_lines.iter().enumerate() {
            widgets::text(
                canvas,
                line,
                x,
                T_NOTE.baseline(top + index as f32 * T_NOTE.line()),
                T_NOTE,
                c.text_dim,
                1.0,
            );
        }
        let field = at(self.field());
        widgets::boxed(canvas, field, p.surface_background, c.field_focused);
        let metrics = Self::metrics();
        let line_h = T_BODY.line();
        let inner = RectF::new(
            field.x + HAIRLINE,
            field.y + HAIRLINE,
            field.w - 2.0 * HAIRLINE,
            field.h - 2.0 * HAIRLINE,
        );
        let text_x = field.x + PAD.1;
        let row_top = |row: usize| field.y + PAD.0 + (row - self.first) as f32 * line_h;
        let rows = self.first..(self.first + self.visible()).min(self.lines.len());
        widgets::clipped(canvas, inner, |canvas| {
            if self.text.is_empty() {
                return;
            }
            if let Some((a, b)) = self.cursor.selection() {
                for row in rows.clone() {
                    let line = &self.lines[row];
                    if b < line.start || a > line.end {
                        continue;
                    }
                    let x0 = editor::x_of(&self.text, line, a.max(line.start), &metrics);
                    let x1 = editor::x_of(&self.text, line, b.min(line.end), &metrics);
                    if x1 > x0 {
                        canvas.fill_rect(
                            (text_x + x0).round() as i32,
                            row_top(row).round() as i32,
                            (x1 - x0).round().max(1.0) as i32,
                            line_h as i32,
                            c.editor_selection,
                            1.0,
                        );
                    }
                }
            }
            for row in rows.clone() {
                let line = &self.lines[row];
                if line.end > line.start {
                    crate::text::draw(
                        canvas,
                        &self.text[line.start..line.end],
                        text_x,
                        T_BODY.baseline(row_top(row)),
                        metrics.weight,
                        metrics.size,
                        p.text,
                        1.0,
                    );
                }
            }
        });
        if self.text.is_empty() {
            widgets::text(
                canvas,
                "Type here",
                text_x,
                T_BODY.baseline(field.y + PAD.0),
                T_BODY,
                c.placeholder,
                1.0,
            );
        }
        // The caret (steady: the sheet does not blink).
        if self.cursor.selection().is_none() {
            let row = editor::line_of(&self.lines, self.cursor.caret);
            if let Some(line) = self.lines.get(row).filter(|_| rows.contains(&row)) {
                let cx = text_x + editor::x_of(&self.text, line, self.cursor.caret, &metrics);
                let empty = self.text.is_empty();
                canvas.fill_rect(
                    (if empty { text_x - 2.0 } else { cx }).round() as i32,
                    (row_top(row) + (line_h - CARET.1) / 2.0).round() as i32,
                    CARET.0 as i32,
                    CARET.1 as i32,
                    p.accent,
                    1.0,
                );
            }
        }
        if self.lines.len() > self.visible() {
            let shown = format!(
                "{}–{} of {} lines",
                self.first + 1,
                self.first + self.visible(),
                self.lines.len()
            );
            widgets::text_right(
                canvas,
                &shown,
                x + self.width,
                T_SMALL.baseline(field.y + field.h + (TOOLS - T_SMALL.line()) / 2.0),
                T_SMALL,
                p.muted,
                1.0,
            );
        }
        for (part, label) in [(Part::Paste, "Paste"), (Part::Clear, "Clear")] {
            let r = at(self.rect(part));
            let down = pressed.and_then(|i| PARTS.get(i)) == Some(&part);
            widgets::text(
                canvas,
                label,
                r.x + if part == Part::Paste { 0.0 } else { 28.0 },
                T_SMALL.baseline(r.y + (TOOLS - T_SMALL.line()) / 2.0),
                T_SMALL,
                p.secondary_accent,
                if down { 0.5 } else { 1.0 },
            );
        }
        let (cancel, confirm) = self.buttons();
        let down = |part: Part| pressed.and_then(|i| PARTS.get(i)) == Some(&part);
        widgets::button(
            canvas,
            at(cancel),
            &Button::new("Cancel", ButtonKind::Secondary).pressed(down(Part::Cancel)),
            p.sidebar_background,
        );
        widgets::button(
            canvas,
            at(confirm),
            &Button::new(&self.confirm, ButtonKind::Primary)
                .pressed(down(Part::Confirm))
                .disabled(!self.can_submit()),
            p.sidebar_background,
        );
    }

    fn item_at(&self, _cx: &Cx, x: f32, y: f32) -> Option<usize> {
        self.probe.set((x, y));
        PARTS
            .iter()
            .position(|part| self.rect(*part).contains(x, y))
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        match PARTS.get(index) {
            Some(Part::Field) => {
                let (x, y) = self.probe.get();
                let index = self.index_at(x, y);
                self.edit(|cursor, _| {
                    *cursor = Cursor::at(index);
                    true
                })
            }
            Some(Part::Paste) => match clipboard::paste().filter(|t| !t.is_empty()) {
                Some(pasted) => self.edit(|cursor, text| cursor.insert(text, &pasted)),
                None => Reply::None,
            },
            Some(Part::Clear) => self.edit(|cursor, text| {
                text.clear();
                *cursor = Cursor::at(0);
                true
            }),
            Some(Part::Cancel) => Reply::Close,
            Some(Part::Confirm) => self.submit(),
            None => Reply::None,
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (
            &self.title,
            &self.message,
            &self.text,
            self.cursor,
            self.first,
            &self.confirm,
            theme::generation(),
        )
            .hash(&mut hasher);
        hasher.finish()
    }

    fn scoped(&self) -> bool {
        true
    }

    fn key_input(&mut self, input: &KeyInput, _cx: &Cx) -> Reply {
        match input {
            KeyInput::Character(character) => {
                let typed = character.to_string();
                self.edit(|cursor, text| cursor.insert(text, &typed))
            }
            KeyInput::Backspace => self.edit(|cursor, text| cursor.backspace(text)),
            // Return types a line break; the button submits.
            KeyInput::Enter => self.edit(|cursor, text| cursor.insert(text, "\n")),
        }
    }

    fn keyboard(&self) -> Option<KeyboardPurpose> {
        Some(KeyboardPurpose::Multiline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::{Model, UiAct};
    use std::time::Instant;

    fn cx(model: &Model) -> Cx<'_> {
        Cx {
            model,
            width: 1080.0,
            height: 2216.0,
            keyboard: true,
            now: Instant::now(),
        }
    }

    #[test]
    fn typing_wraps_breaks_lines_and_submits() {
        let model = Model::new();
        let cx = cx(&model);
        let mut area = TextArea::new(
            "Instructions",
            "Replaces them",
            "",
            "Save",
            Box::new(|text| Action::Ui(UiAct::Toast(text))),
        );
        area.layout(&cx, 992.0);
        assert_eq!(area.visible(), MIN_LINES);
        let confirm = PARTS.iter().position(|p| *p == Part::Confirm).unwrap();
        assert_eq!(area.activate(&cx, confirm), Reply::None, "nothing typed");
        for c in "Be brief.".chars() {
            area.key_input(&KeyInput::Character(c), &cx);
        }
        area.key_input(&KeyInput::Enter, &cx);
        for c in "Answer in French.".chars() {
            area.key_input(&KeyInput::Character(c), &cx);
        }
        assert_eq!(area.text, "Be brief.\nAnswer in French.");
        assert_eq!(area.lines.len(), 2);
        // Many lines scroll inside the field, keeping the caret in view.
        for _ in 0..12 {
            area.key_input(&KeyInput::Enter, &cx);
            area.key_input(&KeyInput::Character('x'), &cx);
        }
        assert_eq!(area.visible(), MAX_LINES);
        assert_eq!(area.first, area.lines.len() - MAX_LINES);
        // A tap on the first shown line puts the caret there.
        let field = area.field();
        area.item_at(&cx, field.x + PAD.1 + 1.0, field.y + PAD.0 + 5.0);
        area.activate(&cx, 0);
        assert_eq!(editor::line_of(&area.lines, area.cursor.caret), area.first);
        assert_eq!(
            area.activate(&cx, confirm),
            Reply::CloseWith(Action::Ui(UiAct::Toast(area.text.trim_end().to_owned())))
        );
        // Clear, then an empty submit only where allowed.
        let clear = PARTS.iter().position(|p| *p == Part::Clear).unwrap();
        area.activate(&cx, clear);
        assert!(area.text.is_empty());
        assert_eq!(area.activate(&cx, confirm), Reply::None);
        area.allow_empty = true;
        assert!(matches!(area.activate(&cx, confirm), Reply::CloseWith(_)));
    }
}
