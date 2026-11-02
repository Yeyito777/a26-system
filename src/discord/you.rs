//! The You tab: the signed-in account, the bridge's state, and its
//! settings: the writes switch (off until the user turns it on; asks first),
//! notifications and their previews; signing in with a token and out.

#![allow(dead_code)]

use std::time::Instant;

use crate::exocortex::gesture::Gesture;
use crate::exocortex::ui::icons::{self, Icon};
use crate::gfx::{Canvas, RectF};
use crate::social::media::Media;
use crate::text::{self, Weight};

use super::Action;
use super::model::Model;
use super::theme::{self, dp};
use super::ui;

const ROW: f32 = dp(56.0);

pub struct YouView {
    /// The writes switch is being turned on: the confirmation is up.
    pub confirm_writes: bool,
    pub confirm_logout: bool,
    /// The last login progress line.
    pub login: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Item {
    Writes,
    Notifications,
    Previews,
    SignIn,
    SignOut,
}

impl YouView {
    pub fn new() -> Self {
        Self {
            confirm_writes: false,
            confirm_logout: false,
            login: None,
        }
    }

    fn items(model: &Model) -> Vec<(Item, f32)> {
        let mut items = Vec::new();
        let mut y = theme::TOP + dp(196.0);
        if model.status.state == "signed_out" {
            items.push((Item::SignIn, y));
            return items;
        }
        y += dp(36.0);
        for item in [Item::Writes, Item::Notifications, Item::Previews] {
            items.push((item, y));
            y += ROW;
        }
        if !model.fixtures {
            y += dp(40.0);
            items.push((Item::SignOut, y));
        }
        items
    }

    pub fn gesture(
        &mut self,
        gesture: Gesture,
        model: &Model,
        width: f32,
        height: f32,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let Gesture::Tap { x, y } = gesture else {
            if matches!(gesture, Gesture::Back) && (self.confirm_writes || self.confirm_logout) {
                self.confirm_writes = false;
                self.confirm_logout = false;
                actions.push(Action::Redraw);
            }
            return actions;
        };
        if self.confirm_writes || self.confirm_logout {
            let (card, yes, no) = confirm_geometry(width, height);
            if yes.contains(x, y) {
                if self.confirm_writes {
                    actions.push(Action::SetWrites(true));
                } else {
                    actions.push(Action::Logout);
                }
            } else if !no.contains(x, y) && card.contains(x, y) {
                return actions;
            }
            self.confirm_writes = false;
            self.confirm_logout = false;
            actions.push(Action::Redraw);
            return actions;
        }
        for (item, top) in Self::items(model) {
            if y < top || y > top + ROW {
                continue;
            }
            match item {
                Item::Writes => {
                    if model.settings.writes {
                        actions.push(Action::SetWrites(false));
                    } else {
                        self.confirm_writes = true;
                        actions.push(Action::Redraw);
                    }
                }
                Item::Notifications => {
                    actions.push(Action::SetNotifications(!model.settings.notifications))
                }
                Item::Previews => actions.push(Action::SetPreviews(!model.settings.previews)),
                Item::SignIn => actions.push(Action::LoginFromClipboard),
                Item::SignOut => {
                    self.confirm_logout = true;
                    actions.push(Action::Redraw);
                }
            }
        }
        actions
    }

    pub fn draw(&self, canvas: &mut Canvas, model: &Model, media: &mut Media, now: Instant) {
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        canvas.fill_rect(0, 0, width as i32, height as i32, theme::RAIL, 1.0);
        // The account: a banner, the avatar over it, the names.
        let top = theme::TOP;
        canvas.fill_rect(
            0,
            top as i32,
            width as i32,
            dp(70.0) as i32,
            theme::BLURPLE,
            1.0,
        );
        let account = model.status.account.as_ref();
        let size = dp(84.0);
        let ax = theme::MARGIN;
        let ay = top + dp(70.0) - size / 2.0;
        canvas.fill_circle(
            ax + size / 2.0,
            ay + size / 2.0,
            size / 2.0 + dp(5.0),
            theme::RAIL,
            1.0,
        );
        ui::avatar(
            canvas,
            media,
            account.and_then(|a| a.avatar.as_ref()),
            ax,
            ay,
            size,
            0x41434a,
            now,
        );
        let (dot, state) = match model.status.state.as_str() {
            "online" => (theme::GREEN, "Connected"),
            "connecting" => (theme::YELLOW, "Connecting…"),
            "offline" => (0x80848e, "Offline"),
            "signed_out" => (0x80848e, "Not signed in"),
            _ => (theme::RED, "Needs attention"),
        };
        canvas.fill_circle(
            ax + size - dp(8.0),
            ay + size - dp(8.0),
            dp(11.0),
            theme::RAIL,
            1.0,
        );
        canvas.fill_circle(ax + size - dp(8.0), ay + size - dp(8.0), dp(7.5), dot, 1.0);
        let name = account.map_or("Discord", |a| a.name.as_str());
        text::draw(
            canvas,
            &ui::fit(name, Weight::Bold, theme::T_H2, width - 2.0 * theme::MARGIN),
            theme::MARGIN,
            ay + size + dp(32.0),
            Weight::Bold,
            theme::T_H2,
            theme::HEADER,
            1.0,
        );
        let mut detail = account.and_then(|a| a.username.clone()).unwrap_or_default();
        if model.fixtures {
            detail = format!("{detail} · made-up account (fixtures)");
        }
        let line = if detail.is_empty() {
            state.to_owned()
        } else {
            format!("{detail} · {state}")
        };
        text::draw(
            canvas,
            &ui::fit(
                &line,
                Weight::Regular,
                theme::T_SMALL,
                width - 2.0 * theme::MARGIN,
            ),
            theme::MARGIN,
            ay + size + dp(56.0),
            Weight::Regular,
            theme::T_SMALL,
            theme::INTERACTIVE,
            1.0,
        );
        if let Some(detail) = &model.status.detail {
            text::draw(
                canvas,
                &ui::fit(
                    detail,
                    Weight::Regular,
                    theme::T_TINY,
                    width - 2.0 * theme::MARGIN,
                ),
                theme::MARGIN,
                ay + size + dp(76.0),
                Weight::Regular,
                theme::T_TINY,
                theme::MUTED,
                1.0,
            );
        }
        let items = Self::items(model);
        if model.status.state == "signed_out" {
            let (_, y) = items[0];
            let card = RectF {
                x: dp(12.0),
                y,
                w: width - dp(24.0),
                h: ROW,
            };
            canvas.fill_rrect(card, dp(12.0), theme::BLURPLE, 1.0);
            text::draw_centered(
                canvas,
                "Sign in with a token from the clipboard",
                width / 2.0,
                text::centered_baseline(card.y + card.h / 2.0, Weight::SemiBold, theme::T_BODY),
                Weight::SemiBold,
                theme::T_BODY,
                theme::WHITE,
                1.0,
            );
            let note = self.login.clone().unwrap_or_else(|| "Copy your Discord token, then tap. It stays on this phone, in the bridge's private state.".into());
            text::draw(
                canvas,
                &ui::fit(&note, Weight::Regular, theme::T_TINY, width - dp(32.0)),
                theme::MARGIN,
                y + ROW + dp(26.0),
                Weight::Regular,
                theme::T_TINY,
                theme::MUTED,
                1.0,
            );
        } else {
            let first = items[0].1;
            text::draw_tracked(
                canvas,
                "BRIDGE",
                theme::MARGIN,
                first - dp(12.0),
                Weight::SemiBold,
                theme::T_TINY,
                0.6,
                theme::MUTED,
                1.0,
            );
            let card = RectF {
                x: dp(12.0),
                y: first,
                w: width - dp(24.0),
                h: ROW * 3.0,
            };
            canvas.fill_rrect(card, dp(12.0), theme::LIST, 1.0);
            for (i, (item, y)) in items.iter().enumerate() {
                let cy = y + ROW / 2.0;
                let (icon, label, sub, on) = match item {
                    Item::Writes => (
                        Icon::Pencil,
                        "Allow sending",
                        if model.settings.writes {
                            "Messages, reactions and read receipts go to Discord"
                        } else {
                            "Off: nothing is sent, marked read or reacted"
                        },
                        model.settings.writes,
                    ),
                    Item::Notifications => (
                        Icon::Bell,
                        "Notifications",
                        "DMs and mentions",
                        model.settings.notifications,
                    ),
                    Item::Previews => (
                        Icon::MessageSquare,
                        "Message previews",
                        "Show the text in notifications",
                        model.settings.previews,
                    ),
                    Item::SignOut => {
                        let out = RectF {
                            x: dp(12.0),
                            y: *y,
                            w: width - dp(24.0),
                            h: ROW,
                        };
                        canvas.fill_rrect(out, dp(12.0), theme::LIST, 1.0);
                        text::draw(
                            canvas,
                            "Sign out",
                            out.x + dp(16.0),
                            text::centered_baseline(cy, Weight::Medium, theme::T_BODY),
                            Weight::Medium,
                            theme::T_BODY,
                            theme::RED,
                            1.0,
                        );
                        continue;
                    }
                    Item::SignIn => continue,
                };
                icons::draw(
                    canvas,
                    icon,
                    card.x + dp(24.0),
                    cy,
                    dp(20.0),
                    theme::INTERACTIVE,
                    1.0,
                );
                text::draw(
                    canvas,
                    label,
                    card.x + dp(48.0),
                    cy - dp(3.0),
                    Weight::Medium,
                    theme::T_BODY,
                    theme::HEADER,
                    1.0,
                );
                text::draw(
                    canvas,
                    &ui::fit(sub, Weight::Regular, theme::T_TINY, card.w - dp(120.0)),
                    card.x + dp(48.0),
                    cy + dp(14.0),
                    Weight::Regular,
                    theme::T_TINY,
                    theme::MUTED,
                    1.0,
                );
                let enabled = !(matches!(item, Item::Previews) && !model.settings.notifications);
                ui::switch(canvas, on, card.x + card.w - dp(16.0), cy, enabled);
                if i < 2 {
                    canvas.fill_rect(
                        (card.x + dp(48.0)) as i32,
                        (y + ROW) as i32,
                        (card.w - dp(48.0)) as i32,
                        2,
                        theme::DIVIDER,
                        1.0,
                    );
                }
            }
        }
        if self.confirm_writes || self.confirm_logout {
            let (card, yes, no) = confirm_geometry(width, height);
            canvas.fill_rect(0, 0, width as i32, height as i32, 0x000000, 0.6);
            canvas.fill_rrect(card, dp(12.0), theme::CHAT, 1.0);
            let (title, body, confirm) = if self.confirm_writes {
                (
                    "Allow sending?",
                    "This app will send messages, reactions and read receipts to Discord as you. Turn it off here at any time.",
                    "Allow",
                )
            } else {
                (
                    "Sign out?",
                    "The bridge forgets this phone's Discord session.",
                    "Sign out",
                )
            };
            text::draw(
                canvas,
                title,
                card.x + dp(20.0),
                card.y + dp(40.0),
                Weight::Bold,
                theme::T_TITLE,
                theme::HEADER,
                1.0,
            );
            let mut ty = card.y + dp(70.0);
            for line in wrap(body, Weight::Regular, theme::T_SMALL, card.w - dp(40.0)) {
                text::draw(
                    canvas,
                    &line,
                    card.x + dp(20.0),
                    ty,
                    Weight::Regular,
                    theme::T_SMALL,
                    theme::TEXT,
                    1.0,
                );
                ty += theme::L_SMALL;
            }
            canvas.fill_rrect(no, dp(8.0), theme::FIELD, 1.0);
            text::draw_centered(
                canvas,
                "Cancel",
                no.x + no.w / 2.0,
                text::centered_baseline(no.y + no.h / 2.0, Weight::Medium, theme::T_BODY),
                Weight::Medium,
                theme::T_BODY,
                theme::HEADER,
                1.0,
            );
            canvas.fill_rrect(
                yes,
                dp(8.0),
                if self.confirm_writes {
                    theme::BLURPLE
                } else {
                    theme::RED
                },
                1.0,
            );
            text::draw_centered(
                canvas,
                confirm,
                yes.x + yes.w / 2.0,
                text::centered_baseline(yes.y + yes.h / 2.0, Weight::Medium, theme::T_BODY),
                Weight::Medium,
                theme::T_BODY,
                theme::WHITE,
                1.0,
            );
        }
    }
}

impl Default for YouView {
    fn default() -> Self {
        Self::new()
    }
}

fn confirm_geometry(width: f32, height: f32) -> (RectF, RectF, RectF) {
    let card = RectF {
        x: dp(24.0),
        y: height / 2.0 - dp(110.0),
        w: width - dp(48.0),
        h: dp(220.0),
    };
    let button_w = (card.w - dp(52.0)) / 2.0;
    let no = RectF {
        x: card.x + dp(20.0),
        y: card.y + card.h - dp(64.0),
        w: button_w,
        h: dp(44.0),
    };
    let yes = RectF {
        x: no.x + button_w + dp(12.0),
        ..no
    };
    (card, yes, no)
}

pub fn wrap(value: &str, weight: Weight, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in value.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && text::measure(&candidate, weight, size) > width {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
