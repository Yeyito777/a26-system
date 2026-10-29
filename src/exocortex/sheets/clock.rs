//! Times and durations as the operations panels write them (the iPhone's
//! `state/ops/time.ts`, ported from Exocortex `shared/src/duration.ts`,
//! `shared/src/goals.ts` and the TUI, with the same output), and local
//! time through the C library (the process follows Moon's zone).

use std::time::{Duration, Instant};

use super::super::model::{Model, now_ms};
use super::super::proto::Goal;

/// The wall clock (epoch ms) at a frame's `now`, as the list reads it
/// (`Sidebar::clock`, set each pass of the event loop): a frame and its
/// redraw show the same times whatever the real clock does meanwhile.
pub fn wall_ms(model: &Model, now: Instant) -> f64 {
    match model.sidebar.clock {
        Some((at, ms)) if now >= at => ms as f64 + now.duration_since(at).as_millis() as f64,
        Some((at, ms)) => ms as f64 - at.duration_since(now).as_millis() as f64,
        None => now_ms() as f64,
    }
}

const GOAL_UNITS: [(&str, u64); 4] = [
    ("d", 86_400_000),
    ("h", 3_600_000),
    ("m", 60_000),
    ("s", 1_000),
];

/// `9h2m1s`, `8h3m`, `1h`, `5m`, `2d` → ms; None when invalid or zero.
pub fn parse_goal_duration(text: &str) -> Option<u64> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let text = text.to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    let mut rest = text.as_str();
    let mut total = 0u64;
    for (unit, ms) in GOAL_UNITS {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(unit) {
            let count: u64 = rest[..digits].parse().ok()?;
            total = total.checked_add(count.checked_mul(ms)?)?;
            rest = &rest[digits + unit.len()..];
        }
    }
    (rest.is_empty() && total > 0).then_some(total)
}

/// `1d2h`, `9h2m1s`, `0s`.
pub fn format_goal_duration(ms: u64) -> String {
    let mut rest = ms / 1000 * 1000;
    let mut out = String::new();
    for (unit, unit_ms) in GOAL_UNITS {
        let count = rest / unit_ms;
        rest -= count * unit_ms;
        if count > 0 {
            out.push_str(&format!("{count}{unit}"));
        }
    }
    if out.is_empty() { "0s".into() } else { out }
}

/// Time the goal has been active (paused, blocked and complete periods
/// left out).
pub fn goal_active_ms(goal: &Goal, now_ms: f64) -> u64 {
    let current = match (goal.status.as_str(), goal.active_since) {
        ("active", Some(since)) => (now_ms - since).max(0.0),
        _ => 0.0,
    };
    (goal.active_ms.unwrap_or(0.0) + current).max(0.0) as u64
}

/// "◆ active · 3 turns · 12m of 1h".
pub fn goal_summary_line(goal: &Goal, now_ms: f64) -> String {
    let used = format_goal_duration(goal_active_ms(goal, now_ms));
    let glyph = if goal.status == "active" {
        "◆"
    } else {
        "◇"
    };
    let turns = format!(
        "{} turn{}",
        goal.turns,
        if goal.turns == 1 { "" } else { "s" }
    );
    let time = match goal.max_time_ms {
        Some(max) if max > 0.0 => format!("{used} of {}", format_goal_duration(max as u64)),
        _ => format!("{used} active"),
    };
    format!("{glyph} {} · {turns} · {time}", goal.status)
}

/// Activity elapsed time: `Ns`, `Mm Ss`, `Hh Mm`, `Dd Hh`, `Ww Dd`.
pub fn format_task_elapsed(started_ms: f64, now_ms: f64) -> String {
    let seconds = ((now_ms - started_ms) / 1000.0).max(0.0).floor() as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s", seconds % 60);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {}m", minutes % 60);
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{days}d {}h", hours % 24);
    }
    format!("{}w {}d", days / 7, days % 7)
}

/// A Chrono countdown: `in Ns`, `in Nm`, `in Nh`, `in Nd`, or `due`.
pub fn format_task_countdown(due_ms: f64, now_ms: f64) -> String {
    let seconds = ((due_ms - now_ms) / 1000.0).ceil();
    if seconds <= 0.0 {
        return "due".into();
    }
    let seconds = seconds as u64;
    if seconds < 60 {
        return format!("in {seconds}s");
    }
    let minutes = seconds.div_ceil(60);
    if minutes < 60 {
        return format!("in {minutes}m");
    }
    let hours = minutes.div_ceil(60);
    if hours < 24 {
        return format!("in {hours}h");
    }
    format!("in {}d", hours.div_ceil(24))
}

/// "1h20m", "3d4h", "45m", "now" (combined usage resets).
pub fn format_short_time_until(reset_ms: f64, now_ms: f64) -> String {
    let diff = ((reset_ms - now_ms) / 1000.0).floor();
    if diff <= 0.0 {
        return "now".into();
    }
    let diff = diff as u64;
    let (days, hours, minutes) = (diff / 86_400, diff % 86_400 / 3600, diff % 3600 / 60);
    if days > 0 {
        if hours > 0 {
            format!("{days}d{hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

/// Accounts' durations: `45m`, `2h05m`, `3d4h`.
pub fn format_account_duration(ms: f64) -> String {
    let minutes = (ms / 60_000.0).round().max(0.0) as u64;
    let (days, hours, mins) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    if days > 0 {
        if hours > 0 {
            format!("{days}d{hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if mins > 0 {
            format!("{hours}h{mins:02}m")
        } else {
            format!("{hours}h")
        }
    } else {
        format!("{mins}m")
    }
}

/// A moment in local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 Sunday … 6 Saturday.
    pub weekday: u32,
}

/// `ms` (since the epoch) in local time.
pub fn local(ms: f64) -> Local {
    // SAFETY: localtime_r fills the zeroed tm from a valid time_t.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = (ms / 1000.0).floor() as libc::time_t;
        libc::localtime_r(&time, &mut tm);
        tm
    };
    Local {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
        weekday: tm.tm_wday as u32,
    }
}

/// The ms of a local date and time (days and fields past their range roll
/// over, as `mktime` does).
pub fn from_local(year: i32, month: i32, day: i32, hour: u32, minute: u32, second: u32) -> f64 {
    // SAFETY: mktime reads and normalizes a fully initialized tm.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = year - 1900;
        tm.tm_mon = month - 1;
        tm.tm_mday = day;
        tm.tm_hour = hour as i32;
        tm.tm_min = minute as i32;
        tm.tm_sec = second as i32;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm) as f64 * 1000.0
    }
}

/// The local UTC offset at `ms`, in seconds east.
pub fn utc_offset(ms: f64) -> i64 {
    // SAFETY: localtime_r fills the zeroed tm from a valid time_t.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = (ms / 1000.0).floor() as libc::time_t;
        libc::localtime_r(&time, &mut tm);
        #[allow(clippy::useless_conversion)] // c_long is i32 on some targets.
        i64::from(tm.tm_gmtoff)
    }
}

/// `ms` as an ISO-8601 instant with the local offset
/// (`2026-10-29T14:30:00-03:00`).
pub fn iso_local(ms: f64) -> String {
    let t = local(ms);
    let offset = utc_offset(ms);
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        t.year,
        t.month,
        t.day,
        t.hour,
        t.minute,
        t.second,
        offset / 3600,
        offset % 3600 / 60
    )
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "14:30" today, else "Oct 18 14:30" (the TUI's queue times).
pub fn format_due_time(due_ms: f64, now_ms: f64) -> String {
    let due = local(due_ms);
    let today = local(now_ms);
    let time = format!("{:02}:{:02}", due.hour, due.minute);
    if (due.year, due.month, due.day) == (today.year, today.month, today.day) {
        time
    } else {
        format!("{} {} {time}", MONTHS[(due.month - 1) as usize], due.day)
    }
}

/// "Oct 17, 2026, 2:30 PM".
pub fn format_date_time(ms: f64) -> String {
    let t = local(ms);
    let (hour, half) = match t.hour {
        0 => (12, "AM"),
        h if h < 12 => (h, "AM"),
        12 => (12, "PM"),
        h => (h - 12, "PM"),
    };
    format!(
        "{} {}, {}, {hour}:{:02} {half}",
        MONTHS[(t.month - 1) as usize],
        t.day,
        t.year,
        t.minute
    )
}

/// When a label counting from `started_ms` (or down to `due_ms`) can
/// next change: seconds in the first hour, then minutes, hours, days.
pub fn next_change(
    started_ms: f64,
    due_ms: Option<f64>,
    now_ms: f64,
    now: Instant,
) -> Option<Instant> {
    let wait = match due_ms {
        Some(due) => {
            let remaining = due - now_ms;
            if remaining <= 0.0 {
                return None;
            }
            let seconds = (remaining / 1000.0).ceil();
            let unit = if seconds < 60.0 {
                1000.0
            } else if (seconds / 60.0).ceil() < 60.0 {
                60_000.0
            } else if (seconds / 3600.0).ceil() < 24.0 {
                3_600_000.0
            } else {
                86_400_000.0
            };
            let rest = remaining % unit;
            if rest == 0.0 { unit } else { rest }
        }
        None => {
            let elapsed = (now_ms - started_ms).max(0.0);
            let unit = if elapsed < 3_600_000.0 {
                1000.0
            } else if elapsed < 86_400_000.0 {
                60_000.0
            } else if elapsed < 7.0 * 86_400_000.0 {
                3_600_000.0
            } else {
                86_400_000.0
            };
            unit - elapsed % unit
        }
    };
    Some(now + Duration::from_millis(wait.max(250.0) as u64 + 20))
}

/// The soonest of two wakes.
pub fn sooner(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_and_write_as_the_tuis() {
        assert_eq!(parse_goal_duration("9h2m1s"), Some(32_521_000));
        assert_eq!(parse_goal_duration("2d"), Some(172_800_000));
        assert_eq!(parse_goal_duration("2h 30m"), Some(9_000_000));
        assert_eq!(parse_goal_duration("0m"), None);
        assert_eq!(parse_goal_duration("3x"), None);
        assert_eq!(parse_goal_duration("5m2h"), None, "units in order");
        assert_eq!(format_goal_duration(32_521_000), "9h2m1s");
        assert_eq!(format_goal_duration(0), "0s");
        assert_eq!(format_task_elapsed(0.0, 59_000.0), "59s");
        assert_eq!(format_task_elapsed(0.0, 61_000.0), "1m 1s");
        assert_eq!(format_task_elapsed(0.0, 3_700_000.0), "1h 1m");
        assert_eq!(format_task_elapsed(0.0, 8.0 * 86_400_000.0), "1w 1d");
        assert_eq!(format_task_countdown(30_000.0, 0.0), "in 30s");
        assert_eq!(format_task_countdown(90_000.0, 0.0), "in 2m");
        assert_eq!(format_task_countdown(0.0, 10.0), "due");
        assert_eq!(format_short_time_until(4_500_000.0, 0.0), "1h15m");
        assert_eq!(format_short_time_until(187_200_000.0, 0.0), "2d4h");
        assert_eq!(format_account_duration(7_500_000.0), "2h05m");
    }

    #[test]
    fn a_goal_summary_counts_its_active_time() {
        let goal = Goal {
            objective: "o".into(),
            status: "active".into(),
            turns: 3,
            active_ms: Some(600_000.0),
            active_since: Some(1_000_000.0),
            max_time_ms: Some(3_600_000.0),
            ..Goal::default()
        };
        assert_eq!(
            goal_summary_line(&goal, 1_120_000.0),
            "◆ active · 3 turns · 12m of 1h"
        );
    }

    #[test]
    fn local_times_round_trip() {
        let at = from_local(2026, 10, 29, 14, 30, 0);
        let t = local(at);
        assert_eq!(
            (t.year, t.month, t.day, t.hour, t.minute),
            (2026, 10, 29, 14, 30)
        );
        assert!(iso_local(at).starts_with("2026-10-29T14:30:00"));
    }
}
