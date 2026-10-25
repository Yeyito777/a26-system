use std::error::Error;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::{Value, json};
use zeroize::Zeroizing;

const SOCKET: &str = "/run/a26-wifi/control.sock";
const RESPONSE_LIMIT: u64 = 64 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments
        .next()
        .ok_or("usage: a26-wifi-ctl status|scan|disconnect|connect-known SSID|connect-enterprise SSID DOMAIN")?;
    let request = match command.as_str() {
        "status" | "scan" | "disconnect" if arguments.next().is_none() => {
            json!({"id": 1, "command": command})
        }
        "connect-known" => {
            let ssid = arguments.next().ok_or("connect-known requires an SSID")?;
            if arguments.next().is_some() || ssid.is_empty() || ssid.len() > 32 {
                return Err("invalid SSID arguments".into());
            }
            json!({"id": 1, "command": "connect", "ssid": ssid})
        }
        "connect-enterprise" => {
            // PEAP/MSCHAPv2. The identity and password are read from stdin,
            // one per line, so neither appears in argv or the environment.
            let ssid = arguments
                .next()
                .ok_or("connect-enterprise requires an SSID")?;
            let domain = arguments
                .next()
                .ok_or("connect-enterprise requires the server domain")?;
            if arguments.next().is_some() || ssid.is_empty() || ssid.len() > 32 {
                return Err("invalid SSID arguments".into());
            }
            let mut input = Zeroizing::new(String::new());
            std::io::stdin().take(1024).read_to_string(&mut input)?;
            let mut lines = input.lines();
            let identity = Zeroizing::new(lines.next().unwrap_or_default().to_owned());
            let password = Zeroizing::new(lines.next().unwrap_or_default().to_owned());
            if identity.is_empty() || password.is_empty() {
                return Err("stdin must hold the identity and password lines".into());
            }
            json!({
                "id": 1,
                "command": "connect_enterprise",
                "ssid": ssid,
                "domain": domain,
                "identity": identity.as_str(),
                "password": password.as_str(),
            })
        }
        _ => return Err("unsupported command".into()),
    };

    let mut stream = UnixStream::connect(SOCKET)?;
    if peer_uid(&stream)? != 0 {
        return Err("Wi-Fi service is not owned by root".into());
    }
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut encoded = Zeroizing::new(serde_json::to_vec(&request)?);
    drop(request);
    encoded.push(b'\n');
    stream.write_all(&encoded)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = Vec::new();
    stream.take(RESPONSE_LIMIT + 1).read_to_end(&mut response)?;
    if response.len() as u64 > RESPONSE_LIMIT {
        return Err("response exceeded limit".into());
    }
    let document: Value = serde_json::from_slice(&response)?;
    if document.get("id").and_then(Value::as_u64) != Some(1) {
        return Err("response ID mismatch".into());
    }
    println!("{}", serde_json::to_string_pretty(&document)?);
    Ok(())
}

fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the output storage is valid for the supplied length and the
    // connected descriptor remains live for this getsockopt call.
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
