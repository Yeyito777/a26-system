//! What the Exocortex app and the `exocortex-notify` bridge tell each
//! other, without talking: the app writes which conversation it shows to a
//! small state file; the bridge, about to notify, reads it and asks Moon
//! whether the app is in front (`view`) on an awake, unlocked screen. A
//! frozen app's stale file is harmless, because Moon then says another view.
//! Notifications themselves are `moon-notifyd`'s (docs/notifications-protocol.md).
#![allow(dead_code)]

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

/// The app id in Moon (and in notifications).
pub const APP_ID: &str = "exocortex";
pub const STATE_FILE: &str = "/run/exocortex-app/view.json";
pub const MOON_SOCKET: &str = "/run/a26-shell/control.sock";
pub const NOTIFY_SOCKET: &str = "/run/moon-notify/notify.sock";

pub fn state_path() -> PathBuf {
    std::env::var_os("EXOCORTEX_APP_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(STATE_FILE))
}

pub fn moon_socket() -> PathBuf {
    std::env::var_os("A26_SHELL_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(MOON_SOCKET))
}

pub fn notify_socket() -> PathBuf {
    std::env::var_os("MOON_NOTIFY_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(NOTIFY_SOCKET))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppView {
    pub pid: u32,
    pub conversation: Option<String>,
}

/// Records the conversation the app shows (None: the list), atomically.
pub fn write_view(conversation: Option<&str>) -> io::Result<()> {
    let path = state_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = json!({ "pid": std::process::id(), "conversation": conversation });
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temporary, format!("{body}\n"))?;
    std::fs::rename(&temporary, &path)
}

pub fn clear_view() {
    let path = state_path();
    if read_view().is_some_and(|view| view.pid == std::process::id()) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn read_view() -> Option<AppView> {
    let text = std::fs::read_to_string(state_path()).ok()?;
    parse_view(&text)
}

fn parse_view(text: &str) -> Option<AppView> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some(AppView {
        pid: u32::try_from(value.get("pid")?.as_u64()?).ok()?,
        conversation: value
            .get("conversation")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
}

/// One request on Moon's control socket (one command per connection).
pub fn moon_request(command: &str, timeout: Duration) -> Option<Value> {
    let mut stream = UnixStream::connect(moon_socket()).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream.write_all(format!("{command}\n").as_bytes()).ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    let mut response = String::new();
    stream
        .take(4 * 1024 * 1024)
        .read_to_string(&mut response)
        .ok()?;
    serde_json::from_str(&response).ok()
}

/// Moon shows `app` in front of an awake, unlocked screen. `None` when Moon
/// cannot be asked (Android mode, or Moon restarting).
pub fn moon_shows(app: &str) -> Option<bool> {
    let state = moon_request("state", Duration::from_millis(400))?;
    Some(shows_in(&state, app))
}

fn shows_in(state: &Value, app: &str) -> bool {
    let result = &state["result"];
    result["view"].as_str() == Some(app)
        && result["screen_awake"].as_bool().unwrap_or(false)
        && !result["locked"].as_bool().unwrap_or(true)
}

/// The app is in front, showing `conversation`, right now.
pub fn app_showing(conversation: &str) -> bool {
    read_view().is_some_and(|view| {
        view.conversation.as_deref() == Some(conversation) && pid_alive(view.pid)
    }) && moon_shows(APP_ID) == Some(true)
}

/// The notification tag of a conversation: its id when moon-notifyd accepts
/// it as a tag (`[A-Za-z0-9._-]{1,64}`), else a stable hash of it.
pub fn tag_for(conversation: &str) -> String {
    let valid = (1..=64).contains(&conversation.len())
        && conversation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        return conversation.to_owned();
    }
    // FNV-1a: stable across builds, unlike the std hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in conversation.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("c{hash:016x}")
}

/// Withdraws a conversation's notification (the user opened it). Blocking,
/// at most about a second: callers run it off their UI thread.
pub fn withdraw(conversation: &str) -> io::Result<()> {
    let stream = UnixStream::connect(notify_socket())?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let request =
        json!({ "id": 1, "cmd": "notify.withdraw", "app": APP_ID, "tag": tag_for(conversation) });
    (&stream).write_all(format!("{request}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_ids_or_their_hash() {
        assert_eq!(tag_for("1793171213972-ab12cd"), "1793171213972-ab12cd");
        let odd = tag_for("conv with spaces/and slashes");
        assert!(odd.starts_with('c') && odd.len() == 17);
        assert_eq!(odd, tag_for("conv with spaces/and slashes"));
        assert_ne!(tag_for(&"x".repeat(65)), "x".repeat(65));
    }

    #[test]
    fn the_view_file_round_trips_and_moon_state_is_read() {
        assert_eq!(
            parse_view(r#"{"pid":12,"conversation":"c1"}"#),
            Some(AppView {
                pid: 12,
                conversation: Some("c1".into())
            })
        );
        assert_eq!(
            parse_view(r#"{"pid":12,"conversation":null}"#)
                .unwrap()
                .conversation,
            None
        );
        assert_eq!(parse_view("{}"), None);
        let state = json!({"ok": true, "result": {"view": "exocortex", "screen_awake": true, "locked": false}});
        assert!(shows_in(&state, "exocortex"));
        let locked = json!({"ok": true, "result": {"view": "exocortex", "screen_awake": true, "locked": true}});
        assert!(!shows_in(&locked, "exocortex"));
        let other = json!({"ok": true, "result": {"view": "launcher", "screen_awake": true, "locked": false}});
        assert!(!shows_in(&other, "exocortex"));
        assert!(pid_alive(std::process::id()));
    }
}
