//! The transcript: every history entry planned once (by its stamp) into
//! pieces, pieces cached by what they show, placed top to bottom; the
//! reader's place kept through changes above it; and drawing the pieces a
//! band of rows shows.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use crate::gfx::Canvas;
use crate::ui::{MARGIN, RED, SECONDARY};

use super::super::markdown::{self, MdBlock};
use super::super::model::{Action, Chat, Model, Page};
use super::super::proto::{Block, Entry, ToolStyle};
use super::blocks::{lay_md_piece, lay_notice, lay_thinking, lay_user, lay_whole};
use super::markdown_layout::{PARAGRAPH_GAP, md_gap};
use super::ops::{Laid, draw_ops, quantized};
use super::tools::{GROUP_MIN, lay_group, lay_tool, tool_run};
use super::{NAV_H, ROW_PRESSED, ViewState, W, hash_of, transcript_bounds};

pub(crate) struct Placed {
    pub(crate) y: f32,
    pub(crate) laid: Rc<Laid>,
    /// Which entry (its stamp; 0: none, as the live reply) and which of its
    /// pieces: what the reader's place is kept by.
    pub(crate) stamp: u64,
    pub(crate) index: u32,
}

/// Where the reader is: pieces on screen (entry stamp, index in the entry,
/// the piece itself) and where each was in the transcript.
pub struct Anchor {
    pub(crate) pieces: Vec<(u64, u32, Rc<Laid>, f32)>,
}

/// The chat as a pass of the event loop found it (`ViewState::before`), so
/// the pass's end (`ViewState::settle`) can keep the reader's place.
pub struct Before {
    pub(crate) id: Option<String>,
    pub(crate) start: (u64, u64),
    pub(crate) height: f32,
    pub(crate) anchor: Option<Anchor>,
}

#[derive(Default)]
pub(crate) struct ChatLayout {
    pub(crate) key: u64,
    pub(crate) items: Vec<Placed>,
    pub(crate) height: f32,
}

/// A cached piece and the layout pass that last used it.
pub(crate) struct CachedPiece {
    pub(crate) laid: Rc<Laid>,
    pub(crate) used: u64,
}

/// An entry's plan and the layout pass that last used it.
pub(crate) struct CachedPlan {
    pub(crate) plan: Rc<Plan>,
    pub(crate) used: u64,
}

pub(crate) struct Context<'a> {
    pub(crate) chat: &'a Chat,
    pub(crate) styles: &'a HashMap<String, ToolStyle>,
    pub(crate) styles_version: u64,
}

pub(crate) fn thinking_key(blocks: &[Block], index: usize) -> String {
    // Stable within an entry: the first tool call before it, or the start.
    let anchor = blocks[..index].iter().rev().find_map(|block| match block {
        Block::ToolCall { tool_call_id, .. } => Some(tool_call_id.as_str()),
        _ => None,
    });
    format!(
        "think:{}:{}",
        anchor.unwrap_or("start"),
        blocks[..index].len()
    )
}

/// What an entry is drawn as, worked out once per entry (by its stamp; a
/// live reply's on every pass): its pieces, a hash of what each shows (the
/// piece cache's key, so the same content is laid out once) and the space
/// above each.
#[derive(Default)]
pub(crate) struct Plan {
    pub(crate) segments: Vec<Segment>,
}

pub(crate) enum Segment {
    /// The whole entry as one piece: a bubble or a notice.
    Whole { hash: u64 },
    /// A block of a reply's markdown.
    Markdown { block: MdBlock, hash: u64, gap: f32 },
    /// A thinking row, which opens to the thinking.
    Thinking {
        index: usize,
        key: String,
        hash: u64,
        gap: f32,
    },
    /// A run of tool calls: a row each, or one folded row (`group`: its
    /// toggle key and what it shows) that opens to them.
    Tools {
        calls: Vec<ToolRef>,
        group: Option<(String, u64)>,
        gap: f32,
    },
}

pub(crate) struct ToolRef {
    pub(crate) call: usize,
    /// Its result's block, if it has one yet.
    pub(crate) result: Option<usize>,
    pub(crate) key: String,
    pub(crate) hash: u64,
}

pub(crate) fn plan_entry(entry: &Entry) -> Plan {
    let whole = |hash: u64| Plan {
        segments: vec![Segment::Whole { hash }],
    };
    match entry {
        Entry::User { text, metadata } => {
            let automated = metadata
                .as_ref()
                .is_some_and(|m| m.automation.is_some() || m.system);
            whole(hash_of(("user", text, automated)))
        }
        Entry::Ai { blocks, .. } => plan_blocks(blocks),
        Entry::System { text, color } => whole(hash_of(("system", text, color))),
        _ => Plan::default(),
    }
}

pub(crate) fn plan_blocks(blocks: &[Block]) -> Plan {
    let results: HashMap<&str, usize> = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| match block {
            Block::ToolResult { tool_call_id, .. } => Some((tool_call_id.as_str(), index)),
            _ => None,
        })
        .collect();
    let mut segments = Vec::new();
    let mut index = 0;
    let mut previous_text = false;
    while index < blocks.len() {
        let first = segments.is_empty();
        match &blocks[index] {
            Block::Text { text } => {
                if !text.trim().is_empty() {
                    let gap = match (first, previous_text) {
                        (true, _) => 0.0,
                        (false, true) => PARAGRAPH_GAP,
                        (false, false) => 18.0,
                    };
                    let parsed = markdown::parse(text);
                    for (at, block) in parsed.iter().enumerate() {
                        segments.push(Segment::Markdown {
                            block: block.clone(),
                            hash: hash_of(("md", block)),
                            gap: if at == 0 {
                                gap
                            } else {
                                md_gap(&parsed[at - 1], block)
                            },
                        });
                    }
                    previous_text = true;
                }
                index += 1;
            }
            Block::Thinking { text } => {
                if !text.trim().is_empty() {
                    let key = thinking_key(blocks, index);
                    segments.push(Segment::Thinking {
                        index,
                        hash: hash_of(("think", &key, text)),
                        key,
                        gap: if first { 0.0 } else { 10.0 },
                    });
                    previous_text = false;
                }
                index += 1;
            }
            Block::ToolCall { tool_call_id, .. } => {
                let (end, calls) = tool_run(blocks, index);
                let calls: Vec<ToolRef> = calls
                    .into_iter()
                    .filter_map(|call| {
                        let Block::ToolCall {
                            tool_call_id,
                            tool_name,
                            summary,
                            ..
                        } = &blocks[call]
                        else {
                            return None;
                        };
                        let result = results.get(tool_call_id.as_str()).copied();
                        // A call's input and result never change once sent (a
                        // live reply's plan is made on every pass: neither is
                        // hashed whole).
                        let outcome = result.and_then(|at| match &blocks[at] {
                            Block::ToolResult {
                                output, is_error, ..
                            } => Some((output.len(), is_error)),
                            _ => None,
                        });
                        Some(ToolRef {
                            call,
                            result,
                            key: format!("tool:{tool_call_id}"),
                            hash: hash_of(("tool", tool_call_id, tool_name, summary, outcome)),
                        })
                    })
                    .collect();
                let group = (calls.len() >= GROUP_MIN).then(|| {
                    let key = format!("group:{tool_call_id}");
                    let shown: Vec<u64> = calls.iter().map(|tool| tool.hash).collect();
                    let hash = hash_of(("group", &key, shown));
                    (key, hash)
                });
                segments.push(Segment::Tools {
                    calls,
                    group,
                    gap: if first { 0.0 } else { 10.0 },
                });
                previous_text = false;
                index = end.max(index + 1);
            }
            _ => index += 1,
        }
    }
    Plan { segments }
}

/// More pieces and plans than this are kept, the least recently used go.
pub(crate) const PIECES_KEPT: usize = 6000;
pub(crate) const PLANS_KEPT: usize = 1500;

impl ViewState {
    /// The piece that shows `key`, laid out by `make` if none does yet.
    pub(crate) fn piece(&mut self, key: u64, make: impl FnOnce() -> Laid) -> Rc<Laid> {
        let pass = self.pass;
        let cached = self.pieces.entry(key).or_insert_with(|| CachedPiece {
            laid: Rc::new(quantized(make())),
            used: pass,
        });
        cached.used = pass;
        cached.laid.clone()
    }

    pub(crate) fn plan(&mut self, stamp: u64, entry: &Entry) -> Rc<Plan> {
        let pass = self.pass;
        let cached = self.plans.entry(stamp).or_insert_with(|| CachedPlan {
            plan: Rc::new(plan_entry(entry)),
            used: pass,
        });
        cached.used = pass;
        cached.plan.clone()
    }

    /// Places a planned entry's pieces from `y`; returns where the next
    /// entry goes.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn place(
        &mut self,
        items: &mut Vec<Placed>,
        y: f32,
        plan: &Plan,
        entry: Option<&Entry>,
        stamp: u64,
        blocks: &[Block],
        context: &Context,
    ) -> f32 {
        let mut h = 0.0;
        let mut index = 0;
        let mut push = |items: &mut Vec<Placed>, h: &mut f32, laid: Rc<Laid>| {
            if laid.h > 0.0 {
                items.push(Placed {
                    y: y + *h,
                    laid: laid.clone(),
                    stamp,
                    index,
                });
                index += 1;
                *h += laid.h;
            }
        };
        let expanded = &context.chat.expanded;
        for segment in &plan.segments {
            match segment {
                Segment::Whole { hash } => {
                    let Some(entry) = entry else { continue };
                    let laid = self.piece(*hash, || lay_whole(entry));
                    push(items, &mut h, laid);
                }
                Segment::Markdown { block, hash, gap } => {
                    h += gap;
                    let laid = self.piece(*hash, || lay_md_piece(block));
                    push(items, &mut h, laid);
                }
                Segment::Thinking {
                    index,
                    key,
                    hash,
                    gap,
                } => {
                    h += gap;
                    let open = expanded.contains(key);
                    let text = match &blocks[*index] {
                        Block::Thinking { text } => text.as_str(),
                        _ => "",
                    };
                    let laid = self.piece(hash_of((hash, open)), || lay_thinking(text, key, open));
                    push(items, &mut h, laid);
                }
                Segment::Tools { calls, group, gap } => {
                    h += gap;
                    let indent = match group {
                        Some((key, hash)) => {
                            let open = expanded.contains(key);
                            let laid = self
                                .piece(hash_of((hash, open, context.styles_version)), || {
                                    lay_group(blocks, calls, key, open, context)
                                });
                            push(items, &mut h, laid);
                            if !open {
                                continue;
                            }
                            34.0
                        }
                        None => 0.0,
                    };
                    for tool in calls {
                        let open = expanded.contains(&tool.key);
                        let id = tool.key.trim_start_matches("tool:");
                        // Opened, it shows the output fetched for it.
                        let fetched = open.then(|| {
                            (
                                context.chat.outputs.get(id).map(String::len),
                                context.chat.outputs_requested.contains(id),
                            )
                        });
                        let key = hash_of((
                            tool.hash,
                            open,
                            indent as i32,
                            context.styles_version,
                            fetched,
                        ));
                        let laid =
                            self.piece(key, || lay_tool(blocks, tool, indent, open, context));
                        push(items, &mut h, laid);
                    }
                }
            }
        }
        if h > 0.0 { y + h + 30.0 } else { y }
    }

    /// Lays the open chat out if it changed since the last frame: every
    /// entry's pieces, from the cache unless what they show is new.
    pub(crate) fn layout(&mut self, model: &Model) {
        let chat = &model.chat;
        let queued = model.queued_here();
        let key = hash_of((
            chat.version,
            chat.id.as_deref(),
            queued.len(),
            chat.loaded,
            model.styles_version,
        ));
        if self.layout.key == key && !self.layout.items.is_empty() {
            return;
        }
        self.pass += 1;
        let context = Context {
            chat,
            styles: &model.tool_styles,
            styles_version: model.styles_version,
        };
        let mut items = Vec::new();
        let mut y = 24.0;
        let single = |view: &mut Self,
                      items: &mut Vec<Placed>,
                      y: f32,
                      key: u64,
                      make: &dyn Fn() -> Laid| {
            let laid = view.piece(key, make);
            if laid.h <= 0.0 {
                return y;
            }
            let next = y + laid.h + 30.0;
            items.push(Placed {
                y,
                laid,
                stamp: 0,
                index: 0,
            });
            next
        };
        if chat.older_request.is_some() {
            y = single(self, &mut items, y, 1, &|| {
                lay_notice("Loading…", SECONDARY, 1)
            });
        }
        debug_assert_eq!(chat.entries.len(), chat.stamps.len());
        for (entry, stamp) in chat.entries.iter().zip(&chat.stamps) {
            let plan = self.plan(*stamp, entry);
            let blocks = match entry {
                Entry::Ai { blocks, .. } => blocks.as_slice(),
                _ => &[],
            };
            y = self.place(&mut items, y, &plan, Some(entry), *stamp, blocks, &context);
        }
        for echo in &chat.echoes {
            let key = hash_of(("echo", &echo.text));
            y = single(self, &mut items, y, key, &|| {
                lay_user(&echo.text, 0.75, None)
            });
        }
        if let Some(live) = &chat.live {
            let plan = plan_blocks(&live.blocks);
            y = self.place(&mut items, y, &plan, None, 0, &live.blocks, &context);
        }
        for queued in queued {
            let key = hash_of(("queued", &queued.text));
            y = single(self, &mut items, y, key, &|| {
                lay_user(&queued.text, 0.45, Some("Queued"))
            });
        }
        if let Some(error) = &chat.error {
            let key = hash_of(("error", error));
            y = single(self, &mut items, y, key, &|| lay_notice(error, RED, 3));
        }
        self.prune();
        self.layout = ChatLayout {
            key,
            items,
            height: y + 40.0,
        };
    }

    /// Forgets the least recently used pieces and plans past the limits.
    pub(crate) fn prune(&mut self) {
        if self.pieces.len() > PIECES_KEPT {
            let mut used: Vec<u64> = self.pieces.values().map(|p| p.used).collect();
            used.sort_unstable();
            let cut = used[self.pieces.len() - PIECES_KEPT / 2];
            let pass = self.pass;
            self.pieces.retain(|_, p| p.used >= cut || p.used == pass);
        }
        if self.plans.len() > PLANS_KEPT {
            let mut used: Vec<u64> = self.plans.values().map(|p| p.used).collect();
            used.sort_unstable();
            let cut = used[self.plans.len() - PLANS_KEPT / 2];
            let pass = self.pass;
            self.plans.retain(|_, p| p.used >= cut || p.used == pass);
        }
    }
}

impl ViewState {
    pub fn chat_max(&mut self, model: &Model, height: f32) -> f32 {
        self.layout(model);
        let (top, bottom) = transcript_bounds(model, height, self.keyboard);
        (self.layout.height - (bottom - top)).max(0.0)
    }

    /// Where the reader is, unless following the newest message.
    pub(crate) fn anchor(&mut self, model: &Model, now: Instant) -> Option<Anchor> {
        if self.follow {
            return None;
        }
        self.layout(model);
        let offset = self.chat_scroll.value(now);
        let items = &self.layout.items;
        let first = items.partition_point(|item| item.y + item.laid.h <= offset);
        let pieces: Vec<(u64, u32, Rc<Laid>, f32)> = items[first..]
            .iter()
            .filter(|item| item.stamp != 0)
            .take(6)
            .map(|item| (item.stamp, item.index, item.laid.clone(), item.y))
            .collect();
        (!pieces.is_empty()).then_some(Anchor { pieces })
    }

    /// Moves the scroll by as much as the reader's place moved in the
    /// transcript (older history, a fresh window or a row opened above it),
    /// and by nothing else: what a finger or a fling did meanwhile stays.
    /// Returns false when none of its pieces is still there.
    pub(crate) fn restore(&mut self, model: &Model, anchor: &Anchor) -> bool {
        self.layout(model);
        // The same piece of the same entry; nearest its old index (a row
        // opened above it in the entry moves it down the list).
        let moved = anchor.pieces.iter().find_map(|(stamp, index, laid, y)| {
            self.layout
                .items
                .iter()
                .filter(|item| item.stamp == *stamp && Rc::ptr_eq(&item.laid, laid))
                .min_by_key(|item| item.index.abs_diff(*index))
                .map(|item| item.y - y)
        });
        let Some(moved) = moved else {
            return false;
        };
        if moved != 0.0 {
            self.chat_scroll.shift(moved);
        }
        true
    }

    /// The chat at the start of an event loop pass.
    pub fn before(&mut self, model: &Model, now: Instant) -> Before {
        if model.page != Page::Chat {
            return Before {
                id: None,
                start: (0, 0),
                height: 0.0,
                anchor: None,
            };
        }
        Before {
            id: model.chat.id.clone(),
            start: (model.chat.start, model.chat.start_block),
            height: self.content_height(model),
            anchor: self.anchor(model, now),
        }
    }

    /// At the end of a pass (after its input and daemon events): keeps what
    /// the reader looks at in place if the transcript changed above it, and
    /// follows the newest message from the bottom. Never undoes a drag or a
    /// fling.
    pub fn settle(&mut self, model: &Model, before: Before, height: f32, now: Instant) {
        if model.page != Page::Chat {
            return;
        }
        let same = before.id == model.chat.id && before.id.is_some();
        let height_now = self.content_height(model);
        let older_added = same && (model.chat.start, model.chat.start_block) < before.start;
        let kept = same
            && !self.follow
            && before
                .anchor
                .as_ref()
                .is_some_and(|anchor| self.restore(model, anchor));
        if older_added && !self.follow && !kept {
            self.chat_scroll.shift(height_now - before.height);
        }
        let max = self.chat_max(model, height);
        if self.follow && !self.chat_scroll.dragging() {
            self.chat_scroll.reset();
            self.chat_scroll.offset = max;
        } else if !self.chat_scroll.dragging()
            && self.chat_scroll.value(now) >= max - 40.0
            && max > 0.0
        {
            self.follow = true;
        }
    }

    /// The transcript is scrolled near its oldest loaded message.
    pub fn near_top(&self, now: Instant) -> bool {
        self.chat_scroll.value(now) < 1500.0
    }

    /// Height of the transcript above `y` of the first item (for keeping the
    /// view still when older history is added above it).
    pub fn content_height(&mut self, model: &Model) -> f32 {
        self.layout(model);
        self.layout.height
    }
}

/// The transcript's items (and the working dots after them) that fall in
/// rows `clip`, scrolled to `offset`.
pub(crate) fn draw_transcript(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    pressed: Option<&Action>,
    clip: (f32, f32),
    offset: f32,
) {
    let (top, bottom) = clip;
    let area_top = NAV_H;
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    // Pieces are in order: skip to the first that may reach the clip (no
    // glyph reaches a screen beyond its piece).
    let items = &view.layout.items;
    let first =
        items.partition_point(|item| area_top + item.y + item.laid.h - offset < top - 400.0);
    for item in &items[first..] {
        let y = area_top + item.y - offset;
        if y - 400.0 > bottom {
            break;
        }
        if y + item.laid.ink.1 < top || y + item.laid.ink.0 > bottom {
            continue;
        }
        for (rect, action) in &item.laid.hits {
            if pressed == Some(action) {
                canvas.fill_rect(
                    0,
                    (y + rect.y) as i32,
                    W as i32,
                    rect.h as i32,
                    ROW_PRESSED,
                    1.0,
                );
            }
        }
        draw_ops(canvas, &item.laid.ops, y, clip);
    }
    if model.streaming() {
        // Three dots after the reply while the model works.
        let cy = area_top + view.layout.height - offset - 34.0;
        for index in 0..3 {
            let lit = usize::from(view.dots % 3) == index;
            canvas.fill_circle(
                MARGIN + 16.0 + index as f32 * 34.0,
                cy,
                10.0,
                SECONDARY,
                if lit { 0.9 } else { 0.35 },
            );
        }
    }
    canvas.reset_clip();
}
