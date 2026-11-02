//! Linking this phone as one of the account's devices, as WhatsApp Web
//! does: the bridge's QR code (`login` events, rows of modules) drawn large
//! on white, the steps to scan it with the primary phone, then "Linking…"
//! until the chats come. The QR is asked for when the page shows and again
//! when it expires.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Ask, WhatsApp, theme};
use crate::exocortex::gesture::Gesture;
use crate::gfx::{Canvas, RectF};
use crate::social::proto::BridgeError;
use crate::social::shell::{Cx, DrawCx};
use crate::text::{self, Weight};

#[derive(Debug, Clone, Default, PartialEq)]
pub enum State {
    /// Not asked yet (or the page was left).
    #[default]
    Idle,
    Starting,
    Qr,
    Linking,
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct LoginView {
    pub state: State,
    pub qr: Option<(usize, Vec<String>)>,
    pub expires: Option<Instant>,
    started: Option<Instant>,
}

impl LoginView {
    /// Whether the page should ask for a QR on its own.
    pub fn wanted(&self) -> bool {
        self.state == State::Idle
    }
}

pub fn start(app: &mut WhatsApp, cx: &mut Cx) {
    app.login.state = State::Starting;
    app.login.qr = None;
    app.login.started = Some(cx.now);
    app.ask(cx, "login.start", json!({"method": "qr"}), Ask::Login);
    cx.redraw();
}

pub fn event(app: &mut WhatsApp, cx: &mut Cx, value: &Value) {
    match value["state"].as_str().unwrap_or("") {
        "qr" => {
            let size = value["qr"]["size"].as_u64().unwrap_or(0) as usize;
            let rows: Vec<String> = value["qr"]["rows"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(|r| r.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if size > 0 && rows.len() == size {
                app.login.qr = Some((size, rows));
                app.login.state = State::Qr;
                let ms = value["expires_ms"].as_u64().unwrap_or(20_000);
                app.login.expires = Some(cx.now + Duration::from_millis(ms));
            }
        }
        "waiting" => app.login.state = State::Linking,
        "done" => {
            app.login = LoginView::default();
            app.login.state = State::Linking;
        }
        "failed" => {
            let detail = value["detail"]
                .as_str()
                .unwrap_or("Linking failed. Try again.");
            app.login.state = State::Failed(detail.to_owned());
            app.login.qr = None;
        }
        _ => {}
    }
    app.full = true;
    cx.redraw();
}

pub fn failed(app: &mut WhatsApp, cx: &mut Cx, error: &BridgeError) {
    let text = match error.code.as_str() {
        "disconnected" => "Can't reach the WhatsApp bridge.".to_owned(),
        "invalid_argument" if error.message.contains("already") => return,
        _ => "Couldn't get a QR code. Try again.".to_owned(),
    };
    app.login.state = State::Failed(text);
    app.full = true;
    cx.redraw();
}

pub fn tick(app: &mut WhatsApp, cx: &mut Cx) {
    if !cx.connected {
        return;
    }
    // Asked and never answered (the bridge restarted meanwhile): again.
    let stale = app.login.state == State::Starting
        && app
            .login
            .started
            .is_some_and(|at| cx.now.duration_since(at) > Duration::from_secs(10));
    if app.login.state == State::Idle || stale {
        start(app, cx);
    }
    if app.login.state == State::Starting {
        // The spinner turns.
        cx.animate();
    }
    // The countdown under the QR.
    if app.login.state == State::Qr {
        cx.wake_at(cx.now + Duration::from_millis(1000));
    }
    if app.login.state == State::Linking {
        cx.animate();
    }
}

fn button_rect(width: f32, height: f32) -> RectF {
    RectF {
        x: width / 2.0 - 260.0,
        y: height - 520.0,
        w: 520.0,
        h: 130.0,
    }
}

pub fn gesture(app: &mut WhatsApp, cx: &mut Cx, gesture: Gesture) {
    if let Gesture::Tap { x, y } = gesture
        && matches!(app.login.state, State::Failed(_))
        && button_rect(cx.width, cx.height).contains(x, y)
    {
        start(app, cx);
    }
}

pub fn draw(app: &mut WhatsApp, canvas: &mut Canvas, cx: &mut DrawCx) {
    let width = canvas.width as f32;
    let height = canvas.height as f32;
    canvas.clear(theme::BG);
    let mut y = theme::TOP + 120.0;
    text::draw_centered(
        canvas,
        "Link this phone",
        width / 2.0,
        y,
        Weight::SemiBold,
        64.0,
        theme::TEXT,
        1.0,
    );
    y += 80.0;
    text::draw_centered(
        canvas,
        "Use WhatsApp on this phone as a linked device",
        width / 2.0,
        y,
        Weight::Regular,
        36.0,
        theme::SECONDARY,
        1.0,
    );
    y += 70.0;
    let side = 820.0;
    let qr_rect = RectF {
        x: (width - side) / 2.0,
        y,
        w: side,
        h: side,
    };
    match (&app.login.state, &app.login.qr) {
        (State::Qr, Some((size, rows))) => {
            canvas.fill_rrect(qr_rect, 36.0, 0xffffff, 1.0);
            // Four modules of quiet zone, whole pixels per module.
            let module = ((side - 40.0) / (*size as f32 + 8.0)).floor();
            let drawn = module * *size as f32;
            let ox = (qr_rect.x + (side - drawn) / 2.0).round();
            let oy = (qr_rect.y + (side - drawn) / 2.0).round();
            for (r, row) in rows.iter().enumerate() {
                let mut start: Option<usize> = None;
                for (c, bit) in row.bytes().chain(std::iter::once(b'0')).enumerate() {
                    match (bit == b'1', start) {
                        (true, None) => start = Some(c),
                        (false, Some(s)) => {
                            canvas.fill_rect(
                                (ox + s as f32 * module) as i32,
                                (oy + r as f32 * module) as i32,
                                ((c - s) as f32 * module) as i32,
                                module as i32,
                                0x000000,
                                1.0,
                            );
                            start = None;
                        }
                        _ => {}
                    }
                }
            }
            if let Some(expires) = app.login.expires {
                let left = expires.saturating_duration_since(cx.now).as_secs();
                let note = if left > 0 {
                    format!("The code refreshes in {left} s")
                } else {
                    "Refreshing the code…".to_owned()
                };
                text::draw_centered(
                    canvas,
                    &note,
                    width / 2.0,
                    qr_rect.y + side + 64.0,
                    Weight::Regular,
                    32.0,
                    theme::SECONDARY,
                    1.0,
                );
            }
        }
        (State::Linking, _) => {
            canvas.fill_rrect(qr_rect, 36.0, theme::SURFACE, 1.0);
            spinner(canvas, width / 2.0, qr_rect.y + side / 2.0 - 40.0, cx.now);
            text::draw_centered(
                canvas,
                "Linking…",
                width / 2.0,
                qr_rect.y + side / 2.0 + 110.0,
                Weight::Medium,
                44.0,
                theme::TEXT,
                1.0,
            );
            text::draw_centered(
                canvas,
                "Keep WhatsApp open on your phone",
                width / 2.0,
                qr_rect.y + side / 2.0 + 170.0,
                Weight::Regular,
                34.0,
                theme::SECONDARY,
                1.0,
            );
        }
        (State::Failed(detail), _) => {
            canvas.fill_rrect(qr_rect, 36.0, theme::SURFACE, 1.0);
            let detail = text::fit(detail, Weight::Regular, 38.0, side - 80.0);
            text::draw_centered(
                canvas,
                &detail,
                width / 2.0,
                qr_rect.y + side / 2.0,
                Weight::Regular,
                38.0,
                theme::TEXT,
                1.0,
            );
        }
        _ => {
            canvas.fill_rrect(qr_rect, 36.0, theme::SURFACE, 1.0);
            spinner(canvas, width / 2.0, qr_rect.y + side / 2.0 - 30.0, cx.now);
            let note = if cx_connected(app) {
                "Loading the QR code…"
            } else {
                "Connecting to the bridge…"
            };
            text::draw_centered(
                canvas,
                note,
                width / 2.0,
                qr_rect.y + side / 2.0 + 110.0,
                Weight::Regular,
                38.0,
                theme::SECONDARY,
                1.0,
            );
        }
    }
    let mut sy = qr_rect.y + side + 150.0;
    let steps = [
        "1. Open WhatsApp on your phone",
        "2. Tap Menu ⋮ or Settings, then Linked devices",
        "3. Tap Link a device",
        "4. Point your phone at this screen",
    ];
    for step in steps {
        let step = text::fit(step, Weight::Regular, 38.0, width - 2.0 * 110.0);
        text::draw(
            canvas,
            &step,
            110.0,
            sy,
            Weight::Regular,
            38.0,
            theme::TEXT,
            1.0,
        );
        sy += 70.0;
    }
    if matches!(app.login.state, State::Failed(_)) {
        let rect = button_rect(width, height);
        canvas.fill_rrect(rect, rect.h / 2.0, theme::GREEN, 1.0);
        text::draw_centered(
            canvas,
            "Try again",
            width / 2.0,
            rect.y + 84.0,
            Weight::SemiBold,
            42.0,
            theme::ON_GREEN,
            1.0,
        );
    }
    text::draw_centered(
        canvas,
        "Your messages stay end-to-end encrypted",
        width / 2.0,
        height - theme::GESTURE_RESERVE - 40.0,
        Weight::Regular,
        30.0,
        theme::SECONDARY,
        1.0,
    );
}

fn cx_connected(app: &WhatsApp) -> bool {
    app.greeted
}

fn spinner(canvas: &mut Canvas, cx: f32, cy: f32, _now: Instant) {
    let phase = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
        % 1200) as f32
        / 1200.0;
    let start = phase * std::f32::consts::TAU;
    canvas.ring_arc(cx, cy, 60.0, 9.0, start, start + 4.2, theme::GREEN, 1.0);
}
