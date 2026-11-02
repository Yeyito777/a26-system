//! The Instagram app on the social shell: four tabs as Instagram has them
//! with Reels left out (Home, Messages, Search, Profile; Activity behind
//! Home's heart), each a stack of screens; the story viewer over them; the
//! bridge's answers and events into the store; and the frame.
//!
//! A frame that only scrolled moves the rows already drawn (`move_rows`)
//! and draws the band that came into view, so a scroll frame costs a copy
//! and a strip, not the screen. Anything else (data, a picture arriving, a
//! tap, the keyboard) draws the whole frame.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::activity::ActivityScreen;
use super::direct::{InboxScreen, ThreadScreen};
use super::feed::FeedScreen;
use super::paint::{self, BG, Cache, DP, Icon, Paint, SAFE_BOTTOM, TAB_BAR, TEXT, Want};
use super::profile::ProfileScreen;
use super::search::SearchScreen;
use super::store::{Ask, Store};
use super::stories::StoryViewer;
use crate::comms::Scroll;
use crate::control::KeyboardPurpose;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::exocortex::ui::Haptic;
use crate::gfx::{Canvas, RectF};
use crate::keys::KeyInput;
use crate::social::proto::{BridgeError, Event, Hello};
use crate::social::shell::{App, Cx, Damage, DrawCx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Home = 0,
    Messages = 1,
    Search = 2,
    Profile = 3,
}

const TABS: [Tab; 4] = [Tab::Home, Tab::Messages, Tab::Search, Tab::Profile];

/// The content area a screen draws into this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub top: f32,
    pub bottom: f32,
    pub width: f32,
    pub height: f32,
    /// The scroll offset (rounded), from the top (or the bottom, inverted).
    pub offset: f32,
    /// Window rows to draw (the whole viewport, or the band a scroll exposed).
    pub band: (f32, f32),
}

impl Area {
    pub fn visible(&self, y0: f32, y1: f32) -> bool {
        y1 > self.band.0 && y0 < self.band.1
    }
}

/// What a screen asks the app to do.
pub enum Action {
    None,
    Redraw,
    Push(Box<dyn Screen>),
    Pop,
    Tab(Tab),
    Request {
        cmd: &'static str,
        args: Value,
        ask: Ask,
    },
    Stories {
        ids: Vec<String>,
        start: usize,
    },
    Toast(String),
    Keyboard(Option<KeyboardPurpose>),
    OpenUrl(String),
    Haptic(Haptic),
    /// Opens a chat on the Messages tab.
    Chat(String),
    /// Changes what the app knows (a choice made here, not the bridge's).
    Mutate(Box<dyn FnOnce(&mut Store)>),
    Many(Vec<Action>),
}

impl Action {
    pub fn request(cmd: &'static str, args: Value, ask: Ask) -> Self {
        Self::Request { cmd, args, ask }
    }
}

pub trait Screen {
    /// The chat it shows (the bridge does not notify for it).
    fn chat(&self) -> Option<&str> {
        None
    }
    /// The scrolled content's rows for a window `height` tall.
    fn viewport(&self, height: f32) -> (f32, f32);
    fn scroll(&mut self) -> Option<&mut Scroll> {
        None
    }
    /// Content anchored at the bottom (a chat): the offset counts up from it.
    fn inverted(&self) -> bool {
        false
    }
    /// Lays out (when the store moved on) and returns the content's height.
    fn layout(&mut self, store: &Store, width: f32) -> f32;
    /// Draws the content rows of `area.band`.
    fn draw(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area);
    /// Draws what stays put (bars, the composer).
    fn chrome(&mut self, canvas: &mut Canvas, p: &mut Paint, store: &Store, area: Area);
    fn tap(&mut self, store: &Store, x: f32, y: f32, area: Area) -> Action;
    fn long_press(&mut self, _store: &Store, _x: f32, _y: f32, _area: Area) -> Action {
        Action::None
    }
    /// A horizontal drag inside the screen (a carousel); false leaves it to Back.
    fn horizontal(&mut self, _store: &Store, _gesture: &Gesture, _area: Area) -> Option<Action> {
        None
    }
    /// Every loop pass: more pages, animations.
    fn tick(&mut self, _store: &Store, _area: Area, _now: Instant) -> Action {
        Action::None
    }
    fn key(&mut self, _store: &Store, _key: KeyInput) -> Action {
        Action::None
    }
    /// Came to the top (pushed, or back to it): what it loads.
    fn shown(&mut self, _store: &Store) -> Action {
        Action::None
    }
    /// Draws in time (an animation) rather than only on scroll.
    fn animating(&self, _now: Instant) -> bool {
        false
    }
    /// When its `tick` wants to run next (a debounce), with no frame before.
    fn wake(&self) -> Option<Instant> {
        None
    }
    /// Its frames may move rows (no overlay floats over the content).
    fn blit_ok(&self) -> bool {
        true
    }
    fn tab_bar(&self) -> bool {
        true
    }
    /// What the bridge just changed moved nothing on screen but these rows
    /// (a page of older messages above, posts below): Some(rows), or None
    /// for a whole frame. Called after `layout`.
    fn changed_rows(&mut self, _area: Area) -> Option<Vec<(i32, i32)>> {
        None
    }
    /// The keyboard went away.
    fn keyboard_hidden(&mut self) {}
    /// Scroll to the top (the tab tapped again).
    fn to_top(&mut self) -> bool {
        match self.scroll() {
            Some(scroll) => {
                scroll.reset();
                true
            }
            None => false,
        }
    }
}

struct LastFrame {
    tab: Tab,
    depth: usize,
    screen: usize,
    area: (f32, f32, f32, f32),
    offset: f32,
    stories: bool,
}

pub struct Instagram {
    store: Store,
    stacks: [Vec<Box<dyn Screen>>; 4],
    tab: Tab,
    stories: Option<StoryViewer>,
    asked: HashMap<u64, Ask>,
    cache: Cache,
    missing: Vec<Want>,
    last: Option<LastFrame>,
    dirty: bool,
    /// The store changed since the last frame (what it changed is the
    /// screen's to say: `Screen::changed_rows`).
    changed: bool,
    toast: Option<(String, Instant)>,
    keyboard: bool,
    shown: bool,
    drag: Option<Axis>,
    frames: u64,
    profile: bool,
    sends: u32,
    verify: bool,
    /// A chat being opened and since when (profiling).
    opening: Option<(String, Instant)>,
}

/// A client message id in the format Instagram's web app makes them
/// (milliseconds shifted 22 bits, 22 bits to tell sends apart, 63 bits).
fn offline_threading_id(counter: &mut u32) -> String {
    *counter = counter.wrapping_add(1);
    let ms = paint::now_ms().max(0) as u64;
    let low = (u64::from(std::process::id()) << 10 ^ u64::from(*counter)) & 0x3f_ffff;
    (((ms << 22) | low) & ((1 << 63) - 1)).to_string()
}

const TOAST_FOR: Duration = Duration::from_millis(2600);

impl Instagram {
    pub fn new() -> Self {
        Self {
            store: Store::default(),
            stacks: [
                vec![Box::new(FeedScreen::new())],
                vec![Box::new(InboxScreen::new())],
                vec![Box::new(SearchScreen::new())],
                vec![Box::new(ProfileScreen::own())],
            ],
            tab: Tab::Home,
            stories: None,
            asked: HashMap::new(),
            cache: Cache::default(),
            missing: Vec::new(),
            last: None,
            dirty: true,
            changed: false,
            toast: None,
            keyboard: false,
            shown: false,
            drag: None,
            frames: 0,
            profile: std::env::var_os("INSTAGRAM_APP_PROFILE").is_some()
                || std::env::var_os("EXOCORTEX_APP_PROFILE").is_some(),
            sends: 0,
            verify: std::env::var_os("INSTAGRAM_APP_VERIFY").is_some(),
            opening: None,
        }
    }

    fn show_top(&mut self) -> Action {
        let store = &self.store;
        self.stacks[self.tab as usize]
            .last_mut()
            .expect("a tab has its root")
            .shown(store)
    }

    fn top(&mut self) -> &mut Box<dyn Screen> {
        self.stacks[self.tab as usize]
            .last_mut()
            .expect("a tab has its root")
    }

    fn area(&mut self, width: f32, height: f32, now: Instant) -> Area {
        let store = &self.store;
        let screen = self.stacks[self.tab as usize].last_mut().expect("root");
        let (top, bottom) = screen.viewport(height);
        let content = screen.layout(store, width);
        let max = (content - (bottom - top)).max(0.0);
        let offset = screen.scroll().map_or(0.0, |scroll| {
            scroll.value(now).clamp(-max - 400.0, max + 400.0).round()
        });
        Area {
            top,
            bottom,
            width,
            height,
            offset,
            band: (top, bottom),
        }
    }

    fn max_scroll(&mut self, width: f32, height: f32) -> f32 {
        let store = &self.store;
        let screen = self.stacks[self.tab as usize].last_mut().expect("root");
        let (top, bottom) = screen.viewport(height);
        (screen.layout(store, width) - (bottom - top)).max(0.0)
    }

    fn perform(&mut self, cx: &mut Cx, action: Action) {
        self.act(cx, action);
        if self.dirty {
            cx.redraw();
        }
    }

    fn act(&mut self, cx: &mut Cx, action: Action) {
        match action {
            Action::None => {}
            Action::Redraw => self.dirty = true,
            Action::Push(mut screen) => {
                if let Some(chat) = screen.chat() {
                    self.opening = Some((chat.to_owned(), Instant::now()));
                }
                self.hide_keyboard(cx);
                cx.transition(true);
                let shown = screen.shown(&self.store);
                self.stacks[self.tab as usize].push(screen);
                self.perform(cx, shown);
                self.dirty = true;
            }
            Action::Pop => {
                if self.stacks[self.tab as usize].len() > 1 {
                    self.hide_keyboard(cx);
                    cx.transition(false);
                    self.stacks[self.tab as usize].pop();
                    let shown = self.show_top();
                    self.perform(cx, shown);
                    self.dirty = true;
                }
            }
            Action::Tab(tab) => self.switch_tab(cx, tab),
            Action::Request {
                cmd,
                mut args,
                mut ask,
            } => {
                if let Ask::Send { chat, .. } = &ask {
                    // Shown at once, pending, under a client id the echo carries.
                    let client_id = offline_threading_id(&mut self.sends);
                    let text = args["text"].as_str().unwrap_or("").to_owned();
                    let me = self
                        .store
                        .account()
                        .map(|a| a.id.clone())
                        .unwrap_or_default();
                    self.store.add_pending(chat, &text, &client_id, &me);
                    args["client_id"] = json!(client_id);
                    ask = Ask::Send {
                        chat: chat.clone(),
                        client_id,
                    };
                    self.dirty = true;
                }
                self.store.asking(&ask);
                let id = cx.request(cmd, args);
                self.asked.insert(id, ask);
            }
            Action::Chat(chat) => self.open_chat(cx, chat),
            Action::Mutate(change) => {
                change(&mut self.store);
                self.store.generation = self.store.generation.wrapping_add(1);
                self.dirty = true;
            }
            Action::Stories { ids, start } => {
                self.hide_keyboard(cx);
                let mut viewer = StoryViewer::new(ids, start, cx.now);
                let load = viewer.load(&self.store);
                self.stories = Some(viewer);
                self.perform(cx, load);
                self.dirty = true;
            }
            Action::Toast(text) => {
                self.toast = Some((text, cx.now));
                cx.wake_at(cx.now + TOAST_FOR);
                self.dirty = true;
            }
            Action::Keyboard(Some(purpose)) => {
                cx.show_keyboard(purpose);
                self.keyboard = cx.keyboard_shown();
                self.dirty = true;
            }
            Action::Keyboard(None) => self.hide_keyboard(cx),
            Action::OpenUrl(url) => {
                if !cx.open_url(&url) {
                    self.perform(cx, Action::Toast("This link can't be opened.".into()));
                }
            }
            Action::Haptic(haptic) => cx.haptic(haptic),
            Action::Many(actions) => {
                for action in actions {
                    self.perform(cx, action);
                }
            }
        }
    }

    fn hide_keyboard(&mut self, cx: &mut Cx) {
        if self.keyboard || cx.keyboard_shown() {
            cx.hide_keyboard();
            self.keyboard = false;
            self.top().keyboard_hidden();
            self.dirty = true;
        }
    }

    fn switch_tab(&mut self, cx: &mut Cx, tab: Tab) {
        self.hide_keyboard(cx);
        if tab == self.tab {
            // Again: to the root, or to the top of the root.
            if self.stacks[tab as usize].len() > 1 {
                cx.transition(false);
                self.stacks[tab as usize].truncate(1);
                let shown = self.show_top();
                self.perform(cx, shown);
            } else if !self.top().to_top() {
                return;
            }
        } else {
            self.tab = tab;
            let shown = self.show_top();
            self.perform(cx, shown);
        }
        self.dirty = true;
    }

    /// Opens a chat (a notification's target, a profile's Message button).
    pub fn open_chat(&mut self, cx: &mut Cx, chat: String) {
        self.stories = None;
        self.tab = Tab::Messages;
        let stack = &mut self.stacks[Tab::Messages as usize];
        if stack.last().and_then(|screen| screen.chat()) == Some(chat.as_str()) {
            self.dirty = true;
            return;
        }
        stack.truncate(1);
        self.perform(cx, Action::Push(Box::new(ThreadScreen::new(chat))));
    }

    fn draw_toast(&self, canvas: &mut Canvas, height: f32) {
        let Some((text, _)) = &self.toast else {
            return;
        };
        let font = paint::BODY;
        let lines = paint::wrap_plain(text, font, TEXT, canvas.width as f32 - 240.0, 3).0;
        let h = lines.len() as f32 * 50.0 + 56.0;
        let y = height - SAFE_BOTTOM - TAB_BAR - h - 40.0;
        let rect = RectF::new(60.0, y, canvas.width as f32 - 120.0, h);
        canvas.fill_rrect(rect, 28.0, 0x363636, 1.0);
        for (n, line) in lines.iter().enumerate() {
            paint::draw_line(canvas, line, 100.0, y + 28.0 + 38.0 + n as f32 * 50.0);
        }
    }

    /// Development check (`INSTAGRAM_APP_VERIFY`): a frame that moved rows
    /// equals one drawn whole.
    fn check_band(
        &mut self,
        canvas: &Canvas,
        media: &mut crate::social::media::Media,
        now: Instant,
        area: Area,
    ) -> bool {
        let mut full = Canvas::new(canvas.width, canvas.height);
        full.clear(BG);
        let store = &self.store;
        let screen = self.stacks[self.tab as usize].last_mut().expect("root");
        let mut missing = Vec::new();
        let mut p = Paint {
            media,
            now,
            missing: &mut missing,
            cache: &self.cache,
        };
        full.set_clip(0, area.top as i32, full.width, area.bottom as i32);
        screen.draw(
            &mut full,
            &mut p,
            store,
            Area {
                band: (area.top, area.bottom),
                ..area
            },
        );
        if !missing.is_empty() || !self.missing.is_empty() {
            // Pictures still on their way: placeholders differ by design.
            return true;
        }
        let width = full.width as usize;
        let rows: Vec<usize> = (area.top as usize..area.bottom as usize)
            .filter(|y| {
                full.pixels[y * width..(y + 1) * width] != canvas.pixels[y * width..(y + 1) * width]
            })
            .collect();
        if let (Some(&first), Some(last)) = (rows.first(), rows.last()) {
            let row = first * width..(first + 1) * width;
            let xs: Vec<usize> = (0..width)
                .filter(|x| full.pixels[row.start + x] != canvas.pixels[row.start + x])
                .collect();
            eprintln!(
                "instagram: band frame differs in {} rows ({first}..{last}), x {:?}..{:?} ({} px, full {:06x} band {:06x}), offset {}",
                rows.len(),
                xs.first(),
                xs.last(),
                xs.len(),
                full.pixels[row.start + xs[0]] & 0xffffff,
                canvas.pixels[row.start + xs[0]] & 0xffffff,
                area.offset,
            );
            return false;
        }
        true
    }

    /// The bars and what floats over them, drawn again over a frame that
    /// otherwise kept its rows.
    fn draw_bars(
        &mut self,
        canvas: &mut Canvas,
        media: &mut crate::social::media::Media,
        now: Instant,
        area: Area,
    ) {
        let height = canvas.height as f32;
        let tab_bar = self.top().tab_bar();
        let store = &self.store;
        let screen = self.stacks[self.tab as usize].last_mut().expect("root");
        let mut p = Paint {
            media,
            now,
            missing: &mut self.missing,
            cache: &self.cache,
        };
        let content = p.missing.len();
        screen.chrome(canvas, &mut p, store, area);
        if tab_bar {
            draw_tab_bar(canvas, &mut p, store, self.tab, height);
        }
        for want in &mut p.missing[content..] {
            want.rows = (i32::MIN, i32::MIN);
        }
    }

    fn full_frame(
        &mut self,
        canvas: &mut Canvas,
        media: &mut crate::social::media::Media,
        now: Instant,
    ) {
        self.missing.clear();
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        canvas.reset_clip();
        canvas.clear(BG);
        if let Some(viewer) = &mut self.stories {
            let mut p = Paint {
                media,
                now,
                missing: &mut self.missing,
                cache: &self.cache,
            };
            viewer.draw(canvas, &mut p, &self.store, width, height);
            return;
        }
        let area = self.area(width, height, now);
        let tab_bar = self.top().tab_bar();
        {
            let store = &self.store;
            let screen = self.stacks[self.tab as usize].last_mut().expect("root");
            let mut p = Paint {
                media,
                now,
                missing: &mut self.missing,
                cache: &self.cache,
            };
            canvas.set_clip(0, area.top as i32, canvas.width, area.bottom as i32);
            screen.draw(canvas, &mut p, store, area);
            canvas.reset_clip();
            let content = p.missing.len();
            screen.chrome(canvas, &mut p, store, area);
            for want in &mut p.missing[content..] {
                want.rows = (i32::MIN, i32::MIN);
            }
        }
        if tab_bar {
            let mut p = Paint {
                media,
                now,
                missing: &mut self.missing,
                cache: &self.cache,
            };
            let content = p.missing.len();
            draw_tab_bar(canvas, &mut p, &self.store, self.tab, height);
            for want in &mut p.missing[content..] {
                want.rows = (i32::MIN, i32::MIN);
            }
        }
        self.draw_toast(canvas, height);
    }
}

fn draw_tab_bar(canvas: &mut Canvas, p: &mut Paint, store: &Store, current: Tab, height: f32) {
    let top = height - SAFE_BOTTOM - TAB_BAR;
    canvas.fill_rect(0, top as i32, canvas.width, (height - top) as i32, BG, 1.0);
    canvas.fill_rect(0, top as i32, canvas.width, 1, 0x1f1f1f, 1.0);
    let slot = canvas.width as f32 / TABS.len() as f32;
    let size = 26.0 * DP;
    let account = store.account().cloned();
    for (index, tab) in TABS.iter().enumerate() {
        let cx = slot * (index as f32 + 0.5);
        let cy = top + TAB_BAR / 2.0;
        let active = *tab == current;
        match tab {
            Tab::Home => paint::icon(
                canvas,
                p.cache,
                Icon::Home,
                cx - size / 2.0,
                cy - size / 2.0,
                size,
                active,
                TEXT,
            ),
            Tab::Messages => {
                paint::icon(
                    canvas,
                    p.cache,
                    Icon::Direct,
                    cx - size / 2.0,
                    cy - size / 2.0,
                    size,
                    active,
                    TEXT,
                );
                let unread = store.unread_chats();
                if unread > 0 {
                    paint::badge(canvas, cx + size * 0.42, cy - size * 0.38, unread);
                }
            }
            Tab::Search => paint::icon(
                canvas,
                p.cache,
                Icon::Search,
                cx - size / 2.0,
                cy - size / 2.0,
                size,
                active,
                TEXT,
            ),
            Tab::Profile => {
                let d = 28.0 * DP;
                if active {
                    canvas.fill_circle(cx, cy, d / 2.0 + 5.0, TEXT, 1.0);
                    canvas.fill_circle(cx, cy, d / 2.0 + 1.0, BG, 1.0);
                }
                let reference = account
                    .as_ref()
                    .and_then(|a| a.avatar.as_ref())
                    .map(|m| m.reference.clone());
                paint::avatar(
                    canvas,
                    p,
                    reference.as_deref(),
                    cx,
                    cy,
                    d,
                    paint::Ring::None,
                );
            }
        }
    }
}

impl App for Instagram {
    fn id(&self) -> &'static str {
        "instagram"
    }

    fn status_bar(&self) -> (String, &'static str) {
        ("#000000".into(), "light")
    }

    fn connected(&mut self, cx: &mut Cx, hello: &Hello) {
        self.store.status = hello.status.clone();
        self.store.fixtures = hello.fixtures;
        self.store.capabilities = hello.capabilities.clone();
        self.store.settings.writes = hello.status.writes;
        self.perform(
            cx,
            Action::request(
                "chats.list",
                json!({"limit": 500, "folder": "inbox"}),
                Ask::Chats,
            ),
        );
        self.perform(
            cx,
            Action::request("settings.get", json!({}), Ask::Settings),
        );
        self.perform(
            cx,
            Action::request("instagram.badge", json!({}), Ask::Badge),
        );
        for stack in 0..self.stacks.len() {
            for index in 0..self.stacks[stack].len() {
                let action = self.stacks[stack][index].shown(&self.store);
                self.perform(cx, action);
            }
        }
        if let Some(viewer) = &mut self.stories {
            let load = viewer.load(&self.store);
            self.perform(cx, load);
        }
        self.dirty = true;
        cx.redraw();
    }

    fn disconnected(&mut self, cx: &mut Cx) {
        self.store.status.state = "connecting".into();
        self.dirty = true;
        cx.redraw();
    }

    fn event(&mut self, cx: &mut Cx, event: Event) {
        let requests_changed =
            matches!(&event, Event::Chat(chat) if chat.folder.as_deref() == Some("requests"));
        if self.store.event(event) {
            self.changed = true;
            cx.redraw();
        }
        let _ = requests_changed;
    }

    fn answer(&mut self, cx: &mut Cx, id: u64, result: Result<Value, BridgeError>) {
        let Some(ask) = self.asked.remove(&id) else {
            return;
        };
        if matches!(ask, Ask::Chats) {
            // Requests live in their own folder: listed once the inbox is in.
            self.perform(
                cx,
                Action::request(
                    "chats.list",
                    json!({"limit": 100, "folder": "requests"}),
                    Ask::Other,
                ),
            );
        }
        let requests =
            matches!(ask, Ask::Other) && result.as_ref().is_ok_and(|v| v.get("chats").is_some());
        if requests {
            if let Ok(value) = &result {
                for chat in value["chats"].as_array().into_iter().flatten() {
                    if let Ok(chat) = serde_json::from_value(chat.clone()) {
                        self.store.event(Event::Chat(Box::new(chat)));
                    }
                }
            }
            self.changed = true;
        } else if self.store.answer(ask, result) {
            self.changed = true;
        }
        if let Some(notice) = self.store.notice.take() {
            self.perform(cx, Action::Toast(notice));
        }
        cx.redraw();
    }

    fn gesture(&mut self, cx: &mut Cx, gesture: Gesture) {
        let now = cx.now;
        // Profiling: a press, in screen rows (the bench times an open from it).
        if self.profile
            && let Gesture::Down { y, .. } = gesture
        {
            eprintln!(
                "instagram: press at {:.3} y {}",
                monotonic_ms(),
                (y + crate::exocortex::wayland::SCREEN_TOP).round() as i32
            );
        }
        let (width, height) = (cx.width, cx.height);
        if let Some(viewer) = &mut self.stories {
            let action = viewer.gesture(&self.store, &gesture, width, height, now);
            if viewer.closed {
                self.stories = None;
                let shown = self.show_top();
                self.perform(cx, shown);
            }
            self.dirty = true;
            self.perform(cx, action);
            cx.animate();
            return;
        }
        let area = self.area(width, height, now);
        let max = self.max_scroll(width, height);
        let inverted = self.top().inverted();
        let sign = if inverted { -1.0 } else { 1.0 };
        match gesture {
            Gesture::Tap { x, y } => {
                let tab_top = height - SAFE_BOTTOM - TAB_BAR;
                if self.top().tab_bar() && y >= tab_top && !self.keyboard {
                    let index = ((x / width) * TABS.len() as f32).clamp(0.0, 3.0) as usize;
                    self.switch_tab(cx, TABS[index]);
                } else {
                    let store = &self.store;
                    let screen = self.stacks[self.tab as usize].last_mut().expect("root");
                    let action = screen.tap(store, x, y, area);
                    self.dirty = true;
                    self.perform(cx, action);
                }
            }
            Gesture::LongPress { x, y } => {
                let store = &self.store;
                let screen = self.stacks[self.tab as usize].last_mut().expect("root");
                let action = screen.long_press(store, x, y, area);
                self.perform(cx, action);
            }
            Gesture::Back => {
                if self.keyboard {
                    self.hide_keyboard(cx);
                }
                self.perform(cx, Action::Pop);
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                y,
                start_y,
                ..
            } => {
                self.drag = Some(Axis::Vertical);
                if let Some(scroll) = self.top().scroll() {
                    scroll.begin(sign * start_y, now);
                    scroll.moved(sign * y, max, now);
                }
                cx.redraw();
            }
            Gesture::Drag {
                axis: Axis::Vertical,
                y,
                ..
            } if self.drag == Some(Axis::Vertical) => {
                if let Some(scroll) = self.top().scroll() {
                    scroll.moved(sign * y, max, now);
                }
                cx.redraw();
            }
            Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            } if self.drag == Some(Axis::Vertical) => {
                self.drag = None;
                if let Some(scroll) = self.top().scroll() {
                    scroll.end(max, now);
                }
                cx.animate();
            }
            Gesture::DragStart {
                axis: Axis::Horizontal,
                ..
            }
            | Gesture::Drag {
                axis: Axis::Horizontal,
                ..
            }
            | Gesture::DragEnd {
                axis: Axis::Horizontal,
                ..
            } => {
                let store = &self.store;
                let screen = self.stacks[self.tab as usize].last_mut().expect("root");
                if let Some(action) = screen.horizontal(store, &gesture, area) {
                    self.dirty = true;
                    self.perform(cx, action);
                    cx.animate();
                } else if let Gesture::DragEnd { x, .. } = gesture
                    && x > width * 0.45
                    && self.stacks[self.tab as usize].len() > 1
                {
                    self.perform(cx, Action::Pop);
                }
            }
            Gesture::Cancel => {
                if self.drag.take().is_some()
                    && let Some(scroll) = self.top().scroll()
                {
                    scroll.end(max, now);
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, cx: &mut Cx, key: KeyInput) {
        let store = &self.store;
        let screen = self.stacks[self.tab as usize].last_mut().expect("root");
        let action = screen.key(store, key);
        self.dirty = true;
        self.perform(cx, action);
        cx.redraw();
    }

    fn open_target(&mut self, cx: &mut Cx, target: &str) {
        if target == "activity" {
            self.stories = None;
            self.tab = Tab::Home;
            self.stacks[0].truncate(1);
            self.perform(cx, Action::Push(Box::new(ActivityScreen::new())));
        } else if !target.is_empty() {
            self.open_chat(cx, target.to_owned());
        }
        cx.redraw();
    }

    fn shown(&mut self, cx: &mut Cx, shown: bool) {
        self.shown = shown;
        // A story holds still while the app is away (no clock, no frames).
        if let Some(viewer) = &mut self.stories {
            if shown {
                viewer.resume(cx.now);
            } else {
                viewer.pause(cx.now);
            }
        }
        if shown {
            self.dirty = true;
            let action = self.show_top();
            self.perform(cx, action);
        }
    }

    fn tick(&mut self, cx: &mut Cx) {
        let now = cx.now;
        let (width, height) = (cx.width, cx.height);
        if let Some((_, at)) = &self.toast
            && now.duration_since(*at) >= TOAST_FOR
        {
            self.toast = None;
            self.dirty = true;
            cx.redraw();
        }
        if cx.keyboard_shown() != self.keyboard {
            self.keyboard = cx.keyboard_shown();
            if !self.keyboard {
                self.top().keyboard_hidden();
            }
            self.dirty = true;
            cx.redraw();
        }
        if let Some(viewer) = &mut self.stories {
            let action = viewer.tick(&self.store, now);
            if viewer.closed {
                self.stories = None;
                self.dirty = true;
                cx.redraw();
            } else if let Some(at) = viewer.next_frame() {
                // The progress bar moves at 60 Hz.
                if at <= now {
                    cx.redraw();
                } else {
                    cx.wake_at(at);
                }
            }
            self.perform(cx, action);
            return;
        }
        let max = self.max_scroll(width, height);
        let moving = self
            .top()
            .scroll()
            .is_some_and(|scroll| scroll.step(max, now));
        if moving {
            cx.animate();
        }
        let area = self.area(width, height, now);
        let store = &self.store;
        let screen = self.stacks[self.tab as usize].last_mut().expect("root");
        let animating = screen.animating(now);
        if let Some(at) = screen.wake() {
            cx.wake_at(at);
        }
        let action = screen.tick(store, area, now);
        if animating {
            self.dirty = true;
            cx.animate();
        }
        self.perform(cx, action);
    }

    fn viewing(&self) -> Option<String> {
        if self.stories.is_some() || self.tab != Tab::Messages {
            return None;
        }
        self.stacks[Tab::Messages as usize]
            .last()
            .and_then(|screen| screen.chat())
            .map(str::to_owned)
    }

    fn draw(&mut self, canvas: &mut Canvas, cx: &mut DrawCx) {
        let now = cx.now;
        // A picture the last frame waited for arrived: draw it.
        let arrived = self.missing.iter().any(|want| {
            cx.media
                .peek(&want.reference, want.w, want.h, want.fit)
                .is_some()
        });
        for want in &self.missing {
            let _ = cx.media.get(&want.reference, want.w, want.h, want.fit, now);
        }
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        let stories = self.stories.is_some();
        let mut kind = "full";
        if stories {
            let viewer = self.stories.as_mut().expect("open");
            let partial = !cx.full
                && !self.dirty
                && !self.changed
                && !arrived
                && self.last.as_ref().is_some_and(|last| last.stories);
            if partial {
                kind = "progress";
                viewer.draw_progress(canvas, &self.store, width, now, false);
                let (y0, y1) = StoryViewer::progress_rows();
                cx.damage = Some(Damage {
                    moved: None,
                    rows: vec![(y0, y1)],
                });
            } else {
                self.full_frame(canvas, cx.media, now);
            }
            if let Some(viewer) = &mut self.stories {
                viewer.drawn_at = Some(now);
            }
            self.last = Some(LastFrame {
                tab: self.tab,
                depth: 0,
                screen: 0,
                area: (0.0, height, width, height),
                offset: 0.0,
                stories: true,
            });
        } else {
            let area = self.area(width, height, now);
            let depth = self.stacks[self.tab as usize].len();
            let screen_id = (&**self.top() as *const dyn Screen).cast::<()>() as usize;
            let geometry = (area.top, area.bottom, width, height);
            let inverted = self.top().inverted();
            let blit_ok = self.top().blit_ok() && self.toast.is_none();
            let same = self.last.as_ref().is_some_and(|last| {
                !last.stories
                    && last.tab == self.tab
                    && last.depth == depth
                    && last.screen == screen_id
                    && last.area == geometry
            });
            let view_h = (area.bottom - area.top) as i32;
            let (top, bottom) = (area.top as i32, area.bottom as i32);
            let mut done = false;
            // Pictures that arrived: their rows are drawn again, when all
            // of them lie in the content (else the frame is drawn whole).
            let arrived_rows: Option<Vec<(i32, i32)>> = arrived.then(|| {
                self.missing
                    .iter()
                    .filter(|want| {
                        cx.media
                            .peek(&want.reference, want.w, want.h, want.fit)
                            .is_some()
                    })
                    .map(|want| want.rows)
                    .collect()
            });
            // (Pictures in the bars and the tab bar: drawn with the whole frame.)
            let rows_ok = arrived_rows
                .as_ref()
                .is_none_or(|rows| rows.iter().all(|&(a, _)| a != i32::MIN));
            // What the store changed, when the screen can say (else whole).
            let changed_rows = if self.changed && same && !cx.full && !self.dirty {
                self.stacks[self.tab as usize]
                    .last_mut()
                    .expect("root")
                    .changed_rows(area)
            } else {
                Some(Vec::new())
            };
            if same && !cx.full && !self.dirty && rows_ok && blit_ok && changed_rows.is_some() {
                let last_offset = self.last.as_ref().map_or(0.0, |last| last.offset);
                let delta = (area.offset - last_offset) as i32;
                // Content moves up as the offset grows (down, inverted).
                let dy = if inverted { delta } else { -delta };
                if dy.abs() < view_h - 8 {
                    let mut bands: Vec<(i32, i32)> = Vec::new();
                    if dy != 0 {
                        canvas.move_rows(top, bottom, dy);
                        bands.push(if dy < 0 {
                            (bottom + dy, bottom)
                        } else {
                            (top, top + dy)
                        });
                        // What waits for pictures moved with the rows.
                        for want in self
                            .missing
                            .iter_mut()
                            .filter(|want| want.rows.0 != i32::MIN)
                        {
                            want.rows = (want.rows.0 + dy, want.rows.1 + dy);
                        }
                    }
                    for (a, b) in arrived_rows.unwrap_or_default() {
                        let (a, b) = ((a + dy).max(top), (b + dy).min(bottom));
                        if a < b {
                            bands.push((a, b));
                        }
                    }
                    // The store's changes: rows the screen named (already
                    // where they are now), and the bars, which may show it.
                    let chrome = self.changed;
                    for (a, b) in changed_rows.unwrap_or_default() {
                        let (a, b) = (a.max(top), b.min(bottom));
                        if a < b {
                            bands.push((a, b));
                        }
                    }
                    // Drawn below; what is still missing is asked for again there.
                    self.missing.retain(|want| {
                        cx.media
                            .peek(&want.reference, want.w, want.h, want.fit)
                            .is_none()
                    });
                    let bands = merge_rows(bands);
                    cx.damage = Some(Damage {
                        moved: (dy != 0).then_some((top, bottom, dy)),
                        rows: bands.clone(),
                    });
                    let store = &self.store;
                    let screen = self.stacks[self.tab as usize].last_mut().expect("root");
                    for &(a, b) in &bands {
                        canvas.set_clip(0, a, canvas.width, b);
                        canvas.fill_rect(0, a, canvas.width, b - a, BG, 1.0);
                        let mut p = Paint {
                            media: cx.media,
                            now,
                            missing: &mut self.missing,
                            cache: &self.cache,
                        };
                        screen.draw(
                            canvas,
                            &mut p,
                            store,
                            Area {
                                band: (a as f32, b as f32),
                                ..area
                            },
                        );
                    }
                    canvas.reset_clip();
                    if chrome {
                        let height = canvas.height;
                        self.draw_bars(canvas, cx.media, now, area);
                        if let Some(damage) = &mut cx.damage {
                            damage.rows.push((0, top));
                            damage.rows.push((bottom, height));
                        }
                    }
                    kind = if bands.is_empty() {
                        "none"
                    } else if dy == 0 {
                        "pictures"
                    } else {
                        "band"
                    };
                    done = true;
                    if self.verify
                        && !bands.is_empty()
                        && !self.check_band(canvas, cx.media, now, area)
                    {
                        eprintln!("instagram: ({kind} frame, dy {dy}, bands {bands:?})");
                    }
                }
            }
            if !done {
                self.full_frame(canvas, cx.media, now);
            }
            self.last = Some(LastFrame {
                tab: self.tab,
                depth,
                screen: screen_id,
                area: geometry,
                offset: area.offset,
                stories: false,
            });
        }
        self.dirty = false;
        self.changed = false;
        self.frames += 1;
        // Profiling (the perf bench): when an opened chat first shows its
        // messages (the shell logs each frame after this).
        if self.profile
            && kind != "none"
            && let Some((chat, at)) = &self.opening
            && self.stacks[Tab::Messages as usize]
                .last()
                .and_then(|s| s.chat())
                == Some(chat.as_str())
            && self.store.threads.get(chat).is_some_and(|t| t.loaded)
        {
            eprintln!(
                "instagram: opened {chat} in {:.1} ms ({} entries)",
                at.elapsed().as_secs_f64() * 1000.0,
                self.store.threads[chat].messages.len()
            );
            self.opening = None;
        }
    }
}

/// Overlapping or touching row ranges as one.
fn merge_rows(mut rows: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    rows.sort();
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(rows.len());
    for (a, b) in rows {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// CLOCK_MONOTONIC in milliseconds (the clock the bench's touch reports use).
fn monotonic_ms() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes the time into a live timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as f64 * 1000.0 + now.tv_nsec as f64 / 1e6
}

#[cfg(test)]
mod tests {
    use super::merge_rows;

    #[test]
    fn row_ranges_merge_when_they_touch() {
        assert_eq!(
            merge_rows(vec![(10, 20), (0, 5), (18, 30), (30, 31)]),
            vec![(0, 5), (10, 31)]
        );
        assert_eq!(merge_rows(Vec::new()), Vec::<(i32, i32)>::new());
    }
}
