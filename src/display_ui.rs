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
use std::time::Instant;

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

/// Wallpaper thumbnails: three across, as many rows as it takes (the grid
/// scrolls under the title), at the grid's unscrolled position.
pub fn thumbnail(index: usize) -> RectF {
    let (w, h) = (300.0, 650.0);
    let gap = (CARD_WIDTH - 3.0 * w) / 2.0;
    let column = (index % 3) as f32;
    let row = (index / 3) as f32;
    RectF::new(
        MARGIN + column * (w + gap),
        GRID_TOP + row * (h + 110.0),
        w,
        h,
    )
}

const GRID_TOP: f32 = 320.0;
/// The grid shows between the title and the home indicator's strip.
const GRID_CLIP: f32 = 290.0;
const GESTURE_RESERVE: f32 = 170.0;
const FOOTER_LINE: f32 = 50.0;

/// The picker's items: Moon's wallpapers, the saved pictures, then the
/// tile that pastes one.
fn item_count(app: &AppModel) -> usize {
    WALLPAPERS.len() + app.saved_wallpapers.len() + 1
}

/// The wallpaper at `index` in the picker (None: the paste tile).
pub fn wallpaper_id(app: &AppModel, index: usize) -> Option<String> {
    match WALLPAPERS.get(index) {
        Some(spec) => Some(spec.id.to_owned()),
        None => app.saved_wallpapers.get(index - WALLPAPERS.len()).cloned(),
    }
}

/// Where the footer starts, unscrolled.
fn footer_top(app: &AppModel) -> f32 {
    let last = thumbnail(item_count(app) - 1);
    last.y + last.h + 70.0
}

/// The picker's scroll range on a screen `height` tall.
pub fn wallpaper_scroll_max(app: &AppModel, height: f32) -> f32 {
    (footer_top(app) + FOOTER_LINE * 4.0 + 60.0 - (height - GESTURE_RESERVE)).max(0.0)
}

/// A touch at `y` on the picker may scroll it.
pub fn wallpaper_scrolls(height: f32, y: f32) -> bool {
    y >= GRID_CLIP && y < height - GESTURE_RESERVE
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
        View::Wallpaper => {
            if y < GRID_CLIP {
                return None;
            }
            let y = y + app.wallpaper_scroll.value(Instant::now());
            let paste = item_count(app) - 1;
            (0..item_count(app))
                .find(|i| thumbnail(*i).inset(-10.0).contains(x, y))
                .map(|i| {
                    if i == paste {
                        Action::PasteWallpaper
                    } else {
                        Action::SetWallpaper(i)
                    }
                })
        }
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
    let height = canvas.height as f32;
    let offset = app.wallpaper_scroll.value(Instant::now());
    canvas.set_clip(
        0,
        GRID_CLIP as i32,
        canvas.width,
        (height - GESTURE_RESERVE) as i32,
    );
    let current = app.shell.wallpaper.as_deref().unwrap_or("moon");
    let paste = item_count(app) - 1;
    for index in 0..item_count(app) {
        let mut rect = thumbnail(index);
        rect.y -= offset;
        if rect.y + rect.h + 80.0 < GRID_CLIP || rect.y > height {
            continue;
        }
        let alpha = if pressed.is_some_and(|p| {
            p == Action::SetWallpaper(index) || (index == paste && p == Action::PasteWallpaper)
        }) {
            0.5
        } else {
            1.0
        };
        let label_y = rect.y + rect.h + 62.0;
        if index == paste {
            draw_paste_tile(canvas, rect, app.wallpaper_adding, alpha);
            continue;
        }
        let Some(id) = wallpaper_id(app, index) else {
            continue;
        };
        // Rounded like the phone's screen corners.
        canvas.fill_rrect(rect, 34.0, 0x000000, 1.0);
        let image = match WALLPAPERS.get(index) {
            Some(_) => Some(&thumbnails()[index]),
            None => saved_thumbnail(&id),
        };
        if let Some(image) = image {
            blit_rounded(canvas, image, rect, 34.0);
        }
        if id == "black" || image.is_none() {
            canvas.stroke_rrect(rect, 34.0, 2.0, 0x38383a, 1.0);
        }
        if crate::wallpapers::is_dynamic(&id) {
            draw_tilt_badge(canvas, rect.x + rect.w - 40.0, rect.y + 40.0);
        }
        if id == current {
            canvas.stroke_rrect(rect.inset(-10.0), 42.0, 6.0, BLUE, 1.0);
        }
        let name = text::fit(
            &crate::wallpapers::name(&id),
            Weight::Regular,
            40.0,
            rect.w + 40.0,
        );
        text::draw_centered(
            canvas,
            &name,
            rect.x + rect.w / 2.0,
            label_y,
            if id == current {
                Weight::SemiBold
            } else {
                Weight::Regular
            },
            40.0,
            WHITE,
            alpha,
        );
    }
    let top = footer_top(app) - offset;
    let mut lines: Vec<(&str, u32)> = vec![
        (
            "Orbit, Ridge and your pictures move as you tilt.",
            SECONDARY,
        ),
        ("Paste adds the picture you copied last.", SECONDARY),
        (
            "Black lights no pixels: it saves the most battery.",
            SECONDARY,
        ),
    ];
    if let Some(status) = &app.wallpaper_status {
        lines.insert(0, (status.as_str(), WHITE));
    }
    for (row, (line, color)) in lines.into_iter().enumerate() {
        footer(
            canvas,
            line,
            top + row as f32 * FOOTER_LINE - 60.0,
            color,
            SECONDARY_ALPHA,
        );
    }
    canvas.reset_clip();
}

/// The tile that adds the clipboard's picture: a plus, or a pulse of
/// dots while it is added.
fn draw_paste_tile(canvas: &mut Canvas, rect: RectF, adding: bool, alpha: f32) {
    canvas.fill_rrect(rect, 34.0, 0x1c1c1e, alpha);
    canvas.stroke_rrect(rect, 34.0, 2.0, 0x38383a, alpha);
    let (cx, cy) = rect.center();
    if adding {
        let phase = (crate::clock::unix_now() % 3) as usize;
        for dot in 0..3 {
            let lit = if dot == phase { 1.0 } else { 0.35 };
            canvas.fill_circle(cx - 36.0 + dot as f32 * 36.0, cy, 9.0, WHITE, lit * alpha);
        }
    } else {
        canvas.fill_rrect(
            RectF::new(cx - 34.0, cy - 4.0, 68.0, 8.0),
            4.0,
            WHITE,
            alpha,
        );
        canvas.fill_rrect(
            RectF::new(cx - 4.0, cy - 34.0, 8.0, 68.0),
            4.0,
            WHITE,
            alpha,
        );
    }
    text::draw_centered(
        canvas,
        if adding { "Adding…" } else { "Paste" },
        cx,
        rect.y + rect.h + 62.0,
        Weight::Regular,
        40.0,
        WHITE,
        alpha,
    );
}

/// Marks a wallpaper that moves with the tilt: two offset frames, a
/// picture and its parallax.
fn draw_tilt_badge(canvas: &mut Canvas, cx: f32, cy: f32) {
    canvas.fill_circle(cx, cy, 24.0, 0x000000, 0.45);
    canvas.stroke_rrect(
        RectF::new(cx - 13.0, cy - 7.0, 18.0, 18.0),
        4.0,
        2.5,
        WHITE,
        0.55,
    );
    canvas.stroke_rrect(
        RectF::new(cx - 5.0, cy - 13.0, 18.0, 18.0),
        4.0,
        2.5,
        WHITE,
        0.95,
    );
}

/// A saved picture's thumbnail, decoded once per file version.
fn saved_thumbnail(id: &str) -> Option<&'static Canvas> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Cache = HashMap<String, (Option<std::time::SystemTime>, &'static Canvas)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let file = id.strip_prefix(crate::wallpapers::SAVED_PREFIX)?;
    let modified = std::fs::metadata(std::path::Path::new(crate::wallpapers::SAVED_DIR).join(file))
        .and_then(|m| m.modified())
        .ok();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((at, image)) = cache.get(id)
        && *at == modified
    {
        return Some(image);
    }
    let rect = thumbnail(0);
    let image = crate::wallpapers::saved_thumbnail(id, rect.w as i32, rect.h as i32)?;
    // Kept for the app's life, like the built-in thumbnails (a new version
    // of a file leaks its old one: rare, and 780 KB).
    let image: &'static Canvas = Box::leak(Box::new(image));
    cache.insert(id.to_owned(), (modified, image));
    Some(image)
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

/// Copies `image` into `rect` with rounded corners, within the canvas's
/// clip.
fn blit_rounded(canvas: &mut Canvas, image: &Canvas, rect: RectF, radius: f32) {
    let (x0, y0) = (rect.x as i32, rect.y as i32);
    let (clip_x0, clip_y0, clip_x1, clip_y1) = canvas.clip();
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
        if ty < clip_y0 || ty >= clip_y1 {
            continue;
        }
        for x in inset..(image.width - inset) {
            let tx = x0 + x;
            if tx >= clip_x0 && tx < clip_x1 {
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

pub(crate) fn draw_switch(canvas: &mut Canvas, rect: RectF, on: bool) {
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
            haptics: Some(true),
            time: None,
            open_target: None,
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
        // After Moon's wallpapers (and no saved ones), the paste tile.
        let (x, y) = thumbnail(WALLPAPERS.len()).center();
        assert_eq!(action_at(&picker, x, y), Some(Action::PasteWallpaper));
        assert_eq!(wallpaper_id(&picker, 6).as_deref(), Some("orbit"));
        assert_eq!(wallpaper_id(&picker, WALLPAPERS.len()), None);
        // Three rows do not fit: the grid scrolls, and a tap lands where
        // the scrolled grid shows.
        assert!(wallpaper_scroll_max(&picker, 2216.0) > 0.0);
        assert_eq!(action_at(&picker, x, 200.0), None);
        assert_eq!(auto_lock_label(120), "2 Minutes");
    }
}
