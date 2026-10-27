//! Phone and Messages: the same binary as the System app, started by Moon as
//! `a26-system --app phone` or `--app messages`. Phone has a keypad and
//! Recents (the daemon's call history); Messages lists conversations and
//! shows them. Both say plainly when there is no SIM or no service. Calls
//! themselves are Moon's: once dialed, its call screen takes over.

use std::error::Error;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::comms_ui::{self, Hit};
use crate::control;
use crate::telephony::{
    CallRecord, Message, Service, TelRequest, TelState, TelUpdate, TelWorker, Thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Phone,
    Messages,
}

impl Kind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "phone" => Some(Self::Phone),
            "messages" => Some(Self::Messages),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Messages => "messages",
        }
    }
}

/// Whether a conversation's view should jump to its newest message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stick {
    No,
    /// It just opened.
    Always,
    /// It grew: follow only if the newest message was in view.
    IfNear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Recents,
    Keypad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    Phone(Tab),
    Threads,
    Thread(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Tab(Tab),
    /// A keypad key, by index (1-9, *, 0, #).
    Key(u8),
    Delete,
    Call,
    /// Call a Recents entry back.
    CallBack(usize),
    OpenThread(usize),
    Back,
}

pub const KEYS: [char; 12] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '*', '0', '#'];
/// How long a notice ("No SIM") stays under the number.
const NOTICE_FOR: Duration = Duration::from_millis(2500);

/// A list scrolled by dragging, with momentum after release.
#[derive(Debug, Default, Clone)]
pub struct Scroll {
    pub offset: f32,
    drag: Option<(f32, f32)>,
    samples: Vec<(Instant, f32)>,
    fling: Option<(Instant, f32, f32)>,
}

/// iOS's normal deceleration rate, per millisecond.
const DECELERATION: f32 = 0.998;

impl Scroll {
    pub fn begin(&mut self, y: f32, now: Instant) {
        self.offset = self.value(now);
        self.fling = None;
        self.drag = Some((y, self.offset));
        self.samples.clear();
        self.samples.push((now, y));
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn moved(&mut self, y: f32, max: f32, now: Instant) {
        if let Some((start, from)) = self.drag {
            let raw = from + start - y;
            // Past either end the list follows the finger at a third.
            self.offset = if raw < 0.0 {
                raw / 3.0
            } else if raw > max {
                max + (raw - max) / 3.0
            } else {
                raw
            };
            self.samples.push((now, y));
            self.samples
                .retain(|(at, _)| now.saturating_duration_since(*at) < Duration::from_millis(100));
        }
    }

    pub fn end(&mut self, max: f32, now: Instant) {
        if self.drag.take().is_none() {
            return;
        }
        let velocity = match (self.samples.first(), self.samples.last()) {
            (Some((t0, y0)), Some((t1, y1))) if t1 > t0 => {
                -(y1 - y0) / t1.duration_since(*t0).as_secs_f32()
            }
            _ => 0.0,
        };
        self.samples.clear();
        if self.offset < 0.0 || self.offset > max || velocity.abs() > 150.0 {
            self.fling = Some((now, self.offset, velocity.clamp(-9000.0, 9000.0)));
        }
    }

    /// The offset at `now`, clamped into `0..=max` once settled.
    pub fn value(&self, now: Instant) -> f32 {
        let Some((started, from, velocity)) = self.fling else {
            return self.offset;
        };
        let ms = now.saturating_duration_since(started).as_secs_f32() * 1000.0;
        // Decelerate (x(t) = v/k (1 - d^t), k = -ln d per ms).
        let k = -DECELERATION.ln();
        let travel = velocity / 1000.0 / k * (1.0 - DECELERATION.powf(ms));
        from + travel
    }

    /// Advances the fling; returns true while it still moves. Overshoot
    /// past an end eases back.
    pub fn step(&mut self, max: f32, now: Instant) -> bool {
        let Some((started, _, velocity)) = self.fling else {
            return false;
        };
        let value = self.value(now);
        let ms = now.saturating_duration_since(started).as_secs_f32() * 1000.0;
        let speed = (velocity * DECELERATION.powf(ms)).abs();
        let outside = value < 0.0 || value > max;
        if outside {
            // Ease back to the edge.
            let edge = value.clamp(0.0, max);
            let next = edge + (value - edge) * 0.78;
            self.offset = if (next - edge).abs() < 0.5 {
                edge
            } else {
                next
            };
            self.fling = (self.offset != edge).then_some((now, self.offset, 0.0));
            return self.fling.is_some();
        }
        if speed < 20.0 {
            self.offset = value;
            self.fling = None;
            return false;
        }
        true
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub struct CommsModel {
    pub kind: Kind,
    pub connected: bool,
    pub state: TelState,
    pub page: Page,
    pub digits: String,
    /// `None` until the daemon answered.
    pub recents: Option<Vec<CallRecord>>,
    pub threads: Option<Vec<Thread>>,
    /// The open conversation, oldest first.
    pub messages: Vec<Message>,
    pub scroll: Scroll,
    pub notice: Option<(String, Instant)>,
    /// A conversation loaded or grew: show its newest messages.
    pub stick_bottom: Stick,
}

impl CommsModel {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            connected: false,
            state: TelState::default(),
            page: match kind {
                Kind::Phone => Page::Phone(Tab::Keypad),
                Kind::Messages => Page::Threads,
            },
            digits: String::new(),
            recents: None,
            threads: None,
            messages: Vec::new(),
            scroll: Scroll::default(),
            notice: None,
            stick_bottom: Stick::No,
        }
    }

    pub fn service(&self) -> Service {
        self.state.service(self.connected)
    }

    /// What the current page needs from the daemon.
    fn refresh(&self) -> Vec<TelRequest> {
        if !self.connected {
            return Vec::new();
        }
        match &self.page {
            Page::Phone(_) => vec![("call.history", json!({ "limit": 100 }))],
            Page::Threads => vec![("sms.threads", json!({}))],
            Page::Thread(peer) => vec![("sms.list", json!({ "peer": peer, "limit": 200 }))],
        }
    }

    pub fn apply(&mut self, update: TelUpdate, now: Instant) -> Vec<TelRequest> {
        match update {
            TelUpdate::Connected(connected) => {
                self.connected = connected;
                if !connected {
                    self.state = TelState::default();
                }
                self.refresh()
            }
            TelUpdate::Event(event) => self.apply_event(&event),
            TelUpdate::Reply(cmd, result) => {
                self.apply_reply(cmd, result);
                Vec::new()
            }
            TelUpdate::Failed(cmd, code) => {
                if cmd == "call.dial" {
                    self.notice = Some((failure_text(&code).into(), now));
                }
                Vec::new()
            }
        }
    }

    fn apply_event(&mut self, event: &Value) -> Vec<TelRequest> {
        let name = event["event"].as_str().unwrap_or_default();
        match name {
            "snapshot" | "state_changed" => {
                if let Ok(state) = serde_json::from_value(event["state"].clone()) {
                    self.state = state;
                }
                Vec::new()
            }
            "call_history_changed" if self.kind == Kind::Phone => self.refresh(),
            "sms_received" | "sms_updated" | "sms_read" | "sms_deleted"
                if self.kind == Kind::Messages =>
            {
                let mut requests = self.refresh();
                // A message into the open conversation is read at once
                // (which also withdraws its notification).
                if name == "sms_received"
                    && let Page::Thread(peer) = &self.page
                    && event["message"]["peer"].as_str() == Some(peer.as_str())
                {
                    requests.push(("sms.mark_read", json!({ "peer": peer })));
                }
                requests
            }
            _ => Vec::new(),
        }
    }

    fn apply_reply(&mut self, cmd: &str, result: Value) {
        match cmd {
            "call.history" => {
                self.recents = serde_json::from_value(result["calls"].clone()).ok();
            }
            "sms.threads" => {
                self.threads = serde_json::from_value(result["threads"].clone()).ok();
            }
            "sms.list" => {
                if let Ok(mut messages) =
                    serde_json::from_value::<Vec<Message>>(result["messages"].clone())
                {
                    messages.reverse();
                    self.stick_bottom = if self.messages.is_empty() {
                        Stick::Always
                    } else if messages.len() > self.messages.len() {
                        Stick::IfNear
                    } else {
                        Stick::No
                    };
                    self.messages = messages;
                }
            }
            "call.dial" => self.digits.clear(),
            _ => {}
        }
    }

    pub fn handle(&mut self, action: Action, now: Instant) -> Vec<TelRequest> {
        match action {
            Action::Tab(tab) => {
                self.page = Page::Phone(tab);
                self.scroll.reset();
                self.refresh()
            }
            Action::Key(index) => {
                if self.digits.len() < 32 {
                    self.digits.push(KEYS[usize::from(index)]);
                }
                Vec::new()
            }
            Action::Delete => {
                self.digits.pop();
                Vec::new()
            }
            Action::Call => {
                if self.digits.is_empty() {
                    return Vec::new();
                }
                let number = self.digits.clone();
                self.dial(number, now)
            }
            Action::CallBack(index) => {
                let number = self
                    .recents
                    .as_ref()
                    .and_then(|recents| recents.get(index))
                    .and_then(|call| call.number.clone());
                match number {
                    Some(number) => self.dial(number, now),
                    None => Vec::new(),
                }
            }
            Action::OpenThread(index) => {
                let Some(peer) = self
                    .threads
                    .as_ref()
                    .and_then(|threads| threads.get(index))
                    .map(|thread| thread.peer.clone())
                else {
                    return Vec::new();
                };
                self.open_thread(peer)
            }
            Action::Back => {
                self.page = Page::Threads;
                self.messages.clear();
                self.scroll.reset();
                self.refresh()
            }
        }
    }

    fn open_thread(&mut self, peer: String) -> Vec<TelRequest> {
        self.page = Page::Thread(peer.clone());
        self.messages.clear();
        self.scroll.reset();
        let mut requests = self.refresh();
        requests.push(("sms.mark_read", json!({ "peer": peer })));
        requests
    }

    /// Places a call the user asked for, or says why it cannot be placed.
    fn dial(&mut self, number: String, now: Instant) -> Vec<TelRequest> {
        match self.service() {
            Service::Ready => vec![("call.dial", json!({ "number": number }))],
            service => {
                self.notice = Some((service_text(service).into(), now));
                Vec::new()
            }
        }
    }

    /// A tapped notification's target (`thread:PEER`, `call:PEER`).
    pub fn open_target(&mut self, target: &str) -> Vec<TelRequest> {
        match (self.kind, target.split_once(':')) {
            (Kind::Messages, Some(("thread", peer))) if !peer.is_empty() => {
                self.open_thread(peer.to_owned())
            }
            (Kind::Phone, Some(("call", _))) => {
                self.handle(Action::Tab(Tab::Recents), Instant::now())
            }
            _ => Vec::new(),
        }
    }

    pub fn notice(&self, now: Instant) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < NOTICE_FOR)
            .map(|(text, _)| text.as_str())
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.notice.as_ref().map(|(_, at)| *at + NOTICE_FOR)
    }
}

/// What the Phone keypad says under the number, and the empty states, for a
/// service that is not ready.
pub fn service_text(service: Service) -> &'static str {
    match service {
        Service::Ready => "",
        Service::Unavailable => "Cellular service unavailable",
        Service::Airplane => "Airplane Mode is on",
        Service::NoSim => "No SIM",
        Service::SimLocked => "SIM locked",
        Service::NoNetwork { emergency: true } => "SOS only",
        Service::NoNetwork { emergency: false } => "No service",
    }
}

fn failure_text(code: &str) -> &'static str {
    match code {
        "unavailable" => "Cellular service unavailable",
        "busy" => "A call is already in progress",
        _ => "Call failed",
    }
}

/// Runs the Phone or Messages app until its window closes.
pub fn run(kind: Kind) -> Result<(), Box<dyn Error>> {
    let (conn, screen_number) = x11rb::rust_connection::RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_number];
    let (window, protocols) = crate::create_window(&conn, screen, kind.id())?;
    let mut renderer = comms_ui::Renderer::new(
        &conn,
        window,
        screen.root_depth,
        crate::REQUESTED_WIDTH,
        crate::REQUESTED_HEIGHT,
    )?;
    let worker = TelWorker::spawn();
    let mut model = CommsModel::new(kind);
    // A target younger than 10 s is still wanted: a notification just
    // started the app for it.
    let mut seen_target = None;
    let mut tap: Option<crate::TapTracker> = None;
    let mut pressed: Option<Hit> = None;
    let mut running = true;
    let mut redraw = true;
    let mut next_shell_poll = Instant::now();
    let mut last_max = 0.0;

    renderer.render(&conn, &model, None, Instant::now())?;
    conn.map_window(window)?;
    conn.flush()?;

    while running {
        let now = Instant::now();
        while let Some(update) = worker.try_recv() {
            for request in model.apply(update, now) {
                worker.send(request);
            }
            redraw = true;
        }
        let mut check_target = false;
        while let Some(event) = conn.poll_for_event()? {
            let now = Instant::now();
            match event {
                Event::Expose(event) if event.window == window && event.count == 0 => {
                    renderer.expose(&conn)?;
                }
                Event::ConfigureNotify(event) if event.window == window => {
                    if (event.width, event.height) != renderer.size() {
                        renderer.resize(&conn, event.width, event.height)?;
                        redraw = true;
                    }
                }
                Event::MapNotify(event) if event.window == window => check_target = true,
                Event::ButtonPress(event) if event.event == window && event.detail == 1 => {
                    tap = crate::TapTracker::new(event.event_x, event.event_y, renderer.height());
                    if tap.is_some() {
                        let height = f32::from(renderer.height());
                        if comms_ui::scrolls(&model, height, f32::from(event.event_y)) {
                            model.scroll.begin(f32::from(event.event_y), now);
                        }
                        pressed = comms_ui::hit(&model, height, event.event_x, event.event_y, now);
                        redraw = true;
                    }
                }
                Event::MotionNotify(event) if event.event == window => {
                    if let Some(tap) = &mut tap {
                        tap.observe(event.event_x, event.event_y);
                        if tap.moved {
                            pressed = None;
                            let max = comms_ui::scroll_max(&model, f32::from(renderer.height()));
                            model.scroll.moved(f32::from(event.event_y), max, now);
                            redraw = true;
                        }
                    }
                }
                Event::ButtonRelease(event) if event.event == window && event.detail == 1 => {
                    let height = f32::from(renderer.height());
                    model.scroll.end(comms_ui::scroll_max(&model, height), now);
                    pressed = None;
                    if let Some((x, y)) = tap
                        .take()
                        .and_then(|tap| tap.finish(event.event_x, event.event_y, renderer.height()))
                        && let Some(Hit::Action(action)) = comms_ui::hit(&model, height, x, y, now)
                    {
                        let before = std::mem::discriminant(&model.page);
                        let opening = matches!(model.page, Page::Threads);
                        let requests = model.handle(action, now);
                        if std::mem::discriminant(&model.page) != before
                            && matches!(model.page, Page::Thread(_) | Page::Threads)
                        {
                            // A conversation opening or closing slides.
                            control::transition(opening);
                        }
                        for request in requests {
                            worker.send(request);
                        }
                    }
                    redraw = true;
                }
                Event::ClientMessage(event)
                    if event.window == window
                        && event.type_ == protocols.wm_protocols
                        && event.data.as_data32()[0] == protocols.wm_delete_window =>
                {
                    running = false;
                }
                Event::DestroyNotify(event) if event.window == window => running = false,
                _ => {}
            }
        }

        let now = Instant::now();
        // A tapped notification's target, handed over in Moon's state:
        // looked at when the window comes forward, and every second.
        if check_target || now >= next_shell_poll {
            next_shell_poll = now + Duration::from_secs(1);
            if let Some(open) = control::read_shell().open_target
                && open.app == kind.id()
                && seen_target != Some(open.serial)
            {
                seen_target = Some(open.serial);
                if open.age_ms < 10_000 {
                    for request in model.open_target(&open.target) {
                        worker.send(request);
                    }
                    redraw = true;
                }
            }
        }
        let max = comms_ui::scroll_max(&model, f32::from(renderer.height()));
        let near = model.scroll.value(now) >= last_max - 60.0;
        let stick = std::mem::replace(&mut model.stick_bottom, Stick::No);
        if (stick == Stick::Always || (stick == Stick::IfNear && near)) && !model.scroll.dragging()
        {
            model.scroll.reset();
            model.scroll.offset = max;
            redraw = true;
        }
        last_max = max;
        let flinging = model.scroll.step(max, now);
        if model.next_deadline().is_some_and(|at| now >= at) {
            model.notice = None;
            redraw = true;
        }
        if (redraw || flinging) && running {
            renderer.render(&conn, &model, pressed, now)?;
            redraw = false;
        }
        let mut deadline = next_shell_poll;
        if flinging {
            deadline = deadline.min(now + Duration::from_millis(16));
        } else {
            // Worker results arrive on a channel; pick them up soon.
            deadline = deadline.min(now + Duration::from_millis(100));
        }
        if let Some(at) = model.next_deadline() {
            deadline = deadline.min(at);
        }
        crate::wait_for_x(&conn, deadline.saturating_duration_since(Instant::now()))?;
    }
    renderer.destroy(&conn);
    let _ = conn.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> TelState {
        serde_json::from_value(json!({
            "available": true, "sim": {"state": "ready"},
            "network": {"registration": "home"}
        }))
        .unwrap()
    }

    #[test]
    fn the_keypad_dials_only_with_service() {
        let now = Instant::now();
        let mut model = CommsModel::new(Kind::Phone);
        for key in [5, 8, 10] {
            model.handle(Action::Key(key), now);
        }
        assert_eq!(model.digits, "690");
        model.handle(Action::Delete, now);
        assert_eq!(model.digits, "69");
        // No daemon: nothing is sent, and the keypad says why.
        assert!(model.handle(Action::Call, now).is_empty());
        assert_eq!(model.notice(now), Some("Cellular service unavailable"));
        model.apply(TelUpdate::Connected(true), now);
        model.state = ready();
        assert_eq!(
            model.handle(Action::Call, now),
            vec![("call.dial", json!({"number": "69"}))]
        );
        model.apply(TelUpdate::Reply("call.dial", json!({})), now);
        assert!(model.digits.is_empty());
    }

    #[test]
    fn recents_and_threads_load_and_refresh() {
        let now = Instant::now();
        let mut phone = CommsModel::new(Kind::Phone);
        assert_eq!(
            phone.apply(TelUpdate::Connected(true), now),
            vec![("call.history", json!({"limit": 100}))]
        );
        phone.apply(
            TelUpdate::Reply(
                "call.history",
                json!({"calls": [{"id": "c1", "number": "+16475551234", "name": null,
                    "direction": "incoming", "started_at": 5, "connected_at": null,
                    "ended_at": 9, "duration_s": 0, "missed": true, "rejected": false}]}),
            ),
            now,
        );
        assert!(phone.recents.as_ref().unwrap()[0].missed);
        assert_eq!(
            phone
                .apply(
                    TelUpdate::Event(json!({"event": "call_history_changed", "ids": ["c2"]})),
                    now
                )
                .len(),
            1
        );

        let mut messages = CommsModel::new(Kind::Messages);
        messages.apply(TelUpdate::Connected(true), now);
        messages.apply(
            TelUpdate::Reply(
                "sms.threads",
                json!({"threads": [{"peer": "+1647", "count": 2, "unread": 1,
                    "last": {"id": "m2", "peer": "+1647", "direction": "in", "text": "Hi",
                        "time": 9, "status": "received", "read": false, "error": null}}]}),
            ),
            now,
        );
        let requests = messages.handle(Action::OpenThread(0), now);
        assert_eq!(
            requests,
            vec![
                ("sms.list", json!({"peer": "+1647", "limit": 200})),
                ("sms.mark_read", json!({"peer": "+1647"}))
            ]
        );
        assert_eq!(messages.page, Page::Thread("+1647".into()));
        assert_eq!(messages.handle(Action::Back, now).len(), 1);
        assert_eq!(messages.open_target("thread:+1416").len(), 2);
        assert_eq!(messages.page, Page::Thread("+1416".into()));
    }

    #[test]
    fn a_fling_decelerates_and_settles_inside_the_list() {
        let start = Instant::now();
        let mut scroll = Scroll::default();
        scroll.begin(1000.0, start);
        scroll.moved(900.0, 5000.0, start + Duration::from_millis(40));
        scroll.moved(700.0, 5000.0, start + Duration::from_millis(80));
        scroll.end(5000.0, start + Duration::from_millis(80));
        let mut t = start + Duration::from_millis(80);
        let mut last = scroll.value(t);
        while scroll.step(5000.0, t) {
            t += Duration::from_millis(16);
            let value = scroll.value(t);
            assert!(value >= last - 0.01);
            last = value;
            assert!(t < start + Duration::from_secs(10));
        }
        assert!(scroll.offset > 1000.0 && scroll.offset <= 5000.0);
        // Pulled past the top, it eases back to 0.
        scroll.begin(500.0, t);
        scroll.moved(800.0, 2000.0, t);
        scroll.offset = -60.0;
        scroll.end(2000.0, t);
        while scroll.step(2000.0, t) {
            t += Duration::from_millis(16);
        }
        assert_eq!(scroll.offset, 0.0);
    }
}
