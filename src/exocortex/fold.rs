//! Folded turns and notifications (WP-C2, PAR-402–407): the TUI's
//! `turncollapse.ts` as the iPhone ports it (`ui/chat/fold.ts`), in its
//! default gutter look.
//!
//! Folded history reads as a plain chat: the owner's messages and each
//! final reply with its footer. Thinking, tool calls, interim text,
//! informational notices and notifications leave no row; a thin line at the
//! left edge of the next reply (or a short one in the gap when nothing
//! answered) marks folded work, and a tap on it opens that stretch. The live
//! turn shows only its newest step under a WORKING row that counts the rest.
//! Show all work opens everything.
//!
//! Pure: messages in, a plan out (by message index); `is_open` reads the
//! disclosures.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::proto::Block;

/// A message as folding sees it.
#[derive(Debug, Clone, Copy)]
pub enum Msg<'a> {
    User {
        text: &'a str,
        /// A daemon notice sent as a user message.
        system: bool,
        automation: Option<&'a Value>,
        queue_entry_id: Option<&'a str>,
        started_at: f64,
    },
    Assistant {
        blocks: &'a [Block],
    },
    System {
        /// An informational notice (it folds with its turn's work).
        informational: bool,
    },
    Other,
}

/// What an assistant message shows of its blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantFold {
    /// Block indices folded away; None shows every block.
    pub hidden: Option<HashSet<usize>>,
    /// The turn's work row ("5 tool calls, 1 message"), or None.
    pub summary: Option<String>,
    /// The turn's disclosure key.
    pub turn: String,
    pub open: bool,
    /// A folded turn with nothing to read drops its footer.
    pub meta_hidden: bool,
}

/// The live turn's plan.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveFold {
    pub hidden: Option<HashSet<usize>>,
    pub summary: Option<String>,
    pub turn: String,
    pub open: bool,
    /// The newest step when it is already committed (message index, block):
    /// it shows under the working row.
    pub revealed: Option<(usize, usize)>,
}

/// A notification (or notice) folded into a turn's work.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedFold {
    pub turn: String,
    pub open: bool,
    pub lead: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Out of sight (unless the group is open).
    Hidden,
    /// The reply that carries the group's mark.
    Host,
    /// No reply to carry it: a short mark in the gap.
    Line,
}

/// A message's place in a fold group (gutter look).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GutterEntry {
    pub group: String,
    pub open: bool,
    pub role: Role,
    /// Draw the mark (Settings → Fold markers).
    pub mark: bool,
}

#[derive(Debug, Default)]
pub struct FoldPlan {
    pub by_msg: HashMap<usize, AssistantFold>,
    pub live: Option<LiveFold>,
    pub gutter: HashMap<usize, GutterEntry>,
    pub merged: HashMap<usize, MergedFold>,
}

impl FoldPlan {
    /// Whether message `at` is out of sight (a closed group's member, or a
    /// notification folded into a closed turn); a group's line may still
    /// mark its place (`line`).
    pub fn hides(&self, at: usize) -> bool {
        self.gutter
            .get(&at)
            .is_some_and(|g| !g.open && g.role != Role::Host)
            || self.merged.get(&at).is_some_and(|m| !m.open)
    }

    /// The closed group whose short line stands where message `at` was.
    pub fn line(&self, at: usize) -> Option<&GutterEntry> {
        self.gutter
            .get(&at)
            .filter(|g| !g.open && g.role == Role::Line && g.mark)
    }

    /// The group whose mark message `at` carries down its left edge: a
    /// closed group's reply, every member of an open one.
    pub fn mark(&self, at: usize) -> Option<&GutterEntry> {
        self.gutter
            .get(&at)
            .filter(|g| g.mark && (g.open || g.role == Role::Host))
    }
}

pub const AUTOMATION_LABELS: [(&str, &str); 8] = [
    ("external_notification", "Notification"),
    ("background_task_completion", "Background completion"),
    ("chrono_wake", "Chrono"),
    ("chrono_hard_wake", "Chrono hard wake"),
    ("exo_send", "Exo delegation"),
    ("subagent_completion", "Subagent completion"),
    ("goal_continuation", "Goal continuation"),
    ("daemon_restart", "Daemon restart"),
];

fn automation_kind(automation: Option<&Value>) -> Option<&str> {
    automation?.get("kind")?.as_str()
}

fn automation_source(automation: Option<&Value>) -> Option<&str> {
    automation?.get("sourceId")?.as_str()
}

fn label_of(kind: &str) -> &'static str {
    AUTOMATION_LABELS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("Notification", |(_, label)| label)
}

/// A prompt an older daemon wrote without `automation` (`[notification]
/// …`, `[chrono wake: …]`, `[Discord Notification] …`).
pub fn legacy_automated(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('[') else {
        return false;
    };
    if rest.starts_with("notification]")
        || rest.starts_with("chrono wake:")
        || rest.starts_with("chrono wake]")
        || rest.starts_with("chrono hard wake:")
        || rest.starts_with("chrono hard wake]")
    {
        return true;
    }
    if let Some(after) = rest.strip_prefix("external notification") {
        return !after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
    }
    rest.find("Notification]").is_some_and(|end| {
        rest[..end]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '/' || c == ' ')
    })
}

/// `isAutomatedUserPrompt`.
pub fn automated(text: &str, automation: Option<&Value>) -> bool {
    automation.is_some() || legacy_automated(text)
}

/// The owner's own words in a notification relaying their message (their
/// Discord DM, a WhatsApp /ai command: the `→ Name <id> [owner] …:` line),
/// as they read in a bubble; None for anything else (`ownerRelayedText`).
pub fn owner_relayed_text(text: &str, automation: Option<&Value>) -> Option<String> {
    match automation {
        Some(_) if automation_kind(automation) != Some("external_notification") => return None,
        None if !legacy_automated(text) => return None,
        _ => {}
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let at = lines.iter().position(|line| line.starts_with("→ "))?;
    let trigger = lines[at];
    // The header's colon is the first one outside bracketed tags, after an
    // `[owner]` tag.
    let mut depth = 0;
    let mut owner = false;
    let mut colon = None;
    let chars: Vec<(usize, char)> = trigger.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (byte, c) = chars[i];
        match c {
            '[' => {
                if depth == 0 && trigger[byte..].starts_with("[owner]") {
                    owner = true;
                }
                depth += 1;
            }
            ']' => depth = (depth - 1).max(0),
            ':' if depth == 0 => {
                colon = Some(byte);
                break;
            }
            _ => {}
        }
        i += 1;
    }
    let colon = colon?;
    if !owner {
        return None;
    }
    let mut words = vec![trigger[colon + 1..].trim_start_matches([' ', '\t']).to_owned()];
    for (index, line) in lines[at + 1..].iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("[msg:") && trimmed.ends_with(']') {
            break;
        }
        let next = lines.get(at + 2 + index).copied().unwrap_or("");
        if trimmed.is_empty()
            && (next.starts_with("Action:") || next.starts_with("Filter output:"))
        {
            break;
        }
        words.push((*line).to_owned());
    }
    readable_relay_body(&words.join("\n"))
}

/// Attachment lines as `📎 name` / `🎤 voice message`; a bare `[message]`
/// as `📎 attachment` (`readableRelayBody`).
fn readable_relay_body(raw: &str) -> Option<String> {
    let mut attachments = 0;
    let mut attachment = |name: &str| {
        attachments += 1;
        let file = name.trim();
        let lower = file.to_ascii_lowercase();
        let audio = lower.starts_with("voice-message")
            || [".ogg", ".oga", ".opus", ".m4a", ".mp3", ".wav", ".aac"]
                .iter()
                .any(|ext| lower.ends_with(ext));
        if audio {
            "🎤 voice message".to_owned()
        } else if file.is_empty() {
            "📎 attachment".to_owned()
        } else {
            format!("📎 {file}")
        }
    };
    let mut lines: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let trimmed = line.trim_start();
        let mut out = if let Some(rest) = trimmed.strip_prefix("[attachment:")
            && let Some(end) = rest.find(']')
        {
            attachment(&rest[end + 1..])
        } else {
            line.to_owned()
        };
        while let Some(start) = out.find("[📎 ") {
            let Some(end) = out[start..].find(']') else {
                break;
            };
            let name = out[start + "[📎 ".len()..start + end].to_owned();
            let replaced = attachment(&name);
            out.replace_range(start..start + end + 1, &replaced);
        }
        lines.push(out);
    }
    let kept: Vec<&String> = lines
        .iter()
        .filter(|line| !(line.trim() == "[message]" && attachments > 0))
        .collect();
    let body = kept
        .iter()
        .map(|line| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim();
    if body == "[message]" {
        return Some("📎 attachment".into());
    }
    (!body.is_empty()).then(|| body.to_owned())
}

/// The first meaningful words of a notification, on one line: the
/// envelope, routing ids (`[ch:…]`, `<@…>`, `[reply-to:…]`) and the
/// agent-facing "Action:" tail stripped.
pub fn notification_preview(text: &str) -> String {
    let mut text = text.to_owned();
    for tail in ["\n\nAction:", "\n\nFilter output:"] {
        if let Some(at) = text.find(tail) {
            text.truncate(at);
        }
    }
    let mut text = text.trim_start();
    if let Some(rest) = text.strip_prefix("[notification]") {
        text = rest;
    } else if let Some(rest) = text
        .strip_prefix("[external notification")
        .or_else(|| text.strip_prefix("[chrono wake:"))
        .or_else(|| text.strip_prefix("[chrono hard wake:"))
        && let Some((_, after)) = rest.split_once(']')
    {
        text = after;
    }
    // Routing noise, with the whitespace (and an arrow) before it.
    let mut cleaned = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let noise = noise_len(rest);
        if noise > 0 {
            // Drop the whitespace (and `↳`) already copied before it.
            loop {
                let trimmed = cleaned.trim_end();
                let trimmed = trimmed.strip_suffix('↳').unwrap_or(trimmed);
                if trimmed.len() == cleaned.len() {
                    break;
                }
                cleaned.truncate(trimmed.len());
            }
            rest = &rest[noise..];
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        cleaned.push(c);
        rest = &rest[c.len_utf8()..];
    }
    let mut out = String::new();
    for word in cleaned.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() > 300 {
            break;
        }
    }
    out
}

/// How long the routing tag at the start of `s` is (0: none).
fn noise_len(s: &str) -> usize {
    for tag in [
        "[ch:",
        "[dm:",
        "[msg:",
        "[thread:",
        "[chat:",
        "[guild:",
        "[reply-to:",
    ] {
        if s.starts_with(tag)
            && let Some(end) = s.find(']')
            && !s[..end].contains('\n')
        {
            return end + 1;
        }
    }
    if let Some(rest) = s.strip_prefix("<@") {
        let rest = rest.strip_prefix('!').unwrap_or(rest);
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with('>') {
            return s.len() - rest.len() + digits + 1;
        }
    }
    0
}

/// The label of a user-role message the user did not type, or None for
/// their own words (`notificationLabel`). In a top-level conversation its
/// own subagents' messages (`own_subagent`) are notifications too.
pub fn notification_label(
    text: &str,
    automation: Option<&Value>,
    own_subagent: &dyn Fn(&str) -> bool,
) -> Option<&'static str> {
    if !automated(text, automation) || owner_relayed_text(text, automation).is_some() {
        return None;
    }
    let kind = automation_kind(automation);
    if kind == Some("exo_send") {
        return automation_source(automation)
            .filter(|id| own_subagent(id))
            .map(|_| label_of("exo_send"));
    }
    Some(kind.map_or("Notification", label_of))
}

/// The line under a notification's or another conversation's bubble:
/// `⚙ <label> · from <source>` (`notificationBubble`, `userBubble`).
pub fn bubble_label(
    text: &str,
    automation: Option<&Value>,
    label: &str,
    title: &dyn Fn(&str) -> String,
) -> String {
    let source = match automation_kind(automation) {
        Some("exo_send") => Some(
            automation_source(automation)
                .map_or_else(|| "another conversation".to_owned(), title),
        ),
        Some("subagent_completion") => automation_source(automation).map(title),
        Some("external_notification") => text
            .strip_prefix("[notification]")
            .and_then(|rest| rest.split('\n').next())
            .map(|line| notification_preview(&format!("[notification]{line}")))
            .filter(|line| !line.is_empty()),
        _ => None,
    };
    match source {
        Some(source) => format!("{label} · from {source}"),
        None => label.to_owned(),
    }
}

fn has_text(block: &Block) -> bool {
    matches!(block, Block::Text { text } if !text.trim().is_empty())
}

fn is_tool(block: &Block) -> bool {
    matches!(block, Block::ToolCall { .. } | Block::ToolResult { .. })
}

/// "5 tool calls, 1 message".
pub fn work_summary(tool_calls: usize, texts: usize) -> Option<String> {
    let mut parts = Vec::new();
    if tool_calls > 0 {
        let s = if tool_calls == 1 { "" } else { "s" };
        parts.push(format!("{tool_calls} tool call{s}"));
    }
    if texts > 0 {
        let s = if texts == 1 { "" } else { "s" };
        parts.push(format!("{texts} message{s}"));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The step a live turn is on: streaming text, else its newest tool call.
fn latest_step(blocks: &[Block]) -> Option<usize> {
    let last = blocks.len().checked_sub(1)?;
    if has_text(&blocks[last]) {
        return Some(last);
    }
    (0..=last)
        .rev()
        .find(|&i| matches!(blocks[i], Block::ToolCall { .. }))
}

/// `Subagent completion`, `Chrono ×2`, or `3 notifications`.
fn combine_labels(labels: &[&str]) -> Option<String> {
    let first = labels.first()?;
    if labels.iter().all(|label| label == first) {
        return Some(if labels.len() == 1 {
            (*first).to_owned()
        } else {
            format!("{first} ×{}", labels.len())
        });
    }
    Some(format!("{} notifications", labels.len()))
}

/// What a message is for folding.
struct View<'a> {
    msgs: &'a [Msg<'a>],
    keys: &'a [String],
    own_subagent: &'a dyn Fn(&str) -> bool,
}

impl View<'_> {
    fn is_user(&self, at: usize) -> bool {
        matches!(self.msgs[at], Msg::User { .. })
    }

    /// A notification's label (a user message that is not a daemon notice).
    fn note(&self, at: usize) -> Option<&'static str> {
        match self.msgs[at] {
            Msg::User {
                text,
                system: false,
                automation,
                ..
            } => notification_label(text, automation, self.own_subagent),
            _ => None,
        }
    }

    fn is_notice(&self, at: usize) -> bool {
        matches!(
            self.msgs[at],
            Msg::System {
                informational: true
            }
        )
    }

    fn blocks(&self, at: usize) -> Option<&[Block]> {
        match self.msgs[at] {
            Msg::Assistant { blocks } => Some(blocks),
            _ => None,
        }
    }

    /// A message delivered into the running turn (queued, stamped after the
    /// run started).
    fn delivered_mid_run(&self, at: usize, run_started_at: Option<f64>) -> bool {
        match (self.msgs[at], run_started_at) {
            (
                Msg::User {
                    queue_entry_id: Some(_),
                    started_at,
                    ..
                },
                Some(run),
            ) => started_at > run,
            _ => false,
        }
    }

    /// Where the running turn's work starts, reaching back past messages
    /// delivered into it.
    fn live_run_start(&self, live_start: usize, run_started_at: Option<f64>) -> usize {
        let mut at = live_start;
        while at > 0 && at < self.msgs.len() && self.delivered_mid_run(at, run_started_at) {
            at -= 1;
            while at > 0 && !self.is_user(at) {
                at -= 1;
            }
        }
        at
    }

    fn live_turn_start(&self) -> usize {
        (1..self.msgs.len())
            .rev()
            .find(|&i| self.is_user(i))
            .unwrap_or(0)
    }
}

/// Plans every assistant message of `msgs` (keyed `keys`, for disclosure
/// keys) and the live tail (`live`, when a turn streams).
pub fn plan(
    msgs: &[Msg],
    keys: &[String],
    live: Option<&[Block]>,
    is_open: &dyn Fn(&str) -> bool,
    marks: bool,
    run_started_at: Option<f64>,
    own_subagent: &dyn Fn(&str) -> bool,
) -> FoldPlan {
    debug_assert_eq!(msgs.len(), keys.len());
    let view = View {
        msgs,
        keys,
        own_subagent,
    };
    let mut out = FoldPlan::default();
    let mut start = 0;
    let mut orphan_start: Option<usize> = None;
    let mut orphans: Vec<&str> = Vec::new();
    let mut hidden_by: HashMap<usize, HashSet<usize>> = HashMap::new();
    let n = msgs.len();
    for i in 0..=n {
        if i < n && (i == 0 || !view.is_user(i)) {
            continue;
        }
        let in_progress = i == n && live.is_some();
        let has_head = start < n;
        let head_label = if has_head { view.note(start) } else { None };
        if !in_progress && i < n && i - start == 1 && head_label.is_some() {
            orphan_start.get_or_insert(start);
            orphans.push(head_label.unwrap_or("Notification"));
            start = i;
            continue;
        }
        if has_head || in_progress {
            let turn = if has_head {
                format!("turn:{}", keys[start])
            } else {
                "turn:live".to_owned()
            };
            let open = is_open(&turn);
            let assistants: Vec<usize> = (start..i).filter(|&j| view.blocks(j).is_some()).collect();
            let live_latest = live.and_then(latest_step).filter(|_| in_progress);
            let run_from = if in_progress {
                view.live_run_start(start, run_started_at)
            } else {
                start
            };
            let run_before: Vec<usize> = (run_from..start)
                .filter(|&j| view.blocks(j).is_some())
                .collect();
            let mut reveal: Option<(usize, usize)> = None;
            if in_progress && live_latest.is_none() {
                for &j in assistants.iter().rev() {
                    if let Some(b) = latest_step(view.blocks(j).unwrap_or_default()) {
                        reveal = Some((j, b));
                        break;
                    }
                }
                if reveal.is_none() {
                    for &j in run_before.iter().rev() {
                        let blocks = view.blocks(j).unwrap_or_default();
                        let Some(b) = latest_step(blocks) else {
                            continue;
                        };
                        if matches!(blocks[b], Block::ToolCall { .. })
                            && !out.by_msg.get(&j).is_some_and(|f| f.open)
                        {
                            reveal = Some((j, b));
                        }
                        break;
                    }
                }
            }
            // The final reply is the text after the last tool block.
            let mut last_tool: Option<(usize, usize)> = None;
            for &j in assistants.iter().rev() {
                let blocks = view.blocks(j).unwrap_or_default();
                if let Some(b) = (0..blocks.len()).rev().find(|&b| is_tool(&blocks[b])) {
                    last_tool = Some((j, b));
                    break;
                }
            }
            let last_tool_at = last_tool.and_then(|(msg, _)| assistants.iter().position(|&a| a == msg));
            let (mut tool_calls, mut texts) = (0, 0);
            let mut host: Option<usize> = None;
            let mut hidden_of: HashMap<usize, HashSet<usize>> = HashMap::new();
            for (at, &j) in assistants.iter().enumerate() {
                let blocks = view.blocks(j).unwrap_or_default();
                let mut hidden = HashSet::new();
                for (bi, block) in blocks.iter().enumerate() {
                    let is_reveal = reveal == Some((j, bi));
                    let work = in_progress
                        || last_tool_at.is_some_and(|lt| {
                            at < lt || (at == lt && bi <= last_tool.map_or(0, |(_, b)| b))
                        })
                        || matches!(block, Block::Thinking { .. });
                    if !work {
                        continue;
                    }
                    hidden.insert(bi);
                    if is_reveal {
                        continue;
                    }
                    if matches!(block, Block::ToolCall { .. }) {
                        tool_calls += 1;
                    } else if has_text(block) {
                        texts += 1;
                    }
                }
                if host.is_none() && !hidden.is_empty() {
                    host = Some(j);
                }
                hidden_by.insert(j, hidden.clone());
                hidden_of.insert(j, hidden);
            }
            // The whole run's folded work counts.
            for &j in &run_before {
                let blocks = view.blocks(j).unwrap_or_default();
                for (bi, block) in blocks.iter().enumerate() {
                    if !hidden_by.get(&j).is_some_and(|h| h.contains(&bi)) || reveal == Some((j, bi)) {
                        continue;
                    }
                    if matches!(block, Block::ToolCall { .. }) {
                        tool_calls += 1;
                    } else if has_text(block) {
                        texts += 1;
                    }
                }
                if let Some(earlier) = out.by_msg.get_mut(&j)
                    && earlier.summary.is_some()
                    && !earlier.open
                {
                    earlier.summary = None;
                }
            }
            let mut live_hidden: Option<HashSet<usize>> = None;
            if in_progress {
                let mut set = HashSet::new();
                for (bi, block) in live.unwrap_or_default().iter().enumerate() {
                    if Some(bi) == live_latest {
                        continue;
                    }
                    set.insert(bi);
                    if matches!(block, Block::ToolCall { .. }) {
                        tool_calls += 1;
                    } else if has_text(block) {
                        texts += 1;
                    }
                }
                live_hidden = Some(set);
            }
            let answered = !assistants.is_empty() || in_progress;
            let own_merged = head_label.is_some() && answered;
            let absorbed = !orphans.is_empty() && answered;
            let mut labels: Vec<&str> = Vec::new();
            if absorbed {
                labels.extend(orphans.iter().copied());
            }
            if own_merged {
                labels.extend(head_label);
            }
            let row_label = combine_labels(&labels);
            if row_label.is_some() && host.is_none() {
                host = assistants.first().copied();
            }
            let reply = assistants.iter().any(|&j| {
                view.blocks(j)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .any(|(bi, b)| has_text(b) && !hidden_of[&j].contains(&bi))
            });
            let summary = [row_label.clone(), work_summary(tool_calls, texts)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            let summary = (!summary.is_empty()).then_some(summary);
            let lead = (open && !in_progress && row_label.is_some())
                .then(|| summary.clone())
                .flatten();
            let lead_at = if absorbed {
                orphan_start.unwrap_or(start)
            } else {
                start
            };
            if own_merged {
                out.merged.insert(
                    start,
                    MergedFold {
                        turn: turn.clone(),
                        open,
                        lead: lead.clone().filter(|_| lead_at == start),
                    },
                );
            }
            for j in start..i {
                if view.is_notice(j) {
                    out.merged.insert(
                        j,
                        MergedFold {
                            turn: turn.clone(),
                            open,
                            lead: None,
                        },
                    );
                }
            }
            if absorbed && let Some(from) = orphan_start {
                for j in from..start {
                    out.merged.insert(
                        j,
                        MergedFold {
                            turn: turn.clone(),
                            open,
                            lead: lead.clone().filter(|_| j == lead_at),
                        },
                    );
                }
            }
            for &j in &assistants {
                out.by_msg.insert(
                    j,
                    AssistantFold {
                        hidden: if open {
                            None
                        } else {
                            hidden_of.remove(&j)
                        },
                        summary: (!in_progress && lead.is_none() && Some(j) == host)
                            .then(|| summary.clone())
                            .flatten(),
                        turn: turn.clone(),
                        open,
                        meta_hidden: !open && !in_progress && !reply,
                    },
                );
            }
            if in_progress {
                out.live = Some(LiveFold {
                    hidden: if open { None } else { live_hidden },
                    summary,
                    turn,
                    open,
                    revealed: if open { None } else { reveal },
                });
            }
        }
        orphan_start = None;
        orphans.clear();
        start = i;
    }
    let live_from = if live.is_none() {
        n
    } else {
        view.live_turn_start()
    };
    plan_gutter(&view, live_from, &mut out, is_open, marks);
    out
}

/// A stretch between prompts, as the gutter sees it.
#[derive(Clone, Copy)]
struct Stretch {
    from: usize,
    to: usize,
    /// Starts with the owner's prompt, a notification, or neither.
    head: Head,
    reply: bool,
    folded: bool,
    host: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Head {
    Owner,
    Note,
    None,
}

fn plan_gutter(
    view: &View,
    live_from: usize,
    out: &mut FoldPlan,
    is_open: &dyn Fn(&str) -> bool,
    marks: bool,
) {
    let segment = |out: &FoldPlan, from: usize, to: usize| -> Stretch {
        let head = if !view.is_user(from) {
            Head::None
        } else if view.note(from).is_some() {
            Head::Note
        } else {
            Head::Owner
        };
        let mut reply = false;
        let mut folded = head == Head::Note;
        let mut host = None;
        for j in from..to {
            if view.is_notice(j) {
                folded = true;
            }
            let Some(blocks) = view.blocks(j) else {
                continue;
            };
            let hidden = out.by_msg.get(&j).and_then(|f| f.hidden.as_ref());
            let mut shows = false;
            for (bi, block) in blocks.iter().enumerate() {
                // Attached images stay in view under their message.
                if matches!(block, Block::ToolResult { images, .. } if !images.is_empty()) {
                    shows = true;
                }
                if hidden.is_some_and(|h| h.contains(&bi)) {
                    if matches!(block, Block::ToolCall { .. }) || has_text(block) {
                        folded = true;
                    }
                } else if has_text(block) {
                    shows = true;
                }
            }
            if shows {
                reply = true;
                host.get_or_insert(j);
            }
        }
        Stretch {
            from,
            to,
            head,
            reply,
            folded,
            host,
        }
    };
    let members = |out: &mut FoldPlan, list: &[Stretch], group: &str, open: bool, keep: Option<usize>| {
        for (index, seg) in list.iter().enumerate() {
            for j in seg.from..seg.to {
                let is_note = view.is_user(j) && view.note(j).is_some();
                if is_note || view.is_notice(j) {
                    if open {
                        out.merged.insert(
                            j,
                            MergedFold {
                                turn: group.to_owned(),
                                open: true,
                                lead: None,
                            },
                        );
                    }
                    out.gutter.insert(
                        j,
                        GutterEntry {
                            group: group.to_owned(),
                            open,
                            role: Role::Hidden,
                            mark: marks,
                        },
                    );
                } else if view.blocks(j).is_some() {
                    let Some(fold) = out.by_msg.get_mut(&j) else {
                        continue;
                    };
                    if open {
                        fold.hidden = None;
                        fold.summary = None;
                        fold.meta_hidden = false;
                    } else if Some(index) == keep {
                        fold.summary = None;
                    }
                    if open || Some(index) != keep {
                        out.gutter.insert(
                            j,
                            GutterEntry {
                                group: group.to_owned(),
                                open,
                                role: Role::Hidden,
                                mark: marks,
                            },
                        );
                    }
                }
            }
        }
    };
    let anchored = |out: &mut FoldPlan, list: &[Stretch]| {
        if !list.iter().any(|seg| seg.folded) {
            for seg in list {
                for j in seg.from..seg.to {
                    if let Some(fold) = out.by_msg.get_mut(&j) {
                        fold.summary = None;
                    }
                }
            }
            return;
        }
        let Some(reply) = list.last() else {
            return;
        };
        let group = format!("group:{}", view.keys[list[0].from]);
        let open = is_open(&group);
        members(out, list, &group, open, Some(list.len() - 1));
        if let Some(host) = reply.host {
            out.gutter.insert(
                host,
                GutterEntry {
                    group,
                    open,
                    role: Role::Host,
                    mark: marks,
                },
            );
        }
    };
    let unanchored = |out: &mut FoldPlan, list: &[Stretch]| {
        if !list.iter().any(|seg| seg.folded) {
            return;
        }
        let group = format!("group:{}", view.keys[list[0].from]);
        let open = is_open(&group);
        members(out, list, &group, open, None);
        let first = list[0];
        // A prompt that only got tool calls keeps its bubble: the mark goes
        // on its first answer.
        let mut at = first.from;
        if first.head == Head::Owner
            && let Some(j) = (first.from + 1..first.to).find(|&j| view.blocks(j).is_some())
        {
            at = j;
        }
        if marks || open {
            out.gutter.insert(
                at,
                GutterEntry {
                    group,
                    open,
                    role: Role::Line,
                    mark: marks,
                },
            );
        }
    };
    let mut stretch: Vec<Stretch> = Vec::new();
    let mut from = 0;
    for i in 1..=live_from {
        if i < live_from && !view.is_user(i) {
            continue;
        }
        if from >= i {
            continue;
        }
        let seg = segment(out, from, i);
        from = i;
        if seg.head == Head::Owner {
            unanchored(out, &stretch);
            stretch.clear();
            if seg.reply {
                anchored(out, &[seg]);
            } else if seg.folded {
                stretch.push(seg);
            }
            continue;
        }
        stretch.push(seg);
        if seg.reply {
            anchored(out, &stretch);
            stretch.clear();
        }
    }
    unanchored(out, &stretch);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(t: &str) -> Block {
        Block::text(t)
    }

    fn call(id: &str) -> Vec<Block> {
        vec![
            Block::ToolCall {
                tool_call_id: id.into(),
                tool_name: "bash".into(),
                input: Value::Null,
                summary: format!("run {id}"),
                presentation: None,
                elided: None,
            },
            Block::ToolResult {
                tool_call_id: id.into(),
                tool_name: "bash".into(),
                output: String::new(),
                is_error: false,
                images: Vec::new(),
            },
        ]
    }

    /// A message list the tests build: (key, message).
    enum M {
        User(&'static str, String, Option<Value>),
        Ai(&'static str, Vec<Block>),
        Notice(&'static str, bool),
    }

    fn plan_of(list: &[M], live: Option<&[Block]>, open: &dyn Fn(&str) -> bool) -> (FoldPlan, Vec<String>) {
        plan_with(list, live, open, true, None, &|_| false)
    }

    fn plan_with(
        list: &[M],
        live: Option<&[Block]>,
        open: &dyn Fn(&str) -> bool,
        marks: bool,
        run: Option<f64>,
        own: &dyn Fn(&str) -> bool,
    ) -> (FoldPlan, Vec<String>) {
        let keys: Vec<String> = list
            .iter()
            .map(|m| match m {
                M::User(k, ..) | M::Ai(k, _) | M::Notice(k, _) => (*k).to_owned(),
            })
            .collect();
        let msgs: Vec<Msg> = list
            .iter()
            .map(|m| match m {
                M::User(_, text, automation) => Msg::User {
                    text,
                    system: false,
                    automation: automation.as_ref(),
                    queue_entry_id: None,
                    started_at: 0.0,
                },
                M::Ai(_, blocks) => Msg::Assistant { blocks },
                M::Notice(_, informational) => Msg::System {
                    informational: *informational,
                },
            })
            .collect();
        (plan(&msgs, &keys, live, open, marks, run, own), keys)
    }

    fn at(keys: &[String], key: &str) -> usize {
        keys.iter().position(|k| k == key).unwrap()
    }

    fn user(key: &'static str, text: &str) -> M {
        M::User(key, text.into(), None)
    }

    fn wake(key: &'static str) -> M {
        M::User(
            key,
            format!("[chrono hard wake: chrono:{key}] Scheduled command failed"),
            Some(json!({"kind": "chrono_hard_wake"})),
        )
    }

    fn ai(key: &'static str, blocks: Vec<Block>) -> M {
        M::Ai(key, blocks)
    }

    fn cat(parts: Vec<Vec<Block>>) -> Vec<Block> {
        parts.into_iter().flatten().collect()
    }

    const CLOSED: &dyn Fn(&str) -> bool = &|_| false;

    #[test]
    fn a_finished_turn_folds_its_work_above_the_final_reply() {
        let list = [
            user("u:1", "what changed?"),
            ai(
                "a:1",
                cat(vec![
                    vec![Block::thinking("hm"), text("Checking.")],
                    call("c1"),
                    call("c2"),
                    vec![text("Two files.")],
                ]),
            ),
        ];
        let (plan, keys) = plan_of(&list, None, CLOSED);
        let fold = &plan.by_msg[&at(&keys, "a:1")];
        let mut hidden: Vec<usize> = fold.hidden.clone().unwrap().into_iter().collect();
        hidden.sort_unstable();
        assert_eq!(hidden, [0, 1, 2, 3, 4, 5]);
        assert_eq!(fold.turn, "turn:u:1");
        // Gutter: the reply carries the mark; the summary row is not used.
        assert_eq!(fold.summary, None);
        assert_eq!(
            plan.gutter[&at(&keys, "a:1")],
            GutterEntry {
                group: "group:u:1".into(),
                open: false,
                role: Role::Host,
                mark: true
            }
        );
    }

    #[test]
    fn a_reply_without_tools_only_hides_its_thinking_and_has_no_mark() {
        let list = [
            user("u:1", "hi"),
            ai("a:1", vec![Block::thinking("greet"), text("Hello.")]),
        ];
        let (plan, keys) = plan_of(&list, None, CLOSED);
        let fold = &plan.by_msg[&at(&keys, "a:1")];
        assert_eq!(fold.hidden.clone().unwrap().into_iter().collect::<Vec<_>>(), [0]);
        assert!(!plan.gutter.contains_key(&at(&keys, "a:1")));
    }

    #[test]
    fn the_live_turn_keeps_only_its_newest_step() {
        let list = [user("u:1", "run")];
        let live = cat(vec![vec![text("Starting.")], call("c1"), call("c2")]);
        let (plan, _) = plan_of(&list, Some(&live), CLOSED);
        let live_fold = plan.live.unwrap();
        assert_eq!(live_fold.summary.as_deref(), Some("1 tool call, 1 message"));
        let mut hidden: Vec<usize> = live_fold.hidden.unwrap().into_iter().collect();
        hidden.sort_unstable();
        assert_eq!(hidden, [0, 1, 2, 4]);
        let more = cat(vec![live.clone(), vec![text("All done")]]);
        let (plan, _) = plan_of(&list, Some(&more), CLOSED);
        assert!(!plan.live.unwrap().hidden.unwrap().contains(&5));
    }

    #[test]
    fn between_rounds_the_newest_committed_step_shows_under_the_working_row() {
        let list = [
            user("u:1", "run"),
            ai("a:1", cat(vec![call("c1"), vec![text("Round done.")]])),
        ];
        let (plan, keys) = plan_of(&list, Some(&[]), CLOSED);
        let a1 = at(&keys, "a:1");
        let mut hidden: Vec<usize> = plan.by_msg[&a1].hidden.clone().unwrap().into_iter().collect();
        hidden.sort_unstable();
        assert_eq!(hidden, [0, 1, 2]);
        let live = plan.live.unwrap();
        assert_eq!(live.summary.as_deref(), Some("1 tool call"));
        assert_eq!(live.revealed, Some((a1, 2)));
    }

    #[test]
    fn notifications_by_metadata_or_legacy_prefixes() {
        let none = |_: &str| false;
        assert_eq!(
            notification_label("done", Some(&json!({"kind": "subagent_completion"})), &none),
            Some("Subagent completion")
        );
        assert_eq!(
            notification_label("[notification] WhatsApp · message Mom", None, &none),
            Some("Notification")
        );
        assert_eq!(
            notification_label("[chrono wake: chrono:1] Check in", None, &none),
            Some("Notification")
        );
        assert_eq!(notification_label("[sic] I typed this", None, &none), None);
        assert_eq!(
            notification_preview(
                "[notification] Discord/paramount · DM\nYeyito [ch:1]\n\n→ Yeyito <@2> [owner] ↳ [reply-to:3]:\nare you there?\n\nAction: Discord notification for `paramount`."
            ),
            "Discord/paramount · DM Yeyito → Yeyito [owner]: are you there?"
        );
        // A parent conversation's prompt is not a notification.
        assert_eq!(
            notification_label(
                "Fix vimbrowser input.",
                Some(&json!({"kind": "exo_send", "sourceId": "p"})),
                &none
            ),
            None
        );
    }

    #[test]
    fn owners_relayed_words_are_theirs() {
        let relay = "[notification] Discord/paramount · DM\nYeyito [ch:1]\n\n→ Yeyito <@3> [owner]:\nfix it\n[msg:5]";
        let automation = json!({"kind": "external_notification"});
        assert_eq!(
            owner_relayed_text(relay, Some(&automation)).as_deref(),
            Some("fix it")
        );
        assert_eq!(notification_label(relay, Some(&automation), &|_| false), None);
        assert_eq!(
            owner_relayed_text("quoting\n→ Yeyito <@3> [owner]:\nhi", Some(&json!({"kind": "exo_send"}))),
            None
        );
        let attach = "[notification] Discord/x\n→ Me <@1> [owner]: [message]\n[attachment:9] IMG_1181.png\n[msg:1]";
        assert_eq!(
            owner_relayed_text(attach, Some(&automation)).as_deref(),
            Some("📎 IMG_1181.png")
        );
        let voice = "[notification] Discord/x\n→ Me <@1> [owner]: [📎 voice-message.ogg]\n[msg:1]";
        assert_eq!(
            owner_relayed_text(voice, Some(&automation)).as_deref(),
            Some("🎤 voice message")
        );
    }

    #[test]
    fn bubble_labels_name_the_sender() {
        let title = |id: &str| {
            if id == "timeline" {
                "You are the timeline feature agent".to_owned()
            } else {
                id.to_owned()
            }
        };
        let label = |text: &str, automation: Value, label: &str| {
            bubble_label(text, Some(&automation), label, &title)
        };
        assert_eq!(
            label(
                "[notification] Subagent stopped: exo:timeline",
                json!({"kind": "subagent_completion", "sourceId": "timeline"}),
                "Subagent completion"
            ),
            "Subagent completion · from You are the timeline feature agent"
        );
        assert_eq!(
            label(
                "[notification] Discord/paramount · DM\nFede [ch:1]\n\n→ Fede <@4>:\nhey",
                json!({"kind": "external_notification"}),
                "Notification"
            ),
            "Notification · from Discord/paramount · DM"
        );
        assert_eq!(
            label("check", json!({"kind": "chrono_wake", "sourceId": "chrono:abc"}), "Chrono"),
            "Chrono"
        );
        assert_eq!(
            label("no id", json!({"kind": "exo_send"}), "Exo delegation"),
            "Exo delegation · from another conversation"
        );
    }

    #[test]
    fn gutter_notifications_and_silent_turns_leave_no_row_the_next_reply_marks_them() {
        let list = [
            user("u:0", "start"),
            ai("a:0", vec![text("ok")]),
            wake("u:1"),
            ai("a:1", cat(vec![vec![text("checking")], call("a1a")])),
            wake("u:2"),
            ai("a:2", cat(vec![call("r"), vec![text("Restarted.")]])),
        ];
        let (plan, keys) = plan_of(&list, None, CLOSED);
        for key in ["u:1", "a:1", "u:2"] {
            assert_eq!(
                plan.gutter[&at(&keys, key)],
                GutterEntry {
                    group: "group:u:1".into(),
                    open: false,
                    role: Role::Hidden,
                    mark: true
                },
                "{key}"
            );
            assert!(plan.hides(at(&keys, key)));
        }
        assert_eq!(plan.gutter[&at(&keys, "a:2")].role, Role::Host);
        assert!(!plan.by_msg[&at(&keys, "a:2")].hidden.as_ref().unwrap().is_empty());
        assert!(!plan.gutter.contains_key(&at(&keys, "a:0")));
        // Opened: every member shows, and carries the mark.
        let (open, keys) = plan_of(&list, None, &|key| key == "group:u:1");
        assert!(open.gutter[&at(&keys, "u:1")].open);
        assert_eq!(
            open.merged[&at(&keys, "u:1")],
            MergedFold {
                turn: "group:u:1".into(),
                open: true,
                lead: None
            }
        );
        assert_eq!(open.by_msg[&at(&keys, "a:2")].hidden, None);
        assert!(!open.hides(at(&keys, "u:1")));
    }

    #[test]
    fn gutter_a_stretch_with_no_reply_leaves_a_line_and_marks_off_nothing() {
        let list = [
            user("u:0", "start"),
            ai("a:0", vec![text("ok")]),
            wake("u:1"),
            ai("a:1", cat(vec![vec![text("checking")], call("a1a")])),
        ];
        let (plan, keys) = plan_of(&list, None, CLOSED);
        assert_eq!(plan.gutter[&at(&keys, "u:1")].role, Role::Line);
        assert!(plan.gutter[&at(&keys, "u:1")].mark);
        let (off, keys) = plan_with(&list, None, CLOSED, false, None, &|_| false);
        assert_eq!(off.gutter[&at(&keys, "u:1")].role, Role::Hidden);
        assert!(!off.gutter[&at(&keys, "u:1")].mark);
    }

    #[test]
    fn informational_notices_fold_errors_and_warnings_stay() {
        let list = [
            user("u:0", "check the VM"),
            ai("a:0", cat(vec![vec![text("On it.")], call("x")])),
            M::Notice("s:1", true),
            ai("a:1", cat(vec![call("y"), vec![text("All healthy.")]])),
            user("u:2", "again"),
            ai("a:2", vec![text("Partial")]),
            M::Notice("s:3", false),
            user("u:4", "more"),
            ai("a:4", vec![text("ok")]),
            M::Notice("s:5", true),
            M::Notice("s:6", false),
        ];
        let (plan, keys) = plan_of(&list, None, CLOSED);
        assert!(plan.hides(at(&keys, "s:1")));
        assert!(plan.hides(at(&keys, "s:5")));
        assert!(!plan.hides(at(&keys, "s:3")));
        assert!(!plan.hides(at(&keys, "s:6")));
        assert_eq!(plan.gutter[&at(&keys, "a:1")].role, Role::Host);
        let (open, keys) = plan_of(&list, None, &|key| key == "group:u:0");
        assert_eq!(
            open.merged[&at(&keys, "s:1")],
            MergedFold {
                turn: "group:u:0".into(),
                open: true,
                lead: None
            }
        );
    }

    #[test]
    fn the_live_turn_reveals_its_newest_committed_step() {
        let list = [
            user("u:0", "run the checks"),
            ai("a:0", cat(vec![call("c1"), call("c2")])),
        ];
        let (plan, keys) = plan_of(&list, Some(&[]), CLOSED);
        let a0 = at(&keys, "a:0");
        assert_eq!(plan.by_msg[&a0].hidden.as_ref().unwrap().len(), 4);
        let live = plan.live.unwrap();
        assert_eq!(live.revealed, Some((a0, 2)));
        assert_eq!(live.summary.as_deref(), Some("1 tool call"));
        let next = call("c3")[..1].to_vec();
        let (streaming, _) = plan_of(&list, Some(&next), CLOSED);
        let live = streaming.live.unwrap();
        assert_eq!(live.revealed, None);
        assert!(!live.hidden.unwrap().contains(&0));
    }

    #[test]
    fn a_notification_leads_its_turns_work_and_mid_turn_ones_join_the_next() {
        let note = |key: &'static str, n: &str| {
            M::User(
                key,
                format!("[notification] Subagent completed: exo:{n}"),
                Some(json!({"kind": "subagent_completion"})),
            )
        };
        let list = [
            user("u:0", "go"),
            ai("a:0", vec![text("ok")]),
            note("u:1", "1"),
            note("u:2", "2"),
            ai("a:2", cat(vec![call("c"), vec![text("Both merged.")]])),
            note("u:4", "4"),
        ];
        let (plan, keys) = plan_of(&list, None, CLOSED);
        // Both fold into the turn's work (and the gutter group).
        assert!(plan.hides(at(&keys, "u:1")));
        assert!(plan.hides(at(&keys, "u:2")));
        assert_eq!(plan.mark(at(&keys, "a:2")).unwrap().group, "group:u:1");
        // The one at the very end nothing answered: a line in its place.
        assert!(plan.hides(at(&keys, "u:4")));
        assert_eq!(plan.line(at(&keys, "u:4")).unwrap().group, "group:u:4");
    }

    #[test]
    fn a_top_level_conversations_own_subagents_fold_like_notifications() {
        let up = M::User(
            "u:2",
            "FYI: the feed API changed.".into(),
            Some(json!({"kind": "exo_send", "sourceId": "timeline"})),
        );
        let list = [
            user("u:1", "build it"),
            ai("a:1", vec![text("Delegated.")]),
            up,
            ai("a:2", cat(vec![call("c1"), vec![text("Noted.")]])),
        ];
        let own = |id: &str| id == "timeline";
        let (plan, keys) = plan_with(&list, None, CLOSED, true, None, &own);
        assert_eq!(plan.gutter[&at(&keys, "u:2")].role, Role::Hidden);
        assert_eq!(plan.gutter[&at(&keys, "a:2")].role, Role::Host);
        let (other, keys) = plan_with(&list, None, CLOSED, true, None, &|_| false);
        assert!(!other.gutter.contains_key(&at(&keys, "u:2")));
        // Live: it leads the working row.
        let live = cat(vec![call("c2"), call("c3")]);
        let (plan, _) = plan_with(&list[..3], Some(&live), CLOSED, true, None, &own);
        assert_eq!(
            plan.live.unwrap().summary.as_deref(),
            Some("Exo delegation · 1 tool call")
        );
    }

    #[test]
    fn a_message_delivered_into_the_run_does_not_start_a_new_turn() {
        // u:1 started the run at 1000; u:2 was queued into it at 5000.
        let keys: Vec<String> = ["u:1", "a:1", "u:2"].iter().map(|k| (*k).to_owned()).collect();
        let a1 = cat(vec![call("t1"), call("t2")]);
        let msgs = [
            Msg::User {
                text: "run the checks",
                system: false,
                automation: None,
                queue_entry_id: None,
                started_at: 1000.0,
            },
            Msg::Assistant { blocks: &a1 },
            Msg::User {
                text: "also check memory",
                system: false,
                automation: None,
                queue_entry_id: Some("q1"),
                started_at: 5000.0,
            },
        ];
        let plan = plan(&msgs, &keys, Some(&[]), CLOSED, true, Some(1000.0), &|_| false);
        let live = plan.live.unwrap();
        assert_eq!(live.summary.as_deref(), Some("1 tool call"));
        assert_eq!(live.revealed, Some((1, 2)));
        assert_eq!(plan.by_msg[&1].summary, None);
    }

    #[test]
    fn images_attached_before_a_message_stay_in_view() {
        let attach = vec![
            Block::ToolCall {
                tool_call_id: "i".into(),
                tool_name: "attach".into(),
                input: Value::Null,
                summary: "a.png".into(),
                presentation: None,
                elided: None,
            },
            Block::ToolResult {
                tool_call_id: "i".into(),
                tool_name: "attach".into(),
                output: "Attached 1 image".into(),
                is_error: false,
                images: vec![super::super::proto::ImageRef {
                    id: "e".repeat(64),
                    ..Default::default()
                }],
            },
        ];
        let list = [
            user("u:0", "send the chart"),
            ai("a:0", attach),
            user("u:1", "thanks"),
            ai("a:1", vec![text("Sure.")]),
        ];
        let (plan, keys) = plan_of(&list, None, CLOSED);
        assert_ne!(
            plan.gutter.get(&at(&keys, "a:0")).map(|g| g.role),
            Some(Role::Hidden)
        );
    }
}
