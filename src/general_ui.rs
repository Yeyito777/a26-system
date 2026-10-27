//! Battery, Date & Time (with its time-zone picker) and Storage pages.
//! Time settings go to Moon (`settings set timezone|clock_24h`), which
//! applies them to itself and the apps; the battery is read from the kernel
//! (battery.rs) and storage counted on a worker (storage.rs).

use std::time::Instant;

use crate::app::{Action, AppModel, View};
use crate::battery::{self, BatteryInfo};
use crate::gfx::{Canvas, RectF, Rgb};
use crate::metrics::{SystemInfo, format_bytes};
use crate::text::{self, Weight};
use crate::tz::{self, ZONES};
use crate::ui::{
    BACK_BUTTON, BLUE, CARD_WIDTH, GREEN, LABEL_SIZE, MARGIN, RED, ROW, SECONDARY, SECONDARY_ALPHA,
    TEXT_X, TRACK, WHITE, back_chevron, baseline, card, chevron, footer, grouped_card, info_rows,
    large_title, section_header, separator,
};

const RIGHT: f32 = MARGIN + CARD_WIDTH - 44.0;
const TOP: f32 = 340.0;
/// Moon owns the bottom of the window for its swipe-up gesture.
const GESTURE_RESERVE: f32 = 170.0;
const ORANGE: Rgb = 0xff9f0a;
const GRAY: Rgb = 0x8e8e93;

fn back(canvas: &mut Canvas, label: &str, pressed: bool) {
    back_chevron(canvas, MARGIN + 6.0, 72.0);
    text::draw(
        canvas,
        label,
        MARGIN + 36.0,
        baseline(72.0, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        BLUE,
        if pressed { 0.4 } else { 1.0 },
    );
}

pub fn action_at(app: &AppModel, height: f32, x: f32, y: f32) -> Option<Action> {
    if BACK_BUTTON.contains(x, y) {
        return Some(if app.view == View::TimeZone {
            Action::ShowDateTime
        } else {
            Action::ShowSystem
        });
    }
    match app.view {
        View::DateTime => {
            if card(TOP, ROW).contains(x, y) {
                Some(Action::Toggle24h)
            } else if card(TOP + ROW, ROW).contains(x, y) {
                Some(Action::ShowTimeZone)
            } else {
                None
            }
        }
        View::TimeZone => {
            let (top, bottom) = (TOP, height - GESTURE_RESERVE);
            if y < top || y >= bottom || !card(top, bottom - top).contains(x, y) {
                return None;
            }
            let index = ((y - top + app.tz_scroll.value(Instant::now())) / ROW).floor();
            (index >= 0.0 && (index as usize) < ZONES.len())
                .then_some(Action::SetTimeZone(index as usize))
        }
        _ => None,
    }
}

/// The picker's scroll range.
pub fn zone_scroll_max(height: f32) -> f32 {
    (ZONES.len() as f32 * ROW - (height - GESTURE_RESERVE - TOP)).max(0.0)
}

/// A touch at `y` on the picker may scroll it.
pub fn zone_scrolls(height: f32, y: f32) -> bool {
    y >= TOP && y < height - GESTURE_RESERVE
}

pub fn draw(canvas: &mut Canvas, app: &AppModel, system: &SystemInfo, pressed: Option<Action>) {
    match app.view {
        View::Battery => draw_battery(canvas, &app.battery, pressed),
        View::DateTime => draw_date_time(canvas, app, pressed),
        View::TimeZone => draw_time_zone(canvas, app, pressed),
        View::Storage => draw_storage(canvas, app, system, pressed),
        _ => {}
    }
}

fn level_color(info: &BatteryInfo) -> Rgb {
    match info.percent {
        _ if info.plugged() => GREEN,
        Some(p) if p <= 20 => RED,
        _ => WHITE,
    }
}

fn draw_battery(canvas: &mut Canvas, info: &BatteryInfo, pressed: Option<Action>) {
    back(canvas, "System", pressed == Some(Action::ShowSystem));
    large_title(canvas, "Battery", 236.0);
    // The level, its state and a bar.
    let level_card = card(TOP, 236.0);
    grouped_card(canvas, level_card, 0, None);
    let label = info.percent.map_or_else(|| "—".into(), |p| format!("{p}%"));
    let width = text::draw(
        canvas,
        &label,
        TEXT_X - 3.0,
        TOP + 116.0,
        Weight::DisplaySemiBold,
        96.0,
        WHITE,
        1.0,
    );
    text::draw(
        canvas,
        info.state_text(),
        TEXT_X + width + 28.0,
        TOP + 112.0,
        Weight::Regular,
        LABEL_SIZE,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    let bar = RectF::new(TEXT_X, TOP + 160.0, CARD_WIDTH - 88.0, 30.0);
    canvas.fill_rrect(bar, 10.0, TRACK, 1.0);
    if let Some(p) = info.percent.filter(|p| *p > 0) {
        let right = (bar.x + bar.w * f32::from(p) / 100.0).round() as i32;
        canvas.set_clip(0, 0, right, canvas.height);
        canvas.fill_rrect(bar, 10.0, level_color(info), 1.0);
        canvas.reset_clip();
    }

    // What the charge left is good for.
    let estimate_y = TOP + 236.0 + 120.0;
    section_header(canvas, "ESTIMATE", estimate_y);
    let rows: Vec<(&str, String)> = if info.plugged() {
        let full = match (info.status.as_str(), info.time_to_full_s) {
            ("Full", _) => "Charged".to_owned(),
            (_, Some(seconds)) => format!("In {}", battery::time_text(seconds)),
            _ => "Charging".to_owned(),
        };
        vec![("Full", full)]
    } else {
        let at = |ma| {
            info.hours_at(ma)
                .map_or_else(|| "—".to_owned(), battery::duration_text)
        };
        vec![
            ("Locked, screen off", at(battery::STANDBY_MA)),
            ("Screen on", at(battery::SCREEN_ON_MA)),
        ]
    };
    info_rows(canvas, estimate_y, &rows);
    let rows_bottom = estimate_y + crate::ui::INFO_ROW * rows.len() as f32;
    footer(
        canvas,
        "Estimated from this phone's measured drain.",
        rows_bottom,
        SECONDARY,
        SECONDARY_ALPHA,
    );

    let details_y = rows_bottom + 200.0;
    section_header(canvas, "BATTERY", details_y);
    let current = info.current_ma.map_or_else(
        || "—".to_owned(),
        |ma| {
            if info.plugged() && ma > 0 {
                format!("{ma} mA in")
            } else {
                format!("{} mA", ma.abs())
            }
        },
    );
    let details = [
        (
            "Health",
            if info.health.is_empty() {
                "—".to_owned()
            } else {
                info.health.clone()
            },
        ),
        (
            "Temperature",
            info.temperature_c
                .map_or_else(|| "—".into(), |c| format!("{c:.1} °C")),
        ),
        (
            "Charge Cycles",
            info.cycles.map_or_else(|| "—".into(), |c| c.to_string()),
        ),
        ("Current", current),
        (
            "Voltage",
            info.voltage_mv
                .map_or_else(|| "—".into(), |mv| format!("{:.2} V", mv as f32 / 1000.0)),
        ),
        (
            "Capacity",
            info.capacity_mah
                .map_or_else(|| "—".into(), |mah| format!("{mah} mAh")),
        ),
    ];
    info_rows(canvas, details_y, &details);
}

/// The zone the user chose, or the one in effect.
fn zone_label(app: &AppModel) -> String {
    let time = app.shell.time.as_ref();
    if let Some(city) = time
        .and_then(|time| time.zone.as_deref())
        .and_then(tz::city)
    {
        return city.to_owned();
    }
    time.and_then(|time| time.tz.as_deref())
        .and_then(|rule| ZONES.iter().find(|(_, _, r)| *r == rule))
        .map_or_else(|| "Not set".to_owned(), |(_, city, _)| (*city).to_owned())
}

fn draw_date_time(canvas: &mut Canvas, app: &AppModel, pressed: Option<Action>) {
    back(canvas, "System", pressed == Some(Action::ShowSystem));
    large_title(canvas, "Date & Time", 236.0);
    let pressed_row = match pressed {
        Some(Action::Toggle24h) => Some(0),
        Some(Action::ShowTimeZone) => Some(1),
        _ => None,
    };
    grouped_card(canvas, card(TOP, ROW * 2.0), 2, pressed_row);
    let on = app.shell.time.as_ref().is_some_and(|time| time.clock_24h);
    let cy = TOP + ROW / 2.0;
    text::draw(
        canvas,
        "24-Hour Time",
        TEXT_X,
        baseline(cy, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    crate::display_ui::draw_switch(
        canvas,
        RectF::new(RIGHT - 104.0, TOP + (ROW - 64.0) / 2.0, 104.0, 64.0),
        on,
    );
    separator(canvas, TOP + ROW - 1.0, TEXT_X);
    let cy = TOP + ROW * 1.5;
    let line = baseline(cy, Weight::Regular, LABEL_SIZE);
    text::draw(
        canvas,
        "Time Zone",
        TEXT_X,
        line,
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    chevron(canvas, RIGHT, cy, SECONDARY, crate::ui::TERTIARY_ALPHA);
    text::draw_right(
        canvas,
        &zone_label(app),
        RIGHT - 40.0,
        line,
        Weight::Regular,
        LABEL_SIZE,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    // The time now, as every clock will show it.
    let now = crate::clock::unix_now();
    footer(
        canvas,
        &format!("Now {}", crate::clock::local_date_time_text(now)),
        TOP + ROW * 2.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}

fn draw_time_zone(canvas: &mut Canvas, app: &AppModel, pressed: Option<Action>) {
    let height = canvas.height as f32;
    let (top, bottom) = (TOP, height - GESTURE_RESERVE);
    let offset = app.tz_scroll.value(Instant::now());
    let chosen = app.shell.time.as_ref().and_then(|time| time.zone.clone());
    canvas.set_clip(0, top as i32, canvas.width, bottom as i32);
    let list = card(top - offset, ROW * ZONES.len() as f32);
    grouped_card(canvas, list, 0, None);
    for (index, (id, city, rule)) in ZONES.iter().enumerate() {
        let y = top + index as f32 * ROW - offset;
        if y + ROW < top || y > bottom {
            continue;
        }
        if pressed == Some(Action::SetTimeZone(index)) {
            canvas.fill_rect(
                MARGIN as i32,
                y as i32,
                CARD_WIDTH as i32,
                ROW as i32,
                0x3a3a3c,
                1.0,
            );
        }
        if index > 0 {
            separator(canvas, y - 1.0, TEXT_X);
        }
        let cy = y + ROW / 2.0;
        let line = baseline(cy, Weight::Regular, LABEL_SIZE);
        text::draw(
            canvas,
            city,
            TEXT_X,
            line,
            Weight::Regular,
            LABEL_SIZE,
            WHITE,
            1.0,
        );
        let right = if chosen.as_deref() == Some(*id) {
            draw_check(canvas, RIGHT - 6.0, cy);
            RIGHT - 60.0
        } else {
            RIGHT
        };
        text::draw_right(
            canvas,
            &tz::offset_label(rule),
            right,
            line,
            Weight::Regular,
            LABEL_SIZE,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
    canvas.reset_clip();
    // The title and back button over the list's top.
    canvas.fill_rect(0, 0, canvas.width, top as i32 - 1, 0x000000, 1.0);
    back(canvas, "Date & Time", pressed == Some(Action::ShowDateTime));
    large_title(canvas, "Time Zone", 236.0);
}

fn draw_check(canvas: &mut Canvas, x: f32, cy: f32) {
    let mut path = crate::gfx::Path::new();
    path.stroke_polyline(
        &[(x - 26.0, cy), (x - 12.0, cy + 15.0), (x + 12.0, cy - 16.0)],
        7.0,
    );
    canvas.fill_path(&path, BLUE, 1.0);
}

fn draw_storage(canvas: &mut Canvas, app: &AppModel, system: &SystemInfo, pressed: Option<Action>) {
    back(canvas, "System", pressed == Some(Action::ShowSystem));
    large_title(canvas, "Storage", 236.0);
    let Some((used, free)) = system.storage_bytes else {
        footer(
            canvas,
            "Storage information is unavailable.",
            TOP,
            SECONDARY,
            SECONDARY_ALPHA,
        );
        return;
    };
    let total = used + free;
    let summary = card(TOP, 236.0);
    grouped_card(canvas, summary, 0, None);
    text::draw(
        canvas,
        &format!("{} of {} used", format_bytes(used), format_bytes(total)),
        TEXT_X,
        TOP + 82.0,
        Weight::SemiBold,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    let bar = RectF::new(TEXT_X, TOP + 124.0, CARD_WIDTH - 88.0, 30.0);
    canvas.fill_rrect(bar, 8.0, TRACK, 1.0);
    // Stacked: OSS Phone system, its data, then Android and the rest.
    let (system_bytes, data_bytes) = app
        .storage
        .map_or((0, 0), |breakdown| (breakdown.system, breakdown.data));
    let other = used.saturating_sub(system_bytes + data_bytes);
    let segments = [(system_bytes, BLUE), (data_bytes, ORANGE), (other, GRAY)];
    let mut x = bar.x;
    for (bytes, color) in segments {
        let width = bar.w * bytes as f32 / total.max(1) as f32;
        if width >= 1.0 {
            canvas.set_clip(
                x.round() as i32,
                0,
                (x + width).round() as i32,
                canvas.height,
            );
            canvas.fill_rrect(bar, 8.0, color, 1.0);
            canvas.reset_clip();
        }
        x += width;
    }
    let legend_y = TOP + 204.0;
    let mut lx = TEXT_X;
    for (label, color) in [
        ("OSS Phone", BLUE),
        ("Data", ORANGE),
        ("Android & other", GRAY),
    ] {
        canvas.fill_circle(lx + 10.0, legend_y - 12.0, 10.0, color, 1.0);
        let w = text::draw(
            canvas,
            label,
            lx + 32.0,
            legend_y,
            Weight::Regular,
            36.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
        lx += 32.0 + w + 40.0;
    }
    let rows_y = TOP + 236.0 + 120.0;
    section_header(canvas, "USED", rows_y);
    let counted = |bytes: u64| {
        if app.storage.is_some() {
            format_bytes(bytes)
        } else {
            "Calculating…".to_owned()
        }
    };
    let rows = [
        ("OSS Phone", counted(system_bytes)),
        ("Data", counted(data_bytes)),
        ("Android & other", counted(other)),
        ("Free", format_bytes(free)),
    ];
    info_rows(canvas, rows_y, &rows);
    footer(
        canvas,
        "Data: the apps' settings, messages and history.",
        rows_y + crate::ui::INFO_ROW * rows.len() as f32,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}
