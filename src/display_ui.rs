//! Display & Brightness, Auto-Lock and Wallpaper pages. Brightness goes to
//! Moon's display backend (OSS-Phone-V1 docs/display-brightness.md); the
//! wallpaper and Auto-Lock to Moon's settings (docs/settings.md), all over
//! Moon's control socket.

use crate::app::{Action, AppModel, View};
use crate::gfx::{Canvas, Path, RectF};
use crate::text::{self, Weight};
use crate::ui::{
    BACK_BUTTON, BLUE, CARD_WIDTH, LABEL_SIZE, MARGIN, ROW, SECONDARY, SECONDARY_ALPHA,
    TERTIARY_ALPHA, TEXT_X, TRACK, WHITE, back_chevron, baseline, card, chevron, footer,
    grouped_card, large_title, section_header, separator,
};
use crate::wallpapers::{WALLPAPERS, render};

const RIGHT: f32 = MARGIN + CARD_WIDTH - 44.0;
const BRIGHTNESS_Y: f32 = 340.0;
const SLIDER_ROW: f32 = 150.0;
/// The brightness slider's track: a tap anywhere along it sets the level
/// (Moon hands apps taps, not drags; Control Centre has the drag slider).
pub const SLIDER: RectF = RectF::new(MARGIN + 120.0, BRIGHTNESS_Y, CARD_WIDTH - 240.0, SLIDER_ROW);
const GREEN: u32 = 0x30d158;

/// Auto-Lock choices: seconds (0 = never) and their labels.
pub const AUTO_LOCK: [(u32, &str); 6] = [
    (15, "15 Seconds"),
    (30, "30 Seconds"),
    (60, "1 Minute"),
    (120, "2 Minutes"),
    (300, "5 Minutes"),
    (0, "Never"),
];

pub fn auto_lock_label(seconds: u32) -> &'static str {
    AUTO_LOCK
        .iter()
        .find(|(s, _)| *s == seconds)
        .map_or("30 Seconds", |(_, label)| label)
}

/// The sensor exists (auto-brightness can be offered).
fn sensor_available(app: &AppModel) -> bool {
    app.shell
        .display
        .as_ref()
        .is_some_and(|display| display.sensor != "unavailable")
}

fn auto_row() -> RectF {
    card(BRIGHTNESS_Y + SLIDER_ROW, ROW)
}

fn screen_y(app: &AppModel) -> f32 {
    let brightness_rows = if sensor_available(app) {
        SLIDER_ROW + ROW
    } else {
        SLIDER_ROW
    };
    // Room for the "not available" footer when Moon has no backend.
    let footer = if app.shell.display.is_none() {
        60.0
    } else {
        0.0
    };
    BRIGHTNESS_Y + brightness_rows + 120.0 + footer
}

fn auto_lock_row(app: &AppModel) -> RectF {
    card(screen_y(app), ROW)
}

fn choice_row(index: usize) -> RectF {
    card(340.0 + ROW * index as f32, ROW)
}

/// Wallpaper thumbnails: three across, two down.
pub fn thumbnail(index: usize) -> RectF {
    let (w, h) = (300.0, 650.0);
    let gap = (CARD_WIDTH - 3.0 * w) / 2.0;
    let column = (index % 3) as f32;
    let row = (index / 3) as f32;
    RectF::new(MARGIN + column * (w + gap), 320.0 + row * (h + 110.0), w, h)
}

/// The brightness for a touch at `x` along the slider.
pub fn slider_value(x: f32) -> u8 {
    (((x - SLIDER.x) / SLIDER.w).clamp(0.0, 1.0) * 100.0).round() as u8
}

pub fn action_at(app: &AppModel, x: f32, y: f32) -> Option<Action> {
    if BACK_BUTTON.contains(x, y) {
        return Some(if app.view == View::AutoLock {
            Action::ShowDisplay
        } else {
            Action::ShowSystem
        });
    }
    match app.view {
        View::Display => {
            if app.shell.display.is_some() && SLIDER.inset(-20.0).contains(x, y) {
                return Some(Action::SetBrightness(slider_value(x)));
            }
            if sensor_available(app) && auto_row().contains(x, y) {
                return Some(Action::ToggleAutoBrightness);
            }
            auto_lock_row(app)
                .contains(x, y)
                .then_some(Action::ShowAutoLock)
        }
        View::AutoLock => (0..AUTO_LOCK.len())
            .find(|i| choice_row(*i).contains(x, y))
            .map(|i| Action::SetAutoLock(AUTO_LOCK[i].0)),
        View::Wallpaper => (0..WALLPAPERS.len())
            .find(|i| thumbnail(*i).inset(-10.0).contains(x, y))
            .map(Action::SetWallpaper),
        _ => None,
    }
}

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

pub fn draw(canvas: &mut Canvas, app: &AppModel, pressed: Option<Action>) {
    match app.view {
        View::Display => draw_display(canvas, app, pressed),
        View::AutoLock => draw_auto_lock(canvas, app, pressed),
        View::Wallpaper => draw_wallpaper(canvas, app, pressed),
        _ => {}
    }
}

fn draw_display(canvas: &mut Canvas, app: &AppModel, pressed: Option<Action>) {
    back(canvas, "System", pressed == Some(Action::ShowSystem));
    large_title(canvas, "Display", 236.0);
    section_header(canvas, "BRIGHTNESS", BRIGHTNESS_Y);
    let sensor = sensor_available(app);
    let rows = if sensor { 2 } else { 1 };
    grouped_card(
        canvas,
        card(BRIGHTNESS_Y, SLIDER_ROW + if sensor { ROW } else { 0.0 }),
        0,
        None,
    );
    let cy = BRIGHTNESS_Y + SLIDER_ROW / 2.0;
    draw_sun(canvas, TEXT_X + 10.0, cy, 11.0, SECONDARY);
    draw_sun(canvas, RIGHT - 16.0, cy, 17.0, SECONDARY);
    let track = RectF::new(SLIDER.x, cy - 6.0, SLIDER.w, 12.0);
    match &app.shell.display {
        Some(display) => {
            canvas.fill_rrect(track, 6.0, TRACK, 1.0);
            let fraction = f32::from(display.brightness) / 100.0;
            canvas.fill_rrect(
                RectF::new(track.x, track.y, (track.w * fraction).max(12.0), track.h),
                6.0,
                WHITE,
                1.0,
            );
            let knob_x = track.x + track.w * fraction;
            canvas.shadow_rrect(
                RectF::new(knob_x - 34.0, cy - 34.0, 68.0, 68.0),
                34.0,
                10.0,
                0x000000,
                0.35,
            );
            canvas.fill_circle(knob_x, cy, 34.0, WHITE, 1.0);
        }
        None => {
            canvas.fill_rrect(track, 6.0, TRACK, 0.5);
        }
    }
    if sensor {
        let row = auto_row();
        separator(canvas, row.y - 1.0, TEXT_X);
        let line = baseline(row.y + ROW / 2.0, Weight::Regular, LABEL_SIZE);
        text::draw(
            canvas,
            "Auto-Brightness",
            TEXT_X,
            line,
            Weight::Regular,
            LABEL_SIZE,
            WHITE,
            1.0,
        );
        let on = app.shell.display.as_ref().is_some_and(|d| d.auto);
        draw_switch(
            canvas,
            RectF::new(RIGHT - 104.0, row.y + (ROW - 64.0) / 2.0, 104.0, 64.0),
            on,
        );
    }
    let _ = rows;
    if app.shell.display.is_none() {
        footer(
            canvas,
            "Brightness control is not available yet.",
            BRIGHTNESS_Y + SLIDER_ROW,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }

    let screen = screen_y(app);
    section_header(canvas, "SCREEN", screen);
    let row = auto_lock_row(app);
    grouped_card(
        canvas,
        row,
        1,
        (pressed == Some(Action::ShowAutoLock)).then_some(0),
    );
    let line = baseline(row.y + ROW / 2.0, Weight::Regular, LABEL_SIZE);
    text::draw(
        canvas,
        "Auto-Lock",
        TEXT_X,
        line,
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    chevron(canvas, RIGHT, row.y + ROW / 2.0, SECONDARY, TERTIARY_ALPHA);
    text::draw_right(
        canvas,
        auto_lock_label(app.shell.auto_lock_s.unwrap_or(30)),
        RIGHT - 40.0,
        line,
        Weight::Regular,
        LABEL_SIZE,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    footer(
        canvas,
        "Dims, then turns off, after this long untouched.",
        row.y + row.h,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}

fn draw_auto_lock(canvas: &mut Canvas, app: &AppModel, pressed: Option<Action>) {
    back(canvas, "Display", pressed == Some(Action::ShowDisplay));
    large_title(canvas, "Auto-Lock", 236.0);
    let current = app.shell.auto_lock_s.unwrap_or(30);
    let pressed_row =
        (0..AUTO_LOCK.len()).find(|i| pressed == Some(Action::SetAutoLock(AUTO_LOCK[*i].0)));
    grouped_card(
        canvas,
        card(340.0, ROW * AUTO_LOCK.len() as f32),
        AUTO_LOCK.len(),
        pressed_row,
    );
    for (index, (seconds, label)) in AUTO_LOCK.iter().enumerate() {
        let row = choice_row(index);
        if index > 0 {
            separator(canvas, row.y - 1.0, TEXT_X);
        }
        let cy = row.y + ROW / 2.0;
        text::draw(
            canvas,
            label,
            TEXT_X,
            baseline(cy, Weight::Regular, LABEL_SIZE),
            Weight::Regular,
            LABEL_SIZE,
            WHITE,
            1.0,
        );
        if *seconds == current {
            let mut path = Path::new();
            path.stroke_polyline(
                &[
                    (RIGHT - 30.0, cy),
                    (RIGHT - 20.0, cy + 11.0),
                    (RIGHT, cy - 13.0),
                ],
                6.0,
            );
            canvas.fill_path(&path, BLUE, 1.0);
        }
    }
}

fn draw_wallpaper(canvas: &mut Canvas, app: &AppModel, pressed: Option<Action>) {
    back(canvas, "System", pressed == Some(Action::ShowSystem));
    large_title(canvas, "Wallpaper", 236.0);
    let current = app.shell.wallpaper.as_deref().unwrap_or("moon");
    for (index, spec) in WALLPAPERS.iter().enumerate() {
        let rect = thumbnail(index);
        let image = &thumbnails()[index];
        // Rounded like the phone's screen corners.
        let mask = RectF::new(rect.x, rect.y, rect.w, rect.h);
        canvas.fill_rrect(mask, 34.0, 0x000000, 1.0);
        blit_rounded(canvas, image, mask, 34.0);
        if spec.id == "black" {
            canvas.stroke_rrect(mask, 34.0, 2.0, 0x38383a, 1.0);
        }
        if spec.id == current {
            canvas.stroke_rrect(mask.inset(-10.0), 42.0, 6.0, BLUE, 1.0);
        }
        let alpha = if pressed == Some(Action::SetWallpaper(index)) {
            0.5
        } else {
            1.0
        };
        text::draw_centered(
            canvas,
            spec.name,
            rect.x + rect.w / 2.0,
            rect.y + rect.h + 62.0,
            if spec.id == current {
                Weight::SemiBold
            } else {
                Weight::Regular
            },
            40.0,
            WHITE,
            alpha,
        );
    }
    footer(
        canvas,
        "Black lights no pixels: it saves the most battery.",
        thumbnail(3).y + thumbnail(3).h + 70.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}

/// The thumbnails, drawn once (each is a few hundred thousand pixels of
/// gradient math, and the page repaints every second).
fn thumbnails() -> &'static [Canvas] {
    static CACHE: std::sync::OnceLock<Vec<Canvas>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        WALLPAPERS
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let rect = thumbnail(index);
                render(spec.id, rect.w as i32, rect.h as i32)
            })
            .collect()
    })
}

/// Copies `image` into `rect` with rounded corners.
fn blit_rounded(canvas: &mut Canvas, image: &Canvas, rect: RectF, radius: f32) {
    let (x0, y0) = (rect.x as i32, rect.y as i32);
    for y in 0..image.height {
        let py = y as f32 + 0.5;
        let dy = if py < radius {
            radius - py
        } else if py > rect.h - radius {
            py - (rect.h - radius)
        } else {
            0.0
        };
        let inset = (radius - (radius * radius - dy * dy).max(0.0).sqrt()).ceil() as i32;
        let ty = y0 + y;
        if ty < 0 || ty >= canvas.height {
            continue;
        }
        for x in inset..(image.width - inset) {
            let tx = x0 + x;
            if tx >= 0 && tx < canvas.width {
                canvas.pixels[(ty * canvas.width + tx) as usize] =
                    image.pixels[(y * image.width + x) as usize];
            }
        }
    }
}

/// A sun: a disc and eight rays.
pub fn draw_sun(canvas: &mut Canvas, cx: f32, cy: f32, r: f32, color: u32) {
    canvas.fill_circle(cx, cy, r * 0.55, color, 1.0);
    let mut rays = Path::new();
    for i in 0..8 {
        let angle = i as f32 * std::f32::consts::TAU / 8.0;
        let (s, c) = angle.sin_cos();
        rays.stroke_segment(
            (cx + c * r * 0.85, cy + s * r * 0.85),
            (cx + c * r * 1.25, cy + s * r * 1.25),
            r * 0.2,
        );
    }
    canvas.fill_path(&rays, color, 1.0);
}

/// The Wallpaper row's glyph: a sun over hills.
pub fn draw_wallpaper_glyph(canvas: &mut Canvas, cx: f32, cy: f32) {
    canvas.fill_circle(cx + 10.0, cy - 10.0, 7.0, WHITE, 1.0);
    let mut hills = Path::new();
    hills.polygon(&[
        (cx - 24.0, cy + 18.0),
        (cx - 8.0, cy - 2.0),
        (cx + 4.0, cy + 10.0),
        (cx + 12.0, cy + 2.0),
        (cx + 24.0, cy + 18.0),
    ]);
    canvas.fill_path(&hills, WHITE, 1.0);
}

fn draw_switch(canvas: &mut Canvas, rect: RectF, on: bool) {
    canvas.fill_rrect(rect, rect.h / 2.0, if on { GREEN } else { TRACK }, 1.0);
    let cx = if on {
        rect.x + rect.w - rect.h / 2.0
    } else {
        rect.x + rect.h / 2.0
    };
    canvas.fill_circle(cx, rect.y + rect.h / 2.0, rect.h / 2.0 - 4.0, WHITE, 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{DisplayState, ShellInfo};

    fn app(view: View, sensor: &str) -> AppModel {
        let mut app = AppModel::default();
        app.view = view;
        app.shell = ShellInfo {
            volume: Some(50),
            display: Some(DisplayState {
                brightness: 40,
                auto: false,
                sensor: sensor.into(),
            }),
            wallpaper: Some("moon".into()),
            auto_lock_s: Some(30),
        };
        app
    }

    #[test]
    fn display_targets_follow_the_sensor() {
        let with = app(View::Display, "ok");
        let (x, y) = auto_row().center();
        assert_eq!(action_at(&with, x, y), Some(Action::ToggleAutoBrightness));
        assert_eq!(
            action_at(&with, SLIDER.x + SLIDER.w / 2.0, SLIDER.y + 70.0),
            Some(Action::SetBrightness(50))
        );
        let without = app(View::Display, "unavailable");
        assert_eq!(action_at(&without, x, y), None);
        let (x, y) = auto_lock_row(&without).center();
        assert_eq!(action_at(&without, x, y), Some(Action::ShowAutoLock));
    }

    #[test]
    fn auto_lock_and_wallpaper_choices() {
        let choices = app(View::AutoLock, "ok");
        let (x, y) = choice_row(5).center();
        assert_eq!(action_at(&choices, x, y), Some(Action::SetAutoLock(0)));
        let picker = app(View::Wallpaper, "ok");
        let (x, y) = thumbnail(4).center();
        assert_eq!(action_at(&picker, x, y), Some(Action::SetWallpaper(4)));
        assert!(thumbnail(5).y + thumbnail(5).h + 120.0 < 2216.0 - 170.0);
        assert_eq!(auto_lock_label(120), "2 Minutes");
    }
}
