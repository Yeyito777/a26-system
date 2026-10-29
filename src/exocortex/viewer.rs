//! The image viewer (WP-C1, PAR-252, 253, 255): a message's images one at
//! a time over the whole window, fitted to it; "2/5" and a swipe sideways
//! between them; a double tap zooms 2.5× where it was tapped (a sharper
//! preview is fetched for it) and a drag then pans; a swipe down, Back or
//! ✕ closes it. While its preview comes, the transcript's thumbnail
//! stands in for it ("Decoding image…" until there is one); one that
//! cannot come says so, with Retry. X core events carry one pointer, so
//! there is no pinch.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::gfx::{Canvas, Image, RectF};

use super::anim::Animator;
use super::gesture::{Axis, Gesture};
use super::images::{Pic, State, Want};
use super::model::{Action, UiAct, rich};
use super::theme::{self, T_META, T_NOTE, T_SMALL, TARGET, TOP_BAR};
use super::ui::icons::{self, Icon};
use super::ui::overlay::{Body, Cx, Overlay, Response};
use super::ui::widgets::{self, Button, ButtonKind};

/// A double tap zooms this much.
const ZOOM: f32 = 2.5;
/// Two taps this close in time and place are a double tap.
const DOUBLE_TAP: Duration = Duration::from_millis(300);
const DOUBLE_TAP_SLOP: f32 = 80.0;
/// A swipe down this far closes it.
const DISMISS: f32 = 110.0;
/// The bar under the image (name and size).
const FOOTER: f32 = 140.0;
/// The largest preview asked for (the daemon's limit).
const MAX_PREVIEW: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Control {
    Close,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// Moving the zoomed image.
    Pan { last: (f32, f32) },
    /// Sideways to the next or previous image.
    Page { start: f32 },
    /// Down, to close.
    Dismiss { start: f32 },
}

pub struct Viewer {
    pics: Vec<Pic>,
    index: usize,
    conv: Option<String>,
    zoom: f32,
    pan: (f32, f32),
    drag: Option<Drag>,
    /// The finger's offset while paging or dismissing.
    offset: (f32, f32),
    last_tap: Option<(Instant, f32, f32)>,
    pressed: Option<Control>,
    closed: bool,
}

impl Viewer {
    pub fn new(pics: Vec<Pic>, index: usize, conv: Option<String>) -> Self {
        let index = index.min(pics.len().saturating_sub(1));
        Self {
            pics,
            index,
            conv,
            zoom: 1.0,
            pan: (0.0, 0.0),
            drag: None,
            offset: (0.0, 0.0),
            last_tap: None,
            pressed: None,
            closed: false,
        }
    }

    /// Where the image goes: under the top bar, over the footer.
    fn area(&self, cx: &Cx) -> RectF {
        let bottom = cx.bottom() - FOOTER;
        RectF::new(0.0, TOP_BAR, cx.width, (bottom - TOP_BAR).max(1.0))
    }

    /// The preview fitted to the window.
    fn fit_want(&self, cx: &Cx, index: usize) -> Option<Want> {
        let pic = self.pics.get(index)?;
        let area = self.area(cx);
        Some(Want {
            id: pic.id.clone()?,
            conv: self.conv.clone(),
            width: area.w as u32,
            height: (area.h as u32 / 16 * 16).max(16),
            thumbnail: false,
        })
    }

    /// The sharper preview a zoom shows: the image itself, within limits.
    fn zoom_want(&self, cx: &Cx) -> Option<Want> {
        let pic = self.pics.get(self.index)?;
        let fit = self.fit_want(cx, self.index)?;
        let (w, h) = if pic.width > 0 && pic.height > 0 {
            (pic.width, pic.height)
        } else {
            (fit.width * 3, fit.height * 3)
        };
        if w <= fit.width && h <= fit.height {
            return None;
        }
        Some(Want {
            width: w.min(MAX_PREVIEW),
            height: h.min(MAX_PREVIEW),
            ..fit
        })
    }

    /// The current image's pixels, best first: the zoomed preview, the
    /// fitted one, else any preview of it (the thumbnail).
    fn image(&self, cx: &Cx) -> Option<Rc<Image>> {
        let store = &cx.model.rich.images;
        let zoomed = (self.zoom > 1.0)
            .then(|| self.zoom_want(cx))
            .flatten()
            .and_then(|want| store.ready(&want).cloned());
        zoomed
            .or_else(|| {
                self.fit_want(cx, self.index)
                    .and_then(|want| store.ready(&want).cloned())
            })
            .or_else(|| {
                let id = self.pics.get(self.index)?.id.as_deref()?;
                store.best(id).cloned()
            })
    }

    /// The current image's fitted preview failed.
    fn failed(&self, cx: &Cx) -> bool {
        match self.pics.get(self.index) {
            Some(pic) if pic.id.is_none() => true,
            _ => self.fit_want(cx, self.index).is_some_and(|want| {
                matches!(cx.model.rich.images.state(&want), Some(State::Failed(_)))
            }),
        }
    }

    /// The image's size: its own, else its preview's.
    fn natural(&self, image: Option<&Image>) -> (f32, f32) {
        match self.pics.get(self.index) {
            Some(pic) if pic.width > 0 && pic.height > 0 => (pic.width as f32, pic.height as f32),
            _ => image.map_or((1.0, 1.0), |image| {
                (image.width as f32, image.height as f32)
            }),
        }
    }

    /// How much the image is scaled to fit the area (never enlarged).
    fn fit_scale(&self, cx: &Cx, image: Option<&Image>) -> f32 {
        let area = self.area(cx);
        let (w, h) = self.natural(image);
        (area.w / w).min(area.h / h).min(1.0)
    }

    /// The image fitted to the area, zoomed and panned.
    fn rect(&self, cx: &Cx, image: Option<&Image>) -> RectF {
        let area = self.area(cx);
        let (w, h) = self.natural(image);
        let scale = self.fit_scale(cx, image);
        let (fw, fh) = ((w * scale).round(), (h * scale).round());
        let (zw, zh) = (fw * self.zoom, fh * self.zoom);
        let cx0 = area.x + area.w / 2.0 + self.pan.0 + self.offset.0;
        let cy0 = area.y + area.h / 2.0 + self.pan.1 + self.offset.1;
        RectF::new(
            (cx0 - zw / 2.0).round(),
            (cy0 - zh / 2.0).round(),
            zw.round(),
            zh.round(),
        )
    }

    /// Keeps the zoomed image over the area.
    fn clamp_pan(&mut self, cx: &Cx) {
        let image = self.image(cx);
        let area = self.area(cx);
        let (w, h) = self.natural(image.as_deref());
        let scale = self.fit_scale(cx, image.as_deref());
        let (zw, zh) = (w * scale * self.zoom, h * scale * self.zoom);
        let max_x = ((zw - area.w) / 2.0).max(0.0);
        let max_y = ((zh - area.h) / 2.0).max(0.0);
        self.pan = (
            self.pan.0.clamp(-max_x, max_x).round(),
            self.pan.1.clamp(-max_y, max_y).round(),
        );
    }

    fn retry_rect(&self, cx: &Cx) -> RectF {
        let area = self.area(cx);
        let button = Button::new("Retry", ButtonKind::Secondary);
        let w = button.width();
        RectF::new(
            (area.x + (area.w - w) / 2.0).round(),
            (area.y + area.h / 2.0 + 60.0).round(),
            w,
            TARGET,
        )
    }

    fn control_at(&self, cx: &Cx, x: f32, y: f32) -> Option<Control> {
        if y < TOP_BAR && x < TARGET + 22.0 {
            return Some(Control::Close);
        }
        (self.failed(cx) && self.retry_rect(cx).contains(x, y)).then_some(Control::Retry)
    }

    fn go(&mut self, index: usize) {
        if index != self.index && index < self.pics.len() {
            self.index = index;
            self.zoom = 1.0;
            self.pan = (0.0, 0.0);
        }
    }
}

impl Overlay for Viewer {
    fn label(&self) -> String {
        format!("viewer: {}/{}", self.index + 1, self.pics.len())
    }

    fn scoped(&self) -> bool {
        true
    }

    fn body(&self, cx: &Cx) -> Body {
        let image = self.image(cx);
        let mut key = DefaultHasher::new();
        (
            self.index,
            self.zoom.to_bits(),
            self.pan.0.to_bits(),
            self.pan.1.to_bits(),
            self.offset.0.to_bits(),
            self.offset.1.to_bits(),
            self.pressed,
            self.failed(cx),
            image.as_ref().map(|image| Rc::as_ptr(image) as usize),
            cx.width.to_bits(),
            cx.height.to_bits(),
        )
            .hash(&mut key);
        if image.is_none() && !self.failed(cx) {
            // The spinner turns.
            ((super::anim::phase(cx.now, 700) * 64.0) as u32).hash(&mut key);
        }
        Body {
            rows: (0, cx.height as i32),
            solid: None,
            scroller: None,
            key: key.finish(),
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx) {
        let p = theme::p();
        let background = p.app_background;
        widgets::fill(
            canvas,
            RectF::new(0.0, 0.0, cx.width, cx.height),
            background,
        );
        let area = self.area(cx);
        let image = self.image(cx);
        let pic = self.pics.get(self.index);
        if let Some(image) = &image {
            let rect = self.rect(cx, Some(image));
            widgets::clipped(canvas, area, |canvas| {
                let (x, y) = (rect.x as i32, rect.y as i32);
                if image.width == rect.w as i32 && image.height == rect.h as i32 {
                    canvas.draw_image(image, x, y, 1.0);
                } else {
                    canvas.draw_image_scaled(image, rect, 1.0);
                }
            });
        } else if self.failed(cx) {
            let title_ty = theme::T_ROW.weight(crate::text::Weight::MonoSemiBold);
            let cy = area.y + area.h / 2.0;
            widgets::text_centered(
                canvas,
                "Image unavailable",
                cx.width / 2.0,
                title_ty.baseline(cy - 110.0),
                title_ty,
                p.text,
                1.0,
            );
            widgets::text_centered(
                canvas,
                "The image bytes are missing or could not be decoded.",
                cx.width / 2.0,
                T_SMALL.baseline(cy - 40.0),
                T_SMALL,
                p.muted,
                1.0,
            );
            if pic.is_some_and(|pic| pic.id.is_some()) {
                let button = Button::new("Retry", ButtonKind::Secondary)
                    .pressed(self.pressed == Some(Control::Retry));
                widgets::button(canvas, self.retry_rect(cx), &button, background);
            }
        } else {
            let cy = area.y + area.h / 2.0;
            widgets::spinner(
                canvas,
                cx.width / 2.0,
                cy - 30.0,
                theme::SPINNER,
                p.accent,
                background,
                cx.now,
            );
            widgets::text_centered(
                canvas,
                "Decoding image…",
                cx.width / 2.0,
                T_META.baseline(cy + 10.0),
                T_META,
                p.muted,
                1.0,
            );
        }
        // The bar: ✕ and the position.
        widgets::fill(canvas, RectF::new(0.0, 0.0, cx.width, TOP_BAR), background);
        icons::draw(
            canvas,
            Icon::X,
            72.0,
            TOP_BAR / 2.0,
            theme::ICON,
            p.text,
            if self.pressed == Some(Control::Close) {
                0.5
            } else {
                1.0
            },
        );
        if self.pics.len() > 1 {
            widgets::text_centered(
                canvas,
                &format!("{}/{}", self.index + 1, self.pics.len()),
                cx.width / 2.0,
                T_NOTE.baseline(TOP_BAR / 2.0 - T_NOTE.line() / 2.0),
                T_NOTE,
                p.text,
                1.0,
            );
        }
        // The footer: name, size.
        let footer = cx.bottom() - FOOTER;
        widgets::fill(
            canvas,
            RectF::new(0.0, footer, cx.width, cx.height - footer),
            background,
        );
        if let Some(pic) = pic {
            let name = widgets::fit(&pic.title(self.index), T_SMALL, cx.width - 88.0);
            widgets::text(
                canvas,
                &name,
                44.0,
                T_SMALL.baseline(footer + 22.0),
                T_SMALL,
                theme::c().text_dim,
                1.0,
            );
            widgets::text(
                canvas,
                &pic.details(),
                44.0,
                T_META.baseline(footer + 22.0 + T_SMALL.line()),
                T_META,
                p.muted,
                1.0,
            );
        }
    }

    fn gesture(&mut self, gesture: &Gesture, cx: &Cx) -> Response {
        match *gesture {
            Gesture::Down { x, y } => {
                self.pressed = self.control_at(cx, x, y);
                Response::Handled
            }
            Gesture::Tap { x, y } => {
                let pressed = self.pressed.take();
                match self.control_at(cx, x, y).filter(|c| Some(*c) == pressed) {
                    Some(Control::Close) => return Response::Action(Action::Ui(UiAct::Dismiss)),
                    Some(Control::Retry) => {
                        if let Some(want) = self.fit_want(cx, self.index) {
                            return Response::Action(Action::Rich(rich::Act::RetryImage(want)));
                        }
                        return Response::Handled;
                    }
                    None => {}
                }
                let double = self.last_tap.is_some_and(|(at, tx, ty)| {
                    cx.now.duration_since(at) <= DOUBLE_TAP
                        && (tx - x).abs() < DOUBLE_TAP_SLOP
                        && (ty - y).abs() < DOUBLE_TAP_SLOP
                });
                if double {
                    self.last_tap = None;
                    if self.zoom > 1.0 {
                        self.zoom = 1.0;
                        self.pan = (0.0, 0.0);
                    } else {
                        // The tapped point stays under the finger.
                        let area = self.area(cx);
                        let (ox, oy) = (x - (area.x + area.w / 2.0), y - (area.y + area.h / 2.0));
                        self.zoom = ZOOM;
                        self.pan = (-ox * (ZOOM - 1.0), -oy * (ZOOM - 1.0));
                        self.clamp_pan(cx);
                    }
                } else {
                    self.last_tap = Some((cx.now, x, y));
                }
                Response::Handled
            }
            Gesture::DragStart {
                axis,
                x,
                y,
                start_x,
                start_y,
                ..
            } => {
                self.pressed = None;
                self.drag = Some(if self.zoom > 1.0 {
                    Drag::Pan {
                        last: (start_x, start_y),
                    }
                } else if axis == Axis::Horizontal {
                    Drag::Page { start: start_x }
                } else {
                    Drag::Dismiss { start: start_y }
                });
                self.dragged(cx, x, y);
                Response::Handled
            }
            Gesture::Drag { x, y, .. } => {
                self.dragged(cx, x, y);
                Response::Handled
            }
            Gesture::DragEnd { vx, vy, .. } => {
                let drag = self.drag.take();
                let offset = std::mem::take(&mut self.offset);
                match drag {
                    Some(Drag::Page { .. }) => {
                        let far = offset.0.abs() > cx.width * 0.25 || vx.abs() > 800.0;
                        if far && (offset.0 < 0.0 || vx < -800.0) {
                            self.go(self.index + 1);
                        } else if far && self.index > 0 {
                            self.go(self.index - 1);
                        }
                    }
                    Some(Drag::Dismiss { .. }) if offset.1 > DISMISS || vy > 1500.0 => {
                        return Response::Action(Action::Ui(UiAct::Dismiss));
                    }
                    _ => {}
                }
                Response::Handled
            }
            Gesture::Back => Response::Action(Action::Ui(UiAct::Dismiss)),
            _ => {
                self.pressed = None;
                Response::Handled
            }
        }
    }

    fn close(&mut self, _now: Instant) {
        self.closed = true;
    }

    fn closing(&self) -> bool {
        self.closed
    }

    fn finished(&self, _now: Instant) -> bool {
        self.closed
    }

    fn animate(&self, cx: &Cx, animator: &mut Animator) {
        if self.image(cx).is_none() && !self.failed(cx) {
            animator.slow_frame(cx.now);
        }
    }

    fn images(&self, cx: &Cx) -> Vec<Want> {
        let mut wants = Vec::new();
        if self.zoom > 1.0
            && let Some(want) = self.zoom_want(cx)
        {
            wants.push(want);
        }
        for index in [self.index, self.index + 1, self.index.wrapping_sub(1)] {
            if let Some(want) = self.fit_want(cx, index) {
                wants.push(want);
            }
        }
        wants
    }
}

impl Viewer {
    fn dragged(&mut self, cx: &Cx, x: f32, y: f32) {
        match &mut self.drag {
            Some(Drag::Pan { last }) => {
                let (dx, dy) = (x - last.0, y - last.1);
                *last = (x, y);
                self.pan = (self.pan.0 + dx, self.pan.1 + dy);
                self.clamp_pan(cx);
            }
            Some(Drag::Page { start }) => {
                let mut dx = x - *start;
                // Resisting past the ends.
                if (dx > 0.0 && self.index == 0) || (dx < 0.0 && self.index + 1 >= self.pics.len())
                {
                    dx /= 3.0;
                }
                self.offset = (dx.round(), 0.0);
            }
            Some(Drag::Dismiss { start }) => {
                self.offset = (0.0, (y - *start).max(0.0).round());
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Model;

    fn pics(n: usize) -> Vec<Pic> {
        (0..n)
            .map(|i| Pic {
                id: Some(format!("img{i}")),
                width: 1080,
                height: 2400,
                size_bytes: 500_000,
                media_type: "image/png".into(),
                name: Some(format!("shot{i}.png")),
            })
            .collect()
    }

    fn cx(model: &Model, now: Instant) -> Cx<'_> {
        Cx {
            model,
            width: 1080.0,
            height: 2276.0,
            keyboard: false,
            now,
        }
    }

    #[test]
    fn it_asks_for_the_image_fitted_and_its_neighbours() {
        let model = Model::new();
        let now = Instant::now();
        let viewer = Viewer::new(pics(3), 1, Some("c".into()));
        let wants = viewer.images(&cx(&model, now));
        let ids: Vec<&str> = wants.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["img1", "img2", "img0"]);
        assert_eq!(wants[0].width, 1080);
        assert!(!wants[0].thumbnail);
        assert_eq!(wants[0].conv.as_deref(), Some("c"));
    }

    #[test]
    fn swipes_page_double_taps_zoom_and_a_swipe_down_closes() {
        let model = Model::new();
        let mut now = Instant::now();
        let mut viewer = Viewer::new(pics(3), 0, None);
        let start = |axis, x: f32, y: f32| Gesture::DragStart {
            axis,
            x,
            y,
            start_x: 540.0,
            start_y: 1000.0,
            edge: false,
        };
        let end = |axis, vx, vy| Gesture::DragEnd {
            axis,
            x: 0.0,
            y: 0.0,
            vx,
            vy,
        };
        // Left: the next image.
        viewer.gesture(&start(Axis::Horizontal, 300.0, 1000.0), &cx(&model, now));
        assert_eq!(viewer.offset.0, -240.0);
        viewer.gesture(&end(Axis::Horizontal, -900.0, 0.0), &cx(&model, now));
        assert_eq!(viewer.index, 1);
        assert_eq!(viewer.label(), "viewer: 2/3");
        // A double tap zooms; the pan stays within the image.
        for _ in 0..2 {
            viewer.gesture(&Gesture::Tap { x: 900.0, y: 600.0 }, &cx(&model, now));
            now += Duration::from_millis(120);
        }
        assert_eq!(viewer.zoom, ZOOM);
        let c = cx(&model, now);
        let area = viewer.area(&c);
        let fitted_w = (1080.0_f32 * (area.h / 2400.0)).round();
        let max_x = ((fitted_w * ZOOM - area.w) / 2.0).max(0.0);
        assert!(viewer.pan.0.abs() <= max_x + 0.5);
        // Zoomed, a drag pans instead of paging.
        viewer.gesture(&start(Axis::Horizontal, 500.0, 1000.0), &c);
        viewer.gesture(&end(Axis::Horizontal, -900.0, 0.0), &c);
        assert_eq!(viewer.index, 1);
        // Double tap back, then a swipe down closes.
        for _ in 0..2 {
            viewer.gesture(&Gesture::Tap { x: 540.0, y: 900.0 }, &cx(&model, now));
            now += Duration::from_millis(120);
        }
        assert_eq!(viewer.zoom, 1.0);
        let c = cx(&model, now);
        viewer.gesture(&start(Axis::Vertical, 540.0, 1200.0), &c);
        assert_eq!(
            viewer.gesture(&end(Axis::Vertical, 0.0, 300.0), &c),
            Response::Action(Action::Ui(UiAct::Dismiss))
        );
    }
}
