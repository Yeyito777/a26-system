mod app;
mod control;
mod font;
mod metrics;
mod ui;
mod wifi;

use std::env;
use std::error::Error;
use std::thread;
use std::time::{Duration, Instant};

use app::AppModel;
use metrics::SystemInfo;
use ui::Renderer;
use wifi::{WifiRequest, WifiWorker};
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, PropMode, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const APP_NAME: &str = "a26-system";
const REQUESTED_WIDTH: u16 = 1080;
const REQUESTED_HEIGHT: u16 = 2340;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const WIFI_STATUS_INTERVAL: Duration = Duration::from_secs(5);
const EVENT_LOOP_SLEEP: Duration = Duration::from_millis(16);
const TAP_MOVEMENT_LIMIT: i16 = 32;
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

    let (conn, screen_number) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_number];
    let window = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        screen.root,
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
                    | EventMask::POINTER_MOTION,
            ),
    )?;

    let protocols = configure_window_metadata(&conn, window)?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, window, &CreateGCAux::new().graphics_exposures(0))?;

    // Mapping a normal window lets the window manager intercept MapRequest and
    // apply its own fullscreen placement policy.
    conn.map_window(window)?;
    conn.flush()?;

    let mut renderer = Renderer::new(window, gc, REQUESTED_WIDTH, REQUESTED_HEIGHT);
    let mut system = SystemInfo::collect();
    let mut volume = control::read_volume();
    let mut app = AppModel::default();
    let wifi_worker = WifiWorker::spawn();
    let mut tap = None;
    let mut redraw = true;
    let mut running = true;
    let mut next_refresh = Instant::now() + REFRESH_INTERVAL;
    let mut next_wifi_status = Instant::now() + WIFI_STATUS_INTERVAL;

    while running {
        while let Some(event) = wifi_worker.try_recv() {
            app.apply_wifi_event(event);
            redraw = true;
        }

        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::Expose(event) if event.window == window && event.count == 0 => {
                    redraw = true;
                }
                Event::ConfigureNotify(event) if event.window == window => {
                    renderer.resize(event.width, event.height);
                    tap = None;
                    redraw = true;
                }
                Event::ButtonPress(event)
                    if event.event == window && event.detail == 1 && tap.is_none() =>
                {
                    tap = TapTracker::new(event.event_x, event.event_y, renderer.height());
                }
                Event::MotionNotify(event) if event.event == window => {
                    if let Some(tap) = &mut tap {
                        tap.observe(event.event_x, event.event_y);
                    }
                }
                Event::ButtonRelease(event) if event.event == window && event.detail == 1 => {
                    if let Some(position) = tap
                        .take()
                        .and_then(|tap| tap.finish(event.event_x, event.event_y, renderer.height()))
                    {
                        if let Some(action) = renderer.action_at(&app, position.0, position.1) {
                            let requests = app.handle(action);
                            dispatch_requests(&wifi_worker, &mut app, requests);
                            redraw = true;
                        }
                    }
                }
                Event::ClientMessage(event)
                    if event.window == window
                        && event.type_ == protocols.wm_protocols
                        && event.data.as_data32()[0] == protocols.wm_delete_window =>
                {
                    running = false;
                }
                Event::DestroyNotify(event) if event.window == window => {
                    running = false;
                }
                _ => {}
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

        if redraw && running {
            renderer.render(&conn, &system, volume, &app)?;
            redraw = false;
        }
        thread::sleep(EVENT_LOOP_SLEEP);
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
