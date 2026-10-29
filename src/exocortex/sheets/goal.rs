//! Goal (WP-E; PAR-297, 298; the iPhone's `ui/ops/goal`): the
//! conversation's goal (objective, status, turns, active time against its
//! limit, reason) with Pause, Resume, Complete and Clear, and setting a
//! new one: the objective typed in a prompt, then its time limit chosen.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;

use super::super::model::{Action, Model, now_ms, ops};
use super::super::proto::{self, Event, Goal};
use super::super::requests::{self, Owner};
use super::super::theme;
use super::super::ui::menu::Confirm;
use super::super::ui::sheet::{ActionItem, ActionList, Prompt, action_sheet, prompt_sheet};
use super::super::ui::textarea::{self, TextArea};
use super::clock;
use super::panel::{self, Btn, Built, Kind, Row, Style, Tap};

/// The goals the daemon reported (`goal_updated`, a window's `goal`),
/// newer than the list's summary until the next summary comes.
#[derive(Debug, Default)]
pub struct State {
    pub goals: HashMap<String, Option<Goal>>,
    /// The last `goal_updated` message, per conversation.
    pub messages: HashMap<String, (String, Instant)>,
    /// A request in flight: its reqId, by conversation.
    pub busy: HashMap<String, String>,
    pub errors: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    /// "Set a goal…": the objective prompt.
    New,
    /// The objective typed: choose a time limit.
    Objective(String),
    /// A custom time limit for the objective.
    Custom(String),
    /// The time limit typed.
    CustomTime(String, String),
    Set(String, Option<u64>),
    /// `pause`, `resume`, `complete`.
    Do(&'static str),
    Clear,
    ClearConfirmed,
}

const LIMITS: [(&str, u64); 6] = [
    ("30 minutes", 1_800_000),
    ("1 hour", 3_600_000),
    ("2 hours", 7_200_000),
    ("4 hours", 14_400_000),
    ("8 hours", 28_800_000),
    ("1 day", 86_400_000),
];

fn act(a: Act) -> Action {
    Action::Ops(ops::Act::Goal(a))
}

impl Model {
    /// The conversation's goal: the daemon's last word on it, else the
    /// list's summary.
    pub fn goal_of(&self, conv: &str) -> Option<Goal> {
        match self.ops.goal.goals.get(conv) {
            Some(goal) => goal.clone(),
            None => self
                .sidebar
                .conversations
                .get(conv)
                .and_then(|s| s.goal.clone()),
        }
    }

    pub(crate) fn handle_goal(&mut self, a: Act) -> Vec<Value> {
        let Some(conv) = self.chat.id.clone() else {
            self.ui.toast("Open a conversation to set a goal.");
            return Vec::new();
        };
        match a {
            Act::New => {
                self.ui.open(Box::new(textarea::sheet(TextArea::new(
                    "Set a goal",
                    "What should this conversation work toward?",
                    "",
                    "Next",
                    Box::new(|text| act(Act::Objective(text))),
                ))));
                Vec::new()
            }
            Act::Objective(objective) => {
                if objective.starts_with("--max-turns") {
                    self.ui
                        .error_toast("--max-turns was removed. Use a max time instead.");
                    return Vec::new();
                }
                let mut items = vec![
                    ActionItem::new("No limit", act(Act::Set(objective.clone(), None)))
                        .detail("Until it is met, paused or cleared"),
                ];
                for (label, ms) in LIMITS {
                    items.push(ActionItem::new(
                        label,
                        act(Act::Set(objective.clone(), Some(ms))),
                    ));
                }
                items.push(ActionItem::new("Custom…", act(Act::Custom(objective))));
                self.ui.open(Box::new(action_sheet(
                    ActionList::new("Max active time", items).scoped(),
                )));
                Vec::new()
            }
            Act::Custom(objective) => {
                self.ui.open(Box::new(prompt_sheet(Prompt::new(
                    "Max active time",
                    "Like 9h2m1s, 8h3m, 1h, 5m or 2d",
                    "",
                    "Set goal",
                    Box::new(move |text| act(Act::CustomTime(objective.clone(), text))),
                ))));
                Vec::new()
            }
            Act::CustomTime(objective, text) => match clock::parse_goal_duration(&text) {
                Some(ms) => self.goal_request(&conv, "set", Some(&objective), Some(ms)),
                None => {
                    self.ui
                        .error_toast("Max time looks like 9h2m1s, 8h3m, 1h, 5m or 2d.");
                    Vec::new()
                }
            },
            Act::Set(objective, ms) => self.goal_request(&conv, "set", Some(&objective), ms),
            Act::Do(action) => self.goal_request(&conv, action, None, None),
            Act::Clear => {
                let objective = self.goal_of(&conv).map(|g| g.objective).unwrap_or_default();
                self.ui.open(Box::new(Confirm::new(
                    "Clear this goal?",
                    objective,
                    "Clear goal",
                    act(Act::ClearConfirmed),
                )));
                Vec::new()
            }
            Act::ClearConfirmed => self.goal_request(&conv, "clear", None, None),
        }
    }

    fn goal_request(
        &mut self,
        conv: &str,
        action: &'static str,
        objective: Option<&str>,
        max_time_ms: Option<u64>,
    ) -> Vec<Value> {
        if !self.ready {
            self.ui.error_toast("Not connected to Exocortex.");
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Ops,
            "set_goal",
            Some(conv),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.ops.goal.busy.insert(conv.to_owned(), request.clone());
        self.ops.goal.errors.remove(conv);
        self.ui.haptic(super::super::ui::Haptic::Light);
        vec![proto::set_goal(
            &request,
            conv,
            action,
            objective,
            max_time_ms,
        )]
    }

    pub(crate) fn apply_goal(&mut self, event: &Event, now: Instant) {
        match event {
            Event::GoalUpdated(e) => {
                self.ops
                    .goal
                    .goals
                    .insert(e.conv_id.clone(), e.goal.clone());
                if let Some(message) = &e.message {
                    self.ops
                        .goal
                        .messages
                        .insert(e.conv_id.clone(), (message.clone(), now));
                }
                if e.req_id.is_some() && self.ops.goal.busy.get(&e.conv_id) == e.req_id.as_ref() {
                    self.ops.goal.busy.remove(&e.conv_id);
                }
            }
            Event::ConversationLoaded { conv_id, goal, .. } => {
                self.ops.goal.goals.insert(conv_id.clone(), goal.clone());
            }
            // The list's summary is newer from now on.
            Event::ConversationUpdated { summary, .. } => {
                self.ops.goal.goals.remove(&summary.id);
            }
            _ => {}
        }
    }

    /// An error answering a goal request; true when it was one.
    pub(crate) fn goal_error(&mut self, id: &str, message: &str) -> bool {
        let Some(conv) = self
            .ops
            .goal
            .busy
            .iter()
            .find(|(_, request)| request.as_str() == id)
            .map(|(conv, _)| conv.clone())
        else {
            return false;
        };
        self.ops.goal.busy.remove(&conv);
        self.ops.goal.errors.insert(conv, message.to_owned());
        true
    }
}

/// The Goal panel's rows.
pub fn build(model: &Model, conv: &str, _width: f32, now: Instant) -> Built {
    let p = theme::p();
    let now_ms = now_ms() as f64;
    let mut rows = Vec::new();
    let goal = model.goal_of(conv);
    let busy = model.ops.goal.busy.contains_key(conv);
    let mut wake = None;
    match &goal {
        Some(goal) => {
            let mut lines = vec![
                (goal.objective.clone(), Style::Body, p.text),
                (
                    clock::goal_summary_line(goal, now_ms),
                    Style::Caption,
                    p.goal,
                ),
            ];
            if let Some(reason) = goal.reason.as_ref().filter(|r| !r.is_empty()) {
                lines.push((format!("Reason: {reason}"), Style::Caption, p.muted));
            }
            if let Some(max) = goal.max_time_ms.filter(|max| *max > 0.0) {
                let left = (max as u64).saturating_sub(clock::goal_active_ms(goal, now_ms));
                lines.push((
                    format!("Time left: {}", clock::format_goal_duration(left)),
                    Style::Caption,
                    p.muted,
                ));
            }
            if goal.created_at > 0.0 {
                lines.push((
                    format!("Set {}", clock::format_date_time(goal.created_at)),
                    Style::Caption,
                    p.muted,
                ));
            }
            rows.push(Row::Rail {
                color: p.goal,
                lines,
            });
            if let Some((message, _)) = model.ops.goal.messages.get(conv) {
                rows.push(panel::text(message.clone(), Style::Caption, p.muted));
            }
            let tap = |a: Act| (!busy).then_some(Tap::Stay(act(a)));
            let mut buttons = Vec::new();
            match goal.status.as_str() {
                "active" => buttons.push(Btn::new("Pause", Kind::Secondary, tap(Act::Do("pause")))),
                "paused" | "blocked" => {
                    buttons.push(Btn::new("Resume", Kind::Secondary, tap(Act::Do("resume"))));
                }
                _ => {}
            }
            if goal.status != "complete" {
                buttons.push(Btn::new(
                    "Complete",
                    Kind::Secondary,
                    tap(Act::Do("complete")),
                ));
            }
            buttons.push(Btn::new("Clear", Kind::Destructive, tap(Act::Clear)));
            rows.push(Row::Buttons(buttons));
            if goal.status == "active" {
                let since = goal.active_since.unwrap_or(now_ms);
                wake = clock::next_change(since, None, now_ms, now);
            }
        }
        None => rows.push(panel::note(
            "No goal. A goal keeps the conversation working on its own until the objective \
             is met, paused or out of time.",
        )),
    }
    rows.push(panel::section(if goal.is_some() {
        "Replace with a new goal"
    } else {
        "Set a goal"
    }));
    rows.push(Row::Buttons(vec![Btn::new(
        if busy { "Working…" } else { "Set a goal…" },
        Kind::Primary,
        (!busy && model.ready).then_some(Tap::Stay(act(Act::New))),
    )]));
    if let Some(error) = model.ops.goal.errors.get(conv) {
        rows.push(panel::text(error.clone(), Style::Caption, p.error));
    }
    Built {
        title: "Goal".into(),
        rows,
        wake,
    }
}

/// The chat menu's line for the goal.
pub fn menu_detail(model: &Model, conv: &str) -> String {
    match model.goal_of(conv) {
        Some(goal) => clock::goal_summary_line(&goal, now_ms() as f64),
        None => "Set an objective".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;

    fn ready() -> Model {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","title":"T","goal":{"objective":"Ship it","status":"active","turns":2,"activeMs":60000,"activeSince":1,"createdAt":1}}}"#),
            Instant::now(),
        );
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        model
    }

    #[test]
    fn the_goal_panel_shows_the_goal_and_its_actions() {
        let model = ready();
        let built = build(&model, "c", 992.0, Instant::now());
        let Row::Rail { lines, .. } = &built.rows[0] else {
            panic!("{:?}", built.rows[0])
        };
        assert_eq!(lines[0].0, "Ship it");
        assert!(lines[1].0.starts_with("◆ active · 2 turns"));
        let Row::Buttons(buttons) = &built.rows[1] else {
            panic!()
        };
        let labels: Vec<&str> = buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Pause", "Complete", "Clear"]);
        assert!(built.wake.is_some(), "the active time ticks");
    }

    #[test]
    fn setting_a_goal_asks_objective_then_limit_and_sends() {
        let mut model = ready();
        model.handle(Action::Ops(ops::Act::Goal(Act::New)));
        assert_eq!(model.ui.opening.pop().unwrap().label(), "Set a goal");
        model.handle(Action::Ops(ops::Act::Goal(Act::Objective("Fix CI".into()))));
        assert_eq!(model.ui.opening.pop().unwrap().label(), "Max active time");
        let sent = model.handle(Action::Ops(ops::Act::Goal(Act::Set(
            "Fix CI".into(),
            Some(3_600_000),
        ))));
        assert_eq!(sent[0]["type"], "set_goal");
        assert_eq!(sent[0]["action"], "set");
        assert_eq!(sent[0]["objective"], "Fix CI");
        assert_eq!(sent[0]["maxTimeMs"], 3_600_000);
        let id = sent[0]["reqId"].as_str().unwrap().to_owned();
        assert!(model.ops.goal.busy.contains_key("c"));
        model.apply(
            parse_event(&format!(
                r#"{{"type":"goal_updated","reqId":"{id}","convId":"c","goal":{{"objective":"Fix CI","status":"active","turns":0}},"message":"Goal set."}}"#
            )),
            Instant::now(),
        );
        assert!(model.ops.goal.busy.is_empty());
        assert_eq!(model.goal_of("c").unwrap().objective, "Fix CI");
        // A bad custom limit says so and sends nothing.
        assert!(
            model
                .handle(Action::Ops(ops::Act::Goal(Act::CustomTime(
                    "x".into(),
                    "soon".into()
                ))))
                .is_empty()
        );
        // An error lands in the panel.
        let sent = model.handle(Action::Ops(ops::Act::Goal(Act::Do("pause"))));
        let id = sent[0]["reqId"].as_str().unwrap().to_owned();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"error","reqId":"{id}","message":"No goal"}}"#
            )),
            Instant::now(),
        );
        assert_eq!(
            model.ops.goal.errors.get("c").map(String::as_str),
            Some("No goal")
        );
    }
}
