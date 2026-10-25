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

pub fn read_volume() -> Option<u8> {
    read_volume_from(&socket())
}

fn read_volume_from(path: &Path) -> Option<u8> {
    parse_volume_response(&request(path, b"state\n")?)
}

/// Asks Moon to show its system keyboard for password entry. Returns whether
/// the keyboard is now visible; Moon refuses while locked or launching.
pub fn show_password_keyboard() -> bool {
    request(&socket(), b"keyboard show password\n")
        .as_deref()
        .and_then(parse_keyboard_visible)
        .unwrap_or(false)
}

pub fn hide_keyboard() {
    let _ = request(&socket(), b"keyboard hide\n");
}

fn request(path: &Path, command: &[u8]) -> Option<String> {
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.write_all(command).ok()?;
    stream.shutdown(Shutdown::Write).ok()?;

    let mut response = String::new();
    stream
        .take(RESPONSE_LIMIT)
        .read_to_string(&mut response)
        .ok()?;
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
