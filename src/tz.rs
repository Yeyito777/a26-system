//! The time zones Moon offers (OSS-Phone-V1 `src/a26-shell/src/tz.rs`, the
//! same ids and POSIX rules from tzdata 2026b), with the city shown in the
//! picker, sorted by city.

pub const ZONES: [(&str, &str, &str); 88] = [
    (
        "Australia/Adelaide",
        "Adelaide",
        "ACST-9:30ACDT,M10.1.0,M4.1.0/3",
    ),
    (
        "Europe/Amsterdam",
        "Amsterdam",
        "CET-1CEST,M3.5.0,M10.5.0/3",
    ),
    ("America/Anchorage", "Anchorage", "AKST9AKDT,M3.2.0,M11.1.0"),
    ("Europe/Athens", "Athens", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    (
        "Pacific/Auckland",
        "Auckland",
        "NZST-12NZDT,M9.5.0,M4.1.0/3",
    ),
    (
        "Atlantic/Azores",
        "Azores",
        "<-01>1<+00>,M3.5.0/0,M10.5.0/1",
    ),
    ("Asia/Bangkok", "Bangkok", "<+07>-7"),
    ("Europe/Berlin", "Berlin", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("America/Bogota", "Bogotá", "<-05>5"),
    ("Australia/Brisbane", "Brisbane", "AEST-10"),
    ("Europe/Brussels", "Brussels", "CET-1CEST,M3.5.0,M10.5.0/3"),
    (
        "Europe/Bucharest",
        "Bucharest",
        "EET-2EEST,M3.5.0/3,M10.5.0/4",
    ),
    ("America/Argentina/Buenos_Aires", "Buenos Aires", "<-03>3"),
    ("Africa/Cairo", "Cairo", "EET-2EEST,M4.5.5/0,M10.5.4/24"),
    ("America/Caracas", "Caracas", "<-04>4"),
    ("Africa/Casablanca", "Casablanca", "<+01>-1"),
    ("America/Chicago", "Chicago", "CST6CDT,M3.2.0,M11.1.0"),
    (
        "Europe/Copenhagen",
        "Copenhagen",
        "CET-1CEST,M3.5.0,M10.5.0/3",
    ),
    ("Australia/Darwin", "Darwin", "ACST-9:30"),
    ("America/Denver", "Denver", "MST7MDT,M3.2.0,M11.1.0"),
    ("Asia/Dhaka", "Dhaka", "<+06>-6"),
    ("Asia/Dubai", "Dubai", "<+04>-4"),
    ("Europe/Dublin", "Dublin", "IST-1GMT0,M10.5.0,M3.5.0/1"),
    ("America/Edmonton", "Edmonton", "MST7MDT,M3.2.0,M11.1.0"),
    ("America/Guatemala", "Guatemala City", "CST6"),
    ("America/Guayaquil", "Guayaquil", "<-05>5"),
    ("America/Halifax", "Halifax", "AST4ADT,M3.2.0,M11.1.0"),
    ("America/Havana", "Havana", "CST5CDT,M3.2.0/0,M11.1.0/1"),
    (
        "Europe/Helsinki",
        "Helsinki",
        "EET-2EEST,M3.5.0/3,M10.5.0/4",
    ),
    ("Asia/Ho_Chi_Minh", "Ho Chi Minh City", "<+07>-7"),
    ("Asia/Hong_Kong", "Hong Kong", "HKT-8"),
    ("Pacific/Honolulu", "Honolulu", "HST10"),
    ("Europe/Istanbul", "Istanbul", "<+03>-3"),
    ("Asia/Jakarta", "Jakarta", "WIB-7"),
    ("Asia/Jerusalem", "Jerusalem", "IST-2IDT,M3.4.4/26,M10.5.0"),
    ("Africa/Johannesburg", "Johannesburg", "SAST-2"),
    ("Asia/Karachi", "Karachi", "PKT-5"),
    ("Asia/Kathmandu", "Kathmandu", "<+0545>-5:45"),
    ("America/Jamaica", "Kingston", "EST5"),
    ("Asia/Kolkata", "Kolkata", "IST-5:30"),
    ("Asia/Kuala_Lumpur", "Kuala Lumpur", "<+08>-8"),
    ("Europe/Kyiv", "Kyiv", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("America/La_Paz", "La Paz", "<-04>4"),
    ("Africa/Lagos", "Lagos", "WAT-1"),
    ("America/Lima", "Lima", "<-05>5"),
    ("Europe/Lisbon", "Lisbon", "WET0WEST,M3.5.0/1,M10.5.0"),
    ("Europe/London", "London", "GMT0BST,M3.5.0/1,M10.5.0"),
    (
        "America/Los_Angeles",
        "Los Angeles",
        "PST8PDT,M3.2.0,M11.1.0",
    ),
    ("Europe/Madrid", "Madrid", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Asia/Manila", "Manila", "PST-8"),
    (
        "Australia/Melbourne",
        "Melbourne",
        "AEST-10AEDT,M10.1.0,M4.1.0/3",
    ),
    ("America/Mexico_City", "Mexico City", "CST6"),
    ("America/Montevideo", "Montevideo", "<-03>3"),
    ("Europe/Moscow", "Moscow", "MSK-3"),
    ("Africa/Nairobi", "Nairobi", "EAT-3"),
    ("America/New_York", "New York", "EST5EDT,M3.2.0,M11.1.0"),
    ("Europe/Oslo", "Oslo", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("America/Panama", "Panama", "EST5"),
    ("Europe/Paris", "Paris", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Australia/Perth", "Perth", "AWST-8"),
    ("America/Phoenix", "Phoenix", "MST7"),
    ("Europe/Prague", "Prague", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("America/Regina", "Regina", "CST6"),
    ("Asia/Riyadh", "Riyadh", "<+03>-3"),
    ("Europe/Rome", "Rome", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("America/Costa_Rica", "San José", "CST6"),
    ("America/Puerto_Rico", "San Juan", "AST4"),
    ("America/El_Salvador", "San Salvador", "CST6"),
    (
        "America/Santiago",
        "Santiago",
        "<-04>4<-03>,M9.1.6/24,M4.1.6/24",
    ),
    ("America/Santo_Domingo", "Santo Domingo", "AST4"),
    ("Asia/Seoul", "Seoul", "KST-9"),
    ("Asia/Shanghai", "Shanghai", "CST-8"),
    ("Asia/Singapore", "Singapore", "<+08>-8"),
    (
        "America/St_Johns",
        "St. John's",
        "NST3:30NDT,M3.2.0,M11.1.0",
    ),
    (
        "Europe/Stockholm",
        "Stockholm",
        "CET-1CEST,M3.5.0,M10.5.0/3",
    ),
    ("Australia/Sydney", "Sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("America/Sao_Paulo", "São Paulo", "<-03>3"),
    ("Asia/Taipei", "Taipei", "CST-8"),
    ("Asia/Tehran", "Tehran", "<+0330>-3:30"),
    ("America/Tijuana", "Tijuana", "PST8PDT,M3.2.0,M11.1.0"),
    ("Asia/Tokyo", "Tokyo", "JST-9"),
    ("America/Toronto", "Toronto", "EST5EDT,M3.2.0,M11.1.0"),
    ("UTC", "UTC", "UTC0"),
    ("America/Vancouver", "Vancouver", "MST7"),
    ("Europe/Vienna", "Vienna", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Warsaw", "Warsaw", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("America/Winnipeg", "Winnipeg", "CST6CDT,M3.2.0,M11.1.0"),
    ("Europe/Zurich", "Zurich", "CET-1CEST,M3.5.0,M10.5.0/3"),
];

pub fn city(id: &str) -> Option<&'static str> {
    ZONES
        .iter()
        .find(|(zone, _, _)| *zone == id)
        .map(|(_, city, _)| *city)
}

/// A POSIX TZ rule (`EST5EDT,M3.2.0,M11.1.0`): offsets in seconds east of
/// UTC, and daylight time's start and end when the zone has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rule {
    standard: i32,
    daylight: Option<Daylight>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Daylight {
    offset: i32,
    start: Transition,
    end: Transition,
}

/// `Mm.w.d/time`: weekday d (0 = Sunday) of week w (5 = the last) of month
/// m, at `time` seconds past local midnight (may be negative or past 24 h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transition {
    month: u32,
    week: u32,
    weekday: u32,
    time: i32,
}

/// Parses `[+-]hh[:mm[:ss]]` from the start of `text`: seconds and the rest.
fn parse_duration(text: &str) -> Option<(i32, &str)> {
    let (sign, text) = match text.as_bytes().first()? {
        b'-' => (-1, &text[1..]),
        b'+' => (1, &text[1..]),
        _ => (1, text),
    };
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == ':'))
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let mut seconds = 0;
    let mut unit = 3600;
    for part in text[..end].split(':') {
        seconds += part.parse::<i32>().ok()? * unit;
        unit /= 60;
    }
    Some((sign * seconds, &text[end..]))
}

/// Skips a zone name: `<...>` or letters.
fn skip_name(text: &str) -> Option<&str> {
    match text.strip_prefix('<') {
        Some(quoted) => quoted.split_once('>').map(|(_, rest)| rest),
        None => {
            let rest = text.trim_start_matches(|c: char| c.is_ascii_alphabetic());
            (rest.len() < text.len()).then_some(rest)
        }
    }
}

fn parse_transition(text: &str) -> Option<Transition> {
    let (date, time) = match text.split_once('/') {
        Some((date, time)) => (date, parse_duration(time)?.0),
        None => (text, 2 * 3600),
    };
    let mut fields = date.strip_prefix('M')?.split('.');
    let mut next = || fields.next()?.parse::<u32>().ok();
    let (month, week, weekday) = (next()?, next()?, next()?);
    ((1..=12).contains(&month) && (1..=5).contains(&week) && weekday <= 6).then_some(Transition {
        month,
        week,
        weekday,
        time,
    })
}

fn parse(rule: &str) -> Option<Rule> {
    let rest = skip_name(rule)?;
    // POSIX counts west as positive: `EST5` is UTC-5.
    let (west, rest) = parse_duration(rest)?;
    let standard = -west;
    if rest.is_empty() {
        return Some(Rule {
            standard,
            daylight: None,
        });
    }
    let rest = skip_name(rest)?;
    let (offset, rest) = match parse_duration(rest) {
        Some((west, rest)) => (-west, rest),
        None => (standard + 3600, rest),
    };
    let (start, end) = rest.strip_prefix(',')?.split_once(',')?;
    Some(Rule {
        standard,
        daylight: Some(Daylight {
            offset,
            start: parse_transition(start)?,
            end: parse_transition(end)?,
        }),
    })
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn year_of(days: i64) -> i64 {
    // The inverse of days_from_civil, as far as the year.
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    year_of_era + era * 400 + i64::from(month_index >= 10)
}

/// The transition in `year`, in seconds since the epoch of local time with
/// `offset` (the offset in force before it).
fn transition_at(transition: Transition, year: i64, offset: i32) -> i64 {
    let first = days_from_civil(year, transition.month, 1);
    // 1970-01-01 was a Thursday (4).
    let first_weekday = (first + 4).rem_euclid(7) as u32;
    let mut day = 1 + (transition.weekday + 7 - first_weekday) % 7 + (transition.week - 1) * 7;
    let (next_year, next_month) = if transition.month == 12 {
        (year + 1, 1)
    } else {
        (year, transition.month + 1)
    };
    let month_days = (days_from_civil(next_year, next_month, 1) - first) as u32;
    while day > month_days {
        day -= 7;
    }
    (first + i64::from(day) - 1) * 86_400 + i64::from(transition.time) - i64::from(offset)
}

/// The zone's offset from UTC at `now` (Unix seconds), in minutes east:
/// daylight time included.
pub fn current_offset(rule: &str, now: i64) -> i32 {
    let Some(rule) = parse(rule) else {
        return 0;
    };
    let Some(daylight) = rule.daylight else {
        return rule.standard / 60;
    };
    let year = year_of((now + i64::from(rule.standard)).div_euclid(86_400));
    let start = transition_at(daylight.start, year, rule.standard);
    let end = transition_at(daylight.end, year, daylight.offset);
    let in_daylight = if start < end {
        (start..end).contains(&now)
    } else {
        // Southern hemisphere: daylight time spans the new year.
        !(end..start).contains(&now)
    };
    if in_daylight {
        daylight.offset / 60
    } else {
        rule.standard / 60
    }
}

/// "UTC−5", "UTC+5:30", "UTC" for an offset in minutes east.
pub fn offset_label(minutes: i32) -> String {
    if minutes == 0 {
        return "UTC".into();
    }
    let sign = if minutes < 0 { '−' } else { '+' };
    let (h, m) = (minutes.abs() / 60, minutes.abs() % 60);
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix seconds of a UTC date and time.
    fn utc(year: i64, month: u32, day: u32, hour: i64, minute: i64) -> i64 {
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60
    }

    #[test]
    fn offsets_read_like_utc() {
        assert_eq!(offset_label(-300), "UTC−5");
        assert_eq!(offset_label(330), "UTC+5:30");
        assert_eq!(offset_label(0), "UTC");
        assert_eq!(current_offset("IST-5:30", 0), 330);
        assert_eq!(current_offset("<+0545>-5:45", 0), 345);
        assert_eq!(current_offset("<-03>3", 0), -180);
        assert_eq!(current_offset("UTC0", 0), 0);
        assert_eq!(city("America/Panama"), Some("Panama"));
        assert_eq!(utc(1970, 1, 1, 0, 0), 0);
        assert_eq!(year_of(days_from_civil(2026, 12, 31)), 2026);
        assert_eq!(year_of(days_from_civil(2027, 1, 1)), 2027);
    }

    #[test]
    fn daylight_time_follows_the_rules() {
        let new_york = "EST5EDT,M3.2.0,M11.1.0";
        // 2026: 8 March 02:00 EST to 1 November 02:00 EDT.
        assert_eq!(current_offset(new_york, utc(2026, 10, 27, 8, 0)), -240);
        assert_eq!(current_offset(new_york, utc(2026, 12, 1, 12, 0)), -300);
        assert_eq!(current_offset(new_york, utc(2026, 3, 8, 6, 59)), -300);
        assert_eq!(current_offset(new_york, utc(2026, 3, 8, 7, 0)), -240);
        assert_eq!(current_offset(new_york, utc(2026, 11, 1, 5, 59)), -240);
        assert_eq!(current_offset(new_york, utc(2026, 11, 1, 6, 0)), -300);
        // Europe: the last Sunday of March 01:00 UTC to October's.
        let berlin = "CET-1CEST,M3.5.0,M10.5.0/3";
        assert_eq!(current_offset(berlin, utc(2026, 3, 29, 0, 59)), 60);
        assert_eq!(current_offset(berlin, utc(2026, 3, 29, 1, 0)), 120);
        assert_eq!(current_offset(berlin, utc(2026, 10, 25, 0, 59)), 120);
        assert_eq!(current_offset(berlin, utc(2026, 10, 25, 1, 0)), 60);
        // Southern hemisphere, half-hour zone: daylight time over the new year.
        let adelaide = "ACST-9:30ACDT,M10.1.0,M4.1.0/3";
        assert_eq!(current_offset(adelaide, utc(2026, 1, 15, 0, 0)), 630);
        assert_eq!(current_offset(adelaide, utc(2026, 6, 15, 0, 0)), 570);
        // Santiago: transitions at 24:00 on a Saturday.
        let santiago = "<-04>4<-03>,M9.1.6/24,M4.1.6/24";
        assert_eq!(current_offset(santiago, utc(2026, 1, 15, 0, 0)), -180);
        assert_eq!(current_offset(santiago, utc(2026, 7, 15, 0, 0)), -240);
        // Every zone offered parses.
        for (id, _, rule) in ZONES {
            assert!(parse(rule).is_some(), "{id}: {rule}");
        }
    }
}
