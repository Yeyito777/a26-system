//! `exocortex-notify`: posts Moon notifications for the Exocortex daemon.
//!
//! It stays connected to the daemon (a plain client whose bootstrap asks for
//! a first page of no rows, so it learns which conversations stream, then
//! only hears the broadcasts every client gets) and to
//! `moon-notifyd`, and sleeps in `poll(2)` between events. When a top-level
//! conversation's turn chain ends with the owner waiting on it
//! (`ownerTurnFinished`), or its goal stops and needs them, it reads the
//! reply's start (one history page, which neither subscribes nor marks the
//! conversation read) and posts it, tagged by conversation, opening the
//! Exocortex app at it. Not while the app shows that conversation in front.
//! The notification is withdrawn when the conversation is read anywhere or
//! deleted (the app withdraws it itself when it opens it).
//!
//! While any conversation streams (subagents too) it holds Moon's
//! `transfer` lease for `exocortex`, renewed every minute, so turning the
//! screen off does not deep-suspend the phone under a running turn.
//!
//! It keeps the Exocortex icon's badge on Moon's home screen (`badge
//! exocortex N`): the unread conversations, as the list's green dots and
//! the iPhone's badge count them (unread, not muted, no model work, neither
//! streaming nor waiting). The bootstrap's first page holds every folder
//! and every conversation showing activity, the broadcasts after it the
//! changes, so the count is right without the whole list; it is set when it
//! changes, and while the app is closed.
//!
//! Its peers come and go: the daemon restarts, moon-notifyd and Moon are
//! absent in Android mode. Each is retried with backoff; posts wait (newest
//! per conversation, at most 12 h) until moon-notifyd is there, the badge
//! until Moon is.

#[path = "../exocortex/handoff.rs"]
mod handoff;
#[path = "../exocortex/line.rs"]
mod line;
#[path = "../exocortex/markdown.rs"]
mod markdown;
#[path = "../exocortex/proto/mod.rs"]
mod proto;
/// The list's activity rules (B), for the badge.
#[path = "../exocortex/list"]
mod list {
    #[allow(dead_code)]
    pub mod activity;
}

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use line::LineSocket;
use list::activity::{self, Kind, QueueInfo};
use proto::{Block, Entry, Event, Folder, Summary};

/// Compact sidebar deltas, and an incremental bootstrap whose first page
/// holds every conversation showing activity (which ones stream).
const CAPABILITIES: [&str; 5] = [
    "token-stats-on-demand",
    "sidebar-state-patch",
    "sidebar-reorder-delta",
    "targeted-unwind",
    "sidebar-incremental",
];
/// The lease Moon grants (its maximum), and how often it is renewed.
const LEASE_SECONDS: u64 = 120;
const LEASE_RENEW: Duration = Duration::from_secs(60);
/// Moon is absent (Android mode, a restart): ask again this much later.
const LEASE_RETRY: Duration = Duration::from_secs(15);
/// Moon is absent or refused the badge: set it again this much later (s).
const BADGE_BACKOFF: [u64; 5] = [5, 15, 60, 300, 900];
const DAEMON_BACKOFF: [u64; 7] = [500, 1000, 2000, 4000, 8000, 15000, 30000];
const NOTIFY_BACKOFF: [u64; 6] = [1000, 2000, 5000, 10000, 30000, 60000];
const POST_TTL: Duration = Duration::from_secs(12 * 3600);
const MAX_PENDING: usize = 50;
const BODY_CHARS: usize = 280;
const MAX_KNOWN: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
enum Effect {
    Daemon(Value),
    Post {
        conversation: String,
        title: String,
        body: String,
    },
    Withdraw(String),
}

#[derive(Debug, Clone, Default)]
struct Known {
    unread: bool,
    goal: Option<String>,
}

/// Why a conversation wants the user.
#[derive(Debug, Clone, PartialEq)]
enum Reason {
    Finished,
    Goal(String),
}

/// The bridge's decisions, without I/O (so they are tested).
struct Bridge<F: FnMut(&str) -> bool> {
    known: HashMap<String, Known>,
    /// Reply fetches in flight: request id → conversation and title.
    fetches: HashMap<String, (String, String)>,
    /// Conversations streaming now (any, subagents included).
    streaming: HashSet<String>,
    next_request: u64,
    app_showing: F,
    /// When each conversation last notified: at most one per 5 s (PAR-315).
    notified: HashMap<String, Instant>,
    unread: Unread,
}

/// A conversation notifies at most this often.
const NOTIFY_EVERY: Duration = Duration::from_secs(5);

impl<F: FnMut(&str) -> bool> Bridge<F> {
    fn new(app_showing: F) -> Self {
        Self {
            known: HashMap::new(),
            fetches: HashMap::new(),
            streaming: HashSet::new(),
            next_request: 0,
            app_showing,
            notified: HashMap::new(),
            unread: Unread::default(),
        }
    }

    fn on_connected(&mut self) -> Vec<Effect> {
        self.fetches.clear();
        // Unknown until the first page: the badge keeps what it shows.
        self.unread = Unread::default();
        // A first page of no rows: just the conversations showing activity.
        vec![
            Effect::Daemon(proto::client_capabilities(&CAPABILITIES, Some(0))),
            Effect::Daemon(proto::ping("notify-bootstrap")),
        ]
    }

    fn on_disconnected(&mut self) {
        self.fetches.clear();
        self.streaming.clear();
    }

    /// Something streams: Moon must not deep-suspend.
    fn wants_lease(&self) -> bool {
        !self.streaming.is_empty()
    }

    /// What the icon's badge should show, once known.
    fn badge(&self) -> Option<u32> {
        self.unread.count()
    }

    fn on_line(&mut self, text: &str) -> Vec<Effect> {
        // Only a handful of event types matter; the rest are not parsed.
        match proto::event_type(text) {
            Some(
                "conversation_updated"
                | "conversation_deleted"
                | "conversation_restored"
                | "conversation_unwound"
                | "sidebar_state_patched"
                | "queue_updated"
                | "conversation_history_loaded"
                | "error",
            )
            | None => {}
            // The bootstrap's rest (megabytes) holds no activity: skipped.
            Some("conversations_list") if !text.contains("\"sidebarRest\":true") => {}
            Some(_) => return Vec::new(),
        }
        let event = proto::parse_event(text);
        self.unread.apply(&event);
        match event {
            Event::ConversationsList { conversations, .. } => {
                self.streaming = conversations
                    .into_iter()
                    .filter(|summary| summary.streaming)
                    .map(|summary| summary.id)
                    .collect();
                Vec::new()
            }
            Event::ConversationUpdated {
                summary,
                stream_stop_reason,
                owner_turn_finished,
            } => self.on_update(summary, stream_stop_reason, owner_turn_finished),
            Event::ConversationDeleted { conv_id, .. } => {
                self.known.remove(&conv_id);
                self.streaming.remove(&conv_id);
                vec![Effect::Withdraw(conv_id)]
            }
            Event::ConversationHistoryLoaded { req_id, window, .. } => {
                let Some((conversation, title)) = req_id.and_then(|id| self.fetches.remove(&id))
                else {
                    return Vec::new();
                };
                let body = reply_body(&window.entries);
                vec![Effect::Post {
                    conversation,
                    title,
                    body,
                }]
            }
            Event::Error { req_id, .. } => {
                // The page could not be read: say it finished, without text.
                match req_id.and_then(|id| self.fetches.remove(&id)) {
                    Some((conversation, title)) => vec![Effect::Post {
                        conversation,
                        title,
                        body: "Finished.".into(),
                    }],
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn on_update(
        &mut self,
        summary: Summary,
        stop_reason: Option<String>,
        owner_turn_finished: bool,
    ) -> Vec<Effect> {
        if summary.streaming {
            self.streaming.insert(summary.id.clone());
        } else {
            self.streaming.remove(&summary.id);
        }
        if summary.parent_conversation_id.is_some() {
            // Subagents report to their parent conversation, not the user.
            return Vec::new();
        }
        if self.known.len() > MAX_KNOWN && !self.known.contains_key(&summary.id) {
            self.known.clear();
        }
        let goal = summary.goal.as_ref().map(goal_state);
        let previous = self.known.insert(
            summary.id.clone(),
            Known {
                unread: summary.unread,
                goal: goal.clone().map(|(state, _)| state),
            },
        );
        let mut effects = Vec::new();
        if previous.as_ref().is_some_and(|known| known.unread) && !summary.unread {
            // Read somewhere (the app, the TUI): the notification is stale.
            effects.push(Effect::Withdraw(summary.id.clone()));
        }
        let reason = if owner_turn_finished && !summary.streaming && stop_reason.is_none() {
            Some(Reason::Finished)
        } else {
            match (
                &goal,
                previous.as_ref().and_then(|known| known.goal.clone()),
            ) {
                (Some((state, text)), Some(before)) if *state != before && !text.is_empty() => {
                    Some(Reason::Goal(text.clone()))
                }
                _ => None,
            }
        };
        let Some(reason) = reason else {
            return effects;
        };
        if summary.notifications_muted || (self.app_showing)(&summary.id) {
            return effects;
        }
        let now = Instant::now();
        if self
            .notified
            .get(&summary.id)
            .is_some_and(|at| now.duration_since(*at) < NOTIFY_EVERY)
        {
            return effects;
        }
        if self.notified.len() > MAX_KNOWN {
            self.notified
                .retain(|_, at| now.duration_since(*at) < NOTIFY_EVERY);
        }
        self.notified.insert(summary.id.clone(), now);
        let title = summary.display_title().to_owned();
        match reason {
            Reason::Goal(body) => effects.push(Effect::Post {
                conversation: summary.id.clone(),
                title,
                body,
            }),
            Reason::Finished => {
                self.next_request += 1;
                let request = format!("notify-{}", self.next_request);
                effects.push(Effect::Daemon(proto::load_newest(&request, &summary.id, 1)));
                self.fetches.insert(request, (summary.id.clone(), title));
            }
        }
        effects
    }
}

/// The conversations the icon's badge counts.
#[derive(Debug, Default)]
struct Unread {
    /// The unread ones: only they can count, and `unread` changes only with
    /// a whole summary.
    conversations: HashMap<String, Summary>,
    folders: HashMap<String, Folder>,
    /// Each conversation's queue entries (one in the global-idle queue makes
    /// it wait).
    queues: HashMap<String, QueueInfo>,
    /// The bootstrap's first page has come.
    known: bool,
}

impl Unread {
    fn upsert(&mut self, summary: &Summary) {
        if summary.unread {
            self.conversations
                .insert(summary.id.clone(), summary.clone());
        } else {
            self.conversations.remove(&summary.id);
        }
    }

    fn apply(&mut self, event: &Event) {
        match event {
            Event::ConversationsList {
                conversations,
                folders,
                sidebar_rest,
                ..
            } => {
                if *sidebar_rest != Some(true) {
                    self.conversations.clear();
                    self.known = true;
                }
                for summary in conversations {
                    self.upsert(summary);
                }
                if let Some(folders) = folders {
                    self.folders = folders.iter().map(|f| (f.id.clone(), f.clone())).collect();
                }
            }
            Event::ConversationUpdated { summary, .. }
            | Event::ConversationRestored { summary, .. }
            | Event::ConversationUnwound { summary, .. } => self.upsert(summary),
            Event::ConversationDeleted { conv_id, .. } => {
                self.conversations.remove(conv_id);
            }
            Event::SidebarStatePatched {
                conversations,
                folders,
                removed_conversation_ids,
                removed_folder_ids,
            } => {
                for summary in conversations {
                    self.upsert(summary);
                }
                for folder in folders {
                    self.folders.insert(folder.id.clone(), folder.clone());
                }
                for id in removed_conversation_ids {
                    self.conversations.remove(id);
                }
                for id in removed_folder_ids {
                    self.folders.remove(id);
                }
            }
            Event::QueueUpdated { messages, .. } => {
                self.queues.clear();
                for entry in messages {
                    let info = self.queues.entry(entry.conv_id.clone()).or_default();
                    info.count += 1;
                    info.idle |= entry.source == "global-idle";
                }
            }
            _ => {}
        }
    }

    /// A folder muted itself or by a folder above.
    fn folder_muted(&self, folder: Option<&str>) -> bool {
        let mut seen = HashSet::new();
        let mut at = folder;
        while let Some(folder) = at.and_then(|id| self.folders.get(id)) {
            if folder.muted {
                return true;
            }
            if !seen.insert(folder.id.as_str()) {
                return false;
            }
            at = folder.parent_id.as_deref();
        }
        false
    }

    /// The list's green dots (the iPhone's `totals.unread`), once known.
    fn count(&self) -> Option<u32> {
        self.known.then(|| {
            self.conversations
                .values()
                .filter(|summary| {
                    let queue = self.queues.get(&summary.id).copied().unwrap_or_default();
                    let muted = self.folder_muted(summary.folder_id.as_deref());
                    activity::conversation(summary, queue, false, muted).kind == Some(Kind::Unread)
                })
                .count() as u32
        })
    }
}

/// A goal's state that needs the user, with what to say about it. Other
/// states are only remembered (so a change into one of these is noticed).
fn goal_state(goal: &proto::Goal) -> (String, String) {
    let reason = goal
        .reason
        .clone()
        .or_else(|| goal.pause_reason.clone())
        .unwrap_or_default();
    let say = |prefix: &str| {
        if reason.trim().is_empty() {
            prefix.trim_end_matches(": ").to_owned()
        } else {
            format!("{prefix}{}", reason.trim())
        }
    };
    match goal.status.as_str() {
        "blocked" => ("blocked".into(), say("Goal blocked: ")),
        "paused" if goal.paused_by.as_deref() == Some("controller") => {
            ("paused".into(), say("Goal paused: "))
        }
        "complete" => ("complete".into(), say("Goal complete: ")),
        other => (other.to_owned(), String::new()),
    }
}

/// The start of the newest reply, as one line of plain text: its text
/// (markdown stripped), or the error that ended the turn.
fn reply_body(entries: &[Entry]) -> String {
    let mut text = String::new();
    for entry in entries.iter().rev() {
        match entry {
            Entry::Ai { blocks, .. } => {
                text = proto::reply_text(blocks);
                if text.is_empty() {
                    text = blocks
                        .iter()
                        .rev()
                        .find_map(|block| match block {
                            Block::ToolCall { summary, .. } if !summary.is_empty() => {
                                Some(summary.clone())
                            }
                            _ => None,
                        })
                        .unwrap_or_default();
                }
                break;
            }
            Entry::System {
                text: notice,
                color,
                ..
            } => {
                if color.as_deref() == Some("error") {
                    text = notice.clone();
                    break;
                }
            }
            Entry::User { .. } => break,
            _ => {}
        }
    }
    let plain: Vec<String> = markdown::parse(&text)
        .into_iter()
        .map(|block| match block {
            markdown::MdBlock::Paragraph(spans)
            | markdown::MdBlock::Heading(_, spans)
            | markdown::MdBlock::Quote(spans) => spans.into_iter().map(|s| s.text).collect(),
            markdown::MdBlock::Item(_, marker, spans) => {
                let text: String = spans.into_iter().map(|s| s.text).collect();
                format!("{marker} {text}")
            }
            markdown::MdBlock::Code { lines, .. } => lines.join(" "),
            markdown::MdBlock::Table(table) => table.text(),
            markdown::MdBlock::Math(tex) => tex,
            markdown::MdBlock::Long { preview, .. } => preview,
            markdown::MdBlock::More { .. } => String::new(),
            markdown::MdBlock::Rule => String::new(),
        })
        .collect();
    let joined = plain
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        return "Finished.".into();
    }
    if joined.chars().count() > BODY_CHARS {
        let cut: String = joined.chars().take(BODY_CHARS - 1).collect();
        let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
        return format!("{}…", cut.trim_end_matches([',', '.', ';', ':']));
    }
    joined
}

fn clean_title(title: &str) -> String {
    let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.is_empty() {
        "Exocortex".to_owned()
    } else {
        title
    };
    if title.chars().count() > 120 {
        format!("{}…", title.chars().take(119).collect::<String>())
    } else {
        title
    }
}

/// Holds Moon's lease while something streams: acquired, renewed before it
/// lapses, released when nothing does. Moon's answers decide `held`.
struct LeaseKeeper {
    held: bool,
    next: Instant,
}

impl LeaseKeeper {
    fn new(now: Instant) -> Self {
        Self {
            held: false,
            next: now,
        }
    }

    /// Does what is due; `moon` sends one control command and says whether
    /// Moon accepted it.
    fn step(&mut self, want: bool, now: Instant, mut moon: impl FnMut(&str) -> bool) {
        if want && now >= self.next {
            let command = format!("lease acquire {} transfer {LEASE_SECONDS}", handoff::APP_ID);
            self.held = moon(&command);
            self.next = now + if self.held { LEASE_RENEW } else { LEASE_RETRY };
        } else if !want && self.held {
            // A lost release lapses by itself within LEASE_SECONDS.
            let _ = moon(&format!("lease release {} transfer", handoff::APP_ID));
            self.held = false;
            self.next = now;
        }
    }

    fn deadline(&self, want: bool) -> Option<Instant> {
        want.then_some(self.next)
    }
}

/// Keeps the icon's badge at the unread count: set when it changes, again
/// with backoff while Moon is absent (Android mode, a restart) or refuses.
/// Moon keeps a badge across its restarts and reboots.
struct BadgeKeeper {
    shown: Option<u32>,
    next: Instant,
    failures: usize,
}

impl BadgeKeeper {
    fn new(now: Instant) -> Self {
        Self {
            shown: None,
            next: now,
            failures: 0,
        }
    }

    /// `moon` sends one control command and says whether Moon accepted it.
    fn step(&mut self, want: Option<u32>, now: Instant, mut moon: impl FnMut(&str) -> bool) {
        let Some(count) = want else {
            return;
        };
        if self.shown == Some(count) || now < self.next {
            return;
        }
        if moon(&format!("badge {} {count}", handoff::APP_ID)) {
            self.shown = Some(count);
            self.failures = 0;
        } else {
            let wait = BADGE_BACKOFF[self.failures.min(BADGE_BACKOFF.len() - 1)];
            self.next = now + Duration::from_secs(wait);
            self.failures += 1;
        }
    }

    fn deadline(&self, want: Option<u32>) -> Option<Instant> {
        want.filter(|count| self.shown != Some(*count))
            .map(|_| self.next)
    }
}

// ----- I/O ------------------------------------------------------------------

/// A post or withdrawal waiting for moon-notifyd.
#[derive(Debug, Clone)]
struct Pending {
    conversation: String,
    request: Value,
    at: Instant,
}

struct Peer {
    socket: Option<LineSocket>,
    attempt: usize,
    retry_at: Instant,
    connected_at: Instant,
}

impl Peer {
    fn new() -> Self {
        Self {
            socket: None,
            attempt: 0,
            retry_at: Instant::now(),
            connected_at: Instant::now(),
        }
    }

    fn lost(&mut self, backoff: &[u64], now: Instant) {
        self.socket = None;
        if now.duration_since(self.connected_at) > Duration::from_secs(10) {
            self.attempt = 0;
        }
        self.retry_at = now + Duration::from_millis(backoff[self.attempt.min(backoff.len() - 1)]);
        self.attempt += 1;
    }
}

fn log(message: &str) {
    eprintln!("exocortex-notify: {message}");
}

fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|a| a == "--help" || a == "-h")
    {
        println!(
            "usage: exocortex-notify\n\nPosts Moon notifications when Exocortex conversations finish.\n\
             Environment: EXOCORTEX_SOCKET, MOON_NOTIFY_SOCKET, A26_SHELL_SOCKET, EXOCORTEX_APP_STATE."
        );
        return;
    }
    // A write to a vanished peer is an error to handle, not a reason to die.
    // SAFETY: setting a signal disposition to SIG_IGN.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let daemon_path = proto::socket_path();
    let notify_path = handoff::notify_socket();
    log(&format!(
        "daemon {}, notifications {}",
        daemon_path.display(),
        notify_path.display()
    ));
    let mut bridge = Bridge::new(handoff::app_showing);
    let mut daemon = Peer::new();
    let mut notify = Peer::new();
    let mut outbox: VecDeque<Pending> = VecDeque::new();
    let mut next_id: u64 = 0;
    let mut daemon_was_up = false;
    let mut lease = LeaseKeeper::new(Instant::now());
    let mut badge = BadgeKeeper::new(Instant::now());

    loop {
        let now = Instant::now();
        // Connect what is missing and due.
        if daemon.socket.is_none() && now >= daemon.retry_at {
            match LineSocket::connect(&daemon_path) {
                Ok(mut socket) => {
                    let mut ok = true;
                    for effect in bridge.on_connected() {
                        if let Effect::Daemon(command) = effect {
                            ok &= socket.send(&command.to_string()).is_ok();
                        }
                    }
                    if ok {
                        log("connected to the daemon");
                        daemon.socket = Some(socket);
                        daemon.connected_at = now;
                        daemon_was_up = true;
                    } else {
                        daemon.lost(&DAEMON_BACKOFF, now);
                    }
                }
                Err(_) => {
                    if daemon_was_up {
                        log("daemon unreachable; retrying");
                        daemon_was_up = false;
                    }
                    daemon.lost(&DAEMON_BACKOFF, now);
                }
            }
        }
        outbox.retain(|pending| now.duration_since(pending.at) < POST_TTL);
        if notify.socket.is_none() && !outbox.is_empty() && now >= notify.retry_at {
            match LineSocket::connect(&notify_path) {
                Ok(socket) => {
                    notify.socket = Some(socket);
                    notify.connected_at = now;
                }
                Err(_) => notify.lost(&NOTIFY_BACKOFF, now),
            }
        }
        // Hand waiting posts to moon-notifyd.
        if let Some(socket) = notify.socket.as_mut() {
            let mut failed = false;
            while let Some(pending) = outbox.pop_front() {
                if socket.send(&pending.request.to_string()).is_err() {
                    outbox.push_front(pending);
                    failed = true;
                    break;
                }
            }
            if failed {
                notify.lost(&NOTIFY_BACKOFF, now);
            }
        }

        let moon = |command: &str| {
            handoff::moon_request(command, Duration::from_millis(500))
                .is_some_and(|reply| reply["ok"].as_bool() == Some(true))
        };
        lease.step(bridge.wants_lease(), now, moon);
        badge.step(bridge.badge(), now, |command| {
            let accepted = moon(command);
            if accepted {
                log(command);
            }
            accepted
        });

        // Sleep until a socket speaks or a retry is due.
        let mut fds = Vec::with_capacity(2);
        if let Some(socket) = &daemon.socket {
            fds.push(line::pollfd(socket.fd(), socket.interest()));
        }
        if let Some(socket) = &notify.socket {
            fds.push(line::pollfd(socket.fd(), socket.interest()));
        }
        let mut deadline: Option<Instant> = None;
        if daemon.socket.is_none() {
            deadline = Some(daemon.retry_at);
        }
        if notify.socket.is_none() && !outbox.is_empty() {
            deadline = Some(deadline.map_or(notify.retry_at, |d| d.min(notify.retry_at)));
        }
        for at in [
            lease.deadline(bridge.wants_lease()),
            badge.deadline(bridge.badge()),
        ]
        .into_iter()
        .flatten()
        {
            deadline = Some(deadline.map_or(at, |d| d.min(at)));
        }
        let timeout = deadline.map(|at| at.saturating_duration_since(Instant::now()));
        if line::poll(&mut fds, timeout).is_err() {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let now = Instant::now();
        let mut index = 0;

        if let Some(socket) = daemon.socket.as_mut() {
            let revents = fds[index].revents;
            index += 1;
            let mut effects = Vec::new();
            let mut alive = true;
            if revents & libc::POLLOUT != 0 && socket.flush().is_err() {
                alive = false;
            }
            if alive && revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                let result = socket.read_lines(|text| effects.extend(bridge.on_line(text)));
                alive = result.is_ok();
            }
            for effect in effects {
                match effect {
                    Effect::Daemon(command) => {
                        if socket.send(&command.to_string()).is_err() {
                            alive = false;
                        }
                    }
                    Effect::Post {
                        conversation,
                        title,
                        body,
                    } => {
                        next_id += 1;
                        log(&format!("notifying for {conversation}"));
                        let tag = handoff::tag_for(&conversation);
                        // The newest post per conversation replaces a waiting one.
                        outbox.retain(|p| p.conversation != conversation);
                        outbox.push_back(Pending {
                            request: json!({
                                "id": next_id,
                                "cmd": "notify.post",
                                "app": handoff::APP_ID,
                                "tag": tag,
                                "title": clean_title(&title),
                                "body": body,
                                "open": { "app": handoff::APP_ID, "target": conversation },
                            }),
                            conversation,
                            at: now,
                        });
                    }
                    Effect::Withdraw(conversation) => {
                        let waiting = outbox.len();
                        outbox.retain(|p| p.conversation != conversation);
                        // A post still waiting here is simply dropped; else
                        // moon-notifyd (whenever it is there) withdraws it.
                        if waiting == outbox.len() {
                            next_id += 1;
                            outbox.push_back(Pending {
                                request: json!({
                                    "id": next_id,
                                    "cmd": "notify.withdraw",
                                    "app": handoff::APP_ID,
                                    "tag": handoff::tag_for(&conversation),
                                }),
                                conversation,
                                at: now,
                            });
                        }
                    }
                }
            }
            while outbox.len() > MAX_PENDING {
                outbox.pop_front();
            }
            if !alive {
                bridge.on_disconnected();
                log("disconnected from the daemon");
                daemon.lost(&DAEMON_BACKOFF, now);
                daemon_was_up = true;
            }
        }

        if let Some(socket) = notify.socket.as_mut() {
            let revents = fds[index].revents;
            let mut alive = true;
            if revents & libc::POLLOUT != 0 && socket.flush().is_err() {
                alive = false;
            }
            if alive && revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                // Answers are not needed: a refused post is logged, not retried.
                let result = socket.read_lines(|text| {
                    // Withdrawing what is already gone is not news.
                    if text.contains("\"ok\":false") && !text.contains("\"not_found\"") {
                        log(&format!("moon-notifyd refused: {text}"));
                    }
                });
                alive = result.is_ok();
            }
            if !alive {
                notify.lost(&NOTIFY_BACKOFF, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(id: &str, extra: &str, finished: bool) -> String {
        format!(
            r#"{{"type":"conversation_updated","summary":{{"id":"{id}","title":"Plan trip","streaming":false,"unread":true{extra}}}{}}}"#,
            if finished {
                r#","ownerTurnFinished":true"#
            } else {
                ""
            }
        )
    }

    #[test]
    fn a_finished_turn_is_read_then_posted() {
        let mut bridge = Bridge::new(|_| false);
        assert_eq!(bridge.on_line(&update("c1", "", false)), vec![]);
        let effects = bridge.on_line(&update("c1", "", true));
        let Effect::Daemon(command) = &effects[0] else {
            panic!("{effects:?}")
        };
        assert_eq!(command["type"], "load_conversation_history");
        let request = command["reqId"].as_str().unwrap();
        let page = format!(
            r#"{{"type":"conversation_history_loaded","reqId":"{request}","convId":"c1","entries":[{{"type":"user","text":"q"}},{{"type":"ai","blocks":[{{"type":"tool_call","toolCallId":"t","toolName":"Bash","input":{{}},"summary":"ls"}},{{"type":"text","text":"**Done.** Booked:\n\n- flight\n- hotel"}}]}}],"historyStartIndex":0,"historyStartUserIndex":0,"historyEndIndex":2,"historyTotalEntries":2,"hasOlderHistory":false}}"#
        );
        assert_eq!(
            bridge.on_line(&page),
            vec![Effect::Post {
                conversation: "c1".into(),
                title: "Plan trip".into(),
                body: "Done. Booked: • flight • hotel".into()
            }]
        );
    }

    #[test]
    fn subagents_muted_interrupted_and_shown_conversations_are_quiet() {
        let mut bridge = Bridge::new(|id| id == "shown");
        assert!(
            bridge
                .on_line(&update("s", r#","parentConversationId":"p""#, true))
                .is_empty()
        );
        assert!(
            bridge
                .on_line(&update("m", r#","notificationsMuted":true"#, true))
                .is_empty()
        );
        assert!(bridge.on_line(&update("shown", "", true)).is_empty());
        let stopped = r#"{"type":"conversation_updated","summary":{"id":"r","streaming":false},"streamStopReason":"daemon-restart","ownerTurnFinished":true}"#;
        assert!(bridge.on_line(stopped).is_empty());
        // Unrelated traffic is not even parsed.
        assert!(
            bridge
                .on_line(r#"{"type":"usage_update","provider":"x"}"#)
                .is_empty()
        );
    }

    #[test]
    fn reading_or_deleting_withdraws_and_goals_that_stop_notify() {
        let mut bridge = Bridge::new(|_| false);
        bridge.on_line(&update("c", "", false));
        let read = r#"{"type":"conversation_updated","summary":{"id":"c","streaming":false,"unread":false}}"#;
        assert_eq!(bridge.on_line(read), vec![Effect::Withdraw("c".into())]);
        assert_eq!(
            bridge.on_line(r#"{"type":"conversation_deleted","convId":"c"}"#),
            vec![Effect::Withdraw("c".into())]
        );
        let goal = |status: &str| {
            format!(
                r#"{{"type":"conversation_updated","summary":{{"id":"g","title":"Goal","streaming":false,"unread":false,"goal":{{"objective":"x","status":"{status}","reason":"Needs a password"}}}}}}"#
            )
        };
        assert!(bridge.on_line(&goal("active")).is_empty());
        assert_eq!(
            bridge.on_line(&goal("blocked")),
            vec![Effect::Post {
                conversation: "g".into(),
                title: "Goal".into(),
                body: "Goal blocked: Needs a password".into()
            }]
        );
        assert!(
            bridge.on_line(&goal("blocked")).is_empty(),
            "only on a change"
        );
    }

    #[test]
    fn a_conversation_notifies_at_most_once_in_five_seconds() {
        let mut bridge = Bridge::new(|_| false);
        bridge.on_connected();
        let finished = |unread: bool| {
            format!(
                r#"{{"type":"conversation_updated","summary":{{"id":"r","title":"R","streaming":false,"unread":{unread}}},"ownerTurnFinished":true}}"#
            )
        };
        let fetches = |effects: &[Effect]| {
            effects
                .iter()
                .filter(|e| matches!(e, Effect::Daemon(_)))
                .count()
        };
        assert_eq!(fetches(&bridge.on_line(&finished(true))), 1);
        assert_eq!(fetches(&bridge.on_line(&finished(true))), 0, "within 5 s");
        let at = bridge.notified.get_mut("r").unwrap();
        *at = Instant::now() - NOTIFY_EVERY;
        assert_eq!(fetches(&bridge.on_line(&finished(true))), 1, "5 s later");
    }

    #[test]
    fn streaming_conversations_are_tracked_for_the_lease() {
        let mut bridge = Bridge::new(|_| false);
        let effects = bridge.on_connected();
        assert_eq!(effects.len(), 2);
        let Effect::Daemon(caps) = &effects[0] else {
            panic!()
        };
        assert_eq!(caps["sidebarView"]["rows"], 0);
        bridge.on_line(r#"{"type":"conversations_list","conversations":[{"id":"a","streaming":true},{"id":"b","streaming":false}],"folders":[],"sidebarPartial":{"remaining":9,"folderConversationCounts":{}}}"#);
        assert!(bridge.wants_lease());
        // The rest of the bootstrap is not even parsed.
        bridge.on_line(r#"{"type":"conversations_list","conversations":[{"id":"z","streaming":true}],"sidebarRest":true}"#);
        assert_eq!(bridge.streaming.len(), 1);
        bridge.on_line(r#"{"type":"conversation_updated","summary":{"id":"s","streaming":true,"parentConversationId":"a"}}"#);
        bridge.on_line(r#"{"type":"conversation_updated","summary":{"id":"a","streaming":false}}"#);
        assert!(bridge.wants_lease(), "a subagent streams");
        bridge.on_line(r#"{"type":"conversation_deleted","convId":"s"}"#);
        assert!(!bridge.wants_lease());
        bridge.on_line(r#"{"type":"conversation_updated","summary":{"id":"a","streaming":true}}"#);
        bridge.on_disconnected();
        assert!(!bridge.wants_lease());
    }

    #[test]
    fn the_lease_is_acquired_renewed_retried_and_released() {
        let start = Instant::now();
        let mut keeper = LeaseKeeper::new(start);
        let mut sent = Vec::new();
        let mut moon_up = false;
        let step = |keeper: &mut LeaseKeeper,
                    want: bool,
                    at: Duration,
                    up: bool,
                    sent: &mut Vec<String>| {
            keeper.step(want, start + at, |command| {
                sent.push(command.to_owned());
                up
            });
        };
        step(&mut keeper, false, Duration::ZERO, moon_up, &mut sent);
        assert!(sent.is_empty());
        // Moon absent: retried later, not in a loop.
        step(&mut keeper, true, Duration::ZERO, moon_up, &mut sent);
        assert_eq!(sent, vec!["lease acquire exocortex transfer 120"]);
        assert!(!keeper.held);
        step(
            &mut keeper,
            true,
            Duration::from_secs(5),
            moon_up,
            &mut sent,
        );
        assert_eq!(sent.len(), 1);
        moon_up = true;
        step(&mut keeper, true, LEASE_RETRY, moon_up, &mut sent);
        assert!(keeper.held);
        assert_eq!(
            keeper.deadline(true),
            Some(start + LEASE_RETRY + LEASE_RENEW)
        );
        step(
            &mut keeper,
            true,
            LEASE_RETRY + LEASE_RENEW,
            moon_up,
            &mut sent,
        );
        assert_eq!(sent.len(), 3, "renewed");
        step(
            &mut keeper,
            false,
            LEASE_RETRY + LEASE_RENEW,
            moon_up,
            &mut sent,
        );
        assert_eq!(sent.last().unwrap(), "lease release exocortex transfer");
        assert!(!keeper.held && keeper.deadline(false).is_none());
    }

    #[test]
    fn the_badge_counts_unread_conversations_as_the_list_does() {
        let mut bridge = Bridge::new(|_| false);
        bridge.on_connected();
        assert_eq!(bridge.badge(), None, "unknown before the first page");
        // a and e count; b sits under a muted folder, c is muted itself, d
        // still streams, z is read.
        bridge.on_line(
            r#"{"type":"conversations_list","conversations":[
                {"id":"a","unread":true},
                {"id":"b","unread":true,"folderId":"inner"},
                {"id":"c","unread":true,"notificationsMuted":true},
                {"id":"d","unread":true,"streaming":true},
                {"id":"e","unread":true,"parentConversationId":"a"},
                {"id":"z","unread":false}],
              "folders":[{"id":"outer","muted":true},{"id":"inner","parentId":"outer"}],
              "sidebarPartial":{"remaining":9,"folderConversationCounts":{}}}"#
                .replace(['\n', ' '], "")
                .as_str(),
        );
        assert_eq!(bridge.badge(), Some(2));
        // Waiting in the global-idle queue is not unread.
        bridge.on_line(r#"{"type":"queue_updated","messages":[{"id":"q","convId":"e","text":"x","timing":"next-turn","source":"global-idle"}]}"#);
        assert_eq!(bridge.badge(), Some(1));
        bridge.on_line(r#"{"type":"conversation_updated","summary":{"id":"a","streaming":false,"unread":false}}"#);
        assert_eq!(bridge.badge(), Some(0));
        // The folder unmuted (a patch), then its conversation deleted.
        bridge.on_line(
            r#"{"type":"sidebar_state_patched","folders":[{"id":"outer","muted":false}]}"#,
        );
        assert_eq!(bridge.badge(), Some(1));
        bridge.on_line(r#"{"type":"sidebar_state_patched","removedConversationIds":["b"]}"#);
        assert_eq!(bridge.badge(), Some(0));
        bridge.on_line(&update("n", "", true));
        assert_eq!(bridge.badge(), Some(1), "a finished turn");
        bridge.on_line(r#"{"type":"conversation_deleted","convId":"n"}"#);
        assert_eq!(bridge.badge(), Some(0));
        bridge.on_disconnected();
        assert_eq!(bridge.badge(), Some(0), "kept while away");
        bridge.on_connected();
        assert_eq!(bridge.badge(), None);
    }

    #[test]
    fn the_badge_is_set_on_a_change_and_again_while_moon_is_away() {
        let start = Instant::now();
        let mut keeper = BadgeKeeper::new(start);
        let mut sent = Vec::new();
        let mut step = |keeper: &mut BadgeKeeper, want: Option<u32>, at: u64, up: bool| {
            keeper.step(want, start + Duration::from_secs(at), |command| {
                sent.push(command.to_owned());
                up
            });
        };
        step(&mut keeper, None, 0, true);
        step(&mut keeper, Some(3), 0, true);
        step(&mut keeper, Some(3), 1, true);
        assert!(keeper.deadline(Some(3)).is_none());
        // Moon away: again 5 s, then 15 s later.
        step(&mut keeper, Some(0), 2, false);
        step(&mut keeper, Some(1), 3, true);
        assert_eq!(
            keeper.deadline(Some(1)),
            Some(start + Duration::from_secs(7))
        );
        step(&mut keeper, Some(1), 7, false);
        assert_eq!(
            keeper.deadline(Some(1)),
            Some(start + Duration::from_secs(22))
        );
        step(&mut keeper, Some(1), 22, true);
        step(&mut keeper, Some(1), 23, true);
        assert_eq!(
            sent,
            [
                "badge exocortex 3",
                "badge exocortex 0",
                "badge exocortex 1",
                "badge exocortex 1"
            ]
        );
        assert!(keeper.deadline(Some(1)).is_none());
    }

    #[test]
    fn bodies_are_short_plain_text_or_the_error() {
        let long = "word ".repeat(200);
        let body = reply_body(&[Entry::Ai {
            blocks: vec![Block::Text { text: long }],
            metadata: None,
        }]);
        assert!(body.chars().count() <= BODY_CHARS && body.ends_with('…'));
        let error = reply_body(&[
            Entry::User {
                text: "q".into(),
                images: Vec::new(),
                metadata: None,
            },
            Entry::System {
                text: "Rate limited".into(),
                color: Some("error".into()),
                metadata: None,
            },
        ]);
        assert_eq!(error, "Rate limited");
        assert_eq!(reply_body(&[]), "Finished.");
        assert_eq!(clean_title("  a\n b "), "a b");
        assert_eq!(clean_title(""), "Exocortex");
    }
}
