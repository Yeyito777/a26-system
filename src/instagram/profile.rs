//! A profile: the avatar (its story ring opens the story), the counts, the
//! name, category, bio and links, who the user follows that follows them,
//! Follow / Message (or the user's own Edit / Share), the highlights, and the
//! grid of posts and the tagged one, in Instagram's 3:4 tiles. A private
//! account the user doesn't follow shows only its header.

use std::time::Instant;

use serde_json::json;

use super::app::{Action, Area, Screen};
use super::model::{Post, Profile};
use super::paint::{
    self, BG, BLUE, BODY, BODY_BOLD, BUTTON, DP, Icon, LINK, Line, MARGIN, Paint, Ring,
    SAFE_BOTTOM, SECONDARY, SEPARATOR, SMALL, STATUS, TAB_BAR, TEXT, TINY, TOP_BAR,
};
use super::post::PostScreen;
use super::settings::SettingsScreen;
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::gfx::{Canvas, RectF};
use crate::social::media::Fit;
use crate::text::Weight;

const AVATAR: f32 = 226.0;
const GAP: f32 = 3.0;
const BUTTON_H: f32 = 89.0;
const HIGHLIGHT: f32 = 168.0;
const TABS_H: f32 = 130.0;

pub struct ProfileScreen {
    own: bool,
    id: Option<String>,
    username: Option<String>,
    scroll: Scroll,
    tagged: bool,
    generation: u64,
    height: f32,
    /// Content y of each part: the buttons, the highlights, the tabs, the grid.
    buttons_y: f32,
    highlights_y: f32,
    tabs_y: f32,
    grid_y: f32,
    bio: Vec<Line>,
    asked_posts: bool,
}

impl ProfileScreen {
    pub fn own() -> Self {
        Self::make(true, None, None)
    }

    pub fn user(id: String, username: Option<String>) -> Self {
        Self::make(false, (!id.is_empty()).then_some(id), username)
    }

    fn make(own: bool, id: Option<String>, username: Option<String>) -> Self {
        Self {
            own,
            id,
            username,
            scroll: Scroll::default(),
            tagged: false,
            generation: u64::MAX,
            height: 0.0,
            buttons_y: 0.0,
            highlights_y: 0.0,
            tabs_y: 0.0,
            grid_y: 0.0,
            bio: Vec::new(),
            asked_posts: false,
        }
    }

    fn user_id(&self, store: &Store) -> Option<String> {
        if self.own {
            return store
                .account()
                .map(|a| a.id.clone())
                .filter(|id| !id.is_empty());
        }
        self.id.clone().or_else(|| {
            self.username
                .as_ref()
                .and_then(|u| store.usernames.get(u).cloned())
        })
    }

    fn profile<'a>(&self, store: &'a Store) -> Option<&'a Profile> {
        self.user_id(store).and_then(|id| store.profiles.get(&id))
    }

    fn posts<'a>(&self, store: &'a Store) -> &'a [Post] {
        let Some(id) = self.user_id(store) else {
            return &[];
        };
        let map = if self.tagged {
            &store.tagged
        } else {
            &store.grids
        };
        map.get(&id).map_or(&[], |grid| grid.posts.as_slice())
    }

    fn tile(width: f32) -> (f32, f32) {
        let w = (width - 2.0 * GAP) / 3.0;
        (w, (w * 4.0 / 3.0).round())
    }

    fn requests(&mut self, store: &Store) -> Action {
        let mut actions = Vec::new();
        match self.user_id(store) {
            Some(id) => {
                if self.own {
                    actions.push(Action::request(
                        "instagram.profile",
                        json!({"user": id, "cached": true}),
                        Ask::Profile {
                            key: id.clone(),
                            cached: true,
                        },
                    ));
                }
                actions.push(Action::request(
                    "instagram.profile",
                    json!({"user": id}),
                    Ask::Profile {
                        key: id.clone(),
                        cached: false,
                    },
                ));
                actions.push(Action::request(
                    "instagram.highlights",
                    json!({"user": id}),
                    Ask::Highlights { user: id.clone() },
                ));
                if !self.asked_posts {
                    self.asked_posts = true;
                    let username = self.username.clone().or_else(|| {
                        store
                            .profiles
                            .get(&id)
                            .and_then(|p| p.user.username.clone())
                    });
                    actions.push(Action::request(
                        "instagram.posts",
                        json!({"user": id, "username": username}),
                        Ask::Posts {
                            user: id.clone(),
                            tagged: false,
                            more: false,
                        },
                    ));
                }
            }
            None => {
                if let Some(username) = &self.username {
                    actions.push(Action::request(
                        "instagram.profile",
                        json!({"username": username}),
                        Ask::Profile {
                            key: username.clone(),
                            cached: false,
                        },
                    ));
                }
            }
        }
        Action::Many(actions)
    }
}

impl Screen for ProfileScreen {
    fn viewport(&self, height: f32) -> (f32, f32) {
        (STATUS + TOP_BAR, height - SAFE_BOTTOM - TAB_BAR)
    }

    fn scroll(&mut self) -> Option<&mut Scroll> {
        Some(&mut self.scroll)
    }

    fn layout(&mut self, store: &Store, width: f32) -> f32 {
        if self.generation == store.generation {
            return self.height;
        }
        self.generation = store.generation;
        let mut y = 40.0 + AVATAR + 30.0;
        self.bio.clear();
        if let Some(profile) = self.profile(store) {
            let mut lines: Vec<Line> = Vec::new();
            lines.push(vec![paint::Run {
                text: profile.user.name.clone(),
                font: BODY_BOLD,
                color: TEXT,
            }]);
            if let Some(category) = &profile.category {
                lines.push(vec![paint::Run {
                    text: category.clone(),
                    font: BODY,
                    color: SECONDARY,
                }]);
            }
            for paragraph in profile.bio.split('\n') {
                lines.extend(
                    paint::wrap_runs(
                        &paint::rich_words(paragraph, BODY, TEXT),
                        width - 2.0 * MARGIN,
                        10,
                    )
                    .0,
                );
            }
            for link in &profile.links {
                let label = if link.title.is_empty() {
                    link.url.clone()
                } else {
                    link.url.trim_start_matches("https://").to_owned()
                };
                lines.push(vec![paint::Run {
                    text: paint::fit(&label, BODY_BOLD, width - 2.0 * MARGIN),
                    font: BODY_BOLD,
                    color: LINK,
                }]);
            }
            if let Some(mutuals) = &profile.mutuals {
                lines.extend(paint::wrap_plain(mutuals, SMALL, TEXT, width - 2.0 * MARGIN, 2).0);
            }
            y += lines.len() as f32 * 50.0;
            self.bio = lines;
        }
        y += 24.0;
        self.buttons_y = y;
        y += BUTTON_H + 40.0;
        self.highlights_y = y;
        let has_highlights = self
            .user_id(store)
            .and_then(|id| store.highlights.get(&id))
            .is_some_and(|h| !h.is_empty());
        if has_highlights {
            y += HIGHLIGHT + 110.0;
        }
        self.tabs_y = y;
        y += TABS_H;
        self.grid_y = y;
        let (tw, th) = Self::tile(width);
        let _ = tw;
        let locked = self.profile(store).is_some_and(|p| p.locked);
        let count = self.posts(store).len();
        y += if locked {
            700.0
        } else {
            (count.div_ceil(3)) as f32 * (th + GAP) + 200.0
        };
        self.height = y;
        y
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let base = area.top - area.offset;
        let width = area.width;
        let profile = self.profile(store);
        let account = store.account().cloned();
        if area.visible(base, base + self.buttons_y) {
            let cy = base + 40.0 + AVATAR / 2.0;
            let (avatar, ring) = match profile {
                Some(profile) => (
                    profile.user.avatar_ref().map(str::to_owned),
                    Ring::from_story(profile.user.story.as_deref()),
                ),
                None if self.own => (
                    account
                        .as_ref()
                        .and_then(|a| a.avatar.as_ref())
                        .map(|m| m.reference.clone()),
                    Ring::None,
                ),
                None => (None, Ring::None),
            };
            paint::avatar(
                canvas,
                p,
                avatar.as_deref(),
                MARGIN + AVATAR / 2.0,
                cy,
                AVATAR,
                ring,
            );
            if let Some(profile) = profile {
                let left = MARGIN + AVATAR + 40.0;
                let col = (width - left - MARGIN) / 3.0;
                for (n, (value, label)) in [
                    (profile.posts, "posts"),
                    (profile.followers, "followers"),
                    (profile.following, "following"),
                ]
                .iter()
                .enumerate()
                {
                    let cx = left + col * (n as f32 + 0.5);
                    paint::draw_text_centered(
                        canvas,
                        &paint::count(*value),
                        cx,
                        cy - 4.0,
                        paint::HEADLINE,
                        TEXT,
                    );
                    paint::draw_text_centered(canvas, label, cx, cy + 46.0, BODY, TEXT);
                }
                let mut ly = base + 40.0 + AVATAR + 30.0;
                for line in &self.bio {
                    paint::draw_line(canvas, line, MARGIN, ly + 38.0);
                    ly += 50.0;
                }
            }
        }
        // Buttons.
        let by = base + self.buttons_y;
        if area.visible(by, by + BUTTON_H) {
            let labels: Vec<(&str, bool)> = if self.own {
                vec![("Edit profile", false), ("Share profile", false)]
            } else {
                match profile.map(|p| p.relation.as_str()) {
                    Some("following") => vec![("Following", false), ("Message", false)],
                    Some("requested") => vec![("Requested", false), ("Message", false)],
                    _ => vec![
                        (
                            if profile.is_some_and(|p| p.followed_by) {
                                "Follow back"
                            } else {
                                "Follow"
                            },
                            true,
                        ),
                        ("Message", false),
                    ],
                }
            };
            let gap = 16.0;
            let bw =
                (width - 2.0 * MARGIN - gap * (labels.len() as f32 - 1.0)) / labels.len() as f32;
            for (n, (label, primary)) in labels.iter().enumerate() {
                let r = RectF::new(MARGIN + n as f32 * (bw + gap), by, bw, BUTTON_H);
                canvas.fill_rrect(r, 8.0 * DP, if *primary { BLUE } else { BUTTON }, 1.0);
                paint::draw_text_centered(
                    canvas,
                    label,
                    r.x + r.w / 2.0,
                    paint::baseline(r.y + r.h / 2.0, BODY_BOLD),
                    BODY_BOLD,
                    TEXT,
                );
            }
        }
        // Highlights.
        let hy = base + self.highlights_y;
        if let Some(highlights) = self.user_id(store).and_then(|id| store.highlights.get(&id))
            && !highlights.is_empty()
            && area.visible(hy, hy + HIGHLIGHT + 110.0)
        {
            let slot = HIGHLIGHT + 50.0;
            for (n, highlight) in highlights.iter().enumerate() {
                let cx = MARGIN + slot / 2.0 - 20.0 + n as f32 * slot;
                if cx - slot > width {
                    break;
                }
                let cy = hy + HIGHLIGHT / 2.0 + 10.0;
                paint::outline(
                    canvas,
                    RectF::new(
                        cx - HIGHLIGHT / 2.0,
                        cy - HIGHLIGHT / 2.0,
                        HIGHLIGHT,
                        HIGHLIGHT,
                    ),
                    HIGHLIGHT / 2.0,
                    3.0,
                    0x3a3a3a,
                    BG,
                );
                paint::avatar(
                    canvas,
                    p,
                    highlight.cover.as_ref().map(|m| m.reference.as_str()),
                    cx,
                    cy,
                    HIGHLIGHT - 16.0,
                    Ring::None,
                );
                let title = paint::fit(&highlight.title, TINY, slot - 10.0);
                paint::draw_text_centered(canvas, &title, cx, hy + HIGHLIGHT + 70.0, TINY, TEXT);
            }
        }
        // Tabs.
        let ty = base + self.tabs_y;
        if area.visible(ty, ty + TABS_H) {
            let half = width / 2.0;
            for (n, icon) in [Icon::Grid, Icon::Tagged].iter().enumerate() {
                let active = (n == 1) == self.tagged;
                let cx = half * (n as f32 + 0.5);
                paint::icon(
                    canvas,
                    p.cache,
                    *icon,
                    cx - 34.0,
                    ty + TABS_H / 2.0 - 34.0,
                    68.0,
                    false,
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
                width as i32,
                1,
                SEPARATOR,
                1.0,
            );
        }
        // Grid.
        let gy = base + self.grid_y;
        if profile.is_some_and(|p| p.locked) {
            if area.visible(gy, gy + 600.0) {
                paint::outline(
                    canvas,
                    RectF::new(width / 2.0 - 90.0, gy + 70.0, 180.0, 180.0),
                    90.0,
                    5.0,
                    TEXT,
                    BG,
                );
                paint::icon(
                    canvas,
                    p.cache,
                    Icon::Lock,
                    width / 2.0 - 45.0,
                    gy + 115.0,
                    90.0,
                    false,
                    TEXT,
                );
                paint::draw_text_centered(
                    canvas,
                    "This account is private",
                    width / 2.0,
                    gy + 330.0,
                    BODY_BOLD,
                    TEXT,
                );
                paint::draw_text_centered(
                    canvas,
                    "Follow to see their photos and videos.",
                    width / 2.0,
                    gy + 390.0,
                    BODY,
                    SECONDARY,
                );
            }
            return;
        }
        let (tw, th) = Self::tile(width);
        let posts = self.posts(store);
        let first_row = (((area.band.0 - gy) / (th + GAP)).floor().max(0.0)) as usize;
        for (index, post) in posts.iter().enumerate().skip(first_row * 3) {
            let row = index / 3;
            let col = index % 3;
            let y = gy + row as f32 * (th + GAP);
            if y > area.band.1 {
                break;
            }
            let x = col as f32 * (tw + GAP);
            let rect = RectF::new(x, y, tw, th);
            if let Some(media) = post.media.first() {
                let reference = media.thumb.as_deref().unwrap_or(&media.reference);
                paint::picture(canvas, p, reference, rect, paint::PLACEHOLDER);
            }
            let icon = if post.pinned {
                Some(Icon::Pin)
            } else if post.media.len() > 1 {
                Some(Icon::Carousel)
            } else if post.video() {
                Some(Icon::Play)
            } else {
                None
            };
            if let Some(icon) = icon {
                paint::icon(
                    canvas,
                    p.cache,
                    icon,
                    x + tw - 66.0,
                    y + 18.0,
                    48.0,
                    true,
                    0xffffff,
                );
            }
        }
        let user = self.user_id(store);
        let grid = user.as_ref().and_then(|id| {
            if self.tagged {
                store.tagged.get(id)
            } else {
                store.grids.get(id)
            }
        });
        let end = gy + posts.len().div_ceil(3) as f32 * (th + GAP);
        if area.visible(end, end + 200.0) {
            if grid.is_none_or(|g| g.loading || !g.loaded) {
                super::direct::spinner(canvas, width / 2.0, end + 100.0);
            } else if posts.is_empty() {
                let note = if self.tagged {
                    "No photos of them yet"
                } else {
                    "No posts yet"
                };
                paint::draw_text_centered(canvas, note, width / 2.0, end + 120.0, BODY, SECONDARY);
            }
        }
        // The next rows' tiles, ahead.
        let next = ((area.bottom - gy) / (th + GAP)).ceil().max(0.0) as usize * 3;
        for post in posts.iter().skip(next).take(9) {
            if let Some(media) = post.media.first() {
                p.prefetch(
                    media.thumb.as_deref().unwrap_or(&media.reference),
                    tw.round() as u32,
                    th.round() as u32,
                    Fit::Cover,
                );
            }
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        let cy = STATUS + TOP_BAR / 2.0;
        let profile = self.profile(store);
        let name = profile
            .and_then(|p| p.user.username.clone())
            .or_else(|| self.username.clone())
            .or_else(|| {
                if self.own {
                    store.account().and_then(|a| a.username.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        if self.own {
            let mut x = MARGIN;
            if profile.is_some_and(|p| p.user.private) {
                paint::icon(canvas, p.cache, Icon::Lock, x, cy - 24.0, 48.0, false, TEXT);
                x += 60.0;
            }
            let w = crate::text::draw(
                canvas,
                &name,
                x,
                paint::baseline(cy, paint::TITLE),
                Weight::Bold,
                54.0,
                TEXT,
                1.0,
            );
            paint::icon(
                canvas,
                p.cache,
                Icon::ChevronDown,
                x + w + 10.0,
                cy - 18.0,
                40.0,
                false,
                TEXT,
            );
            paint::icon(
                canvas,
                p.cache,
                Icon::Menu,
                area.width - MARGIN - 66.0,
                cy - 33.0,
                66.0,
                false,
                TEXT,
            );
        } else {
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
            let x = MARGIN + 100.0;
            let w = paint::draw_text(
                canvas,
                &paint::fit(&name, paint::TITLE, area.width - 400.0),
                x,
                paint::baseline(cy, paint::TITLE),
                paint::TITLE,
                TEXT,
            );
            if profile.is_some_and(|p| p.user.verified) {
                paint::verified(canvas, p.cache, x + w + 14.0, cy, 40.0);
            }
            paint::icon(
                canvas,
                p.cache,
                Icon::More,
                area.width - MARGIN - 60.0,
                cy - 30.0,
                60.0,
                false,
                TEXT,
            );
        }
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            if self.own && x > area.width - MARGIN - 120.0 {
                return Action::Push(Box::new(SettingsScreen::new()));
            }
            if !self.own && x < MARGIN + 110.0 {
                return Action::Pop;
            }
            return Action::None;
        }
        let cy = y - area.top + area.offset;
        let profile = self.profile(store);
        // The counts: followers and following open their lists.
        if cy > 40.0
            && cy < 40.0 + AVATAR
            && x > MARGIN + AVATAR + 40.0
            && let Some(profile) = profile
            && !profile.locked
        {
            let col = (area.width - MARGIN - AVATAR - 40.0 - MARGIN) / 3.0;
            let n = ((x - MARGIN - AVATAR - 40.0) / col).floor() as i32;
            if n == 1 || n == 2 {
                return Action::Push(Box::new(super::follows::FollowsScreen::new(
                    profile.user.id.clone(),
                    profile.user.handle().to_owned(),
                    n == 2,
                    (profile.followers, profile.following),
                )));
            }
            return Action::None;
        }
        if cy < 40.0 + AVATAR + 20.0 && x < MARGIN + AVATAR {
            if let Some(profile) = profile
                && profile.user.story.is_some()
            {
                return Action::Stories {
                    ids: vec![profile.user.id.clone()],
                    start: 0,
                };
            }
            return Action::None;
        }
        if cy >= self.buttons_y && cy < self.buttons_y + BUTTON_H {
            let left = x < area.width / 2.0;
            if self.own {
                return Action::Toast("Editing your profile isn't available here.".into());
            }
            let Some(profile) = profile else {
                return Action::None;
            };
            if left {
                let remove = profile.relation == "following" || profile.relation == "requested";
                return Action::request(
                    "instagram.follow",
                    json!({"user": profile.user.id, "remove": remove}),
                    Ask::Write { what: "follow" },
                );
            }
            return match store.chat_with.get(&profile.user.id) {
                Some(Some(chat)) => Action::Chat(chat.clone()),
                _ => Action::Toast("You don't have a chat with them yet.".into()),
            };
        }
        if cy >= self.highlights_y && cy < self.tabs_y {
            if let Some(highlights) = self.user_id(store).and_then(|id| store.highlights.get(&id)) {
                let slot = HIGHLIGHT + 50.0;
                let index = ((x - MARGIN + 20.0) / slot).floor();
                if index >= 0.0 && (index as usize) < highlights.len() {
                    return Action::Stories {
                        ids: highlights.iter().map(|h| h.id.clone()).collect(),
                        start: index as usize,
                    };
                }
            }
            return Action::None;
        }
        if cy >= self.tabs_y && cy < self.grid_y {
            let tagged = x >= area.width / 2.0;
            if tagged != self.tagged {
                self.tagged = tagged;
                self.generation = u64::MAX;
                if tagged
                    && let Some(id) = self.user_id(store)
                    && !store.tagged.contains_key(&id)
                {
                    return Action::request(
                        "instagram.tagged",
                        json!({"user": id}),
                        Ask::Posts {
                            user: id,
                            tagged: true,
                            more: false,
                        },
                    );
                }
                return Action::Redraw;
            }
            return Action::None;
        }
        if cy >= self.grid_y {
            let (tw, th) = Self::tile(area.width);
            let row = ((cy - self.grid_y) / (th + GAP)).floor() as usize;
            let col = ((x / (tw + GAP)).floor() as usize).min(2);
            if let Some(post) = self.posts(store).get(row * 3 + col) {
                return Action::Push(Box::new(PostScreen::new(post.id.clone(), false)));
            }
        }
        Action::None
    }

    fn tick(&mut self, store: &Store, area: Area, _now: Instant) -> Action {
        // A profile opened by username: its posts once its id is known.
        if !self.asked_posts && self.user_id(store).is_some() {
            return self.requests(store);
        }
        let Some(id) = self.user_id(store) else {
            return Action::None;
        };
        let map = if self.tagged {
            &store.tagged
        } else {
            &store.grids
        };
        if let Some(grid) = map.get(&id)
            && !grid.loading
            && let Some(cursor) = &grid.cursor
            && area.offset > self.height - (area.bottom - area.top) - 2500.0
        {
            let cmd = if self.tagged {
                "instagram.tagged"
            } else {
                "instagram.posts"
            };
            return Action::request(
                cmd,
                json!({"user": id, "cursor": cursor}),
                Ask::Posts {
                    user: id.clone(),
                    tagged: self.tagged,
                    more: true,
                },
            );
        }
        Action::None
    }

    fn shown(&mut self, store: &Store) -> Action {
        let loaded = self.profile(store).is_some();
        if loaded && self.asked_posts {
            return Action::None;
        }
        self.requests(store)
    }
}
