//! Usage limits, Accounts, Connection and Token statistics (WP-E;
//! PAR-285–291, 295, 035; the iPhone's `ui/ops/usage`, `accounts`,
//! `tokens` and its connection card): what the daemon says about the
//! providers' windows, the accounts it routes between, the link to it, and
//! the tokens it counted. The usage rows also go into Settings.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::Value;

use super::super::model::{Action, Model, Nav, now_ms, ops};
use super::super::proto::{self, AccountsSnapshot, ClaudeAccount, Event, Usage, UsageWindow};
use super::super::requests::{self, Owner};
use super::super::theme;
use super::clock;
use super::panel::{self, Built, Row, Style, Tap};

/// Usage per provider (with when it came), the accounts snapshot, the
/// token statistics and the link's history.
#[derive(Debug, Default)]
pub struct State {
    pub usage: BTreeMap<String, (Option<Usage>, Instant)>,
    pub accounts: Option<AccountsSnapshot>,
    /// The Accounts panel watches (`accounts {watch}`).
    pub watching: bool,
    pub accounts_request: Option<String>,
    pub tokens: Option<Value>,
    pub tokens_request: Option<String>,
    pub link: Link,
}

/// The connection's history (Settings → Connection, diagnostics).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Link {
    pub connects: u32,
    pub drops: u32,
    /// When the current connection came up, and its last pong (ms).
    pub since: Option<u64>,
    pub last_pong: Option<u64>,
    /// `daemon_shutdown`'s mode, until the connection drops.
    pub shutdown: Option<String>,
    /// The daemon's network monitor: online, offline since (ms).
    pub online: Option<bool>,
    pub offline_since: Option<f64>,
}

/// After this, usage reads "LAST KNOWN".
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

pub fn provider_name(id: &str) -> String {
    match id {
        "openai" => "OpenAI".into(),
        "anthropic" => "Anthropic".into(),
        "deepseek" => "DeepSeek".into(),
        "opencode" => "OpenCode Zen".into(),
        "openrouter" => "OpenRouter".into(),
        _ => {
            let mut chars = id.chars();
            chars
                .next()
                .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

/// Accent above 20 % left, warning at 20 % or less, error at none.
pub fn usage_color(remaining: f64) -> u32 {
    let p = theme::p();
    if remaining <= 0.0 {
        p.error
    } else if remaining <= 20.0 {
        p.warning
    } else {
        p.accent
    }
}

/// A usage window as the iPhone shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowView {
    pub title: &'static str,
    /// Remaining share of capacity, 0–100.
    pub fraction: f64,
    /// "66% remaining", "140/200% remaining".
    pub label: String,
    pub color: u32,
    pub reset: String,
}

pub fn window_view(title: &'static str, w: &UsageWindow, capacity: f64, now: f64) -> WindowView {
    let cap = if capacity > 0.0 { capacity } else { 100.0 };
    let used = w.utilization.clamp(0.0, cap);
    let remaining = cap - used;
    let fraction = remaining / cap * 100.0;
    let shown = remaining.round();
    let future: Vec<f64> = w.resets.iter().copied().filter(|r| *r > now).collect();
    let reset = if w.resets.len() > 1 {
        if future.is_empty() {
            "Reset time passed · awaiting provider update".to_owned()
        } else {
            format!(
                "Resets in {}",
                future
                    .iter()
                    .take(2)
                    .map(|r| clock::format_short_time_until(*r, now))
                    .collect::<Vec<_>>()
                    .join(" · ")
            )
        }
    } else {
        match w.resets_at {
            None => "Reset time unavailable".into(),
            Some(at) if at <= now => "Reset time passed · awaiting provider update".into(),
            Some(at) => format!("Resets {}", clock::format_date_time(at)),
        }
    };
    WindowView {
        title,
        fraction,
        label: if (cap - 100.0).abs() > f64::EPSILON {
            format!("{shown}/{cap}% remaining")
        } else {
            format!("{shown}% remaining")
        },
        color: usage_color(fraction),
        reset,
    }
}

/// The 5-hour and weekly remaining shares, for the chat menu's line.
pub fn compact(usage: Option<&Usage>) -> Option<(Option<f64>, Option<f64>)> {
    let usage = usage?;
    if usage.five_hour.is_none() && usage.seven_day.is_none() {
        return None;
    }
    let capacity = usage.capacity.filter(|c| *c > 0.0).unwrap_or(100.0);
    let now = now_ms() as f64;
    let share = |w: &Option<UsageWindow>| {
        w.as_ref()
            .map(|w| window_view("5-hour", w, capacity, now).fraction)
    };
    Some((share(&usage.five_hour), share(&usage.seven_day)))
}

impl Model {
    pub fn usage_of(&self, provider: &str) -> Option<&Usage> {
        self.ops
            .usage
            .usage
            .get(provider)
            .and_then(|(usage, _)| usage.as_ref())
    }

    /// Several accounts are likely (the Accounts screen opens).
    pub fn accounts_likely(&self) -> bool {
        self.usage_of("anthropic")
            .and_then(|u| u.capacity)
            .is_some_and(|c| c > 100.0)
            || self.ops.usage.accounts.as_ref().is_some_and(|s| s.enabled)
    }

    /// The Accounts panel opens: watch while it shows.
    pub(crate) fn watch_accounts(&mut self) -> Vec<Value> {
        if !self.ready {
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Ops,
            "accounts",
            None,
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.ops.usage.watching = true;
        self.ops.usage.accounts_request = Some(request.clone());
        vec![proto::accounts(
            &request,
            Some(true),
            self.chat.id.as_deref(),
        )]
    }

    /// The Accounts panel went: stop watching.
    pub(crate) fn unwatch_accounts(&mut self) -> Vec<Value> {
        if !std::mem::take(&mut self.ops.usage.watching) || !self.connected {
            return Vec::new();
        }
        let request = self.requests.id();
        vec![proto::accounts(&request, Some(false), None)]
    }

    /// Ask for the token statistics (they are sent on demand).
    pub(crate) fn ask_token_stats(&mut self) -> Vec<Value> {
        if !self.ready || self.ops.usage.tokens_request.is_some() {
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Ops,
            "token_stats",
            None,
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.ops.usage.tokens_request = Some(request.clone());
        vec![proto::get_token_stats(&request)]
    }

    pub(crate) fn apply_usage(&mut self, event: &Event, now: Instant) {
        let link = &mut self.ops.usage.link;
        match event {
            Event::UsageUpdate(e) => {
                self.ops
                    .usage
                    .usage
                    .insert(e.provider.clone(), (e.usage.clone(), now));
            }
            Event::Accounts(e) => {
                if let Ok(snapshot) = serde_json::from_value::<AccountsSnapshot>(e.snapshot.clone())
                {
                    self.ops.usage.accounts = Some(snapshot);
                }
                if e.req_id.is_some() && e.req_id == self.ops.usage.accounts_request {
                    self.ops.usage.accounts_request = None;
                }
            }
            Event::TokenStats(e) => {
                self.ops.usage.tokens = Some(e.stats.clone());
                if e.req_id == self.ops.usage.tokens_request {
                    self.ops.usage.tokens_request = None;
                }
            }
            Event::Pong { .. } if self.ready => {
                let at = now_ms();
                link.last_pong = Some(at);
                link.since.get_or_insert(at);
            }
            Event::DaemonShutdown { mode } => {
                link.shutdown = Some(mode.clone().unwrap_or_else(|| "restart".into()));
            }
            Event::ConnectivityStatus {
                online,
                offline_since,
                ..
            } => {
                link.online = Some(*online);
                link.offline_since = *offline_since;
            }
            _ => {}
        }
    }

    pub(crate) fn usage_connected(&mut self) {
        let link = &mut self.ops.usage.link;
        link.connects += 1;
        link.since = None;
        link.shutdown = None;
    }

    pub(crate) fn usage_disconnected(&mut self) {
        let link = &mut self.ops.usage.link;
        if link.since.take().is_some() {
            link.drops += 1;
        }
        link.last_pong = None;
        self.ops.usage.watching = false;
        self.ops.usage.accounts_request = None;
        self.ops.usage.tokens_request = None;
    }
}

/// The usage rows (the Usage panel and Settings).
pub fn usage_rows(model: &Model, with_links: bool, now: Instant) -> Vec<Row> {
    let p = theme::p();
    let now_ms = now_ms() as f64;
    let mut rows = Vec::new();
    let waiting: Vec<_> = model
        .sidebar
        .conversations
        .values()
        .filter(|s| s.provider_waiting && s.streaming)
        .collect();
    if !waiting.is_empty() {
        rows.push(Row::Sub(format!(
            "Waiting for capacity · {}",
            waiting.len()
        )));
        for s in waiting.iter().take(6) {
            rows.push(Row::Task {
                label: "●".into(),
                color: p.warning,
                title: s.display_title().to_owned(),
                time: model.model_label(&proto::Selection {
                    provider: s.provider.clone(),
                    model: s.model.clone(),
                    ..Default::default()
                }),
                tap: Some(Tap::Close(Action::Nav(Nav::OpenConversation(s.id.clone())))),
                more: None,
            });
        }
    }
    let entries: Vec<_> = model
        .ops
        .usage
        .usage
        .iter()
        .filter(|(_, (usage, _))| usage.is_some())
        .collect();
    if entries.is_empty() {
        rows.push(panel::note(if model.ready {
            "Waiting for provider usage…"
        } else {
            "Connect to load usage"
        }));
    }
    for (index, (provider, (usage, at))) in entries.iter().enumerate() {
        let Some(usage) = usage else { continue };
        if index > 0 {
            rows.push(Row::Rule);
        }
        let capacity = usage.capacity.filter(|c| *c > 0.0).unwrap_or(100.0);
        let badge = if !model.ready {
            "  OFFLINE · LAST KNOWN"
        } else if now.duration_since(*at) > STALE_AFTER {
            "  LAST KNOWN"
        } else {
            ""
        };
        let accounts = if (capacity - 100.0).abs() > f64::EPSILON {
            format!("{} accounts", capacity / 100.0)
        } else {
            String::new()
        };
        rows.push(Row::Lead {
            left: format!("{}{badge}", provider_name(provider).to_uppercase()),
            right: accounts,
            style: Style::CaptionBold,
            color: p.muted,
        });
        let windows = [("5-hour", &usage.five_hour), ("Weekly", &usage.seven_day)];
        let mut any = false;
        for (title, window) in windows {
            let Some(window) = window else { continue };
            any = true;
            let view = window_view(title, window, capacity, now_ms);
            rows.push(Row::Gap(11));
            rows.push(Row::Pair {
                left: view.title.into(),
                right: view.label.clone(),
                style: Style::Body,
                color: view.color,
            });
            rows.push(Row::Bar {
                permille: (view.fraction * 10.0).round().clamp(0.0, 1000.0) as u16,
                color: view.color,
            });
            rows.push(panel::text(view.reset, Style::Meta, p.muted));
        }
        if !any {
            rows.push(panel::text(
                "Usage unavailable from this provider",
                Style::Meta,
                p.muted,
            ));
        }
    }
    if with_links {
        if !entries.is_empty() {
            rows.push(Row::Rule);
        }
        if model.accounts_likely() {
            rows.push(panel::nav(
                "Accounts",
                Some("Sign-in, limits and conversations per account".into()),
                None,
                Tap::Stay(Action::Ops(ops::Act::OpenAccounts)),
            ));
        }
        let detail = model.ops.usage.tokens.as_ref().map_or_else(
            || "Daily heatmap, models, cost".to_owned(),
            |stats| {
                format!(
                    "Today {} tokens · models, cost",
                    format_big(stats["today"]["totalTokens"].as_f64().unwrap_or(0.0))
                )
            },
        );
        rows.push(panel::nav(
            "Token statistics",
            Some(detail),
            None,
            Tap::Stay(Action::Ops(ops::Act::OpenTokens)),
        ));
    }
    rows
}

/// The Usage panel.
pub fn build(model: &Model, now: Instant) -> Built {
    Built {
        title: "Usage limits".into(),
        rows: usage_rows(model, true, now),
        wake: Some(now + std::time::Duration::from_secs(60)),
    }
}

/// "440M", "59.1B".
pub fn format_big(n: f64) -> String {
    if n >= 1e9 {
        if n >= 1e11 {
            format!("{:.0}B", n / 1e9)
        } else {
            format!("{:.1}B", n / 1e9)
        }
    } else {
        ops::format_tokens(n.max(0.0) as u64)
    }
}

fn usd(n: f64) -> String {
    if n >= 100.0 {
        format!("${n:.0}")
    } else if n >= 1.0 {
        format!("${n:.2}")
    } else if n > 0.0 {
        format!("${n:.3}")
    } else {
        "$0".into()
    }
}

fn totals_line(t: &Value) -> String {
    let n = |k: &str| t[k].as_f64().unwrap_or(0.0);
    format!(
        "{} tokens · {} in · {} out · {} requests",
        format_big(n("totalTokens")),
        format_big(n("inputTokens")),
        format_big(n("outputTokens")),
        ops::grouped(n("requests") as u64)
    )
}

fn cost(t: &Value) -> f64 {
    t["estimatedInputCostUsd"].as_f64().unwrap_or(0.0)
        + t["estimatedOutputCostUsd"].as_f64().unwrap_or(0.0)
}

/// The Token statistics panel (PAR-295, without the heatmap).
pub fn tokens(model: &Model) -> Built {
    let p = theme::p();
    let mut rows = Vec::new();
    match &model.ops.usage.tokens {
        None => rows.push(panel::note(if model.ready {
            "Asking the daemon for its token statistics…"
        } else {
            "Connect to load token statistics."
        })),
        Some(stats) => {
            rows.push(panel::section("Today"));
            rows.push(panel::kv("Tokens", totals_line(&stats["today"])));
            rows.push(panel::kv("Est. cost", usd(cost(&stats["today"]))));
            let life = &stats["lifetime"];
            rows.push(panel::section("Lifetime"));
            rows.push(panel::kv("Tokens", totals_line(life)));
            rows.push(panel::kv("Est. cost", usd(cost(life))));
            rows.push(panel::kv(
                "Active days",
                stats["days"].as_array().map_or(0, Vec::len).to_string(),
            ));
            for (title, key) in [
                ("Models", "byModel"),
                ("Providers", "byProvider"),
                ("Sources", "bySource"),
            ] {
                let Some(map) = life[key].as_object() else {
                    continue;
                };
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|a, b| {
                    let n = |v: &Value| v["totalTokens"].as_f64().unwrap_or(0.0);
                    n(b.1).total_cmp(&n(a.1))
                });
                rows.push(panel::section(title));
                for (name, t) in entries.into_iter().take(12) {
                    let label = if key == "byProvider" {
                        provider_name(name)
                    } else {
                        name.replace('_', " ")
                    };
                    rows.push(Row::Kv {
                        key: label,
                        value: format!(
                            "{} · {}",
                            format_big(t["totalTokens"].as_f64().unwrap_or(0.0)),
                            usd(cost(t))
                        ),
                        color: p.text,
                    });
                }
            }
            if let Some(at) = stats["updatedAt"].as_f64() {
                rows.push(panel::text(
                    format!(
                        "Recorded by the daemon · updated {}",
                        clock::format_date_time(at)
                    ),
                    Style::Meta,
                    p.muted,
                ));
            }
        }
    }
    Built {
        title: "Token statistics".into(),
        rows,
        wake: None,
    }
}

fn account_usage(usage: Option<&Usage>, now: f64) -> String {
    let cell = |label: &str, w: &Option<UsageWindow>| {
        w.as_ref().map(|w| {
            let pct = match w.resets_at {
                Some(at) if at <= now => 0,
                _ => w.utilization.round() as i64,
            };
            let reset = w
                .resets_at
                .filter(|at| *at > now)
                .map(|at| format!(" {}", clock::format_account_duration(at - now)))
                .unwrap_or_default();
            format!("{label} {pct}%{reset}")
        })
    };
    let usage = usage.cloned().unwrap_or_default();
    [cell("5h", &usage.five_hour), cell("wk", &usage.seven_day)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn near_limit(account: &ClaudeAccount, floors: (f64, f64), now: f64) -> Option<&'static str> {
    let pct = |w: &Option<UsageWindow>| {
        w.as_ref().map(|w| match w.resets_at {
            Some(at) if at <= now => 0.0,
            _ => w.utilization.round(),
        })
    };
    let usage = account.usage.as_ref()?;
    if pct(&usage.seven_day).is_some_and(|w| 100.0 - w < floors.1) {
        return Some("weekly");
    }
    if pct(&usage.five_hour).is_some_and(|f| 100.0 - f < floors.0) {
        return Some("5-hour");
    }
    None
}

/// The Accounts panel (PAR-288–291, the TUI's accounts screen).
pub fn accounts(model: &Model, _conv: Option<&str>, _now: Instant) -> Built {
    let p = theme::p();
    let now = now_ms() as f64;
    let mut rows = Vec::new();
    let Some(snapshot) = &model.ops.usage.accounts else {
        rows.push(panel::note(if model.ready {
            "Loading accounts…"
        } else {
            "Connect to load accounts."
        }));
        return Built {
            title: "Accounts".into(),
            rows,
            wake: None,
        };
    };
    let claude = &snapshot.claude;
    if claude.proxied {
        rows.push(panel::section("Claude"));
        rows.push(panel::note("The remote daemon's login (/proxy)"));
    } else if !claude.accounts.is_empty() {
        rows.push(panel::section("Claude"));
        let floors = (claude.routing.five_hour_floor, claude.routing.weekly_floor);
        let mut notes = Vec::new();
        for account in &claude.accounts {
            let current = claude
                .current_conversation
                .as_ref()
                .and_then(|c| c.account.as_deref())
                == Some(account.id.as_str());
            let (dot, note) = match account.state.as_str() {
                "limited" => (
                    p.warning,
                    Some(match account.limited_until.filter(|u| *u > now) {
                        Some(until) => {
                            format!("limited {}", clock::format_account_duration(until - now))
                        }
                        None => "limited".into(),
                    }),
                ),
                "blocked" => (
                    p.error,
                    Some(format!(
                        "blocked{}{}",
                        account
                            .state_detail
                            .as_ref()
                            .map(|d| format!(": {d}"))
                            .unwrap_or_default(),
                        account
                            .blocked_until
                            .filter(|u| *u > now)
                            .map(|u| format!(
                                " · retry in {}",
                                clock::format_account_duration(u - now)
                            ))
                            .unwrap_or_default()
                    )),
                ),
                "signed_out" => (p.error, Some("signed out".into())),
                "not_subscription" => (p.warning, Some("not a subscription".into())),
                "unchecked" => (p.muted, Some("checking…".into())),
                _ => (p.success, None),
            };
            let mut tags = Vec::new();
            if let Some(note) = &note {
                tags.push(note.clone());
            }
            if claude
                .next_new_work
                .as_ref()
                .is_some_and(|n| n.account == account.id)
            {
                tags.push("new work".into());
            }
            let usage = if matches!(account.state.as_str(), "ready" | "limited") {
                account_usage(account.usage.as_ref(), now)
            } else {
                String::new()
            };
            rows.push(Row::Task {
                label: format!("● {}", account.id),
                color: if current { p.accent } else { dot },
                title: tags.join(" · "),
                time: usage,
                tap: None,
                more: None,
            });
            if let Some(command) = &account.login_command {
                notes.push(format!("sign in {}: {command}", account.id));
            }
        }
        match &claude.next_new_work {
            None => notes.insert(0, "no account can take new work".into()),
            Some(next) => {
                let chosen = claude.accounts.iter().find(|a| a.id == next.account);
                if chosen.is_some_and(|a| near_limit(a, floors, now).is_some()) {
                    notes.insert(
                        0,
                        format!(
                            "every account is near a limit → new work goes to {}",
                            next.account
                        ),
                    );
                } else {
                    let skipped: Vec<&ClaudeAccount> = claude
                        .accounts
                        .iter()
                        .filter(|a| a.id != next.account && a.state == "ready")
                        .filter(|a| near_limit(a, floors, now).is_some())
                        .collect();
                    if let [one] = skipped.as_slice() {
                        notes.insert(
                            0,
                            format!(
                                "{} is near its {} limit → new work goes to {}",
                                one.id,
                                near_limit(one, floors, now).unwrap_or("usage"),
                                next.account
                            ),
                        );
                    } else if !skipped.is_empty() {
                        let ids: Vec<&str> = skipped.iter().map(|a| a.id.as_str()).collect();
                        notes.insert(
                            0,
                            format!(
                                "{} are near their limits → new work goes to {}",
                                ids.join(", "),
                                next.account
                            ),
                        );
                    }
                }
            }
        }
        if let Some(remote) = &claude.remote {
            notes.insert(
                0,
                format!("accounts on {} (/proxy) · {}", remote.alias, remote.state),
            );
        }
        for note in notes {
            rows.push(panel::text(note, Style::Caption, p.muted));
        }
    }
    let openai = &snapshot.openai;
    if !openai.accounts.is_empty() {
        rows.push(panel::section("OpenAI"));
        if openai.proxied {
            rows.push(panel::note("The remote daemon's login (/proxy)"));
        }
        for account in &openai.accounts {
            let name = account
                .email
                .clone()
                .or_else(|| account.display_name.clone())
                .unwrap_or_else(|| format!("account {}", account.index));
            rows.push(Row::Task {
                label: format!("● {name}"),
                color: if account.current { p.accent } else { p.muted },
                title: account.plan.clone().unwrap_or_default(),
                time: if account.current {
                    account_usage(model.usage_of("openai"), now)
                } else {
                    String::new()
                },
                tap: None,
                more: None,
            });
        }
    }
    if snapshot.generated_at > 0.0 {
        rows.push(panel::text(
            format!("Updated {}", clock::format_date_time(snapshot.generated_at)),
            Style::Meta,
            p.muted,
        ));
    }
    Built {
        title: "Accounts".into(),
        rows,
        wake: None,
    }
}

/// The connection's label and colour (the card, the inspector).
pub fn connection_state(model: &Model) -> (&'static str, u32) {
    let p = theme::p();
    if model.ready {
        ("Connected", p.success)
    } else if model.connected {
        ("Syncing…", p.warning)
    } else if model.was_connected {
        ("Reconnecting…", p.warning)
    } else {
        ("Connecting…", p.warning)
    }
}

/// The connection rows (Settings → Connection and the diagnostics panel).
pub fn connection_rows(model: &Model, detailed: bool) -> Vec<Row> {
    let p = theme::p();
    let link = &model.ops.usage.link;
    let (label, color) = connection_state(model);
    let mut card = vec![(label.to_uppercase(), Style::CaptionBold, p.text)];
    let socket = proto::socket_path().display().to_string();
    card.push((format!("local daemon · {socket}"), Style::Meta, p.muted));
    if let Some(mode) = &link.shutdown {
        card.push((format!("Daemon {mode}ing…"), Style::Meta, p.warning));
    }
    if link.online == Some(false) {
        card.push((
            "⚠ offline: the daemon cannot reach the network".into(),
            Style::Meta,
            p.warning,
        ));
    }
    let mut rows = vec![Row::Rail { color, lines: card }];
    if let Some(since) = link.since {
        let at = clock::format_due_time(since as f64, now_ms() as f64);
        rows.push(panel::kv(
            "Up since",
            if detailed {
                format!(
                    "{at} ({})",
                    clock::format_task_elapsed(since as f64, now_ms() as f64)
                )
            } else {
                at
            },
        ));
    }
    if detailed {
        rows.push(panel::kv("Socket", socket));
        rows.push(panel::kv(
            "Last pong",
            link.last_pong.map_or_else(
                || "—".into(),
                |at| clock::format_due_time(at as f64, now_ms() as f64),
            ),
        ));
        rows.push(panel::kv("Connections", link.connects.to_string()));
        rows.push(panel::kv("Drops", link.drops.to_string()));
        rows.push(panel::kv(
            "Network",
            match link.online {
                Some(true) => "online".into(),
                Some(false) => match link.offline_since {
                    Some(at) => format!(
                        "offline since {}",
                        clock::format_due_time(at, now_ms() as f64)
                    ),
                    None => "offline".into(),
                },
                None => "unknown".into(),
            },
        ));
        rows.push(panel::kv(
            "Requests waiting",
            model.requests.len().to_string(),
        ));
    } else {
        rows.push(panel::nav(
            "Connection diagnostics",
            Some(format!(
                "{} drop{} · {} reconnect{} since launch",
                link.drops,
                if link.drops == 1 { "" } else { "s" },
                link.connects.saturating_sub(1),
                if link.connects == 2 { "" } else { "s" }
            )),
            None,
            Tap::Stay(Action::Ops(ops::Act::OpenConnection)),
        ));
    }
    rows
}

/// The Connection diagnostics panel.
pub fn connection(model: &Model, now: Instant) -> Built {
    Built {
        title: "Connection".into(),
        rows: connection_rows(model, true),
        wake: Some(now + std::time::Duration::from_secs(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;

    #[test]
    fn windows_read_and_colour_as_on_the_iphone() {
        let now = 1_000_000.0;
        let w = UsageWindow {
            utilization: 21.0,
            resets_at: Some(now + 3_600_000.0),
            resets: vec![now + 4_500_000.0, now + 14_700_000.0],
        };
        let v = window_view("5-hour", &w, 200.0, now);
        assert_eq!(v.label, "179/200% remaining");
        assert_eq!(v.reset, "Resets in 1h15m · 4h05m");
        assert_eq!(v.color, theme::p().accent);
        let at = |used: f64| {
            window_view(
                "5-hour",
                &UsageWindow {
                    utilization: used,
                    ..Default::default()
                },
                100.0,
                now,
            )
        };
        assert_eq!(at(79.0).color, theme::p().accent, "21 % left");
        assert_eq!(at(80.0).color, theme::p().warning, "20 % left");
        assert_eq!(at(100.0).color, theme::p().error, "none left");
        assert_eq!(at(100.0).reset, "Reset time unavailable");
    }

    #[test]
    fn the_accounts_watch_starts_and_stops() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        let sent = model.watch_accounts();
        assert_eq!(sent[0]["watch"], true);
        let id = sent[0]["reqId"].as_str().unwrap().to_owned();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"accounts","reqId":"{id}","snapshot":{{"generatedAt":1,"enabled":true,"claude":{{"proxied":false,"accounts":[{{"id":"main","state":"ready","configDir":"~/.claude","activeTurns":0,"usage":{{"fiveHour":{{"utilization":12,"resetsAt":null}}}}}}],"routing":{{"fiveHourFloor":5,"weeklyFloor":5}},"nextNewWork":{{"account":"main","detail":""}}}},"openai":{{"proxied":false,"accounts":[]}}}}}}"#
            )),
            Instant::now(),
        );
        let built = accounts(&model, None, Instant::now());
        assert!(built.rows.iter().any(
            |r| matches!(r, Row::Task { label, time, .. } if label == "● main" && time == "5h 12%")
        ));
        let stop = model.unwatch_accounts();
        assert_eq!(stop[0]["watch"], false);
        assert!(model.unwatch_accounts().is_empty(), "once");
    }
}
