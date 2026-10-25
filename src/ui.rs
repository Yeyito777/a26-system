//! The System app's interface, styled after the iOS Settings app in dark
//! appearance so it reads as part of the Moon shell: a large title, inset
//! grouped lists, and a pushed Wi-Fi page with a back button.
//!
//! Geometry is in window pixels (1080 wide). One iPhone point is ~2.77 px.

use std::error::Error;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::Window;

use crate::app::{Action, AppModel, Field, KeyAction, KeyboardMode, PasswordDialog, View};
use crate::gfx::{Canvas, Path, RectF, Rgb};
use crate::metrics::{SystemInfo, usage_text};
use crate::present::Presenter;
use crate::text::{self, Weight};
use crate::wifi::{LinkState, WifiNetwork};

pub const BG: Rgb = 0x000000;
const CELL: Rgb = 0x1c1c1e;
const CELL_PRESSED: Rgb = 0x3a3a3c;
const SEPARATOR: Rgb = 0x38383a;
const WHITE: Rgb = 0xffffff;
const SECONDARY: Rgb = 0xebebf5;
const SECONDARY_ALPHA: f32 = 0.6;
const TERTIARY_ALPHA: f32 = 0.3;
const BLUE: Rgb = 0x0a84ff;
const GREEN: Rgb = 0x30d158;
const RED: Rgb = 0xff453a;
const PINK: Rgb = 0xff375f;
const TRACK: Rgb = 0x3a3a3c;

const MARGIN: f32 = 44.0;
const CARD_WIDTH: f32 = 1080.0 - 2.0 * MARGIN;
const CARD_RADIUS: f32 = 30.0;
const ROW: f32 = 128.0;
const INFO_ROW: f32 = 118.0;
const TEXT_X: f32 = MARGIN + 44.0;
const LABEL_SIZE: f32 = 47.0;
const HEADER_SIZE: f32 = 36.0;
const LARGE_TITLE_SIZE: f32 = 92.0;
/// Moon owns the bottom of the screen for its swipe-up gesture.
const BOTTOM_GESTURE_RESERVE: f32 = 170.0;

// System page.
const SYSTEM_CARD_Y: f32 = 190.0;
const ACTIVITY_Y: f32 = 584.0;
const ACTIVITY_HEIGHT: f32 = 236.0;
const STORAGE_Y: f32 = 920.0;
const STORAGE_HEIGHT: f32 = 236.0;
const ABOUT_Y: f32 = 1256.0;

// Wi-Fi page.
const BACK_BUTTON: RectF = RectF::new(0.0, 0.0, 420.0, 140.0);
const STATUS_CARD_Y: f32 = 290.0;
const NETWORK_ROW: f32 = 118.0;

// Password page.
const NAV_HEIGHT: f32 = 150.0;
const CANCEL_BUTTON: RectF = RectF::new(0.0, 0.0, 330.0, NAV_HEIGHT);
const JOIN_BUTTON: RectF = RectF::new(750.0, 0.0, 330.0, NAV_HEIGHT);
const PASSWORD_CARD_Y: f32 = 300.0;
/// Gap between the last field and the fallback in-app keyboard.
const FALLBACK_KEYBOARD_GAP: f32 = 212.0;

pub struct Renderer {
    window: Window,
    depth: u8,
    canvas: Canvas,
    presenter: Presenter,
}

impl Renderer {
    pub fn new<C: Connection>(
        conn: &C,
        window: Window,
        depth: u8,
        width: u16,
        height: u16,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            window,
            depth,
            canvas: Canvas::new(i32::from(width), i32::from(height)),
            presenter: Presenter::create(conn, window, depth, width, height)?,
        })
    }

    pub fn resize<C: Connection>(
        &mut self,
        conn: &C,
        width: u16,
        height: u16,
    ) -> Result<(), Box<dyn Error>> {
        let presenter = Presenter::create(conn, self.window, self.depth, width, height)?;
        self.presenter.destroy(conn);
        self.presenter = presenter;
        self.canvas = Canvas::new(i32::from(width), i32::from(height));
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        (self.canvas.width as u16, self.canvas.height as u16)
    }

    pub fn height(&self) -> u16 {
        self.canvas.height as u16
    }

    pub fn render<C: Connection>(
        &mut self,
        conn: &C,
        system: &SystemInfo,
        volume: Option<u8>,
        app: &AppModel,
        pressed: Option<Action>,
    ) -> Result<(), Box<dyn Error>> {
        let canvas = &mut self.canvas;
        canvas.clear(BG);
        if let Some(dialog) = &app.password_dialog {
            draw_password(canvas, dialog, pressed);
        } else {
            match app.view {
                View::System => draw_system(canvas, system, volume, app, pressed),
                View::Wifi => draw_wifi(canvas, app, pressed),
            }
        }
        self.presenter.present(conn, &self.canvas, 0)?;
        conn.flush()?;
        Ok(())
    }

    /// Repairs exposed window areas from the last frame.
    pub fn expose<C: Connection>(&self, conn: &C) -> Result<(), Box<dyn Error>> {
        self.presenter.expose(conn, 0)?;
        conn.flush()?;
        Ok(())
    }

    pub fn destroy<C: Connection>(&self, conn: &C) {
        self.presenter.destroy(conn);
    }

    pub fn action_at(&self, app: &AppModel, x: i16, y: i16) -> Option<Action> {
        action_at(app, self.canvas.height as f32, x, y)
    }
}

/// Maps a tap in window coordinates to an app action.
pub fn action_at(app: &AppModel, height: f32, x: i16, y: i16) -> Option<Action> {
    let (x, y) = (f32::from(x), f32::from(y));
    if y >= height - BOTTOM_GESTURE_RESERVE {
        return None;
    }
    if let Some(dialog) = &app.password_dialog {
        if CANCEL_BUTTON.contains(x, y) {
            return Some(Action::CancelPassword);
        }
        if JOIN_BUTTON.contains(x, y) {
            return Some(Action::SubmitPassword);
        }
        if let Some(field) = dialog
            .fields()
            .iter()
            .enumerate()
            .find(|(index, _)| field_rect(*index).contains(x, y))
            .map(|(_, field)| *field)
        {
            return Some(Action::Focus(field));
        }
        if dialog.system_keyboard {
            return None;
        }
        return fallback_keys(dialog.keyboard_mode, fallback_keyboard_y(dialog))
            .into_iter()
            .find(|key| key.bounds.contains(x, y))
            .map(|key| Action::Key(key.action));
    }
    match app.view {
        View::System => wifi_row().contains(x, y).then_some(Action::ShowWifi),
        View::Wifi => {
            if BACK_BUTTON.contains(x, y) {
                return Some(Action::ShowSystem);
            }
            let layout = WifiLayout::new(app);
            if layout.scan_button.contains(x, y) && !app.scanning && !app.busy {
                return Some(Action::RefreshWifi);
            }
            if let Some(disconnect) = layout.disconnect_row
                && disconnect.contains(x, y)
                && !app.busy
            {
                return Some(Action::Disconnect);
            }
            for (row, index) in app.page_range().enumerate() {
                if layout.network_row(row).contains(x, y) {
                    return Some(Action::SelectNetwork(index));
                }
            }
            if layout.previous.contains(x, y) && app.page > 0 {
                return Some(Action::PreviousNetworks);
            }
            if layout.next.contains(x, y) && app.page + 1 < app.page_count() {
                return Some(Action::NextNetworks);
            }
            None
        }
    }
}

fn card(y: f32, height: f32) -> RectF {
    RectF::new(MARGIN, y, CARD_WIDTH, height)
}

fn wifi_row() -> RectF {
    card(SYSTEM_CARD_Y, ROW)
}

fn field_rect(index: usize) -> RectF {
    card(PASSWORD_CARD_Y + index as f32 * ROW, ROW)
}

fn fields_bottom(dialog: &PasswordDialog) -> f32 {
    PASSWORD_CARD_Y + dialog.fields().len() as f32 * ROW
}

fn fallback_keyboard_y(dialog: &PasswordDialog) -> f32 {
    fields_bottom(dialog) + FALLBACK_KEYBOARD_GAP
}

fn baseline(center_y: f32, weight: Weight, size: f32) -> f32 {
    text::centered_baseline(center_y, weight, size)
}

fn large_title(canvas: &mut Canvas, title: &str, baseline_y: f32) {
    text::draw(
        canvas,
        title,
        MARGIN,
        baseline_y,
        Weight::Bold,
        LARGE_TITLE_SIZE,
        WHITE,
        1.0,
    );
}

fn section_header(canvas: &mut Canvas, title: &str, card_y: f32) {
    text::draw_tracked(
        canvas,
        title,
        TEXT_X,
        card_y - 26.0,
        Weight::Regular,
        HEADER_SIZE,
        1.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
}

fn footer(canvas: &mut Canvas, message: &str, card_bottom: f32, color: Rgb, alpha: f32) {
    let fitted = text::fit(message, Weight::Regular, 38.0, CARD_WIDTH - 88.0);
    text::draw(
        canvas,
        &fitted,
        TEXT_X,
        card_bottom + 60.0,
        Weight::Regular,
        38.0,
        color,
        alpha,
    );
}

/// Draws an inset grouped card with optional pressed row highlighting.
fn grouped_card(canvas: &mut Canvas, rect: RectF, rows: usize, pressed_row: Option<usize>) {
    canvas.fill_rrect(rect, CARD_RADIUS, CELL, 1.0);
    if rows == 0 {
        return;
    }
    let row_height = rect.h / rows as f32;
    if let Some(row) = pressed_row.filter(|row| *row < rows) {
        let top = rect.y + row as f32 * row_height;
        canvas.set_clip(
            0,
            top.round() as i32,
            canvas.width,
            (top + row_height).round() as i32,
        );
        canvas.fill_rrect(rect, CARD_RADIUS, CELL_PRESSED, 1.0);
        canvas.reset_clip();
    }
}

fn separator(canvas: &mut Canvas, y: f32, inset: f32) {
    canvas.fill_rect(
        inset as i32,
        y.round() as i32,
        (MARGIN + CARD_WIDTH - inset) as i32,
        2,
        SEPARATOR,
        1.0,
    );
}

fn chevron(canvas: &mut Canvas, x: f32, cy: f32, color: Rgb, alpha: f32) {
    let mut path = Path::new();
    path.stroke_polyline(
        &[(x - 9.0, cy - 17.0), (x + 7.0, cy), (x - 9.0, cy + 17.0)],
        6.0,
    );
    canvas.fill_path(&path, color, alpha);
}

fn back_chevron(canvas: &mut Canvas, x: f32, cy: f32) {
    let mut path = Path::new();
    path.stroke_polyline(
        &[(x + 12.0, cy - 25.0), (x - 12.0, cy), (x + 12.0, cy + 25.0)],
        8.0,
    );
    canvas.fill_path(&path, BLUE, 1.0);
}

fn icon_tile(canvas: &mut Canvas, x: f32, cy: f32, color: Rgb) -> RectF {
    let tile = RectF::new(x, cy - 40.0, 80.0, 80.0);
    canvas.fill_rrect(tile, 19.0, color, 1.0);
    tile
}

/// Wi-Fi fan with its apex at (`cx`, `apex_y`); `bars` of three are lit.
fn wifi_glyph(canvas: &mut Canvas, cx: f32, apex_y: f32, scale: f32, bars: u8, color: Rgb) {
    let quarter = std::f32::consts::FRAC_PI_4;
    let mut wedge = Path::new();
    wedge.move_to(cx, apex_y);
    wedge.arc(cx, apex_y, 11.5 * scale, -3.0 * quarter, -quarter);
    wedge.close();
    canvas.fill_path(&wedge, color, if bars >= 1 { 1.0 } else { 0.3 });
    for (index, radius) in [23.0, 35.0].into_iter().enumerate() {
        let lit = bars as usize >= index + 2;
        canvas.ring_arc(
            cx,
            apex_y,
            radius * scale,
            7.0 * scale,
            -quarter + 0.08,
            quarter - 0.08,
            color,
            if lit { 1.0 } else { 0.3 },
        );
    }
}

fn signal_bars(signal: u8) -> u8 {
    match signal {
        0..=33 => 1,
        34..=66 => 2,
        _ => 3,
    }
}

fn speaker_glyph(canvas: &mut Canvas, cx: f32, cy: f32) {
    let mut body = Path::new();
    body.polygon(&[
        (cx - 18.0, cy - 8.0),
        (cx - 8.0, cy - 8.0),
        (cx + 6.0, cy - 20.0),
        (cx + 6.0, cy + 20.0),
        (cx - 8.0, cy + 8.0),
        (cx - 18.0, cy + 8.0),
    ]);
    canvas.fill_path(&body, WHITE, 1.0);
    canvas.ring_arc(cx + 4.0, cy, 16.0, 4.5, 0.75, 2.39, WHITE, 1.0);
}

fn lock_glyph(canvas: &mut Canvas, cx: f32, cy: f32, alpha: f32) {
    let points: Vec<(f32, f32)> = (0..=16)
        .map(|step| {
            let angle = std::f32::consts::PI + std::f32::consts::PI * step as f32 / 16.0;
            (cx + 9.0 * angle.cos(), cy - 5.0 + 9.0 * angle.sin())
        })
        .collect();
    let mut shackle = Path::new();
    let mut outline = vec![(cx - 9.0, cy + 1.0)];
    outline.extend(points);
    outline.push((cx + 9.0, cy + 1.0));
    shackle.stroke_polyline(&outline, 4.0);
    canvas.fill_path(&shackle, WHITE, alpha);
    canvas.fill_rrect(
        RectF::new(cx - 14.0, cy - 2.0, 28.0, 22.0),
        5.0,
        WHITE,
        alpha,
    );
}

fn checkmark(canvas: &mut Canvas, cx: f32, cy: f32) {
    let mut path = Path::new();
    path.stroke_polyline(
        &[
            (cx - 14.0, cy),
            (cx - 4.0, cy + 11.0),
            (cx + 16.0, cy - 13.0),
        ],
        6.0,
    );
    canvas.fill_path(&path, BLUE, 1.0);
}

fn spinner(canvas: &mut Canvas, cx: f32, cy: f32, radius: f32) {
    // A static activity indicator; it redraws once a second with metrics,
    // which is enough to show progress without animating continuously.
    let step = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() / 125)
        .unwrap_or(0)
        % 8) as u32;
    for spoke in 0..8_u32 {
        let angle = spoke as f32 * std::f32::consts::TAU / 8.0;
        let age = (step + 8 - spoke) % 8;
        let (sin, cos) = angle.sin_cos();
        let mut path = Path::new();
        path.stroke_segment(
            (cx + sin * radius * 0.5, cy - cos * radius * 0.5),
            (cx + sin * radius, cy - cos * radius),
            radius * 0.25,
        );
        canvas.fill_path(&path, SECONDARY, 0.9 - age as f32 * 0.1);
    }
}

fn wifi_summary(app: &AppModel) -> String {
    match &app.wifi_status {
        None => "Not Connected".into(),
        Some(status) if !status.radio_enabled => "Off".into(),
        Some(status) => match &status.state {
            LinkState::Connected { ssid, .. } => ssid.clone(),
            LinkState::Connecting { .. } => "Connecting…".into(),
            LinkState::Disconnecting => "Disconnecting…".into(),
            LinkState::Disconnected => "Not Connected".into(),
        },
    }
}

fn draw_system(
    canvas: &mut Canvas,
    system: &SystemInfo,
    volume: Option<u8>,
    app: &AppModel,
    pressed: Option<Action>,
) {
    large_title(canvas, "System", 132.0);

    // Wi-Fi and Sound.
    let top = card(SYSTEM_CARD_Y, ROW * 2.0);
    grouped_card(
        canvas,
        top,
        2,
        (pressed == Some(Action::ShowWifi)).then_some(0),
    );
    let right = MARGIN + CARD_WIDTH - 44.0;
    let wifi_cy = SYSTEM_CARD_Y + ROW / 2.0;
    icon_tile(canvas, TEXT_X - 4.0, wifi_cy, BLUE);
    wifi_glyph(canvas, TEXT_X + 36.0, wifi_cy + 17.0, 0.95, 3, WHITE);
    let label_x = TEXT_X + 112.0;
    text::draw(
        canvas,
        "Wi-Fi",
        label_x,
        baseline(wifi_cy, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    chevron(canvas, right, wifi_cy, SECONDARY, TERTIARY_ALPHA);
    let summary = text::fit(&wifi_summary(app), Weight::Regular, LABEL_SIZE, 440.0);
    text::draw_right(
        canvas,
        &summary,
        right - 40.0,
        baseline(wifi_cy, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    separator(canvas, SYSTEM_CARD_Y + ROW - 1.0, label_x);
    let sound_cy = SYSTEM_CARD_Y + ROW * 1.5;
    icon_tile(canvas, TEXT_X - 4.0, sound_cy, PINK);
    speaker_glyph(canvas, TEXT_X + 34.0, sound_cy);
    text::draw(
        canvas,
        "Volume",
        label_x,
        baseline(sound_cy, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    let volume_text = volume.map_or_else(|| "--".into(), |value| format!("{value}%"));
    text::draw_right(
        canvas,
        &volume_text,
        right,
        baseline(sound_cy, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        SECONDARY,
        SECONDARY_ALPHA,
    );

    // Activity: CPU and GPU meters side by side.
    section_header(canvas, "ACTIVITY", ACTIVITY_Y);
    grouped_card(canvas, card(ACTIVITY_Y, ACTIVITY_HEIGHT), 0, None);
    let half = CARD_WIDTH / 2.0;
    canvas.fill_rect(
        (MARGIN + half) as i32,
        (ACTIVITY_Y + 36.0) as i32,
        2,
        (ACTIVITY_HEIGHT - 72.0) as i32,
        SEPARATOR,
        1.0,
    );
    for (column, (name, usage, color)) in [
        ("CPU", system.cpu_usage, GREEN),
        ("GPU", system.gpu_usage, BLUE),
    ]
    .into_iter()
    .enumerate()
    {
        let x = TEXT_X + column as f32 * half;
        text::draw(
            canvas,
            name,
            x,
            ACTIVITY_Y + 68.0,
            Weight::SemiBold,
            36.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
        text::draw(
            canvas,
            &usage.map_or_else(|| "—".to_owned(), |_| usage_text(usage)),
            x - 3.0,
            ACTIVITY_Y + 158.0,
            Weight::DisplaySemiBold,
            84.0,
            WHITE,
            1.0,
        );
        let bar = RectF::new(x, ACTIVITY_Y + 186.0, half - 88.0, 14.0);
        canvas.fill_rrect(bar, 7.0, TRACK, 1.0);
        if let Some(value) = usage {
            let width = bar.w * f32::from(value.min(100)) / 100.0;
            if width >= 1.0 {
                canvas.fill_rrect(
                    RectF::new(bar.x, bar.y, width.max(14.0), bar.h),
                    7.0,
                    color,
                    1.0,
                );
            }
        }
    }

    // Storage bar.
    section_header(canvas, "STORAGE", STORAGE_Y);
    grouped_card(canvas, card(STORAGE_Y, STORAGE_HEIGHT), 0, None);
    text::draw(
        canvas,
        &format!("{} used", system.storage_used),
        TEXT_X,
        STORAGE_Y + 82.0,
        Weight::SemiBold,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    text::draw_right(
        canvas,
        &format!("{} free", system.storage_free),
        right,
        STORAGE_Y + 82.0,
        Weight::Regular,
        LABEL_SIZE,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    let bar = RectF::new(TEXT_X, STORAGE_Y + 124.0, CARD_WIDTH - 88.0, 30.0);
    canvas.fill_rrect(bar, 8.0, TRACK, 1.0);
    if let Some(fraction) = system.storage_fraction {
        let used_right = (bar.x + bar.w * fraction.clamp(0.0, 1.0)).round() as i32;
        canvas.set_clip(0, 0, used_right, canvas.height);
        canvas.fill_rrect(bar, 8.0, BLUE, 1.0);
        canvas.reset_clip();
    }
    let legend_y = STORAGE_Y + 204.0;
    canvas.fill_circle(TEXT_X + 10.0, legend_y - 12.0, 10.0, BLUE, 1.0);
    text::draw(
        canvas,
        "Used",
        TEXT_X + 32.0,
        legend_y,
        Weight::Regular,
        36.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );
    canvas.fill_circle(TEXT_X + 160.0, legend_y - 12.0, 10.0, TRACK, 1.0);
    text::draw(
        canvas,
        "Free",
        TEXT_X + 182.0,
        legend_y,
        Weight::Regular,
        36.0,
        SECONDARY,
        SECONDARY_ALPHA,
    );

    // About.
    section_header(canvas, "ABOUT", ABOUT_Y);
    let rows = [
        ("Device", "Samsung Galaxy A26".to_owned()),
        (
            "Software",
            format!("Alpine Linux {}", title_case(&system.alpine)),
        ),
        ("Kernel", system.kernel.clone()),
        ("Memory", system.memory.clone()),
        (
            "Display",
            format!("{} × {} · 120 Hz", canvas.width, canvas.height + 124),
        ),
    ];
    info_rows(canvas, ABOUT_Y, &rows);
}

fn title_case(value: &str) -> String {
    if value.eq_ignore_ascii_case("unknown") {
        "".into()
    } else {
        value.to_owned()
    }
}

fn info_rows(canvas: &mut Canvas, y: f32, rows: &[(&str, String)]) {
    grouped_card(canvas, card(y, INFO_ROW * rows.len() as f32), 0, None);
    let right = MARGIN + CARD_WIDTH - 44.0;
    for (index, (label, value)) in rows.iter().enumerate() {
        let cy = y + INFO_ROW * (index as f32 + 0.5);
        let line = baseline(cy, Weight::Regular, LABEL_SIZE);
        let label_width = text::draw(
            canvas,
            label,
            TEXT_X,
            line,
            Weight::Regular,
            LABEL_SIZE,
            WHITE,
            1.0,
        );
        let available = right - TEXT_X - label_width - 40.0;
        let value = text::fit(value.trim(), Weight::Regular, LABEL_SIZE, available);
        text::draw_right(
            canvas,
            &value,
            right,
            line,
            Weight::Regular,
            LABEL_SIZE,
            SECONDARY,
            SECONDARY_ALPHA,
        );
        if index + 1 < rows.len() {
            separator(canvas, y + INFO_ROW * (index as f32 + 1.0) - 1.0, TEXT_X);
        }
    }
}

/// Wi-Fi page geometry, shared by drawing and hit testing.
struct WifiLayout {
    status_rows: usize,
    disconnect_row: Option<RectF>,
    networks_y: f32,
    scan_button: RectF,
    previous: RectF,
    next: RectF,
}

impl WifiLayout {
    fn new(app: &AppModel) -> Self {
        let connected = matches!(
            app.wifi_status.as_ref().map(|status| &status.state),
            Some(LinkState::Connected { .. })
        );
        let status_rows = if connected { 2 } else { 1 };
        let disconnect_row = connected.then(|| card(STATUS_CARD_Y + ROW, ROW));
        let status_bottom = STATUS_CARD_Y + ROW * status_rows as f32;
        // Leave room for an inline notice under the status card.
        let networks_y = status_bottom + 190.0;
        let rows = app.page_range().len().max(1) as f32;
        let pager_y = networks_y + NETWORK_ROW * rows + 30.0;
        Self {
            status_rows,
            disconnect_row,
            networks_y,
            scan_button: RectF::new(700.0, networks_y - 110.0, 380.0, 110.0),
            previous: RectF::new(MARGIN, pager_y, 330.0, 120.0),
            next: RectF::new(1080.0 - MARGIN - 330.0, pager_y, 330.0, 120.0),
        }
    }

    fn network_row(&self, row: usize) -> RectF {
        card(self.networks_y + row as f32 * NETWORK_ROW, NETWORK_ROW)
    }
}

fn draw_wifi(canvas: &mut Canvas, app: &AppModel, pressed: Option<Action>) {
    let back_pressed = pressed == Some(Action::ShowSystem);
    back_chevron(canvas, MARGIN + 6.0, 72.0);
    text::draw(
        canvas,
        "System",
        MARGIN + 36.0,
        baseline(72.0, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        BLUE,
        if back_pressed { 0.4 } else { 1.0 },
    );
    large_title(canvas, "Wi-Fi", 236.0);

    let layout = WifiLayout::new(app);
    let right = MARGIN + CARD_WIDTH - 44.0;
    let status_card = card(STATUS_CARD_Y, ROW * layout.status_rows as f32);
    let pressed_status = (pressed == Some(Action::Disconnect)).then_some(1);
    grouped_card(canvas, status_card, layout.status_rows, pressed_status);
    let cy = STATUS_CARD_Y + ROW / 2.0;
    let line = baseline(cy, Weight::Regular, LABEL_SIZE);
    match app.wifi_status.as_ref() {
        None => {
            text::draw(
                canvas,
                "Wi-Fi Service Unavailable",
                TEXT_X,
                line,
                Weight::Regular,
                LABEL_SIZE,
                SECONDARY,
                SECONDARY_ALPHA,
            );
        }
        Some(status) if !status.radio_enabled => {
            text::draw(
                canvas,
                "Wi-Fi Is Off",
                TEXT_X,
                line,
                Weight::Regular,
                LABEL_SIZE,
                SECONDARY,
                SECONDARY_ALPHA,
            );
        }
        Some(status) => match &status.state {
            LinkState::Connected {
                ssid,
                signal,
                ip_address,
            } => {
                checkmark(canvas, TEXT_X + 14.0, cy);
                let name = text::fit(ssid, Weight::Regular, LABEL_SIZE, 560.0);
                text::draw(
                    canvas,
                    &name,
                    TEXT_X + 64.0,
                    cy - 4.0,
                    Weight::Regular,
                    LABEL_SIZE,
                    WHITE,
                    1.0,
                );
                let detail = ip_address
                    .as_deref()
                    .map_or_else(|| "Connected".to_owned(), |ip| format!("Connected · {ip}"));
                text::draw(
                    canvas,
                    &detail,
                    TEXT_X + 64.0,
                    cy + 42.0,
                    Weight::Regular,
                    34.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
                wifi_glyph(
                    canvas,
                    right - 26.0,
                    cy + 16.0,
                    0.95,
                    signal.map_or(3, signal_bars),
                    WHITE,
                );
                separator(canvas, STATUS_CARD_Y + ROW - 1.0, TEXT_X);
                text::draw(
                    canvas,
                    "Disconnect",
                    TEXT_X,
                    baseline(STATUS_CARD_Y + ROW * 1.5, Weight::Regular, LABEL_SIZE),
                    Weight::Regular,
                    LABEL_SIZE,
                    RED,
                    if app.busy { 0.4 } else { 1.0 },
                );
            }
            LinkState::Connecting { ssid } => {
                spinner(canvas, TEXT_X + 18.0, cy, 22.0);
                let name = text::fit(
                    &format!("Joining “{ssid}”…"),
                    Weight::Regular,
                    LABEL_SIZE,
                    760.0,
                );
                text::draw(
                    canvas,
                    &name,
                    TEXT_X + 64.0,
                    line,
                    Weight::Regular,
                    LABEL_SIZE,
                    WHITE,
                    1.0,
                );
            }
            LinkState::Disconnecting => {
                spinner(canvas, TEXT_X + 18.0, cy, 22.0);
                text::draw(
                    canvas,
                    "Disconnecting…",
                    TEXT_X + 64.0,
                    line,
                    Weight::Regular,
                    LABEL_SIZE,
                    WHITE,
                    1.0,
                );
            }
            LinkState::Disconnected => {
                text::draw(
                    canvas,
                    "Not Connected",
                    TEXT_X,
                    line,
                    Weight::Regular,
                    LABEL_SIZE,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
            }
        },
    }
    // The status row already says "Joining …" or "Service Unavailable".
    let joining = matches!(
        app.wifi_status.as_ref().map(|status| &status.state),
        Some(LinkState::Connecting { .. })
    );
    let redundant = (app.wifi_status.is_none() && app.notice == Some("Wi-Fi service unavailable"))
        || (joining && app.notice == Some("Connecting…"));
    if let Some(notice) = app.notice.filter(|_| !redundant) {
        let error = !matches!(notice, "Connected" | "Disconnected" | "Connecting…");
        footer(
            canvas,
            notice,
            status_card.y + status_card.h,
            if error { RED } else { SECONDARY },
            if error { 1.0 } else { SECONDARY_ALPHA },
        );
    }

    section_header(canvas, "NETWORKS", layout.networks_y);
    let scan_baseline = layout.networks_y - 26.0;
    if app.scanning {
        spinner(canvas, right - 20.0, scan_baseline - 13.0, 20.0);
    } else {
        let scan_pressed = pressed == Some(Action::RefreshWifi);
        text::draw_right(
            canvas,
            "Scan",
            right,
            scan_baseline,
            Weight::Regular,
            42.0,
            BLUE,
            if app.busy || scan_pressed { 0.4 } else { 1.0 },
        );
    }

    let range = app.page_range();
    if range.is_empty() {
        let empty = card(layout.networks_y, NETWORK_ROW);
        grouped_card(canvas, empty, 1, None);
        let message = if app.scanning {
            "Looking for networks…"
        } else {
            "No Networks Found"
        };
        text::draw(
            canvas,
            message,
            TEXT_X,
            baseline(empty.y + empty.h / 2.0, Weight::Regular, LABEL_SIZE),
            Weight::Regular,
            LABEL_SIZE,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    } else {
        let rows = range.len();
        let pressed_row = match pressed {
            Some(Action::SelectNetwork(index)) if range.contains(&index) => {
                Some(index - range.start)
            }
            _ => None,
        };
        grouped_card(
            canvas,
            card(layout.networks_y, NETWORK_ROW * rows as f32),
            rows,
            pressed_row,
        );
        for (row, index) in range.enumerate() {
            let rect = layout.network_row(row);
            network_row(canvas, rect, &app.networks[index], app.busy);
            if row + 1 < rows {
                separator(canvas, rect.y + rect.h - 1.0, TEXT_X);
            }
        }
    }

    if app.page_count() > 1 {
        let cy = layout.previous.y + layout.previous.h / 2.0;
        let line = baseline(cy, Weight::Regular, 42.0);
        let previous_enabled = app.page > 0;
        let next_enabled = app.page + 1 < app.page_count();
        text::draw(
            canvas,
            "Previous",
            TEXT_X,
            line,
            Weight::Regular,
            42.0,
            BLUE,
            if previous_enabled { 1.0 } else { 0.3 },
        );
        text::draw_right(
            canvas,
            "Next",
            right,
            line,
            Weight::Regular,
            42.0,
            BLUE,
            if next_enabled { 1.0 } else { 0.3 },
        );
        text::draw_centered(
            canvas,
            &format!("{} of {}", app.page + 1, app.page_count()),
            540.0,
            line,
            Weight::Regular,
            38.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
}

fn network_row(canvas: &mut Canvas, rect: RectF, network: &WifiNetwork, busy: bool) {
    let cy = rect.y + rect.h / 2.0;
    let right = rect.x + rect.w - 44.0;
    let unsupported = !network.joinable();
    let mut name_x = TEXT_X;
    if network.connected {
        checkmark(canvas, TEXT_X + 14.0, cy);
        name_x += 64.0;
    }
    let alpha = if busy || unsupported { 0.45 } else { 1.0 };
    let name = text::fit(&network.ssid, Weight::Regular, LABEL_SIZE, 560.0);
    text::draw(
        canvas,
        &name,
        name_x,
        baseline(cy, Weight::Regular, LABEL_SIZE),
        Weight::Regular,
        LABEL_SIZE,
        WHITE,
        alpha,
    );
    wifi_glyph(
        canvas,
        right - 26.0,
        cy + 16.0,
        0.95,
        signal_bars(network.signal),
        WHITE,
    );
    if network.requires_password {
        lock_glyph(canvas, right - 92.0, cy - 2.0, 0.9);
    }
    if unsupported {
        text::draw_right(
            canvas,
            "Unsupported",
            right - 130.0,
            baseline(cy, Weight::Regular, 36.0),
            Weight::Regular,
            36.0,
            SECONDARY,
            SECONDARY_ALPHA,
        );
    }
}

fn draw_password(canvas: &mut Canvas, dialog: &PasswordDialog, pressed: Option<Action>) {
    let nav_cy = NAV_HEIGHT / 2.0;
    let nav_line = baseline(nav_cy, Weight::Regular, LABEL_SIZE);
    text::draw(
        canvas,
        "Cancel",
        MARGIN,
        nav_line,
        Weight::Regular,
        LABEL_SIZE,
        BLUE,
        if pressed == Some(Action::CancelPassword) {
            0.4
        } else {
            1.0
        },
    );
    text::draw_centered(
        canvas,
        "Enter Password",
        540.0,
        nav_line,
        Weight::SemiBold,
        LABEL_SIZE,
        WHITE,
        1.0,
    );
    let join_alpha = if !dialog.password_is_valid() {
        0.3
    } else if pressed == Some(Action::SubmitPassword) {
        0.4
    } else {
        1.0
    };
    text::draw_right(
        canvas,
        "Join",
        1080.0 - MARGIN,
        nav_line,
        Weight::SemiBold,
        LABEL_SIZE,
        BLUE,
        join_alpha,
    );

    let prompt = if dialog.enterprise.is_some() {
        format!("Sign in to “{}”", dialog.ssid)
    } else {
        format!("Enter the password for “{}”", dialog.ssid)
    };
    let prompt = text::fit(&prompt, Weight::SemiBold, 40.0, CARD_WIDTH);
    text::draw_centered(
        canvas,
        &prompt,
        540.0,
        PASSWORD_CARD_Y - 60.0,
        Weight::SemiBold,
        40.0,
        WHITE,
        1.0,
    );

    let fields = dialog.fields();
    grouped_card(
        canvas,
        card(PASSWORD_CARD_Y, ROW * fields.len() as f32),
        fields.len(),
        None,
    );
    let value_x = TEXT_X
        + if dialog.enterprise.is_some() {
            250.0
        } else {
            260.0
        };
    let value_width = MARGIN + CARD_WIDTH - 44.0 - value_x - 12.0;
    for (index, field) in fields.iter().copied().enumerate() {
        let rect = field_rect(index);
        let cy = rect.y + rect.h / 2.0;
        let line = baseline(cy, Weight::Regular, LABEL_SIZE);
        let label = match field {
            Field::Identity => "Username",
            Field::Password => "Password",
            Field::Domain => "Server",
        };
        let flagged = dialog.submit_attempted && !dialog.field_is_valid(field);
        text::draw(
            canvas,
            label,
            TEXT_X,
            line,
            Weight::Regular,
            LABEL_SIZE,
            if flagged { RED } else { WHITE },
            1.0,
        );
        let focused = dialog.focus == field;
        let caret_x = match field {
            Field::Password => {
                let pitch = 30.0;
                let fit = ((value_width - 20.0) / pitch) as usize;
                let visible = dialog.password_len().min(fit);
                for dot in 0..visible {
                    canvas.fill_circle(value_x + 12.0 + dot as f32 * pitch, cy, 9.0, WHITE, 1.0);
                }
                (visible > 0 || focused).then_some(value_x + visible as f32 * pitch + 2.0)
            }
            Field::Identity | Field::Domain => {
                let value = match field {
                    Field::Identity => dialog.identity_text(),
                    _ => dialog.enterprise.as_ref().map_or("", |e| e.domain.as_str()),
                };
                if value.is_empty() {
                    None
                } else {
                    let shown = tail_fit(value, Weight::Regular, LABEL_SIZE, value_width - 12.0);
                    let width = text::draw(
                        canvas,
                        &shown,
                        value_x,
                        line,
                        Weight::Regular,
                        LABEL_SIZE,
                        WHITE,
                        1.0,
                    );
                    Some(value_x + width + 4.0)
                }
            }
        };
        if caret_x.is_none() && !(focused && field == Field::Password) {
            let placeholder = match field {
                Field::Domain => "radius.example.edu",
                _ => "Required",
            };
            text::draw(
                canvas,
                placeholder,
                value_x + if focused { 12.0 } else { 0.0 },
                line,
                Weight::Regular,
                LABEL_SIZE,
                SECONDARY,
                TERTIARY_ALPHA,
            );
        }
        if focused {
            canvas.fill_rrect(
                RectF::new(caret_x.unwrap_or(value_x), cy - 30.0, 5.0, 60.0),
                2.5,
                BLUE,
                1.0,
            );
        }
        if index + 1 < fields.len() {
            separator(canvas, rect.y + rect.h - 1.0, TEXT_X);
        }
    }

    let invalid = fields
        .iter()
        .copied()
        .find(|field| !dialog.field_is_valid(*field));
    let (hint, color, alpha) = match (dialog.submit_attempted, invalid, &dialog.enterprise) {
        (true, Some(Field::Password), None) => {
            ("Use 8–63 characters or a 64-digit hex key.", RED, 1.0)
        }
        (true, Some(Field::Identity), _) => ("Enter your username.", RED, 1.0),
        (true, Some(Field::Password), Some(_)) => ("Enter your password.", RED, 1.0),
        (true, Some(Field::Domain), _) => {
            ("Enter the server name, like radius.example.edu.", RED, 1.0)
        }
        (_, _, Some(_)) => (
            "Server is the name its certificate must match.",
            SECONDARY,
            SECONDARY_ALPHA,
        ),
        _ => (
            "Sent only to the phone’s own Wi-Fi service.",
            SECONDARY,
            SECONDARY_ALPHA,
        ),
    };
    footer(canvas, hint, fields_bottom(dialog), color, alpha);
    let _ = dialog.security;

    if !dialog.system_keyboard {
        for key in fallback_keys(dialog.keyboard_mode, fallback_keyboard_y(dialog)) {
            let key_pressed = pressed == Some(Action::Key(key.action));
            let special = !matches!(key.action, KeyAction::Character(_) | KeyAction::Space);
            let fill = match (key_pressed, special) {
                (true, _) => 0x8a8a8f,
                (false, true) => 0x38383b,
                (false, false) => 0x5a5a5e,
            };
            canvas.fill_rrect(
                RectF::new(key.bounds.x, key.bounds.y + 3.0, key.bounds.w, key.bounds.h),
                15.0,
                0x000000,
                0.55,
            );
            canvas.fill_rrect(key.bounds, 15.0, fill, 1.0);
            let size = if key.label.chars().count() == 1 {
                60.0
            } else {
                40.0
            };
            let (kx, ky) = key.bounds.center();
            text::draw_centered(
                canvas,
                &key.label,
                kx,
                baseline(ky, Weight::Regular, size),
                Weight::Regular,
                size,
                WHITE,
                1.0,
            );
        }
    }
}

/// Shows the end of `text` (where typing happens) with a leading ellipsis
/// when it is wider than `max_width`.
fn tail_fit(text: &str, weight: Weight, size: f32, max_width: f32) -> String {
    if text::measure(text, weight, size) <= max_width {
        return text.to_owned();
    }
    let characters: Vec<char> = text.chars().collect();
    for start in 1..characters.len() {
        let candidate: String = std::iter::once('…')
            .chain(characters[start..].iter().copied())
            .collect();
        if text::measure(&candidate, weight, size) <= max_width {
            return candidate;
        }
    }
    "…".into()
}

#[derive(Clone)]
struct KeySpec {
    bounds: RectF,
    label: String,
    action: KeyAction,
}

/// The in-app keyboard, used only when Moon's system keyboard is unavailable.
fn fallback_keys(mode: KeyboardMode, top: f32) -> Vec<KeySpec> {
    let mut keys = Vec::new();
    let row = |index: usize| top + index as f32 * 150.0;
    match mode {
        KeyboardMode::Lower | KeyboardMode::Upper => {
            character_row(&mut keys, "qwertyuiop", 16.0, row(0), 1048.0, mode);
            character_row(&mut keys, "asdfghjkl", 68.0, row(1), 944.0, mode);
            keys.push(KeySpec {
                bounds: RectF::new(16.0, row(2), 140.0, 132.0),
                label: if mode == KeyboardMode::Upper {
                    "⇧".into()
                } else {
                    "shift".into()
                },
                action: KeyAction::Shift,
            });
            character_row(&mut keys, "zxcvbnm", 172.0, row(2), 736.0, mode);
            keys.push(KeySpec {
                bounds: RectF::new(924.0, row(2), 140.0, 132.0),
                label: "delete".into(),
                action: KeyAction::Backspace,
            });
            bottom_keys(
                &mut keys,
                row(3),
                "123",
                KeyAction::Symbols,
                "clear",
                KeyAction::Clear,
            );
        }
        KeyboardMode::Symbols => {
            character_row(&mut keys, "1234567890", 16.0, row(0), 1048.0, mode);
            character_row(&mut keys, "!@#$%^&*()", 16.0, row(1), 1048.0, mode);
            character_row(&mut keys, "-_+=[]{}:;", 16.0, row(2), 1048.0, mode);
            symbol_bottom_keys(&mut keys, row(3), "#+=", KeyAction::MoreSymbols);
        }
        KeyboardMode::MoreSymbols => {
            character_row(&mut keys, "`~|\\/\"'<>?", 16.0, row(0), 1048.0, mode);
            character_row(&mut keys, ".,:;!@#$%&", 16.0, row(1), 1048.0, mode);
            character_row(&mut keys, "^*()_+-=[]", 16.0, row(2), 1048.0, mode);
            symbol_bottom_keys(&mut keys, row(3), "123", KeyAction::Symbols);
        }
    }
    keys
}

fn character_row(
    keys: &mut Vec<KeySpec>,
    characters: &str,
    x: f32,
    y: f32,
    width: f32,
    mode: KeyboardMode,
) {
    let count = characters.chars().count() as f32;
    let gap = 12.0;
    let key_width = (width - gap * (count - 1.0)) / count;
    for (index, character) in characters.chars().enumerate() {
        let label = if mode == KeyboardMode::Upper {
            character.to_ascii_uppercase()
        } else {
            character
        };
        keys.push(KeySpec {
            bounds: RectF::new(x + index as f32 * (key_width + gap), y, key_width, 132.0),
            label: label.to_string(),
            action: KeyAction::Character(character),
        });
    }
}

fn bottom_keys(
    keys: &mut Vec<KeySpec>,
    y: f32,
    left_label: &str,
    left_action: KeyAction,
    right_label: &str,
    right_action: KeyAction,
) {
    keys.extend([
        KeySpec {
            bounds: RectF::new(16.0, y, 220.0, 132.0),
            label: left_label.into(),
            action: left_action,
        },
        KeySpec {
            bounds: RectF::new(248.0, y, 584.0, 132.0),
            label: "space".into(),
            action: KeyAction::Space,
        },
        KeySpec {
            bounds: RectF::new(844.0, y, 220.0, 132.0),
            label: right_label.into(),
            action: right_action,
        },
    ]);
}

fn symbol_bottom_keys(keys: &mut Vec<KeySpec>, y: f32, right_label: &str, right_action: KeyAction) {
    keys.extend([
        KeySpec {
            bounds: RectF::new(16.0, y, 200.0, 132.0),
            label: "ABC".into(),
            action: KeyAction::Letters,
        },
        KeySpec {
            bounds: RectF::new(228.0, y, 420.0, 132.0),
            label: "space".into(),
            action: KeyAction::Space,
        },
        KeySpec {
            bounds: RectF::new(660.0, y, 200.0, 132.0),
            label: "delete".into(),
            action: KeyAction::Backspace,
        },
        KeySpec {
            bounds: RectF::new(872.0, y, 192.0, 132.0),
            label: right_label.into(),
            action: right_action,
        },
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::{Security, WifiNetwork};

    const HEIGHT: f32 = 2216.0;

    #[test]
    fn system_and_wifi_hit_targets_are_large_and_deterministic() {
        let mut app = AppModel::default();
        let (wx, wy) = wifi_row().center();
        assert_eq!(
            action_at(&app, HEIGHT, wx as i16, wy as i16),
            Some(Action::ShowWifi)
        );
        assert_eq!(action_at(&app, HEIGHT, 540, 1400), None);
        assert_eq!(action_at(&app, HEIGHT, 540, 2150), None);

        app.view = View::Wifi;
        app.networks.push(WifiNetwork {
            ssid: "test".into(),
            signal: 70,
            security: Security::Open,
            requires_password: false,
            connected: false,
        });
        let row = WifiLayout::new(&app).network_row(0);
        assert_eq!(
            action_at(&app, HEIGHT, 500, (row.y + row.h / 2.0) as i16),
            Some(Action::SelectNetwork(0))
        );
        assert_eq!(action_at(&app, HEIGHT, 120, 70), Some(Action::ShowSystem));
        assert_eq!(action_at(&app, HEIGHT, wx as i16, wy as i16), None);
    }

    #[test]
    fn password_keyboard_mapping_matches_rendered_keys() {
        let mut app = AppModel::default();
        app.networks.push(WifiNetwork {
            ssid: "secure".into(),
            signal: 70,
            security: Security::Wpa2,
            requires_password: true,
            connected: false,
        });
        app.handle(Action::SelectNetwork(0));
        let keyboard_y = fallback_keyboard_y(app.password_dialog.as_ref().unwrap());
        assert_eq!(keyboard_y, 640.0);
        assert_eq!(
            action_at(&app, HEIGHT, 60, (keyboard_y + 60.0) as i16),
            Some(Action::Key(KeyAction::Character('q')))
        );
        assert_eq!(
            action_at(&app, HEIGHT, 960, 70),
            Some(Action::SubmitPassword)
        );
        assert_eq!(
            action_at(&app, HEIGHT, 100, 70),
            Some(Action::CancelPassword)
        );
        assert_eq!(action_at(&app, HEIGHT, 500, 2150), None);

        // With Moon's keyboard up, the in-app keys are neither drawn nor hit.
        app.password_dialog.as_mut().unwrap().system_keyboard = true;
        assert_eq!(
            action_at(&app, 1396.0, 60, (keyboard_y + 60.0) as i16),
            None
        );
        let (fx, fy) = field_rect(0).center();
        assert_eq!(
            action_at(&app, 1396.0, fx as i16, fy as i16),
            Some(Action::Focus(Field::Password))
        );
    }

    #[test]
    fn enterprise_join_has_three_tappable_fields_and_a_lower_fallback_keyboard() {
        let mut app = AppModel::default();
        app.networks.push(WifiNetwork {
            ssid: "eduroam".into(),
            signal: 70,
            security: Security::Enterprise,
            requires_password: true,
            connected: false,
        });
        app.handle(Action::SelectNetwork(0));
        let dialog = app.password_dialog.as_ref().unwrap();
        for (index, field) in [Field::Identity, Field::Password, Field::Domain]
            .into_iter()
            .enumerate()
        {
            let (x, y) = field_rect(index).center();
            assert_eq!(
                action_at(&app, 1396.0, x as i16, y as i16),
                Some(Action::Focus(field))
            );
        }
        let keyboard_y = fallback_keyboard_y(dialog);
        assert!(keyboard_y >= field_rect(2).y + ROW + 150.0);
        assert!(keyboard_y + 4.0 * 150.0 <= HEIGHT - BOTTOM_GESTURE_RESERVE);
        assert_eq!(
            action_at(&app, HEIGHT, 60, (keyboard_y + 60.0) as i16),
            Some(Action::Key(KeyAction::Character('q')))
        );
    }

    #[test]
    fn long_values_show_their_end() {
        let shown = tail_fit(
            "a.very.long.identity.that.keeps.going@example.edu",
            Weight::Regular,
            LABEL_SIZE,
            300.0,
        );
        assert!(shown.starts_with('…'));
        assert!(shown.ends_with(".edu"));
        assert!(text::measure(&shown, Weight::Regular, LABEL_SIZE) <= 300.0);
        assert_eq!(
            tail_fit("short", Weight::Regular, LABEL_SIZE, 300.0),
            "short"
        );
    }

    #[test]
    fn a_full_network_page_stays_clear_of_the_gesture_zone() {
        let mut app = AppModel::default();
        app.view = View::Wifi;
        app.networks = (0..20)
            .map(|index| WifiNetwork {
                ssid: format!("n{index}"),
                signal: 50,
                security: Security::Open,
                requires_password: false,
                connected: false,
            })
            .collect();
        let layout = WifiLayout::new(&app);
        let last = layout.network_row(crate::app::NETWORKS_PER_PAGE - 1);
        assert!(last.y + last.h <= layout.previous.y);
        assert!(layout.previous.y + layout.previous.h <= HEIGHT - BOTTOM_GESTURE_RESERVE);
    }
}
