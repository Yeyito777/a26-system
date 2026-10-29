//! The chat's top bar (WP-E; roadmap §3.6.1): back, the conversation's
//! title over its subtitle (a status dot, the model or what the
//! conversation is doing, its context), ⋯ for the chat menu, and the page
//! rail. Tapping the title opens the model inspector.
//!
//! The subtitle follows the iPhone's priorities (PAR-009): the connection,
//! a failed load, a retry, a wait, a running turn, a window on its way,
//! else the model label. It is one line of fixed height, so a status
//! change never moves the transcript (PAR-177).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::gfx::{Canvas, Rgb};

use super::super::anim::{self, Animator};
use super::super::model::{Action, Model, Nav, UiAct, chat, composer, ops};
use super::super::sheets;
use super::super::theme::{self, T_SUBTITLE, T_TITLE, TOP_BAR};
use super::super::ui::icons::{self, Icon, Style};
use super::super::ui::sheet::{ActionItem, ActionList, Sheet, Tone, action_sheet};
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
/// The status dot (5 pt) and the gap after it.
const DOT: f32 = 14.0;
const DOT_GAP: f32 = 14.0;

/// The subtitle's status dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dot {
    /// Streaming or loading (pulses).
    Live,
    /// Not ready, retrying, waiting.
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

/// What the subtitle says (PAR-009, 279).
pub fn subtitle(model: &Model) -> Subtitle {
    let resolved = model.resolve();
    let label = model.generation_label(&resolved, false);
    let summary = model.open_summary();
    let live = model.live_status();
    let streaming = model.streaming();
    let connected = model.ready || (!model.connected && !model.offline);
    let link = &model.ops.usage.link;
    let (mut text, dot) = if !model.connected && model.offline {
        // A restart the daemon announced (PAR-029).
        let text = if link.shutdown.as_deref() == Some("restart") {
            "Daemon restarting…"
        } else if link.shutdown.is_some() {
            "Daemon stopped"
        } else if model.was_connected {
            "Reconnecting…"
        } else {
            "Connecting…"
        };
        (text.to_owned(), Some(Dot::Waiting))
    } else if !connected {
        ("Syncing…".to_owned(), Some(Dot::Waiting))
    } else if model.chat.error.is_some() {
        (
            "Couldn't load · Retry in menu".to_owned(),
            Some(Dot::Failed),
        )
    } else if live.is_some_and(|l| l.retrying) {
        (format!("Retrying · {label}"), Some(Dot::Waiting))
    } else if summary.is_some_and(|s| s.provider_waiting) || live.is_some_and(|l| l.waiting) {
        (format!("Waiting · {label}"), Some(Dot::Waiting))
    } else if streaming {
        (format!("Working · {label}"), Some(Dot::Live))
    } else if model.chat_pending() && !model.chat.loaded {
        ("Syncing…".to_owned(), Some(Dot::Live))
    } else {
        (model.generation_label(&resolved, true), None)
    };
    if connected
        && dot != Some(Dot::Failed)
        && let Some(goal) = summary.and_then(|s| s.goal.as_ref())
        && goal.status != "complete"
        && !goal.status.is_empty()
    {
        text = format!("{text} · goal {}", goal.status);
    }
    // The daemon's own network is down (PAR-030; not the phone's).
    if connected && link.online == Some(false) {
        text = format!("⚠ offline · {text}");
    }
    let detail = model
        .context_tokens()
        .filter(|_| connected && !streaming)
        .map(|tokens| format!(" · {} ctx", ops::format_tokens(tokens)));
    Subtitle { text, detail, dot }
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
    draw_subtitle(canvas, &subtitle(model), cx, width, alpha, now);
}

/// The subtitle as one centred group: dot, text, detail.
fn draw_subtitle(
    canvas: &mut Canvas,
    subtitle: &Subtitle,
    cx: f32,
    width: f32,
    alpha: f32,
    now: Instant,
) {
    let p = theme::p();
    let c = theme::c();
    let detail = subtitle.detail.clone().unwrap_or_default();
    let detail_w = widgets::measure(&detail, T_SUBTITLE);
    let dot_w = if subtitle.dot.is_some() {
        DOT + DOT_GAP
    } else {
        0.0
    };
    let main = widgets::fit(&subtitle.text, T_SUBTITLE, width - dot_w - detail_w);
    let main_w = widgets::measure(&main, T_SUBTITLE);
    let left = (cx - (dot_w + main_w + detail_w) / 2.0).round();
    if let Some(dot) = subtitle.dot {
        let (color, shade): (Rgb, f32) = match dot {
            Dot::Live => (p.accent, anim::pulse(now, 1400)),
            Dot::Waiting => (p.warning, 1.0),
            Dot::Failed => (p.error, 1.0),
        };
        let color = theme::mix(color, (shade * 100.0).round() as u32, p.app_background);
        canvas.fill_circle(
            left + DOT / 2.0,
            SUBTITLE_BASELINE - 10.0,
            DOT / 2.0,
            color,
            alpha,
        );
    }
    let x = left + dot_w;
    widgets::text(
        canvas,
        &main,
        x,
        SUBTITLE_BASELINE,
        T_SUBTITLE,
        c.text_dim,
        alpha,
    );
    if !detail.is_empty() {
        widgets::text(
            canvas,
            &detail,
            x + main_w,
            SUBTITLE_BASELINE,
            T_SUBTITLE,
            p.muted,
            alpha,
        );
    }
}

/// The ⋯ menu (PAR-010, `fake-09-chat-menu`): Stop and Retry when they
/// apply, the model, the operations panels, reload, read, export, the
/// work toggle, the id.
pub fn chat_menu(model: &Model) -> Sheet<ActionList> {
    let resolved = model.resolve();
    let label = model.generation_label(&resolved, true);
    let model_item = ActionItem::new(
        "Model, effort & speed…",
        Action::Ops(ops::Act::OpenInspector),
    )
    .icon(Icon::Sparkle)
    .detail(label.clone());
    // D's Discard draft (PAR-190), when there is one to discard.
    let discard = (!model.composer.text.trim().is_empty() && !model.submitting()).then(|| {
        ActionItem::new("Discard draft", Action::Composer(composer::Act::Discard))
            .icon(Icon::Trash)
            .tone(Tone::Destructive)
    });
    let Some(id) = model.chat.id.clone() else {
        let mut items = vec![
            model_item,
            ActionItem::new("System prompt", Action::Ops(ops::Act::OpenSystemPrompt))
                .icon(Icon::FileText)
                .detail("Read the effective prompt"),
        ];
        items.extend(discard);
        return action_sheet(ActionList::new("New chat", items).scoped());
    };
    let mut items = Vec::new();
    if model.streaming() {
        items.push(
            ActionItem::new("Stop", Action::Composer(composer::Act::Abort))
                .icon(Icon::Stop)
                .tone(Tone::Destructive),
        );
    }
    if model.chat.error.is_some() {
        items.push(
            ActionItem::new("Retry opening conversation", Action::Chat(chat::Act::Retry))
                .icon(Icon::Refresh),
        );
    }
    items.push(model_item);
    items.push(ActionItem::new("Find in chat", Action::Ops(ops::Act::OpenFind)).icon(Icon::Search));
    let tasks = model.activity_count(&id);
    items.push(
        ActionItem::new("Activity & tasks", Action::Ops(ops::Act::OpenActivity))
            .icon(Icon::ListChecks)
            .detail(match tasks {
                0 => "Nothing running".to_owned(),
                1 => "1 item".to_owned(),
                n => format!("{n} items"),
            }),
    );
    items.push(
        ActionItem::new("Goal", Action::Ops(ops::Act::OpenGoal))
            .icon(Icon::Target)
            .detail(sheets::goal::menu_detail(model, &id)),
    );
    items.push(
        ActionItem::new("Chrono schedules", Action::Ops(ops::Act::OpenChrono))
            .icon(Icon::Timer)
            .detail("Schedule a message or command"),
    );
    items.push(
        ActionItem::new("Ask aside (BTW)", Action::Ops(ops::Act::OpenBtw))
            .icon(Icon::MessageSquare)
            .detail(sheets::btw::menu_detail(model, &id)),
    );
    items.push(
        ActionItem::new("System prompt", Action::Ops(ops::Act::OpenSystemPrompt))
            .icon(Icon::FileText)
            .detail("Read the effective prompt"),
    );
    let usage = sheets::usage::compact(model.usage_of(&resolved.provider))
        .map(|(five, week)| {
            let pct =
                |v: Option<f64>| v.map_or_else(|| "?".to_owned(), |v| format!("{}", v.round()));
            format!("5h {}% · weekly {}% left", pct(five), pct(week))
        })
        .unwrap_or_else(|| "Provider limits".into());
    let (usage_label, usage_action) = if model.accounts_likely() {
        ("Accounts & usage", ops::Act::OpenAccounts)
    } else {
        ("Usage limits", ops::Act::OpenUsage)
    };
    items.push(
        ActionItem::new(usage_label, Action::Ops(usage_action))
            .icon(Icon::Gauge)
            .detail(usage),
    );
    items.push(
        ActionItem::new("Reload conversation", Action::Chat(chat::Act::Retry)).icon(Icon::Refresh),
    );
    // Their text is made when chosen (a menu's items are hashed every
    // frame).
    if last_reply(model).is_some() {
        items.push(
            ActionItem::new("Read the last reply", Action::Ops(ops::Act::ReadLastReply))
                .icon(Icon::FileText),
        );
    }
    if !model.chat.entries.is_empty() {
        items.push(
            ActionItem::new("Export loaded transcript", Action::Ops(ops::Act::Export))
                .icon(Icon::Export),
        );
    }
    let all = model.persist.settings().show_all_work;
    items.push(
        ActionItem::new(
            if all {
                "Fold finished work"
            } else {
                "Show all work"
            },
            Action::Ops(ops::Act::ToggleAllWork),
        )
        .icon(Icon::ListChecks)
        .detail(if all {
            "Replies only; tap a row to open one"
        } else {
            "Every tool call and notification"
        }),
    );
    // D's Edit… (PAR-268): queued and sent messages, instructions.
    items.push(
        ActionItem::new("Edit…", Action::Composer(composer::Act::EditPicker))
            .icon(Icon::Pencil)
            .detail("Rewind a sent message or edit the queue"),
    );
    items.push(
        ActionItem::new("Copy conversation id", Action::Ui(UiAct::Copy(id.clone())))
            .icon(Icon::Copy)
            .detail(id),
    );
    items.extend(discard);
    let mut line = label;
    if let Some(tokens) = model.context_tokens() {
        line = format!("{line} · {} context tokens", ops::grouped(tokens));
    }
    action_sheet(ActionList::new(title(model), items).subtitle(line).scoped())
}

/// The loaded history as plain text (PAR-333): who said what, tool calls
/// as one line each.
pub fn export(model: &Model) -> String {
    use super::super::proto::{Block, Entry};
    let mut out = Vec::new();
    for entry in &model.chat.entries {
        match entry {
            Entry::User { text, .. } => out.push(format!("You:\n{text}")),
            Entry::Ai { blocks, .. } => {
                let mut parts = Vec::new();
                for block in blocks {
                    match block {
                        Block::Text { text } if !text.trim().is_empty() => {
                            parts.push(text.trim().to_owned());
                        }
                        Block::ToolCall {
                            tool_name, summary, ..
                        } => parts.push(format!("[{tool_name}] {summary}")),
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    out.push(format!("Assistant:\n{}", parts.join("\n\n")));
                }
            }
            Entry::System { text, .. } if !text.trim().is_empty() => {
                out.push(format!("System: {}", text.trim()));
            }
            _ => {}
        }
    }
    out.join("\n\n")
}

/// The newest reply's text, if any is loaded.
pub fn last_reply(model: &Model) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;

    const CATALOG: &str = r#"{"type":"tools_available","providers":[{"id":"anthropic","label":"Anthropic","defaultModel":"opus","supportsFastMode":true,"models":[{"id":"opus","label":"Opus 5.5","defaultEffort":"high","supportedEfforts":[{"effort":"high"}]}]}],"authByProvider":{"anthropic":true},"tools":[],"conversationDefaults":{"defaults":{"provider":"anthropic","model":"opus","effort":"xhigh","fastMode":false},"configured":false}}"#;

    /// Connected, the daemon ready, the catalog in, conversation `c`
    /// (with `fields`) open.
    fn open(fields: &str) -> Model {
        let mut model = Model::new();
        let commands = model.on_connected();
        let ping = commands.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        let now = Instant::now();
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            now,
        );
        model.apply(parse_event(CATALOG), now);
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_updated","summary":{{"id":"c","title":"T","provider":"anthropic","model":"opus","effort":"high"{fields}}}}}"#
            )),
            now,
        );
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        model
    }

    fn event(model: &mut Model, line: &str) {
        model.apply(parse_event(line), Instant::now());
    }

    fn loaded(model: &mut Model) {
        let id = model.chat.load_request.clone().unwrap();
        event(
            model,
            &format!(
                r#"{{"type":"conversation_loaded","reqId":"{id}","convId":"c","entries":[],"contextTokens":1000}}"#
            ),
        );
    }

    fn says(model: &Model) -> (String, Option<Dot>) {
        let s = subtitle(model);
        (s.text, s.dot)
    }

    #[test]
    fn the_subtitle_follows_the_iphones_priorities() {
        let mut model = open("");
        // The window on its way, nothing shown yet.
        assert_eq!(says(&model), ("Syncing…".into(), Some(Dot::Live)));
        loaded(&mut model);
        assert_eq!(says(&model), ("Opus 5.5 · high".into(), None));
        assert_eq!(subtitle(&model).detail.as_deref(), Some(" · 1.0k ctx"));
        // Working, then waiting, then retrying, each over the last.
        event(
            &mut model,
            r#"{"type":"streaming_started","convId":"c","provider":"anthropic","model":"opus","startedAt":5}"#,
        );
        assert_eq!(says(&model), ("Working · Opus 5.5".into(), Some(Dot::Live)));
        assert_eq!(subtitle(&model).detail, None, "no context while working");
        event(
            &mut model,
            r#"{"type":"connection_wait_status","convId":"c","active":true}"#,
        );
        assert_eq!(
            says(&model),
            ("Waiting · Opus 5.5".into(), Some(Dot::Waiting))
        );
        event(
            &mut model,
            r#"{"type":"stream_retry","convId":"c","attempt":1,"maxAttempts":3,"errorMessage":"x","delaySec":2}"#,
        );
        assert_eq!(
            says(&model),
            ("Retrying · Opus 5.5".into(), Some(Dot::Waiting))
        );
        // A failed load over all of those.
        model.chat.error = Some("boom".into());
        assert_eq!(
            says(&model),
            ("Couldn't load · Retry in menu".into(), Some(Dot::Failed))
        );
        // The connection over everything.
        model.on_disconnected();
        assert!(model.offline);
        assert_eq!(says(&model), ("Reconnecting…".into(), Some(Dot::Waiting)));
        model.on_connected();
        assert_eq!(says(&model), ("Syncing…".into(), Some(Dot::Waiting)));
    }

    #[test]
    fn a_restarting_or_offline_daemon_says_so() {
        let mut model = open("");
        loaded(&mut model);
        event(
            &mut model,
            r#"{"type":"connectivity_status","online":false,"offlineSince":1}"#,
        );
        assert_eq!(says(&model).0, "⚠ offline · Opus 5.5 · high");
        event(
            &mut model,
            r#"{"type":"connectivity_status","online":true}"#,
        );
        event(&mut model, r#"{"type":"daemon_shutdown","mode":"restart"}"#);
        model.on_disconnected();
        assert_eq!(
            says(&model),
            ("Daemon restarting…".into(), Some(Dot::Waiting))
        );
        model.on_connected();
        model.on_disconnected();
        assert_eq!(
            says(&model).0,
            "Reconnecting…",
            "a new connection forgets it"
        );
    }

    #[test]
    fn the_first_connection_says_connecting_and_a_quiet_one_nothing() {
        let model = Model::new();
        assert_eq!(says(&model), ("Connecting…".into(), Some(Dot::Waiting)));
        let mut model = open("");
        loaded(&mut model);
        let now = Instant::now();
        model.reconnect_quietly(now, std::time::Duration::from_millis(1500));
        model.on_disconnected();
        assert_eq!(says(&model), ("Opus 5.5 · high".into(), None));
    }

    #[test]
    fn a_goal_and_speed_show_in_the_label() {
        let mut model = open(r#","fastMode":true,"goal":{"objective":"o","status":"paused"}"#);
        loaded(&mut model);
        assert_eq!(
            says(&model),
            ("Opus 5.5 · high · fast · goal paused".into(), None)
        );
        let mut model = open(r#","goal":{"objective":"o","status":"complete"}"#);
        loaded(&mut model);
        assert_eq!(says(&model).0, "Opus 5.5 · high");
    }

    #[test]
    fn a_draft_says_its_model_or_the_daemon_default() {
        let mut model = Model::new();
        model.handle(Action::Nav(Nav::NewConversation));
        model.on_connected();
        model.ready = true;
        assert_eq!(says(&model), ("Daemon default".into(), None));
        event(&mut model, CATALOG);
        assert_eq!(says(&model), ("Opus 5.5 · xhigh".into(), None));
    }

    #[test]
    fn the_menu_offers_retry_and_stop_when_they_apply() {
        let mut model = open(r#","streaming":true"#);
        model.chat.error = Some("boom".into());
        let menu = chat_menu(&model);
        let labels: Vec<&str> = menu
            .content
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "Stop",
                "Retry opening conversation",
                "Model, effort & speed…",
                "Find in chat",
                "Activity & tasks",
                "Goal",
                "Chrono schedules",
                "Ask aside (BTW)",
                "System prompt",
                "Usage limits",
                "Reload conversation",
                "Show all work",
                "Edit…",
                "Copy conversation id"
            ]
        );
    }
}
