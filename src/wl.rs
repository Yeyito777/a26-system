//! The System app as a native Wayland client of Moon.
//!
//! One full-screen xdg toplevel. The canvas is copied into one of two
//! wl_shm buffers on each render (renders happen on change or once a
//! second, never in a loop). Touch arrives as wl_touch and drives the same
//! tap logic as X11 pointer events. Moon's keyboard arrives through
//! text-input-v3: enabling it with the field's purpose (text, URL,
//! password) is how this app asks for the keyboard, typed characters come
//! as committed text, Backspace as a deleted character and Return as a key.
//! Nothing typed is logged.

use std::error::Error;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::time::Duration;

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
    wl_touch,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::Input;
use crate::control::KeyboardPurpose;
use crate::gfx::Canvas;
use crate::keys::KeyInput;

/// The text input of a Wayland session, for control::show_keyboard.
static TEXT_INPUT: Mutex<Option<(zwp_text_input_v3::ZwpTextInputV3, Connection)>> =
    Mutex::new(None);

/// Enables text input with a purpose; None when this is not a Wayland
/// session (the caller uses Moon's control socket instead).
pub fn show_keyboard(purpose: KeyboardPurpose) -> Option<bool> {
    let guard = TEXT_INPUT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (input, connection) = guard.as_ref()?;
    input.enable();
    input.set_content_type(
        zwp_text_input_v3::ContentHint::None,
        match purpose {
            KeyboardPurpose::Text => zwp_text_input_v3::ContentPurpose::Normal,
            KeyboardPurpose::Url => zwp_text_input_v3::ContentPurpose::Url,
            KeyboardPurpose::Password => zwp_text_input_v3::ContentPurpose::Password,
        },
    );
    input.commit();
    let _ = connection.flush();
    Some(true)
}

/// Disables text input; false when this is not a Wayland session.
pub fn hide_keyboard() -> bool {
    let guard = TEXT_INPUT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Some((input, connection)) = guard.as_ref() else {
        return false;
    };
    input.disable();
    input.commit();
    let _ = connection.flush();
    true
}

struct Slot {
    buffer: wl_buffer::WlBuffer,
    offset: usize,
    busy: bool,
}

#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    text_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    /// The toplevel's size in surface units, and the buffer scale Moon
    /// prefers (wl_surface v6): the canvas is `logical * scale` pixels.
    logical: (i32, i32),
    scale: i32,
    configured: bool,
    inputs: Vec<Input>,
    slots: Vec<Slot>,
    /// The touch point being followed (the first finger only, as on X11).
    touch: Option<(i32, f64, f64)>,
    pending_text: String,
    pending_delete: u32,
}

pub struct WlBackend {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    surface: wl_surface::WlSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    pool: Option<(wl_shm_pool::WlShmPool, Map)>,
}

impl WlBackend {
    pub fn connect(app_id: &str) -> Result<Self, Box<dyn Error>> {
        let connection = Connection::connect_to_env()?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        connection.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state)?;
        let (Some(compositor), Some(wm)) = (state.compositor.clone(), state.wm.clone()) else {
            return Err("the compositor lacks wl_compositor or xdg_wm_base".into());
        };
        if state.shm.is_none() {
            return Err("the compositor lacks wl_shm".into());
        }
        let surface = compositor.create_surface(&qh, ());
        let xdg = wm.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title(app_id.into());
        toplevel.set_app_id(app_id.into());
        toplevel.set_fullscreen(None);
        surface.commit();
        if let (Some(manager), Some(seat)) = (&state.text_manager, &state.seat) {
            let input = manager.get_text_input(seat, &qh, ());
            *TEXT_INPUT
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some((input, connection.clone()));
        }
        while !state.configured {
            queue.blocking_dispatch(&mut state)?;
        }
        // The preferred scale arrives with the surface; one more roundtrip
        // makes sure it is in before the first frame is sized.
        queue.roundtrip(&mut state)?;
        if state.logical.0 <= 0 || state.logical.1 <= 0 {
            state.logical = (
                i32::from(crate::REQUESTED_WIDTH),
                i32::from(crate::REQUESTED_HEIGHT),
            );
        }
        state.inputs.clear();
        Ok(Self {
            connection,
            queue,
            state,
            surface,
            _toplevel: toplevel,
            pool: None,
        })
    }

    pub fn size(&self) -> (u16, u16) {
        let scale = self.state.scale.max(1);
        (
            (self.state.logical.0 * scale).clamp(1, 8192) as u16,
            (self.state.logical.1 * scale).clamp(1, 8192) as u16,
        )
    }

    pub fn inputs(&mut self) -> Vec<Input> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        std::mem::take(&mut self.state.inputs)
    }

    /// Copies the canvas into a free buffer and shows it.
    pub fn present(&mut self, canvas: &Canvas) -> Result<(), Box<dyn Error>> {
        let (width, height) = (canvas.width, canvas.height);
        if width <= 0 || height <= 0 {
            return Ok(());
        }
        self.ensure_pool(width, height)?;
        // Both buffers busy (Moon has not copied the last frame yet): wait a
        // little for a release rather than tearing a buffer in use.
        for _ in 0..50 {
            if self.state.slots.iter().any(|slot| !slot.busy) {
                break;
            }
            self.wait(Duration::from_millis(4))?;
        }
        let Some(index) = self.state.slots.iter().position(|slot| !slot.busy) else {
            return Ok(());
        };
        let offset = self.state.slots[index].offset;
        let (_, map) = self.pool.as_mut().expect("pool exists");
        let pixels = map.pixels(offset, (width * height) as usize);
        pixels.copy_from_slice(&canvas.pixels);
        let slot = &mut self.state.slots[index];
        slot.busy = true;
        self.surface.set_buffer_scale(self.state.scale.max(1));
        self.surface.attach(Some(&slot.buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
        self.connection.flush()?;
        Ok(())
    }

    fn ensure_pool(&mut self, width: i32, height: i32) -> Result<(), Box<dyn Error>> {
        let slot_size = ((width * height * 4) as usize).div_ceil(4096) * 4096;
        if let Some((_, map)) = &self.pool
            && map.len == slot_size * 2
            && self.state.slots.len() == 2
        {
            return Ok(());
        }
        for slot in self.state.slots.drain(..) {
            slot.buffer.destroy();
        }
        if let Some((pool, _)) = self.pool.take() {
            pool.destroy();
        }
        let name = c"a26-system";
        // SAFETY: a fresh memfd, owned below. Sealing is allowed so the
        // compositor can protect itself against a shrinking pool.
        let raw = unsafe {
            libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)
        };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: raw is a new descriptor nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let file = std::fs::File::from(fd);
        file.set_len((slot_size * 2) as u64)?;
        let fd = OwnedFd::from(file);
        let qh = self.queue.handle();
        let shm = self.state.shm.as_ref().expect("checked at connect");
        let pool = shm.create_pool(fd.as_fd(), (slot_size * 2) as i32, &qh, ());
        for index in 0..2 {
            let buffer = pool.create_buffer(
                (index * slot_size) as i32,
                width,
                height,
                width * 4,
                wl_shm::Format::Xrgb8888,
                &qh,
                index,
            );
            self.state.slots.push(Slot {
                buffer,
                offset: index * slot_size,
                busy: false,
            });
        }
        let map = Map::new(fd, slot_size * 2)?;
        self.pool = Some((pool, map));
        Ok(())
    }

    /// Flushes, then waits up to `timeout` for events and reads them.
    pub fn wait(&mut self, timeout: Duration) -> std::io::Result<()> {
        let _ = self.connection.flush();
        let _ = self.queue.dispatch_pending(&mut self.state);
        if !self.state.inputs.is_empty() {
            return Ok(());
        }
        let Some(guard) = self.queue.prepare_read() else {
            return Ok(());
        };
        let mut descriptor = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let milliseconds = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: one initialized pollfd that outlives the call.
        let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
        if result > 0 {
            if let Err(error) = guard.read() {
                return Err(std::io::Error::other(error.to_string()));
            }
            let _ = self.queue.dispatch_pending(&mut self.state);
        } else if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        Ok(())
    }
}

struct Map {
    _fd: OwnedFd,
    ptr: *mut u32,
    len: usize,
}

impl Map {
    fn new(fd: OwnedFd, len: usize) -> std::io::Result<Self> {
        // SAFETY: a shared mapping of a memfd sized to `len`.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            _fd: fd,
            ptr: ptr.cast(),
            len,
        })
    }

    fn pixels(&mut self, offset: usize, count: usize) -> &mut [u32] {
        assert!(offset + count * 4 <= self.len && offset % 4 == 0);
        // SAFETY: inside the mapping, aligned, and only written by this
        // process while the buffer is not attached.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(offset / 4), count) }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what new() mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

fn clamp16(value: f64) -> i16 {
    value
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(6), qh, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm = Some(registry.bind(name, version.min(6), qh, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
                "zwp_text_input_manager_v3" => {
                    state.text_manager = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 {
                    state.logical = (width, height);
                }
            }
            xdg_toplevel::Event::Close => state.inputs.push(Input::Close),
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event
            && let Some(slot) = state.slots.get_mut(*index)
        {
            slot.busy = false;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Touch) {
                seat.get_touch(qh, ());
            }
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_touch::WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_touch::Event::Down { id, x, y, .. } if state.touch.is_none() => {
                let scale = f64::from(state.scale.max(1));
                let (x, y) = (x * scale, y * scale);
                state.touch = Some((id, x, y));
                state.inputs.push(Input::Press(clamp16(x), clamp16(y)));
            }
            wl_touch::Event::Motion { id, x, y, .. } => {
                let scale = f64::from(state.scale.max(1));
                let (x, y) = (x * scale, y * scale);
                if let Some(touch) = state.touch.as_mut().filter(|touch| touch.0 == id) {
                    touch.1 = x;
                    touch.2 = y;
                    state.inputs.push(Input::Motion(clamp16(x), clamp16(y)));
                }
            }
            wl_touch::Event::Up { id, .. } => {
                if let Some((_, x, y)) = state.touch.take_if(|touch| touch.0 == id) {
                    state.inputs.push(Input::Release(clamp16(x), clamp16(y)));
                }
            }
            wl_touch::Event::Cancel => {
                // Moon took the touch (the home gesture): no tap. A release
                // far outside any control ends the tap tracker harmlessly.
                if state.touch.take().is_some() {
                    state.inputs.push(Input::Motion(i16::MIN / 2, i16::MIN / 2));
                    state
                        .inputs
                        .push(Input::Release(i16::MIN / 2, i16::MIN / 2));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            // Text arrives through text-input; only these keys matter.
            match key {
                28 | 96 => state.inputs.push(Input::Key(KeyInput::Enter)),
                14 => state.inputs.push(Input::Key(KeyInput::Backspace)),
                _ => {}
            }
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::CommitString { text: Some(text) } => {
                state.pending_text.push_str(&text);
            }
            zwp_text_input_v3::Event::DeleteSurroundingText { before_length, .. } => {
                state.pending_delete += before_length;
            }
            zwp_text_input_v3::Event::Done { .. } => {
                for _ in 0..std::mem::take(&mut state.pending_delete) {
                    state.inputs.push(Input::Key(KeyInput::Backspace));
                }
                let text = std::mem::take(&mut state.pending_text);
                for character in text.chars() {
                    state
                        .inputs
                        .push(Input::Key(KeyInput::Character(character)));
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.scale = factor.max(1);
        }
    }
}
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);
