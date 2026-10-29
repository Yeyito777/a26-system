//! The Exocortex app's event loop: X input, the daemon link and Moon's
//! state, one `poll(2)` for all of them. It draws only when something
//! changed (or a fling, the working dots, a notice ends), and never waits
//! on the daemon: that is the link thread's job.

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

use super::handoff;
use super::line::{self, Waker};
use super::link::{DaemonLink, LinkEvent};
use super::model::{Action, Model, Page};
use super::proto;
use super::view::{self, Hit, ScrollArea, ViewState};

/// How long a notice ("Message not sent") stays.
const NOTICE_FOR: Duration = Duration::from_secs(4);
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
/// The panel's refresh (120 Hz): the cadence of animation frames.
const REFRESH: Duration = Duration::from_nanos(8_333_333);
/// At most this long is spent on queued X input before a frame is drawn.
const INPUT_BUDGET: Duration = Duration::from_millis(6);

struct Renderer {
    window: Window,
    depth: u8,
    canvas: Canvas,
    presenter: Presenter,
    /// What the last chat frame showed (view::chat_frame), so the next one
    /// redraws only what changed.
    chat_frame: Option<view::ChatFrame>,
    list_frame: Option<view::ListFrame>,
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
            chat_frame: None,
            list_frame: None,
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
        self.chat_frame = None;
        self.list_frame = None;
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

/// The height taps are judged against: Moon keeps the bottom of the screen
/// for its home gesture, but with its keyboard up the window ends above it.
fn tap_height(renderer: &Renderer, view: &ViewState) -> u16 {
    renderer.size().1 + if view.keyboard { 400 } else { 0 }
}

fn page_depth(model: &Model) -> usize {
    match model.page {
        Page::List => model.folders.len(),
        Page::Chat => 100,
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
    // parses its own: Bold and Regular).
    use crate::text::Weight;
    crate::text::preload(&[
        Weight::SemiBold,
        Weight::Medium,
        Weight::Italic,
        Weight::Mono,
    ]);
    boost_ui_thread(std::env::var_os("EXOCORTEX_APP_NO_BOOST").is_none());
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
    let mut model = Model::new();
    let mut view = ViewState::new();
    let mut tap: Option<crate::TapTracker> = None;
    let mut pressed: Option<Hit> = None;
    let mut dragging: Option<ScrollArea> = None;
    let mut seen_target: Option<u64> = None;
    let mut shown: Option<Option<String>> = None;
    let mut keyboard_requested = false;
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

    // The first frame is drawn before the window maps (as the System app).
    draw_frame(
        &conn,
        &mut renderer,
        &mut model,
        &mut view,
        pressed.as_ref(),
        Instant::now(),
    )?;
    conn.map_window(window)?;
    conn.flush()?;
    shell.ask();

    while running {
        let now = Instant::now();
        let before_depth = page_depth(&model);
        let before_folders = model.folders.len();
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
                    let from_list = model.page == Page::List;
                    for command in model.open_target(&open.target) {
                        link.send(command);
                    }
                    if from_list {
                        control::transition(true);
                    }
                    view.chat_scroll.reset();
                    view.follow = true;
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
            match event {
                XEvent::Expose(event) if event.window == window && event.count == 0 => {
                    renderer.presenter.expose(&conn, 0)?;
                    conn.flush()?;
                }
                XEvent::ConfigureNotify(event) if event.window == window => {
                    if (event.width, event.height) != renderer.size() {
                        renderer.resize(&conn, event.width, event.height)?;
                        // Moon shrinks the window above its keyboard.
                        view.keyboard =
                            keyboard_requested && event.height < crate::REQUESTED_HEIGHT - 300;
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
                        && model.connected
                    {
                        model.reconnect_quietly(now, QUIET_RECONNECT);
                        dropping = true;
                        link.reconnect();
                        if let Some(back) = &mut back {
                            back.2 = true;
                        }
                    }
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
                }
                XEvent::ButtonPress(event) if event.event == window && event.detail == 1 => {
                    if env_flag("EXOCORTEX_APP_PROFILE") {
                        eprintln!(
                            "exocortex: press at {:.3} y {}",
                            monotonic_ms(),
                            event.root_y
                        );
                    }
                    tap = crate::TapTracker::new(
                        event.event_x,
                        event.event_y,
                        tap_height(&renderer, &view),
                    );
                    if tap.is_some() {
                        let height = renderer.height();
                        let y = f32::from(event.event_y);
                        dragging = view::scroll_area(&model, &view, height, y);
                        match dragging {
                            Some(ScrollArea::List) => view.list_scroll.begin(y, now),
                            Some(ScrollArea::Chat) => view.chat_scroll.begin(y, now),
                            Some(ScrollArea::Sheet) => view.sheet_scroll.begin(y, now),
                            None => {}
                        }
                        pressed = view::hit(
                            &mut model,
                            &mut view,
                            height,
                            event.event_x,
                            event.event_y,
                            now,
                        )
                        .filter(|hit| matches!(hit, Hit::Action(_)));
                        redraw = true;
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
                    if let Some(tap) = &mut tap {
                        tap.observe(event.event_x, event.event_y);
                        if tap.moved {
                            pressed = None;
                            let height = renderer.height();
                            let y = f32::from(event.event_y);
                            match dragging {
                                Some(ScrollArea::List) => {
                                    let max = view::list_max(&mut model, height);
                                    view.list_scroll.moved(y, max, now);
                                }
                                Some(ScrollArea::Chat) => {
                                    let max = view.chat_max(&model, height);
                                    view.chat_scroll.moved(y, max, now);
                                    view.follow = false;
                                    if keyboard_requested {
                                        // Dragging the transcript puts the keyboard away.
                                        control::hide_keyboard();
                                        keyboard_requested = false;
                                        view.keyboard = false;
                                    }
                                }
                                Some(ScrollArea::Sheet) => {
                                    let max = view::sheet_max(&model, height);
                                    view.sheet_scroll.moved(y, max, now);
                                }
                                None => {}
                            }
                            redraw = true;
                        }
                    }
                }
                XEvent::ButtonRelease(event) if event.event == window && event.detail == 1 => {
                    let height = renderer.height();
                    match dragging.take() {
                        Some(ScrollArea::List) => {
                            let max = view::list_max(&mut model, height);
                            view.list_scroll.end(max, now);
                        }
                        Some(ScrollArea::Chat) => {
                            let max = view.chat_max(&model, height);
                            view.chat_scroll.end(max, now);
                        }
                        Some(ScrollArea::Sheet) => {
                            let max = view::sheet_max(&model, height);
                            view.sheet_scroll.end(max, now);
                        }
                        None => {}
                    }
                    pressed = None;
                    if let Some((x, y)) = tap.take().and_then(|tap| {
                        tap.finish(event.event_x, event.event_y, tap_height(&renderer, &view))
                    }) && let Some(hit) = view::hit(&mut model, &mut view, height, x, y, now)
                    {
                        match hit {
                            Hit::Action(action) => {
                                perform(
                                    &link,
                                    &mut model,
                                    &mut view,
                                    action,
                                    &mut keyboard_requested,
                                );
                            }
                            Hit::Composer => {
                                let visible = control::show_keyboard(KeyboardPurpose::Text);
                                keyboard_requested = visible;
                                view.follow = true;
                            }
                            Hit::Transcript => {
                                if keyboard_requested {
                                    control::hide_keyboard();
                                    keyboard_requested = false;
                                    view.keyboard = false;
                                }
                            }
                            Hit::SheetBackdrop => {
                                model.handle(Action::HidePicker);
                            }
                        }
                    }
                    redraw = true;
                }
                XEvent::KeyPress(event) if event.event == window => {
                    if model.page == Page::Chat
                        && !model.picker
                        && let Some(input) = key_map.decode(event.detail, event.state)
                    {
                        match input {
                            KeyInput::Character(character) => model.composer.push(character),
                            KeyInput::Backspace => {
                                model.composer.pop();
                            }
                            KeyInput::Enter => {
                                perform(
                                    &link,
                                    &mut model,
                                    &mut view,
                                    Action::Send,
                                    &mut keyboard_requested,
                                );
                                // Moon puts its keyboard away on Return; a chat keeps it.
                                if keyboard_requested {
                                    keyboard_requested =
                                        control::show_keyboard(KeyboardPurpose::Text);
                                }
                            }
                        }
                        view.follow = true;
                        redraw = true;
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
        let height = renderer.height();

        // A page change slides (Moon snapshots the old page first).
        let after_depth = page_depth(&model);
        if after_depth != before_depth {
            // Each folder level opens at its top (one scroll serves them
            // all); back from a chat, the list is where it was.
            if model.folders.len() != before_folders {
                view.list_scroll.reset();
            }
            if model.page == Page::Chat {
                view.chat_scroll.reset();
                view.follow = true;
            }
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
        if model.page != Page::Chat && keyboard_requested {
            control::hide_keyboard();
            keyboard_requested = false;
            view.keyboard = false;
        }

        // Keep the transcript where the user left it.
        view.settle(&model, before_view, height, now);
        if model.page == Page::Chat && view.near_top(now) {
            for command in model.load_older() {
                link.send(command);
            }
        }

        let flinging = match (model.picker, model.page) {
            (true, _) => {
                let max = view::sheet_max(&model, height);
                view.sheet_scroll.step(max, now)
            }
            (false, Page::List) => {
                let max = view::list_max(&mut model, height);
                view.list_scroll.step(max, now)
            }
            (false, Page::Chat) => {
                let max = view.chat_max(&model, height);
                view.chat_scroll.step(max, now)
            }
        };
        // A fling's last step settles the offset (at an edge) without
        // asking for a frame: draw it.
        redraw |= was_flinging && !flinging;
        was_flinging = flinging;
        // A fling is drawn on a steady cadence, one panel refresh after the
        // last frame whatever that frame took, so its steps stay even (8 ms
        // after each frame's end made it 8.6 ms on average, beating against
        // the 120 Hz panel).
        let fling_frame = flinging && fling_due.is_none_or(|due| now >= due);
        if !flinging {
            fling_due = None;
        } else if fling_frame {
            fling_due = Some(match fling_due {
                Some(due) if now < due + REFRESH => due + REFRESH,
                _ => now + REFRESH,
            });
        }
        if model
            .notice
            .as_ref()
            .is_some_and(|(_, at)| now.duration_since(*at) >= NOTICE_FOR)
        {
            model.notice = None;
            redraw = true;
        }
        redraw |= model.tick(now);
        redraw |= model.expire(now);
        let animating = mapped && model.page == Page::Chat && model.streaming();
        if animating && now >= next_dots {
            view.dots = view.dots.wrapping_add(1);
            next_dots = now + DOTS_EVERY;
            redraw = true;
        }
        if mapped && now >= next_shell_poll {
            shell.ask();
            next_shell_poll = now + SHELL_POLL;
        }

        // Hidden (in the background, perhaps leased awake), nothing is drawn:
        // MapNotify draws what changed meanwhile.
        if (redraw || fling_frame) && running && mapped && !holding {
            draw_frame(
                &conn,
                &mut renderer,
                &mut model,
                &mut view,
                pressed.as_ref(),
                now,
            )?;
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
        }
        if let Some(due) = fling_due {
            soonest(due);
        }
        if animating {
            soonest(next_dots);
        }
        if let Some((_, at)) = &model.notice {
            soonest(*at + NOTICE_FOR);
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

    if keyboard_requested {
        control::hide_keyboard();
    }
    handoff::clear_view();
    renderer.presenter.destroy(&conn);
    let _ = conn.flush();
    Ok(())
}

fn perform(
    link: &DaemonLink,
    model: &mut Model,
    view: &mut ViewState,
    action: Action,
    keyboard: &mut bool,
) {
    let depth = page_depth(model);
    match &action {
        Action::Send => view.follow = true,
        // The tapped row stays under the finger as it opens.
        Action::ToggleExpanded(_) => view.follow = false,
        Action::ShowPicker => view.sheet_scroll.reset(),
        Action::Back if *keyboard => {
            control::hide_keyboard();
            *keyboard = false;
            view.keyboard = false;
        }
        _ => {}
    }
    // The daemon is asked first: its answer is on the way while Moon
    // snapshots the page.
    for command in model.handle(action) {
        link.send(command);
    }
    let after = page_depth(model);
    if after != depth {
        // Before the new page is drawn: Moon slides it over the old one.
        control::transition(after > depth);
    }
}

/// CLOCK_MONOTONIC in milliseconds, to line profiles up with other processes.
fn monotonic_ms() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes the time into a live timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as f64 * 1000.0 + now.tv_nsec as f64 / 1e6
}

/// A development switch from the environment, read once.
/// Profiling: `EXOCORTEX_APP_PROFILE` at start, toggled at run time by
/// SIGUSR2 (measuring the app on the phone without restarting it).
static PROFILE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn toggle_profile(_: libc::c_int) {
    PROFILE.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
}

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
    let height = renderer.height();
    // Only what changed is drawn and presented when the last frame is known.
    let chat = view::chat_frame(model, view, pressed, height, now);
    let list = view::list_frame(model, view, pressed, height, now);
    let laid = started.elapsed();
    let update = match (&renderer.chat_frame, &chat, &renderer.list_frame, &list) {
        (Some(old), Some(new), _, _) => {
            view::draw_chat_update(&mut renderer.canvas, model, view, old, new)
        }
        (_, _, Some(old), Some(new)) => {
            view::draw_list_update(&mut renderer.canvas, model, old, new)
        }
        _ => None,
    };
    renderer.chat_frame = chat;
    renderer.list_frame = list;
    let kind = match update {
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
                // Development check: the partial frame equals a full one.
                let mut full = Canvas::new(renderer.canvas.width, renderer.canvas.height);
                full.clear(crate::ui::BG);
                view::draw(&mut full, model, view, pressed, now);
                let width = full.width as usize;
                let rows: Vec<usize> = (0..full.height as usize)
                    .filter(|y| {
                        full.pixels[y * width..(y + 1) * width]
                            != renderer.canvas.pixels[y * width..(y + 1) * width]
                    })
                    .collect();
                if let Some(&y) = rows.first() {
                    let row = y * width..(y + 1) * width;
                    let xs: Vec<usize> = (0..width)
                        .filter(|x| {
                            full.pixels[row.start + x] != renderer.canvas.pixels[row.start + x]
                        })
                        .collect();
                    eprintln!(
                        "exocortex: partial frame differs in {} rows from {y} (x {:?}..{:?}, {} px; full {:06x} partial {:06x}) ({damage:?})",
                        rows.len(),
                        xs.first(),
                        xs.last(),
                        xs.len(),
                        full.pixels[row.start + xs[0]],
                        renderer.canvas.pixels[row.start + xs[0]],
                    );
                }
            }
            (drawn, "partial")
        }
        None => {
            renderer.canvas.clear(crate::ui::BG);
            view::draw(&mut renderer.canvas, model, view, pressed, now);
            let drawn = started.elapsed();
            renderer.presenter.present(conn, &renderer.canvas, 0)?;
            (drawn, "full")
        }
    };
    let (drawn, kind) = kind;
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
