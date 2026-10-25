use std::fmt;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const WIFI_SOCKET: &str = "/run/a26-wifi/control.sock";

const IO_TIMEOUT: Duration = Duration::from_millis(750);
const RESPONSE_LIMIT: u64 = 64 * 1024;
const MAX_NETWORKS: usize = 64;
const MAX_SSID_BYTES: usize = 32;

/// A short-lived, in-memory credential buffer.
///
/// It deliberately has no `Clone`, `Display`, or serialization implementation.
/// Its `Debug` implementation is always redacted and its allocation is wiped
/// when moved out of the UI model and eventually dropped by the worker.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretString {
    bytes: [u8; SECRET_CAPACITY],
    len: usize,
}

/// Large enough for an enterprise identity or password (128 bytes each);
/// WPA Personal keys are still validated to their 63/64-byte limits.
const SECRET_CAPACITY: usize = 128;
const MAX_DOMAIN_BYTES: usize = 253;

impl Default for SecretString {
    fn default() -> Self {
        Self {
            bytes: [0; SECRET_CAPACITY],
            len: 0,
        }
    }
}

impl SecretString {
    pub fn push(&mut self, character: char) -> bool {
        if !character.is_ascii() || !matches!(character, ' '..='~') || self.len >= self.bytes.len()
        {
            return false;
        }
        self.bytes[self.len] = character as u8;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            self.bytes[self.len] = 0;
        }
    }

    pub fn clear(&mut self) {
        self.bytes.zeroize();
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_valid_wpa_psk(&self) -> bool {
        valid_wpa_psk(self.expose_for_socket())
    }

    /// Enterprise identities and passwords: 1-128 printable ASCII bytes
    /// (`push` admits nothing else).
    pub fn is_valid_enterprise_field(&self) -> bool {
        self.len > 0
    }

    /// The text, for drawing an identity the user is typing. Never log it.
    pub fn reveal_for_display(&self) -> &str {
        self.expose_for_socket()
    }

    fn expose_for_socket(&self) -> &str {
        // `push` accepts printable ASCII only, so this prefix is always UTF-8.
        std::str::from_utf8(&self.bytes[..self.len]).expect("ASCII secret buffer")
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

/// A server domain for certificate validation, matching the backend's rule:
/// at least two lowercase labels of letters, digits, and inner hyphens.
pub fn valid_domain(domain: &str) -> bool {
    domain.len() <= MAX_DOMAIN_BYTES
        && domain.split('.').count() >= 2
        && domain.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn valid_wpa_psk(password: &str) -> bool {
    let bytes = password.as_bytes();
    (bytes.len() == 64 && bytes.iter().all(u8::is_ascii_hexdigit))
        || ((8..=63).contains(&bytes.len()) && bytes.iter().all(|byte| matches!(byte, b' '..=b'~')))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    Wpa2,
    Wpa3,
    Enterprise,
}

impl Security {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "wpa2" => Some(Self::Wpa2),
            "wpa3" => Some(Self::Wpa3),
            "enterprise" => Some(Self::Enterprise),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u8,
    pub security: Security,
    pub requires_password: bool,
    pub connected: bool,
}

impl WifiNetwork {
    /// Whether this UI can join the network. WPA3 cannot be joined. A saved
    /// enterprise network (`requires_password: false`) reconnects directly;
    /// a new one asks for an identity, password, and server domain.
    pub fn joinable(&self) -> bool {
        self.security != Security::Wpa3
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    Disconnected,
    Connecting {
        ssid: String,
    },
    Connected {
        ssid: String,
        signal: Option<u8>,
        ip_address: Option<String>,
    },
    Disconnecting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiStatus {
    pub radio_enabled: bool,
    pub state: LinkState,
}

pub enum WifiRequest {
    Status,
    Scan,
    Connect {
        ssid: String,
        password: Option<SecretString>,
    },
    /// WPA2-Enterprise (PEAP/MSCHAPv2) with server validation by `domain`.
    ConnectEnterprise {
        ssid: String,
        identity: SecretString,
        password: SecretString,
        domain: String,
    },
    Disconnect,
}

pub enum WifiEvent {
    Status(Result<WifiStatus, WifiError>),
    Scan(Result<Vec<WifiNetwork>, WifiError>),
    Connect(Result<WifiStatus, WifiError>),
    Disconnect(Result<WifiStatus, WifiError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    AuthenticationFailed,
    NetworkNotFound,
    RadioDisabled,
    Busy,
    Unsupported,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiError {
    Unavailable,
    Timeout,
    Protocol,
    Rejected(Rejection),
}

impl fmt::Display for WifiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unavailable => "Wi-Fi service unavailable",
            Self::Timeout => "Wi-Fi service timed out",
            Self::Protocol => "invalid Wi-Fi service response",
            Self::Rejected(_) => "Wi-Fi request rejected",
        };
        formatter.write_str(message)
    }
}

pub struct WifiWorker {
    requests: Sender<WifiRequest>,
    events: Receiver<WifiEvent>,
}

impl WifiWorker {
    pub fn spawn() -> Self {
        // A26_WIFI_SOCKET lets development hosts point the UI at a stand-in
        // backend; the phone always uses the root-only default.
        let socket = std::env::var_os("A26_WIFI_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(WIFI_SOCKET));
        Self::spawn_for(socket)
    }

    fn spawn_for(socket: PathBuf) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || worker_loop(&socket, request_rx, event_tx));
        Self {
            requests: request_tx,
            events: event_rx,
        }
    }

    pub fn send(&self, request: WifiRequest) -> bool {
        self.requests.send(request).is_ok()
    }

    pub fn try_recv(&self) -> Option<WifiEvent> {
        self.events.try_recv().ok()
    }
}

fn worker_loop(socket: &Path, requests: Receiver<WifiRequest>, events: Sender<WifiEvent>) {
    let mut request_id = 1_u64;
    while let Ok(request) = requests.recv() {
        let event = execute(socket, request_id, request);
        request_id = request_id.wrapping_add(1).max(1);
        if events.send(event).is_err() {
            break;
        }
    }
}

fn execute(socket: &Path, request_id: u64, request: WifiRequest) -> WifiEvent {
    match request {
        WifiRequest::Status => WifiEvent::Status(status(socket, request_id)),
        WifiRequest::Scan => WifiEvent::Scan(scan(socket, request_id)),
        WifiRequest::Connect { ssid, password } => {
            WifiEvent::Connect(connect(socket, request_id, &ssid, password.as_ref()))
        }
        WifiRequest::ConnectEnterprise {
            ssid,
            identity,
            password,
            domain,
        } => WifiEvent::Connect(connect_enterprise(
            socket, request_id, &ssid, &identity, &password, &domain,
        )),
        WifiRequest::Disconnect => WifiEvent::Disconnect(disconnect(socket, request_id)),
    }
}

#[derive(Serialize)]
struct BasicRequest {
    id: u64,
    command: &'static str,
}

#[derive(Serialize)]
struct ConnectRequest<'a> {
    id: u64,
    command: &'static str,
    ssid: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
}

#[derive(Serialize)]
struct EnterpriseRequest<'a> {
    id: u64,
    command: &'static str,
    ssid: &'a str,
    identity: &'a str,
    password: &'a str,
    domain: &'a str,
}

fn status(socket: &Path, request_id: u64) -> Result<WifiStatus, WifiError> {
    let request = BasicRequest {
        id: request_id,
        command: "status",
    };
    let result = transact(socket, request_id, &request)?;
    parse_status(result)
}

fn scan(socket: &Path, request_id: u64) -> Result<Vec<WifiNetwork>, WifiError> {
    let request = BasicRequest {
        id: request_id,
        command: "scan",
    };
    let result = transact(socket, request_id, &request)?;
    parse_scan(result)
}

fn connect(
    socket: &Path,
    request_id: u64,
    ssid: &str,
    password: Option<&SecretString>,
) -> Result<WifiStatus, WifiError> {
    if !valid_ssid(ssid) || password.is_some_and(|secret| !secret.is_valid_wpa_psk()) {
        return Err(WifiError::Protocol);
    }
    let request = ConnectRequest {
        id: request_id,
        command: "connect",
        ssid,
        password: password.map(SecretString::expose_for_socket),
    };
    let result = transact(socket, request_id, &request)?;
    parse_status(result)
}

fn connect_enterprise(
    socket: &Path,
    request_id: u64,
    ssid: &str,
    identity: &SecretString,
    password: &SecretString,
    domain: &str,
) -> Result<WifiStatus, WifiError> {
    if !valid_ssid(ssid)
        || !identity.is_valid_enterprise_field()
        || !password.is_valid_enterprise_field()
        || !valid_domain(domain)
    {
        return Err(WifiError::Protocol);
    }
    // Serialized straight into the socket: no copy of the credentials is
    // kept in an intermediate buffer.
    let request = EnterpriseRequest {
        id: request_id,
        command: "connect_enterprise",
        ssid,
        identity: identity.expose_for_socket(),
        password: password.expose_for_socket(),
        domain,
    };
    let result = transact(socket, request_id, &request)?;
    parse_status(result)
}

fn disconnect(socket: &Path, request_id: u64) -> Result<WifiStatus, WifiError> {
    let request = BasicRequest {
        id: request_id,
        command: "disconnect",
    };
    let result = transact(socket, request_id, &request)?;
    parse_status(result)
}

fn transact<T: Serialize>(socket: &Path, request_id: u64, request: &T) -> Result<Value, WifiError> {
    let mut stream = UnixStream::connect(socket).map_err(map_io_error)?;
    let uid = peer_uid(&stream).map_err(map_io_error)?;
    // SAFETY: geteuid has no pointer arguments or memory-safety preconditions.
    let own_uid = unsafe { libc::geteuid() };
    // A development stand-in on an overridden socket may run as the same
    // unprivileged user; the phone's default endpoint must be root's.
    let development = cfg!(test) || socket != Path::new(WIFI_SOCKET);
    if uid != 0 && !(development && uid == own_uid) {
        return Err(WifiError::Unavailable);
    }
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(map_io_error)?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(map_io_error)?;

    serde_json::to_writer(&mut stream, request).map_err(|_| WifiError::Protocol)?;
    stream.write_all(b"\n").map_err(map_io_error)?;
    stream.flush().map_err(map_io_error)?;
    stream.shutdown(Shutdown::Write).map_err(map_io_error)?;

    let mut response = Vec::new();
    stream
        .take(RESPONSE_LIMIT + 1)
        .read_to_end(&mut response)
        .map_err(map_io_error)?;
    if response.len() as u64 > RESPONSE_LIMIT {
        return Err(WifiError::Protocol);
    }

    let document: Value = serde_json::from_slice(&response).map_err(|_| WifiError::Protocol)?;
    if document.get("id").and_then(Value::as_u64) != Some(request_id) {
        return Err(WifiError::Protocol);
    }
    match document.get("ok").and_then(Value::as_bool) {
        Some(true) => document.get("result").cloned().ok_or(WifiError::Protocol),
        Some(false) => Err(parse_rejection(&document)),
        None => Err(WifiError::Protocol),
    }
}

fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the output storage is valid for the supplied length and the
    // connected Unix-stream descriptor remains live during getsockopt.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(std::io::Error::other("unexpected peer credential length"));
    }
    // SAFETY: successful getsockopt initialized the complete ucred value.
    Ok(unsafe { credentials.assume_init() }.uid)
}

fn map_io_error(error: std::io::Error) -> WifiError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => WifiError::Timeout,
        _ => WifiError::Unavailable,
    }
}

fn parse_rejection(document: &Value) -> WifiError {
    let code = document
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str);
    let rejection = match code {
        Some("authentication_failed") => Rejection::AuthenticationFailed,
        Some("network_not_found") => Rejection::NetworkNotFound,
        Some("radio_disabled") => Rejection::RadioDisabled,
        Some("busy") => Rejection::Busy,
        Some("unsupported") => Rejection::Unsupported,
        _ => Rejection::Other,
    };
    WifiError::Rejected(rejection)
}

fn parse_status(result: Value) -> Result<WifiStatus, WifiError> {
    let radio_enabled = result
        .get("radio_enabled")
        .and_then(Value::as_bool)
        .ok_or(WifiError::Protocol)?;
    let state = match result.get("state").and_then(Value::as_str) {
        Some("disconnected") => LinkState::Disconnected,
        Some("disconnecting") => LinkState::Disconnecting,
        Some("connecting") => LinkState::Connecting {
            ssid: parse_ssid(&result)?,
        },
        Some("connected") => LinkState::Connected {
            ssid: parse_ssid(&result)?,
            signal: optional_signal(&result)?,
            ip_address: optional_short_string(&result, "ip_address", 64)?,
        },
        _ => return Err(WifiError::Protocol),
    };
    if !radio_enabled && !matches!(&state, LinkState::Disconnected) {
        return Err(WifiError::Protocol);
    }
    Ok(WifiStatus {
        radio_enabled,
        state,
    })
}

fn parse_scan(result: Value) -> Result<Vec<WifiNetwork>, WifiError> {
    let values = result
        .get("networks")
        .and_then(Value::as_array)
        .ok_or(WifiError::Protocol)?;
    if values.len() > MAX_NETWORKS {
        return Err(WifiError::Protocol);
    }
    values
        .iter()
        .map(|value| {
            let ssid = value
                .get("ssid")
                .and_then(Value::as_str)
                .filter(|ssid| valid_ssid(ssid))
                .ok_or(WifiError::Protocol)?
                .to_owned();
            let signal = value
                .get("signal")
                .and_then(Value::as_u64)
                .and_then(|signal| u8::try_from(signal).ok())
                .filter(|signal| *signal <= 100)
                .ok_or(WifiError::Protocol)?;
            let security = value
                .get("security")
                .and_then(Value::as_str)
                .and_then(Security::parse)
                .ok_or(WifiError::Protocol)?;
            let requires_password = value
                .get("requires_password")
                .and_then(Value::as_bool)
                .ok_or(WifiError::Protocol)?;
            if security == Security::Open && requires_password {
                return Err(WifiError::Protocol);
            }
            let connected = value
                .get("connected")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(WifiNetwork {
                ssid,
                signal,
                security,
                requires_password,
                connected,
            })
        })
        .collect()
}

fn parse_ssid(value: &Value) -> Result<String, WifiError> {
    value
        .get("ssid")
        .and_then(Value::as_str)
        .filter(|ssid| valid_ssid(ssid))
        .map(str::to_owned)
        .ok_or(WifiError::Protocol)
}

fn valid_ssid(ssid: &str) -> bool {
    !ssid.is_empty() && ssid.len() <= MAX_SSID_BYTES && !ssid.chars().any(char::is_control)
}

fn optional_signal(value: &Value) -> Result<Option<u8>, WifiError> {
    match value.get("signal") {
        None | Some(Value::Null) => Ok(None),
        Some(signal) => signal
            .as_u64()
            .and_then(|signal| u8::try_from(signal).ok())
            .filter(|signal| *signal <= 100)
            .map(Some)
            .ok_or(WifiError::Protocol),
    }
}

fn optional_short_string(
    value: &Value,
    field: &str,
    max_bytes: usize,
) -> Result<Option<String>, WifiError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|text| text.len() <= max_bytes && !text.chars().any(char::is_control))
            .map(str::to_owned)
            .map(Some)
            .ok_or(WifiError::Protocol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_NUMBER: AtomicU64 = AtomicU64::new(1);

    fn secret(text: &str) -> SecretString {
        let mut secret = SecretString::default();
        for character in text.chars() {
            assert!(secret.push(character));
        }
        secret
    }

    fn test_socket() -> PathBuf {
        std::env::temp_dir().join(format!(
            "a26-system-wifi-{}-{}.sock",
            std::process::id(),
            TEST_NUMBER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn password_validation_matches_wpa_personal_rules() {
        assert!(!valid_wpa_psk("short"));
        assert!(valid_wpa_psk("eight888"));
        assert!(valid_wpa_psk(&"f".repeat(64)));
        assert!(!valid_wpa_psk(&"g".repeat(64)));
        assert!(!valid_wpa_psk(&"a".repeat(65)));
    }

    #[test]
    fn secret_debug_is_always_redacted() {
        let secret = secret("not-for-debug");
        let debug = format!("{secret:?}");
        assert_eq!(debug, "SecretString([REDACTED])");
        assert!(!debug.contains("not-for-debug"));
    }

    #[test]
    fn parses_bounded_status_and_scan_documents() {
        let status = parse_status(serde_json::json!({
            "radio_enabled": true,
            "state": "connected",
            "ssid": "A26 Lab",
            "signal": 87,
            "ip_address": "192.0.2.7"
        }))
        .unwrap();
        assert_eq!(
            status.state,
            LinkState::Connected {
                ssid: "A26 Lab".into(),
                signal: Some(87),
                ip_address: Some("192.0.2.7".into())
            }
        );

        let networks = parse_scan(serde_json::json!({"networks": [{
            "ssid": "A26 Lab",
            "signal": 87,
            "security": "wpa3",
            "requires_password": true,
            "connected": false
        }]}))
        .unwrap();
        assert_eq!(networks[0].security, Security::Wpa3);
        assert!(networks[0].requires_password);
    }

    #[test]
    fn rejects_invalid_backend_ranges_and_shapes() {
        assert_eq!(
            parse_status(serde_json::json!({
                "radio_enabled": true,
                "state": "connected",
                "ssid": "ok",
                "signal": 101
            })),
            Err(WifiError::Protocol)
        );
        assert_eq!(
            parse_scan(serde_json::json!({"networks": [{
                "ssid": "bad\nname",
                "signal": 20,
                "security": "wpa2",
                "requires_password": true
            }]})),
            Err(WifiError::Protocol)
        );
    }

    #[test]
    fn connect_uses_one_json_request_and_accepts_only_matching_response_id() {
        let socket = test_socket();
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            let document: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(document.get("id").and_then(Value::as_u64), Some(41));
            assert_eq!(
                document.get("command").and_then(Value::as_str),
                Some("connect")
            );
            assert_eq!(
                document.get("ssid").and_then(Value::as_str),
                Some("A26 Lab")
            );
            assert_eq!(
                document.get("password").and_then(Value::as_str),
                Some("correct horse")
            );
            stream
                .write_all(
                    b"{\"id\":41,\"ok\":true,\"result\":{\"radio_enabled\":true,\"state\":\"connected\",\"ssid\":\"A26 Lab\",\"signal\":90}}\n",
                )
                .unwrap();
        });

        let result = connect(&socket, 41, "A26 Lab", Some(&secret("correct horse"))).unwrap();
        assert!(matches!(result.state, LinkState::Connected { .. }));
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
    }

    #[test]
    fn domain_validation_matches_the_backend() {
        assert!(valid_domain("radius.wireless.utoronto.ca"));
        assert!(!valid_domain("utoronto"));
        assert!(!valid_domain("Radius.example.org"));
        assert!(!valid_domain("bad\"quote.org"));
        assert!(!valid_domain("-bad.org"));
        assert!(!valid_domain("trailing.dot."));
    }

    #[test]
    fn enterprise_connect_sends_identity_password_and_domain_once() {
        let socket = test_socket();
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            let document: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(
                document.get("command").and_then(Value::as_str),
                Some("connect_enterprise")
            );
            assert_eq!(
                document.get("ssid").and_then(Value::as_str),
                Some("eduroam")
            );
            assert_eq!(
                document.get("identity").and_then(Value::as_str),
                Some("student@example.edu")
            );
            assert_eq!(
                document.get("password").and_then(Value::as_str),
                Some("p@ss word")
            );
            assert_eq!(
                document.get("domain").and_then(Value::as_str),
                Some("radius.example.edu")
            );
            stream
                .write_all(
                    b"{\"id\":7,\"ok\":true,\"result\":{\"radio_enabled\":true,\"state\":\"connecting\",\"ssid\":\"eduroam\"}}\n",
                )
                .unwrap();
        });
        let result = connect_enterprise(
            &socket,
            7,
            "eduroam",
            &secret("student@example.edu"),
            &secret("p@ss word"),
            "radius.example.edu",
        )
        .unwrap();
        assert!(matches!(result.state, LinkState::Connecting { .. }));
        server.join().unwrap();
        fs::remove_file(&socket).unwrap();

        // Invalid fields never reach the socket.
        assert_eq!(
            connect_enterprise(
                &socket,
                8,
                "eduroam",
                &secret("student"),
                &secret("pw"),
                "not a domain",
            ),
            Err(WifiError::Protocol)
        );
        assert_eq!(
            connect_enterprise(
                &socket,
                9,
                "eduroam",
                &SecretString::default(),
                &secret("pw"),
                "radius.example.edu",
            ),
            Err(WifiError::Protocol)
        );
    }

    #[test]
    fn rejection_parser_uses_codes_and_ignores_backend_messages() {
        let error = parse_rejection(&serde_json::json!({
            "error": {
                "code": "authentication_failed",
                "message": "must never reach UI state"
            }
        }));
        assert_eq!(error, WifiError::Rejected(Rejection::AuthenticationFailed));
        assert_eq!(error.to_string(), "Wi-Fi request rejected");
    }
}
