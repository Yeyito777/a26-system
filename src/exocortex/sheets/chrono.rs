//! Chrono schedules (WP-E; PAR-303, 304; the iPhone's `ui/ops/chrono`):
//! the conversation's schedules with cancel, and a new one written in the
//! `/chrono` grammar (ported from the TUI's `chronoschedule.ts` through
//! the iPhone's `state/ops/chrono.ts`, same semantics; times are local).
//! D's `/chrono` command can use `parse` too.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::{Value, json};

use super::super::model::{Action, Model, UiAct, now_ms, ops};
use super::super::proto::{self, ChronoSchedule, Event};
use super::super::requests::{self, Owner};
use super::super::theme;
use super::super::ui::menu::Confirm;
use super::super::ui::sheet::{ActionItem, ActionList, Prompt, Tone, action_sheet, prompt_sheet};
use super::clock::{self, Local};
use super::panel::{self, Btn, Built, Kind, Row, Style, Tap};

pub const USAGE: &str = "Usage: /chrono <when> <message, or !command to run in the shell>\n  \
once:   30m · in 2h 30m · at 9am · tomorrow 14:30 · fri 5pm · 2026-12-24 18:00\n  \
repeat: every 2h · every day at 9am · every weekday at 8:30 · every mon,thu at 18:00\n\
/chrono [list] shows this conversation's schedules · /chrono cancel <id|all>";

const TEAL: u32 = 0x4ec9b0;
const COMMAND: u32 = 0xd19a66;

/// A conversation's schedules as the daemon last listed them.
#[derive(Debug, Default)]
pub struct State {
    pub lists: HashMap<String, Vec<ChronoSchedule>>,
    /// `chrono` requests in flight, by reqId: their conversation.
    pub busy: HashMap<String, String>,
    pub errors: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    Refresh,
    /// A schedule's actions.
    More(String),
    Cancel(String),
    CancelConfirmed(String),
    /// "New schedule…": the grammar prompt.
    New,
    Create(String),
}

fn act(a: Act) -> Action {
    Action::Ops(ops::Act::Chrono(a))
}

/// What a `/chrono` line asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    List,
    Cancel(String),
    Create {
        at: f64,
        repeat: Option<Value>,
        message: Option<String>,
        command: Option<String>,
    },
}

// ----- The grammar ------------------------------------------------------------------

struct Token {
    lower: String,
    end: usize,
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(index),
            (true, Some(s)) => {
                tokens.push(Token {
                    lower: text[s..index].to_lowercase(),
                    end: index,
                });
                start = None;
            }
            _ => {}
        }
    }
    tokens
}

const DAY_MS: f64 = 86_400_000.0;

fn weekday(name: &str) -> Option<u32> {
    Some(match name {
        "sun" | "sunday" => 0,
        "mon" | "monday" => 1,
        "tue" | "tues" | "tuesday" => 2,
        "wed" | "wednesday" => 3,
        "thu" | "thur" | "thurs" | "thursday" => 4,
        "fri" | "friday" => 5,
        "sat" | "saturday" => 6,
        _ => return None,
    })
}

/// `2h30m`, `90s`, `1.5h` → ms (`shared/duration.ts`).
fn duration_token(token: &str) -> Option<f64> {
    let mut rest = token;
    let mut total = 0.0;
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let number = rest
            .char_indices()
            .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
            .count();
        if number == 0 {
            return None;
        }
        let amount: f64 = rest[..number].parse().ok()?;
        rest = &rest[number..];
        let (unit, ms) = if rest.starts_with("ms") {
            ("ms", 1.0)
        } else if rest.starts_with('s') {
            ("s", 1000.0)
        } else if rest.starts_with('m') {
            ("m", 60_000.0)
        } else if rest.starts_with('h') {
            ("h", 3_600_000.0)
        } else if rest.starts_with('d') {
            ("d", DAY_MS)
        } else {
            return None;
        };
        rest = &rest[unit.len()..];
        total += amount * ms;
    }
    (total.round() > 0.0).then_some(total.round())
}

fn read_duration(tokens: &[Token], i: usize) -> Option<(f64, usize)> {
    let mut ms = 0.0;
    let mut next = i;
    while let Some(part) = tokens.get(next).and_then(|t| duration_token(&t.lower)) {
        ms += part;
        next += 1;
    }
    (next > i).then_some((ms, next))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Clock {
    hour: u32,
    minute: u32,
    second: u32,
}

fn read_clock(tokens: &[Token], i: usize) -> Option<(Clock, usize)> {
    let token = tokens.get(i)?.lower.as_str();
    match token {
        "noon" => {
            return Some((
                Clock {
                    hour: 12,
                    minute: 0,
                    second: 0,
                },
                i + 1,
            ));
        }
        "midnight" => {
            return Some((
                Clock {
                    hour: 0,
                    minute: 0,
                    second: 0,
                },
                i + 1,
            ));
        }
        _ => {}
    }
    let digits = token.chars().take_while(char::is_ascii_digit).count();
    if !(1..=2).contains(&digits) {
        return None;
    }
    let mut hour: u32 = token[..digits].parse().ok()?;
    let mut rest = &token[digits..];
    let part = |rest: &mut &str| -> Option<Option<u32>> {
        if let Some(after) = rest.strip_prefix(':') {
            if after.len() < 2 || !after[..2].bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let value = after[..2].parse().ok()?;
            *rest = &after[2..];
            Some(Some(value))
        } else {
            Some(None)
        }
    };
    let minute = part(&mut rest)?;
    let second = if minute.is_some() {
        part(&mut rest)?
    } else {
        None
    };
    let mut meridiem = match rest {
        "" => None,
        "am" | "a" => Some(false),
        "pm" | "p" => Some(true),
        _ => return None,
    };
    let mut next = i + 1;
    if meridiem.is_none() {
        match tokens.get(next).map(|t| t.lower.as_str()) {
            Some("am") => {
                meridiem = Some(false);
                next += 1;
            }
            Some("pm") => {
                meridiem = Some(true);
                next += 1;
            }
            _ => {}
        }
    }
    // A bare number is prose, not a time: 9:00, 9am or 9 pm.
    if meridiem.is_none() && minute.is_none() {
        return None;
    }
    let (minute, second) = (minute.unwrap_or(0), second.unwrap_or(0));
    if minute > 59 || second > 59 {
        return None;
    }
    match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            hour = hour % 12 + if pm { 12 } else { 0 };
        }
        None if hour > 23 => return None,
        None => {}
    }
    Some((
        Clock {
            hour,
            minute,
            second,
        },
        next,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Day {
    Today,
    Tomorrow,
    Weekday(u32),
    Date(i32, u32, u32),
}

fn read_day(tokens: &[Token], i: usize) -> Result<Option<(Day, usize)>, String> {
    let Some(token) = tokens.get(i).map(|t| t.lower.as_str()) else {
        return Ok(None);
    };
    match token {
        "today" => return Ok(Some((Day::Today, i + 1))),
        "tomorrow" => return Ok(Some((Day::Tomorrow, i + 1))),
        _ => {}
    }
    if let Some(day) = weekday(token) {
        return Ok(Some((Day::Weekday(day), i + 1)));
    }
    let Some(date) = parse_date(token) else {
        return Ok(None);
    };
    let (year, month, day) = date;
    let check = clock::local(clock::from_local(year, month as i32, day as i32, 12, 0, 0));
    if (check.year, check.month, check.day) != (year, month, day) {
        return Err(format!("Not a date: {token}"));
    }
    Ok(Some((Day::Date(year, month, day), i + 1)))
}

fn parse_date(token: &str) -> Option<(i32, u32, u32)> {
    let b = token.as_bytes();
    if token.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    Some((
        token[..4].parse().ok()?,
        token[5..7].parse().ok()?,
        token[8..10].parse().ok()?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct When {
    day: Option<Day>,
    clock: Option<Clock>,
    instant: Option<f64>,
}

fn read_when(tokens: &[Token], i: usize) -> Result<Option<(When, usize)>, String> {
    if let Some(token) = tokens.get(i).map(|t| t.lower.as_str())
        && let Some((date, time)) = token.split_once('t')
        && let Some((year, month, day)) = parse_date(date)
    {
        // An ISO-8601 local date-time.
        let fields: Vec<&str> = time.split(':').collect();
        let number = |s: Option<&&str>| s.and_then(|s| s.parse::<u32>().ok());
        let (Some(hour), Some(minute)) = (number(fields.first()), number(fields.get(1))) else {
            return Err(format!("Not a date/time: {token}"));
        };
        let second = number(fields.get(2)).unwrap_or(0);
        let instant = clock::from_local(year, month as i32, day as i32, hour, minute, second);
        return Ok(Some((
            When {
                day: None,
                clock: None,
                instant: Some(instant),
            },
            i + 1,
        )));
    }
    let day = read_day(tokens, i)?;
    let clock = read_clock(tokens, day.map_or(i, |(_, next)| next));
    match (day, clock) {
        (None, None) => Ok(None),
        (Some(_), None) => Err("Add a time to the day, such as tomorrow 9am or fri 17:30.".into()),
        (day, Some((clock, next))) => Ok(Some((
            When {
                day: day.map(|(d, _)| d),
                clock: Some(clock),
                instant: None,
            },
            next,
        ))),
    }
}

fn at_local(base: Local, offset: i32, clock: Clock) -> f64 {
    clock::from_local(
        base.year,
        base.month as i32,
        base.day as i32 + offset,
        clock.hour,
        clock.minute,
        clock.second,
    )
}

fn resolve(when: When, now: f64) -> Result<f64, String> {
    if let Some(instant) = when.instant {
        return if instant <= now {
            Err("That time has already passed.".into())
        } else {
            Ok(instant)
        };
    }
    let clock = when.clock.ok_or("Could not resolve that time.")?;
    let today = clock::local(now);
    match when.day {
        None => {
            let at = at_local(today, 0, clock);
            Ok(if at > now {
                at
            } else {
                at_local(today, 1, clock)
            })
        }
        Some(Day::Today) => {
            let at = at_local(today, 0, clock);
            if at <= now {
                Err(format!(
                    "Today at {:02}:{:02} has already passed.",
                    clock.hour, clock.minute
                ))
            } else {
                Ok(at)
            }
        }
        Some(Day::Tomorrow) => Ok(at_local(today, 1, clock)),
        Some(Day::Weekday(day)) => (0..=7)
            .map(|offset| at_local(today, offset, clock))
            .find(|at| *at > now && clock::local(*at).weekday == day)
            .ok_or_else(|| "Could not resolve that time.".into()),
        Some(Day::Date(year, month, day)) => {
            let at = clock::from_local(
                year,
                month as i32,
                day as i32,
                clock.hour,
                clock.minute,
                clock.second,
            );
            if at <= now {
                Err("That time has already passed.".into())
            } else {
                Ok(at)
            }
        }
    }
}

enum Period {
    Fixed(f64),
    Calendar(&'static str, u32),
    Weekdays(Vec<u32>),
}

fn read_period(tokens: &[Token], i: usize) -> Result<(Period, usize), String> {
    if let Some((ms, next)) = read_duration(tokens, i) {
        if ms % DAY_MS == 0.0 {
            return Ok((Period::Calendar("day", (ms / DAY_MS) as u32), next));
        }
        if ms % 60_000.0 != 0.0 {
            return Err("Repeat periods must be whole minutes.".into());
        }
        return Ok((Period::Fixed(ms), next));
    }
    let token = tokens.get(i).map_or("", |t| t.lower.as_str());
    match token {
        "weekday" | "weekdays" => return Ok((Period::Weekdays(vec![1, 2, 3, 4, 5]), i + 1)),
        "weekend" | "weekends" => return Ok((Period::Weekdays(vec![6, 0]), i + 1)),
        _ => {}
    }
    let mut days = Vec::new();
    let mut after = i;
    while let Some(token) = tokens.get(after) {
        let names: Vec<&str> = token.lower.split(',').filter(|s| !s.is_empty()).collect();
        if names.is_empty() || !names.iter().all(|n| weekday(n).is_some()) {
            break;
        }
        for name in names {
            let day = weekday(name).unwrap_or(0);
            if !days.contains(&day) {
                days.push(day);
            }
        }
        after += 1;
    }
    if !days.is_empty() {
        return Ok((Period::Weekdays(days), after));
    }
    let mut next = i;
    let mut interval = 1;
    if !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()) {
        interval = token.parse().unwrap_or(1);
        next += 1;
    }
    let unit = tokens.get(next).map_or("", |t| t.lower.as_str());
    let unit = unit.strip_suffix('s').unwrap_or(unit);
    if interval >= 1 {
        match unit {
            "minute" | "min" => return Ok((Period::Fixed(interval as f64 * 60_000.0), next + 1)),
            "hour" => return Ok((Period::Fixed(interval as f64 * 3_600_000.0), next + 1)),
            "day" => return Ok((Period::Calendar("day", interval), next + 1)),
            "week" => return Ok((Period::Calendar("week", interval), next + 1)),
            "month" => return Ok((Period::Calendar("month", interval), next + 1)),
            _ => {}
        }
    }
    Err("Say how often, such as every 2h, every day, every weekday, or every mon,thu.".into())
}

fn alias(token: &str) -> Option<&'static str> {
    Some(match token {
        "hourly" => "hour",
        "daily" => "day",
        "weekly" => "week",
        "monthly" => "month",
        _ => return None,
    })
}

fn add_calendar(now: f64, unit: &str, interval: u32) -> f64 {
    let t = clock::local(now);
    let (month, day) = match unit {
        "month" => (t.month as i32 + interval as i32, t.day as i32),
        "week" => (t.month as i32, t.day as i32 + 7 * interval as i32),
        _ => (t.month as i32, t.day as i32 + interval as i32),
    };
    clock::from_local(t.year, month, day, t.hour, t.minute, t.second)
}

fn interval_repeat(unit: &str, interval: u32) -> Value {
    if interval == 1 {
        json!({ "unit": unit })
    } else {
        json!({ "unit": unit, "interval": interval })
    }
}

fn repeat(tokens: &[Token], now: f64) -> Result<(f64, Value, usize), String> {
    let (period, mut next) = match alias(&tokens[0].lower) {
        Some(unit) => (Period::Calendar(unit, 1), 1),
        None => read_period(tokens, 1)?,
    };
    let has_at = tokens.get(next).is_some_and(|t| t.lower == "at");
    let when = read_when(tokens, if has_at { next + 1 } else { next })?;
    if has_at && when.is_none() {
        return Err("Add a time after at, such as at 9am or at fri 17:30.".into());
    }
    if let Some((_, after)) = when {
        next = after;
    }
    let when = when.map(|(w, _)| w);
    match period {
        Period::Fixed(ms) => {
            let repeat = if ms % 3_600_000.0 == 0.0 {
                interval_repeat("hour", (ms / 3_600_000.0) as u32)
            } else {
                interval_repeat("minute", (ms / 60_000.0) as u32)
            };
            let at = match when {
                Some(w) => resolve(w, now)?,
                None => now + ms,
            };
            Ok((at, repeat, next))
        }
        Period::Calendar(unit, interval) => {
            let at = match when {
                Some(w) => resolve(w, now)?,
                None => add_calendar(now, unit, interval),
            };
            Ok((at, interval_repeat(unit, interval), next))
        }
        Period::Weekdays(mut days) => {
            if when.is_some_and(|w| w.day.is_some() || w.instant.is_some()) {
                return Err(
                    "Give only a time with weekdays, such as every mon,thu at 18:00.".into(),
                );
            }
            let today = clock::local(now);
            let time = when.and_then(|w| w.clock).unwrap_or(Clock {
                hour: today.hour,
                minute: today.minute,
                second: today.second,
            });
            let at = (0..=7)
                .map(|offset| at_local(today, offset, time))
                .find(|at| *at > now && days.contains(&clock::local(*at).weekday))
                .unwrap_or(0.0);
            days.sort_unstable();
            const NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
            let names: Vec<&str> = days.iter().map(|d| NAMES[*d as usize % 7]).collect();
            Ok((at, json!({ "unit": "week", "weekdays": names }), next))
        }
    }
}

fn once(tokens: &[Token], now: f64) -> Result<(f64, usize), String> {
    let first = tokens[0].lower.as_str();
    let start = usize::from(first == "in" || first == "at");
    if first != "at" {
        if let Some((ms, next)) = read_duration(tokens, start) {
            return Ok((now + ms, next));
        }
        if first == "in" {
            return Err("Add a delay after in, such as in 30m or in 2h 30m.".into());
        }
    }
    match read_when(tokens, start)? {
        Some((when, next)) => Ok((resolve(when, now)?, next)),
        None => Err(USAGE.into()),
    }
}

/// The text after `/chrono`, read at `now` (ms).
pub fn parse(text: &str, now: f64) -> Result<Request, String> {
    let tokens = tokenize(text);
    let Some(first) = tokens.first().map(|t| t.lower.as_str()) else {
        return Ok(Request::List);
    };
    if first == "list" && tokens.len() == 1 {
        return Ok(Request::List);
    }
    if first == "cancel" {
        if tokens.len() != 2 {
            return Err("Usage: /chrono cancel <id|all>".into());
        }
        let id = text.split_whitespace().nth(1).unwrap_or_default();
        return Ok(Request::Cancel(id.to_owned()));
    }
    let (at, repeat, next) = if first == "every" || alias(first).is_some() {
        let (at, repeat, next) = repeat(&tokens, now)?;
        (at, Some(repeat), next)
    } else {
        let (at, next) = once(&tokens, now)?;
        (at, None, next)
    };
    let from = if next > 0 { tokens[next - 1].end } else { 0 };
    let payload = text[from..].trim();
    if payload.is_empty() {
        return Err("Add a message to send, or !command to run in the shell.".into());
    }
    let (message, command) = match payload.strip_prefix('!') {
        Some(command) if command.trim().is_empty() => {
            return Err("Add a shell command after !.".into());
        }
        Some(command) => (None, Some(command.trim().to_owned())),
        None => (Some(payload.to_owned()), None),
    };
    Ok(Request::Create {
        at,
        repeat,
        message,
        command,
    })
}

// ----- Display ------------------------------------------------------------------------

pub fn short_id(id: &str) -> String {
    id.trim_start_matches("chrono:").chars().take(8).collect()
}

/// "14:30 (in 2h)", "Oct 18 09:00 (in 15h)", "… (due now)".
pub fn format_due(next_at: f64, now: f64) -> String {
    let remaining = next_at - now;
    let relative = if remaining <= 0.0 {
        "due now".to_owned()
    } else {
        let rounded = if remaining >= 3_600_000.0 {
            (remaining / 60_000.0).round() * 60_000.0
        } else {
            remaining
        };
        format!("in {}", clock::format_goal_duration(rounded as u64))
    };
    format!("{} ({relative})", clock::format_due_time(next_at, now))
}

/// "every 2h", "every day at 09:00", "every weekday at 08:30", "once".
pub fn repeat_label(schedule: &ChronoSchedule) -> String {
    let Some(r) = &schedule.recurrence else {
        return "once".into();
    };
    match r["kind"].as_str() {
        Some("interval") => format!(
            "every {}",
            clock::format_goal_duration(r["everyMs"].as_f64().unwrap_or(0.0) as u64)
        ),
        Some("cron") => format!("cron {}", r["expression"].as_str().unwrap_or("")),
        Some("calendar") => {
            let interval = r["interval"].as_u64().unwrap_or(1);
            let every = |unit: &str| {
                if interval == 1 {
                    format!("every {unit}")
                } else {
                    format!("every {interval} {unit}s")
                }
            };
            let second = r["second"].as_u64().unwrap_or(0);
            let at = format!(
                "at {:02}:{:02}{}",
                r["hour"].as_u64().unwrap_or(0),
                r["minute"].as_u64().unwrap_or(0),
                if second > 0 {
                    format!(":{second:02}")
                } else {
                    String::new()
                }
            );
            match r["unit"].as_str() {
                Some("day") => format!("{} {at}", every("day")),
                Some("month") => format!(
                    "{} on the {} {at}",
                    every("month"),
                    ordinal(r["dayOfMonth"].as_u64().unwrap_or(1))
                ),
                _ => {
                    let mut days: Vec<u64> = r["weekdays"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_u64).collect())
                        .unwrap_or_default();
                    days.sort_unstable();
                    const LABELS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
                    let list = if days == [1, 2, 3, 4, 5] {
                        "weekday".to_owned()
                    } else if days == [0, 6] {
                        "weekend day".to_owned()
                    } else {
                        days.iter()
                            .map(|d| LABELS[*d as usize % 7])
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    if interval == 1 {
                        format!("every {list} {at}")
                    } else {
                        format!("{} on {list} {at}", every("week"))
                    }
                }
            }
        }
        _ => "repeats".into(),
    }
}

fn ordinal(day: u64) -> String {
    let suffix = match (day % 100, day % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

pub fn payload_label(schedule: &ChronoSchedule, max: usize) -> String {
    let text = if schedule.kind == "command" {
        format!("!{}", schedule.payload)
    } else {
        schedule.payload.clone()
    };
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max - 1).collect::<String>())
    } else {
        line
    }
}

/// What a `chrono_result` says (a toast).
pub fn result_text(
    created: Option<&ChronoSchedule>,
    cancelled: &[Value],
    now: f64,
) -> Option<String> {
    if let Some(s) = created {
        return Some(format!(
            "Scheduled {} {} · {}",
            short_id(&s.id),
            format_due(s.next_at, now),
            repeat_label(s)
        ));
    }
    match cancelled {
        [] => None,
        [one] => Some(format!(
            "Cancelled {} · {}",
            short_id(one["id"].as_str().unwrap_or("")),
            one["title"].as_str().unwrap_or("")
        )),
        many => Some(format!("Cancelled {} Chrono schedules.", many.len())),
    }
}

impl Model {
    fn chrono_request(&mut self, conv: &str, command: impl FnOnce(&str) -> Value) -> Vec<Value> {
        if !self.ready {
            self.ui.error_toast("Not connected to Exocortex.");
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Ops,
            "chrono",
            Some(conv),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.ops
            .chrono
            .busy
            .insert(request.clone(), conv.to_owned());
        self.ops.chrono.errors.remove(conv);
        vec![command(&request)]
    }

    /// The panel opens: list the schedules.
    pub(crate) fn list_chrono(&mut self, conv: &str) -> Vec<Value> {
        if !self.ready {
            return Vec::new();
        }
        self.chrono_request(conv, |request| proto::chrono(request, conv, "list", None))
    }

    pub(crate) fn handle_chrono(&mut self, a: Act) -> Vec<Value> {
        let Some(conv) = self.chat.id.clone() else {
            return Vec::new();
        };
        match a {
            Act::Refresh => self.list_chrono(&conv),
            Act::More(id) => {
                let Some(s) = self
                    .ops
                    .chrono
                    .lists
                    .get(&conv)
                    .and_then(|list| list.iter().find(|s| s.id == id))
                    .cloned()
                else {
                    return Vec::new();
                };
                let items = vec![
                    ActionItem::new("Copy schedule id", Action::Ui(UiAct::Copy(s.id.clone())))
                        .icon(super::super::ui::icons::Icon::Copy),
                    ActionItem::new("Cancel schedule", act(Act::Cancel(s.id.clone())))
                        .icon(super::super::ui::icons::Icon::X)
                        .tone(Tone::Destructive),
                ];
                let line = format!(
                    "{} · {}{}",
                    format_due(s.next_at, now_ms() as f64),
                    repeat_label(&s),
                    if s.status == "scheduled" {
                        String::new()
                    } else {
                        format!(" · {}", s.status)
                    }
                );
                self.ui.open(Box::new(action_sheet(
                    ActionList::new(payload_label(&s, 80), items)
                        .subtitle(line)
                        .scoped(),
                )));
                Vec::new()
            }
            Act::Cancel(id) => {
                let count = self.ops.chrono.lists.get(&conv).map_or(0, Vec::len);
                let (title, message, confirm) = if id == "all" {
                    (
                        "Cancel every schedule?".to_owned(),
                        format!("{count} schedules"),
                        "Cancel all",
                    )
                } else {
                    let label = self
                        .ops
                        .chrono
                        .lists
                        .get(&conv)
                        .and_then(|list| list.iter().find(|s| s.id == id))
                        .map(|s| payload_label(s, 80))
                        .unwrap_or_default();
                    (
                        format!("Cancel {}?", short_id(&id)),
                        label,
                        "Cancel schedule",
                    )
                };
                self.ui.open(Box::new(Confirm::new(
                    title,
                    message,
                    confirm,
                    act(Act::CancelConfirmed(id)),
                )));
                Vec::new()
            }
            Act::CancelConfirmed(id) => self.chrono_request(&conv, |request| {
                proto::chrono(request, &conv, "cancel", Some(&id))
            }),
            Act::New => {
                self.ui.open(Box::new(prompt_sheet(Prompt::new(
                    "New schedule",
                    "in 2h check the deploy · every weekday at 8:30 !make test",
                    "",
                    "Schedule",
                    Box::new(|text| act(Act::Create(text))),
                ))));
                Vec::new()
            }
            Act::Create(text) => match parse(&text, now_ms() as f64) {
                Ok(Request::Create {
                    at,
                    repeat,
                    message,
                    command,
                }) => self.chrono_request(&conv, |request| {
                    proto::chrono_create(
                        request,
                        &conv,
                        &clock::iso_local(at),
                        repeat.as_ref(),
                        message.as_deref(),
                        command.as_deref(),
                    )
                }),
                Ok(_) => {
                    self.ui.toast("Use the list to see or cancel schedules.");
                    Vec::new()
                }
                Err(error) => {
                    self.ui.error_toast(error);
                    Vec::new()
                }
            },
        }
    }

    pub(crate) fn apply_chrono(&mut self, event: &Event) {
        if let Event::ChronoResult(e) = event {
            let ours = e
                .req_id
                .as_ref()
                .and_then(|id| self.ops.chrono.busy.remove(id))
                .is_some();
            self.ops
                .chrono
                .lists
                .insert(e.conv_id.clone(), e.schedules.clone());
            if ours
                && let Some(text) = result_text(e.created.as_ref(), &e.cancelled, now_ms() as f64)
            {
                self.ui.haptic(super::super::ui::Haptic::Light);
                self.ui.toast(text);
            }
        }
    }

    /// An error answering a Chrono request; true when it was one.
    pub(crate) fn chrono_error(&mut self, id: &str, message: &str) -> bool {
        match self.ops.chrono.busy.remove(id) {
            Some(conv) => {
                self.ops.chrono.errors.insert(conv, message.to_owned());
                true
            }
            None => false,
        }
    }
}

/// The Chrono panel's rows.
pub fn build(model: &Model, conv: &str, now: Instant) -> Built {
    let p = theme::p();
    let now_ms = clock::wall_ms(model, now);
    let list = model.ops.chrono.lists.get(conv);
    let loading = model.ops.chrono.busy.values().any(|c| c == conv);
    let mut rows = vec![Row::Section {
        title: match list {
            Some(list) => format!("Schedules ({})", list.len()),
            None => "Schedules".into(),
        },
        button: Some((
            if loading { "Loading…" } else { "Refresh" }.into(),
            Tap::Stay(act(Act::Refresh)),
        )),
    }];
    if let Some(error) = model.ops.chrono.errors.get(conv) {
        rows.push(panel::text(error.clone(), Style::Caption, p.error));
    }
    let mut wake = None;
    match list {
        Some(list) if list.is_empty() => {
            rows.push(panel::note("No Chrono schedules in this conversation."));
        }
        Some(list) => {
            for s in list {
                rows.push(Row::Task {
                    label: short_id(&s.id),
                    color: TEAL,
                    title: payload_label(s, 200),
                    time: format_due(s.next_at, now_ms),
                    tap: Some(Tap::Stay(act(Act::More(s.id.clone())))),
                    more: Some(Tap::Stay(act(Act::More(s.id.clone())))),
                });
                let mut detail = repeat_label(s);
                if s.status != "scheduled" {
                    detail = format!("{detail} · {}", s.status);
                }
                if s.kind == "command" {
                    detail = format!("{detail} · shell");
                }
                rows.push(panel::text(
                    detail,
                    Style::Meta,
                    if s.kind == "command" {
                        COMMAND
                    } else {
                        p.muted
                    },
                ));
                wake = clock::sooner(
                    wake,
                    clock::next_change(now_ms, Some(s.next_at), now_ms, now),
                );
            }
            if list.len() > 1 {
                rows.push(Row::Buttons(vec![Btn::new(
                    "Cancel all",
                    Kind::Destructive,
                    Some(Tap::Stay(act(Act::Cancel("all".into())))),
                )]));
            }
        }
        None => rows.push(panel::note("Loading the schedules…")),
    }
    rows.push(panel::section("New schedule"));
    rows.push(panel::text(
        USAGE.lines().skip(1).collect::<Vec<_>>().join("\n"),
        Style::Small,
        p.muted,
    ));
    rows.push(Row::Buttons(vec![Btn::new(
        "New schedule…",
        Kind::Primary,
        model.ready.then_some(Tap::Stay(act(Act::New))),
    )]));
    Built {
        title: "Chrono".into(),
        rows,
        wake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(text: &str, now: f64) -> (f64, Option<Value>, Option<String>, Option<String>) {
        match parse(text, now) {
            Ok(Request::Create {
                at,
                repeat,
                message,
                command,
            }) => (at, repeat, message, command),
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn the_grammar_reads_as_the_tuis() {
        let now = clock::from_local(2026, 10, 29, 10, 0, 0); // a Thursday
        let (at, repeat, message, _) = create("30m check the deploy", now);
        assert_eq!(at, now + 1_800_000.0);
        assert_eq!(repeat, None);
        assert_eq!(message.as_deref(), Some("check the deploy"));
        let (at, _, _, _) = create("in 2h 30m hi", now);
        assert_eq!(at, now + 9_000_000.0);
        let (at, _, _, _) = create("at 9am hi", now);
        assert_eq!(
            at,
            clock::from_local(2026, 10, 30, 9, 0, 0),
            "9am passed: tomorrow"
        );
        let (at, _, _, _) = create("tomorrow 14:30 hi", now);
        assert_eq!(at, clock::from_local(2026, 10, 30, 14, 30, 0));
        let (at, _, _, _) = create("fri 5pm hi", now);
        assert_eq!(at, clock::from_local(2026, 10, 30, 17, 0, 0));
        let (at, _, _, command) = create("2026-12-24 18:00 !make test", now);
        assert_eq!(at, clock::from_local(2026, 12, 24, 18, 0, 0));
        assert_eq!(command.as_deref(), Some("make test"));
        let (at, repeat, _, _) = create("every 2h ping", now);
        assert_eq!(at, now + 7_200_000.0);
        assert_eq!(repeat, Some(json!({"unit": "hour", "interval": 2})));
        let (at, repeat, _, _) = create("every day at 9am standup", now);
        assert_eq!(at, clock::from_local(2026, 10, 30, 9, 0, 0));
        assert_eq!(repeat, Some(json!({"unit": "day"})));
        let (at, repeat, _, _) = create("every weekday at 8:30 standup", now);
        assert_eq!(at, clock::from_local(2026, 10, 30, 8, 30, 0));
        assert_eq!(
            repeat,
            Some(json!({"unit": "week", "weekdays": ["mon", "tue", "wed", "thu", "fri"]}))
        );
        let (_, repeat, _, _) = create("every mon,thu at 18:00 gym", now);
        assert_eq!(
            repeat,
            Some(json!({"unit": "week", "weekdays": ["mon", "thu"]}))
        );
        let (_, repeat, _, _) = create("daily report", now);
        assert_eq!(repeat, Some(json!({"unit": "day"})));
        assert_eq!(parse("", now), Ok(Request::List));
        assert_eq!(
            parse("cancel ab12", now),
            Ok(Request::Cancel("ab12".into()))
        );
        assert!(parse("30m", now).unwrap_err().starts_with("Add a message"));
        assert!(
            parse("in soon hi", now)
                .unwrap_err()
                .starts_with("Add a delay")
        );
        assert!(
            parse("today 9am hi", now)
                .unwrap_err()
                .contains("already passed")
        );
        assert!(
            parse("tomorrow hi", now)
                .unwrap_err()
                .starts_with("Add a time")
        );
        assert!(parse("9 lives", now).is_err(), "a bare number is prose");
        assert!(parse("30m !", now).unwrap_err().contains("shell command"));
    }

    #[test]
    fn schedules_read_as_on_the_iphone() {
        let s = ChronoSchedule {
            id: "chrono:abcdef1234".into(),
            payload: "make test".into(),
            kind: "command".into(),
            recurrence: Some(
                json!({"kind":"calendar","unit":"week","interval":1,"hour":8,"minute":30,"second":0,"weekdays":[5,1,2,3,4]}),
            ),
            ..ChronoSchedule::default()
        };
        assert_eq!(short_id(&s.id), "abcdef12");
        assert_eq!(repeat_label(&s), "every weekday at 08:30");
        assert_eq!(payload_label(&s, 80), "!make test");
        let now = clock::from_local(2026, 10, 29, 10, 0, 0);
        assert_eq!(format_due(now + 7_200_000.0, now), "12:00 (in 2h)");
    }
}
