//! Home: Instagram's top bar (the wordmark, the heart), the stories tray and
//! the feed of posts from accounts the user follows (the bridge leaves ads,
//! suggestions and reel recommendations out). A post is laid out once
//! (header, media at its clamped shape, actions, likes, caption, comments
//! link, age) and drawn from that; carousels page sideways under the finger.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::json;

use super::activity::ActivityScreen;
use super::app::{Action, Area, Screen};
use super::model::{Post, ReelSummary};
use super::paint::{
    self, Icon, Line, Paint, Ring, BG, BLUE, BODY, BODY_BOLD, DP, MARGIN, SAFE_BOTTOM, SECONDARY, SEPARATOR,
    SMALL, STATUS, TAB_BAR, TEXT, TINY, TOP_BAR,
};
use super::post::PostScreen;
use super::profile::ProfileScreen;
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::exocortex::gesture::Gesture;
use crate::gfx::{Canvas, RectF};
use crate::social::media::Fit;
use crate::text::Weight;

pub const TRAY_H: f32 = 112.0 * DP;
const TRAY_RING: f32 = 184.0;
const TRAY_SLOT: f32 = 220.0;
const HEADER_H: f32 = 56.0 * DP;
const ACTIONS_H: f32 = 121.0;
const ICON: f32 = 71.0;
const LINE: f32 = 50.0;

/// One post laid out at a width.
#[derive(Debug, Clone)]
pub struct PostLayout {
    pub signature: u64,
    pub media_h: f32,
    pub caption: Vec<Line>,
    pub truncated: bool,
    pub height: f32,
}

fn signature(post: &Post, expanded: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    post.id.hash(&mut h);
    post.caption.hash(&mut h);
    post.likes.hash(&mut h);
    post.comments.hash(&mut h);
    post.liked.hash(&mut h);
    post.saved.hash(&mut h);
    post.author.story.hash(&mut h);
    expanded.hash(&mut h);
    h.finish()
}

pub fn lay_post(post: &Post, width: f32, expanded: bool) -> PostLayout {
    let media_h = (width / post.aspect()).round();
    let mut words = vec![(post.author.handle().to_owned(), paint::BODY_BOLD, TEXT)];
    words.extend(paint::rich_words(&post.caption, BODY, TEXT));
    let (caption, truncated) = if post.caption.is_empty() {
        (Vec::new(), false)
    } else {
        paint::wrap_runs(&words, width - 2.0 * MARGIN, if expanded { 400 } else { 2 })
    };
    let mut height = HEADER_H + media_h + ACTIONS_H;
    if post.likes.is_some() || post.liked_by.is_some() {
        height += LINE;
    }
    height += caption.len() as f32 * LINE;
    if post.comments > 0 {
        height += LINE;
    }
    height += 60.0 + 30.0;
    PostLayout {
        signature: signature(post, expanded),
        media_h,
        caption,
        truncated,
        height,
    }
}

fn likes_line(post: &Post) -> Option<String> {
    match (post.likes, &post.liked_by) {
        (Some(n), _) if n > 0 => Some(format!("{} like{}", paint::count(n), if n == 1 { "" } else { "s" })),
        (_, Some(by)) => Some(format!("Liked by {by} and others")),
        _ => None,
    }
}

/// Draws a post with its top at `y`; `index` is the carousel item shown,
/// `drag` how far it is dragged sideways.
#[allow(clippy::too_many_arguments)]
pub fn draw_post(canvas: &mut Canvas, p: &mut Paint, post: &Post, layout: &PostLayout, y: f32, width: f32, index: usize, drag: f32, band: (f32, f32)) {
    let now = paint::now_ms();
    // Header.
    if y + HEADER_H > band.0 && y < band.1 {
        let cy = y + HEADER_H / 2.0;
        let d = 38.0 * DP;
        let ring = Ring::from_story(post.author.story.as_deref());
        paint::avatar(canvas, p, post.author.avatar_ref(), MARGIN + d / 2.0 - 6.0, cy, d, ring);
        let x = MARGIN + d + 18.0;
        let name = post.author.handle();
        let w = paint::draw_text(canvas, name, x, if post.location.is_some() { cy - 4.0 } else { cy + 13.0 }, BODY_BOLD, TEXT);
        if post.author.verified {
            paint::verified(canvas, p.cache, x + w + 10.0, if post.location.is_some() { cy - 17.0 } else { cy }, 34.0);
        }
        if let Some(location) = &post.location {
            paint::draw_text(canvas, &paint::fit(location, TINY, width - x - 140.0), x, cy + 38.0, TINY, TEXT);
        }
        paint::icon(canvas, p.cache, Icon::More, width - MARGIN - 54.0, cy - 27.0, 54.0, false, TEXT);
    }
    // Media.
    let my = y + HEADER_H;
    if my + layout.media_h > band.0 && my < band.1 {
        let item = |i: usize| post.media.get(i);
        let rect = |dx: f32| RectF::new(dx, my, width, layout.media_h);
        let w = width.round() as u32;
        let h = layout.media_h.round() as u32;
        let draw_item = |canvas: &mut Canvas, p: &mut Paint, i: usize, dx: f32| {
            if let Some(media) = item(i) {
                let r = rect(dx);
                match p.picture(&media.reference, w, h, Fit::Cover, r.y) {
                    Some(image) => paint::blit(canvas, p.cache, &image, r.x.round() as i32, r.y.round() as i32),
                    None => {
                        // The small version while the full one loads.
                        let thumb = media.thumb.as_deref().and_then(|t| p.media.peek(t, w / 3, h / 3, Fit::Cover));
                        match thumb {
                            Some(image) => canvas.draw_image_scaled(&image, r, 1.0),
                            None => canvas.fill_rect(r.x as i32, r.y as i32, r.w as i32, r.h as i32, paint::PLACEHOLDER, 1.0),
                        }
                    }
                }
                if media.kind == "video" {
                    let label = if post.reel { "Reel" } else { "Video" };
                    let bx = r.x + r.w - MARGIN - 180.0;
                    canvas.fill_rrect(RectF::new(bx, r.y + 30.0, 180.0, 64.0), 32.0, 0x000000, 0.55);
                    paint::icon(canvas, p.cache, Icon::Play, bx + 22.0, r.y + 44.0, 36.0, true, 0xffffff);
                    paint::draw_text(canvas, label, bx + 70.0, r.y + 74.0, TINY, 0xffffff);
                }
            }
        };
        draw_item(canvas, p, index, drag);
        if drag < 0.0 && index + 1 < post.media.len() {
            draw_item(canvas, p, index + 1, drag + width);
        } else if drag > 0.0 && index > 0 {
            draw_item(canvas, p, index - 1, drag - width);
        }
        if post.media.len() > 1 {
            let label = format!("{}/{}", index + 1, post.media.len());
            let lw = paint::measure(&label, TINY) + 36.0;
            canvas.fill_rrect(RectF::new(width - MARGIN - lw, my + 30.0, lw, 56.0), 28.0, 0x121212, 0.7);
            paint::draw_text(canvas, &label, width - MARGIN - lw + 18.0, my + 68.0, TINY, TEXT);
        }
        // The next items ahead of a swipe.
        for i in [index + 1, index.saturating_sub(1)] {
            if let Some(media) = item(i) {
                p.prefetch(&media.reference, w, h, Fit::Cover);
            }
        }
    }
    // Actions.
    let ay = my + layout.media_h;
    if ay + ACTIONS_H > band.0 && ay < band.1 {
        let iy = ay + (ACTIONS_H - ICON) / 2.0;
        paint::icon(canvas, p.cache, Icon::Heart, MARGIN - 4.0, iy, ICON, post.liked, if post.liked { paint::RED } else { TEXT });
        paint::icon(canvas, p.cache, Icon::Comment, MARGIN + ICON + 44.0, iy, ICON, false, TEXT);
        paint::icon(canvas, p.cache, Icon::Share, MARGIN + 2.0 * ICON + 88.0, iy, ICON, false, TEXT);
        paint::icon(canvas, p.cache, Icon::Bookmark, width - MARGIN - ICON, iy, ICON, post.saved, TEXT);
        if post.media.len() > 1 {
            let n = post.media.len().min(10);
            let step = 22.0;
            let x0 = width / 2.0 - (n as f32 - 1.0) * step / 2.0;
            for i in 0..n {
                let active = i == index.min(n - 1);
                canvas.fill_circle(x0 + i as f32 * step, ay + ACTIONS_H / 2.0, 6.5, if active { BLUE } else { 0x555555 }, 1.0);
            }
        }
    }
    let mut ty = ay + ACTIONS_H;
    if let Some(likes) = likes_line(post) {
        if ty + LINE > band.0 && ty < band.1 {
            paint::draw_text(canvas, &likes, MARGIN, ty + 38.0, BODY_BOLD, TEXT);
        }
        ty += LINE;
    }
    for (n, line) in layout.caption.iter().enumerate() {
        if ty + LINE > band.0 && ty < band.1 {
            let w = paint::draw_line(canvas, line, MARGIN, ty + 38.0);
            if layout.truncated && n + 1 == layout.caption.len() {
                paint::draw_text(canvas, "… more", MARGIN + w, ty + 38.0, BODY, SECONDARY);
            }
        }
        ty += LINE;
    }
    if post.comments > 0 {
        if ty + LINE > band.0 && ty < band.1 {
            let label = if post.comments == 1 {
                "View 1 comment".to_owned()
            } else {
                format!("View all {} comments", paint::count(post.comments))
            };
            paint::draw_text(canvas, &label, MARGIN, ty + 38.0, BODY, SECONDARY);
        }
        ty += LINE;
    }
    if ty + 60.0 > band.0 && ty < band.1 {
        paint::draw_text(canvas, &paint::age_long(post.ts, now), MARGIN, ty + 40.0, TINY, SECONDARY);
    }
}

/// What a tap on a post hit.
pub enum PostHit {
    Author,
    Avatar,
    Media,
    Like,
    Comment,
    Share,
    Save,
    Caption,
    Comments,
    More,
    Nothing,
}

pub fn hit_post(post: &Post, layout: &PostLayout, x: f32, y: f32, width: f32) -> PostHit {
    if y < HEADER_H {
        if x < MARGIN + 38.0 * DP {
            return PostHit::Avatar;
        }
        if x > width - MARGIN - 100.0 {
            return PostHit::More;
        }
        return PostHit::Author;
    }
    let y = y - HEADER_H;
    if y < layout.media_h {
        return PostHit::Media;
    }
    let y = y - layout.media_h;
    if y < ACTIONS_H {
        return if x < MARGIN + ICON + 22.0 {
            PostHit::Like
        } else if x < MARGIN + 2.0 * ICON + 66.0 {
            PostHit::Comment
        } else if x < MARGIN + 3.0 * ICON + 110.0 {
            PostHit::Share
        } else if x > width - MARGIN - ICON - 30.0 {
            PostHit::Save
        } else {
            PostHit::Nothing
        };
    }
    let mut y = y - ACTIONS_H;
    if likes_line(post).is_some() {
        y -= LINE;
    }
    let caption = layout.caption.len() as f32 * LINE;
    if y >= 0.0 && y < caption {
        return PostHit::Caption;
    }
    if post.comments > 0 && y >= caption && y < caption + LINE {
        return PostHit::Comments;
    }
    PostHit::Nothing
}

/// What tapping a post does, for any list of posts.
pub fn post_action(post: &Post, hit: PostHit, store: &Store) -> Action {
    match hit {
        PostHit::Author => Action::Push(Box::new(ProfileScreen::user(post.author.id.clone(), post.author.username.clone()))),
        PostHit::Avatar => {
            if matches!(post.author.story.as_deref(), Some("unseen") | Some("close_friends") | Some("seen")) {
                Action::Stories {
                    ids: vec![post.author.id.clone()],
                    start: 0,
                }
            } else {
                Action::Push(Box::new(ProfileScreen::user(post.author.id.clone(), post.author.username.clone())))
            }
        }
        PostHit::Like => like(post, store),
        PostHit::Save => Action::request(
            "instagram.save",
            json!({"post": post.id, "remove": post.saved}),
            Ask::Write { what: "save" },
        ),
        PostHit::Comment | PostHit::Comments => Action::Push(Box::new(PostScreen::new(post.id.clone(), true))),
        PostHit::Share => Action::Toast("Sharing isn't available here yet.".into()),
        PostHit::More | PostHit::Media | PostHit::Caption | PostHit::Nothing => Action::None,
    }
}

pub fn like(post: &Post, _store: &Store) -> Action {
    Action::Many(vec![
        Action::request(
            "instagram.like",
            json!({"post": post.id, "remove": post.liked}),
            Ask::Write { what: "like" },
        ),
        Action::Haptic(crate::exocortex::ui::Haptic::Light),
    ])
}

/// Posts laid out once, by id.
#[derive(Default)]
pub struct Layouts {
    map: HashMap<String, PostLayout>,
}

impl Layouts {
    pub fn get(&mut self, post: &Post, width: f32, expanded: bool) -> &PostLayout {
        let sig = signature(post, expanded);
        let fresh = self.map.get(&post.id).is_none_or(|layout| layout.signature != sig);
        if fresh {
            self.map.insert(post.id.clone(), lay_post(post, width, expanded));
        }
        &self.map[&post.id]
    }
}

pub struct FeedScreen {
    scroll: Scroll,
    tray_scroll: Scroll,
    layouts: Layouts,
    /// Content y of each post's top, and the total.
    tops: Vec<f32>,
    height: f32,
    generation: u64,
    expanded: HashSet<String>,
    carousel: HashMap<String, usize>,
    /// A carousel being dragged: post id, where the drag started, how far it went.
    swipe: Option<(String, f32, f32)>,
    tray_drag: bool,
    refreshing: bool,
}

impl FeedScreen {
    pub fn new() -> Self {
        Self {
            scroll: Scroll::default(),
            tray_scroll: Scroll::default(),
            layouts: Layouts::default(),
            tops: Vec::new(),
            height: 0.0,
            generation: u64::MAX,
            expanded: HashSet::new(),
            carousel: HashMap::new(),
            swipe: None,
            tray_drag: false,
            refreshing: false,
        }
    }

    fn tray<'a>(store: &'a Store) -> Vec<&'a ReelSummary> {
        store.tray.iter().filter(|reel| !reel.muted).collect()
    }

    fn post_at(&self, content_y: f32) -> Option<usize> {
        if content_y < TRAY_H {
            return None;
        }
        let index = self.tops.partition_point(|top| *top <= content_y);
        index.checked_sub(1)
    }

    fn tray_max(store: &Store, width: f32) -> f32 {
        ((Self::tray(store).len() + 1) as f32 * TRAY_SLOT + 2.0 * MARGIN - width).max(0.0)
    }
}

impl Screen for FeedScreen {
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
        self.tops.clear();
        let mut y = TRAY_H + 1.0;
        for post in &store.feed.posts {
            self.tops.push(y);
            y += self.layouts.get(post, width, self.expanded.contains(&post.id)).height;
        }
        let tail = if store.feed.end || store.feed.posts.is_empty() { 300.0 } else { 220.0 };
        self.height = y + tail;
        self.height
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let base = area.top - area.offset;
        if area.offset < -40.0 {
            super::direct::spinner(canvas, area.width / 2.0, area.top + (-area.offset) / 2.0);
        }
        // The stories tray.
        if area.visible(base, base + TRAY_H) {
            let shift = self.tray_scroll.value(p.now).round();
            let tray = Self::tray(store);
            let own = store.account().cloned();
            let mut x = MARGIN - 8.0 - shift;
            let cy = base + TRAY_RING / 2.0 + 24.0;
            // Your story first.
            if x + TRAY_SLOT > 0.0 {
                let own_reel = store.own_reel.as_ref();
                let ring = if own_reel.is_some_and(|r| r.unseen) { Ring::Unseen } else if own_reel.is_some() { Ring::Seen } else { Ring::None };
                let reference = own.as_ref().and_then(|a| a.avatar.as_ref()).map(|m| m.reference.as_str());
                let d = if ring == Ring::None { TRAY_RING - 14.0 } else { TRAY_RING };
                paint::avatar(canvas, p, reference, x + TRAY_SLOT / 2.0, cy, d, ring);
                if own_reel.is_none() {
                    let bx = x + TRAY_SLOT / 2.0 + TRAY_RING * 0.33;
                    let by = cy + TRAY_RING * 0.33;
                    canvas.fill_circle(bx, by, 34.0, BG, 1.0);
                    canvas.fill_circle(bx, by, 28.0, TEXT, 1.0);
                    paint::icon(canvas, p.cache, Icon::Plus, bx - 20.0, by - 20.0, 40.0, false, BG);
                }
                paint::draw_text_centered(canvas, "Your story", x + TRAY_SLOT / 2.0, base + TRAY_H - 36.0, TINY, SECONDARY);
            }
            x += TRAY_SLOT;
            for reel in tray {
                if x > area.width {
                    break;
                }
                if x + TRAY_SLOT > 0.0 {
                    let ring = if reel.unseen {
                        if reel.close_friends { Ring::CloseFriends } else { Ring::Unseen }
                    } else {
                        Ring::Seen
                    };
                    paint::avatar(canvas, p, reel.user.avatar_ref(), x + TRAY_SLOT / 2.0, cy, TRAY_RING, ring);
                    let name = paint::fit(reel.user.handle(), TINY, TRAY_SLOT - 10.0);
                    paint::draw_text_centered(canvas, &name, x + TRAY_SLOT / 2.0, base + TRAY_H - 36.0, TINY, if reel.unseen { TEXT } else { SECONDARY });
                }
                x += TRAY_SLOT;
            }
            canvas.fill_rect(0, (base + TRAY_H) as i32, canvas.width, 1, SEPARATOR, 1.0);
        }
        // Posts.
        let first = self.post_at((area.band.0 - base).max(TRAY_H + 1.0)).unwrap_or(0);
        for (index, post) in store.feed.posts.iter().enumerate().skip(first) {
            let top = base + self.tops[index];
            if top > area.band.1 {
                // The next two posts' first pictures, ahead of the scroll.
                for ahead in store.feed.posts.iter().skip(index).take(2) {
                    if let Some(media) = ahead.media.first() {
                        let h = (area.width / ahead.aspect()).round() as u32;
                        p.prefetch(&media.reference, area.width as u32, h, Fit::Cover);
                    }
                }
                break;
            }
            let expanded = self.expanded.contains(&post.id);
            let layout = self.layouts.get(post, area.width, expanded).clone();
            let index_shown = self.carousel.get(&post.id).copied().unwrap_or(0).min(post.media.len().saturating_sub(1));
            let drag = match &self.swipe {
                Some((id, _, dx)) if id == &post.id => *dx,
                _ => 0.0,
            };
            draw_post(canvas, p, post, &layout, top, area.width, index_shown, drag, area.band);
        }
        let end = base + self.height - (if store.feed.end || store.feed.posts.is_empty() { 300.0 } else { 220.0 });
        if area.visible(end, end + 300.0) {
            if store.feed.posts.is_empty() {
                let note = if store.status.signed_out() {
                    "Sign in to see posts"
                } else if let Some(failed) = &store.feed.failed {
                    failed
                } else {
                    ""
                };
                if note.is_empty() {
                    super::direct::spinner(canvas, area.width / 2.0, end + 120.0);
                } else {
                    paint::draw_text_centered(canvas, note, area.width / 2.0, end + 140.0, BODY, SECONDARY);
                }
            } else if store.feed.end {
                canvas.fill_circle(area.width / 2.0, end + 90.0, 50.0, 0x000000, 1.0);
                canvas.stroke_circle(area.width / 2.0, end + 90.0, 50.0, 5.0, 0xc13584, 1.0);
                paint::icon(canvas, p.cache, Icon::Check, area.width / 2.0 - 30.0, end + 60.0, 60.0, false, 0xc13584);
                paint::draw_text_centered(canvas, "You're all caught up", area.width / 2.0, end + 210.0, BODY_BOLD, TEXT);
                paint::draw_text_centered(canvas, "You've seen all new posts from the past few days.", area.width / 2.0, end + 266.0, SMALL, SECONDARY);
            } else {
                super::direct::spinner(canvas, area.width / 2.0, end + 100.0);
            }
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        let cy = STATUS + TOP_BAR / 2.0;
        crate::text::draw(canvas, "Instagram", MARGIN, cy + 26.0, Weight::DisplaySemiBold, 76.0, TEXT, 1.0);
        let hx = area.width - MARGIN - ICON;
        paint::icon(canvas, p.cache, Icon::Heart, hx, cy - ICON / 2.0, ICON, false, TEXT);
        if store.activity.badge.unseen > 0 {
            canvas.fill_circle(hx + ICON - 4.0, cy - ICON / 2.0 + 6.0, 5.0 * DP, paint::RED, 1.0);
        }
        if store.feed.pending.is_some() && area.offset > 400.0 {
            let w = 330.0;
            let r = RectF::new(area.width / 2.0 - w / 2.0, area.top + 40.0, w, 96.0);
            canvas.fill_rrect(r, 48.0, BLUE, 1.0);
            paint::draw_text_centered(canvas, "↑ New posts", area.width / 2.0, r.y + 62.0, BODY_BOLD, 0xffffff);
        }
        if !store.status.online() && store.status.state != "connecting" {
            let note = super::direct::status_note(store);
            paint::draw_text_right(canvas, &note, hx - 30.0, paint::baseline(cy, TINY), TINY, SECONDARY);
        }
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            if x > area.width - MARGIN - ICON - 40.0 {
                return Action::Push(Box::new(ActivityScreen::new()));
            }
            return Action::None;
        }
        if store.feed.pending.is_some() && area.offset > 400.0 && y < area.top + 150.0 && (x - area.width / 2.0).abs() < 170.0 {
            self.scroll.reset();
            return Action::Mutate(Box::new(|store: &mut Store| {
                if let Some((posts, cursor, end)) = store.feed.pending.take() {
                    store.feed.posts = posts;
                    store.feed.cursor = cursor;
                    store.feed.end = end;
                }
            }));
        }
        let content_y = y - area.top + area.offset;
        if content_y < TRAY_H {
            let slot = ((x - MARGIN + 8.0 + self.tray_scroll.value(Instant::now())) / TRAY_SLOT).floor();
            if slot < 0.0 {
                return Action::None;
            }
            let tray = Self::tray(store);
            if slot as usize == 0 {
                if let Some(own) = &store.own_reel {
                    return Action::Stories {
                        ids: vec![own.id.clone()],
                        start: 0,
                    };
                }
                return Action::Toast("Posting stories isn't available here.".into());
            }
            let index = slot as usize - 1;
            if index < tray.len() {
                return Action::Stories {
                    ids: tray.iter().map(|reel| reel.id.clone()).collect(),
                    start: index,
                };
            }
            return Action::None;
        }
        let Some(index) = self.post_at(content_y) else {
            return Action::None;
        };
        let Some(post) = store.feed.posts.get(index) else {
            return Action::None;
        };
        let expanded = self.expanded.contains(&post.id);
        let layout = self.layouts.get(post, area.width, expanded).clone();
        let hit = hit_post(post, &layout, x, content_y - self.tops[index], area.width);
        if let PostHit::Caption = hit {
            if layout.truncated {
                self.expanded.insert(post.id.clone());
                self.generation = u64::MAX;
                return Action::Redraw;
            }
            return Action::None;
        }
        post_action(post, hit, store)
    }

    fn horizontal(&mut self, store: &Store, gesture: &Gesture, area: Area) -> Option<Action> {
        match *gesture {
            Gesture::DragStart { start_x, start_y, x, .. } => {
                let content_y = start_y - area.top + area.offset;
                if start_y > area.top && content_y < TRAY_H {
                    self.tray_drag = true;
                    self.tray_scroll.begin(-start_x, Instant::now());
                    self.tray_scroll.moved(-x, Self::tray_max(store, area.width), Instant::now());
                    return Some(Action::Redraw);
                }
                let index = self.post_at(content_y)?;
                let post = store.feed.posts.get(index)?;
                let layout = self.layouts.get(post, area.width, self.expanded.contains(&post.id));
                let within = content_y - self.tops[index] - HEADER_H;
                if post.media.len() > 1 && within >= 0.0 && within < layout.media_h {
                    self.swipe = Some((post.id.clone(), start_x, x - start_x));
                    return Some(Action::Redraw);
                }
                None
            }
            Gesture::Drag { x, .. } => {
                if self.tray_drag {
                    self.tray_scroll.moved(-x, Self::tray_max(store, area.width), Instant::now());
                    return Some(Action::Redraw);
                }
                let (id, start, _) = self.swipe.clone()?;
                self.swipe = Some((id, start, x - start));
                Some(Action::Redraw)
            }
            Gesture::DragEnd { x, vx, .. } => {
                if self.tray_drag {
                    self.tray_drag = false;
                    self.tray_scroll.end(Self::tray_max(store, area.width), Instant::now());
                    return Some(Action::Redraw);
                }
                let (id, _, dx) = self.swipe.take()?;
                let count = store.feed.posts.iter().find(|post| post.id == id).map_or(1, |post| post.media.len());
                let current = self.carousel.get(&id).copied().unwrap_or(0);
                let _ = x;
                let next = if (dx < -area.width * 0.25 || vx < -800.0) && current + 1 < count {
                    current + 1
                } else if (dx > area.width * 0.25 || vx > 800.0) && current > 0 {
                    current - 1
                } else {
                    current
                };
                self.carousel.insert(id, next);
                Some(Action::Redraw)
            }
            _ => None,
        }
    }

    fn tick(&mut self, store: &Store, area: Area, now: Instant) -> Action {
        let mut actions = Vec::new();
        if self.tray_scroll.step(Self::tray_max(store, area.width), now) {
            actions.push(Action::Redraw);
        }
        // Pull to refresh.
        if area.offset < -180.0 && !self.refreshing && self.scroll.dragging() {
            self.refreshing = true;
            actions.push(Action::request("instagram.feed", json!({}), Ask::Feed { cursor: None, cached: false }));
            actions.push(Action::request("instagram.tray", json!({}), Ask::Tray { cached: false }));
        }
        if area.offset >= 0.0 && !self.scroll.dragging() {
            self.refreshing = false;
        }
        let feed = &store.feed;
        if !feed.loading && !feed.end && feed.loaded && !feed.posts.is_empty()
            && let Some(cursor) = &feed.cursor
            && area.offset > self.height - (area.bottom - area.top) - 4000.0
        {
            actions.push(Action::request(
                "instagram.feed",
                json!({"cursor": cursor}),
                Ask::Feed {
                    cursor: Some(cursor.clone()),
                    cached: false,
                },
            ));
        }
        if self.tray_scroll.dragging() || self.swipe.is_some() {
            actions.push(Action::Redraw);
        }
        Action::Many(actions)
    }

    fn animating(&self, now: Instant) -> bool {
        let _ = now;
        self.swipe.is_some()
    }

    fn shown(&mut self, store: &Store) -> Action {
        let mut actions = vec![Action::request("instagram.tray", json!({"cached": !store.tray_loaded}), Ask::Tray { cached: !store.tray_loaded })];
        if !store.tray_loaded {
            actions.push(Action::request("instagram.tray", json!({}), Ask::Tray { cached: false }));
        }
        if !store.feed.loaded {
            if store.feed.posts.is_empty() {
                actions.push(Action::request("instagram.feed", json!({"cached": true}), Ask::Feed { cursor: None, cached: true }));
            }
            actions.push(Action::request("instagram.feed", json!({}), Ask::Feed { cursor: None, cached: false }));
        }
        Action::Many(actions)
    }

    fn blit_ok(&self) -> bool {
        self.swipe.is_none() && !self.tray_drag
    }

    fn to_top(&mut self) -> bool {
        self.scroll.reset();
        true
    }
}
