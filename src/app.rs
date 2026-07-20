use std::cmp::Ordering;

use crate::wifi::{
    LinkState, Rejection, SecretString, Security, WifiError, WifiEvent, WifiNetwork, WifiRequest,
    WifiStatus,
};

pub const NETWORKS_PER_PAGE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    System,
    Wifi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardMode {
    Lower,
    Upper,
    Symbols,
    MoreSymbols,
}

pub struct PasswordDialog {
    pub ssid: String,
    pub security: Security,
    password: SecretString,
    pub keyboard_mode: KeyboardMode,
    pub submit_attempted: bool,
}

impl PasswordDialog {
    fn new(ssid: String, security: Security) -> Self {
        Self {
            ssid,
            security,
            password: SecretString::default(),
            keyboard_mode: KeyboardMode::Lower,
            submit_attempted: false,
        }
    }

    pub fn password_len(&self) -> usize {
        self.password.len()
    }

    pub fn password_is_valid(&self) -> bool {
        self.password.is_valid_wpa_psk()
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
    RefreshWifi,
    SelectNetwork(usize),
    PreviousNetworks,
    NextNetworks,
    Disconnect,
    CancelPassword,
    SubmitPassword,
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
    wifi_initialized: bool,
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
            wifi_initialized: false,
        }
    }
}

impl AppModel {
    pub fn handle(&mut self, action: Action) -> Vec<WifiRequest> {
        match action {
            Action::ShowSystem => {
                self.password_dialog = None;
                self.view = View::System;
                Vec::new()
            }
            Action::ShowWifi => {
                self.view = View::Wifi;
                if self.wifi_initialized {
                    vec![WifiRequest::Status]
                } else {
                    self.wifi_initialized = true;
                    self.scanning = true;
                    vec![WifiRequest::Status, WifiRequest::Scan]
                }
            }
            Action::RefreshWifi if !self.busy && self.password_dialog.is_none() => {
                self.scanning = true;
                self.notice = None;
                vec![WifiRequest::Status, WifiRequest::Scan]
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
                self.notice = None;
                vec![WifiRequest::Disconnect]
            }
            Action::CancelPassword => {
                self.password_dialog = None;
                Vec::new()
            }
            Action::SubmitPassword => self.submit_password(),
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
                    self.set_status(status);
                    self.notice = None;
                }
                Err(error) => self.service_error(error),
            },
            WifiEvent::Scan(result) => {
                self.scanning = false;
                match result {
                    Ok(networks) => {
                        self.networks = normalized_networks(networks);
                        self.clamp_page();
                        self.reconcile_connected_network();
                        self.notice = None;
                    }
                    Err(error) => self.notice = Some(error_notice(error)),
                }
            }
            WifiEvent::Connect(result) => {
                self.busy = false;
                match result {
                    Ok(status) => {
                        self.set_status(status);
                        self.reconcile_connected_network();
                        self.notice = Some("CONNECTED");
                    }
                    Err(error) => {
                        if let Some(status) = &mut self.wifi_status {
                            status.state = LinkState::Disconnected;
                        }
                        self.reconcile_connected_network();
                        self.notice = Some(error_notice(error));
                    }
                }
            }
            WifiEvent::Disconnect(result) => {
                self.busy = false;
                match result {
                    Ok(status) => {
                        self.set_status(status);
                        self.reconcile_connected_network();
                        self.notice = Some("DISCONNECTED");
                    }
                    Err(error) => self.notice = Some(error_notice(error)),
                }
            }
        }
    }

    pub fn worker_unavailable(&mut self) {
        self.busy = false;
        self.scanning = false;
        self.wifi_status = None;
        self.password_dialog = None;
        self.notice = Some("WI-FI SERVICE UNAVAILABLE");
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
        if network.security == Security::Enterprise {
            self.notice = Some("ENTERPRISE WI-FI IS NOT SUPPORTED");
            return Vec::new();
        }

        let ssid = network.ssid.clone();
        let security = network.security;
        if network.requires_password {
            self.notice = None;
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
            self.password_dialog = Some(dialog);
            return Vec::new();
        }
        self.begin_connect(dialog.ssid, Some(dialog.password))
    }

    fn begin_connect(&mut self, ssid: String, password: Option<SecretString>) -> Vec<WifiRequest> {
        self.busy = true;
        self.notice = None;
        self.wifi_status = Some(WifiStatus {
            radio_enabled: true,
            state: LinkState::Connecting { ssid: ssid.clone() },
        });
        vec![WifiRequest::Connect { ssid, password }]
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
                dialog.password.push(character);
                if dialog.keyboard_mode == KeyboardMode::Upper {
                    dialog.keyboard_mode = KeyboardMode::Lower;
                }
            }
            KeyAction::Space => {
                dialog.password.push(' ');
            }
            KeyAction::Backspace => dialog.password.pop(),
            KeyAction::Clear => dialog.password.clear(),
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

    fn set_status(&mut self, status: WifiStatus) {
        self.wifi_status = Some(status);
        self.reconcile_connected_network();
    }

    fn service_error(&mut self, error: WifiError) {
        self.wifi_status = None;
        self.notice = Some(error_notice(error));
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
        WifiError::Unavailable => "WI-FI SERVICE UNAVAILABLE",
        WifiError::Timeout => "WI-FI SERVICE TIMED OUT",
        WifiError::Protocol => "WI-FI SERVICE ERROR",
        WifiError::Rejected(Rejection::AuthenticationFailed) => "PASSWORD NOT ACCEPTED",
        WifiError::Rejected(Rejection::NetworkNotFound) => "NETWORK NOT FOUND",
        WifiError::Rejected(Rejection::RadioDisabled) => "WI-FI IS TURNED OFF",
        WifiError::Rejected(Rejection::Busy) => "WI-FI SERVICE IS BUSY",
        WifiError::Rejected(Rejection::Unsupported) => "NETWORK IS NOT SUPPORTED",
        WifiError::Rejected(Rejection::Other) => "CONNECTION FAILED",
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
            networks: vec![network("A26 Lab", 90, Security::Wpa3)],
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
        assert_eq!(model.page_count(), 3);
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
        assert_eq!(model.notice, Some("PASSWORD NOT ACCEPTED"));
    }
}
