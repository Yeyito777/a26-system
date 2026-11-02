//! Times as WhatsApp shows them, in Moon's zone and clock: "14:05" today,
//! "Yesterday", the weekday this week, else the date.

#![allow(dead_code)]

const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

pub fn local(t_secs: i64) -> libc::tm {
    // SAFETY: localtime_r writes into a zeroed tm it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = t_secs as _;
        libc::localtime_r(&time, &mut tm);
        tm
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A local day as a number (days since 1970 in local time): for "same day".
pub fn day_of(t_ms: i64) -> i64 {
    let tm = local(t_ms / 1000);
    days_from_civil(i64::from(tm.tm_year) + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
}

/// Local days of many times in order (a chat's messages): one look at the
/// zone per day, not per time.
#[derive(Default)]
pub struct Days {
    /// The last day seen: its first and last millisecond, and its number.
    span: Option<(i64, i64, i64)>,
}

impl Days {
    pub fn day(&mut self, t_ms: i64) -> i64 {
        if let Some((start, end, day)) = self.span
            && (start..end).contains(&t_ms)
        {
            return day;
        }
        let tm = local(t_ms.div_euclid(1000));
        let day = days_from_civil(i64::from(tm.tm_year) + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32);
        let into = (i64::from(tm.tm_hour) * 3600 + i64::from(tm.tm_min) * 60 + i64::from(tm.tm_sec)) * 1000
            + t_ms.rem_euclid(1000);
        let start = t_ms - into;
        // A day with a clock change is an hour shorter or longer: the span
        // stops short of the next midnight rather than past it.
        self.span = Some((start, start + 23 * 3_600_000, day));
        day
    }
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "14:05" or "2:05 PM".
pub fn clock(t_ms: i64) -> String {
    let tm = local(t_ms / 1000);
    crate::clock::clock_text(tm.tm_hour as u8, tm.tm_min as u8)
}

fn numeric_date(tm: &libc::tm) -> String {
    let (d, m, y) = (tm.tm_mday, tm.tm_mon + 1, (tm.tm_year + 1900) % 100);
    if crate::control::clock_24h() {
        format!("{d}/{m}/{y:02}")
    } else {
        format!("{m}/{d}/{y:02}")
    }
}

/// The chat list's time: today's clock, "Yesterday", a weekday, a date.
pub fn list_time(t_ms: i64) -> String {
    if t_ms <= 0 {
        return String::new();
    }
    let today = day_of(now_secs() * 1000);
    let day = day_of(t_ms);
    let tm = local(t_ms / 1000);
    match today - day {
        0 => clock(t_ms),
        1 => "Yesterday".into(),
        2..=6 => WEEKDAYS[tm.tm_wday as usize % 7].into(),
        _ => numeric_date(&tm),
    }
}

/// A day chip in a chat: "Today", "Yesterday", a weekday, "October 28, 2026".
pub fn day_chip(t_ms: i64) -> String {
    let today = day_of(now_secs() * 1000);
    let day = day_of(t_ms);
    let tm = local(t_ms / 1000);
    match today - day {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => WEEKDAYS[tm.tm_wday as usize % 7].into(),
        _ => {
            let month = MONTHS[tm.tm_mon as usize % 12];
            if tm.tm_year == local(now_secs()).tm_year {
                format!("{month} {}", tm.tm_mday)
            } else {
                format!("{month} {}, {}", tm.tm_mday, tm.tm_year + 1900)
            }
        }
    }
}

/// A voice note's or video's length: "0:42", "12:05".
pub fn duration(ms: u64) -> String {
    let s = ms.div_ceil(1000);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A file's size: "182 kB", "2.4 MB".
pub fn size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else if bytes >= 1000 {
        format!("{} kB", bytes / 1000)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_sizes() {
        assert_eq!(duration(42_000), "0:42");
        assert_eq!(duration(725_400), "12:06");
        assert_eq!(size(182_400), "182 kB");
        assert_eq!(size(2_400_000), "2.4 MB");
        assert_eq!(days_from_civil(1970, 1, 2), 1);
    }
}
