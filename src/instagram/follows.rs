//! Followers and following: a profile's lists, the two as tabs, paged as
//! they scroll (a private account the user doesn't follow lists nothing).

use std::time::Instant;

use serde_json::json;

use super::app::{Action, Area, Screen};
use super::paint::{
    self, BG, BODY_BOLD, DP, Icon, MARGIN, Paint, Ring, SAFE_BOTTOM, SECONDARY, SMALL, STATUS,
    TAB_BAR, TEXT, TOP_BAR,
};
use super::profile::ProfileScreen;
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::gfx::Canvas;

const ROW: f32 = 168.0;
const AVATAR: f32 = 54.0 * DP;
const TABS_H: f32 = 120.0;

pub struct FollowsScreen {
    user: String,
    username: String,
    kind: &'static str,
    counts: (u64, u64),
    scroll: Scroll,
}

impl FollowsScreen {
    pub fn new(user: String, username: String, following: bool, counts: (u64, u64)) -> Self {
        Self {
            user,
            username,
            kind: if following { "following" } else { "followers" },
            counts,
            scroll: Scroll::default(),
        }
    }

    fn request(&self, cursor: Option<&str>) -> Action {
        let mut args = json!({"user": self.user, "kind": self.kind});
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        Action::request(
            "instagram.follows",
            args,
            Ask::Follows {
                user: self.user.clone(),
                kind: self.kind,
                more: cursor.is_some(),
            },
        )
    }
}

impl Screen for FollowsScreen {
    fn viewport(&self, height: f32) -> (f32, f32) {
        (STATUS + TOP_BAR + TABS_H, height - SAFE_BOTTOM - TAB_BAR)
    }

    fn scroll(&mut self) -> Option<&mut Scroll> {
        Some(&mut self.scroll)
    }

    fn layout(&mut self, store: &Store, _width: f32) -> f32 {
        let count = store
            .follows
            .get(&(self.user.clone(), self.kind))
            .map_or(0, |list| list.users.len());
        count as f32 * ROW + 200.0
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let base = area.top - area.offset + 10.0;
        let Some(list) = store.follows.get(&(self.user.clone(), self.kind)) else {
            super::direct::spinner(canvas, area.width / 2.0, area.top + 200.0);
            return;
        };
        let first = (((area.band.0 - base) / ROW).floor().max(0.0)) as usize;
        for (index, user) in list.users.iter().enumerate().skip(first) {
            let y = base + index as f32 * ROW;
            if y > area.band.1 {
                break;
            }
            let cy = y + ROW / 2.0;
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
        let end = base + list.users.len() as f32 * ROW;
        if area.visible(end, end + 200.0) {
            if list.loading || list.cursor.is_some() {
                super::direct::spinner(canvas, area.width / 2.0, end + 80.0);
            } else if list.users.is_empty() {
                paint::draw_text_centered(
                    canvas,
                    "No one here yet",
                    area.width / 2.0,
                    end + 100.0,
                    paint::BODY,
                    SECONDARY,
                );
            }
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, _store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        let cy = STATUS + TOP_BAR / 2.0;
        paint::icon(
            canvas,
            p.cache,
            Icon::Back,
            MARGIN - 10.0,
            cy - 36.0,
            72.0,
            false,
            TEXT,
        );
        paint::draw_text(
            canvas,
            &self.username,
            MARGIN + 100.0,
            paint::baseline(cy, paint::TITLE),
            paint::TITLE,
            TEXT,
        );
        let ty = STATUS + TOP_BAR;
        let half = area.width / 2.0;
        for (n, (label, count)) in [("followers", self.counts.0), ("following", self.counts.1)]
            .iter()
            .enumerate()
        {
            let active = *label == self.kind;
            let text = format!("{} {label}", paint::count(*count));
            paint::draw_text_centered(
                canvas,
                &text,
                half * (n as f32 + 0.5),
                ty + 70.0,
                BODY_BOLD,
                if active { TEXT } else { SECONDARY },
            );
            if active {
                canvas.fill_rect(
                    (half * n as f32) as i32,
                    (ty + TABS_H - 4.0) as i32,
                    half as i32,
                    4,
                    TEXT,
                    1.0,
                );
            }
        }
        canvas.fill_rect(
            0,
            (ty + TABS_H - 1.0) as i32,
            canvas.width,
            1,
            0x262626,
            1.0,
        );
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < STATUS + TOP_BAR {
            return if x < MARGIN + 110.0 {
                Action::Pop
            } else {
                Action::None
            };
        }
        if y < area.top {
            let kind = if x < area.width / 2.0 {
                "followers"
            } else {
                "following"
            };
            if kind != self.kind {
                self.kind = kind;
                self.scroll.reset();
                if !store.follows.contains_key(&(self.user.clone(), kind)) {
                    return self.request(None);
                }
            }
            return Action::Redraw;
        }
        let index = ((y - area.top + area.offset - 10.0) / ROW).floor();
        if index < 0.0 {
            return Action::None;
        }
        match store
            .follows
            .get(&(self.user.clone(), self.kind))
            .and_then(|list| list.users.get(index as usize))
        {
            Some(user) => Action::Push(Box::new(ProfileScreen::user(
                user.id.clone(),
                user.username.clone(),
            ))),
            None => Action::None,
        }
    }

    fn tick(&mut self, store: &Store, area: Area, _now: Instant) -> Action {
        if let Some(list) = store.follows.get(&(self.user.clone(), self.kind))
            && !list.loading
            && let Some(cursor) = &list.cursor
            && area.offset > list.users.len() as f32 * ROW - (area.bottom - area.top) - 1500.0
        {
            return self.request(Some(cursor));
        }
        Action::None
    }

    fn shown(&mut self, store: &Store) -> Action {
        if store.follows.contains_key(&(self.user.clone(), self.kind)) {
            return Action::None;
        }
        self.request(None)
    }
}
