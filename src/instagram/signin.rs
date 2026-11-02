//! Signed out (or a session Instagram refused): what to do, and the button
//! that imports the session from the Browser (the bridge's `login.start`).

use serde_json::json;

use super::app::{Action, Area};
use super::paint::{self, BLUE, BODY, BODY_BOLD, DP, Icon, MARGIN, Paint, SECONDARY, TEXT};
use super::store::{Ask, Store};
use crate::gfx::{Canvas, RectF};

/// Whether the panel shows instead of the content.
pub fn wanted(store: &Store) -> bool {
    !store.fixtures && (store.status.signed_out() || store.status.state == "error")
}

fn button(area: Area) -> RectF {
    let w = area.width - 4.0 * MARGIN;
    RectF::new(2.0 * MARGIN, area.top + 820.0, w, 44.0 * DP)
}

pub fn draw(canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
    let cx = area.width / 2.0;
    let top = area.top + 180.0;
    canvas.stroke_rrect(
        RectF::new(cx - 110.0, top, 220.0, 220.0),
        64.0,
        10.0,
        TEXT,
        1.0,
    );
    canvas.stroke_circle(cx, top + 110.0, 52.0, 10.0, TEXT, 1.0);
    canvas.fill_circle(cx + 62.0, top + 50.0, 13.0, TEXT, 1.0);
    let title = if store.status.signed_out() {
        "Sign in to Instagram"
    } else {
        "Instagram needs you"
    };
    paint::draw_text_centered(canvas, title, cx, top + 340.0, paint::TITLE, TEXT);
    let detail = match store.status.detail.as_deref() {
        Some(detail) if !store.status.signed_out() => detail.to_owned(),
        _ => "Sign in to instagram.com in the Browser first, then import that session here. It stays on this phone.".to_owned(),
    };
    for (n, line) in paint::wrap_plain(&detail, BODY, SECONDARY, area.width - 4.0 * MARGIN, 5)
        .0
        .iter()
        .enumerate()
    {
        let w = paint::line_width(line);
        paint::draw_line(canvas, line, cx - w / 2.0, top + 420.0 + n as f32 * 52.0);
    }
    let b = button(area);
    canvas.fill_rrect(b, 8.0 * DP, BLUE, 1.0);
    let label = if store.status.signed_out() {
        "Import from Browser"
    } else {
        "Import again"
    };
    paint::draw_text_centered(
        canvas,
        label,
        cx,
        paint::baseline(b.y + b.h / 2.0, BODY_BOLD),
        BODY_BOLD,
        0xffffff,
    );
    if let Some(login) = &store.login {
        paint::draw_text_centered(
            canvas,
            &paint::fit(login, BODY, area.width - 4.0 * MARGIN),
            cx,
            b.y + b.h + 80.0,
            BODY,
            SECONDARY,
        );
    }
    let _ = (p, Icon::Camera);
}

pub fn tap(area: Area, x: f32, y: f32) -> Action {
    if !button(area).contains(x, y) {
        return Action::None;
    }
    Action::Many(vec![
        Action::Mutate(Box::new(|store: &mut Store| {
            store.login = Some("Importing…".into())
        })),
        Action::request("login.start", json!({"method": "browser"}), Ask::Login),
    ])
}
