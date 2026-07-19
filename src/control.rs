use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub const CONTROL_SOCKET: &str = "/run/a26-shell/control.sock";
const RESPONSE_LIMIT: u64 = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_millis(200);

pub fn read_volume() -> Option<u8> {
    read_volume_from(Path::new(CONTROL_SOCKET))
}

fn read_volume_from(path: &Path) -> Option<u8> {
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.write_all(b"state\n").ok()?;
    stream.shutdown(Shutdown::Write).ok()?;

    let mut response = String::new();
    stream
        .take(RESPONSE_LIMIT)
        .read_to_string(&mut response)
        .ok()?;
    parse_volume_response(&response)
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
