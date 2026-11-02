//! Drawing helpers the Discord app's views share: pictures cut to circles
//! and rounded squares (cached per picture and shape), rounded corners for
//! photos, count badges, italic text (Inter has no italic cut here: the
//! upright glyphs are slanted 12°), switches and simple icons the shared
//! icon set lacks (a speaker, a megaphone, a forum's bubbles, a thread's
//! spine).

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use crate::gfx::{Canvas, Image, Path, RectF, Rgb};
use crate::social::media::{Fit, Media};
use crate::social::proto;
use crate::text::{self, Weight};

use super::theme::{self, dp};

#[derive(Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
    reference: String,
    size: u32,
    radius: u32,
}

struct Shaped {
    source: Rc<Image>,
    image: Rc<Image>,
}

thread_local! {
    /// Pictures asked for and not ready, since the last full frame.
    static MISSES: RefCell<Vec<(String, u32, u32, Fit)>> = const { RefCell::new(Vec::new()) };
    static SHAPED: RefCell<HashMap<ShapeKey, Shaped>> = RefCell::new(HashMap::new());
    static CORNERS: RefCell<HashMap<u32, Rc<[Vec<u8>; 4]>>> = RefCell::new(HashMap::new());
    static ITALIC: RefCell<HashMap<(String, u8, u32), Rc<(i32, i32, i32, Vec<u8>)>>> = RefCell::new(HashMap::new());
    static MASKS: RefCell<HashMap<(u8, u32), Rc<(i32, i32, i32, i32, Vec<u8>)>>> = RefCell::new(HashMap::new());
}

/// `Media::get`, remembering what was not ready (a frame drawn in part
/// must be drawn whole once one of those comes).
pub fn get(
    media: &mut Media,
    reference: &str,
    width: u32,
    height: u32,
    fit: Fit,
    now: Instant,
) -> Option<Rc<Image>> {
    let got = media.get(reference, width, height, fit, now);
    if got.is_none() && !reference.is_empty() && width > 0 && height > 0 {
        MISSES.with(|cell| {
            let mut misses = cell.borrow_mut();
            if misses.len() < 512 {
                misses.push((reference.to_owned(), width.min(4096), height.min(4096), fit));
            }
        });
    }
    got
}

/// Has a picture missed since `clear_misses` come?
pub fn misses_ready(media: &Media) -> bool {
    MISSES.with(|cell| {
        cell.borrow()
            .iter()
            .any(|(reference, w, h, fit)| media.peek(reference, *w, *h, *fit).is_some())
    })
}

pub fn clear_misses() {
    MISSES.with(|cell| cell.borrow_mut().clear());
}

/// A picture `size` px square, cut to a rounded square of `radius` (half
/// the size: a circle). None while it loads.
pub fn shaped(
    media: &mut Media,
    picture: &proto::Media,
    size: f32,
    radius: f32,
    now: Instant,
) -> Option<Rc<Image>> {
    let px = size.round().max(1.0) as u32;
    let source = get(media, &picture.reference, px, px, Fit::Cover, now)?;
    let key = ShapeKey {
        reference: picture.reference.clone(),
        size: px,
        radius: radius.round() as u32,
    };
    SHAPED.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some(found) = cache.get(&key)
            && Rc::ptr_eq(&found.source, &source)
        {
            return Some(found.image.clone());
        }
        if cache.len() > 600 {
            cache.clear();
        }
        let image = Rc::new(source.icon_mask(px as i32, radius));
        cache.insert(
            key,
            Shaped {
                source,
                image: image.clone(),
            },
        );
        Some(image)
    })
}

/// A round avatar at (`x`, `y`) (its top left); a disc of `fallback` while it loads.
pub fn avatar(
    canvas: &mut Canvas,
    media: &mut Media,
    picture: Option<&proto::Media>,
    x: f32,
    y: f32,
    size: f32,
    fallback: Rgb,
    now: Instant,
) {
    let image = picture.and_then(|picture| shaped(media, picture, size, size / 2.0, now));
    match image {
        Some(image) => canvas.draw_image(&image, x.round() as i32, y.round() as i32, 1.0),
        None => canvas.fill_circle(x + size / 2.0, y + size / 2.0, size / 2.0, fallback, 1.0),
    }
}

/// The four corner masks (outside a quarter circle) for `radius`.
fn corner_masks(radius: u32) -> Rc<[Vec<u8>; 4]> {
    CORNERS.with(|cell| {
        cell.borrow_mut()
            .entry(radius)
            .or_insert_with(|| {
                let r = radius as i32;
                let mut masks: [Vec<u8>; 4] = Default::default();
                for (corner, mask) in masks.iter_mut().enumerate() {
                    *mask = vec![0; (r * r) as usize];
                    for y in 0..r {
                        for x in 0..r {
                            // Distance from the arc's centre, sampled 4x4.
                            let mut outside = 0;
                            for sy in 0..4 {
                                for sx in 0..4 {
                                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                                    let (cx, cy) = match corner {
                                        0 => (r as f32, r as f32),
                                        1 => (0.0, r as f32),
                                        2 => (r as f32, 0.0),
                                        _ => (0.0, 0.0),
                                    };
                                    if ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() > r as f32 {
                                        outside += 1;
                                    }
                                }
                            }
                            mask[(y * r + x) as usize] = (outside * 255 / 16) as u8;
                        }
                    }
                }
                Rc::new(masks)
            })
            .clone()
    })
}

/// Rounds a drawn rectangle's corners into `background`.
pub fn round_corners(canvas: &mut Canvas, rect: RectF, radius: f32, background: Rgb) {
    let r = radius.round().max(1.0) as u32;
    let masks = corner_masks(r);
    let (x0, y0) = (rect.x.round() as i32, rect.y.round() as i32);
    let (x1, y1) = (
        (rect.x + rect.w).round() as i32,
        (rect.y + rect.h).round() as i32,
    );
    let ri = r as i32;
    canvas.draw_mask(x0, y0, ri, ri, &masks[0], background, 1.0);
    canvas.draw_mask(x1 - ri, y0, ri, ri, &masks[1], background, 1.0);
    canvas.draw_mask(x0, y1 - ri, ri, ri, &masks[2], background, 1.0);
    canvas.draw_mask(x1 - ri, y1 - ri, ri, ri, &masks[3], background, 1.0);
}

/// A red count pill (a mention badge) centred at (`cx`, `cy`), with a ring of `ring`.
pub fn badge(canvas: &mut Canvas, count: u32, cx: f32, cy: f32, ring: Option<Rgb>) -> f32 {
    let label = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    let size = dp(12.0);
    let h = dp(16.0);
    let w = (text::measure(&label, Weight::Bold, size) + dp(9.0)).max(h);
    let rect = RectF {
        x: cx - w / 2.0,
        y: cy - h / 2.0,
        w,
        h,
    };
    if let Some(ring) = ring {
        canvas.fill_rrect(rect.inset(-dp(3.0)), h / 2.0 + dp(3.0), ring, 1.0);
    }
    canvas.fill_rrect(rect, h / 2.0, theme::RED, 1.0);
    text::draw_centered(
        canvas,
        &label,
        cx,
        text::centered_baseline(cy, Weight::Bold, size),
        Weight::Bold,
        size,
        theme::WHITE,
        1.0,
    );
    w
}

const SLANT: f32 = 0.21;

/// Draws `text` slanted (italic); returns its width.
#[allow(clippy::too_many_arguments)]
pub fn draw_italic(
    canvas: &mut Canvas,
    value: &str,
    x: f32,
    baseline: f32,
    weight: Weight,
    size: f32,
    color: Rgb,
    alpha: f32,
) -> f32 {
    let width = text::measure(value, weight, size);
    let key = (value.to_owned(), weight as u8, size.to_bits());
    let mask = ITALIC.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        if cache.len() > 400 {
            cache.clear();
        }
        let ascent = (size * 1.05).ceil() as i32;
        let descent = (size * 0.35).ceil() as i32;
        let h = ascent + descent;
        let w = width.ceil() as i32 + 4;
        let mut scratch = Canvas::new(w, h);
        scratch.clear(0xff00_0000);
        text::draw(
            &mut scratch,
            value,
            1.0,
            ascent as f32,
            weight,
            size,
            0xffffff,
            1.0,
        );
        let coverage: Vec<u8> = scratch.pixels.iter().map(|p| (p & 0xff) as u8).collect();
        let found = Rc::new((w, h, ascent, coverage));
        cache.insert(key, found.clone());
        found
    });
    let (w, h, ascent, coverage) = &*mask;
    let top = baseline.round() as i32 - ascent;
    for row in 0..*h {
        let shift = ((*ascent - row) as f32 * SLANT).round() as i32;
        let start = (row * w) as usize;
        canvas.draw_mask(
            x.round() as i32 - 1 + shift,
            top + row,
            *w,
            1,
            &coverage[start..start + *w as usize],
            color,
            alpha,
        );
    }
    width
}

/// An on/off switch with its right edge at `right`, centred on `cy`.
pub fn switch(canvas: &mut Canvas, on: bool, right: f32, cy: f32, enabled: bool) -> RectF {
    let (w, h) = (dp(40.0), dp(24.0));
    let rect = RectF {
        x: right - w,
        y: cy - h / 2.0,
        w,
        h,
    };
    let alpha = if enabled { 1.0 } else { 0.4 };
    canvas.fill_rrect(
        rect,
        h / 2.0,
        if on { theme::BLURPLE } else { 0x80848e },
        alpha,
    );
    let knob = h - dp(6.0);
    let kx = if on {
        rect.x + w - h / 2.0
    } else {
        rect.x + h / 2.0
    };
    canvas.fill_circle(kx, cy, knob / 2.0, theme::WHITE, alpha);
    rect
}

/// Icons the shared set does not have, drawn from paths and kept as masks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Speaker,
    Megaphone,
    Forum,
    /// A thread's curve from its channel.
    Spine,
    /// Discord's controller-ish logo, simplified: the app's mark.
    Logo,
    /// A play triangle in a dark disc.
    PlayDisc,
    Send,
    /// Two people: the member list.
    Members,
}

fn glyph_path(glyph: Glyph, s: f32) -> Path {
    let mut path = Path::new();
    let u = s / 24.0;
    match glyph {
        Glyph::Speaker => {
            path.polygon(&[
                (3.0 * u, 9.0 * u),
                (7.0 * u, 9.0 * u),
                (12.0 * u, 4.5 * u),
                (12.0 * u, 19.5 * u),
                (7.0 * u, 15.0 * u),
                (3.0 * u, 15.0 * u),
            ]);
            path.stroke_polyline(&arc_points(12.0 * u, 12.0 * u, 4.5 * u, -0.9, 0.9), 2.0 * u);
            path.stroke_polyline(
                &arc_points(12.0 * u, 12.0 * u, 8.0 * u, -0.95, 0.95),
                2.0 * u,
            );
        }
        Glyph::Megaphone => {
            path.polygon(&[
                (3.0 * u, 9.5 * u),
                (8.0 * u, 9.5 * u),
                (18.0 * u, 4.5 * u),
                (18.0 * u, 19.5 * u),
                (8.0 * u, 14.5 * u),
                (3.0 * u, 14.5 * u),
            ]);
            path.polygon(&[
                (6.0 * u, 14.5 * u),
                (9.0 * u, 14.5 * u),
                (10.5 * u, 20.0 * u),
                (7.5 * u, 20.0 * u),
            ]);
            path.stroke_segment((20.5 * u, 10.0 * u), (20.5 * u, 14.0 * u), 2.0 * u);
        }
        Glyph::Forum => {
            path.stroke_polyline(
                &[
                    (4.0 * u, 5.0 * u),
                    (16.0 * u, 5.0 * u),
                    (16.0 * u, 13.0 * u),
                    (9.0 * u, 13.0 * u),
                    (5.5 * u, 16.0 * u),
                    (5.5 * u, 13.0 * u),
                    (4.0 * u, 13.0 * u),
                    (4.0 * u, 5.0 * u),
                ],
                2.0 * u,
            );
            path.stroke_polyline(
                &[
                    (18.5 * u, 9.0 * u),
                    (20.0 * u, 9.0 * u),
                    (20.0 * u, 17.0 * u),
                    (18.5 * u, 17.0 * u),
                    (18.5 * u, 20.0 * u),
                    (15.0 * u, 17.0 * u),
                    (10.0 * u, 17.0 * u),
                    (10.0 * u, 15.5 * u),
                ],
                2.0 * u,
            );
        }
        Glyph::Spine => {
            let mut points = vec![(6.0 * u, 0.0)];
            points.extend(
                arc_points(
                    14.0 * u,
                    12.0 * u,
                    8.0 * u,
                    std::f32::consts::PI,
                    std::f32::consts::PI * 0.5,
                )
                .into_iter()
                .rev()
                .collect::<Vec<_>>(),
            );
            points.push((24.0 * u, 20.0 * u));
            path.stroke_polyline(&[(6.0 * u, 0.0), (6.0 * u, 12.0 * u)], 2.0 * u);
            path.stroke_polyline(
                &arc_points(
                    14.0 * u,
                    12.0 * u,
                    8.0 * u,
                    std::f32::consts::PI * 0.5,
                    std::f32::consts::PI,
                ),
                2.0 * u,
            );
            path.stroke_segment((14.0 * u, 20.0 * u), (22.0 * u, 20.0 * u), 2.0 * u);
        }
        Glyph::Logo => {
            // A rounded body with two eyes knocked out (drawn by the caller in the background colour).
            path.polygon(&rounded_rect_points(
                2.0 * u,
                6.0 * u,
                20.0 * u,
                13.0 * u,
                5.0 * u,
            ));
            path.polygon(&[(5.0 * u, 7.0 * u), (8.0 * u, 4.0 * u), (9.5 * u, 7.0 * u)]);
            path.polygon(&[
                (19.0 * u, 7.0 * u),
                (16.0 * u, 4.0 * u),
                (14.5 * u, 7.0 * u),
            ]);
        }
        Glyph::PlayDisc => {
            path.polygon(&[
                (9.0 * u, 7.0 * u),
                (18.0 * u, 12.0 * u),
                (9.0 * u, 17.0 * u),
            ]);
        }
        Glyph::Members => {
            path.circle(9.0 * u, 8.0 * u, 3.6 * u);
            path.polygon(&rounded_rect_points(
                2.5 * u,
                13.0 * u,
                13.0 * u,
                8.0 * u,
                3.5 * u,
            ));
            path.circle(16.5 * u, 8.5 * u, 3.0 * u);
            path.polygon(&[
                (15.5 * u, 13.2 * u),
                (19.0 * u, 13.2 * u),
                (21.5 * u, 15.5 * u),
                (21.5 * u, 20.0 * u),
                (16.8 * u, 20.0 * u),
                (16.8 * u, 15.5 * u),
            ]);
        }
        Glyph::Send => {
            path.polygon(&[
                (3.0 * u, 4.0 * u),
                (21.0 * u, 12.0 * u),
                (3.0 * u, 20.0 * u),
                (5.0 * u, 13.0 * u),
                (13.0 * u, 12.0 * u),
                (5.0 * u, 11.0 * u),
            ]);
        }
    }
    path
}

fn arc_points(cx: f32, cy: f32, r: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
    let steps = 14;
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * i as f32 / steps as f32;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn rounded_rect_points(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    use std::f32::consts::PI;
    let mut points = Vec::new();
    points.extend(arc_points(x + w - r, y + r, r, -PI / 2.0, 0.0));
    points.extend(arc_points(x + w - r, y + h - r, r, 0.0, PI / 2.0));
    points.extend(arc_points(x + r, y + h - r, r, PI / 2.0, PI));
    points.extend(arc_points(x + r, y + r, r, PI, PI * 1.5));
    points
}

/// Draws a glyph `size` px square centred on (`cx`, `cy`).
pub fn glyph(
    canvas: &mut Canvas,
    glyph: Glyph,
    cx: f32,
    cy: f32,
    size: f32,
    color: Rgb,
    alpha: f32,
) {
    let key = (glyph as u8, size.round() as u32);
    let mask = MASKS.with(|cell| {
        cell.borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let path = glyph_path(glyph, size.round());
                let (x, y, w, h, coverage) = path.rasterize().unwrap_or((0, 0, 0, 0, Vec::new()));
                Rc::new((x, y, w, h, coverage))
            })
            .clone()
    });
    let (x, y, w, h, coverage) = &*mask;
    if *w == 0 {
        return;
    }
    let left = (cx - size / 2.0).round() as i32;
    let top = (cy - size / 2.0).round() as i32;
    canvas.draw_mask(left + x, top + y, *w, *h, coverage, color, alpha);
}

/// Shortens `value` with an ellipsis to fit `width`.
pub fn fit(value: &str, weight: Weight, size: f32, width: f32) -> String {
    if width <= 0.0 {
        return String::new();
    }
    text::fit(value, weight, size, width)
}

/// "Today at 9:15 PM", "Yesterday at 9:15 PM", else "10/24/26, 9:15 PM", as Discord's mobile app writes a message's time.
pub fn message_time(ts_ms: i64) -> String {
    let tm = local(ts_ms / 1000);
    let now = local(super::model::now_ms() / 1000);
    let clock = clock(&tm);
    let day = |t: &libc::tm| (t.tm_year, t.tm_yday);
    if day(&tm) == day(&now) {
        format!("Today at {clock}")
    } else if is_yesterday(&tm, &now) {
        format!("Yesterday at {clock}")
    } else {
        format!(
            "{}/{}/{:02}, {clock}",
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_year % 100
        )
    }
}

fn is_yesterday(tm: &libc::tm, now: &libc::tm) -> bool {
    (tm.tm_year == now.tm_year && tm.tm_yday + 1 == now.tm_yday)
        || (tm.tm_year + 1 == now.tm_year
            && now.tm_yday == 0
            && tm.tm_mon == 11
            && tm.tm_mday == 31)
}

pub fn clock(tm: &libc::tm) -> String {
    if crate::control::clock_24h() {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        let hour = match tm.tm_hour % 12 {
            0 => 12,
            h => h,
        };
        format!(
            "{hour}:{:02} {}",
            tm.tm_min,
            if tm.tm_hour < 12 { "AM" } else { "PM" }
        )
    }
}

/// A day divider's words: "October 24, 2026".
pub fn day_label(ts_ms: i64) -> String {
    let tm = local(ts_ms / 1000);
    format!(
        "{} {}, {}",
        MONTHS[tm.tm_mon.clamp(0, 11) as usize],
        tm.tm_mday,
        tm.tm_year + 1900
    )
}

pub fn same_day(a_ms: i64, b_ms: i64) -> bool {
    let (a, b) = (local(a_ms / 1000), local(b_ms / 1000));
    a.tm_year == b.tm_year && a.tm_yday == b.tm_yday
}

/// The DM list's time: "9:15 PM" today, else "Yesterday", "Oct 24".
pub fn short_time(ts_ms: i64) -> String {
    if ts_ms <= 0 {
        return String::new();
    }
    let tm = local(ts_ms / 1000);
    let now = local(super::model::now_ms() / 1000);
    if tm.tm_year == now.tm_year && tm.tm_yday == now.tm_yday {
        clock(&tm)
    } else if is_yesterday(&tm, &now) {
        "Yesterday".to_owned()
    } else if tm.tm_year == now.tm_year {
        format!(
            "{} {}",
            &MONTHS[tm.tm_mon.clamp(0, 11) as usize][..3],
            tm.tm_mday
        )
    } else {
        format!("{}/{}/{:02}", tm.tm_mon + 1, tm.tm_mday, tm.tm_year % 100)
    }
}

/// Discord's `<t:…:S>` timestamps.
pub fn timestamp(seconds: i64, style: char) -> String {
    let tm = local(seconds);
    let month = MONTHS[tm.tm_mon.clamp(0, 11) as usize];
    let date = format!("{month} {}, {}", tm.tm_mday, tm.tm_year + 1900);
    match style {
        't' => clock(&tm),
        'T' => format!("{} ({:02}s)", clock(&tm), tm.tm_sec),
        'd' => format!("{}/{}/{}", tm.tm_mon + 1, tm.tm_mday, tm.tm_year + 1900),
        'D' => date,
        'F' => format!(
            "{}, {date} {}",
            WEEKDAYS[tm.tm_wday.clamp(0, 6) as usize],
            clock(&tm)
        ),
        'R' => relative(seconds),
        _ => format!("{date} {}", clock(&tm)),
    }
}

fn relative(seconds: i64) -> String {
    let delta = seconds - super::model::now_ms() / 1000;
    let past = delta < 0;
    let abs = delta.abs();
    let (n, unit) = match abs {
        0..=44 => {
            return if past {
                "a few seconds ago".into()
            } else {
                "in a few seconds".into()
            };
        }
        45..=3599 => ((abs + 30) / 60, "minute"),
        3600..=86_399 => ((abs + 1800) / 3600, "hour"),
        86_400..=2_591_999 => ((abs + 43_200) / 86_400, "day"),
        2_592_000..=31_535_999 => ((abs + 1_296_000) / 2_592_000, "month"),
        _ => ((abs + 15_768_000) / 31_536_000, "year"),
    };
    let plural = if n == 1 { "" } else { "s" };
    if past {
        format!("{n} {unit}{plural} ago")
    } else {
        format!("in {n} {unit}{plural}")
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

pub fn local(t: i64) -> libc::tm {
    // SAFETY: localtime_r writes into a zeroed tm it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = t as _;
        libc::localtime_r(&time, &mut tm);
        tm
    }
}

/// "1.2 MB".
pub fn file_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} bytes"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.2} MB", bytes as f64 / 1_048_576.0),
    }
}

/// `fg` laid over `background` at `amount` (a tint).
pub fn shade(fg: Rgb, background: Rgb, amount: f32) -> Rgb {
    crate::gfx::mix(background, fg, amount)
}

/// Drawing a scrolled list in part: while only its offset changed since
/// the last frame, its rows are moved and only the band that came into
/// view is drawn (`social::shell::Damage`).
#[derive(Default)]
pub struct Partial {
    last: Option<(i32, u64)>,
    /// Something besides the offset changed: the next frame is whole.
    pub dirty: bool,
}

impl Partial {
    /// How far to move the rows `y0..y1` for a list now at `offset`
    /// (Some(0): nothing changed), or None to draw the frame whole.
    pub fn plan(
        &mut self,
        offset: i32,
        key: u64,
        full: bool,
        media: &Media,
        span: i32,
    ) -> Option<i32> {
        let (last, last_key) = self.last?;
        if full || self.dirty || last_key != key || misses_ready(media) {
            return None;
        }
        let dy = last - offset;
        (dy.abs() < span / 2).then_some(dy)
    }

    pub fn drawn(&mut self, offset: i32, key: u64, whole: bool) {
        self.last = Some((offset, key));
        if whole {
            self.dirty = false;
        }
    }
}

/// Moves rows `y0..y1` by `dy`, then draws the band that came into view
/// with `draw` (clipped to it); the damage for the shell.
pub fn scroll_rows(
    canvas: &mut Canvas,
    y0: i32,
    y1: i32,
    dy: i32,
    background: Rgb,
    draw: impl FnOnce(&mut Canvas),
) -> crate::social::shell::Damage {
    if dy == 0 {
        return crate::social::shell::Damage::default();
    }
    canvas.move_rows(y0, y1, dy);
    let band = if dy > 0 { (y0, y0 + dy) } else { (y1 + dy, y1) };
    canvas.set_clip(0, band.0, canvas.width, band.1);
    canvas.fill_rect(0, band.0, canvas.width, band.1 - band.0, background, 1.0);
    draw(canvas);
    canvas.reset_clip();
    crate::social::shell::Damage {
        moved: Some((y0, y1, dy)),
        rows: vec![band],
    }
}
