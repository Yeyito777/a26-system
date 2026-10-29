//! The composer's microphone (WP-D; PAR-243–250): dictation through Moon's
//! voice hook (OSS-Phone-V1 `docs/voice.md`, "Dictation for apps"). The
//! app runs `exocortex-voice dictate` with pipes: it records at once and
//! prints `level L` lines (0..1, every 50 ms) while recording; `stop` on its
//! stdin transcribes (the daemon's `transcribe_audio`), after which it
//! prints one `text "…"` (a JSON string) or `error MESSAGE` line and exits;
//! `cancel` (or closing stdin) drops the recording.
//!
//! This is the event loop's half (the model has no I/O): one recording at a
//! time, a thread reading the hook's lines, and an eventfd the loop polls.
//! The model asks for `Effect::Dictate(..)` and gets `Line`s back.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};

use super::line::Waker;

/// The hook, unless `EXOCORTEX_VOICE_HOOK` names another.
const HOOK: &str = "/usr/local/bin/exocortex-voice";

/// What the event loop is asked to do with the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Do {
    Start,
    /// Stop recording and transcribe.
    Stop,
    /// Drop the recording.
    Cancel,
}

/// What the hook said.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// How loud the voice is, 0..1.
    Level(f32),
    /// The transcription (empty: nothing was said).
    Text(String),
    Error(String),
    /// It exited (after its answer, or without one: cancelled, failed).
    Exited,
    /// It could not be started.
    Failed(String),
}

/// One hook line, parsed.
pub fn parse(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    match word {
        "level" => rest
            .trim()
            .parse::<f32>()
            .ok()
            .map(|l| Line::Level(l.clamp(0.0, 1.0))),
        "text" => Some(Line::Text(
            serde_json::from_str::<String>(rest.trim()).unwrap_or_else(|_| rest.trim().to_owned()),
        )),
        "error" => Some(Line::Error(rest.trim().to_owned())),
        _ => None,
    }
}

fn hook() -> PathBuf {
    std::env::var_os("EXOCORTEX_VOICE_HOOK").map_or_else(|| PathBuf::from(HOOK), PathBuf::from)
}

/// Whether dictation can run here (the hook is installed). Checked once.
pub fn available() -> bool {
    if cfg!(test) {
        return true;
    }
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(hook()).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

struct Recording {
    /// Which recording (a newer one replaces it).
    serial: u64,
    stdin: Option<ChildStdin>,
}

struct Shared {
    waker: Waker,
    lines: Mutex<Option<Receiver<Line>>>,
    recording: Mutex<Option<Recording>>,
}

fn shared() -> Option<&'static Arc<Shared>> {
    static SHARED: OnceLock<Option<Arc<Shared>>> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            Waker::new().ok().map(|waker| {
                Arc::new(Shared {
                    waker,
                    lines: Mutex::new(None),
                    recording: Mutex::new(None),
                })
            })
        })
        .as_ref()
}

/// The fd the event loop polls: readable when lines are waiting.
pub fn fd() -> Option<RawFd> {
    shared().map(|s| s.waker.fd())
}

/// Does what the model asked.
pub fn command(what: Do) {
    let Some(shared) = shared() else {
        return;
    };
    match what {
        Do::Start => start(shared),
        Do::Stop => {
            if let Some(recording) = shared
                .recording
                .lock()
                .ok()
                .as_mut()
                .and_then(|r| r.as_mut())
                && let Some(stdin) = recording.stdin.as_mut()
            {
                let _ = stdin.write_all(b"stop\n");
                let _ = stdin.flush();
            }
        }
        Do::Cancel => stop_recording(shared, true),
    }
}

/// Keeps the screen on while a recording runs and the app is in front
/// (Moon's `awake`, docs/app-services.md), on a thread: Moon answers at
/// once, but the UI never waits on it.
fn awake(on: bool) {
    if cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("exocortex-awake".into())
        .spawn(move || {
            crate::control::send(if on { "awake on" } else { "awake off" });
        });
}

fn start(shared: &'static Arc<Shared>) {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    stop_recording(shared, true);
    let spawned = Command::new(hook())
        .arg("dictate")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn();
    let (sender, receiver) = mpsc::channel();
    if let Ok(mut lines) = shared.lines.lock() {
        *lines = Some(receiver);
    }
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(Line::Failed(error.to_string()));
            shared.waker.wake();
            return;
        }
    };
    let stdout = child.stdout.take();
    let stdin = child.stdin.take();
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if let Ok(mut recording) = shared.recording.lock() {
        *recording = Some(Recording { serial, stdin });
    }
    awake(true);
    let wake = shared.clone();
    let _ = std::thread::Builder::new()
        .name("exocortex-dictate".into())
        .spawn(move || {
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Some(parsed) = parse(&line) {
                        if sender.send(parsed).is_err() {
                            return;
                        }
                        wake.waker.wake();
                    }
                }
            }
            // Reap it (it exits once it answered or its stdin closed); the
            // screen may sleep again unless a newer recording runs.
            let _ = child.wait();
            if let Ok(mut recording) = wake.recording.lock()
                && recording.as_ref().is_some_and(|r| r.serial == serial)
            {
                *recording = None;
                awake(false);
            }
            let _ = sender.send(Line::Exited);
            wake.waker.wake();
        });
}

/// Ends the recording: `cancel` and stdin closed, so the hook exits
/// without an answer; its reader thread reaps it.
fn stop_recording(shared: &Shared, cancel: bool) {
    let Ok(mut recording) = shared.recording.lock() else {
        return;
    };
    if let Some(recording) = recording.as_mut()
        && let Some(mut stdin) = recording.stdin.take()
    {
        if cancel {
            let _ = stdin.write_all(b"cancel\n");
        }
        drop(stdin);
    }
    // A cancelled one's lines are no one's any more.
    if cancel && let Ok(mut lines) = shared.lines.lock() {
        *lines = None;
    }
}

/// The hook's lines since last asked.
pub fn take() -> Vec<Line> {
    let Some(shared) = shared() else {
        return Vec::new();
    };
    shared.waker.drain();
    let Ok(lines) = shared.lines.lock() else {
        return Vec::new();
    };
    match lines.as_ref() {
        Some(receiver) => receiver.try_iter().collect(),
        None => Vec::new(),
    }
}

/// Drops any recording (the app is closing).
pub fn shutdown() {
    if let Some(shared) = shared() {
        stop_recording(shared, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_lines_parse() {
        assert_eq!(parse("level 0.42\n"), Some(Line::Level(0.42)));
        assert_eq!(parse("level 3"), Some(Line::Level(1.0)));
        assert_eq!(
            parse(r#"text "Hello, \"world\"""#),
            Some(Line::Text("Hello, \"world\"".into()))
        );
        assert_eq!(parse(r#"text """#), Some(Line::Text(String::new())));
        assert_eq!(
            parse("error The microphone recorded nothing."),
            Some(Line::Error("The microphone recorded nothing.".into()))
        );
        assert_eq!(parse("exocortex-voice: log"), None);
    }
}
