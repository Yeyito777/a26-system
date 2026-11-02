//! A scrolled band of the page (the chat list, a chat's messages) drawn the
//! cheap way when only its scroll moved since the last frame: what is drawn
//! moves (`Canvas::move_rows`) and only the rows that came into view are
//! drawn again, as the Exocortex app does. Anything else (new content, a
//! pressed row, a resize) draws the whole band.

use crate::gfx::Canvas;
use crate::social::shell::Damage;

#[derive(Debug, Clone, Copy, Default)]
pub struct Band {
    /// The content's vertical translation last drawn, and what it showed.
    drawn: Option<(i32, u64)>,
}

impl Band {
    /// Forgets what was drawn (the next frame draws the whole band).
    pub fn invalidate(&mut self) {
        self.drawn = None;
    }

    /// Draws rows `y0..y1` at `translate` (the content's y offset; rounded)
    /// for content `key`. `draw` draws the content clipped to a row range.
    /// What changed goes into `damage`. Returns how far the band moved (0:
    /// not at all), or None when it was drawn whole.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        damage: &mut Damage,
        full: bool,
        (y0, y1): (i32, i32),
        translate: i32,
        key: u64,
        mut draw: impl FnMut(&mut Canvas, (i32, i32)),
    ) -> Option<i32> {
        let shift = match self.drawn {
            Some((before, drawn_key)) if !full && drawn_key == key => translate - before,
            _ => i32::MAX,
        };
        self.drawn = Some((translate, key));
        if shift == 0 {
            return Some(0);
        }
        if shift.abs() < (y1 - y0) / 2 {
            canvas.move_rows(y0, y1, shift);
            let strip = if shift < 0 { (y1 + shift, y1) } else { (y0, y0 + shift) };
            canvas.set_clip(0, strip.0, canvas.width, strip.1);
            draw(canvas, strip);
            canvas.reset_clip();
            damage.moved = Some((y0, y1, shift));
            damage.rows.push(strip);
            return Some(shift);
        }
        canvas.set_clip(0, y0, canvas.width, y1);
        draw(canvas, (y0, y1));
        canvas.reset_clip();
        damage.rows.push((y0, y1));
        None
    }
}

/// A key for what a band shows, from anything hashable.
pub fn key_of(parts: &[u64]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        hash ^= *part;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
