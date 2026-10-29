//! Laid-out pieces (A): drawing commands relative to a piece's top
//! (`Op`), snapped so a moved piece draws the same pixels, and how they
//! are drawn. Every transcript piece (a markdown block, a bubble, a tool
//! row) is a `Laid`: its height, its ops, its tap targets.
//!
//! The ops: text (with tracking), filled and bordered rectangles, lines,
//! icons, images, chevrons, and clip groups whose content scrolls
//! sideways by an offset the view keeps per key (code blocks and tables,
//! C1), so dragging one draws only its rows.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::gfx::{Canvas, Image, Path, RectF, Rgb};
use crate::text::{self, Weight};

use super::super::model::Action;
use super::super::theme;
use super::super::ui::icons::{self, Icon};

/// An image an op draws, compared by identity. Weak: the image cache
/// (`images.rs`) alone keeps previews in memory; one it dropped is no
/// longer laid out (its pieces' keys change), and draws nothing.
#[derive(Clone)]
pub struct ImageHandle(pub Weak<Image>);

impl ImageHandle {
    pub fn new(image: &Rc<Image>) -> Self {
        Self(Rc::downgrade(image))
    }
}

impl PartialEq for ImageHandle {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ImageHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.upgrade() {
            Some(image) => write!(f, "Image({}x{})", image.width, image.height),
            None => write!(f, "Image(gone)"),
        }
    }
}

/// Drawing commands of a laid-out piece, relative to its top.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Lines, icons, images and clips are C1's and C2's.
pub enum Op {
    Text {
        x: f32,
        baseline: f32,
        text: String,
        weight: Weight,
        size: f32,
        /// Px between glyphs (uppercase labels).
        tracking: f32,
        color: Rgb,
        alpha: f32,
    },
    /// A filled rectangle (radius 0: square, at whole pixels).
    Rect {
        rect: RectF,
        radius: f32,
        color: Rgb,
        alpha: f32,
    },
    /// A rectangle's border, `width` wide inside it (square).
    Border { rect: RectF, width: f32, color: Rgb },
    /// A stroked segment with round caps.
    Line {
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
        color: Rgb,
        alpha: f32,
    },
    Icon {
        icon: Icon,
        cx: f32,
        cy: f32,
        size: f32,
        color: Rgb,
        alpha: f32,
    },
    /// An image scaled into `rect`.
    Image { rect: RectF, image: ImageHandle },
    /// A disclosure chevron (right; down when open).
    Chevron { x: f32, cy: f32, open: bool },
    /// Ops drawn only inside `rect`, shifted left by the view's offset for
    /// `xscroll` (a horizontally scrolled code block or table) and up by
    /// its offset for `yscroll` (a tall table's rows; C1).
    Clip {
        rect: RectF,
        xscroll: Option<u64>,
        yscroll: Option<u64>,
        ops: Vec<Op>,
    },
}

/// A text op: no tracking, opaque.
pub fn text(
    x: f32,
    baseline: f32,
    text: impl Into<String>,
    weight: Weight,
    size: f32,
    color: Rgb,
) -> Op {
    Op::Text {
        x,
        baseline,
        text: text.into(),
        weight,
        size,
        tracking: 0.0,
        color,
        alpha: 1.0,
    }
}

/// A square opaque rectangle op.
pub fn rect(rect: RectF, color: Rgb) -> Op {
    Op::Rect {
        rect,
        radius: 0.0,
        color,
        alpha: 1.0,
    }
}

/// Vertical positions on a 1/16 pixel grid: then moving a piece by whole
/// pixels changes no float rounding, so a scrolled frame (rows moved) and a
/// redrawn one agree to the last bit.
pub fn snap(value: f32) -> f32 {
    (value * 16.0).round() / 16.0
}

fn snap_op(op: &mut Op, keys: &mut Vec<u64>) {
    match op {
        Op::Text { baseline, .. } => *baseline = snap(*baseline),
        Op::Rect { rect, .. } | Op::Border { rect, .. } | Op::Image { rect, .. } => {
            rect.y = snap(rect.y);
            rect.h = snap(rect.h);
        }
        Op::Line { from, to, .. } => {
            from.1 = snap(from.1);
            to.1 = snap(to.1);
        }
        Op::Icon { cy, .. } | Op::Chevron { cy, .. } => *cy = snap(*cy),
        Op::Clip {
            rect,
            xscroll,
            yscroll,
            ops,
        } => {
            rect.y = snap(rect.y);
            rect.h = snap(rect.h);
            keys.extend(xscroll.iter().chain(yscroll.iter()));
            for op in ops {
                snap_op(op, keys);
            }
        }
    }
}

/// A laid-out piece made exact: positions snapped, the rows it reaches and
/// its horizontal scrollers worked out.
pub fn quantized(mut laid: Laid) -> Laid {
    let mut ink = (0.0_f32, laid.h);
    let mut keys = Vec::new();
    for op in &mut laid.ops {
        snap_op(op, &mut keys);
        let (top, bottom) = op_rows(op);
        ink = (ink.0.min(top), ink.1.max(bottom));
    }
    for (rect, _) in &mut laid.hits {
        rect.y = snap(rect.y);
        rect.h = snap(rect.h);
    }
    for (_, _, rect) in &mut laid.ymax {
        rect.y = snap(rect.y);
        rect.h = snap(rect.h);
    }
    laid.h = snap(laid.h);
    laid.ink = (ink.0.floor(), ink.1.max(laid.h).ceil());
    laid.xkeys = keys;
    laid
}

/// One laid-out transcript piece (a bubble, a markdown block, a tool or
/// thinking row): its height, drawing and tap targets, relative to its top.
#[derive(Debug, Default)]
pub struct Laid {
    pub h: f32,
    pub ops: Vec<Op>,
    pub hits: Vec<(RectF, Action)>,
    /// Rows its ops may touch (glyphs reach outside `0..h`).
    pub ink: (f32, f32),
    /// Its scrollers' keys (`Op::Clip`, horizontal and vertical): a
    /// frame where one moved draws the piece again.
    pub xkeys: Vec<u64>,
    /// How far each of its horizontal scrollers can scroll (C1).
    pub xmax: Vec<(u64, f32)>,
    /// Its vertical scrollers (a tall table's rows, C1): how far each can
    /// scroll and where a drag moves it, relative to the piece.
    pub ymax: Vec<(u64, f32, RectF)>,
    /// The image previews it shows (fetched while it is near the screen).
    pub images: Vec<super::super::images::Want>,
}

/// Rows an op may touch, relative to its piece (generous: glyphs, emoji
/// and anti-aliasing included).
pub fn op_rows(op: &Op) -> (f32, f32) {
    match op {
        Op::Text { baseline, size, .. } => (baseline - size * 1.4, baseline + size * 0.7),
        Op::Rect { rect, .. } | Op::Border { rect, .. } | Op::Image { rect, .. } => {
            (rect.y - 2.0, rect.y + rect.h + 2.0)
        }
        Op::Line {
            from, to, width, ..
        } => (
            from.1.min(to.1) - width - 2.0,
            from.1.max(to.1) + width + 2.0,
        ),
        Op::Icon { cy, size, .. } => icons::rows(*cy, *size),
        Op::Chevron { cy, .. } => (cy - 20.0, cy + 20.0),
        Op::Clip { rect, .. } => (rect.y - 2.0, rect.y + rect.h + 2.0),
    }
}

/// A position on the pixel grid: half up for every value, so a whole-pixel
/// shift moves it by exactly that (`round` and `as` are not, around zero).
pub fn px(value: f32) -> i32 {
    (value + 0.5).floor() as i32
}

/// No horizontal scroll (draws without the view).
#[allow(dead_code)]
pub fn no_xscroll() -> &'static HashMap<u64, f32> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<HashMap<u64, f32>> = OnceLock::new();
    EMPTY.get_or_init(HashMap::new)
}

/// Draws the ops of a piece placed at `dy` that can reach rows `clip`.
pub fn draw_ops(
    canvas: &mut Canvas,
    ops: &[Op],
    dy: f32,
    clip: (f32, f32),
    xscroll: &HashMap<u64, f32>,
) {
    draw_ops_at(canvas, ops, 0.0, dy, clip, xscroll);
}

fn draw_ops_at(
    canvas: &mut Canvas,
    ops: &[Op],
    dx: f32,
    dy: f32,
    clip: (f32, f32),
    xscroll: &HashMap<u64, f32>,
) {
    for op in ops {
        let (top, bottom) = op_rows(op);
        if top + dy >= clip.1 || bottom + dy <= clip.0 {
            continue;
        }
        match op {
            Op::Text {
                x,
                baseline,
                text: string,
                weight,
                size,
                tracking,
                color,
                alpha,
            } => {
                text::draw_tracked(
                    canvas,
                    string,
                    x + dx,
                    baseline + dy,
                    *weight,
                    *size,
                    *tracking,
                    *color,
                    *alpha,
                );
            }
            Op::Rect {
                rect,
                radius,
                color,
                alpha,
            } => {
                let r = RectF::new(rect.x + dx, rect.y + dy, rect.w, rect.h);
                if *radius > 0.0 {
                    canvas.fill_rrect(r, *radius, *color, *alpha);
                } else {
                    // Edges at whole pixels, the same for any whole-pixel
                    // shift (a rect starting above the canvas included).
                    let (x0, y0) = (px(r.x), px(r.y));
                    let (x1, y1) = (px(r.x + r.w), px(r.y + r.h).max(y0 + 1));
                    canvas.fill_rect(x0, y0, x1 - x0, y1 - y0, *color, *alpha);
                }
            }
            Op::Border { rect, width, color } => {
                let (x, y) = (px(rect.x + dx), px(rect.y + dy));
                let (w, h) = (px(rect.x + dx + rect.w) - x, px(rect.y + dy + rect.h) - y);
                let b = width.round().max(1.0) as i32;
                canvas.fill_rect(x, y, w, b, *color, 1.0);
                canvas.fill_rect(x, y + h - b, w, b, *color, 1.0);
                canvas.fill_rect(x, y + b, b, h - 2 * b, *color, 1.0);
                canvas.fill_rect(x + w - b, y + b, b, h - 2 * b, *color, 1.0);
            }
            Op::Line {
                from,
                to,
                width,
                color,
                alpha,
            } => {
                // Rasterized near the origin and placed at whole pixels: the
                // same coverage wherever the piece is.
                let base = ((from.0 + dx).floor() as i32, (from.1 + dy).floor() as i32);
                let local = |(x, y): (f32, f32)| (x + dx - base.0 as f32, y + dy - base.1 as f32);
                let mut path = Path::new();
                path.stroke_segment(local(*from), local(*to), *width);
                if let Some((x, y, w, h, mask)) = path.rasterize() {
                    canvas.draw_mask(x + base.0, y + base.1, w, h, &mask, *color, *alpha);
                }
            }
            Op::Icon {
                icon,
                cx,
                cy,
                size,
                color,
                alpha,
            } => icons::draw(canvas, *icon, cx + dx, cy + dy, *size, *color, *alpha),
            Op::Image { rect, image } => {
                if let Some(image) = image.0.upgrade() {
                    let r = RectF::new(rect.x + dx, rect.y + dy, rect.w, rect.h);
                    let (x, y) = (px(r.x), px(r.y));
                    if image.width == px(r.x + r.w) - x && image.height == px(r.y + r.h) - y {
                        // Its own size: copied, pixel for pixel.
                        canvas.draw_image(&image, x, y, 1.0);
                    } else {
                        canvas.draw_image_scaled(&image, r, 1.0);
                    }
                }
            }
            Op::Chevron { x, cy, open } => small_chevron(canvas, x + dx, cy + dy, *open),
            Op::Clip {
                rect,
                xscroll: xkey,
                yscroll: ykey,
                ops,
            } => {
                let offset = |key: &Option<u64>| {
                    key.and_then(|key| xscroll.get(&key))
                        .copied()
                        .unwrap_or(0.0)
                };
                let (ox, oy) = (offset(xkey), offset(ykey));
                let area = RectF::new(rect.x + dx, rect.y + dy, rect.w, rect.h);
                super::super::ui::widgets::clipped(canvas, area, |canvas| {
                    draw_ops_at(canvas, ops, dx - ox, dy - oy, clip, xscroll);
                });
            }
        }
    }
}

/// A disclosure chevron (an icon: whole pixels, cached).
pub fn small_chevron(canvas: &mut Canvas, x: f32, cy: f32, open: bool) {
    let icon = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    icons::draw_styled(
        canvas,
        icon,
        x,
        cy,
        icons::Style::new(48.0, theme::p().muted).stroke(5.0),
    );
}
