mod control;
mod font;
mod metrics;
mod ui;

use std::env;
use std::error::Error;
use std::thread;
use std::time::{Duration, Instant};

use metrics::SystemInfo;
use ui::Renderer;
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
const EVENT_LOOP_SLEEP: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy)]
struct WindowProtocols {
    wm_protocols: u32,
    wm_delete_window: u32,
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
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
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
    let mut redraw = true;
    let mut running = true;
    let mut next_refresh = Instant::now() + REFRESH_INTERVAL;

    while running {
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::Expose(event) if event.window == window && event.count == 0 => {
                    redraw = true;
                }
                Event::ConfigureNotify(event) if event.window == window => {
                    renderer.resize(event.width, event.height);
                    redraw = true;
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

        if redraw && running {
            renderer.render(&conn, &system, volume)?;
            redraw = false;
        }
        thread::sleep(EVENT_LOOP_SLEEP);
    }

    Ok(())
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
        return Err(format!("unknown argument: {argument}").into());
    }
    Ok(())
}
