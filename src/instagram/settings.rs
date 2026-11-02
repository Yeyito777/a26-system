//! Settings (the profile's ☰): the account and the bridge's state, signing
//! in by importing the session from the Browser, the writes switch (off: the
//! app only reads; nothing is sent, liked, followed or marked seen), the
//! notifications and their previews, and logging out.

use serde_json::json;

use super::app::{Action, Area, Screen};
use super::paint::{
    self, Icon, Line, Paint, Ring, BG, BLUE, BODY, BODY_BOLD, DP, MARGIN, RED, SAFE_BOTTOM, SECONDARY, SEPARATOR,
    SMALL, STATUS, TEXT, TOP_BAR,
};
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::gfx::{Canvas, RectF};

#[derive(Clone, Copy, PartialEq)]
enum Item {
    Account,
    Import,
    Writes,
    Notifications,
    Previews,
    Logout,
}

pub struct SettingsScreen {
    scroll: Scroll,
    rows: Vec<(f32, f32, Item, Vec<Line>)>,
    height: f32,
    generation: u64,
    confirm_logout: bool,
}

const TOGGLE_W: f32 = 52.0 * DP;
const TOGGLE_H: f32 = 32.0 * DP;

impl SettingsScreen {
    pub fn new() -> Self {
        Self {
            scroll: Scroll::default(),
            rows: Vec::new(),
            height: 0.0,
            generation: u64::MAX,
            confirm_logout: false,
        }
    }

    fn note(store: &Store, item: Item) -> &'static str {
        match item {
            Item::Import if store.fixtures => "This bridge runs on made-up data (fixtures).",
            Item::Import => "Sign in to instagram.com in the Browser first; this copies that session to the phone's Instagram bridge. It stays on the phone.",
            Item::Writes => "Off: this app only reads. Nothing is sent, liked, followed, or marked seen or read. On: it acts as Instagram's app does.",
            Item::Notifications => "New messages and activity, through Moon.",
            Item::Previews => "Show message text in notifications.",
            _ => "",
        }
    }
}

fn toggle(canvas: &mut Canvas, x: f32, cy: f32, on: bool) {
    let r = RectF::new(x, cy - TOGGLE_H / 2.0, TOGGLE_W, TOGGLE_H);
    canvas.fill_rrect(r, TOGGLE_H / 2.0, if on { BLUE } else { 0x3a3a3a }, 1.0);
    let knob = if on { r.x + r.w - TOGGLE_H / 2.0 } else { r.x + TOGGLE_H / 2.0 };
    canvas.fill_circle(knob, cy, TOGGLE_H / 2.0 - 6.0, 0xffffff, 1.0);
}

impl Screen for SettingsScreen {
    fn viewport(&self, height: f32) -> (f32, f32) {
        (STATUS + TOP_BAR, height - SAFE_BOTTOM)
    }

    fn scroll(&mut self) -> Option<&mut Scroll> {
        Some(&mut self.scroll)
    }

    fn layout(&mut self, store: &Store, width: f32) -> f32 {
        if self.generation == store.generation {
            return self.height;
        }
        self.generation = store.generation;
        self.rows.clear();
        let mut y = 30.0;
        for item in [Item::Account, Item::Import, Item::Writes, Item::Notifications, Item::Previews, Item::Logout] {
            let note = Self::note(store, item);
            let lines = if note.is_empty() { Vec::new() } else { paint::wrap_plain(note, SMALL, SECONDARY, width - 2.0 * MARGIN - if matches!(item, Item::Writes | Item::Notifications | Item::Previews) { TOGGLE_W + 40.0 } else { 0.0 }, 6).0 };
            let h = match item {
                Item::Account => 64.0 * DP + 60.0,
                _ => 130.0 + lines.len() as f32 * 46.0,
            };
            self.rows.push((y, h, item, lines));
            y += h;
        }
        self.height = y + 100.0;
        self.height
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let base = area.top - area.offset;
        for (y, h, item, lines) in &self.rows {
            let y = base + y;
            if !area.visible(y, y + h) {
                continue;
            }
            let right = area.width - MARGIN;
            match item {
                Item::Account => {
                    let d = 56.0 * DP;
                    let cy = y + d / 2.0 + 20.0;
                    let account = store.account();
                    paint::avatar(canvas, p, account.and_then(|a| a.avatar.as_ref()).map(|m| m.reference.as_str()), MARGIN + d / 2.0, cy, d, Ring::None);
                    let x = MARGIN + d + 34.0;
                    let name = account.map_or("Not signed in".to_owned(), |a| a.name.clone());
                    paint::draw_text(canvas, &name, x, cy - 8.0, BODY_BOLD, TEXT);
                    let state = match store.status.state.as_str() {
                        "online" => account.and_then(|a| a.username.clone()).map_or("Online".to_owned(), |u| format!("@{u} · online")),
                        _ => store.status.detail.clone().unwrap_or_else(|| super::direct::status_note(store)),
                    };
                    paint::draw_text(canvas, &paint::fit(&state, SMALL, right - x), x, cy + 40.0, SMALL, SECONDARY);
                    canvas.fill_rect(0, (y + h - 1.0) as i32, canvas.width, 1, SEPARATOR, 1.0);
                }
                Item::Import => {
                    let label = if store.status.signed_out() { "Import session from Browser" } else { "Import session again" };
                    paint::draw_text(canvas, label, MARGIN, y + 70.0, BODY_BOLD, if store.fixtures { SECONDARY } else { BLUE });
                    if let Some(login) = &store.login {
                        paint::draw_text_right(canvas, &paint::fit(login, SMALL, 360.0), right, y + 70.0, SMALL, SECONDARY);
                    }
                    for (n, line) in lines.iter().enumerate() {
                        paint::draw_line(canvas, line, MARGIN, y + 120.0 + n as f32 * 46.0);
                    }
                }
                Item::Writes | Item::Notifications | Item::Previews => {
                    let (label, on) = match item {
                        Item::Writes => ("Writes", store.settings.writes),
                        Item::Notifications => ("Notifications", store.settings.notifications),
                        _ => ("Message previews", store.settings.previews),
                    };
                    paint::draw_text(canvas, label, MARGIN, y + 70.0, BODY, TEXT);
                    toggle(canvas, right - TOGGLE_W, y + 56.0, on);
                    for (n, line) in lines.iter().enumerate() {
                        paint::draw_line(canvas, line, MARGIN, y + 120.0 + n as f32 * 46.0);
                    }
                }
                Item::Logout => {
                    let label = if self.confirm_logout { "Tap again to log out" } else { "Log out" };
                    paint::draw_text(canvas, label, MARGIN, y + 70.0, BODY_BOLD, RED);
                }
            }
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, _store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        let cy = STATUS + TOP_BAR / 2.0;
        paint::icon(canvas, p.cache, Icon::Back, MARGIN - 10.0, cy - 36.0, 72.0, false, TEXT);
        paint::draw_text(canvas, "Settings", MARGIN + 100.0, paint::baseline(cy, paint::TITLE), paint::TITLE, TEXT);
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            return if x < MARGIN + 110.0 { Action::Pop } else { Action::None };
        }
        let cy = y - area.top + area.offset;
        let Some((_, _, item, _)) = self.rows.iter().find(|(row_y, h, _, _)| cy >= *row_y && cy < row_y + h) else {
            return Action::None;
        };
        let item = *item;
        if item != Item::Logout {
            self.confirm_logout = false;
        }
        match item {
            Item::Import if !store.fixtures => Action::Many(vec![
                Action::Mutate(Box::new(|store: &mut Store| store.login = Some("Importing…".into()))),
                Action::request("login.start", json!({"method": "browser"}), Ask::Login),
            ]),
            Item::Writes => Action::request("settings.set", json!({"writes": !store.settings.writes}), Ask::Settings),
            Item::Notifications => Action::request("settings.set", json!({"notifications": !store.settings.notifications}), Ask::Settings),
            Item::Previews => Action::request("settings.set", json!({"previews": !store.settings.previews}), Ask::Settings),
            Item::Logout => {
                if self.confirm_logout {
                    self.confirm_logout = false;
                    Action::request("logout", json!({}), Ask::Write { what: "logout" })
                } else {
                    self.confirm_logout = true;
                    self.generation = u64::MAX;
                    Action::Redraw
                }
            }
            _ => Action::None,
        }
    }

    fn shown(&mut self, _store: &Store) -> Action {
        Action::request("settings.get", json!({}), Ask::Settings)
    }

    fn tab_bar(&self) -> bool {
        false
    }
}
