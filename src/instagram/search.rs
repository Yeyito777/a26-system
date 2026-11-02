//! Search: Instagram's field at the top; while empty, the recent searches
//! (shown as Instagram keeps them, nothing registered from here); while
//! typing, accounts and hashtags as they come (a quarter second after the
//! last key). No Explore grid: it is Reels upstream, and this app has none.

use std::time::{Duration, Instant};

use serde_json::json;

use super::app::{Action, Area, Screen};
use super::model::{Hashtag, User};
use super::paint::{
    self, BG, BODY, BODY_BOLD, DP, FIELD, Icon, MARGIN, Paint, Ring, SAFE_BOTTOM, SECONDARY, SMALL,
    STATUS, TAB_BAR, TEXT, TOP_BAR,
};
use super::profile::ProfileScreen;
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::control::KeyboardPurpose;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;

const ROW: f32 = 168.0;
const AVATAR: f32 = 48.0 * DP;
const DEBOUNCE: Duration = Duration::from_millis(250);

enum Row<'a> {
    Header(&'static str),
    User(&'a User),
    Tag(&'a Hashtag),
}

pub struct SearchScreen {
    scroll: Scroll,
    query: String,
    focused: bool,
    typed_at: Option<Instant>,
    asked: String,
    asked_recent: bool,
}

impl SearchScreen {
    pub fn new() -> Self {
        Self {
            scroll: Scroll::default(),
            query: String::new(),
            focused: false,
            typed_at: None,
            asked: String::new(),
            asked_recent: false,
        }
    }

    fn rows<'a>(&self, store: &'a Store) -> Vec<Row<'a>> {
        let mut rows = Vec::new();
        if self.query.trim().is_empty() {
            if !store.search.recent_users.is_empty() || !store.search.recent_tags.is_empty() {
                rows.push(Row::Header("Recent"));
            }
            rows.extend(store.search.recent_users.iter().map(Row::User));
            rows.extend(store.search.recent_tags.iter().map(Row::Tag));
        } else {
            let q = self.query.trim().to_lowercase();
            // What is known matches at once; the bridge's answer replaces it.
            if store.search.query.trim() == self.query.trim() {
                rows.extend(store.search.users.iter().map(Row::User));
                rows.extend(store.search.hashtags.iter().map(Row::Tag));
            } else {
                rows.extend(
                    store
                        .search
                        .users
                        .iter()
                        .filter(|user| {
                            user.handle().to_lowercase().contains(&q)
                                || user.name.to_lowercase().contains(&q)
                        })
                        .map(Row::User),
                );
            }
        }
        rows
    }
}

impl Screen for SearchScreen {
    fn viewport(&self, height: f32) -> (f32, f32) {
        let bottom = if self.focused {
            height
        } else {
            height - SAFE_BOTTOM - TAB_BAR
        };
        (STATUS + TOP_BAR, bottom)
    }

    fn scroll(&mut self) -> Option<&mut Scroll> {
        Some(&mut self.scroll)
    }

    fn layout(&mut self, store: &Store, _width: f32) -> f32 {
        self.rows(store).len() as f32 * ROW + 60.0
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let base = area.top - area.offset + 20.0;
        let rows = self.rows(store);
        for (index, row) in rows.iter().enumerate() {
            let y = base + index as f32 * ROW;
            if y > area.band.1 {
                break;
            }
            if y + ROW < area.band.0 {
                continue;
            }
            let cy = y + ROW / 2.0;
            match row {
                Row::Header(label) => {
                    paint::draw_text(canvas, label, MARGIN, cy + 14.0, paint::HEADLINE, TEXT);
                }
                Row::User(user) => {
                    paint::avatar(
                        canvas,
                        p,
                        user.avatar_ref(),
                        MARGIN + AVATAR / 2.0,
                        cy,
                        AVATAR,
                        Ring::from_story(user.story.as_deref()),
                    );
                    let x = MARGIN + AVATAR + 32.0;
                    let w = paint::draw_text(canvas, user.handle(), x, cy - 6.0, BODY_BOLD, TEXT);
                    if user.verified {
                        paint::verified(canvas, p.cache, x + w + 10.0, cy - 19.0, 34.0);
                    }
                    paint::draw_text(
                        canvas,
                        &paint::fit(&user.name, SMALL, area.width - x - MARGIN),
                        x,
                        cy + 40.0,
                        SMALL,
                        SECONDARY,
                    );
                }
                Row::Tag(tag) => {
                    canvas.stroke_circle(
                        MARGIN + AVATAR / 2.0,
                        cy,
                        AVATAR / 2.0 - 2.0,
                        3.0,
                        0x363636,
                        1.0,
                    );
                    paint::icon(
                        canvas,
                        p.cache,
                        Icon::Hashtag,
                        MARGIN + AVATAR / 2.0 - 28.0,
                        cy - 28.0,
                        56.0,
                        false,
                        TEXT,
                    );
                    let x = MARGIN + AVATAR + 32.0;
                    paint::draw_text(
                        canvas,
                        &format!("#{}", tag.name),
                        x,
                        cy - 6.0,
                        BODY_BOLD,
                        TEXT,
                    );
                    paint::draw_text(
                        canvas,
                        &format!("{} posts", paint::count(tag.posts)),
                        x,
                        cy + 40.0,
                        SMALL,
                        SECONDARY,
                    );
                }
            }
        }
        if rows.is_empty()
            && !self.query.trim().is_empty()
            && !store.search.loading
            && store.search.query == self.query
        {
            paint::draw_text_centered(
                canvas,
                &format!("No results for \"{}\"", self.query.trim()),
                area.width / 2.0,
                area.top + 200.0,
                BODY,
                SECONDARY,
            );
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, _store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        let h = 38.0 * DP;
        let field = RectF::new(
            MARGIN,
            STATUS + (TOP_BAR - h) / 2.0,
            area.width - 2.0 * MARGIN - if self.focused { 170.0 } else { 0.0 },
            h,
        );
        canvas.fill_rrect(field, 10.0 * DP, FIELD, 1.0);
        paint::icon(
            canvas,
            p.cache,
            Icon::Search,
            field.x + 30.0,
            field.y + (h - 44.0) / 2.0,
            44.0,
            false,
            SECONDARY,
        );
        let tx = field.x + 96.0;
        let by = paint::baseline(field.y + h / 2.0, BODY);
        if self.query.is_empty() {
            paint::draw_text(canvas, "Search", tx, by, BODY, SECONDARY);
        } else {
            let shown = paint::fit(&self.query, BODY, field.w - 140.0);
            paint::draw_text(canvas, &shown, tx, by, BODY, TEXT);
        }
        if self.focused {
            let caret =
                tx + paint::measure(&paint::fit(&self.query, BODY, field.w - 140.0), BODY) + 3.0;
            canvas.fill_rect(
                caret as i32,
                (field.y + 22.0) as i32,
                3,
                (h - 44.0) as i32,
                paint::BLUE,
                1.0,
            );
            paint::draw_text_right(canvas, "Cancel", area.width - MARGIN, by, BODY, TEXT);
        }
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            if self.focused && x > area.width - MARGIN - 170.0 {
                self.focused = false;
                self.query.clear();
                return Action::Keyboard(None);
            }
            self.focused = true;
            return Action::Keyboard(Some(KeyboardPurpose::Search));
        }
        let index = ((y - area.top + area.offset - 20.0) / ROW).floor();
        if index < 0.0 {
            return Action::None;
        }
        match self.rows(store).get(index as usize) {
            Some(Row::User(user)) => Action::Push(Box::new(ProfileScreen::user(
                user.id.clone(),
                user.username.clone(),
            ))),
            Some(Row::Tag(_)) => {
                Action::Toast("Hashtag pages aren't here: they're mostly Reels.".into())
            }
            _ => Action::None,
        }
    }

    fn key(&mut self, _store: &Store, key: KeyInput) -> Action {
        match key {
            KeyInput::Character(c) => self.query.push(c),
            KeyInput::Backspace => {
                self.query.pop();
            }
            KeyInput::Enter => {
                self.typed_at = Some(Instant::now() - DEBOUNCE);
                return Action::Redraw;
            }
        }
        self.typed_at = Some(Instant::now());
        self.scroll.reset();
        Action::Redraw
    }

    fn keyboard_hidden(&mut self) {
        self.focused = false;
    }

    fn tick(&mut self, _store: &Store, _area: Area, now: Instant) -> Action {
        if let Some(at) = self.typed_at
            && now.duration_since(at) >= DEBOUNCE
        {
            self.typed_at = None;
            let query = self.query.trim().to_owned();
            if !query.is_empty() && query != self.asked {
                self.asked = query.clone();
                return Action::Many(vec![
                    Action::Mutate(Box::new({
                        let query = self.query.clone();
                        move |store: &mut Store| {
                            store.search.query = query;
                            store.search.loading = true;
                        }
                    })),
                    Action::request(
                        "instagram.search",
                        json!({"query": query}),
                        Ask::Search {
                            query: self.query.clone(),
                        },
                    ),
                ]);
            }
        }
        Action::None
    }

    fn wake(&self) -> Option<Instant> {
        self.typed_at.map(|at| at + DEBOUNCE)
    }

    fn shown(&mut self, _store: &Store) -> Action {
        if self.asked_recent {
            return Action::None;
        }
        self.asked_recent = true;
        Action::request("instagram.recent", json!({}), Ask::Recent)
    }

    fn tab_bar(&self) -> bool {
        !self.focused
    }
}
