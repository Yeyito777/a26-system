//! Settings: who is signed in, the writes switch (off unless the user turns
//! it on: nothing is sent from this phone while it is off), notifications
//! and their previews, the connection, and logging this phone out.

use serde_json::json;

use super::icons::{self, Glyph};
use super::{Ask, Page, WhatsApp, avatar, theme};
use crate::exocortex::gesture::Gesture;
use crate::exocortex::ui::Haptic;
use crate::gfx::{Canvas, RectF};
use crate::social::shell::{Cx, DrawCx};
use crate::text::{self, Weight};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Logout,
}

#[derive(Debug, Clone, Default)]
pub struct SettingsView {
    pub dialog: Option<Dialog>,
}

const PROFILE_H: f32 = 300.0;
const ROW_H: f32 = 200.0;
const SECTION_H: f32 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Writes,
    Notifications,
    Previews,
    Connection,
    Logout,
}

const ROWS: [Row; 5] = [
    Row::Writes,
    Row::Notifications,
    Row::Previews,
    Row::Connection,
    Row::Logout,
];

/// Each row's top (window y).
fn rows() -> Vec<(Row, f32)> {
    let mut y = theme::TOP + theme::HEADER + PROFILE_H;
    let mut out = Vec::new();
    for row in ROWS {
        if matches!(row, Row::Writes | Row::Connection) {
            y += SECTION_H;
        }
        out.push((row, y));
        y += if row == Row::Logout || row == Row::Connection {
            150.0
        } else {
            ROW_H
        };
    }
    out
}

fn dialog_buttons(width: f32, height: f32) -> (RectF, RectF) {
    let w = width - 160.0;
    let y = height / 2.0 + 90.0;
    (
        RectF {
            x: 80.0 + w - 520.0,
            y,
            w: 240.0,
            h: 110.0,
        },
        RectF {
            x: 80.0 + w - 260.0,
            y,
            w: 240.0,
            h: 110.0,
        },
    )
}

pub fn gesture(app: &mut WhatsApp, cx: &mut Cx, gesture: Gesture) {
    let Gesture::Tap { x, y } = gesture else {
        if let Gesture::DragEnd {
            axis: crate::exocortex::gesture::Axis::Horizontal,
            x,
            ..
        } = gesture
            && x > cx.width * 0.4
        {
            app.back(cx);
        }
        return;
    };
    if let Some(Dialog::Logout) = app.settings_view.dialog {
        let (cancel, confirm) = dialog_buttons(cx.width, cx.height);
        if confirm.contains(x, y) {
            app.settings_view.dialog = None;
            app.ask(cx, "logout", json!({}), Ask::Other("log out"));
            app.page = Page::List;
            app.full = true;
        } else if cancel.contains(x, y) || !dialog_rect(cx.width, cx.height).contains(x, y) {
            app.settings_view.dialog = None;
            app.full = true;
        }
        cx.redraw();
        return;
    }
    if super::on_back(x, y) {
        app.back(cx);
        return;
    }
    let Some((row, _)) = rows().into_iter().find(|(row, top)| {
        y >= *top
            && y < *top
                + if matches!(row, Row::Logout | Row::Connection) {
                    150.0
                } else {
                    ROW_H
                }
    }) else {
        return;
    };
    match row {
        Row::Writes => {
            let on = !app.settings.writes;
            app.settings.writes = on;
            app.set_setting(cx, "writes", on);
            cx.haptic(Haptic::Light);
        }
        Row::Notifications => {
            let on = !app.settings.notifications;
            app.settings.notifications = on;
            app.set_setting(cx, "notifications", on);
            cx.haptic(Haptic::Light);
        }
        Row::Previews => {
            let on = !app.settings.previews;
            app.settings.previews = on;
            app.set_setting(cx, "previews", on);
            cx.haptic(Haptic::Light);
        }
        Row::Connection => {}
        Row::Logout => app.settings_view.dialog = Some(Dialog::Logout),
    }
    app.full = true;
    cx.redraw();
}

fn dialog_rect(width: f32, height: f32) -> RectF {
    RectF {
        x: 80.0,
        y: height / 2.0 - 260.0,
        w: width - 160.0,
        h: 500.0,
    }
}

pub fn draw(app: &mut WhatsApp, canvas: &mut Canvas, cx: &mut DrawCx) {
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    canvas.clear(theme::BG);
    super::draw_bar(canvas, "Settings", None, theme::BAR);
    // Who is signed in.
    let top = theme::TOP + theme::HEADER;
    let account = app.status.account.clone().unwrap_or_default();
    let mut waiting = Vec::new();
    let mut pics = avatar::Pics::new(&mut *cx.media, &mut waiting, 0.0);
    avatar::draw(
        canvas,
        &mut pics,
        &mut app.rounded,
        account.avatar.as_ref(),
        false,
        theme::MARGIN,
        top + 50.0,
        190.0,
        cx.now,
    );
    let name = if account.name.is_empty() {
        "WhatsApp".to_owned()
    } else {
        account.name.clone()
    };
    let name = text::fit(&name, Weight::Medium, 52.0, width - 330.0);
    text::draw(
        canvas,
        &name,
        280.0,
        top + 140.0,
        Weight::Medium,
        52.0,
        theme::TEXT,
        1.0,
    );
    if let Some(phone) = &account.username {
        text::draw(
            canvas,
            phone,
            280.0,
            top + 200.0,
            Weight::Regular,
            38.0,
            theme::SECONDARY,
            1.0,
        );
    }
    canvas.fill_rect(
        0,
        (top + PROFILE_H - 2.0) as i32,
        canvas.width,
        2,
        theme::DIVIDER,
        1.0,
    );
    let fixtures = app.hello.fixtures;
    for (row, y) in rows() {
        if matches!(row, Row::Writes | Row::Connection) {
            let label = if row == Row::Writes {
                "On this phone"
            } else {
                "Linked device"
            };
            text::draw(
                canvas,
                label,
                theme::MARGIN,
                y - 28.0,
                Weight::Medium,
                34.0,
                theme::SECONDARY,
                1.0,
            );
        }
        match row {
            Row::Writes => switch_row(
                canvas,
                y,
                width,
                "Send from this phone",
                "Messages, reactions, read receipts. While off, nothing is sent and nobody sees you online.",
                app.settings.writes,
            ),
            Row::Notifications => switch_row(
                canvas,
                y,
                width,
                "Notifications",
                "New messages notify on this phone",
                app.settings.notifications,
            ),
            Row::Previews => switch_row(
                canvas,
                y,
                width,
                "Message previews",
                "Show the text in notifications",
                app.settings.previews,
            ),
            Row::Connection => {
                let (state, color) = match app.status.state.as_str() {
                    "online" => ("Connected", theme::GREEN),
                    "offline" => ("Waiting for network", theme::SECONDARY),
                    "error" => ("Needs attention", theme::DANGER),
                    "signed_out" => ("Not linked", theme::SECONDARY),
                    _ => ("Connecting…", theme::SECONDARY),
                };
                let title = if fixtures {
                    "WhatsApp (made-up account)"
                } else {
                    "WhatsApp on this phone"
                };
                text::draw(
                    canvas,
                    title,
                    theme::MARGIN,
                    y + 62.0,
                    Weight::Regular,
                    44.0,
                    theme::TEXT,
                    1.0,
                );
                let detail = app
                    .status
                    .detail
                    .clone()
                    .map_or_else(|| state.to_owned(), |d| format!("{state}: {d}"));
                let detail = text::fit(&detail, Weight::Regular, 34.0, width - 2.0 * theme::MARGIN);
                text::draw(
                    canvas,
                    &detail,
                    theme::MARGIN,
                    y + 116.0,
                    Weight::Regular,
                    34.0,
                    color,
                    1.0,
                );
            }
            Row::Logout => {
                icons::draw(
                    canvas,
                    Glyph::Back,
                    theme::MARGIN + 30.0,
                    y + 75.0,
                    56.0,
                    theme::DANGER,
                    1.0,
                );
                text::draw(
                    canvas,
                    "Log out",
                    theme::MARGIN + 100.0,
                    y + 90.0,
                    Weight::Regular,
                    44.0,
                    theme::DANGER,
                    1.0,
                );
            }
        }
    }
    if let Some(Dialog::Logout) = app.settings_view.dialog {
        canvas.fill_rect(0, 0, canvas.width, canvas.height, 0x000000, 0.6);
        let rect = dialog_rect(width, height);
        canvas.fill_rrect(rect, 36.0, theme::SURFACE, 1.0);
        text::draw(
            canvas,
            "Log out?",
            rect.x + 60.0,
            rect.y + 100.0,
            Weight::Medium,
            50.0,
            theme::TEXT,
            1.0,
        );
        let lines = super::rich::layout(
            "This phone is unlinked from your WhatsApp account, and its chats are removed from it.",
            rect.w - 120.0,
            38.0,
            54.0,
            Weight::Regular,
        );
        super::rich::draw(
            canvas,
            &lines,
            rect.x + 60.0,
            rect.y + 180.0,
            Weight::Regular,
            theme::SECONDARY,
            theme::LINK,
            (0.0, height),
        );
        let (cancel, confirm) = dialog_buttons(width, height);
        text::draw_centered(
            canvas,
            "Cancel",
            cancel.x + cancel.w / 2.0,
            cancel.y + 70.0,
            Weight::Medium,
            40.0,
            theme::GREEN,
            1.0,
        );
        text::draw_centered(
            canvas,
            "Log out",
            confirm.x + confirm.w / 2.0,
            confirm.y + 70.0,
            Weight::Medium,
            40.0,
            theme::DANGER,
            1.0,
        );
    }
}

fn switch_row(canvas: &mut Canvas, y: f32, width: f32, title: &str, subtitle: &str, on: bool) {
    text::draw(
        canvas,
        title,
        theme::MARGIN,
        y + 70.0,
        Weight::Regular,
        44.0,
        theme::TEXT,
        1.0,
    );
    let laid = super::rich::layout(
        subtitle,
        width - 2.0 * theme::MARGIN - 200.0,
        33.0,
        44.0,
        Weight::Regular,
    );
    super::rich::draw(
        canvas,
        &laid,
        theme::MARGIN,
        y + 122.0,
        Weight::Regular,
        theme::SECONDARY,
        theme::LINK,
        (0.0, 4000.0),
    );
    let track = RectF {
        x: width - theme::MARGIN - 116.0,
        y: y + 42.0,
        w: 116.0,
        h: 64.0,
    };
    canvas.fill_rrect(
        track,
        32.0,
        if on { theme::GREEN } else { theme::SURFACE_HI },
        1.0,
    );
    let knob_x = if on {
        track.x + track.w - 32.0
    } else {
        track.x + 32.0
    };
    canvas.fill_circle(
        knob_x,
        track.y + 32.0,
        if on { 24.0 } else { 20.0 },
        if on {
            theme::ON_GREEN
        } else {
            theme::SECONDARY
        },
        1.0,
    );
}
