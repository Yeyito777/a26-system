//! Activity & tasks (WP-E; PAR-299–302, 309; the TUI's activity panel as
//! the iPhone shows it): the conversation's incomplete goal, then its
//! tasks (subagents, Agent/Workflow, Chrono but `wait`, background
//! commands), then its subscriptions. A subagent's row opens it; ⋯ offers
//! stopping a task exactly (`stop_task`), a subagent (`abort`), or a
//! schedule.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::Value;

use super::super::model::{Action, Model, Nav, UiAct, now_ms, ops};
use super::super::proto::{self, Event, TaskSummary};
use super::super::requests::{self, Owner};
use super::super::theme;
use super::super::ui::menu::Confirm;
use super::super::ui::sheet::{ActionItem, ActionList, Tone, action_sheet};
use super::clock;
use super::panel::{self, Btn, Built, Kind, Row, Style, Tap};

const EXO: &str = "#1d9bf0";
const BASH: &str = "#d19a66";
const GOAL: &str = "#c792ea";
const CHRONO: &str = "#4ec9b0";

/// Tasks as `list_tasks` reported them (status, tool), by conversation.
#[derive(Debug, Default)]
pub struct State {
    pub tasks: HashMap<String, Vec<TaskSummary>>,
    /// `list_tasks` in flight, by reqId: its conversation.
    pub listing: HashMap<String, String>,
    /// Tasks asked to stop, shown as "stopping…" until they go.
    pub stopping: HashSet<String>,
    /// `stop_task` in flight, by reqId: the task.
    pub stops: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    /// A task's actions.
    More(String),
    Stop(String),
    StopConfirmed(String),
    StopSubagent(String),
    StopSubagentConfirmed(String),
}

fn act(a: Act) -> Action {
    Action::Ops(ops::Act::Activity(a))
}

/// A display row of the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub kind: &'static str,
    pub label: String,
    pub color: u32,
    pub title: String,
    pub time: String,
    pub task: Option<TaskSummary>,
    pub divider: Option<&'static str>,
    pub wake: Option<Instant>,
}

fn shown(task: &TaskSummary) -> bool {
    task.kind != "chrono" || task.chrono_mode.as_deref() != Some("wait")
}

fn agent(task: &TaskSummary) -> bool {
    task.kind == "background"
        && matches!(task.tool_name.as_deref(), Some("Agent") | Some("Workflow"))
}

fn clean(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

impl Model {
    fn tool_color(&self, name: &str, fallback: &str) -> u32 {
        self.ops
            .tool_styles
            .get(name)
            .and_then(|s| theme::parse_hex(&s.color))
            .or_else(|| theme::parse_hex(fallback))
            .unwrap_or(theme::p().tool)
    }

    /// The activity rows of a conversation (the TUI's order).
    pub fn activity_items(&self, conv: &str, now: Instant) -> Vec<Item> {
        let now_ms = now_ms() as f64;
        let mut items = Vec::new();
        if let Some(goal) = self.goal_of(conv).filter(|g| g.status != "complete") {
            let active = goal.status == "active";
            let title = if let Some(reason) = goal.reason.as_ref().filter(|r| !r.is_empty()) {
                format!("{} — {reason}", goal.objective)
            } else {
                goal.objective.clone()
            };
            items.push(Item {
                key: format!("goal:{}", goal.created_at),
                kind: "goal",
                label: format!("{} Goal", if active { "◆" } else { "◇" }),
                color: self.tool_color("goal", GOAL),
                title: clean(&title),
                time: if active {
                    clock::format_task_elapsed(goal.created_at, now_ms)
                } else {
                    goal.status.clone()
                },
                task: None,
                divider: None,
                wake: active
                    .then(|| clock::next_change(goal.created_at, None, now_ms, now))
                    .flatten(),
            });
        }
        let summary = self.sidebar.conversations.get(conv);
        for task in summary
            .map_or(&[][..], |s| &s.tasks[..])
            .iter()
            .filter(|t| shown(t))
        {
            let sub = task.kind == "subagent";
            let agent = agent(task);
            let chrono = task.kind == "chrono";
            let countdown = chrono && !task.chrono_running && task.due_at.is_some();
            let tool = || match task.tool_name.as_deref() {
                None | Some("bash") | Some("exec_command") => "Bash".to_owned(),
                Some(name) => name.to_owned(),
            };
            let (kind, label, color) = if sub {
                (
                    "subagent",
                    "◆ Exocortex".to_owned(),
                    self.tool_color("exo", EXO),
                )
            } else if agent {
                (
                    "agent",
                    format!("◆ {}", tool()),
                    self.tool_color("exo", EXO),
                )
            } else if chrono {
                (
                    "chrono",
                    "◷ Chrono".to_owned(),
                    self.tool_color("chrono", CHRONO),
                )
            } else {
                (
                    "bash",
                    format!("$ {}", tool()),
                    self.tool_color("bash", BASH),
                )
            };
            let fallback = match kind {
                "subagent" | "agent" => "Subagent task",
                "chrono" => "Chrono task",
                _ => "Background task",
            };
            let title = clean(&task.title);
            let due = task.due_at.filter(|_| countdown);
            items.push(Item {
                key: format!("task:{}:{}", task.kind, task.id),
                kind,
                label,
                color,
                title: if title.is_empty() {
                    fallback.into()
                } else {
                    title
                },
                time: if self.ops.activity.stopping.contains(&task.id) {
                    "stopping…".into()
                } else if let Some(due) = due {
                    clock::format_task_countdown(due, now_ms)
                } else {
                    clock::format_task_elapsed(task.started_at, now_ms)
                },
                task: Some(task.clone()),
                divider: None,
                wake: clock::next_change(task.started_at, due, now_ms, now),
            });
        }
        let integrations = summary.map_or(&[][..], |s| &s.integrations[..]);
        for (index, integration) in integrations.iter().enumerate() {
            let get = |key: &str| integration[key].as_str().unwrap_or("").to_owned();
            let tool = get("toolName");
            let style = self.ops.external_styles.iter().find(|s| s.cmd == tool);
            let label = style.map_or(tool.clone(), |s| s.label.clone());
            let title = [get("label"), get("description"), get("sourceId")]
                .into_iter()
                .map(|t| clean(&t))
                .find(|t| !t.is_empty())
                .unwrap_or_else(|| "Subscription".into());
            items.push(Item {
                key: format!("sub:{}", get("id")),
                kind: "integration",
                label: if label.is_empty() {
                    "External".into()
                } else {
                    clean(&label)
                },
                color: style
                    .and_then(|s| theme::parse_hex(&s.color))
                    .unwrap_or(theme::p().tool),
                title,
                time: format!("{} {}", get("delivery"), get("status"))
                    .trim()
                    .to_owned(),
                task: None,
                divider: (index == 0 && !items.is_empty()).then_some("Subscriptions"),
                wake: None,
            });
        }
        items
    }

    /// What the chat's tasks chip shows, without building the rows (it
    /// asks every frame): the kinds of work, in `activity_items`' order and
    /// colours (the goal ◆ or ◇, subagents ◆, Chrono ◷, commands $), and how
    /// many rows the panel has.
    pub fn activity_glance(&self, conv: &str) -> (Vec<(&'static str, u32)>, usize) {
        let mut glyphs = Vec::new();
        let mut count = 0;
        let summary = self.sidebar.conversations.get(conv);
        let goal = match self.ops.goal.goals.get(conv) {
            Some(goal) => goal.as_ref(),
            None => summary.and_then(|s| s.goal.as_ref()),
        };
        if let Some(goal) = goal.filter(|g| g.status != "complete") {
            count += 1;
            let glyph = if goal.status == "active" {
                "◆"
            } else {
                "◇"
            };
            glyphs.push((glyph, self.tool_color("goal", GOAL)));
        }
        let (mut agents, mut chrono, mut bash) = (false, false, false);
        for task in summary
            .map_or(&[][..], |s| &s.tasks[..])
            .iter()
            .filter(|t| shown(t))
        {
            count += 1;
            if task.kind == "subagent" || agent(task) {
                agents = true;
            } else if task.kind == "chrono" {
                chrono = true;
            } else {
                bash = true;
            }
        }
        count += summary.map_or(0, |s| s.integrations.len());
        if agents {
            glyphs.push(("◆", self.tool_color("exo", EXO)));
        }
        if chrono {
            glyphs.push(("◷", self.tool_color("chrono", CHRONO)));
        }
        if bash {
            glyphs.push(("$", self.tool_color("bash", BASH)));
        }
        (glyphs, count)
    }

    /// How many rows the panel has (the chat menu's detail).
    pub fn activity_count(&self, conv: &str) -> usize {
        self.activity_items(conv, Instant::now()).len()
    }

    /// The panel opens: ask for the tasks' details.
    pub(crate) fn list_tasks(&mut self, conv: &str) -> Vec<Value> {
        if !self.ready {
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Ops,
            "list_tasks",
            Some(conv),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.ops
            .activity
            .listing
            .insert(request.clone(), conv.to_owned());
        vec![proto::list_tasks(&request, conv)]
    }

    pub(crate) fn handle_activity(&mut self, act_: Act) -> Vec<Value> {
        let Some(conv) = self.chat.id.clone() else {
            return Vec::new();
        };
        match act_ {
            Act::More(key) => {
                let items = self.activity_items(&conv, Instant::now());
                let Some(item) = items.into_iter().find(|i| i.key == key) else {
                    return Vec::new();
                };
                let Some(task) = item.task.clone() else {
                    return Vec::new();
                };
                let detail = self
                    .ops
                    .activity
                    .tasks
                    .get(&conv)
                    .and_then(|tasks| tasks.iter().find(|t| t.id == task.id))
                    .map(|t| {
                        [
                            t.tool_name.as_ref().map(|n| format!("tool {n}")),
                            t.status.clone(),
                            Some(format!("id {}", t.id)),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ")
                    })
                    .unwrap_or_else(|| format!("{} · {}", item.label, item.time));
                let items = match item.kind {
                    "subagent" => vec![
                        ActionItem::new(
                            "Open conversation",
                            Action::Nav(Nav::OpenConversation(task.id.clone())),
                        )
                        .icon(super::super::ui::icons::Icon::Chat),
                        ActionItem::new("Stop subagent", act(Act::StopSubagent(task.id.clone())))
                            .icon(super::super::ui::icons::Icon::Stop)
                            .tone(Tone::Destructive),
                    ],
                    "chrono" => {
                        let mut items = vec![
                            ActionItem::new("Chrono schedules", Action::Ops(ops::Act::OpenChrono))
                                .icon(super::super::ui::icons::Icon::Timer),
                        ];
                        if task.id.starts_with("chrono:")
                            && task.chrono_mode.as_deref() == Some("wake")
                        {
                            items.push(
                                ActionItem::new(
                                    "Cancel schedule",
                                    Action::Ops(ops::Act::Chrono(super::chrono::Act::Cancel(
                                        task.id.clone(),
                                    ))),
                                )
                                .icon(super::super::ui::icons::Icon::X)
                                .tone(Tone::Destructive),
                            );
                        }
                        items
                    }
                    _ => vec![
                        ActionItem::new("Copy task id", Action::Ui(UiAct::Copy(task.id.clone())))
                            .icon(super::super::ui::icons::Icon::Copy),
                        ActionItem::new("Stop exact task", act(Act::Stop(task.id.clone())))
                            .icon(super::super::ui::icons::Icon::Stop)
                            .tone(Tone::Destructive),
                    ],
                };
                self.ui.open(Box::new(action_sheet(
                    ActionList::new(item.title, items).subtitle(detail).scoped(),
                )));
                Vec::new()
            }
            Act::Stop(task) => {
                let title = self
                    .activity_items(&conv, Instant::now())
                    .into_iter()
                    .find(|i| i.task.as_ref().is_some_and(|t| t.id == task))
                    .map_or_else(String::new, |i| format!("{} · {}", i.label, i.title));
                self.ui.open(Box::new(Confirm::new(
                    "Stop this task?",
                    title,
                    "Stop task",
                    act(Act::StopConfirmed(task)),
                )));
                Vec::new()
            }
            Act::StopConfirmed(task) => {
                if !self.ready {
                    self.ui.error_toast("Not connected to Exocortex.");
                    return Vec::new();
                }
                let request = self.requests.issue(
                    Owner::Ops,
                    "stop_task",
                    Some(&conv),
                    requests::DEFAULT_WITHIN,
                    Instant::now(),
                );
                self.ops.activity.stopping.insert(task.clone());
                self.ops
                    .activity
                    .stops
                    .insert(request.clone(), task.clone());
                self.ui.haptic(super::super::ui::Haptic::Medium);
                vec![proto::stop_task(&request, &conv, &task)]
            }
            Act::StopSubagent(child) => {
                let title = self
                    .sidebar
                    .conversations
                    .get(&child)
                    .map(|s| s.display_title().to_owned())
                    .unwrap_or_default();
                self.ui.open(Box::new(Confirm::new(
                    "Stop this subagent?",
                    title,
                    "Stop subagent",
                    act(Act::StopSubagentConfirmed(child)),
                )));
                Vec::new()
            }
            Act::StopSubagentConfirmed(child) => {
                if !self.connected {
                    return Vec::new();
                }
                let request = self.requests.issue(
                    Owner::Ops,
                    "abort",
                    Some(&child),
                    requests::ABORT_WITHIN,
                    Instant::now(),
                );
                self.ui.haptic(super::super::ui::Haptic::Medium);
                vec![proto::abort(&request, &child, None)]
            }
        }
    }

    pub(crate) fn apply_activity(&mut self, event: &Event) -> Vec<Value> {
        match event {
            Event::TasksList(e) => {
                if let Some(conv) = e
                    .req_id
                    .as_ref()
                    .and_then(|id| self.ops.activity.listing.remove(id))
                {
                    let tasks: Vec<TaskSummary> = e
                        .tasks
                        .iter()
                        .filter(|t| t.owner_conversation_id.as_deref().is_none_or(|o| o == conv))
                        .cloned()
                        .collect();
                    // A stopped task leaves the list.
                    self.ops
                        .activity
                        .stopping
                        .retain(|id| e.tasks.iter().any(|t| t.id == *id));
                    self.ops.activity.tasks.insert(conv, tasks);
                }
                Vec::new()
            }
            Event::TaskStopped(e) => {
                if e.req_id
                    .as_ref()
                    .and_then(|id| self.ops.activity.stops.remove(id))
                    .is_some()
                {
                    return self.list_tasks(&e.conv_id.clone());
                }
                Vec::new()
            }
            Event::ConversationUpdated { summary, .. } => {
                let ids: HashSet<&str> = summary.tasks.iter().map(|t| t.id.as_str()).collect();
                if self.chat.id.as_deref() == Some(summary.id.as_str()) {
                    self.ops
                        .activity
                        .stopping
                        .retain(|id| ids.contains(id.as_str()));
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// An error answering a task request; true when it was one.
    pub(crate) fn activity_error(&mut self, id: &str) -> bool {
        if let Some(task) = self.ops.activity.stops.remove(id) {
            self.ops.activity.stopping.remove(&task);
            return true;
        }
        self.ops.activity.listing.remove(id).is_some()
    }
}

/// The Activity panel's rows.
pub fn build(model: &Model, conv: &str, now: Instant) -> Built {
    let p = theme::p();
    let items = model.activity_items(conv, now);
    let mut rows = Vec::new();
    let mut wake = None;
    if items.is_empty() {
        rows.push(panel::note(
            "No goal, tasks or subscriptions in this conversation.",
        ));
    }
    for item in &items {
        if let Some(divider) = item.divider {
            rows.push(Row::Sub(divider.into()));
        }
        let (tap, more) = match (item.kind, &item.task) {
            ("goal", _) => (Some(Tap::Stay(Action::Ops(ops::Act::OpenGoal))), None),
            ("subagent", Some(task)) => (
                Some(Tap::Close(Action::Nav(Nav::OpenConversation(
                    task.id.clone(),
                )))),
                Some(Tap::Stay(act(Act::More(item.key.clone())))),
            ),
            (_, Some(_)) => (
                Some(Tap::Stay(act(Act::More(item.key.clone())))),
                Some(Tap::Stay(act(Act::More(item.key.clone())))),
            ),
            _ => (None, None),
        };
        rows.push(Row::Task {
            label: item.label.clone(),
            color: item.color,
            title: item.title.clone(),
            time: item.time.clone(),
            tap,
            more,
        });
        wake = clock::sooner(wake, item.wake);
    }
    let summary = model.sidebar.conversations.get(conv);
    let live = model.ops.live.get(conv);
    if live.is_some_and(|l| l.waiting) || summary.is_some_and(|s| s.provider_waiting) {
        rows.push(panel::text(
            "Waiting for provider capacity or the network",
            Style::Caption,
            p.warning,
        ));
    }
    if let Some((message, _)) = model.ops.goal.messages.get(conv) {
        rows.push(panel::text(message.clone(), Style::Caption, p.muted));
    }
    rows.push(Row::Buttons(vec![
        Btn::new(
            "Goal…",
            Kind::Secondary,
            Some(Tap::Stay(Action::Ops(ops::Act::OpenGoal))),
        ),
        Btn::new(
            "Chrono…",
            Kind::Secondary,
            Some(Tap::Stay(Action::Ops(ops::Act::OpenChrono))),
        ),
    ]));
    Built {
        title: "Activity".into(),
        rows,
        wake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;

    #[test]
    fn rows_come_in_the_tuis_order_and_stop_exactly() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        let now = Instant::now();
        let t = now_ms();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_updated","summary":{{"id":"c","title":"T","goal":{{"objective":"Goal","status":"paused","turns":1}},"tasks":[{{"id":"s1","kind":"subagent","title":"Child","startedAt":{t}}},{{"id":"chrono:w","kind":"chrono","title":"Wait","chronoMode":"wait","startedAt":{t}}},{{"id":"b1","kind":"background","toolName":"bash","title":"make","startedAt":{t}}}],"integrations":[{{"id":"i1","toolName":"gmail","label":"Inbox","delivery":"push","status":"ok"}}]}}}}"#
            )),
            now,
        );
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let items = model.activity_items("c", now);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["◇ Goal", "◆ Exocortex", "$ Bash", "gmail"]);
        assert_eq!(items[0].time, "paused");
        assert_eq!(items[3].divider, Some("Subscriptions"));
        // Stop the background task: confirm, then stop_task for its id.
        model.handle(Action::Ops(ops::Act::Activity(Act::Stop("b1".into()))));
        assert_eq!(model.ui.opening.len(), 1, "a confirmation");
        model.ui.opening.clear();
        let sent = model.handle(Action::Ops(ops::Act::Activity(Act::StopConfirmed(
            "b1".into(),
        ))));
        assert_eq!(sent[0]["type"], "stop_task");
        assert_eq!(sent[0]["taskId"], "b1");
        assert_eq!(model.activity_items("c", now)[2].time, "stopping…");
        let id = sent[0]["reqId"].as_str().unwrap().to_owned();
        let again = model.apply(
            parse_event(&format!(
                r#"{{"type":"task_stopped","reqId":"{id}","convId":"c","taskId":"b1","status":"stopped"}}"#
            )),
            now,
        );
        assert_eq!(again[0]["type"], "list_tasks", "the list refreshes");
        // A subagent's row opens it.
        let built = build(&model, "c", now);
        assert!(built.rows.iter().any(|r| matches!(r, Row::Task { tap: Some(Tap::Close(Action::Nav(Nav::OpenConversation(id)))), .. } if id == "s1")));
    }
}
