//! Phone and Messages, drawn in the System app's iOS dark style: a keypad
//! and Recents under a tab bar, the conversation list, and a conversation's
//! bubbles. Lists scroll (Moon hands its own apps drags).

use std::error::Error;
use std::time::Instant;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::Window;

use crate::comms::{Action, CommsModel, KEYS, Kind, Page, Tab};
use crate::gfx::{Canvas, Path, RectF, Rgb};
use crate::present::Presenter;
use crate::telephony::{self, CallRecord, Message, Service, Thread};
use crate::text::{self, Weight};
use crate::ui::{
    BG, BLUE, GREEN, MARGIN, RED, SECONDARY, SECONDARY_ALPHA, SEPARATOR, TERTIARY_ALPHA, WHITE,
    back_chevron, baseline, chevron, large_title,
};

const W: f32 = 1080.0;
/// Moon owns the bottom of the window for its swipe-up gesture.
const GESTURE_RESERVE: f32 = 170.0;
const TAB_BAR_H: f32 = 150.0;
const TITLE_BASELINE: f32 = 132.0;
const LIST_TOP: f32 = 190.0;
const INSET: f32 = MARGIN + 44.0;
const ROW_PRESSED: Rgb = 0x2c2c2e;
const BAR: Rgb = 0x161618;
const BUBBLE_IN: Rgb = 0x26252a;
const GRAY: Rgb = 0x8e8e93;
const KEY: Rgb = 0x333333;
const KEY_DOWN: Rgb = 0x737373;

// Keypad.
const NUMBER_Y: f32 = 250.0;
const NOTICE_Y: f32 = 350.0;
const KEY_RADIUS: f32 = 106.0;
const KEY_COLUMNS: [f32; 3] = [240.0, 540.0, 840.0];
const KEY_TOP: f32 = 560.0;
const KEY_PITCH: f32 = 248.0;
const CALL_Y: f32 = KEY_TOP + 4.0 * KEY_PITCH + 22.0;
const LETTERS: [&str; 12] = [
    "", "ABC", "DEF", "GHI", "JKL", "MNO", "PQRS", "TUV", "WXYZ", "", "+", "",
];

// Lists.
const RECENT_ROW: f32 = 140.0;
const THREAD_ROW: f32 = 190.0;
const NAV_H: f32 = 150.0;

// Conversation bubbles.
const BUBBLE_TEXT: f32 = 47.0;
const BUBBLE_LINE: f32 = 60.0;
const BUBBLE_PAD_X: f32 = 36.0;
const BUBBLE_PAD_Y: f32 = 24.0;
const BUBBLE_MAX: f32 = 780.0;
const BUBBLE_RADIUS: f32 = 44.0;

/// What a touch is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Action(Action),
}

pub struct Renderer {
    window: Window,
    depth: u8,
    canvas: Canvas,
    presenter: Presenter,
}

impl Renderer {
    pub fn new<C: Connection>(
        conn: &C,
        window: Window,
        depth: u8,
        width: u16,
        height: u16,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            window,
            depth,
            canvas: Canvas::new(i32::from(width), i32::from(height)),
            presenter: Presenter::create(conn, window, depth, width, height)?,
        })
    }

    pub fn resize<C: Connection>(
        &mut self,
        conn: &C,
        width: u16,
        height: u16,
    ) -> Result<(), Box<dyn Error>> {
        let presenter = Presenter::create(conn, self.window, self.depth, width, height)?;
        self.presenter.destroy(conn);
        self.presenter = presenter;
        self.canvas = Canvas::new(i32::from(width), i32::from(height));
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        (self.canvas.width as u16, self.canvas.height as u16)
    }

    pub fn height(&self) -> u16 {
        self.canvas.height as u16
    }

    pub fn render<C: Connection>(
        &mut self,
        conn: &C,
        model: &CommsModel,
        pressed: Option<Hit>,
        now: Instant,
    ) -> Result<(), Box<dyn Error>> {
        let canvas = &mut self.canvas;
        canvas.clear(BG);
        draw(canvas, model, pressed, now);
        self.presenter.present(conn, &self.canvas, 0)?;
        conn.flush()?;
        Ok(())
    }

    pub fn expose<C: Connection>(&self, conn: &C) -> Result<(), Box<dyn Error>> {
        self.presenter.expose(conn, 0)?;
        conn.flush()?;
        Ok(())
    }

    pub fn destroy<C: Connection>(&self, conn: &C) {
        self.presenter.destroy(conn);
    }
}

fn pressed_action(pressed: Option<Hit>) -> Option<Action> {
    pressed.map(|Hit::Action(action)| action)
}

pub fn draw(canvas: &mut Canvas, model: &CommsModel, pressed: Option<Hit>, now: Instant) {
    let height = canvas.height as f32;
    let pressed = pressed_action(pressed);
    match &model.page {
        Page::Phone(Tab::Keypad) => draw_keypad(canvas, model, pressed, now),
        Page::Phone(Tab::Recents) => draw_recents(canvas, model, pressed, height, now),
        Page::Threads => draw_threads(canvas, model, pressed, height, now),
        Page::Thread(peer) => draw_thread(canvas, model, peer, pressed, height, now),
    }
    if model.kind == Kind::Phone {
        draw_tab_bar(canvas, model, height);
    }
}

// ----- Phone -------------------------------------------------------------

fn key_center(index: usize) -> (f32, f32) {
    (
        KEY_COLUMNS[index % 3],
        KEY_TOP + (index / 3) as f32 * KEY_PITCH,
    )
}

fn draw_keypad(canvas: &mut Canvas, model: &CommsModel, pressed: Option<Action>, now: Instant) {
    let number = telephony::format_typed(&model.digits);
    let size = if number.len() > 14 { 76.0 } else { 100.0 };
    let fitted = fit_tail(&number, size, W - 120.0);
    text::draw_centered(
        canvas,
        &fitted,
        W / 2.0,
        baseline(NUMBER_Y, Weight::Regular, size),
        Weight::Regular,
        size,
        WHITE,
        1.0,
    );
    let service = model.service();
    let (line, color, alpha) = match model.notice(now) {
        Some(notice) => (notice, WHITE, 0.9),
        None => (
            crate::comms::service_text(service),
            SECONDARY,
            SECONDARY_ALPHA,
        ),
    };
    if !line.is_empty() {
        text::draw_centered(
            canvas,
            line,
            W / 2.0,
            baseline(NOTICE_Y, Weight::Regular, 42.0),
            Weight::Regular,
            42.0,
            color,
            alpha,
        );
    }
    for index in 0..12 {
        let (cx, cy) = key_center(index);
        let down = pressed == Some(Action::Key(index as u8));
        canvas.fill_circle(cx, cy, KEY_RADIUS, if down { KEY_DOWN } else { KEY }, 1.0);
        let symbol = KEYS[index].to_string();
        let letters = LETTERS[index];
        let (size, base) = match KEYS[index] {
            '*' => (130.0, cy + 60.0),
            '#' => (92.0, baseline(cy, Weight::Regular, 92.0)),
            _ if letters.is_empty() => (100.0, baseline(cy, Weight::Regular, 100.0)),
            _ => (100.0, cy + 18.0),
        };
        text::draw_centered(canvas, &symbol, cx, base, Weight::Regular, size, WHITE, 1.0);
        if !letters.is_empty() {
            let tracking = 5.0;
            let width = text::measure_tracked(letters, Weight::SemiBold, 27.0, tracking);
            text::draw_tracked(
                canvas,
                letters,
                cx - width / 2.0,
                cy + 62.0,
                Weight::SemiBold,
                27.0,
                tracking,
                WHITE,
                1.0,
            );
        }
    }
    let call_down = pressed == Some(Action::Call);
    canvas.fill_circle(
        W / 2.0,
        CALL_Y,
        KEY_RADIUS,
        GREEN,
        if call_down { 0.6 } else { 1.0 },
    );
    draw_handset(canvas, W / 2.0, CALL_Y, 1.25, WHITE);
    if !model.digits.is_empty() {
        let alpha = if pressed == Some(Action::Delete) {
            0.4
        } else {
            1.0
        };
        draw_backspace(canvas, KEY_COLUMNS[2], CALL_Y, alpha);
    }
}

fn draw_tab_bar(canvas: &mut Canvas, model: &CommsModel, height: f32) {
    let top = height - GESTURE_RESERVE - TAB_BAR_H;
    canvas.fill_rect(0, top as i32, W as i32, (height - top) as i32 + 1, BAR, 1.0);
    canvas.fill_rect(0, top as i32, W as i32, 2, SEPARATOR, 1.0);
    let current = match model.page {
        Page::Phone(tab) => tab,
        _ => Tab::Keypad,
    };
    for (tab, cx, label) in [
        (Tab::Recents, 360.0, "Recents"),
        (Tab::Keypad, 720.0, "Keypad"),
    ] {
        let color = if tab == current { BLUE } else { GRAY };
        let icon_y = top + 52.0;
        match tab {
            Tab::Recents => draw_clock_glyph(canvas, cx, icon_y, color),
            Tab::Keypad => {
                for row in 0..4 {
                    for column in 0..3 {
                        if row == 3 && column != 1 {
                            continue;
                        }
                        canvas.fill_circle(
                            cx + (column as f32 - 1.0) * 17.0,
                            icon_y - 25.0 + row as f32 * 17.0,
                            6.5,
                            color,
                            1.0,
                        );
                    }
                }
            }
        }
        text::draw_centered(
            canvas,
            label,
            cx,
            top + 122.0,
            Weight::Medium,
            30.0,
            color,
            1.0,
        );
    }
}

fn recents_top() -> f32 {
    LIST_TOP
}

fn recents_bottom(height: f32) -> f32 {
    height - GESTURE_RESERVE - TAB_BAR_H
}

fn draw_recents(
    canvas: &mut Canvas,
    model: &CommsModel,
    pressed: Option<Action>,
    height: f32,
    now: Instant,
) {
    large_title(canvas, "Recents", TITLE_BASELINE);
    let recents = model.recents.as_deref().unwrap_or_default();
    let (top, bottom) = (recents_top(), recents_bottom(height));
    if recents.is_empty() {
        if model.recents.is_some() || !model.connected {
            let detail = match model.service() {
                Service::Ready => "",
                service => crate::comms::service_text(service),
            };
            empty_state(canvas, "No Recents", detail, (top + bottom) / 2.0);
        }
        return;
    }
    let offset = model.scroll.value(now);
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    let today = local_day(crate::clock::unix_now());
    for (index, call) in recents.iter().enumerate() {
        let y = top + index as f32 * RECENT_ROW - offset;
        if y + RECENT_ROW < top || y > bottom {
            continue;
        }
        if pressed == Some(Action::CallBack(index)) {
            canvas.fill_rect(0, y as i32, W as i32, RECENT_ROW as i32, ROW_PRESSED, 1.0);
        }
        draw_recent(canvas, call, y, today);
        canvas.fill_rect(
            INSET as i32,
            (y + RECENT_ROW - 2.0) as i32,
            (W - INSET) as i32,
            2,
            SEPARATOR,
            1.0,
        );
    }
    canvas.reset_clip();
}

fn draw_recent(canvas: &mut Canvas, call: &CallRecord, y: f32, today: (i32, i32)) {
    let name = call
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .or_else(|| call.number.as_deref().map(telephony::format_number))
        .unwrap_or_else(|| "Unknown".into());
    let missed = call.missed;
    let name = text::fit(&name, Weight::SemiBold, 47.0, 620.0);
    text::draw(
        canvas,
        &name,
        INSET,
        y + 62.0,
        Weight::SemiBold,
        47.0,
        if missed { RED } else { WHITE },
        1.0,
    );
    let kind = match (call.direction.as_str(), call.missed, call.rejected) {
        (_, true, _) => "Missed".to_owned(),
        (_, _, true) => "Declined".to_owned(),
        ("outgoing", ..) if call.duration_s == 0 => "Cancelled".to_owned(),
        ("outgoing", ..) => format!("Outgoing · {}", duration_text(call.duration_s)),
        _ => format!("Incoming · {}", duration_text(call.duration_s)),
    };
    text::draw(
        canvas,
        &kind,
        INSET,
        y + 112.0,
        Weight::Regular,
        38.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    let when = when_text(call.started_at, today);
    text::draw_right(
        canvas,
        &when,
        W - MARGIN - 44.0,
        y + 84.0,
        Weight::Regular,
        40.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}

fn duration_text(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds} sec"),
        60..=3599 => format!("{} min", seconds / 60),
        _ => format!("{} hr {} min", seconds / 3600, seconds / 60 % 60),
    }
}

// ----- Messages ----------------------------------------------------------

fn threads_bottom(height: f32) -> f32 {
    height - GESTURE_RESERVE
}

/// The line under the title when messages cannot go out.
fn threads_top(model: &CommsModel) -> f32 {
    match model.service() {
        Service::Ready => LIST_TOP,
        _ => LIST_TOP + 70.0,
    }
}

fn draw_threads(
    canvas: &mut Canvas,
    model: &CommsModel,
    pressed: Option<Action>,
    height: f32,
    now: Instant,
) {
    large_title(canvas, "Messages", TITLE_BASELINE);
    let service = model.service();
    let threads = model.threads.as_deref().unwrap_or_default();
    let (top, bottom) = (threads_top(model), threads_bottom(height));
    if threads.is_empty() {
        if model.threads.is_some() || !model.connected {
            let (title, detail) = match service {
                Service::NoSim => (
                    "No SIM",
                    "Messages can be sent and received once a SIM is in.",
                ),
                Service::Unavailable => ("No Messages", "Cellular service unavailable"),
                _ => ("No Messages", ""),
            };
            empty_state(canvas, title, detail, (LIST_TOP + bottom) / 2.0);
        }
        return;
    }
    if service != Service::Ready {
        let line = format!(
            "{} · messages can't be sent",
            crate::comms::service_text(service)
        );
        text::draw(
            canvas,
            &line,
            MARGIN,
            LIST_TOP + 30.0,
            Weight::Regular,
            38.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
    let offset = model.scroll.value(now);
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    let today = local_day(crate::clock::unix_now());
    for (index, thread) in threads.iter().enumerate() {
        let y = top + index as f32 * THREAD_ROW - offset;
        if y + THREAD_ROW < top || y > bottom {
            continue;
        }
        if pressed == Some(Action::OpenThread(index)) {
            canvas.fill_rect(0, y as i32, W as i32, THREAD_ROW as i32, ROW_PRESSED, 1.0);
        }
        draw_thread_row(canvas, thread, y, today);
    }
    canvas.reset_clip();
}

fn draw_thread_row(canvas: &mut Canvas, thread: &Thread, y: f32, today: (i32, i32)) {
    if thread.unread > 0 {
        canvas.fill_circle(30.0, y + THREAD_ROW / 2.0, 11.0, BLUE, 1.0);
    }
    draw_avatar(canvas, 128.0, y + THREAD_ROW / 2.0, 62.0);
    let x = 224.0;
    let right = W - MARGIN - 20.0;
    let when = when_text(thread.last.time, today);
    let when_w = text::measure(&when, Weight::Regular, 38.0);
    text::draw_right(
        canvas,
        &when,
        right - 34.0,
        y + 72.0,
        Weight::Regular,
        38.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    chevron(canvas, right - 6.0, y + 60.0, SECONDARY, TERTIARY_ALPHA);
    let name = text::fit(
        &telephony::format_number(&thread.peer),
        Weight::SemiBold,
        47.0,
        right - 70.0 - when_w - x,
    );
    text::draw(
        canvas,
        &name,
        x,
        y + 74.0,
        Weight::SemiBold,
        47.0,
        WHITE,
        1.0,
    );
    let preview: String = thread
        .last
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let preview = text::fit(&preview, Weight::Regular, 42.0, right - x);
    text::draw(
        canvas,
        &preview,
        x,
        y + 132.0,
        Weight::Regular,
        42.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    canvas.fill_rect(
        x as i32,
        (y + THREAD_ROW - 2.0) as i32,
        (W - x) as i32,
        2,
        SEPARATOR,
        1.0,
    );
}

fn draw_avatar(canvas: &mut Canvas, cx: f32, cy: f32, r: f32) {
    canvas.fill_circle(cx, cy, r, 0x6e6e73, 1.0);
    canvas.set_clip(
        (cx - r) as i32,
        (cy - r) as i32,
        (cx + r) as i32,
        (cy + r) as i32,
    );
    canvas.fill_circle(cx, cy - r * 0.2, r * 0.36, 0xd1d1d6, 1.0);
    let mut shoulders = Path::new();
    let steps = 48;
    let points: Vec<(f32, f32)> = (0..steps)
        .map(|step| {
            let angle = std::f32::consts::TAU * step as f32 / steps as f32;
            (
                cx + r * 0.68 * angle.cos(),
                cy + r * 0.92 + r * 0.55 * angle.sin(),
            )
        })
        .collect();
    shoulders.polygon(&points);
    canvas.fill_path(&shoulders, 0xd1d1d6, 1.0);
    canvas.reset_clip();
}

/// A conversation's bubbles, top to bottom, with their heights.
struct Bubble<'a> {
    message: &'a Message,
    lines: Vec<String>,
    /// A time header above it.
    header: Option<String>,
    y: f32,
    h: f32,
}

fn layout_thread(messages: &[Message], today: (i32, i32)) -> (Vec<Bubble<'_>>, f32) {
    let mut bubbles = Vec::with_capacity(messages.len());
    let mut y = 36.0;
    let mut previous: Option<&Message> = None;
    for message in messages {
        let header = match previous {
            Some(p) if message.time - p.time < 3600 => None,
            _ => Some(header_text(message.time, today)),
        };
        if header.is_some() {
            y += if previous.is_some() { 40.0 } else { 0.0 };
            y += 70.0;
        } else if previous.is_some_and(|p| p.direction != message.direction) {
            y += 22.0;
        } else if previous.is_some() {
            y += 8.0;
        }
        let lines = wrap(&message.text, BUBBLE_MAX - 2.0 * BUBBLE_PAD_X);
        let h = lines.len().max(1) as f32 * BUBBLE_LINE + 2.0 * BUBBLE_PAD_Y;
        bubbles.push(Bubble {
            message,
            lines,
            header,
            y,
            h,
        });
        y += h;
        previous = Some(message);
    }
    // Room for the delivery status under the last outgoing message.
    (bubbles, y + 90.0)
}

fn thread_top() -> f32 {
    NAV_H
}

fn thread_bottom(height: f32) -> f32 {
    height - GESTURE_RESERVE
}

fn draw_thread(
    canvas: &mut Canvas,
    model: &CommsModel,
    peer: &str,
    pressed: Option<Action>,
    height: f32,
    now: Instant,
) {
    let (top, bottom) = (thread_top(), thread_bottom(height));
    let today = local_day(crate::clock::unix_now());
    let (bubbles, content) = layout_thread(&model.messages, today);
    let offset = model.scroll.value(now);
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    let last_out = model.messages.iter().rposition(|m| m.direction == "out");
    for (index, bubble) in bubbles.iter().enumerate() {
        let y = top + bubble.y - offset;
        if let Some(header) = &bubble.header {
            text::draw_centered(
                canvas,
                header,
                W / 2.0,
                y - 30.0,
                Weight::Regular,
                34.0,
                SECONDARY,
                SECONDARY_ALPHA,
            );
        }
        if y + bubble.h < top || y > bottom {
            continue;
        }
        let outgoing = bubble.message.direction == "out";
        let width = bubble
            .lines
            .iter()
            .map(|line| text::measure(line, Weight::Regular, BUBBLE_TEXT))
            .fold(0.0, f32::max)
            + 2.0 * BUBBLE_PAD_X;
        let width = width.max(2.0 * BUBBLE_RADIUS);
        let x = if outgoing { W - MARGIN - width } else { MARGIN };
        let failed = bubble.message.status == "failed";
        let fill = if outgoing { BLUE } else { BUBBLE_IN };
        canvas.fill_rrect(RectF::new(x, y, width, bubble.h), BUBBLE_RADIUS, fill, 1.0);
        for (line_index, line) in bubble.lines.iter().enumerate() {
            text::draw(
                canvas,
                line,
                x + BUBBLE_PAD_X,
                y + BUBBLE_PAD_Y + line_index as f32 * BUBBLE_LINE + 45.0,
                Weight::Regular,
                BUBBLE_TEXT,
                WHITE,
                1.0,
            );
        }
        if Some(index) == last_out {
            let (status, color) = match bubble.message.status.as_str() {
                "delivered" => ("Delivered", SECONDARY),
                "sent" => ("Sent", SECONDARY),
                "sending" => ("Sending…", SECONDARY),
                _ if failed => ("Not Delivered", RED),
                _ => ("", SECONDARY),
            };
            text::draw_right(
                canvas,
                status,
                W - MARGIN - 10.0,
                y + bubble.h + 48.0,
                Weight::SemiBold,
                32.0,
                color,
                if color == RED { 1.0 } else { SECONDARY_ALPHA },
            );
        }
    }
    if model.service() != Service::Ready {
        let line = format!(
            "{} · messages can't be sent",
            crate::comms::service_text(model.service())
        );
        text::draw_centered(
            canvas,
            &line,
            W / 2.0,
            top + content - offset + 10.0,
            Weight::Regular,
            34.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
    canvas.reset_clip();
    // The navigation bar over the list.
    canvas.fill_rect(0, 0, W as i32, NAV_H as i32, BG, 1.0);
    let back_alpha = if pressed == Some(Action::Back) {
        0.4
    } else {
        1.0
    };
    back_chevron(canvas, MARGIN + 6.0, 72.0);
    text::draw(
        canvas,
        "Messages",
        MARGIN + 36.0,
        baseline(72.0, Weight::Regular, 47.0),
        Weight::Regular,
        47.0,
        BLUE,
        back_alpha,
    );
    let title = text::fit(
        &telephony::format_number(peer),
        Weight::SemiBold,
        47.0,
        460.0,
    );
    text::draw_centered(
        canvas,
        &title,
        W / 2.0 + 120.0,
        baseline(72.0, Weight::SemiBold, 47.0),
        Weight::SemiBold,
        47.0,
        WHITE,
        1.0,
    );
    canvas.fill_rect(0, NAV_H as i32 - 2, W as i32, 2, SEPARATOR, 1.0);
}

// ----- Shared ------------------------------------------------------------

/// A long typed number keeps its end in view, as the iPhone keypad does.
fn fit_tail(number: &str, size: f32, width: f32) -> String {
    if text::measure(number, Weight::Regular, size) <= width {
        return number.to_owned();
    }
    let chars: Vec<char> = number.chars().collect();
    (1..chars.len())
        .map(|skip| format!("…{}", chars[skip..].iter().collect::<String>()))
        .find(|tail| text::measure(tail, Weight::Regular, size) <= width)
        .unwrap_or_default()
}

fn empty_state(canvas: &mut Canvas, title: &str, detail: &str, cy: f32) {
    text::draw_centered(
        canvas,
        title,
        W / 2.0,
        cy,
        Weight::SemiBold,
        58.0,
        WHITE,
        0.9,
    );
    if !detail.is_empty() {
        let detail = text::fit(detail, Weight::Regular, 40.0, W - 2.0 * INSET);
        text::draw_centered(
            canvas,
            &detail,
            W / 2.0,
            cy + 70.0,
            Weight::Regular,
            40.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
}

/// Word-wraps `text` to `width` at the bubble size.
fn wrap(text_in: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text_in.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if text::measure(&candidate, Weight::Regular, BUBBLE_TEXT) <= width || line.is_empty() {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_owned();
            }
            // A single word longer than the bubble is cut to fit.
            if text::measure(&line, Weight::Regular, BUBBLE_TEXT) > width {
                lines.push(text::fit(&line, Weight::Regular, BUBBLE_TEXT, width));
                line.clear();
            }
        }
        lines.push(line);
    }
    while lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// (year, day of year) in local time.
fn local_day(t: i64) -> (i32, i32) {
    let tm = local(t);
    (tm.tm_year, tm.tm_yday)
}

fn local(t: i64) -> libc::tm {
    // SAFETY: localtime_r fills the zeroed tm from a valid time_t.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = t as _;
        libc::localtime_r(&time, &mut tm);
        tm
    }
}

fn clock_text(tm: &libc::tm) -> String {
    let (time, half) = crate::clock::alarm_time(tm.tm_hour as u8, tm.tm_min as u8);
    format!("{time} {half}")
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// iOS list times: "4:12 PM" today, "Yesterday", a weekday this week, else
/// the date.
fn when_text(t: i64, today: (i32, i32)) -> String {
    let tm = local(t);
    let days = if tm.tm_year == today.0 {
        today.1 - tm.tm_yday
    } else {
        i32::MAX
    };
    match days {
        0 => clock_text(&tm),
        1 => "Yesterday".into(),
        2..=6 => WEEKDAYS[tm.tm_wday as usize % 7].into(),
        _ => format!("{}/{}/{:02}", tm.tm_mon + 1, tm.tm_mday, tm.tm_year % 100),
    }
}

/// Conversation time headers: "Today 4:12 PM", "Yesterday 9:03 AM".
fn header_text(t: i64, today: (i32, i32)) -> String {
    let tm = local(t);
    let day = when_text(t, today);
    if day.contains(':') {
        format!("Today {day}")
    } else {
        format!("{day} {}", clock_text(&tm))
    }
}

fn draw_handset(canvas: &mut Canvas, cx: f32, cy: f32, scale: f32, color: Rgb) {
    // The same outline as Moon's call screen (calls.rs).
    let s = scale;
    let radius = 50.0 * s;
    let (spine, pad) = (15.0 * s, 34.0 * s);
    let (from, to) = (200_f32.to_radians(), 70_f32.to_radians());
    let (ox, oy) = (0.32 * radius, -0.32 * radius);
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    const STEPS: usize = 64;
    let mut outer = Vec::with_capacity(STEPS + 1);
    let mut inner = Vec::with_capacity(STEPS + 1);
    for step in 0..=STEPS {
        let u = step as f32 / STEPS as f32;
        let angle = from + (to - from) * u;
        let (sin, cos) = angle.sin_cos();
        let piece = smooth(0.30, 0.17, u) + smooth(0.70, 0.83, u);
        let thickness = spine + (pad - spine) * piece;
        let corner = 7.0 * s;
        let from_end = u.min(1.0 - u) * (from - to).abs() * radius;
        let round = if from_end < corner {
            corner
                - (corner * corner - (corner - from_end).powi(2))
                    .max(0.0)
                    .sqrt()
        } else {
            0.0
        };
        let out = radius + spine / 2.0 - round;
        let inn = radius + spine / 2.0 - thickness + round;
        outer.push((cx + ox + cos * out, cy + oy + sin * out));
        inner.push((cx + ox + cos * inn, cy + oy + sin * inn));
    }
    let mut outline = outer;
    outline.extend(inner.into_iter().rev());
    let mut path = Path::new();
    path.polygon(&outline);
    canvas.fill_path(&path, color, 1.0);
}

fn draw_backspace(canvas: &mut Canvas, cx: f32, cy: f32, alpha: f32) {
    let (w, h) = (84.0, 60.0);
    let left = cx - w / 2.0;
    let mut outline = Path::new();
    outline.stroke_polyline(
        &[
            (left + 22.0, cy - h / 2.0),
            (left + w, cy - h / 2.0),
            (left + w, cy + h / 2.0),
            (left + 22.0, cy + h / 2.0),
            (left - 6.0, cy),
            (left + 22.0, cy - h / 2.0),
        ],
        6.0,
    );
    let (xc, r) = (left + 50.0, 12.0);
    outline.stroke_segment((xc - r, cy - r), (xc + r, cy + r), 6.0);
    outline.stroke_segment((xc - r, cy + r), (xc + r, cy - r), 6.0);
    canvas.fill_path(&outline, GRAY, alpha);
}

fn draw_clock_glyph(canvas: &mut Canvas, cx: f32, cy: f32, color: Rgb) {
    canvas.fill_circle(cx, cy, 30.0, color, 1.0);
    let mut hands = Path::new();
    hands.stroke_segment((cx, cy), (cx, cy - 18.0), 5.0);
    hands.stroke_segment((cx, cy), (cx + 13.0, cy + 6.0), 5.0);
    canvas.fill_path(&hands, BAR, 1.0);
}

// ----- Hit testing and scrolling ----------------------------------------

/// The scrollable list's range for the page, and its content height.
fn list(model: &CommsModel, height: f32) -> Option<(f32, f32, f32)> {
    match &model.page {
        Page::Phone(Tab::Recents) => {
            let rows = model.recents.as_ref().map_or(0, Vec::len);
            Some((
                recents_top(),
                recents_bottom(height),
                rows as f32 * RECENT_ROW,
            ))
        }
        Page::Threads => {
            let rows = model.threads.as_ref().map_or(0, Vec::len);
            Some((
                threads_top(model),
                threads_bottom(height),
                rows as f32 * THREAD_ROW,
            ))
        }
        Page::Thread(_) => {
            let today = local_day(crate::clock::unix_now());
            let (_, content) = layout_thread(&model.messages, today);
            Some((thread_top(), thread_bottom(height), content + 40.0))
        }
        Page::Phone(Tab::Keypad) => None,
    }
}

pub fn scroll_max(model: &CommsModel, height: f32) -> f32 {
    list(model, height).map_or(0.0, |(top, bottom, content)| {
        (content - (bottom - top)).max(0.0)
    })
}

/// A touch at `y` may scroll the page's list.
pub fn scrolls(model: &CommsModel, height: f32, y: f32) -> bool {
    list(model, height).is_some_and(|(top, bottom, _)| y >= top && y < bottom)
}

pub fn hit(model: &CommsModel, height: f32, x: i16, y: i16, now: Instant) -> Option<Hit> {
    let (x, y) = (f32::from(x), f32::from(y));
    if model.kind == Kind::Phone {
        let top = height - GESTURE_RESERVE - TAB_BAR_H;
        if y >= top {
            let tab = if x < W / 2.0 {
                Tab::Recents
            } else {
                Tab::Keypad
            };
            return (y < top + TAB_BAR_H).then_some(Hit::Action(Action::Tab(tab)));
        }
    }
    let offset = model.scroll.value(now);
    let action = match &model.page {
        Page::Phone(Tab::Keypad) => {
            let near = |(cx, cy): (f32, f32)| (x - cx).abs() < 140.0 && (y - cy).abs() < 120.0;
            if let Some(index) = (0..12).find(|index| near(key_center(*index))) {
                Some(Action::Key(index as u8))
            } else if near((W / 2.0, CALL_Y)) {
                Some(Action::Call)
            } else if !model.digits.is_empty() && near((KEY_COLUMNS[2], CALL_Y)) {
                Some(Action::Delete)
            } else {
                None
            }
        }
        Page::Phone(Tab::Recents) => {
            let rows = model.recents.as_ref().map_or(0, Vec::len);
            row_at(
                y,
                recents_top(),
                recents_bottom(height),
                offset,
                RECENT_ROW,
                rows,
            )
            .map(Action::CallBack)
        }
        Page::Threads => {
            let rows = model.threads.as_ref().map_or(0, Vec::len);
            row_at(
                y,
                threads_top(model),
                threads_bottom(height),
                offset,
                THREAD_ROW,
                rows,
            )
            .map(Action::OpenThread)
        }
        Page::Thread(_) => (y < NAV_H && x < 420.0).then_some(Action::Back),
    };
    action.map(Hit::Action)
}

fn row_at(y: f32, top: f32, bottom: f32, offset: f32, row: f32, rows: usize) -> Option<usize> {
    if y < top || y >= bottom {
        return None;
    }
    let index = ((y - top + offset) / row).floor();
    (index >= 0.0 && (index as usize) < rows).then_some(index as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::CommsModel;

    #[test]
    fn keypad_keys_call_and_tabs_hit() {
        let now = Instant::now();
        let mut model = CommsModel::new(Kind::Phone);
        let height = 2216.0;
        let (cx, cy) = key_center(10);
        assert_eq!(
            hit(&model, height, cx as i16, cy as i16, now),
            Some(Hit::Action(Action::Key(10)))
        );
        assert_eq!(
            hit(&model, height, 540, CALL_Y as i16, now),
            Some(Hit::Action(Action::Call))
        );
        assert_eq!(hit(&model, height, 840, CALL_Y as i16, now), None);
        model.digits.push('5');
        assert_eq!(
            hit(&model, height, 840, CALL_Y as i16, now),
            Some(Hit::Action(Action::Delete))
        );
        let tab_y = (height - GESTURE_RESERVE - TAB_BAR_H / 2.0) as i16;
        assert_eq!(
            hit(&model, height, 300, tab_y, now),
            Some(Hit::Action(Action::Tab(Tab::Recents)))
        );
        // Keys clear the tab bar.
        assert!(CALL_Y + KEY_RADIUS < height - GESTURE_RESERVE - TAB_BAR_H);
    }

    #[test]
    fn bubbles_wrap_and_lay_out_with_headers() {
        let lines = wrap(
            "one two three four five six seven eight nine ten eleven twelve",
            400.0,
        );
        assert!(lines.len() > 1);
        assert!(
            lines
                .iter()
                .all(|line| text::measure(line, Weight::Regular, BUBBLE_TEXT) <= 400.0)
        );
        let message = |id: &str, time: i64, direction: &str| Message {
            id: id.into(),
            peer: "+1".into(),
            direction: direction.into(),
            text: "Hi".into(),
            time,
            status: "delivered".into(),
            read: true,
        };
        let messages = [
            message("m1", 1000, "in"),
            message("m2", 1100, "out"),
            message("m3", 9000, "in"),
        ];
        let (bubbles, content) = layout_thread(&messages, (0, 0));
        assert!(bubbles[0].header.is_some());
        assert!(bubbles[1].header.is_none());
        assert!(bubbles[2].header.is_some());
        assert!(bubbles[1].y > bubbles[0].y + bubbles[0].h);
        assert!(content > bubbles[2].y + bubbles[2].h);
    }
}
