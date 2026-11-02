//! Instagram's look, drawn: its dark palette and metrics (on a 1080 px wide
//! window, 2.625 px to Instagram's point), avatars cut round with their
//! story rings, Instagram's line icons, pictures copied straight in when
//! they are opaque, bubbles with per-corner radii, text wrapped into lines
//! of styled runs, and counts and ages written as Instagram writes them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use crate::gfx::{Canvas, Image, Path, RectF, Rgb};
use crate::social::media::{Fit, Media};
use crate::text::{self, Weight};

// ----- Palette (Instagram, dark) ----------------------------------------

pub const BG: Rgb = 0x000000;
pub const TEXT: Rgb = 0xf5f5f5;
pub const SECONDARY: Rgb = 0xa8a8a8;
pub const TERTIARY: Rgb = 0x737373;
pub const SEPARATOR: Rgb = 0x262626;
pub const ELEVATED: Rgb = 0x262626;
pub const FIELD: Rgb = 0x262626;
pub const BUTTON: Rgb = 0x363636;
pub const BLUE: Rgb = 0x0095f6;
pub const LINK: Rgb = 0xe0f1ff;
pub const RED: Rgb = 0xff3040;
pub const GREEN: Rgb = 0x1cd14f;
pub const PLACEHOLDER: Rgb = 0x1c1c1c;
/// The story ring, bottom-left to top-right.
pub const RING: [Rgb; 5] = [0xfeda75, 0xfa7e1e, 0xd62976, 0x962fbf, 0x4f5bd5];

// ----- Metrics ------------------------------------------------------------

/// Instagram's point on this screen.
pub const DP: f32 = 2.625;
/// Where the app's own content starts: Moon puts the window under its
/// status bar (y 124 on the screen), so at the window's top.
pub const STATUS: f32 = 0.0;
pub const TOP_BAR: f32 = 147.0;
pub const TAB_BAR: f32 = 131.0;
/// Under the tab bar: Moon's home gesture (taps still come).
pub const SAFE_BOTTOM: f32 = 94.0;
pub const MARGIN: f32 = 42.0;

pub const fn dp(v: f32) -> f32 {
    v * DP
}

// ----- Type ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Font {
    pub size: f32,
    pub weight: Weight,
}

pub const BODY: Font = Font {
    size: 37.0,
    weight: Weight::Regular,
};
pub const BODY_BOLD: Font = Font {
    size: 37.0,
    weight: Weight::SemiBold,
};
pub const SMALL: Font = Font {
    size: 33.0,
    weight: Weight::Regular,
};
pub const SMALL_BOLD: Font = Font {
    size: 33.0,
    weight: Weight::SemiBold,
};
pub const TINY: Font = Font {
    size: 29.0,
    weight: Weight::Regular,
};
pub const TITLE: Font = Font {
    size: 50.0,
    weight: Weight::Bold,
};
pub const HEADLINE: Font = Font {
    size: 44.0,
    weight: Weight::SemiBold,
};
pub const MESSAGE: Font = Font {
    size: 40.0,
    weight: Weight::Regular,
};

pub fn draw_text(
    canvas: &mut Canvas,
    s: &str,
    x: f32,
    baseline: f32,
    font: Font,
    color: Rgb,
) -> f32 {
    text::draw(canvas, s, x, baseline, font.weight, font.size, color, 1.0)
}

pub fn draw_text_right(
    canvas: &mut Canvas,
    s: &str,
    right: f32,
    baseline: f32,
    font: Font,
    color: Rgb,
) {
    text::draw_right(
        canvas,
        s,
        right,
        baseline,
        font.weight,
        font.size,
        color,
        1.0,
    );
}

pub fn draw_text_centered(
    canvas: &mut Canvas,
    s: &str,
    cx: f32,
    baseline: f32,
    font: Font,
    color: Rgb,
) {
    text::draw_centered(canvas, s, cx, baseline, font.weight, font.size, color, 1.0);
}

pub fn measure(s: &str, font: Font) -> f32 {
    text::measure(s, font.weight, font.size)
}

pub fn fit(s: &str, font: Font, width: f32) -> String {
    text::fit(s, font.weight, font.size, width)
}

/// The baseline that centres capitals on `cy`.
pub fn baseline(cy: f32, font: Font) -> f32 {
    text::centered_baseline(cy, font.weight, font.size)
}

/// One styled run on a line.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub font: Font,
    pub color: Rgb,
}

pub type Line = Vec<Run>;

/// Wraps styled words (`(word, font, colour)`, spaces between them) into
/// lines at most `width` wide; a word wider than a line is cut with an
/// ellipsis. `\n` inside a word breaks the line; a word starting with
/// `GLUE` follows the one before without a space (a comma after a bold name).
pub const GLUE: char = '\u{2060}';

pub fn wrap_runs(words: &[(String, Font, Rgb)], width: f32, max_lines: usize) -> (Vec<Line>, bool) {
    let mut lines: Vec<Line> = Vec::new();
    let mut line: Line = Vec::new();
    let mut used = 0.0;
    let mut truncated = false;
    let push_word =
        |line: &mut Line, used: &mut f32, word: &str, font: Font, color: Rgb, space: bool| {
            let piece = if space {
                format!(" {word}")
            } else {
                word.to_owned()
            };
            *used += measure(&piece, font);
            match line.last_mut() {
                Some(run) if run.font == font && run.color == color => run.text.push_str(&piece),
                _ => line.push(Run {
                    text: piece,
                    font,
                    color,
                }),
            }
        };
    'words: for (word, font, color) in words {
        let parts: Vec<&str> = word.split('\n').collect();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                lines.push(std::mem::take(&mut line));
                used = 0.0;
                if lines.len() >= max_lines {
                    truncated = true;
                    break 'words;
                }
            }
            let (part, glued) = match part.strip_prefix(GLUE) {
                Some(rest) => (rest, index == 0),
                None => (*part, false),
            };
            if part.is_empty() {
                continue;
            }
            let space = !line.is_empty() && !glued;
            let space_w = if space { measure(" ", *font) } else { 0.0 };
            let w = measure(part, *font);
            if used + space_w + w <= width || line.is_empty() {
                if w > width && line.is_empty() {
                    let cut = fit(part, *font, width);
                    push_word(&mut line, &mut used, &cut, *font, *color, false);
                } else {
                    push_word(&mut line, &mut used, part, *font, *color, space);
                }
            } else {
                lines.push(std::mem::take(&mut line));
                used = 0.0;
                if lines.len() >= max_lines {
                    truncated = true;
                    break 'words;
                }
                let cut = if w > width {
                    fit(part, *font, width)
                } else {
                    part.to_owned()
                };
                push_word(&mut line, &mut used, &cut, *font, *color, false);
            }
        }
    }
    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    } else if !line.is_empty() {
        truncated = true;
    }
    (lines, truncated)
}

/// Plain text in one font and colour, wrapped.
pub fn wrap_plain(
    s: &str,
    font: Font,
    color: Rgb,
    width: f32,
    max_lines: usize,
) -> (Vec<Line>, bool) {
    let words: Vec<(String, Font, Rgb)> = s
        .split(' ')
        .filter(|word| !word.is_empty())
        .map(|word| (word.to_owned(), font, color))
        .collect();
    wrap_runs(&words, width, max_lines)
}

/// Words of a caption or comment: `@mentions` and `#tags` in the link colour.
pub fn rich_words(s: &str, font: Font, color: Rgb) -> Vec<(String, Font, Rgb)> {
    s.split(' ')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let tinted = (word.starts_with('@') || word.starts_with('#')) && word.len() > 1;
            (word.to_owned(), font, if tinted { LINK } else { color })
        })
        .collect()
}

pub fn line_width(line: &Line) -> f32 {
    line.iter().map(|run| measure(&run.text, run.font)).sum()
}

pub fn draw_line(canvas: &mut Canvas, line: &Line, x: f32, baseline: f32) -> f32 {
    let mut pen = x;
    for run in line {
        pen += draw_text(canvas, &run.text, pen, baseline, run.font, run.color);
    }
    pen - x
}

// ----- Numbers and times -----------------------------------------------------

/// 1,234 below ten thousand; 12.3K, 1.2M above (Instagram's counts).
pub fn count(n: u64) -> String {
    if n < 10_000 {
        let s = n.to_string();
        if s.len() > 3 {
            format!("{},{}", &s[..s.len() - 3], &s[s.len() - 3..])
        } else {
            s
        }
    } else if n < 1_000_000 {
        let k = n as f64 / 1000.0;
        if k < 100.0 {
            format!("{}K", trim_decimal(format!("{k:.1}")))
        } else {
            format!("{}K", k.floor())
        }
    } else {
        let m = n as f64 / 1_000_000.0;
        if m < 100.0 {
            format!("{}M", trim_decimal(format!("{m:.1}")))
        } else {
            format!("{}M", m.floor())
        }
    }
}

fn trim_decimal(s: String) -> String {
    s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `now`, `5m`, `3h`, `2d`, `4w` (rows, comments).
pub fn age_short(ts: i64, now: i64) -> String {
    let s = ((now - ts) / 1000).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=604_799 => format!("{}d", s / 86_400),
        _ => format!("{}w", s / 604_800),
    }
}

/// `2 hours ago`, `3 days ago`, `October 12` (posts).
pub fn age_long(ts: i64, now: i64) -> String {
    let s = ((now - ts) / 1000).max(0);
    let plural = |n: i64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match s {
        0..=59 => "Just now".into(),
        60..=3599 => plural(s / 60, "minute"),
        3600..=86_399 => plural(s / 3600, "hour"),
        86_400..=604_799 => plural(s / 86_400, "day"),
        _ => {
            let tm = local(ts / 1000);
            let month = MONTHS[tm.tm_mon.clamp(0, 11) as usize];
            if local(now / 1000).tm_year == tm.tm_year {
                format!("{month} {}", tm.tm_mday)
            } else {
                format!("{month} {}, {}", tm.tm_mday, tm.tm_year + 1900)
            }
        }
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
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

pub fn local(t: i64) -> libc::tm {
    // SAFETY: localtime_r writes into a zeroed tm it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        #[allow(deprecated)]
        let time = t as libc::time_t;
        libc::localtime_r(&time, &mut tm);
        tm
    }
}

pub fn clock(ts: i64) -> String {
    let tm = local(ts / 1000);
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

/// A chat's time separator: `2:14 PM`, `Yesterday 9:03 AM`, `Tue 5:40 PM`, `Oct 3, 11:20 AM`.
pub fn separator_time(ts: i64, now: i64) -> String {
    let tm = local(ts / 1000);
    let today = local(now / 1000);
    let days = (now / 86_400_000) - (ts / 86_400_000);
    if tm.tm_yday == today.tm_yday && tm.tm_year == today.tm_year {
        clock(ts)
    } else if days <= 1 && (today.tm_yday - tm.tm_yday == 1 || today.tm_yday == 0) {
        format!("Yesterday {}", clock(ts))
    } else if days < 7 {
        format!("{} {}", DAYS[tm.tm_wday.clamp(0, 6) as usize], clock(ts))
    } else {
        format!(
            "{} {}, {}",
            &MONTHS[tm.tm_mon.clamp(0, 11) as usize][..3],
            tm.tm_mday,
            clock(ts)
        )
    }
}

// ----- Pictures ------------------------------------------------------------

/// A picture wanted for a frame but not ready: asked again every frame, and
/// a frame that only scrolled is redrawn whole once one of them arrives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Want {
    pub reference: String,
    pub w: u32,
    pub h: u32,
    pub fit: Fit,
    /// The window rows it is drawn in (they move with a scroll).
    pub rows: (i32, i32),
}

/// What drawing needs beside the canvas.
pub struct Paint<'a> {
    pub media: &'a mut Media,
    pub now: Instant,
    pub missing: &'a mut Vec<Want>,
    pub cache: &'a Cache,
}

impl Paint<'_> {
    /// The picture, or None (and it is asked for); `y` is the window row
    /// its top is drawn at.
    pub fn picture(
        &mut self,
        reference: &str,
        w: u32,
        h: u32,
        fit: Fit,
        y: f32,
    ) -> Option<Rc<Image>> {
        if reference.is_empty() || w == 0 || h == 0 {
            return None;
        }
        let image = self.media.get(reference, w, h, fit, self.now);
        if image.is_none() {
            self.missing.push(Want {
                reference: reference.to_owned(),
                w,
                h,
                fit,
                rows: (y.floor() as i32 - 1, (y + h as f32).ceil() as i32 + 1),
            });
        }
        image
    }

    /// Asks for a picture drawn soon (no frame waits for it).
    pub fn prefetch(&mut self, reference: &str, w: u32, h: u32, fit: Fit) {
        if !reference.is_empty() && w > 0 && h > 0 {
            let _ = self.media.get(reference, w, h, fit, self.now);
        }
    }
}

/// Things made once and drawn often: round-cut avatars, rings, icons,
/// whether a picture is opaque.
#[derive(Default)]
pub struct Cache {
    rounds: RefCell<HashMap<(usize, i32), Rounded>>,
    circles: RefCell<HashMap<(i32, i32), Coverage>>,
    rings: RefCell<HashMap<(i32, i32, u8), Rc<Image>>>,
    icons: RefCell<HashMap<(Icon, i32, bool), Rc<Mask>>>,
    opaque: RefCell<HashMap<usize, (Rc<Image>, bool)>>,
}

/// A picture and its round cut.
type Rounded = (Rc<Image>, Rc<Image>);
/// A circle's coverage per pixel, 0..=256.
type Coverage = Rc<Vec<u32>>;

pub struct Mask {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub data: Vec<u8>,
}

impl Cache {
    fn is_opaque(&self, image: &Rc<Image>) -> bool {
        let key = Rc::as_ptr(image) as usize;
        let mut map = self.opaque.borrow_mut();
        if let Some((_, opaque)) = map.get(&key) {
            return *opaque;
        }
        if map.len() > 600 {
            map.clear();
        }
        let opaque = image.pixels.iter().all(|p| p >> 24 == 0xff);
        map.insert(key, (image.clone(), opaque));
        opaque
    }

    /// `image` cut to a circle of `d` px (it is `d`x`d`, Fit::Cover). The
    /// source is kept with it, so its address can't be reused by another.
    fn round(&self, image: &Rc<Image>, d: i32) -> Rc<Image> {
        let key = (Rc::as_ptr(image) as usize, d);
        if let Some((_, found)) = self.rounds.borrow().get(&key) {
            return found.clone();
        }
        let mask = self.circle(image.width, image.height);
        let mut out = (**image).clone();
        for (pixel, &m) in out.pixels.iter_mut().zip(mask.iter()) {
            match m {
                256 => {}
                0 => *pixel = 0,
                m => {
                    let p = *pixel;
                    let scale = |c: u32| (c * m) >> 8;
                    *pixel = scale(p >> 24) << 24
                        | scale(p >> 16 & 0xff) << 16
                        | scale(p >> 8 & 0xff) << 8
                        | scale(p & 0xff);
                }
            }
        }
        let out = Rc::new(out);
        let mut map = self.rounds.borrow_mut();
        if map.len() > 400 {
            map.clear();
        }
        map.insert(key, (image.clone(), out.clone()));
        out
    }

    /// A circle's coverage (0..=256) per pixel of a `w`x`h` box, made once per size.
    fn circle(&self, w: i32, h: i32) -> Rc<Vec<u32>> {
        if let Some(found) = self.circles.borrow().get(&(w, h)) {
            return found.clone();
        }
        let r = w.min(h) as f32 / 2.0;
        let mut mask = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let dx = x as f32 + 0.5 - w as f32 / 2.0;
                let dy = y as f32 + 0.5 - h as f32 / 2.0;
                let coverage = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
                mask.push((coverage * 256.0).round() as u32);
            }
        }
        let mask = Rc::new(mask);
        self.circles.borrow_mut().insert((w, h), mask.clone());
        mask
    }

    /// A story ring of outer diameter `d` and width `w`: kind 0 the
    /// gradient, 1 close friends' green, 2 seen grey.
    fn ring(&self, d: i32, w: i32, kind: u8) -> Rc<Image> {
        if let Some(found) = self.rings.borrow().get(&(d, w, kind)) {
            return found.clone();
        }
        let mut pixels = vec![0_u32; (d * d) as usize];
        let r = d as f32 / 2.0;
        let inner = r - w as f32;
        for y in 0..d {
            for x in 0..d {
                let dx = x as f32 + 0.5 - r;
                let dy = y as f32 + 0.5 - r;
                let dist = (dx * dx + dy * dy).sqrt();
                let coverage =
                    ((r - dist + 0.5).clamp(0.0, 1.0)) * ((dist - inner + 0.5).clamp(0.0, 1.0));
                if coverage <= 0.0 {
                    continue;
                }
                let color = match kind {
                    0 => {
                        let t = ((x as f32 / d as f32) + (1.0 - y as f32 / d as f32)) / 2.0;
                        gradient(&RING, t)
                    }
                    1 => GREEN,
                    _ => 0x3a3a3a,
                };
                let a = (coverage * 255.0) as u32;
                let pre = |c: u32| c * a / 255;
                pixels[(y * d + x) as usize] = a << 24
                    | pre(color >> 16 & 0xff) << 16
                    | pre(color >> 8 & 0xff) << 8
                    | pre(color & 0xff);
            }
        }
        let image = Rc::new(Image {
            width: d,
            height: d,
            pixels,
        });
        self.rings.borrow_mut().insert((d, w, kind), image.clone());
        image
    }

    pub fn icon(&self, icon: Icon, size: i32, filled: bool) -> Rc<Mask> {
        if let Some(found) = self.icons.borrow().get(&(icon, size, filled)) {
            return found.clone();
        }
        let mut path = Path::new();
        icon_path(&mut path, icon, size as f32, filled);
        let mask = match path.rasterize() {
            Some((x, y, w, h, data)) => Mask { x, y, w, h, data },
            None => Mask {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
                data: Vec::new(),
            },
        };
        let mask = Rc::new(mask);
        self.icons
            .borrow_mut()
            .insert((icon, size, filled), mask.clone());
        mask
    }
}

pub fn gradient(stops: &[Rgb], t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (t.floor() as usize).min(stops.len() - 2);
    crate::gfx::mix(stops[i], stops[i + 1], t - i as f32)
}

/// Copies an opaque picture in (a row at a time), else composites it.
pub fn blit(canvas: &mut Canvas, cache: &Cache, image: &Rc<Image>, x: i32, y: i32) {
    if !cache.is_opaque(image) {
        canvas.draw_image(image, x, y, 1.0);
        return;
    }
    let (cx0, cy0, cx1, cy1) = canvas.clip();
    let x0 = x.max(cx0);
    let x1 = (x + image.width).min(cx1);
    let y0 = y.max(cy0);
    let y1 = (y + image.height).min(cy1);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let width = canvas.width;
    for row in y0..y1 {
        let src = ((row - y) * image.width + (x0 - x)) as usize;
        let dst = (row * width + x0) as usize;
        let n = (x1 - x0) as usize;
        canvas.pixels[dst..dst + n].copy_from_slice(&image.pixels[src..src + n]);
    }
}

/// A picture in `rect` (Fit::Cover), or a placeholder while it loads.
pub fn picture(
    canvas: &mut Canvas,
    p: &mut Paint,
    reference: &str,
    rect: RectF,
    placeholder: Rgb,
) -> bool {
    let (w, h) = (rect.w.round() as u32, rect.h.round() as u32);
    match p.picture(reference, w, h, Fit::Cover, rect.y) {
        Some(image) => {
            blit(
                canvas,
                p.cache,
                &image,
                rect.x.round() as i32,
                rect.y.round() as i32,
            );
            true
        }
        None => {
            canvas.fill_rect(
                rect.x.round() as i32,
                rect.y.round() as i32,
                w as i32,
                h as i32,
                placeholder,
                1.0,
            );
            false
        }
    }
}

/// Which ring an avatar wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ring {
    None,
    Unseen,
    CloseFriends,
    Seen,
}

impl Ring {
    pub fn from_story(story: Option<&str>) -> Self {
        match story {
            Some("unseen") => Self::Unseen,
            Some("close_friends") => Self::CloseFriends,
            Some("seen") => Self::Seen,
            _ => Self::None,
        }
    }
}

/// A round avatar of diameter `d` centred on (cx, cy); with a ring, the
/// ring is `d` and the picture sits inside it with a gap.
pub fn avatar(
    canvas: &mut Canvas,
    p: &mut Paint,
    reference: Option<&str>,
    cx: f32,
    cy: f32,
    d: f32,
    ring: Ring,
) {
    let (inner, ring_w) = match ring {
        Ring::None => (d, 0.0),
        Ring::Seen => (d - 2.0 * (dp(1.0) + dp(3.0)), dp(1.0)),
        _ => (d - 2.0 * (dp(2.5) + dp(3.0)), dp(2.5)),
    };
    if ring != Ring::None {
        let size = d.round() as i32;
        let kind = match ring {
            Ring::Unseen => 0,
            Ring::CloseFriends => 1,
            _ => 2,
        };
        let image = p.cache.ring(size, ring_w.round() as i32, kind);
        canvas.draw_image(
            &image,
            (cx - size as f32 / 2.0).round() as i32,
            (cy - size as f32 / 2.0).round() as i32,
            1.0,
        );
    }
    let size = inner.round() as i32;
    let x = (cx - size as f32 / 2.0).round() as i32;
    let y = (cy - size as f32 / 2.0).round() as i32;
    let image =
        reference.and_then(|r| p.picture(r, size as u32, size as u32, Fit::Cover, y as f32));
    match image {
        Some(image) => {
            let round = p.cache.round(&image, size);
            canvas.draw_image(&round, x, y, 1.0);
        }
        None => canvas.fill_circle(cx, cy, size as f32 / 2.0, 0x2a2a2a, 1.0),
    }
}

/// A rounded rectangle with its own radius per corner (top-left, top-right,
/// bottom-right, bottom-left), filled with `color` or, per row, `shade(y)`.
pub fn fill_rrect4(canvas: &mut Canvas, rect: RectF, radii: [f32; 4], shade: &dyn Fn(i32) -> Rgb) {
    let (cx0, cy0, cx1, cy1) = canvas.clip();
    let y0 = (rect.y.floor() as i32).max(cy0);
    let y1 = ((rect.y + rect.h).ceil() as i32).min(cy1);
    let left = rect.x;
    let right = rect.x + rect.w;
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        let color = shade(y);
        // How far in each side is at this row (corner arcs).
        let inset = |r: f32, top: bool| -> f32 {
            if r <= 0.0 {
                return 0.0;
            }
            let cy = if top { rect.y + r } else { rect.y + rect.h - r };
            let dy = if top { cy - py } else { py - cy };
            if dy <= 0.0 {
                0.0
            } else if dy >= r {
                r
            } else {
                r - (r * r - dy * dy).sqrt()
            }
        };
        let top_half = py < rect.y + rect.h / 2.0;
        let (rl, rr) = if top_half {
            (radii[0], radii[1])
        } else {
            (radii[3], radii[2])
        };
        let l = left + inset(rl, top_half);
        let r = right - inset(rr, top_half);
        // Vertical coverage at the top and bottom rows.
        let vcov = ((py + 0.5).min(rect.y + rect.h) - (py - 0.5).max(rect.y)).clamp(0.0, 1.0);
        if r <= l || vcov <= 0.0 {
            continue;
        }
        let xa = (l.floor() as i32).max(cx0);
        let xb = (r.ceil() as i32).min(cx1);
        let row = (y * canvas.width) as usize;
        for x in xa..xb {
            let px0 = x as f32;
            let cov = ((px0 + 1.0).min(r) - px0.max(l)).clamp(0.0, 1.0) * vcov;
            if cov <= 0.0 {
                continue;
            }
            let i = row + x as usize;
            canvas.pixels[i] = crate::gfx::blend(canvas.pixels[i], color, (cov * 256.0) as u32);
        }
    }
}

/// A rounded outline `width` thick over the background `bg`, drawn only
/// in the clip's rows (a band frame pays for its own rows).
pub fn outline(canvas: &mut Canvas, rect: RectF, radius: f32, width: f32, color: Rgb, bg: Rgb) {
    let r = radius.min(rect.w / 2.0).min(rect.h / 2.0);
    fill_rrect4(canvas, rect, [r; 4], &|_| color);
    let inner = RectF::new(
        rect.x + width,
        rect.y + width,
        rect.w - 2.0 * width,
        rect.h - 2.0 * width,
    );
    let ir = (r - width).max(0.0);
    fill_rrect4(canvas, inner, [ir; 4], &|_| bg);
}

/// Paints `bg` over the outside of a rounded corner box: rounds a picture
/// drawn square.
pub fn round_corners(canvas: &mut Canvas, rect: RectF, radius: f32, bg: Rgb) {
    let r = radius.min(rect.w / 2.0).min(rect.h / 2.0);
    let corners = [
        (rect.x, rect.y, rect.x + r, rect.y + r),
        (rect.x + rect.w - r, rect.y, rect.x + rect.w - r, rect.y + r),
        (rect.x, rect.y + rect.h - r, rect.x + r, rect.y + rect.h - r),
        (
            rect.x + rect.w - r,
            rect.y + rect.h - r,
            rect.x + rect.w - r,
            rect.y + rect.h - r,
        ),
    ];
    let (cx0, cy0, cx1, cy1) = canvas.clip();
    for (bx, by, ox, oy) in corners {
        let x0 = (bx.floor() as i32).max(cx0);
        let x1 = ((bx + r).ceil() as i32).min(cx1);
        let y0 = (by.floor() as i32).max(cy0);
        let y1 = ((by + r).ceil() as i32).min(cy1);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - ox;
                let dy = y as f32 + 0.5 - oy;
                let d = (dx * dx + dy * dy).sqrt() - r;
                let outside = (d + 0.5).clamp(0.0, 1.0);
                if outside > 0.0 {
                    let i = (y * canvas.width + x) as usize;
                    canvas.pixels[i] =
                        crate::gfx::blend(canvas.pixels[i], bg, (outside * 256.0) as u32);
                }
            }
        }
    }
}

// ----- Icons ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Home,
    Search,
    Direct,
    Heart,
    Comment,
    Share,
    Bookmark,
    More,
    Back,
    Close,
    Camera,
    Plus,
    Grid,
    Tagged,
    Carousel,
    Play,
    ChevronDown,
    ChevronRight,
    Compose,
    Menu,
    Lock,
    Check,
    Pin,
    Mic,
    Photo,
    Phone,
    Video,
    Hashtag,
}

/// Draws an icon `size` px square with its top-left at (x, y).
#[allow(clippy::too_many_arguments)]
pub fn icon(
    canvas: &mut Canvas,
    cache: &Cache,
    icon: Icon,
    x: f32,
    y: f32,
    size: f32,
    filled: bool,
    color: Rgb,
) {
    let mask = cache.icon(icon, size.round() as i32, filled);
    if mask.w > 0 {
        canvas.draw_mask(
            x.round() as i32 + mask.x,
            y.round() as i32 + mask.y,
            mask.w,
            mask.h,
            &mask.data,
            color,
            1.0,
        );
    }
}

fn arc_points(cx: f32, cy: f32, r: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
    let steps = (((to - from).abs() * r) / 3.0).ceil().clamp(6.0, 120.0) as usize;
    (0..=steps)
        .map(|n| {
            let a = from + (to - from) * n as f32 / steps as f32;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn heart_points(s: f32) -> Vec<(f32, f32)> {
    // A heart in a s x s box, from the classic parametric curve.
    let mut points = Vec::new();
    for n in 0..=96 {
        let t = std::f32::consts::TAU * n as f32 / 96.0;
        let x = 16.0 * t.sin().powi(3);
        let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
        points.push((s / 2.0 + x * s / 36.0, s * 0.47 - y * s / 36.0));
    }
    points
}

fn icon_path(path: &mut Path, icon: Icon, s: f32, filled: bool) {
    let w = (s / 12.0).max(2.0);
    let c = s / 2.0;
    use std::f32::consts::{FRAC_PI_2, PI};
    match icon {
        Icon::Home => {
            let pts = [
                (s * 0.14, s * 0.42),
                (c, s * 0.1),
                (s * 0.86, s * 0.42),
                (s * 0.86, s * 0.88),
                (s * 0.14, s * 0.88),
            ];
            if filled {
                path.polygon(&pts);
            } else {
                let mut closed = pts.to_vec();
                closed.push(pts[0]);
                path.stroke_polyline(&closed, w);
                path.stroke_polyline(
                    &[
                        (s * 0.4, s * 0.88),
                        (s * 0.4, s * 0.62),
                        (s * 0.6, s * 0.62),
                        (s * 0.6, s * 0.88),
                    ],
                    w,
                );
            }
        }
        Icon::Search => {
            let r = s * 0.3;
            let cx = s * 0.43;
            let cy = s * 0.43;
            path.stroke_polyline(
                &arc_points(cx, cy, r, 0.0, PI * 2.0),
                if filled { w * 1.6 } else { w },
            );
            path.stroke_segment(
                (cx + r * 0.72, cy + r * 0.72),
                (s * 0.88, s * 0.88),
                if filled { w * 1.6 } else { w },
            );
        }
        Icon::Direct | Icon::Share => {
            let a = (s * 0.1, s * 0.14);
            let b = (s * 0.9, s * 0.14);
            let tip = (s * 0.52, s * 0.9);
            let mid = (s * 0.42, s * 0.5);
            if filled {
                path.polygon(&[a, b, tip, mid]);
            } else {
                path.stroke_polyline(&[a, b, tip, mid, a], w);
                path.stroke_segment(mid, b, w);
            }
        }
        Icon::Heart => {
            let pts = heart_points(s);
            if filled {
                path.polygon(&pts);
            } else {
                path.stroke_polyline(&pts, w);
            }
        }
        Icon::Comment => {
            let r = s * 0.4;
            let mut pts = arc_points(c, c, r, PI * 0.75, PI * 0.75 + PI * 2.0 - 0.5);
            pts.push((s * 0.12, s * 0.9));
            pts.push(pts[0]);
            path.stroke_polyline(&pts, w);
        }
        Icon::Bookmark => {
            let pts = [
                (s * 0.2, s * 0.1),
                (s * 0.8, s * 0.1),
                (s * 0.8, s * 0.9),
                (c, s * 0.62),
                (s * 0.2, s * 0.9),
            ];
            if filled {
                path.polygon(&pts);
            } else {
                let mut closed = pts.to_vec();
                closed.push(pts[0]);
                path.stroke_polyline(&closed, w);
            }
        }
        Icon::More => {
            for x in [s * 0.2, c, s * 0.8] {
                path.circle(x, c, w * 0.9);
            }
        }
        Icon::Back => path.stroke_polyline(
            &[(s * 0.62, s * 0.14), (s * 0.3, c), (s * 0.62, s * 0.86)],
            w,
        ),
        Icon::Close => {
            path.stroke_segment((s * 0.18, s * 0.18), (s * 0.82, s * 0.82), w);
            path.stroke_segment((s * 0.82, s * 0.18), (s * 0.18, s * 0.82), w);
        }
        Icon::Camera => {
            path.stroke_polyline(
                &[
                    (s * 0.1, s * 0.3),
                    (s * 0.32, s * 0.3),
                    (s * 0.4, s * 0.16),
                    (s * 0.6, s * 0.16),
                    (s * 0.68, s * 0.3),
                    (s * 0.9, s * 0.3),
                    (s * 0.9, s * 0.84),
                    (s * 0.1, s * 0.84),
                    (s * 0.1, s * 0.3),
                ],
                w,
            );
            path.stroke_polyline(&arc_points(c, s * 0.56, s * 0.16, 0.0, PI * 2.0), w);
        }
        Icon::Plus => {
            path.stroke_segment((c, s * 0.15), (c, s * 0.85), w);
            path.stroke_segment((s * 0.15, c), (s * 0.85, c), w);
        }
        Icon::Grid => {
            let pts = [
                (s * 0.12, s * 0.12),
                (s * 0.88, s * 0.12),
                (s * 0.88, s * 0.88),
                (s * 0.12, s * 0.88),
                (s * 0.12, s * 0.12),
            ];
            path.stroke_polyline(&pts, w);
            for t in [0.37, 0.63] {
                path.stroke_segment((s * t, s * 0.12), (s * t, s * 0.88), w * 0.8);
                path.stroke_segment((s * 0.12, s * t), (s * 0.88, s * t), w * 0.8);
            }
        }
        Icon::Tagged => {
            let pts = [
                (s * 0.12, s * 0.12),
                (s * 0.88, s * 0.12),
                (s * 0.88, s * 0.88),
                (s * 0.12, s * 0.88),
                (s * 0.12, s * 0.12),
            ];
            path.stroke_polyline(&pts, w);
            path.stroke_polyline(&arc_points(c, s * 0.42, s * 0.13, 0.0, PI * 2.0), w);
            path.stroke_polyline(&arc_points(c, s * 0.9, s * 0.25, PI * 1.15, PI * 1.85), w);
        }
        Icon::Carousel => {
            path.polygon(&[
                (s * 0.3, s * 0.08),
                (s * 0.92, s * 0.08),
                (s * 0.92, s * 0.7),
                (s * 0.3, s * 0.7),
            ]);
            path.stroke_polyline(
                &[
                    (s * 0.12, s * 0.3),
                    (s * 0.12, s * 0.92),
                    (s * 0.72, s * 0.92),
                ],
                w,
            );
        }
        Icon::Play => path.polygon(&[(s * 0.25, s * 0.12), (s * 0.88, c), (s * 0.25, s * 0.88)]),
        Icon::ChevronDown => path.stroke_polyline(
            &[(s * 0.2, s * 0.36), (c, s * 0.66), (s * 0.8, s * 0.36)],
            w,
        ),
        Icon::ChevronRight => path.stroke_polyline(
            &[(s * 0.36, s * 0.2), (s * 0.66, c), (s * 0.36, s * 0.8)],
            w,
        ),
        Icon::Compose => {
            path.stroke_polyline(
                &[
                    (s * 0.55, s * 0.12),
                    (s * 0.12, s * 0.12),
                    (s * 0.12, s * 0.88),
                    (s * 0.88, s * 0.88),
                    (s * 0.88, s * 0.45),
                ],
                w,
            );
            path.stroke_segment((s * 0.45, s * 0.55), (s * 0.86, s * 0.14), w);
        }
        Icon::Menu => {
            for y in [s * 0.22, c, s * 0.78] {
                path.stroke_segment((s * 0.12, y), (s * 0.88, y), w);
            }
        }
        Icon::Lock => {
            path.stroke_polyline(
                &[
                    (s * 0.2, s * 0.44),
                    (s * 0.8, s * 0.44),
                    (s * 0.8, s * 0.9),
                    (s * 0.2, s * 0.9),
                    (s * 0.2, s * 0.44),
                ],
                w,
            );
            path.stroke_polyline(&arc_points(c, s * 0.34, s * 0.2, PI, PI * 2.0), w);
            path.stroke_segment((s * 0.3, s * 0.34), (s * 0.3, s * 0.44), w);
            path.stroke_segment((s * 0.7, s * 0.34), (s * 0.7, s * 0.44), w);
        }
        Icon::Check => path.stroke_polyline(
            &[
                (s * 0.16, s * 0.52),
                (s * 0.4, s * 0.76),
                (s * 0.84, s * 0.26),
            ],
            w * 1.3,
        ),
        Icon::Pin => {
            path.polygon(&[
                (s * 0.3, s * 0.1),
                (s * 0.7, s * 0.1),
                (s * 0.64, s * 0.46),
                (s * 0.82, s * 0.62),
                (s * 0.18, s * 0.62),
                (s * 0.36, s * 0.46),
            ]);
            path.stroke_segment((c, s * 0.6), (c, s * 0.92), w);
        }
        Icon::Mic => {
            path.stroke_polyline(&arc_points(c, s * 0.36, s * 0.16, PI, PI * 2.0), w);
            path.stroke_polyline(&arc_points(c, s * 0.5, s * 0.16, 0.0, PI), w);
            path.stroke_segment((s * 0.34, s * 0.36), (s * 0.34, s * 0.5), w);
            path.stroke_segment((s * 0.66, s * 0.36), (s * 0.66, s * 0.5), w);
            path.stroke_polyline(&arc_points(c, s * 0.5, s * 0.3, 0.0, PI), w);
            path.stroke_segment((c, s * 0.8), (c, s * 0.92), w);
        }
        Icon::Photo => {
            path.stroke_polyline(
                &[
                    (s * 0.1, s * 0.14),
                    (s * 0.9, s * 0.14),
                    (s * 0.9, s * 0.86),
                    (s * 0.1, s * 0.86),
                    (s * 0.1, s * 0.14),
                ],
                w,
            );
            path.stroke_polyline(
                &[
                    (s * 0.14, s * 0.76),
                    (s * 0.4, s * 0.48),
                    (s * 0.6, s * 0.66),
                    (s * 0.72, s * 0.56),
                    (s * 0.88, s * 0.72),
                ],
                w,
            );
            path.circle(s * 0.68, s * 0.34, w * 1.1);
        }
        Icon::Phone => {
            let pts = arc_points(s * 0.86, s * 0.14, s * 0.62, FRAC_PI_2, PI);
            path.stroke_polyline(&pts, w * 1.6);
        }
        Icon::Video => {
            path.stroke_polyline(
                &[
                    (s * 0.08, s * 0.26),
                    (s * 0.66, s * 0.26),
                    (s * 0.66, s * 0.74),
                    (s * 0.08, s * 0.74),
                    (s * 0.08, s * 0.26),
                ],
                w,
            );
            path.stroke_polyline(
                &[
                    (s * 0.66, s * 0.44),
                    (s * 0.92, s * 0.3),
                    (s * 0.92, s * 0.7),
                    (s * 0.66, s * 0.56),
                ],
                w,
            );
        }
        Icon::Hashtag => {
            path.stroke_segment((s * 0.38, s * 0.12), (s * 0.3, s * 0.88), w);
            path.stroke_segment((s * 0.7, s * 0.12), (s * 0.62, s * 0.88), w);
            path.stroke_segment((s * 0.14, s * 0.36), (s * 0.88, s * 0.36), w);
            path.stroke_segment((s * 0.12, s * 0.64), (s * 0.86, s * 0.64), w);
        }
    }
}

/// Instagram's verified badge: a blue rosette with a white check.
pub fn verified(canvas: &mut Canvas, cache: &Cache, x: f32, cy: f32, size: f32) {
    let r = size / 2.0;
    let mut path = Path::new();
    let n = 16;
    let mut pts = Vec::with_capacity(n * 2);
    for i in 0..n * 2 {
        let a = std::f32::consts::PI * i as f32 / n as f32;
        let rr = if i % 2 == 0 { r } else { r * 0.86 };
        pts.push((x + r + rr * a.cos(), cy + rr * a.sin()));
    }
    path.polygon(&pts);
    canvas.fill_path(&path, 0x0095f6, 1.0);
    icon(
        canvas,
        cache,
        Icon::Check,
        x + size * 0.2,
        cy - size * 0.3,
        size * 0.6,
        false,
        0xffffff,
    );
}

/// A red dot or count badge centred at (cx, cy).
pub fn badge(canvas: &mut Canvas, cx: f32, cy: f32, n: u32) {
    if n == 0 {
        canvas.fill_circle(cx, cy, dp(4.0), RED, 1.0);
        return;
    }
    let label = if n > 9 {
        "9+".to_owned()
    } else {
        n.to_string()
    };
    let font = Font {
        size: 30.0,
        weight: Weight::SemiBold,
    };
    let w = (measure(&label, font) + dp(10.0)).max(dp(18.0));
    let h = dp(18.0);
    canvas.fill_rrect(
        RectF::new(cx - w / 2.0 - 4.0, cy - h / 2.0 - 4.0, w + 8.0, h + 8.0),
        (h + 8.0) / 2.0,
        BG,
        1.0,
    );
    canvas.fill_rrect(
        RectF::new(cx - w / 2.0, cy - h / 2.0, w, h),
        h / 2.0,
        RED,
        1.0,
    );
    draw_text_centered(canvas, &label, cx, baseline(cy, font), font, 0xffffff);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_instagram_writes_them() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1234), "1,234");
        assert_eq!(count(12_300), "12.3K");
        assert_eq!(count(120_000), "120K");
        assert_eq!(count(1_000_000), "1M");
        assert_eq!(count(2_450_000), "2.5M");
    }

    #[test]
    fn ages_are_short_and_long() {
        let now = 1_800_000_000_000;
        assert_eq!(age_short(now - 30_000, now), "now");
        assert_eq!(age_short(now - 5 * 60_000, now), "5m");
        assert_eq!(age_short(now - 3 * 3_600_000, now), "3h");
        assert_eq!(age_long(now - 2 * 3_600_000, now), "2 hours ago");
        assert_eq!(age_long(now - 86_400_000, now), "1 day ago");
    }

    #[test]
    fn styled_words_wrap_into_lines() {
        let words = rich_words("hello @friend this is a long caption #tag", BODY, TEXT);
        let (lines, truncated) = wrap_runs(&words, 300.0, 10);
        assert!(lines.len() > 1);
        assert!(!truncated);
        assert!(lines.iter().all(|line| line_width(line) <= 301.0));
        let (two, cut) = wrap_runs(&words, 300.0, 2);
        assert_eq!(two.len(), 2);
        assert!(cut);
        let tinted: Vec<_> = lines
            .iter()
            .flatten()
            .filter(|run| run.color == LINK)
            .collect();
        assert!(!tinted.is_empty());
    }
}
