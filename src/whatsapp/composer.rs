//! The bottom of a chat: WhatsApp's message field and its round send (or
//! mic) button, a reply being written above it; or, while writes are off,
//! a bar that says why nothing can be sent from here.

use super::icons::{self, Glyph};
use super::{Page, WhatsApp, theme};
use crate::exocortex::ui::icons::Icon;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::social::shell::Cx;
use crate::text::{self, Weight};

const FIELD_SIZE: f32 = 44.0;
const FIELD_LINE: f32 = 58.0;
const MAX_LINES: usize = 6;
const BUTTON: f32 = 124.0;
const REPLY_H: f32 = 132.0;
const READ_ONLY_H: f32 = 150.0;

#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub id: String,
    pub author: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Composer {
    pub text: String,
    pub reply: Option<Reply>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Field,
    Send,
    CancelReply,
    ReadOnly,
}

impl Composer {
    pub fn clear(&mut self) {
        self.text.clear();
        self.reply = None;
    }

    /// A key from Moon's keyboard; true when it asks to send (never: Return
    /// is a new line, as in WhatsApp; the button sends).
    pub fn key(&mut self, key: KeyInput) -> bool {
        match key {
            KeyInput::Character(c) => self.text.push(c),
            KeyInput::Enter => self.text.push('\n'),
            KeyInput::Backspace => {
                // A whole emoji sequence goes at once: its modifiers, and
                // what a joiner ties it to.
                while let Some(c) = self.text.pop() {
                    if c == '\u{fe0f}'
                        || c == '\u{200d}'
                        || ('\u{1f3fb}'..='\u{1f3ff}').contains(&c)
                    {
                        continue;
                    }
                    if self.text.ends_with('\u{200d}') {
                        self.text.pop();
                        continue;
                    }
                    break;
                }
            }
        }
        false
    }
}

fn keyboard_up(height: f32) -> bool {
    height < f32::from(crate::REQUESTED_HEIGHT) - 200.0
}

fn bottom_inset(height: f32) -> f32 {
    if keyboard_up(height) {
        14.0
    } else {
        theme::SAFE_BOTTOM
    }
}

/// The field's text as laid out (lines), within its width.
fn lines(app: &WhatsApp, width: f32) -> Vec<String> {
    let inner = field_rect(width, 0.0, 1).w - 140.0;
    let laid = super::rich::layout(
        &app.composer.text,
        inner,
        FIELD_SIZE,
        FIELD_LINE,
        Weight::Regular,
    );
    laid.lines
        .iter()
        .map(|line| line.runs.iter().map(|r| r.text.as_str()).collect())
        .collect()
}

fn field_rect(width: f32, bottom: f32, lines: usize) -> RectF {
    let h = (lines.clamp(1, MAX_LINES) as f32 * FIELD_LINE + 60.0).max(BUTTON);
    RectF {
        x: 20.0,
        y: bottom - h,
        w: width - 40.0 - BUTTON - 14.0,
        h,
    }
}

/// How much of the window's bottom the composer takes.
pub fn height(app: &WhatsApp, height: f32) -> f32 {
    if app.page != Page::Chat {
        return 0.0;
    }
    if !app.settings.writes {
        return READ_ONLY_H + bottom_inset(height);
    }
    let lines = lines(app, f32::from(crate::REQUESTED_WIDTH)).len();
    let field = field_rect(f32::from(crate::REQUESTED_WIDTH), 0.0, lines);
    let reply = if app.composer.reply.is_some() {
        REPLY_H
    } else {
        0.0
    };
    field.h + reply + 26.0 + bottom_inset(height)
}

/// What the composer shows (the band above it redraws whole when it changes).
pub fn key(app: &WhatsApp) -> u64 {
    let mut parts = vec![
        app.composer.text.len() as u64,
        u64::from(app.composer.reply.is_some()),
    ];
    parts.extend(app.composer.text.bytes().map(u64::from));
    super::band::key_of(&parts)
}

pub fn hit(app: &WhatsApp, x: f32, y: f32, width: f32, height: f32) -> Option<Hit> {
    if !app.settings.writes {
        return Some(Hit::ReadOnly);
    }
    let bottom = height - bottom_inset(height);
    let field = field_rect(width, bottom, lines(app, width).len());
    if x > field.x + field.w {
        return Some(Hit::Send);
    }
    if app.composer.reply.is_some() && y < field.y && x > width - 160.0 {
        return Some(Hit::CancelReply);
    }
    Some(Hit::Field)
}

pub fn tap(app: &mut WhatsApp, cx: &mut Cx, hit: Hit) {
    match hit {
        Hit::ReadOnly => app.go(cx, Page::Settings),
        Hit::Field => {
            if !cx.keyboard_shown() {
                cx.show_keyboard(crate::control::KeyboardPurpose::Multiline);
            }
        }
        Hit::Send => {
            if app.composer.text.trim().is_empty() {
                app.toast(cx, "Voice messages can't be recorded here");
            } else {
                app.send(cx);
            }
        }
        Hit::CancelReply => app.composer.reply = None,
    }
    cx.redraw();
}

pub fn draw(app: &WhatsApp, canvas: &mut Canvas, width: f32, height: f32) {
    let area = self::height(app, height);
    let top = height - area;
    canvas.fill_rect(
        0,
        top as i32,
        canvas.width,
        area.ceil() as i32 + 1,
        theme::BG,
        1.0,
    );
    if !app.settings.writes {
        let rect = RectF {
            x: 20.0,
            y: top + 14.0,
            w: width - 40.0,
            h: READ_ONLY_H - 28.0,
        };
        canvas.fill_rrect(rect, 30.0, theme::CHIP, 1.0);
        icons::draw(
            canvas,
            Glyph::Lock,
            rect.x + 60.0,
            rect.y + rect.h / 2.0,
            48.0,
            theme::SECONDARY,
            1.0,
        );
        text::draw(
            canvas,
            "Writes are off on this phone",
            rect.x + 116.0,
            rect.y + 52.0,
            Weight::Medium,
            36.0,
            theme::TEXT,
            1.0,
        );
        text::draw(
            canvas,
            "Turn them on in Settings to send",
            rect.x + 116.0,
            rect.y + 98.0,
            Weight::Regular,
            32.0,
            theme::SECONDARY,
            1.0,
        );
        return;
    }
    let bottom = height - bottom_inset(height);
    let lines = lines(app, width);
    let field = field_rect(width, bottom, lines.len());
    if let Some(reply) = &app.composer.reply {
        let rect = RectF {
            x: field.x,
            y: field.y - REPLY_H,
            w: field.w,
            h: REPLY_H + 40.0,
        };
        canvas.fill_rrect(rect, 36.0, theme::SURFACE, 1.0);
        let q = RectF {
            x: rect.x + 14.0,
            y: rect.y + 14.0,
            w: rect.w - 28.0,
            h: REPLY_H - 22.0,
        };
        canvas.fill_rrect(q, 16.0, 0x000000, 0.22);
        let color = if reply.author == "You" {
            theme::TEAL
        } else {
            theme::name_color(&reply.author)
        };
        canvas.fill_rrect(
            RectF {
                x: q.x,
                y: q.y,
                w: 9.0,
                h: q.h,
            },
            4.0,
            color,
            1.0,
        );
        let author = text::fit(&reply.author, Weight::SemiBold, 34.0, q.w - 120.0);
        text::draw(
            canvas,
            &author,
            q.x + 30.0,
            q.y + 44.0,
            Weight::SemiBold,
            34.0,
            color,
            1.0,
        );
        let quoted = text::fit(&reply.text, Weight::Regular, 34.0, q.w - 120.0);
        text::draw(
            canvas,
            &quoted,
            q.x + 30.0,
            q.y + 90.0,
            Weight::Regular,
            34.0,
            theme::SECONDARY,
            1.0,
        );
        crate::exocortex::ui::icons::draw(
            canvas,
            Icon::X,
            q.x + q.w - 50.0,
            q.y + q.h / 2.0,
            44.0,
            theme::SECONDARY,
            1.0,
        );
    }
    canvas.fill_rrect(field, BUTTON / 2.0, theme::SURFACE, 1.0);
    icons::draw(
        canvas,
        Glyph::Smiley,
        field.x + 66.0,
        field.y + field.h - BUTTON / 2.0,
        58.0,
        theme::SECONDARY,
        1.0,
    );
    let text_x = field.x + 124.0;
    let shown = lines.len().min(MAX_LINES);
    let first = lines.len() - shown;
    if app.composer.text.is_empty() {
        text::draw(
            canvas,
            "Message",
            text_x,
            field.y + field.h / 2.0 + FIELD_SIZE * 0.36,
            Weight::Regular,
            FIELD_SIZE,
            theme::SECONDARY,
            1.0,
        );
    } else {
        for (n, line) in lines.iter().skip(first).enumerate() {
            text::draw(
                canvas,
                line,
                text_x,
                field.y + 30.0 + FIELD_SIZE * 0.9 + n as f32 * FIELD_LINE,
                Weight::Regular,
                FIELD_SIZE,
                theme::TEXT,
                1.0,
            );
        }
    }
    if keyboard_up(height) {
        let last = lines.last().map_or("", String::as_str);
        let w = if app.composer.text.is_empty() {
            0.0
        } else {
            text::measure(last, Weight::Regular, FIELD_SIZE)
        };
        let row = if app.composer.text.is_empty() {
            0
        } else {
            shown - 1
        };
        let caret_y = if app.composer.text.is_empty() {
            field.y + field.h / 2.0 - 28.0
        } else {
            field.y + 30.0 + row as f32 * FIELD_LINE + 4.0
        };
        canvas.fill_rect(
            (text_x + w + 2.0) as i32,
            caret_y as i32,
            4,
            56,
            theme::GREEN,
            1.0,
        );
    }
    let bx = field.x + field.w + 14.0 + BUTTON / 2.0;
    let by = bottom - BUTTON / 2.0;
    canvas.fill_circle(bx, by, BUTTON / 2.0, theme::GREEN, 1.0);
    if app.composer.text.trim().is_empty() {
        crate::exocortex::ui::icons::draw(canvas, Icon::Mic, bx, by, 60.0, theme::ON_GREEN, 1.0);
    } else {
        icons::draw(
            canvas,
            Glyph::Send,
            bx + 4.0,
            by,
            58.0,
            theme::ON_GREEN,
            1.0,
        );
    }
}
