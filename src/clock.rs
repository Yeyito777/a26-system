//! Alarms and timers: the System app's Clock page, a client of
//! `moon-notifyd` (OSS-Phone-V1 docs/notifications-protocol.md, revision 2).
//! The daemon owns every alarm and timer; this page shows its state and
//! sends the user's edits as `alarm.*` / `timer.*` commands. Moon shows
//! the ringing screen; nothing here needs to run for an alarm to ring.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

pub const NOTIFY_SOCKET: &str = "/run/moon-notify/notify.sock";
/// Alarms shown on the Clock page; the protocol allows 32.
pub const MAX_ALARM_ROWS: usize = 5;
pub const MAX_TIMER_ROWS: usize = 3;
pub const TIMER_PRESETS: [u32; 6] = [60, 180, 300, 600, 900, 1800];
const MAX_LABEL: usize = 40;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Alarm {
    pub id: String,
    pub hour: u8,
    pub minute: u8,
    #[serde(default)]
    pub days: Vec<u8>,
    #[serde(default)]
    pub label: String,
    pub enabled: bool,
    #[serde(default)]
    pub next_fire_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Timer {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub duration_s: u32,
    pub state: String,
    #[serde(default)]
    pub ends_at: Option<i64>,
    #[serde(default)]
    pub remaining_s: u32,
}

impl Timer {
    /// Seconds left now (a running timer counts down locally from
    /// `ends_at`; the daemon sends no ticks).
    pub fn left(&self, now: i64) -> u32 {
        match (self.state.as_str(), self.ends_at) {
            ("running", Some(ends_at)) => (ends_at - now).max(0) as u32,
            ("ringing", _) => 0,
            _ => self.remaining_s,
        }
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// "7:30" and "AM"/"PM".
pub fn alarm_time(hour: u8, minute: u8) -> (String, &'static str) {
    let display = match hour % 12 {
        0 => 12,
        h => h,
    };
    (
        format!("{display}:{minute:02}"),
        if hour < 12 { "AM" } else { "PM" },
    )
}

/// "Every day", "Weekdays", "Weekends", "Mon, Wed", or "" for one-shot.
pub fn repeat_text(days: &[u8]) -> String {
    const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let mut sorted: Vec<u8> = days.iter().copied().filter(|d| *d < 7).collect();
    sorted.sort_unstable();
    sorted.dedup();
    match sorted.as_slice() {
        [] => String::new(),
        [0, 1, 2, 3, 4, 5, 6] => "Every day".into(),
        [1, 2, 3, 4, 5] => "Weekdays".into(),
        [0, 6] => "Weekends".into(),
        days => days
            .iter()
            .map(|d| NAMES[usize::from(*d)])
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// "9:05" or "1:02:03".
pub fn countdown(seconds: u32) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn preset_label(seconds: u32) -> String {
    format!("{} min", seconds / 60)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorField {
    Hour,
    Minute,
    Second,
}

/// The add/edit alarm page.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmEditor {
    /// The alarm being edited; None for a new one.
    pub id: Option<String>,
    pub hour: u8,
    pub minute: u8,
    pub days: [bool; 7],
    pub label: String,
    pub label_focused: bool,
    pub system_keyboard: bool,
}

/// The custom-timer page.
#[derive(Debug, Clone, PartialEq)]
pub struct TimerEditor {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub label: String,
    pub label_focused: bool,
    pub system_keyboard: bool,
}

impl TimerEditor {
    pub fn duration(&self) -> u32 {
        u32::from(self.hours) * 3600 + u32::from(self.minutes) * 60 + u32::from(self.seconds)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockAction {
    NewAlarm,
    EditAlarm(usize),
    ToggleAlarm(usize),
    /// Hour or minute (or second, on the timer page) up (+1) or down (-1).
    Step(EditorField, i8),
    QuickMinute(u8),
    ToggleDay(u8),
    FocusLabel,
    Save,
    Cancel,
    DeleteAlarm,
    StartPreset(usize),
    NewTimer,
    PauseOrResume(usize),
    CancelTimer(usize),
}

/// A command for the daemon: `cmd` and its arguments.
pub type NotifyRequest = (&'static str, Value);

#[derive(Debug, Default)]
pub struct ClockModel {
    pub alarms: Vec<Alarm>,
    pub timers: Vec<Timer>,
    pub connected: bool,
    pub alarm_editor: Option<AlarmEditor>,
    pub timer_editor: Option<TimerEditor>,
    pub notice: Option<&'static str>,
}

impl ClockModel {
    pub fn label_focused(&self) -> bool {
        self.alarm_editor.as_ref().is_some_and(|e| e.label_focused)
            || self.timer_editor.as_ref().is_some_and(|e| e.label_focused)
    }

    pub fn set_system_keyboard(&mut self, shown: bool) {
        if let Some(editor) = self.alarm_editor.as_mut() {
            editor.system_keyboard = shown;
        }
        if let Some(editor) = self.timer_editor.as_mut() {
            editor.system_keyboard = shown;
        }
    }

    pub fn handle(&mut self, action: ClockAction) -> Vec<NotifyRequest> {
        self.notice = None;
        match action {
            ClockAction::NewAlarm => {
                if !self.connected {
                    self.notice = Some("Clock service unavailable");
                    return Vec::new();
                }
                self.alarm_editor = Some(AlarmEditor {
                    id: None,
                    hour: 7,
                    minute: 0,
                    days: [false; 7],
                    label: String::new(),
                    label_focused: false,
                    system_keyboard: false,
                });
                Vec::new()
            }
            ClockAction::EditAlarm(index) => {
                if let Some(alarm) = self.alarms.get(index) {
                    let mut days = [false; 7];
                    for day in &alarm.days {
                        if let Some(slot) = days.get_mut(usize::from(*day)) {
                            *slot = true;
                        }
                    }
                    self.alarm_editor = Some(AlarmEditor {
                        id: Some(alarm.id.clone()),
                        hour: alarm.hour,
                        minute: alarm.minute,
                        days,
                        label: alarm.label.clone(),
                        label_focused: false,
                        system_keyboard: false,
                    });
                }
                Vec::new()
            }
            ClockAction::ToggleAlarm(index) => {
                let Some(alarm) = self.alarms.get_mut(index) else {
                    return Vec::new();
                };
                alarm.enabled = !alarm.enabled;
                vec![(
                    "alarm.update",
                    json!({"alarm": alarm.id, "enabled": alarm.enabled}),
                )]
            }
            ClockAction::Step(field, delta) => {
                if let Some(editor) = self.alarm_editor.as_mut() {
                    editor.label_focused = false;
                    match field {
                        EditorField::Hour => editor.hour = wrap(editor.hour, delta, 24),
                        EditorField::Minute => editor.minute = wrap(editor.minute, delta, 60),
                        EditorField::Second => {}
                    }
                } else if let Some(editor) = self.timer_editor.as_mut() {
                    editor.label_focused = false;
                    match field {
                        EditorField::Hour => editor.hours = wrap(editor.hours, delta, 24),
                        EditorField::Minute => editor.minutes = wrap(editor.minutes, delta, 60),
                        EditorField::Second => editor.seconds = wrap(editor.seconds, delta, 60),
                    }
                }
                Vec::new()
            }
            ClockAction::QuickMinute(minute) => {
                if let Some(editor) = self.alarm_editor.as_mut() {
                    editor.minute = minute % 60;
                    editor.label_focused = false;
                }
                Vec::new()
            }
            ClockAction::ToggleDay(day) => {
                if let Some(editor) = self.alarm_editor.as_mut()
                    && let Some(slot) = editor.days.get_mut(usize::from(day))
                {
                    *slot = !*slot;
                    editor.label_focused = false;
                }
                Vec::new()
            }
            ClockAction::FocusLabel => {
                if let Some(editor) = self.alarm_editor.as_mut() {
                    editor.label_focused = true;
                }
                if let Some(editor) = self.timer_editor.as_mut() {
                    editor.label_focused = true;
                }
                Vec::new()
            }
            ClockAction::Cancel => {
                self.alarm_editor = None;
                self.timer_editor = None;
                Vec::new()
            }
            ClockAction::Save => {
                if let Some(editor) = self.alarm_editor.take() {
                    let days: Vec<u8> =
                        (0..7_u8).filter(|d| editor.days[usize::from(*d)]).collect();
                    let label = editor.label.trim().to_owned();
                    return vec![match editor.id {
                        Some(id) => (
                            "alarm.update",
                            json!({"alarm": id, "hour": editor.hour, "minute": editor.minute,
                                   "days": days, "label": label, "enabled": true}),
                        ),
                        None => (
                            "alarm.create",
                            json!({"hour": editor.hour, "minute": editor.minute,
                                   "days": days, "label": label}),
                        ),
                    }];
                }
                if let Some(editor) = self.timer_editor.take() {
                    let duration = editor.duration();
                    if duration == 0 {
                        self.timer_editor = Some(editor);
                        return Vec::new();
                    }
                    return vec![(
                        "timer.create",
                        json!({"duration_s": duration, "label": editor.label.trim()}),
                    )];
                }
                Vec::new()
            }
            ClockAction::DeleteAlarm => {
                let Some(id) = self.alarm_editor.take().and_then(|editor| editor.id) else {
                    return Vec::new();
                };
                self.alarms.retain(|alarm| alarm.id != id);
                vec![("alarm.delete", json!({ "alarm": id }))]
            }
            ClockAction::StartPreset(index) => {
                if !self.connected {
                    self.notice = Some("Clock service unavailable");
                    return Vec::new();
                }
                match TIMER_PRESETS.get(index) {
                    Some(seconds) => vec![("timer.create", json!({ "duration_s": seconds }))],
                    None => Vec::new(),
                }
            }
            ClockAction::NewTimer => {
                if !self.connected {
                    self.notice = Some("Clock service unavailable");
                    return Vec::new();
                }
                self.timer_editor = Some(TimerEditor {
                    hours: 0,
                    minutes: 5,
                    seconds: 0,
                    label: String::new(),
                    label_focused: false,
                    system_keyboard: false,
                });
                Vec::new()
            }
            ClockAction::PauseOrResume(index) => match self.timers.get(index) {
                Some(timer) if timer.state == "running" => {
                    vec![("timer.pause", json!({ "timer": timer.id }))]
                }
                Some(timer) if timer.state == "paused" => {
                    vec![("timer.resume", json!({ "timer": timer.id }))]
                }
                _ => Vec::new(),
            },
            ClockAction::CancelTimer(index) => {
                let Some(timer) = self.timers.get(index).cloned() else {
                    return Vec::new();
                };
                self.timers.remove(index);
                vec![("timer.cancel", json!({ "timer": timer.id }))]
            }
        }
    }

    /// Typed text for the focused label.
    pub fn type_text(&mut self, input: crate::keys::KeyInput) {
        let label = match (self.alarm_editor.as_mut(), self.timer_editor.as_mut()) {
            (Some(editor), _) if editor.label_focused => &mut editor.label,
            (_, Some(editor)) if editor.label_focused => &mut editor.label,
            _ => return,
        };
        match input {
            crate::keys::KeyInput::Character(character) => {
                if !character.is_control() && label.chars().count() < MAX_LABEL {
                    label.push(character);
                }
            }
            crate::keys::KeyInput::Backspace => {
                label.pop();
            }
            crate::keys::KeyInput::Enter => {
                if let Some(editor) = self.alarm_editor.as_mut() {
                    editor.label_focused = false;
                }
                if let Some(editor) = self.timer_editor.as_mut() {
                    editor.label_focused = false;
                }
            }
        }
    }

    /// Applies a daemon event (§5); unknown ones are ignored.
    pub fn apply(&mut self, update: ClockUpdate) {
        match update {
            ClockUpdate::Connected(connected) => {
                self.connected = connected;
                if !connected {
                    self.alarms.clear();
                    self.timers.clear();
                }
            }
            ClockUpdate::Failed(cmd) => {
                eprintln!("moon-notifyd refused {cmd}");
                self.notice = Some("Could not save the change");
            }
            ClockUpdate::Event(event) => self.apply_event(&event),
        }
    }

    fn apply_event(&mut self, event: &Value) {
        let parse_alarm = |value: &Value| serde_json::from_value::<Alarm>(value.clone()).ok();
        let parse_timer = |value: &Value| serde_json::from_value::<Timer>(value.clone()).ok();
        match event.get("event").and_then(Value::as_str) {
            Some("snapshot") => {
                self.alarms = event
                    .get("alarms")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(parse_alarm).collect())
                    .unwrap_or_default();
                self.timers = event
                    .get("timers")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(parse_timer).collect())
                    .unwrap_or_default();
            }
            Some("alarm_changed" | "alarm_ringing") => {
                if let Some(alarm) = event.get("alarm").and_then(parse_alarm) {
                    match self.alarms.iter_mut().find(|a| a.id == alarm.id) {
                        Some(existing) => *existing = alarm,
                        None => self.alarms.push(alarm),
                    }
                }
            }
            Some("alarm_deleted") => {
                if let Some(id) = event.get("id").and_then(Value::as_str) {
                    self.alarms.retain(|a| a.id != id);
                }
            }
            Some("timer_changed" | "timer_ringing") => {
                if let Some(timer) = event.get("timer").and_then(parse_timer) {
                    match self.timers.iter_mut().find(|t| t.id == timer.id) {
                        Some(existing) => *existing = timer,
                        None => self.timers.push(timer),
                    }
                }
            }
            Some("timer_deleted") => {
                if let Some(id) = event.get("id").and_then(Value::as_str) {
                    self.timers.retain(|t| t.id != id);
                }
            }
            _ => {}
        }
        // Alarms in time order, as on iPhone.
        self.alarms
            .sort_by_key(|a| (u16::from(a.hour) * 60 + u16::from(a.minute), a.id.clone()));
    }
}

fn wrap(value: u8, delta: i8, modulus: u8) -> u8 {
    ((i16::from(value) + i16::from(delta)).rem_euclid(i16::from(modulus))) as u8
}

/// What the worker thread reports.
#[derive(Debug)]
pub enum ClockUpdate {
    Connected(bool),
    Event(Value),
    Failed(String),
}

/// A connection to `moon-notifyd` on its own thread: subscribes, forwards
/// events, and sends the page's commands. It reconnects while the daemon
/// is away and never blocks the UI.
pub struct NotifyWorker {
    requests: Sender<NotifyRequest>,
    updates: Receiver<ClockUpdate>,
}

impl NotifyWorker {
    pub fn spawn() -> Self {
        let socket = std::env::var_os("MOON_NOTIFY_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(NOTIFY_SOCKET));
        let (request_tx, request_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        thread::spawn(move || worker_loop(&socket, request_rx, update_tx));
        Self {
            requests: request_tx,
            updates: update_rx,
        }
    }

    pub fn send(&self, request: NotifyRequest) {
        let _ = self.requests.send(request);
    }

    pub fn try_recv(&self) -> Option<ClockUpdate> {
        self.updates.try_recv().ok()
    }
}

fn worker_loop(socket: &Path, requests: Receiver<NotifyRequest>, updates: Sender<ClockUpdate>) {
    let mut connected = false;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => {
                if serve(stream, &requests, &updates).is_err() {
                    return;
                }
                connected = false;
                if updates.send(ClockUpdate::Connected(false)).is_err() {
                    return;
                }
            }
            Err(_) => {
                if connected {
                    connected = false;
                }
                // Commands made while the daemon is away cannot be applied.
                loop {
                    match requests.try_recv() {
                        Ok((cmd, _)) => {
                            if updates.send(ClockUpdate::Failed(cmd.into())).is_err() {
                                return;
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
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
    requests: &Receiver<NotifyRequest>,
    updates: &Sender<ClockUpdate>,
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
    updates.send(ClockUpdate::Connected(true)).map_err(|_| ())?;
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
                let Some(value) = value else { continue };
                if value.get("event").is_some() {
                    updates.send(ClockUpdate::Event(value)).map_err(|_| ())?;
                } else if let Some(id) = value.get("id").and_then(Value::as_u64)
                    && let Some(position) = outbox.pending.iter().position(|(p, _)| *p == id)
                {
                    let (_, cmd) = outbox.pending.remove(position);
                    if value.get("ok").and_then(Value::as_bool) != Some(true) {
                        updates
                            .send(ClockUpdate::Failed(cmd.into()))
                            .map_err(|_| ())?;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn connected() -> ClockModel {
        ClockModel {
            connected: true,
            ..ClockModel::default()
        }
    }

    #[test]
    fn times_and_repeats_read_like_ios() {
        assert_eq!(alarm_time(0, 5), ("12:05".into(), "AM"));
        assert_eq!(alarm_time(13, 30), ("1:30".into(), "PM"));
        assert_eq!(repeat_text(&[1, 2, 3, 4, 5]), "Weekdays");
        assert_eq!(repeat_text(&[0, 6]), "Weekends");
        assert_eq!(repeat_text(&[0, 1, 2, 3, 4, 5, 6]), "Every day");
        assert_eq!(repeat_text(&[1, 3]), "Mon, Wed");
        assert_eq!(repeat_text(&[]), "");
        assert_eq!(countdown(65), "1:05");
        assert_eq!(countdown(3723), "1:02:03");
    }

    #[test]
    fn a_new_alarm_is_created_with_the_chosen_time_days_and_label() {
        let mut model = connected();
        model.handle(ClockAction::NewAlarm);
        model.handle(ClockAction::Step(EditorField::Hour, -1));
        model.handle(ClockAction::Step(EditorField::Minute, -1));
        model.handle(ClockAction::ToggleDay(1));
        model.handle(ClockAction::ToggleDay(3));
        model.handle(ClockAction::FocusLabel);
        for character in "Gym".chars() {
            model.type_text(crate::keys::KeyInput::Character(character));
        }
        let requests = model.handle(ClockAction::Save);
        assert_eq!(requests.len(), 1);
        let (cmd, arguments) = &requests[0];
        assert_eq!(*cmd, "alarm.create");
        assert_eq!(
            *arguments,
            json!({"hour": 6, "minute": 59, "days": [1, 3], "label": "Gym"})
        );
        assert!(model.alarm_editor.is_none());
    }

    #[test]
    fn edits_toggles_and_deletes_name_the_alarm() {
        let mut model = connected();
        model.apply(ClockUpdate::Event(json!({"event": "snapshot", "alarms": [
            {"id": "a2", "hour": 9, "minute": 0, "days": [], "label": "", "enabled": true},
            {"id": "a1", "hour": 7, "minute": 30, "days": [1], "label": "Up", "enabled": true}
        ], "timers": []})));
        assert_eq!(model.alarms[0].id, "a1");
        let requests = model.handle(ClockAction::ToggleAlarm(0));
        assert_eq!(
            requests,
            vec![("alarm.update", json!({"alarm": "a1", "enabled": false}))]
        );
        model.handle(ClockAction::EditAlarm(1));
        assert_eq!(model.alarm_editor.as_ref().unwrap().hour, 9);
        assert_eq!(
            model.handle(ClockAction::DeleteAlarm),
            vec![("alarm.delete", json!({"alarm": "a2"}))]
        );
        assert_eq!(model.alarms.len(), 1);
    }

    #[test]
    fn timers_start_pause_and_count_down() {
        let mut model = connected();
        assert_eq!(
            model.handle(ClockAction::StartPreset(3)),
            vec![("timer.create", json!({"duration_s": 600}))]
        );
        model.apply(ClockUpdate::Event(json!({"event": "timer_changed", "timer":
            {"id": "t1", "label": "", "duration_s": 600, "state": "running", "ends_at": 1000, "remaining_s": 600}})));
        assert_eq!(model.timers[0].left(400), 600);
        assert_eq!(model.timers[0].left(990), 10);
        assert_eq!(
            model.handle(ClockAction::PauseOrResume(0)),
            vec![("timer.pause", json!({"timer": "t1"}))]
        );
        model.handle(ClockAction::NewTimer);
        model.handle(ClockAction::Step(EditorField::Second, 1));
        assert_eq!(
            model.handle(ClockAction::Save),
            vec![("timer.create", json!({"duration_s": 301, "label": ""}))]
        );
    }

    #[test]
    fn the_worker_subscribes_and_forwards_events() {
        let dir = std::env::temp_dir().join(format!("a26-clock-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("notify.sock");
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        let socket = path.clone();
        thread::spawn(move || worker_loop(&socket, request_rx, update_tx));
        let (server, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(server.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let subscribe: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(subscribe["cmd"], "subscribe");
        (&server)
            .write_all(b"{\"event\":\"snapshot\",\"alarms\":[],\"timers\":[]}\n")
            .unwrap();
        request_tx
            .send(("timer.cancel", json!({"timer": "t9"})))
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let cancel: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(cancel["cmd"], "timer.cancel");
        assert_eq!(cancel["timer"], "t9");
        let mut got = Vec::new();
        while got.len() < 2 {
            got.push(update_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        assert!(matches!(got[0], ClockUpdate::Connected(true)));
        assert!(matches!(&got[1], ClockUpdate::Event(e) if e["event"] == "snapshot"));
        let _ = std::fs::remove_file(&path);
    }
}
