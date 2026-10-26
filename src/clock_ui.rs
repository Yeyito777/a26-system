//! The Clock page (alarms and timers) and its add/edit pages: drawing and
//! hit testing, in the System app's iOS grouped-list style. Clock accents
//! are orange, as in the iPhone Clock app.

use crate::app::Action;
use crate::clock::{
    AlarmEditor, ClockAction, ClockModel, EditorField, MAX_ALARM_ROWS, MAX_TIMER_ROWS,
    TIMER_PRESETS, TimerEditor, alarm_time, countdown, preset_label, repeat_text, unix_now,
};
use crate::gfx::{Canvas, Path, RectF};
use crate::text::{self, Weight};
use crate::ui::{
    BACK_BUTTON, CANCEL_BUTTON, CARD_WIDTH, CELL, GREEN, JOIN_BUTTON, LABEL_SIZE, MARGIN,
    NAV_HEIGHT, RED, ROW, SECONDARY, SECONDARY_ALPHA, TERTIARY_ALPHA, TEXT_X, TRACK, WHITE,
    back_chevron, baseline, card, chevron, footer, grouped_card, large_title, section_header,
    separator,
};

pub const ORANGE: u32 = 0xff9f0a;
const RIGHT: f32 = MARGIN + CARD_WIDTH - 44.0;

const ALARMS_Y: f32 = 340.0;
const ALARM_ROW: f32 = 168.0;
const TIMER_ROW: f32 = 150.0;
const ADD_BUTTON: RectF = RectF::new(860.0, 0.0, 220.0, 140.0);
const SWITCH_W: f32 = 104.0;
const SWITCH_H: f32 = 64.0;

/// Where everything on the Clock page is, from the current alarms/timers.
pub struct ClockLayout {
    pub alarm_rows: usize,
    pub alarms_card: RectF,
    pub timers_y: f32,
    pub timer_rows: usize,
    pub presets: Vec<RectF>,
    pub custom_row: RectF,
}

impl ClockLayout {
    pub fn new(model: &ClockModel) -> Self {
        let alarm_rows = model.alarms.len().clamp(1, MAX_ALARM_ROWS);
        let alarms_card = card(ALARMS_Y, ALARM_ROW * alarm_rows as f32);
        let timers_y = alarms_card.y + alarms_card.h + 120.0;
        let timer_rows = model.timers.len().min(MAX_TIMER_ROWS);
        let mut presets_y = timers_y;
        if timer_rows > 0 {
            presets_y += TIMER_ROW * timer_rows as f32 + 40.0;
        }
        let chip_w = (CARD_WIDTH - 64.0 - 5.0 * 16.0) / 6.0;
        let presets = (0..TIMER_PRESETS.len())
            .map(|i| {
                RectF::new(
                    MARGIN + 32.0 + i as f32 * (chip_w + 16.0),
                    presets_y + 20.0,
                    chip_w,
                    88.0,
                )
            })
            .collect();
        Self {
            alarm_rows,
            alarms_card,
            timers_y,
            timer_rows,
            presets,
            custom_row: card(presets_y + ROW, ROW),
        }
    }

    fn alarm_row(&self, index: usize) -> RectF {
        RectF::new(
            self.alarms_card.x,
            self.alarms_card.y + ALARM_ROW * index as f32,
            self.alarms_card.w,
            ALARM_ROW,
        )
    }

    fn switch(&self, index: usize) -> RectF {
        let row = self.alarm_row(index);
        RectF::new(
            RIGHT - SWITCH_W,
            row.y + (ALARM_ROW - SWITCH_H) / 2.0,
            SWITCH_W,
            SWITCH_H,
        )
    }

    fn timer_row(&self, index: usize) -> RectF {
        card(self.timers_y + TIMER_ROW * index as f32, TIMER_ROW)
    }

    /// The pause/resume and cancel buttons of a timer row.
    fn timer_buttons(&self, index: usize) -> (RectF, RectF) {
        let row = self.timer_row(index);
        let cy = row.y + TIMER_ROW / 2.0;
        (
            RectF::new(RIGHT - 104.0 - 120.0, cy - 52.0, 104.0, 104.0),
            RectF::new(RIGHT - 104.0, cy - 52.0, 104.0, 104.0),
        )
    }
}

// Alarm editor geometry.
const TIME_Y: f32 = 470.0;
const HOUR_X: f32 = 400.0;
const MINUTE_X: f32 = 690.0;
const STEP_UP_Y: f32 = 190.0;
const STEP_DOWN_Y: f32 = 560.0;
const STEP: (f32, f32) = (190.0, 110.0);
const MERIDIEM: RectF = RectF::new(860.0, 340.0, 170.0, 150.0);
const QUICK_Y: f32 = 720.0;
const REPEAT_Y: f32 = 920.0;
const LABEL_Y: f32 = 1170.0;
const DELETE_Y: f32 = 1400.0;

fn step_button(column_x: f32, up: bool) -> RectF {
    RectF::new(
        column_x - STEP.0 / 2.0,
        if up { STEP_UP_Y } else { STEP_DOWN_Y },
        STEP.0,
        STEP.1,
    )
}

fn quick_minutes() -> [(u8, RectF); 4] {
    let w = (CARD_WIDTH - 3.0 * 24.0) / 4.0;
    [0, 15, 30, 45].map(|minute| {
        let index = f32::from(minute / 15);
        (
            minute,
            RectF::new(MARGIN + index * (w + 24.0), QUICK_Y, w, 96.0),
        )
    })
}

fn day_circle(day: u8) -> RectF {
    let pitch = CARD_WIDTH / 7.0;
    let cx = MARGIN + pitch * (f32::from(day) + 0.5);
    RectF::new(cx - 56.0, REPEAT_Y + 16.0, 112.0, 112.0)
}

fn label_row() -> RectF {
    card(LABEL_Y, ROW)
}

fn delete_row() -> RectF {
    card(DELETE_Y, ROW)
}

// Timer editor geometry: hours, minutes, seconds columns.
const TIMER_COLUMNS: [(EditorField, f32); 3] = [
    (EditorField::Hour, 250.0),
    (EditorField::Minute, 540.0),
    (EditorField::Second, 830.0),
];
const TIMER_LABEL_Y: f32 = 820.0;

/// Maps a tap on the Clock pages to an action.
pub fn action_at(model: &ClockModel, x: f32, y: f32) -> Option<Action> {
    let clock = |action| Some(Action::Clock(action));
    if let Some(editor) = &model.alarm_editor {
        if CANCEL_BUTTON.contains(x, y) {
            return clock(ClockAction::Cancel);
        }
        if JOIN_BUTTON.contains(x, y) {
            return clock(ClockAction::Save);
        }
        for (field, column) in [(EditorField::Hour, HOUR_X), (EditorField::Minute, MINUTE_X)] {
            if step_button(column, true).contains(x, y) {
                return clock(ClockAction::Step(field, 1));
            }
            if step_button(column, false).contains(x, y) {
                return clock(ClockAction::Step(field, -1));
            }
        }
        if MERIDIEM.contains(x, y) {
            return clock(ClockAction::Step(EditorField::Hour, 12));
        }
        for (minute, rect) in quick_minutes() {
            if rect.contains(x, y) {
                return clock(ClockAction::QuickMinute(minute));
            }
        }
        for day in 0..7 {
            if day_circle(day).contains(x, y) {
                return clock(ClockAction::ToggleDay(day));
            }
        }
        if label_row().contains(x, y) {
            return clock(ClockAction::FocusLabel);
        }
        if editor.id.is_some() && delete_row().contains(x, y) {
            return clock(ClockAction::DeleteAlarm);
        }
        return None;
    }
    if model.timer_editor.is_some() {
        if CANCEL_BUTTON.contains(x, y) {
            return clock(ClockAction::Cancel);
        }
        if JOIN_BUTTON.contains(x, y) {
            return clock(ClockAction::Save);
        }
        for (field, column) in TIMER_COLUMNS {
            if step_button(column, true).contains(x, y) {
                return clock(ClockAction::Step(field, 1));
            }
            if step_button(column, false).contains(x, y) {
                return clock(ClockAction::Step(field, -1));
            }
        }
        if card(TIMER_LABEL_Y, ROW).contains(x, y) {
            return clock(ClockAction::FocusLabel);
        }
        return None;
    }
    if BACK_BUTTON.contains(x, y) {
        return Some(Action::ShowSystem);
    }
    if ADD_BUTTON.contains(x, y) {
        return clock(ClockAction::NewAlarm);
    }
    let layout = ClockLayout::new(model);
    for index in 0..model.alarms.len().min(layout.alarm_rows) {
        if layout.switch(index).inset(-20.0).contains(x, y) {
            return clock(ClockAction::ToggleAlarm(index));
        }
        if layout.alarm_row(index).contains(x, y) {
            return clock(ClockAction::EditAlarm(index));
        }
    }
    for index in 0..layout.timer_rows {
        let (pause, cancel) = layout.timer_buttons(index);
        if pause.inset(-12.0).contains(x, y) {
            return clock(ClockAction::PauseOrResume(index));
        }
        if cancel.inset(-12.0).contains(x, y) {
            return clock(ClockAction::CancelTimer(index));
        }
    }
    for (index, chip) in layout.presets.iter().enumerate() {
        if chip.inset(-8.0).contains(x, y) {
            return clock(ClockAction::StartPreset(index));
        }
    }
    if layout.custom_row.contains(x, y) {
        return clock(ClockAction::NewTimer);
    }
    None
}

fn pressed_alpha(pressed: Option<Action>, action: Action) -> f32 {
    if pressed == Some(action) { 0.4 } else { 1.0 }
}

/// The Clock page.
pub fn draw_clock(canvas: &mut Canvas, model: &ClockModel, pressed: Option<Action>) {
    if let Some(editor) = &model.alarm_editor {
        draw_alarm_editor(canvas, editor, pressed);
        return;
    }
    if let Some(editor) = &model.timer_editor {
        draw_timer_editor(canvas, editor, pressed);
        return;
    }
    back_chevron(canvas, MARGIN + 6.0, 72.0);
    text::draw(
        canvas,
        "System",
        MARGIN + 36.0,
        baseline(72.0, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        ORANGE,
        pressed_alpha(pressed, Action::ShowSystem),
    );
    draw_plus(
        canvas,
        (1080.0 - MARGIN - 30.0, 72.0),
        ORANGE,
        pressed_alpha(pressed, Action::Clock(ClockAction::NewAlarm)),
    );
    large_title(canvas, "Clock", 236.0);

    let layout = ClockLayout::new(model);
    section_header(canvas, "ALARMS", ALARMS_Y);
    let pressed_row = (0..model.alarms.len())
        .find(|i| pressed == Some(Action::Clock(ClockAction::EditAlarm(*i))));
    grouped_card(canvas, layout.alarms_card, layout.alarm_rows, pressed_row);
    if model.alarms.is_empty() {
        let message = if model.connected {
            "No Alarms"
        } else {
            "Clock service unavailable"
        };
        text::draw(
            canvas,
            message,
            TEXT_X,
            baseline(ALARMS_Y + ALARM_ROW / 2.0, Weight::Regular, LABEL_SIZE),
            Weight::Regular,
            LABEL_SIZE,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
    for (index, alarm) in model.alarms.iter().take(layout.alarm_rows).enumerate() {
        let row = layout.alarm_row(index);
        if index > 0 {
            separator(canvas, row.y - 1.0, TEXT_X);
        }
        let alpha = if alarm.enabled { 1.0 } else { 0.45 };
        let (time, meridiem) = alarm_time(alarm.hour, alarm.minute);
        let time_width = text::draw(
            canvas,
            &time,
            TEXT_X,
            row.y + 100.0,
            Weight::DisplayLight,
            92.0,
            WHITE,
            alpha,
        );
        text::draw(
            canvas,
            meridiem,
            TEXT_X + time_width + 10.0,
            row.y + 100.0,
            Weight::Regular,
            44.0,
            WHITE,
            alpha,
        );
        let mut detail = if alarm.label.is_empty() {
            "Alarm".to_owned()
        } else {
            alarm.label.clone()
        };
        let repeat = repeat_text(&alarm.days);
        if !repeat.is_empty() {
            detail = format!("{detail}, {repeat}");
        }
        let detail = text::fit(
            &detail,
            Weight::Regular,
            36.0,
            RIGHT - SWITCH_W - TEXT_X - 40.0,
        );
        text::draw(
            canvas,
            &detail,
            TEXT_X,
            row.y + 146.0,
            Weight::Regular,
            36.0,
            SECONDARY,
            SECONDARY_ALPHA * alpha,
        );
        draw_switch(canvas, layout.switch(index), alarm.enabled);
    }
    if model.alarms.len() > layout.alarm_rows {
        footer(
            canvas,
            &format!("{} more alarms", model.alarms.len() - layout.alarm_rows),
            layout.alarms_card.y + layout.alarms_card.h - 20.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }

    section_header(canvas, "TIMERS", layout.timers_y);
    let now = unix_now();
    for (index, timer) in model.timers.iter().take(layout.timer_rows).enumerate() {
        let row = layout.timer_row(index);
        canvas.fill_rrect(row.inset(0.0), 30.0, CELL, 1.0);
        let paused = timer.state == "paused";
        text::draw(
            canvas,
            &countdown(timer.left(now)),
            TEXT_X,
            row.y + 92.0,
            Weight::DisplayLight,
            80.0,
            WHITE,
            if paused { 0.55 } else { 1.0 },
        );
        let label = if timer.label.is_empty() {
            format!("{} timer", countdown(timer.duration_s))
        } else {
            timer.label.clone()
        };
        text::draw(
            canvas,
            &text::fit(&label, Weight::Regular, 34.0, 420.0),
            TEXT_X,
            row.y + 132.0,
            Weight::Regular,
            34.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
        let (pause, cancel) = layout.timer_buttons(index);
        let (pcx, pcy) = (pause.x + pause.w / 2.0, pause.y + pause.h / 2.0);
        let tint = if paused { GREEN } else { ORANGE };
        canvas.fill_circle(
            pcx,
            pcy,
            pause.w / 2.0,
            tint,
            0.22 * pressed_alpha(pressed, Action::Clock(ClockAction::PauseOrResume(index))),
        );
        if paused {
            draw_play(canvas, (pcx, pcy), tint);
        } else {
            draw_pause(canvas, (pcx, pcy), tint);
        }
        let (ccx, ccy) = (cancel.x + cancel.w / 2.0, cancel.y + cancel.h / 2.0);
        canvas.fill_circle(ccx, ccy, cancel.w / 2.0, TRACK, 1.0);
        draw_cross(canvas, (ccx, ccy), 16.0, WHITE);
    }
    // Quick timers, then a custom one.
    let chips_row = card(layout.presets[0].y - 20.0, ROW);
    grouped_card(canvas, chips_row, 0, None);
    for (index, chip) in layout.presets.iter().enumerate() {
        let alpha = pressed_alpha(pressed, Action::Clock(ClockAction::StartPreset(index)));
        canvas.fill_rrect(*chip, chip.h / 2.0, ORANGE, 0.2 * alpha);
        text::draw_centered(
            canvas,
            &preset_label(TIMER_PRESETS[index]),
            chip.x + chip.w / 2.0,
            baseline(chip.y + chip.h / 2.0, Weight::SemiBold, 34.0),
            Weight::SemiBold,
            34.0,
            ORANGE,
            alpha,
        );
    }
    let custom = layout.custom_row;
    grouped_card(
        canvas,
        custom,
        1,
        (pressed == Some(Action::Clock(ClockAction::NewTimer))).then_some(0),
    );
    text::draw(
        canvas,
        "Custom Timer",
        TEXT_X,
        baseline(custom.y + ROW / 2.0, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    chevron(
        canvas,
        RIGHT,
        custom.y + ROW / 2.0,
        SECONDARY,
        TERTIARY_ALPHA,
    );
    if let Some(notice) = model.notice {
        footer(canvas, notice, custom.y + custom.h, RED, 1.0);
    }
}

fn draw_nav(
    canvas: &mut Canvas,
    title: &str,
    confirm: &str,
    enabled: bool,
    pressed: Option<Action>,
) {
    let line = baseline(NAV_HEIGHT / 2.0, Weight::Regular, LABEL_SIZE);
    text::draw(
        canvas,
        "Cancel",
        MARGIN,
        line,
        Weight::Regular,
        LABEL_SIZE,
        ORANGE,
        pressed_alpha(pressed, Action::Clock(ClockAction::Cancel)),
    );
    text::draw_centered(
        canvas,
        title,
        540.0,
        line,
        Weight::SemiBold,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    text::draw_right(
        canvas,
        confirm,
        1080.0 - MARGIN,
        line,
        Weight::SemiBold,
        LABEL_SIZE,
        ORANGE,
        if enabled {
            pressed_alpha(pressed, Action::Clock(ClockAction::Save))
        } else {
            0.3
        },
    );
}

fn draw_stepper(canvas: &mut Canvas, column: f32, field: EditorField, pressed: Option<Action>) {
    for (up, delta) in [(true, 1), (false, -1)] {
        let rect = step_button(column, up);
        let alpha = pressed_alpha(pressed, Action::Clock(ClockAction::Step(field, delta)));
        canvas.fill_rrect(rect, 30.0, CELL, 1.0);
        let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        let d = if up { -1.0 } else { 1.0 };
        let mut path = Path::new();
        path.stroke_polyline(
            &[
                (cx - 26.0, cy - d * 12.0),
                (cx, cy + d * 12.0),
                (cx + 26.0, cy - d * 12.0),
            ],
            7.0,
        );
        canvas.fill_path(&path, ORANGE, alpha);
    }
}

fn draw_label_row(canvas: &mut Canvas, rect: RectF, label: &str, placeholder: &str, focused: bool) {
    grouped_card(canvas, rect, 1, None);
    let line = baseline(rect.y + ROW / 2.0, Weight::Regular, LABEL_SIZE);
    text::draw(
        canvas,
        "Label",
        TEXT_X,
        line,
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    let (shown, alpha) = if label.is_empty() {
        (placeholder.to_owned(), SECONDARY_ALPHA)
    } else {
        (label.to_owned(), 1.0)
    };
    let shown = text::fit(&shown, Weight::Regular, LABEL_SIZE, 560.0);
    let width = text::measure(&shown, Weight::Regular, LABEL_SIZE);
    text::draw_right(
        canvas,
        &shown,
        RIGHT,
        line,
        Weight::Regular,
        LABEL_SIZE,
        SECONDARY,
        alpha,
    );
    if focused {
        let caret_x = if label.is_empty() {
            RIGHT - width - 8.0
        } else {
            RIGHT + 6.0
        };
        canvas.fill_rect(caret_x as i32, (rect.y + 34.0) as i32, 4, 60, ORANGE, 1.0);
    }
}

fn draw_alarm_editor(canvas: &mut Canvas, editor: &AlarmEditor, pressed: Option<Action>) {
    draw_nav(
        canvas,
        if editor.id.is_some() {
            "Edit Alarm"
        } else {
            "Add Alarm"
        },
        "Save",
        true,
        pressed,
    );
    let (time, meridiem) = alarm_time(editor.hour, editor.minute);
    let (hours, minutes) = time.split_once(':').unwrap_or((&time, "00"));
    text::draw_right(
        canvas,
        hours,
        500.0,
        TIME_Y,
        Weight::DisplayLight,
        200.0,
        WHITE,
        1.0,
    );
    text::draw_centered(
        canvas,
        ":",
        545.0,
        TIME_Y - 14.0,
        Weight::DisplayLight,
        200.0,
        WHITE,
        1.0,
    );
    text::draw(
        canvas,
        minutes,
        590.0,
        TIME_Y,
        Weight::DisplayLight,
        200.0,
        WHITE,
        1.0,
    );
    draw_stepper(canvas, HOUR_X, EditorField::Hour, pressed);
    draw_stepper(canvas, MINUTE_X, EditorField::Minute, pressed);
    canvas.fill_rrect(MERIDIEM, 30.0, CELL, 1.0);
    text::draw_centered(
        canvas,
        meridiem,
        MERIDIEM.x + MERIDIEM.w / 2.0,
        baseline(MERIDIEM.y + MERIDIEM.h / 2.0, Weight::SemiBold, 52.0),
        Weight::SemiBold,
        52.0,
        ORANGE,
        pressed_alpha(
            pressed,
            Action::Clock(ClockAction::Step(EditorField::Hour, 12)),
        ),
    );
    for (minute, rect) in quick_minutes() {
        let selected = editor.minute == minute;
        canvas.fill_rrect(
            rect,
            rect.h / 2.0,
            if selected { ORANGE } else { CELL },
            1.0,
        );
        text::draw_centered(
            canvas,
            &format!(":{minute:02}"),
            rect.x + rect.w / 2.0,
            baseline(rect.y + rect.h / 2.0, Weight::SemiBold, 40.0),
            Weight::SemiBold,
            40.0,
            if selected { 0x000000 } else { WHITE },
            pressed_alpha(pressed, Action::Clock(ClockAction::QuickMinute(minute))),
        );
    }
    section_header(canvas, "REPEAT", REPEAT_Y);
    grouped_card(canvas, card(REPEAT_Y, 144.0), 0, None);
    for (day, letter) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        let circle = day_circle(day as u8);
        let (cx, cy) = (circle.x + circle.w / 2.0, circle.y + circle.h / 2.0);
        let on = editor.days[day];
        if on {
            canvas.fill_circle(cx, cy, 46.0, ORANGE, 1.0);
        } else {
            canvas.stroke_circle(cx, cy, 46.0, 3.0, SECONDARY, 0.4);
        }
        text::draw_centered(
            canvas,
            letter,
            cx,
            baseline(cy, Weight::SemiBold, 40.0),
            Weight::SemiBold,
            40.0,
            if on { 0x000000 } else { WHITE },
            1.0,
        );
    }
    draw_label_row(
        canvas,
        label_row(),
        &editor.label,
        "Alarm",
        editor.label_focused,
    );
    if editor.id.is_some() {
        let row = delete_row();
        grouped_card(
            canvas,
            row,
            1,
            (pressed == Some(Action::Clock(ClockAction::DeleteAlarm))).then_some(0),
        );
        text::draw_centered(
            canvas,
            "Delete Alarm",
            540.0,
            baseline(row.y + ROW / 2.0, Weight::Regular, LABEL_SIZE),
            Weight::Regular,
            LABEL_SIZE,
            RED,
            1.0,
        );
    }
}

fn draw_timer_editor(canvas: &mut Canvas, editor: &TimerEditor, pressed: Option<Action>) {
    draw_nav(canvas, "Timer", "Start", editor.duration() > 0, pressed);
    for ((field, column), (value, unit)) in TIMER_COLUMNS.iter().zip([
        (editor.hours, "hours"),
        (editor.minutes, "min"),
        (editor.seconds, "sec"),
    ]) {
        draw_stepper(canvas, *column, *field, pressed);
        text::draw_centered(
            canvas,
            &format!("{value:02}"),
            *column,
            TIME_Y,
            Weight::DisplayLight,
            170.0,
            WHITE,
            1.0,
        );
        text::draw_centered(
            canvas,
            unit,
            *column,
            TIME_Y + 60.0,
            Weight::Regular,
            38.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
    draw_label_row(
        canvas,
        card(TIMER_LABEL_Y, ROW),
        &editor.label,
        "Timer",
        editor.label_focused,
    );
}

/// An iOS switch: green when on.
fn draw_switch(canvas: &mut Canvas, rect: RectF, on: bool) {
    canvas.fill_rrect(rect, rect.h / 2.0, if on { GREEN } else { TRACK }, 1.0);
    let knob = rect.h / 2.0 - 4.0;
    let cx = if on {
        rect.x + rect.w - rect.h / 2.0
    } else {
        rect.x + rect.h / 2.0
    };
    canvas.fill_circle(cx, rect.y + rect.h / 2.0, knob, WHITE, 1.0);
}

fn draw_plus(canvas: &mut Canvas, (cx, cy): (f32, f32), color: u32, alpha: f32) {
    let mut path = Path::new();
    path.stroke_segment((cx - 26.0, cy), (cx + 26.0, cy), 7.0);
    path.stroke_segment((cx, cy - 26.0), (cx, cy + 26.0), 7.0);
    canvas.fill_path(&path, color, alpha);
}

fn draw_cross(canvas: &mut Canvas, (cx, cy): (f32, f32), r: f32, color: u32) {
    let mut path = Path::new();
    path.stroke_segment((cx - r, cy - r), (cx + r, cy + r), 6.0);
    path.stroke_segment((cx - r, cy + r), (cx + r, cy - r), 6.0);
    canvas.fill_path(&path, color, 1.0);
}

fn draw_pause(canvas: &mut Canvas, (cx, cy): (f32, f32), color: u32) {
    for dx in [-11.0, 11.0] {
        canvas.fill_rrect(
            RectF::new(cx + dx - 6.0, cy - 20.0, 12.0, 40.0),
            3.0,
            color,
            1.0,
        );
    }
}

fn draw_play(canvas: &mut Canvas, (cx, cy): (f32, f32), color: u32) {
    let mut path = Path::new();
    path.polygon(&[
        (cx - 12.0, cy - 22.0),
        (cx + 20.0, cy),
        (cx - 12.0, cy + 22.0),
    ]);
    canvas.fill_path(&path, color, 1.0);
}

/// The System page's row leading here: an alarm clock tile.
pub fn draw_clock_tile_glyph(canvas: &mut Canvas, cx: f32, cy: f32) {
    canvas.stroke_circle(cx, cy + 2.0, 20.0, 4.5, WHITE, 1.0);
    let mut hands = Path::new();
    hands.stroke_segment((cx, cy + 2.0), (cx, cy - 9.0), 4.0);
    hands.stroke_segment((cx, cy + 2.0), (cx + 8.0, cy + 4.0), 4.0);
    canvas.fill_path(&hands, WHITE, 1.0);
    for side in [-1.0, 1.0] {
        canvas.fill_circle(cx + side * 15.0, cy - 16.0, 6.0, WHITE, 1.0);
    }
}

pub fn clock_summary(model: &ClockModel) -> String {
    let enabled = model.alarms.iter().filter(|a| a.enabled).count();
    match (enabled, model.timers.len()) {
        (_, timers) if timers > 0 => {
            format!("{timers} timer{}", if timers == 1 { "" } else { "s" })
        }
        (0, _) => String::new(),
        (n, _) => format!("{n} alarm{}", if n == 1 { "" } else { "s" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Alarm;

    #[test]
    fn clock_page_targets_match_the_drawing() {
        let model = ClockModel {
            alarms: vec![Alarm {
                id: "a1".into(),
                hour: 7,
                minute: 30,
                days: vec![1],
                label: "Up".into(),
                enabled: true,
                next_fire_at: None,
            }],
            connected: true,
            ..ClockModel::default()
        };
        let layout = ClockLayout::new(&model);
        let switch = layout.switch(0);
        assert_eq!(
            action_at(&model, switch.x + 10.0, switch.y + 10.0),
            Some(Action::Clock(ClockAction::ToggleAlarm(0)))
        );
        assert_eq!(
            action_at(&model, TEXT_X + 50.0, ALARMS_Y + 60.0),
            Some(Action::Clock(ClockAction::EditAlarm(0)))
        );
        let chip = layout.presets[2];
        assert_eq!(
            action_at(&model, chip.x + 5.0, chip.y + 5.0),
            Some(Action::Clock(ClockAction::StartPreset(2)))
        );
        // Everything stays above the home-gesture zone of a 2216-pixel app.
        assert!(layout.custom_row.y + layout.custom_row.h < 2216.0 - 170.0);
    }
}
