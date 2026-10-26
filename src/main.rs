mod app;
mod clock;
mod clock_ui;
mod control;
mod gfx;
mod keys;
mod metrics;
mod present;
mod text;
mod ui;
mod wifi;
mod wl;

use std::env;
use std::error::Error;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use app::{Action, AppModel, Field, KeyAction};
use control::KeyboardPurpose;
use keys::{KeyInput, KeyMap};
use metrics::SystemInfo;
use ui::Renderer;
use wifi::{WifiRequest, WifiWorker};
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const APP_NAME: &str = "a26-system";
const REQUESTED_WIDTH: u16 = 1080;
const REQUESTED_HEIGHT: u16 = 2340;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const WIFI_STATUS_INTERVAL: Duration = Duration::from_secs(5);
/// While Wi-Fi work is in flight, wake this often to collect worker results
/// and step the activity indicator. Otherwise the loop blocks until X input
/// or the next one-second metrics refresh.
const BUSY_POLL_INTERVAL: Duration = Duration::from_millis(125);
const TAP_MOVEMENT_LIMIT: i16 = 32;
/// How often the Clock page looks for daemon events.
const CLOCK_POLL_INTERVAL: Duration = Duration::from_millis(200);
const BOTTOM_GESTURE_RESERVE: i16 = 170;

#[derive(Debug, Clone, Copy)]
struct WindowProtocols {
    wm_protocols: u32,
    wm_delete_window: u32,
}

#[derive(Debug, Clone, Copy)]
struct TapTracker {
    start_x: i16,
    start_y: i16,
    moved: bool,
}

impl TapTracker {
    fn new(x: i16, y: i16, window_height: u16) -> Option<Self> {
        if in_bottom_gesture_area(y, window_height) {
            return None;
        }
        Some(Self {
            start_x: x,
            start_y: y,
            moved: false,
        })
    }

    fn observe(&mut self, x: i16, y: i16) {
        let distance_x = i32::from(x) - i32::from(self.start_x);
        let distance_y = i32::from(y) - i32::from(self.start_y);
        let limit = i32::from(TAP_MOVEMENT_LIMIT);
        if distance_x * distance_x + distance_y * distance_y > limit * limit {
            self.moved = true;
        }
    }

    fn finish(mut self, x: i16, y: i16, window_height: u16) -> Option<(i16, i16)> {
        self.observe(x, y);
        (!self.moved && !in_bottom_gesture_area(y, window_height)).then_some((x, y))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    parse_arguments()?;
    disable_process_dumps()?;
    // Under Moon's Wayland server (WAYLAND_DISPLAY set) the app is a native
    // Wayland client; otherwise, or if that fails, it is the X11 app it
    // always was.
    let backend = if env::var_os("WAYLAND_DISPLAY").is_some() {
        match wl::WlBackend::connect(APP_NAME) {
            Ok(backend) => Backend::Wayland(backend),
            Err(error) => {
                eprintln!("{APP_NAME}: Wayland unavailable ({error}); using X11");
                Backend::X11(XBackend::open()?)
            }
        }
    } else {
        Backend::X11(XBackend::open()?)
    };
    run(backend)
}

/// What the display delivered, in window pixels.
#[derive(Debug, Clone, Copy)]
enum Input {
    Press(i16, i16),
    Motion(i16, i16),
    Release(i16, i16),
    Key(KeyInput),
    /// The window was configured; `resized` when its size changed.
    Configured {
        resized: bool,
    },
    Close,
}

struct XBackend {
    conn: RustConnection,
    window: Window,
    depth: u8,
    protocols: WindowProtocols,
    key_map: KeyMap,
    mapped: bool,
}

impl XBackend {
    fn open() -> Result<Self, Box<dyn Error>> {
        let (conn, screen_number) = RustConnection::connect(None)?;
        let screen = &conn.setup().roots[screen_number];
        let depth = screen.root_depth;
        let root = screen.root;
        let window = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            REQUESTED_WIDTH,
            REQUESTED_HEIGHT,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(ui::BG)
                .override_redirect(0)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::POINTER_MOTION
                        | EventMask::KEY_PRESS,
                ),
        )?;
        let protocols = configure_window_metadata(&conn, window)?;
        let key_map = KeyMap::query(&conn)?;
        Ok(Self {
            conn,
            window,
            depth,
            protocols,
            key_map,
            mapped: false,
        })
    }

    fn inputs(&mut self, renderer: &mut Renderer) -> Result<Vec<Input>, Box<dyn Error>> {
        let mut inputs = Vec::new();
        let window = self.window;
        while let Some(event) = self.conn.poll_for_event()? {
            match event {
                Event::Expose(event) if event.window == window && event.count == 0 => {
                    renderer.expose(&self.conn)?;
                }
                Event::ConfigureNotify(event) if event.window == window => {
                    let resized = renderer.size() != (event.width, event.height);
                    if resized {
                        renderer.resize(Some(&self.conn), event.width, event.height)?;
                    }
                    inputs.push(Input::Configured { resized });
                }
                Event::ButtonPress(event) if event.event == window && event.detail == 1 => {
                    inputs.push(Input::Press(event.event_x, event.event_y));
                }
                Event::MotionNotify(event) if event.event == window => {
                    inputs.push(Input::Motion(event.event_x, event.event_y));
                }
                Event::ButtonRelease(event) if event.event == window && event.detail == 1 => {
                    inputs.push(Input::Release(event.event_x, event.event_y));
                }
                Event::KeyPress(event) if event.event == window => {
                    // Keys come from Moon's system keyboard (XTEST).
                    if let Some(input) = self.key_map.decode(event.detail, event.state) {
                        inputs.push(Input::Key(input));
                    }
                }
                Event::ClientMessage(event)
                    if event.window == window
                        && event.type_ == self.protocols.wm_protocols
                        && event.data.as_data32()[0] == self.protocols.wm_delete_window =>
                {
                    inputs.push(Input::Close);
                }
                Event::DestroyNotify(event) if event.window == window => {
                    inputs.push(Input::Close);
                }
                _ => {}
            }
        }
        Ok(inputs)
    }
}

enum Backend {
    X11(XBackend),
    Wayland(wl::WlBackend),
}

impl Backend {
    fn renderer(&mut self) -> Result<Renderer, Box<dyn Error>> {
        match self {
            Self::X11(x) => Renderer::new(
                &x.conn,
                x.window,
                x.depth,
                REQUESTED_WIDTH,
                REQUESTED_HEIGHT,
            ),
            Self::Wayland(wl) => {
                let (width, height) = wl.size();
                Ok(Renderer::offscreen(width, height))
            }
        }
    }

    fn inputs(&mut self, renderer: &mut Renderer) -> Result<Vec<Input>, Box<dyn Error>> {
        match self {
            Self::X11(x) => x.inputs(renderer),
            Self::Wayland(wl) => {
                let mut inputs = wl.inputs();
                let size = wl.size();
                if renderer.size() != size {
                    renderer.resize::<RustConnection>(None, size.0, size.1)?;
                    inputs.insert(0, Input::Configured { resized: true });
                }
                Ok(inputs)
            }
        }
    }

    fn render(
        &mut self,
        renderer: &mut Renderer,
        system: &SystemInfo,
        volume: Option<u8>,
        app: &AppModel,
        pressed: Option<Action>,
    ) -> Result<(), Box<dyn Error>> {
        match self {
            Self::X11(x) => {
                renderer.render(&x.conn, system, volume, app, pressed)?;
                if !x.mapped {
                    // The first complete frame is drawn while the window is
                    // still unmapped, so Moon can reveal this fast-starting
                    // app directly instead of an empty X11 background.
                    x.conn.map_window(x.window)?;
                    x.conn.flush()?;
                    x.mapped = true;
                }
                Ok(())
            }
            Self::Wayland(wl) => {
                let canvas = renderer.draw(system, volume, app, pressed);
                wl.present(canvas)
            }
        }
    }

    fn wait(&mut self, timeout: Duration) -> std::io::Result<()> {
        match self {
            Self::X11(x) => wait_for_x(&x.conn, timeout),
            Self::Wayland(wl) => wl.wait(timeout),
        }
    }

    fn finish(&mut self, renderer: &Renderer) {
        if let Self::X11(x) = self {
            renderer.destroy(&x.conn);
            let _ = x.conn.destroy_window(x.window);
            let _ = x.conn.flush();
        }
    }
}

fn run(mut backend: Backend) -> Result<(), Box<dyn Error>> {
    let mut renderer = backend.renderer()?;
    let mut system = SystemInfo::collect();
    let mut volume = control::read_volume();
    let mut app = AppModel::default();
    let wifi_worker = WifiWorker::spawn();
    let clock_worker = clock::NotifyWorker::spawn();
    // The System page summarizes the current network, so ask once up front.
    dispatch_requests(&wifi_worker, &mut app, vec![WifiRequest::Status]);
    let mut tap = None;
    let mut pressed: Option<Action> = None;
    let mut redraw = false;
    let mut running = true;
    let mut keyboard_requested: Option<KeyboardPurpose> = None;
    let mut next_refresh = Instant::now() + REFRESH_INTERVAL;
    let mut next_wifi_status = Instant::now() + WIFI_STATUS_INTERVAL;
    let mut next_busy_frame = Instant::now();

    backend.render(&mut renderer, &system, volume, &app, None)?;

    while running {
        while let Some(event) = wifi_worker.try_recv() {
            app.apply_wifi_event(event);
            redraw = true;
        }
        while let Some(update) = clock_worker.try_recv() {
            app.clock.apply(update);
            redraw = true;
        }

        for input in backend.inputs(&mut renderer)? {
            match input {
                Input::Configured { resized } => {
                    redraw |= resized;
                    tap = None;
                    pressed = None;
                }
                Input::Press(x, y) if tap.is_none() => {
                    tap = TapTracker::new(x, y, renderer.height());
                    if tap.is_some() {
                        pressed = renderer.action_at(&app, x, y);
                        redraw |= pressed.is_some();
                    }
                }
                Input::Press(..) => {}
                Input::Motion(x, y) => {
                    if let Some(tap) = &mut tap {
                        tap.observe(x, y);
                        if tap.moved && pressed.take().is_some() {
                            redraw = true;
                        }
                    }
                }
                Input::Release(x, y) => {
                    if pressed.take().is_some() {
                        redraw = true;
                    }
                    if let Some(position) = tap
                        .take()
                        .and_then(|tap| tap.finish(x, y, renderer.height()))
                    {
                        if let Some(action) = renderer.action_at(&app, position.0, position.1) {
                            perform(
                                &wifi_worker,
                                &clock_worker,
                                &mut app,
                                action,
                                &mut keyboard_requested,
                            );
                            redraw = true;
                        }
                    }
                }
                Input::Key(input) => {
                    // Keys come from Moon's system keyboard; only the password
                    // field and the Clock page's label accept them.
                    if app.password_dialog.is_none() && app.clock.label_focused() {
                        app.clock.type_text(input);
                        sync_keyboard(&mut app, None, &mut keyboard_requested);
                        redraw = true;
                    } else if app.password_dialog.is_some() {
                        let action = match input {
                            KeyInput::Character(character) => {
                                Action::Key(KeyAction::Character(character))
                            }
                            KeyInput::Backspace => Action::Key(KeyAction::Backspace),
                            KeyInput::Enter => Action::Return,
                        };
                        perform(
                            &wifi_worker,
                            &clock_worker,
                            &mut app,
                            action,
                            &mut keyboard_requested,
                        );
                        redraw = true;
                    }
                }
                Input::Close => running = false,
            }
        }

        let now = Instant::now();
        if now >= next_refresh {
            system.refresh();
            volume = control::read_volume();
            redraw = true;
            next_refresh = now + REFRESH_INTERVAL;
        }
        if now >= next_wifi_status {
            if app.may_poll_status() {
                dispatch_requests(&wifi_worker, &mut app, vec![WifiRequest::Status]);
            }
            next_wifi_status = now + WIFI_STATUS_INTERVAL;
        }
        if app.next_deadline().is_some_and(|at| now >= at) {
            let requests = app.tick(now);
            dispatch_requests(&wifi_worker, &mut app, requests);
            redraw = true;
        }
        let busy = app.busy || app.scanning;
        if busy && now >= next_busy_frame {
            redraw = true;
            next_busy_frame = now + BUSY_POLL_INTERVAL;
        }

        if redraw && running {
            backend.render(&mut renderer, &system, volume, &app, pressed)?;
            redraw = false;
        }

        let mut deadline = next_refresh.min(next_wifi_status);
        if busy {
            deadline = deadline.min(next_busy_frame);
        }
        if let Some(at) = app.next_deadline() {
            deadline = deadline.min(at);
        }
        if app.view == app::View::Clock {
            // Daemon events arrive on the worker thread; pick them up soon.
            deadline = deadline.min(Instant::now() + CLOCK_POLL_INTERVAL);
        }
        backend.wait(deadline.saturating_duration_since(Instant::now()))?;
    }

    if keyboard_requested.is_some() {
        control::hide_keyboard();
    }
    backend.finish(&renderer);
    Ok(())
}

/// Applies a UI action and keeps Moon's system keyboard in step with the
/// password field.
fn perform(
    worker: &WifiWorker,
    clock_worker: &clock::NotifyWorker,
    app: &mut AppModel,
    action: Action,
    keyboard_requested: &mut Option<KeyboardPurpose>,
) {
    if let Action::Clock(clock_action) = action {
        for request in app.clock.handle(clock_action) {
            clock_worker.send(request);
        }
    }
    let requests = app.handle(action);
    dispatch_requests(worker, app, requests);
    sync_keyboard(app, Some(action), keyboard_requested);
}

/// Keeps Moon's system keyboard up exactly while a text field has focus.
fn sync_keyboard(
    app: &mut AppModel,
    action: Option<Action>,
    keyboard_requested: &mut Option<KeyboardPurpose>,
) {
    if app.password_dialog.is_none() && app.clock.label_focused() {
        let refocused = action == Some(Action::Clock(clock::ClockAction::FocusLabel));
        if refocused || *keyboard_requested != Some(KeyboardPurpose::Text) {
            let shown = control::show_keyboard(KeyboardPurpose::Text);
            app.clock.set_system_keyboard(shown);
            if shown {
                *keyboard_requested = Some(KeyboardPurpose::Text);
            }
        }
        return;
    }
    let Some(action) = action else {
        if app.password_dialog.is_none() && keyboard_requested.is_some() {
            control::hide_keyboard();
            *keyboard_requested = None;
        }
        return;
    };
    match app.password_dialog.as_mut() {
        Some(dialog) => {
            let purpose = match dialog.focus {
                Field::Identity => KeyboardPurpose::Text,
                Field::Domain => KeyboardPurpose::Url,
                Field::Password => KeyboardPurpose::Password,
            };
            // Moon hides its keyboard on Return, so a Return that keeps the
            // page open (next field, or an invalid entry) asks for it again.
            let wants_keyboard = !dialog.system_keyboard
                || *keyboard_requested != Some(purpose)
                || matches!(action, Action::Focus(_) | Action::Return)
                || (action == Action::SubmitPassword && dialog.submit_attempted);
            if wants_keyboard {
                dialog.system_keyboard = control::show_keyboard(purpose);
                if dialog.system_keyboard {
                    *keyboard_requested = Some(purpose);
                }
            }
        }
        None if keyboard_requested.is_some() => {
            control::hide_keyboard();
            *keyboard_requested = None;
        }
        None => {}
    }
}

fn wait_for_x(conn: &RustConnection, timeout: Duration) -> std::io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: conn.stream().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let milliseconds = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: one initialized pollfd that outlives the call.
    let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

fn disable_process_dumps() -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: setrlimit reads the complete local rlimit value; prctl is called
    // with PR_SET_DUMPABLE and scalar arguments only.
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } != 0
        || unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn dispatch_requests(worker: &WifiWorker, app: &mut AppModel, requests: Vec<WifiRequest>) {
    for request in requests {
        if !worker.send(request) {
            app.worker_unavailable();
            break;
        }
    }
}

fn in_bottom_gesture_area(y: i16, window_height: u16) -> bool {
    i32::from(y) >= i32::from(window_height).saturating_sub(i32::from(BOTTOM_GESTURE_RESERVE))
}

fn configure_window_metadata(
    conn: &RustConnection,
    window: Window,
) -> Result<WindowProtocols, Box<dyn Error>> {
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        APP_NAME.as_bytes(),
    )?;

    let mut class = Vec::with_capacity(APP_NAME.len() * 2 + 2);
    class.extend_from_slice(APP_NAME.as_bytes());
    class.push(0);
    class.extend_from_slice(APP_NAME.as_bytes());
    class.push(0);
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        &class,
    )?;

    let utf8_string = intern_atom(conn, b"UTF8_STRING")?;
    let net_wm_name = intern_atom(conn, b"_NET_WM_NAME")?;
    conn.change_property8(
        PropMode::REPLACE,
        window,
        net_wm_name,
        utf8_string,
        APP_NAME.as_bytes(),
    )?;

    let net_wm_state = intern_atom(conn, b"_NET_WM_STATE")?;
    let net_wm_state_fullscreen = intern_atom(conn, b"_NET_WM_STATE_FULLSCREEN")?;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        net_wm_state,
        AtomEnum::ATOM,
        &[net_wm_state_fullscreen],
    )?;

    let wm_protocols = intern_atom(conn, b"WM_PROTOCOLS")?;
    let wm_delete_window = intern_atom(conn, b"WM_DELETE_WINDOW")?;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        wm_protocols,
        AtomEnum::ATOM,
        &[wm_delete_window],
    )?;

    Ok(WindowProtocols {
        wm_protocols,
        wm_delete_window,
    })
}

fn intern_atom(conn: &RustConnection, name: &[u8]) -> Result<u32, Box<dyn Error>> {
    Ok(conn.intern_atom(false, name)?.reply()?.atom)
}

fn parse_arguments() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    if let Some(argument) = arguments.next() {
        if matches!(argument.as_str(), "--help" | "-h") && arguments.next().is_none() {
            println!("usage: {APP_NAME}");
            std::process::exit(0);
        }
        // Never echo arbitrary argv: credentials are not accepted here, and an
        // accidental secret-shaped argument must not be copied to diagnostics.
        return Err(format!("unexpected argument; usage: {APP_NAME}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stationary_pointer_sequence_is_a_tap() {
        let tracker = TapTracker::new(500, 700, REQUESTED_HEIGHT).unwrap();
        assert_eq!(tracker.finish(510, 710, REQUESTED_HEIGHT), Some((510, 710)));
    }

    #[test]
    fn drag_and_bottom_edge_sequences_are_not_app_taps() {
        let mut tracker = TapTracker::new(500, 700, REQUESTED_HEIGHT).unwrap();
        tracker.observe(500, 760);
        assert_eq!(tracker.finish(500, 760, REQUESTED_HEIGHT), None);
        assert!(TapTracker::new(500, 2250, REQUESTED_HEIGHT).is_none());
    }
}
