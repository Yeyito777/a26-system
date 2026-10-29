use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const CONTROL_SOCKET: &str = "/run/a26-shell/control.sock";
const RESPONSE_LIMIT: u64 = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_millis(200);

/// Moon's control socket; `A26_SHELL_SOCKET` overrides it on development
/// hosts, as it does for `a26-shellctl`.
fn socket() -> PathBuf {
    std::env::var_os("A26_SHELL_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(CONTROL_SOCKET))
}

/// Moon's display state (docs/display-brightness.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayState {
    pub brightness: u8,
    pub auto: bool,
    /// `ok`, `unavailable` or `off`.
    pub sensor: String,
}

/// What the System app shows of Moon's settings (docs/settings.md).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellInfo {
    pub volume: Option<u8>,
    pub display: Option<DisplayState>,
    pub wallpaper: Option<String>,
    pub auto_lock_s: Option<u32>,
    /// Tap haptics on or off (Moon's `haptics` setting).
    pub haptics: Option<bool>,
    /// Moon's local time (zone id, TZ rule, 24-hour clock).
    pub time: Option<TimeInfo>,
    /// What a tapped notification asked an app to show.
    pub open_target: Option<OpenTarget>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeInfo {
    pub zone: Option<String>,
    pub tz: Option<String>,
    pub clock_24h: bool,
}

static CLOCK_24H: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The 24-hour clock is on (as of Moon's last state).
pub fn clock_24h() -> bool {
    CLOCK_24H.load(std::sync::atomic::Ordering::Relaxed)
}

/// Follows Moon's local time: its time zone (this process's `TZ`) and its
/// 24-hour clock. Returns true when anything changed (redraw).
pub fn apply_time(info: &ShellInfo) -> bool {
    let Some(time) = &info.time else {
        return false;
    };
    let mut changed =
        CLOCK_24H.swap(time.clock_24h, std::sync::atomic::Ordering::Relaxed) != time.clock_24h;
    if let Some(rule) = &time.tz
        && std::env::var("TZ").ok().as_deref() != Some(rule.as_str())
    {
        // SAFETY: the UI thread is the only one that reads the time zone;
        // the workers never call into the C library's local time.
        unsafe {
            std::env::set_var("TZ", rule);
            tzset();
        }
        changed = true;
    }
    changed
}

unsafe extern "C" {
    fn tzset();
}

/// Moon's `open_target`: a notification's `open.target` for an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    pub app: String,
    pub target: String,
    pub serial: u64,
    pub age_ms: u64,
}

pub fn read_shell() -> ShellInfo {
    request(&socket(), b"state\n")
        .as_deref()
        .map(parse_shell)
        .unwrap_or_default()
}

fn parse_shell(response: &str) -> ShellInfo {
    let mut info = ShellInfo {
        volume: parse_volume_response(response),
        ..ShellInfo::default()
    };
    let Ok(document) = serde_json::from_str::<serde_json::Value>(response) else {
        return info;
    };
    let result = &document["result"];
    if let Some(display) = result.get("display").filter(|d| d.is_object()) {
        info.display = Some(DisplayState {
            brightness: display["brightness"].as_u64().unwrap_or(50).min(100) as u8,
            auto: display["auto"].as_bool().unwrap_or(false),
            sensor: display["sensor"]
                .as_str()
                .unwrap_or("unavailable")
                .to_owned(),
        });
    }
    info.wallpaper = result["settings"]["wallpaper"].as_str().map(str::to_owned);
    info.auto_lock_s = result["settings"]["auto_lock_s"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok());
    info.haptics = result["settings"]["haptics"].as_bool();
    if let Some(time) = result.get("time").filter(|t| t.is_object()) {
        info.time = Some(TimeInfo {
            zone: time["zone"].as_str().map(str::to_owned),
            tz: time["tz"].as_str().map(str::to_owned),
            clock_24h: time["clock_24h"].as_bool().unwrap_or(false),
        });
    }
    let open = &result["open_target"];
    if let (Some(app), Some(target), Some(serial)) = (
        open["app"].as_str(),
        open["target"].as_str(),
        open["serial"].as_u64(),
    ) {
        info.open_target = Some(OpenTarget {
            app: app.to_owned(),
            target: target.to_owned(),
            serial,
            age_ms: open["age_ms"].as_u64().unwrap_or(u64::MAX),
        });
    }
    info
}

/// Tells Moon this app is about to show another page (`push` deeper, else
/// back). Moon snapshots the page on screen and answers; the app then draws
/// the new page and Moon slides one over the other (GPU mode; elsewhere it
/// declines and the page simply changes).
pub fn transition(push: bool) -> bool {
    let command: &[u8] = if push {
        b"transition push\n"
    } else {
        b"transition pop\n"
    };
    let started = request(&socket(), command)
        .and_then(|response| serde_json::from_str::<serde_json::Value>(&response).ok())
        .and_then(|document| document.pointer("/result/started")?.as_bool())
        .unwrap_or(false);
    TRANSITION_DRAWING.store(started, std::sync::atomic::Ordering::Relaxed);
    started
}

/// A transition started and the new page has not been reported drawn yet.
static TRANSITION_DRAWING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// After drawing: tells Moon the new page is on screen (`transition
/// ready`), so the slide starts with it complete. `synced` runs first: an X
/// round trip, so the server has drawn everything sent so far.
pub fn page_drawn(synced: impl FnOnce()) {
    if TRANSITION_DRAWING.swap(false, std::sync::atomic::Ordering::Relaxed) {
        synced();
        send("transition ready");
    }
}

/// Sends one command to Moon and returns its `result` when Moon accepted it
/// (`clipboard get`).
pub fn query(command: &str) -> Option<serde_json::Value> {
    let response = request(&socket(), format!("{command}\n").as_bytes())?;
    let mut document: serde_json::Value = serde_json::from_str(&response).ok()?;
    if !document.get("ok")?.as_bool()? {
        return None;
    }
    Some(document.get_mut("result")?.take())
}

/// `query` for a long answer (`clipboard get image`: megabytes of base64 on
/// one line): up to `limit` bytes, each read and write within `timeout`.
pub fn query_long(command: &str, timeout: Duration, limit: u64) -> Option<serde_json::Value> {
    let response = request_limited(&socket(), format!("{command}\n").as_bytes(), timeout, limit)?;
    let mut document: serde_json::Value = serde_json::from_str(&response).ok()?;
    if !document.get("ok")?.as_bool()? {
        return None;
    }
    Some(document.get_mut("result")?.take())
}

/// Sends one command to Moon (`brightness set 40`, `settings set ...`).
/// Returns whether Moon accepted it.
pub fn send(command: &str) -> bool {
    request(&socket(), format!("{command}\n").as_bytes())
        .and_then(|response| serde_json::from_str::<serde_json::Value>(&response).ok())
        .and_then(|document| document.get("ok")?.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
fn read_volume_from(path: &Path) -> Option<u8> {
    // Moon answers within IO_TIMEOUT; the test's server thread may not even
    // be scheduled that soon on a loaded machine.
    parse_volume_response(&request_within(path, b"state\n", Duration::from_secs(10))?)
}

/// Moon keyboard layouts this app asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Multiline and Search are the Exocortex packages' (D, B).
pub enum KeyboardPurpose {
    Text,
    /// A text area: Return types a newline and the keyboard stays up
    /// (OSS-Phone-V1 `docs/app-services.md`).
    Multiline,
    Url,
    Password,
    /// A search field.
    Search,
}

/// Asks Moon to show its system keyboard. Returns whether the keyboard is
/// now visible; Moon refuses while locked or launching.
pub fn show_keyboard(purpose: KeyboardPurpose) -> bool {
    let command: &[u8] = match purpose {
        KeyboardPurpose::Text => b"keyboard show text\n",
        KeyboardPurpose::Multiline => b"keyboard show multiline\n",
        KeyboardPurpose::Url => b"keyboard show url\n",
        KeyboardPurpose::Search => b"keyboard show search\n",
        KeyboardPurpose::Password => b"keyboard show password\n",
    };
    request(&socket(), command)
        .as_deref()
        .and_then(parse_keyboard_visible)
        .unwrap_or(false)
}

pub fn hide_keyboard() {
    let _ = request(&socket(), b"keyboard hide\n");
}

fn request(path: &Path, command: &[u8]) -> Option<String> {
    request_within(path, command, IO_TIMEOUT)
}

fn request_within(path: &Path, command: &[u8], timeout: Duration) -> Option<String> {
    request_limited(path, command, timeout, RESPONSE_LIMIT)
}

fn request_limited(path: &Path, command: &[u8], timeout: Duration, limit: u64) -> Option<String> {
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream.write_all(command).ok()?;
    stream.shutdown(Shutdown::Write).ok()?;

    let mut response = String::new();
    stream.take(limit).read_to_string(&mut response).ok()?;
    Some(response)
}

fn parse_keyboard_visible(response: &str) -> Option<bool> {
    let document: serde_json::Value = serde_json::from_str(response).ok()?;
    if !document.get("ok")?.as_bool()? {
        return None;
    }
    document
        .get("result")?
        .get("keyboard")?
        .get("visible")?
        .as_bool()
}

fn parse_volume_response(response: &str) -> Option<u8> {
    let document: serde_json::Value = serde_json::from_str(response).ok()?;
    if !document.get("ok")?.as_bool()? {
        return None;
    }
    let volume = document.get("result")?.get("volume")?.as_u64()?;
    u8::try_from(volume).ok().filter(|value| *value <= 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn parses_shell_state_volume() {
        let response = r#"{"ok":true,"result":{"view":"system","volume":65}}"#;
        assert_eq!(parse_volume_response(response), Some(65));
    }

    #[test]
    fn rejects_errors_missing_fields_and_out_of_range_values() {
        assert_eq!(
            parse_volume_response(r#"{"ok":false,"result":{"volume":50}}"#),
            None
        );
        assert_eq!(parse_volume_response(r#"{"ok":true,"result":{}}"#), None);
        assert_eq!(
            parse_volume_response(r#"{"ok":true,"result":{"volume":101}}"#),
            None
        );
        assert_eq!(parse_volume_response("not json"), None);
    }

    #[test]
    fn parses_display_and_settings() {
        let response = r#"{"ok":true,"result":{"volume":40,"display":{"brightness":42,"auto":true,"auto_offset":-6,"lux":180,"sensor":"ok"},"settings":{"version":1,"wallpaper":"dusk","auto_lock_s":60}}}"#;
        let info = parse_shell(response);
        assert_eq!(info.volume, Some(40));
        assert_eq!(
            info.display,
            Some(DisplayState {
                brightness: 42,
                auto: true,
                sensor: "ok".into()
            })
        );
        assert_eq!(info.wallpaper.as_deref(), Some("dusk"));
        assert_eq!(info.auto_lock_s, Some(60));
        // An older Moon: no display, no settings.
        let old = parse_shell(r#"{"ok":true,"result":{"volume":10}}"#);
        assert_eq!(old.display, None);
        assert_eq!(old.wallpaper, None);
    }

    #[test]
    fn parses_keyboard_visibility() {
        assert_eq!(
            parse_keyboard_visible(r#"{"ok":true,"result":{"keyboard":{"visible":true}}}"#),
            Some(true)
        );
        assert_eq!(
            parse_keyboard_visible(r#"{"ok":true,"result":{"keyboard":{"visible":false}}}"#),
            Some(false)
        );
        assert_eq!(parse_keyboard_visible(r#"{"ok":false}"#), None);
    }

    #[test]
    fn unavailable_socket_is_graceful() {
        assert_eq!(
            read_volume_from(Path::new("/a26-system-test/socket-does-not-exist")),
            None
        );
    }

    #[test]
    fn sends_state_command_and_reads_response() {
        let socket = std::env::temp_dir().join(format!(
            "a26-system-control-test-{}.sock",
            std::process::id()
        ));
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            assert_eq!(request, "state\n");
            stream
                .write_all(b"{\"ok\":true,\"result\":{\"volume\":40}}\n")
                .unwrap();
        });

        assert_eq!(read_volume_from(&socket), Some(40));
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
    }
}
