//! The battery as the kernel reports it (Samsung's `battery` power supply),
//! for the Battery page, and what it is good for: hours left estimated from
//! the drain measured in Moon mode (OSS-Phone-V1 docs/battery.md).

use std::fs;

const ROOT: &str = "/sys/class/power_supply/battery";
/// Measured averages (docs/battery.md): locked with the screen off, and the
/// home screen on at the default brightness.
pub const STANDBY_MA: f32 = 37.0;
pub const SCREEN_ON_MA: f32 = 213.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatteryInfo {
    pub percent: Option<u8>,
    /// `Charging`, `Discharging`, `Full`, `Not charging`.
    pub status: String,
    pub health: String,
    pub temperature_c: Option<f32>,
    pub cycles: Option<u32>,
    /// The current right now, in mA (Samsung's sign: negative drains).
    pub current_ma: Option<i32>,
    pub voltage_mv: Option<u32>,
    /// Rated capacity, mAh.
    pub capacity_mah: Option<u32>,
    /// While charging: seconds until full, as the charger estimates it.
    pub time_to_full_s: Option<u32>,
}

fn read(name: &str) -> Option<String> {
    let root = std::env::var("MOON_STATUS_ROOT")
        .map(|prefix| format!("{prefix}{ROOT}"))
        .unwrap_or_else(|_| ROOT.to_owned());
    fs::read_to_string(format!("{root}/{name}"))
        .ok()
        .map(|text| text.trim().to_owned())
}

fn number<T: std::str::FromStr>(name: &str) -> Option<T> {
    read(name)?.parse().ok()
}

impl BatteryInfo {
    pub fn read() -> Self {
        Self {
            percent: number::<u8>("capacity").map(|p| p.min(100)),
            status: read("status").unwrap_or_default(),
            health: read("health").unwrap_or_default(),
            temperature_c: number::<i32>("temp").map(|tenths| tenths as f32 / 10.0),
            cycles: number("battery_cycle"),
            current_ma: number("current_now"),
            voltage_mv: number::<u32>("voltage_now").map(|uv| uv / 1000),
            capacity_mah: number::<u32>("charge_full")
                .map(|uah| uah / 1000)
                .filter(|mah| *mah > 0),
            time_to_full_s: number::<u32>("time_to_full_now").filter(|s| *s > 0),
        }
    }

    pub fn plugged(&self) -> bool {
        matches!(self.status.as_str(), "Charging" | "Full")
    }

    /// The state line: "Charging", "Fully charged", "On battery".
    pub fn state_text(&self) -> &'static str {
        match self.status.as_str() {
            "Charging" => "Charging",
            "Full" => "Fully charged",
            "Not charging" => "Plugged in, not charging",
            _ => "On battery",
        }
    }

    /// Hours left at a drain of `ma`, from the charge that remains.
    pub fn hours_at(&self, ma: f32) -> Option<f32> {
        let remaining = self.capacity_mah? as f32 * f32::from(self.percent?) / 100.0;
        Some(remaining / ma)
    }
}

/// "About 3 days", "About 17 h", "About 40 min".
pub fn duration_text(hours: f32) -> String {
    if hours >= 48.0 {
        format!("About {} days", (hours / 24.0).round())
    } else if hours >= 1.0 {
        format!("About {} h", hours.round())
    } else {
        format!("About {} min", (hours * 60.0).round().max(1.0))
    }
}

/// "1 h 20 min", "35 min".
pub fn time_text(seconds: u32) -> String {
    let (h, m) = (seconds / 3600, seconds / 60 % 60);
    if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{} min", m.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_read_naturally() {
        let info = BatteryInfo {
            percent: Some(50),
            capacity_mah: Some(5000),
            ..BatteryInfo::default()
        };
        assert_eq!(info.hours_at(250.0), Some(10.0));
        assert_eq!(duration_text(10.0), "About 10 h");
        assert_eq!(duration_text(67.0), "About 3 days");
        assert_eq!(duration_text(0.5), "About 30 min");
        assert_eq!(time_text(4800), "1 h 20 min");
        assert_eq!(time_text(20), "1 min");
    }
}
