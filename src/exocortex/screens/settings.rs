//! The Settings page (WP-E; roadmap §3.6.19): left of the list, with its
//! own top bar ("SETTINGS", a chevron back to the list) and the page rail.
//! WP-A's version is the empty page; E brings Usage limits, Appearance,
//! Text & transcript, New chats, Notifications, Connection, Local storage
//! and About, writing the settings through `Persist::update_settings`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::gfx::{Canvas, RectF};

use super::super::model::{Action, Model, Nav};
use super::super::theme::{self, HAIRLINE, T_PAGE_TITLE, TOP_BAR, W};
use super::super::ui::icons::Icon;
use super::super::ui::widgets::{self, RAIL_SETTINGS};
use super::super::view::ViewState;

const CLOSE_X: f32 = 1008.0;
const ROW_Y: f32 = 61.0;

/// What the page's pixels depend on.
pub fn frame_key(model: &Model, view: &ViewState, pressed: Option<&Action>, now: Instant) -> u64 {
    let mut hasher = DefaultHasher::new();
    (
        super::super::view::action_key(pressed),
        view.settings_scroll.value(now).round() as i32,
        model.connected,
        theme::generation(),
    )
        .hash(&mut hasher);
    hasher.finish()
}

pub fn draw(
    canvas: &mut Canvas,
    _model: &mut Model,
    _view: &mut ViewState,
    pressed: Option<&Action>,
    _now: Instant,
) {
    let p = theme::p();
    let height = canvas.height as f32;
    widgets::fill(
        canvas,
        RectF::new(0.0, 0.0, W, height),
        p.sidebar_background,
    );
    widgets::empty_state(
        canvas,
        RectF::new(
            0.0,
            TOP_BAR + HAIRLINE,
            W,
            height - TOP_BAR - theme::GESTURE_RESERVE,
        ),
        "Settings",
        "Usage limits, appearance and the transcript's options come here.",
        None,
        p.sidebar_background,
    );
    widgets::top_bar(canvas, p.sidebar_background);
    widgets::page_rail(canvas, RAIL_SETTINGS);
    widgets::text(
        canvas,
        "Settings",
        theme::S3,
        T_PAGE_TITLE.baseline(ROW_Y - T_PAGE_TITLE.line() / 2.0),
        T_PAGE_TITLE,
        p.text,
        1.0,
    );
    let down = pressed == Some(&Action::Nav(Nav::CloseSettings));
    widgets::icon_button(
        canvas,
        Icon::ChevronRight,
        CLOSE_X,
        ROW_Y,
        60.0,
        p.text,
        down,
    );
}

/// The page's target at (`x`, `y`).
pub fn hit(_model: &Model, _view: &ViewState, x: f32, y: f32, _now: Instant) -> Option<Action> {
    (y < TOP_BAR && x > CLOSE_X - 90.0).then_some(Action::Nav(Nav::CloseSettings))
}
