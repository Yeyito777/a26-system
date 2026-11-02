//! An app's connection to its bridge, on a thread of its own (as the
//! Exocortex app's `link.rs`): it connects, says `hello` and `subscribe`s,
//! reconnects with backoff while the bridge restarts, writes the UI's
//! requests and reads and parses what comes back, so a long history page
//! never costs the UI thread a frame. The UI is woken through an eventfd it
//! polls beside its display.
//!
//! Every request gets exactly one answer: the bridge's, or `disconnected`
//! when the connection drops first (or was down when it was made).

#![allow(dead_code)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::proto::{self, BridgeError, Event, Hello, Line};
use crate::exocortex::line::{self, LineSocket, Waker};

#[derive(Debug)]
pub enum LinkEvent {
    /// Connected and greeted: what `hello` said.
    Connected(Box<Hello>),
    Disconnected,
    Answer {
        id: u64,
        result: Result<Value, BridgeError>,
    },
    Event(Box<Event>),
}

enum Command {
    Send(u64, String),
    Reconnect,
}

pub struct Link {
    commands: Sender<Command>,
    events: Receiver<LinkEvent>,
    worker: Arc<Waker>,
    ui: Arc<Waker>,
    next_id: AtomicU64,
}

const BACKOFF: [u64; 7] = [100, 250, 500, 1000, 2000, 4000, 8000];
/// The link's own requests (`hello`, `subscribe`) use ids below this.
const FIRST_ID: u64 = 16;
const HELLO: u64 = 1;
const SUBSCRIBE: u64 = 2;

impl Link {
    /// `client` names the app in `hello` (the bridge's log).
    pub fn spawn(path: PathBuf, client: &'static str) -> std::io::Result<Self> {
        let worker = Arc::new(Waker::new()?);
        let ui = Arc::new(Waker::new()?);
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (worker_wake, ui_wake) = (worker.clone(), ui.clone());
        thread::Builder::new()
            .name("social-link".into())
            .spawn(move || {
                run(
                    &path,
                    client,
                    &command_rx,
                    &event_tx,
                    &worker_wake,
                    &ui_wake,
                )
            })?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
            worker,
            ui,
            next_id: AtomicU64::new(FIRST_ID),
        })
    }

    /// Sends `cmd` with `args` (an object); returns the request's id, which
    /// its answer carries.
    pub fn request(&self, cmd: &str, args: Value) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut request = match args {
            Value::Object(map) => Value::Object(map),
            _ => json!({}),
        };
        request["id"] = json!(id);
        request["cmd"] = json!(cmd);
        if self
            .commands
            .send(Command::Send(id, request.to_string()))
            .is_ok()
        {
            self.worker.wake();
        }
        id
    }

    pub fn reconnect(&self) {
        if self.commands.send(Command::Reconnect).is_ok() {
            self.worker.wake();
        }
    }

    /// The fd that becomes readable when something waits.
    pub fn fd(&self) -> std::os::fd::RawFd {
        self.ui.fd()
    }

    /// Takes everything waiting (and clears the wake-up).
    pub fn drain(&self) -> Vec<LinkEvent> {
        self.ui.drain();
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }
}

fn disconnected() -> BridgeError {
    BridgeError::local("disconnected", "the bridge is not connected")
}

fn run(
    path: &std::path::Path,
    client: &str,
    commands: &Receiver<Command>,
    events: &Sender<LinkEvent>,
    worker: &Waker,
    ui: &Waker,
) {
    let mut attempt = 0;
    loop {
        if let Ok(mut socket) = LineSocket::connect(path) {
            let hello = json!({"id": HELLO, "cmd": "hello", "client": client, "protocol": 1});
            let subscribe = json!({"id": SUBSCRIBE, "cmd": "subscribe"});
            let greeted = socket.send(&hello.to_string()).is_ok()
                && socket.send(&subscribe.to_string()).is_ok();
            let started = Instant::now();
            let outcome = if greeted {
                serve(socket, commands, events, worker, ui)
            } else {
                Outcome::Lost(HashSet::new(), false)
            };
            let (outcome, pending, greeted) = match outcome {
                Outcome::Lost(pending, greeted) => (Next::Lost, pending, greeted),
                Outcome::Reconnect(pending, greeted) => (Next::Reconnect, pending, greeted),
                Outcome::Quit => return,
            };
            for id in pending {
                if events
                    .send(LinkEvent::Answer {
                        id,
                        result: Err(disconnected()),
                    })
                    .is_err()
                {
                    return;
                }
            }
            if greeted && events.send(LinkEvent::Disconnected).is_err() {
                return;
            }
            ui.wake();
            match outcome {
                Next::Reconnect => {
                    attempt = 0;
                    continue;
                }
                Next::Lost if started.elapsed() > Duration::from_secs(5) => attempt = 0,
                Next::Lost => {}
            }
        }
        // Wait out the backoff; a Reconnect cuts it short. Requests made
        // meanwhile are answered `disconnected` at once.
        let deadline =
            Instant::now() + Duration::from_millis(BACKOFF[attempt.min(BACKOFF.len() - 1)]);
        attempt += 1;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let mut fds = [line::pollfd(worker.fd(), libc::POLLIN)];
            if line::poll(&mut fds, Some(deadline - now)).is_err() {
                thread::sleep(Duration::from_millis(100));
            }
            worker.drain();
            let mut skip = false;
            loop {
                match commands.try_recv() {
                    Ok(Command::Send(id, _)) => {
                        let answer = LinkEvent::Answer {
                            id,
                            result: Err(disconnected()),
                        };
                        if events.send(answer).is_err() {
                            return;
                        }
                        ui.wake();
                    }
                    Ok(Command::Reconnect) => skip = true,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            if skip {
                attempt = 0;
                break;
            }
        }
    }
}

enum Next {
    Lost,
    Reconnect,
}

/// How a connection ended, with the requests it left unanswered and
/// whether it had been greeted (the UI was told it connected).
enum Outcome {
    Lost(HashSet<u64>, bool),
    Reconnect(HashSet<u64>, bool),
    Quit,
}

fn serve(
    mut socket: LineSocket,
    commands: &Receiver<Command>,
    events: &Sender<LinkEvent>,
    worker: &Waker,
    ui: &Waker,
) -> Outcome {
    let mut pending: HashSet<u64> = HashSet::new();
    let mut greeted = false;
    loop {
        let mut fds = [
            line::pollfd(socket.fd(), socket.interest()),
            line::pollfd(worker.fd(), libc::POLLIN),
        ];
        if line::poll(&mut fds, None).is_err() {
            return Outcome::Lost(pending, greeted);
        }
        if fds[1].revents != 0 {
            worker.drain();
            loop {
                match commands.try_recv() {
                    Ok(Command::Send(id, text)) => {
                        pending.insert(id);
                        if socket.send(&text).is_err() {
                            return Outcome::Lost(pending, greeted);
                        }
                    }
                    Ok(Command::Reconnect) => return Outcome::Reconnect(pending, greeted),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Outcome::Quit,
                }
            }
        }
        if fds[0].revents & libc::POLLOUT != 0 && socket.flush().is_err() {
            return Outcome::Lost(pending, greeted);
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut any = false;
            let mut quit = false;
            let result = socket.read_lines(|text| {
                let Some(line) = proto::parse_line(text) else {
                    return;
                };
                let event = match line {
                    Line::Answer { id: HELLO, result } => {
                        greeted = true;
                        let hello = result
                            .ok()
                            .and_then(|value| serde_json::from_value::<Hello>(value).ok())
                            .unwrap_or_default();
                        LinkEvent::Connected(Box::new(hello))
                    }
                    Line::Answer { id, .. } if id < FIRST_ID => return,
                    Line::Answer { id, result } => {
                        if !pending.remove(&id) {
                            return;
                        }
                        LinkEvent::Answer { id, result }
                    }
                    Line::Event(event) => LinkEvent::Event(event),
                };
                any = true;
                quit |= events.send(event).is_err();
            });
            if any {
                ui.wake();
            }
            if quit {
                return Outcome::Quit;
            }
            if result.is_err() {
                return Outcome::Lost(pending, greeted);
            }
        }
    }
}
