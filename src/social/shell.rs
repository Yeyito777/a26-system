//! The social apps' event loop: the display (an X window, or Moon's Wayland
//! server when Moon asks for it), the bridge link, the picture decoder and
//! Moon's state, one `poll(2)` for all of them, as the Exocortex app's
//! `app.rs` (whose display code this copies, so neither app's changes
//! reach the other). Touches become gestures (`exocortex::gesture`); the app
//! (`App`) gets them, the bridge's events and answers, and draws frames when
//! something changed or animates. It never waits on the bridge.
//!
//! The shell also tells the bridge what the app shows (`view`, for its
//! notifications), opens a tapped notification's target, and asks Moon for
//! the status bar's colour.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::error::Error;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use x11rb::connection::Connection;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use super::link::{Link, LinkEvent};
use super::media::Media;
use super::proto::{self, BridgeError, Event, Hello};
use crate::control::{self, KeyboardPurpose, ShellInfo};
use crate::exocortex::gesture::{Gesture, Recognizer};
use crate::exocortex::line::{self, Waker};
use crate::exocortex::ui::Haptic;
use crate::exocortex::wayland::{self, Wayland};
use crate::gfx::Canvas;
use crate::keys::{KeyInput, KeyMap};
use crate::present::Presenter;

/// While in front, Moon's state (a tapped notification's target) is read
/// this often.
const SHELL_POLL: Duration = Duration::from_secs(1);
/// Back from the background after this long: reconnect, so the bridge's
/// buffered events for the frozen app are dropped and the app catches up.
const STALE_AFTER: Duration = Duration::from_secs(20);
/// At most this long is spent on queued input before a frame is drawn.
const INPUT_BUDGET: Duration = Duration::from_millis(6);
/// One panel refresh (120 Hz).
pub const FRAME: Duration = crate::exocortex::anim::FRAME;

/// A social app: its model and view, driven by the shell.
pub trait App {
    /// Moon's id for it (`instagram`): its window, bridge and notifications.
    fn id(&self) -> &'static str;
    /// The status bar over it: `#RRGGBB` and `light` or `dark` content.
    fn status_bar(&self) -> (String, &'static str);
    /// The bridge answered `hello` (at start and after every reconnection).
    fn connected(&mut self, cx: &mut Cx, hello: &Hello);
    /// The bridge went away; requests in flight were answered
    /// `disconnected` just before.
    fn disconnected(&mut self, cx: &mut Cx);
    fn event(&mut self, cx: &mut Cx, event: Event);
    /// The answer to a request the app made (`Cx::request`).
    fn answer(&mut self, cx: &mut Cx, id: u64, result: Result<Value, BridgeError>);
    fn gesture(&mut self, cx: &mut Cx, gesture: Gesture);
    /// A key from Moon's keyboard.
    fn key(&mut self, _cx: &mut Cx, _key: KeyInput) {}
    /// A tapped notification's target (a chat id), or Moon's `open APP TARGET`.
    fn open_target(&mut self, _cx: &mut Cx, _target: &str) {}
    /// In front (mapped) or hidden.
    fn shown(&mut self, _cx: &mut Cx, _shown: bool) {}
    /// Every pass of the loop, before drawing: flings and animations step
    /// here and ask for frames (`Cx::animate`).
    fn tick(&mut self, _cx: &mut Cx) {}
    /// The chat on screen, if any: the bridge does not notify for it.
    fn viewing(&self) -> Option<String>;
    /// Draws the frame (the canvas holds the last one).
    fn draw(&mut self, canvas: &mut Canvas, cx: &mut DrawCx);
}

/// What the app may do while handling something.
pub struct Cx<'a> {
    pub link: &'a Link,
    pub media: &'a mut Media,
    pub now: Instant,
    pub width: f32,
    pub height: f32,
    /// Greeted by the bridge and not dropped since.
    pub connected: bool,
    redraw: bool,
    wake: Option<Instant>,
    keyboard: &'a mut bool,
    transition: &'a mut bool,
}

impl Cx<'_> {
    /// Sends a request; its answer comes to `App::answer` with this id.
    pub fn request(&self, cmd: &str, args: Value) -> u64 {
        self.link.request(cmd, args)
    }

    /// Something shows differently: draw a frame.
    pub fn redraw(&mut self) {
        self.redraw = true;
    }

    /// An animation runs: draw the next refresh too.
    pub fn animate(&mut self) {
        self.redraw = true;
        self.wake_at(self.now + FRAME);
    }

    /// Run the loop again at `at` (a timer, a long press).
    pub fn wake_at(&mut self, at: Instant) {
        self.wake = Some(self.wake.map_or(at, |w| w.min(at)));
    }

    pub fn keyboard_shown(&self) -> bool {
        *self.keyboard
    }

    pub fn show_keyboard(&mut self, purpose: KeyboardPurpose) {
        *self.keyboard = control::show_keyboard(purpose);
    }

    pub fn hide_keyboard(&mut self) {
        if *self.keyboard {
            control::hide_keyboard();
        }
        *self.keyboard = false;
    }

    /// Before the app shows another page: Moon snapshots this one and
    /// slides the next over it (`push`) or this one off it.
    pub fn transition(&mut self, push: bool) {
        control::transition(push);
        *self.transition = true;
        self.redraw = true;
    }

    pub fn haptic(&self, haptic: Haptic) {
        crate::exocortex::ui::haptic(haptic);
    }

    pub fn open_url(&self, url: &str) -> bool {
        crate::exocortex::ui::open_url(url)
    }

    pub fn copy(&self, text: &str) {
        crate::exocortex::clipboard::copy(text);
    }
}

/// What drawing may use: the pictures, the time.
pub struct DrawCx<'a> {
    pub media: &'a mut Media,
    pub now: Instant,
    /// The whole canvas must be drawn (the first frame, a resize, a
    /// reopened window); otherwise it holds the last frame.
    pub full: bool,
    /// What the frame changed, when the app knows (it leaves None
    /// otherwise): the display is then told to move the rows itself and
    /// sent only the rows drawn anew.
    pub damage: Option<Damage>,
}

/// A frame's changes: rows `y0..y1` moved by `dy` (as `Canvas::move_rows`
/// moved them), and the rows drawn anew. Nothing else may have changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Damage {
    pub moved: Option<(i32, i32, i32)>,
    pub rows: Vec<(i32, i32)>,
}

/// The app's X window and what shows the canvas in it.
struct XOutput {
    conn: RustConnection,
    window: Window,
    depth: u8,
    protocols: crate::WindowProtocols,
    key_map: KeyMap,
    presenter: Presenter,
    last_motion: Option<(i16, i16)>,
}

impl XOutput {
    fn open(app_id: &str, width: u16, height: u16) -> Result<Self, Box<dyn Error>> {
        let (conn, screen_number) = RustConnection::connect(None)?;
        let screen = conn.setup().roots[screen_number].clone();
        let (window, protocols) = crate::create_window(&conn, &screen, app_id)?;
        let key_map = KeyMap::query(&conn)?;
        let mut presenter = Presenter::create(&conn, window, screen.root_depth, width, height)?;
        presenter.enable_shm(&conn);
        Ok(Self {
            conn,
            window,
            depth: screen.root_depth,
            protocols,
            key_map,
            presenter,
            last_motion: None,
        })
    }
}

enum Output {
    X(Box<XOutput>),
    Wayland(Box<Wayland>),
}

#[derive(Debug, Clone, Copy)]
enum DisplayEvent {
    Size(u16, u16),
    Shown(bool),
    Press(f32, f32, Instant),
    Motion(f32, f32, Instant),
    Release(f32, f32, Instant),
    Cancel,
    Key(KeyInput),
    Close,
}

struct Renderer {
    app_id: &'static str,
    canvas: Canvas,
    output: Output,
    /// The canvas holds nothing yet (or no longer matches the window).
    full: bool,
    reopened: bool,
}

impl Renderer {
    fn open(app_id: &'static str) -> Result<Self, Box<dyn Error>> {
        if let Some(opened) = Wayland::connect(app_id) {
            match opened {
                Ok(wayland) => {
                    let (width, height) = wayland.size();
                    eprintln!("{app_id}: on Wayland, {width}x{height}");
                    return Ok(Self {
                        app_id,
                        canvas: Canvas::new(i32::from(width), i32::from(height)),
                        output: Output::Wayland(Box::new(wayland)),
                        full: true,
                        reopened: false,
                    });
                }
                Err(error) => eprintln!("{app_id}: Wayland unavailable ({error}); X instead"),
            }
        }
        let (width, height) = (crate::REQUESTED_WIDTH, crate::REQUESTED_HEIGHT);
        Ok(Self {
            app_id,
            canvas: Canvas::new(i32::from(width), i32::from(height)),
            output: Output::X(Box::new(XOutput::open(app_id, width, height)?)),
            full: true,
            reopened: false,
        })
    }

    fn fall_back(&mut self, error: Box<dyn Error>) -> Result<(), Box<dyn Error>> {
        if let Output::X(_) = self.output {
            return Err(error);
        }
        eprintln!("{}: Wayland failed ({error}); going on in X", self.app_id);
        let (width, height) = (crate::REQUESTED_WIDTH, crate::REQUESTED_HEIGHT);
        self.output = Output::X(Box::new(XOutput::open(self.app_id, width, height)?));
        self.canvas = Canvas::new(i32::from(width), i32::from(height));
        self.full = true;
        self.reopened = true;
        Ok(())
    }

    fn resize(&mut self, width: u16, height: u16) -> Result<(), Box<dyn Error>> {
        if let Output::X(x) = &mut self.output {
            let mut presenter = Presenter::create(&x.conn, x.window, x.depth, width, height)?;
            presenter.enable_shm(&x.conn);
            x.presenter.destroy(&x.conn);
            x.presenter = presenter;
        }
        self.canvas = Canvas::new(i32::from(width), i32::from(height));
        self.full = true;
        Ok(())
    }

    fn size(&self) -> (u16, u16) {
        (self.canvas.width as u16, self.canvas.height as u16)
    }

    fn ready(&self) -> bool {
        match &self.output {
            Output::X(_) => true,
            Output::Wayland(wayland) => wayland.ready(),
        }
    }

    fn show(&mut self, events: &mut VecDeque<DisplayEvent>) -> Result<(), Box<dyn Error>> {
        match &self.output {
            Output::X(x) => {
                x.conn.map_window(x.window)?;
                x.conn.flush()?;
            }
            Output::Wayland(wayland) => events.push_back(DisplayEvent::Shown(wayland.shown())),
        }
        Ok(())
    }

    /// Hands the canvas to the display (X compares it with what the server
    /// has and uploads only what changed).
    fn present(&mut self, damage: Option<Damage>) -> Result<(), Box<dyn Error>> {
        match &mut self.output {
            Output::X(x) => {
                match damage {
                    Some(damage) => {
                        if let Some((y0, y1, dy)) = damage.moved {
                            x.presenter.scroll(&x.conn, y0, y1, dy)?;
                        }
                        x.presenter
                            .present_rows(&x.conn, &self.canvas, 0, &damage.rows)?;
                    }
                    None => {
                        x.presenter.present(&x.conn, &self.canvas, 0)?;
                    }
                }
                x.conn.flush()?;
            }
            Output::Wayland(wayland) => {
                let width = self.canvas.width;
                let areas: Option<Vec<wayland::Area>> = damage.map(|damage| {
                    let mut areas: Vec<wayland::Area> = damage
                        .rows
                        .iter()
                        .map(|&(y0, y1)| (0, y0, width, y1))
                        .collect();
                    if let Some((y0, y1, _)) = damage.moved {
                        areas.push((0, y0, width, y1));
                    }
                    areas
                });
                wayland.present(&self.canvas, areas.as_deref(), None)?;
            }
        }
        self.full = false;
        Ok(())
    }

    fn read(&mut self, events: &mut VecDeque<DisplayEvent>) -> Result<(), Box<dyn Error>> {
        match &mut self.output {
            Output::X(x) => {
                let window = x.window;
                while let Some(event) = x.conn.poll_for_event()? {
                    x.presenter.note_event(&event);
                    let event = match event {
                        XEvent::Expose(event) if event.window == window && event.count == 0 => {
                            x.presenter.expose(&x.conn, 0)?;
                            x.conn.flush()?;
                            continue;
                        }
                        XEvent::ConfigureNotify(event) if event.window == window => {
                            DisplayEvent::Size(event.width, event.height)
                        }
                        XEvent::MapNotify(event) if event.window == window => {
                            DisplayEvent::Shown(true)
                        }
                        XEvent::UnmapNotify(event) if event.window == window => {
                            DisplayEvent::Shown(false)
                        }
                        XEvent::ButtonPress(event)
                            if event.event == window && event.detail == 1 =>
                        {
                            x.last_motion = Some((event.event_x, event.event_y));
                            DisplayEvent::Press(
                                f32::from(event.event_x),
                                f32::from(event.event_y),
                                event_instant(event.time),
                            )
                        }
                        // Moon passes each touch report on twice: a motion to
                        // where the finger already is says nothing.
                        XEvent::MotionNotify(event)
                            if event.event == window
                                && x.last_motion.replace((event.event_x, event.event_y))
                                    != Some((event.event_x, event.event_y)) =>
                        {
                            DisplayEvent::Motion(
                                f32::from(event.event_x),
                                f32::from(event.event_y),
                                event_instant(event.time),
                            )
                        }
                        XEvent::ButtonRelease(event)
                            if event.event == window && event.detail == 1 =>
                        {
                            DisplayEvent::Release(
                                f32::from(event.event_x),
                                f32::from(event.event_y),
                                event_instant(event.time),
                            )
                        }
                        XEvent::KeyPress(event) if event.event == window => {
                            match x.key_map.decode(event.detail, event.state) {
                                Some(input) => DisplayEvent::Key(input),
                                None => continue,
                            }
                        }
                        XEvent::ClientMessage(event)
                            if event.window == window
                                && event.type_ == x.protocols.wm_protocols
                                && event.data.as_data32()[0] == x.protocols.wm_delete_window =>
                        {
                            DisplayEvent::Close
                        }
                        XEvent::DestroyNotify(event) if event.window == window => {
                            DisplayEvent::Close
                        }
                        _ => continue,
                    };
                    events.push_back(event);
                }
            }
            Output::Wayland(wl) => {
                for event in wl.events() {
                    events.push_back(match event {
                        wayland::Event::Size(width, height) => DisplayEvent::Size(width, height),
                        wayland::Event::Shown(shown) => DisplayEvent::Shown(shown),
                        wayland::Event::Press(x, y, time) => {
                            DisplayEvent::Press(x, y, event_instant(time))
                        }
                        wayland::Event::Motion(x, y, time) => {
                            DisplayEvent::Motion(x, y, event_instant(time))
                        }
                        wayland::Event::Release(x, y, time) => {
                            DisplayEvent::Release(x, y, event_instant(time))
                        }
                        wayland::Event::Cancel => DisplayEvent::Cancel,
                        wayland::Event::Key(input) => DisplayEvent::Key(input),
                        wayland::Event::Close => DisplayEvent::Close,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Reads Moon's state off the UI thread when asked.
struct ShellReader {
    requests: Sender<()>,
    results: Receiver<ShellInfo>,
    waker: Arc<Waker>,
    busy: bool,
}

impl ShellReader {
    fn spawn() -> std::io::Result<Self> {
        let waker = Arc::new(Waker::new()?);
        let (request_tx, request_rx) = mpsc::channel::<()>();
        let (result_tx, result_rx) = mpsc::channel();
        let wake = waker.clone();
        thread::Builder::new()
            .name("social-shell".into())
            .spawn(move || {
                while request_rx.recv().is_ok() {
                    if result_tx.send(control::read_shell()).is_err() {
                        return;
                    }
                    wake.wake();
                }
            })?;
        Ok(Self {
            requests: request_tx,
            results: result_rx,
            waker,
            busy: false,
        })
    }

    fn ask(&mut self) {
        if !self.busy && self.requests.send(()).is_ok() {
            self.busy = true;
        }
    }

    fn take(&mut self) -> Option<ShellInfo> {
        self.waker.drain();
        let mut latest = None;
        while let Ok(info) = self.results.try_recv() {
            latest = Some(info);
            self.busy = false;
        }
        latest
    }
}

/// Keeps the UI thread on a fast core while the app is in front (as the
/// Exocortex app does: short bursts after idling otherwise start on a slow
/// core at a low clock).
fn boost_ui_thread(on: bool) {
    #[repr(C)]
    struct SchedAttr {
        size: u32,
        policy: u32,
        flags: u64,
        nice: i32,
        priority: u32,
        runtime: u64,
        deadline: u64,
        period: u64,
        util_min: u32,
        util_max: u32,
    }
    const KEEP_POLICY: u64 = 0x08;
    const KEEP_PARAMS: u64 = 0x10;
    const UTIL_CLAMP_MIN: u64 = 0x20;
    let attr = SchedAttr {
        size: std::mem::size_of::<SchedAttr>() as u32,
        policy: 0,
        flags: KEEP_POLICY | KEEP_PARAMS | UTIL_CLAMP_MIN,
        nice: 0,
        priority: 0,
        runtime: 0,
        deadline: 0,
        period: 0,
        util_min: if on { 1024 } else { 0 },
        util_max: 1024,
    };
    // SAFETY: sched_setattr reads a live, fully initialized sched_attr of
    // the size it is told; pid 0 is the calling thread.
    unsafe {
        libc::syscall(libc::SYS_sched_setattr, 0, &attr as *const SchedAttr, 0);
    }
}

fn status_bar(color: &str, content: &str) {
    if cfg!(test) {
        return;
    }
    let command = format!("statusbar color {color} {content}");
    let _ = thread::Builder::new()
        .name("social-moon".into())
        .spawn(move || {
            control::send(&command);
        });
}

fn monotonic_ms() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes the time into a live timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as f64 * 1000.0 + now.tv_nsec as f64 / 1e6
}

/// When an event with a CLOCK_MONOTONIC millisecond time happened; now, for
/// a time that is not plausibly recent.
fn event_instant(ms: u32) -> Instant {
    let now = Instant::now();
    let age = (monotonic_ms() as u64 as u32).wrapping_sub(ms);
    if age < 1000 {
        now.checked_sub(Duration::from_millis(u64::from(age)))
            .unwrap_or(now)
    } else {
        now
    }
}

/// Runs `app` until its window closes.
pub fn run<A: App>(mut app: A) -> Result<(), Box<dyn Error>> {
    let id = app.id();
    let boost = std::env::var_os("SOCIAL_APP_NO_BOOST").is_none();
    if boost {
        boost_ui_thread(true);
    }
    let (color, content) = app.status_bar();
    status_bar(&color, content);

    let mut renderer = Renderer::open(id)?;
    let link = Link::spawn(proto::socket_path(id), id)?;
    let mut media = Media::new()?;
    let mut shell = ShellReader::spawn()?;
    let mut recognizer = Recognizer::new(renderer.canvas.width as f32);
    let mut backlog: VecDeque<DisplayEvent> = VecDeque::new();
    let mut connected = false;
    let mut keyboard = false;
    let mut transition = false;
    let mut mapped = false;
    let mut unmapped_at: Option<Instant> = None;
    let mut slept_at = Instant::now();
    let mut seen_target: Option<u64> = None;
    // What the bridge was last told the app shows.
    let mut told: Option<(Option<String>, bool)> = None;
    let mut redraw;
    let mut wake: Option<Instant> = None;
    let mut next_shell_poll = Instant::now();
    let mut running = true;
    // Measurement (`SOCIAL_APP_PROFILE`, or the bench's
    // `EXOCORTEX_APP_PROFILE`): each frame's draw and present times and the
    // finger row it was drawn for, in the Exocortex app's words, so the
    // perf bench reads both apps alike.
    let profile = std::env::var_os("SOCIAL_APP_PROFILE").is_some()
        || std::env::var_os("EXOCORTEX_APP_PROFILE").is_some();
    let mut finger: Option<f32> = None;

    macro_rules! cx {
        ($now:expr) => {
            Cx {
                link: &link,
                media: &mut media,
                now: $now,
                width: renderer.canvas.width as f32,
                height: renderer.canvas.height as f32,
                connected,
                redraw: false,
                wake: None,
                keyboard: &mut keyboard,
                transition: &mut transition,
            }
        };
    }
    // Takes what a Cx asked for.
    macro_rules! settle {
        ($cx:expr) => {{
            let cx = $cx;
            redraw |= cx.redraw;
            if let Some(at) = cx.wake {
                wake = Some(wake.map_or(at, |w: Instant| w.min(at)));
            }
        }};
    }

    // The first frame is drawn before the window maps.
    {
        let full = renderer.full;
        let mut dcx = DrawCx {
            media: &mut media,
            now: Instant::now(),
            full,
            damage: None,
        };
        app.draw(&mut renderer.canvas, &mut dcx);
        if let Err(error) = renderer.present(None) {
            renderer.fall_back(error)?;
            let mut dcx = DrawCx {
                media: &mut media,
                now: Instant::now(),
                full: true,
                damage: None,
            };
            app.draw(&mut renderer.canvas, &mut dcx);
            renderer.present(None)?;
        }
        redraw = false;
    }
    renderer.show(&mut backlog)?;
    shell.ask();

    while running {
        if std::mem::take(&mut renderer.reopened) {
            redraw = true;
            renderer.show(&mut backlog)?;
        }
        let now = Instant::now();

        // The bridge.
        for event in link.drain() {
            let mut cx = cx!(now);
            match event {
                LinkEvent::Connected(hello) => {
                    cx.connected = true;
                    connected = true;
                    told = None;
                    app.connected(&mut cx, &hello);
                }
                LinkEvent::Disconnected => {
                    cx.connected = false;
                    connected = false;
                    cx.media.disconnected();
                    app.disconnected(&mut cx);
                }
                LinkEvent::Answer { id, result } => {
                    if !cx.media.answer(id, &result, now) {
                        app.answer(&mut cx, id, result);
                    } else {
                        cx.redraw();
                    }
                }
                LinkEvent::Event(event) => app.event(&mut cx, *event),
            }
            settle!(cx);
        }
        if media.take_decoded(now) {
            redraw = true;
        }

        // Moon's state: a tapped notification's target.
        if let Some(info) = shell.take() {
            redraw |= control::apply_time(&info);
            if let Some(open) = info.open_target
                && open.app == id
                && seen_target != Some(open.serial)
            {
                seen_target = Some(open.serial);
                if open.age_ms < 10_000 {
                    let mut cx = cx!(now);
                    app.open_target(&mut cx, &open.target);
                    settle!(cx);
                }
            }
        }

        // The display's events, within a budget.
        let reading = Instant::now();
        if let Err(error) = renderer.read(&mut backlog) {
            renderer.fall_back(error)?;
            mapped = false;
            redraw = true;
        }
        while let Some(event) = backlog.pop_front() {
            if reading.elapsed() > INPUT_BUDGET {
                backlog.push_front(event);
                break;
            }
            let now = Instant::now();
            let mut gestures = Vec::new();
            match event {
                DisplayEvent::Size(width, height) => {
                    if (width, height) != renderer.size() {
                        renderer.resize(width, height)?;
                        recognizer.set_width(renderer.canvas.width as f32);
                        redraw = true;
                    }
                }
                DisplayEvent::Shown(shown) => {
                    mapped = shown;
                    if boost {
                        boost_ui_thread(shown);
                    }
                    if shown {
                        if unmapped_at
                            .take()
                            .is_some_and(|at| now.duration_since(at) > STALE_AFTER)
                            && connected
                        {
                            link.reconnect();
                        }
                        let (color, content) = app.status_bar();
                        status_bar(&color, content);
                        shell.ask();
                        next_shell_poll = now + SHELL_POLL;
                        redraw = true;
                    } else {
                        unmapped_at = Some(slept_at.min(now));
                        gestures = recognizer.cancel();
                    }
                    let mut cx = cx!(now);
                    app.shown(&mut cx, shown);
                    settle!(cx);
                }
                DisplayEvent::Press(x, y, at) => {
                    finger = Some(y);
                    gestures = recognizer.down(x, y, at);
                }
                DisplayEvent::Motion(x, y, at) => {
                    finger = Some(y);
                    gestures = recognizer.moved(x, y, at);
                }
                DisplayEvent::Release(x, y, at) => {
                    finger = None;
                    gestures = recognizer.up(x, y, at);
                }
                DisplayEvent::Cancel => gestures = recognizer.cancel(),
                DisplayEvent::Key(input) => {
                    let mut cx = cx!(now);
                    app.key(&mut cx, input);
                    settle!(cx);
                }
                DisplayEvent::Close => running = false,
            }
            for gesture in gestures {
                let mut cx = cx!(now);
                app.gesture(&mut cx, gesture);
                settle!(cx);
            }
        }
        let now = Instant::now();
        // Held presses: the lift at 150 ms, the long press at 600 ms.
        for gesture in recognizer.tick(now) {
            let mut cx = cx!(now);
            app.gesture(&mut cx, gesture);
            settle!(cx);
        }
        if wake.is_some_and(|at| at <= now) {
            wake = None;
        }
        {
            let mut cx = cx!(now);
            app.tick(&mut cx);
            settle!(cx);
        }

        // Tell the bridge what is shown.
        let showing = (app.viewing(), mapped);
        if connected && told.as_ref() != Some(&showing) {
            link.request("view", json!({"chat": showing.0, "front": showing.1}));
            told = Some(showing);
        }
        if mapped && now >= next_shell_poll {
            shell.ask();
            next_shell_poll = now + SHELL_POLL;
        }

        // Draw.
        let ready = renderer.ready();
        if let Output::Wayland(wayland) = &mut renderer.output
            && let Err(error) = wayland.retry(&renderer.canvas)
        {
            renderer.fall_back(error)?;
            mapped = false;
            redraw = true;
        }
        if redraw && mapped && ready && running {
            let full = renderer.full;
            let mut dcx = DrawCx {
                media: &mut media,
                now,
                full,
                damage: None,
            };
            let started = Instant::now();
            app.draw(&mut renderer.canvas, &mut dcx);
            let drawn = started.elapsed();
            // A frame drawn whole (a resize, a reopened window) is sent whole.
            let damage = if full { None } else { dcx.damage.take() };
            // Nothing on screen changed: no frame (the loop sleeps as usual).
            let unchanged = damage
                .as_ref()
                .is_some_and(|damage| damage.moved.is_none() && damage.rows.is_empty());
            if !unchanged && let Err(error) = renderer.present(damage) {
                renderer.fall_back(error)?;
                mapped = false;
                redraw = true;
                continue;
            }
            if profile && !unchanged {
                eprintln!(
                    "{id}: frame drawn in {:.2} ms (layout 0.00), presented in {:.2} ms ({})",
                    drawn.as_secs_f64() * 1000.0,
                    (started.elapsed() - drawn).as_secs_f64() * 1000.0,
                    if full { "full" } else { "partial" }
                );
                eprintln!(
                    "{id}: shown at {:.3} finger {}",
                    monotonic_ms(),
                    finger.map_or(-1, |y| (y + wayland::SCREEN_TOP).round() as i32)
                );
            }
            if std::mem::take(&mut transition) {
                control::page_drawn(|| match &mut renderer.output {
                    Output::X(x) => {
                        let _ = x.conn.get_input_focus().map(|cookie| cookie.reply());
                    }
                    Output::Wayland(wayland) => {
                        let _ = wayland.sync(Duration::from_millis(50));
                    }
                });
            }
            redraw = false;
        }
        // Pictures wanted while drawing are asked for now.
        if let Some(at) = media.pump(&link, connected, Instant::now()) {
            wake = Some(wake.map_or(at, |w| w.min(at)));
        }

        // Sleep until something comes or a deadline.
        let mut deadline: Option<Instant> = None;
        let mut soonest = |at: Instant| deadline = Some(deadline.map_or(at, |d| d.min(at)));
        if mapped {
            soonest(next_shell_poll);
        }
        if let Some(at) = wake {
            soonest(at);
        }
        if redraw && mapped && !renderer.ready() {
            if let Output::Wayland(wayland) = &renderer.output
                && let Some(at) = wayland.ready_at()
            {
                soonest(at);
            }
        } else if redraw && mapped {
            soonest(Instant::now());
        }
        if let Some(at) = recognizer.deadline() {
            soonest(at);
        }
        let mut timeout = deadline.map(|at| at.saturating_duration_since(Instant::now()));
        let mut guard = None;
        let display_fd = match &mut renderer.output {
            Output::X(x) => {
                x.conn.flush()?;
                x.conn.stream().as_raw_fd()
            }
            Output::Wayland(wayland) => {
                match wayland.prepare() {
                    Ok(read) => {
                        if read.is_none() {
                            timeout = Some(Duration::ZERO);
                        }
                        guard = read;
                    }
                    Err(error) => {
                        renderer.fall_back(error)?;
                        mapped = false;
                        redraw = true;
                        continue;
                    }
                }
                wayland.fd()
            }
        };
        if let Err(error) = renderer.read(&mut backlog) {
            renderer.fall_back(error)?;
            mapped = false;
            redraw = true;
            continue;
        }
        if !backlog.is_empty() {
            timeout = Some(Duration::ZERO);
        }
        let mut fds = [
            line::pollfd(display_fd, libc::POLLIN),
            line::pollfd(link.fd(), libc::POLLIN),
            line::pollfd(media.fd(), libc::POLLIN),
            line::pollfd(shell.waker.fd(), libc::POLLIN),
        ];
        slept_at = Instant::now();
        line::poll(&mut fds, timeout)?;
        if let Output::Wayland(wayland) = &mut renderer.output
            && let Err(error) = wayland.after_poll(guard, fds[0].revents != 0)
        {
            renderer.fall_back(error)?;
            mapped = false;
            redraw = true;
        }
    }

    if keyboard {
        control::hide_keyboard();
    }
    if let Output::X(x) = &renderer.output {
        x.presenter.destroy(&x.conn);
        let _ = x.conn.flush();
    }
    Ok(())
}
