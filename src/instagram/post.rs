//! A post on its own, with its comments under it (Instagram's post page):
//! each comment's author, text, age, likes and its replies on request.
//! Opened from a grid, a comment link or an activity row; drawn at once
//! from what the list knew while the bridge reads it again.

use std::time::Instant;

use serde_json::json;

use super::app::{Action, Area, Screen};
use super::feed::{self, Layouts, PostHit};
use super::model::{Comment, Post};
use super::paint::{
    self, Icon, Line, Paint, Ring, BG, BODY, BODY_BOLD, DP, MARGIN, SAFE_BOTTOM, SECONDARY, SEPARATOR, STATUS,
    TAB_BAR, TEXT, TINY, TOP_BAR,
};
use super::profile::ProfileScreen;
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::gfx::Canvas;

const AVATAR: f32 = 36.0 * DP;
const TEXT_X: f32 = MARGIN + AVATAR + 30.0;

#[derive(Clone)]
struct LaidComment {
    comment: Comment,
    lines: Vec<Line>,
    y: f32,
    h: f32,
    reply: bool,
}

pub struct PostScreen {
    id: String,
    scroll: Scroll,
    layouts: Layouts,
    generation: u64,
    height: f32,
    post_h: f32,
    comments: Vec<LaidComment>,
    carousel: usize,
    open_replies: Vec<String>,
    jump_to_comments: bool,
}

impl PostScreen {
    pub fn new(id: String, comments: bool) -> Self {
        Self {
            id,
            scroll: Scroll::default(),
            layouts: Layouts::default(),
            generation: u64::MAX,
            height: 0.0,
            post_h: 0.0,
            comments: Vec::new(),
            carousel: 0,
            open_replies: Vec::new(),
            jump_to_comments: comments,
        }
    }

    fn post<'a>(&self, store: &'a Store) -> Option<&'a Post> {
        store
            .posts
            .get(&self.id)
            .or_else(|| store.feed.posts.iter().find(|p| p.id == self.id))
            .or_else(|| store.grids.values().chain(store.tagged.values()).flat_map(|g| g.posts.iter()).find(|p| p.id == self.id))
    }

    fn lay_comment(comment: &Comment, width: f32, reply: bool) -> (Vec<Line>, f32) {
        let indent = if reply { AVATAR + 30.0 } else { 0.0 };
        let mut words = vec![(comment.author.handle().to_owned(), BODY_BOLD, TEXT)];
        words.extend(paint::rich_words(&comment.text, BODY, TEXT));
        let lines = paint::wrap_runs(&words, width - TEXT_X - indent - MARGIN - 60.0, 30).0;
        let h = lines.len() as f32 * 50.0 + 70.0 + 30.0;
        (lines, h)
    }
}

impl Screen for PostScreen {
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
        let mut y = 0.0;
        if let Some(post) = self.post(store) {
            self.post_h = self.layouts.get(post, width, true).height;
            y = self.post_h;
        }
        y += 20.0;
        self.comments.clear();
        if let Some(comments) = store.comments.get(&self.id) {
            for comment in &comments.items {
                let (lines, h) = Self::lay_comment(comment, width, false);
                self.comments.push(LaidComment {
                    comment: comment.clone(),
                    lines,
                    y,
                    h,
                    reply: false,
                });
                y += h;
                if self.open_replies.contains(&comment.id)
                    && let Some(replies) = comments.replies.get(&comment.id)
                {
                    for reply in replies {
                        let (lines, h) = Self::lay_comment(reply, width, true);
                        self.comments.push(LaidComment {
                            comment: reply.clone(),
                            lines,
                            y,
                            h,
                            reply: true,
                        });
                        y += h;
                    }
                }
            }
        }
        self.height = y + 260.0;
        if self.jump_to_comments && self.post_h > 0.0 && !self.comments.is_empty() {
            self.jump_to_comments = false;
        }
        self.height
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let base = area.top - area.offset;
        let now = paint::now_ms();
        if let Some(post) = self.post(store) {
            if area.visible(base, base + self.post_h) {
                let layout = self.layouts.get(post, area.width, true).clone();
                feed::draw_post(canvas, p, post, &layout, base, area.width, self.carousel.min(post.media.len().saturating_sub(1)), 0.0, area.band);
            }
        } else {
            super::direct::spinner(canvas, area.width / 2.0, area.top + 300.0);
        }
        let sep = base + self.post_h;
        if area.visible(sep, sep + 2.0) {
            canvas.fill_rect(0, sep as i32, canvas.width, 1, SEPARATOR, 1.0);
        }
        for laid in &self.comments {
            let y = base + laid.y;
            if y > area.band.1 {
                break;
            }
            if y + laid.h < area.band.0 {
                continue;
            }
            let indent = if laid.reply { AVATAR + 30.0 } else { 0.0 };
            let d = if laid.reply { AVATAR * 0.75 } else { AVATAR };
            paint::avatar(canvas, p, laid.comment.author.avatar_ref(), MARGIN + indent + d / 2.0, y + 30.0 + d / 2.0, d, Ring::from_story(laid.comment.author.story.as_deref()));
            let x = TEXT_X + indent;
            for (n, line) in laid.lines.iter().enumerate() {
                paint::draw_line(canvas, line, x, y + 66.0 + n as f32 * 50.0);
            }
            let meta_y = y + 66.0 + laid.lines.len() as f32 * 50.0 + 6.0;
            let mut meta = paint::age_short(laid.comment.ts, now);
            if laid.comment.likes > 0 {
                meta.push_str(&format!("   {} like{}", paint::count(laid.comment.likes), if laid.comment.likes == 1 { "" } else { "s" }));
            }
            meta.push_str("   Reply");
            paint::draw_text(canvas, &meta, x, meta_y, TINY, SECONDARY);
            paint::icon(canvas, p.cache, Icon::Heart, area.width - MARGIN - 40.0, y + 40.0, 40.0, laid.comment.liked, if laid.comment.liked { paint::RED } else { SECONDARY });
            if !laid.reply && laid.comment.replies > 0 {
                let open = self.open_replies.contains(&laid.comment.id);
                let label = if open { "Hide replies".to_owned() } else { format!("View {} more repl{}", laid.comment.replies, if laid.comment.replies == 1 { "y" } else { "ies" }) };
                canvas.fill_rect(x as i32, (meta_y + 44.0) as i32, 60, 2, SECONDARY, 1.0);
                paint::draw_text(canvas, &label, x + 80.0, meta_y + 56.0, TINY, SECONDARY);
            }
        }
        let comments = store.comments.get(&self.id);
        let end = base + self.height - 260.0;
        if area.visible(end, end + 260.0) {
            match comments {
                Some(c) if c.loaded && c.items.is_empty() => {
                    paint::draw_text_centered(canvas, "No comments yet", area.width / 2.0, end + 120.0, BODY, SECONDARY);
                }
                Some(c) if c.loaded && c.cursor.is_none() => {}
                _ => super::direct::spinner(canvas, area.width / 2.0, end + 100.0),
            }
        }
    }

    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        canvas.fill_rect(0, 0, canvas.width, area.top as i32, BG, 1.0);
        let cy = STATUS + TOP_BAR / 2.0;
        paint::icon(canvas, p.cache, Icon::Back, MARGIN - 10.0, cy - 36.0, 72.0, false, TEXT);
        let title = if self.post(store).is_some_and(|post| post.reel) { "Reel" } else { "Post" };
        paint::draw_text(canvas, title, MARGIN + 100.0, paint::baseline(cy, paint::TITLE), paint::TITLE, TEXT);
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            return if x < MARGIN + 110.0 { Action::Pop } else { Action::None };
        }
        let cy = y - area.top + area.offset;
        if cy < self.post_h {
            let Some(post) = self.post(store) else {
                return Action::None;
            };
            let layout = self.layouts.get(post, area.width, true).clone();
            let hit = feed::hit_post(post, &layout, x, cy, area.width);
            return match hit {
                PostHit::Media if post.media.len() > 1 => {
                    self.carousel = if x > area.width / 2.0 {
                        (self.carousel + 1).min(post.media.len() - 1)
                    } else {
                        self.carousel.saturating_sub(1)
                    };
                    Action::Redraw
                }
                PostHit::Comment | PostHit::Comments => Action::None,
                hit => feed::post_action(post, hit, store),
            };
        }
        for laid in &self.comments {
            if cy >= laid.y && cy < laid.y + laid.h {
                let indent = if laid.reply { AVATAR + 30.0 } else { 0.0 };
                if x < TEXT_X + indent {
                    return Action::Push(Box::new(ProfileScreen::user(laid.comment.author.id.clone(), laid.comment.author.username.clone())));
                }
                if x > area.width - MARGIN - 80.0 {
                    return Action::Toast("Liking comments needs writes on.".into());
                }
                if !laid.reply && laid.comment.replies > 0 && cy > laid.y + laid.h - 70.0 {
                    let id = laid.comment.id.clone();
                    if let Some(index) = self.open_replies.iter().position(|open| open == &id) {
                        self.open_replies.remove(index);
                        self.generation = u64::MAX;
                        return Action::Redraw;
                    }
                    self.open_replies.push(id.clone());
                    self.generation = u64::MAX;
                    return Action::request(
                        "instagram.replies",
                        json!({"post": self.id, "comment": id}),
                        Ask::Replies { post: self.id.clone(), comment: id },
                    );
                }
                return Action::None;
            }
        }
        Action::None
    }

    fn tick(&mut self, store: &Store, area: Area, _now: Instant) -> Action {
        if let Some(comments) = store.comments.get(&self.id)
            && comments.loaded
            && !comments.loading
            && let Some(cursor) = &comments.cursor
            && area.offset > self.height - (area.bottom - area.top) - 2000.0
        {
            return Action::request(
                "instagram.comments",
                json!({"post": self.id, "cursor": cursor}),
                Ask::Comments { post: self.id.clone(), more: true },
            );
        }
        Action::None
    }

    fn shown(&mut self, store: &Store) -> Action {
        let mut actions = Vec::new();
        if !store.posts.contains_key(&self.id) {
            actions.push(Action::request("instagram.post", json!({"post": self.id}), Ask::Post { id: self.id.clone() }));
        }
        if !store.comments.contains_key(&self.id) {
            actions.push(Action::request("instagram.comments", json!({"post": self.id}), Ask::Comments { post: self.id.clone(), more: false }));
        }
        Action::Many(actions)
    }
}

/// A one-line summary of a comment (activity rows).
pub fn comment_line(comment: &Comment) -> String {
    format!("{} {}", comment.author.handle(), comment.text)
}
