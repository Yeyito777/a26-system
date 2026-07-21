use std::error::Error;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ChangeGCAux, ConnectionExt as _, Gcontext, Rectangle, Window};

use crate::app::{Action, AppModel, KeyAction, KeyboardMode, PasswordDialog, View};
use crate::font;
use crate::metrics::{SystemInfo, usage_text};
use crate::wifi::{LinkState, WifiNetwork};

pub const BG: u32 = 0x0b1020;
const BG_CARD: u32 = 0x182235;
const BG_KEY: u32 = 0x24334d;
const BG_DISABLED: u32 = 0x263043;
const FG: u32 = 0xf5f7fb;
const MUTED: u32 = 0x94a3b8;
const ACCENT: u32 = 0x5eead4;
const ACCENT_2: u32 = 0x60a5fa;
const WARNING: u32 = 0xfbbf24;
const DANGER: u32 = 0xfb7185;

const SYSTEM_TAB: UiRect = UiRect::new(55, 65, 250, 120);
const WIFI_TAB: UiRect = UiRect::new(325, 65, 220, 120);
const STATUS_ACTION: UiRect = UiRect::new(735, 285, 270, 120);
const REFRESH_BUTTON: UiRect = UiRect::new(765, 495, 240, 105);
const NETWORK_X: i16 = 55;
const NETWORK_Y: i16 = 670;
const NETWORK_WIDTH: u16 = 970;
const NETWORK_HEIGHT: u16 = 210;
const NETWORK_STEP: i16 = 230;
const PREVIOUS_BUTTON: UiRect = UiRect::new(55, 1870, 190, 110);
const NEXT_BUTTON: UiRect = UiRect::new(835, 1870, 190, 110);
const CANCEL_BUTTON: UiRect = UiRect::new(755, 75, 270, 110);
const CONNECT_BUTTON: UiRect = UiRect::new(55, 1490, 970, 160);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UiRect {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

impl UiRect {
    const fn new(x: i16, y: i16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn contains(self, x: i16, y: i16) -> bool {
        let right = i32::from(self.x) + i32::from(self.width);
        let bottom = i32::from(self.y) + i32::from(self.height);
        i32::from(x) >= i32::from(self.x)
            && i32::from(x) < right
            && i32::from(y) >= i32::from(self.y)
            && i32::from(y) < bottom
    }

    fn x11(self) -> Rectangle {
        Rectangle {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone)]
struct KeySpec {
    bounds: UiRect,
    label: String,
    action: KeyAction,
}

pub struct Renderer {
    window: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
}

impl Renderer {
    pub fn new(window: Window, gc: Gcontext, width: u16, height: u16) -> Self {
        Self {
            window,
            gc,
            width,
            height,
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn render<C: Connection>(
        &self,
        conn: &C,
        system: &SystemInfo,
        volume: Option<u8>,
        app: &AppModel,
    ) -> Result<(), Box<dyn Error>> {
        self.fill(conn, BG, UiRect::new(0, 0, self.width, self.height).x11())?;

        if let Some(dialog) = &app.password_dialog {
            self.render_password(conn, dialog)?;
        } else {
            match app.view {
                View::System => self.render_system(conn, system, volume)?,
                View::Wifi => self.render_wifi(conn, app)?,
            }
        }
        conn.flush()?;
        Ok(())
    }

    pub fn action_at(&self, app: &AppModel, x: i16, y: i16) -> Option<Action> {
        if app.password_dialog.is_some() {
            if CANCEL_BUTTON.contains(x, y) {
                return Some(Action::CancelPassword);
            }
            if CONNECT_BUTTON.contains(x, y) {
                return Some(Action::SubmitPassword);
            }
            let dialog = app.password_dialog.as_ref()?;
            return keyboard_keys(dialog.keyboard_mode)
                .into_iter()
                .find(|key| key.bounds.contains(x, y))
                .map(|key| Action::Key(key.action));
        }

        match app.view {
            View::System => WIFI_TAB.contains(x, y).then_some(Action::ShowWifi),
            View::Wifi => {
                if SYSTEM_TAB.contains(x, y) {
                    return Some(Action::ShowSystem);
                }
                if REFRESH_BUTTON.contains(x, y) {
                    return Some(Action::RefreshWifi);
                }
                if STATUS_ACTION.contains(x, y)
                    && !app.busy
                    && matches!(
                        app.wifi_status.as_ref().map(|status| &status.state),
                        Some(LinkState::Connected { .. })
                    )
                {
                    return Some(Action::Disconnect);
                }
                for (row, index) in app.page_range().enumerate() {
                    let bounds = network_bounds(row);
                    if bounds.contains(x, y) {
                        return Some(Action::SelectNetwork(index));
                    }
                }
                if PREVIOUS_BUTTON.contains(x, y) && app.page > 0 {
                    return Some(Action::PreviousNetworks);
                }
                if NEXT_BUTTON.contains(x, y) && app.page + 1 < app.page_count() {
                    return Some(Action::NextNetworks);
                }
                None
            }
        }
    }

    fn render_system<C: Connection>(
        &self,
        conn: &C,
        system: &SystemInfo,
        volume: Option<u8>,
    ) -> Result<(), Box<dyn Error>> {
        self.navigation(conn, View::System)?;
        let volume = volume.map_or_else(|| "--".into(), |value| value.to_string());
        self.text(conn, &format!("VOLUME {volume}"), 70, 260, 5, MUTED)?;

        self.metric_card(
            conn,
            (55, 390),
            "CPU",
            &usage_text(system.cpu_usage),
            ACCENT,
        )?;
        self.metric_card(
            conn,
            (555, 390),
            "GPU",
            &usage_text(system.gpu_usage),
            ACCENT_2,
        )?;
        self.metric_card(conn, (55, 575), "SPACE USED", &system.storage_used, FG)?;
        self.metric_card(conn, (555, 575), "SPACE FREE", &system.storage_free, FG)?;

        self.card_line(conn, 780, "DEVICE", "SAMSUNG GALAXY A26")?;
        self.card_line(conn, 945, "KERNEL", &system.kernel)?;
        self.card_line(conn, 1110, "ROOTFS", &format!("ALPINE {}", system.alpine))?;
        self.card_line(conn, 1275, "MEMORY", &system.memory)?;
        self.card_line(
            conn,
            1440,
            "DISPLAY",
            &format!("{}X{} 120 HZ", self.width, self.height),
        )?;
        self.card_line(conn, 1605, "WINDOW MANAGER", "A26 SHELL RUST")?;
        self.card_line(conn, 1770, "CONTROL", "ADB UNIX SOCKET")?;
        Ok(())
    }

    fn render_wifi<C: Connection>(&self, conn: &C, app: &AppModel) -> Result<(), Box<dyn Error>> {
        self.navigation(conn, View::Wifi)?;

        self.panel(conn, UiRect::new(55, 235, 970, 225), ACCENT)?;
        self.text(conn, "CONNECTION", 85, 270, 3, MUTED)?;
        let (headline, detail, can_disconnect) = wifi_status_text(app);
        self.text(conn, &headline, 85, 325, 6, FG)?;
        self.text(conn, &detail, 85, 395, 3, MUTED)?;
        if can_disconnect {
            self.button(conn, STATUS_ACTION, "DISCONNECT", false, !app.busy)?;
        }

        self.text(conn, "AVAILABLE NETWORKS", 55, 525, 5, FG)?;
        self.button(
            conn,
            REFRESH_BUTTON,
            if app.scanning { "SCANNING" } else { "REFRESH" },
            false,
            !app.scanning && !app.busy,
        )?;
        if let Some(notice) = app.notice {
            self.text(conn, notice, 55, 610, 3, notice_color(notice))?;
        } else if app.scanning {
            self.text(conn, "LOOKING FOR NEARBY WI-FI", 55, 610, 3, MUTED)?;
        } else {
            self.text(conn, "TAP A NETWORK TO CONNECT", 55, 610, 3, MUTED)?;
        }

        if app.networks.is_empty() {
            self.panel(conn, UiRect::new(55, NETWORK_Y, 970, 210), ACCENT_2)?;
            self.center_text(conn, "NO SCAN RESULTS", NETWORK_Y + 62, 5, MUTED)?;
        } else {
            for (row, index) in app.page_range().enumerate() {
                self.network_row(conn, row, &app.networks[index], app.busy)?;
            }
        }

        if app.page_count() > 1 {
            self.button(conn, PREVIOUS_BUTTON, "PREV", false, app.page > 0)?;
            self.button(
                conn,
                NEXT_BUTTON,
                "NEXT",
                false,
                app.page + 1 < app.page_count(),
            )?;
            self.center_text(
                conn,
                &format!("PAGE {} OF {}", app.page + 1, app.page_count()),
                1905,
                4,
                MUTED,
            )?;
        }
        Ok(())
    }

    fn navigation<C: Connection>(&self, conn: &C, active: View) -> Result<(), Box<dyn Error>> {
        let system_color = if active == View::System {
            ACCENT
        } else {
            MUTED
        };
        let wifi_color = if active == View::Wifi { ACCENT } else { MUTED };
        self.text(conn, "SYSTEM", 70, 100, 6, system_color)?;
        self.text(conn, "WI-FI", 345, 100, 6, wifi_color)?;

        let active_tab = if active == View::System {
            SYSTEM_TAB
        } else {
            WIFI_TAB
        };
        self.fill(
            conn,
            ACCENT,
            UiRect::new(active_tab.x + 15, 171, active_tab.width - 30, 4).x11(),
        )?;
        self.fill(
            conn,
            BG_CARD,
            UiRect::new(55, 200, self.width.saturating_sub(110), 1).x11(),
        )
    }

    fn render_password<C: Connection>(
        &self,
        conn: &C,
        dialog: &PasswordDialog,
    ) -> Result<(), Box<dyn Error>> {
        self.text(conn, "JOIN WI-FI", 55, 95, 8, ACCENT)?;
        self.button(conn, CANCEL_BUTTON, "CANCEL", false, true)?;

        self.panel(conn, UiRect::new(55, 235, 970, 185), ACCENT_2)?;
        self.text(conn, "NETWORK", 85, 270, 3, MUTED)?;
        self.text(conn, &safe_text(&dialog.ssid, 26), 85, 325, 6, FG)?;
        self.text(conn, dialog.security.label(), 85, 380, 3, MUTED)?;

        self.panel(conn, UiRect::new(55, 460, 970, 185), ACCENT)?;
        self.text(conn, "PASSWORD", 85, 495, 3, MUTED)?;
        let password_line = if dialog.password_len() == 0 {
            "TAP KEYS TO ENTER".to_owned()
        } else {
            let visible = dialog.password_len().min(20);
            let prefix = if dialog.password_len() > visible {
                "..."
            } else {
                ""
            };
            format!(
                "{prefix}{}  {}/64",
                "*".repeat(visible),
                dialog.password_len()
            )
        };
        self.text(
            conn,
            &password_line,
            85,
            550,
            5,
            if dialog.password_len() == 0 {
                MUTED
            } else {
                FG
            },
        )?;
        let (hint, hint_color) = if dialog.submit_attempted {
            ("USE 8-63 CHARACTERS OR A 64-DIGIT HEX KEY", DANGER)
        } else if dialog.password_is_valid() {
            ("PASSWORD LENGTH IS VALID", ACCENT)
        } else {
            ("PASSWORD IS NEVER SAVED BY THIS APP", MUTED)
        };
        self.text(conn, hint, 55, 690, 3, hint_color)?;

        for key in keyboard_keys(dialog.keyboard_mode) {
            self.button(conn, key.bounds, &key.label, false, true)?;
        }
        self.button(
            conn,
            CONNECT_BUTTON,
            "CONNECT",
            true,
            dialog.password_is_valid(),
        )?;
        self.center_text(
            conn,
            "CREDENTIAL IS SENT ONLY TO THE LOCAL WI-FI SERVICE",
            1715,
            3,
            MUTED,
        )?;
        Ok(())
    }

    fn network_row<C: Connection>(
        &self,
        conn: &C,
        row: usize,
        network: &WifiNetwork,
        busy: bool,
    ) -> Result<(), Box<dyn Error>> {
        let bounds = network_bounds(row);
        self.panel(
            conn,
            bounds,
            if network.connected { ACCENT } else { ACCENT_2 },
        )?;
        self.text(
            conn,
            &safe_text(&network.ssid, 24),
            85,
            bounds.y + 35,
            5,
            FG,
        )?;
        let access = if network.connected {
            "CONNECTED"
        } else if network.security == crate::wifi::Security::Enterprise {
            "UNSUPPORTED"
        } else if network.requires_password {
            "PASSWORD"
        } else {
            "TAP TO JOIN"
        };
        self.text(
            conn,
            &format!(
                "{}  /  {}%  /  {access}",
                network.security.label(),
                network.signal
            ),
            85,
            bounds.y + 120,
            3,
            if busy { BG_DISABLED } else { MUTED },
        )?;
        self.signal_bars(conn, 880, bounds.y + 55, network.signal)?;
        Ok(())
    }

    fn signal_bars<C: Connection>(
        &self,
        conn: &C,
        x: i16,
        y: i16,
        signal: u8,
    ) -> Result<(), Box<dyn Error>> {
        for index in 0..4_u8 {
            let height = u16::from(index + 1) * 14;
            let active = signal > index * 25;
            self.fill(
                conn,
                if active { ACCENT } else { BG_DISABLED },
                Rectangle {
                    x: x + i16::from(index) * 28,
                    y: y + 56 - i16::try_from(height).unwrap_or(0),
                    width: 18,
                    height,
                },
            )?;
        }
        Ok(())
    }

    fn metric_card<C: Connection>(
        &self,
        conn: &C,
        position: (i16, i16),
        noun: &str,
        value: &str,
        color: u32,
    ) -> Result<(), Box<dyn Error>> {
        let (x, y) = position;
        self.fill(conn, BG_CARD, UiRect::new(x, y, 470, 145).x11())?;
        self.text(conn, noun, x + 28, y + 24, 3, MUTED)?;
        let clipped: String = value.chars().take(12).collect();
        self.text(conn, &clipped, x + 28, y + 73, 6, color)?;
        Ok(())
    }

    fn card_line<C: Connection>(
        &self,
        conn: &C,
        y: i16,
        noun: &str,
        value: &str,
    ) -> Result<(), Box<dyn Error>> {
        self.fill(
            conn,
            BG_CARD,
            UiRect::new(55, y, self.width.saturating_sub(110), 145).x11(),
        )?;
        self.text(conn, noun, 85, y + 25, 4, MUTED)?;
        self.text(conn, &safe_text(value, 28), 85, y + 78, 4, FG)?;
        Ok(())
    }

    fn panel<C: Connection>(
        &self,
        conn: &C,
        bounds: UiRect,
        stripe: u32,
    ) -> Result<(), Box<dyn Error>> {
        self.fill(conn, BG_CARD, bounds.x11())?;
        self.fill(
            conn,
            stripe,
            UiRect::new(bounds.x, bounds.y, 9, bounds.height).x11(),
        )
    }

    fn button<C: Connection>(
        &self,
        conn: &C,
        bounds: UiRect,
        label: &str,
        primary: bool,
        enabled: bool,
    ) -> Result<(), Box<dyn Error>> {
        let background = if !enabled {
            BG_DISABLED
        } else if primary {
            ACCENT
        } else {
            BG_KEY
        };
        let foreground = if primary && enabled {
            BG
        } else if enabled {
            FG
        } else {
            MUTED
        };
        self.fill(conn, background, bounds.x11())?;
        if enabled && !primary {
            self.fill(
                conn,
                ACCENT_2,
                UiRect::new(bounds.x, bounds.y, bounds.width, 5).x11(),
            )?;
        }
        let scale = button_scale(label, bounds.width);
        let text_width = font::text_width(label, scale);
        let x = bounds.x + (bounds.width.saturating_sub(text_width) / 2) as i16;
        let glyph_height = 7_u16.saturating_mul(scale);
        let y = bounds.y + (bounds.height.saturating_sub(glyph_height) / 2) as i16;
        self.text(conn, label, x, y, scale, foreground)
    }

    fn center_text<C: Connection>(
        &self,
        conn: &C,
        text: &str,
        y: i16,
        scale: u16,
        color: u32,
    ) -> Result<(), Box<dyn Error>> {
        let width = font::text_width(text, scale);
        let x = self.width.saturating_sub(width) / 2;
        self.text(conn, text, x as i16, y, scale, color)
    }

    fn text<C: Connection>(
        &self,
        conn: &C,
        text: &str,
        x: i16,
        y: i16,
        scale: u16,
        color: u32,
    ) -> Result<(), Box<dyn Error>> {
        let rectangles = font::rectangles(text, x, y, scale);
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(color))?;
        if !rectangles.is_empty() {
            conn.poly_fill_rectangle(self.window, self.gc, &rectangles)?;
        }
        Ok(())
    }

    fn fill<C: Connection>(
        &self,
        conn: &C,
        color: u32,
        rectangle: Rectangle,
    ) -> Result<(), Box<dyn Error>> {
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(color))?;
        conn.poly_fill_rectangle(self.window, self.gc, &[rectangle])?;
        Ok(())
    }
}

fn network_bounds(row: usize) -> UiRect {
    UiRect::new(
        NETWORK_X,
        NETWORK_Y + i16::try_from(row).unwrap_or(0) * NETWORK_STEP,
        NETWORK_WIDTH,
        NETWORK_HEIGHT,
    )
}

fn wifi_status_text(app: &AppModel) -> (String, String, bool) {
    let Some(status) = &app.wifi_status else {
        return (
            "SERVICE UNAVAILABLE".into(),
            "CHECK THE LOCAL WI-FI BACKEND".into(),
            false,
        );
    };
    if !status.radio_enabled {
        return ("WI-FI OFF".into(), "ENABLE IT IN THE BACKEND".into(), false);
    }
    match &status.state {
        LinkState::Disconnected => (
            "NOT CONNECTED".into(),
            "CHOOSE A NETWORK BELOW".into(),
            false,
        ),
        LinkState::Connecting { ssid } => (
            format!("JOINING {}", safe_text(ssid, 15)),
            "AUTHENTICATING".into(),
            false,
        ),
        LinkState::Connected {
            ssid,
            signal,
            ip_address,
        } => {
            let mut details =
                signal.map_or_else(|| "CONNECTED".into(), |value| format!("{value}% SIGNAL"));
            if let Some(ip) = ip_address {
                details.push_str("  /  ");
                details.push_str(&safe_text(ip, 20));
            }
            (safe_text(ssid, 18), details, true)
        }
        LinkState::Disconnecting => ("DISCONNECTING".into(), "PLEASE WAIT".into(), false),
    }
}

fn notice_color(notice: &str) -> u32 {
    if matches!(notice, "CONNECTED" | "DISCONNECTED") {
        ACCENT
    } else if notice.contains("PASSWORD") || notice.contains("FAILED") {
        DANGER
    } else {
        WARNING
    }
}

fn safe_text(text: &str, max_characters: usize) -> String {
    text.chars()
        .take(max_characters)
        .map(|character| {
            if character == ' ' || character.is_ascii_graphic() {
                character
            } else {
                '?'
            }
        })
        .collect()
}

fn button_scale(label: &str, width: u16) -> u16 {
    for scale in (2..=5).rev() {
        if font::text_width(label, scale) <= width.saturating_sub(24) {
            return scale;
        }
    }
    2
}

fn keyboard_keys(mode: KeyboardMode) -> Vec<KeySpec> {
    let mut keys = Vec::new();
    match mode {
        KeyboardMode::Lower | KeyboardMode::Upper => {
            character_row(&mut keys, "qwertyuiop", 55, 790, 970, mode);
            character_row(&mut keys, "asdfghjkl", 105, 945, 870, mode);
            keys.push(KeySpec {
                bounds: UiRect::new(55, 1100, 140, 135),
                label: "SHIFT".into(),
                action: KeyAction::Shift,
            });
            character_row(&mut keys, "zxcvbnm", 205, 1100, 620, mode);
            keys.push(KeySpec {
                bounds: UiRect::new(835, 1100, 190, 135),
                label: "BACK".into(),
                action: KeyAction::Backspace,
            });
            bottom_keys(
                &mut keys,
                "123",
                KeyAction::Symbols,
                "CLEAR",
                KeyAction::Clear,
            );
        }
        KeyboardMode::Symbols => {
            character_row(&mut keys, "1234567890", 55, 790, 970, mode);
            character_row(&mut keys, "!@#$%^&*()", 55, 945, 970, mode);
            character_row(&mut keys, "-_+=[]{}:;", 55, 1100, 970, mode);
            symbol_bottom_keys(&mut keys, "MORE", KeyAction::MoreSymbols);
        }
        KeyboardMode::MoreSymbols => {
            character_row(&mut keys, "`~|\\/\"'<>?", 55, 790, 970, mode);
            character_row(&mut keys, ".,:;!@#$%&", 55, 945, 970, mode);
            character_row(&mut keys, "^*()_+-=[]", 55, 1100, 970, mode);
            symbol_bottom_keys(&mut keys, "123", KeyAction::Symbols);
        }
    }
    keys
}

fn character_row(
    keys: &mut Vec<KeySpec>,
    characters: &str,
    x: i16,
    y: i16,
    width: u16,
    mode: KeyboardMode,
) {
    let count = characters.chars().count() as u16;
    let gap = 10_u16;
    let key_width = width.saturating_sub(gap.saturating_mul(count.saturating_sub(1))) / count;
    for (index, character) in characters.chars().enumerate() {
        let offset = u16::try_from(index)
            .unwrap_or(0)
            .saturating_mul(key_width.saturating_add(gap));
        let label = if mode == KeyboardMode::Upper {
            character.to_ascii_uppercase()
        } else {
            character
        };
        keys.push(KeySpec {
            bounds: UiRect::new(x + offset as i16, y, key_width, 135),
            label: label.to_string(),
            action: KeyAction::Character(character),
        });
    }
}

fn bottom_keys(
    keys: &mut Vec<KeySpec>,
    left_label: &str,
    left_action: KeyAction,
    right_label: &str,
    right_action: KeyAction,
) {
    keys.extend([
        KeySpec {
            bounds: UiRect::new(55, 1255, 180, 135),
            label: left_label.into(),
            action: left_action,
        },
        KeySpec {
            bounds: UiRect::new(245, 1255, 570, 135),
            label: "SPACE".into(),
            action: KeyAction::Space,
        },
        KeySpec {
            bounds: UiRect::new(825, 1255, 200, 135),
            label: right_label.into(),
            action: right_action,
        },
    ]);
}

fn symbol_bottom_keys(keys: &mut Vec<KeySpec>, right_label: &str, right_action: KeyAction) {
    keys.extend([
        KeySpec {
            bounds: UiRect::new(55, 1255, 170, 135),
            label: "ABC".into(),
            action: KeyAction::Letters,
        },
        KeySpec {
            bounds: UiRect::new(235, 1255, 420, 135),
            label: "SPACE".into(),
            action: KeyAction::Space,
        },
        KeySpec {
            bounds: UiRect::new(665, 1255, 170, 135),
            label: "BACK".into(),
            action: KeyAction::Backspace,
        },
        KeySpec {
            bounds: UiRect::new(845, 1255, 180, 135),
            label: right_label.into(),
            action: right_action,
        },
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::{Security, WifiNetwork};

    #[test]
    fn system_and_wifi_hit_targets_are_large_and_deterministic() {
        let renderer = Renderer::new(1, 2, 1080, 2340);
        let mut app = AppModel::default();
        assert_eq!(renderer.action_at(&app, 430, 120), Some(Action::ShowWifi));
        assert_eq!(renderer.action_at(&app, 170, 120), None);
        assert_eq!(renderer.action_at(&app, 540, 2290), None);

        app.view = View::Wifi;
        app.networks.push(WifiNetwork {
            ssid: "test".into(),
            signal: 70,
            security: Security::Open,
            requires_password: false,
            connected: false,
        });
        assert_eq!(
            renderer.action_at(&app, 500, NETWORK_Y + 100),
            Some(Action::SelectNetwork(0))
        );
        assert_eq!(renderer.action_at(&app, 170, 120), Some(Action::ShowSystem));
        assert_eq!(renderer.action_at(&app, 430, 120), None);
    }

    #[test]
    fn password_keyboard_mapping_matches_rendered_keys() {
        let renderer = Renderer::new(1, 2, 1080, 2340);
        let mut app = AppModel::default();
        app.networks.push(WifiNetwork {
            ssid: "secure".into(),
            signal: 70,
            security: Security::Wpa2,
            requires_password: true,
            connected: false,
        });
        app.handle(Action::SelectNetwork(0));
        assert_eq!(
            renderer.action_at(&app, 90, 850),
            Some(Action::Key(KeyAction::Character('q')))
        );
        assert_eq!(
            renderer.action_at(&app, 500, 1570),
            Some(Action::SubmitPassword)
        );
        assert_eq!(renderer.action_at(&app, 500, 2250), None);
    }

    #[test]
    fn pages_never_render_more_than_five_network_rows() {
        assert_eq!(crate::app::NETWORKS_PER_PAGE, 5);
        assert!(
            network_bounds(crate::app::NETWORKS_PER_PAGE - 1).y + NETWORK_HEIGHT as i16 <= 1830
        );
    }
}
