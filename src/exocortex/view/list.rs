//! The conversation list (WP-B; roadmap §3.6.1–3.6.5, `real-01-sidebar`):
//! the Whale top bar (search field, ⋯, new folder, new chat, the page
//! rail), then the rows — the PINNED header, conversations, folders that
//! open in place, dividers — or search results; empty, loading and error
//! states and the connection pill.
//!
//! Each row on screen is described by what it draws (`Look`), and a frame
//! records the hash of each; the next frame moves the rows a scroll moved
//! and draws again only the rows whose look changed (a pulsing dot, a
//! relative time turning over, a row pressed, an update from the daemon).
//! Only the rows on screen are looked at, however long the list.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::gfx::{Canvas, RectF, Rgb};
use crate::text::Weight;

use super::super::anim::{self, Animator};
use super::super::list::activity::{self, Activity, Kind};
use super::super::list::search::Search;
use super::super::model::sidebar::{Act, Aggregates, Drag, DropTarget, Key, Layout};
use super::super::model::{self, Action, Model, Page, Row};
use super::super::theme::{
    self, FOLDER_ICON, FOLDER_INDENT, GESTURE_RESERVE, HAIRLINE, LIST_PAD, LIST_ROW, S1, SPINNER,
    T_CAPTION, T_EYEBROW, T_META, T_NOTE, T_ROW, TARGET, TOP_BAR, W,
};
use super::super::ui::icons::{self, Icon};
use super::super::ui::widgets::{self, Button, ButtonKind, RAIL_LIST};
use super::{FrameDamage, Hit, ViewState, pressed_action};

pub(crate) const ROW_H: f32 = LIST_ROW;
/// The header's controls' centres.
const MORE_X: f32 = 768.0;
const FOLDER_PLUS_X: f32 = 888.0;
const PLUS_X: f32 = 1008.0;
const ROW_Y: f32 = 61.0;
/// The search field (`real-01-sidebar`: x 44 to 692; the roadmap's 17 to
/// 680 is not where the iPhone draws it).
const SEARCH: RectF = RectF::new(44.0, 14.0, 648.0, 94.0);
/// Room under the last row (the iPhone's footer, less its safe area).
const FOOT: f32 = 111.0;
/// Where a conversation row's trailing items end (12 pt in).
const RIGHT: f32 = W - LIST_PAD;
/// A folder row's "+": its target is the row's right 122 px.
const FOLDER_PLUS_CX: f32 = W - TARGET / 2.0;
/// Where a folder row's trailing items end: left of the "+", 10 pt apart.
const FOLDER_RIGHT: f32 = W - TARGET - 28.0;
/// Gaps between a row's inline items and its trailing ones (6 pt).
const GAP: f32 = 17.0;
/// Half the rows an activity dot covers (25 px, anti-aliased).
const DOT_HALF: i32 = 14;
/// A folder's chevron turns 90° in this many steps, over 240 ms.
const TURN: u8 = 16;
const TURN_FOR: f32 = 0.24;

/// Moon shrinks the window above its keyboard: no home gesture under it.
fn keyboard_up(height: f32) -> bool {
    height < f32::from(crate::REQUESTED_HEIGHT) - 300.0
}

/// Where the list's rows are: under the bar, above Moon's home gesture (or
/// its keyboard).
pub fn bounds(model: &Model, height: f32) -> (f32, f32) {
    let bar = if model.sidebar.selection.is_some() {
        SELECT_BAR
    } else {
        0.0
    };
    (TOP_BAR + HAIRLINE, bottom(height) - bar)
}

/// The page's bottom: Moon's home gesture (or its keyboard) below it.
fn bottom(height: f32) -> f32 {
    let reserve = if keyboard_up(height) {
        0.0
    } else {
        GESTURE_RESERVE
    };
    height - reserve
}

use super::super::model::sidebar::SELECT_BAR;
/// The select bar's buttons' actions, left to right.
const SELECT_ACTS: [Act; 4] = [
    Act::SelectMove,
    Act::SelectPin,
    Act::SelectGroup,
    Act::SelectDelete,
];

/// The select bar: its count line, whether every selected item is pinned,
/// the button pressed.
fn select_bar_key(model: &Model, pressed: Option<&Action>) -> Option<u64> {
    let keys = model.sidebar.selected_keys();
    model.sidebar.selection.as_ref()?;
    let all_pinned = !keys.is_empty()
        && keys
            .iter()
            .all(|key| model.sidebar.pinned_of(key) == Some(true));
    let pressed = pressed.and_then(|action| match action {
        Action::List(act) => SELECT_ACTS.iter().position(|a| a == act),
        _ => None,
    });
    Some(super::hash_of((
        keys.len(),
        all_pinned,
        pressed,
        theme::generation(),
    )))
}

fn draw_select_bar(canvas: &mut Canvas, model: &Model, pressed: Option<&Action>, height: f32) {
    let y = bottom(height) - SELECT_BAR;
    if !widgets::shows(canvas, y, y + SELECT_BAR) {
        return;
    }
    let p = theme::p();
    let c = theme::c();
    widgets::fill(canvas, RectF::new(0.0, y, W, SELECT_BAR), p.app_background);
    widgets::hairline(canvas, 0.0, W, y, c.rule_topbar);
    let keys = model.sidebar.selected_keys();
    let n = keys.len();
    let count = if n == 0 {
        "Tap chats or folders to select".to_owned()
    } else {
        format!("{n} selected")
    };
    widgets::text_centered(
        canvas,
        &count,
        W / 2.0,
        T_EYEBROW.baseline(y + 17.0),
        T_EYEBROW,
        p.muted,
        1.0,
    );
    let all_pinned = n > 0
        && keys
            .iter()
            .all(|key| model.sidebar.pinned_of(key) == Some(true));
    let cell = W / 4.0;
    let labels = [
        (Icon::FolderMove, "Move"),
        if all_pinned {
            (Icon::PinOff, "Unpin")
        } else {
            (Icon::Pin, "Pin")
        },
        (Icon::FolderPlus, "Group"),
        (Icon::Trash, "Delete"),
    ];
    for (index, (icon, label)) in labels.into_iter().enumerate() {
        let cx = cell * index as f32 + cell / 2.0;
        let down = pressed == Some(&Action::List(SELECT_ACTS[index].clone()));
        let color = if index == 3 { p.error } else { p.accent };
        let alpha = if n == 0 {
            0.3
        } else if down {
            0.5
        } else {
            1.0
        };
        icons::draw(canvas, icon, cx, y + 98.0, 60.0, color, alpha);
        let ty = theme::T_FOOTER.weight(Weight::MonoBold);
        widgets::text_centered(canvas, label, cx, ty.baseline(y + 138.0), ty, color, alpha);
    }
}

/// The select bar's button at (`x`, `y`).
fn select_bar_hit(model: &Model, height: f32, x: f32, y: f32) -> Option<Action> {
    model.sidebar.selection.as_ref()?;
    let top = bottom(height) - SELECT_BAR;
    if y < top + 50.0 || y >= bottom(height) || model.sidebar.selected_keys().is_empty() {
        return None;
    }
    let index = ((x / (W / 4.0)).floor() as usize).min(3);
    Some(Action::List(SELECT_ACTS[index].clone()))
}

/// Where the first row starts on screen, unscrolled.
pub(crate) fn list_top(_model: &Model) -> f32 {
    TOP_BAR + HAIRLINE + S1
}

pub fn list_max(model: &mut Model, height: f32) -> f32 {
    let layout = model.sidebar.layout();
    let (_, bottom) = bounds(model, height);
    (layout.height + FOOT - (bottom - list_top(model))).max(0.0)
}

// ----- What a row draws ----------------------------------------------------------

/// How a row's background shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Face {
    Plain,
    Selected,
    Pressed,
    Lifted,
    /// Selected in select mode.
    Checked,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConversationLook {
    depth: usize,
    /// None: blank ("Untitled chat").
    title: Option<String>,
    /// The search hit in the title (byte range).
    hit: Option<(usize, usize)>,
    mark: Option<&'static str>,
    star: bool,
    pin: bool,
    muted: bool,
    meta: String,
    activity: Activity,
    /// The streaming dot's pulse (percent).
    pulse: u32,
    face: Face,
    /// In select mode: whether it is selected.
    check: Option<bool>,
    /// A drag's reorder line at its top (false) or bottom (true).
    edge: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FolderLook {
    depth: usize,
    open: bool,
    /// The chevron's turn, 0 (closed, pointing right) to `TURN` (open).
    turn: u8,
    name: String,
    hit: Option<(usize, usize)>,
    pin: bool,
    muted: bool,
    meta: String,
    indicator: Option<(Kind, usize)>,
    pulse: u32,
    /// ◆, $, ◷ (when the folder's own rows are not under it).
    badges: (u32, u32, u32),
    unread: usize,
    face: Face,
    plus: bool,
    check: Option<bool>,
    /// A drag would drop into it.
    drop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Look {
    /// A row whose conversation or folder is gone (drawn blank).
    Missing,
    Pinned,
    Results(usize),
    Divider(usize),
    Conversation(ConversationLook),
    Folder(FolderLook),
}

impl Look {
    /// A hash of everything it draws but its dot's pulse, and the pulse:
    /// a frame where only the pulse moved redraws only the dot's rows.
    fn keys(&self) -> (u64, u32) {
        let mut still = self.clone();
        let pulse = match &mut still {
            Look::Conversation(c) => std::mem::replace(&mut c.pulse, 0),
            Look::Folder(f) => std::mem::replace(&mut f.pulse, 0),
            _ => 0,
        };
        let mut hasher = DefaultHasher::new();
        still.hash(&mut hasher);
        (hasher.finish(), pulse)
    }

    fn pulsing(&self) -> bool {
        match self {
            Look::Conversation(c) => c.activity.kind == Some(Kind::Streaming),
            Look::Folder(f) => matches!(f.indicator, Some((Kind::Streaming, _))),
            _ => false,
        }
    }
}

/// What looking at rows needs, gathered once per frame.
struct Ctx<'a> {
    aggregates: Rc<Aggregates>,
    now_ms: f64,
    pulse: u32,
    pressed: Option<&'a Action>,
    lifted: Option<&'a Action>,
    selected: Option<String>,
    /// The search query, lower-cased (empty when not searching).
    query: String,
    /// The folder toggled last and how far its chevron has turned (0..1).
    turning: Option<(String, f32)>,
    /// Select mode's selection.
    selection: Option<BTreeSet<Key>>,
    /// A drag under way: what is dragged and where it would land.
    drag: Option<(Key, Option<DropTarget>)>,
}

impl<'a> Ctx<'a> {
    fn new(
        model: &mut Model,
        pressed: Option<&'a Action>,
        lifted: Option<&'a Action>,
        now: Instant,
    ) -> Self {
        let selected = match model.page {
            Page::Chat => model.chat.id.clone(),
            _ => model.sidebar.selected.clone(),
        };
        Ctx {
            aggregates: model.sidebar.aggregates(&model.composer.queued),
            now_ms: model.sidebar.wall_ms(now),
            pulse: anim::pulse_percent(now, 1600),
            pressed,
            lifted,
            selected,
            query: model.sidebar.search.query().to_lowercase(),
            turning: model.sidebar.toggled.as_ref().map(|(id, at)| {
                let t = now.saturating_duration_since(*at).as_secs_f32() / TURN_FOR;
                (id.clone(), anim::ease_out(t))
            }),
            selection: model.sidebar.selection.clone(),
            drag: model
                .sidebar
                .drag
                .as_ref()
                .filter(|drag| drag.active)
                .map(|drag| (drag.key.clone(), drag.target.clone())),
        }
    }

    /// In select mode, whether `key` is selected.
    fn check(&self, key: Key) -> Option<bool> {
        self.selection
            .as_ref()
            .map(|selection| selection.contains(&key))
    }

    /// The dragged item shows lifted.
    fn dragged(&self, key: Key) -> bool {
        self.drag
            .as_ref()
            .is_some_and(|(dragged, _)| *dragged == key)
    }

    fn face(&self, action: &Action) -> Face {
        if self.lifted == Some(action) {
            Face::Lifted
        } else if self.pressed == Some(action) {
            Face::Pressed
        } else {
            Face::Plain
        }
    }
}

/// The item a row shows.
fn row_key(row: &Row) -> Option<Key> {
    match row {
        Row::Conversation { id, .. } => Some(Key::Conv(id.clone())),
        Row::Folder { id, .. } => Some(Key::Folder(id.clone())),
        _ => None,
    }
}

/// The action a row performs when tapped (its pressed and lifted state);
/// in select mode, it is selected or not.
fn row_action(row: &Row, searching: bool, selecting: bool) -> Option<Action> {
    if selecting {
        return row_key(row).map(|key| Action::List(Act::ToggleSelect(key)));
    }
    match row {
        Row::Conversation { id, .. } => Some(Action::List(Act::Open(id.clone()))),
        Row::Folder { id, .. } if searching => Some(Action::List(Act::ShowFolder(id.clone()))),
        Row::Folder { id, .. } => Some(Action::List(Act::Toggle(id.clone()))),
        _ => None,
    }
}

fn badge_label(symbol: &str, count: u32) -> String {
    if count > 1 {
        format!("{symbol}{}", widgets::count_text(count as usize))
    } else {
        symbol.to_owned()
    }
}

fn look(model: &Model, ctx: &Ctx, row: &Row) -> Look {
    let searching = !ctx.query.is_empty();
    let selecting = ctx.selection.is_some();
    match row {
        Row::Pinned => Look::Pinned,
        Row::Results(count) => Look::Results(*count),
        Row::Divider { depth } => Look::Divider(*depth),
        Row::Conversation { id, depth } => {
            let Some(c) = model.sidebar.conversations.get(id) else {
                return Look::Missing;
            };
            let folder_muted = c
                .folder_id
                .as_ref()
                .is_some_and(|f| ctx.aggregates.muted.contains(f));
            let queue = ctx.aggregates.queues.get(id).copied().unwrap_or_default();
            let activity = activity::conversation(c, queue, false, folder_muted);
            let title = activity::row_title(c).map(str::to_owned);
            let hit = title
                .as_deref()
                .and_then(|title| Search::find(title, &ctx.query));
            let count = c.message_count;
            let mut meta = format!(
                "{} · {} {}",
                activity::ago(c.updated_at, ctx.now_ms),
                activity::thousands(count),
                if count == 1 { "message" } else { "messages" }
            );
            if activity.queued > 0 {
                meta.push_str(&format!(" · {} queued", activity.queued));
            }
            let action = row_action(row, searching, selecting).expect("a conversation's");
            let check = ctx.check(Key::Conv(id.clone()));
            let edge = match &ctx.drag {
                Some((_, Some(DropTarget::Reorder { key, after })))
                    if *key == Key::Conv(id.clone()) =>
                {
                    Some(*after)
                }
                _ => None,
            };
            Look::Conversation(ConversationLook {
                depth: *depth,
                title,
                hit,
                mark: activity::mark_prefix(activity::first_line(&c.title)),
                star: c.marked,
                pin: c.pinned && (*depth > 0 || searching),
                muted: c.muted,
                meta,
                pulse: if activity.kind == Some(Kind::Streaming) {
                    ctx.pulse
                } else {
                    100
                },
                activity,
                face: match ctx.face(&action) {
                    _ if ctx.dragged(Key::Conv(id.clone())) => Face::Lifted,
                    Face::Plain if check == Some(true) => Face::Checked,
                    Face::Plain
                        if check.is_none() && ctx.selected.as_deref() == Some(id.as_str()) =>
                    {
                        Face::Selected
                    }
                    face => face,
                },
                check,
                edge,
            })
        }
        Row::Folder { id, depth, open } => {
            let Some(f) = model.sidebar.folders.get(id) else {
                return Look::Missing;
            };
            let aggregate = ctx.aggregates.folders.get(id).copied().unwrap_or_default();
            let muted = ctx.aggregates.muted.contains(id);
            // An open folder's rows show their own badges right below it.
            let shown = !open || searching;
            let indicator = shown.then(|| aggregate.indicator(muted)).flatten();
            let name = if f.name.is_empty() {
                "Folder".to_owned()
            } else {
                f.name.clone()
            };
            let count = aggregate.count;
            let mut meta = format!(
                "{} {}",
                activity::thousands(count as u64),
                if count == 1 { "chat" } else { "chats" }
            );
            if searching {
                meta = format!(
                    "{} · {meta}",
                    model.sidebar.folder_path(f.parent_id.as_deref())
                );
            }
            let action = row_action(row, searching, selecting).expect("a folder's");
            let check = ctx.check(Key::Folder(id.clone()));
            let face = match ctx.face(&action) {
                _ if ctx.dragged(Key::Folder(id.clone())) => Face::Lifted,
                Face::Plain if check == Some(true) => Face::Checked,
                face => face,
            };
            let drop = matches!(&ctx.drag, Some((_, Some(DropTarget::Folder(f)))) if f == id);
            let progress = match &ctx.turning {
                Some((turning, progress)) if turning == id => *progress,
                _ => 1.0,
            };
            let progress = if *open { progress } else { 1.0 - progress };
            Look::Folder(FolderLook {
                depth: *depth,
                open: *open,
                turn: (progress * TURN as f32).round() as u8,
                hit: Search::find(&name, &ctx.query),
                name,
                pin: f.pinned && (*depth > 0 || searching),
                muted: f.muted,
                meta: meta.to_uppercase(),
                pulse: if matches!(indicator, Some((Kind::Streaming, _))) {
                    ctx.pulse
                } else {
                    100
                },
                indicator,
                badges: if shown {
                    (aggregate.agents, aggregate.commands, aggregate.chrono)
                } else {
                    (0, 0, 0)
                },
                unread: if shown && !muted { aggregate.unread } else { 0 },
                face,
                plus: ctx.pressed == Some(&Action::List(Act::NewChatIn(id.clone()))),
                check,
                drop,
            })
        }
    }
}

// ----- Drawing rows ---------------------------------------------------------------

fn face_color(face: Face) -> Rgb {
    let p = theme::p();
    let c = theme::c();
    match face {
        Face::Plain => p.sidebar_background,
        Face::Selected => p.selection_background,
        Face::Pressed => c.row_pressed,
        Face::Lifted => c.row_lifted,
        Face::Checked => c.row_checked,
    }
}

fn activity_color(kind: Kind) -> Rgb {
    let p = theme::p();
    match kind {
        Kind::Streaming => p.accent,
        Kind::Warning => p.warning,
        Kind::Unread => p.success,
    }
}

/// Text with a search hit highlighted; returns its width.
#[allow(clippy::too_many_arguments)]
fn text_with_hit(
    canvas: &mut Canvas,
    text: &str,
    hit: Option<(usize, usize)>,
    x: f32,
    baseline: f32,
    ty: theme::Type,
    color: Rgb,
    width: f32,
) -> f32 {
    let p = theme::p();
    let shown = widgets::fit(text, ty, width);
    if let Some((from, to)) = hit {
        // The visible part of the title (an ellipsis may cut the hit).
        let visible = shown.strip_suffix('…').unwrap_or(&shown);
        let to = to.min(visible.len());
        if from < to && visible.is_char_boundary(to) {
            let x0 = x + widgets::measure(&visible[..from], ty);
            let w = widgets::measure(&visible[from..to], ty);
            let top = (baseline - (ty.line() + ty.px() * 0.74) / 2.0).round();
            widgets::fill(
                canvas,
                RectF::new(x0.round(), top, w.round(), ty.line()),
                p.search_background,
            );
            widgets::text(canvas, &visible[..from], x, baseline, ty, color, 1.0);
            widgets::text(
                canvas,
                &visible[from..to],
                x0,
                baseline,
                ty,
                p.search_foreground,
                1.0,
            );
            let rest = &shown[to..];
            widgets::text(canvas, rest, x0 + w, baseline, ty, color, 1.0);
            return widgets::measure(&shown, ty);
        }
    }
    widgets::text(canvas, &shown, x, baseline, ty, color, 1.0)
}

/// Where the ◆ badge of these counts is (x from, x to), right to left from
/// `right` as `draw_badges` places it.
fn agents_badge((agents, commands, chrono): (u32, u32, u32), right: f32) -> Option<(f32, f32)> {
    if agents == 0 {
        return None;
    }
    let ty = T_CAPTION.weight(Weight::MonoBold);
    let mut right = right;
    for (count, symbol) in [(commands, "$"), (chrono, "◷")] {
        if count > 0 {
            right -= widgets::measure(&badge_label(symbol, count), ty) + GAP;
        }
    }
    let w = widgets::measure(&badge_label("◆", agents), ty);
    Some((right - w, right))
}

/// Badges ◆ $ ◷ right to left from `right`; returns where they start.
fn draw_badges(
    canvas: &mut Canvas,
    (agents, commands, chrono): (u32, u32, u32),
    right: f32,
    cy: f32,
) -> f32 {
    let p = theme::p();
    let ty = T_CAPTION.weight(Weight::MonoBold);
    let baseline = ty.baseline(cy - ty.line() / 2.0);
    let mut right = right;
    for (count, symbol, color) in [
        (commands, "$", p.warning),
        (chrono, "◷", p.success),
        (agents, "◆", p.accent),
    ] {
        if count > 0 {
            let w = widgets::mark(canvas, &badge_label(symbol, count), right, baseline, color);
            right -= w + GAP;
        }
    }
    right
}

/// The activity dot (and a count after it); returns where it starts.
fn draw_dot(
    canvas: &mut Canvas,
    kind: Kind,
    count: usize,
    pulse: u32,
    right: f32,
    cy: f32,
    background: Rgb,
) -> f32 {
    let color = activity_color(kind);
    let mut right = right;
    if count > 1 {
        let ty = T_META.weight(Weight::MonoBold);
        let w = widgets::text_right(
            canvas,
            &widgets::count_text(count),
            right,
            ty.baseline(cy - ty.line() / 2.0),
            ty,
            color,
            1.0,
        );
        right -= w + 8.0;
    }
    let alpha = if kind == Kind::Streaming {
        pulse as f32 / 100.0
    } else {
        1.0
    };
    widgets::activity_dot(canvas, right - 12.5, cy, color, background, alpha);
    right - 25.0
}

fn draw_conversation(canvas: &mut Canvas, c: &ConversationLook, y: f32) {
    let p = theme::p();
    let background = face_color(c.face);
    widgets::fill(canvas, RectF::new(0.0, y, W, ROW_H), background);
    if c.face == Face::Selected {
        widgets::fill(
            canvas,
            RectF::new(0.0, y + 17.0, theme::RAIL, ROW_H - 34.0),
            p.accent,
        );
    }
    let mut x = LIST_PAD + FOLDER_INDENT * c.depth as f32;
    let cy = y + ROW_H / 2.0;
    if let Some(checked) = c.check {
        x += draw_check(canvas, x, cy, checked);
    }
    // Trailing: badges, then the dot left of them.
    let mut trail = draw_badges(
        canvas,
        (c.activity.agents, c.activity.commands, c.activity.chrono),
        RIGHT,
        cy,
    );
    if let Some(kind) = c.activity.kind {
        trail = draw_dot(canvas, kind, 0, c.pulse, trail, cy, background) - GAP;
    }
    let baseline = y + 72.0;
    let mut tx = x;
    if c.pin {
        icons::draw(
            canvas,
            Icon::Pin,
            tx + 17.0,
            baseline - 14.0,
            34.0,
            p.accent,
            1.0,
        );
        tx += 34.0 + GAP;
    }
    if c.star {
        icons::draw(
            canvas,
            Icon::Star,
            tx + 18.0,
            baseline - 14.0,
            36.0,
            p.warning,
            1.0,
        );
        tx += 36.0 + GAP;
    }
    if let Some(mark) = c.mark {
        tx += widgets::text(canvas, mark, tx, baseline, T_NOTE, p.text, 1.0) + GAP;
    }
    let muted_w = if c.muted { 36.0 + GAP } else { 0.0 };
    let room = (trail - tx - muted_w).max(0.0);
    let title_w = match &c.title {
        Some(title) => {
            let ty = if c.face == Face::Selected {
                T_ROW.weight(Weight::MonoBold)
            } else {
                T_ROW
            };
            text_with_hit(canvas, title, c.hit, tx, baseline, ty, p.text, room)
        }
        None => {
            let ty = T_ROW.weight(theme::T_ITALIC);
            let shown = widgets::fit("Untitled chat", ty, room);
            widgets::text(canvas, &shown, tx, baseline, ty, p.muted, 1.0)
        }
    };
    if c.muted {
        icons::draw(
            canvas,
            Icon::BellOff,
            tx + title_w + GAP + 18.0,
            baseline - 14.0,
            36.0,
            p.muted,
            1.0,
        );
    }
    let meta = widgets::fit(&c.meta, T_META, (RIGHT - x).max(0.0));
    widgets::text(canvas, &meta, x, y + 124.0, T_META, p.muted, 1.0);
    // A drag's reorder line (§3.6.3): an 8 px capsule at the edge it
    // would land on, inside the row.
    if let Some(after) = c.edge {
        let ly = if after { y + ROW_H - 8.0 } else { y };
        canvas.fill_rrect(RectF::new(22.0, ly, W - 44.0, 8.0), 4.0, p.accent, 1.0);
    }
}

/// The chevron (`Icon::ChevronRight`'s stroke) turned `turn` steps of 90°.
fn chevron(canvas: &mut Canvas, cx: f32, cy: f32, size: f32, turn: u8, color: Rgb) {
    let angle = f32::from(turn) / f32::from(TURN) * std::f32::consts::FRAC_PI_2;
    let (sin, cos) = angle.sin_cos();
    let scale = size / 24.0;
    // "M9 5l7 7-7 7" about the box's centre (12, 12).
    let points: Vec<(f32, f32)> = [(-3.0, -7.0), (4.0, 0.0), (-3.0, 7.0)]
        .iter()
        .map(|(x, y): &(f32, f32)| {
            (
                cx + (x * cos - y * sin) * scale,
                cy + (x * sin + y * cos) * scale,
            )
        })
        .collect();
    let mut path = crate::gfx::Path::new();
    path.stroke_polyline(&points, 2.0 * scale);
    canvas.fill_path(&path, color, 1.0);
}

/// Select mode's check circle at a row's start; returns how far the row's
/// content moves right.
fn draw_check(canvas: &mut Canvas, x: f32, cy: f32, checked: bool) -> f32 {
    let p = theme::p();
    if checked {
        icons::draw(canvas, Icon::CheckCircle, x + 30.0, cy, 60.0, p.accent, 1.0);
    } else {
        icons::draw(canvas, Icon::Circle, x + 30.0, cy, 60.0, p.muted, 1.0);
    }
    60.0 + 28.0
}

fn draw_folder(canvas: &mut Canvas, f: &FolderLook, y: f32) {
    let p = theme::p();
    let c = theme::c();
    let background = if f.drop {
        c.drop_target
    } else {
        face_color(f.face)
    };
    widgets::fill(canvas, RectF::new(0.0, y, W, ROW_H), background);
    if f.drop {
        widgets::border(canvas, RectF::new(0.0, y, W, ROW_H), 6.0, p.accent);
    }
    // 8 pt in, 14 pt a level.
    let mut base = 22.0 + FOLDER_INDENT * f.depth as f32;
    let cy = y + ROW_H / 2.0;
    if let Some(checked) = f.check {
        base += draw_check(canvas, base, cy, checked);
    }
    chevron(canvas, base + 19.5, cy, 39.0, f.turn, p.muted);
    let icon = RectF::new(
        base + 67.0,
        y + ((ROW_H - FOLDER_ICON) / 2.0).floor(),
        FOLDER_ICON,
        FOLDER_ICON,
    );
    widgets::boxed(canvas, icon, c.folder_icon_bg, c.folder_icon_border);
    let (icx, icy) = icon.center();
    icons::draw(
        canvas,
        if f.open {
            Icon::FolderOpen
        } else {
            Icon::FolderFill
        },
        icx,
        icy,
        47.0,
        p.secondary_accent,
        1.0,
    );
    // Trailing: the unread count, badges, the dot; then the "+".
    // In select mode there is no "+".
    let mut trail = if f.check.is_some() {
        RIGHT
    } else {
        FOLDER_RIGHT
    };
    if f.unread > 0 {
        let w = widgets::badge(
            canvas,
            &widgets::count_text(f.unread),
            trail,
            cy,
            p.notification_background,
            p.notification_foreground,
        );
        trail -= w + GAP;
    }
    trail = draw_badges(canvas, f.badges, trail, cy);
    if let Some((kind, count)) = f.indicator {
        trail = draw_dot(canvas, kind, count, f.pulse, trail, cy, background) - GAP;
    }
    if f.check.is_none() {
        widgets::icon_button(
            canvas,
            Icon::Plus,
            FOLDER_PLUS_CX,
            cy,
            50.0,
            c.text_output,
            f.plus,
        );
    }
    let x = base + 178.0;
    let baseline = y + 72.0;
    let mut tx = x;
    if f.pin {
        icons::draw(
            canvas,
            Icon::Pin,
            tx + 17.0,
            baseline - 14.0,
            34.0,
            p.accent,
            1.0,
        );
        tx += 34.0 + GAP;
    }
    let muted_w = if f.muted { 36.0 + GAP } else { 0.0 };
    let ty = T_ROW.weight(Weight::MonoSemiBold);
    let room = (trail - tx - muted_w).max(0.0);
    let w = text_with_hit(canvas, &f.name, f.hit, tx, baseline, ty, p.text, room);
    if f.muted {
        icons::draw(
            canvas,
            Icon::BellOff,
            tx + w + GAP + 18.0,
            baseline - 14.0,
            36.0,
            p.muted,
            1.0,
        );
    }
    let meta_ty = T_META.tracked(1.2);
    let meta = widgets::fit(&f.meta, meta_ty, (trail - x).max(0.0));
    widgets::text(canvas, &meta, x, y + 124.0, meta_ty, p.muted, 1.0);
}

fn draw_look(canvas: &mut Canvas, look: &Look, y: f32) {
    let p = theme::p();
    let c = theme::c();
    match look {
        Look::Pinned | Look::Results(_) => {
            widgets::fill(
                canvas,
                RectF::new(0.0, y, W, theme::HEADER_ROW),
                p.sidebar_background,
            );
            let baseline = y + 66.0;
            let mut x = LIST_PAD;
            if matches!(look, Look::Pinned) {
                icons::draw(
                    canvas,
                    Icon::Pin,
                    x + 16.5,
                    baseline - 11.0,
                    33.0,
                    p.muted,
                    1.0,
                );
                x += 33.0 + 19.0;
                widgets::eyebrow(canvas, "Pinned", x, baseline, p.muted);
            } else if let Look::Results(count) = look {
                let w = widgets::eyebrow(canvas, "Search results", x, baseline, p.muted);
                if *count > 0 {
                    widgets::text(
                        canvas,
                        &format!(" // {}", activity::thousands(*count as u64)),
                        x + w,
                        baseline,
                        T_EYEBROW,
                        p.muted,
                        0.8,
                    );
                }
            }
        }
        Look::Missing => widgets::fill(canvas, RectF::new(0.0, y, W, ROW_H), p.sidebar_background),
        Look::Divider(depth) => {
            let h = theme::DIVIDER_ROW;
            widgets::fill(canvas, RectF::new(0.0, y, W, h), p.sidebar_background);
            let x = LIST_PAD + FOLDER_INDENT * *depth as f32;
            widgets::hairline(canvas, x, RIGHT, y + 14.0, c.rule_list);
        }
        Look::Conversation(look) => draw_conversation(canvas, look, y),
        Look::Folder(look) => draw_folder(canvas, look, y),
    }
}

// ----- The header --------------------------------------------------------------

fn header_owns(action: &Action) -> bool {
    matches!(
        action,
        Action::List(
            Act::NewChat
                | Act::NewFolder { parent: None, .. }
                | Act::Menu(_)
                | Act::Search
                | Act::ClearSearch
                | Act::EndSelect
        )
    )
}

/// The header's folder-plus.
fn new_folder() -> Act {
    Act::NewFolder {
        parent: None,
        items: Vec::new(),
    }
}

/// Select mode's Done: where the header's controls are otherwise.
const DONE: RectF = RectF::new(708.0, 0.0, W - 708.0, TOP_BAR);

/// The search field's ✕ target (when there is text).
fn clear_target() -> RectF {
    RectF::new(SEARCH.x + SEARCH.w - TARGET, 0.0, TARGET, TOP_BAR)
}

/// The header's control at (`x`, `y`).
fn header_hit(model: &Model, x: f32, y: f32) -> Option<Action> {
    if y >= TOP_BAR {
        return None;
    }
    let near = |cx: f32| (x - cx).abs() < 60.0;
    let selecting = model.sidebar.selection.is_some();
    let act = if selecting && DONE.contains(x, y) {
        Act::EndSelect
    } else if selecting && x >= SEARCH.x + SEARCH.w {
        return None;
    } else if near(PLUS_X) {
        Act::NewChat
    } else if near(FOLDER_PLUS_X) {
        new_folder()
    } else if near(MORE_X) {
        Act::Menu(widgets::target(MORE_X, ROW_Y))
    } else if x < SEARCH.x + SEARCH.w {
        if !model.sidebar.search.text.is_empty() && clear_target().contains(x, y) {
            Act::ClearSearch
        } else {
            Act::Search
        }
    } else {
        return None;
    };
    Some(Action::List(act))
}

/// What the header draws (its frame key).
fn header_key(model: &Model, pressed: Option<&Action>) -> u64 {
    let mut hasher = DefaultHasher::new();
    (
        &model.sidebar.search.text,
        model.sidebar.search.focused,
        model.sidebar.selection.is_some(),
        super::action_key(pressed.filter(|action| header_owns(action))),
        theme::generation(),
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// The top bar: the search field, ⋯, new folder, new chat.
fn draw_header(canvas: &mut Canvas, model: &Model, pressed: Option<&Action>) {
    if !widgets::shows(canvas, 0.0, TOP_BAR + HAIRLINE) {
        return;
    }
    let p = theme::p();
    let c = theme::c();
    let down = |act: Act| pressed == Some(&Action::List(act));
    widgets::top_bar(canvas, p.sidebar_background);
    widgets::page_rail(canvas, RAIL_LIST);
    widgets::boxed(canvas, SEARCH, c.search_box, c.search_field_border);
    let search = &model.sidebar.search;
    let alpha = if down(Act::Search) { 0.5 } else { 1.0 };
    icons::draw(
        canvas,
        Icon::Search,
        SEARCH.x + 49.0,
        ROW_Y,
        44.0,
        p.muted,
        alpha,
    );
    let text_x = SEARCH.x + 97.0;
    if search.text.is_empty() {
        let placeholder = T_CAPTION.tracked(2.2);
        widgets::text(
            canvas,
            "SEARCH CHATS",
            text_x,
            placeholder.baseline(ROW_Y - placeholder.line() / 2.0),
            placeholder,
            p.muted,
            alpha,
        );
    } else {
        // The end of what is typed stays in view, left of the ✕.
        let room = clear_target().x - text_x;
        let mut shown = search.text.as_str();
        while widgets::measure(shown, T_ROW) > room - 12.0 && !shown.is_empty() {
            let mut chars = shown.chars();
            chars.next();
            shown = chars.as_str();
        }
        let w = widgets::text(
            canvas,
            shown,
            text_x,
            T_ROW.baseline(ROW_Y - T_ROW.line() / 2.0),
            T_ROW,
            p.text,
            1.0,
        );
        if search.focused {
            widgets::fill(
                canvas,
                RectF::new((text_x + w + 2.0).round(), ROW_Y - 26.0, 5.0, 52.0),
                p.accent,
            );
        }
        let clear = clear_target();
        icons::draw(
            canvas,
            Icon::X,
            clear.x + clear.w / 2.0,
            ROW_Y,
            39.0,
            p.muted,
            if down(Act::ClearSearch) { 0.5 } else { 1.0 },
        );
    }
    if search.focused && search.text.is_empty() {
        widgets::fill(
            canvas,
            RectF::new(text_x - 8.0, ROW_Y - 26.0, 5.0, 52.0),
            p.accent,
        );
    }
    if model.sidebar.selection.is_some() {
        let ty = T_NOTE.weight(Weight::MonoBold);
        widgets::text_centered(
            canvas,
            "DONE",
            DONE.x + DONE.w / 2.0,
            ty.baseline(ROW_Y - ty.line() / 2.0),
            ty,
            p.accent,
            if down(Act::EndSelect) { 0.5 } else { 1.0 },
        );
        return;
    }
    widgets::ellipsis(
        canvas,
        MORE_X,
        ROW_Y,
        p.text,
        if matches!(pressed, Some(Action::List(Act::Menu(_)))) {
            0.5
        } else {
            1.0
        },
    );
    widgets::icon_button(
        canvas,
        Icon::FolderPlus,
        FOLDER_PLUS_X,
        ROW_Y,
        60.0,
        p.text,
        down(new_folder()),
    );
    widgets::icon_button(
        canvas,
        Icon::Plus,
        PLUS_X,
        ROW_Y,
        60.0,
        p.text,
        down(Act::NewChat),
    );
}

// ----- States -----------------------------------------------------------------------

/// What shows instead of rows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum State {
    Rows,
    /// Connected, the list on its way; else the connection's label.
    Loading(&'static str),
    Error(String),
    Empty,
}

fn state(model: &Model, layout: &Layout) -> State {
    if !layout.rows.is_empty() || model.sidebar.search.active() {
        return State::Rows;
    }
    if let Some(error) = &model.sidebar.error {
        return State::Error(error.clone());
    }
    if !model.sidebar.loaded {
        return State::Loading(if model.connected {
            "Loading chats…"
        } else if model.was_connected {
            "Reconnecting…"
        } else {
            "Connecting…"
        });
    }
    State::Empty
}

/// The rows' area below the header.
fn area(model: &Model, height: f32) -> RectF {
    let (top, bottom) = bounds(model, height);
    RectF::new(0.0, top, W, bottom - top)
}

/// The state's button and its rect (`widgets::empty_state`'s geometry).
fn state_button(state: &State, area: RectF) -> Option<(Button<'static>, Act, RectF)> {
    let (button, act) = match state {
        State::Error(_) => (Button::new("Retry", ButtonKind::Secondary), Act::Retry),
        State::Empty => (Button::new("New chat", ButtonKind::Secondary), Act::NewChat),
        _ => return None,
    };
    let title_ty = T_ROW.weight(Weight::MonoSemiBold);
    let body_lines = match state {
        State::Error(error) => widgets::wrap_plain(error, T_NOTE, area.w - 88.0).len(),
        _ => 0,
    };
    let height = title_ty.line() + body_lines as f32 * T_NOTE.line() + 11.0 + TARGET + 34.0;
    let y = (area.y + (area.h - height) / 2.0).round();
    let w = button.width();
    let by = y + title_ty.line() + 11.0 + body_lines as f32 * T_NOTE.line() + 34.0;
    let rect = RectF::new((area.x + area.w / 2.0 - w / 2.0).round(), by, w, TARGET);
    Some((button, act, rect))
}

fn draw_state(
    canvas: &mut Canvas,
    state: &State,
    area: RectF,
    pressed: Option<&Action>,
    now: Instant,
) {
    let p = theme::p();
    match state {
        State::Rows => {}
        State::Loading(label) => {
            let cy = (area.y + area.h / 2.0).round();
            widgets::spinner(
                canvas,
                W / 2.0,
                cy - 40.0,
                SPINNER,
                p.accent,
                p.sidebar_background,
                now,
            );
            widgets::text_centered(
                canvas,
                label,
                W / 2.0,
                T_NOTE.baseline(cy),
                T_NOTE,
                p.muted,
                1.0,
            );
        }
        State::Error(_) | State::Empty => {
            let Some((button, act, _)) = state_button(state, area) else {
                return;
            };
            let (title, body) = match state {
                State::Error(error) => ("Couldn't load chats", error.as_str()),
                _ => ("No conversations", ""),
            };
            let button = button.pressed(pressed == Some(&Action::List(act)));
            widgets::empty_state(
                canvas,
                area,
                title,
                body,
                Some(&button),
                p.sidebar_background,
            );
        }
    }
}

/// "No Results" under the results header.
fn draw_no_results(canvas: &mut Canvas, area: RectF) {
    let p = theme::p();
    let area = RectF::new(
        area.x,
        area.y + theme::HEADER_ROW,
        area.w,
        area.h - theme::HEADER_ROW,
    );
    let cy = (area.y + area.h * 0.4).round();
    icons::draw(
        canvas,
        Icon::Search,
        W / 2.0,
        cy - 110.0,
        111.0,
        p.muted,
        1.0,
    );
    let title_ty = T_ROW.weight(Weight::MonoSemiBold);
    widgets::text_centered(
        canvas,
        "No Results",
        W / 2.0,
        title_ty.baseline(cy),
        title_ty,
        p.text,
        1.0,
    );
    widgets::text_centered(
        canvas,
        "Check the spelling or try a new search.",
        W / 2.0,
        T_NOTE.baseline(cy + title_ty.line() + 11.0),
        T_NOTE,
        p.muted,
        1.0,
    );
}

/// The connection pill's top (bottom centre, above the home gesture).
fn pill_top(height: f32) -> f32 {
    height - GESTURE_RESERVE - 44.0 - 88.0
}

/// The pill shows over a list that has rows, while not connected.
fn pill_shows(model: &Model) -> bool {
    model.offline && model.sidebar.loaded && model.sidebar.selection.is_none()
}

// ----- Frames ---------------------------------------------------------------------

/// What a list frame showed, so the next moves and redraws only rows
/// (`draw_list_update`).
#[derive(Clone, PartialEq)]
pub struct ListFrame {
    pub(crate) bounds: (i32, i32),
    pub(crate) offset: f32,
    /// Everything but the rows and the header: the state, the size, the theme.
    pub(crate) key: u64,
    pub(crate) header: u64,
    /// The rows on screen: top (from the list's top), height, look (its
    /// hash and its dot's pulse).
    pub(crate) rows: Vec<(i32, i32, (u64, u32))>,
    /// Where the rows end.
    pub(crate) end: i32,
    /// The connection pill's spinner step, when it shows.
    pub(crate) pill: Option<u32>,
    /// What shows pressed (bands drawn for the frame agree).
    pub(crate) pressed: Option<Action>,
    /// Select mode's bar, as drawn.
    pub(crate) bar: Option<u64>,
    /// The lifted row (top, height), whose shadow falls on its neighbours.
    pub(crate) lifted: Option<(i32, i32)>,
    /// A pull past the top: the gap it opens and how the indicator shows.
    pub(crate) pull: Option<(i32, u64)>,
    /// A dragged row's preview: its rows and what it shows.
    pub(crate) preview: Option<(i32, i32, u64)>,
}

/// The rows of `layout` reaching into `y0..y1` (from the list's top).
fn visible(layout: &Layout, y0: f32, y1: f32) -> std::ops::Range<usize> {
    let first = layout.first_below(y0);
    let last = layout.tops.partition_point(|top| *top < y1);
    first.min(last)..last
}

/// The current list frame's record.
pub fn list_frame(
    model: &mut Model,
    view: &ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) -> Option<ListFrame> {
    if model.page != Page::List {
        return None;
    }
    let layout = model.sidebar.layout();
    let pressed = pressed_action(pressed);
    let (top, bottom) = bounds(model, height);
    let offset = view.list_scroll.value(now).round();
    let content_top = list_top(model);
    let header = header_key(model, pressed);
    let lifted = view.lifted.as_ref();
    let state = state(model, &layout);
    let no_results = model.sidebar.search.active() && layout.rows.len() <= 1;
    let mut key = DefaultHasher::new();
    (
        &state,
        no_results,
        footer(model),
        height as i32,
        theme::generation(),
        matches!(state, State::Loading(_)).then(|| (anim::phase(now, 700) * 30.0) as u32),
        super::action_key(pressed.filter(|a| matches!(a, Action::List(Act::NewChat | Act::Retry)))),
    )
        .hash(&mut key);
    let range = visible(
        &layout,
        top - content_top + offset,
        bottom - content_top + offset,
    );
    let ctx = Ctx::new(model, pressed, lifted, now);
    let mut pulsing = false;
    let searching = model.sidebar.search.active();
    let selecting = model.sidebar.selection.is_some();
    let mut lifted_row = None;
    let rows = layout.rows[range.clone()]
        .iter()
        .zip(&layout.tops[range])
        .map(|(row, y)| {
            let look = look(model, &ctx, row);
            pulsing |= look.pulsing();
            if lifted.is_some() && row_action(row, searching, selecting).as_ref() == lifted {
                lifted_row = Some((*y as i32, row.height() as i32));
            }
            (*y as i32, row.height() as i32, look.keys())
        })
        .collect();
    // For `animate`: frames while a dot on screen pulses.
    model.sidebar.pulsing = pulsing;
    Some(ListFrame {
        bounds: (top as i32, bottom as i32),
        offset,
        key: key.finish(),
        header,
        rows,
        end: layout.height as i32,
        pill: pill_shows(model).then(|| (anim::phase(now, 700) * 60.0) as u32),
        pressed: pressed.cloned(),
        bar: select_bar_key(model, pressed),
        lifted: lifted_row,
        pull: pull(model, offset, now),
        preview: preview_rect(model, height).map(|(rect, title, folder)| {
            (
                rect.y as i32,
                (rect.y + rect.h) as i32,
                super::hash_of((rect.x as i32, rect.w as i32, title, folder)),
            )
        }),
    })
}

/// The daemon's update status under the list (PAR-086), when it asks for
/// something.
fn footer(model: &Model) -> Option<&'static str> {
    match model.sidebar.update_status.as_deref() {
        Some("update_available") => Some("↑ Daemon update available"),
        Some("restart_needed") => Some("↻ Daemon restart needed"),
        _ => None,
    }
}

/// The footer at `y`: a hairline, then the status in the accent.
fn draw_footer(canvas: &mut Canvas, label: &str, y: f32) {
    if !widgets::shows(canvas, y, y + 70.0) {
        return;
    }
    let p = theme::p();
    widgets::hairline(canvas, LIST_PAD, RIGHT, y, theme::c().rule_list);
    let ty = T_META.weight(Weight::MonoBold).tracked(1.2);
    widgets::text(
        canvas,
        label,
        LIST_PAD,
        ty.baseline(y + 14.0),
        ty,
        p.accent,
        1.0,
    );
}

/// How far past the top a pull must go to refresh (the iPhone's 56 pt of
/// its damped pull).
pub const PULL: f32 = 95.0;

/// A pull past the top: the gap, and a key of its indicator (the arrow's
/// look, or the spinner's step while the list is being asked for).
fn pull(model: &Model, offset: f32, now: Instant) -> Option<(i32, u64)> {
    if offset >= 0.0 {
        return None;
    }
    let gap = -offset;
    let look = if model.list_refreshing() {
        (2, (anim::phase(now, 700) * 30.0) as u32)
    } else {
        (u32::from(gap >= PULL), (gap.min(60.0) / 2.0) as u32)
    };
    Some((gap as i32, super::hash_of(look)))
}

/// The pull indicator in the gap above the first row.
fn draw_pull(canvas: &mut Canvas, model: &Model, top: f32, gap: f32, now: Instant) {
    let p = theme::p();
    let clip = canvas.clip();
    let y1 = (top + gap).floor() as i32;
    if y1 <= clip.1 || (top as i32) >= clip.3 {
        return;
    }
    canvas.set_clip(clip.0, clip.1.max(top as i32), clip.2, clip.3.min(y1));
    let cy = (top + gap / 2.0).round();
    if model.list_refreshing() {
        widgets::spinner(
            canvas,
            W / 2.0,
            cy,
            SPINNER,
            p.accent,
            p.sidebar_background,
            now,
        );
    } else {
        let color = if gap >= PULL { p.accent } else { p.muted };
        let alpha = ((gap.min(60.0) / 2.0).floor() / 30.0).min(1.0);
        icons::draw(canvas, Icon::Refresh, W / 2.0, cy, 50.0, color, alpha);
    }
    canvas.set_clip(clip.0, clip.1, clip.2, clip.3);
}

/// A lifted row's shadow (§3.5: blur 44, black 34 %, 19 down), drawn on
/// the rows around it.
const LIFT_BLUR: f32 = 44.0;
const LIFT_DY: f32 = 19.0;

/// The rows a lifted row's shadow reaches (content coordinates).
fn lift_reach((y, h): (i32, i32)) -> (i32, i32) {
    (
        y - (LIFT_BLUR - LIFT_DY) as i32 - 1,
        y + h + (LIFT_BLUR + LIFT_DY) as i32 + 1,
    )
}

/// Asks for the frames the list needs: pulsing dots and spinners at 30 Hz,
/// the next minute (relative times), the search's debounce.
pub fn animate(model: &Model, now: Instant, animator: &mut Animator) {
    if model.offline || !model.sidebar.loaded || model.sidebar.pulsing {
        animator.slow_frame(now);
    }
    let into_minute = model.sidebar.clock.map_or(model::now_ms(), |(at, ms)| {
        ms + now.saturating_duration_since(at).as_millis() as u64
    }) % 60_000;
    animator.request(now + Duration::from_millis(60_000 - into_minute + 5));
    if let Some(due) = model.sidebar.search.due() {
        animator.request(due);
    }
    // A dragged row scrolling the list.
    if model
        .sidebar
        .drag
        .as_ref()
        .is_some_and(|drag| drag.active && drag.speed != 0.0)
    {
        animator.frame(now);
    }
    // A folder's chevron turning.
    if let Some((_, at)) = &model.sidebar.toggled
        && now.saturating_duration_since(*at).as_secs_f32() < TURN_FOR
    {
        animator.frame(now);
    }
}

/// The list frame on `canvas` (drawn for `old`) turned into `new`'s: rows
/// moved by a scroll, the uncovered strip and the rows whose look changed
/// drawn again. None when more changed.
pub fn draw_list_update(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    old: &ListFrame,
    new: &ListFrame,
    now: Instant,
) -> Option<FrameDamage> {
    if old.key != new.key || old.bounds != new.bounds {
        return None;
    }
    let (top, bottom) = new.bounds;
    let shift = (new.offset - old.offset) as i32;
    let mut damage = FrameDamage::default();
    let mut redraw: Vec<(i32, i32)> = Vec::new();
    if shift != 0 {
        if shift.abs() > (bottom - top) / 2 {
            return None;
        }
        let width = canvas.width as usize;
        if shift > 0 {
            canvas.pixels.copy_within(
                (top + shift) as usize * width..bottom as usize * width,
                top as usize * width,
            );
            redraw.push((bottom - shift, bottom));
        } else {
            canvas.pixels.copy_within(
                top as usize * width..(bottom + shift) as usize * width,
                (top - shift) as usize * width,
            );
            redraw.push((top, top - shift));
        }
        damage.moved = Some((top, bottom, -shift));
    }
    let content_top = list_top(model) as i32 - new.offset as i32;
    let on_screen = |y0: i32, y1: i32| (y0.max(top), y1.min(bottom));
    // Rows whose look changed (or that were not there); of a pulsing dot,
    // the dot's rows.
    let before: HashMap<(i32, i32), (u64, u32)> =
        old.rows.iter().map(|(y, h, k)| ((*y, *h), *k)).collect();
    for (y, h, (key, pulse)) in &new.rows {
        let row = (content_top + y, content_top + y + h);
        match before.get(&(*y, *h)) {
            Some((old_key, old_pulse)) if old_key == key => {
                if old_pulse != pulse {
                    let cy = row.0 + h / 2;
                    redraw.push(on_screen(cy - DOT_HALF, cy + DOT_HALF));
                }
            }
            _ => redraw.push(on_screen(row.0, row.1)),
        }
    }
    // Where the rows end moved: the blank under them.
    if old.end != new.end {
        redraw.push(on_screen(content_top + old.end.min(new.end), bottom));
    }
    if old.header != new.header {
        redraw.push(widgets::TOP_BAR_ROWS);
    }
    // A lifted row's shadow, come or gone (it moves with a scroll).
    if old.lifted != new.lifted || (shift != 0 && new.lifted.is_some()) {
        for (y0, y1) in [old.lifted, new.lifted]
            .into_iter()
            .flatten()
            .map(lift_reach)
        {
            redraw.push(on_screen(content_top + y0, content_top + y1));
            // Where the old frame's shadow was, before this frame's move.
            redraw.push(on_screen(
                content_top + y0 - shift,
                content_top + y1 - shift,
            ));
        }
    }
    // The pull's gap.
    if old.pull != new.pull {
        let gap = old
            .pull
            .map_or(0, |(g, _)| g)
            .max(new.pull.map_or(0, |(g, _)| g));
        redraw.push(on_screen(top, top + gap + theme::S1 as i32 + 2));
    }
    // A dragged row's preview: where it was and is (a scroll moved its
    // pixels with the rows).
    if old.preview != new.preview || (shift != 0 && new.preview.is_some()) {
        for (y0, y1, _) in [old.preview, new.preview].into_iter().flatten() {
            redraw.push(on_screen(y0, y1));
            redraw.push(on_screen(y0 - shift, y1 - shift));
        }
    }
    // Select mode's bar.
    if old.bar != new.bar {
        redraw.push((bottom, bottom + SELECT_BAR as i32));
    }
    if new.pill.is_some() && (old.pill != new.pill || shift != 0)
        || old.pill.is_some() != new.pill.is_some()
    {
        let top = pill_top(canvas.height as f32) as i32 - 2;
        redraw.push((top, top + 92));
    }
    let merged = super::frame::merge(redraw);
    for &(y0, y1) in &merged {
        canvas.set_clip(0, y0, canvas.width, y1);
        draw_list_with(
            canvas,
            model,
            view,
            new.pressed.as_ref(),
            canvas.height as f32,
            now,
        );
        canvas.reset_clip();
    }
    damage.rows = merged;
    Some(damage)
}

pub(crate) fn draw_list(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    draw_list_with(canvas, model, view, pressed_action(pressed), height, now);
}

/// Draws the list (within the canvas's clip) with `pressed` pressed.
fn draw_list_with(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Action>,
    height: f32,
    now: Instant,
) {
    let p = theme::p();
    let (_, clip_top, _, clip_bottom) = canvas.clip();
    widgets::fill(
        canvas,
        RectF::new(0.0, clip_top as f32, W, (clip_bottom - clip_top) as f32),
        p.sidebar_background,
    );
    let layout = model.sidebar.layout();
    let (top, bottom) = bounds(model, height);
    // Whole pixels, so a scrolled frame equals a shifted one.
    let offset = view.list_scroll.value(now).round();
    let content_top = list_top(model) - offset;
    let state = state(model, &layout);
    let area = area(model, height);
    let lifted = view.lifted.clone();
    {
        let y0 = (clip_top as f32).max(top);
        let y1 = (clip_bottom as f32).min(bottom);
        if y0 < y1 {
            let outer = canvas.clip();
            canvas.set_clip(outer.0, y0 as i32, outer.2, y1 as i32);
            let range = visible(&layout, y0 - content_top, y1 - content_top);
            let searching = model.sidebar.search.active();
            let selecting = model.sidebar.selection.is_some();
            let ctx = Ctx::new(model, pressed, lifted.as_ref(), now);
            for index in range {
                let look = look(model, &ctx, &layout.rows[index]);
                draw_look(canvas, &look, content_top + layout.tops[index]);
            }
            // A lifted row's shadow on the rows around it.
            if let Some(lifted) = &lifted {
                let reach = visible(
                    &layout,
                    y0 - content_top - (LIFT_BLUR + LIFT_DY),
                    y1 - content_top + LIFT_BLUR,
                );
                for index in reach {
                    let row = &layout.rows[index];
                    if row_action(row, searching, selecting).as_ref() == Some(lifted) {
                        let rect =
                            RectF::new(0.0, content_top + layout.tops[index], W, row.height());
                        widgets::shadow(canvas, rect, LIFT_BLUR, LIFT_DY, 0.34);
                    }
                }
            }
            if offset < 0.0 {
                draw_pull(canvas, model, top, -offset, now);
            }
            draw_preview(canvas, model, height);
            if let Some(label) = footer(model)
                && !layout.rows.is_empty()
            {
                draw_footer(canvas, label, content_top + layout.height + S1);
            }
            if searching && layout.rows.len() <= 1 {
                draw_no_results(
                    canvas,
                    RectF::new(0.0, content_top, W, bottom - content_top),
                );
            }
            draw_state(canvas, &state, area, pressed, now);
            canvas.set_clip(outer.0, outer.1, outer.2, outer.3);
        }
    }
    if model.sidebar.selection.is_some() {
        draw_select_bar(canvas, model, pressed, height);
    }
    draw_header(canvas, model, pressed);
    if pill_shows(model) {
        let message = if model.was_connected {
            "Reconnecting…"
        } else {
            "Connecting…"
        };
        widgets::pill(
            canvas,
            W / 2.0,
            pill_top(height),
            message,
            true,
            p.sidebar_background,
            now,
        );
    }
}

// ----- Touches ----------------------------------------------------------------------

/// The row at window `y`, and its top on screen.
fn row_at(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    y: f32,
    now: Instant,
) -> Option<(Row, f32)> {
    let (top, bottom) = bounds(model, height);
    if y < top || y >= bottom {
        return None;
    }
    let layout = model.sidebar.layout();
    let content_top = list_top(model) - view.list_scroll.value(now).round();
    let index = layout.at(y - content_top)?;
    Some((layout.rows[index].clone(), content_top + layout.tops[index]))
}

/// The list's target at (`x`, `y`).
pub fn hit(
    model: &mut Model,
    view: &mut ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Hit> {
    if y < TOP_BAR {
        return header_hit(model, x, y).map(Hit::Action);
    }
    if let Some(action) = select_bar_hit(model, height, x, y) {
        return Some(Hit::Action(action));
    }
    let layout = model.sidebar.layout();
    let state = state(model, &layout);
    if let Some((_, act, rect)) = state_button(&state, area(model, height)) {
        return rect
            .contains(x, y)
            .then_some(Hit::Action(Action::List(act)));
    }
    let searching = model.sidebar.search.active();
    let selecting = model.sidebar.selection.is_some();
    match row_at(model, view, height, y, now)? {
        (Row::Folder { id, .. }, _) if x >= W - TARGET && !selecting => {
            Some(Hit::Action(Action::List(Act::NewChatIn(id))))
        }
        (Row::Conversation { id, .. }, _) if !selecting && on_agents(model, &id, x) => {
            Some(Hit::Action(Action::List(Act::Subagents(id))))
        }
        (row, _) => row_action(&row, searching, selecting).map(Hit::Action),
    }
}

/// Whether `x` is on the ◆ badge of conversation `id` (a 122 px target).
fn on_agents(model: &mut Model, id: &str, x: f32) -> bool {
    let Some(c) = model.sidebar.conversations.get(id) else {
        return false;
    };
    if !c.tasks.iter().any(|t| t.kind == "subagent") {
        return false;
    }
    let aggregates = model.sidebar.aggregates(&model.composer.queued);
    let Some(c) = model.sidebar.conversations.get(id) else {
        return false;
    };
    let folder_muted = c
        .folder_id
        .as_ref()
        .is_some_and(|f| aggregates.muted.contains(f));
    let queue = aggregates.queues.get(id).copied().unwrap_or_default();
    let a = activity::conversation(c, queue, false, folder_muted);
    agents_badge((a.agents, a.commands, a.chrono), RIGHT).is_some_and(|(x0, x1)| {
        let cx = (x0 + x1) / 2.0;
        let half = ((x1 - x0) / 2.0).max(TARGET / 2.0);
        (x - cx).abs() <= half
    })
}

/// The row a press held at (`x`, `y`) lifts.
pub fn liftable(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Action> {
    // Select mode taps, it does not lift.
    if model.sidebar.selection.is_some() {
        return None;
    }
    let searching = model.sidebar.search.active();
    match row_at(model, view, height, y, now)? {
        (Row::Folder { .. }, _) if x >= W - TARGET => None,
        (row, _) => row_action(&row, searching, false),
    }
}

/// What a long press at (`x`, `y`) opens: a row's menu, anchored to it.
pub fn long_press(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Action> {
    liftable(model, view, height, x, y, now)?;
    let (row, row_y) = row_at(model, view, height, y, now)?;
    let rect = RectF::new(0.0, row_y.round(), W, ROW_H);
    let key = row_key(&row)?;
    // Held: the menu opens; moved on far enough, it becomes a drag.
    model.sidebar.drag = Some(Drag {
        key,
        start: (x, y),
        at: (x, y),
        active: false,
        target: None,
        speed: 0.0,
        stepped: None,
    });
    match row {
        Row::Conversation { id, .. } => Some(Action::List(Act::RowMenu { id, rect })),
        Row::Folder { id, .. } => Some(Action::List(Act::FolderMenu { id, rect })),
        _ => None,
    }
}

// ----- Dragging a row (PAR-070, 071) ------------------------------------------

/// How far a held row moves before its menu gives way to a drag (16 pt).
const DRAG_START: f32 = 44.0;
/// The edge zones that scroll the list under a dragged row (76 pt), and
/// the fastest they scroll (1,240 pt/s).
const EDGE_ZONE: f32 = 210.0;
const EDGE_SPEED: f32 = 3434.0;

/// Where a dragged `key` would land with the finger at window `y`: a folder
/// it is not in (and not itself or below itself), or next to a conversation
/// of its pinned group (never while searching).
fn drop_target(
    model: &mut Model,
    view: &ViewState,
    height: f32,
    key: &Key,
    y: f32,
    now: Instant,
) -> Option<DropTarget> {
    let (row, row_y) = row_at(model, view, height, y, now)?;
    if row_key(&row).as_ref() == Some(key) {
        return None;
    }
    let sidebar = &model.sidebar;
    let source_parent = sidebar.parent_of(key)?;
    let source_pinned = sidebar.pinned_of(key)?;
    let below_source = |folder: Option<&str>| match (key, folder) {
        (Key::Folder(source), Some(folder)) => sidebar.descendant_folders(source).contains(folder),
        _ => false,
    };
    match row {
        Row::Folder { id, .. } => {
            if source_parent.as_deref() == Some(id.as_str()) || below_source(Some(&id)) {
                return None;
            }
            Some(DropTarget::Folder(id))
        }
        Row::Conversation { id, .. } => {
            if sidebar.search.active() {
                return None;
            }
            let c = sidebar.conversations.get(&id)?;
            if c.pinned != source_pinned || below_source(c.folder_id.as_deref()) {
                return None;
            }
            Some(DropTarget::Reorder {
                key: Key::Conv(id),
                after: y >= row_y + ROW_H / 2.0,
            })
        }
        _ => None,
    }
}

/// The dragged row's drop target again (the finger or the rows moved);
/// a selection haptic when it changes to another.
fn retarget(model: &mut Model, view: &ViewState, height: f32, now: Instant) {
    let Some(drag) = model.sidebar.drag.clone().filter(|drag| drag.active) else {
        return;
    };
    let target = drop_target(model, view, height, &drag.key, drag.at.1, now);
    if target != drag.target {
        if target.is_some() {
            model.ui.haptic(super::super::ui::Haptic::Selection);
        }
        if let Some(drag) = &mut model.sidebar.drag {
            drag.target = target;
        }
        model.sidebar.changed();
    }
}

/// A held row's gesture after its long press: moved, it is dragged (the
/// menu closes); let go over a target, it moves there. None when the
/// gesture is not a drag's (let go without moving: the menu stays).
/// Returns whether anything shows differently, and commands to send.
pub fn drag(
    model: &mut Model,
    view: &mut ViewState,
    height: f32,
    gesture: &super::super::gesture::Gesture,
    now: Instant,
) -> Option<(bool, Vec<serde_json::Value>)> {
    use super::super::gesture::Gesture;
    match *gesture {
        Gesture::LongPressMove { x, y } => {
            let drag = model.sidebar.drag.as_mut()?;
            drag.at = (x, y);
            if !drag.active {
                let (dx, dy) = (x - drag.start.0, y - drag.start.1);
                if (dx * dx + dy * dy).sqrt() < DRAG_START {
                    return Some((false, Vec::new()));
                }
                // The menu gives way to the dragged row.
                drag.active = true;
                model.ui.close_all = true;
            }
            // Near an edge, the list scrolls under the finger.
            let (top, bottom) = bounds(model, height);
            let near_top = ((top + EDGE_ZONE - y) / EDGE_ZONE).clamp(0.0, 1.0);
            let near_bottom = ((y - (bottom - EDGE_ZONE)) / EDGE_ZONE).clamp(0.0, 1.0);
            let speed = EDGE_SPEED * (near_bottom.powf(1.45) - near_top.powf(1.45));
            if let Some(drag) = &mut model.sidebar.drag {
                drag.speed = speed;
                if drag.stepped.is_none() || speed == 0.0 {
                    drag.stepped = Some(now);
                }
            }
            retarget(model, view, height, now);
            model.sidebar.changed();
            Some((true, Vec::new()))
        }
        Gesture::LongPressEnd { .. } | Gesture::Cancel => {
            let drag = model.sidebar.drag.take()?;
            if !drag.active {
                return None;
            }
            view.lifted = None;
            model.sidebar.changed();
            let commands = match (drag.target, gesture) {
                (Some(target), Gesture::LongPressEnd { .. }) => model.list_drop(drag.key, target),
                _ => Vec::new(),
            };
            Some((true, commands))
        }
        _ => None,
    }
}

/// The dragged row's preview: an icon and its title in a chip 24 pt above
/// the finger, inside the list.
fn preview_rect(model: &Model, height: f32) -> Option<(RectF, String, bool)> {
    let drag = model.sidebar.drag.as_ref().filter(|drag| drag.active)?;
    let (title, folder) = match &drag.key {
        Key::Conv(id) => (
            model
                .sidebar
                .conversations
                .get(id)
                .and_then(activity::row_title)
                .unwrap_or("Untitled chat")
                .to_owned(),
            false,
        ),
        Key::Folder(id) => (model.sidebar.folders.get(id)?.name.clone(), true),
    };
    let (top, bottom) = bounds(model, height);
    let ty = T_ROW.weight(Weight::MonoSemiBold);
    let w = (widgets::measure(&title, ty) + 44.0 + 60.0 + 28.0)
        .min(637.0)
        .round();
    let h = 110.0;
    let cx = drag.at.0.clamp(w / 2.0 + 34.0, W - w / 2.0 - 34.0);
    let cy = (drag.at.1 - 66.0).clamp(top + h / 2.0 + 11.0, bottom - h / 2.0 - 11.0);
    let rect = RectF::new((cx - w / 2.0).round(), (cy - h / 2.0).round(), w, h);
    Some((rect, title, folder))
}

fn draw_preview(canvas: &mut Canvas, model: &Model, height: f32) {
    let Some((rect, title, folder)) = preview_rect(model, height) else {
        return;
    };
    if !widgets::shows(canvas, rect.y, rect.y + rect.h) {
        return;
    }
    let p = theme::p();
    widgets::boxed(canvas, rect, p.surface_background, p.accent);
    let icon = if folder { Icon::FolderFill } else { Icon::Chat };
    icons::draw(
        canvas,
        icon,
        rect.x + 22.0 + 30.0,
        rect.y + rect.h / 2.0,
        44.0,
        p.secondary_accent,
        1.0,
    );
    let ty = T_ROW.weight(Weight::MonoSemiBold);
    let x = rect.x + 22.0 + 60.0 + 17.0;
    let shown = widgets::fit(&title, ty, rect.x + rect.w - 22.0 - x);
    widgets::text(
        canvas,
        &shown,
        x,
        ty.baseline(rect.y + (rect.h - ty.line()) / 2.0),
        ty,
        p.text,
        1.0,
    );
}

/// A drag near an edge scrolls the list, a frame at a time; true when it
/// moved.
fn auto_scroll(model: &mut Model, view: &mut ViewState, height: f32, now: Instant) -> bool {
    let Some(drag) = model.sidebar.drag.as_mut().filter(|drag| drag.active) else {
        return false;
    };
    if drag.speed == 0.0 {
        return false;
    }
    let last = drag.stepped.replace(now).unwrap_or(now);
    let dt = now
        .saturating_duration_since(last)
        .as_secs_f32()
        .clamp(1.0 / 120.0, 1.0 / 30.0);
    let step = drag.speed * dt;
    let max = list_max(model, height);
    let offset = view.list_scroll.value(now);
    let next = (offset + step).clamp(0.0, max).round();
    if next == offset.round() {
        return false;
    }
    view.list_scroll.reset();
    view.list_scroll.offset = next;
    // The rows moved under a resting finger.
    retarget(model, view, height, now);
    true
}

/// Before a frame: the search follows the typing, its results start at
/// their top (the tree comes back where it was), a revealed row comes into
/// view, the scroll stays inside a list that got shorter (a folder closed),
/// and a pull past the top, let go, asks for the list again (PAR-062).
/// Returns whether anything changed, and commands to send.
pub fn settle(
    model: &mut Model,
    view: &mut ViewState,
    height: f32,
    now: Instant,
) -> (bool, Vec<serde_json::Value>) {
    let mut commands = Vec::new();
    if model.page == Page::List {
        let offset = view.list_scroll.value(now);
        if view.list_scroll.dragging() {
            model.sidebar.pull_armed = -offset >= PULL;
        } else if std::mem::take(&mut model.sidebar.pull_armed) {
            model.ui.haptic(super::super::ui::Haptic::Light);
            commands = model.refresh_list();
        }
    }
    (settle_scroll(model, view, height, now), commands)
}

fn settle_scroll(model: &mut Model, view: &mut ViewState, height: f32, now: Instant) -> bool {
    let mut changed = model.list_tick(now);
    if model.page != Page::List {
        return changed;
    }
    changed |= auto_scroll(model, view, height, now);
    let max = list_max(model, height);
    // Search results start at their top; the tree comes back where it was.
    if std::mem::take(&mut model.sidebar.scroll_top) {
        let offset = view.list_scroll.value(now);
        let target = if model.sidebar.search.active() {
            model.sidebar.tree_scroll.get_or_insert(offset);
            0.0
        } else {
            model.sidebar.tree_scroll.take().unwrap_or(0.0)
        };
        view.list_scroll.reset();
        view.list_scroll.offset = target.clamp(0.0, max).round();
        changed = true;
    }
    if let Some(row) = model.sidebar.reveal.take() {
        let offset = view.list_scroll.value(now);
        if let Some(target) = reveal_target(model, &row, height, offset) {
            view.list_scroll.reset();
            view.list_scroll.offset = target.clamp(0.0, max).round();
            return true;
        }
        return changed;
    }
    if !view.list_scroll.dragging() && view.list_scroll.value(now) > max + 0.5 {
        view.list_scroll.reset();
        view.list_scroll.offset = max.round();
        changed = true;
    }
    changed
}

/// The scroll that brings `row` into view from `offset`, if it is not.
fn reveal_target(model: &mut Model, row: &Row, height: f32, offset: f32) -> Option<f32> {
    let id = match row {
        Row::Conversation { id, .. } | Row::Folder { id, .. } => id,
        _ => return None,
    };
    let layout = model.sidebar.layout();
    let index = layout.index_of(|r| match r {
        Row::Conversation { id: other, .. } | Row::Folder { id: other, .. } => other == id,
        _ => false,
    })?;
    let (top, bottom) = bounds(model, height);
    let window = bottom - top - S1;
    let row_top = layout.tops[index];
    let row_bottom = row_top + layout.rows[index].height();
    if row_top < offset {
        Some(row_top - theme::S3)
    } else if row_bottom > offset + window {
        Some(row_bottom - window + theme::S3)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::view::draw;

    const H: f32 = 2216.0;
    const BG: Rgb = 0x123456;

    /// Ready, with 40 conversations (two pinned, some streaming, unread,
    /// marked or muted) and a folder of 12 with a subfolder.
    fn model() -> Model {
        let mut model = Model::new();
        let bootstrap = model.on_connected();
        let ping = bootstrap.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        let now = Instant::now();
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            now,
        );
        let conversations: Vec<String> = (0..52)
            .map(|i| {
                let folder = match i {
                    40..46 => r#""f""#,
                    46.. => r#""g""#,
                    _ => "null",
                };
                format!(
                    r#"{{"id":"c{i}","title":"{}Conversation number {i}","pinned":{},"sortOrder":{i},"streaming":{},"unread":{},"marked":{},"muted":{},"messageCount":{},"updatedAt":{},"folderId":{folder},"subagentCount":{}}}"#,
                    if i == 5 { "🔥 " } else { "" },
                    i < 2,
                    i % 7 == 0,
                    i % 3 == 0,
                    i % 5 == 0,
                    i % 11 == 0,
                    i * 37,
                    crate::exocortex::model::now_ms() - i as u64 * 3_600_000,
                    i % 4,
                )
            })
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversations_list","conversations":[{}],"folders":[{{"id":"f","name":"Work","sortOrder":3}},{{"id":"g","name":"Deep","parentId":"f"}}]}}"#,
                conversations.join(",")
            )),
            now,
        );
        model
    }

    fn full(
        model: &mut Model,
        view: &mut ViewState,
        pressed: Option<&Hit>,
        now: Instant,
    ) -> Canvas {
        let mut canvas = Canvas::new(1080, H as i32);
        canvas.clear(BG);
        draw(&mut canvas, model, view, pressed, now);
        canvas
    }

    /// Draws a frame, changes the state, draws the next incrementally:
    /// it must equal a full redraw and change nothing outside its damage.
    fn update_matches(
        model: &mut Model,
        view: &mut ViewState,
        (before, after): (Option<&Hit>, Option<&Hit>),
        (t0, t1): (Instant, Instant),
        change: impl FnOnce(&mut Model, &mut ViewState),
    ) -> FrameDamage {
        let mut canvas = full(model, view, before, t0);
        let old = list_frame(model, view, before, H, t0).unwrap();
        let mut shadow = canvas.pixels.clone();
        change(model, view);
        let new = list_frame(model, view, after, H, t1).unwrap();
        let damage =
            draw_list_update(&mut canvas, model, view, &old, &new, t1).expect("incremental");
        let whole = full(model, view, after, t1);
        let differing: Vec<usize> = (0..H as usize)
            .filter(|y| {
                whole.pixels[y * 1080..(y + 1) * 1080] != canvas.pixels[y * 1080..(y + 1) * 1080]
            })
            .collect();
        assert!(
            differing.is_empty(),
            "differs from a full redraw in rows {:?}..{:?}: {damage:?}",
            differing.first(),
            differing.last()
        );
        if let Some((y0, y1, dy)) = damage.moved {
            let rows = y1 - y0 - dy.abs();
            let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
            shadow.copy_within(
                (src * 1080) as usize..((src + rows) * 1080) as usize,
                (dst * 1080) as usize,
            );
        }
        for y in 0..H as i32 {
            if damage.rows.iter().any(|(y0, y1)| (*y0..*y1).contains(&y)) {
                continue;
            }
            let row = (y * 1080) as usize..((y + 1) * 1080) as usize;
            assert!(
                shadow[row.clone()] == canvas.pixels[row],
                "row {y} changed outside the damage"
            );
        }
        damage
    }

    fn drawn(damage: &FrameDamage) -> i32 {
        damage.rows.iter().map(|(a, b)| b - a).sum()
    }

    fn act(act: Act) -> Hit {
        Hit::Action(Action::List(act))
    }

    #[test]
    fn scrolling_moves_rows_and_draws_the_uncovered_strip() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        for to in [161.0, 140.0, 900.0, 600.0, 0.0] {
            let damage = update_matches(
                &mut model,
                &mut view,
                (None, None),
                (now, now),
                |_, view| {
                    view.list_scroll.offset = to;
                },
            );
            if let Some((_, _, dy)) = damage.moved {
                assert!(drawn(&damage) <= dy.abs() + 1, "{damage:?}");
            }
        }
        let damage = update_matches(&mut model, &mut view, (None, None), (now, now), |_, _| {});
        assert_eq!(damage, FrameDamage::default());
    }

    /// Relative times read the frames' clock (`now`), not the wall clock
    /// at drawing: a label turning over redraws its row, and a frame and
    /// its redraw agree across a minute's turn.
    #[test]
    fn relative_times_turn_over_with_the_frames_clock() {
        let mut model = model();
        let mut view = ViewState::new();
        let t0 = Instant::now();
        // c2 was updated 59.5 s before t0, by the frames' clock.
        let updated = model.sidebar.conversations["c2"].updated_at;
        model.sidebar.clock = Some((t0, (updated + 59_500.0) as u64));
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (t0, t0 + Duration::from_secs(1)),
            |_, _| {},
        );
        let layout = model.sidebar.layout();
        let index = layout
            .index_of(|row| matches!(row, Row::Conversation { id, .. } if id == "c2"))
            .unwrap();
        let y = (list_top(&model) + layout.tops[index]) as i32;
        assert!(
            damage
                .rows
                .iter()
                .any(|(y0, y1)| *y0 <= y + 124 && *y1 >= y + 124),
            "c2's meta line drawn again: {damage:?}"
        );
        // Unchanged time: nothing, whatever the wall clock does.
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (t0 + Duration::from_secs(1), t0 + Duration::from_secs(1)),
            |_, _| {},
        );
        assert!(
            damage.rows.iter().all(|(y0, y1)| y1 - y0 <= 2 * DOT_HALF),
            "{damage:?}"
        );
    }

    #[test]
    fn a_pulse_draws_only_the_dots() {
        let mut model = model();
        let mut view = ViewState::new();
        let t0 = Instant::now();
        let mut redrawn = 0;
        for step in 1..=12u64 {
            let at = t0 + Duration::from_millis(step * 34);
            let before = at - Duration::from_millis(34);
            let damage =
                update_matches(&mut model, &mut view, (None, None), (before, at), |_, _| {});
            for (y0, y1) in &damage.rows {
                assert!(y1 - y0 <= 2 * DOT_HALF, "only dots: {damage:?}");
            }
            redrawn += damage.rows.len();
        }
        assert!(redrawn > 0, "the dots pulsed");
    }

    #[test]
    fn pressing_typing_and_updates_draw_only_their_rows() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let press = act(Act::Open("c4".into()));
        for (before, after) in [(None, Some(&press)), (Some(&press), None)] {
            let damage = update_matches(
                &mut model,
                &mut view,
                (before, after),
                (now, now),
                |_, _| {},
            );
            assert_eq!(damage.rows.len(), 1);
            assert!(drawn(&damage) <= ROW_H as i32);
        }
        // A key in the search field: the header.
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (now, now),
            |model, _| {
                model.handle(Action::List(Act::Search));
                model.handle(Action::List(Act::Key(crate::keys::KeyInput::Character(
                    'x',
                ))));
            },
        );
        assert_eq!(damage.rows, [widgets::TOP_BAR_ROWS]);
        model.handle(Action::List(Act::ClearSearch));
        model.handle(Action::List(Act::Key(crate::keys::KeyInput::Enter)));
        // A conversation renamed: its row.
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (now, now),
            |model, _| {
                model.apply(
                parse_event(r#"{"type":"conversation_updated","summary":{"id":"c6","title":"Renamed","sortOrder":6,"messageCount":3}}"#),
                now,
            );
            },
        );
        assert!(drawn(&damage) <= ROW_H as i32 + 2 * DOT_HALF, "{damage:?}");
        // A folder opened in place: the rows under it.
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (now, now),
            |model, _| {
                model.handle(Action::List(Act::Toggle("f".into())));
            },
        );
        assert!(damage.moved.is_none() && !damage.rows.is_empty());
    }

    #[test]
    fn a_folders_chevron_turns_drawing_only_its_row() {
        let mut model = model();
        let mut view = ViewState::new();
        model.handle(Action::List(Act::Toggle("f".into())));
        let (_, at) = model.sidebar.toggled.clone().unwrap();
        let mut turned = 0;
        for step in 1..=40u64 {
            let (t0, t1) = (
                at + Duration::from_millis((step - 1) * 8),
                at + Duration::from_millis(step * 8),
            );
            let damage = update_matches(&mut model, &mut view, (None, None), (t0, t1), |_, _| {});
            // The folder's row (and pulsing dots elsewhere).
            assert!(
                drawn(&damage) <= ROW_H as i32 + 8 * 2 * DOT_HALF,
                "{damage:?}"
            );
            turned += damage
                .rows
                .iter()
                .filter(|(a, b)| b - a == ROW_H as i32)
                .count();
        }
        assert!(turned > 5, "the chevron turned over several frames");
    }

    #[test]
    fn a_lifted_row_casts_its_shadow_on_its_neighbours() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let lift = Action::List(Act::Open("c4".into()));
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (now, now),
            |_, view| {
                view.lifted = Some(lift.clone());
            },
        );
        // The row and the shadow's reach around it, nothing more.
        assert!(
            drawn(&damage) <= ROW_H as i32 + (2.0 * LIFT_BLUR) as i32 + 4 + 2 * 2 * DOT_HALF,
            "{damage:?}"
        );
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (now, now),
            |_, view| {
                view.lifted = None;
            },
        );
        assert!(!damage.rows.is_empty());
    }

    #[test]
    fn select_mode_checks_rows_and_counts_them_in_its_bar() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        model.handle(Action::List(Act::Select(None)));
        let layout = model.sidebar.layout();
        let y = list_top(&model) + layout.tops[3] + 40.0;
        let Some(Hit::Action(toggle)) = hit(&mut model, &mut view, H, 400.0, y, now) else {
            panic!("a row");
        };
        assert!(matches!(toggle, Action::List(Act::ToggleSelect(_))));
        assert_eq!(liftable(&mut model, &view, H, 400.0, y, now), None);
        let damage = update_matches(
            &mut model,
            &mut view,
            (None, None),
            (now, now),
            |model, _| {
                model.handle(toggle.clone());
            },
        );
        // The row, and the bar's count.
        assert!(
            drawn(&damage) <= ROW_H as i32 + SELECT_BAR as i32 + 2 * 2 * DOT_HALF,
            "{damage:?}"
        );
        // The bar's buttons, now that something is selected.
        let bar = bottom(H) - SELECT_BAR + 100.0;
        assert_eq!(
            hit(&mut model, &mut view, H, 100.0, bar, now),
            Some(act(Act::SelectMove))
        );
        assert_eq!(
            hit(&mut model, &mut view, H, 1000.0, bar, now),
            Some(act(Act::SelectDelete))
        );
        assert_eq!(
            hit(&mut model, &mut view, H, 900.0, 60.0, now),
            Some(act(Act::EndSelect))
        );
    }

    #[test]
    fn a_pull_past_the_top_opens_a_gap_and_asks_for_the_list() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        for to in [-30.0, -60.0, -100.0, -40.0, 0.0] {
            update_matches(
                &mut model,
                &mut view,
                (None, None),
                (now, now),
                |_, view| {
                    view.list_scroll.offset = to;
                },
            );
        }
        // Dragged past the threshold and let go: the list again.
        view.list_scroll.begin(500.0, now);
        view.list_scroll
            .moved(500.0 + 3.0 * (PULL + 10.0), 1000.0, now);
        let (_, commands) = settle(&mut model, &mut view, H, now);
        assert!(commands.is_empty() && model.sidebar.pull_armed);
        view.list_scroll.end(1000.0, now);
        let (_, commands) = settle(&mut model, &mut view, H, now);
        assert_eq!(commands[0]["type"], "list_conversations");
        assert!(model.list_refreshing());
    }

    /// Pixel rows where two frames differ.
    fn differing(a: &Canvas, b: &Canvas) -> Vec<f32> {
        (0..H as usize)
            .filter(|y| a.pixels[y * 1080..(y + 1) * 1080] != b.pixels[y * 1080..(y + 1) * 1080])
            .map(|y| y as f32)
            .collect()
    }

    /// Pixel rows in `y0..y1` where `color` is drawn (text ink).
    fn ink(canvas: &Canvas, y0: f32, y1: f32, color: Rgb) -> Vec<f32> {
        (y0 as usize..y1 as usize)
            .filter(|y| canvas.pixels[y * 1080..(y + 1) * 1080].contains(&color))
            .map(|y| y as f32)
            .collect()
    }

    /// Taps where each row is drawn — its title's ink, its top and bottom
    /// pixel rows, scrolled or not — land on that row; a header or a
    /// divider takes no tap and gives none to its neighbours.
    #[test]
    fn taps_land_on_the_row_drawn_there() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        model.handle(Action::List(Act::Toggle("f".into())));
        let text = theme::p().text;
        for offset in [0.0, 37.0, 311.0, 1500.0] {
            view.list_scroll.offset = offset;
            let base = full(&mut model, &mut view, None, now);
            let layout = model.sidebar.layout();
            let content_top = list_top(&model) - offset;
            let (top, bottom) = bounds(&model, H);
            let mut checked = 0;
            for (index, row) in layout.rows.iter().enumerate() {
                let y = content_top + layout.tops[index];
                // Rows wholly on screen.
                if y < top || y + row.height() > bottom {
                    continue;
                }
                let Some(action) = row_action(row, false, false) else {
                    // A header or a divider: nothing, wherever it is drawn.
                    for ty in [y + 1.0, y + row.height() / 2.0, y + row.height() - 1.0] {
                        assert_eq!(
                            hit(&mut model, &mut view, H, 300.0, ty, now),
                            None,
                            "{row:?} at {ty}"
                        );
                    }
                    continue;
                };
                // Where the row is drawn: what its pressed face changes.
                let pressed = Hit::Action(action.clone());
                let lit = full(&mut model, &mut view, Some(&pressed), now);
                let drawn = differing(&base, &lit);
                let (y0, y1) = (drawn[0], *drawn.last().unwrap());
                assert!(y1 - y0 >= ROW_H - 2.0, "{row:?} drawn at {y0}..{y1}");
                let title = ink(&base, y0, y0 + ROW_H / 2.0, text);
                assert!(
                    !title.is_empty(),
                    "{row:?}: its title is drawn in its upper half"
                );
                let title_mid = title[title.len() / 2];
                for ty in [y0, title_mid, (y0 + y1) / 2.0, y1] {
                    for tx in [60.0, 300.0, 700.0] {
                        assert_eq!(
                            hit(&mut model, &mut view, H, tx, ty, now),
                            Some(pressed.clone()),
                            "{row:?} drawn at {y0}..{y1} (title at {title_mid}), tapped at ({tx}, {ty}) with the list at {offset}"
                        );
                    }
                }
                // A folder's "+" at its right; the row elsewhere.
                if let Row::Folder { id, .. } = row {
                    assert_eq!(
                        hit(
                            &mut model,
                            &mut view,
                            H,
                            FOLDER_PLUS_CX,
                            (y0 + y1) / 2.0,
                            now
                        ),
                        Some(act(Act::NewChatIn(id.clone())))
                    );
                }
                checked += 1;
            }
            assert!(checked >= 8, "rows checked at {offset}: {checked}");
        }
    }

    /// A long-pressed row's menu: each item answers where it is drawn (its
    /// pressed face), the quick row's cells included.
    #[test]
    fn menu_items_answer_where_they_are_drawn() {
        use crate::exocortex::gesture::Gesture;
        use crate::exocortex::ui::overlay::Response;
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let layout = model.sidebar.layout();
        let y = list_top(&model) + layout.tops[1] + 80.0;
        let Some(menu) = long_press(&mut model, &view, H, 300.0, y, now) else {
            panic!("a row's menu");
        };
        model.handle(menu);
        view.take_ui(&mut model, now);
        // Opened at `now` (its first layout), drawn at rest.
        super::super::frame::record(&mut model, &mut view, None, (1080, H as i32), now);
        let later = now + Duration::from_millis(400);
        // As a frame is: laid out (the overlays place themselves), then drawn.
        let draw = |model: &mut Model, view: &mut ViewState| {
            super::super::frame::record(model, view, None, (1080, H as i32), later);
            full(model, view, None, later)
        };
        let base = draw(&mut model, &mut view);
        let expected = [
            "Pin",
            "Mark",
            "Mute",
            "Rename",
            "MarkEmoji",
            "MovePicker",
            "Clone",
            "Select",
            "Copy",
            "ConfirmDelete",
        ];
        let mut found = Vec::new();
        // Probe a grid; each press lights one item's cell.
        let mut seen_cells: Vec<(f32, f32, f32)> = Vec::new();
        for gy in (150..2000).step_by(46) {
            for gx in [120.0, 340.0, 560.0] {
                let gy = gy as f32;
                let mut overlays = std::mem::take(&mut view.overlays);
                let cx = view.cx(&model, 1080.0, H, later);
                overlays.gesture(&Gesture::Down { x: gx, y: gy }, &cx);
                view.overlays = overlays;
                let lit = draw(&mut model, &mut view);
                let mut overlays = std::mem::take(&mut view.overlays);
                let cx = view.cx(&model, 1080.0, H, later);
                overlays.gesture(&Gesture::Cancel, &cx);
                view.overlays = overlays;
                let rows = differing(&base, &lit);
                let Some((&y0, &y1)) = rows.first().zip(rows.last()) else {
                    continue;
                };
                assert!(gy >= y0 && gy <= y1, "pressed at {gy}, lit {y0}..{y1}");
                if seen_cells.contains(&(y0, y1, gx)) {
                    continue;
                }
                seen_cells.push((y0, y1, gx));
                // A tap there performs that item.
                let mut overlays = std::mem::take(&mut view.overlays);
                let cx = view.cx(&model, 1080.0, H, later);
                overlays.gesture(&Gesture::Down { x: gx, y: gy }, &cx);
                let response = overlays.gesture(&Gesture::Tap { x: gx, y: gy }, &cx);
                let Response::Action(action) = response else {
                    panic!("no item at {gx},{gy}: {response:?}");
                };
                // The variant: `List(Pin(…))` → Pin, `Ui(Copy(…))` → Copy.
                let name = format!("{action:?}");
                let inner = name.split_once('(').map_or("", |(_, rest)| rest);
                let variant: String = inner.chars().take_while(|c| c.is_alphanumeric()).collect();
                if let Some(item) = expected.iter().find(|e| **e == variant) {
                    if !found.contains(item) {
                        found.push(*item);
                    }
                }
                // Open it again for the next probe.
                overlays.sweep(later + Duration::from_secs(5));
                view.overlays = overlays;
                let again = long_press(&mut model, &view, H, 300.0, y, now).unwrap();
                model.handle(again);
                view.take_ui(&mut model, now);
                super::super::frame::record(&mut model, &mut view, None, (1080, H as i32), now);
            }
        }
        assert_eq!(found.len(), expected.len(), "items answered: {found:?}");
    }

    /// Drags `from` (window y) to each of `to` and lets go; returns the
    /// commands and the drag's target before the end.
    fn drag_to(
        model: &mut Model,
        view: &mut ViewState,
        from: f32,
        to: &[f32],
        now: Instant,
    ) -> (Option<DropTarget>, Vec<serde_json::Value>) {
        use crate::exocortex::gesture::Gesture;
        let menu = long_press(model, view, H, 300.0, from, now).expect("a row's menu");
        model.handle(menu);
        view.take_ui(model, now);
        for &y in to {
            drag(model, view, H, &Gesture::LongPressMove { x: 300.0, y }, now);
        }
        let target = model.sidebar.drag.as_ref().and_then(|d| d.target.clone());
        let end = drag(
            model,
            view,
            H,
            &Gesture::LongPressEnd {
                x: 300.0,
                y: *to.last().unwrap(),
            },
            now,
        );
        (
            target,
            end.map(|(_, commands)| commands).unwrap_or_default(),
        )
    }

    fn row_y(model: &mut Model, row: impl Fn(&Row) -> bool) -> f32 {
        let layout = model.sidebar.layout();
        let index = layout.index_of(row).unwrap();
        list_top(model) + layout.tops[index]
    }

    #[test]
    fn a_held_row_dragged_onto_a_folder_moves_into_it() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let from = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c4"),
        ) + 80.0;
        let folder = row_y(
            &mut model,
            |r| matches!(r, Row::Folder { id, .. } if id == "f"),
        ) + 80.0;
        // A small move keeps the menu (no drag yet).
        let (target, commands) = drag_to(&mut model, &mut view, from, &[from + 20.0], now);
        assert_eq!((target, commands.len()), (None, 0));
        assert!(model.sidebar.drag.is_none());
        let (target, commands) = drag_to(&mut model, &mut view, from, &[from + 60.0, folder], now);
        assert_eq!(target, Some(DropTarget::Folder("f".into())));
        assert_eq!(commands[0]["type"], "move_sidebar_items");
        assert_eq!(commands[0]["parentId"], "f");
        assert!(commands[0].get("before").is_none());
        // Shown there at once, the folder opened.
        assert_eq!(
            model.sidebar.conversations["c4"].folder_id.as_deref(),
            Some("f")
        );
        assert!(model.sidebar.expanded.contains("f"));
    }

    #[test]
    fn a_held_row_dragged_between_rows_reorders_in_its_group() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let from = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c9"),
        ) + 80.0;
        let c4 = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c4"),
        );
        // The top half of c4: before it.
        let (target, commands) =
            drag_to(&mut model, &mut view, from, &[from - 60.0, c4 + 30.0], now);
        assert_eq!(
            target,
            Some(DropTarget::Reorder {
                key: Key::Conv("c4".into()),
                after: false
            })
        );
        assert_eq!(commands[0]["before"]["id"], "c4");
        assert_eq!(commands[0]["preservePinned"], true);
        let (c3, c4s, c9) = (
            model.sidebar.conversations["c3"].sort_order,
            model.sidebar.conversations["c4"].sort_order,
            model.sidebar.conversations["c9"].sort_order,
        );
        assert!(c3 < c9 && c9 < c4s, "between c3 and c4: {c3} {c9} {c4s}");
        // A pinned row takes no unpinned reorder; the bottom half: after.
        let c0 = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c0"),
        );
        let from = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c6"),
        ) + 80.0;
        let (target, _) = drag_to(&mut model, &mut view, from, &[from - 60.0, c0 + 120.0], now);
        assert_eq!(target, None);
        let c7 = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c7"),
        );
        let (target, commands) =
            drag_to(&mut model, &mut view, from, &[from + 60.0, c7 + 120.0], now);
        assert_eq!(
            target,
            Some(DropTarget::Reorder {
                key: Key::Conv("c7".into()),
                after: true
            })
        );
        // After c7: before the one after it.
        assert_eq!(commands[0]["before"]["id"], "c8");
    }

    #[test]
    fn a_drag_near_the_bottom_scrolls_the_list() {
        use crate::exocortex::gesture::Gesture;
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let from = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c4"),
        ) + 80.0;
        let menu = long_press(&mut model, &view, H, 300.0, from, now).unwrap();
        model.handle(menu);
        let (_, bottom) = bounds(&model, H);
        drag(
            &mut model,
            &mut view,
            H,
            &Gesture::LongPressMove {
                x: 300.0,
                y: bottom - 20.0,
            },
            now,
        );
        assert!(model.sidebar.drag.as_ref().unwrap().speed > 2000.0);
        let mut at = now;
        for _ in 0..30 {
            at += Duration::from_millis(16);
            let _ = settle(&mut model, &mut view, H, at);
        }
        assert!(
            view.list_scroll.offset > 500.0,
            "scrolled to {}",
            view.list_scroll.offset
        );
        // Back in the middle: it stops.
        drag(
            &mut model,
            &mut view,
            H,
            &Gesture::LongPressMove {
                x: 300.0,
                y: H / 2.0,
            },
            at,
        );
        let still = view.list_scroll.offset;
        let _ = settle(&mut model, &mut view, H, at + Duration::from_millis(16));
        assert_eq!(view.list_scroll.offset, still);
        drag(&mut model, &mut view, H, &Gesture::Cancel, at);
        assert!(model.sidebar.drag.is_none());
    }

    #[test]
    fn drag_frames_draw_only_what_moved() {
        use crate::exocortex::gesture::Gesture;
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let from = row_y(
            &mut model,
            |r| matches!(r, Row::Conversation { id, .. } if id == "c4"),
        ) + 80.0;
        let menu = long_press(&mut model, &view, H, 300.0, from, now).unwrap();
        model.handle(menu);
        // Under way (the menu gone).
        drag(
            &mut model,
            &mut view,
            H,
            &Gesture::LongPressMove {
                x: 300.0,
                y: from + 80.0,
            },
            now,
        );
        model.ui.close_all = false;
        for step in 1..=6 {
            let y = from + 80.0 + step as f32 * 47.0;
            let damage = update_matches(
                &mut model,
                &mut view,
                (None, None),
                (now, now),
                |model, view| {
                    drag(model, view, H, &Gesture::LongPressMove { x: 300.0, y }, now);
                },
            );
            assert!(drawn(&damage) < 900, "step {step}: {damage:?}");
        }
    }

    #[test]
    fn rows_hit_and_long_press_where_they_are() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        let layout = model.sidebar.layout();
        let at = |index: usize| list_top(&Model::new()) + layout.tops[index] + 40.0;
        let folder = layout
            .index_of(|row| matches!(row, Row::Folder { id, .. } if id == "f"))
            .unwrap();
        assert_eq!(layout.rows[0], Row::Pinned);
        assert_eq!(
            hit(&mut model, &mut view, H, 400.0, at(1), now),
            Some(act(Act::Open("c0".into())))
        );
        assert_eq!(hit(&mut model, &mut view, H, 400.0, at(0), now), None);
        assert_eq!(
            hit(&mut model, &mut view, H, 400.0, at(folder), now),
            Some(act(Act::Toggle("f".into())))
        );
        assert_eq!(
            hit(&mut model, &mut view, H, 1030.0, at(folder), now),
            Some(act(Act::NewChatIn("f".into())))
        );
        assert_eq!(
            liftable(&mut model, &view, H, 1030.0, at(folder), now),
            None
        );
        let Some(Action::List(Act::FolderMenu { id, rect })) =
            long_press(&mut model, &view, H, 400.0, at(folder), now)
        else {
            panic!("a folder menu");
        };
        assert_eq!(id, "f");
        assert_eq!(rect.y, list_top(&Model::new()) + layout.tops[folder]);
        // The header.
        assert_eq!(
            hit(&mut model, &mut view, H, 300.0, 60.0, now),
            Some(act(Act::Search))
        );
        assert_eq!(
            hit(&mut model, &mut view, H, 888.0, 60.0, now),
            Some(act(new_folder()))
        );
        model.sidebar.search.text = "x".into();
        assert_eq!(
            hit(&mut model, &mut view, H, 650.0, 60.0, now),
            Some(act(Act::ClearSearch))
        );
    }

    #[test]
    fn a_revealed_row_scrolls_into_view() {
        let mut model = model();
        let mut view = ViewState::new();
        let now = Instant::now();
        model.sidebar.reveal_row(Row::Conversation {
            id: "c50".into(),
            depth: 0,
        });
        let _ = settle(&mut model, &mut view, H, now);
        let layout = model.sidebar.layout();
        let index = layout
            .index_of(|row| matches!(row, Row::Conversation { id, .. } if id == "c50"))
            .unwrap();
        assert!(matches!(
            layout.rows[index],
            Row::Conversation { depth: 2, .. }
        ));
        let offset = view.list_scroll.offset;
        let (top, bottom) = bounds(&model, H);
        let y = list_top(&model) + layout.tops[index] - offset;
        assert!(y >= top && y + ROW_H <= bottom, "on screen: {y}");
        // Closing the folders shortens the list: the scroll stays inside.
        model.handle(Action::List(Act::Toggle("f".into())));
        let _ = settle(&mut model, &mut view, H, now);
        assert!(view.list_scroll.offset <= list_max(&mut model, H));
    }

    #[test]
    fn results_start_at_their_top_and_the_tree_comes_back_where_it_was() {
        let mut model = model();
        let mut view = ViewState::new();
        let t = Instant::now();
        view.list_scroll.offset = 900.0;
        model.handle(Action::List(Act::Search));
        model.handle(Action::List(Act::Key(crate::keys::KeyInput::Character(
            '3',
        ))));
        model.tick(t + Duration::from_secs(1));
        let _ = settle(&mut model, &mut view, H, t);
        assert_eq!(view.list_scroll.offset, 0.0);
        assert!(matches!(model.sidebar.layout().rows[0], Row::Results(_)));
        model.handle(Action::Nav(crate::exocortex::model::Nav::Back));
        let _ = settle(&mut model, &mut view, H, t);
        assert_eq!(view.list_scroll.offset, 900.0);
    }

    #[test]
    fn the_update_status_shows_under_the_last_row() {
        let mut model = Model::new();
        model.on_connected();
        let now = Instant::now();
        model.apply(
            parse_event(r#"{"type":"conversations_list","conversations":[{"id":"a","title":"A"}],"folders":[]}"#),
            now,
        );
        let mut view = ViewState::new();
        let last = list_top(&model) + model.sidebar.layout().height;
        let below = |canvas: &Canvas| ink(canvas, last, last + 80.0, theme::p().accent);
        assert!(below(&full(&mut model, &mut view, None, now)).is_empty());
        model.apply(
            parse_event(r#"{"type":"update_status","status":"restart_needed"}"#),
            now,
        );
        assert!(!below(&full(&mut model, &mut view, None, now)).is_empty());
    }

    #[test]
    fn states_show_without_rows() {
        let now = Instant::now();
        let mut model = Model::new();
        let mut view = ViewState::new();
        let layout = model.sidebar.layout();
        assert_eq!(state(&model, &layout), State::Loading("Connecting…"));
        model.on_connected();
        assert_eq!(state(&model, &layout), State::Loading("Loading chats…"));
        model.apply(
            parse_event(r#"{"type":"conversations_list","conversations":[],"folders":[]}"#),
            now,
        );
        let layout = model.sidebar.layout();
        assert_eq!(state(&model, &layout), State::Empty);
        let (_, button, rect) = state_button(&State::Empty, area(&model, H)).unwrap();
        assert_eq!(button, Act::NewChat);
        let (cx, cy) = rect.center();
        assert_eq!(
            hit(&mut model, &mut view, H, cx, cy, now),
            Some(act(Act::NewChat))
        );
        let _ = full(&mut model, &mut view, None, now);
    }
}
