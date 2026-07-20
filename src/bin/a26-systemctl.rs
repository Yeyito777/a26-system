use std::error::Error;

use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ButtonPressEvent, ConnectionExt as _,
    EventMask, KeyButMask, Window,
};
use x11rb::rust_connection::RustConnection;

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments
        .next()
        .ok_or("usage: a26-systemctl wifi|system|tap X Y")?;
    let (x, y) = match command.as_str() {
        "wifi" if arguments.next().is_none() => (900, 120),
        "system" if arguments.next().is_none() => (900, 120),
        "tap" => {
            let x = arguments.next().ok_or("tap requires X Y")?.parse::<i16>()?;
            let y = arguments.next().ok_or("tap requires X Y")?.parse::<i16>()?;
            if arguments.next().is_some() {
                return Err("tap requires exactly X Y".into());
            }
            (x, y)
        }
        _ => return Err("unsupported command".into()),
    };

    let (conn, screen_number) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_number].root;
    let window = find_system_window(&conn, root)?.ok_or("a26-system window not found")?;
    send_button(&conn, root, window, x, y, BUTTON_PRESS_EVENT)?;
    send_button(&conn, root, window, x, y, BUTTON_RELEASE_EVENT)?;
    conn.flush()?;
    Ok(())
}

fn find_system_window(
    conn: &RustConnection,
    root: Window,
) -> Result<Option<Window>, Box<dyn Error>> {
    for window in conn.query_tree(root)?.reply()?.children.into_iter().rev() {
        let property = conn
            .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 64)?
            .reply()?;
        if property.value == b"a26-system" {
            return Ok(Some(window));
        }
    }
    Ok(None)
}

fn send_button(
    conn: &RustConnection,
    root: Window,
    window: Window,
    x: i16,
    y: i16,
    response_type: u8,
) -> Result<(), Box<dyn Error>> {
    let event = ButtonPressEvent {
        response_type,
        detail: 1,
        sequence: 0,
        time: CURRENT_TIME,
        root,
        event: window,
        child: 0,
        root_x: x,
        root_y: y,
        event_x: x,
        event_y: y,
        state: KeyButMask::default(),
        same_screen: true,
    };
    conn.send_event(false, window, EventMask::NO_EVENT, event)?
        .check()?;
    Ok(())
}
