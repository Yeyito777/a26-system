//! The Exocortex app's event loop (A): X input, the daemon link and Moon's
//! state, one `poll(2)` for all of them. Touches become gestures
//! (`gesture.rs`), offered to the overlays first and then to the page; what
//! the model asks for (overlays, haptics, the clipboard, the keyboard) is
//! taken after every change; the kept files are written in the background.
//! It draws only when something changed or animates (`Animator`), and never
//! waits on the daemon: that is the link thread's job.

use std::error::Error;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};

use crate::control::{self, KeyboardPurpose, ShellInfo};
use crate::gfx::Canvas;
use crate::keys::{KeyInput, KeyMap};
use crate::present::Presenter;

use super::anim;
use super::clipboard;
use super::gesture::{Axis, Gesture, Recognizer};
use super::handoff;
use super::line::{self, Waker};
use super::link::{DaemonLink, LinkEvent};
use super::model::{Action, Effect, Model, Nav, Page, chat, composer, sidebar};
use super::proto;
use super::screens::settings;
use super::store::{self, Store};
use super::theme;
use super::ui::overlay::Response;
use super::ui::{self, Haptic};
use super::view::{self, Frame, Hit, ScrollArea, ViewState, frame};

/// While in front, Moon's state (a tapped notification's target) is read
/// this often.
const SHELL_POLL: Duration = Duration::from_secs(1);
/// Back from the background after this long: reconnect, so the daemon drops
/// what it buffered for the frozen app (Moon freezes it right after hiding
/// it, so it cannot unsubscribe first) and the list is current. Quietly:
/// what was on screen stays until the new connection has caught up.
const STALE_AFTER: Duration = Duration::from_secs(20);
const QUIET_RECONNECT: Duration = Duration::from_millis(1500);
const DOTS_EVERY: Duration = Duration::from_millis(420);
/// A conversation opened with nothing to show yet waits this long for its
/// window before "Loading…" is drawn (Moon holds its page slide for the
/// first frame up to 150 ms), so it usually slides in with its messages.
const HOLD_FOR_WINDOW: Duration = Duration::from_millis(110);
/// At most this long is spent on queued X input before a frame is drawn.
const INPUT_BUDGET: Duration = Duration::from_millis(6);
/// Moon owns the bottom of the screen for its home gesture: touches there
/// are its.
const BOTTOM_GESTURE_RESERVE: f32 = 170.0;

struct Renderer {
    window: Window,
    depth: u8,
    canvas: Canvas,
    presenter: Presenter,
    /// What the last frame showed, so the next one draws only what changed.
    frame: Option<Frame>,
}

impl Renderer {
    fn new<C: Connection>(
        conn: &C,
        window: Window,
        depth: u8,
        width: u16,
        height: u16,
    ) -> Result<Self, Box<dyn Error>> {
        let mut presenter = Presenter::create(conn, window, depth, width, height)?;
        presenter.enable_shm(conn);
        Ok(Self {
            window,
            depth,
            canvas: Canvas::new(i32::from(width), i32::from(height)),
            presenter,
            frame: None,
        })
    }

    fn resize<C: Connection>(
        &mut self,
        conn: &C,
        width: u16,
        height: u16,
    ) -> Result<(), Box<dyn Error>> {
        let mut presenter = Presenter::create(conn, self.window, self.depth, width, height)?;
        presenter.enable_shm(conn);
        self.presenter.destroy(conn);
        self.presenter = presenter;
        self.canvas = Canvas::new(i32::from(width), i32::from(height));
        self.frame = None;
        Ok(())
    }

    fn size(&self) -> (u16, u16) {
        (self.canvas.width as u16, self.canvas.height as u16)
    }

    fn height(&self) -> f32 {
        self.canvas.height as f32
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
            .name("exocortex-shell".into())
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

/// How much of a fast core the UI thread is treated as needing while the
/// app is in front (Linux utilization clamping, 0..=1024).
const UI_BOOST: u32 = 1024;

/// Asks the scheduler to treat the UI thread as busy (a minimum utilization
/// clamp) while the app is in front, or not. Its work comes in short bursts
/// after idling, which energy-aware scheduling starts on a slow core at a low
/// clock: a full chat frame took 25-35 ms there and 4 ms on a fast core. An
/// idle thread costs nothing either way. Best effort: a kernel without
/// utilization clamping refuses it.
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
        util_min: if on { UI_BOOST } else { 0 },
        util_max: 1024,
    };
    // SAFETY: sched_setattr reads a live, fully initialized sched_attr of
    // the size it is told; pid 0 is the calling thread.
    unsafe {
        libc::syscall(libc::SYS_sched_setattr, 0, &attr as *const SchedAttr, 0);
    }
}

fn withdraw_notification(conversation: String) {
    // Best effort: moon-notifyd may be absent (Android, a host).
    let _ = thread::Builder::new()
        .name("exocortex-withdraw".into())
        .spawn(move || {
            let _ = handoff::withdraw(&conversation);
        });
}

/// How deep a page is, for Moon's transitions: deeper pushes, shallower
/// pops. Settings sits left of the list (it opens with the pop look).
fn page_depth(model: &Model) -> usize {
    match model.page {
        Page::Settings => 0,
        Page::List => 1,
        Page::Chat => 100,
    }
}

/// The app's side of touches and the keyboard.
#[derive(Default)]
struct Input {
    /// The target under the finger, drawn pressed.
    pressed: Option<Hit>,
    /// The list a vertical drag scrolls.
    dragging: Option<ScrollArea>,
    /// The horizontal scroller a sideways drag moves, and the finger's last x.
    sideways: Option<(u64, f32)>,
    /// The inner scroller a vertical drag moves (a tall table's rows, C1),
    /// and the finger's last y.
    inner: Option<(u64, f32)>,
    /// Moon's keyboard was asked for.
    keyboard: bool,
    /// …by an overlay (a prompt), which puts it away when it goes.
    keyboard_for_overlay: bool,
    /// A drag the composer's editor took (a selection handle, its text).
    editing: Option<view::composer::EditDrag>,
}

/// What acting on input needs.
struct Context<'a> {
    link: &'a DaemonLink,
    model: &'a mut Model,
    view: &'a mut ViewState,
    input: &'a mut Input,
    height: f32,
    width: f32,
}

impl Context<'_> {
    /// Performs an action: the daemon is asked first (its answer is on the
    /// way while Moon snapshots the page), then a page change slides.
    fn perform(&mut self, action: Action) {
        let depth = page_depth(self.model);
        match &action {
            Action::Composer(composer::Act::Send | composer::Act::Deliver(_)) => {
                self.view.follow = true
            }
            // The tapped row stays under the finger as it opens.
            Action::Chat(chat::Act::ToggleExpanded(_)) => self.view.follow = false,
            // Back to the newest message at once, any fling stopped.
            Action::Chat(chat::Act::JumpToLatest) => {
                self.view.chat_scroll.reset();
                self.view.follow = true;
            }
            Action::Nav(_) if self.input.keyboard => self.hide_keyboard(),
            _ => {}
        }
        for command in self.model.handle(action) {
            self.link.send(command);
        }
        let after = page_depth(self.model);
        if after != depth {
            // Before the new page is drawn: Moon slides it over the old one.
            control::transition(after > depth);
        }
        self.take_ui(Instant::now());
    }

    fn hide_keyboard(&mut self) {
        if self.input.keyboard {
            control::hide_keyboard();
        }
        self.input.keyboard = false;
        self.input.keyboard_for_overlay = false;
        self.view.keyboard = false;
    }

    fn show_keyboard(&mut self, purpose: KeyboardPurpose) {
        self.input.keyboard = control::show_keyboard(purpose);
    }

    /// Takes what the model asked of the interface: overlays and effects.
    fn take_ui(&mut self, now: Instant) -> bool {
        let mut changed = self.view.take_ui(self.model, now);
        let haptics = self.model.persist.settings().haptics;
        for effect in std::mem::take(&mut self.model.ui.effects) {
            match effect {
                Effect::Haptic(haptic) => {
                    if haptics {
                        ui::haptic(haptic);
                    }
                }
                Effect::Copy(text) => clipboard::copy(&text),
                Effect::OpenUrl(url) => {
                    ui::open_url(&url);
                }
                Effect::ShowKeyboard(purpose) => {
                    self.show_keyboard(purpose);
                    self.view.follow = true;
                }
                Effect::HideKeyboard => self.hide_keyboard(),
                Effect::Paste => self.model.paste_text(clipboard::paste()),
                Effect::StatusBar => ui::status_bar(&theme::status_bar()),
            }
            changed = true;
        }
        // A prompt on top wants the keyboard; gone, it puts it away.
        match self.view.overlays.keyboard() {
            Some(purpose) if !self.input.keyboard => {
                self.show_keyboard(purpose);
                self.input.keyboard_for_overlay = self.input.keyboard;
            }
            None if self.input.keyboard_for_overlay => self.hide_keyboard(),
            _ => {}
        }
        changed
    }

    /// Offers a gesture to the overlays, then the page. Returns whether
    /// anything shows differently.
    fn gesture(&mut self, gesture: Gesture, now: Instant) -> bool {
        // A list row held after its long press, dragged (B): its moves and
        // its end are the list's, its menu up or not.
        if self.model.page == Page::List
            && matches!(
                gesture,
                Gesture::LongPressMove { .. } | Gesture::LongPressEnd { .. } | Gesture::Cancel
            )
            && let Some((changed, commands)) =
                view::list::drag(self.model, self.view, self.height, &gesture, now)
        {
            for command in commands {
                self.link.send(command);
            }
            self.take_ui(now);
            return changed;
        }
        let response = {
            let mut overlays = std::mem::take(&mut self.view.overlays);
            let cx = self.view.cx(self.model, self.width, self.height, now);
            let response = overlays.gesture(&gesture, &cx);
            self.view.overlays = overlays;
            response
        };
        match response {
            Response::Pass => {}
            Response::Handled => {
                self.input.pressed = None;
                return true;
            }
            Response::Action(action) => {
                self.input.pressed = None;
                self.perform(action);
                return true;
            }
        }
        let height = self.height;
        // A drag on the composer's editor: a selection handle (or the
        // caret) moved, or its text scrolled.
        if let Gesture::DragStart {
            x,
            y,
            start_x,
            start_y,
            edge: false,
            ..
        } = gesture
            && self.model.page == Page::Chat
            && let Some(drag) =
                view::composer::drag_start(self.model, height, self.view.keyboard, start_x, start_y)
        {
            self.input.pressed = None;
            self.view.lifted = None;
            self.input.editing = Some(drag);
            if let Some(action) =
                view::composer::drag(self.model, height, self.view.keyboard, drag, x, y)
            {
                self.perform(action);
            }
            return true;
        }
        match gesture {
            Gesture::Down { x, y } => {
                self.input.pressed =
                    view::hit(self.model, self.view, height, x as i16, y as i16, now)
                        .filter(|hit| matches!(hit, Hit::Action(_)));
                true
            }
            Gesture::Lift { x, y } => {
                self.view.lifted = view::liftable(self.model, self.view, height, x, y, now);
                self.view.lifted.is_some()
            }
            Gesture::LongPressMove { x, y } => {
                match view::long_press_move(self.model, self.view, height, x, y) {
                    Some(action) => {
                        self.perform(action);
                        true
                    }
                    None => false,
                }
            }
            Gesture::LongPressEnd { .. } if self.model.composer.selecting => {
                self.input.pressed = None;
                self.perform(Action::Composer(composer::Act::EndSelect));
                true
            }
            Gesture::Drag { x, y, .. } if self.input.editing.is_some() => {
                if let Some(drag) = self.input.editing
                    && let Some(action) =
                        view::composer::drag(self.model, height, self.view.keyboard, drag, x, y)
                {
                    self.perform(action);
                }
                true
            }
            Gesture::DragEnd { .. } if self.input.editing.is_some() => {
                if let Some(action) = self.input.editing.take().and_then(view::composer::drag_end) {
                    self.perform(action);
                }
                true
            }
            Gesture::LongPress { x, y } => {
                self.input.pressed = None;
                match view::long_press(self.model, self.view, height, x, y, now) {
                    Some(action) => {
                        self.model.ui.haptic(Haptic::Medium);
                        self.perform(action);
                    }
                    None => self.view.lifted = None,
                }
                true
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                y,
                start_x,
                start_y,
                edge: false,
                ..
            } => {
                self.input.pressed = None;
                self.view.lifted = None;
                // On a tall table that can scroll that way: its rows move.
                self.input.inner = view::inner_vertical_target(
                    self.model,
                    self.view,
                    height,
                    (start_x, start_y),
                    y - start_y,
                    now,
                )
                .map(|key| (key, start_y));
                if let Some((key, last)) = &mut self.input.inner {
                    view::inner_drag(self.view, *key, y - *last);
                    *last = y;
                    self.input.dragging = None;
                    return true;
                }
                self.input.dragging = view::scroll_area(self.model, self.view, height, start_y);
                match self.input.dragging {
                    Some(ScrollArea::List) => {
                        self.view.list_scroll.begin(start_y, now);
                        let max = view::list_max(self.model, height);
                        self.view.list_scroll.moved(y, max, now);
                    }
                    Some(ScrollArea::Chat) => {
                        self.view.chat_scroll.begin(start_y, now);
                        self.drag_chat(y, now);
                    }
                    Some(ScrollArea::Settings) => {
                        self.view.settings_scroll.begin(start_y, now);
                        let max = settings::max_scroll(self.model, height);
                        self.view.settings_scroll.moved(y, max, now);
                    }
                    None => {}
                }
                true
            }
            Gesture::DragStart {
                axis: Axis::Horizontal,
                x,
                start_x,
                start_y,
                edge: false,
                ..
            } => {
                self.input.pressed = None;
                self.view.lifted = None;
                self.input.sideways =
                    view::horizontal_target(self.model, self.view, height, start_x, start_y, now)
                        .map(|key| (key, start_x));
                if let Some((key, last)) = &mut self.input.sideways {
                    view::horizontal_drag(self.view, *key, x - *last);
                    *last = x;
                }
                true
            }
            Gesture::DragStart { edge: true, .. } => {
                self.input.pressed = None;
                self.view.lifted = None;
                true
            }
            Gesture::Drag {
                axis: Axis::Vertical,
                y,
                ..
            } => {
                if let Some((key, last)) = &mut self.input.inner {
                    view::inner_drag(self.view, *key, y - *last);
                    *last = y;
                    return true;
                }
                match self.input.dragging {
                    Some(ScrollArea::List) => {
                        let max = view::list_max(self.model, height);
                        self.view.list_scroll.moved(y, max, now);
                    }
                    Some(ScrollArea::Chat) => self.drag_chat(y, now),
                    Some(ScrollArea::Settings) => {
                        let max = settings::max_scroll(self.model, height);
                        self.view.settings_scroll.moved(y, max, now);
                    }
                    None => {}
                }
                true
            }
            Gesture::Drag {
                axis: Axis::Horizontal,
                x,
                ..
            } => {
                if let Some((key, last)) = &mut self.input.sideways {
                    view::horizontal_drag(self.view, *key, x - *last);
                    *last = x;
                }
                true
            }
            Gesture::DragEnd { axis, .. } => {
                if axis == Axis::Vertical {
                    match self.input.dragging.take() {
                        Some(ScrollArea::List) => {
                            let max = view::list_max(self.model, height);
                            self.view.list_scroll.end(max, now);
                        }
                        Some(ScrollArea::Chat) => {
                            let max = self.view.chat_max(self.model, height);
                            self.view.chat_scroll.end(max, now);
                        }
                        Some(ScrollArea::Settings) => {
                            let max = settings::max_scroll(self.model, height);
                            self.view.settings_scroll.end(max, now);
                        }
                        None => {}
                    }
                }
                self.input.sideways = None;
                self.input.inner = None;
                true
            }
            Gesture::Tap { x, y } => {
                self.input.pressed = None;
                self.view.lifted = None;
                match view::hit(self.model, self.view, height, x as i16, y as i16, now) {
                    Some(Hit::Action(action)) => self.perform(action),
                    Some(Hit::Composer) => self.perform(Action::Composer(composer::Act::Focus)),
                    Some(Hit::Transcript) => self.hide_keyboard(),
                    None => {}
                }
                true
            }
            // The left-edge swipe: Back (Settings has nothing to its left).
            Gesture::Back => {
                if self.model.page != Page::Settings {
                    self.perform(Action::Nav(Nav::Back));
                }
                true
            }
            Gesture::Cancel | Gesture::LongPressEnd { .. } => {
                let changed = self.input.pressed.is_some();
                self.input.pressed = None;
                self.input.editing = None;
                if !self.view.overlays.has_modal() {
                    self.view.lifted = None;
                }
                changed
            }
        }
    }

    fn drag_chat(&mut self, y: f32, now: Instant) {
        let max = self.view.chat_max(self.model, self.height);
        self.view.chat_scroll.moved(y, max, now);
        self.view.follow = false;
        // Dragging the transcript puts the keyboard away.
        if self.input.keyboard {
            self.hide_keyboard();
        }
    }

    /// A key from Moon's keyboard: the top overlay's (a prompt), else the
    /// composer's.
    fn key(&mut self, input: KeyInput, now: Instant) -> bool {
        if self.view.overlays.blocking() {
            let response = {
                let mut overlays = std::mem::take(&mut self.view.overlays);
                let cx = self.view.cx(self.model, self.width, self.height, now);
                let response = overlays.key(&input, &cx);
                self.view.overlays = overlays;
                response
            };
            if let Response::Action(action) = response {
                self.perform(action);
            }
            return true;
        }
        // The list's search field (B).
        if self.model.page == Page::List {
            self.perform(Action::List(sidebar::Act::Key(input)));
            return true;
        }
        // The composer's (Moon's `multiline` keyboard: Return is a line
        // break and the keyboard stays up).
        if !self.model.composer_key(input) {
            return false;
        }
        self.take_ui(now);
        self.view.follow = true;
        true
    }
}

pub fn run() -> Result<(), Box<dyn Error>> {
    PROFILE.store(
        std::env::var_os("EXOCORTEX_APP_PROFILE").is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    // SAFETY: the handler only flips an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(
            libc::SIGUSR2,
            toggle_profile as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
    // Launched into the front, the first frames get a fast core; the faces
    // they do not use are parsed meanwhile on other cores (the first frame
    // parses its own: Mono and MonoMedium).
    use crate::text::Weight;
    crate::text::preload(&[Weight::MonoBold, Weight::MonoSemiBold, Weight::MonoItalic]);
    boost_ui_thread(std::env::var_os("EXOCORTEX_APP_NO_BOOST").is_none());
    // What was kept: the settings set the theme before the first frame.
    let mut store = Store::open(store::dir());
    let mut model = Model::new();
    store.load_into(&mut model.persist);
    let settings = model.persist.settings();
    theme::set_current(theme::by_id(&settings.theme));
    theme::set_text_scale(settings.text_scale);
    for (name, _) in model.persist.problems().to_vec() {
        model
            .ui
            .error_toast(format!("{name} could not be read: changes are not kept."));
    }
    ui::status_bar(&theme::status_bar());

    let (conn, screen_number) = x11rb::rust_connection::RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_number];
    let (window, protocols) = crate::create_window(&conn, screen, handoff::APP_ID)?;
    let key_map = KeyMap::query(&conn)?;
    let mut renderer = Renderer::new(
        &conn,
        window,
        screen.root_depth,
        crate::REQUESTED_WIDTH,
        crate::REQUESTED_HEIGHT,
    )?;
    let link = DaemonLink::spawn(proto::socket_path())?;
    let mut shell = ShellReader::spawn()?;
    let mut view = ViewState::new();
    let mut input = Input::default();
    let mut recognizer = Recognizer::new(f32::from(crate::REQUESTED_WIDTH));
    let mut seen_target: Option<u64> = None;
    let mut shown: Option<Option<String>> = None;
    let mut mapped = false;
    let mut unmapped_at: Option<Instant> = None;
    // When the loop last went to sleep: a frozen app reads UnmapNotify only
    // once thawed, so this is when it was hidden.
    let mut slept_at = Instant::now();
    // Events from a connection being dropped on purpose are stale.
    let mut dropping = false;
    let boost = std::env::var_os("EXOCORTEX_APP_NO_BOOST").is_none();
    let mut next_shell_poll = Instant::now();
    let mut next_dots = Instant::now();
    let mut running = true;
    let mut redraw = true;
    let mut was_flinging = false;
    // When the next fling frame is due.
    let mut fling_due: Option<Instant> = None;
    let started = Instant::now();
    let mut list_shown = false;
    // A conversation opened and when, until it is drawn (profiling).
    let mut opening: Option<(String, Instant)> = None;
    // Back in front: when, whether drawn, whether a reconnection is still
    // to come (profiling).
    let mut back: Option<(Instant, bool, bool)> = None;
    // Until when the first frame of a conversation opened empty waits.
    let mut hold_until: Option<Instant> = None;
    // The finger's last screen row (profiling).
    let mut finger: Option<i16> = None;
    // An X event already read from the connection, not yet handled.
    let mut queued_x: Option<XEvent> = None;

    view.take_ui(&mut model, Instant::now());
    // The first frame is drawn before the window maps (as the System app).
    draw_frame(
        &conn,
        &mut renderer,
        &mut model,
        &mut view,
        input.pressed.as_ref(),
        Instant::now(),
    )?;
    // When the next animation frame is wanted.
    let mut next_frame = view.animator.take();
    conn.map_window(window)?;
    conn.flush()?;
    shell.ask();

    while running {
        let now = Instant::now();
        let before_depth = page_depth(&model);
        let before_id = model.chat.id.clone();
        // The reader's place, kept when the transcript changes above it.
        let before_view = view.before(&model, now);

        // Daemon events.
        let applying = Instant::now();
        let events = link.drain();
        let count = events.len();
        for event in events {
            match event {
                LinkEvent::Event(_) if dropping => continue,
                LinkEvent::Connected => {
                    if let Some(back) = &mut back {
                        back.2 = false;
                    }
                    for command in model.on_connected() {
                        link.send(command);
                    }
                }
                LinkEvent::Disconnected => {
                    dropping = false;
                    model.on_disconnected();
                }
                LinkEvent::Event(event) => {
                    redraw |= model.affects_view(&event);
                    for command in model.apply(*event, now) {
                        link.send(command);
                    }
                    continue;
                }
            }
            redraw = true;
        }
        if env_flag("EXOCORTEX_APP_PROFILE") && applying.elapsed() > Duration::from_millis(2) {
            eprintln!(
                "exocortex: {count} events applied in {:.2} ms",
                applying.elapsed().as_secs_f64() * 1000.0
            );
        }

        let mut cx = Context {
            link: &link,
            model: &mut model,
            view: &mut view,
            input: &mut input,
            height: renderer.height(),
            width: renderer.canvas.width as f32,
        };
        redraw |= cx.take_ui(now);

        // Moon's state: a tapped notification's target.
        if let Some(info) = shell.take() {
            redraw |= control::apply_time(&info);
            if let Some(open) = info.open_target
                && open.app == handoff::APP_ID
                && seen_target != Some(open.serial)
            {
                seen_target = Some(open.serial);
                // A target younger than 10 s is still wanted: a notification
                // just started (or brought forward) the app for it.
                if open.age_ms < 10_000 {
                    let from_list = cx.model.page != Page::Chat;
                    for command in cx.model.open_target(&open.target) {
                        link.send(command);
                    }
                    if from_list {
                        control::transition(true);
                    }
                    cx.view.chat_scroll.reset();
                    cx.view.follow = true;
                    redraw = true;
                }
            }
        }

        // X events, all that are queued (a motion only moves the scroll; the
        // frame after shows the latest), within a budget so a flood of input
        // can never hold frames off.
        let reading = Instant::now();
        loop {
            if reading.elapsed() > INPUT_BUDGET {
                break;
            }
            let event = match queued_x.take() {
                Some(event) => event,
                None => match conn.poll_for_event()? {
                    Some(event) => event,
                    None => break,
                },
            };
            let now = Instant::now();
            renderer.presenter.note_event(&event);
            // The height touches are judged against: with Moon's keyboard
            // up the window ends above it (no home gesture there).
            let touch_height = cx.height + if cx.view.keyboard { 400.0 } else { 0.0 };
            match event {
                XEvent::Expose(event) if event.window == window && event.count == 0 => {
                    renderer.presenter.expose(&conn, 0)?;
                    conn.flush()?;
                }
                XEvent::ConfigureNotify(event) if event.window == window => {
                    if (event.width, event.height) != renderer.size() {
                        renderer.resize(&conn, event.width, event.height)?;
                        cx.height = renderer.height();
                        cx.width = renderer.canvas.width as f32;
                        recognizer.set_width(cx.width);
                        // Moon shrinks the window above its keyboard.
                        cx.view.keyboard =
                            cx.input.keyboard && event.height < crate::REQUESTED_HEIGHT - 300;
                        redraw = true;
                    }
                }
                XEvent::MapNotify(event) if event.window == window => {
                    mapped = true;
                    if env_flag("EXOCORTEX_APP_PROFILE") {
                        let hidden =
                            unmapped_at.map_or(0.0, |at| now.duration_since(at).as_secs_f64());
                        eprintln!("exocortex: back in front after {hidden:.1} s hidden");
                        back = Some((now, false, false));
                    }
                    if boost {
                        boost_ui_thread(true);
                    }
                    if unmapped_at
                        .take()
                        .is_some_and(|at| now.duration_since(at) > STALE_AFTER)
                        && cx.model.connected
                    {
                        cx.model.reconnect_quietly(now, QUIET_RECONNECT);
                        dropping = true;
                        link.reconnect();
                        if let Some(back) = &mut back {
                            back.2 = true;
                        }
                    }
                    // Moon knows the app by now: its status bar takes the
                    // theme's background (asked again, the same colour).
                    ui::status_bar(&theme::status_bar());
                    shell.ask();
                    next_shell_poll = now + SHELL_POLL;
                    redraw = true;
                }
                XEvent::UnmapNotify(event) if event.window == window => {
                    mapped = false;
                    if boost {
                        boost_ui_thread(false);
                    }
                    unmapped_at = Some(slept_at.min(now));
                    for gesture in recognizer.cancel() {
                        cx.gesture(gesture, now);
                    }
                }
                XEvent::ButtonPress(event) if event.event == window && event.detail == 1 => {
                    if env_flag("EXOCORTEX_APP_PROFILE") {
                        eprintln!(
                            "exocortex: press at {:.3} y {}",
                            monotonic_ms(),
                            event.root_y
                        );
                    }
                    let (x, y) = (f32::from(event.event_x), f32::from(event.event_y));
                    if y < touch_height - BOTTOM_GESTURE_RESERVE {
                        for gesture in recognizer.down(x, y, now) {
                            redraw |= cx.gesture(gesture, now);
                        }
                    }
                }
                XEvent::MotionNotify(event) if event.event == window => {
                    finger = Some(event.root_y);
                    if env_flag("EXOCORTEX_APP_PROFILE") {
                        eprintln!(
                            "exocortex: motion at {:.3} y {} time {}",
                            monotonic_ms(),
                            event.root_y,
                            event.time
                        );
                    }
                    let (x, y) = (f32::from(event.event_x), f32::from(event.event_y));
                    for gesture in recognizer.moved(x, y, now) {
                        redraw |= cx.gesture(gesture, now);
                    }
                }
                XEvent::ButtonRelease(event) if event.event == window && event.detail == 1 => {
                    let (x, y) = (f32::from(event.event_x), f32::from(event.event_y));
                    // Let go in Moon's gesture area: a drag ends, a press is
                    // no tap.
                    let gestures = if y < touch_height - BOTTOM_GESTURE_RESERVE
                        || recognizer.dragging().is_some()
                    {
                        recognizer.up(x, y, now)
                    } else {
                        recognizer.cancel()
                    };
                    for gesture in gestures {
                        redraw |= cx.gesture(gesture, now);
                    }
                }
                XEvent::KeyPress(event) if event.event == window => {
                    if let Some(input) = key_map.decode(event.detail, event.state) {
                        redraw |= cx.key(input, now);
                    }
                }
                XEvent::ClientMessage(event)
                    if event.window == window
                        && event.type_ == protocols.wm_protocols
                        && event.data.as_data32()[0] == protocols.wm_delete_window =>
                {
                    running = false;
                }
                XEvent::DestroyNotify(event) if event.window == window => running = false,
                _ => {}
            }
        }
        let now = Instant::now();
        // Held presses: the lift at 150 ms, the long press at 600 ms.
        for gesture in recognizer.tick(now) {
            redraw |= cx.gesture(gesture, now);
        }
        redraw |= cx.take_ui(now);
        let height = cx.height;
        let width = cx.width;
        if view.overlays.sweep(now) {
            redraw = true;
            if !view.overlays.has_modal() {
                view.lifted = None;
            }
        }

        // A page change slides (Moon snapshots the old page first).
        let after_depth = page_depth(&model);
        if after_depth != before_depth {
            if model.page == Page::Chat {
                view.chat_scroll.reset();
                view.follow = true;
            }
            view.lifted = None;
            redraw = true;
        }

        if model.page == Page::Chat
            && model.chat.id != before_id
            && let Some(id) = &model.chat.id
        {
            opening = Some((id.clone(), now));
            hold_until = (!model.chat.loaded).then_some(now + HOLD_FOR_WINDOW);
        }
        let holding = hold_until.is_some_and(|until| {
            now < until && model.page == Page::Chat && model.connected && !model.chat.loaded
        });
        if !holding {
            hold_until = None;
        }

        // Tell the bridge what is shown; withdraw what the user now sees.
        let showing = model.shown_conversation().map(str::to_owned);
        if shown.as_ref() != Some(&showing) {
            if let Some(id) = &showing {
                withdraw_notification(id.clone());
            }
            let _ = handoff::write_view(showing.as_deref());
            shown = Some(showing);
        }
        if model.page != Page::Chat
            && input.keyboard
            && !input.keyboard_for_overlay
            && !model.list_keeps_keyboard()
        {
            control::hide_keyboard();
            input.keyboard = false;
            view.keyboard = false;
        }

        // Keep the transcript where the user left it.
        view.settle(&model, before_view, height, now);
        // Bring a revealed list row into view (B).
        let (settled, commands) = view::list::settle(&mut model, &mut view, height, now);
        redraw |= settled;
        for command in commands {
            link.send(command);
        }
        // Older history near the top, tool outputs near the screen (C2).
        for command in view::transcript::requests(&mut model, &mut view, height, now) {
            link.send(command);
        }

        let mut flinging = match model.page {
            Page::List => {
                let max = view::list_max(&mut model, height);
                view.list_scroll.step(max, now)
            }
            Page::Chat => {
                let max = view.chat_max(&model, height);
                view.chat_scroll.step(max, now)
            }
            Page::Settings => {
                let max = settings::max_scroll(&model, height);
                view.settings_scroll.step(max, now)
            }
        };
        {
            let mut overlays = std::mem::take(&mut view.overlays);
            let overlay_cx = view.cx(&model, width, height, now);
            flinging |= overlays.step(&overlay_cx);
            view.overlays = overlays;
        }
        // A fling's last step settles the offset (at an edge) without
        // asking for a frame: draw it.
        redraw |= was_flinging && !flinging;
        was_flinging = flinging;
        // A fling is drawn on a steady cadence, one panel refresh after the
        // last frame was due whatever that frame took, so its steps stay
        // even (8 ms after each frame's end averaged 8.6 ms and beat against
        // the 120 Hz panel).
        let fling_frame = flinging && fling_due.is_none_or(|due| now >= due);
        if !flinging {
            fling_due = None;
        } else if fling_frame {
            fling_due = Some(match fling_due {
                Some(due) if now < due + anim::FRAME => due + anim::FRAME,
                _ => now + anim::FRAME,
            });
        }
        redraw |= model.tick(now);
        redraw |= model.expire(now);
        redraw |= view.take_ui(&mut model, now);
        // Image previews near the screen (C1), asked for nearest first.
        if mapped && model.connected {
            let wants = view.image_wants(&model, height, now);
            for command in model.want_images(&wants, now) {
                link.send(command);
            }
        }
        let dots = mapped && model.page == Page::Chat && model.streaming();
        if dots && now >= next_dots {
            view.dots = view.dots.wrapping_add(1);
            next_dots = now + DOTS_EVERY;
            redraw = true;
        }
        if mapped && now >= next_shell_poll {
            shell.ask();
            next_shell_poll = now + SHELL_POLL;
        }
        // What the model kept changing is written in the background.
        store.take(&mut model.persist, now);
        store.flush_due(now);

        // An animation's frame is due.
        let animating = next_frame.is_some_and(|at| now >= at);
        // Hidden (in the background, perhaps leased awake), nothing is drawn:
        // MapNotify draws what changed meanwhile.
        if (redraw || fling_frame || animating) && running && mapped && !holding {
            draw_frame(
                &conn,
                &mut renderer,
                &mut model,
                &mut view,
                input.pressed.as_ref(),
                now,
            )?;
            next_frame = view.animator.take();
            control::page_drawn(|| {
                let _ = conn.get_input_focus().map(|cookie| cookie.reply());
            });
            redraw = false;
            if env_flag("EXOCORTEX_APP_PROFILE") {
                eprintln!(
                    "exocortex: shown at {:.3} finger {}",
                    monotonic_ms(),
                    finger.map_or(-1, i32::from)
                );
                if !list_shown && model.sidebar.loaded {
                    list_shown = true;
                    eprintln!(
                        "exocortex: list shown {:.1} ms after start",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                if let Some((at, drawn, reconnecting)) = &mut back {
                    if !*drawn {
                        *drawn = true;
                        eprintln!(
                            "exocortex: back: first frame in {:.1} ms",
                            at.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    if !*reconnecting && model.connected && !model.chat_pending() {
                        eprintln!(
                            "exocortex: back: caught up in {:.1} ms",
                            at.elapsed().as_secs_f64() * 1000.0
                        );
                        back = None;
                    }
                }
                if let Some((id, at)) = &opening
                    && model.chat.id.as_ref() == Some(id)
                    && model.chat.loaded
                {
                    eprintln!(
                        "exocortex: opened {id} in {:.1} ms ({} entries{})",
                        at.elapsed().as_secs_f64() * 1000.0,
                        model.chat.entries.len(),
                        if model.chat_pending() {
                            ", fresh window pending"
                        } else {
                            ""
                        }
                    );
                    if !model.chat_pending() {
                        opening = None;
                    }
                }
            }
        }

        // Sleep until X input, a daemon event, Moon's answer or a deadline.
        let mut deadline: Option<Instant> = None;
        let mut soonest = |at: Instant| deadline = Some(deadline.map_or(at, |d| d.min(at)));
        if mapped {
            soonest(next_shell_poll);
            if let Some(at) = next_frame {
                soonest(at);
            }
        }
        if let Some(due) = fling_due {
            soonest(due);
        }
        if dots {
            soonest(next_dots);
        }
        if let Some(until) = hold_until {
            soonest(until);
        }
        if let Some(until) = model.quiet_deadline() {
            soonest(until);
        }
        if let Some(at) = model.answer_deadline() {
            soonest(at);
        }
        if let Some(at) = recognizer.deadline() {
            soonest(at);
        }
        if let Some(at) = store.deadline() {
            soonest(at);
        }
        let mut timeout = deadline.map(|at| at.saturating_duration_since(Instant::now()));
        conn.flush()?;
        // An event the connection has already read (during a round trip, or
        // left by the budget) does not make its socket readable: look first.
        if queued_x.is_none() {
            queued_x = conn.poll_for_event()?;
        }
        if queued_x.is_some() {
            timeout = Some(Duration::ZERO);
        }
        let mut fds = [
            line::pollfd(conn.stream().as_raw_fd(), libc::POLLIN),
            line::pollfd(link.fd(), libc::POLLIN),
            line::pollfd(shell.waker.fd(), libc::POLLIN),
        ];
        slept_at = Instant::now();
        line::poll(&mut fds, timeout)?;
    }

    if input.keyboard {
        control::hide_keyboard();
    }
    model.flush_drafts();
    store.take(&mut model.persist, Instant::now());
    store.flush_all();
    handoff::clear_view();
    renderer.presenter.destroy(&conn);
    let _ = conn.flush();
    Ok(())
}

/// CLOCK_MONOTONIC in milliseconds, to line profiles up with other
/// processes (the touchscreen's evdev reports, Moon, moon-compositor).
fn monotonic_ms() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes the time into a live timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as f64 * 1000.0 + now.tv_nsec as f64 / 1e6
}

/// Profiling: `EXOCORTEX_APP_PROFILE` at start, toggled at run time by
/// SIGUSR2 (measuring the app on the phone without restarting it).
static PROFILE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn toggle_profile(_: libc::c_int) {
    PROFILE.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
}

/// A development switch: profiling (above), or `EXOCORTEX_APP_VERIFY` read
/// once from the environment.
fn env_flag(name: &'static str) -> bool {
    use std::sync::OnceLock;
    static VERIFY: OnceLock<bool> = OnceLock::new();
    match name {
        "EXOCORTEX_APP_PROFILE" => PROFILE.load(std::sync::atomic::Ordering::Relaxed),
        _ => *VERIFY.get_or_init(|| std::env::var_os("EXOCORTEX_APP_VERIFY").is_some()),
    }
}

fn draw_frame(
    conn: &impl Connection,
    renderer: &mut Renderer,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let size = (renderer.canvas.width, renderer.canvas.height);
    // Only what changed is drawn and presented when the last frame is known.
    let record = frame::record(model, view, pressed, size, now);
    let laid = started.elapsed();
    let update = match &renderer.frame {
        Some(old) => frame::update(
            &mut renderer.canvas,
            model,
            view,
            pressed,
            old,
            &record,
            now,
        ),
        None => None,
    };
    renderer.frame = Some(record);
    let (drawn, kind) = match update {
        // Nothing on screen changed.
        Some(damage) if damage.rows.is_empty() && damage.moved.is_none() => return Ok(()),
        Some(damage) => {
            if let Some((y0, y1, dy)) = damage.moved {
                renderer.presenter.scroll(conn, y0, y1, dy)?;
            }
            let drawn = started.elapsed();
            renderer
                .presenter
                .present_rows(conn, &renderer.canvas, 0, &damage.rows)?;
            if env_flag("EXOCORTEX_APP_VERIFY") {
                verify(&renderer.canvas, model, view, pressed, now, &damage);
            }
            (drawn, "partial")
        }
        None => {
            frame::clear(&mut renderer.canvas);
            frame::draw_full(&mut renderer.canvas, model, view, pressed, now);
            let drawn = started.elapsed();
            renderer.presenter.present(conn, &renderer.canvas, 0)?;
            (drawn, "full")
        }
    };
    conn.flush()?;
    if env_flag("EXOCORTEX_APP_PROFILE") {
        eprintln!(
            "exocortex: frame drawn in {:.2} ms (layout {:.2}), presented in {:.2} ms ({kind})",
            drawn.as_secs_f64() * 1000.0,
            laid.as_secs_f64() * 1000.0,
            (started.elapsed() - drawn).as_secs_f64() * 1000.0
        );
    }
    Ok(())
}

/// Development check (`EXOCORTEX_APP_VERIFY`): the partial frame equals a
/// full one.
fn verify(
    canvas: &Canvas,
    model: &mut Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    now: Instant,
    damage: &view::FrameDamage,
) {
    let mut full = Canvas::new(canvas.width, canvas.height);
    frame::clear(&mut full);
    frame::draw_full(&mut full, model, view, pressed, now);
    let width = full.width as usize;
    let rows: Vec<usize> = (0..full.height as usize)
        .filter(|y| {
            full.pixels[y * width..(y + 1) * width] != canvas.pixels[y * width..(y + 1) * width]
        })
        .collect();
    if let Some(&y) = rows.first() {
        let row = y * width..(y + 1) * width;
        let xs: Vec<usize> = (0..width)
            .filter(|x| full.pixels[row.start + x] != canvas.pixels[row.start + x])
            .collect();
        eprintln!(
            "exocortex: partial frame differs in {} rows from {y} (x {:?}..{:?}, {} px; full {:06x} partial {:06x}) ({damage:?})",
            rows.len(),
            xs.first(),
            xs.last(),
            xs.len(),
            full.pixels[row.start + xs[0]],
            canvas.pixels[row.start + xs[0]],
        );
    }
}
