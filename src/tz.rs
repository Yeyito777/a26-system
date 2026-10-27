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

/// A rule's standard offset from UTC in minutes (POSIX counts west as
/// positive: `EST5` is UTC-5).
pub fn standard_offset(rule: &str) -> i32 {
    let rest = match rule.strip_prefix('<') {
        Some(quoted) => quoted.split_once('>').map_or("", |(_, rest)| rest),
        None => rule.trim_start_matches(|c: char| c.is_ascii_alphabetic()),
    };
    let (sign, rest) = match rest.as_bytes().first() {
        Some(b'-') => (1, &rest[1..]),
        Some(b'+') => (-1, &rest[1..]),
        _ => (-1, rest),
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ':'))
        .unwrap_or(rest.len());
    let mut parts = rest[..end].split(':');
    let hours: i32 = parts.next().and_then(|h| h.parse().ok()).unwrap_or(0);
    let minutes: i32 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    sign * (hours * 60 + minutes)
}

/// "UTC−5", "UTC+5:30", "UTC".
pub fn offset_label(rule: &str) -> String {
    let minutes = standard_offset(rule);
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

    #[test]
    fn offsets_read_like_utc() {
        assert_eq!(offset_label("EST5EDT,M3.2.0,M11.1.0"), "UTC−5");
        assert_eq!(offset_label("IST-5:30"), "UTC+5:30");
        assert_eq!(offset_label("<+0545>-5:45"), "UTC+5:45");
        assert_eq!(offset_label("<-03>3"), "UTC−3");
        assert_eq!(offset_label("UTC0"), "UTC");
        assert_eq!(city("America/Panama"), Some("Panama"));
    }
}
