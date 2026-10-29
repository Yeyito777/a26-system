//! The Settings page (WP-E; roadmap §3.6.19, `real-08-settings-top`,
//! `fake-20-settings`): left of the list, with its own top bar
//! ("SETTINGS", a chevron back to the list) and the page rail, then
//! sections as on the iPhone (`ui/settings`, `ui/ops/settings.tsx`): an
//! accent title over a rule, then rows. Appearance holds the theme cards
//! (each drawn in its own palette) and the transcript's switches; New chats
//! shows the daemon's defaults. Changes go to `settings.json` through
//! `ops::Act` (`Persist::update_settings`).

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::gfx::{Canvas, RectF};
use crate::text::Weight;

use super::super::anim::{Animator, Tween};
use super::super::model::{Action, Model, Nav, now_ms, ops};
use super::super::proto;
use super::super::sheets::{panel, usage};
use super::super::store;
use super::super::theme::{
    self, HAIRLINE, SETTINGS_PAD, SWITCH, T_BODY, T_CAPTION, T_EYEBROW, T_META, T_NOTE,
    T_PAGE_TITLE, T_SECTION, T_SMALL, TOGGLE_ROW, TOP_BAR, Theme, W,
};
use super::super::ui::icons::{self, Icon};
use super::super::ui::widgets::{self, RAIL_SETTINGS};
use super::super::view::ViewState;
use super::super::view::frame::FrameDamage;

const CLOSE_X: f32 = 1008.0;
const ROW_Y: f32 = 61.0;
/// Where the page's content starts (under the top bar's rule).
const CONTENT_TOP: f32 = TOP_BAR + HAIRLINE;
/// A section's title: padding above, its line, the gap to the rule, the
/// rule, the padding under it.
const SECTION_HEAD: f32 = 50.0 + 40.0 + 28.0 + HAIRLINE + 22.0;
/// A section's padding below.
const SECTION_END: f32 = 17.0;
/// Between a section's rows (2 pt).
const ROW_GAP: f32 = 6.0;
/// Theme cards: the gap between them, their padding, the swatches.
const CARD_GAP: f32 = 22.0;
const CARD_PAD: f32 = 28.0;
const SWATCH: f32 = 39.0;
const SWATCH_GAP: f32 = 11.0;
/// The cards' summary lines (10 pt at 1.3).
const SUMMARY_LINE: f32 = 38.0;
/// The grid's padding above and below.
const GRID_PAD: (f32, f32) = (11.0, 33.0);
/// A switch's knob slides this long.
const SLIDE: Duration = Duration::from_millis(220);

/// The width rows have (the page minus its side padding).
fn width() -> f32 {
    W - 2.0 * SETTINGS_PAD.1
}

/// Something on the page.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    /// A section's title and rule.
    Section(&'static str),
    /// A section's end.
    End,
    /// Theme cards, tallest card's height.
    Themes(i32),
    /// A switch with its label and wrapped detail.
    Switch(ops::Toggle, Vec<String>),
    /// A field: an eyebrow label over value lines (`T_SMALL`).
    Field(&'static str, Vec<String>),
    /// Wrapped lines in `T_META` `muted`.
    Note(Vec<String>),
    /// A panel's row (usage, connection, …), laid out at the page's width.
    Panel(Box<panel::Laid>),
    /// Text size: its step.
    TextSize(usize),
    /// A line of text in the body type as it now reads.
    Sample,
}

impl Row {
    fn height(&self) -> f32 {
        match self {
            Row::Section(_) => SECTION_HEAD,
            Row::End => SECTION_END,
            Row::Themes(card) => GRID_PAD.0 + *card as f32 + GRID_PAD.1,
            Row::TextSize(_) => (T_BODY.line() + T_META.line()).max(TOGGLE_ROW),
            Row::Sample => T_BODY.line() + 22.0,
            Row::Switch(_, detail) => {
                (T_BODY.line() + detail.len() as f32 * T_META.line()).max(TOGGLE_ROW)
            }
            Row::Field(_, lines) => {
                20.0 + T_EYEBROW.line() + 8.0 + lines.len() as f32 * T_SMALL.line() + 20.0
            }
            Row::Note(lines) => 22.0 + lines.len() as f32 * T_META.line() + 11.0,
            Row::Panel(laid) => laid.height,
        }
    }

    /// A section's rows are 2 pt apart.
    fn gap_after(&self, next: Option<&Row>) -> f32 {
        match (self, next) {
            (Row::Section(_) | Row::End, _) | (_, Some(Row::End) | None) => 0.0,
            _ => ROW_GAP,
        }
    }
}

/// A theme card's width (three to a row).
fn card_width() -> f32 {
    ((width() - 2.0 * CARD_GAP) / 3.0).floor()
}

/// A card's summary, wrapped.
fn summary_lines(theme: &Theme) -> Vec<String> {
    widgets::wrap_plain(theme.summary, T_META, card_width() - 2.0 * CARD_PAD)
}

/// The tallest card.
fn card_height() -> f32 {
    let lines = theme::THEMES
        .iter()
        .map(|t| summary_lines(t).len())
        .max()
        .unwrap_or(1);
    let name = T_NOTE.line();
    (CARD_PAD + SWATCH + SWATCH_GAP + name + SWATCH_GAP + lines as f32 * SUMMARY_LINE + CARD_PAD)
        .round()
}

/// A panel row laid out at the page's width.
fn panel_row(row: panel::Row) -> Row {
    Row::Panel(Box::new(panel::lay(row, 0.0, width())))
}

fn note_row(text: &str) -> Row {
    Row::Note(widgets::wrap_plain(text, T_META, width()))
}

/// The page's rows, top to bottom (content coordinates: 0 is under the
/// top bar), in the iPhone's order.
/// The page's rows, made again only when what they show changes (a frame
/// asks for them several times: its record, its drawing, hits, scrolls).
fn rows(model: &Model, now: Instant) -> Rc<Vec<(f32, Row)>> {
    cached(model, now).0
}

/// Rows made for a key of their inputs, and a hash of them.
type Cached = (u64, Rc<Vec<(f32, Row)>>, u64);

/// The rows and a hash of them (the frame's key).
fn cached(model: &Model, now: Instant) -> (Rc<Vec<(f32, Row)>>, u64) {
    thread_local! {
        static CACHE: RefCell<Option<Cached>> = const { RefCell::new(None) };
    }
    let mut hasher = DefaultHasher::new();
    let usage = &model.ops.usage;
    (
        theme::generation(),
        model.ready,
        model.connected,
        model.was_connected,
        format!(
            "{:?}",
            usage.usage.values().map(|(u, _)| u).collect::<Vec<_>>()
        ),
        usage.usage.keys().collect::<Vec<_>>(),
        format!("{:?}", usage.link),
        usage
            .tokens
            .as_ref()
            .map(|t| t["today"]["totalTokens"].to_string()),
        model.accounts_likely(),
    )
        .hash(&mut hasher);
    (
        format!("{:?}", model.ops.defaults),
        format!("{:?}", model.persist.settings()),
        model.sidebar.version,
        model.persist.problems().len(),
        now_ms() / 60_000,
    )
        .hash(&mut hasher);
    let key = hasher.finish();
    CACHE.with(|cache| {
        if let Some((k, rows, hash)) = &*cache.borrow()
            && *k == key
        {
            return (rows.clone(), *hash);
        }
        let rows = Rc::new(build_rows(model, now));
        let mut hasher = DefaultHasher::new();
        for (y, row) in rows.iter() {
            (*y as i32, format!("{row:?}")).hash(&mut hasher);
        }
        let hash = hasher.finish();
        *cache.borrow_mut() = Some((key, rows.clone(), hash));
        (rows, hash)
    })
}

fn build_rows(model: &Model, now: Instant) -> Vec<(f32, Row)> {
    let p = theme::p();
    let detail_w = width() - SWITCH.0 - 34.0;
    let mut list = vec![Row::Section("Usage limits")];
    list.extend(
        usage::usage_rows(model, true, now)
            .into_iter()
            .map(panel_row),
    );
    list.push(Row::End);
    list.push(Row::Section("Appearance"));
    list.push(Row::Themes(card_height() as i32));
    for toggle in ops::Toggle::APPEARANCE {
        let detail = if toggle.detail().is_empty() {
            Vec::new()
        } else {
            widgets::wrap_plain(toggle.detail(), T_META, detail_w)
        };
        list.push(Row::Switch(toggle, detail));
    }
    list.push(Row::End);
    list.push(Row::Section("Text & transcript"));
    list.push(Row::TextSize(ops::text_step(
        model.persist.settings().text_scale,
    )));
    list.push(Row::Sample);
    list.push(Row::End);
    list.push(Row::Section("New chats"));
    let defaults = match &model.ops.defaults {
        Some(snapshot) => ops::format_defaults(snapshot)
            .lines()
            .flat_map(|line| widgets::wrap_plain(line, T_SMALL, width()))
            .collect(),
        None => vec!["Waiting for defaults from the connected daemon.".to_owned()],
    };
    list.push(Row::Field("Daemon default", defaults));
    list.push(note_row(
        "Change them from a chat's model inspector (\"Use as daemon default\") or /default-model.",
    ));
    list.push(Row::End);
    list.push(Row::Section("Notifications"));
    list.push(note_row(
        "When a chat you started finishes and it isn't on screen, exocortex-notify posts \
         it to Moon's notifications (one per chat, withdrawn once read anywhere). Mute a chat \
         or a folder from its menu in the list to silence it. While any chat works, the \
         phone stays awake for it.",
    ));
    let muted = model
        .sidebar
        .conversations
        .values()
        .filter(|s| s.notifications_muted)
        .count();
    if muted > 0 {
        list.push(panel_row(panel::kv("Muted chats", muted.to_string())));
    }
    list.push(Row::End);
    list.push(Row::Section("Connection"));
    list.extend(
        usage::connection_rows(model, false)
            .into_iter()
            .map(panel_row),
    );
    list.push(Row::End);
    list.push(Row::Section("Local storage"));
    list.push(note_row(&format!(
        "Drafts, settings and the list's folders are kept on this phone in {}. \
         Clearing cached chats does not erase them.",
        store::dir().display()
    )));
    for (name, error) in model.persist.problems() {
        list.push(panel_row(panel::text(
            format!(
                "{name} could not be read ({error}): it is left untouched and changes are not kept."
            ),
            panel::Style::Caption,
            p.error,
        )));
    }
    list.push(panel_row(panel::Row::Buttons(vec![panel::Btn::new(
        "Clear cached chats",
        panel::Kind::Secondary,
        Some(panel::Tap::Stay(Action::Ops(ops::Act::ClearCache))),
    )])));
    list.push(Row::End);
    list.push(Row::Section("About"));
    for (key, value) in [
        ("App", "Exocortex for OSS Phone (a26-system)".to_owned()),
        ("Version", env!("CARGO_PKG_VERSION").to_owned()),
        ("Daemon", proto::socket_path().display().to_string()),
        ("Device", "Samsung Galaxy A26 · Moon".to_owned()),
    ] {
        list.push(panel_row(panel::kv(key, value)));
    }
    list.push(note_row(
        "The app updates with a26-system; Exocortex itself updates on the daemon's side.",
    ));
    list.push(Row::End);
    let mut y = 0.0;
    let mut out = Vec::with_capacity(list.len());
    for index in 0..list.len() {
        let row = list[index].clone();
        let next = list.get(index + 1);
        let step = row.height() + row.gap_after(next);
        out.push((y, row));
        y += step;
    }
    out
}

/// The content's height.
fn content_height(rows: &[(f32, Row)]) -> f32 {
    rows.last().map_or(0.0, |(y, row)| y + row.height()) + 34.0
}

/// How far the page scrolls: its content past the window, above Moon's
/// home gesture.
pub fn max_scroll(model: &Model, height: f32) -> f32 {
    let visible = height - CONTENT_TOP - theme::GESTURE_RESERVE;
    (content_height(&rows(model, Instant::now())) - visible)
        .max(0.0)
        .round()
}

/// Where a switch's knob is at `now`: 0 off … 1 on, sliding for 220 ms
/// after it flipped, in 1/32 steps (so two frames of one state agree).
fn knob(model: &Model, toggle: ops::Toggle, now: Instant) -> f32 {
    let on = toggle.get(&model.persist.settings());
    let target = if on { 1.0 } else { 0.0 };
    match model.ops.flipped {
        Some((flipped, at)) if flipped == toggle => {
            let tween = Tween::new(at, SLIDE, 1.0 - target, target);
            (tween.value(now) * 32.0).round() / 32.0
        }
        _ => target,
    }
}

fn sliding(model: &Model, now: Instant) -> Option<Tween> {
    model
        .ops
        .flipped
        .map(|(_, at)| Tween::new(at, SLIDE, 0.0, 1.0))
        .filter(|tween| !tween.done(now))
}

/// What a Settings frame showed: enough to draw the next one by changing
/// only what differs (a scroll moves the content's rows, a sliding switch
/// redraws its row).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsFrame {
    offset: i32,
    /// Everything else the page's pixels depend on.
    key: u64,
    /// Each switch's knob, in 1/32 steps.
    knobs: Vec<i32>,
    pressed: Option<Action>,
}

/// The current frame's record.
pub fn settings_frame(
    model: &Model,
    view: &ViewState,
    pressed: Option<&Action>,
    now: Instant,
) -> SettingsFrame {
    let mut hasher = DefaultHasher::new();
    (
        &model.persist.settings().theme,
        theme::generation(),
        cached(model, now).1,
    )
        .hash(&mut hasher);
    SettingsFrame {
        offset: view.settings_scroll.value(now).round() as i32,
        key: hasher.finish(),
        knobs: ops::Toggle::APPEARANCE
            .iter()
            .map(|t| (knob(model, *t, now) * 32.0) as i32)
            .collect(),
        pressed: pressed.cloned(),
    }
}

/// Turns the frame drawn for `old` into `new`'s: the content's rows moved
/// for a scroll and the strip it uncovers drawn, rows whose switch moved
/// or whose target was pressed drawn again. None: draw everything.
pub fn draw_settings_update(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    old: &SettingsFrame,
    new: &SettingsFrame,
    now: Instant,
) -> Option<FrameDamage> {
    if old.key != new.key {
        return None;
    }
    let (top, bottom) = (CONTENT_TOP as i32, canvas.height);
    let shift = new.offset - old.offset;
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
    let list = rows(model, now);
    let band = |wanted: &dyn Fn(&Row) -> bool| {
        list.iter().find(|(_, row)| wanted(row)).map(|(y, row)| {
            let y0 = (CONTENT_TOP - new.offset as f32 + y).floor() as i32;
            (
                y0.max(top),
                (y0 + row.height().ceil() as i32 + 1).min(bottom),
            )
        })
    };
    for (index, toggle) in ops::Toggle::APPEARANCE.iter().enumerate() {
        if old.knobs.get(index) != new.knobs.get(index)
            && let Some(rows) = band(&|row| matches!(row, Row::Switch(t, _) if t == toggle))
        {
            redraw.push(rows);
        }
    }
    if old.pressed != new.pressed {
        for action in [&old.pressed, &new.pressed].into_iter().flatten() {
            match action {
                Action::Ops(ops::Act::SetTheme(_)) => {
                    redraw.extend(band(&|row| matches!(row, Row::Themes(_))));
                }
                Action::Nav(Nav::CloseSettings) => redraw.push((0, top)),
                // A panel row's target (a navigation row, a button).
                action => redraw.extend(band(&|row| match row {
                    Row::Panel(laid) => panel::targets(laid, width())
                        .iter()
                        .any(|(_, tap)| panel::tap_action(tap) == action),
                    _ => false,
                })),
            }
        }
    }
    let merged = super::super::view::frame::merge(redraw);
    let pressed = new.pressed.clone();
    for &(y0, y1) in &merged {
        canvas.set_clip(0, y0, canvas.width, y1);
        draw(canvas, model, view, pressed.as_ref(), now);
        canvas.reset_clip();
    }
    damage.rows = merged;
    Some(damage)
}

/// Asks for frames while a switch slides.
pub fn animate(model: &Model, now: Instant, animator: &mut Animator) {
    if let Some(tween) = sliding(model, now) {
        animator.follow(&tween, now);
    }
    // Usage resets and times read in minutes.
    animator.request(now + Duration::from_secs(60));
}

pub fn draw(
    canvas: &mut Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Action>,
    now: Instant,
) {
    let p = theme::p();
    let height = canvas.height as f32;
    widgets::fill(
        canvas,
        RectF::new(0.0, 0.0, W, height),
        p.sidebar_background,
    );
    let offset = view.settings_scroll.value(now).round();
    let top = CONTENT_TOP - offset;
    widgets::clipped(
        canvas,
        RectF::new(0.0, CONTENT_TOP, W, height - CONTENT_TOP),
        |canvas| {
            for (y, row) in rows(model, now).iter() {
                let y = top + y;
                if widgets::shows(canvas, y, y + row.height()) {
                    draw_row(canvas, model, row, y, pressed, now);
                }
            }
        },
    );
    widgets::top_bar(canvas, p.sidebar_background);
    widgets::page_rail(canvas, RAIL_SETTINGS);
    widgets::text(
        canvas,
        "Settings",
        theme::S3,
        T_PAGE_TITLE.baseline(ROW_Y - T_PAGE_TITLE.line() / 2.0),
        T_PAGE_TITLE,
        p.text,
        1.0,
    );
    let down = pressed == Some(&Action::Nav(Nav::CloseSettings));
    widgets::icon_button(
        canvas,
        Icon::ChevronRight,
        CLOSE_X,
        ROW_Y,
        60.0,
        p.text,
        down,
    );
}

fn draw_row(
    canvas: &mut Canvas,
    model: &Model,
    row: &Row,
    y: f32,
    pressed: Option<&Action>,
    now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let x = SETTINGS_PAD.1;
    let right = W - SETTINGS_PAD.1;
    match row {
        Row::Section(title) => {
            widgets::text(
                canvas,
                title,
                x,
                T_SECTION.baseline(y + 50.0),
                T_SECTION,
                p.accent,
                1.0,
            );
            widgets::hairline(canvas, x, right, y + 50.0 + 40.0 + 28.0, c.rule_settings);
        }
        Row::End => {}
        Row::Themes(card) => {
            let current = model.persist.settings().theme;
            let w = card_width();
            for (index, theme) in theme::THEMES.iter().enumerate() {
                let rect = RectF::new(
                    x + index as f32 * (w + CARD_GAP),
                    y + GRID_PAD.0,
                    w,
                    *card as f32,
                );
                let down = pressed == Some(&Action::Ops(ops::Act::SetTheme(theme.id)));
                theme_card(canvas, theme, rect, theme.id == current, down);
            }
        }
        Row::Switch(toggle, detail) => {
            let h = row.height();
            let lines_h = T_BODY.line() + detail.len() as f32 * T_META.line();
            let text_top = y + ((h - lines_h) / 2.0).round();
            widgets::text(
                canvas,
                toggle.label(),
                x,
                T_BODY.baseline(text_top),
                T_BODY,
                p.text,
                1.0,
            );
            for (index, line) in detail.iter().enumerate() {
                widgets::text(
                    canvas,
                    line,
                    x,
                    T_META.baseline(text_top + T_BODY.line() + index as f32 * T_META.line()),
                    T_META,
                    p.muted,
                    1.0,
                );
            }
            widgets::switch(
                canvas,
                right - SWITCH.0,
                y + ((h - SWITCH.1) / 2.0).round(),
                knob(model, *toggle, now),
                p.sidebar_background,
            );
            widgets::hairline(
                canvas,
                x,
                right,
                y + h - HAIRLINE,
                theme::mix(p.border, 25, p.sidebar_background),
            );
        }
        Row::Field(label, lines) => {
            widgets::text(
                canvas,
                label,
                x,
                T_EYEBROW.baseline(y + 20.0),
                T_EYEBROW,
                p.muted,
                1.0,
            );
            let top = y + 20.0 + T_EYEBROW.line() + 8.0;
            for (index, line) in lines.iter().enumerate() {
                widgets::text(
                    canvas,
                    line,
                    x,
                    T_SMALL.baseline(top + index as f32 * T_SMALL.line()),
                    T_SMALL,
                    p.text,
                    1.0,
                );
            }
        }
        Row::Note(lines) => {
            for (index, line) in lines.iter().enumerate() {
                widgets::text(
                    canvas,
                    line,
                    x,
                    T_META.baseline(y + 22.0 + index as f32 * T_META.line()),
                    T_META,
                    p.muted,
                    1.0,
                );
            }
        }
        Row::Panel(laid) => panel::draw_row(canvas, laid, x, y, width(), pressed),
        Row::TextSize(step) => {
            let h = row.height();
            let (label, _) = ops::TEXT_SIZES[*step];
            let text_top = y + ((h - T_BODY.line() - T_META.line()) / 2.0).round();
            widgets::text(
                canvas,
                "Text size",
                x,
                T_BODY.baseline(text_top),
                T_BODY,
                p.text,
                1.0,
            );
            widgets::text(
                canvas,
                label,
                x,
                T_META.baseline(text_top + T_BODY.line()),
                T_META,
                p.muted,
                1.0,
            );
            let (small, track, large) = text_size_parts();
            let last = ops::TEXT_SIZES.len() - 1;
            let bold = |size: f32| T_CAPTION.weight(Weight::MonoBold).sized(size);
            for (rect, size, enabled) in [(small, 34.0, *step > 0), (large, 60.0, *step < last)] {
                let ty = bold(size);
                widgets::text_centered(
                    canvas,
                    "A",
                    rect.x + rect.w / 2.0,
                    ty.baseline(y + (h - ty.line()) / 2.0),
                    ty,
                    p.secondary_accent,
                    if enabled { 1.0 } else { 0.3 },
                );
            }
            let cy = y + h / 2.0;
            let bar = RectF::new(track.x, cy - 8.0, track.w, 16.0);
            canvas.fill_rrect(bar, 8.0, c.switch_off, 1.0);
            let knob_x = knob_x(track, *step);
            canvas.fill_rrect(
                RectF::new(bar.x, bar.y, knob_x - bar.x, bar.h),
                8.0,
                p.accent,
                1.0,
            );
            canvas.fill_circle(knob_x, cy, 24.0, p.accent, 1.0);
        }
        Row::Sample => {
            let baseline = T_BODY.baseline(y + 11.0);
            let w = widgets::text(
                canvas,
                "The quick brown fox · ",
                x,
                baseline,
                T_BODY,
                p.muted,
                1.0,
            );
            widgets::text(
                canvas,
                "tok/s",
                x + w,
                baseline,
                T_BODY,
                p.secondary_accent,
                1.0,
            );
        }
    }
}

/// The text size controls (page x; the row's full height): the small A,
/// the track, the large A.
fn text_size_parts() -> (RectF, RectF, RectF) {
    let right = W - SETTINGS_PAD.1;
    let large = RectF::new(right - 110.0, 0.0, 110.0, 0.0);
    let track = RectF::new(large.x - 330.0, 0.0, 330.0, 0.0);
    let small = RectF::new(track.x - 110.0, 0.0, 110.0, 0.0);
    (small, track, large)
}

/// Where the knob is for `step` on `track`.
fn knob_x(track: RectF, step: usize) -> f32 {
    let last = (ops::TEXT_SIZES.len() - 1) as f32;
    (track.x + 24.0 + (track.w - 48.0) * step as f32 / last).round()
}

/// A theme card (PAR-323) in its own palette: three swatches (accent,
/// secondary, surface), the name with a check when current, the summary.
fn theme_card(canvas: &mut Canvas, theme: &Theme, rect: RectF, current: bool, pressed: bool) {
    let t = &theme.p;
    // Pressed, it shrinks to 97 % (as the iPhone's `:active`).
    let rect = if pressed {
        let dx = (rect.w * 0.015).round();
        let dy = (rect.h * 0.015).round();
        RectF::new(
            rect.x + dx,
            rect.y + dy,
            rect.w - 2.0 * dx,
            rect.h - 2.0 * dy,
        )
    } else {
        rect
    };
    widgets::fill(canvas, rect, t.app_background);
    if current {
        widgets::border(canvas, rect, 4.0, t.accent);
    } else {
        widgets::border(
            canvas,
            rect,
            HAIRLINE,
            theme::mix(t.border, 50, t.app_background),
        );
    }
    let x = rect.x + CARD_PAD;
    let mut y = rect.y + CARD_PAD;
    for (index, color) in [t.accent, t.secondary_accent, t.surface_background]
        .into_iter()
        .enumerate()
    {
        widgets::fill(
            canvas,
            RectF::new(x + index as f32 * (SWATCH + SWATCH_GAP), y, SWATCH, SWATCH),
            color,
        );
    }
    y += SWATCH + SWATCH_GAP;
    let name = T_NOTE.weight(Weight::MonoBold);
    let name_w = widgets::text(canvas, theme.label, x, name.baseline(y), name, t.text, 1.0);
    if current {
        icons::draw(
            canvas,
            Icon::Check,
            x + name_w + 11.0 + 20.0,
            y + name.line() / 2.0,
            40.0,
            t.text,
            1.0,
        );
    }
    y += name.line() + SWATCH_GAP;
    for (index, line) in summary_lines(theme).iter().enumerate() {
        widgets::text(
            canvas,
            line,
            x,
            T_META.baseline(y + index as f32 * SUMMARY_LINE - (T_META.line() - SUMMARY_LINE) / 2.0),
            T_META.weight(Weight::Mono),
            t.muted,
            1.0,
        );
    }
}

/// The page's target at (`x`, `y`).
pub fn hit(model: &Model, view: &ViewState, x: f32, y: f32, now: Instant) -> Option<Action> {
    if y < TOP_BAR {
        return (x > CLOSE_X - 90.0).then_some(Action::Nav(Nav::CloseSettings));
    }
    let offset = view.settings_scroll.value(now).round();
    let cy = y - CONTENT_TOP + offset;
    let (row_y, row) = rows(model, now)
        .iter()
        .find(|(top, row)| cy >= *top && cy < top + row.height())
        .cloned()?;
    match row {
        Row::Themes(card) => {
            let w = card_width();
            let inside = cy - row_y - GRID_PAD.0;
            if !(0.0..card as f32).contains(&inside) {
                return None;
            }
            let column = ((x - SETTINGS_PAD.1) / (w + CARD_GAP)).floor();
            let within = x - SETTINGS_PAD.1 - column * (w + CARD_GAP);
            if column < 0.0 || within > w {
                return None;
            }
            theme::THEMES
                .get(column as usize)
                .map(|t| Action::Ops(ops::Act::SetTheme(t.id)))
        }
        Row::Switch(toggle, _) => Some(Action::Ops(ops::Act::Toggle(toggle))),
        Row::TextSize(step) => {
            let (small, track, large) = text_size_parts();
            let last = ops::TEXT_SIZES.len() - 1;
            // The track's ends are its own: the knob's last place is near.
            if x >= large.x {
                Some(Action::Ops(ops::Act::TextSize((step + 1).min(last))))
            } else if x >= track.x {
                let nearest = (0..=last)
                    .min_by(|a, b| {
                        (knob_x(track, *a) - x)
                            .abs()
                            .total_cmp(&(knob_x(track, *b) - x).abs())
                    })
                    .unwrap_or(step);
                Some(Action::Ops(ops::Act::TextSize(nearest)))
            } else if x >= small.x - 20.0 {
                Some(Action::Ops(ops::Act::TextSize(step.saturating_sub(1))))
            } else {
                None
            }
        }
        Row::Panel(laid) => {
            let mut laid = *laid;
            laid.top = row_y;
            panel::targets(&laid, width())
                .into_iter()
                .find(|(rect, _)| rect.contains(x - SETTINGS_PAD.1, cy))
                .map(|(_, tap)| panel::tap_action(&tap).clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Page;
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::view::frame::{self, Frame};

    const SIZE: (i32, i32) = (1080, 2216);

    fn settings_page() -> (Model, ViewState) {
        let mut model = Model::new();
        model.on_connected();
        model.apply(
            parse_event(r#"{"type":"tools_available","providers":[],"tools":[],"conversationDefaults":{"defaults":{"provider":"anthropic","model":"m","effort":"high"},"configured":true}}"#),
            Instant::now(),
        );
        model.page = Page::Settings;
        (model, ViewState::new())
    }

    fn full(model: &mut Model, view: &mut ViewState, now: Instant) -> Canvas {
        let mut canvas = Canvas::new(SIZE.0, SIZE.1);
        frame::clear(&mut canvas);
        frame::draw_full(&mut canvas, model, view, None, now);
        canvas
    }

    /// One frame drawn incrementally: it equals a full redraw and changes
    /// nothing outside the rows it reports. Returns the rows it drew.
    fn step(
        canvas: &mut Canvas,
        model: &mut Model,
        view: &mut ViewState,
        old: &Frame,
        now: Instant,
    ) -> (Frame, i32) {
        let new = frame::record(model, view, None, SIZE, now);
        let mut shadow = canvas.pixels.clone();
        let damage = frame::update(canvas, model, view, None, old, &new, now).expect("incremental");
        let whole = full(model, view, now);
        let width = SIZE.0 as usize;
        assert!(whole.pixels == canvas.pixels, "differs from a full redraw");
        if let Some((y0, y1, dy)) = damage.moved {
            let rows = y1 - y0 - dy.abs();
            let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
            shadow.copy_within(
                src as usize * width..(src + rows) as usize * width,
                dst as usize * width,
            );
        }
        for y in 0..SIZE.1 {
            if damage.rows.iter().any(|(y0, y1)| (*y0..*y1).contains(&y)) {
                continue;
            }
            let row = y as usize * width..(y as usize + 1) * width;
            assert!(shadow[row.clone()] == canvas.pixels[row], "row {y} changed");
        }
        (new, damage.rows.iter().map(|(a, b)| b - a).sum())
    }

    #[test]
    fn a_sliding_switch_draws_only_its_row() {
        let (mut model, mut view) = settings_page();
        let t0 = Instant::now();
        let mut old = frame::record(&mut model, &mut view, None, SIZE, t0);
        let mut canvas = full(&mut model, &mut view, t0);
        model.handle(Action::Ops(ops::Act::Toggle(ops::Toggle::ShowAllWork)));
        let start = model.ops.flipped.unwrap().1;
        for ms in (0..=240).step_by(8) {
            let (next, drawn) = step(
                &mut canvas,
                &mut model,
                &mut view,
                &old,
                start + Duration::from_millis(ms),
            );
            assert!(drawn <= 160, "{drawn} rows for a switch");
            old = next;
        }
    }

    #[test]
    fn scrolling_moves_rows_and_draws_the_uncovered_strip() {
        let (mut model, mut view) = settings_page();
        let t0 = Instant::now();
        let mut old = frame::record(&mut model, &mut view, None, SIZE, t0);
        let mut canvas = full(&mut model, &mut view, t0);
        // Rubber-banding past the top (the page fits the window today).
        for step_px in [-12.0, -30.0, -18.0, 0.0] {
            view.settings_scroll.offset = step_px;
            let (next, drawn) = step(&mut canvas, &mut model, &mut view, &old, t0);
            assert!(drawn <= 40, "{drawn} rows for a scroll step");
            old = next;
        }
    }

    #[test]
    fn another_theme_leaves_no_whale_colour_on_the_page() {
        let (mut model, mut view) = settings_page();
        let now = Instant::now();
        model.handle(Action::Ops(ops::Act::SetTheme("cerberus")));
        let canvas = full(&mut model, &mut view, now);
        theme::set_current(&theme::WHALE);
        let w = theme::WHALE.p;
        let whale = [
            w.app_background,
            w.sidebar_background,
            w.surface_background,
            w.selection_background,
            w.accent,
            w.secondary_accent,
        ];
        // The Whale card previews Whale: leave it out.
        let themes = rows(&model, now)
            .iter()
            .find(|(_, r)| matches!(r, Row::Themes(_)))
            .map(|(y, _)| *y)
            .unwrap();
        let card_top = (CONTENT_TOP + themes + GRID_PAD.0) as usize;
        let card_bottom = card_top + card_height() as usize;
        let card_right = (SETTINGS_PAD.1 + card_width()) as usize + 1;
        let width = SIZE.0 as usize;
        for (index, pixel) in canvas.pixels.iter().enumerate() {
            let (x, y) = (index % width, index / width);
            if (card_top..card_bottom).contains(&y) && x <= card_right {
                continue;
            }
            assert!(
                !whale.contains(pixel),
                "a Whale colour at {x}, {y}: {pixel:06x}"
            );
        }
        // The list and a conversation (with every kind of entry the
        // fixtures have) draw in the new palette too.
        theme::set_current(&theme::CERBERUS);
        model.apply(
            parse_event(r#"{"type":"conversations_list","conversations":[{"id":"c","title":"A chat","provider":"anthropic","model":"m","streaming":true,"unread":true,"pinned":true},{"id":"d","title":"Other","folderId":"f"}],"folders":[{"id":"f","name":"Work"}]}"#),
            now,
        );
        model.page = Page::List;
        let list = full(&mut model, &mut view, now);
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let id = model.chat.load_request.clone().unwrap();
        model.apply(
            parse_event(&format!(r##"{{"type":"conversation_loaded","reqId":"{id}","convId":"c","entries":[{{"type":"user","text":"Hello"}},{{"type":"ai","blocks":[{{"type":"thinking","text":"hm"}},{{"type":"tool_call","toolCallId":"t","toolName":"Bash","input":{{"command":"ls"}},"summary":"ls"}},{{"type":"tool_result","toolCallId":"t","toolName":"Bash","output":"a","isError":true}},{{"type":"text","text":"# Title\n\n**bold** `code` [link](https://x.org)\n\n> quote\n\n- item\n\n```rust\nfn main() {{}}\n```"}}]}},{{"type":"system","text":"Error","color":"error"}}]}}"##)),
            now,
        );
        let chat = full(&mut model, &mut view, now);
        theme::set_current(&theme::WHALE);
        for (page, canvas) in [("list", &list), ("chat", &chat)] {
            for (index, pixel) in canvas.pixels.iter().enumerate() {
                assert!(
                    !whale.contains(pixel),
                    "{page}: a Whale colour at {}, {}: {pixel:06x}",
                    index % width,
                    index / width
                );
            }
        }
    }

    fn at(model: &Model, wanted: impl Fn(&Row) -> bool) -> f32 {
        rows(model, Instant::now())
            .iter()
            .find(|(_, row)| wanted(row))
            .map(|(y, _)| y + CONTENT_TOP)
            .unwrap()
    }

    #[test]
    fn rows_and_cards_answer_taps() {
        let model = Model::new();
        let view = ViewState::new();
        let now = Instant::now();
        let cards = at(&model, |r| matches!(r, Row::Themes(_)));
        let second = SETTINGS_PAD.1 + card_width() + CARD_GAP + 40.0;
        assert_eq!(
            hit(&model, &view, second, cards + 100.0, now),
            Some(Action::Ops(ops::Act::SetTheme("cerberus")))
        );
        assert_eq!(
            hit(
                &model,
                &view,
                SETTINGS_PAD.1 + card_width() + 5.0,
                cards + 100.0,
                now
            ),
            None,
            "between cards"
        );
        let haptics = at(&model, |r| {
            matches!(r, Row::Switch(ops::Toggle::Haptics, _))
        });
        assert_eq!(
            hit(&model, &view, 500.0, haptics + 10.0, now),
            Some(Action::Ops(ops::Act::Toggle(ops::Toggle::Haptics)))
        );
        assert_eq!(
            hit(&model, &view, 1030.0, 60.0, now),
            Some(Action::Nav(Nav::CloseSettings))
        );
        // Text size: the track's ends are its first and last steps, the
        // A's a step down and up.
        let size = at(&model, |r| matches!(r, Row::TextSize(_))) + 20.0;
        let (small, track, large) = text_size_parts();
        let tap = |x: f32| match hit(&model, &view, x, size, now) {
            Some(Action::Ops(ops::Act::TextSize(step))) => Some(step),
            _ => None,
        };
        assert_eq!(tap(track.x + 4.0), Some(0));
        assert_eq!(tap(track.x + track.w - 4.0), Some(5));
        assert_eq!(tap(large.x + 50.0), Some(3));
        assert_eq!(tap(small.x + 50.0), Some(1));
    }

    #[test]
    fn rows_are_as_tall_as_the_iphones() {
        let model = Model::new();
        let rows = rows(&model, Instant::now());
        // Two-line details need more than the 144 minimum.
        let height = |toggle| {
            rows.iter()
                .find(|(_, r)| matches!(r, Row::Switch(t, _) if *t == toggle))
                .map(|(_, r)| r.height())
                .unwrap()
        };
        assert_eq!(height(ops::Toggle::Haptics), TOGGLE_ROW);
        assert_eq!(height(ops::Toggle::ExpandThinking), TOGGLE_ROW);
        assert!(height(ops::Toggle::ShowAllWork) > TOGGLE_ROW);
        assert!((270.0..300.0).contains(&card_height()), "{}", card_height());
        // Taller than the window: it scrolls.
        assert!(max_scroll(&model, 2216.0) > 0.0);
        assert!(max_scroll(&model, 1000.0) > max_scroll(&model, 2216.0));
    }

    #[test]
    fn a_flipped_switch_slides_in_steps() {
        let mut model = Model::new();
        model.handle(Action::Ops(ops::Act::Toggle(ops::Toggle::ExpandThinking)));
        let start = model.ops.flipped.unwrap().1;
        assert_eq!(knob(&model, ops::Toggle::ExpandThinking, start), 0.0);
        let mid = knob(
            &model,
            ops::Toggle::ExpandThinking,
            start + Duration::from_millis(60),
        );
        assert!(mid > 0.0 && mid < 1.0 && (mid * 32.0).fract() == 0.0);
        assert_eq!(
            knob(&model, ops::Toggle::ExpandThinking, start + SLIDE),
            1.0
        );
        let mut animator = Animator::default();
        animate(&model, start, &mut animator);
        assert!(animator.take().is_some());
        animate(&model, start + SLIDE, &mut animator);
        assert_eq!(
            animator.take(),
            Some(start + SLIDE + Duration::from_secs(60)),
            "then only the minute's"
        );
    }
}
