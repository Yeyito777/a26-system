//! The composer island (WP-D; roadmap §3.6.15): a rounded surface above
//! Moon's keyboard or home gesture, "+" on the left, the editor, and on the
//! right Send (accent), Queue (warning, while a turn runs), Stop (while it
//! runs and nothing is typed) or nothing. WP-A's shell: geometry, colours
//! and today's behaviour (typing appends, Return sends); D brings the real
//! editor (caret, selection, newlines), Run and the mic.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::gfx::{Canvas, RectF};

use super::super::anim::Animator;
use super::super::model::{Action, Model, composer};
use super::super::theme::{
    self, COMPOSER_BUTTON, COMPOSER_FACE, COMPOSER_GAP, COMPOSER_MARGIN, EDITOR_INSET,
    EDITOR_LINES, EDITOR_PAD_Y, GESTURE_RESERVE, HAIRLINE, ISLAND_MIN, ISLAND_PAD, ISLAND_RADIUS,
    T_COMPOSER, W,
};
use super::super::ui::icons::{self, Icon, Style};
use super::Hit;
use super::markdown_layout::{Run, line_width, plain, wrap};

/// The band above the island filled with the background.
const BAND: f32 = 17.0;

/// Where the island, its editor and buttons are.
pub struct ComposerGeometry {
    pub island: RectF,
    /// The editor's text area.
    pub editor: RectF,
    pub lines: Vec<Vec<Run>>,
    /// The first line shown (the editor shows the last few).
    pub first: usize,
    /// The "+" button's centre.
    pub plus: (f32, f32),
    /// The trailing button's centre.
    pub send: (f32, f32),
}

impl ComposerGeometry {
    /// Where the composer's band (and so the transcript's bottom) begins.
    pub fn band_top(&self) -> f32 {
        self.island.y - BAND
    }
}

/// The trailing button the composer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trailing {
    None,
    Send { enabled: bool },
    Queue,
    Stop,
}

pub fn trailing(model: &Model) -> Trailing {
    let typed = !model.composer.text.trim().is_empty();
    match (typed, model.streaming()) {
        (true, true) => Trailing::Queue,
        (true, false) => Trailing::Send {
            enabled: model.ready,
        },
        (false, true) => Trailing::Stop,
        (false, false) => Trailing::None,
    }
}

pub fn geometry(model: &Model, height: f32, keyboard: bool) -> ComposerGeometry {
    let bottom = height - COMPOSER_GAP - if keyboard { 0.0 } else { GESTURE_RESERVE };
    let x0 = COMPOSER_MARGIN;
    let x1 = W - COMPOSER_MARGIN;
    let editor_x = x0 + ISLAND_PAD + COMPOSER_BUTTON + EDITOR_INSET - 12.0;
    let editor_right = x1 - ISLAND_PAD - COMPOSER_BUTTON;
    let lines = composer_lines(&model.composer.text, editor_right - editor_x);
    let shown = lines.len().clamp(1, EDITOR_LINES);
    let line = T_COMPOSER.line();
    let h = (shown as f32 * line + 2.0 * EDITOR_PAD_Y + 2.0 * ISLAND_PAD).max(ISLAND_MIN);
    let island = RectF::new(x0, (bottom - h).round(), x1 - x0, h.round());
    let row_cy = island.y + island.h - ISLAND_PAD - COMPOSER_BUTTON / 2.0;
    let editor_h = shown as f32 * line;
    ComposerGeometry {
        editor: RectF::new(
            editor_x,
            island.y + (island.h - editor_h) / 2.0,
            editor_right - editor_x,
            editor_h,
        ),
        island,
        first: lines.len().saturating_sub(EDITOR_LINES),
        lines,
        plus: (x0 + ISLAND_PAD + COMPOSER_BUTTON / 2.0, row_cy),
        send: (x1 - ISLAND_PAD - COMPOSER_BUTTON / 2.0, row_cy),
    }
}

/// The composer's text wrapped, kept until it changes: the island's height
/// (and so the transcript's bounds) is asked for many times a frame.
pub(crate) fn composer_lines(text: &str, width: f32) -> Vec<Vec<Run>> {
    use std::cell::RefCell;
    thread_local! {
        static LAST: RefCell<(String, f32, u64, Vec<Vec<Run>>)> =
            const { RefCell::new((String::new(), 0.0, 0, Vec::new())) };
    }
    let generation = theme::generation();
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        if last.0 != text || last.1 != width || last.2 != generation || last.3.is_empty() {
            *last = (
                text.to_owned(),
                width,
                generation,
                wrap(&plain(text), width, T_COMPOSER.px()),
            );
        }
        last.3.clone()
    })
}

/// Whether `action` is one of the composer's buttons.
pub fn owns(action: &Action) -> bool {
    matches!(
        action,
        Action::Composer(composer::Act::Send | composer::Act::Abort | composer::Act::Attach)
    )
}

/// What the composer's pixels depend on.
pub fn frame_key(model: &Model, keyboard: bool, pressed: Option<&Action>, _now: Instant) -> u64 {
    let mut hasher = DefaultHasher::new();
    (
        &model.composer.text,
        keyboard,
        trailing(model),
        super::action_key(pressed.filter(|action| owns(action))),
        theme::generation(),
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// The caret blinks (D); nothing animates in the shell.
pub fn animate(_model: &Model, _keyboard: bool, _now: Instant, _animator: &mut Animator) {}

/// The composer's target at (`x`, `y`).
pub fn hit(model: &Model, height: f32, keyboard: bool, x: f32, y: f32) -> Option<Hit> {
    let geometry = geometry(model, height, keyboard);
    let near = |(cx, cy): (f32, f32)| {
        (x - cx).abs() < COMPOSER_BUTTON / 2.0 && (y - cy).abs() < COMPOSER_BUTTON / 2.0
    };
    if near(geometry.plus) {
        return Some(Hit::Action(Action::Composer(composer::Act::Attach)));
    }
    if near(geometry.send) {
        return match trailing(model) {
            Trailing::Stop => Some(Hit::Action(Action::Composer(composer::Act::Abort))),
            Trailing::Send { .. } | Trailing::Queue => {
                Some(Hit::Action(Action::Composer(composer::Act::Send)))
            }
            Trailing::None => Some(Hit::Composer),
        };
    }
    if y >= geometry.band_top() && geometry.island.inset(-16.0).contains(x, y) {
        return Some(Hit::Composer);
    }
    None
}

/// The composer: its band, the island, the editor's text, the buttons.
pub fn draw(
    canvas: &mut Canvas,
    model: &Model,
    keyboard: bool,
    pressed: Option<&Action>,
    height: f32,
    _now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let geometry = geometry(model, height, keyboard);
    let band = geometry.band_top();
    if !super::super::ui::widgets::shows(canvas, band, height) {
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
    // "+".
    let plus_down = pressed == Some(&Action::Composer(composer::Act::Attach));
    icons::draw_styled(
        canvas,
        Icon::Plus,
        geometry.plus.0,
        geometry.plus.1,
        Style::new(60.0, c.text_output)
            .stroke(5.0)
            .alpha(if plus_down { 0.5 } else { 1.0 }),
    );
    // The editor.
    let line = T_COMPOSER.line();
    let editor = geometry.editor;
    let baseline = |index: usize| T_COMPOSER.baseline(editor.y + index as f32 * line);
    if model.composer.text.is_empty() {
        super::super::ui::widgets::text(
            canvas,
            "Message",
            editor.x,
            baseline(0),
            T_COMPOSER,
            c.placeholder,
            1.0,
        );
    }
    let mut caret = (editor.x, editor.y);
    for (index, row) in geometry.lines.iter().skip(geometry.first).enumerate() {
        for run in row {
            crate::text::draw(
                canvas,
                &run.text,
                editor.x + run.x,
                baseline(index),
                T_COMPOSER.weight,
                T_COMPOSER.px(),
                p.text,
                1.0,
            );
        }
        caret = (editor.x + line_width(row), editor.y + index as f32 * line);
    }
    if keyboard {
        canvas.fill_rect(
            caret.0.round() as i32 + 2,
            (caret.1 + (line - 52.0) / 2.0).round() as i32,
            5,
            52,
            p.accent,
            1.0,
        );
    }
    // The trailing button.
    let (sx, sy) = geometry.send;
    let face = |down: bool| if down { COMPOSER_FACE * 0.88 } else { COMPOSER_FACE } / 2.0;
    match trailing(model) {
        Trailing::None => {}
        Trailing::Send { enabled } => {
            let down = enabled && pressed == Some(&Action::Composer(composer::Act::Send));
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
        Trailing::Queue => {
            let down = pressed == Some(&Action::Composer(composer::Act::Send));
            canvas.fill_circle(sx, sy, face(down), p.warning, 1.0);
            icons::draw_styled(
                canvas,
                Icon::ListPlus,
                sx,
                sy,
                Style::new(if down { 46.0 } else { 52.0 }, 0x000000).stroke(5.0),
            );
        }
        Trailing::Stop => {
            let down = pressed == Some(&Action::Composer(composer::Act::Abort));
            canvas.fill_circle(sx, sy, face(down), c.stop_face, 1.0);
            let side = if down { 26.0 } else { 30.0 };
            canvas.fill_rrect(
                RectF::new(sx - side / 2.0, sy - side / 2.0, side, side),
                4.0,
                p.error,
                1.0,
            );
        }
    }
}
