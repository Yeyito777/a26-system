//! The app's connection to the Exocortex daemon, on its own thread: it
//! connects (and reconnects with backoff while the daemon restarts), writes
//! the UI's commands, and reads and parses events, so a multi-megabyte
//! conversation list never costs the UI thread a frame. The thread sleeps in
//! `poll(2)`; the UI is woken through an eventfd it polls next to X.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use super::line::{self, LineSocket, Waker};
use super::proto::{self, Event};

#[derive(Debug)]
pub enum LinkEvent {
    Connected,
    Disconnected,
    Event(Box<Event>),
}

enum LinkCommand {
    Send(String),
    /// Drop the connection and connect again now.
    Reconnect,
}

pub struct DaemonLink {
    commands: Sender<LinkCommand>,
    events: Receiver<LinkEvent>,
    worker: Arc<Waker>,
    ui: Arc<Waker>,
}

const BACKOFF: [u64; 6] = [100, 250, 500, 1000, 2000, 4000];

impl DaemonLink {
    pub fn spawn(path: PathBuf) -> std::io::Result<Self> {
        let worker = Arc::new(Waker::new()?);
        let ui = Arc::new(Waker::new()?);
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (worker_wake, ui_wake) = (worker.clone(), ui.clone());
        thread::Builder::new()
            .name("exocortex-link".into())
            .spawn(move || run(&path, &command_rx, &event_tx, &worker_wake, &ui_wake))?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
            worker,
            ui,
        })
    }

    pub fn send(&self, command: serde_json::Value) {
        if self
            .commands
            .send(LinkCommand::Send(command.to_string()))
            .is_ok()
        {
            self.worker.wake();
        }
    }

    pub fn reconnect(&self) {
        if self.commands.send(LinkCommand::Reconnect).is_ok() {
            self.worker.wake();
        }
    }

    /// The fd that becomes readable when events wait.
    pub fn fd(&self) -> std::os::fd::RawFd {
        self.ui.fd()
    }

    /// Takes every waiting event (and clears the wake-up).
    pub fn drain(&self) -> Vec<LinkEvent> {
        self.ui.drain();
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }
}

fn run(
    path: &std::path::Path,
    commands: &Receiver<LinkCommand>,
    events: &Sender<LinkEvent>,
    worker: &Waker,
    ui: &Waker,
) {
    let mut attempt = 0;
    loop {
        if let Ok(socket) = LineSocket::connect(path) {
            if events.send(LinkEvent::Connected).is_err() {
                return;
            }
            ui.wake();
            let started = Instant::now();
            let outcome = serve(socket, commands, events, worker, ui);
            if events.send(LinkEvent::Disconnected).is_err() {
                return;
            }
            ui.wake();
            match outcome {
                Outcome::Quit => return,
                Outcome::Reconnect => attempt = 0,
                Outcome::Lost if started.elapsed() > Duration::from_secs(5) => attempt = 0,
                Outcome::Lost => {}
            }
        }
        // Wait out the backoff; a Reconnect cuts it short. Sends made while
        // disconnected are dropped: the UI bootstraps again on Connected.
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
                    Ok(LinkCommand::Send(_)) => {}
                    Ok(LinkCommand::Reconnect) => skip = true,
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

enum Outcome {
    Lost,
    Reconnect,
    Quit,
}

fn serve(
    mut socket: LineSocket,
    commands: &Receiver<LinkCommand>,
    events: &Sender<LinkEvent>,
    worker: &Waker,
    ui: &Waker,
) -> Outcome {
    loop {
        let mut fds = [
            line::pollfd(socket.fd(), socket.interest()),
            line::pollfd(worker.fd(), libc::POLLIN),
        ];
        if line::poll(&mut fds, None).is_err() {
            return Outcome::Lost;
        }
        if fds[1].revents != 0 {
            worker.drain();
            loop {
                match commands.try_recv() {
                    Ok(LinkCommand::Send(text)) => {
                        if socket.send(&text).is_err() {
                            return Outcome::Lost;
                        }
                    }
                    Ok(LinkCommand::Reconnect) => return Outcome::Reconnect,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Outcome::Quit,
                }
            }
        }
        if fds[0].revents & libc::POLLOUT != 0 && socket.flush().is_err() {
            return Outcome::Lost;
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut any = false;
            let mut quit = false;
            let result = socket.read_lines(|text| {
                let event = proto::parse_event(text);
                if event != Event::Other {
                    any = true;
                    quit |= events.send(LinkEvent::Event(Box::new(event))).is_err();
                }
            });
            if any {
                ui.wake();
            }
            if quit {
                return Outcome::Quit;
            }
            if result.is_err() {
                return Outcome::Lost;
            }
        }
    }
}
