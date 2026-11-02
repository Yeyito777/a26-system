//! Round pictures: profile pictures cut to a circle once per picture and
//! size (kept while the picture is), and WhatsApp's grey silhouette for
//! chats without one. Also photos with rounded corners, for bubbles.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use super::icons::{self, Glyph};
use super::theme;
use crate::gfx::{Canvas, Image};
use crate::social::media::{Fit, Media};
use crate::social::proto;

/// A picture a frame drew without, and the rows that showed its place
/// (in the band's content: screen rows less the band's translation).
#[derive(Clone)]
pub struct Waiting {
    key: (String, u32, u32, Fit),
    rows: (f32, f32),
}

/// The pictures a frame draws: missing ones are noted with the rows of the
/// thing drawn (`span`), so only those rows are drawn again when they come.
pub struct Pics<'a> {
    pub media: &'a mut Media,
    pub waiting: &'a mut Vec<Waiting>,
    /// Screen rows less this are content rows.
    pub translate: f32,
    /// The screen rows of what is being drawn now.
    pub span: (f32, f32),
}

impl<'a> Pics<'a> {
    pub fn new(media: &'a mut Media, waiting: &'a mut Vec<Waiting>, translate: f32) -> Self {
        Self {
            media,
            waiting,
            translate,
            span: (0.0, 0.0),
        }
    }

    pub fn get(
        &mut self,
        reference: &str,
        w: u32,
        h: u32,
        fit: Fit,
        now: Instant,
    ) -> Option<Rc<Image>> {
        let got = self.media.get(reference, w, h, fit, now);
        if got.is_none() && !reference.is_empty() {
            let key = (reference.to_owned(), w.min(4096), h.min(4096), fit);
            let rows = (self.span.0 - self.translate, self.span.1 - self.translate);
            if let Some(waiting) = self.waiting.iter_mut().find(|waiting| waiting.key == key) {
                waiting.rows = rows;
            } else if self.waiting.len() < 96 {
                self.waiting.push(Waiting { key, rows });
            }
        }
        got
    }
}

/// Rows of pictures that came, to draw again this frame.
pub struct Arrived {
    /// Content rows (overlapping ones merged), at most a frame's budget.
    pub rows: Vec<(f32, f32)>,
    /// More came than the budget: the next frame draws them.
    pub more: bool,
}

/// Pictures a frame draws again at most (a burst of them that came at
/// once is spread over frames).
const ARRIVED_BUDGET: usize = 2;

/// The content rows of pictures noted missing that have come since (they
/// are forgotten: the frame that draws them notes what is still missing).
/// Those still missing are asked for again: a frame that draws only what
/// moved into view would not ask for them, and `Media` asks the bridge for
/// what was wanted since the last frame only.
pub fn arrived(media: &mut Media, waiting: &mut Vec<Waiting>, now: Instant) -> Arrived {
    let mut rows: Vec<(f32, f32)> = Vec::new();
    let mut more = false;
    waiting.retain(|waiting| {
        let (reference, w, h, fit) = &waiting.key;
        if media.get(reference, *w, *h, *fit, now).is_none() {
            return true;
        }
        let (a, b) = waiting.rows;
        if let Some(row) = rows.iter_mut().find(|(c, d)| a < *d && b > *c) {
            *row = (row.0.min(a), row.1.max(b));
            return false;
        }
        if rows.len() < ARRIVED_BUDGET {
            rows.push((a, b));
            return false;
        }
        more = true;
        true
    });
    Arrived { rows, more }
}

/// Draws a premultiplied picture at (x, y) within the canvas's clip, as
/// `Canvas::draw_image` does (the same pixels), but opaque runs are copied
/// and clear pixels skipped: blending every pixel of a 700 px photo cost
/// 8 M instructions a frame that drew it.
pub fn blit(canvas: &mut Canvas, image: &Image, x: i32, y: i32) {
    let (cx0, cy0, cx1, cy1) = canvas.clip();
    let (x0, x1) = (
        x.max(cx0).max(0),
        (x + image.width).min(cx1).min(canvas.width),
    );
    let (y0, y1) = (
        y.max(cy0).max(0),
        (y + image.height).min(cy1).min(canvas.height),
    );
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let width = canvas.width as usize;
    let span = (x1 - x0) as usize;
    for ty in y0..y1 {
        let from = ((ty - y) * image.width + (x0 - x)) as usize;
        let src = &image.pixels[from..from + span];
        let at = ty as usize * width + x0 as usize;
        let dst = &mut canvas.pixels[at..at + span];
        // A photo's rows are opaque but at its rounded corners: such a row
        // is copied whole (both loops vectorise).
        if src.iter().fold(0xff, |all, s| all & (s >> 24)) == 0xff {
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = s & 0x00ff_ffff;
            }
            continue;
        }
        for (d, &s) in dst.iter_mut().zip(src) {
            let a = s >> 24;
            if a == 0xff {
                *d = s & 0x00ff_ffff;
            } else if a != 0 {
                let inverse = 256 - (a + (a >> 7));
                let channel = |shift: u32| {
                    ((s >> shift & 0xff) + (((*d >> shift & 0xff) * inverse) >> 8)).min(255)
                        << shift
                };
                *d = channel(16) | channel(8) | channel(0);
            }
        }
    }
}

#[derive(Default)]
pub struct Rounded {
    /// (source picture, radius) → its rounded copy.
    cache: HashMap<(usize, u32), (Rc<Image>, Rc<Image>)>,
}

impl Rounded {
    /// `image` with corners of `radius` (a circle at half its size).
    pub fn get(&mut self, image: &Rc<Image>, radius: f32) -> Rc<Image> {
        let key = (Rc::as_ptr(image) as usize, (radius * 2.0) as u32);
        if let Some((source, rounded)) = self.cache.get(&key)
            && Rc::ptr_eq(source, image)
        {
            return rounded.clone();
        }
        let rounded = Rc::new(round(image, radius));
        if self.cache.len() > 400 {
            self.cache
                .retain(|_, (source, _)| Rc::strong_count(source) > 1);
        }
        self.cache.insert(key, (image.clone(), rounded.clone()));
        rounded
    }
}

/// A copy of a premultiplied image with its corners cut round.
pub fn round(image: &Image, radius: f32) -> Image {
    let (w, h) = (image.width, image.height);
    let mut pixels = image.pixels.clone();
    let r = radius.min(w as f32 / 2.0).min(h as f32 / 2.0);
    let ri = r.ceil() as i32;
    for y in 0..h {
        let dy = if y < ri {
            r - (y as f32 + 0.5)
        } else if y >= h - ri {
            (y as f32 + 0.5) - (h as f32 - r)
        } else {
            continue;
        };
        for x in (0..ri.min(w)).chain((w - ri).max(ri)..w) {
            let dx = if x < ri {
                r - (x as f32 + 0.5)
            } else {
                (x as f32 + 0.5) - (w as f32 - r)
            };
            if dx <= 0.0 || dy <= 0.0 {
                continue;
            }
            let distance = (dx * dx + dy * dy).sqrt() - r;
            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            if coverage >= 1.0 {
                continue;
            }
            let index = (y * w + x) as usize;
            let p = pixels[index];
            let scale = |c: u32| ((c as f32) * coverage + 0.5) as u32;
            pixels[index] = scale(p >> 24) << 24
                | scale(p >> 16 & 0xff) << 16
                | scale(p >> 8 & 0xff) << 8
                | scale(p & 0xff);
        }
    }
    Image {
        width: w,
        height: h,
        pixels,
    }
}

/// Draws a chat's or person's picture in a circle of `size` at (x, y), or
/// the silhouette while there is none.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &mut Canvas,
    pics: &mut Pics,
    rounded: &mut Rounded,
    avatar: Option<&proto::Media>,
    group: bool,
    x: f32,
    y: f32,
    size: f32,
    now: Instant,
) {
    let px = size.round() as u32;
    let picture = avatar.and_then(|avatar| pics.get(&avatar.reference, px, px, Fit::Cover, now));
    match picture {
        Some(image) => {
            let round = rounded.get(&image, size / 2.0);
            blit(canvas, &round, x.round() as i32, y.round() as i32);
        }
        None => silhouette(canvas, group, x, y, size),
    }
}

pub fn silhouette(canvas: &mut Canvas, group: bool, x: f32, y: f32, size: f32) {
    let r = size / 2.0;
    canvas.fill_circle(x + r, y + r, r, theme::AVATAR_BG, 1.0);
    let glyph = if group { Glyph::Group } else { Glyph::Person };
    icons::draw(
        canvas,
        glyph,
        x + r,
        y + r + size * 0.04,
        size * 0.62,
        theme::AVATAR_FG,
        1.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blit_draws_what_draw_image_draws() {
        // A rounded photo (opaque rows, soft corners) and a sticker-like
        // picture (clear, translucent and opaque pixels mixed), clipped and
        // not, over a busy background.
        let photo = round(
            &Image {
                width: 40,
                height: 30,
                pixels: (0..1200u32)
                    .map(|n| 0xff00_0000 | n.wrapping_mul(2_654_435_761) & 0x00ff_ffff)
                    .collect(),
            },
            9.0,
        );
        let sticker = Image {
            width: 25,
            height: 20,
            pixels: (0..500u32)
                .map(|n| match n % 3 {
                    0 => 0,
                    1 => 0x80_40_20_10,
                    _ => 0xff_10_80_f0,
                })
                .collect(),
        };
        for image in [&photo, &sticker] {
            for clip in [None, Some((5, 8, 37, 21)), Some((0, 0, 60, 3))] {
                let mut want = Canvas::new(60, 50);
                for (n, pixel) in want.pixels.iter_mut().enumerate() {
                    *pixel = (n as u32).wrapping_mul(40_503) & 0x00ff_ffff;
                }
                let mut got = Canvas::new(60, 50);
                got.pixels.clone_from(&want.pixels);
                if let Some((x0, y0, x1, y1)) = clip {
                    want.set_clip(x0, y0, x1, y1);
                    got.set_clip(x0, y0, x1, y1);
                }
                want.draw_image(image, 3, 4, 1.0);
                blit(&mut got, image, 3, 4);
                assert!(want.pixels == got.pixels, "clip {clip:?}");
            }
        }
    }
}
