//! A floating overlay drawn as a layer (E: the chat's chips). Its look is
//! drawn once into a `Sprite` (each pixel's colour and how much of what is
//! under it still shows, from one render on black and one on white), and a
//! `Layer` keeps the page's pixels under it. When the page moves or draws
//! under it, `Layer::repaint` puts the page's pixels back where the move
//! carried the layer's and blends the sprite over the page again: a copy
//! and a blend of its own pixels, instead of drawing the page there again
//! (which, under the chips, cost as much as the rest of a scroll frame).

use std::cell::RefCell;

use crate::gfx::Canvas;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Nothing drawn: the page shows.
    Clear,
    /// Opaque: the sprite's colour.
    Solid,
    /// Partly covering (a shadow, an anti-aliased edge).
    Blend,
}

/// What a layer looks like, at a place on the canvas.
pub struct Sprite {
    /// `x0, y0, x1, y1` on the canvas.
    pub rect: (i32, i32, i32, i32),
    /// Drawn over black: the colour it adds.
    base: Vec<u32>,
    /// How much of what is under it stays, 0..=256.
    keep: Vec<u32>,
    /// Each row's runs of one kind: `x0, x1` (relative), kind.
    runs: Vec<Vec<(i32, i32, Kind)>>,
    /// A hash of each row (two equal rows look the same over the same page).
    rows: Vec<u64>,
}

impl Sprite {
    /// Renders `draw` (which draws at canvas coordinates minus the offset
    /// it is given) over black and over white.
    pub fn render(rect: (i32, i32, i32, i32), draw: impl Fn(&mut Canvas, f32, f32)) -> Self {
        let (w, h) = ((rect.2 - rect.0).max(0), (rect.3 - rect.1).max(0));
        let mut black = Canvas::new(w, h);
        black.clear(0x000000);
        let mut white = Canvas::new(w, h);
        white.clear(0xff_ffff);
        draw(&mut black, rect.0 as f32, rect.1 as f32);
        draw(&mut white, rect.0 as f32, rect.1 as f32);
        let mut keep = Vec::with_capacity(black.pixels.len());
        for (&b, &wt) in black.pixels.iter().zip(&white.pixels) {
            let channel = |shift: u32| ((wt >> shift) & 0xff).saturating_sub((b >> shift) & 0xff);
            let shows = channel(16).min(channel(8)).min(channel(0));
            // Floor, so a blend never carries into the next channel.
            keep.push(shows * 256 / 255);
        }
        let mut runs = Vec::with_capacity(h as usize);
        for y in 0..h as usize {
            let mut row: Vec<(i32, i32, Kind)> = Vec::new();
            for x in 0..w as usize {
                let i = y * w as usize + x;
                let kind = match (keep[i], black.pixels[i]) {
                    (256, 0) => Kind::Clear,
                    (0, _) => Kind::Solid,
                    _ => Kind::Blend,
                };
                match row.last_mut() {
                    Some(last) if last.2 == kind => last.1 += 1,
                    _ => row.push((x as i32, x as i32 + 1, kind)),
                }
            }
            runs.push(row);
        }
        let rows = (0..h as usize)
            .map(|y| {
                let at = y * w as usize..(y + 1) * w as usize;
                black.pixels[at.clone()]
                    .iter()
                    .chain(&keep[at])
                    .fold(0xcbf2_9ce4_8422_2325_u64, |hash, v| {
                        (hash ^ u64::from(*v)).wrapping_mul(0x0100_0000_01b3)
                    })
            })
            .collect();
        Self {
            rect,
            base: black.pixels,
            keep,
            runs,
            rows,
        }
    }

    /// Tests: its size and pixels of each kind.
    #[cfg(test)]
    pub fn stats(&self) -> String {
        let mut count = [0; 3];
        let mut runs = 0;
        for row in &self.runs {
            runs += row.len();
            for &(a, b, kind) in row {
                count[kind as usize] += b - a;
            }
        }
        let faint: Vec<usize> = [240, 248, 250, 252, 254]
            .iter()
            .map(|t| self.keep.iter().filter(|k| **k >= *t && **k < 256).count())
            .collect();
        format!(
            "{:?}: clear {}, solid {}, blend {}, {runs} runs; keep >= 240/248/250/252/254: {faint:?}",
            self.rect, count[0], count[1], count[2]
        )
    }

    fn width(&self) -> usize {
        (self.rect.2 - self.rect.0) as usize
    }

    /// Row `y` (relative) between columns `x0..x1` (relative) over `under`
    /// (the page's pixels there), into `out`.
    fn composite(&self, y: usize, x0: i32, x1: i32, under: &[u32], out: &mut [u32]) {
        let row = y * self.width();
        for &(a, b, kind) in &self.runs[y] {
            let (a, b) = (a.max(x0), b.min(x1));
            if a >= b {
                continue;
            }
            let (from, to) = ((a - x0) as usize, (b - x0) as usize);
            let (sa, sb) = (row + a as usize, row + b as usize);
            match kind {
                Kind::Clear => out[from..to].copy_from_slice(&under[from..to]),
                Kind::Solid => out[from..to].copy_from_slice(&self.base[sa..sb]),
                Kind::Blend => blend(
                    &mut out[from..to],
                    &under[from..to],
                    &self.base[sa..sb],
                    &self.keep[sa..sb],
                ),
            }
        }
    }
}

/// `out = base + under × keep / 256`, per channel (one loop the compiler
/// vectorizes).
fn blend(out: &mut [u32], under: &[u32], base: &[u32], keep: &[u32]) {
    let n = out.len();
    let (under, base, keep) = (&under[..n], &base[..n], &keep[..n]);
    for i in 0..n {
        let page = under[i];
        let rb = (((page & 0x00ff_00ff) * keep[i]) >> 8) & 0x00ff_00ff;
        let g = (((page & 0x0000_ff00) * keep[i]) >> 8) & 0x0000_ff00;
        out[i] = base[i] + (rb | g);
    }
}

#[cfg(test)]
thread_local! {
    /// Repaints done (tests: the cheap path was taken).
    pub static REPAINTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Where a layer is painted and the page's pixels under it.
struct Painted {
    rect: (i32, i32, i32, i32),
    /// The page's pixels under it, a row per row of `rect`, rotated by
    /// `origin` rows (a move turns it instead of copying it).
    under: Vec<u32>,
    origin: usize,
    /// Every pixel of `under` was taken from the page (not only a clipped
    /// part of it).
    complete: bool,
}

impl Painted {
    /// Where row `y`'s pixels start in `under`.
    fn row(&self, y: i32) -> usize {
        let (h, w) = (self.rect.3 - self.rect.1, self.rect.2 - self.rect.0);
        ((y - self.rect.1) as usize + self.origin) % h as usize * w as usize
    }
}

/// The page's pixels under a layer painted on the canvas.
#[derive(Default)]
pub struct Layer {
    painted: RefCell<Option<Painted>>,
}

/// The intersection of two rectangles, if not empty.
fn intersect(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let r = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (r.2 > r.0 && r.3 > r.1).then_some(r)
}

impl Layer {
    /// Nothing of it is on the canvas any more.
    pub fn clear(&self) {
        *self.painted.borrow_mut() = None;
    }

    /// Paints `sprite` over what the canvas holds within its clip (the
    /// page, just drawn there), keeping those pixels.
    pub fn paint(&self, canvas: &mut Canvas, sprite: &Sprite) {
        let bounds = (0, 0, canvas.width, canvas.height);
        let Some(rect) = intersect(sprite.rect, bounds) else {
            self.clear();
            return;
        };
        let mut painted = self.painted.borrow_mut();
        if painted.as_ref().is_none_or(|p| p.rect != rect) {
            let len = ((rect.2 - rect.0) * (rect.3 - rect.1)) as usize;
            *painted = Some(Painted {
                rect,
                under: vec![0; len],
                origin: 0,
                complete: false,
            });
        }
        let painted = painted.as_mut().expect("just set");
        let Some(area) = intersect(rect, canvas.clip()) else {
            return;
        };
        painted.complete |= area == rect;
        let width = canvas.width as usize;
        for y in area.1..area.3 {
            let at = painted.row(y) + (area.0 - rect.0) as usize;
            let under = &mut painted.under[at..at + (area.2 - area.0) as usize];
            let row = y as usize * width;
            let out = &mut canvas.pixels[row + area.0 as usize..row + area.2 as usize];
            under.copy_from_slice(out);
            sprite.composite(
                (y - sprite.rect.1) as usize,
                area.0 - sprite.rect.0,
                area.2 - sprite.rect.0,
                under,
                out,
            );
        }
    }

    /// The page under it changed: `moved` rows (`y0, y1, dy`) shifted with
    /// its pixels in them, then `rows` drawn again whole. Puts the page's
    /// pixels back where the move carried the layer's, takes the page's new
    /// pixels under it and paints `sprite` over them. The areas changed
    /// (`x0, y0, x1, y1`), or None when it cannot (not wholly painted, or the
    /// page drew in some columns only near it: `areas`).
    pub fn repaint(
        &self,
        canvas: &mut Canvas,
        sprite: &Sprite,
        moved: Option<(i32, i32, i32)>,
        rows: &[(i32, i32)],
        areas: &[(i32, i32, i32, i32)],
    ) -> Option<Vec<(i32, i32, i32, i32)>> {
        let mut painted = self.painted.borrow_mut();
        let painted = painted.as_mut()?;
        let (x0, y0, x1, y1) = painted.rect;
        if !painted.complete
            || sprite.rect != painted.rect
            || canvas.clip() != (0, 0, canvas.width, canvas.height)
        {
            return None;
        }
        let (m0, m1, dy) = moved.unwrap_or((0, 0, 0));
        // Where the move carried the layer's pixels (its moved rows).
        let carried = ((y0.max(m0) + dy).max(m0), (y1.min(m1) + dy).min(m1));
        let carried = (dy != 0 && carried.1 > carried.0).then_some(carried);
        let reach = match carried {
            Some((c0, c1)) => (x0, y0.min(c0), x1, y1.max(c1)),
            None => painted.rect,
        };
        if areas.iter().any(|&area| intersect(area, reach).is_some()) {
            return None;
        }
        // The move turns what was under the layer with the page; it must have
        // moved all of it.
        if dy != 0 && (y0 < m0 || y1 > m1) {
            return None;
        }
        let drawn = |y: i32| rows.iter().any(|&(a, b)| (a..b).contains(&y));
        let w = (x1 - x0) as usize;
        let width = canvas.width as usize;
        let span = |y: i32| y as usize * width + x0 as usize;
        // Rows of the canvas (and the window) to send again.
        let mut changed: Vec<(i32, i32)> = Vec::new();
        let mut mark = |y: i32| match changed.last_mut() {
            Some(last) if last.1 == y => last.1 = y + 1,
            _ => changed.push((y, y + 1)),
        };
        for y in carried.map_or(0..0, |(c0, c1)| c0..c1) {
            if (y0..y1).contains(&y) || drawn(y) {
                continue;
            }
            // The page's pixels that were under the layer before the move.
            let from = painted.row(y - dy);
            let at = span(y);
            canvas.pixels[at..at + w].copy_from_slice(&painted.under[from..from + w]);
            mark(y);
        }
        // What was under the layer moved with the page: row y now has what
        // row y - dy had. Rows moved in from outside it, and rows drawn
        // again, are the page's own now.
        let h = (y1 - y0) as i64;
        painted.origin = (painted.origin as i64 - dy as i64).rem_euclid(h) as usize;
        for y in y0..y1 {
            let source = y - dy;
            let fresh = drawn(y) || (dy != 0 && !(y0..y1).contains(&source));
            let at = span(y);
            let from = painted.row(y);
            if fresh {
                painted.under[from..from + w].copy_from_slice(&canvas.pixels[at..at + w]);
            }
            // The row the move brought here was this one of the layer over
            // this page: it is right, here and in the window, as it is.
            let r = (y - y0) as usize;
            if !fresh && (dy == 0 || sprite.rows[r] == sprite.rows[(source - y0) as usize]) {
                continue;
            }
            sprite.composite(
                r,
                0,
                w as i32,
                &painted.under[from..from + w],
                &mut canvas.pixels[at..at + w],
            );
            mark(y);
        }
        #[cfg(test)]
        REPAINTS.with(|n| n.set(n.get() + 1));
        changed.sort_unstable();
        let mut merged: Vec<(i32, i32)> = Vec::new();
        for (a, b) in changed {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        Some(merged.into_iter().map(|(a, b)| (x0, a, x1, b)).collect())
    }
}
