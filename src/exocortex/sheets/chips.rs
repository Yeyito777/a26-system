//! The chat's chips (WP-E; PAR-287, 299, 305; the iPhone's
//! `ui/ops/chat/ChatOverlays.tsx`): small floating buttons under the chat
//! header, right-aligned, so the transcript never shifts: usage when this
//! chat's provider is at 20 % or less of a window (two bars and the
//! share), the aside while there is one, and the tasks with their count.
//! A floating overlay of the page's (it hides under sheets); a tap opens
//! the panel. A layer (`ui/layer.rs`): drawn once into a sprite, it is
//! blended again over the page a scroll moves under it, without the page
//! being drawn there again.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;

use crate::gfx::{Canvas, RectF, Rgb};
use crate::text::Weight;

use super::super::anim::Animator;
use super::super::gesture::Gesture;
use super::super::model::{Action, Model, Page, ops};
use super::super::theme::{self, HAIRLINE, T_CAPTION, T_EYEBROW, T_META, TOP_BAR, W};
use super::super::ui::layer::{Layer, Sprite};
use super::super::ui::overlay::{Body, Cx, Overlay, Response};
use super::super::ui::widgets;
use super::usage;

/// A chip's target and its face inside it.
const TARGET_H: f32 = 122.0;
const FACE_H: f32 = 78.0;
const PAD_X: f32 = 25.0;
const GAP: f32 = 14.0;
/// The usage chip's bars.
const BAR_W: f32 = 78.0;
const BAR_H: f32 = 8.0;
/// The shadow's blur and its drop: compact, as the jump button's (the
/// iPhone's 14 pt blur barely shows on the dark themes, and every row and
/// column it reaches is drawn and sent again on every frame of a scroll).
const SHADOW: f32 = 14.0;
const SHADOW_DY: f32 = 5.0;

/// What a chip shows.
#[derive(Debug, Clone, PartialEq, Hash)]
enum Chip {
    /// Remaining shares 0–100 of the 5-hour and weekly windows.
    Usage {
        five: Option<u32>,
        week: Option<u32>,
        color: Rgb,
        low: u32,
    },
    Btw {
        busy: bool,
        status: String,
    },
    Tasks {
        glyphs: Vec<(&'static str, Rgb)>,
        count: usize,
    },
}

impl Chip {
    fn action(&self, model: &Model) -> Action {
        Action::Ops(match self {
            Chip::Usage { .. } if model.accounts_likely() => ops::Act::OpenAccounts,
            Chip::Usage { .. } => ops::Act::OpenUsage,
            Chip::Btw { .. } => ops::Act::OpenBtw,
            Chip::Tasks { .. } => ops::Act::OpenActivity,
        })
    }

    fn width(&self) -> f32 {
        let inner = match self {
            Chip::Usage { low, .. } => {
                BAR_W + GAP + widgets::measure(&format!("{low}%"), bold_meta())
            }
            Chip::Btw { status, .. } => {
                14.0 + GAP
                    + widgets::measure("BTW", T_EYEBROW)
                    + GAP
                    + widgets::measure(status, T_CAPTION)
            }
            Chip::Tasks { glyphs, count } => {
                glyphs.len() as f32 * (widgets::measure("◆", T_CAPTION) + 8.0)
                    + widgets::measure("Tasks", T_EYEBROW)
                    + GAP
                    + widgets::measure(&count.to_string(), bold_caption())
            }
        };
        (inner + 2.0 * PAD_X).ceil()
    }
}

fn bold_meta() -> theme::Type {
    T_META.weight(Weight::MonoBold)
}

fn bold_caption() -> theme::Type {
    T_CAPTION.weight(Weight::MonoBold)
}

/// The chips the open chat shows now, left to right.
fn chips(model: &Model) -> Vec<Chip> {
    let mut chips = Vec::new();
    let Some(conv) = model
        .chat
        .id
        .as_deref()
        .filter(|_| model.page == Page::Chat)
    else {
        return chips;
    };
    let provider = model.resolve().provider;
    if let Some((five, week)) = usage::compact(model.usage_of(&provider)) {
        let low = five.unwrap_or(100.0).min(week.unwrap_or(100.0));
        if low <= 20.0 {
            chips.push(Chip::Usage {
                five: five.map(|v| v.round() as u32),
                week: week.map(|v| v.round() as u32),
                color: usage::usage_color(low),
                low: low.round() as u32,
            });
        }
    }
    if let Some(aside) = model.aside(conv) {
        let busy = aside.busy();
        let status = if busy {
            let last = aside
                .turns
                .last()
                .map(|t| t.status.clone())
                .unwrap_or_default();
            if last.is_empty() {
                "Thinking…".into()
            } else {
                last
            }
        } else if aside.phase == "error" {
            "error".into()
        } else {
            aside.turns.len().to_string()
        };
        chips.push(Chip::Btw {
            busy,
            status: widgets::fit(&status, T_CAPTION, 305.0),
        });
    }
    let (glyphs, count) = model.activity_glance(conv);
    if count > 0 {
        chips.push(Chip::Tasks { glyphs, count });
    }
    chips
}

/// The chat's chips, one overlay for all of them.
#[derive(Default)]
pub struct ChatChips {
    shown: Vec<(RectF, Chip)>,
    pressed: Option<usize>,
    /// Everything their pixels depend on (`ChatChips::look`).
    look: u64,
    /// The theme generation (text size) they were placed at.
    placed: u64,
    /// The chips as last drawn, by `look`.
    sprite: RefCell<Option<(u64, Rc<Sprite>)>>,
    /// The page under them.
    layer: Layer,
}

impl ChatChips {
    /// Where the chips' faces are, right-aligned under the header.
    fn place(chips: Vec<Chip>) -> Vec<(RectF, Chip)> {
        let mut right = W - 11.0;
        let top = TOP_BAR + HAIRLINE + 6.0 + (TARGET_H - FACE_H) / 2.0;
        let mut out: Vec<(RectF, Chip)> = chips
            .into_iter()
            .rev()
            .map(|chip| {
                let w = chip.width();
                let rect = RectF::new((right - w).round(), top.round(), w, FACE_H);
                right -= w + 11.0;
                (rect, chip)
            })
            .collect();
        out.reverse();
        out
    }

    /// Everything the chips' pixels depend on.
    fn look(&self) -> u64 {
        let mut key = DefaultHasher::new();
        for (rect, chip) in &self.shown {
            (rect.x as i32, chip).hash(&mut key);
        }
        (self.pressed, theme::generation()).hash(&mut key);
        key.finish()
    }

    /// Their rows, shadow included.
    fn rows(&self) -> Option<(i32, i32)> {
        let first = self.shown.first()?;
        Some((
            (first.0.y - SHADOW).floor() as i32,
            (first.0.y + FACE_H + SHADOW_DY + SHADOW).ceil() as i32,
        ))
    }

    /// The chips drawn (once for what they show).
    fn sprite(&self) -> Option<Rc<Sprite>> {
        let ((x0, x1), (y0, y1)) = (self.columns()?, self.rows()?);
        let key = self.look;
        let mut cached = self.sprite.borrow_mut();
        if let Some((k, sprite)) = cached.as_ref()
            && *k == key
        {
            return Some(sprite.clone());
        }
        let sprite = Rc::new(Sprite::render((x0, y0, x1, y1), |canvas, ox, oy| {
            self.draw_chips(canvas, ox, oy)
        }));
        *cached = Some((key, sprite.clone()));
        Some(sprite)
    }

    /// Draws the chips, at canvas coordinates less (`ox`, `oy`).
    fn draw_chips(&self, canvas: &mut Canvas, ox: f32, oy: f32) {
        let p = theme::p();
        let c = theme::c();
        let face = theme::mix(p.surface_background, 88, p.app_background);
        let border = theme::mix(p.border, 70, p.app_background);
        for (index, (rect, chip)) in self.shown.iter().enumerate() {
            let rect = RectF::new(rect.x - ox, rect.y - oy, rect.w, rect.h);
            // Pressed, the face shrinks to 95 %.
            let rect = if self.pressed == Some(index) {
                let dx = (rect.w * 0.025).round();
                let dy = (rect.h * 0.025).round();
                RectF::new(
                    rect.x + dx,
                    rect.y + dy,
                    rect.w - 2.0 * dx,
                    rect.h - 2.0 * dy,
                )
            } else {
                rect
            };
            widgets::shadow(canvas, rect, SHADOW, SHADOW_DY, 0.35);
            widgets::boxed(canvas, rect, face, border);
            let mut x = rect.x + PAD_X;
            let cy = rect.y + rect.h / 2.0;
            let baseline = |ty: theme::Type| ty.baseline(cy - ty.line() / 2.0);
            match chip {
                Chip::Usage {
                    five,
                    week,
                    color,
                    low,
                } => {
                    for (index, share) in [five, week].into_iter().enumerate() {
                        let y = (cy - BAR_H - 4.0 + index as f32 * (BAR_H + 8.0)).round();
                        widgets::fill(canvas, RectF::new(x, y, BAR_W, BAR_H), c.switch_off);
                        let w = (BAR_W * share.unwrap_or(0).min(100) as f32 / 100.0).round();
                        if w > 0.0 {
                            widgets::fill(canvas, RectF::new(x, y, w, BAR_H), *color);
                        }
                    }
                    x += BAR_W + GAP;
                    let ty = bold_meta();
                    widgets::text(canvas, &format!("{low}%"), x, baseline(ty), ty, *color, 1.0);
                }
                Chip::Btw { busy, status } => {
                    let dot = if *busy { p.accent } else { p.secondary_accent };
                    canvas.fill_circle(x + 7.0, cy, 7.0, dot, 1.0);
                    x += 14.0 + GAP;
                    x += widgets::text(
                        canvas,
                        "BTW",
                        x,
                        baseline(T_EYEBROW),
                        T_EYEBROW,
                        p.muted,
                        1.0,
                    ) + GAP;
                    widgets::text(
                        canvas,
                        status,
                        x,
                        baseline(T_CAPTION),
                        T_CAPTION,
                        p.text,
                        1.0,
                    );
                }
                Chip::Tasks { glyphs, count } => {
                    for (glyph, color) in glyphs {
                        x += widgets::text(
                            canvas,
                            glyph,
                            x,
                            baseline(T_CAPTION),
                            T_CAPTION,
                            *color,
                            1.0,
                        ) + 8.0;
                    }
                    x += widgets::text(
                        canvas,
                        "Tasks",
                        x,
                        baseline(T_EYEBROW),
                        T_EYEBROW,
                        p.muted,
                        1.0,
                    ) + GAP;
                    let ty = bold_caption();
                    widgets::text(canvas, &count.to_string(), x, baseline(ty), ty, p.text, 1.0);
                }
            }
        }
    }

    fn at(&self, x: f32, y: f32) -> Option<usize> {
        self.shown.iter().position(|(rect, _)| {
            let target = RectF::new(
                rect.x - 5.0,
                rect.y - (TARGET_H - FACE_H) / 2.0,
                rect.w + 10.0,
                TARGET_H,
            );
            target.contains(x, y)
        })
    }
}

impl Overlay for ChatChips {
    fn label(&self) -> String {
        "chat chips".into()
    }

    fn modal(&self) -> bool {
        false
    }

    fn page_level(&self) -> bool {
        true
    }

    fn columns(&self) -> Option<(i32, i32)> {
        let left = self.shown.first()?.0.x;
        Some(((left - SHADOW - 2.0).floor() as i32, W as i32))
    }

    fn layout(&mut self, cx: &Cx) {
        let chips = chips(cx.model);
        // Placed (text measured) only when they change.
        let generation = theme::generation();
        if self.placed != generation || !self.shown.iter().map(|(_, c)| c).eq(chips.iter()) {
            self.shown = Self::place(chips);
            self.placed = generation;
        }
        if self.pressed.is_some_and(|i| i >= self.shown.len()) {
            self.pressed = None;
        }
        self.look = self.look();
    }

    fn body(&self, _cx: &Cx) -> Body {
        let Some(rows) = self.rows() else {
            return Body::empty();
        };
        Body {
            rows,
            solid: None,
            scroller: None,
            key: self.look,
        }
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx) {
        match self.sprite() {
            Some(sprite) => self.layer.paint(canvas, &sprite),
            None => self.layer.clear(),
        }
    }

    fn repaint(
        &self,
        canvas: &mut Canvas,
        moved: Option<(i32, i32, i32)>,
        rows: &[(i32, i32)],
        areas: &[(i32, i32, i32, i32)],
    ) -> Option<Vec<(i32, i32, i32, i32)>> {
        let sprite = self.sprite()?;
        self.layer.repaint(canvas, &sprite, moved, rows, areas)
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } => match self.at(x, y) {
                Some(index) => {
                    self.pressed = Some(index);
                    Response::Handled
                }
                None => Response::Pass,
            },
            Gesture::Tap { x, y } => {
                self.pressed = None;
                match self.at(x, y) {
                    Some(index) => Response::Action(self.shown[index].1.action(cx.model)),
                    None => Response::Handled,
                }
            }
            _ => {
                self.pressed = None;
                Response::Handled
            }
        }
    }

    fn close(&mut self, _now: Instant) {}

    fn closing(&self) -> bool {
        false
    }

    fn finished(&self, _now: Instant) -> bool {
        false
    }

    fn animate(&self, _cx: &Cx, _animator: &mut Animator) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::ui::overlay::OverlayStack;
    use crate::exocortex::ui::sheet::{ActionList, action_sheet};

    #[test]
    fn chips_show_low_usage_the_aside_and_tasks_and_hide_under_sheets() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        let now = Instant::now();
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","title":"T","provider":"anthropic","model":"m","goal":{"objective":"o","status":"active","turns":1,"createdAt":1}}}"#),
            now,
        );
        model.apply(
            parse_event(r#"{"type":"usage_update","provider":"anthropic","usage":{"fiveHour":{"utilization":90,"resetsAt":null},"sevenDay":{"utilization":50,"resetsAt":null}}}"#),
            now,
        );
        assert!(chips(&model).is_empty(), "on the list");
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let shown = chips(&model);
        assert!(matches!(&shown[0], Chip::Usage { low: 10, .. }));
        assert!(matches!(&shown[1], Chip::Tasks { count: 1, .. }));
        let mut stack = OverlayStack::default();
        stack.push(Box::new(ChatChips::default()));
        let cx = Cx {
            model: &model,
            width: 1080.0,
            height: 2216.0,
            keyboard: false,
            now,
        };
        stack.layout(&cx);
        let (_, floating) = stack.bodies(&cx);
        assert!(floating[0].1.rows.1 > 0);
        // A tap on the tasks chip opens the panel.
        let tasks = ChatChips::place(chips(&model))[1].0;
        let tap = Gesture::Tap {
            x: tasks.x + 10.0,
            y: tasks.y + 10.0,
        };
        stack.gesture(
            &Gesture::Down {
                x: tap_x(&tap),
                y: tap_y(&tap),
            },
            &cx,
        );
        assert_eq!(
            stack.gesture(&tap, &cx),
            Response::Action(Action::Ops(ops::Act::OpenActivity))
        );
        // Under a sheet: nothing.
        stack.push(Box::new(action_sheet(ActionList::new("x", Vec::new()))));
        stack.layout(&cx);
        let (_, floating) = stack.bodies(&cx);
        assert_eq!(floating[0].1, Body::empty());
    }

    #[test]
    fn the_tasks_chip_agrees_with_the_activity_panel() {
        let mut model = Model::new();
        model.on_connected();
        let now = Instant::now();
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","title":"T","goal":{"objective":"o","status":"paused","turns":1},"tasks":[{"id":"s","kind":"subagent","title":"a","startedAt":1},{"id":"w","kind":"chrono","chronoMode":"wait","title":"w","startedAt":1},{"id":"k","kind":"chrono","chronoMode":"schedule","title":"k","startedAt":1},{"id":"b","kind":"background","toolName":"Agent","title":"b","startedAt":1},{"id":"x","kind":"background","toolName":"exec_command","title":"x","startedAt":1}],"integrations":[{"id":"i","toolName":"gmail"}]}}"#),
            now,
        );
        let items = model.activity_items("c", now);
        let mut expected: Vec<(&str, Rgb)> = Vec::new();
        let first = |kinds: &[&str]| items.iter().find(|i| kinds.contains(&i.kind));
        if let Some(goal) = first(&["goal"]) {
            expected.push((
                if goal.label.starts_with('◆') {
                    "◆"
                } else {
                    "◇"
                },
                goal.color,
            ));
        }
        for (kinds, glyph) in [
            (&["subagent", "agent"][..], "◆"),
            (&["chrono"], "◷"),
            (&["bash"], "$"),
        ] {
            if let Some(item) = first(kinds) {
                expected.push((glyph, item.color));
            }
        }
        assert_eq!(model.activity_glance("c"), (expected, items.len()));
        assert_eq!(items.len(), 6, "the wait is not listed");
    }

    #[test]
    fn a_scroll_under_the_chips_draws_what_a_full_frame_does() {
        use crate::exocortex::view::{ViewState, frame};
        use crate::gfx::Canvas;
        const SIZE: (i32, i32) = (1080, 2216);
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        let now = Instant::now();
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","title":"T","provider":"anthropic","model":"m","goal":{"objective":"o","status":"paused","turns":1}}}"#),
            now,
        );
        model.apply(
            parse_event(r#"{"type":"usage_update","provider":"anthropic","usage":{"fiveHour":{"utilization":95,"resetsAt":null}}}"#),
            now,
        );
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let request = commands[0]["reqId"].as_str().unwrap().to_owned();
        let entries: Vec<String> = (0..30)
            .map(|i| format!(r#"{{"type":"user","text":"Question {i}"}},{{"type":"ai","blocks":[{{"type":"text","text":"Answer {i}: a few words that wrap onto a second line of the transcript, more or less."}}]}}"#))
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{}]}}"#,
                entries.join(",")
            )),
            now,
        );
        let mut view = ViewState::new();
        view.overlays.push(Box::new(ChatChips::default()));
        let full = |model: &mut Model, view: &mut ViewState| {
            let mut canvas = Canvas::new(SIZE.0, SIZE.1);
            frame::clear(&mut canvas);
            frame::draw_full(&mut canvas, model, view, None, now);
            canvas
        };
        view.chat_scroll.offset = 400.0;
        let mut old = frame::record(&mut model, &mut view, None, SIZE, now);
        let mut canvas = full(&mut model, &mut view);
        let mut chips_drawn = false;
        let repaints = crate::exocortex::ui::layer::REPAINTS.with(|n| n.get());
        let steps = [430.0, 380.0, 500.0, 120.0, 121.0, 90.0];
        for to in steps {
            view.chat_scroll.offset = to;
            let new = frame::record(&mut model, &mut view, None, SIZE, now);
            let mut shadow = canvas.pixels.clone();
            let damage = frame::update(&mut canvas, &mut model, &mut view, None, &old, &new, now)
                .expect("incremental");
            let whole = full(&mut model, &mut view);
            assert!(
                whole.pixels == canvas.pixels,
                "differs from a full frame at {to}"
            );
            // The window (the last frame, moved as the damage says) differs
            // only where the damage says.
            let width = SIZE.0;
            if let Some((y0, y1, dy)) = damage.moved {
                let rows = y1 - y0 - dy.abs();
                let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
                shadow.copy_within(
                    (src * width) as usize..((src + rows) * width) as usize,
                    (dst * width) as usize,
                );
            }
            frame::assert_unchanged_outside(&shadow, &canvas.pixels, width as usize, &damage);
            chips_drawn |= damage
                .areas
                .iter()
                .any(|(x0, y0, x1, y1)| *x0 > 600 && *x1 == 1080 && *y0 <= 200 && *y1 > 200);
            old = new;
        }
        assert!(
            chips_drawn,
            "the chips' rows are drawn again over the moved page"
        );
        assert_eq!(
            crate::exocortex::ui::layer::REPAINTS.with(|n| n.get()) - repaints,
            steps.len(),
            "the chips repainted themselves over the page, which was not drawn under them"
        );
    }

    /// What a scroll frame costs with the chips and without them (release:
    /// `cargo test --release -- --ignored chips_cost --nocapture`).
    #[test]
    #[ignore]
    fn chips_cost() {
        use crate::exocortex::view::{ViewState, frame};
        use crate::gfx::Canvas;
        const SIZE: (i32, i32) = (1080, 2216);
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        let now = Instant::now();
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","title":"T","provider":"anthropic","model":"m","goal":{"objective":"o","status":"active","turns":1,"createdAt":1},"tasks":[{"id":"s1","kind":"subagent","title":"a","startedAt":1},{"id":"b1","kind":"background","title":"b","startedAt":1}]}}"#),
            now,
        );
        model.apply(
            parse_event(r#"{"type":"usage_update","provider":"anthropic","usage":{"fiveHour":{"utilization":95,"resetsAt":null}}}"#),
            now,
        );
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let request = commands[0]["reqId"].as_str().unwrap().to_owned();
        let entries: Vec<String> = (0..200)
            .map(|i| format!(r#"{{"type":"user","text":"Question {i}"}},{{"type":"ai","blocks":[{{"type":"text","text":"Answer {i}: **a few words** that wrap onto a second line of the transcript, more or less, and `code`."}}]}}"#))
            .collect();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{}]}}"#,
                entries.join(",")
            )),
            now,
        );
        {
            let t = Instant::now();
            for _ in 0..400 {
                std::hint::black_box(super::chips(&model));
            }
            println!("chips(): {:.1} µs", t.elapsed().as_secs_f64() * 1e6 / 400.0);
            let overlay = ChatChips {
                shown: ChatChips::place(super::chips(&model)),
                ..ChatChips::default()
            };
            let t = Instant::now();
            for _ in 0..400 {
                std::hint::black_box(overlay.look());
            }
            println!("look(): {:.1} µs", t.elapsed().as_secs_f64() * 1e6 / 400.0);
            let t = Instant::now();
            for _ in 0..400 {
                std::hint::black_box(ChatChips::place(super::chips(&model)));
            }
            println!(
                "place(chips()): {:.1} µs",
                t.elapsed().as_secs_f64() * 1e6 / 400.0
            );
            let mut canvas = Canvas::new(SIZE.0, SIZE.1);
            overlay.layer.paint(&mut canvas, &overlay.sprite().unwrap());
            let sprite = overlay.sprite().unwrap();
            let t = Instant::now();
            for n in 0..400 {
                let dy = if n % 2 == 0 { 23 } else { -23 };
                std::hint::black_box(overlay.layer.repaint(
                    &mut canvas,
                    &sprite,
                    Some((250, 1900, dy)),
                    &[],
                    &[],
                ));
            }
            println!("repaint: {:.1} µs", t.elapsed().as_secs_f64() * 1e6 / 400.0);
            println!("sprite: {}", sprite.stats());
        }
        for chips in [false, true, false, true] {
            let mut view = ViewState::new();
            if chips {
                view.overlays.push(Box::new(ChatChips::default()));
            }
            view.follow = false;
            view.chat_scroll.offset = 3000.0;
            let mut canvas = Canvas::new(SIZE.0, SIZE.1);
            frame::draw_full(&mut canvas, &mut model, &mut view, None, now);
            let mut old = frame::record(&mut model, &mut view, None, SIZE, now);
            let started = Instant::now();
            let frames = 400;
            for n in 0..frames {
                view.chat_scroll.offset = 3000.0 + (n % 100) as f32 * 23.0;
                let new = frame::record(&mut model, &mut view, None, SIZE, now);
                frame::update(&mut canvas, &mut model, &mut view, None, &old, &new, now);
                old = new;
            }
            println!(
                "chips {chips}: {:.1} µs a frame, {} repaints",
                started.elapsed().as_secs_f64() * 1e6 / frames as f64,
                crate::exocortex::ui::layer::REPAINTS.with(|n| n.get())
            );
        }
    }

    fn tap_x(gesture: &Gesture) -> f32 {
        match gesture {
            Gesture::Tap { x, .. } => *x,
            _ => 0.0,
        }
    }

    fn tap_y(gesture: &Gesture) -> f32 {
        match gesture {
            Gesture::Tap { y, .. } => *y,
            _ => 0.0,
        }
    }
}
