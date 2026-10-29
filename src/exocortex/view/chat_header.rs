//! The chat's top bar (WP-E; roadmap §3.6.1): back, the conversation's
//! title over its subtitle (model · effort, a status dot), ⋯ for the chat
//! menu, and the page rail. Tapping the title opens the model inspector.
//!
//! `subtitle` is WP-A's stub (the model label); E gives it the status
//! priorities of PAR-009 (Connecting…, Couldn't load, Syncing…, Working ·
//! model) and fills `chat_menu`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::gfx::{Canvas, Rgb};

use super::super::anim::{self, Animator};
use super::super::model::{Action, Model, Nav, UiAct, ops};
use super::super::theme::{self, T_SUBTITLE, T_TITLE, TOP_BAR};
use super::super::ui::icons::{self, Icon, Style};
use super::super::ui::sheet::{ActionItem, ActionList, Sheet, action_sheet};
use super::super::ui::widgets::{self, RAIL_CHAT};

/// The centre column (title and subtitle).
const CENTRE: (f32, f32) = (140.0, 940.0);
const BACK_X: f32 = 72.0;
const MORE_X: f32 = 1008.0;
const ROW_Y: f32 = 61.0;
/// The title's and subtitle's baselines (`real-04-new-chat`: the
/// roadmap's 64 for the title sits lower than the iPhone's).
const TITLE_BASELINE: f32 = 47.0;
const SUBTITLE_BASELINE: f32 = 104.0;

/// The subtitle's status dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dot {
    /// Streaming or loading (pulses).
    Live,
    /// Not ready.
    Waiting,
    /// Failed to load.
    Failed,
}

/// The subtitle under the title.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Subtitle {
    pub text: String,
    /// Shown after it in `muted` (" · 1.0k ctx").
    pub detail: Option<String>,
    pub dot: Option<Dot>,
}

/// What the subtitle says (A's stub: the model and effort, a dot while the
/// conversation streams, isn't ready or failed to load).
pub fn subtitle(model: &Model) -> Subtitle {
    let selection = model.selection();
    let mut text = model.model_label(&selection);
    if !selection.effort.is_empty() {
        text = format!("{text} · {}", selection.effort);
    }
    let dot = if model.chat.error.is_some() {
        Some(Dot::Failed)
    } else if !model.ready {
        Some(Dot::Waiting)
    } else if model.streaming() || model.chat_pending() {
        Some(Dot::Live)
    } else {
        None
    };
    Subtitle {
        text,
        detail: None,
        dot,
    }
}

fn title(model: &Model) -> String {
    match model.open_summary() {
        Some(summary) => summary.display_title().to_owned(),
        None if model.chat.id.is_none() => "New chat".into(),
        None => "Conversation".into(),
    }
}

/// Whether `action` is one of the bar's (its pressed state redraws it).
pub fn owns(action: &Action) -> bool {
    matches!(
        action,
        Action::Nav(Nav::Back) | Action::Ops(ops::Act::OpenInspector | ops::Act::ChatMenu)
    )
}

/// What the bar's pixels depend on.
pub fn frame_key(model: &Model, pressed: Option<&Action>, now: Instant) -> u64 {
    let subtitle = subtitle(model);
    let pulse = (subtitle.dot == Some(Dot::Live)).then(|| anim::pulse_percent(now, 1400));
    let mut hasher = DefaultHasher::new();
    (
        title(model),
        subtitle,
        pulse,
        super::action_key(pressed.filter(|action| owns(action))),
        theme::generation(),
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// Asks for frames while the status dot pulses.
pub fn animate(model: &Model, now: Instant, animator: &mut Animator) {
    if subtitle(model).dot == Some(Dot::Live) {
        animator.slow_frame(now);
    }
}

/// The bar's target at (`x`, `y`).
pub fn hit(_model: &Model, x: f32, y: f32) -> Option<Action> {
    if y >= TOP_BAR {
        return None;
    }
    if x < CENTRE.0 {
        Some(Action::Nav(Nav::Back))
    } else if x >= CENTRE.1 {
        Some(Action::Ops(ops::Act::ChatMenu))
    } else {
        Some(Action::Ops(ops::Act::OpenInspector))
    }
}

pub fn draw(canvas: &mut Canvas, model: &Model, pressed: Option<&Action>, now: Instant) {
    if !widgets::shows(canvas, 0.0, TOP_BAR + theme::HAIRLINE) {
        return;
    }
    let p = theme::p();
    widgets::top_bar(canvas, p.app_background);
    widgets::page_rail(canvas, RAIL_CHAT);
    let back = pressed == Some(&Action::Nav(Nav::Back));
    icons::draw_styled(
        canvas,
        Icon::ChevronLeft,
        BACK_X,
        ROW_Y,
        Style::new(60.0, p.text)
            .stroke(5.0)
            .alpha(if back { 0.5 } else { 1.0 }),
    );
    let more = pressed == Some(&Action::Ops(ops::Act::ChatMenu));
    widgets::ellipsis(canvas, MORE_X, ROW_Y, p.text, if more { 0.5 } else { 1.0 });
    let centre = pressed == Some(&Action::Ops(ops::Act::OpenInspector));
    let alpha = if centre { 0.6 } else { 1.0 };
    let width = CENTRE.1 - CENTRE.0;
    let cx = (CENTRE.0 + CENTRE.1) / 2.0;
    let title = widgets::fit(&title(model), T_TITLE, width);
    widgets::text_centered(canvas, &title, cx, TITLE_BASELINE, T_TITLE, p.text, alpha);
    let subtitle = subtitle(model);
    let c = theme::c();
    let main = widgets::fit(&subtitle.text, T_SUBTITLE, width - 40.0);
    let detail = subtitle.detail.clone().unwrap_or_default();
    let main_w = widgets::measure(&main, T_SUBTITLE);
    let detail_w = widgets::measure(&detail, T_SUBTITLE);
    let left = (cx - (main_w + detail_w) / 2.0).round();
    widgets::text(
        canvas,
        &main,
        left,
        SUBTITLE_BASELINE,
        T_SUBTITLE,
        c.text_dim,
        alpha,
    );
    if !detail.is_empty() {
        widgets::text(
            canvas,
            &detail,
            left + main_w,
            SUBTITLE_BASELINE,
            T_SUBTITLE,
            p.muted,
            alpha,
        );
    }
    if let Some(dot) = subtitle.dot {
        let (color, shade): (Rgb, f32) = match dot {
            Dot::Live => (p.accent, anim::pulse(now, 1400)),
            Dot::Waiting => (p.warning, 1.0),
            Dot::Failed => (p.error, 1.0),
        };
        let color = theme::mix(color, (shade * 100.0).round() as u32, p.app_background);
        canvas.fill_circle(
            left - 14.0 - 7.0,
            SUBTITLE_BASELINE - 10.0,
            7.0,
            color,
            alpha,
        );
    }
}

/// The ⋯ menu (WP-E fills it: Reconnect, Configuration and activity, Find,
/// Export, Discard draft; PAR-010).
pub fn chat_menu(model: &Model) -> Sheet<ActionList> {
    let selection = model.selection();
    let mut items = vec![
        ActionItem::new(
            "Model, effort & speed…",
            Action::Ops(ops::Act::OpenInspector),
        )
        .detail(subtitle(model).text),
    ];
    if let Some(text) = last_reply(model) {
        items.push(
            ActionItem::new(
                "Read the last reply",
                Action::Ui(UiAct::Read {
                    title: "Last reply".into(),
                    text,
                }),
            )
            .icon(Icon::FileText),
        );
    }
    if let Some(id) = &model.chat.id {
        items.push(
            ActionItem::new("Copy conversation id", Action::Ui(UiAct::Copy(id.clone())))
                .icon(Icon::Copy),
        );
    }
    let label = model.model_label(&selection);
    action_sheet(
        ActionList::new(title(model), items)
            .subtitle(if selection.effort.is_empty() {
                label
            } else {
                format!("{label} · {}", selection.effort)
            })
            .scoped(),
    )
}

/// The newest reply's text, if any is loaded.
fn last_reply(model: &Model) -> Option<String> {
    use super::super::proto::{self, Entry};
    model
        .chat
        .entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            Entry::Ai { blocks, .. } => {
                let text = proto::reply_text(blocks);
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        })
}
