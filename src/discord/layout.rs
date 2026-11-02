//! Laying out a message as Discord's mobile app draws it: its markdown
//! flowed into lines (mixed weights, inline code, mention pills, custom
//! emoji, jumbo emoji when that is all there is), quotes, code blocks,
//! headings and lists; then embeds, attachments, stickers and reactions
//! under it. A layout is pure geometry relative to the message's text
//! origin, made once per message, width and grouping and drawn every frame
//! from cache, so scrolling only draws.

#![allow(dead_code)]

use std::rc::Rc;
use std::time::Instant;

use crate::gfx::{Canvas, RectF, Rgb};
use crate::social::media::{Fit, Media};
use crate::social::proto;
use crate::text::{self, Weight};

use super::markdown::{self, Block, Kind, Span, Style};
use super::model::{Embed, Entry, MessageExt};
use super::theme::{self, dp};
use super::ui;

/// How a run of text is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub weight: Weight,
    pub size: f32,
    pub color: Rgb,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fill {
    None,
    /// A pill behind the text (a mention, inline code, a timestamp).
    Pill(Rgb, f32),
}

#[derive(Debug, Clone)]
pub enum Item {
    Text {
        x: f32,
        w: f32,
        text: String,
        paint: Paint,
        fill: Fill,
        link: Option<Rc<str>>,
        spoiler: u16,
    },
    Emoji {
        x: f32,
        size: f32,
        media: Option<proto::Media>,
        fallback: String,
        spoiler: u16,
    },
}

#[derive(Debug, Clone)]
pub struct Line {
    pub y: f32,
    pub height: f32,
    pub baseline: f32,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Decor {
    QuoteBar { x: f32, y0: f32, y1: f32 },
    Code(RectF),
}

/// Flowed text: lines and what is drawn behind them.
#[derive(Debug, Clone, Default)]
pub struct Flow {
    pub lines: Vec<Line>,
    pub decor: Vec<Decor>,
    pub height: f32,
    pub width: f32,
}

/// Names for the text's tokens.
pub struct Names<'a> {
    pub ext: &'a MessageExt,
    pub channel: &'a dyn Fn(&str) -> Option<String>,
}

fn weight_of(style: Style, heading: bool) -> Weight {
    if style.has(Style::BOLD) || heading {
        Weight::Bold
    } else {
        Weight::Regular
    }
}

struct Flower<'a> {
    names: &'a Names<'a>,
    width: f32,
    lines: Vec<Line>,
    items: Vec<Item>,
    x: f32,
    left: f32,
    y: f32,
    line_h: f32,
    max_size: f32,
    used: f32,
    jumbo: bool,
}

impl Flower<'_> {
    fn break_line(&mut self, min_h: f32) {
        let height = self.line_h.max(min_h);
        let baseline =
            text::centered_baseline(self.y + height / 2.0, Weight::Regular, self.max_size)
                - dp(0.5);
        let items = std::mem::take(&mut self.items);
        if let Some(right) = items
            .iter()
            .map(|item| match item {
                Item::Text { x, w, .. } => x + w,
                Item::Emoji { x, size, .. } => x + size,
            })
            .reduce(f32::max)
        {
            self.used = self.used.max(right);
        }
        self.lines.push(Line {
            y: self.y,
            height,
            baseline,
            items,
        });
        self.y += height;
        self.x = self.left;
        self.line_h = 0.0;
        self.max_size = 0.0;
    }

    fn room(&self) -> f32 {
        self.left + self.width - self.x
    }

    fn push_text(
        &mut self,
        text: &str,
        paint: Paint,
        fill: Fill,
        link: Option<Rc<str>>,
        spoiler: u16,
        line_h: f32,
    ) {
        let pad = if matches!(fill, Fill::Pill(..)) {
            dp(2.0)
        } else {
            0.0
        };
        let measure = |s: &str| {
            text::measure(s, paint.weight, paint.size)
                + if paint.italic { paint.size * 0.08 } else { 0.0 }
        };
        // A pill (a mention, a timestamp, inline code) stays whole when it fits a line.
        if pad > 0.0 {
            let w = measure(text) + pad * 2.0;
            if w <= self.width {
                if w > self.room() && self.x > self.left + 0.5 {
                    self.break_line(0.0);
                }
                self.place(text.to_owned(), w, paint, fill, link, spoiler, line_h);
                return;
            }
        }
        // Words and the spaces after them; a word wider than a line breaks anywhere.
        let mut rest = text;
        while !rest.is_empty() {
            let end = match rest.find(char::is_whitespace) {
                Some(0) => rest
                    .find(|c: char| !c.is_whitespace())
                    .unwrap_or(rest.len()),
                Some(at) => at,
                None => rest.len(),
            };
            let piece = &rest[..end];
            rest = &rest[end..];
            let space = piece.starts_with(char::is_whitespace);
            if space && self.x <= self.left + 0.5 && !self.lines.is_empty() {
                continue;
            }
            let mut w = measure(piece) + pad * 2.0;
            if !space && w > self.room() && self.x > self.left + 0.5 {
                self.break_line(0.0);
            }
            if !space && w > self.width {
                // Too long for any line: as many characters as fit, again and again.
                let mut chunk = String::new();
                for c in piece.chars() {
                    let mut next = chunk.clone();
                    next.push(c);
                    if measure(&next) + pad * 2.0 > self.room() && !chunk.is_empty() {
                        let cw = measure(&chunk) + pad * 2.0;
                        self.place(
                            std::mem::take(&mut chunk),
                            cw,
                            paint,
                            fill,
                            link.clone(),
                            spoiler,
                            line_h,
                        );
                        self.break_line(0.0);
                        next = c.to_string();
                    }
                    chunk = next;
                }
                if !chunk.is_empty() {
                    w = measure(&chunk) + pad * 2.0;
                    self.place(chunk, w, paint, fill, link.clone(), spoiler, line_h);
                }
                continue;
            }
            if space && w > self.room() {
                self.break_line(0.0);
                continue;
            }
            self.place(
                piece.to_owned(),
                w,
                paint,
                fill,
                link.clone(),
                spoiler,
                line_h,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        text: String,
        w: f32,
        paint: Paint,
        fill: Fill,
        link: Option<Rc<str>>,
        spoiler: u16,
        line_h: f32,
    ) {
        // Runs of one paint join (fewer draw calls).
        if let Some(Item::Text {
            x,
            w: last_w,
            text: last_text,
            paint: last_paint,
            fill: last_fill,
            link: last_link,
            spoiler: last_spoiler,
        }) = self.items.last_mut()
            && *last_paint == paint
            && *last_fill == fill
            && fill == Fill::None
            && *last_link == link
            && *last_spoiler == spoiler
            && (*x + *last_w - self.x).abs() < 0.5
        {
            last_text.push_str(&text);
            *last_w += w;
        } else {
            self.items.push(Item::Text {
                x: self.x,
                w,
                text,
                paint,
                fill,
                link,
                spoiler,
            });
        }
        self.x += w;
        self.line_h = self.line_h.max(line_h);
        self.max_size = self.max_size.max(paint.size);
    }

    fn push_emoji(
        &mut self,
        media: Option<proto::Media>,
        fallback: String,
        size: f32,
        spoiler: u16,
        line_h: f32,
    ) {
        let w = size + dp(2.0);
        if w > self.room() && self.x > self.left + 0.5 {
            self.break_line(0.0);
        }
        self.items.push(Item::Emoji {
            x: self.x + dp(1.0),
            size,
            media,
            fallback,
            spoiler,
        });
        self.x += w;
        self.line_h = self.line_h.max(line_h.max(size + dp(4.0)));
        self.max_size = self.max_size.max(if self.jumbo {
            size * 0.6
        } else {
            theme::T_BODY
        });
    }

    fn spans(&mut self, spans: &[Span], base: Paint, line_h: f32, heading: bool) {
        let names = self.names;
        let emoji_size = if self.jumbo { dp(48.0) } else { dp(22.0) };
        for span in spans {
            let mut paint = Paint {
                weight: weight_of(span.style, heading),
                italic: span.style.has(Style::ITALIC),
                underline: span.style.has(Style::UNDERLINE),
                strike: span.style.has(Style::STRIKE),
                ..base
            };
            match &span.kind {
                Kind::Break => self.break_line(line_h),
                Kind::Text => {
                    let text = ui::emoji_presentation(&span.text);
                    if self.jumbo {
                        // Unicode emoji draw big too.
                        paint.size = dp(40.0);
                        self.push_text(&text, paint, Fill::None, None, span.spoiler, dp(54.0));
                    } else {
                        self.push_text(&text, paint, Fill::None, None, span.spoiler, line_h);
                    }
                }
                Kind::Code => {
                    let code = Paint {
                        weight: Weight::Mono,
                        size: base.size * 0.86,
                        italic: false,
                        ..paint
                    };
                    self.push_text(
                        &span.text,
                        code,
                        Fill::Pill(theme::CODE_BG, dp(4.0)),
                        None,
                        span.spoiler,
                        line_h,
                    );
                }
                Kind::Link(url) => {
                    paint.color = theme::LINK;
                    self.push_text(
                        &span.text,
                        paint,
                        Fill::None,
                        Some(url.clone()),
                        span.spoiler,
                        line_h,
                    );
                }
                Kind::User(id) => {
                    let name = names
                        .ext
                        .users
                        .get(&**id)
                        .cloned()
                        .unwrap_or_else(|| "unknown-user".into());
                    let pill = Paint {
                        color: theme::MENTION_TEXT,
                        weight: Weight::Medium,
                        ..paint
                    };
                    self.push_text(
                        &format!("@{name}"),
                        pill,
                        Fill::Pill(theme::MENTION_PILL, dp(3.0)),
                        None,
                        span.spoiler,
                        line_h,
                    );
                }
                Kind::Role(id) => {
                    let role = names.ext.roles.get(&**id);
                    let name = role.map_or("deleted-role", |role| role.name.as_str());
                    let color = role.and_then(|role| role.color).filter(|c| *c != 0);
                    let (text_color, pill) = match color {
                        Some(c) => (c, gfx_mix(c, base_bg(), 0.16)),
                        None => (theme::MENTION_TEXT, theme::MENTION_PILL),
                    };
                    let paint = Paint {
                        color: text_color,
                        weight: Weight::Medium,
                        ..paint
                    };
                    self.push_text(
                        &format!("@{name}"),
                        paint,
                        Fill::Pill(pill, dp(3.0)),
                        None,
                        span.spoiler,
                        line_h,
                    );
                }
                Kind::Channel(id) => {
                    let name = names
                        .ext
                        .channels
                        .get(&**id)
                        .cloned()
                        .or_else(|| (names.channel)(id))
                        .unwrap_or_else(|| "unknown".into());
                    let pill = Paint {
                        color: theme::MENTION_TEXT,
                        weight: Weight::Medium,
                        ..paint
                    };
                    self.push_text(
                        &format!("#{name}"),
                        pill,
                        Fill::Pill(theme::MENTION_PILL, dp(3.0)),
                        None,
                        span.spoiler,
                        line_h,
                    );
                }
                Kind::Everyone => {
                    let pill = Paint {
                        color: theme::MENTION_TEXT,
                        weight: Weight::Medium,
                        ..paint
                    };
                    self.push_text(
                        &span.text,
                        pill,
                        Fill::Pill(theme::MENTION_PILL, dp(3.0)),
                        None,
                        span.spoiler,
                        line_h,
                    );
                }
                Kind::Emoji { id, .. } => {
                    let media = names.ext.emoji.get(&**id).cloned();
                    self.push_emoji(media, span.text.clone(), emoji_size, span.spoiler, line_h);
                }
                Kind::Time(seconds, style) => {
                    let label = ui::timestamp(*seconds, *style);
                    self.push_text(
                        &label,
                        paint,
                        Fill::Pill(0x3b3d44, dp(3.0)),
                        None,
                        span.spoiler,
                        line_h,
                    );
                }
                Kind::Command => {
                    let pill = Paint {
                        color: theme::MENTION_TEXT,
                        weight: Weight::Medium,
                        ..paint
                    };
                    self.push_text(
                        &span.text,
                        pill,
                        Fill::Pill(theme::MENTION_PILL, dp(3.0)),
                        None,
                        span.spoiler,
                        line_h,
                    );
                }
            }
        }
        if !self.items.is_empty() || self.x > self.left {
            self.break_line(line_h);
        }
    }
}

fn base_bg() -> Rgb {
    theme::CHAT
}

pub fn gfx_mix(fg: Rgb, bg: Rgb, amount: f32) -> Rgb {
    crate::gfx::mix(bg, fg, amount)
}

fn body_paint() -> Paint {
    Paint {
        weight: Weight::Regular,
        size: theme::T_BODY,
        color: theme::TEXT,
        italic: false,
        underline: false,
        strike: false,
    }
}

/// Lays out markdown `blocks` at `width`.
pub fn flow(blocks: &[Block], width: f32, names: &Names, base: Paint, line_h: f32) -> Flow {
    let jumbo = markdown::only_emoji(blocks);
    let mut flower = Flower {
        names,
        width,
        lines: Vec::new(),
        items: Vec::new(),
        x: 0.0,
        left: 0.0,
        y: 0.0,
        line_h: 0.0,
        max_size: 0.0,
        used: 0.0,
        jumbo,
    };
    let mut decor = Vec::new();
    flow_blocks(&mut flower, &mut decor, blocks, base, line_h, 0.0);
    let height = flower.y;
    let width_used = flower.used;
    Flow {
        lines: flower.lines,
        decor,
        height,
        width: width_used,
    }
}

fn flow_blocks(
    flower: &mut Flower,
    decor: &mut Vec<Decor>,
    blocks: &[Block],
    base: Paint,
    line_h: f32,
    left: f32,
) {
    let full = flower.width;
    for (index, block) in blocks.iter().enumerate() {
        flower.left = left;
        flower.x = left;
        flower.width = full - left;
        match block {
            Block::Para(spans) => flower.spans(spans, base, line_h, false),
            Block::Heading(level, spans) => {
                let size = match level {
                    1 => theme::T_H1,
                    2 => theme::T_H2,
                    _ => theme::T_H3,
                };
                if index > 0 {
                    flower.y += dp(8.0);
                }
                let paint = Paint {
                    size,
                    color: theme::HEADER,
                    weight: Weight::Bold,
                    ..base
                };
                flower.spans(spans, paint, size * 1.3, true);
                flower.y += dp(2.0);
            }
            Block::Subtext(spans) => {
                let paint = Paint {
                    size: theme::T_TINY,
                    color: theme::MUTED,
                    ..base
                };
                flower.spans(spans, paint, theme::L_TINY, false);
            }
            Block::Quote(inner) => {
                let top = flower.y;
                let indent = left + dp(4.0) + dp(12.0);
                flow_blocks(flower, decor, inner, base, line_h, indent);
                decor.push(Decor::QuoteBar {
                    x: left,
                    y0: top,
                    y1: flower.y,
                });
                flower.left = left;
                flower.width = full - left;
            }
            Block::Code { text: body, .. } => {
                let pad = dp(8.0);
                let top = flower.y + dp(4.0);
                let inner = full - left - pad * 2.0;
                let paint = Paint {
                    weight: Weight::Mono,
                    size: theme::T_CODE,
                    color: theme::TEXT,
                    italic: false,
                    underline: false,
                    strike: false,
                };
                let mut y = top + pad;
                for line in body.split('\n') {
                    // Code wraps at the edge, character by character.
                    let mut rest = line.to_owned();
                    loop {
                        let mut cut = rest.len();
                        if text::measure(&rest, paint.weight, paint.size) > inner {
                            let mut width = 0.0;
                            for (at, c) in rest.char_indices() {
                                width += text::measure(
                                    c.encode_utf8(&mut [0; 4]),
                                    paint.weight,
                                    paint.size,
                                );
                                if width > inner {
                                    cut = at.max(c.len_utf8());
                                    break;
                                }
                            }
                        }
                        let (now, later) = rest.split_at(cut);
                        let baseline = text::centered_baseline(
                            y + theme::L_CODE / 2.0,
                            Weight::Mono,
                            paint.size,
                        );
                        let w = text::measure(now, paint.weight, paint.size);
                        flower.lines.push(Line {
                            y,
                            height: theme::L_CODE,
                            baseline,
                            items: vec![Item::Text {
                                x: left + pad,
                                w,
                                text: now.to_owned(),
                                paint,
                                fill: Fill::None,
                                link: None,
                                spoiler: 0,
                            }],
                        });
                        y += theme::L_CODE;
                        if later.is_empty() {
                            break;
                        }
                        rest = later.to_owned();
                    }
                }
                let rect = RectF {
                    x: left,
                    y: top,
                    w: full - left,
                    h: y + pad - top,
                };
                decor.push(Decor::Code(rect));
                flower.used = flower.used.max(full);
                flower.y = rect.y + rect.h + dp(4.0);
            }
            Block::Item {
                depth,
                number,
                spans,
            } => {
                let indent = left + dp(18.0) * (*depth as f32 + 1.0);
                let marker = match number {
                    Some(n) => format!("{n}."),
                    None => {
                        if *depth == 0 {
                            "•".to_owned()
                        } else {
                            "◦".to_owned()
                        }
                    }
                };
                let marker_w = text::measure(&marker, Weight::Regular, base.size);
                let line_start = flower.lines.len();
                flower.left = indent;
                flower.x = indent;
                flower.width = full - indent;
                flower.spans(spans, base, line_h, false);
                if let Some(first) = flower.lines.get_mut(line_start) {
                    first.items.insert(
                        0,
                        Item::Text {
                            x: indent - marker_w - dp(6.0),
                            w: marker_w,
                            text: marker,
                            paint: base,
                            fill: Fill::None,
                            link: None,
                            spoiler: 0,
                        },
                    );
                }
            }
        }
    }
    flower.left = left;
    flower.width = full;
}

// ---------------------------------------------------------------------------
// Whole messages

/// A box under the text.
#[derive(Debug, Clone)]
pub enum Piece {
    Picture {
        rect: RectF,
        media: proto::Media,
        /// Index in the message's media (the viewer opens it).
        index: usize,
        fit: Fit,
        play: bool,
        gif: bool,
    },
    File {
        rect: RectF,
        name: String,
        detail: String,
        audio: bool,
        index: usize,
    },
    Embed(Box<EmbedLayout>),
    Reaction {
        rect: RectF,
        emoji: String,
        media: Option<proto::Media>,
        count: u32,
        mine: bool,
    },
    /// "Started a thread: name, N messages ›".
    Thread {
        rect: RectF,
        id: String,
        name: String,
    },
    /// A Lottie sticker nothing here can draw: its name.
    StickerName {
        rect: RectF,
        name: String,
    },
}

#[derive(Debug, Clone)]
pub struct EmbedLayout {
    pub rect: RectF,
    pub bar: Rgb,
    pub provider: Option<(f32, String)>,
    pub author: Option<(f32, String, Option<proto::Media>)>,
    pub title: Option<(f32, Flow, Option<String>)>,
    pub description: Option<(f32, Flow)>,
    pub fields: Vec<(f32, f32, Flow, Flow)>,
    pub image: Option<(RectF, proto::Media, bool)>,
    pub thumbnail: Option<(RectF, proto::Media)>,
    pub footer: Option<(f32, String)>,
    /// Only the picture (a link to an image, a GIF): no card.
    pub bare: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MessageLayout {
    pub width: f32,
    pub header: bool,
    pub height: f32,
    /// The reply line above the header.
    pub reply: Option<(String, String, Rgb)>,
    pub name: String,
    pub name_color: Rgb,
    pub name_w: f32,
    pub time: String,
    pub bot: bool,
    /// Where the text starts (below the header when there is one).
    pub body_y: f32,
    pub body: Flow,
    pub edited: Option<(f32, f32)>,
    pub pieces: Vec<Piece>,
    pub system: bool,
    pub mentioned: bool,
}

pub const REPLY_H: f32 = dp(22.0);
pub const HEADER_LINE: f32 = dp(22.0);

/// The size of a picture of `w`x`h` in a box at most `max_w` x `max_h`.
pub fn fit_box(w: Option<u32>, h: Option<u32>, max_w: f32, max_h: f32) -> (f32, f32) {
    match (w, h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            // At most its own size in dp, as Discord draws it.
            let scale = (max_w / w as f32).min(max_h / h as f32).min(theme::DP);
            (
                (w as f32 * scale).round().max(dp(40.0)).min(max_w),
                (h as f32 * scale).round().max(dp(40.0)).min(max_h),
            )
        }
        _ => (max_w.min(dp(300.0)), dp(200.0).min(max_h)),
    }
}

/// Lays out `entry` with its text `width` px wide; `header` when it starts a group.
pub fn message(
    entry: &Entry,
    width: f32,
    header: bool,
    channel: &dyn Fn(&str) -> Option<String>,
) -> MessageLayout {
    let message = &entry.message;
    let ext = &entry.ext;
    let names = Names { ext, channel };
    let system = message.system;
    let mut y = 0.0;
    let reply = message.reply_to.as_ref().map(|reply| {
        let names_fn = |kind: &Kind| -> Option<String> {
            match kind {
                Kind::User(id) => ext.users.get(&**id).map(|n| format!("@{n}")),
                Kind::Channel(id) => ext.channels.get(&**id).map(|n| format!("#{n}")),
                Kind::Emoji { .. } => None,
                _ => None,
            }
        };
        let summary = markdown::plain(&reply.text, &names_fn);
        (
            reply
                .author
                .clone()
                .unwrap_or_else(|| "Original message was deleted".into()),
            summary,
            theme::name_color(ext.reply_color),
        )
    });
    let header = header || reply.is_some();
    if reply.is_some() {
        y += REPLY_H + dp(4.0);
    }
    let name = message.author.name.clone();
    let name_w = text::measure(&name, Weight::SemiBold, theme::T_BODY);
    if header && !system {
        y += HEADER_LINE + dp(2.0);
    }
    let body_y = y;
    let (body, edited) = if system {
        let paint = Paint {
            color: theme::MUTED,
            size: theme::T_BODY,
            ..body_paint()
        };
        let spans = vec![Span {
            text: message.text.clone(),
            style: Style::default(),
            kind: Kind::Text,
            spoiler: 0,
        }];
        (
            flow(&[Block::Para(spans)], width, &names, paint, theme::L_BODY),
            None,
        )
    } else if message.text.trim().is_empty() {
        (Flow::default(), None)
    } else {
        let blocks = markdown::parse(&message.text);
        let mut body = flow(&blocks, width, &names, body_paint(), theme::L_BODY);
        // "(edited)" after the last line, as Discord puts it.
        let edited = message.edited_ts.map(|_| {
            let label_w = text::measure("(edited)", Weight::Regular, theme::T_TINY);
            match body.lines.last_mut() {
                Some(last) => {
                    let right = last
                        .items
                        .iter()
                        .map(|item| match item {
                            Item::Text { x, w, .. } => x + w,
                            Item::Emoji { x, size, .. } => x + size,
                        })
                        .fold(0.0, f32::max);
                    if right + dp(4.0) + label_w <= width {
                        (right + dp(4.0), last.baseline)
                    } else {
                        let baseline = text::centered_baseline(
                            body.height + theme::L_TINY / 2.0,
                            Weight::Regular,
                            theme::T_TINY,
                        );
                        body.height += theme::L_TINY;
                        (0.0, baseline)
                    }
                }
                None => (
                    0.0,
                    text::centered_baseline(theme::L_TINY / 2.0, Weight::Regular, theme::T_TINY),
                ),
            }
        });
        (body, edited)
    };
    y += body.height;

    let mut pieces = Vec::new();
    let gap = dp(4.0);
    // Attachments: pictures one under another (two side by side when several), then files.
    let pictures: Vec<(usize, &proto::Media)> = message
        .media
        .iter()
        .enumerate()
        .filter(|(_, media)| matches!(media.kind.as_str(), "image" | "gif" | "video"))
        .collect();
    let max_w = width.min(dp(330.0));
    let max_h = dp(330.0);
    if pictures.len() == 1 {
        let (index, media) = pictures[0];
        let (w, h) = fit_box(media.w, media.h, max_w, max_h);
        y += gap;
        let shown = if media.kind == "video" {
            proto::Media {
                reference: media.thumb.clone().unwrap_or_default(),
                ..media.clone()
            }
        } else {
            media.clone()
        };
        pieces.push(Piece::Picture {
            rect: RectF { x: 0.0, y, w, h },
            media: shown,
            index,
            fit: Fit::Contain,
            play: media.kind == "video",
            gif: media.kind == "gif",
        });
        y += h;
    } else if pictures.len() > 1 {
        y += gap;
        let cell = (max_w - gap) / 2.0;
        for (n, (index, media)) in pictures.iter().enumerate() {
            let col = n % 2;
            let row = n / 2;
            let shown = if media.kind == "video" {
                proto::Media {
                    reference: media.thumb.clone().unwrap_or_default(),
                    ..(*media).clone()
                }
            } else {
                (*media).clone()
            };
            pieces.push(Piece::Picture {
                rect: RectF {
                    x: col as f32 * (cell + gap),
                    y: y + row as f32 * (cell + gap),
                    w: cell,
                    h: cell,
                },
                media: shown,
                index: *index,
                fit: Fit::Cover,
                play: media.kind == "video",
                gif: media.kind == "gif",
            });
        }
        let rows = pictures.len().div_ceil(2);
        y += rows as f32 * cell + (rows as f32 - 1.0) * gap;
    }
    for (index, media) in message.media.iter().enumerate() {
        match media.kind.as_str() {
            "sticker" => {
                let size = dp(160.0);
                y += gap;
                pieces.push(Piece::Picture {
                    rect: RectF {
                        x: 0.0,
                        y,
                        w: size,
                        h: size,
                    },
                    media: media.clone(),
                    index,
                    fit: Fit::Contain,
                    play: false,
                    gif: false,
                });
                y += size;
            }
            "file" | "audio" => {
                y += gap;
                let h = dp(58.0);
                let audio = media.kind == "audio";
                let detail = if audio {
                    media.duration_ms.map_or("Voice message".to_owned(), |ms| {
                        format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60)
                    })
                } else {
                    media.size.map(ui::file_size).unwrap_or_default()
                };
                pieces.push(Piece::File {
                    rect: RectF {
                        x: 0.0,
                        y,
                        w: width.min(dp(330.0)),
                        h,
                    },
                    name: media.name.clone().unwrap_or_else(|| {
                        if audio {
                            "Voice message".into()
                        } else {
                            "file".into()
                        }
                    }),
                    detail,
                    audio,
                    index,
                });
                y += h;
            }
            _ => {}
        }
    }
    for sticker in &ext.stickers {
        y += gap;
        let h = dp(28.0);
        pieces.push(Piece::StickerName {
            rect: RectF {
                x: 0.0,
                y,
                w: width.min(dp(200.0)),
                h,
            },
            name: sticker.clone(),
        });
        y += h;
    }
    for embed in &ext.embeds {
        let layout = embed_layout(embed, width, &names, y + gap);
        y = layout.rect.y + layout.rect.h;
        pieces.push(Piece::Embed(Box::new(layout)));
    }
    if let Some(thread) = &ext.thread {
        y += gap;
        let h = dp(44.0);
        pieces.push(Piece::Thread {
            rect: RectF {
                x: 0.0,
                y,
                w: width.min(dp(300.0)),
                h,
            },
            id: thread.id.clone(),
            name: thread.name.clone().unwrap_or_else(|| "Thread".into()),
        });
        y += h;
    }
    if !message.reactions.is_empty() {
        y += dp(6.0);
        let h = dp(28.0);
        let mut x = 0.0;
        for reaction in &message.reactions {
            let custom = reaction.emoji.starts_with('<');
            let media = if custom {
                reaction
                    .emoji
                    .rsplit(':')
                    .next()
                    .map(|id| id.trim_end_matches('>'))
                    .and_then(|id| ext.emoji.get(id))
                    .cloned()
            } else {
                None
            };
            let count = reaction.count.to_string();
            let emoji_w = if custom {
                dp(18.0)
            } else {
                text::measure(
                    &ui::emoji_presentation(&reaction.emoji),
                    Weight::Regular,
                    dp(16.0),
                )
            };
            let w = dp(8.0)
                + emoji_w
                + dp(6.0)
                + text::measure(&count, Weight::SemiBold, theme::T_SMALL)
                + dp(8.0);
            if x + w > width && x > 0.0 {
                x = 0.0;
                y += h + dp(4.0);
            }
            pieces.push(Piece::Reaction {
                rect: RectF { x, y, w, h },
                emoji: reaction.emoji.clone(),
                media,
                count: reaction.count,
                mine: reaction.mine,
            });
            x += w + dp(4.0);
        }
        y += h;
    }
    MessageLayout {
        width,
        header,
        height: y,
        reply,
        name,
        name_color: theme::name_color(ext.color),
        name_w,
        time: ui::message_time(message.ts),
        bot: ext.bot,
        body_y,
        body,
        edited,
        pieces,
        system,
        mentioned: ext.mentioned,
    }
}

fn embed_layout(embed: &Embed, width: f32, names: &Names, top: f32) -> EmbedLayout {
    let max = width.min(dp(340.0));
    let bare = matches!(embed.kind.as_deref(), Some("image") | Some("gifv"))
        && embed.title.is_none()
        && embed.description.is_none();
    if bare {
        let media = embed.thumbnail.clone().or_else(|| embed.image.clone());
        if let Some(media) = media {
            let (w, h) = fit_box(media.w, media.h, max, dp(300.0));
            let rect = RectF {
                x: 0.0,
                y: top,
                w,
                h,
            };
            return EmbedLayout {
                rect,
                bar: 0,
                provider: None,
                author: None,
                title: None,
                description: None,
                fields: Vec::new(),
                image: Some((rect, media, embed.kind.as_deref() == Some("gifv"))),
                thumbnail: None,
                footer: None,
                bare: true,
                url: embed.url.clone(),
            };
        }
    }
    let pad = dp(12.0);
    let bar_w = dp(4.0);
    let inner_x = bar_w + pad;
    let thumb_size = dp(80.0);
    let has_thumb = embed.thumbnail.is_some()
        && embed.kind.as_deref() != Some("video")
        && embed.kind.as_deref() != Some("gifv")
        && embed.image.is_none();
    let inner_w = max - inner_x - pad - if has_thumb { thumb_size + pad } else { 0.0 };
    let mut y = pad;
    let small = Paint {
        weight: Weight::Regular,
        size: theme::T_TINY,
        color: theme::MUTED,
        italic: false,
        underline: false,
        strike: false,
    };
    let mut provider = None;
    if let Some(name) = &embed.provider {
        provider = Some((y, ui::fit(name, Weight::Regular, theme::T_TINY, inner_w)));
        y += theme::L_TINY;
    }
    let mut author = None;
    if let Some(a) = &embed.author {
        author = Some((
            y,
            ui::fit(
                &a.name,
                Weight::SemiBold,
                theme::T_SMALL,
                inner_w - dp(26.0),
            ),
            a.icon.clone(),
        ));
        y += theme::L_SMALL + dp(2.0);
    }
    let mut title = None;
    if let Some(t) = &embed.title {
        let paint = Paint {
            weight: Weight::SemiBold,
            size: theme::T_BODY,
            color: if embed.url.is_some() {
                theme::LINK
            } else {
                theme::HEADER
            },
            ..small
        };
        let spans = markdown::parse_inline(t, &mut 0);
        let flowed = flow(&[Block::Para(spans)], inner_w, names, paint, theme::L_BODY);
        let h = flowed.height;
        title = Some((y, flowed, embed.url.clone()));
        y += h + dp(2.0);
    }
    let mut description = None;
    if let Some(d) = &embed.description {
        let paint = Paint {
            size: theme::T_SMALL,
            color: theme::TEXT,
            ..small
        };
        let flowed = flow(&markdown::parse(d), inner_w, names, paint, theme::L_SMALL);
        let h = flowed.height;
        description = Some((y, flowed));
        y += h + dp(4.0);
    }
    let mut fields = Vec::new();
    let mut column = 0;
    let mut row_y = y;
    let mut row_h: f32 = 0.0;
    for field in &embed.fields {
        let inline = field.inline;
        let columns = 2;
        let col_w = if inline {
            (inner_w - pad) / columns as f32
        } else {
            inner_w
        };
        if !inline || column >= columns {
            if column > 0 {
                row_y += row_h + dp(6.0);
            }
            column = 0;
            row_h = 0.0;
        }
        let x = inner_x + column as f32 * (col_w + pad);
        let name_paint = Paint {
            weight: Weight::SemiBold,
            size: theme::T_SMALL,
            color: theme::HEADER,
            ..small
        };
        let value_paint = Paint {
            size: theme::T_SMALL,
            color: theme::TEXT,
            ..small
        };
        let name = flow(
            &[Block::Para(markdown::parse_inline(&field.name, &mut 0))],
            col_w,
            names,
            name_paint,
            theme::L_SMALL,
        );
        let value = flow(
            &markdown::parse(&field.value),
            col_w,
            names,
            value_paint,
            theme::L_SMALL,
        );
        row_h = row_h.max(name.height + value.height);
        fields.push((x, row_y, name, value));
        if inline {
            column += 1;
        } else {
            row_y += row_h + dp(6.0);
            row_h = 0.0;
        }
    }
    if !embed.fields.is_empty() {
        y = row_y + if column > 0 { row_h + dp(6.0) } else { 0.0 };
    }
    let mut image = None;
    let media = embed.image.clone().or_else(|| {
        if matches!(embed.kind.as_deref(), Some("video") | Some("gifv")) {
            embed.thumbnail.clone()
        } else {
            None
        }
    });
    if let Some(media) = media {
        let (w, h) = fit_box(media.w, media.h, max - inner_x - pad, dp(260.0));
        y += dp(4.0);
        let play = embed.kind.as_deref() == Some("video") || embed.video.is_some();
        image = Some((
            RectF {
                x: inner_x,
                y,
                w,
                h,
            },
            media,
            play,
        ));
        y += h + dp(4.0);
    }
    let mut footer = None;
    if let Some(f) = &embed.footer {
        let mut label = f.text.clone();
        if let Some(ts) = embed.timestamp {
            label = format!("{label} • {}", ui::message_time(ts));
        }
        footer = Some((
            y + dp(4.0),
            ui::fit(&label, Weight::Regular, theme::T_TINY, inner_w),
        ));
        y += theme::L_TINY + dp(4.0);
    } else if let Some(ts) = embed.timestamp {
        footer = Some((y + dp(4.0), ui::message_time(ts)));
        y += theme::L_TINY + dp(4.0);
    }
    let mut thumbnail = None;
    if has_thumb && let Some(media) = embed.thumbnail.clone() {
        thumbnail = Some((
            RectF {
                x: max - pad - thumb_size,
                y: pad,
                w: thumb_size,
                h: thumb_size,
            },
            media,
        ));
        y = y.max(pad + thumb_size);
    }
    y += pad;
    EmbedLayout {
        rect: RectF {
            x: 0.0,
            y: top,
            w: max,
            h: y,
        },
        bar: embed.color.filter(|c| *c != 0).unwrap_or(0x202225),
        provider,
        author,
        title,
        description,
        fields,
        image,
        thumbnail,
        footer,
        bare: false,
        url: embed.url.clone(),
    }
}

// ---------------------------------------------------------------------------
// Drawing

/// What a draw needs besides the layout.
pub struct DrawEnv<'a> {
    pub media: &'a mut Media,
    pub now: Instant,
    /// The background behind the message (rounded corners blend into it).
    pub background: Rgb,
    /// Revealed spoilers of this message.
    pub revealed: &'a dyn Fn(u16) -> bool,
}

pub fn draw_flow(canvas: &mut Canvas, flow: &Flow, x0: f32, y0: f32, env: &mut DrawEnv) {
    let (clip_top, clip_bottom) = (canvas.clip().1 as f32, canvas.clip().3 as f32);
    for decor in &flow.decor {
        match decor {
            Decor::QuoteBar { x, y0: a, y1: b } => {
                canvas.fill_rrect(
                    RectF {
                        x: x0 + x,
                        y: y0 + a,
                        w: dp(4.0),
                        h: b - a,
                    },
                    dp(2.0),
                    theme::QUOTE_BAR,
                    1.0,
                );
            }
            Decor::Code(rect) => {
                let r = RectF {
                    x: x0 + rect.x,
                    y: y0 + rect.y,
                    ..*rect
                };
                if r.y > clip_bottom || r.y + r.h < clip_top {
                    continue;
                }
                canvas.fill_rrect(r, dp(4.0), theme::CODE_BORDER, 1.0);
                canvas.fill_rrect(r.inset(dp(1.0)), dp(3.5), theme::CODE_BG, 1.0);
            }
        }
    }
    for line in &flow.lines {
        let top = y0 + line.y;
        if top > clip_bottom || top + line.height < clip_top {
            continue;
        }
        let baseline = y0 + line.baseline;
        for item in &line.items {
            match item {
                Item::Text {
                    x,
                    w,
                    text: value,
                    paint,
                    fill,
                    link: _,
                    spoiler,
                } => {
                    let x = x0 + x;
                    let hidden = *spoiler > 0 && !(env.revealed)(*spoiler);
                    if *spoiler > 0 {
                        let rect = RectF {
                            x: x - dp(1.0),
                            y: top + dp(2.0),
                            w: w + dp(2.0),
                            h: line.height - dp(4.0),
                        };
                        canvas.fill_rrect(
                            rect,
                            dp(3.0),
                            if hidden { theme::SPOILER } else { 0x45474e },
                            1.0,
                        );
                        if hidden {
                            continue;
                        }
                    }
                    if let Fill::Pill(color, radius) = fill {
                        let h = paint.size * 1.3;
                        let rect = RectF {
                            x,
                            y: baseline - paint.size * 0.98,
                            w: *w,
                            h,
                        };
                        canvas.fill_rrect(rect, *radius, *color, 1.0);
                    }
                    let tx = if matches!(fill, Fill::Pill(..)) {
                        x + dp(2.0)
                    } else {
                        x
                    };
                    if paint.italic {
                        ui::draw_italic(
                            canvas,
                            value,
                            tx,
                            baseline,
                            paint.weight,
                            paint.size,
                            paint.color,
                            1.0,
                        );
                    } else {
                        text::draw(
                            canvas,
                            value,
                            tx,
                            baseline,
                            paint.weight,
                            paint.size,
                            paint.color,
                            1.0,
                        );
                    }
                    if paint.underline {
                        canvas.fill_rect(
                            tx as i32,
                            (baseline + paint.size * 0.12) as i32,
                            (*w) as i32,
                            (paint.size * 0.07).max(2.0) as i32,
                            paint.color,
                            1.0,
                        );
                    }
                    if paint.strike {
                        canvas.fill_rect(
                            tx as i32,
                            (baseline - paint.size * 0.3) as i32,
                            (*w) as i32,
                            (paint.size * 0.07).max(2.0) as i32,
                            paint.color,
                            1.0,
                        );
                    }
                }
                Item::Emoji {
                    x,
                    size,
                    media,
                    fallback,
                    spoiler,
                } => {
                    let hidden = *spoiler > 0 && !(env.revealed)(*spoiler);
                    let ex = x0 + x;
                    let ey = top + (line.height - size) / 2.0;
                    if hidden {
                        canvas.fill_rrect(
                            RectF {
                                x: ex,
                                y: ey,
                                w: *size,
                                h: *size,
                            },
                            dp(3.0),
                            theme::SPOILER,
                            1.0,
                        );
                        continue;
                    }
                    let px = size.round() as u32;
                    let picture = media.as_ref().and_then(|media| {
                        ui::get(env.media, &media.reference, px, px, Fit::Contain, env.now)
                    });
                    match picture {
                        Some(image) => {
                            let ox = (px as i32 - image.width) / 2;
                            let oy = (px as i32 - image.height) / 2;
                            canvas.draw_image(
                                &image,
                                ex.round() as i32 + ox,
                                ey.round() as i32 + oy,
                                1.0,
                            );
                        }
                        None => {
                            let label =
                                ui::fit(fallback, Weight::Regular, theme::T_SMALL, *size * 3.0);
                            if media.is_some() {
                                canvas.fill_rrect(
                                    RectF {
                                        x: ex,
                                        y: ey,
                                        w: *size,
                                        h: *size,
                                    },
                                    dp(4.0),
                                    0x3b3d44,
                                    1.0,
                                );
                            } else {
                                text::draw(
                                    canvas,
                                    &label,
                                    ex,
                                    baseline,
                                    Weight::Regular,
                                    theme::T_SMALL,
                                    theme::MUTED,
                                    1.0,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn picture(
    canvas: &mut Canvas,
    rect: RectF,
    media: &proto::Media,
    fit: Fit,
    env: &mut DrawEnv,
    radius: f32,
) {
    let (w, h) = (rect.w.round() as u32, rect.h.round() as u32);
    match ui::get(env.media, &media.reference, w, h, fit, env.now) {
        Some(image) => {
            let x = rect.x + (rect.w - image.width as f32) / 2.0;
            let y = rect.y + (rect.h - image.height as f32) / 2.0;
            canvas.draw_image(&image, x.round() as i32, y.round() as i32, 1.0);
            if radius > 0.0 {
                let drawn = RectF {
                    x: x.round(),
                    y: y.round(),
                    w: image.width as f32,
                    h: image.height as f32,
                };
                ui::round_corners(canvas, drawn, radius, env.background);
            }
        }
        None => canvas.fill_rrect(rect, radius, 0x2b2d31, 1.0),
    }
}

/// Draws the pieces under a message's text, its origin at (`x0`, `y0`).
pub fn draw_pieces(
    canvas: &mut Canvas,
    layout: &MessageLayout,
    x0: f32,
    y0: f32,
    env: &mut DrawEnv,
) {
    let (clip_top, clip_bottom) = (canvas.clip().1 as f32, canvas.clip().3 as f32);
    for piece in &layout.pieces {
        let bounds = piece_rect(piece);
        if y0 + bounds.y > clip_bottom || y0 + bounds.y + bounds.h < clip_top {
            continue;
        }
        match piece {
            Piece::Picture {
                rect,
                media,
                fit,
                play,
                gif,
                ..
            } => {
                let r = RectF {
                    x: x0 + rect.x,
                    y: y0 + rect.y,
                    ..*rect
                };
                picture(canvas, r, media, *fit, env, dp(8.0));
                if *play {
                    canvas.fill_circle(r.x + r.w / 2.0, r.y + r.h / 2.0, dp(24.0), 0x000000, 0.6);
                    ui::glyph(
                        canvas,
                        ui::Glyph::PlayDisc,
                        r.x + r.w / 2.0 + dp(1.5),
                        r.y + r.h / 2.0,
                        dp(30.0),
                        theme::WHITE,
                        1.0,
                    );
                }
                if *gif {
                    gif_tag(canvas, r);
                }
            }
            Piece::File {
                rect,
                name,
                detail,
                audio,
                ..
            } => {
                let r = RectF {
                    x: x0 + rect.x,
                    y: y0 + rect.y,
                    ..*rect
                };
                canvas.fill_rrect(r, dp(8.0), theme::CODE_BORDER, 1.0);
                canvas.fill_rrect(r.inset(dp(1.0)), dp(7.5), theme::LIST, 1.0);
                let icon = if *audio {
                    crate::exocortex::ui::icons::Icon::Play
                } else {
                    crate::exocortex::ui::icons::Icon::FileText
                };
                if *audio {
                    canvas.fill_circle(
                        r.x + dp(28.0),
                        r.y + r.h / 2.0,
                        dp(16.0),
                        theme::BLURPLE,
                        1.0,
                    );
                }
                crate::exocortex::ui::icons::draw(
                    canvas,
                    icon,
                    r.x + dp(28.0),
                    r.y + r.h / 2.0,
                    dp(22.0),
                    if *audio {
                        theme::WHITE
                    } else {
                        theme::INTERACTIVE
                    },
                    1.0,
                );
                let tx = r.x + dp(52.0);
                let name = ui::fit(name, Weight::Medium, theme::T_SMALL, r.w - dp(64.0));
                text::draw(
                    canvas,
                    &name,
                    tx,
                    r.y + r.h / 2.0 - dp(3.0),
                    Weight::Medium,
                    theme::T_SMALL,
                    if *audio { theme::HEADER } else { theme::LINK },
                    1.0,
                );
                text::draw(
                    canvas,
                    detail,
                    tx,
                    r.y + r.h / 2.0 + dp(14.0),
                    Weight::Regular,
                    theme::T_TINY,
                    theme::MUTED,
                    1.0,
                );
            }
            Piece::Embed(embed) => draw_embed(canvas, embed, x0, y0, env),
            Piece::Reaction {
                rect,
                emoji,
                media,
                count,
                mine,
            } => {
                let r = RectF {
                    x: x0 + rect.x,
                    y: y0 + rect.y,
                    ..*rect
                };
                if *mine {
                    canvas.fill_rrect(r, dp(8.0), theme::BLURPLE, 1.0);
                    canvas.fill_rrect(r.inset(dp(1.0)), dp(7.5), theme::REACTION_MINE, 1.0);
                } else {
                    canvas.fill_rrect(r, dp(8.0), theme::REACTION, 1.0);
                }
                let ex = r.x + dp(8.0);
                let cy = r.y + r.h / 2.0;
                let count_x = if emoji.starts_with('<') {
                    let size = dp(18.0);
                    let px = size.round() as u32;
                    match media.as_ref().and_then(|m| {
                        ui::get(env.media, &m.reference, px, px, Fit::Contain, env.now)
                    }) {
                        Some(image) => canvas.draw_image(
                            &image,
                            ex.round() as i32,
                            (cy - size / 2.0).round() as i32,
                            1.0,
                        ),
                        None => canvas.fill_rrect(
                            RectF {
                                x: ex,
                                y: cy - size / 2.0,
                                w: size,
                                h: size,
                            },
                            dp(4.0),
                            0x3b3d44,
                            1.0,
                        ),
                    }
                    ex + size + dp(6.0)
                } else {
                    let size = dp(16.0);
                    let w = text::draw(
                        canvas,
                        &ui::emoji_presentation(emoji),
                        ex,
                        text::centered_baseline(cy, Weight::Regular, size),
                        Weight::Regular,
                        size,
                        theme::TEXT,
                        1.0,
                    );
                    ex + w + dp(6.0)
                };
                let color = if *mine { 0xdee0fc } else { theme::INTERACTIVE };
                text::draw(
                    canvas,
                    &count.to_string(),
                    count_x,
                    text::centered_baseline(cy, Weight::SemiBold, theme::T_SMALL),
                    Weight::SemiBold,
                    theme::T_SMALL,
                    color,
                    1.0,
                );
            }
            Piece::Thread { rect, name, .. } => {
                let r = RectF {
                    x: x0 + rect.x,
                    y: y0 + rect.y,
                    ..*rect
                };
                canvas.fill_rrect(r, dp(8.0), theme::LIST, 1.0);
                crate::exocortex::ui::icons::draw(
                    canvas,
                    crate::exocortex::ui::icons::Icon::MessageSquare,
                    r.x + dp(20.0),
                    r.y + r.h / 2.0,
                    dp(18.0),
                    theme::INTERACTIVE,
                    1.0,
                );
                let label = ui::fit(name, Weight::SemiBold, theme::T_SMALL, r.w - dp(56.0));
                text::draw(
                    canvas,
                    &label,
                    r.x + dp(38.0),
                    text::centered_baseline(r.y + r.h / 2.0, Weight::SemiBold, theme::T_SMALL),
                    Weight::SemiBold,
                    theme::T_SMALL,
                    theme::HEADER,
                    1.0,
                );
                crate::exocortex::ui::icons::draw(
                    canvas,
                    crate::exocortex::ui::icons::Icon::ChevronRight,
                    r.x + r.w - dp(14.0),
                    r.y + r.h / 2.0,
                    dp(16.0),
                    theme::MUTED,
                    1.0,
                );
            }
            Piece::StickerName { rect, name } => {
                let r = RectF {
                    x: x0 + rect.x,
                    y: y0 + rect.y,
                    ..*rect
                };
                let label = ui::fit(
                    &format!("Sticker: {name}"),
                    Weight::Medium,
                    theme::T_SMALL,
                    r.w,
                );
                text::draw(
                    canvas,
                    &label,
                    r.x,
                    text::centered_baseline(r.y + r.h / 2.0, Weight::Medium, theme::T_SMALL),
                    Weight::Medium,
                    theme::T_SMALL,
                    theme::MUTED,
                    1.0,
                );
            }
        }
    }
}

fn gif_tag(canvas: &mut Canvas, r: RectF) {
    let tag = RectF {
        x: r.x + dp(6.0),
        y: r.y + dp(6.0),
        w: dp(30.0),
        h: dp(18.0),
    };
    canvas.fill_rrect(tag, dp(4.0), 0x000000, 0.65);
    text::draw_centered(
        canvas,
        "GIF",
        tag.x + tag.w / 2.0,
        text::centered_baseline(tag.y + tag.h / 2.0, Weight::Bold, theme::T_TINY * 0.85),
        Weight::Bold,
        theme::T_TINY * 0.85,
        theme::WHITE,
        1.0,
    );
}

pub fn piece_rect(piece: &Piece) -> RectF {
    match piece {
        Piece::Picture { rect, .. }
        | Piece::File { rect, .. }
        | Piece::Reaction { rect, .. }
        | Piece::Thread { rect, .. }
        | Piece::StickerName { rect, .. } => *rect,
        Piece::Embed(embed) => embed.rect,
    }
}

fn draw_embed(canvas: &mut Canvas, embed: &EmbedLayout, x0: f32, y0: f32, env: &mut DrawEnv) {
    let r = RectF {
        x: x0 + embed.rect.x,
        y: y0 + embed.rect.y,
        ..embed.rect
    };
    if embed.bare {
        if let Some((rect, media, gif)) = &embed.image {
            let rr = RectF {
                x: x0 + rect.x,
                y: y0 + rect.y,
                ..*rect
            };
            picture(canvas, rr, media, Fit::Contain, env, dp(8.0));
            if *gif {
                gif_tag(canvas, rr);
            }
        }
        return;
    }
    canvas.fill_rrect(r, dp(4.0), embed.bar, 1.0);
    canvas.fill_rrect(
        RectF {
            x: r.x + dp(4.0),
            y: r.y,
            w: r.w - dp(4.0),
            h: r.h,
        },
        dp(4.0),
        theme::LIST,
        1.0,
    );
    canvas.fill_rect(
        (r.x + dp(4.0)) as i32,
        r.y as i32,
        dp(4.0) as i32,
        r.h as i32,
        theme::LIST,
        1.0,
    );
    let ix = r.x + dp(16.0);
    if let Some((y, name)) = &embed.provider {
        text::draw(
            canvas,
            name,
            ix,
            text::centered_baseline(
                r.y + y + theme::L_TINY / 2.0,
                Weight::Regular,
                theme::T_TINY,
            ),
            Weight::Regular,
            theme::T_TINY,
            theme::MUTED,
            1.0,
        );
    }
    if let Some((y, name, icon)) = &embed.author {
        let cy = r.y + y + theme::L_SMALL / 2.0;
        let mut tx = ix;
        if let Some(icon) = icon {
            ui::avatar(
                canvas,
                env.media,
                Some(icon),
                ix,
                cy - dp(10.0),
                dp(20.0),
                0x3b3d44,
                env.now,
            );
            tx += dp(26.0);
        }
        text::draw(
            canvas,
            name,
            tx,
            text::centered_baseline(cy, Weight::SemiBold, theme::T_SMALL),
            Weight::SemiBold,
            theme::T_SMALL,
            theme::HEADER,
            1.0,
        );
    }
    let mut env2 = DrawEnv {
        media: env.media,
        now: env.now,
        background: theme::LIST,
        revealed: env.revealed,
    };
    if let Some((y, flow, _)) = &embed.title {
        draw_flow(canvas, flow, ix, r.y + y, &mut env2);
    }
    if let Some((y, flow)) = &embed.description {
        draw_flow(canvas, flow, ix, r.y + y, &mut env2);
    }
    for (x, y, name, value) in &embed.fields {
        draw_flow(canvas, name, r.x + x, r.y + y, &mut env2);
        draw_flow(canvas, value, r.x + x, r.y + y + name.height, &mut env2);
    }
    if let Some((rect, media, play)) = &embed.image {
        let rr = RectF {
            x: r.x + rect.x,
            y: r.y + rect.y,
            ..*rect
        };
        picture(canvas, rr, media, Fit::Contain, &mut env2, dp(4.0));
        if *play {
            canvas.fill_circle(
                rr.x + rr.w / 2.0,
                rr.y + rr.h / 2.0,
                dp(24.0),
                0x000000,
                0.6,
            );
            ui::glyph(
                canvas,
                ui::Glyph::PlayDisc,
                rr.x + rr.w / 2.0 + dp(1.5),
                rr.y + rr.h / 2.0,
                dp(30.0),
                theme::WHITE,
                1.0,
            );
        }
    }
    if let Some((rect, media)) = &embed.thumbnail {
        let rr = RectF {
            x: r.x + rect.x,
            y: r.y + rect.y,
            ..*rect
        };
        picture(canvas, rr, media, Fit::Contain, &mut env2, dp(4.0));
    }
    if let Some((y, label)) = &embed.footer {
        text::draw(
            canvas,
            label,
            ix,
            text::centered_baseline(
                r.y + y + theme::L_TINY / 2.0,
                Weight::Regular,
                theme::T_TINY,
            ),
            Weight::Regular,
            theme::T_TINY,
            theme::MUTED,
            1.0,
        );
    }
}

/// What is under (`x`, `y`) of a message laid out at its text origin.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Link(String),
    Spoiler(u16),
    Picture(usize),
    File(usize),
    Reaction(String),
    Thread(String),
    EmbedImage(String),
}

pub fn hit(layout: &MessageLayout, x: f32, y: f32) -> Option<Hit> {
    let in_flow = |flow: &Flow, fx: f32, fy: f32| -> Option<Hit> {
        for line in &flow.lines {
            if fy < line.y || fy > line.y + line.height {
                continue;
            }
            for item in &line.items {
                if let Item::Text {
                    x,
                    w,
                    link,
                    spoiler,
                    ..
                } = item
                    && fx >= *x - dp(2.0)
                    && fx <= x + w + dp(2.0)
                {
                    if *spoiler > 0 {
                        return Some(Hit::Spoiler(*spoiler));
                    }
                    if let Some(url) = link {
                        return Some(Hit::Link(url.to_string()));
                    }
                }
            }
        }
        None
    };
    if let Some(found) = in_flow(&layout.body, x, y - layout.body_y) {
        return Some(found);
    }
    for piece in &layout.pieces {
        let rect = piece_rect(piece);
        if !rect.contains(x, y) {
            continue;
        }
        return match piece {
            Piece::Picture { index, .. } => Some(Hit::Picture(*index)),
            Piece::File { index, .. } => Some(Hit::File(*index)),
            Piece::Reaction { emoji, .. } => Some(Hit::Reaction(emoji.clone())),
            Piece::Thread { id, .. } => Some(Hit::Thread(id.clone())),
            Piece::Embed(embed) => {
                if let Some((title_y, flow, url)) = &embed.title
                    && let Some(url) = url
                    && y - rect.y >= *title_y
                    && y - rect.y <= title_y + flow.height
                {
                    return Some(Hit::Link(url.clone()));
                }
                if let Some((_, flow)) = &embed.description
                    && let Some(found) = in_flow(
                        flow,
                        x - dp(16.0),
                        y - rect.y - embed.description.as_ref().map_or(0.0, |d| d.0),
                    )
                {
                    return Some(found);
                }
                if embed.image.is_some() || embed.bare {
                    return embed.url.clone().map(Hit::EmbedImage);
                }
                None
            }
            Piece::StickerName { .. } => None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::proto::{Message, Reaction};

    #[test]
    fn a_message_lays_out_text_pictures_embeds_and_reactions() {
        let mut ext = MessageExt::default();
        ext.users.insert("7".into(), "Ana".into());
        ext.embeds.push(Embed {
            title: Some("A title".into()),
            description: Some("Some **bold** words".into()),
            color: Some(0x5865f2),
            ..Embed::default()
        });
        let entry = Entry {
            message: Message {
                id: "1".into(),
                text: "hello <@7>, see `code` and https://example.com".into(),
                media: vec![proto::Media {
                    reference: "p:1".into(),
                    kind: "image".into(),
                    w: Some(1600),
                    h: Some(1200),
                    ..proto::Media::default()
                }],
                reactions: vec![Reaction {
                    emoji: "👍".into(),
                    count: 3,
                    mine: true,
                }],
                ..Message::default()
            },
            ext,
            revision: 1,
        };
        let layout = message(&entry, 800.0, true, &|_| None);
        assert!(layout.header);
        assert!(layout.height > layout.body.height);
        let texts: Vec<String> = layout
            .body
            .lines
            .iter()
            .flat_map(|line| line.items.iter())
            .filter_map(|item| match item {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "@Ana"));
        assert!(texts.iter().any(|t| t == "code"));
        assert!(
            layout
                .pieces
                .iter()
                .any(|p| matches!(p, Piece::Picture { .. }))
        );
        assert!(layout.pieces.iter().any(|p| matches!(p, Piece::Embed(_))));
        assert!(layout.pieces.iter().any(|p| matches!(
            p,
            Piece::Reaction {
                count: 3,
                mine: true,
                ..
            }
        )));
        // A link is tappable where it is drawn.
        let link = layout.body.lines.iter().find_map(|line| {
            line.items.iter().find_map(|item| match item {
                Item::Text {
                    x, link: Some(url), ..
                } => Some((*x + 4.0, line.y + line.height / 2.0, url.to_string())),
                _ => None,
            })
        });
        let (x, y, url) = link.expect("a link");
        assert_eq!(hit(&layout, x, layout.body_y + y), Some(Hit::Link(url)));
    }

    #[test]
    fn pictures_fit_their_box_at_most_at_their_own_size() {
        assert_eq!(
            fit_box(Some(4000), Some(2000), 900.0, 900.0),
            (900.0, 450.0)
        );
        assert_eq!(fit_box(Some(100), Some(50), 900.0, 900.0), (275.0, 138.0));
        assert_eq!(fit_box(None, None, 900.0, 900.0).1, dp(200.0));
    }
}
