//! Activity (Home's heart): follow requests, then Instagram's notifications
//! in its sections (New, Today, This week, This month, Earlier), each with
//! the account, the line with usernames in bold, the age, and the post or a
//! Follow back button. Instagram marks its activity seen when the list is
//! read, so with writes off the bridge doesn't read it: the screen shows
//! the counts Instagram has and says why. Opening it clears the badge here.

use std::time::Instant;

use serde_json::json;

use super::app::{Action, Area, Screen};
use super::model::Activity;
use super::paint::{
    self, BG, BLUE, BODY, BODY_BOLD, BUTTON, DP, Icon, Line, MARGIN, Paint, Ring, SAFE_BOTTOM,
    SECONDARY, SMALL, STATUS, TAB_BAR, TEXT, TOP_BAR,
};
use super::post::PostScreen;
use super::profile::ProfileScreen;
use super::store::{Ask, Store};
use crate::comms::Scroll;
use crate::gfx::{Canvas, RectF};

const AVATAR: f32 = 44.0 * DP;
const THUMB: f32 = 44.0 * DP;

enum Row {
    Requests(usize),
    Section(&'static str),
    Item(usize, Vec<Line>, f32),
    Note(Vec<Line>),
}

pub struct ActivityScreen {
    scroll: Scroll,
    rows: Vec<(f32, Row)>,
    height: f32,
    generation: u64,
    viewed: bool,
}

fn section(ts: i64, now: i64) -> &'static str {
    let age = now - ts;
    if age < 86_400_000 {
        "Today"
    } else if age < 7 * 86_400_000 {
        "This week"
    } else if age < 30 * 86_400_000 {
        "This month"
    } else {
        "Earlier"
    }
}

/// Plain words; the first sticks to what is before it unless a space separates them.
fn push_plain(words: &mut Vec<(String, paint::Font, u32)>, segment: &str) {
    for (n, word) in segment.split(' ').enumerate() {
        if word.is_empty() {
            continue;
        }
        let word = if n == 0 && !words.is_empty() {
            format!("{}{word}", paint::GLUE)
        } else {
            word.to_owned()
        };
        words.push((word, BODY, TEXT));
    }
}

fn item_lines(item: &Activity, width: f32) -> Vec<Line> {
    // Bold where the bridge says an account is named.
    let mut words: Vec<(String, paint::Font, u32)> = Vec::new();
    let mut pos = 0;
    let text = &item.text;
    let mut bolds = item.bold.clone();
    bolds.sort_by_key(|b| b.start);
    let to_byte = |utf16: usize| -> usize {
        let mut units = 0;
        for (i, c) in text.char_indices() {
            if units >= utf16 {
                return i;
            }
            units += c.len_utf16();
        }
        text.len()
    };
    for bold in &bolds {
        let (start, end) = (to_byte(bold.start), to_byte(bold.end));
        if start < pos || end <= start {
            continue;
        }
        push_plain(&mut words, &text[pos..start]);
        let glued = start > 0 && !text[..start].ends_with(' ');
        let bold = text[start..end].to_owned();
        words.push((
            if glued {
                format!("{}{bold}", paint::GLUE)
            } else {
                bold
            },
            BODY_BOLD,
            TEXT,
        ));
        pos = end;
    }
    push_plain(&mut words, &text[pos..]);
    words.push((paint::age_short(item.ts, paint::now_ms()), BODY, SECONDARY));
    paint::wrap_runs(&words, width, 4).0
}

impl ActivityScreen {
    pub fn new() -> Self {
        Self {
            scroll: Scroll::default(),
            rows: Vec::new(),
            height: 0.0,
            generation: u64::MAX,
            viewed: false,
        }
    }

    fn text_width(width: f32) -> f32 {
        width - 2.0 * MARGIN - AVATAR - 30.0 - THUMB - 40.0
    }
}

impl Screen for ActivityScreen {
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
        self.rows.clear();
        let mut y = 20.0;
        let activity = &store.activity;
        if !activity.requests.is_empty() {
            self.rows.push((y, Row::Requests(activity.requests.len())));
            y += 64.0 * DP;
        }
        let now = paint::now_ms();
        match &activity.items {
            Some(items) => {
                let mut current = "";
                for (index, item) in items.iter().enumerate() {
                    let name = section(item.ts, now);
                    if name != current {
                        current = name;
                        self.rows.push((y, Row::Section(name)));
                        y += 110.0;
                    }
                    let lines = item_lines(item, Self::text_width(width));
                    let h = (lines.len() as f32 * 50.0 + 50.0).max(AVATAR + 40.0);
                    self.rows.push((y, Row::Item(index, lines, h)));
                    y += h;
                }
                if items.is_empty() {
                    let note = paint::wrap_plain(
                        "Activity on your posts will show here.",
                        BODY,
                        SECONDARY,
                        width - 2.0 * MARGIN,
                        3,
                    )
                    .0;
                    self.rows.push((y, Row::Note(note)));
                    y += 200.0;
                }
            }
            None if activity.loaded || !activity.writes => {
                let badge = &activity.badge;
                let mut parts = Vec::new();
                for (n, one, many) in [
                    (badge.likes, "like", "likes"),
                    (badge.comments, "comment", "comments"),
                    (badge.relationships, "new follower", "new followers"),
                    (badge.usertags, "tag", "tags"),
                ] {
                    if n > 0 {
                        parts.push(format!("{n} {}", if n == 1 { one } else { many }));
                    }
                }
                let summary = if parts.is_empty() {
                    "Nothing new on Instagram.".to_owned()
                } else {
                    format!("New on Instagram: {}.", parts.join(", "))
                };
                let mut lines =
                    paint::wrap_plain(&summary, BODY_BOLD, TEXT, width - 2.0 * MARGIN, 4).0;
                lines.push(Vec::new());
                lines.extend(
                    paint::wrap_plain(
                        "Instagram marks your activity as seen when its list is read, so the list loads only with writes on (Profile › ☰ › Writes).",
                        SMALL,
                        SECONDARY,
                        width - 2.0 * MARGIN,
                        6,
                    )
                    .0,
                );
                let h = lines.len() as f32 * 50.0 + 80.0;
                self.rows.push((y, Row::Note(lines)));
                y += h;
            }
            None => {}
        }
        self.height = y + 100.0;
        self.height
    }

    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area) {
        let base = area.top - area.offset;
        let activity = &store.activity;
        for (y, row) in &self.rows {
            let y = base + y;
            if y > area.band.1 {
                break;
            }
            match row {
                Row::Requests(count) => {
                    let h = 64.0 * DP;
                    if !area.visible(y, y + h) {
                        continue;
                    }
                    let cy = y + h / 2.0;
                    let first = &activity.requests[0];
                    paint::avatar(
                        canvas,
                        p,
                        first.avatar_ref(),
                        MARGIN + AVATAR / 2.0,
                        cy,
                        AVATAR,
                        Ring::None,
                    );
                    let x = MARGIN + AVATAR + 30.0;
                    paint::draw_text(canvas, "Follow requests", x, cy - 6.0, BODY_BOLD, TEXT);
                    let others = if *count > 1 {
                        format!("{} + {} others", first.handle(), count - 1)
                    } else {
                        first.handle().to_owned()
                    };
                    paint::draw_text(canvas, &others, x, cy + 40.0, SMALL, SECONDARY);
                    canvas.fill_circle(area.width - MARGIN - 80.0, cy, 4.0 * DP, BLUE, 1.0);
                    paint::icon(
                        canvas,
                        p.cache,
                        Icon::ChevronRight,
                        area.width - MARGIN - 50.0,
                        cy - 25.0,
                        50.0,
                        false,
                        SECONDARY,
                    );
                }
                Row::Section(name) => {
                    if area.visible(y, y + 110.0) {
                        paint::draw_text(canvas, name, MARGIN, y + 74.0, paint::HEADLINE, TEXT);
                    }
                }
                Row::Item(index, lines, h) => {
                    if !area.visible(y, y + h) {
                        continue;
                    }
                    let Some(item) = activity.items.as_ref().and_then(|items| items.get(*index))
                    else {
                        continue;
                    };
                    let cy = y + 20.0 + AVATAR / 2.0;
                    if item.actors.len() >= 2 {
                        let small = AVATAR * 0.72;
                        paint::avatar(
                            canvas,
                            p,
                            item.actors[1].avatar_ref(),
                            MARGIN + small / 2.0 + AVATAR * 0.28,
                            cy - AVATAR * 0.14,
                            small,
                            Ring::None,
                        );
                        canvas.fill_circle(
                            MARGIN + small / 2.0,
                            cy + AVATAR * 0.14,
                            small / 2.0 + 4.0,
                            BG,
                            1.0,
                        );
                        paint::avatar(
                            canvas,
                            p,
                            item.actors[0].avatar_ref(),
                            MARGIN + small / 2.0,
                            cy + AVATAR * 0.14,
                            small,
                            Ring::None,
                        );
                    } else if let Some(actor) = item.actors.first() {
                        paint::avatar(
                            canvas,
                            p,
                            actor.avatar_ref(),
                            MARGIN + AVATAR / 2.0,
                            cy,
                            AVATAR,
                            Ring::from_story(actor.story.as_deref()),
                        );
                    }
                    let x = MARGIN + AVATAR + 30.0;
                    for (n, line) in lines.iter().enumerate() {
                        paint::draw_line(canvas, line, x, y + 52.0 + n as f32 * 50.0);
                    }
                    let right = area.width - MARGIN;
                    if let Some(media) = &item.media {
                        let r = RectF::new(right - THUMB, y + 20.0, THUMB, THUMB);
                        let reference = media.thumb.as_deref().unwrap_or(&media.reference);
                        paint::picture(canvas, p, reference, r, paint::PLACEHOLDER);
                        paint::round_corners(canvas, r, 6.0 * DP, BG);
                    } else if item.kind == "follow" {
                        let following = item.following == Some(true);
                        let label = if following {
                            "Following"
                        } else {
                            "Follow back"
                        };
                        let w = paint::measure(label, BODY_BOLD) + 60.0;
                        let r = RectF::new(right - w, y + 20.0, w, 32.0 * DP);
                        canvas.fill_rrect(r, 8.0 * DP, if following { BUTTON } else { BLUE }, 1.0);
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
                Row::Note(lines) => {
                    for (n, line) in lines.iter().enumerate() {
                        if !line.is_empty() {
                            paint::draw_line(canvas, line, MARGIN, y + 60.0 + n as f32 * 50.0);
                        }
                    }
                }
            }
        }
        if !activity.loaded && activity.items.is_none() {
            super::direct::spinner(canvas, area.width / 2.0, area.top + 300.0);
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
            "Notifications",
            MARGIN + 100.0,
            paint::baseline(cy, paint::TITLE),
            paint::TITLE,
            TEXT,
        );
    }

    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action {
        if y < area.top {
            return if x < MARGIN + 110.0 {
                Action::Pop
            } else {
                Action::None
            };
        }
        let cy = y - area.top + area.offset;
        for (row_y, row) in &self.rows {
            let h = match row {
                Row::Requests(_) => 64.0 * DP,
                Row::Item(_, _, h) => *h,
                _ => continue,
            };
            if cy < *row_y || cy >= row_y + h {
                continue;
            }
            match row {
                Row::Requests(_) => {
                    return Action::Toast("Approving requests needs writes on.".into());
                }
                Row::Item(index, _, _) => {
                    let Some(item) = store
                        .activity
                        .items
                        .as_ref()
                        .and_then(|items| items.get(*index))
                    else {
                        return Action::None;
                    };
                    if x < MARGIN + AVATAR + 20.0 || item.target.kind == "profile" {
                        if let Some(actor) = item.actors.first() {
                            return Action::Push(Box::new(ProfileScreen::user(
                                actor.id.clone(),
                                actor.username.clone(),
                            )));
                        }
                    }
                    return match item.target.kind.as_str() {
                        "post" => Action::Push(Box::new(PostScreen::new(
                            item.target.id.clone(),
                            item.kind == "comment",
                        ))),
                        "story" => Action::Stories {
                            ids: vec![item.target.id.clone()],
                            start: 0,
                        },
                        _ => Action::None,
                    };
                }
                _ => {}
            }
        }
        Action::None
    }

    fn shown(&mut self, store: &Store) -> Action {
        let mut actions = vec![Action::request(
            "instagram.activity",
            json!({"cached": true}),
            Ask::Activity { cached: true },
        )];
        actions.push(Action::request(
            "instagram.activity",
            json!({}),
            Ask::Activity { cached: false },
        ));
        if !self.viewed || store.activity.badge.unseen > 0 {
            self.viewed = true;
            actions.push(Action::request(
                "instagram.activity.viewed",
                json!({}),
                Ask::Badge,
            ));
        }
        Action::Many(actions)
    }

    fn tick(&mut self, _store: &Store, _area: Area, _now: Instant) -> Action {
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instagram::model::Bold;

    #[test]
    fn names_are_bold_and_punctuation_stays_on_them() {
        let item = Activity {
            text: "ana, leo and 3 others liked your photo.".into(),
            bold: vec![
                Bold {
                    start: 0,
                    end: 3,
                    username: "ana".into(),
                },
                Bold {
                    start: 5,
                    end: 8,
                    username: "leo".into(),
                },
            ],
            ..Default::default()
        };
        let lines = item_lines(&item, 2000.0);
        let text: String = lines[0].iter().map(|run| run.text.as_str()).collect();
        assert!(text.starts_with("ana, leo and 3 others"), "{text}");
        assert_eq!(lines[0][0].font, BODY_BOLD);
        assert_eq!(lines[0][0].text, "ana");
    }
}
