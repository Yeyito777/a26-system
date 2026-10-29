//! What a list row says about its conversation or folder: the title as the
//! list shows it (first line, emoji mark apart), and activity with the
//! TUI's semantics (exocortex `tui/src/sidebar/activityindicator.ts`,
//! `render.ts`, as the iPhone's `state/sidebar-model.ts` ports them).
//!
//! A conversation's dot: accent while it streams, warning while it waits
//! (on its provider, in a long Chrono sleep, or in the global-idle queue),
//! success when unread with no model work and not muted. Its badges: ◆
//! subagents (and Agent/Workflow background tasks), $ other background
//! commands, ◷ Chrono tasks other than sleeps and waits. A folder adds up
//! everything below it.

use super::super::proto::{Summary, TaskSummary};

/// The TUI's emoji marks (`tui/src/marks.ts`, digit keys 1–9 there).
pub const MARKS: [(&str, &str); 9] = [
    ("🕐", "scheduled"),
    ("🔥", "urgent"),
    ("🧪", "experiment"),
    ("📝", "reference"),
    ("🐛", "bug"),
    ("💡", "idea"),
    ("🔒", "protected"),
    ("✅", "done"),
    ("📡", "watching"),
];

/// A Chrono sleep or wait longer than this suspends the turn (exocortex
/// `shared/src/chrono.ts`): the conversation is sleeping, not working.
pub const LONG_SLEEP_MS: f64 = 5.0 * 60.0 * 1000.0;

/// The emoji mark a title starts with ("🔥 Fix the build"), if any.
pub fn mark_prefix(title: &str) -> Option<&'static str> {
    MARKS.iter().map(|(emoji, _)| *emoji).find(|emoji| {
        title
            .strip_prefix(emoji)
            .is_some_and(|rest| rest.starts_with(' '))
    })
}

pub fn strip_mark(title: &str) -> &str {
    match mark_prefix(title) {
        Some(emoji) => &title[emoji.len() + 1..],
        None => title,
    }
}

pub fn first_line(title: &str) -> &str {
    title.split('\n').next().unwrap_or("")
}

/// What a row shows: the first line, its mark apart (drawn as its own
/// glyph); None when blank ("Untitled chat", drawn as a placeholder). A
/// title being generated reads "pending" (PAR-055).
pub fn row_title(summary: &Summary) -> Option<&str> {
    let title = strip_mark(first_line(&summary.title)).trim();
    (!title.is_empty()).then_some(title)
}

/// What search looks in: marks are stripped only from titles that do not
/// start with ASCII (the TUI's `getSearchFilterConversationTitle`).
pub fn search_title(summary: &Summary) -> &str {
    let title = first_line(&summary.title);
    if title.as_bytes().first().is_none_or(|byte| *byte < 0x80) {
        title
    } else {
        strip_mark(title)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Streaming,
    Warning,
    Unread,
}

/// One conversation's indicator and badges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Activity {
    pub kind: Option<Kind>,
    /// ◆ subagents (and Claude Code Agent/Workflow background tasks).
    pub agents: u32,
    /// $ other background commands.
    pub commands: u32,
    /// ◷ Chrono tasks other than sleeps and waits.
    pub chrono: u32,
    pub queued: u32,
    /// Notifications muted: its own, a folder above's, or the daemon's.
    pub muted: bool,
}

/// What a folder's conversations add up to (subfolders included).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderActivity {
    pub count: usize,
    /// Streaming conversations (not those sleeping inside their turn).
    pub streaming: usize,
    /// One is in a long sleep or waits in the global-idle queue.
    pub warning: bool,
    /// Unread conversations without model work (the folder's mute applies
    /// when shown).
    pub unread: usize,
    pub agents: u32,
    pub commands: u32,
    pub chrono: u32,
}

/// The queue entries of one conversation, as activity needs them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueInfo {
    pub count: u32,
    /// One waits in the global-idle queue.
    pub idle: bool,
}

fn work_counts(summary: &Summary) -> (u32, u32, u32) {
    let agent_tasks = summary
        .tasks
        .iter()
        .filter(|t| {
            t.kind == "background"
                && t.tool_name
                    .as_deref()
                    .is_some_and(|tool| tool == "Agent" || tool == "Workflow")
        })
        .count() as u32;
    let background = summary.background_task_count;
    // A sleep shows as the streaming or sleeping dot; a wait repeats a task
    // already shown.
    let chrono = summary
        .tasks
        .iter()
        .filter(|t| {
            t.kind == "chrono" && !matches!(t.chrono_mode.as_deref(), Some("sleep" | "wait"))
        })
        .count() as u32;
    (
        summary.subagent_count + agent_tasks.min(background),
        background.saturating_sub(agent_tasks),
        chrono,
    )
}

/// Whether a Chrono task is the conversation's own turn sleeping or waiting.
fn turn_sleep(task: &TaskSummary) -> bool {
    task.kind == "chrono"
        && match task.chrono_mode.as_deref() {
            Some("sleep") => true,
            Some("wait") => task.id.starts_with("chrono:wait:"),
            _ => false,
        }
}

/// (sleeping inside a streaming turn, sleeping long, model work).
fn sleep_state(summary: &Summary) -> (bool, bool, bool) {
    let inline = summary.streaming
        && summary.tasks.iter().any(|task| {
            turn_sleep(task)
                && task
                    .due_at
                    .is_some_and(|due| due - task.started_at > LONG_SLEEP_MS)
        });
    let durable = !summary.streaming && summary.tasks.iter().any(turn_sleep);
    (inline, inline || durable, summary.streaming || durable)
}

/// One conversation's activity. `local` is a turn the app knows runs before
/// the daemon's summary says so; `folder_muted` a muted folder above it.
pub fn conversation(
    summary: &Summary,
    queue: QueueInfo,
    local: bool,
    folder_muted: bool,
) -> Activity {
    let (inline, long, model_work) = sleep_state(summary);
    let muted = summary.muted || folder_muted || summary.notifications_muted;
    let model_work = model_work || local;
    let streaming = (summary.streaming || local) && !inline && !summary.provider_waiting;
    let warning = long || queue.idle || summary.provider_waiting;
    let unread = !muted && summary.unread && !model_work;
    let (agents, commands, chrono) = work_counts(summary);
    Activity {
        kind: if streaming {
            Some(Kind::Streaming)
        } else if warning {
            Some(Kind::Warning)
        } else if unread {
            Some(Kind::Unread)
        } else {
            None
        },
        agents,
        commands,
        chrono,
        queued: queue.count,
        muted,
    }
}

impl FolderActivity {
    /// Adds a conversation below the folder.
    pub fn add(&mut self, summary: &Summary, queue: QueueInfo, local: bool) {
        let (inline, long, model_work) = sleep_state(summary);
        let (agents, commands, chrono) = work_counts(summary);
        self.count += 1;
        if (summary.streaming || local) && !inline {
            self.streaming += 1;
        }
        self.warning |= long || queue.idle;
        if summary.unread && !(model_work || local) {
            self.unread += 1;
        }
        self.agents += agents;
        self.commands += commands;
        self.chrono += chrono;
    }

    /// The folder's one indicator: accent with a count, else warning, else
    /// unread unless muted.
    pub fn indicator(&self, muted: bool) -> Option<(Kind, usize)> {
        if self.streaming > 0 {
            Some((Kind::Streaming, self.streaming))
        } else if self.warning {
            Some((Kind::Warning, 0))
        } else if self.unread > 0 && !muted {
            Some((Kind::Unread, 0))
        } else {
            None
        }
    }
}

/// "now", "5 min. ago", "2 hr. ago", "3 days ago", "1 wk. ago", "2 mo.
/// ago", "1 yr. ago" (`Intl.RelativeTimeFormat` short, numeric, as the
/// iPhone's `formatAgo`); "in 5 min." ahead.
pub fn ago(then_ms: f64, now_ms: f64) -> String {
    let diff = now_ms - then_ms;
    if diff.abs() < 60_000.0 {
        return "now".into();
    }
    const DAY: f64 = 86_400_000.0;
    let units: [(f64, &str, &str); 6] = [
        (365.0 * DAY, "yr.", "yr."),
        (30.0 * DAY, "mo.", "mo."),
        (7.0 * DAY, "wk.", "wk."),
        (DAY, "day", "days"),
        (3_600_000.0, "hr.", "hr."),
        (60_000.0, "min.", "min."),
    ];
    let abs = diff.abs();
    for (ms, one, many) in units {
        if abs >= ms {
            let n = (abs / ms).floor() as u64;
            let unit = if n == 1 { one } else { many };
            return if diff > 0.0 {
                format!("{n} {unit} ago")
            } else {
                format!("in {n} {unit}")
            };
        }
    }
    "now".into()
}

/// 1234567 → "1,234,567".
pub fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(extra: &str) -> Summary {
        let mut value = serde_json::json!({ "id": "a", "title": "t" });
        let extra: serde_json::Value = serde_json::from_str(extra).unwrap();
        for (key, field) in extra.as_object().unwrap() {
            value[key] = field.clone();
        }
        serde_json::from_value(value).unwrap()
    }

    fn task(id: &str, kind: &str, extra: &str) -> String {
        format!(r#"{{"id":"{id}","kind":"{kind}","title":"t","startedAt":1000{extra}}}"#)
    }

    const NONE: QueueInfo = QueueInfo {
        count: 0,
        idle: false,
    };

    fn kind(extra: &str) -> Option<Kind> {
        conversation(&summary(extra), NONE, false, false).kind
    }

    // The iPhone's test/sidebar.test.ts, "activity" (the TUI's semantics).
    #[test]
    fn streaming_beats_warning_beats_unread_and_mute_hides_unread() {
        assert_eq!(kind(r#"{"streaming":true}"#), Some(Kind::Streaming));
        assert_eq!(
            kind(r#"{"streaming":true,"providerWaiting":true}"#),
            Some(Kind::Warning)
        );
        assert_eq!(kind(r#"{"unread":true}"#), Some(Kind::Unread));
        assert_eq!(kind(r#"{"unread":true,"notificationsMuted":true}"#), None);
        assert_eq!(kind(r#"{"unread":true,"muted":true}"#), None);
        let unread = summary(r#"{"unread":true}"#);
        assert_eq!(conversation(&unread, NONE, false, true).kind, None);
        let idle = QueueInfo {
            count: 1,
            idle: true,
        };
        let queued = conversation(&unread, idle, false, false);
        assert_eq!((queued.kind, queued.queued), (Some(Kind::Warning), 1));
        assert_eq!(
            conversation(&summary("{}"), NONE, true, false).kind,
            Some(Kind::Streaming)
        );
        // Unread hides while there is model work (a local send too).
        assert_eq!(
            conversation(&unread, NONE, true, false).kind,
            Some(Kind::Streaming)
        );
    }

    #[test]
    fn only_a_long_sleep_inside_a_streaming_turn_is_a_warning() {
        let long = task(
            "chrono:sleep:1",
            "chrono",
            r#","chronoMode":"sleep","dueAt":361000"#,
        );
        let short = task(
            "chrono:sleep:2",
            "chrono",
            r#","chronoMode":"sleep","dueAt":61000"#,
        );
        assert_eq!(
            kind(&format!(r#"{{"streaming":true,"tasks":[{long}]}}"#)),
            Some(Kind::Warning)
        );
        assert_eq!(
            kind(&format!(r#"{{"streaming":true,"tasks":[{short}]}}"#)),
            Some(Kind::Streaming)
        );
        // A durable sleep (not streaming) is model work: a warning, and
        // unread stays hidden.
        assert_eq!(
            kind(&format!(r#"{{"unread":true,"tasks":[{short}]}}"#)),
            Some(Kind::Warning)
        );
        // A wait is the turn's own only with a chrono:wait: id.
        let other = task("other", "chrono", r#","chronoMode":"wait""#);
        assert_eq!(
            kind(&format!(r#"{{"unread":true,"tasks":[{other}]}}"#)),
            Some(Kind::Unread)
        );
        let own = task("chrono:wait:9", "chrono", r#","chronoMode":"wait""#);
        assert_eq!(
            kind(&format!(r#"{{"tasks":[{own}]}}"#)),
            Some(Kind::Warning)
        );
    }

    #[test]
    fn badges_count_agents_commands_and_chrono() {
        let tasks = [
            task("1", "chrono", r#","chronoMode":"sleep""#),
            task("2", "chrono", r#","chronoMode":"wake""#),
            task("w", "chrono", r#","chronoMode":"wait""#),
            task("3", "background", r#","toolName":"Agent""#),
            task("4", "background", r#","toolName":"Workflow""#),
        ];
        let summary = summary(&format!(
            r#"{{"tasks":[{}],"subagentCount":2,"backgroundTaskCount":3}}"#,
            tasks.join(",")
        ));
        let activity = conversation(&summary, NONE, false, false);
        assert_eq!(
            (activity.agents, activity.commands, activity.chrono),
            (4, 1, 1)
        );
    }

    #[test]
    fn folders_add_up_and_show_one_indicator() {
        let mut folder = FolderActivity::default();
        folder.add(
            &summary(r#"{"streaming":true,"providerWaiting":true}"#),
            NONE,
            false,
        );
        folder.add(&summary(r#"{"unread":true}"#), NONE, false);
        // providerWaiting is not added up as a warning.
        assert!(!folder.warning);
        assert_eq!(folder.indicator(true), Some((Kind::Streaming, 1)));
        let quiet = FolderActivity {
            streaming: 0,
            ..folder
        };
        assert_eq!(quiet.indicator(true), None);
        assert_eq!(quiet.indicator(false), Some((Kind::Unread, 0)));
        assert_eq!(folder.count, 2);
    }

    #[test]
    fn titles_lose_their_first_line_rest_and_marks() {
        assert_eq!(
            row_title(&summary(r#"{"title":"🔥 Fix the build\nmore"}"#)),
            Some("Fix the build")
        );
        assert_eq!(row_title(&summary(r#"{"title":"  "}"#)), None);
        assert_eq!(
            row_title(&summary(r#"{"title":"pending"}"#)),
            Some("pending")
        );
        assert_eq!(mark_prefix("🐛 crash"), Some("🐛"));
        assert_eq!(mark_prefix("🐛crash"), None);
        // Search keeps marks on ASCII-led titles only.
        assert_eq!(search_title(&summary(r#"{"title":"🐛 crash"}"#)), "crash");
        assert_eq!(
            search_title(&summary(r#"{"title":"a 🐛 crash"}"#)),
            "a 🐛 crash"
        );
    }

    #[test]
    fn relative_times_read_as_on_the_iphone() {
        let now = 1_000_000_000_000.0;
        let ago_s = |s: f64| ago(now - s * 1000.0, now);
        assert_eq!(ago_s(30.0), "now");
        assert_eq!(ago_s(60.0 * 24.0), "24 min. ago");
        assert_eq!(ago_s(3600.0 * 2.5), "2 hr. ago");
        assert_eq!(ago_s(86_400.0), "1 day ago");
        assert_eq!(ago_s(86_400.0 * 3.0), "3 days ago");
        assert_eq!(ago_s(86_400.0 * 8.0), "1 wk. ago");
        assert_eq!(ago_s(86_400.0 * 65.0), "2 mo. ago");
        assert_eq!(ago_s(86_400.0 * 400.0), "1 yr. ago");
        assert_eq!(ago_s(-300.0), "in 5 min.");
        assert_eq!(thousands(15_067), "15,067");
        assert_eq!(thousands(999), "999");
    }
}
