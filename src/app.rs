use std::cmp::Ordering;
use std::time::{Duration, Instant};

use crate::wifi::{
    LinkState, Rejection, SecretString, Security, WifiError, WifiEvent, WifiNetwork, WifiRequest,
    WifiStatus, valid_domain,
};

pub const NETWORKS_PER_PAGE: usize = 7;
/// The backend answers a scan with its cached results and starts a fresh
/// scan, which takes the A26's firmware about 5.1 s to finish (measured);
/// a requested scan asks again after this long so one press shows the new
/// networks.
const SCAN_FOLLOW_UP: Duration = Duration::from_millis(6000);
/// How long a notice stays up when nothing else replaces it.
const NOTICE_DURATION: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    System,
    Wifi,
    /// Alarms and timers (clock.rs).
    Clock,
    /// Display & Brightness, its Auto-Lock choices, and the wallpaper
    /// picker (display_ui.rs).
    Display,
    AutoLock,
    Wallpaper,
    /// Activity, storage and the device.
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardMode {
    Lower,
    Upper,
    Symbols,
    MoreSymbols,
}

/// A text field on the join page. WPA Personal pages have only `Password`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Identity,
    Password,
    Domain,
}

const ENTERPRISE_FIELDS: [Field; 3] = [Field::Identity, Field::Password, Field::Domain];
const MAX_DOMAIN_BYTES: usize = 253;

/// The extra fields a WPA2-Enterprise (PEAP/MSCHAPv2) join needs. The
/// identity is kept in a zeroizing, debug-redacted buffer like the password;
/// the domain names the RADIUS server whose certificate must match.
pub struct EnterpriseFields {
    identity: SecretString,
    pub domain: String,
}

pub struct PasswordDialog {
    pub ssid: String,
    pub security: Security,
    password: SecretString,
    pub enterprise: Option<EnterpriseFields>,
    pub focus: Field,
    pub keyboard_mode: KeyboardMode,
    pub submit_attempted: bool,
    /// Moon's system keyboard is up; the in-app keyboard is only a fallback.
    pub system_keyboard: bool,
}

impl PasswordDialog {
    fn new(ssid: String, security: Security) -> Self {
        let enterprise = (security == Security::Enterprise).then(|| EnterpriseFields {
            identity: SecretString::default(),
            domain: String::new(),
        });
        Self {
            ssid,
            security,
            password: SecretString::default(),
            focus: if enterprise.is_some() {
                Field::Identity
            } else {
                Field::Password
            },
            enterprise,
            keyboard_mode: KeyboardMode::Lower,
            submit_attempted: false,
            system_keyboard: false,
        }
    }

    pub fn password_len(&self) -> usize {
        self.password.len()
    }

    /// The identity being typed, for display only (never logged).
    pub fn identity_text(&self) -> &str {
        self.enterprise
            .as_ref()
            .map_or("", |fields| fields.identity.reveal_for_display())
    }

    pub fn field_is_valid(&self, field: Field) -> bool {
        match (field, &self.enterprise) {
            (Field::Password, None) => self.password.is_valid_wpa_psk(),
            (Field::Password, Some(_)) => self.password.is_valid_enterprise_field(),
            (Field::Identity, Some(fields)) => fields.identity.is_valid_enterprise_field(),
            (Field::Domain, Some(fields)) => valid_domain(&fields.domain),
            (_, None) => true,
        }
    }

    pub fn password_is_valid(&self) -> bool {
        self.fields()
            .iter()
            .all(|field| self.field_is_valid(*field))
    }

    pub fn fields(&self) -> &'static [Field] {
        if self.enterprise.is_some() {
            &ENTERPRISE_FIELDS
        } else {
            &[Field::Password]
        }
    }

    /// The first invalid field after the focused one (wrapping), or `None`
    /// when every field is complete.
    fn next_incomplete_field(&self) -> Option<Field> {
        let fields = self.fields();
        let start = fields.iter().position(|f| *f == self.focus).unwrap_or(0);
        (1..=fields.len())
            .map(|offset| fields[(start + offset) % fields.len()])
            .find(|field| !self.field_is_valid(*field))
    }

    fn push(&mut self, character: char) {
        match (self.focus, self.enterprise.as_mut()) {
            (Field::Identity, Some(fields)) => {
                fields.identity.push(character);
            }
            (Field::Domain, Some(fields)) => {
                // DNS names are case-insensitive; the backend wants lowercase.
                let character = character.to_ascii_lowercase();
                if (character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
                    && fields.domain.len() < MAX_DOMAIN_BYTES
                {
                    fields.domain.push(character);
                }
            }
            _ => {
                self.password.push(character);
            }
        }
    }

    fn pop(&mut self) {
        match (self.focus, self.enterprise.as_mut()) {
            (Field::Identity, Some(fields)) => fields.identity.pop(),
            (Field::Domain, Some(fields)) => {
                fields.domain.pop();
            }
            _ => self.password.pop(),
        }
    }

    fn clear_focused(&mut self) {
        match (self.focus, self.enterprise.as_mut()) {
            (Field::Identity, Some(fields)) => fields.identity.clear(),
            (Field::Domain, Some(fields)) => fields.domain.clear(),
            _ => self.password.clear(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Character(char),
    Backspace,
    Clear,
    Shift,
    Symbols,
    MoreSymbols,
    Letters,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ShowSystem,
    ShowWifi,
    ShowClock,
    ShowDisplay,
    ShowAutoLock,
    ShowWallpaper,
    ShowAbout,
    /// Moon settings (sent over its control socket by the main loop).
    SetAutoLock(u32),
    SetWallpaper(usize),
    ToggleAutoBrightness,
    SetBrightness(u8),
    ToggleHaptics,
    /// On the Clock page (handled by `ClockModel`).
    Clock(crate::clock::ClockAction),
    RefreshWifi,
    SelectNetwork(usize),
    PreviousNetworks,
    NextNetworks,
    Disconnect,
    CancelPassword,
    SubmitPassword,
    /// Return on the keyboard: move to the next incomplete field, or join.
    Return,
    /// Tap on a field: focus it and bring Moon's keyboard back if dismissed.
    Focus(Field),
    Key(KeyAction),
}

pub struct AppModel {
    pub view: View,
    pub wifi_status: Option<WifiStatus>,
    pub networks: Vec<WifiNetwork>,
    pub page: usize,
    pub scanning: bool,
    pub busy: bool,
    pub notice: Option<&'static str>,
    pub password_dialog: Option<PasswordDialog>,
    pub clock: crate::clock::ClockModel,
    /// Moon's display state and settings, refreshed every second.
    pub shell: crate::control::ShellInfo,
    wifi_initialized: bool,
    /// When the shown notice goes away (`None`: it stays).
    notice_until: Option<Instant>,
    /// The backend keeps reporting a failed join for a while; it is shown
    /// once per attempt.
    failure_acknowledged: bool,
    /// The scan in flight answers from the cache; ask again once it lands.
    scan_follow_up: bool,
    scan_follow_up_at: Option<Instant>,
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            view: View::System,
            wifi_status: None,
            networks: Vec::new(),
            page: 0,
            scanning: false,
            busy: false,
            notice: None,
            password_dialog: None,
            clock: crate::clock::ClockModel::default(),
            shell: crate::control::ShellInfo::default(),
            wifi_initialized: false,
            notice_until: None,
            failure_acknowledged: false,
            scan_follow_up: false,
            scan_follow_up_at: None,
        }
    }
}

impl AppModel {
    pub fn handle(&mut self, action: Action) -> Vec<WifiRequest> {
        // Any tap or key dismisses the last notice.
        self.notice = None;
        match action {
            Action::ShowSystem => {
                self.password_dialog = None;
                self.clock.alarm_editor = None;
                self.clock.timer_editor = None;
                self.view = View::System;
                Vec::new()
            }
            Action::ShowClock => {
                self.view = View::Clock;
                Vec::new()
            }
            Action::ShowDisplay => {
                self.view = View::Display;
                Vec::new()
            }
            Action::ShowAutoLock => {
                self.view = View::AutoLock;
                Vec::new()
            }
            Action::ShowWallpaper => {
                self.view = View::Wallpaper;
                Vec::new()
            }
            Action::ShowAbout => {
                self.view = View::About;
                Vec::new()
            }
            // The main loop sends these to Moon.
            Action::SetAutoLock(_)
            | Action::SetWallpaper(_)
            | Action::ToggleAutoBrightness
            | Action::ToggleHaptics
            | Action::SetBrightness(_) => Vec::new(),
            // The main loop sends these to moon-notifyd.
            Action::Clock(_) => Vec::new(),
            Action::ShowWifi => {
                self.view = View::Wifi;
                if self.wifi_initialized {
                    vec![WifiRequest::Status]
                } else {
                    self.wifi_initialized = true;
                    self.start_scan()
                }
            }
            Action::RefreshWifi
                if !self.busy && !self.scanning && self.password_dialog.is_none() =>
            {
                self.start_scan()
            }
            Action::SelectNetwork(index) if !self.busy && self.password_dialog.is_none() => {
                self.select_network(index)
            }
            Action::PreviousNetworks => {
                self.page = self.page.saturating_sub(1);
                Vec::new()
            }
            Action::NextNetworks => {
                if self.page + 1 < self.page_count() {
                    self.page += 1;
                }
                Vec::new()
            }
            Action::Disconnect if !self.busy && self.password_dialog.is_none() => {
                self.busy = true;
                vec![WifiRequest::Disconnect]
            }
            Action::CancelPassword => {
                self.password_dialog = None;
                Vec::new()
            }
            Action::SubmitPassword => self.submit_password(),
            Action::Return => match self.password_dialog.as_mut() {
                Some(dialog) => match dialog.next_incomplete_field() {
                    None => self.submit_password(),
                    Some(field) => {
                        if field == dialog.focus {
                            dialog.submit_attempted = true;
                        }
                        dialog.focus = field;
                        Vec::new()
                    }
                },
                None => Vec::new(),
            },
            Action::Focus(field) => {
                if let Some(dialog) = self.password_dialog.as_mut()
                    && dialog.fields().contains(&field)
                {
                    dialog.focus = field;
                }
                Vec::new()
            }
            Action::Key(key) => {
                self.handle_key(key);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub fn apply_wifi_event(&mut self, event: WifiEvent) {
        match event {
            WifiEvent::Status(result) => match result {
                Ok(status) => {
                    // A join that was accepted but then failed (credentials
                    // or server certificate rejected, or no association)
                    // only shows up here.
                    match &status.failure {
                        Some(failure) if !self.failure_acknowledged => {
                            self.failure_acknowledged = true;
                            self.show_notice(error_notice(WifiError::Rejected(failure.rejection)));
                        }
                        Some(_) => {}
                        None => self.failure_acknowledged = false,
                    }
                    self.set_status(status);
                }
                Err(error) => self.service_error(error),
            },
            WifiEvent::Scan(result) => match result {
                Ok(networks) => {
                    self.networks = normalized_networks(networks);
                    self.clamp_page();
                    self.reconcile_connected_network();
                    if std::mem::take(&mut self.scan_follow_up) {
                        self.scan_follow_up_at = Some(Instant::now() + SCAN_FOLLOW_UP);
                    } else {
                        self.scanning = false;
                    }
                }
                Err(error) => {
                    self.stop_scan();
                    self.show_notice(error_notice(error));
                }
            },
            WifiEvent::Connect(result) => {
                self.busy = false;
                match result {
                    Ok(status) => {
                        let connected = matches!(status.state, LinkState::Connected { .. });
                        self.set_status(status);
                        self.reconcile_connected_network();
                        self.show_notice(if connected {
                            "Connected"
                        } else {
                            "Connecting…"
                        });
                    }
                    Err(error) => {
                        if let Some(status) = &mut self.wifi_status {
                            status.state = LinkState::Disconnected;
                        }
                        self.reconcile_connected_network();
                        self.show_notice(error_notice(error));
                    }
                }
            }
            WifiEvent::Disconnect(result) => {
                self.busy = false;
                match result {
                    Ok(status) => {
                        self.set_status(status);
                        self.reconcile_connected_network();
                        self.show_notice("Disconnected");
                    }
                    Err(error) => self.show_notice(error_notice(error)),
                }
            }
        }
    }

    pub fn worker_unavailable(&mut self) {
        self.busy = false;
        self.stop_scan();
        self.wifi_status = None;
        self.password_dialog = None;
        // The worker does not come back; this one stays.
        self.notice = Some("Wi-Fi service unavailable");
        self.notice_until = None;
    }

    /// The next time `tick` has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        let notice = self.notice.and(self.notice_until);
        match (notice, self.scan_follow_up_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Expires the notice and sends a due follow-up scan.
    pub fn tick(&mut self, now: Instant) -> Vec<WifiRequest> {
        if self.notice_until.is_some_and(|until| now >= until) {
            self.notice = None;
            self.notice_until = None;
        }
        if self.scan_follow_up_at.is_some_and(|at| now >= at) {
            self.scan_follow_up_at = None;
            return vec![WifiRequest::Scan];
        }
        Vec::new()
    }

    pub fn page_count(&self) -> usize {
        self.networks.len().max(1).div_ceil(NETWORKS_PER_PAGE)
    }

    pub fn page_range(&self) -> std::ops::Range<usize> {
        let start = self.page.saturating_mul(NETWORKS_PER_PAGE);
        let end = (start + NETWORKS_PER_PAGE).min(self.networks.len());
        start.min(end)..end
    }

    pub fn may_poll_status(&self) -> bool {
        self.view == View::Wifi && !self.busy && !self.scanning && self.password_dialog.is_none()
    }

    fn select_network(&mut self, index: usize) -> Vec<WifiRequest> {
        let Some(network) = self.networks.get(index) else {
            return Vec::new();
        };
        if network.connected {
            return Vec::new();
        }
        if !network.joinable() {
            self.show_notice("This network’s security is not supported");
            return Vec::new();
        }

        let ssid = network.ssid.clone();
        let security = network.security;
        if network.requires_password {
            self.password_dialog = Some(PasswordDialog::new(ssid, security));
            Vec::new()
        } else {
            self.begin_connect(ssid, None)
        }
    }

    fn submit_password(&mut self) -> Vec<WifiRequest> {
        let Some(mut dialog) = self.password_dialog.take() else {
            return Vec::new();
        };
        if !dialog.password_is_valid() {
            dialog.submit_attempted = true;
            if let Some(field) = dialog
                .fields()
                .iter()
                .copied()
                .find(|field| !dialog.field_is_valid(*field))
            {
                dialog.focus = field;
            }
            self.password_dialog = Some(dialog);
            return Vec::new();
        }
        let PasswordDialog {
            ssid,
            password,
            enterprise,
            ..
        } = dialog;
        let request = match enterprise {
            Some(EnterpriseFields { identity, domain }) => WifiRequest::ConnectEnterprise {
                ssid: ssid.clone(),
                identity,
                password,
                domain,
            },
            None => WifiRequest::Connect {
                ssid: ssid.clone(),
                password: Some(password),
            },
        };
        self.begin_request(ssid, request)
    }

    fn begin_connect(&mut self, ssid: String, password: Option<SecretString>) -> Vec<WifiRequest> {
        let request = WifiRequest::Connect {
            ssid: ssid.clone(),
            password,
        };
        self.begin_request(ssid, request)
    }

    fn begin_request(&mut self, ssid: String, request: WifiRequest) -> Vec<WifiRequest> {
        self.busy = true;
        self.failure_acknowledged = false;
        // No scan while associating; the list is refreshed afterwards.
        self.stop_scan();
        self.wifi_status = Some(WifiStatus {
            radio_enabled: true,
            state: LinkState::Connecting { ssid },
            failure: None,
        });
        vec![request]
    }

    fn handle_key(&mut self, key: KeyAction) {
        let Some(dialog) = &mut self.password_dialog else {
            return;
        };
        dialog.submit_attempted = false;
        match key {
            KeyAction::Character(mut character) => {
                if dialog.keyboard_mode == KeyboardMode::Upper {
                    character = character.to_ascii_uppercase();
                }
                dialog.push(character);
                if dialog.keyboard_mode == KeyboardMode::Upper {
                    dialog.keyboard_mode = KeyboardMode::Lower;
                }
            }
            KeyAction::Space => dialog.push(' '),
            KeyAction::Backspace => dialog.pop(),
            KeyAction::Clear => dialog.clear_focused(),
            KeyAction::Shift => {
                dialog.keyboard_mode = match dialog.keyboard_mode {
                    KeyboardMode::Upper => KeyboardMode::Lower,
                    _ => KeyboardMode::Upper,
                };
            }
            KeyAction::Symbols => dialog.keyboard_mode = KeyboardMode::Symbols,
            KeyAction::MoreSymbols => dialog.keyboard_mode = KeyboardMode::MoreSymbols,
            KeyAction::Letters => dialog.keyboard_mode = KeyboardMode::Lower,
        }
    }

    fn start_scan(&mut self) -> Vec<WifiRequest> {
        self.scanning = true;
        self.scan_follow_up = true;
        vec![WifiRequest::Status, WifiRequest::Scan]
    }

    fn stop_scan(&mut self) {
        self.scanning = false;
        self.scan_follow_up = false;
        self.scan_follow_up_at = None;
    }

    fn show_notice(&mut self, notice: &'static str) {
        self.notice = Some(notice);
        self.notice_until = Some(Instant::now() + NOTICE_DURATION);
    }

    fn set_status(&mut self, status: WifiStatus) {
        self.wifi_status = Some(status);
        self.reconcile_connected_network();
    }

    fn service_error(&mut self, error: WifiError) {
        self.wifi_status = None;
        self.show_notice(error_notice(error));
    }

    fn reconcile_connected_network(&mut self) {
        let Some(status) = self.wifi_status.as_ref() else {
            self.networks = normalized_networks(std::mem::take(&mut self.networks));
            self.clamp_page();
            return;
        };
        let connected_ssid = if let LinkState::Connected { ssid, .. } = &status.state {
            Some(ssid.as_str())
        } else {
            None
        };
        for network in &mut self.networks {
            network.connected = connected_ssid == Some(network.ssid.as_str());
        }
        self.networks = normalized_networks(std::mem::take(&mut self.networks));
        self.clamp_page();
    }

    fn clamp_page(&mut self) {
        self.page = self.page.min(self.page_count().saturating_sub(1));
    }
}

fn normalized_networks(mut networks: Vec<WifiNetwork>) -> Vec<WifiNetwork> {
    // Keep only the strongest observation for duplicate SSIDs. The backend is
    // expected to merge BSSIDs, but this also makes UI behavior deterministic.
    networks.sort_by(|left, right| {
        left.ssid
            .cmp(&right.ssid)
            .then_with(|| right.connected.cmp(&left.connected))
            .then_with(|| right.signal.cmp(&left.signal))
    });
    networks.dedup_by(|left, right| left.ssid == right.ssid);
    networks.sort_by(|left, right| match right.connected.cmp(&left.connected) {
        Ordering::Equal => right
            .signal
            .cmp(&left.signal)
            .then_with(|| left.ssid.cmp(&right.ssid)),
        ordering => ordering,
    });
    networks
}

fn error_notice(error: WifiError) -> &'static str {
    match error {
        WifiError::Unavailable => "Wi-Fi service unavailable",
        WifiError::Timeout => "Wi-Fi service timed out",
        WifiError::Protocol => "Wi-Fi service error",
        WifiError::Rejected(Rejection::AuthenticationFailed) => "Incorrect password",
        WifiError::Rejected(Rejection::NetworkNotFound) => "Network not found",
        WifiError::Rejected(Rejection::RadioDisabled) => "Wi-Fi is turned off",
        WifiError::Rejected(Rejection::Busy) => "Wi-Fi service is busy",
        WifiError::Rejected(Rejection::Unsupported) => "Network is not supported",
        WifiError::Rejected(Rejection::Other) => "Could not connect",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(ssid: &str, signal: u8, security: Security) -> WifiNetwork {
        WifiNetwork {
            ssid: ssid.into(),
            signal,
            security,
            requires_password: security != Security::Open,
            connected: false,
        }
    }

    #[test]
    fn first_wifi_open_requests_status_and_scan_once() {
        let mut model = AppModel::default();
        let requests = model.handle(Action::ShowWifi);
        assert_eq!(requests.len(), 2);
        assert!(matches!(requests[0], WifiRequest::Status));
        assert!(matches!(requests[1], WifiRequest::Scan));
        assert!(model.scanning);

        model.handle(Action::ShowSystem);
        let requests = model.handle(Action::ShowWifi);
        assert_eq!(requests.len(), 1);
        assert!(matches!(requests[0], WifiRequest::Status));
    }

    #[test]
    fn protected_network_requires_valid_zeroizing_entry_before_connect() {
        let mut model = AppModel {
            view: View::Wifi,
            networks: vec![network("A26 Lab", 90, Security::Wpa2)],
            ..AppModel::default()
        };
        assert!(model.handle(Action::SelectNetwork(0)).is_empty());
        assert_eq!(model.password_dialog.as_ref().unwrap().ssid, "A26 Lab");

        for character in "short".chars() {
            model.handle(Action::Key(KeyAction::Character(character)));
        }
        assert!(model.handle(Action::SubmitPassword).is_empty());
        assert!(model.password_dialog.as_ref().unwrap().submit_attempted);

        model.handle(Action::Key(KeyAction::Clear));
        for character in "validpass".chars() {
            model.handle(Action::Key(KeyAction::Character(character)));
        }
        let requests = model.handle(Action::SubmitPassword);
        assert!(model.password_dialog.is_none());
        assert!(model.busy);
        assert_eq!(requests.len(), 1);
        match &requests[0] {
            WifiRequest::Connect {
                ssid,
                password: Some(password),
            } => {
                assert_eq!(ssid, "A26 Lab");
                assert_eq!(password.len(), 9);
                assert!(password.is_valid_wpa_psk());
            }
            _ => panic!("expected password connect request"),
        }
    }

    #[test]
    fn unsupported_network_does_not_open_password_dialog() {
        let mut model = AppModel {
            view: View::Wifi,
            networks: vec![network("Future", 90, Security::Wpa3)],
            ..AppModel::default()
        };
        assert!(model.handle(Action::SelectNetwork(0)).is_empty());
        assert!(model.password_dialog.is_none());
        assert_eq!(
            model.notice,
            Some("This network’s security is not supported")
        );
    }

    #[test]
    fn accepted_join_that_fails_later_explains_why() {
        let mut model = AppModel {
            view: View::Wifi,
            ..AppModel::default()
        };
        let status = |code: &str| WifiStatus {
            radio_enabled: true,
            state: LinkState::Disconnected,
            failure: Some(crate::wifi::JoinFailure {
                ssid: "eduroam".into(),
                rejection: crate::wifi::rejection_from_code(Some(code)),
            }),
        };
        model.apply_wifi_event(WifiEvent::Status(Ok(status("authentication_failed"))));
        assert_eq!(model.notice, Some("Incorrect password"));
        model.apply_wifi_event(WifiEvent::Status(Ok(WifiStatus {
            radio_enabled: true,
            state: LinkState::Disconnected,
            failure: None,
        })));
        model.apply_wifi_event(WifiEvent::Status(Ok(status("server_untrusted"))));
        assert_eq!(model.notice, Some("Could not connect"));
    }

    #[test]
    fn a_failed_join_is_reported_once_and_goes_away() {
        let mut model = AppModel {
            view: View::Wifi,
            networks: vec![network("Home", 80, Security::Open)],
            ..AppModel::default()
        };
        let failed = || {
            WifiEvent::Status(Ok(WifiStatus {
                radio_enabled: true,
                state: LinkState::Disconnected,
                failure: Some(crate::wifi::JoinFailure {
                    ssid: "Home".into(),
                    rejection: Rejection::AuthenticationFailed,
                }),
            }))
        };
        model.apply_wifi_event(failed());
        assert_eq!(model.notice, Some("Incorrect password"));
        // It expires on its own, and later polls of the same failure stay quiet.
        let later = Instant::now() + NOTICE_DURATION + Duration::from_millis(1);
        assert!(model.next_deadline().is_some_and(|at| at < later));
        model.tick(later);
        assert_eq!(model.notice, None);
        model.apply_wifi_event(failed());
        assert_eq!(model.notice, None);

        // A new attempt that fails again is reported again; a tap dismisses it.
        model.handle(Action::SelectNetwork(0));
        model.apply_wifi_event(WifiEvent::Connect(Ok(WifiStatus {
            radio_enabled: true,
            state: LinkState::Connecting {
                ssid: "Home".into(),
            },
            failure: None,
        })));
        model.apply_wifi_event(failed());
        assert_eq!(model.notice, Some("Incorrect password"));
        model.handle(Action::NextNetworks);
        assert_eq!(model.notice, None);
    }

    #[test]
    fn one_scan_press_shows_the_fresh_results() {
        let mut model = AppModel {
            view: View::Wifi,
            ..AppModel::default()
        };
        let requests = model.handle(Action::RefreshWifi);
        assert!(matches!(
            requests.as_slice(),
            [WifiRequest::Status, WifiRequest::Scan]
        ));
        // A second press while scanning does nothing.
        assert!(model.handle(Action::RefreshWifi).is_empty());

        // The cached answer shows at once, and a follow-up is scheduled.
        model.apply_wifi_event(WifiEvent::Scan(Ok(vec![network(
            "Old",
            50,
            Security::Open,
        )])));
        assert_eq!(model.networks.len(), 1);
        assert!(model.scanning);
        let due = model.next_deadline().expect("follow-up scan");
        assert!(model.tick(due - Duration::from_millis(1)).is_empty());
        assert!(matches!(model.tick(due).as_slice(), [WifiRequest::Scan]));
        assert!(model.next_deadline().is_none());

        model.apply_wifi_event(WifiEvent::Scan(Ok(vec![
            network("Old", 50, Security::Open),
            network("New", 70, Security::Open),
        ])));
        assert_eq!(model.networks.len(), 2);
        assert!(!model.scanning);
        assert!(model.next_deadline().is_none());
    }

    #[test]
    fn joining_cancels_a_pending_follow_up_scan() {
        let mut model = AppModel {
            view: View::Wifi,
            ..AppModel::default()
        };
        model.handle(Action::RefreshWifi);
        model.apply_wifi_event(WifiEvent::Scan(Ok(vec![network(
            "Cafe",
            60,
            Security::Open,
        )])));
        assert!(!model.handle(Action::SelectNetwork(0)).is_empty());
        assert!(!model.scanning);
        assert!(model.next_deadline().is_none());
    }

    #[test]
    fn saved_enterprise_network_connects_without_a_password() {
        let mut saved = network("UofT", 80, Security::Enterprise);
        saved.requires_password = false;
        let mut model = AppModel {
            view: View::Wifi,
            networks: vec![saved],
            ..AppModel::default()
        };
        let requests = model.handle(Action::SelectNetwork(0));
        assert!(model.password_dialog.is_none());
        assert!(matches!(
            requests.as_slice(),
            [WifiRequest::Connect { ssid, password: None }] if ssid == "UofT"
        ));
    }

    fn type_text(model: &mut AppModel, text: &str) {
        for character in text.chars() {
            model.handle(Action::Key(KeyAction::Character(character)));
        }
    }

    #[test]
    fn new_enterprise_network_asks_for_identity_password_and_server() {
        let mut model = AppModel {
            view: View::Wifi,
            networks: vec![network("eduroam", 80, Security::Enterprise)],
            ..AppModel::default()
        };
        assert!(model.handle(Action::SelectNetwork(0)).is_empty());
        let dialog = model.password_dialog.as_ref().unwrap();
        assert!(dialog.enterprise.is_some());
        assert_eq!(dialog.focus, Field::Identity);

        // Return walks through the incomplete fields in order.
        type_text(&mut model, "student@example.edu");
        assert!(model.handle(Action::Return).is_empty());
        assert_eq!(
            model.password_dialog.as_ref().unwrap().focus,
            Field::Password
        );
        type_text(&mut model, "p@ss word");
        assert!(model.handle(Action::Return).is_empty());
        assert_eq!(model.password_dialog.as_ref().unwrap().focus, Field::Domain);

        // The domain is normalized to lowercase and filtered to DNS bytes.
        type_text(&mut model, "Radius.Example.EDU/");
        assert_eq!(
            model
                .password_dialog
                .as_ref()
                .unwrap()
                .enterprise
                .as_ref()
                .unwrap()
                .domain,
            "radius.example.edu"
        );
        let requests = model.handle(Action::Return);
        assert!(model.password_dialog.is_none());
        assert!(model.busy);
        match requests.as_slice() {
            [
                WifiRequest::ConnectEnterprise {
                    ssid,
                    identity,
                    password,
                    domain,
                },
            ] => {
                assert_eq!(ssid, "eduroam");
                assert_eq!(identity.len(), "student@example.edu".len());
                assert_eq!(password.len(), "p@ss word".len());
                assert_eq!(domain, "radius.example.edu");
                assert!(!format!("{identity:?}{password:?}").contains("student"));
            }
            _ => panic!("expected an enterprise connect request"),
        }
    }

    #[test]
    fn incomplete_enterprise_join_focuses_the_missing_field() {
        let mut model = AppModel {
            view: View::Wifi,
            networks: vec![network("eduroam", 80, Security::Enterprise)],
            ..AppModel::default()
        };
        model.handle(Action::SelectNetwork(0));
        type_text(&mut model, "student");
        model.handle(Action::Focus(Field::Domain));
        type_text(&mut model, "localhost");
        assert!(model.handle(Action::SubmitPassword).is_empty());
        let dialog = model.password_dialog.as_ref().unwrap();
        assert!(dialog.submit_attempted);
        assert_eq!(dialog.focus, Field::Password);
        assert!(!dialog.field_is_valid(Field::Domain));

        // Return on the last invalid field flags it instead of moving on.
        model.handle(Action::Focus(Field::Password));
        type_text(&mut model, "pw");
        model.handle(Action::Return);
        let dialog = model.password_dialog.as_mut().unwrap();
        assert_eq!(dialog.focus, Field::Domain);
        dialog.submit_attempted = false;
        model.handle(Action::Return);
        assert!(model.password_dialog.as_ref().unwrap().submit_attempted);

        model.handle(Action::Key(KeyAction::Clear));
        type_text(&mut model, "radius.example.edu");
        assert_eq!(model.handle(Action::Return).len(), 1);
    }

    #[test]
    fn personal_networks_keep_a_single_password_field() {
        let mut model = AppModel {
            view: View::Wifi,
            networks: vec![network("Home", 80, Security::Wpa2)],
            ..AppModel::default()
        };
        model.handle(Action::SelectNetwork(0));
        let dialog = model.password_dialog.as_ref().unwrap();
        assert!(dialog.enterprise.is_none());
        assert_eq!(dialog.fields(), &[Field::Password]);
        model.handle(Action::Focus(Field::Identity));
        assert_eq!(
            model.password_dialog.as_ref().unwrap().focus,
            Field::Password
        );
        type_text(&mut model, "longenough");
        assert!(matches!(
            model.handle(Action::Return).as_slice(),
            [WifiRequest::Connect {
                password: Some(_),
                ..
            }]
        ));
    }

    #[test]
    fn shift_is_one_shot_and_symbol_modes_are_deterministic() {
        let mut model = AppModel {
            password_dialog: Some(PasswordDialog::new("test".into(), Security::Wpa2)),
            ..AppModel::default()
        };
        model.handle(Action::Key(KeyAction::Shift));
        assert_eq!(
            model.password_dialog.as_ref().unwrap().keyboard_mode,
            KeyboardMode::Upper
        );
        model.handle(Action::Key(KeyAction::Character('a')));
        let dialog = model.password_dialog.as_ref().unwrap();
        assert_eq!(dialog.keyboard_mode, KeyboardMode::Lower);
        assert_eq!(dialog.password_len(), 1);

        model.handle(Action::Key(KeyAction::Symbols));
        assert_eq!(
            model.password_dialog.as_ref().unwrap().keyboard_mode,
            KeyboardMode::Symbols
        );
        model.handle(Action::Key(KeyAction::MoreSymbols));
        assert_eq!(
            model.password_dialog.as_ref().unwrap().keyboard_mode,
            KeyboardMode::MoreSymbols
        );
    }

    #[test]
    fn scan_results_are_deduplicated_sorted_and_paginated() {
        let mut networks = vec![
            network("weak", 10, Security::Open),
            network("strong", 95, Security::Open),
            network("same", 20, Security::Wpa2),
            network("same", 80, Security::Wpa2),
        ];
        networks.extend((0..8).map(|index| network(&format!("n{index}"), 50, Security::Open)));
        let mut model = AppModel::default();
        model.apply_wifi_event(WifiEvent::Scan(Ok(networks)));
        assert_eq!(
            model
                .networks
                .iter()
                .filter(|item| item.ssid == "same")
                .count(),
            1
        );
        assert_eq!(model.networks[0].ssid, "strong");
        assert_eq!(model.page_count(), 2);
        assert_eq!(model.page_range(), 0..NETWORKS_PER_PAGE);
        model.handle(Action::NextNetworks);
        assert_eq!(model.page, 1);
    }

    #[test]
    fn backend_messages_never_enter_notice_state() {
        let mut model = AppModel::default();
        model.apply_wifi_event(WifiEvent::Connect(Err(WifiError::Rejected(
            Rejection::AuthenticationFailed,
        ))));
        assert_eq!(model.notice, Some("Incorrect password"));
    }
}
