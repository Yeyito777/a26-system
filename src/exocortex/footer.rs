//! The assistant metadata footer (WP-C2, PAR-116–119, roadmap §3.6.12):
//! `OPUS-5.5 | 76.2 TOK/S | 2:03PM`, one per run of assistant segments,
//! as the TUI's `metadata.ts` and the iPhone's `metadata.ts` make it.
//!
//! A run aggregates the assistant messages back to the last real user
//! message (or an error notice); a gap of more than 2 h restarts it. While
//! the AI works the third part is the duration, ticking; once final it is
//! the finish clock in the phone's zone.

use super::model_display::display_name;
use super::proto::{Entry, Metadata, Throughput};

const MAX_SUMMARY_SPAN_MS: f64 = 48.0 * 3600.0 * 1000.0;
const MAX_IDLE_GAP_MS: f64 = 2.0 * 3600.0 * 1000.0;
/// Throughput samples weighed (the daemon's `GENERATION_RATE_CUTOFF`).
const RATE_CUTOFF: usize = 10;

/// `combineMessageMetadata`: the metadata of two segments of a run.
pub fn combine(current: Option<&Metadata>, next: Option<&Metadata>) -> Option<Metadata> {
    match (current, next) {
        (None, None) => None,
        (None, Some(one)) | (Some(one), None) => Some(one.clone()),
        (Some(current), Some(next)) => {
            let same_turn = current.started_at == next.started_at;
            Some(Metadata {
                started_at: current.started_at.min(next.started_at),
                ended_at: match (current.ended_at, next.ended_at) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                },
                tokens: if same_turn {
                    current.tokens.max(next.tokens)
                } else {
                    current.tokens + next.tokens
                },
                ..next.clone()
            })
        }
    }
}

/// `combineSegmentMetadata`: as `combine`, but a gap of more than 2 h
/// between them starts the run again.
pub fn combine_segment(current: Option<&Metadata>, next: Option<&Metadata>) -> Option<Metadata> {
    if let (Some(current), Some(next)) = (current, next) {
        let current_end = current.ended_at.unwrap_or(current.started_at);
        let next_end = next.ended_at.unwrap_or(next.started_at);
        let gap = if next.started_at > current_end {
            next.started_at - current_end
        } else if current.started_at > next_end {
            current.started_at - next_end
        } else {
            0.0
        };
        if gap > MAX_IDLE_GAP_MS {
            return Some(next.clone());
        }
    }
    combine(current, next)
}

/// An assistant entry's metadata as a run counts it: an older compaction
/// summary's overbroad span never counts as work time.
fn for_aggregation(entry: &Entry) -> Option<&Metadata> {
    let Entry::Ai { blocks, metadata } = entry else {
        return None;
    };
    let metadata = metadata.as_ref()?;
    let span = metadata.ended_at.map_or(0.0, |end| end - metadata.started_at);
    let summary = blocks.iter().any(
        |block| matches!(block, super::proto::Block::Text { text } if text.starts_with("[Summary")),
    );
    if metadata.work_timer_started_at.is_none() && span > MAX_SUMMARY_SPAN_MS && summary {
        return None;
    }
    Some(metadata)
}

/// An error notice ends a run (`✗ …` or colour `error`).
pub fn is_terminal_notice(entry: &Entry) -> bool {
    matches!(entry, Entry::System { text, color, .. }
        if text.starts_with('✗') || color.as_deref() == Some("error"))
}

/// What starts a run: a user message (not a daemon notice) or an error.
fn is_boundary(entry: &Entry) -> bool {
    match entry {
        Entry::User { metadata, .. } => !metadata.as_ref().is_some_and(|m| m.system),
        other => is_terminal_notice(other),
    }
}

/// The footers of `entries`: for each entry, the run's metadata when it
/// closes a run of assistant segments (the last of consecutive ones;
/// `live_follows`: a live reply continues the last), and the metadata the
/// live reply's footer starts from (what precedes it in its run).
pub fn footers(entries: &[Entry], live_follows: bool) -> (Vec<Option<Metadata>>, Option<Metadata>) {
    let mut out = vec![None; entries.len()];
    let mut run: Option<Metadata> = None;
    for (index, entry) in entries.iter().enumerate() {
        if is_boundary(entry) {
            run = None;
            continue;
        }
        if !matches!(entry, Entry::Ai { .. }) {
            continue;
        }
        run = combine_segment(run.as_ref(), for_aggregation(entry));
        let continues = match entries.get(index + 1) {
            Some(next) => matches!(next, Entry::Ai { .. }),
            None => live_follows,
        };
        if !continues {
            out[index] = run.clone();
        }
    }
    (out, run)
}

/// Output tokens per second: exponentially weighted over the last ten API
/// rounds, newest first (`generationTokensPerSecond`). None when unknown;
/// 0 for a reply with no round yet.
pub fn tokens_per_second(throughput: Option<&Throughput>) -> Option<f64> {
    let rates = &throughput?.rates;
    if rates.is_empty() {
        return Some(0.0);
    }
    let samples: Vec<f64> = rates
        .iter()
        .copied()
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .collect();
    let samples = &samples[samples.len().saturating_sub(RATE_CUTOFF)..];
    if samples.is_empty() {
        return None;
    }
    let alpha = 2.0 / (RATE_CUTOFF as f64 + 1.0);
    let (mut total, mut weights, mut weight) = (0.0, 0.0, 1.0);
    for rate in samples.iter().rev() {
        total += rate * weight;
        weights += weight;
        weight *= 1.0 - alpha;
    }
    let rate = total / weights;
    rate.is_finite().then_some(rate)
}

/// `0s`, `12s`, `3m 4s`, `2h 5m 0s`, `1d 2h 3m 4s`, `1w …` (PAR-119).
pub fn format_duration(ms: f64) -> String {
    if !ms.is_finite() || ms < 1000.0 {
        return "0s".into();
    }
    let mut s = (ms / 1000.0).floor() as u64;
    let w = s / 604_800;
    s -= w * 604_800;
    let d = s / 86_400;
    s -= d * 86_400;
    let h = s / 3600;
    s -= h * 3600;
    let m = s / 60;
    s -= m * 60;
    if w > 0 {
        format!("{w}w {d}d {h}h {m}m {s}s")
    } else if d > 0 {
        format!("{d}d {h}h {m}m {s}s")
    } else if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Wall-clock fields of `ms` in the phone's zone: (year, month 1-12, day,
/// hour, minute).
fn local(ms: f64) -> (i32, u32, u32, u32, u32) {
    // SAFETY: localtime_r fills the zeroed tm from a valid time_t.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = (ms / 1000.0).floor() as _;
        libc::localtime_r(&time, &mut tm);
        tm
    };
    (
        tm.tm_year + 1900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
    )
}

/// Days since 1970 of a calendar date (for "yesterday").
fn day_number(year: i32, month: u32, day: u32) -> i64 {
    // Howard Hinnant's days_from_civil.
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The local calendar day of `ms`: what "today" and "yesterday" turn on.
pub fn local_day(ms: f64) -> i64 {
    let (year, month, day, _, _) = local(ms);
    day_number(year, month, day)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// When the AI finished, on a 12-hour clock (TUI `formatFinishTime`):
/// `2:03pm` today, `yesterday 10:41pm`, `Oct 19 9:12am`, `Oct 19 2025
/// 9:12am` in another year.
pub fn format_finish_time(ended_at: f64, now: f64) -> Option<String> {
    if !ended_at.is_finite() || ended_at <= 0.0 {
        return None;
    }
    let (year, month, day, hour, minute) = local(ended_at);
    let (now_year, now_month, now_day, _, _) = local(now);
    let half = if hour < 12 { "am" } else { "pm" };
    let time = format!("{}:{minute:02}{half}", if hour % 12 == 0 { 12 } else { hour % 12 });
    let days = day_number(now_year, now_month, now_day) - day_number(year, month, day);
    Some(match days {
        0 => time,
        1 => format!("yesterday {time}"),
        _ => {
            let month = MONTHS[(month as usize).clamp(1, 12) - 1];
            if year == now_year {
                format!("{month} {day} {time}")
            } else {
                format!("{month} {day} {year} {time}")
            }
        }
    })
}

/// The footer's line (before the type token uppercases it): the model, the
/// throughput, and the live duration (`active`) or the finish clock.
pub fn text(metadata: &Metadata, now: f64, active: bool) -> String {
    let throughput = metadata
        .generation_throughput
        .as_ref()
        .filter(|throughput| throughput.model == metadata.model);
    // A new message shows 0, never a previous message's rate.
    let rate = tokens_per_second(throughput).or_else(|| metadata.ended_at.is_none().then_some(0.0));
    let rate = rate.map_or_else(|| "—".to_owned(), |rate| format!("{rate:.1}"));
    let model = display_name(&metadata.model);
    let model = if model.is_empty() { "—".to_owned() } else { model };
    let end = if active {
        now
    } else {
        metadata.ended_at.unwrap_or(now)
    };
    let duration =
        format_duration(end - metadata.work_timer_started_at.unwrap_or(metadata.started_at));
    let when = if active {
        duration
    } else {
        metadata
            .ended_at
            .and_then(|ended| format_finish_time(ended, now))
            .unwrap_or(duration)
    };
    format!("{model} | {rate} tok/s | {when}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::Block;

    fn meta(started: f64, ended: Option<f64>, tokens: u64) -> Metadata {
        Metadata {
            started_at: started,
            ended_at: ended,
            model: "claude-opus-5-5".into(),
            tokens,
            ..Metadata::default()
        }
    }

    fn ai(metadata: Metadata) -> Entry {
        Entry::Ai {
            blocks: vec![Block::text("x")],
            metadata: Some(metadata),
        }
    }

    fn user(text: &str) -> Entry {
        Entry::User {
            text: text.into(),
            images: Vec::new(),
            metadata: None,
        }
    }

    #[test]
    fn durations_follow_the_tui() {
        assert_eq!(format_duration(999.0), "0s");
        assert_eq!(format_duration(12_400.0), "12s");
        assert_eq!(format_duration(184_000.0), "3m 4s");
        assert_eq!(format_duration(7_500_000.0), "2h 5m 0s");
        assert_eq!(format_duration(93_784_000.0), "1d 2h 3m 4s");
        assert_eq!(format_duration(f64::NAN), "0s");
    }

    #[test]
    fn throughput_is_weighted_toward_the_newest_rounds() {
        let tp = |rates: Vec<f64>| Throughput {
            provider: "anthropic".into(),
            model: "m".into(),
            rates,
        };
        assert_eq!(tokens_per_second(None), None);
        assert_eq!(tokens_per_second(Some(&tp(vec![]))), Some(0.0));
        assert_eq!(tokens_per_second(Some(&tp(vec![-1.0]))), None);
        assert_eq!(tokens_per_second(Some(&tp(vec![50.0]))), Some(50.0));
        let rate = tokens_per_second(Some(&tp(vec![10.0, 100.0]))).unwrap();
        assert!(rate > 55.0 && rate < 100.0, "{rate}");
    }

    #[test]
    fn segments_combine_within_a_run_and_restart_after_two_hours() {
        let a = meta(1000.0, Some(5000.0), 10);
        let b = meta(6000.0, Some(9000.0), 5);
        let both = combine_segment(Some(&a), Some(&b)).unwrap();
        assert_eq!((both.started_at, both.ended_at, both.tokens), (1000.0, Some(9000.0), 15));
        // The same turn's fragments: tokens already cumulative.
        let again = combine(Some(&a), Some(&meta(1000.0, Some(6000.0), 12))).unwrap();
        assert_eq!(again.tokens, 12);
        // Live: no end yet.
        assert_eq!(combine(Some(&a), Some(&meta(6000.0, None, 1))).unwrap().ended_at, None);
        let late = meta(5000.0 + 3.0 * 3600.0 * 1000.0, None, 1);
        assert_eq!(combine_segment(Some(&a), Some(&late)).unwrap(), late);
    }

    #[test]
    fn one_footer_closes_each_run() {
        let entries = vec![
            user("q"),
            ai(meta(1000.0, Some(2000.0), 1)),
            ai(meta(3000.0, Some(4000.0), 2)),
            Entry::System {
                text: "note".into(),
                color: None,
                metadata: None,
            },
            ai(meta(5000.0, Some(6000.0), 4)),
            user("next"),
            ai(meta(7000.0, Some(8000.0), 8)),
        ];
        let (footers, live) = footers(&entries, false);
        assert!(footers[1].is_none(), "continued by the next segment");
        assert_eq!(footers[2].as_ref().unwrap().tokens, 3);
        // A notice does not end the run: the next footer covers it all.
        let third = footers[4].as_ref().unwrap();
        assert_eq!((third.started_at, third.tokens), (1000.0, 7));
        assert_eq!(footers[6].as_ref().unwrap().tokens, 8);
        assert_eq!(live.unwrap().tokens, 8);
        // A live reply continues the last segment: its footer is the live one.
        let (footers, _) = super::footers(&entries, true);
        assert!(footers[6].is_none());
        // An error notice ends the run.
        let entries = vec![
            ai(meta(1000.0, Some(2000.0), 1)),
            Entry::System {
                text: "✗ failed".into(),
                color: Some("error".into()),
                metadata: None,
            },
            ai(meta(3000.0, Some(4000.0), 2)),
        ];
        let (footers, _) = super::footers(&entries, false);
        assert_eq!(footers[2].as_ref().unwrap().tokens, 2);
    }

    #[test]
    fn the_line_shows_the_duration_while_live_and_the_clock_once_done() {
        let mut m = meta(0.0, None, 0);
        m.started_at = 1_000_000.0;
        m.work_timer_started_at = Some(1_000_000.0 - 60_000.0);
        let now = 1_000_000.0 + 35_000.0;
        assert_eq!(text(&m, now, true), "Opus-5.5 | 0.0 tok/s | 1m 35s");
        m.ended_at = Some(now);
        m.generation_throughput = Some(Throughput {
            provider: "anthropic".into(),
            model: "claude-opus-5-5".into(),
            rates: vec![76.2],
        });
        let line = text(&m, now, false);
        assert!(line.starts_with("Opus-5.5 | 76.2 tok/s | "), "{line}");
        assert!(line.ends_with("am") || line.ends_with("pm"), "{line}");
        // Another model's rate is not this one's.
        m.generation_throughput.as_mut().unwrap().model = "other".into();
        assert!(text(&m, now, false).contains("| — tok/s |"));
    }

    #[test]
    fn finish_times_name_the_day_when_not_today() {
        let day = 86_400_000.0;
        // Noon, ten days into 2026 (UTC; the test's zone is whatever runs it).
        let noon = 1_767_225_600_000.0 + 10.0 * day + 12.0 * 3_600_000.0;
        let today = format_finish_time(noon, noon).unwrap();
        assert!(!today.contains(' '), "{today}");
        assert!(format_finish_time(noon - day, noon).unwrap().starts_with("yesterday "));
        let older = format_finish_time(noon - 5.0 * day, noon).unwrap();
        assert!(older.starts_with("Jan "), "{older}");
        let last_year = format_finish_time(noon - 60.0 * day, noon).unwrap();
        assert!(last_year.contains(" 2025 "), "{last_year}");
        assert_eq!(format_finish_time(f64::NAN, noon), None);
        assert_eq!(local_day(noon) - local_day(noon - day), 1);
    }
}
