//! The app as a Wayland client of Moon (A; OSS-Phone-V1
//! `notes/wayland/MOON-WAYLAND.md`), when Moon names it in `wayland_apps`
//! and passes `WAYLAND_DISPLAY`.
//!
//! One full-screen xdg toplevel. Its buffers go to moon-compositor as they
//! are: a dma-heap buffer is shown in place (on a display plane), with none
//! of X's copies (the app's pixmap, its window, the X mirror). Touches come
//! from Moon as wl_touch, Moon's keyboard as wl_keyboard keys (the keyboard
//! is still asked for over Moon's control socket, as on X), and frames are
//! paced by frame callbacks, which Moon answers at the flip that showed the
//! last one. The app keeps drawing into its canvas as on X; each frame
//! copies into a free buffer what the canvas changed since that buffer was
//! last written.
//!
//! Every request is one-way except at connect and `sync`; a connection
//! that fails later is reported as an error, and the event loop goes back
//! to X with everything it holds.

use std::error::Error;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use wayland_client::backend::ReadEventsGuard;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface, wl_touch,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::gfx::{self, Canvas};
use crate::keys::KeyInput;

/// The app's area starts this far down the screen, under Moon's status
/// strip: touch rows are logged in screen pixels, as on X (profiling).
pub const SCREEN_TOP: f32 = 124.0;

/// A frame callback Moon has not answered by then (the app went to the
/// background before its frame was shown, a compositor gone away) no longer
/// holds frames back.
const FRAME_PATIENCE: Duration = Duration::from_millis(100);

/// Moon gets at most this many buffers: two alternate, a third only while
/// both are held (Moon still copying one through X, say).
const MAX_SLOTS: usize = 3;

/// What happened on the Wayland side, for the event loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Moon gave the app this size (pixels).
    Size(u16, u16),
    /// In front (true), or in the background or under a screen turned off
    /// (xdg `suspended`).
    Shown(bool),
    Press(f32, f32),
    Motion(f32, f32),
    Release(f32, f32),
    /// Moon took the touch over (its home gesture, the switcher).
    Cancel,
    Key(KeyInput),
    Close,
}

/// An area of the canvas (`x0, y0, x1, y1`).
pub type Area = (i32, i32, i32, i32);

/// Adds `new` to `areas`; past a handful they become their bounds (one
/// copy instead of many, and Moon merges them anyway).
fn add_area(areas: &mut Vec<Area>, new: Area) {
    if new.2 <= new.0 || new.3 <= new.1 {
        return;
    }
    if areas
        .iter()
        .any(|a| a.0 <= new.0 && a.1 <= new.1 && a.2 >= new.2 && a.3 >= new.3)
    {
        return;
    }
    areas.retain(|a| !(new.0 <= a.0 && new.1 <= a.1 && new.2 >= a.2 && new.3 >= a.3));
    areas.push(new);
    if areas.len() > 12 {
        let bounds = areas
            .iter()
            .copied()
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
            .unwrap_or(new);
        areas.clear();
        areas.push(bounds);
    }
}

/// One buffer and the memory behind it.
struct Slot {
    buffer: wl_buffer::WlBuffer,
    pool: wl_shm_pool::WlShmPool,
    map: Map,
    /// The buffer's size (pixels); the memory may hold more (a smaller
    /// window keeps its buffers' memory).
    width: i32,
    height: i32,
    /// Pixels per row (rows are padded to 64 bytes, as display planes and
    /// the GPU want them).
    stride: usize,
    /// Moon holds it: attached and not released yet.
    busy: bool,
    /// What the canvas changed since this buffer was last written.
    behind: Vec<Area>,
}

/// Why a callback fired.
#[derive(Debug, Clone, Copy)]
enum Callback {
    Frame,
    Sync,
}

#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    presentation: Option<wp_presentation::WpPresentation>,
    /// The toplevel's size in surface units and Moon's preferred scale:
    /// the canvas is `logical * scale` pixels.
    logical: (i32, i32),
    scale: i32,
    configured: bool,
    suspended: Option<bool>,
    closed: bool,
    events: Vec<Event>,
    slots: Vec<Slot>,
    /// The finger being followed (the first one only, as on X): its id and
    /// where it is, in pixels.
    touch: Option<(i32, f32, f32)>,
    shift: bool,
    /// A frame callback is outstanding, since then.
    frame_asked: Option<Instant>,
    synced: bool,
    /// Flips that showed a frame (CLOCK_MONOTONIC ms), for profiling.
    presented: Vec<(f64, i32)>,
}

impl State {
    fn scale(&self) -> f32 {
        self.scale.max(1) as f32
    }

    fn size(&self) -> (u16, u16) {
        let scale = self.scale.max(1);
        (
            (self.logical.0 * scale).clamp(1, 8192) as u16,
            (self.logical.1 * scale).clamp(1, 8192) as u16,
        )
    }
}

pub struct Wayland {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    surface: wl_surface::WlSurface,
    _xdg: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    /// The size last reported to the event loop.
    reported: (u16, u16),
    /// What the canvas changed since the last commit (Moon's damage).
    pending: Vec<Area>,
    /// A frame drawn but not handed over (no buffer was free).
    unpresented: bool,
    /// Buffers come from the dma heap (else memfds).
    dmabuf: bool,
}

impl Wayland {
    /// Connects to `WAYLAND_DISPLAY` and opens the app's toplevel. None
    /// when the variable is unset (Moon wants X).
    pub fn connect(app_id: &str) -> Option<Result<Self, Box<dyn Error>>> {
        std::env::var_os("WAYLAND_DISPLAY")?;
        Some(Self::open(app_id))
    }

    fn open(app_id: &str) -> Result<Self, Box<dyn Error>> {
        let connection = Connection::connect_to_env()?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        connection.display().get_registry(&qh, ());
        let mut state = State {
            scale: 1,
            ..State::default()
        };
        queue.roundtrip(&mut state)?;
        let (Some(compositor), Some(wm)) = (state.compositor.clone(), state.wm.clone()) else {
            return Err("Moon offers no wl_compositor or xdg_wm_base".into());
        };
        if state.shm.is_none() {
            return Err("Moon offers no wl_shm".into());
        }
        let surface = compositor.create_surface(&qh, ());
        let xdg = wm.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title(app_id.into());
        toplevel.set_app_id(app_id.into());
        toplevel.set_fullscreen(None);
        surface.commit();
        // The first configure, and the preferred scale with it.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !state.configured {
            if Instant::now() >= deadline {
                return Err("Moon did not configure the window".into());
            }
            wait(
                &connection,
                &mut queue,
                &mut state,
                Duration::from_millis(100),
            )?;
        }
        queue.roundtrip(&mut state)?;
        if state.logical.0 <= 0 || state.logical.1 <= 0 {
            return Err("Moon configured an empty window".into());
        }
        let dmabuf = std::env::var_os("EXOCORTEX_APP_MEMFD").is_none();
        let mut wayland = Self {
            connection,
            queue,
            state,
            surface,
            _xdg: xdg,
            _toplevel: toplevel,
            reported: (0, 0),
            pending: Vec::new(),
            unpresented: false,
            dmabuf,
        };
        wayland.reported = wayland.state.size();
        // What arrived with the configure is the starting state, not news.
        wayland.state.events.clear();
        Ok(wayland)
    }

    /// The size the canvas should have (pixels).
    pub fn size(&self) -> (u16, u16) {
        self.state.size()
    }

    /// Whether the app is in front (not `suspended`).
    pub fn shown(&self) -> bool {
        self.state.suspended != Some(true)
    }

    pub fn fd(&self) -> RawFd {
        self.connection.backend().poll_fd().as_raw_fd()
    }

    /// Before sleeping: sends what is queued, and starts a read unless
    /// events are already waiting (then the loop must not sleep).
    pub fn prepare(&mut self) -> Result<Option<ReadEventsGuard>, Box<dyn Error>> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.flush()?;
        if !self.state.events.is_empty() {
            return Ok(None);
        }
        Ok(self.queue.prepare_read())
    }

    /// After sleeping: reads what came (when the socket is readable).
    pub fn after_poll(
        &mut self,
        guard: Option<ReadEventsGuard>,
        readable: bool,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(guard) = guard
            && readable
        {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        if self.state.closed {
            self.state.events.push(Event::Close);
            self.state.closed = false;
        }
        Ok(())
    }

    /// What happened since the last call, in order. A size change comes
    /// first: input after it is in the new size.
    pub fn events(&mut self) -> Vec<Event> {
        let size = self.state.size();
        let mut events = Vec::new();
        if size != self.reported {
            self.reported = size;
            events.push(Event::Size(size.0, size.1));
        }
        events.append(&mut self.state.events);
        events
    }

    /// Whether a frame may be handed over now: the last one's callback has
    /// come (it is on the panel), or Moon is not answering it.
    pub fn ready(&self) -> bool {
        self.state
            .frame_asked
            .is_none_or(|at| at.elapsed() >= FRAME_PATIENCE)
    }

    /// When `ready` turns true without a callback.
    pub fn ready_at(&self) -> Option<Instant> {
        self.state.frame_asked.map(|at| at + FRAME_PATIENCE)
    }

    /// Flips that showed frames since the last call (CLOCK_MONOTONIC ms,
    /// and the finger row each frame was drawn for), for profiling.
    pub fn take_presented(&mut self) -> Vec<(f64, i32)> {
        std::mem::take(&mut self.state.presented)
    }

    /// The canvas changed in `damage` (everything when None): hands the
    /// frame to Moon in a free buffer. False when none is free (the frame
    /// goes out with the next call, or `retry`). `finger`: the screen row
    /// the frame was drawn for, logged when it is presented (profiling;
    /// asks Moon for presentation feedback).
    pub fn present(
        &mut self,
        canvas: &Canvas,
        damage: Option<&[Area]>,
        finger: Option<i32>,
    ) -> Result<bool, Box<dyn Error>> {
        let (width, height) = (canvas.width, canvas.height);
        let all = (0, 0, width, height);
        if self
            .state
            .slots
            .iter()
            .any(|slot| (slot.width, slot.height) != (width, height))
        {
            self.resize_slots(width, height)?;
        }
        let areas: Vec<Area> = match damage {
            Some(areas) => areas
                .iter()
                .map(|&(x0, y0, x1, y1)| (x0.max(0), y0.max(0), x1.min(width), y1.min(height)))
                .collect(),
            None => vec![all],
        };
        for &area in &areas {
            add_area(&mut self.pending, area);
            for slot in &mut self.state.slots {
                add_area(&mut slot.behind, area);
            }
        }
        if self.pending.is_empty() {
            return Ok(true);
        }
        let index = match self.state.slots.iter().position(|slot| !slot.busy) {
            Some(index) => index,
            None if self.state.slots.len() < MAX_SLOTS => {
                let slot = self.new_slot(width, height)?;
                self.state.slots.push(slot);
                self.state.slots.len() - 1
            }
            None => {
                self.unpresented = true;
                return Ok(false);
            }
        };
        let qh = self.queue.handle();
        let slot = &mut self.state.slots[index];
        let w = width as usize;
        for (x0, y0, x1, y1) in std::mem::take(&mut slot.behind) {
            let (x0, x1) = (x0 as usize, x1 as usize);
            let pixels = slot.map.pixels();
            for row in y0 as usize..y1 as usize {
                let from = &canvas.pixels[row * w + x0..row * w + x1];
                let to = &mut pixels[row * slot.stride + x0..row * slot.stride + x1];
                gfx::copy_pixels(to, from);
            }
        }
        slot.busy = true;
        let buffer = slot.buffer.clone();
        self.surface.set_buffer_scale(self.state.scale.max(1));
        self.surface.attach(Some(&buffer), 0, 0);
        for (x0, y0, x1, y1) in std::mem::take(&mut self.pending) {
            self.surface.damage_buffer(x0, y0, x1 - x0, y1 - y0);
        }
        self.surface.frame(&qh, Callback::Frame);
        if let (Some(presentation), Some(finger)) = (&self.state.presentation, finger) {
            presentation.feedback(&self.surface, &qh, finger);
        }
        self.surface.commit();
        self.state.frame_asked = Some(Instant::now());
        self.unpresented = false;
        self.flush()?;
        Ok(true)
    }

    /// Hands over a frame that found no free buffer, once one is.
    pub fn retry(&mut self, canvas: &Canvas) -> Result<(), Box<dyn Error>> {
        if self.unpresented && self.state.slots.iter().any(|slot| !slot.busy) {
            self.present(canvas, Some(&[]), None)?;
        }
        Ok(())
    }

    /// Waits (up to `timeout`) until Moon has handled everything sent so
    /// far: before `transition ready`, so Moon slides with the new page.
    pub fn sync(&mut self, timeout: Duration) -> Result<(), Box<dyn Error>> {
        let qh = self.queue.handle();
        self.state.synced = false;
        self.connection.display().sync(&qh, Callback::Sync);
        let deadline = Instant::now() + timeout;
        while !self.state.synced {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            wait(&self.connection, &mut self.queue, &mut self.state, left)?;
        }
        Ok(())
    }

    pub fn flush(&self) -> Result<(), Box<dyn Error>> {
        match self.connection.flush() {
            Ok(()) => Ok(()),
            Err(wayland_client::backend::WaylandError::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn drop_slots(&mut self) {
        for slot in self.state.slots.drain(..) {
            slot.buffer.destroy();
            slot.pool.destroy();
        }
    }

    /// Another size (Moon's keyboard came or went): new buffers, in the
    /// memory the old ones had when it is large enough.
    fn resize_slots(&mut self, width: i32, height: i32) -> Result<(), Box<dyn Error>> {
        let stride_bytes = stride_bytes(width);
        let qh = self.queue.handle();
        for (index, slot) in self.state.slots.iter_mut().enumerate() {
            if slot.stride * 4 != stride_bytes || stride_bytes * height as usize > slot.map.len {
                continue;
            }
            slot.buffer.destroy();
            slot.buffer = slot.pool.create_buffer(
                0,
                width,
                height,
                stride_bytes as i32,
                wl_shm::Format::Xrgb8888,
                &qh,
                index,
            );
            (slot.width, slot.height) = (width, height);
            slot.busy = false;
            slot.behind = vec![(0, 0, width, height)];
        }
        // Too small: allocated again when needed.
        if self
            .state
            .slots
            .iter()
            .any(|slot| (slot.width, slot.height) != (width, height))
        {
            self.drop_slots();
        }
        Ok(())
    }

    /// A buffer for a `width` x `height` canvas: a dma-heap buffer when the
    /// heap is there (Moon shows it in place), else a sealable memfd (Moon
    /// copies it to the compositor).
    fn new_slot(&mut self, width: i32, height: i32) -> Result<Slot, Box<dyn Error>> {
        let stride_bytes = stride_bytes(width);
        let len = (stride_bytes * height as usize).div_ceil(4096) * 4096;
        let fd = match self.dmabuf.then(|| dma_heap_alloc(len)) {
            Some(Ok(fd)) => fd,
            Some(Err(error)) => {
                eprintln!("exocortex: no dma-heap buffers ({error}); shared memory instead");
                self.dmabuf = false;
                memfd(len)?
            }
            None => memfd(len)?,
        };
        let map = Map::new(fd, len)?;
        let qh = self.queue.handle();
        let shm = self.state.shm.as_ref().ok_or("no wl_shm")?;
        let pool = shm.create_pool(map.fd.as_fd(), len as i32, &qh, ());
        let index = self.state.slots.len();
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            stride_bytes as i32,
            wl_shm::Format::Xrgb8888,
            &qh,
            index,
        );
        Ok(Slot {
            buffer,
            pool,
            map,
            width,
            height,
            stride: stride_bytes / 4,
            busy: false,
            behind: vec![(0, 0, width, height)],
        })
    }
}

impl Drop for Wayland {
    fn drop(&mut self) {
        self.drop_slots();
        let _ = self.connection.flush();
    }
}

/// Flushes, then waits up to `timeout` for events and dispatches them.
fn wait(
    connection: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    timeout: Duration,
) -> Result<(), Box<dyn Error>> {
    queue.dispatch_pending(state)?;
    let _ = connection.flush();
    let Some(guard) = queue.prepare_read() else {
        queue.dispatch_pending(state)?;
        return Ok(());
    };
    let mut descriptor = libc::pollfd {
        fd: guard.connection_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let milliseconds = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
    // SAFETY: one initialized pollfd that outlives the call.
    let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
    if result > 0 {
        match guard.read() {
            Ok(_) => {}
            Err(wayland_client::backend::WaylandError::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
    } else {
        drop(guard);
    }
    queue.dispatch_pending(state)?;
    Ok(())
}

/// Bytes per row of a `width`-pixel buffer: padded to 64 bytes, as display
/// planes and the GPU want them (1080 pixels: 4352, like the X mirror's).
fn stride_bytes(width: i32) -> usize {
    (width.max(1) as usize * 4).div_ceil(64) * 64
}

/// A buffer's memory, mapped.
struct Map {
    fd: OwnedFd,
    ptr: *mut u32,
    len: usize,
}

impl Map {
    fn new(fd: OwnedFd, len: usize) -> std::io::Result<Self> {
        // SAFETY: a shared mapping of `len` bytes of a descriptor sized to
        // at least that; checked for failure.
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
            fd,
            ptr: ptr.cast(),
            len,
        })
    }

    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: the whole mapping, aligned (page-aligned start), written
        // only by this process and only while Moon does not hold the buffer.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len / 4) }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what `new` mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// A dma-heap buffer of `len` bytes from `/dev/dma_heap/system`: Moon hands
/// it to moon-compositor, which shows it in place.
fn dma_heap_alloc(len: usize) -> std::io::Result<OwnedFd> {
    #[repr(C)]
    struct Allocation {
        len: u64,
        fd: u32,
        fd_flags: u32,
        heap_flags: u64,
    }
    const DMA_HEAP_IOCTL_ALLOC: libc::c_ulong = 0xc018_4800;
    let heap = std::fs::File::open("/dev/dma_heap/system")?;
    let mut allocation = Allocation {
        len: len as u64,
        fd: 0,
        fd_flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
        heap_flags: 0,
    };
    // SAFETY: DMA_HEAP_IOCTL_ALLOC reads and fills one live allocation.
    if unsafe { libc::ioctl(heap.as_raw_fd(), DMA_HEAP_IOCTL_ALLOC as _, &mut allocation) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the kernel just gave us this descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(allocation.fd as RawFd) })
}

/// A memfd of `len` bytes that Moon may seal against shrinking.
fn memfd(len: usize) -> std::io::Result<OwnedFd> {
    // SAFETY: a name and flags; returns a new descriptor or -1.
    let raw = unsafe {
        libc::memfd_create(
            c"exocortex-app".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh descriptor nothing else owns.
    let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    file.set_len(len as u64)?;
    Ok(OwnedFd::from(file))
}

/// Moon's keyboard on wl_keyboard: the US keymap it sends
/// (OSS-Phone-V1 `wayland/seat.rs`, `us_key`), evdev codes.
fn us_char(code: u32, shift: bool) -> Option<char> {
    const ROWS: [(u32, &str, &str); 4] = [
        (2, "1234567890-=", "!@#$%^&*()_+"),
        (16, "qwertyuiop[]", "QWERTYUIOP{}"),
        (30, "asdfghjkl;'`", "ASDFGHJKL:\"~"),
        (44, "zxcvbnm,./", "ZXCVBNM<>?"),
    ];
    match code {
        57 => return Some(' '),
        43 => return Some(if shift { '|' } else { '\\' }),
        _ => {}
    }
    ROWS.iter().find_map(|&(first, plain, shifted)| {
        let index = code.checked_sub(first)? as usize;
        let row = if shift { shifted } else { plain };
        row.chars().nth(index)
    })
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
                "wp_presentation" => {
                    state.presentation = Some(registry.bind(name, 1, qh, ()));
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

/// xdg_toplevel's `suspended` state (version 6).
const SUSPENDED: u32 = 9;

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
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                if width > 0 && height > 0 {
                    state.logical = (width, height);
                }
                let suspended = states.chunks_exact(4).any(|word| {
                    u32::from_ne_bytes([word[0], word[1], word[2], word[3]]) == SUSPENDED
                });
                if state.suspended != Some(suspended) {
                    state.suspended = Some(suspended);
                    state.events.push(Event::Shown(!suspended));
                    if suspended && state.touch.take().is_some() {
                        state.events.push(Event::Cancel);
                    }
                }
            }
            xdg_toplevel::Event::Close => state.closed = true,
            _ => {}
        }
    }
}

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
            state.scale = factor.clamp(1, 4);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, Callback> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        kind: &Callback,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            match kind {
                Callback::Frame => state.frame_asked = None,
                Callback::Sync => state.synced = true,
            }
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
        let scale = state.scale();
        match event {
            wl_touch::Event::Down { id, x, y, .. } if state.touch.is_none() => {
                let (x, y) = (x as f32 * scale, y as f32 * scale);
                state.touch = Some((id, x, y));
                state.events.push(Event::Press(x, y));
            }
            wl_touch::Event::Motion { id, x, y, .. } => {
                if let Some(touch) = state.touch.as_mut().filter(|touch| touch.0 == id) {
                    touch.1 = x as f32 * scale;
                    touch.2 = y as f32 * scale;
                    state.events.push(Event::Motion(touch.1, touch.2));
                }
            }
            wl_touch::Event::Up { id, .. } => {
                if let Some((_, x, y)) = state.touch.take_if(|touch| touch.0 == id) {
                    state.events.push(Event::Release(x, y));
                }
            }
            wl_touch::Event::Cancel => {
                if state.touch.take().is_some() {
                    state.events.push(Event::Cancel);
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
        match event {
            wl_keyboard::Event::Modifiers { mods_depressed, .. } => {
                state.shift = mods_depressed & 1 != 0;
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                let input = match key {
                    28 | 96 => Some(KeyInput::Enter),
                    14 => Some(KeyInput::Backspace),
                    code => us_char(code, state.shift).map(KeyInput::Character),
                };
                if let Some(input) = input {
                    state.events.push(Event::Key(input));
                }
            }
            // The keymap's descriptor closes as it drops: Moon's keys are
            // the US map above.
            _ => {}
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, i32> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        finger: &i32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation_feedback::Event::Presented {
            tv_sec_hi,
            tv_sec_lo,
            tv_nsec,
            ..
        } = event
        {
            let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
            state
                .presented
                .push((seconds as f64 * 1000.0 + f64::from(tv_nsec) / 1e6, *finger));
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wp_presentation::WpPresentation);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moons_us_keys_decode_back() {
        for character in "abc xyz ABC 0123456789 !@#$%^&*() -_=+[]{};:'\"`~\\|,<.>/?".chars() {
            let found = (1..64)
                .flat_map(|code| [(code, false), (code, true)])
                .find(|&(code, shift)| us_char(code, shift) == Some(character));
            assert!(found.is_some(), "{character:?}");
        }
        assert_eq!(us_char(30, false), Some('a'));
        assert_eq!(us_char(30, true), Some('A'));
        assert_eq!(us_char(2, true), Some('!'));
        assert_eq!(us_char(11, false), Some('0'));
        assert_eq!(us_char(53, true), Some('?'));
        assert_eq!(us_char(41, false), Some('`'));
    }

    #[test]
    fn areas_merge_and_collapse() {
        let mut areas = Vec::new();
        add_area(&mut areas, (0, 0, 10, 10));
        add_area(&mut areas, (2, 2, 5, 5));
        assert_eq!(areas, vec![(0, 0, 10, 10)]);
        add_area(&mut areas, (0, 0, 20, 20));
        assert_eq!(areas, vec![(0, 0, 20, 20)]);
        add_area(&mut areas, (5, 5, 5, 9));
        assert_eq!(areas.len(), 1);
        for i in 0..20 {
            add_area(&mut areas, (0, 100 + i * 10, 4, 104 + i * 10));
        }
        assert!(areas.len() <= 12);
        let bounds = areas[areas.len() - 1];
        assert!(bounds.3 >= 294);
    }
}
