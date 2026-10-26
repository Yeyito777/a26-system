//! The Phone and Messages apps' connection to `moon-telephonyd`
//! (OSS-Phone-V1 docs/telephony-protocol.md, revision 1.2): JSON lines over
//! a Unix socket, on a worker thread so the UI never waits on it. Moon
//! itself shows calls (its call screen); these apps read the SIM state, the
//! call history and messages, and place calls the user dialed.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

pub const TELEPHONY_SOCKET: &str = "/run/moon-telephony/telephony.sock";

/// A command and its arguments.
pub type TelRequest = (&'static str, Value);

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Sim {
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Network {
    #[serde(default)]
    pub registration: String,
    #[serde(default)]
    pub emergency_only: bool,
}

/// The parts of the daemon's State object these apps use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TelState {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub airplane: bool,
    #[serde(default)]
    pub sim: Sim,
    #[serde(default)]
    pub network: Network,
}

/// Why calls and texts cannot go out right now, if they cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Ready,
    /// No daemon, or it has no modem.
    Unavailable,
    Airplane,
    NoSim,
    /// A SIM that needs its PIN or PUK, or is locked to another network.
    SimLocked,
    /// No network; `emergency` when emergency calls are possible.
    NoNetwork {
        emergency: bool,
    },
}

impl TelState {
    pub fn service(&self, connected: bool) -> Service {
        if !connected || !self.available {
            return Service::Unavailable;
        }
        if self.airplane {
            return Service::Airplane;
        }
        match self.sim.state.as_str() {
            "ready" => {}
            "pin_required" | "puk_required" | "network_locked" => return Service::SimLocked,
            _ => return Service::NoSim,
        }
        match self.network.registration.as_str() {
            "home" | "roaming" => Service::Ready,
            _ => Service::NoNetwork {
                emergency: self.network.emergency_only,
            },
        }
    }
}

/// One ended call (revision 1.1's call record).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CallRecord {
    pub id: String,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub duration_s: u64,
    #[serde(default)]
    pub missed: bool,
    #[serde(default)]
    pub rejected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Message {
    pub id: String,
    pub peer: String,
    pub direction: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub time: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub read: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Thread {
    pub peer: String,
    pub last: Message,
    #[serde(default)]
    pub unread: u32,
}

/// What the worker reports to the UI.
#[derive(Debug, Clone)]
pub enum TelUpdate {
    Connected(bool),
    Event(Value),
    /// A command's result, by command name.
    Reply(&'static str, Value),
    Failed(&'static str, String),
}

pub struct TelWorker {
    requests: Sender<TelRequest>,
    updates: Receiver<TelUpdate>,
}

impl TelWorker {
    pub fn spawn() -> Self {
        let socket = std::env::var_os("MOON_TELEPHONY_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(TELEPHONY_SOCKET));
        let (request_tx, request_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        thread::spawn(move || worker_loop(&socket, request_rx, update_tx));
        Self {
            requests: request_tx,
            updates: update_rx,
        }
    }

    pub fn send(&self, request: TelRequest) {
        let _ = self.requests.send(request);
    }

    pub fn try_recv(&self) -> Option<TelUpdate> {
        self.updates.try_recv().ok()
    }
}

fn worker_loop(socket: &Path, requests: Receiver<TelRequest>, updates: Sender<TelUpdate>) {
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            if serve(stream, &requests, &updates).is_err() {
                return;
            }
            if updates.send(TelUpdate::Connected(false)).is_err() {
                return;
            }
        }
        // Commands made while the daemon is away cannot be applied.
        loop {
            match requests.try_recv() {
                Ok((cmd, _)) => {
                    let failed = TelUpdate::Failed(cmd, "unavailable".into());
                    if updates.send(failed).is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Requests written on a connection, awaiting their answers.
#[derive(Default)]
struct Outbox {
    next_id: u64,
    pending: Vec<(u64, &'static str)>,
}

impl Outbox {
    fn send(
        &mut self,
        writer: &mut UnixStream,
        cmd: &'static str,
        mut arguments: Value,
    ) -> std::io::Result<()> {
        self.next_id += 1;
        if !arguments.is_object() {
            arguments = json!({});
        }
        arguments["id"] = json!(self.next_id);
        arguments["cmd"] = json!(cmd);
        self.pending.push((self.next_id, cmd));
        writer.write_all(format!("{arguments}\n").as_bytes())
    }
}

/// One connection's life. Returns Err when the UI has gone away.
fn serve(
    stream: UnixStream,
    requests: &Receiver<TelRequest>,
    updates: &Sender<TelUpdate>,
) -> Result<(), ()> {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let Ok(mut writer) = stream.try_clone() else {
        return Ok(());
    };
    let mut reader = BufReader::new(stream);
    let mut outbox = Outbox::default();
    if outbox.send(&mut writer, "subscribe", json!({})).is_err() {
        return Ok(());
    }
    updates.send(TelUpdate::Connected(true)).map_err(|_| ())?;
    let mut line = String::new();
    loop {
        loop {
            match requests.try_recv() {
                Ok((cmd, arguments)) => {
                    if outbox.send(&mut writer, cmd, arguments).is_err() {
                        return Ok(());
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(()),
            }
        }
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                if !line.ends_with('\n') {
                    continue;
                }
                let value: Option<Value> = serde_json::from_str(line.trim_end()).ok();
                line.clear();
                let Some(mut value) = value else { continue };
                if value.get("event").is_some() {
                    updates.send(TelUpdate::Event(value)).map_err(|_| ())?;
                } else if let Some(id) = value.get("id").and_then(Value::as_u64)
                    && let Some(position) = outbox.pending.iter().position(|(p, _)| *p == id)
                {
                    let (_, cmd) = outbox.pending.remove(position);
                    let update = if value.get("ok").and_then(Value::as_bool) == Some(true) {
                        TelUpdate::Reply(cmd, value["result"].take())
                    } else {
                        let code = value
                            .pointer("/error/code")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        TelUpdate::Failed(cmd, code.to_owned())
                    };
                    updates.send(update).map_err(|_| ())?;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return Ok(()),
        }
    }
}

/// "+16475551234" as "+1 (647) 555-1234"; other numbers as they are.
pub fn format_number(number: &str) -> String {
    let digits: String = number.chars().filter(char::is_ascii_digit).collect();
    let nanp = |d: &str| format!("({}) {}-{}", &d[..3], &d[3..6], &d[6..]);
    if number.starts_with("+1") && digits.len() == 11 {
        format!("+1 {}", nanp(&digits[1..]))
    } else if !number.starts_with('+') && digits.len() == 10 && digits == number {
        nanp(&digits)
    } else {
        number.to_owned()
    }
}

/// Digits typed on the keypad, grouped as they are typed (NANP), as on
/// iPhone: "647555" reads "647-555", "6475551234" "(647) 555-1234".
pub fn format_typed(digits: &str) -> String {
    let plain = digits.chars().all(|c| c.is_ascii_digit());
    let (prefix, rest) = match digits.strip_prefix('1') {
        Some(rest) if plain && digits.len() > 1 => ("1 ", rest),
        _ => ("", digits),
    };
    if !plain || rest.len() > 10 || rest.len() < 4 {
        return digits.to_owned();
    }
    let text = match rest.len() {
        4..=7 => format!("{}-{}", &rest[..3], &rest[3..]),
        _ => format!("({}) {}-{}", &rest[..3], &rest[3..6], &rest[6..]),
    };
    format!("{prefix}{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_reads_the_sim_and_network() {
        let mut state: TelState = serde_json::from_value(json!({
            "available": true, "airplane": false,
            "sim": {"state": "absent", "slots": []},
            "network": {"registration": "none", "emergency_only": true}
        }))
        .unwrap();
        assert_eq!(state.service(false), Service::Unavailable);
        assert_eq!(state.service(true), Service::NoSim);
        state.sim.state = "ready".into();
        assert_eq!(state.service(true), Service::NoNetwork { emergency: true });
        state.network.registration = "home".into();
        assert_eq!(state.service(true), Service::Ready);
        state.airplane = true;
        assert_eq!(state.service(true), Service::Airplane);
    }

    #[test]
    fn numbers_read_like_ios() {
        assert_eq!(format_number("+16475551234"), "+1 (647) 555-1234");
        assert_eq!(format_number("32665"), "32665");
        assert_eq!(format_typed("647"), "647");
        assert_eq!(format_typed("6475"), "647-5");
        assert_eq!(format_typed("6475551234"), "(647) 555-1234");
        assert_eq!(format_typed("16475551234"), "1 (647) 555-1234");
        assert_eq!(format_typed("*#06#"), "*#06#");
    }
}
