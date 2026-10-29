//! The composer island (WP-D; roadmap §3.6.15): a rounded surface above
//! Moon's keyboard or home gesture with "+" on the left, the editor, and on
//! the right Send (accent), Queue (warning: a turn is running; it asks when
//! the message joins it), a spinner while a message is on its way, and
//! Stop while a turn runs. Above it, the draft's error label (PAR-186).
//!
//! The editor draws the text wrapped by `editor::wrap` (so a touch maps to
//! the glyph under it), six lines at most and then scrolled inside, the
//! caret blinking in the accent, the selection with its two handles, and
//! the edit menu (`EditMenu`: Cut, Copy, Paste, Select, Select all) over
//! it.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::gfx::{Canvas, RectF};

use super::super::anim::{self, Animator, Tween};
use super::super::dictation;
use super::super::editor::{self, Line, Metrics};
use super::super::gesture::Gesture;
#[cfg(test)]
use super::super::model::composer;
use super::super::model::composer::{Act, End, VoicePhase};
use super::super::model::{Action, Model, Page};
use super::super::theme::{
    self, COMPOSER_BUTTON, COMPOSER_FACE, COMPOSER_GAP, COMPOSER_MARGIN, EDITOR_INSET,
    EDITOR_LINES, EDITOR_PAD_Y, GESTURE_RESERVE, HAIRLINE, ISLAND_MIN, ISLAND_PAD, ISLAND_RADIUS,
    S1, S2, T_CAPTION, T_COMPOSER, T_MENU, T_NOTE, W,
};
use super::super::ui::icons::{self, Icon, Style};
use super::super::ui::overlay::{Body, Cx, Overlay, Response};
use super::super::ui::widgets;
use super::{CHAT_TOP, Hit};

/// The band above the island filled with the background.
const BAND: f32 = 17.0;
/// The error label: lines at most, the gap under it, its fade.
const ERROR_LINES: usize = 3;
const ERROR_GAP: f32 = 19.0;
const ERROR_IN: Duration = Duration::from_millis(200);
const ERROR_RISE: f32 = 17.0;
/// A finger this close to a selection handle (or the caret, with the
/// keyboard up) drags it rather than scrolling.
const GRAB: f32 = 70.0;
/// The caret.
const CARET_W: f32 = 5.0;
const CARET_H: f32 = 52.0;
/// The handles' dots.
const HANDLE_R: f32 = 13.0;
/// The Run capsule's width (its height is a button face's).
const RUN_W: f32 = 133.0;
/// The spinner in a button's face, and how often it is drawn a step on
/// (20 steps a turn).
const SPINNER: f32 = 39.0;
const SPINNER_STEP_MS: u64 = 35;

/// The time a spinner is drawn at: `now` back to the start of its step, so
/// every frame within a step draws the same pixels (the frame key names the
/// step).
fn spinner_time(now: Instant) -> Instant {
    let into = (anim::phase(now, SPINNER_STEP_MS) * SPINNER_STEP_MS as f32).round() as u64;
    now.checked_sub(Duration::from_millis(into)).unwrap_or(now)
}

fn spinner_step(now: Instant) -> u32 {
    (anim::phase(spinner_time(now), 700) * 20.0).round() as u32
}

/// The button left of Stop (or rightmost).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primary {
    None,
    Send {
        enabled: bool,
    },
    /// A turn is running: Send asks when the message joins it.
    Queue {
        enabled: bool,
    },
    /// A message from this draft is on its way.
    Busy,
    /// Nothing typed: dictate (PAR-180, PAR-243).
    Mic,
    /// A command is typed: Run it (PAR-183).
    Run {
        enabled: bool,
    },
}

/// The composer's trailing buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Trailing {
    pub primary: Primary,
    /// Stop, while a turn runs (true: a stop is on its way).
    pub stop: Option<bool>,
}

pub fn trailing(model: &Model) -> Trailing {
    let typed = !model.composer.text.trim().is_empty();
    let streaming = model.chat.id.is_some() && model.streaming();
    let primary = if model.submitting() {
        Primary::Busy
    } else if !typed && dictation::available() {
        Primary::Mic
    } else if !typed {
        Primary::None
    } else if model.composer.is_command {
        Primary::Run {
            enabled: model.ready,
        }
    } else if streaming {
        Primary::Queue {
            enabled: model.ready,
        }
    } else {
        Primary::Send {
            enabled: model.ready,
        }
    };
    Trailing {
        primary,
        stop: streaming.then(|| model.stopping()),
    }
}

/// The error label over the island.
pub struct ErrorLabel {
    /// Its rows (where it ends up once risen).
    pub rect: RectF,
    pub lines: Vec<String>,
    /// The ✕'s centre.
    pub close: (f32, f32),
    /// When it appeared (it rises and fades in).
    pub since: Instant,
}

/// How far an error label's entrance is at `now`, in 16 steps.
fn error_step(since: Instant, now: Instant) -> u32 {
    let progress = Tween::new(since, ERROR_IN, 0.0, 1.0)
        .with_curve(anim::ease_out)
        .value(now.max(since));
    (progress * 16.0).round() as u32
}

/// Where the island, its editor and buttons are.
pub struct ComposerGeometry {
    pub island: RectF,
    /// The editor's text: lines start at `editor.y - scroll`, `line` apart.
    pub editor: RectF,
    /// Where the editor's lines show (they scroll under the padding).
    pub viewport: RectF,
    pub lines: Rc<Vec<Line>>,
    pub metrics: Metrics,
    pub scroll: f32,
    /// The "+" button's centre.
    pub plus: (f32, f32),
    /// The primary trailing button's centre.
    pub send: (f32, f32),
    /// Stop's centre, while a turn runs.
    pub stop: Option<(f32, f32)>,
    pub trailing: Trailing,
    pub error: Option<ErrorLabel>,
}

impl ComposerGeometry {
    /// Where the composer's band (and so the transcript's bottom) begins.
    pub fn band_top(&self) -> f32 {
        let top = match &self.error {
            Some(error) => error.rect.y,
            None => self.island.y,
        };
        top - BAND
    }

    fn line(&self) -> f32 {
        T_COMPOSER.line()
    }

    /// Where text index `index` is drawn: its x and its line's top.
    pub fn locate(&self, text: &str, index: usize) -> (f32, f32) {
        let row = editor::line_of(&self.lines, index);
        let line = &self.lines[row];
        let x = editor::x_of(text, line, index, &self.metrics).min(self.editor.w);
        (
            self.editor.x + x,
            self.editor.y + row as f32 * self.line() - self.scroll,
        )
    }

    /// The text index nearest to (`x`, `y`).
    pub fn index_at(&self, text: &str, x: f32, y: f32) -> usize {
        let row = ((y - self.editor.y + self.scroll) / self.line()).floor();
        let row = (row.max(0.0) as usize).min(self.lines.len() - 1);
        editor::index_at(text, &self.lines[row], x - self.editor.x, &self.metrics)
    }

    fn max_scroll(&self) -> f32 {
        (self.lines.len() as f32 * self.line() - self.editor.h).max(0.0)
    }
}

/// The editor's face at the current text size.
fn metrics() -> Metrics {
    thread_local! {
        static LAST: RefCell<Option<(u64, Metrics)>> = const { RefCell::new(None) };
    }
    let generation = theme::generation();
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        match *last {
            Some((at, metrics)) if at == generation => metrics,
            _ => {
                let metrics = Metrics::new(T_COMPOSER.weight, T_COMPOSER.px());
                *last = Some((generation, metrics));
                metrics
            }
        }
    })
}

/// The composer's text wrapped, kept until it changes: the island's height
/// (and so the transcript's bounds) is asked for many times a frame.
/// The last text wrapped: the text, the width, the theme generation, its
/// lines.
type Wrapped = (String, f32, u64, Option<Rc<Vec<Line>>>);

fn wrapped(text: &str, width: f32) -> Rc<Vec<Line>> {
    thread_local! {
        static LAST: RefCell<Wrapped> = const { RefCell::new((String::new(), 0.0, 0, None)) };
    }
    let generation = theme::generation();
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        let fresh = last.1 == width && last.2 == generation && last.0 == text;
        match &last.3 {
            Some(lines) if fresh => lines.clone(),
            _ => {
                let lines = Rc::new(editor::wrap(text, width, &metrics()));
                *last = (text.to_owned(), width, generation, Some(lines.clone()));
                lines
            }
        }
    })
}

pub fn geometry(model: &Model, height: f32, keyboard: bool) -> ComposerGeometry {
    let bottom = height - COMPOSER_GAP - if keyboard { 0.0 } else { GESTURE_RESERVE };
    let trailing = trailing(model);
    let x0 = COMPOSER_MARGIN;
    let x1 = W - COMPOSER_MARGIN;
    let editor_x = x0 + ISLAND_PAD + COMPOSER_BUTTON + EDITOR_INSET - 12.0;
    let buttons = 1.0 + if trailing.stop.is_some() { 1.0 } else { 0.0 };
    let editor_right = x1 - ISLAND_PAD - buttons * COMPOSER_BUTTON;
    let lines = wrapped(&model.composer.text, editor_right - editor_x);
    // Dictating, the recording strip takes the row: one line high.
    let shown = if model.composer.voice.active() {
        1
    } else {
        lines.len().clamp(1, EDITOR_LINES)
    };
    let line = T_COMPOSER.line();
    let h = (shown as f32 * line + 2.0 * EDITOR_PAD_Y + 2.0 * ISLAND_PAD).max(ISLAND_MIN);
    let island = RectF::new(x0, (bottom - h).round(), x1 - x0, h.round());
    let row_cy = island.y + island.h - ISLAND_PAD - COMPOSER_BUTTON / 2.0;
    let editor_h = shown as f32 * line;
    let editor = RectF::new(
        editor_x,
        (island.y + (island.h - editor_h) / 2.0).round(),
        editor_right - editor_x,
        editor_h,
    );
    let viewport = RectF::new(
        editor_x - EDITOR_INSET / 2.0,
        island.y + ISLAND_PAD,
        editor_right - editor_x + EDITOR_INSET / 2.0,
        island.h - 2.0 * ISLAND_PAD,
    );
    let right = (x1 - ISLAND_PAD - COMPOSER_BUTTON / 2.0, row_cy);
    let (send, stop) = match trailing.stop {
        Some(_) => ((right.0 - COMPOSER_BUTTON, row_cy), Some(right)),
        None => (right, None),
    };
    let error = model.composer_error_label().map(|error| {
        let text_x = x0 + 28.0 + 36.0 + S1 + 6.0;
        let close = x1 - COMPOSER_BUTTON / 2.0 + 17.0;
        let mut lines = widgets::wrap_plain(
            &error.text,
            T_CAPTION,
            close - COMPOSER_BUTTON / 2.0 - text_x,
        );
        if lines.len() > ERROR_LINES {
            lines.truncate(ERROR_LINES);
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
        }
        let h = lines.len().max(1) as f32 * T_CAPTION.line();
        let rect = RectF::new(x0, island.y - ERROR_GAP - h, x1 - x0, h);
        ErrorLabel {
            close: (close, rect.y + T_CAPTION.line() / 2.0),
            rect,
            lines,
            since: error.since,
        }
    });
    let mut geometry = ComposerGeometry {
        island,
        editor,
        viewport,
        lines,
        metrics: metrics(),
        scroll: 0.0,
        plus: (x0 + ISLAND_PAD + COMPOSER_BUTTON / 2.0, row_cy),
        send,
        stop,
        trailing,
        error,
    };
    // The editor's scroll: kept inside the text, and moved (as little as
    // it takes) to show the caret after it moved or the text changed.
    let mut scroll = model
        .composer
        .scroll
        .get()
        .clamp(0.0, geometry.max_scroll());
    if model.composer.reveal.get() {
        let caret = model.composer.cursor().caret;
        let top = editor::line_of(&geometry.lines, caret) as f32 * line;
        if top < scroll {
            scroll = top;
        } else if top + line > scroll + editor_h {
            scroll = top + line - editor_h;
        }
        model.composer.reveal.set(false);
    }
    geometry.scroll = scroll.round();
    model.composer.scroll.set(geometry.scroll);
    geometry
}

/// Whether `action` is one of the composer's buttons.
pub fn owns(action: &Action) -> bool {
    matches!(
        action,
        Action::Composer(
            Act::Send
                | Act::Abort
                | Act::Attach
                | Act::DismissError
                | Act::Mic
                | Act::MicDone
                | Act::MicCancel
        )
    )
}

/// The caret shows (it blinks while the keyboard is up for the composer).
fn caret_shows(model: &Model, keyboard: bool, now: Instant) -> bool {
    keyboard
        && model.composer.cursor().selection().is_none()
        && model
            .composer
            .since
            .is_none_or(|since| anim::caret_on(since, now))
}

/// What the composer's pixels depend on (the scroll as the last geometry
/// left it: the frame lays the transcript out above it first).
pub fn frame_key(model: &Model, keyboard: bool, pressed: Option<&Action>, now: Instant) -> u64 {
    let mut hasher = DefaultHasher::new();
    let t = trailing(model);
    let busy = matches!(t.primary, Primary::Busy) || t.stop == Some(true);
    let error = model
        .composer_error_label()
        .map(|error| (&error.text, error_step(error.since, now)));
    (
        &model.composer.text,
        model.composer.cursor(),
        model.composer.scroll.get().to_bits(),
        keyboard,
        t,
        error,
        caret_shows(model, keyboard, now),
        busy.then(|| spinner_step(now)),
        voice_key(model, now),
        super::action_key(pressed.filter(|action| owns(action))),
        theme::generation(),
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// The caret's blink, a spinner, the error label's entrance.
pub fn animate(model: &Model, keyboard: bool, now: Instant, animator: &mut Animator) {
    if keyboard
        && model.composer.cursor().selection().is_none()
        && let Some(since) = model.composer.since
    {
        let half = Duration::from_millis(530);
        let elapsed = now.saturating_duration_since(since).as_millis() as u32 / 530;
        animator.request(since + half * (elapsed + 1));
    }
    let t = trailing(model);
    if matches!(t.primary, Primary::Busy) || t.stop == Some(true) || model.composer.voice.active() {
        animator.slow_frame(now);
    }
    if let Some(error) = model.composer_error_label()
        && now < error.since + ERROR_IN
    {
        animator.follow(&Tween::new(error.since, ERROR_IN, 0.0, 1.0), now);
    }
}

fn near((cx, cy): (f32, f32), x: f32, y: f32) -> bool {
    (x - cx).abs() < COMPOSER_BUTTON / 2.0 && (y - cy).abs() < COMPOSER_BUTTON / 2.0
}

/// The composer's target at (`x`, `y`): a button, or the editor (a tap
/// there places the caret, PAR-181).
pub fn hit(model: &Model, height: f32, keyboard: bool, x: f32, y: f32) -> Option<Hit> {
    let geometry = geometry(model, height, keyboard);
    if let Some(error) = &geometry.error
        && near(error.close, x, y)
    {
        return Some(Hit::Action(Action::Composer(Act::DismissError)));
    }
    // The recording strip: ✕ and ✓, nothing else.
    let voice = &model.composer.voice;
    if voice.active() {
        if near(geometry.plus, x, y) {
            return Some(Hit::Action(Action::Composer(Act::MicCancel)));
        }
        if voice.phase == VoicePhase::Recording && near(strip_finish(&geometry), x, y) {
            return Some(Hit::Action(Action::Composer(Act::MicDone)));
        }
        return None;
    }
    if near(geometry.plus, x, y) {
        return Some(Hit::Action(Action::Composer(Act::Attach)));
    }
    if let Some(stop) = geometry.stop
        && near(stop, x, y)
    {
        return Some(Hit::Action(Action::Composer(Act::Abort)));
    }
    if near(geometry.send, x, y) {
        match geometry.trailing.primary {
            Primary::Send { .. } | Primary::Queue { .. } | Primary::Run { .. } => {
                return Some(Hit::Action(Action::Composer(Act::Send)));
            }
            Primary::Mic => return Some(Hit::Action(Action::Composer(Act::Mic))),
            Primary::None | Primary::Busy => {}
        }
    }
    if geometry.island.inset(-16.0).contains(x, y) {
        let index = geometry.index_at(&model.composer.text, x, y);
        return Some(Hit::Action(Action::Composer(Act::Place {
            index,
            focused: keyboard,
        })));
    }
    // The band around it: the editor, its caret where it was.
    (y >= geometry.band_top()).then_some(Hit::Composer)
}

/// A long press in the editor: its word selected.
pub fn long_press(model: &Model, height: f32, keyboard: bool, x: f32, y: f32) -> Option<Action> {
    if model.composer.voice.active() {
        return None;
    }
    let geometry = geometry(model, height, keyboard);
    let buttons = [Some(geometry.plus), Some(geometry.send), geometry.stop];
    if !geometry.island.contains(x, y) || buttons.iter().flatten().any(|b| near(*b, x, y)) {
        return None;
    }
    Some(Action::Composer(Act::SelectWord(geometry.index_at(
        &model.composer.text,
        x,
        y,
    ))))
}

/// After a long press in the editor, the finger moving: the selection
/// follows it.
pub fn long_press_move(
    model: &Model,
    height: f32,
    keyboard: bool,
    x: f32,
    y: f32,
) -> Option<Action> {
    if !model.composer.selecting {
        return None;
    }
    let geometry = geometry(model, height, keyboard);
    Some(Action::Composer(Act::MoveEnd {
        end: End::Caret,
        index: geometry.index_at(&model.composer.text, x, y),
    }))
}

/// A drag the editor takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditDrag {
    /// An end of the selection (or the caret), the finger `dy` below where
    /// its line's middle was.
    End { end: End, dy: f32 },
    /// The editor's text, scrolled from `from` as the finger moves from `y`.
    Scroll { from: f32, y: f32 },
}

/// A drag starting at (`x`, `y`): the editor's, if it starts on a handle
/// (or the caret) or in text that scrolls.
pub fn drag_start(model: &Model, height: f32, keyboard: bool, x: f32, y: f32) -> Option<EditDrag> {
    if model.composer.voice.active() {
        return None;
    }
    let geometry = geometry(model, height, keyboard);
    if !geometry.island.inset(-16.0).contains(x, y) {
        return None;
    }
    let text = &model.composer.text;
    let line = T_COMPOSER.line();
    if keyboard {
        let cursor = model.composer.cursor();
        let ends = match cursor.selection() {
            Some((a, _)) if a == cursor.caret => {
                [(End::Caret, cursor.caret), (End::Anchor, cursor.anchor)]
            }
            Some(_) => [(End::Anchor, cursor.anchor), (End::Caret, cursor.caret)],
            None => [(End::Caret, cursor.caret), (End::Caret, cursor.caret)],
        };
        let mut best: Option<(f32, End, f32)> = None;
        for (end, index) in ends {
            let (hx, top) = geometry.locate(text, index);
            let middle = top + line / 2.0;
            let distance = ((x - hx).powi(2) + (y - middle).powi(2)).sqrt();
            if distance < GRAB && best.is_none_or(|(d, _, _)| distance < d) {
                best = Some((distance, end, y - middle));
            }
        }
        if let Some((_, end, dy)) = best {
            return Some(EditDrag::End { end, dy });
        }
    }
    (geometry.max_scroll() > 0.0).then_some(EditDrag::Scroll {
        from: geometry.scroll,
        y,
    })
}

/// The editor's drag moved to (`x`, `y`).
pub fn drag(
    model: &Model,
    height: f32,
    keyboard: bool,
    drag: EditDrag,
    x: f32,
    y: f32,
) -> Option<Action> {
    let geometry = geometry(model, height, keyboard);
    match drag {
        EditDrag::End { end, dy } => {
            let index = geometry.index_at(&model.composer.text, x, y - dy);
            Some(Action::Composer(Act::MoveEnd { end, index }))
        }
        EditDrag::Scroll { from, y: y0 } => {
            let to = (from - (y - y0)).clamp(0.0, geometry.max_scroll()).round();
            (to != geometry.scroll).then_some(Action::Composer(Act::Scroll(to)))
        }
    }
}

/// The editor's drag ended.
pub fn drag_end(drag: EditDrag) -> Option<Action> {
    match drag {
        EditDrag::End { .. } => Some(Action::Composer(Act::EndSelect)),
        EditDrag::Scroll { .. } => None,
    }
}

/// The composer: its band, the error label, the island, the editor, the
/// buttons.
pub fn draw(
    canvas: &mut Canvas,
    model: &Model,
    keyboard: bool,
    pressed: Option<&Action>,
    height: f32,
    now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let geometry = geometry(model, height, keyboard);
    let band = geometry.band_top();
    if !widgets::shows(canvas, band, height) {
        return;
    }
    canvas.fill_rect(
        0,
        band as i32,
        W as i32,
        (height - band).ceil() as i32,
        p.app_background,
        1.0,
    );
    if let Some(error) = &geometry.error {
        draw_error(canvas, error, pressed, now);
    }
    let island = geometry.island;
    // The border as a filled pill under the face (a stroke would visit
    // every pixel of the island to draw its edge).
    canvas.fill_rrect(
        island,
        ISLAND_RADIUS,
        if keyboard {
            c.island_border_focused
        } else {
            c.island_border
        },
        1.0,
    );
    canvas.fill_rrect(
        island.inset(HAIRLINE),
        ISLAND_RADIUS - HAIRLINE,
        p.surface_background,
        1.0,
    );
    if model.composer.voice.active() {
        draw_strip(canvas, model, &geometry, pressed, now);
        return;
    }
    // "+".
    let plus_down = pressed == Some(&Action::Composer(Act::Attach));
    icons::draw_styled(
        canvas,
        Icon::Plus,
        geometry.plus.0,
        geometry.plus.1,
        Style::new(60.0, c.text_output)
            .stroke(5.0)
            .alpha(if plus_down { 0.5 } else { 1.0 }),
    );
    draw_editor(canvas, model, &geometry, keyboard, now);
    draw_trailing(canvas, &geometry, pressed, now);
}

fn draw_error(canvas: &mut Canvas, error: &ErrorLabel, pressed: Option<&Action>, now: Instant) {
    let p = theme::p();
    let alpha = error_step(error.since, now) as f32 / 16.0;
    let rise = ERROR_RISE * (1.0 - alpha);
    let top = (error.rect.y + rise).round();
    let line = T_CAPTION.line();
    icons::draw(
        canvas,
        Icon::WarningTriangle,
        error.rect.x + 28.0 + 18.0,
        top + line / 2.0,
        36.0,
        p.error,
        alpha,
    );
    let text_x = error.rect.x + 28.0 + 36.0 + S1 + 6.0;
    for (index, text) in error.lines.iter().enumerate() {
        widgets::text(
            canvas,
            text,
            text_x,
            T_CAPTION.baseline(top + index as f32 * line),
            T_CAPTION,
            p.error,
            alpha,
        );
    }
    let down = pressed == Some(&Action::Composer(Act::DismissError));
    icons::draw_styled(
        canvas,
        Icon::X,
        error.close.0,
        top + line / 2.0,
        Style::new(36.0, p.error)
            .stroke(5.0)
            .alpha(alpha * if down { 0.45 } else { 0.75 }),
    );
}

fn draw_editor(
    canvas: &mut Canvas,
    model: &Model,
    geometry: &ComposerGeometry,
    keyboard: bool,
    now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let text = &model.composer.text;
    let editor = geometry.editor;
    let line = T_COMPOSER.line();
    if text.is_empty() {
        widgets::text(
            canvas,
            "Message",
            editor.x,
            T_COMPOSER.baseline(editor.y),
            T_COMPOSER,
            c.placeholder,
            1.0,
        );
    }
    let cursor = model.composer.cursor();
    let lines = &geometry.lines;
    let viewport = geometry.viewport;
    let first = ((viewport.y - editor.y + geometry.scroll) / line)
        .floor()
        .max(0.0) as usize;
    let last = (((viewport.y + viewport.h - editor.y + geometry.scroll) / line)
        .ceil()
        .max(0.0) as usize)
        .min(lines.len());
    let top_of = |row: usize| editor.y + row as f32 * line - geometry.scroll;
    widgets::clipped(canvas, viewport, |canvas| {
        if let Some((a, b)) = cursor.selection() {
            for (row, l) in lines.iter().enumerate().take(last).skip(first) {
                let breaks = text[l.end..].starts_with('\n');
                if b < l.start || a > l.end {
                    continue;
                }
                let s = a.max(l.start);
                let e = b.min(l.end);
                let x0 = editor::x_of(text, l, s, &geometry.metrics);
                let mut x1 = editor::x_of(text, l, e, &geometry.metrics);
                // A selected line break shows as a sliver past the line.
                if breaks && a <= l.end && b > l.end {
                    x1 += geometry.metrics.width(" ") / 2.0;
                }
                if x1 > x0 {
                    canvas.fill_rect(
                        (editor.x + x0).round() as i32,
                        top_of(row).round() as i32,
                        (x1 - x0).round().max(1.0) as i32,
                        line as i32,
                        c.editor_selection,
                        1.0,
                    );
                }
            }
        }
        let highlights = &model.composer.highlights;
        for (row, l) in lines.iter().enumerate().take(last).skip(first) {
            if l.end <= l.start {
                continue;
            }
            // The line in runs: plain text, and valid commands in their
            // colour (PAR-210).
            let mut at = l.start;
            while at < l.end {
                let span = highlights.iter().find(|(a, b)| *b > at && *a < l.end);
                let (until, color) = match span {
                    Some(&(a, b)) if a <= at => (b.min(l.end), p.command),
                    Some(&(a, _)) => (a, p.text),
                    None => (l.end, p.text),
                };
                // Spans end on word boundaries; never an empty run.
                let until = if until > at && text.is_char_boundary(until) {
                    until
                } else {
                    l.end
                };
                crate::text::draw(
                    canvas,
                    &text[at..until],
                    editor.x + editor::x_of(text, l, at, &geometry.metrics),
                    T_COMPOSER.baseline(top_of(row)),
                    geometry.metrics.weight,
                    geometry.metrics.size,
                    color,
                    1.0,
                );
                at = until;
            }
        }
        if !keyboard {
            return;
        }
        match cursor.selection() {
            Some((a, b)) => {
                for (index, below) in [(a, false), (b, true)] {
                    let (x, top) = geometry.locate(text, index);
                    let x = x.round();
                    canvas.fill_rect(
                        x as i32 - 2,
                        top.round() as i32,
                        4,
                        line as i32,
                        p.accent,
                        1.0,
                    );
                    let cy = if below {
                        top + line + HANDLE_R - 2.0
                    } else {
                        top - HANDLE_R + 2.0
                    };
                    canvas.fill_circle(x, cy.round(), HANDLE_R, p.accent, 1.0);
                }
            }
            None if caret_shows(model, keyboard, now) => {
                let (x, top) = geometry.locate(text, cursor.caret);
                canvas.fill_rect(
                    x.round() as i32 + 2,
                    (top + (line - CARET_H) / 2.0).round() as i32,
                    CARET_W as i32,
                    CARET_H as i32,
                    p.accent,
                    1.0,
                );
            }
            None => {}
        }
    });
}

fn draw_trailing(
    canvas: &mut Canvas,
    geometry: &ComposerGeometry,
    pressed: Option<&Action>,
    now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let face = |down: bool| if down { COMPOSER_FACE * 0.88 } else { COMPOSER_FACE } / 2.0;
    let (sx, sy) = geometry.send;
    let send_down = pressed == Some(&Action::Composer(Act::Send));
    match geometry.trailing.primary {
        Primary::None => {}
        Primary::Send { enabled } => {
            let down = enabled && send_down;
            let (fill, icon) = if enabled {
                (p.accent, p.on_accent)
            } else {
                (c.send_disabled_face, c.send_disabled_icon)
            };
            canvas.fill_circle(sx, sy, face(down), fill, 1.0);
            icons::draw_styled(
                canvas,
                Icon::ArrowUp,
                sx,
                sy,
                Style::new(if down { 50.0 } else { 56.0 }, icon).stroke(7.0),
            );
        }
        Primary::Queue { enabled } => {
            let down = enabled && send_down;
            let (fill, icon) = if enabled {
                (p.warning, 0x000000)
            } else {
                (c.send_disabled_face, c.send_disabled_icon)
            };
            canvas.fill_circle(sx, sy, face(down), fill, 1.0);
            icons::draw_styled(
                canvas,
                Icon::ListPlus,
                sx,
                sy,
                Style::new(if down { 46.0 } else { 52.0 }, icon).stroke(5.0),
            );
        }
        Primary::Run { enabled } => {
            let down = enabled && send_down;
            let (fill, ink) = if enabled {
                (p.accent, p.on_accent)
            } else {
                (c.send_disabled_face, c.send_disabled_icon)
            };
            let (w, h) = if down {
                (RUN_W * 0.88, COMPOSER_FACE * 0.88)
            } else {
                (RUN_W, COMPOSER_FACE)
            };
            canvas.fill_rrect(
                RectF::new(
                    (sx - w / 2.0).round(),
                    (sy - h / 2.0).round(),
                    w.round(),
                    h.round(),
                ),
                (h / 2.0).round(),
                fill,
                1.0,
            );
            let bold = T_NOTE.weight(crate::text::Weight::MonoBold);
            widgets::text_centered(
                canvas,
                "Run",
                sx,
                T_NOTE.baseline(sy - T_NOTE.line() / 2.0),
                bold,
                ink,
                1.0,
            );
        }
        Primary::Mic => {
            let down = pressed == Some(&Action::Composer(Act::Mic));
            icons::draw_styled(
                canvas,
                Icon::Mic,
                sx,
                sy,
                Style::new(60.0, c.text_output)
                    .stroke(5.0)
                    .alpha(if down { 0.5 } else { 1.0 }),
            );
        }
        Primary::Busy => {
            canvas.fill_circle(sx, sy, face(false), c.send_disabled_face, 1.0);
            widgets::spinner(
                canvas,
                sx,
                sy,
                SPINNER,
                p.accent,
                c.send_disabled_face,
                spinner_time(now),
            );
        }
    }
    if let (Some((x, y)), Some(stopping)) = (geometry.stop, geometry.trailing.stop) {
        let down = !stopping && pressed == Some(&Action::Composer(Act::Abort));
        canvas.fill_circle(x, y, face(down), c.stop_face, 1.0);
        if stopping {
            widgets::spinner(
                canvas,
                x,
                y,
                SPINNER,
                p.error,
                c.stop_face,
                spinner_time(now),
            );
        } else {
            let side = if down { 26.0 } else { 30.0 };
            canvas.fill_rrect(
                RectF::new(x - side / 2.0, y - side / 2.0, side, side),
                4.0,
                p.error,
                1.0,
            );
        }
    }
}

// ----- The recording strip ---------------------------------------------------------

/// The strip's waveform bars: width and gap (2 and 3 pt), height 2 pt plus
/// up to 28 pt of loudness.
const BAR_W: f32 = 6.0;
const BAR_GAP: f32 = 8.0;
const BAR_MIN: f32 = 6.0;
const BAR_SPAN: f32 = 78.0;
/// The recording dot's pulse.
const DOT_PULSE_MS: u64 = 1200;

/// Where the strip's ✓ is: the rightmost button's place.
fn strip_finish(geometry: &ComposerGeometry) -> (f32, f32) {
    (
        W - COMPOSER_MARGIN - ISLAND_PAD - COMPOSER_BUTTON / 2.0,
        geometry.plus.1,
    )
}

/// "1:05": a recording's length.
fn clock(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// What the strip's pixels depend on at `now`.
fn voice_key(model: &Model, now: Instant) -> Option<(VoicePhase, u64, u64, u32)> {
    let voice = &model.composer.voice;
    let since = voice.since?;
    match voice.phase {
        VoicePhase::Idle => None,
        VoicePhase::Recording => Some((
            voice.phase,
            voice.version,
            now.saturating_duration_since(since).as_secs(),
            anim::pulse_percent(now, DOT_PULSE_MS),
        )),
        VoicePhase::Transcribing => Some((voice.phase, 0, 0, spinner_step(now))),
    }
}

/// The recording strip in the island (PAR-243, PAR-244): ✕, the red dot
/// and the clock, the waveform of measured levels (newest on the right) and
/// ✓; while transcribing, a spinner and "Transcribing…".
fn draw_strip(
    canvas: &mut Canvas,
    model: &Model,
    geometry: &ComposerGeometry,
    pressed: Option<&Action>,
    now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let voice = &model.composer.voice;
    let (cx, cy) = geometry.plus;
    let cancel_down = pressed == Some(&Action::Composer(Act::MicCancel));
    icons::draw_styled(
        canvas,
        Icon::X,
        cx,
        cy,
        Style::new(50.0, c.text_output)
            .stroke(5.0)
            .alpha(if cancel_down { 0.5 } else { 1.0 }),
    );
    let x = cx + COMPOSER_BUTTON / 2.0 + S1;
    let baseline = T_NOTE.baseline(cy - T_NOTE.line() / 2.0);
    match voice.phase {
        VoicePhase::Idle => {}
        VoicePhase::Transcribing => {
            widgets::spinner(
                canvas,
                x + SPINNER / 2.0,
                cy,
                SPINNER,
                p.accent,
                p.surface_background,
                spinner_time(now),
            );
            widgets::text(
                canvas,
                "Transcribing…",
                x + SPINNER + S2,
                baseline,
                T_NOTE,
                p.muted,
                1.0,
            );
        }
        VoicePhase::Recording => {
            let elapsed = voice
                .since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
            canvas.fill_circle(
                x + 9.0,
                cy,
                9.0,
                p.error,
                anim::pulse_percent(now, DOT_PULSE_MS) as f32 / 100.0,
            );
            let clock_x = x + 18.0 + S1;
            let w = widgets::text(
                canvas,
                &clock(elapsed),
                clock_x,
                baseline,
                T_NOTE,
                p.text,
                1.0,
            );
            let finish = strip_finish(geometry);
            let left = clock_x + w.max(widgets::measure("0:00", T_NOTE)) + S2;
            let right = finish.0 - COMPOSER_BUTTON / 2.0 - S1;
            let on = theme::mix(p.accent, 85, p.surface_background);
            let off = theme::mix(p.accent, 20, p.surface_background);
            let mut bar_x = right - BAR_W;
            for level in voice.levels.iter().rev() {
                if bar_x < left {
                    break;
                }
                let h = (BAR_MIN + level * BAR_SPAN).round();
                canvas.fill_rect(
                    bar_x.round() as i32,
                    (cy - h / 2.0).round() as i32,
                    BAR_W as i32,
                    h as i32,
                    if *level > 0.01 { on } else { off },
                    1.0,
                );
                bar_x -= BAR_W + BAR_GAP;
            }
            let down = pressed == Some(&Action::Composer(Act::MicDone));
            let face = if down {
                COMPOSER_FACE * 0.88
            } else {
                COMPOSER_FACE
            } / 2.0;
            canvas.fill_circle(finish.0, finish.1, face, p.accent, 1.0);
            icons::draw_styled(
                canvas,
                Icon::Check,
                finish.0,
                finish.1,
                Style::new(if down { 46.0 } else { 52.0 }, p.on_accent).stroke(7.0),
            );
        }
    }
}

// ----- The edit menu ---------------------------------------------------------------

const MENU_IN: Duration = Duration::from_millis(120);
const MENU_OUT: Duration = Duration::from_millis(100);
const MENU_H: f32 = 110.0;
const MENU_PAD: f32 = 34.0;
const MENU_SHADOW: f32 = 44.0;
const MENU_SHADOW_DY: f32 = 11.0;

/// The edit menu over the editor's caret or selection: a row of Cut,
/// Copy, Paste, Select, Select all. Floating: touches elsewhere go on to
/// the page; it closes when the composer's menu changes (an edit, the
/// caret moved, a choice made) or the keyboard goes.
pub struct EditMenu {
    serial: u64,
    items: Vec<(&'static str, Act)>,
    rect: RectF,
    /// Each item's right edge.
    edges: Vec<f32>,
    opened: Option<Instant>,
    closed: Option<Instant>,
    pressed: Option<usize>,
    keyboard_seen: bool,
}

impl EditMenu {
    pub fn new(serial: u64) -> Self {
        Self {
            serial,
            items: Vec::new(),
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
            edges: Vec::new(),
            opened: None,
            closed: None,
            pressed: None,
            keyboard_seen: false,
        }
    }

    fn shown(&self, now: Instant) -> f32 {
        let value = match (self.opened, self.closed) {
            (_, Some(closed)) => 1.0 - Tween::new(closed, MENU_OUT, 0.0, 1.0).value(now),
            (Some(opened), None) => Tween::new(opened, MENU_IN, 0.0, 1.0)
                .with_curve(anim::ease_out)
                .value(now),
            (None, None) => 0.0,
        };
        (value * 32.0).round() / 32.0
    }

    fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.rect.contains(x, y) {
            return None;
        }
        self.edges.iter().position(|edge| x < *edge)
    }
}

impl Overlay for EditMenu {
    fn label(&self) -> String {
        let labels: Vec<&str> = self.items.iter().map(|(label, _)| *label).collect();
        format!("edit menu: {}", labels.join(", "))
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
        if cx.keyboard {
            self.keyboard_seen = true;
        }
        let gone = model.composer.menu != Some(self.serial)
            || model.page != Page::Chat
            || (self.keyboard_seen && !cx.keyboard);
        if gone && self.closed.is_none() {
            self.closed = Some(cx.now);
        }
        if self.closed.is_some() && !self.items.is_empty() {
            return;
        }
        let text = &model.composer.text;
        let cursor = model.composer.cursor();
        let selection = cursor.selection();
        let mut items: Vec<(&'static str, Act)> = Vec::new();
        if selection.is_some() {
            items.push(("Cut", Act::Cut));
            items.push(("Copy", Act::Copy));
        }
        items.push(("Paste", Act::Paste));
        if selection.is_none() && !text.trim().is_empty() {
            items.push(("Select", Act::Select));
        }
        if !text.is_empty() && selection != Some((0, text.len())) {
            items.push(("Select all", Act::SelectAll));
        }
        let widths: Vec<f32> = items
            .iter()
            .map(|(label, _)| (widgets::measure(label, T_MENU) + 2.0 * MENU_PAD).round())
            .collect();
        let w: f32 = widths.iter().sum();
        let geometry = geometry(model, cx.height, cx.keyboard);
        let at = selection.map_or(cursor.caret, |(a, _)| a);
        let (x, top) = geometry.locate(text, at);
        let line_top = top.clamp(
            geometry.viewport.y,
            geometry.viewport.y + geometry.viewport.h,
        );
        let y = (line_top - S2 - HANDLE_R * 2.0 - MENU_H).max(CHAT_TOP + S1);
        let left = (x - w / 2.0).clamp(S2, W - S2 - w).round();
        self.rect = RectF::new(left, y.round(), w, MENU_H);
        let mut edge = left;
        self.edges = widths
            .iter()
            .map(|width| {
                edge += width;
                edge
            })
            .collect();
        self.items = items;
    }

    fn body(&self, cx: &Cx) -> Body {
        let mut key = DefaultHasher::new();
        (
            self.serial,
            self.items
                .iter()
                .map(|(label, _)| *label)
                .collect::<Vec<_>>(),
            self.rect.x.to_bits(),
            self.rect.y.to_bits(),
            self.rect.w.to_bits(),
            (self.shown(cx.now) * 32.0) as u32,
            self.pressed,
            theme::generation(),
        )
            .hash(&mut key);
        Body {
            rows: (
                (self.rect.y - MENU_SHADOW).floor() as i32,
                (self.rect.y + self.rect.h + MENU_SHADOW + MENU_SHADOW_DY).ceil() as i32 + 1,
            ),
            solid: None,
            scroller: None,
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let alpha = self.shown(cx.now);
        if alpha <= 0.0 || self.items.is_empty() {
            return;
        }
        let p = theme::p();
        let c = theme::c();
        let r = self.rect;
        widgets::shadow(canvas, r, MENU_SHADOW, MENU_SHADOW_DY, 0.4 * alpha);
        canvas.fill_rect(
            r.x as i32,
            r.y as i32,
            r.w as i32,
            r.h as i32,
            p.surface_background,
            alpha,
        );
        let mut left = r.x;
        for (index, ((label, _), right)) in self.items.iter().zip(&self.edges).enumerate() {
            if self.pressed == Some(index) {
                canvas.fill_rect(
                    left as i32,
                    r.y as i32,
                    (right - left) as i32,
                    r.h as i32,
                    c.menu_item_pressed,
                    alpha,
                );
            }
            if index > 0 {
                canvas.fill_rect(
                    left as i32,
                    (r.y + 22.0) as i32,
                    HAIRLINE as i32,
                    (r.h - 44.0) as i32,
                    c.menu_border,
                    alpha,
                );
            }
            widgets::text_centered(
                canvas,
                label,
                (left + right) / 2.0,
                T_MENU.baseline(r.y + (r.h - T_MENU.line()) / 2.0),
                T_MENU,
                p.text,
                alpha,
            );
            left = *right;
        }
        for edge in [
            RectF::new(r.x, r.y, r.w, HAIRLINE),
            RectF::new(r.x, r.y + r.h - HAIRLINE, r.w, HAIRLINE),
            RectF::new(r.x, r.y, HAIRLINE, r.h),
            RectF::new(r.x + r.w - HAIRLINE, r.y, HAIRLINE, r.h),
        ] {
            canvas.fill_rect(
                edge.x as i32,
                edge.y as i32,
                edge.w as i32,
                edge.h as i32,
                c.menu_border,
                alpha,
            );
        }
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } => match self.item_at(x, y) {
                Some(index) => {
                    self.pressed = Some(index);
                    Response::Handled
                }
                None => Response::Pass,
            },
            Gesture::Tap { x, y } => {
                self.pressed = None;
                match self.item_at(x, y) {
                    Some(index) => {
                        let act = self.items[index].1.clone();
                        self.close(cx.now);
                        Response::Action(Action::Composer(act))
                    }
                    None => Response::Handled,
                }
            }
            _ => {
                self.pressed = None;
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
        self.closed.is_some_and(|closed| now >= closed + MENU_OUT)
    }

    fn animate(&self, cx: &Cx, animator: &mut Animator) {
        match (self.opened, self.closed) {
            (_, Some(closed)) => animator.follow(&Tween::new(closed, MENU_OUT, 0.0, 1.0), cx.now),
            (Some(opened), None) => animator.follow(&Tween::new(opened, MENU_IN, 0.0, 1.0), cx.now),
            (None, None) => animator.frame(cx.now),
        }
    }
}

#[cfg(test)]
mod tests_support {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;
    use crate::keys::KeyInput;

    /// Ready, conversation `c` open with an exchange in it.
    pub fn chat(streaming: bool) -> Model {
        let mut model = Model::new();
        let bootstrap = model.on_connected();
        let ping = bootstrap.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        let now = Instant::now();
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            now,
        );
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_updated","summary":{{"id":"c","provider":"anthropic","model":"m","effort":"low","title":"c","pinned":false,"sortOrder":0,"streaming":{streaming},"unread":false}}}}"#
            )),
            now,
        );
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let request = commands[0]["reqId"].as_str().unwrap().to_owned();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{{"type":"user","text":"Hello"}},{{"type":"ai","blocks":[{{"type":"text","text":"Hi there."}}]}}],"historyStartIndex":0}}"#
            )),
            now,
        );
        model
    }

    pub fn type_text(model: &mut Model, text: &str) {
        for character in text.chars() {
            model.composer_key(if character == '\n' {
                KeyInput::Enter
            } else {
                KeyInput::Character(character)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::exocortex::view::{Frame, FrameDamage, ViewState, frame};
    use crate::keys::KeyInput;

    const SIZE: (i32, i32) = (1080, 2216);

    fn full(model: &mut Model, view: &mut ViewState, now: Instant) -> Canvas {
        let mut canvas = Canvas::new(SIZE.0, SIZE.1);
        frame::clear(&mut canvas);
        frame::draw_full(&mut canvas, model, view, None, now);
        canvas
    }

    /// The next frame drawn from `old` (incrementally when the pipeline
    /// can): it equals a full redraw.
    fn step(
        canvas: &mut Canvas,
        model: &mut Model,
        view: &mut ViewState,
        old: &Frame,
        now: Instant,
    ) -> (Frame, Option<FrameDamage>) {
        view.take_ui(model, now);
        let new = frame::record(model, view, None, SIZE, now);
        let damage = frame::update(canvas, model, view, None, old, &new, now);
        let whole = full(model, view, now);
        if damage.is_none() {
            *canvas = whole;
            return (new, None);
        }
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
        (new, damage)
    }

    #[test]
    fn sixteen_numbered_lines_keep_the_caret_in_view() {
        let mut model = chat(false);
        let lines: Vec<String> = (1..=16).map(|i| format!("{i}. item number {i}")).collect();
        type_text(&mut model, &lines.join("\n"));
        let height = SIZE.1 as f32 - 900.0;
        for _ in 0..2 {
            let g = geometry(&model, height, true);
            assert_eq!(g.lines.len(), 16);
            assert_eq!(
                g.editor.h,
                EDITOR_LINES as f32 * T_COMPOSER.line(),
                "six lines shown"
            );
            let (_, top) = g.locate(&model.composer.text, model.composer.cursor().caret);
            assert!(
                top >= g.editor.y && top + T_COMPOSER.line() <= g.editor.y + g.editor.h,
                "the caret's line shows"
            );
        }
        // Scrolled up by hand, it stays there until the caret moves.
        model.handle(Action::Composer(Act::Scroll(0.0)));
        assert_eq!(geometry(&model, height, true).scroll, 0.0);
        assert_eq!(geometry(&model, height, true).scroll, 0.0);
        type_text(&mut model, "!");
        let g = geometry(&model, height, true);
        assert_eq!(g.scroll, g.max_scroll());
        // A drag in the text scrolls it.
        let drag = drag_start(&model, height, false, 500.0, g.editor.y + 100.0).unwrap();
        assert!(matches!(drag, EditDrag::Scroll { .. }));
        let action = super::drag(&model, height, false, drag, 500.0, g.editor.y + 300.0).unwrap();
        assert_eq!(
            action,
            Action::Composer(Act::Scroll(g.max_scroll() - 200.0))
        );
    }

    #[test]
    fn a_tap_places_the_caret_at_the_glyph_under_it() {
        let mut model = chat(false);
        type_text(
            &mut model,
            "a😀b é\nwrapping words go on and on and on and on until the line ends",
        );
        let height = SIZE.1 as f32;
        let g = geometry(&model, height, true);
        let text = model.composer.text.clone();
        assert!(g.lines.len() >= 3, "it wraps");
        let mut index = 0;
        loop {
            let (x, top) = g.locate(&text, index);
            let hit = hit(&model, height, true, x + 2.0, top + 20.0);
            let row = editor::line_of(&g.lines, index);
            // The end of a soft-wrapped line is the next line's start.
            let soft = !text[index..].starts_with('\n');
            let expected = if soft && index == g.lines[row].end && row + 1 < g.lines.len() {
                g.lines[row + 1].start
            } else {
                index
            };
            assert_eq!(
                hit,
                Some(Hit::Action(Action::Composer(Act::Place {
                    index: expected,
                    focused: true
                }))),
                "at {index}"
            );
            if index == text.len() {
                break;
            }
            index = editor::cluster_end(&text, index);
        }
        // A long press selects the word under the finger.
        let (x, top) = g.locate(&text, text.find("words").unwrap() + 2);
        let action = long_press(&model, height, true, x, top + 20.0).unwrap();
        model.handle(action);
        assert_eq!(
            model.composer.cursor().selected(&model.composer.text),
            "words"
        );
        // Its handles are grabbed by a drag near them.
        let cursor = model.composer.cursor();
        let (hx, htop) = g.locate(&text, cursor.caret.max(cursor.anchor));
        let drag = drag_start(&model, height, true, hx + 10.0, htop + 40.0);
        assert!(matches!(drag, Some(EditDrag::End { .. })));
    }

    #[test]
    fn the_send_queue_and_stop_buttons_follow_the_turn() {
        let mut model = chat(true);
        assert_eq!(
            trailing(&model),
            Trailing {
                primary: Primary::Mic,
                stop: Some(false)
            }
        );
        type_text(&mut model, "x");
        assert_eq!(trailing(&model).primary, Primary::Queue { enabled: true });
        let g = geometry(&model, 2216.0, false);
        assert_eq!(
            hit(&model, 2216.0, false, g.send.0, g.send.1),
            Some(Hit::Action(Action::Composer(Act::Send)))
        );
        let stop = g.stop.unwrap();
        assert_eq!(
            hit(&model, 2216.0, false, stop.0, stop.1),
            Some(Hit::Action(Action::Composer(Act::Abort)))
        );
        model.handle(Action::Composer(Act::Abort));
        assert_eq!(trailing(&model).stop, Some(true));
        model.ready = false;
        assert_eq!(trailing(&model).primary, Primary::Queue { enabled: false });
    }

    #[test]
    fn editing_frames_equal_full_redraws() {
        let mut model = chat(true);
        let mut view = ViewState::new();
        view.keyboard = true;
        let t0 = Instant::now();
        view.take_ui(&mut model, t0);
        let mut old = frame::record(&mut model, &mut view, None, SIZE, t0);
        let mut canvas = full(&mut model, &mut view, t0);
        let mut at = t0;
        let mut frames = |model: &mut Model,
                          view: &mut ViewState,
                          change: &dyn Fn(&mut Model),
                          ms: u64|
         -> Option<FrameDamage> {
            change(model);
            at += Duration::from_millis(ms);
            let (new, damage) = step(&mut canvas, model, view, &old, at);
            old = new;
            damage
        };
        // Typing redraws the composer's rows only.
        for character in "Hello there".chars() {
            let damage = frames(
                &mut model,
                &mut view,
                &|model| {
                    model.composer_key(KeyInput::Character(character));
                },
                30,
            )
            .expect("typing is incremental");
            // (The header's working dot pulses meanwhile.)
            let rows: i32 = damage
                .rows
                .iter()
                .filter(|rows| **rows != widgets::TOP_BAR_ROWS)
                .map(|(a, b)| b - a)
                .sum();
            assert!(rows < 300, "only the island: {damage:?}");
        }
        // The picker opens over the transcript as a slash word is typed,
        // follows the typing, and goes when a suggestion is taken.
        for character in " /co".chars() {
            frames(
                &mut model,
                &mut view,
                &|model| {
                    model.composer_key(KeyInput::Character(character));
                },
                20,
            );
        }
        assert!(model.composer.picker_state.is_some());
        for _ in 0..12 {
            frames(&mut model, &mut view, &|_| {}, 16);
        }
        frames(
            &mut model,
            &mut view,
            &|model| {
                model.handle(Action::Composer(Act::Complete("/consider".into())));
            },
            10,
        );
        for _ in 0..12 {
            frames(&mut model, &mut view, &|_| {}, 16);
        }
        // The caret blinks; it moves; a word is selected, with the menu
        // fading in over its own rows.
        frames(&mut model, &mut view, &|_| {}, 600);
        frames(&mut model, &mut view, &|_| {}, 530);
        frames(
            &mut model,
            &mut view,
            &|model| {
                model.handle(Action::Composer(Act::Place {
                    index: 2,
                    focused: true,
                }));
            },
            10,
        );
        frames(
            &mut model,
            &mut view,
            &|model| {
                model.handle(Action::Composer(Act::SelectWord(1)));
                model.handle(Action::Composer(Act::EndSelect));
            },
            10,
        );
        for _ in 0..20 {
            frames(&mut model, &mut view, &|_| {}, 8);
        }
        assert!(!view.overlays.is_empty(), "the edit menu");
        // Stop pressed: its spinner turns.
        frames(
            &mut model,
            &mut view,
            &|model| {
                model.handle(Action::Composer(Act::Abort));
            },
            10,
        );
        for _ in 0..5 {
            frames(&mut model, &mut view, &|_| {}, 33);
        }
        // Dictation: the strip takes the row; levels, the clock and the
        // dot redraw it; transcribing turns a spinner.
        frames(
            &mut model,
            &mut view,
            &|model| {
                model.handle(Action::Composer(Act::Mic));
            },
            10,
        );
        for step in 0..12 {
            frames(
                &mut model,
                &mut view,
                &|model| {
                    model.dictation_line(dictation::Line::Level((step % 5) as f32 / 4.0));
                },
                50,
            );
        }
        frames(
            &mut model,
            &mut view,
            &|model| {
                model.handle(Action::Composer(Act::MicDone));
            },
            10,
        );
        for _ in 0..4 {
            frames(&mut model, &mut view, &|_| {}, 33);
        }
        frames(
            &mut model,
            &mut view,
            &|model| model.dictation_line(dictation::Line::Text("dictated".into())),
            10,
        );
        // An error label rises over the island (the transcript's bottom
        // moves: drawn whole), then fades in over its own rows.
        frames(
            &mut model,
            &mut view,
            &|model| {
                let key = model.composer.key.clone().unwrap();
                model.composer.errors.insert(
                    key,
                    composer::ComposerError {
                        text: "Not sent: something went wrong".into(),
                        since: Instant::now(),
                    },
                );
            },
            0,
        );
        for _ in 0..30 {
            frames(&mut model, &mut view, &|_| {}, 8);
        }
    }
}

/// Renders the composer's states to PNGs for looking at them next to the
/// iPhone's (`cargo test -- --ignored composer_states_to_png`, written to
/// `/tmp/wpd-render`).
#[cfg(test)]
mod render {
    use super::tests_support::*;
    use super::*;
    use crate::exocortex::view::{ViewState, frame};

    fn save(canvas: &Canvas, name: &str) {
        let dir = std::path::Path::new("/tmp/wpd-render");
        std::fs::create_dir_all(dir).unwrap();
        let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
        let mut encoder = png::Encoder::new(file, canvas.width as u32, canvas.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let bytes: Vec<u8> = canvas
            .pixels
            .iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
            .collect();
        writer.write_image_data(&bytes).unwrap();
    }

    fn shot(model: &mut Model, keyboard: bool, name: &str, now: Instant) {
        let mut view = ViewState::new();
        view.keyboard = keyboard;
        view.take_ui(model, now);
        let height = if keyboard { 1300 } else { 2216 };
        let _ = frame::record(model, &mut view, None, (1080, height), now);
        let mut canvas = Canvas::new(1080, height);
        frame::clear(&mut canvas);
        frame::draw_full(&mut canvas, model, &mut view, None, now);
        save(&canvas, name);
    }

    #[test]
    #[ignore]
    fn composer_states_to_png() {
        let t0 = Instant::now();
        let mut model = chat(false);
        type_text(
            &mut model,
            "Can you also check the logs from yesterday and summarise them?",
        );
        shot(&mut model, true, "01-typing", t0);
        let key = model.composer.key.clone().unwrap();
        model.composer.errors.insert(
            key,
            super::super::super::model::composer::ComposerError {
                text: "Not sent: Already streaming".into(),
                since: t0,
            },
        );
        shot(&mut model, false, "02-error", t0 + Duration::from_secs(1));
        let mut model = chat(true);
        shot(&mut model, false, "03-streaming-empty", t0);
        type_text(&mut model, "also bump the version when you're done");
        shot(&mut model, false, "04-streaming-text", t0);
        model.handle(Action::Composer(Act::Abort));
        shot(&mut model, false, "05-stopping", t0);
        let mut model = chat(false);
        type_text(&mut model, "select some words here\nand a second line");
        model.handle(Action::Composer(Act::SelectWord(9)));
        model.handle(Action::Composer(Act::MoveEnd {
            end: End::Caret,
            index: 30,
        }));
        model.handle(Action::Composer(Act::EndSelect));
        shot(
            &mut model,
            true,
            "06-selection-menu",
            t0 + Duration::from_secs(1),
        );
        model.ready = false;
        model.composer.cursor = editor::Cursor::at(5);
        shot(&mut model, true, "07-not-ready", t0);
    }
}
