//! Images in the transcript (WP-C1, PAR-108, 113, 127, 347, 400): the
//! images a model attached, after its message (above its footer, as the
//! TUI and the iPhone draw them), and the user's pasted images under their
//! bubble. Model images keep their shape within a 465 px box (168 pt);
//! pasted ones are 233 px squares, filled (84 pt). They wrap onto more rows
//! when they do not fit one (the iPhone scrolls them sideways). A tap
//! opens the viewer (`viewer.rs`) on that image.
//!
//! Previews are fetched while their strip is near the screen
//! (`ViewState::image_wants`, every pass of the event loop), nearest first.

use crate::gfx::RectF;
use crate::text::Weight;

use super::super::images::{ImageStore, Pic, State, Want, hash_of};
use super::super::model::{Action, Model, Page, rich};
use super::super::proto::{Block, ImageAttachment};
use super::super::theme::{self, HAIRLINE, T_META, TRANSCRIPT_PAD, W};
use super::ViewState;
use super::ops::{self, ImageHandle, Laid, Op};
use super::transcript_bounds;

/// A model image fits this box (168 pt).
pub const THUMB: f32 = 465.0;
/// A pasted image is a square this big (84 pt).
pub const USER_THUMB: f32 = 233.0;
/// Between images (8 pt).
pub const GAP: f32 = 22.0;
/// The largest preview asked for (the daemon's limit).
const MAX_PREVIEW: u32 = 4096;

/// A message's images, as one piece.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Strip {
    pub pics: Vec<Pic>,
    /// The user's (squares, right-aligned) rather than a model's.
    pub user: bool,
}

impl Strip {
    /// The images a reply's tool calls attached (the attach tool), in order.
    pub fn of_blocks(blocks: &[Block]) -> Option<Strip> {
        let pics: Vec<Pic> = blocks
            .iter()
            .filter_map(|block| match block {
                Block::ToolResult { images, .. } => Some(images.iter().map(Pic::of_ref)),
                _ => None,
            })
            .flatten()
            .collect();
        (!pics.is_empty()).then_some(Strip { pics, user: false })
    }

    /// A user message's pasted images.
    pub fn of_user(images: &[ImageAttachment]) -> Option<Strip> {
        let pics: Vec<Pic> = images.iter().map(Pic::of_attachment).collect();
        (!pics.is_empty()).then_some(Strip { pics, user: true })
    }

    pub fn hash(&self) -> u64 {
        hash_of(("strip", self))
    }

    /// The previews it draws.
    pub fn wants(&self, conv: Option<&str>) -> Vec<Want> {
        self.pics
            .iter()
            .filter_map(|pic| want_for(pic, self.user, conv))
            .collect()
    }

    /// What its look depends on in the cache: each preview's state. Part of
    /// its piece's key.
    pub fn state_key(&self, store: &ImageStore, conv: Option<&str>) -> u64 {
        let states: Vec<u64> = self
            .pics
            .iter()
            .map(|pic| {
                want_for(pic, self.user, conv)
                    .and_then(|want| store.state(&want).map(State::key))
                    .unwrap_or(0)
            })
            .collect();
        hash_of((self.hash(), conv, states))
    }
}

/// The preview a strip draws of `pic`: fitted in the thumbnail box (a
/// model's), or covering the square (the user's).
fn want_for(pic: &Pic, user: bool, conv: Option<&str>) -> Option<Want> {
    let id = pic.id.clone()?;
    let (width, height) = if user {
        if pic.width > 0 && pic.height > 0 {
            let scale = USER_THUMB / pic.width.min(pic.height) as f32;
            (
                ((pic.width as f32 * scale).ceil() as u32).min(MAX_PREVIEW),
                ((pic.height as f32 * scale).ceil() as u32).min(MAX_PREVIEW),
            )
        } else {
            (USER_THUMB as u32, USER_THUMB as u32)
        }
    } else {
        (THUMB as u32, THUMB as u32)
    };
    Some(Want {
        id,
        conv: conv.map(str::to_owned),
        width,
        height,
        thumbnail: true,
    })
}

/// A model image's thumbnail size: its preview's when here, else its own
/// fitted in the box (never enlarged), else the box.
fn thumb_size(pic: &Pic, ready: Option<(i32, i32)>) -> (f32, f32) {
    if let Some((w, h)) = ready {
        return (w as f32, h as f32);
    }
    if pic.width == 0 || pic.height == 0 {
        return (THUMB, THUMB);
    }
    let scale = (THUMB / pic.width.max(pic.height) as f32).min(1.0);
    (
        (pic.width as f32 * scale).round().max(1.0),
        (pic.height as f32 * scale).round().max(1.0),
    )
}

/// The strip as a piece, its top at 0.
pub fn lay_strip(strip: &Strip, store: &ImageStore, conv: Option<&str>) -> Laid {
    let p = theme::p();
    let mut laid = Laid::default();
    let shown: Vec<&Pic> = strip.pics.iter().filter(|pic| pic.id.is_some()).collect();
    let missing = strip.pics.len() - shown.len();
    // Tiles: (pic index, size, state).
    let tiles: Vec<(usize, (f32, f32), Option<&State>)> = strip
        .pics
        .iter()
        .enumerate()
        .filter(|(_, pic)| pic.id.is_some())
        .map(|(index, pic)| {
            let want = want_for(pic, strip.user, conv);
            let state = want.as_ref().and_then(|want| store.state(want));
            let ready = match state {
                Some(State::Ready(image)) => Some((image.width, image.height)),
                _ => None,
            };
            let size = if strip.user {
                (USER_THUMB, USER_THUMB)
            } else {
                thumb_size(pic, ready)
            };
            (index, size, state)
        })
        .collect();
    // Rows of tiles that fit the transcript's width.
    let width = W - 2.0 * TRANSCRIPT_PAD;
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row_w = 0.0;
    for (at, (_, size, _)) in tiles.iter().enumerate() {
        match rows.last_mut() {
            Some(row) if row_w + GAP + size.0 <= width => {
                row.push(at);
                row_w += GAP + size.0;
            }
            _ => {
                rows.push(vec![at]);
                row_w = size.0;
            }
        }
    }
    let mut y = 0.0_f32;
    for row in &rows {
        let row_h = row.iter().map(|at| tiles[*at].1.1).fold(0.0_f32, f32::max);
        let total: f32 =
            row.iter().map(|at| tiles[*at].1.0).sum::<f32>() + GAP * (row.len() as f32 - 1.0);
        let mut x = if strip.user {
            W - TRANSCRIPT_PAD - total
        } else {
            TRANSCRIPT_PAD
        };
        for at in row {
            let (index, (w, h), state) = tiles[*at];
            let rect = RectF::new(x.round(), y, w, h);
            tile(&mut laid, rect, state, strip.user);
            laid.hits.push((
                rect,
                Action::Rich(rich::Act::ViewImages {
                    pics: strip.pics.clone(),
                    index,
                    conv: conv.map(str::to_owned),
                }),
            ));
            x += w + GAP;
        }
        y += row_h + GAP;
    }
    y = (y - GAP).max(0.0);
    if missing > 0 {
        // No bytes and no id (PAR-255): a count instead of empty tiles.
        let ty = T_META.weight(theme::T_ITALIC);
        let label = format!(
            "{missing} image attachment{}",
            if missing == 1 { "" } else { "s" }
        );
        if y > 0.0 {
            y += 11.0;
        }
        let w = crate::text::measure(&label, ty.weight, ty.px());
        let x = if strip.user {
            W - TRANSCRIPT_PAD - w
        } else {
            TRANSCRIPT_PAD
        };
        laid.ops.push(ops::text(
            x,
            ty.baseline(y),
            label,
            ty.weight,
            ty.px(),
            p.muted,
        ));
        y += ty.line();
    }
    laid.h = y;
    laid.images = strip.wants(conv);
    laid
}

/// One image: its preview, or a box saying why there is none yet.
fn tile(laid: &mut Laid, rect: RectF, state: Option<&State>, user: bool) {
    let p = theme::p();
    let border = theme::mix(p.border, 50, p.app_background);
    match state {
        Some(State::Ready(image)) => {
            let (iw, ih) = (image.width as f32, image.height as f32);
            let fitted = if user {
                // Covers the square, cut to it.
                let scale = (rect.w / iw).max(rect.h / ih);
                let (w, h) = ((iw * scale).round(), (ih * scale).round());
                RectF::new(
                    (rect.x + (rect.w - w) / 2.0).round(),
                    (rect.y + (rect.h - h) / 2.0).round(),
                    w,
                    h,
                )
            } else {
                rect
            };
            laid.ops.push(ops::rect(rect, p.surface_background));
            laid.ops.push(Op::Clip {
                rect,
                xscroll: None,
                yscroll: None,
                ops: vec![Op::Image {
                    rect: fitted,
                    image: ImageHandle::new(image),
                }],
            });
        }
        other => {
            laid.ops.push(ops::rect(rect, p.surface_background));
            let label = match other {
                Some(State::Failed(_)) => "image unavailable",
                _ => "loading image…",
            };
            let ty = T_META;
            let lines = super::super::ui::widgets::wrap_plain(label, ty, rect.w - 24.0);
            let top = rect.y + (rect.h - lines.len() as f32 * ty.line()) / 2.0;
            for (index, line) in lines.iter().enumerate() {
                let w = crate::text::measure(line, Weight::MonoMedium, ty.px());
                laid.ops.push(ops::text(
                    (rect.x + (rect.w - w) / 2.0).round(),
                    ty.baseline(top + index as f32 * ty.line()),
                    line.clone(),
                    ty.weight,
                    ty.px(),
                    p.muted,
                ));
            }
        }
    }
    laid.ops.push(Op::Border {
        rect,
        width: HAIRLINE,
        color: border,
    });
}

impl ViewState {
    /// The previews the chat shows within a screen of what is on it, and
    /// those its overlays (the viewer) show: on screen first, then below,
    /// then above.
    pub fn image_wants(
        &mut self,
        model: &Model,
        height: f32,
        now: std::time::Instant,
    ) -> Vec<Want> {
        let mut wants = self.overlays.images(&self.cx(model, W, height, now));
        if model.page != Page::Chat || model.chat.id.is_none() {
            return wants;
        }
        self.layout(model);
        let (top, bottom) = transcript_bounds(model, height, self.keyboard);
        let screen = bottom - top;
        let offset = self.chat_scroll.value(now);
        let (from, to) = (offset - screen, offset + 2.0 * screen);
        let items = &self.layout.items;
        let first = items.partition_point(|item| item.y + item.laid.h < from);
        let mut near: Vec<(u8, f32, &Want)> = Vec::new();
        for item in &items[first..] {
            if item.y > to {
                break;
            }
            if item.laid.images.is_empty() {
                continue;
            }
            let (y0, y1) = (item.y, item.y + item.laid.h);
            let rank = if y1 >= offset && y0 <= offset + screen {
                0
            } else if y0 > offset {
                1
            } else {
                2
            };
            let distance = if y0 > offset {
                y0 - offset
            } else {
                offset - y1
            };
            for want in &item.laid.images {
                near.push((rank, distance, want));
            }
        }
        near.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for (_, _, want) in near {
            if !wants.contains(want) {
                wants.push(want.clone());
            }
        }
        wants
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::ImageRef;

    fn pic(id: &str, w: u32, h: u32) -> ImageRef {
        ImageRef {
            id: id.into(),
            media_type: "image/png".into(),
            width: w,
            height: h,
            size_bytes: 1000,
            name: Some(format!("{id}.png")),
        }
    }

    #[test]
    fn model_images_keep_their_shape_and_wrap_onto_rows() {
        let blocks = vec![
            Block::ToolResult {
                tool_call_id: "t".into(),
                tool_name: "attach".into(),
                output: String::new(),
                is_error: false,
                images: vec![
                    pic("a", 1080, 2400),
                    pic("b", 2000, 1000),
                    pic("c", 100, 50),
                ],
            },
            Block::text("Here."),
        ];
        let strip = Strip::of_blocks(&blocks).unwrap();
        let store = ImageStore::default();
        let laid = lay_strip(&strip, &store, Some("c"));
        let tiles: Vec<RectF> = laid.hits.iter().map(|(rect, _)| *rect).collect();
        // A phone screenshot: 209 × 465; a wide one 465 × 233 beside it;
        // the small one is never enlarged and wraps (209 + 465 + 100 and
        // the gaps exceed 1000 px? no: it fits) on the first row.
        assert_eq!((tiles[0].w, tiles[0].h), (209.0, 465.0));
        assert_eq!((tiles[1].w, tiles[1].h), (465.0, 233.0));
        assert_eq!((tiles[2].w, tiles[2].h), (100.0, 50.0));
        assert_eq!(tiles[0].x, TRANSCRIPT_PAD);
        assert_eq!(tiles[1].x, TRANSCRIPT_PAD + 209.0 + GAP);
        assert_eq!(laid.h, 465.0);
        assert_eq!(laid.images.len(), 3);
        assert_eq!(laid.images[0].width, 465);
        // A fourth wide image goes onto a second row.
        let mut more = strip.clone();
        more.pics.push(Pic::of_ref(&pic("d", 2000, 1000)));
        let laid = lay_strip(&more, &store, Some("c"));
        assert_eq!(laid.hits[3].0.y, 465.0 + GAP);
        assert_eq!(laid.h, 465.0 + GAP + 233.0);
        match &laid.hits[3].1 {
            Action::Rich(rich::Act::ViewImages { index, pics, .. }) => {
                assert_eq!(*index, 3);
                assert_eq!(pics.len(), 4);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pasted_images_are_right_aligned_squares_fetched_to_cover() {
        let images = vec![ImageAttachment {
            media_type: "image/jpeg".into(),
            base64: String::new(),
            size_bytes: 300_000,
            image_id: Some("u1".into()),
            width: Some(1000),
            height: Some(500),
        }];
        let strip = Strip::of_user(&images).unwrap();
        let laid = lay_strip(&strip, &ImageStore::default(), Some("c"));
        let rect = laid.hits[0].0;
        assert_eq!((rect.w, rect.h), (USER_THUMB, USER_THUMB));
        assert_eq!(rect.x + rect.w, W - TRANSCRIPT_PAD);
        // Its preview covers the square: 466 × 233.
        assert_eq!((laid.images[0].width, laid.images[0].height), (466, 233));
        assert_eq!(laid.images[0].conv.as_deref(), Some("c"));
    }

    #[test]
    fn images_with_neither_id_nor_bytes_are_counted() {
        let images = vec![ImageAttachment::default(), ImageAttachment::default()];
        let strip = Strip::of_user(&images).unwrap();
        let laid = lay_strip(&strip, &ImageStore::default(), None);
        assert!(laid.hits.is_empty());
        assert!(
            laid.ops
                .iter()
                .any(|op| matches!(op, Op::Text { text, .. } if text == "2 image attachments"))
        );
    }
}
