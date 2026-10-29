//! The transcript (WP-C2): every history entry planned once (by its stamp)
//! into pieces, pieces cached by what they show, placed top to bottom with
//! the iPhone's rhythm (roadmap §3.4, §3.6.6–3.6.14): the history edge,
//! bubbles and notification rows, replies with their thinking, tool rows,
//! notices and one footer per run, the live reply with its status lines
//! and ticking footer, queued messages. The reader's place is kept through
//! changes above it, the tail followed when at it; the jump-to-latest
//! button floats over the bottom.
//!
//! What changes with time inside a piece (spinners, the WORKING shimmer,
//! the live footer's clock, status clocks) is a mark: placed with its
//! piece, keyed per frame, and redrawn alone (`TranscriptFrame`), so a
//! ticking second costs one line of rows, not a layout pass.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::gfx::{Canvas, RectF};

use super::super::anim::{self, Animator, Tween};
use super::super::fold::{self, FoldPlan, MergedFold};
use super::super::footer;
use super::super::markdown::{self, MdBlock};
use super::super::model::chat::{
    COMPACTION_KIND, Disclosure, SavedAnchor, merged_thinking, thinking_key, visible,
};
use super::super::model::{Action, Chat, Model, Page, chat, composer, rich::Rich};
use super::super::proto::{Block, Entry, Metadata, ToolStyle};
use super::super::theme::{
    self, BLOCK_GAP, JUMP, JUMP_INSET, ROW_GAP, TRANSCRIPT_BOTTOM, TRANSCRIPT_TOP, W,
};
use super::super::ui::icons::{self, Icon};
use super::super::ui::widgets;
use super::blocks::{
    self, Edge, FOOTER_GAP, Mark, Piece, StatusKind, Tone, draw_mark, lay_bubble, lay_context,
    lay_divider, lay_edge, lay_footer, lay_instructions, lay_live_footer, lay_load_error,
    lay_md_piece, lay_notice, lay_notification, lay_status, lay_thinking, lay_turn_fold,
    lay_working, mark_frames, mark_key, notice_color, wall_ms,
};
use super::markdown_layout::{md_gap, state_key};
use super::media::{Strip, lay_strip};
use super::ops::{Laid, draw_ops, px, quantized};
use super::tools::{Output, expandable, lay_tool, output_state, subagent_child, tool_run};
use super::{CHAT_TOP, ViewState, hash_of, transcript_bounds};

/// What a placed piece belongs to (what a long press on it opens).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Role {
    /// A history entry or the live reply (`Placed::stamp`).
    Message,
    /// A queued message (its queue id).
    Queued(String),
    /// The edge, a sent bubble, an error.
    Other,
}

pub(crate) struct Placed {
    pub(crate) y: f32,
    pub(crate) laid: Rc<Laid>,
    /// Which entry (its stamp, the live reply's too; 0: none) and which of
    /// its pieces: what the reader's place is kept by.
    pub(crate) stamp: u64,
    pub(crate) index: u32,
    pub(crate) role: Role,
}

/// A mark where its piece is (`top`: the piece's top in the transcript).
pub(crate) struct PlacedMark {
    pub(crate) top: f32,
    pub(crate) mark: Mark,
}

impl PlacedMark {
    fn rows(&self) -> (f32, f32) {
        let y = self.top + self.mark.y;
        (y, y + self.mark.h)
    }
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
    pub(crate) marks: Vec<PlacedMark>,
    /// Tool rows whose result came without its output (for prefetching):
    /// their top and id, in order.
    pub(crate) tools: Vec<(f32, String)>,
    /// Fold marks at the left edge, in order (they never overlap).
    pub(crate) gutters: Vec<Gutter>,
    /// Where each fold's first row is (a group's, a turn's): where a
    /// toggle keeps the tapped spot.
    pub(crate) fold_tops: HashMap<String, f32>,
    /// Each placed entry's index in the chat by its stamp, and its top
    /// (the reader's place is kept by it).
    pub(crate) entry_tops: Vec<(usize, f32)>,
    pub(crate) height: f32,
}

/// A fold's mark at the left edge (PAR-407): down a row that carries
/// folded work (or every row of an opened group), or a short line in a gap
/// where work folded with no reply to carry it. A tap opens or closes it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Gutter {
    pub(crate) y0: f32,
    pub(crate) y1: f32,
    pub(crate) key: String,
    pub(crate) open: bool,
    pub(crate) line: bool,
}

/// The mark's column (the transcript's left padding) and its line in it.
const GUTTER_W: f32 = theme::TRANSCRIPT_PAD;
const GUTTER_X: f32 = 14.0;
const GUTTER_LINE: f32 = 6.0;
/// A row's mark leaves this much of it at each end.
const GUTTER_INSET: f32 = 9.0;
/// A gap's line, and how far its target reaches beyond it.
const GUTTER_GAP_LINE: f32 = 34.0;
const GUTTER_GAP_REACH: f32 = 26.0;

impl Gutter {
    /// Rows it draws.
    fn ink(&self) -> (f32, f32) {
        if self.line {
            (self.y0, self.y1)
        } else {
            (self.y0 + GUTTER_INSET, self.y1 - GUTTER_INSET)
        }
    }

    /// Rows its target covers.
    fn target(&self) -> (f32, f32) {
        if self.line {
            (self.y0 - GUTTER_GAP_REACH, self.y1 + GUTTER_GAP_REACH)
        } else {
            (self.y0, self.y1)
        }
    }

    fn action(&self) -> Action {
        Action::Chat(chat::Act::ToggleExpanded(self.key.clone()))
    }

    fn pressed(&self, pressed: Option<&Action>) -> bool {
        matches!(pressed, Some(Action::Chat(chat::Act::ToggleExpanded(key))) if *key == self.key)
    }

    /// Its line on screen, `dy` from the transcript's coordinates.
    fn draw(&self, canvas: &mut Canvas, dy: f32, pressed: bool) {
        let c = theme::c();
        let color = if pressed {
            theme::p().muted
        } else if self.open {
            c.fold_mark_open
        } else {
            c.fold_mark
        };
        let (y0, y1) = self.ink();
        let (y0, y1) = (px(dy + y0), px(dy + y1));
        if y1 > y0 {
            canvas.fill_rect(px(GUTTER_X), y0, px(GUTTER_LINE), y1 - y0, color, 1.0);
        }
    }
}

/// A cached piece and the layout pass that last used it.
pub(crate) struct CachedPiece {
    pub(crate) laid: Rc<Laid>,
    pub(crate) marks: Rc<[Mark]>,
    pub(crate) used: u64,
}

/// An entry's plan and the layout pass that last used it.
pub(crate) struct CachedPlan {
    pub(crate) plan: Rc<Plan>,
    pub(crate) used: u64,
}

/// The transcript's own view state (in `ViewState::transcript`).
#[derive(Default)]
pub struct TranscriptView {
    /// The jump button: shown or not, and the fade since.
    jump: Option<(bool, Tween)>,
    /// The scroll offset the last frame showed (the button waits for the
    /// transcript to stop moving), and whether it had moved then and the
    /// button would show once it is still.
    last_offset: Option<f32>,
    held: bool,
    /// Since when the transcript has not moved (tool outputs are fetched
    /// only then: never while it scrolls).
    still_since: Option<Instant>,
    /// The transcript's rows at the last frame (for `animate`).
    bounds: (f32, f32),
    /// The open chat's folding, while it shows the same.
    fold: Option<Folding>,
    /// A fold just tapped: its first row goes where the tapped row was.
    pin: Option<Pin>,
    /// The chat placed at its saved reading place (or its latest response,
    /// or the bottom) once it loaded.
    positioned: Option<String>,
    /// Show all work as last seen (flipping it closes the folds).
    show_all: Option<bool>,
    /// The reading place last kept: the chat, its entry and that entry's
    /// top below the transcript's (None: at the newest message).
    kept: Option<(String, Option<(usize, i32)>)>,
}

impl TranscriptView {
    /// A fold's toggle was touched at `row` (below the transcript's top):
    /// once it opens or folds, its first row goes there.
    fn pin_fold(&mut self, model: &Model, key: String, row: f32, now: Instant) {
        let open = model.chat.expanded.contains(&key);
        self.pin = Some(Pin {
            key,
            top: row,
            open,
            at: now,
        });
    }
}

/// A pin waits this long for its fold to toggle (a press that became a
/// drag never does).
const PIN_WAIT: Duration = Duration::from_secs(2);

/// The fold plan of the open chat and what it was made from.
pub(crate) struct Folding {
    key: u64,
    plan: Rc<FoldPlan>,
}

/// A fold toggle's tapped row: where (on screen) and whether the fold was
/// open then.
struct Pin {
    key: String,
    top: f32,
    open: bool,
    at: Instant,
}

pub(crate) struct Context<'a> {
    pub(crate) model: &'a Model,
    pub(crate) chat: &'a Chat,
    /// Image previews and code blocks' state (C1).
    pub(crate) rich: &'a Rich,
    pub(crate) styles: &'a HashMap<String, ToolStyle>,
    pub(crate) styles_version: u64,
    /// Laying out the live reply.
    pub(crate) live: bool,
    /// What opens by default.
    pub(crate) disclosure: Disclosure,
}

// ----- Plans ---------------------------------------------------------------------------

/// An automated prompt (PAR-112, 402): how it is labelled.
pub(crate) struct Automated {
    kind: String,
    preview: String,
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
    /// A user message: the user's own bubble, or an automated prompt; or
    /// the owner's own words a notification relays (`relay`: them and where
    /// they came from).
    User {
        automated: Option<Automated>,
        relay: Option<(String, String)>,
    },
    /// A system notice (its key opens it).
    Notice { key: String, muted: bool },
    /// The compaction divider.
    Divider,
    /// A block of a reply's markdown (of its block `index`).
    Markdown {
        index: usize,
        block: MdBlock,
        hash: u64,
        gap: f32,
    },
    /// A `[Context:` text block.
    Context { index: usize, gap: f32 },
    /// Thinking blocks `index..end` (merged), which open to the thinking.
    Thinking {
        index: usize,
        end: usize,
        key: String,
        hash: u64,
        gap: f32,
    },
    /// A run of tool calls, a row each.
    Tools { calls: Vec<ToolRef>, gap: f32 },
    /// A message's images (C1's `media.rs`): a reply's after it, the
    /// user's under their bubble.
    Images { strip: Strip, gap: f32 },
}

/// A notification row's target reaches this far into the gaps around it.
const NOTIFICATION_TRIM: f32 = 28.0;

pub(crate) struct ToolRef {
    pub(crate) call: usize,
    /// Its result's block, if it has one yet.
    pub(crate) result: Option<usize>,
    pub(crate) key: String,
    pub(crate) hash: u64,
}

/// Notices that keep the model up to date and are never shown (PAR-107).
const MODEL_ONLY: [&str; 3] = ["client_host", "remote_host", "conversation_goal"];

fn plan_user(text: &str, metadata: Option<&Metadata>) -> Vec<Segment> {
    if metadata.is_some_and(|m| m.system) {
        return vec![Segment::Notice {
            key: format!("n:{}", hash_of(text)),
            muted: false,
        }];
    }
    let automation = metadata.and_then(|m| m.automation.as_ref());
    // The owner's own words, relayed: their bubble, where it came from.
    if let Some(words) = fold::owner_relayed_text(text, automation) {
        let header = text.split('\n').next().unwrap_or_default();
        let source = fold::notification_preview(header);
        return vec![Segment::User {
            automated: None,
            relay: Some((words, source)),
        }];
    }
    if !fold::automated(text, automation) {
        return vec![Segment::User {
            automated: None,
            relay: None,
        }];
    }
    let field = |name: &str| {
        automation
            .and_then(|a| a.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    vec![Segment::User {
        automated: Some(Automated {
            kind: field("kind").unwrap_or_default(),
            preview: fold::notification_preview(text),
        }),
        relay: None,
    }]
}

/// Whether a notice folds with its turn's work (`isInformationalNotice`):
/// compaction, provider notices, restarts, interruptions, retries.
pub(crate) fn informational(text: &str, kind: Option<&str>) -> bool {
    kind == Some(COMPACTION_KIND)
        || kind == Some("provider_notice")
        || text == chat::COMPACTION_TEXT
        || text == "✗ Daemon restarted"
        || text == "✗ Interrupted"
        || text.starts_with("⟳ ")
}

pub(crate) fn plan_entry(entry: &Entry) -> Plan {
    let segments = match entry {
        Entry::User {
            text,
            metadata,
            images,
        } => {
            let mut segments = plan_user(text, metadata.as_ref());
            // The user's images under their bubble (C1's strip).
            if matches!(
                segments.first(),
                Some(Segment::User {
                    automated: None,
                    ..
                })
            ) && let Some(strip) = Strip::of_user(images)
            {
                segments.push(Segment::Images { strip, gap: 11.0 });
            }
            segments
        }
        Entry::Ai { blocks, .. } => return plan_blocks(blocks),
        Entry::System { text, metadata, .. } => {
            let kind = metadata.as_ref().and_then(|m| m.kind.as_deref());
            if kind.is_some_and(|kind| MODEL_ONLY.contains(&kind)) {
                Vec::new()
            } else if kind == Some(COMPACTION_KIND) || text == chat::COMPACTION_TEXT {
                vec![Segment::Divider]
            } else {
                vec![Segment::Notice {
                    key: format!("n:{}", hash_of(text)),
                    muted: kind == Some("realtime_call_status"),
                }]
            }
        }
        _ => Vec::new(),
    };
    Plan { segments }
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
    while index < blocks.len() {
        let gap = if segments.is_empty() { 0.0 } else { BLOCK_GAP };
        match &blocks[index] {
            Block::Text { text } => {
                if text.trim_start_matches('\n').starts_with("[Context:") {
                    segments.push(Segment::Context { index, gap });
                } else if !text.trim().is_empty() {
                    let parsed = markdown::parse_capped(text);
                    for (at, block) in parsed.iter().enumerate() {
                        segments.push(Segment::Markdown {
                            index,
                            block: block.clone(),
                            hash: hash_of(("md", block)),
                            gap: if at == 0 {
                                gap
                            } else {
                                md_gap(&parsed[at - 1], block)
                            },
                        });
                    }
                }
                index += 1;
            }
            Block::Thinking { .. } => {
                let (end, text) = merged_thinking(blocks, index);
                if !text.trim().is_empty() || end == blocks.len() {
                    let key = thinking_key(blocks, index);
                    segments.push(Segment::Thinking {
                        index,
                        end,
                        hash: hash_of(("think", &key, &text)),
                        key,
                        gap,
                    });
                }
                index = end.max(index + 1);
            }
            Block::ToolCall { .. } => {
                let (end, calls) = tool_run(blocks, index);
                let calls: Vec<ToolRef> = calls
                    .into_iter()
                    .filter_map(|call| {
                        let Block::ToolCall {
                            tool_call_id,
                            tool_name,
                            summary,
                            presentation,
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
                        let detail = presentation.as_ref().and_then(|p| p.detail.as_ref());
                        Some(ToolRef {
                            call,
                            result,
                            key: format!("tool:{tool_call_id}"),
                            hash: hash_of((
                                "tool",
                                tool_call_id,
                                tool_name,
                                summary,
                                detail,
                                outcome,
                            )),
                        })
                    })
                    .collect();
                segments.push(Segment::Tools { calls, gap });
                index = end.max(index + 1);
            }
            _ => index += 1,
        }
    }
    if let Some(strip) = Strip::of_blocks(blocks) {
        let gap = if segments.is_empty() { 0.0 } else { 25.0 };
        segments.push(Segment::Images { strip, gap });
    }
    Plan { segments }
}

/// Today's local day number (what "today" and "yesterday" in footers turn
/// on), looked up at most once a second.
fn today() -> i64 {
    use std::cell::Cell;
    thread_local! {
        static TODAY: Cell<Option<(Instant, i64)>> = const { Cell::new(None) };
    }
    let now = Instant::now();
    TODAY.with(|cell| match cell.get() {
        Some((at, day)) if now.duration_since(at) < Duration::from_secs(1) => day,
        _ => {
            let day = footer::local_day(wall_ms(now));
            cell.set(Some((now, day)));
            day
        }
    })
}

/// More pieces and plans than this are kept, the least recently used go.
pub(crate) const PIECES_KEPT: usize = 6000;
pub(crate) const PLANS_KEPT: usize = 1500;

/// Whether `id` is one of the open conversation's own subagents (the open
/// one being top-level).
fn own_subagent(model: &Model, id: &str) -> bool {
    let Some(open) = model.chat.id.as_deref() else {
        return false;
    };
    let conversations = &model.sidebar.conversations;
    conversations
        .get(open)
        .is_some_and(|own| own.parent_conversation_id.is_none())
        && conversations
            .get(id)
            .is_some_and(|child| child.parent_conversation_id.as_deref() == Some(open))
}

fn title_of(model: &Model, id: &str) -> String {
    model
        .sidebar
        .conversations
        .get(id)
        .and_then(|s| s.title.lines().next().map(str::to_owned))
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| id.to_owned())
}

/// Where an entry's pieces went: their bottom below where it was placed
/// (a notification row reaches above it), the gap it leaves to the next
/// row, and whether anything showed.
struct EntryLaid {
    h: f32,
    tail_gap: f32,
    any: bool,
}

/// What of an entry shows (`Shown::default()`: all of it, unfolded).
#[derive(Clone, Copy, Default)]
pub(crate) struct Shown<'a> {
    /// Blocks folded away.
    pub(crate) hidden: Option<&'a HashSet<usize>>,
    /// Only this block (the live turn's newest step, already committed).
    pub(crate) only: Option<usize>,
    /// Folding is on: a notification is its row.
    pub(crate) folding: bool,
    /// A notification folded into a turn's work, shown while that is open.
    pub(crate) merged: Option<&'a MergedFold>,
    /// The turn's folded-work row above it: what folded, its key, open.
    pub(crate) summary: Option<(&'a str, &'a str, bool)>,
    /// The space above its first piece (something is above it).
    pub(crate) gap: f32,
}

impl Shown<'_> {
    fn block(&self, index: usize) -> bool {
        self.only.is_none_or(|only| only == index)
            && !self.hidden.is_some_and(|hidden| hidden.contains(&index))
    }
}

/// Puts an entry's pieces one under the other.
struct Placer<'a> {
    layout: &'a mut ChatLayout,
    y: f32,
    h: f32,
    index: u32,
    stamp: u64,
    /// The space above the first piece.
    first_gap: f32,
}

impl Placer<'_> {
    /// The space above a piece (none above the entry's first).
    fn gap(&mut self, gap: f32) {
        if self.index > 0 {
            self.h += gap;
        }
    }

    /// The entry's first row is a 44 pt target reaching into the gap above.
    fn lead(&mut self) {
        if self.index == 0 {
            self.h -= NOTIFICATION_TRIM;
        }
    }

    fn put(&mut self, (laid, marks): (Rc<Laid>, Rc<[Mark]>)) {
        if laid.h <= 0.0 {
            return;
        }
        if self.index == 0 {
            self.h += self.first_gap;
        }
        let top = self.y + self.h;
        for mark in marks.iter() {
            self.layout.marks.push(PlacedMark {
                top,
                mark: mark.clone(),
            });
        }
        self.layout.items.push(Placed {
            y: top,
            laid,
            stamp: self.stamp,
            index: self.index,
            role: Role::Message,
        });
        self.index += 1;
        self.h += self.layout.items.last().map_or(0.0, |item| item.laid.h);
    }
}

impl ViewState {
    /// The piece that shows `key`, laid out by `make` if none does yet.
    pub(crate) fn piece(
        &mut self,
        key: u64,
        make: impl FnOnce() -> Piece,
    ) -> (Rc<Laid>, Rc<[Mark]>) {
        let pass = self.pass;
        let cached = self.pieces.entry(key).or_insert_with(|| {
            let piece = make();
            CachedPiece {
                laid: Rc::new(quantized(piece.laid)),
                marks: piece.marks.into(),
                used: pass,
            }
        });
        cached.used = pass;
        (cached.laid.clone(), cached.marks.clone())
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

    /// Places a planned entry's pieces from `y`, what `shown` shows of it.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        layout: &mut ChatLayout,
        y: f32,
        plan: &Plan,
        entry: Option<&Entry>,
        stamp: u64,
        blocks: &[Block],
        context: &Context,
        shown: Shown,
    ) -> EntryLaid {
        let mut at = Placer {
            layout,
            y,
            h: 0.0,
            index: 0,
            stamp,
            first_gap: shown.gap,
        };
        let mut tail_gap = ROW_GAP;
        let chat = context.chat;
        let open_at = |key: &str| chat.is_open(key, context.disclosure);
        let model = context.model;
        if let Some((label, key, open)) = shown.summary {
            let piece = self.piece(hash_of(("turn fold", label, key, open)), || {
                lay_turn_fold(Some(label), key, open, false)
            });
            at.put(piece);
        }
        for segment in &plan.segments {
            match segment {
                Segment::User { automated, relay } => {
                    let Some(Entry::User { text, .. }) = entry else {
                        continue;
                    };
                    if let Some((words, source)) = relay {
                        let label = format!("via {source}");
                        let piece = self.piece(hash_of(("relay", words, &label)), || {
                            lay_bubble(words, Tone::Own, Some(&label), false)
                        });
                        at.put(piece);
                        continue;
                    }
                    let Some(a) = automated else {
                        // Images alone: their strip, no empty bubble.
                        if text.trim().is_empty()
                            && matches!(entry, Some(Entry::User { images, .. }) if !images.is_empty())
                        {
                            continue;
                        }
                        let piece = self.piece(hash_of(("bubble", text, Tone::Own)), || {
                            lay_bubble(text, Tone::Own, None, false)
                        });
                        at.put(piece);
                        continue;
                    };
                    let source = |id: &str| title_of(model, id);
                    let automation = match entry {
                        Some(Entry::User { metadata, .. }) => {
                            metadata.as_ref().and_then(|m| m.automation.as_ref())
                        }
                        _ => None,
                    };
                    let own = |id: &str| own_subagent(model, id);
                    // A parent's delegation is this conversation's prompt.
                    let Some(label) = fold::notification_label(text, automation, &own) else {
                        let label =
                            fold::bubble_label(text, automation, fold::label_of(&a.kind), &source);
                        let piece = self.piece(hash_of(("auto", text, &label)), || {
                            lay_bubble(text, Tone::Automated, Some(&label), true)
                        });
                        at.put(piece);
                        continue;
                    };
                    let bubble = |view: &mut Self, at: &mut Placer| {
                        let label = fold::bubble_label(text, automation, label, &source);
                        let piece = view.piece(hash_of(("auto", text, &label)), || {
                            lay_bubble(text, Tone::Automated, Some(&label), true)
                        });
                        at.put(piece);
                    };
                    // Opened with its turn's work: the row it opened leads.
                    if let Some(merged) = shown.merged
                        && let Some(lead) = &merged.lead
                    {
                        at.lead();
                        let piece = self.piece(hash_of(("turn fold", lead, &merged.turn)), || {
                            lay_turn_fold(Some(lead), &merged.turn, true, false)
                        });
                        at.put(piece);
                        at.h -= NOTIFICATION_TRIM - 17.0;
                    }
                    // Shown in full (all work shown, or its fold open).
                    if !shown.folding || shown.merged.is_some_and(|m| m.open) {
                        bubble(self, &mut at);
                        continue;
                    }
                    let key = format!("u:{}", hash_of(text));
                    let open = open_at(&key);
                    if at.index == 0 {
                        at.lead();
                    }
                    let piece = self
                        .piece(hash_of(("note", &key, label, &a.preview, open)), || {
                            lay_notification(label, &a.preview, &key, open)
                        });
                    at.put(piece);
                    tail_gap = ROW_GAP - NOTIFICATION_TRIM;
                    if open {
                        at.h -= NOTIFICATION_TRIM - 17.0;
                        bubble(self, &mut at);
                        tail_gap = ROW_GAP;
                    }
                }
                Segment::Notice { key, muted } => {
                    let (text, color) = match entry {
                        Some(Entry::System { text, color, .. }) => {
                            (text, notice_color(color.as_deref()))
                        }
                        Some(Entry::User { text, .. }) => (text, theme::p().muted),
                        _ => continue,
                    };
                    let open = open_at(key);
                    let piece = self.piece(
                        hash_of(("notice", key, text, color, *muted, open, stamp)),
                        || lay_notice(text, color, *muted, key, stamp, open),
                    );
                    at.put(piece);
                }
                Segment::Divider => {
                    let piece =
                        self.piece(hash_of("divider"), || lay_divider("Compaction finished"));
                    at.put(piece);
                }
                Segment::Markdown {
                    index,
                    block,
                    hash,
                    gap,
                } => {
                    if !shown.block(*index) {
                        continue;
                    }
                    at.gap(*gap);
                    let rich = context.rich;
                    let key = hash_of((hash, state_key(block, Some(rich))));
                    let piece = self.piece(key, || lay_md_piece(block, rich));
                    at.put(piece);
                }
                Segment::Context { index, gap } => {
                    let Some(Block::Text { text }) = blocks.get(*index) else {
                        continue;
                    };
                    if !shown.block(*index) {
                        continue;
                    }
                    at.gap(*gap);
                    let piece = self.piece(hash_of(("context", text)), || lay_context(text));
                    at.put(piece);
                }
                Segment::Thinking {
                    index,
                    end,
                    key,
                    hash,
                    gap,
                } => {
                    if !shown.block(*index) {
                        continue;
                    }
                    at.gap(*gap);
                    let open = open_at(key);
                    let growing = context.live && *end == blocks.len();
                    // Its text merged only when laid out (a cached piece
                    // needs none of it).
                    let piece = self.piece(hash_of((hash, open, growing, stamp)), || {
                        let (_, text) = merged_thinking(blocks, *index);
                        lay_thinking(&text, key, stamp, open, growing)
                    });
                    at.put(piece);
                }
                Segment::Tools { calls, gap } => {
                    let mut first = true;
                    for tool in calls {
                        if !shown.block(tool.call) {
                            continue;
                        }
                        if first {
                            at.gap(*gap);
                            first = false;
                        } else {
                            at.h += 17.0;
                        }
                        let output = output_state(chat, tool, blocks, context.live);
                        let open = open_at(&tool.key) && expandable(&output);
                        if matches!(output, Output::Unknown) {
                            let id = tool.key.trim_start_matches("tool:").to_owned();
                            at.layout.tools.push((at.y + at.h, id));
                        }
                        let child = match &blocks[tool.call] {
                            Block::ToolCall { tool_name, .. } => subagent_child(tool_name, &output)
                                .filter(|id| model.sidebar.conversations.contains_key(id)),
                            _ => None,
                        };
                        let key = hash_of((
                            tool.hash,
                            open,
                            context.styles_version,
                            open.then(|| output.key()),
                            (!open).then(|| expandable(&output)),
                            matches!(output, Output::Running) && context.live,
                            &child,
                        ));
                        let piece = self.piece(key, || {
                            lay_tool(blocks, tool, open, &output, child.as_deref(), context)
                        });
                        at.put(piece);
                    }
                }
                Segment::Images { strip, gap } => {
                    if shown.only.is_some() {
                        continue;
                    }
                    at.gap(*gap);
                    let conv = context.chat.id.as_deref();
                    let images = &context.rich.images;
                    let piece = self.piece(strip.state_key(images, conv), || {
                        lay_strip(strip, images, conv).into()
                    });
                    at.put(piece);
                }
            }
        }
        EntryLaid {
            h: at.h,
            tail_gap,
            any: at.index > 0,
        }
    }

    /// The open chat's fold plan (none while all work shows), made again
    /// only when what it is made from changed: the entries, the live
    /// reply's steps, the folds toggled, the fold marks setting, the
    /// sidebar (whose subagents a conversation has).
    fn folding(&mut self, model: &Model, disclosure: Disclosure) -> Option<Rc<FoldPlan>> {
        use std::hash::{Hash, Hasher};
        if disclosure.show_all {
            self.transcript.fold = None;
            return None;
        }
        let chat = &model.chat;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (chat.id.as_deref(), chat.start, &chat.stamps).hash(&mut hasher);
        if let Some(live) = &chat.live {
            live.started_at.to_bits().hash(&mut hasher);
            for block in &live.blocks {
                let step = match block {
                    Block::Text { text } => 1 + u8::from(!text.trim().is_empty()),
                    Block::Thinking { .. } => 3,
                    Block::ToolCall { .. } => 4,
                    Block::ToolResult { .. } => 5,
                    Block::Other => 6,
                };
                step.hash(&mut hasher);
            }
        }
        // The folds toggled open (any order).
        let toggled = chat
            .expanded
            .iter()
            .filter(|key| key.starts_with("turn:") || key.starts_with("group:"))
            .fold(0u64, |sum, key| sum ^ hash_of(key));
        (toggled, disclosure.marks, model.sidebar.version).hash(&mut hasher);
        let key = hasher.finish();
        if let Some(folding) = &self.transcript.fold
            && folding.key == key
        {
            return Some(folding.plan.clone());
        }
        let msgs: Vec<fold::Msg> = chat
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::User { text, metadata, .. } => {
                    let meta = metadata.as_ref();
                    fold::Msg::User {
                        text,
                        system: meta.is_some_and(|m| m.system),
                        automation: meta.and_then(|m| m.automation.as_ref()),
                        queue_entry_id: meta.and_then(|m| m.queue_entry_id.as_deref()),
                        started_at: meta.map_or(0.0, |m| m.started_at),
                    }
                }
                Entry::Ai { blocks, .. } => fold::Msg::Assistant { blocks },
                Entry::System { text, metadata, .. } => fold::Msg::System {
                    informational: informational(
                        text,
                        metadata.as_ref().and_then(|m| m.kind.as_deref()),
                    ),
                },
                _ => fold::Msg::Other,
            })
            .collect();
        let keys = fold::keys(&chat.entries, chat.start);
        let plan = Rc::new(fold::plan(
            &msgs,
            &keys,
            chat.live.as_ref().map(|live| live.blocks.as_slice()),
            &|key| chat.expanded.contains(key),
            disclosure.marks,
            chat.live.as_ref().map(|live| live.started_at),
            &|id| own_subagent(model, id),
        ));
        self.transcript.fold = Some(Folding {
            key,
            plan: plan.clone(),
        });
        Some(plan)
    }

    /// Lays the open chat out if it changed since the last frame: every
    /// entry's pieces, from the cache unless what they show is new; folded
    /// (`fold.rs`) unless Show all work is on.
    pub(crate) fn layout(&mut self, model: &Model) {
        self.check_generation();
        let chat = &model.chat;
        let day = today();
        let disclosure = Disclosure::of(&model.persist);
        let key = hash_of((
            disclosure,
            chat.version,
            chat.id.as_deref(),
            chat.loaded,
            model.ops.styles_version,
            model.rich.version(),
            self.generation,
            day,
            model.open_summary().map(|s| (&s.provider, &s.title)),
        ));
        if self.layout.key == key && !self.layout.items.is_empty() {
            return;
        }
        self.pass += 1;
        let fold = self.folding(model, disclosure);
        let folding = fold.is_some();
        let context = Context {
            model,
            chat,
            rich: &model.rich,
            styles: &model.ops.tool_styles,
            styles_version: model.ops.styles_version,
            live: false,
            disclosure,
        };
        let mut layout = ChatLayout::default();
        let mut y = TRANSCRIPT_TOP;
        let mut gap = 0.0;
        let single = |view: &mut Self,
                      layout: &mut ChatLayout,
                      y: f32,
                      key: u64,
                      role: Role,
                      make: &dyn Fn() -> Piece| {
            let (laid, marks) = view.piece(key, make);
            for mark in marks.iter() {
                layout.marks.push(PlacedMark {
                    top: y,
                    mark: mark.clone(),
                });
            }
            let h = laid.h;
            layout.items.push(Placed {
                y,
                laid,
                stamp: 0,
                index: 0,
                role,
            });
            y + h
        };
        debug_assert_eq!(chat.entries.len(), chat.stamps.len());
        let empty = chat.entries.is_empty();
        if !empty {
            let edge = if let Some(error) = &chat.older_error {
                Edge::Failed(error.clone())
            } else if chat.older_request.is_some() {
                Edge::Loading
            } else if chat.has_older {
                Edge::More
            } else {
                Edge::Beginning
            };
            y = single(
                self,
                &mut layout,
                y,
                hash_of(("edge", &edge)),
                Role::Other,
                &|| lay_edge(&edge),
            );
            gap = ROW_GAP;
            // The System Instructions box, pinned at the top (PAR-132).
            for (n, text) in chat.instructions.iter().enumerate() {
                let key = match n {
                    0 => "instructions".to_owned(),
                    n => format!("instructions#{}", n + 1),
                };
                let open = chat.is_open(&key, disclosure);
                y += gap;
                let toggle = Action::Chat(chat::Act::ToggleExpanded(key.clone()));
                y = single(
                    self,
                    &mut layout,
                    y,
                    hash_of(("instructions", text, &key, open)),
                    Role::Other,
                    &|| lay_instructions("System Instructions", text, toggle.clone(), open),
                );
            }
        }
        let live_follows = chat.live.is_some() && chat.echoes.is_empty();
        let (footers, live_base) = footer::footers(&chat.entries, live_follows);
        let now = wall_ms(Instant::now());
        // Whether the run of replies so far shows anything (else no lone
        // footer under folded work).
        let mut run_shows = false;
        for (at, (entry, stamp)) in chat.entries.iter().zip(&chat.stamps).enumerate() {
            let reply = matches!(entry, Entry::Ai { .. });
            if !reply {
                run_shows = false;
            }
            if let Some(fold) = &fold {
                // Work folded with no reply to carry its mark: a line in
                // the gap, where it was.
                if let Some(line) = fold.line(at) {
                    let y1 = y + gap - 3.0;
                    let y0 = y1 - GUTTER_GAP_LINE;
                    layout.fold_tops.entry(line.group.clone()).or_insert(y0);
                    layout.gutters.push(Gutter {
                        y0,
                        y1,
                        key: line.group.clone(),
                        open: false,
                        line: true,
                    });
                }
                if fold.hides(at) {
                    continue;
                }
            }
            let plan = self.plan(*stamp, entry);
            if plan.segments.is_empty() {
                continue;
            }
            let blocks = match entry {
                Entry::Ai { blocks, .. } => blocks.as_slice(),
                _ => &[],
            };
            let folded = fold.as_ref().and_then(|f| f.by_msg.get(&at));
            let shown = Shown {
                hidden: folded.and_then(|f| f.hidden.as_ref()),
                folding,
                merged: fold.as_ref().and_then(|f| f.merged.get(&at)),
                summary: folded.and_then(|f| {
                    f.summary
                        .as_deref()
                        .map(|label| (label, f.turn.as_str(), f.open))
                }),
                ..Shown::default()
            };
            let top = y + gap;
            let first = layout.items.len();
            let placed = self.place(
                &mut layout,
                top,
                &plan,
                Some(entry),
                *stamp,
                blocks,
                &context,
                shown,
            );
            run_shows |= placed.any && reply;
            let footer = footers[at]
                .as_ref()
                .filter(|_| !folding || (run_shows && !folded.is_some_and(|f| f.meta_hidden)));
            if !placed.any && footer.is_none() {
                continue;
            }
            y = top + placed.h;
            gap = placed.tail_gap;
            if let Some(meta) = footer {
                let line = footer::text(meta, now, false);
                let suspended = chat.suspended == Some(*stamp);
                let key = hash_of(("footer", &line, *stamp, suspended));
                let (laid, marks) = self.piece(key, || lay_footer(&line, *stamp, suspended));
                debug_assert!(marks.is_empty());
                let h = laid.h;
                layout.items.push(Placed {
                    y,
                    laid,
                    stamp: *stamp,
                    index: u32::MAX,
                    role: Role::Message,
                });
                y += h;
                gap = FOOTER_GAP;
            }
            let entry_top = layout.items.get(first).map_or(top, |item| item.y);
            layout.entry_tops.push((at, entry_top));
            if let Some(summary) = shown.summary {
                layout
                    .fold_tops
                    .entry(summary.1.to_owned())
                    .or_insert(entry_top);
            }
            if let Some(merged) = shown.merged {
                layout
                    .fold_tops
                    .entry(merged.turn.clone())
                    .or_insert(entry_top);
            }
            // The mark down a row that carries folded work, and down every
            // row of an opened group.
            if let Some(mark) = fold.as_ref().and_then(|f| f.mark(at)) {
                layout
                    .fold_tops
                    .entry(mark.group.clone())
                    .or_insert(entry_top);
                layout.gutters.push(Gutter {
                    y0: entry_top,
                    y1: y,
                    key: mark.group.clone(),
                    open: mark.open,
                    line: false,
                });
            }
        }
        for echo in &chat.echoes {
            y += gap;
            let key = hash_of(("echo", &echo.text));
            y = single(self, &mut layout, y, key, Role::Other, &|| {
                lay_bubble(&echo.text, Tone::Sending, None, false)
            });
            gap = ROW_GAP;
        }
        if let Some(live) = &chat.live {
            let live_context = Context {
                live: true,
                ..context
            };
            let live_fold = fold.as_ref().and_then(|f| f.live.as_ref());
            let status = &chat.status;
            y += gap;
            let mut h = 0.0;
            // The space above the next piece (none above the first).
            let mut next_gap = 0.0;
            // Folded, the live turn leads with WORKING and what folded of
            // it (a tap opens it), then its newest step (PAR-404).
            if let Some(lf) = live_fold
                && !status.any()
            {
                let (laid, marks) = self.piece(
                    hash_of(("working fold", &lf.summary, &lf.turn, lf.open)),
                    || lay_turn_fold(lf.summary.as_deref(), &lf.turn, lf.open, true),
                );
                let top = y - NOTIFICATION_TRIM;
                for mark in marks.iter() {
                    layout.marks.push(PlacedMark {
                        top,
                        mark: mark.clone(),
                    });
                }
                h = laid.h - NOTIFICATION_TRIM;
                layout.items.push(Placed {
                    y: top,
                    laid,
                    stamp: live.stamp,
                    index: u32::MAX - 2,
                    role: Role::Message,
                });
                layout.fold_tops.entry(lf.turn.clone()).or_insert(top);
                next_gap = BLOCK_GAP - NOTIFICATION_TRIM;
            }
            if let Some((msg, block)) = live_fold.and_then(|lf| lf.revealed)
                && let (Some(entry), Some(stamp)) = (chat.entries.get(msg), chat.stamps.get(msg))
            {
                let plan = self.plan(*stamp, entry);
                let blocks = match entry {
                    Entry::Ai { blocks, .. } => blocks.as_slice(),
                    _ => &[],
                };
                let shown = Shown {
                    only: Some(block),
                    gap: next_gap,
                    ..Shown::default()
                };
                let placed = self.place(
                    &mut layout,
                    y + h,
                    &plan,
                    Some(entry),
                    *stamp,
                    blocks,
                    &live_context,
                    shown,
                );
                if placed.any {
                    h += placed.h;
                    next_gap = BLOCK_GAP;
                }
            }
            let plan = plan_blocks(&live.blocks);
            let shown = Shown {
                hidden: live_fold.and_then(|lf| lf.hidden.as_ref()),
                folding,
                gap: next_gap,
                ..Shown::default()
            };
            let placed = self.place(
                &mut layout,
                y + h,
                &plan,
                None,
                live.stamp,
                &live.blocks,
                &live_context,
                shown,
            );
            if placed.any {
                h += placed.h;
            }
            let push = |view: &mut Self,
                        layout: &mut ChatLayout,
                        h: &mut f32,
                        key: u64,
                        make: &dyn Fn() -> Piece| {
                if *h > 0.0 {
                    *h += BLOCK_GAP;
                }
                let (laid, marks) = view.piece(key, make);
                for mark in marks.iter() {
                    layout.marks.push(PlacedMark {
                        top: y + *h,
                        mark: mark.clone(),
                    });
                }
                let piece_h = laid.h;
                layout.items.push(Placed {
                    y: y + *h,
                    laid,
                    stamp: live.stamp,
                    index: u32::MAX - 1,
                    role: Role::Message,
                });
                *h += piece_h;
            };
            if live_fold.is_none() && !live.blocks.iter().any(visible) && !status.any() {
                push(self, &mut layout, &mut h, hash_of("working"), &lay_working);
            }
            for (on, kind) in [
                (status.retry.is_some(), StatusKind::Retry),
                (status.compaction.is_some(), StatusKind::Compaction),
                (status.wait.is_some(), StatusKind::Wait),
            ] {
                if on {
                    push(
                        self,
                        &mut layout,
                        &mut h,
                        hash_of(("status", format!("{kind:?}"))),
                        &|| lay_status(kind),
                    );
                }
            }
            // The footer row: no gap above it.
            let base = if live_follows {
                live_base.clone()
            } else {
                None
            };
            let (laid, marks) = self.piece(
                hash_of(("live footer", live.stamp, format!("{base:?}"))),
                || lay_live_footer(live.stamp, base.clone()),
            );
            for mark in marks.iter() {
                layout.marks.push(PlacedMark {
                    top: y + h,
                    mark: mark.clone(),
                });
            }
            let footer_h = laid.h;
            layout.items.push(Placed {
                y: y + h,
                laid,
                stamp: live.stamp,
                index: u32::MAX,
                role: Role::Message,
            });
            y += h + footer_h;
            gap = FOOTER_GAP;
        } else if !chat.echoes.is_empty() {
            y += BLOCK_GAP;
            y = single(
                self,
                &mut layout,
                y,
                hash_of("working"),
                Role::Other,
                &lay_working,
            );
            gap = ROW_GAP;
        }
        // Queued messages (D's captions and actions: "queued: next turn",
        // "Removing…"; a tap opens the message's queue actions).
        for queued in model.queued_here() {
            y += gap;
            let caption = model.queued_caption(queued);
            let sending = model.composer.optimistic(&queued.id);
            let key = hash_of(("queued", &queued.id, &queued.text, &caption, sending));
            let id = queued.id.clone();
            y = single(self, &mut layout, y, key, Role::Queued(id.clone()), &|| {
                let mut piece = lay_bubble(&queued.text, Tone::Queued, Some(&caption), false);
                let h = piece.laid.h;
                piece.laid.hits.push((
                    RectF::new(0.0, 0.0, W, h),
                    Action::Composer(composer::Act::Queued(id.clone())),
                ));
                if sending {
                    blocks::queued_spinner(&mut piece, &caption);
                }
                piece
            });
            gap = ROW_GAP;
        }
        if let Some(error) = &chat.error
            && !empty
        {
            y += gap;
            y = single(
                self,
                &mut layout,
                y,
                hash_of(("error", error)),
                Role::Other,
                &|| lay_load_error(error),
            );
        }
        layout.marks.sort_by(|a, b| a.top.total_cmp(&b.top));
        layout.tools.sort_by(|a, b| a.0.total_cmp(&b.0));
        layout.gutters.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        layout.key = key;
        layout.height = y + TRANSCRIPT_BOTTOM;
        self.layout = layout;
        self.prune();
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

// ----- Scrolling, the reader's place, the tail ------------------------------------------

/// Following resumes within 12 pt of the tail (PAR-167).
const FOLLOW_SLACK: f32 = 33.0;
/// The jump button shows beyond 120 pt from the tail (PAR-169).
const JUMP_THRESHOLD: f32 = 332.0;
const JUMP_FADE: Duration = Duration::from_millis(180);
/// How long the transcript must be still before the button comes back.
const JUMP_SETTLE: Duration = Duration::from_millis(120);

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
            self.transcript.positioned = None;
            return;
        }
        let same = before.id == model.chat.id && before.id.is_some();
        // Another conversation: from its own place.
        if before.id.is_some() && before.id != model.chat.id {
            self.chat_scroll.reset();
            self.follow = true;
            self.transcript.positioned = None;
        }
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
        // Loaded: where it was read, or its latest response, or the end
        // (unless it was scrolled already).
        if model.chat.loaded
            && !model.chat.entries.is_empty()
            && self.transcript.positioned != model.chat.id
        {
            self.transcript.positioned = model.chat.id.clone();
            if self.follow && !self.chat_scroll.dragging() {
                self.place_opened(model, height);
            }
        }
        self.keep_pinned(model, height, now);
        let max = self.chat_max(model, height);
        if self.follow && !self.chat_scroll.dragging() {
            self.chat_scroll.reset();
            self.chat_scroll.offset = max;
        } else if !self.chat_scroll.dragging()
            && self.chat_scroll.value(now) >= max - FOLLOW_SLACK
            && max > 0.0
        {
            self.follow = true;
        }
    }

    /// Scrolls to an entry (its stamp), its top 8 pt below the
    /// transcript's top (the iPhone's `reveal`), and stops following (E: a
    /// Find in chat match). False when it isn't laid out.
    pub fn reveal(&mut self, model: &Model, stamp: u64, height: f32) -> bool {
        let max = self.chat_max(model, height);
        let Some(item) = self.layout.items.iter().find(|item| item.stamp == stamp) else {
            return false;
        };
        self.chat_scroll.reset();
        self.chat_scroll.offset = (item.y - 22.0).clamp(0.0, max);
        self.follow = false;
        true
    }

    /// A fold toggled from a tap: its first row where the tapped row was
    /// (what opened reads below it, in order).
    fn keep_pinned(&mut self, model: &Model, height: f32, now: Instant) {
        let Some(pin) = self.transcript.pin.take() else {
            return;
        };
        if model.chat.expanded.contains(&pin.key) == pin.open {
            if now.saturating_duration_since(pin.at) < PIN_WAIT {
                self.transcript.pin = Some(pin);
            }
            return;
        }
        let max = self.chat_max(model, height);
        if let Some(y) = self.layout.fold_tops.get(&pin.key) {
            self.follow = false;
            self.chat_scroll.reset();
            self.chat_scroll.offset = (y - pin.top).clamp(0.0, max);
        }
    }

    /// Where a chat opens (PAR-104, 105): at its saved reading place; an
    /// unread one at its latest response (when that is taller than the
    /// screen); else following the newest message.
    fn place_opened(&mut self, model: &Model, height: f32) {
        let chat = &model.chat;
        let Some(conv) = chat.id.as_deref() else {
            return;
        };
        let max = self.chat_max(model, height);
        let (top, bottom) = transcript_bounds(model, height, self.keyboard);
        let saved = model.saved_anchor(conv);
        let mut target = None;
        if let Some(SavedAnchor {
            key: Some(key),
            offset,
            ..
        }) = &saved
        {
            let keys = fold::keys(&chat.entries, chat.start);
            target = keys
                .iter()
                .position(|k| k == key)
                .and_then(|at| self.entry_top(at))
                .map(|y| y - offset);
        }
        if target.is_none()
            && saved.is_none()
            && chat.opened_unread
            && let Some(y) = latest_response(chat).and_then(|at| self.entry_top(at))
            && self.layout.height - y > bottom - top
        {
            target = Some(y - 22.0);
        }
        match target {
            Some(offset) => {
                self.follow = false;
                self.chat_scroll.reset();
                self.chat_scroll.offset = offset.clamp(0.0, max);
            }
            None => self.follow = true,
        }
        self.kept_at(chat, target.is_none());
    }

    /// Where entry `at` (or the first shown after it) begins.
    fn entry_top(&self, at: usize) -> Option<f32> {
        let tops = &self.layout.entry_tops;
        let from = tops.partition_point(|(index, _)| *index < at);
        tops.get(from).map(|(_, y)| *y)
    }

    /// Records the reading place as kept (`bottom`: at the newest message).
    fn kept_at(&mut self, chat: &Chat, bottom: bool) {
        if let Some(id) = &chat.id {
            let place = (!bottom).then(|| self.reading_place()).flatten();
            self.transcript.kept = Some((id.clone(), place));
        }
    }

    /// The entry at the top of the screen and its top below the
    /// transcript's (whole pixels).
    fn reading_place(&self) -> Option<(usize, i32)> {
        let offset = self.chat_scroll.offset;
        let tops = &self.layout.entry_tops;
        let at = tops.partition_point(|(_, y)| *y <= offset + 1.0);
        let (index, y) = tops.get(at.saturating_sub(1))?;
        Some((*index, (y - offset).round() as i32))
    }

    /// Keeps where the open chat is read (PAR-104) once the transcript has
    /// been still a moment, when it changed.
    fn keep_reading_place(&mut self, model: &mut Model, now: Instant) {
        let Some(id) = model.chat.id.clone() else {
            return;
        };
        let still = self
            .transcript
            .still_since
            .is_some_and(|since| now.saturating_duration_since(since) >= KEEP_AFTER);
        if !still
            || self.chat_scroll.dragging()
            || self.transcript.positioned.as_ref() != Some(&id)
            || !model.chat.loaded
        {
            return;
        }
        let place = if self.follow {
            None
        } else {
            self.reading_place()
        };
        if self.transcript.kept.as_ref() == Some(&(id.clone(), place)) {
            return;
        }
        let anchor = match place {
            None => SavedAnchor {
                conv: id.clone(),
                key: None,
                offset: 0.0,
            },
            Some((at, rel)) => SavedAnchor {
                conv: id.clone(),
                key: fold::keys(&model.chat.entries, model.chat.start)
                    .get(at)
                    .cloned(),
                offset: rel as f32,
            },
        };
        model.save_anchor(anchor);
        self.transcript.kept = Some((id, place));
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

/// Tool outputs are fetched once the transcript has been still this long.
const PREFETCH_AFTER: Duration = Duration::from_millis(300);
/// The reading place is kept once it has been still this long.
const KEEP_AFTER: Duration = Duration::from_millis(400);

/// The latest response: the first reply after the last user message.
fn latest_response(chat: &Chat) -> Option<usize> {
    let last = chat
        .entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::User { .. }))?;
    (last + 1..chat.entries.len()).find(|&at| matches!(chat.entries[at], Entry::Ai { .. }))
}

/// Folded tool rows on screen (and half a screen around) whose output is
/// not here yet, once the transcript is still.
fn prefetchable(model: &Model, view: &ViewState, height: f32, now: Instant) -> Vec<String> {
    let still = view
        .transcript
        .still_since
        .is_some_and(|since| now.saturating_duration_since(since) >= PREFETCH_AFTER);
    if !still
        || view.chat_scroll.dragging()
        || view.layout.tools.is_empty()
        || !model.chat.outputs_inflight.is_empty()
    {
        return Vec::new();
    }
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    let screen = bottom - top;
    let offset = view.chat_scroll.value(now);
    let tools = &view.layout.tools;
    let from = tools.partition_point(|(y, _)| *y < offset - screen / 2.0);
    tools[from..]
        .iter()
        .take_while(|(y, _)| *y < offset + 1.5 * screen)
        .map(|(_, id)| id)
        .filter(|id| {
            !model.chat.outputs.contains_key(*id)
                && !model.chat.outputs_requested.contains(*id)
                && !model.chat.output_errors.contains_key(*id)
        })
        .cloned()
        .collect()
}

/// What the chat asks the daemon for: older history near the top; the
/// outputs of folded tool rows on screen once it is still (never while it
/// scrolls: their answers cost the daemon and the link).
pub fn requests(model: &mut Model, view: &mut ViewState, height: f32, now: Instant) -> Vec<Value> {
    if model.page != Page::Chat {
        return Vec::new();
    }
    let mut commands = Vec::new();
    if view.near_top(now) {
        commands.extend(model.load_older());
    }
    // Show all work flipped: the folds opened one by one close.
    let show_all = Disclosure::of(&model.persist).show_all;
    if view
        .transcript
        .show_all
        .replace(show_all)
        .is_some_and(|was| was != show_all)
    {
        model.close_folds();
    }
    view.layout(model);
    view.keep_reading_place(model, now);
    let near = prefetchable(model, view, height, now);
    if !near.is_empty() {
        let near: Vec<&str> = near.iter().map(String::as_str).collect();
        commands.extend(model.prefetch_outputs(&near));
    }
    commands
}

// ----- Frames ---------------------------------------------------------------------------

/// The transcript's part of a chat frame beyond its pieces: its marks on
/// screen (content rows and what they show) and the jump button's fade.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranscriptFrame {
    marks: Vec<(i32, i32, u64)>,
    /// Fold marks on screen: their rows and how they show (closed, open,
    /// pressed).
    gutters: Vec<(i32, i32, u8)>,
    /// The jump button's opacity in 32nds (0: not drawn).
    jump: u32,
    jump_rows: (i32, i32),
}

/// The jump button's square (§3.6.14): 34 px from the right, 34 above the
/// island.
fn jump_rect(bottom: f32) -> RectF {
    RectF::new(W - JUMP_INSET - JUMP, bottom - 17.0 - JUMP, JUMP, JUMP)
}

/// Its shadow's reach: blur and drop. Compact (the spec's 44 px blur is
/// barely visible on the dark themes, and every row and column it reaches
/// is drawn and sent again when the button comes or goes over moved rows).
const JUMP_SHADOW: (f32, f32) = (14.0, 5.0);

/// The columns the jump button and its shadow cover.
pub(crate) fn jump_columns() -> i32 {
    (W - JUMP_INSET - JUMP - JUMP_SHADOW.0 - 2.0).floor() as i32
}

/// The jump button's opacity at `now`, 0..=32.
fn jump_alpha(view: &ViewState, now: Instant) -> u32 {
    match &view.transcript.jump {
        Some((_, tween)) => (tween.value(now).clamp(0.0, 1.0) * 32.0).round() as u32,
        None => 0,
    }
}

/// Records what the transcript shows beyond its pieces at `now` (and
/// starts the jump button's fade when it should show or go).
pub(crate) fn frame(
    model: &Model,
    view: &mut ViewState,
    (top, bottom): (f32, f32),
    offset: f32,
    pressed: Option<&Action>,
    now: Instant,
) -> TranscriptFrame {
    view.transcript.bounds = (top, bottom);
    let max = (view.layout.height - (bottom - top)).max(0.0);
    let loading = model.chat.older_request.is_some() || !model.chat.loaded;
    // Not while the transcript moves under it (a drag, a fling): a floating
    // button over moving rows would cost its rows in every frame. It fades
    // in once the transcript is still, and goes at once.
    let moving = view
        .transcript
        .last_offset
        .is_some_and(|last| last != offset)
        || view.chat_scroll.dragging();
    view.transcript.last_offset = Some(offset);
    if moving {
        view.transcript.still_since = None;
    } else if view.transcript.still_since.is_none() {
        view.transcript.still_since = Some(now);
    }
    let far = max - offset > JUMP_THRESHOLD && !loading;
    // Shown once the transcript has been still a moment: a fling's slow end
    // moves it a pixel now and then, and must not make it flicker.
    let settled = view
        .transcript
        .still_since
        .is_some_and(|since| now.saturating_duration_since(since) >= JUMP_SETTLE);
    view.transcript.held = far && !settled;
    let shown = view.transcript.jump.as_ref().is_some_and(|(on, _)| *on);
    if far && settled && !shown {
        let tween = Tween::new(now, JUMP_FADE, 0.0, 1.0);
        view.transcript.jump = Some((true, tween));
    } else if shown && (!far || moving) {
        view.transcript.jump = None;
    }
    let jump = jump_alpha(view, now);
    let rect = jump_rect(bottom);
    let jump_rows = (
        (rect.y - JUMP_SHADOW.0).floor() as i32,
        (rect.y + rect.h + JUMP_SHADOW.0 + JUMP_SHADOW.1).ceil() as i32,
    );
    let (lo, hi) = (offset, offset + bottom - top);
    let marks = view
        .layout
        .marks
        .iter()
        .filter(|m| {
            let (y0, y1) = m.rows();
            y1 > lo && y0 < hi
        })
        .map(|m| {
            let (y0, y1) = m.rows();
            (
                y0.floor() as i32,
                y1.ceil() as i32,
                mark_key(&m.mark, model, now),
            )
        })
        .collect();
    let gutters = view
        .layout
        .gutters
        .iter()
        .filter(|g| {
            let (y0, y1) = g.ink();
            y1 > lo && y0 < hi
        })
        .map(|g| {
            let (y0, y1) = g.ink();
            let state = if g.pressed(pressed) {
                2
            } else {
                u8::from(g.open)
            };
            (y0.floor() as i32, y1.ceil() as i32, state)
        })
        .collect();
    TranscriptFrame {
        marks,
        gutters,
        jump,
        jump_rows,
    }
}

/// Rows (on screen) to draw again between two frames for the marks: those
/// whose key changed, where they were and where they are.
pub(crate) fn mark_damage(
    old: &TranscriptFrame,
    new: &TranscriptFrame,
    offset: f32,
    (top, bottom): (i32, i32),
) -> Vec<(i32, i32)> {
    let screen = |(y0, y1): (i32, i32)| {
        let base = CHAT_TOP - offset;
        (
            ((base + y0 as f32).floor() as i32).max(top),
            ((base + y1 as f32).ceil() as i32).min(bottom),
        )
    };
    let mut rows = Vec::new();
    for (y0, y1, key) in &new.marks {
        if !old.marks.contains(&(*y0, *y1, *key)) {
            rows.push(screen((*y0, *y1)));
        }
    }
    for (y0, y1, key) in &old.marks {
        if !new.marks.contains(&(*y0, *y1, *key)) {
            rows.push(screen((*y0, *y1)));
        }
    }
    // Fold marks that came, went or changed (a press, Fold markers).
    for (y0, y1, state) in &new.gutters {
        if !old.gutters.contains(&(*y0, *y1, *state)) {
            rows.push(screen((*y0, *y1)));
        }
    }
    for (y0, y1, state) in &old.gutters {
        if !new.gutters.contains(&(*y0, *y1, *state)) {
            rows.push(screen((*y0, *y1)));
        }
    }
    rows.retain(|(y0, y1)| y1 > y0);
    rows
}

/// Rows of the jump button's columns to draw again: where it is when it
/// faded or the transcript moved under it (and where the move took its
/// old image).
pub(crate) fn jump_damage(
    old: &TranscriptFrame,
    new: &TranscriptFrame,
    shift: i32,
    (top, bottom): (i32, i32),
) -> Option<(i32, i32)> {
    if old.jump == 0 && new.jump == 0 {
        return None;
    }
    if shift == 0 && old.jump == new.jump && old.jump_rows == new.jump_rows {
        return None;
    }
    let (mut y0, mut y1) = new.jump_rows;
    if old.jump > 0 {
        // The move carried its old image up or down by `shift`.
        let (o0, o1) = old.jump_rows;
        y0 = y0.min(o0 - shift).min(o0);
        y1 = y1.max(o1 - shift).max(o1);
    }
    let (y0, y1) = (y0.max(top), y1.min(bottom));
    (y1 > y0).then_some((y0, y1))
}

/// Rows `rows` without those already drawn (`drawn`, merged and sorted).
pub(crate) fn subtract_rows(rows: (i32, i32), drawn: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out = vec![rows];
    for &(c0, c1) in drawn {
        out = out
            .into_iter()
            .flat_map(|(y0, y1)| {
                if c1 <= y0 || c0 >= y1 {
                    vec![(y0, y1)]
                } else {
                    [(y0, c0.max(y0)), (c1.min(y1), y1)]
                        .into_iter()
                        .filter(|(a, b)| b > a)
                        .collect()
                }
            })
            .collect();
    }
    out
}

/// Asks for the frames the transcript's marks and the jump button need.
pub fn animate(model: &Model, view: &ViewState, now: Instant, animator: &mut Animator) {
    if model.page != Page::Chat {
        return;
    }
    if stateful(model) {
        if !model.chat.loaded {
            animator.slow_frame(now);
        }
        return;
    }
    if let Some((_, tween)) = &view.transcript.jump {
        animator.follow(tween, now);
    }
    // Held back while the transcript moved: a frame once it may be still.
    if view.transcript.held {
        let since = view.transcript.still_since.unwrap_or(now);
        animator.request((since + JUMP_SETTLE).max(now + anim::FRAME));
    }
    // The reading place is kept once still: a frame to see it still, a
    // pass when it has been.
    match view.transcript.still_since {
        None if !view.chat_scroll.dragging() => animator.request(now + KEEP_AFTER),
        Some(since) if now.saturating_duration_since(since) < KEEP_AFTER => {
            animator.request(since + KEEP_AFTER);
        }
        _ => {}
    }
    // Outputs to fetch once still: a pass of the loop then.
    if let Some(since) = view.transcript.still_since
        && !view.layout.tools.is_empty()
        && model.chat.outputs_inflight.is_empty()
        && now.saturating_duration_since(since) < PREFETCH_AFTER
    {
        animator.request(since + PREFETCH_AFTER);
    }
    let (top, bottom) = view.transcript.bounds;
    let offset = view.chat_scroll.value(now);
    let (lo, hi) = (offset, offset + bottom - top);
    for placed in &view.layout.marks {
        let (y0, y1) = placed.rows();
        if y1 <= lo || y0 >= hi {
            continue;
        }
        match mark_frames(&placed.mark) {
            Some(every) if every >= Duration::from_millis(1000) => {
                // The next whole second of the clock.
                let ms = wall_ms(now);
                let next = 1000.0 - ms.rem_euclid(1000.0);
                animator.request(now + Duration::from_millis(next.ceil() as u64 + 1));
            }
            Some(every) => animator.request(now + every),
            None => {}
        }
    }
}

// ----- Drawing --------------------------------------------------------------------------

/// The transcript's items, marks and jump button that fall in rows `clip`,
/// scrolled to `offset` (and within the canvas's clip, its columns too).
pub(crate) fn draw_transcript(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    pressed: Option<&Action>,
    clip: (f32, f32),
    offset: f32,
    now: Instant,
) {
    let outer = canvas.clip();
    let (top, bottom) = (clip.0.max(outer.1 as f32), clip.1.min(outer.3 as f32));
    if top >= bottom {
        return;
    }
    let clip = (top, bottom);
    let area_top = CHAT_TOP;
    canvas.set_clip(outer.0, top as i32, outer.2, bottom as i32);
    // Pieces are in order: skip to the first that may reach the clip (no
    // glyph reaches a screen beyond its piece).
    let items = &view.layout.items;
    let first =
        items.partition_point(|item| area_top + item.y + item.laid.h - offset < top - 400.0);
    let text = theme::p().text;
    for item in &items[first..] {
        let y = area_top + item.y - offset;
        if y - 400.0 > bottom {
            break;
        }
        if y + item.laid.ink.1 < top || y + item.laid.ink.0 > bottom {
            continue;
        }
        draw_ops(canvas, &item.laid.ops, y, clip, &view.xscroll);
        // Pressed: a light veil over the target.
        for (rect, action) in &item.laid.hits {
            if pressed == Some(action) {
                let (x0, y0) = (px(rect.x), px(y + rect.y));
                let (x1, y1) = (px(rect.x + rect.w), px(y + rect.y + rect.h));
                canvas.fill_rect(x0, y0, x1 - x0, y1 - y0, text, 0.06);
            }
        }
    }
    let dy = area_top - offset;
    for gutter in &view.layout.gutters {
        let (y0, y1) = gutter.ink();
        if dy + y1 < top {
            continue;
        }
        if dy + y0 > bottom {
            break;
        }
        gutter.draw(canvas, dy, gutter.pressed(pressed));
    }
    let marks = &view.layout.marks;
    let first = marks.partition_point(|m| area_top + m.rows().1 - offset < top - 400.0);
    for placed in &marks[first..] {
        let dy = area_top + placed.top - offset;
        let (y0, y1) = (dy + placed.mark.y, dy + placed.mark.y + placed.mark.h);
        if y0 > bottom + 400.0 {
            break;
        }
        if y1 < top || y0 > bottom {
            continue;
        }
        draw_mark(canvas, &placed.mark, dy, model, now);
    }
    draw_jump(canvas, view, now);
    canvas.set_clip(outer.0, outer.1, outer.2, outer.3);
}

/// The jump button (§3.6.14) at its fade.
fn draw_jump(canvas: &mut Canvas, view: &ViewState, now: Instant) {
    let alpha = jump_alpha(view, now);
    if alpha == 0 {
        return;
    }
    let a = alpha as f32 / 32.0;
    let p = theme::p();
    let c = theme::c();
    let rect = jump_rect(view.transcript.bounds.1);
    jump_shadow(canvas, rect, 0.4 * a);
    canvas.fill_rect(
        rect.x as i32,
        rect.y as i32,
        rect.w as i32,
        rect.h as i32,
        p.app_background,
        a,
    );
    let b = theme::HAIRLINE as i32;
    let (x, y, w, h) = (rect.x as i32, rect.y as i32, rect.w as i32, rect.h as i32);
    for (x, y, w, h) in [
        (x, y, w, b),
        (x, y + h - b, w, b),
        (x, y + b, b, h - 2 * b),
        (x + w - b, y + b, b, h - 2 * b),
    ] {
        canvas.fill_rect(x, y, w, h, c.jump_border, a);
    }
    icons::draw_styled(
        canvas,
        Icon::ArrowDown,
        rect.x + rect.w / 2.0,
        rect.y + rect.h / 2.0,
        icons::Style::new(56.0, p.text).stroke(5.0).alpha(a),
    );
}

/// The jump button's soft shadow: a cached coverage mask of the rows and
/// columns around it (none under the button itself), blended in one pass.
fn jump_shadow(canvas: &mut Canvas, rect: RectF, alpha: f32) {
    use std::cell::OnceCell;
    let (blur, dy) = JUMP_SHADOW;
    let b = blur.ceil() as i32;
    let (w, h) = (rect.w as i32 + 2 * b, rect.h as i32 + 2 * b + dy as i32);
    thread_local! {
        static MASK: OnceCell<Vec<u8>> = const { OnceCell::new() };
    }
    let falloff = |d: f32| {
        let t = (d / blur).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    };
    MASK.with(|cell| {
        let mask = cell.get_or_init(|| {
            let (rw, rh) = (rect.w as i32, rect.h as i32);
            let mut mask = vec![0u8; (w * h) as usize];
            for my in 0..h {
                for mx in 0..w {
                    // Inside the button: nothing (it covers it).
                    let (bx, by) = (mx - b, my - b);
                    if (0..rw).contains(&bx) && (0..rh).contains(&by) {
                        continue;
                    }
                    let sy = by - dy as i32;
                    let ox = if bx < 0 {
                        -bx
                    } else if bx >= rw {
                        bx - rw + 1
                    } else {
                        0
                    };
                    let oy = if sy < 0 {
                        -sy
                    } else if sy >= rh {
                        sy - rh + 1
                    } else {
                        0
                    };
                    let a = (2.0 * falloff(ox as f32) * falloff(oy as f32)).min(1.0);
                    mask[(my * w + mx) as usize] = (a * 255.0).round() as u8;
                }
            }
            mask
        });
        canvas.draw_mask(
            rect.x as i32 - b,
            rect.y as i32 - b,
            w,
            h,
            mask,
            0x000000,
            alpha,
        );
    });
}

/// Draws rows `y0..y1` of the transcript again, within columns `x0..`:
/// background, pieces, marks, the jump button.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_band(
    canvas: &mut Canvas,
    model: &Model,
    view: &ViewState,
    pressed: Option<&Action>,
    (y0, y1): (i32, i32),
    x0: i32,
    offset: f32,
    now: Instant,
) {
    let outer = canvas.clip();
    canvas.set_clip(x0.max(outer.0), outer.1, outer.2, outer.3);
    canvas.fill_rect(
        x0,
        y0,
        W as i32 - x0,
        y1 - y0,
        theme::p().app_background,
        1.0,
    );
    draw_transcript(
        canvas,
        model,
        view,
        pressed,
        (y0 as f32, y1 as f32),
        offset,
        now,
    );
    canvas.set_clip(outer.0, outer.1, outer.2, outer.3);
}

// ----- States, touches ---------------------------------------------------------------------

/// The chat shows a state block instead of a transcript: loading, an error
/// or no messages (drawn whole each frame, no pieces).
pub fn stateful(model: &Model) -> bool {
    let chat = &model.chat;
    chat.id.is_some()
        && (!chat.loaded
            || (chat.entries.is_empty()
                && chat.live.is_none()
                && chat.echoes.is_empty()
                && model.queued_here().is_empty()))
}

/// The state block's Retry button, when it has one.
fn state_button(
    model: &Model,
    (top, bottom): (f32, f32),
) -> Option<(RectF, widgets::Button<'static>)> {
    model.chat.error.as_ref()?;
    let button = widgets::Button::new("Retry", widgets::ButtonKind::Secondary);
    let w = button.width();
    let cy = ((top + bottom) / 2.0).round();
    Some((
        RectF::new(((W - w) / 2.0).round(), cy + 40.0, w, theme::TARGET),
        button,
    ))
}

/// Loading, the load error with Retry, or "No messages" (PAR-014, 092,
/// 102, 145), centred in the transcript.
pub(crate) fn draw_state(
    canvas: &mut Canvas,
    model: &Model,
    (top, bottom): (f32, f32),
    pressed: Option<&Action>,
    now: Instant,
) {
    let p = theme::p();
    let cx = W / 2.0;
    let cy = ((top + bottom) / 2.0).round();
    if !model.chat.loaded {
        let label = "Loading conversation…";
        widgets::spinner(
            canvas,
            cx,
            cy - 40.0,
            theme::SPINNER,
            p.accent,
            p.app_background,
            now,
        );
        widgets::text_centered(
            canvas,
            label,
            cx,
            theme::T_NOTE.baseline(cy),
            theme::T_NOTE,
            p.muted,
            1.0,
        );
        return;
    }
    match (&model.chat.error, state_button(model, (top, bottom))) {
        (Some(error), Some((rect, button))) => {
            let lines = widgets::wrap_plain(error, theme::T_NOTE, W - 2.0 * theme::S4 * 2.0);
            let mut y = rect.y - 34.0 - lines.len() as f32 * theme::T_NOTE.line();
            for line in &lines {
                widgets::text_centered(
                    canvas,
                    line,
                    cx,
                    theme::T_NOTE.baseline(y),
                    theme::T_NOTE,
                    p.error,
                    1.0,
                );
                y += theme::T_NOTE.line();
            }
            let down = pressed == Some(&Action::Chat(chat::Act::Retry));
            widgets::button(canvas, rect, &button.pressed(down), p.app_background);
        }
        _ => {
            widgets::text_centered(
                canvas,
                "No messages",
                cx,
                theme::T_NOTE.baseline(cy - theme::T_NOTE.line() / 2.0),
                theme::T_NOTE,
                p.muted,
                1.0,
            );
        }
    }
}

/// What a touch on the state block hits (its Retry).
pub(crate) fn state_hit(model: &Model, bounds: (f32, f32), x: f32, y: f32) -> Option<Action> {
    let (rect, _) = state_button(model, bounds)?;
    rect.contains(x, y)
        .then_some(Action::Chat(chat::Act::Retry))
}

/// What a touch at (`x`, `y`) hits in the transcript: the jump button,
/// then a piece's target.
pub(crate) fn hit(
    view: &mut ViewState,
    model: &Model,
    (top, bottom): (f32, f32),
    x: f32,
    y: f32,
    now: Instant,
) -> Option<Action> {
    if jump_alpha(view, now) > 16 && jump_rect(bottom).contains(x, y) {
        return Some(Action::Chat(chat::Act::JumpToLatest));
    }
    view.layout(model);
    let offset = view.chat_scroll.value(now);
    // A fold mark at the left edge.
    if x < GUTTER_W
        && let Some(gutter) = view.layout.gutters.iter().find(|g| {
            let (t0, t1) = g.target();
            y >= top + t0 - offset && y < top + t1 - offset
        })
    {
        let (key, row) = (gutter.key.clone(), gutter.y0 - offset);
        view.transcript.pin_fold(model, key, row, now);
        return Some(gutter.action());
    }
    let items = &view.layout.items;
    let first = items.partition_point(|item| top + item.y + item.laid.h - offset <= y);
    for item in &items[first..] {
        let iy = top + item.y - offset;
        if y < iy {
            break;
        }
        if y >= iy + item.laid.h {
            continue;
        }
        for (rect, action) in &item.laid.hits {
            if x >= rect.x && x < rect.x + rect.w && y >= iy + rect.y && y < iy + rect.y + rect.h {
                // A turn's row keeps its spot as it opens or folds.
                if let Action::Chat(chat::Act::ToggleExpanded(key)) = action
                    && (key.starts_with("turn:") || key.starts_with("group:"))
                {
                    let (key, row) = (key.clone(), item.y - offset);
                    let action = action.clone();
                    view.transcript.pin_fold(model, key, row, now);
                    return Some(action);
                }
                return Some(action.clone());
            }
        }
        // Inside scrollers (links in tables, C1), where they are now.
        for hit in &item.laid.scrolled_hits {
            if hit.contains(x, y - iy, &view.xscroll) {
                return Some(hit.action.clone());
            }
        }
    }
    None
}

/// A long press on a message opens its menu (PAR-142); on a queued one,
/// the queue's.
pub(crate) fn long_press(
    model: &Model,
    view: &mut ViewState,
    (top, bottom): (f32, f32),
    y: f32,
    now: Instant,
) -> Option<Action> {
    if y < top || y >= bottom || stateful(model) {
        return None;
    }
    view.layout(model);
    let offset = view.chat_scroll.value(now);
    let item = view.layout.items.iter().find(|item| {
        let iy = top + item.y - offset;
        y >= iy && y < iy + item.laid.h
    })?;
    match &item.role {
        Role::Message if item.stamp != 0 => Some(Action::Chat(chat::Act::MessageMenu(item.stamp))),
        Role::Queued(id) => Some(Action::Composer(composer::Act::Queued(id.clone()))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;
    use crate::exocortex::view::{Hit, chat_frame, draw, draw_chat_update};
    use crate::gfx::Canvas;

    const HEIGHT: f32 = 2216.0;

    /// A conversation open on `entries` (JSON, comma-separated).
    fn open_with(entries: &str) -> Model {
        let mut model = Model::new();
        let commands = model.on_connected();
        let ping = commands.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            Instant::now(),
        );
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","provider":"anthropic","model":"claude-opus-5-5","effort":"high","title":"Test","pinned":false,"sortOrder":0,"streaming":false,"unread":false}}"#),
            Instant::now(),
        );
        let commands = model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let request = commands
            .iter()
            .find(|c| c["type"] == "load_conversation")
            .unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_loaded","reqId":"{request}","convId":"c","entries":[{entries}],"historyStartIndex":0,"hasOlderHistory":false}}"#
            )),
            Instant::now(),
        );
        model.persist.update_settings(|s| s.show_all_work = true);
        model
    }

    /// The same, folded (the default).
    fn open_folded(entries: &str) -> Model {
        let mut model = open_with(entries);
        model.persist.update_settings(|s| s.show_all_work = false);
        model
    }

    fn feed(model: &mut Model, lines: &[&str]) {
        for line in lines {
            model.apply(parse_event(line), Instant::now());
        }
    }

    fn long_entries(n: usize) -> String {
        (0..n)
            .map(|i| format!(r#"{{"type":"user","text":"Question {i}"}},{{"type":"ai","blocks":[{{"type":"text","text":"Answer {i}, which runs over a couple of lines so the transcript is long enough to scroll well past one screen."}}],"metadata":{{"startedAt":{},"endedAt":{},"model":"claude-opus-5-5","tokens":10}}}}"#, 1000 + i * 10, 1005 + i * 10))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Draws at `t0`, changes, then draws the next frame incrementally at
    /// `t1`: it must equal a full redraw and change nothing outside its
    /// damage. Returns the damaged rows.
    fn update(
        model: &mut Model,
        view: &mut ViewState,
        (t0, t1): (Instant, Instant),
        change: impl FnOnce(&mut Model, &mut ViewState),
    ) -> crate::exocortex::view::FrameDamage {
        let mut canvas = Canvas::new(1080, HEIGHT as i32);
        canvas.clear(0);
        let old = chat_frame(model, view, None, HEIGHT, t0).expect("a chat frame");
        draw(&mut canvas, model, view, None, t0);
        let mut shadow = canvas.pixels.clone();
        change(model, view);
        let new = chat_frame(model, view, None, HEIGHT, t1).expect("a chat frame");
        let damage =
            draw_chat_update(&mut canvas, model, view, &old, &new, t1).expect("incremental");
        let mut full = Canvas::new(1080, HEIGHT as i32);
        full.clear(0);
        draw(&mut full, model, view, None, t1);
        let width = 1080usize;
        for y in 0..HEIGHT as usize {
            let row = y * width..(y + 1) * width;
            assert!(
                full.pixels[row.clone()] == canvas.pixels[row],
                "row {y} differs from a full redraw: {damage:?}"
            );
        }
        if let Some((y0, y1, dy)) = damage.moved {
            let rows = y1 - y0 - dy.abs();
            let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
            shadow.copy_within(
                src as usize * width..(src + rows) as usize * width,
                dst as usize * width,
            );
        }
        crate::exocortex::view::frame::assert_unchanged_outside(
            &shadow,
            &canvas.pixels,
            width,
            &damage,
        );
        damage
    }

    /// Rows damaged in the transcript (the header pulses on its own).
    fn rows(damage: &crate::exocortex::view::FrameDamage) -> i32 {
        let top = CHAT_TOP as i32;
        let rows: i32 = damage
            .rows
            .iter()
            .map(|(a, b)| (b - a.max(&top)).max(0))
            .sum();
        rows + damage.areas.iter().map(|(_, a, _, b)| b - a).sum::<i32>()
    }

    const START: &str = r#"{"type":"streaming_started","convId":"c","provider":"anthropic","model":"claude-opus-5-5","startedAt":5000,"snapshotKind":"start"}"#;

    #[test]
    fn completion_keeps_the_reply_its_pieces_and_its_height() {
        let mut model = open_with(r#"{"type":"user","text":"Write"}"#);
        feed(
            &mut model,
            &[
                START,
                r#"{"type":"text_chunk","convId":"c","text":"First.\n\nSecond paragraph."}"#,
            ],
        );
        let mut view = ViewState::new();
        view.layout(&model);
        let live: Vec<(f32, Rc<Laid>)> = view
            .layout
            .items
            .iter()
            .map(|i| (i.y, i.laid.clone()))
            .collect();
        let height = view.layout.height;
        feed(
            &mut model,
            &[
                r#"{"type":"message_complete","convId":"c","blocks":[{"type":"text","text":"First.\n\nSecond paragraph."}],"endedAt":9000,"tokens":5}"#,
            ],
        );
        view.layout(&model);
        assert_eq!(view.layout.height, height, "no height change at completion");
        let done: Vec<(f32, Rc<Laid>)> = view
            .layout
            .items
            .iter()
            .map(|i| (i.y, i.laid.clone()))
            .collect();
        assert_eq!(live.len(), done.len());
        // Every piece but the footer's row is the same, at the same place.
        for ((y0, a), (y1, b)) in live.iter().zip(&done).take(live.len() - 1) {
            assert!(y0 == y1 && Rc::ptr_eq(a, b));
        }
        // The daemon's copy changes nothing on screen.
        feed(
            &mut model,
            &[
                r#"{"type":"history_updated","convId":"c","entries":[{"type":"user","text":"Write"},{"type":"ai","blocks":[{"type":"text","text":"First.\n\nSecond paragraph."}],"metadata":{"startedAt":5000,"endedAt":9000,"model":"claude-opus-5-5","tokens":5}}],"historyStartIndex":0,"pendingAI":null}"#,
            ],
        );
        view.layout(&model);
        assert_eq!(view.layout.height, height);
        let settled: Vec<(f32, Rc<Laid>)> = view
            .layout
            .items
            .iter()
            .map(|i| (i.y, i.laid.clone()))
            .collect();
        assert!(
            done.iter()
                .zip(&settled)
                .all(|((y0, a), (y1, b))| y0 == y1 && Rc::ptr_eq(a, b))
        );
    }

    #[test]
    fn the_reader_stays_put_through_completion_and_keyboard() {
        let mut model = open_with(&long_entries(12));
        feed(
            &mut model,
            &[
                START,
                r#"{"type":"text_chunk","convId":"c","text":"Streaming now."}"#,
            ],
        );
        let mut view = ViewState::new();
        let now = Instant::now();
        let max = view.chat_max(&model, HEIGHT);
        view.follow = false;
        view.chat_scroll.offset = (max / 2.0).round();
        let screen_y = |view: &mut ViewState, model: &Model, laid: &Rc<Laid>| {
            view.layout(model);
            let item = view
                .layout
                .items
                .iter()
                .find(|i| Rc::ptr_eq(&i.laid, laid))
                .unwrap();
            item.y - view.chat_scroll.value(now)
        };
        view.layout(&model);
        let first = view
            .layout
            .items
            .iter()
            .find(|i| i.y >= view.chat_scroll.offset && i.stamp != 0)
            .unwrap()
            .laid
            .clone();
        let at = screen_y(&mut view, &model, &first);
        // Completion and the daemon's copy below it.
        let before = view.before(&model, now);
        feed(
            &mut model,
            &[
                r#"{"type":"message_complete","convId":"c","blocks":[{"type":"text","text":"Streaming now."}],"endedAt":9999,"tokens":3}"#,
            ],
        );
        view.settle(&model, before, HEIGHT, now);
        assert_eq!(screen_y(&mut view, &model, &first), at);
        // Keyboard up (the window shrinks), then down again.
        for height in [HEIGHT - 900.0, HEIGHT] {
            let before = view.before(&model, now);
            view.settle(&model, before, height, now);
            assert_eq!(screen_y(&mut view, &model, &first), at);
        }
        // Following the tail, the tail stays in view as the window shrinks.
        view.follow = true;
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT - 900.0, now);
        let max = view.chat_max(&model, HEIGHT - 900.0);
        assert_eq!(view.chat_scroll.offset, max);
    }

    #[test]
    fn following_resumes_within_twelve_points_of_the_tail() {
        let model = open_with(&long_entries(12));
        let mut view = ViewState::new();
        let now = Instant::now();
        let max = view.chat_max(&model, HEIGHT);
        view.follow = false;
        view.chat_scroll.offset = max - 40.0;
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT, now);
        assert!(!view.follow);
        view.chat_scroll.offset = max - 30.0;
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT, now);
        assert!(view.follow);
    }

    #[test]
    fn the_jump_button_shows_far_from_the_tail_and_not_while_loading() {
        let mut model = open_with(&long_entries(12));
        let mut view = ViewState::new();
        let t0 = Instant::now();
        let max = view.chat_max(&model, HEIGHT);
        view.follow = false;
        view.chat_scroll.offset = max - 300.0;
        chat_frame(&model, &mut view, None, HEIGHT, t0);
        assert_eq!(
            jump_alpha(&view, t0 + JUMP_FADE),
            0,
            "within 120 pt: hidden"
        );
        view.chat_scroll.offset = max - 400.0;
        chat_frame(&model, &mut view, None, HEIGHT, t0);
        assert_eq!(jump_alpha(&view, t0 + JUMP_FADE), 0, "not while it moves");
        chat_frame(&model, &mut view, None, HEIGHT, t0);
        assert_eq!(
            jump_alpha(&view, t0 + JUMP_FADE),
            0,
            "not before it settles"
        );
        let settled = t0 + JUMP_SETTLE;
        chat_frame(&model, &mut view, None, HEIGHT, settled);
        let shown = settled + JUMP_FADE;
        assert_eq!(jump_alpha(&view, shown), 32);
        // Its tap jumps to the newest message.
        let (top, bottom) = transcript_bounds(&model, HEIGHT, false);
        let rect = jump_rect(bottom);
        assert_eq!(
            hit(
                &mut view,
                &model,
                (top, bottom),
                rect.x + 60.0,
                rect.y + 60.0,
                shown
            ),
            Some(Action::Chat(chat::Act::JumpToLatest))
        );
        // Hidden while older history loads.
        model.chat.has_older = true;
        model.chat.older_request = Some("r".into());
        model.chat.touch();
        chat_frame(&model, &mut view, None, HEIGHT, shown);
        assert_eq!(jump_alpha(&view, shown + JUMP_FADE), 0);
    }

    #[test]
    fn the_jump_button_steps_aside_while_the_transcript_moves() {
        let mut model = open_with(&long_entries(16));
        let mut view = ViewState::new();
        let t0 = Instant::now();
        let max = view.chat_max(&model, HEIGHT);
        view.follow = false;
        view.chat_scroll.offset = (max / 2.0).round();
        chat_frame(&model, &mut view, None, HEIGHT, t0);
        chat_frame(&model, &mut view, None, HEIGHT, t0);
        chat_frame(&model, &mut view, None, HEIGHT, t0 + JUMP_SETTLE);
        let t = t0 + JUMP_SETTLE + JUMP_FADE;
        assert_eq!(jump_alpha(&view, t), 32);
        // The first step of a scroll takes it away (its rows drawn once).
        let damage = update(&mut model, &mut view, (t, t), |_, view| {
            view.chat_scroll.offset += 37.0;
        });
        assert!(damage.moved.is_some());
        assert_eq!(jump_alpha(&view, t), 0);
        // Then a scroll costs only the strip it uncovers.
        for step in [-20.0, 5.0, 120.0] {
            let damage = update(&mut model, &mut view, (t, t), |_, view| {
                view.chat_scroll.offset += step;
            });
            assert!(rows(&damage) <= step.abs() as i32 + 2, "{damage:?}");
        }
        // Moving, it asks for a frame to look again once still.
        let mut animator = Animator::default();
        animate(&model, &view, t, &mut animator);
        assert_eq!(animator.take(), Some(t + JUMP_SETTLE));
        // A pixel now and then (a fling's end) does not bring it back.
        let damage = update(&mut model, &mut view, (t, t + JUMP_SETTLE / 2), |_, _| {});
        assert!(damage.moved.is_none() && rows(&damage) == 0, "{damage:?}");
        let damage = update(
            &mut model,
            &mut view,
            (t, t + JUMP_SETTLE / 2),
            |_, view| {
                view.chat_scroll.offset += 1.0;
            },
        );
        assert!(rows(&damage) <= 3, "{damage:?}");
        // Still a moment: it fades back in, in its own columns.
        let t = t + JUMP_SETTLE;
        chat_frame(&model, &mut view, None, HEIGHT, t);
        let damage = update(&mut model, &mut view, (t, t + JUMP_SETTLE), |_, _| {});
        assert!(damage.moved.is_none());
        assert!(rows(&damage) <= 122 + 2 * 44 + 11 + 4, "{damage:?}");
        assert!(jump_alpha(&view, t + JUMP_SETTLE + JUMP_FADE) == 32);
    }

    #[test]
    fn tool_outputs_are_fetched_only_once_the_transcript_is_still() {
        let calls: Vec<String> = (0..4)
            .map(|i| format!(r#"{{"type":"tool_call","toolCallId":"t{i}","toolName":"Bash","input":{{}},"summary":"ls {i}"}},{{"type":"tool_result","toolCallId":"t{i}","toolName":"Bash","output":"","isError":false}},{{"type":"text","text":"step {i}"}}"#))
            .collect();
        let mut model = open_with(&format!(
            r#"{{"type":"ai","blocks":[{}]}}"#,
            calls.join(",")
        ));
        let mut view = ViewState::new();
        let t0 = Instant::now();
        chat_frame(&model, &mut view, None, HEIGHT, t0);
        let outputs = |commands: Vec<Value>| {
            commands
                .iter()
                .filter(|c| c["type"] == "load_tool_outputs")
                .count()
        };
        // Not yet still long enough; then asked for, once.
        assert_eq!(outputs(requests(&mut model, &mut view, HEIGHT, t0)), 0);
        let later = t0 + PREFETCH_AFTER;
        assert_eq!(outputs(requests(&mut model, &mut view, HEIGHT, later)), 1);
        assert_eq!(outputs(requests(&mut model, &mut view, HEIGHT, later)), 0);
        // Their answer changes nothing on screen: no layout pass.
        let version = model.chat.version;
        let request = model.chat.outputs_inflight.keys().next().unwrap().clone();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"tool_outputs_loaded","reqId":"{request}","convId":"c","outputs":[{{"toolCallId":"t0","output":"a"}},{{"toolCallId":"t1","output":"b"}},{{"toolCallId":"t2","output":"c"}},{{"toolCallId":"t3","output":"d"}}]}}"#
            )),
            Instant::now(),
        );
        assert_eq!(model.chat.version, version);
        assert_eq!(model.chat.output_of("t2"), Some("c"));
        // While it moves, nothing is asked for.
        let mut model = open_with(&format!(
            r#"{{"type":"ai","blocks":[{}]}}"#,
            calls.join(",")
        ));
        let mut view = ViewState::new();
        chat_frame(&model, &mut view, None, HEIGHT, t0);
        view.chat_scroll.begin(500.0, later);
        assert_eq!(outputs(requests(&mut model, &mut view, HEIGHT, later)), 0);
    }

    #[test]
    fn the_live_footer_ticks_in_its_own_rows() {
        let mut model = open_with(r#"{"type":"user","text":"Go"}"#);
        feed(
            &mut model,
            &[
                START,
                r#"{"type":"text_chunk","convId":"c","text":"Working on it."}"#,
            ],
        );
        let mut view = ViewState::new();
        view.chat_max(&model, HEIGHT);
        let t0 = Instant::now();
        // Two seconds later: only the footer's line changed.
        let damage = update(
            &mut model,
            &mut view,
            (t0, t0 + Duration::from_millis(2000)),
            |_, _| {},
        );
        assert!(damage.moved.is_none());
        assert!(rows(&damage) <= 2 * 40 + 2, "{damage:?}");
        assert!(rows(&damage) > 0, "the clock moved");
        // A new rate (tokens_update) without a layout pass.
        let damage = update(&mut model, &mut view, (t0, t0), |model, _| {
            model.apply(
                parse_event(r#"{"type":"tokens_update","convId":"c","tokens":40,"generationThroughput":{"provider":"anthropic","model":"claude-opus-5-5","rates":[61.5]}}"#),
                Instant::now(),
            );
        });
        assert!(rows(&damage) <= 2 * 40 + 2, "{damage:?}");
    }

    #[test]
    fn a_running_tool_spins_in_its_own_rows() {
        let mut model = open_with(r#"{"type":"user","text":"Go"}"#);
        feed(
            &mut model,
            &[
                START,
                r#"{"type":"tool_call","convId":"c","toolCallId":"t1","toolName":"Bash","input":{"command":"sleep 5"},"summary":"sleep 5"}"#,
            ],
        );
        let mut view = ViewState::new();
        view.chat_max(&model, HEIGHT);
        assert!(
            view.layout
                .marks
                .iter()
                .any(|m| matches!(m.mark.kind, super::super::blocks::MarkKind::Spinner { .. }))
        );
        let t0 = Instant::now();
        for step in 1..4 {
            let at = t0 + Duration::from_millis(step * 100);
            let damage = update(&mut model, &mut view, (t0, at), |_, _| {});
            assert!(
                rows(&damage) <= 2 * (28 + 4) + 2 * 40 + 4,
                "spinner and clock only: {damage:?}"
            );
        }
        // Its result: the row keeps its height; the spinner goes.
        let damage = update(&mut model, &mut view, (t0, t0), |model, _| {
            model.apply(
                parse_event(r#"{"type":"tool_result","convId":"c","toolCallId":"t1","toolName":"Bash","output":"ok","isError":false}"#),
                Instant::now(),
            );
        });
        assert!(rows(&damage) < 200, "the row alone: {damage:?}");
        assert!(
            view.layout
                .marks
                .iter()
                .all(|m| !matches!(m.mark.kind, super::super::blocks::MarkKind::Spinner { .. }))
        );
    }

    #[test]
    fn status_lines_show_under_the_live_reply() {
        let mut model = open_with(r#"{"type":"user","text":"Go"}"#);
        feed(
            &mut model,
            &[
                START,
                r#"{"type":"stream_retry","convId":"c","attempt":1,"maxAttempts":5,"errorMessage":"Overloaded","delaySec":30}"#,
                r#"{"type":"connection_wait_status","convId":"c","active":true,"startedAt":1}"#,
            ],
        );
        let mut view = ViewState::new();
        view.chat_max(&model, HEIGHT);
        let kinds: Vec<StatusKind> = view
            .layout
            .marks
            .iter()
            .filter_map(|m| match m.mark.kind {
                super::super::blocks::MarkKind::Status(kind) => Some(kind),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, [StatusKind::Retry, StatusKind::Wait]);
        // No WORKING while a status speaks for the reply.
        assert!(!view.layout.marks.iter().any(|m| matches!(&m.mark.kind, super::super::blocks::MarkKind::Shimmer { text, .. } if text == "WORKING")));
        let t0 = Instant::now();
        update(
            &mut model,
            &mut view,
            (t0, t0 + Duration::from_millis(1100)),
            |_, _| {},
        );
    }

    #[test]
    fn notifications_fold_notices_show_and_model_only_ones_do_not() {
        const ENTRIES: &str = r#"{"type":"user","text":"[notification] Discord/main · dm\nHello there from Discord","metadata":{"startedAt":1,"model":"","tokens":0,"automation":{"kind":"external_notification"}}},{"type":"system","text":"Claude account switched from main to max2","color":"warning","metadata":{"startedAt":2,"model":"","tokens":0,"kind":"provider_notice"}},{"type":"system","text":"client host info","metadata":{"startedAt":3,"model":"","tokens":0,"kind":"client_host"}},{"type":"system","text":"Usage limit reached","color":"error","metadata":{"startedAt":4,"model":"","tokens":0}},{"type":"system","text":"--- Compaction finished ---","metadata":{"startedAt":5,"model":"","tokens":0,"kind":"context_compaction_finished","system":true}}"#;
        let texts = |laid: &Laid| -> Vec<String> {
            laid.ops
                .iter()
                .filter_map(|op| match op {
                    crate::exocortex::view::ops::Op::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        // All work shown: the notification in full, the notices.
        let model = open_with(ENTRIES);
        let mut view = ViewState::new();
        view.layout(&model);
        // The edge, the automated bubble, the provider notice, the error,
        // the divider (not the model-only notice).
        assert_eq!(view.layout.items.len(), 5);
        let bubble = texts(&view.layout.items[1].laid);
        assert!(
            bubble
                .iter()
                .any(|t| t.contains("Notification · from Discord/main · dm")),
            "{bubble:?}"
        );
        assert!(view.layout.items[2].laid.h >= super::super::blocks::HEAD);
        assert!(view.layout.gutters.is_empty());
        // Folded: what nothing answered leaves a line in the gap; the error
        // stays (only informational notices fold).
        let mut model = open_folded(ENTRIES);
        let mut view = ViewState::new();
        view.layout(&model);
        assert_eq!(view.layout.items.len(), 2, "the edge and the error");
        assert!(
            texts(&view.layout.items[1].laid)
                .iter()
                .any(|t| t.contains("Usage limit"))
        );
        let [line] = &view.layout.gutters[..] else {
            panic!("{:?}", view.layout.gutters);
        };
        let line = line.clone();
        assert!(line.line && !line.open);
        // A tap on it opens them all, in full.
        let (top, bottom) = transcript_bounds(&model, HEIGHT, false);
        let now = Instant::now();
        let y = top + (line.y0 + line.y1) / 2.0;
        let action = hit(&mut view, &model, (top, bottom), 10.0, y, now).unwrap();
        assert_eq!(action, line.action());
        model.handle(action);
        view.layout(&model);
        assert_eq!(view.layout.items.len(), 5);
        assert!(
            texts(&view.layout.items[1].laid)
                .iter()
                .any(|t| t.contains("Hello there"))
        );
        assert!(view.layout.gutters.iter().all(|g| g.open && !g.line));
    }

    #[test]
    fn a_folded_turn_shows_its_reply_under_a_mark_that_opens_its_work() {
        let calls: Vec<String> = (0..4)
            .map(|i| format!(r#"{{"type":"tool_call","toolCallId":"t{i}","toolName":"Bash","input":{{"command":"ls {i}"}},"summary":"ls {i}"}},{{"type":"tool_result","toolCallId":"t{i}","toolName":"Bash","output":"ok","isError":false}}"#))
            .collect();
        let mut model = open_folded(&format!(
            r#"{{"type":"user","text":"Please list","metadata":{{"startedAt":1000,"model":"m","tokens":0}}}},{{"type":"ai","blocks":[{{"type":"thinking","text":"Listing."}},{},{{"type":"text","text":"Done."}}],"metadata":{{"startedAt":1001,"endedAt":2000,"model":"claude-opus-5-5","tokens":10}}}}"#,
            calls.join(",")
        ));
        let mut view = ViewState::new();
        let now = Instant::now();
        view.layout(&model);
        // The edge, the prompt, the reply, its footer: no thinking, no calls.
        assert_eq!(view.layout.items.len(), 4);
        let reply = view.layout.items[2].laid.clone();
        let [mark] = &view.layout.gutters[..] else {
            panic!("{:?}", view.layout.gutters);
        };
        assert_eq!(mark.key, "group:u:1000");
        assert!(!mark.line && !mark.open);
        assert_eq!(mark.y0, view.layout.items[2].y);
        assert_eq!(
            mark.y1,
            view.layout.items[3].y + view.layout.items[3].laid.h
        );
        // The mark opens the work; the first opened row comes where the
        // reply was.
        let (top, bottom) = transcript_bounds(&model, HEIGHT, false);
        let before = view.before(&model, now);
        let at = view.layout.items[2].y - view.chat_scroll.value(now);
        let action = hit(&mut view, &model, (top, bottom), 12.0, top + at + 30.0, now).unwrap();
        assert_eq!(
            action,
            Action::Chat(chat::Act::ToggleExpanded("group:u:1000".into()))
        );
        model.handle(action);
        view.follow = false;
        view.settle(&model, before, HEIGHT, now);
        // The thinking, four calls, the same reply piece, the footer.
        assert_eq!(view.layout.items.len(), 2 + 1 + 4 + 1 + 1);
        assert!(Rc::ptr_eq(&view.layout.items[7].laid, &reply));
        let first = view.layout.items[2].y - view.chat_scroll.value(now);
        assert!(
            (first - at).abs() < 1.0 || view.chat_scroll.value(now) == 0.0,
            "{first} {at}"
        );
        assert!(view.layout.gutters.iter().all(|g| g.open));
        // Show all work flipped: the fold closes again.
        view.transcript.show_all = Some(false);
        model.persist.update_settings(|s| s.show_all_work = true);
        requests(&mut model, &mut view, HEIGHT, now);
        assert!(!model.chat.expanded.contains("group:u:1000"));
        model.persist.update_settings(|s| s.show_all_work = false);
        view.layout(&model);
        assert_eq!(view.layout.items.len(), 4);
    }

    #[test]
    fn the_live_turn_shows_working_and_its_newest_step() {
        let mut model = open_folded(
            r#"{"type":"user","text":"Run the checks","metadata":{"startedAt":1000,"model":"m","tokens":0}}"#,
        );
        feed(
            &mut model,
            &[
                r#"{"type":"streaming_started","convId":"c","model":"claude-opus-5-5","startedAt":1001}"#,
                r#"{"type":"tool_call","convId":"c","toolCallId":"a","toolName":"Bash","input":{"command":"uptime"},"summary":"uptime"}"#,
                r#"{"type":"tool_result","convId":"c","toolCallId":"a","toolName":"Bash","output":"up","isError":false}"#,
                r#"{"type":"tool_call","convId":"c","toolCallId":"b","toolName":"Bash","input":{"command":"df -h"},"summary":"df -h"}"#,
            ],
        );
        let mut view = ViewState::new();
        view.layout(&model);
        let actions: Vec<Action> = view
            .layout
            .items
            .iter()
            .flat_map(|item| item.laid.hits.iter().map(|(_, a)| a.clone()))
            .collect();
        // WORKING · 1 tool call opens the turn; only `df -h` shows.
        assert!(actions.contains(&Action::Chat(chat::Act::ToggleExpanded(
            "turn:u:1000".into()
        ))));
        let tools: Vec<&PlacedMark> = view
            .layout
            .marks
            .iter()
            .filter(|m| matches!(m.mark.kind, blocks::MarkKind::Spinner { .. }))
            .collect();
        assert_eq!(
            tools.len(),
            2,
            "the working row's spinner and the running call's"
        );
        assert!(!actions.contains(&Action::Chat(chat::Act::ToggleExpanded("tool:a".into()))));
        model.handle(Action::Chat(chat::Act::ToggleExpanded(
            "turn:u:1000".into(),
        )));
        view.layout(&model);
        let actions: Vec<Action> = view
            .layout
            .items
            .iter()
            .flat_map(|item| item.laid.hits.iter().map(|(_, a)| a.clone()))
            .collect();
        assert!(actions.contains(&Action::Chat(chat::Act::ToggleExpanded("tool:a".into()))));
    }

    #[test]
    fn the_system_instructions_box_sits_at_the_top_and_opens() {
        let mut model = open_with(
            r#"{"type":"system_instructions","text":"Be brief.\nAlways test."},{"type":"user","text":"Hi"}"#,
        );
        let mut view = ViewState::new();
        view.layout(&model);
        assert_eq!(model.chat.instructions, ["Be brief.\nAlways test."]);
        // The edge, the box, the prompt.
        assert_eq!(view.layout.items.len(), 3);
        let folded = view.layout.items[1].laid.h;
        let toggle = view.layout.items[1].laid.hits[0].1.clone();
        assert_eq!(
            toggle,
            Action::Chat(chat::Act::ToggleExpanded("instructions".into()))
        );
        model.handle(toggle);
        view.layout(&model);
        let open = &view.layout.items[1].laid;
        assert!(open.h > folded);
        assert!(open.hits.iter().any(|(_, a)| *a
            == Action::Ui(crate::exocortex::model::UiAct::Copy(
                "Be brief.\nAlways test.".into()
            ))));
        // Cleared: gone.
        feed(
            &mut model,
            &[r#"{"type":"system_instructions_updated","convId":"c","text":""}"#],
        );
        view.layout(&model);
        assert_eq!(view.layout.items.len(), 2);
    }

    #[test]
    fn a_subagent_call_opens_its_conversation_and_long_calls_fold() {
        let summary = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\\n");
        let script = (0..6)
            .map(|i| format!("echo {i}"))
            .collect::<Vec<_>>()
            .join("\\n");
        let model_with = |child_known: bool| {
            let mut model = open_with(&format!(
                r#"{{"type":"user","text":"Go"}},{{"type":"ai","blocks":[{{"type":"tool_call","toolCallId":"x","toolName":"exo","input":{{"subagent":"Do the thing"}},"summary":"subagent"}},{{"type":"tool_result","toolCallId":"x","toolName":"exo","output":"{{\"conversation_id\":\"kid\"}}","isError":false}},{{"type":"tool_call","toolCallId":"r","toolName":"Read","input":{{}},"summary":"{summary}"}},{{"type":"tool_result","toolCallId":"r","toolName":"Read","output":"","isError":false}},{{"type":"tool_call","toolCallId":"s","toolName":"bash","input":{{"command":"x"}},"summary":"{script}"}},{{"type":"tool_result","toolCallId":"s","toolName":"bash","output":"","isError":false}}]}}"#
            ));
            if child_known {
                feed(
                    &mut model,
                    &[
                        r#"{"type":"conversation_updated","summary":{"id":"kid","provider":"anthropic","model":"m","effort":"low","title":"Kid","pinned":false,"sortOrder":1,"streaming":false,"unread":false,"parentConversationId":"c"}}"#,
                    ],
                );
            }
            model
        };
        let texts = |laid: &Laid| -> Vec<String> {
            laid.ops
                .iter()
                .filter_map(|op| match op {
                    crate::exocortex::view::ops::Op::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let model = model_with(true);
        let mut view = ViewState::new();
        view.layout(&model);
        let exo = &view.layout.items[2].laid;
        assert!(
            texts(exo)
                .iter()
                .any(|t| t.trim() == "subagent: Do the thing"),
            "{:?}",
            texts(exo)
        );
        assert!(
            exo.hits
                .iter()
                .any(|(_, a)| *a == Action::Nav(Nav::OpenConversation("kid".into())))
        );
        // Twenty lines: eight and the rest counted (PAR-126).
        let read = texts(&view.layout.items[3].laid);
        assert!(read.iter().any(|t| t == "line 7"), "{read:?}");
        assert!(!read.iter().any(|t| t == "line 8"));
        assert!(read.iter().any(|t| t == "… +12 more lines"), "{read:?}");
        // A shell script folds to three rows.
        let bash = texts(&view.layout.items[4].laid);
        assert!(bash.iter().any(|t| t == "… +3 more lines"), "{bash:?}");
        // Not in the list: no way to open it.
        let model = model_with(false);
        let mut view = ViewState::new();
        view.layout(&model);
        assert!(
            !view.layout.items[2]
                .laid
                .hits
                .iter()
                .any(|(_, a)| matches!(a, Action::Nav(_)))
        );
    }

    #[test]
    fn disclosures_and_the_reading_place_are_kept_and_restored() {
        let mut model = open_with(&long_entries(16));
        let mut view = ViewState::new();
        let now = Instant::now();
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT, now);
        assert!(view.follow, "no place kept: the newest message");
        model.handle(Action::Chat(chat::Act::ToggleExpanded("tool:t1".into())));
        let kept = model.kept();
        assert_eq!(kept.disclosures, ["c\u{0}tool:t1"]);
        // Read somewhere, still: kept.
        view.follow = false;
        view.chat_scroll.offset = 900.0;
        chat_frame(&model, &mut view, None, HEIGHT, now);
        chat_frame(&model, &mut view, None, HEIGHT, now + KEEP_AFTER);
        requests(&mut model, &mut view, HEIGHT, now + KEEP_AFTER);
        let anchor = model.saved_anchor("c").expect("kept");
        let key = anchor.key.clone().expect("an entry");
        assert!(key.starts_with("u:") || key.starts_with("a:"), "{key}");
        // Reopened: the same toggles, the same place.
        model.handle(Action::Nav(Nav::Back));
        let mut fresh = ViewState::new();
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        assert!(model.chat.expanded.contains("tool:t1"));
        let before = fresh.before(&model, now);
        fresh.settle(&model, before, HEIGHT, now);
        assert!(!fresh.follow);
        assert!(
            (fresh.chat_scroll.offset - 900.0).abs() <= 1.0,
            "{}",
            fresh.chat_scroll.offset
        );
    }

    #[test]
    fn an_unread_conversation_opens_at_its_latest_response() {
        let mut model = open_with(&format!(
            r#"{},{{"type":"user","text":"And now?"}},{{"type":"ai","blocks":[{{"type":"text","text":"{}"}}]}}"#,
            long_entries(10),
            "A long answer. ".repeat(400)
        ));
        model.handle(Action::Nav(Nav::Back));
        feed(
            &mut model,
            &[
                r#"{"type":"conversation_updated","summary":{"id":"c","provider":"anthropic","model":"claude-opus-5-5","effort":"high","title":"Test","pinned":false,"sortOrder":0,"streaming":false,"unread":true}}"#,
            ],
        );
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        assert!(model.chat.opened_unread);
        let mut view = ViewState::new();
        let now = Instant::now();
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT, now);
        assert!(!view.follow);
        let response = view.entry_top(21).unwrap();
        let max = view.chat_max(&model, HEIGHT);
        assert!(max > response, "taller than the screen");
        assert_eq!(view.chat_scroll.offset, (response - 22.0).clamp(0.0, max));
    }

    #[test]
    fn a_long_press_opens_the_message_menu_and_the_footer_too() {
        let mut model = open_with(&long_entries(2));
        let mut view = ViewState::new();
        let now = Instant::now();
        let bounds = transcript_bounds(&model, HEIGHT, false);
        view.layout(&model);
        let bubble = view.layout.items.iter().find(|i| i.stamp != 0).unwrap();
        let (stamp, y) = (
            bubble.stamp,
            bounds.0 + bubble.y + 10.0 - view.chat_scroll.value(now),
        );
        assert_eq!(
            long_press(&model, &mut view, bounds, y, now),
            Some(Action::Chat(chat::Act::MessageMenu(stamp)))
        );
        model.handle(Action::Chat(chat::Act::MessageMenu(stamp)));
        assert_eq!(model.ui.opening.len(), 1);
        // The footer's row is a tap target for the same menu.
        let footer = view
            .layout
            .items
            .iter()
            .find(|i| i.index == u32::MAX)
            .unwrap();
        assert!(matches!(
            footer.laid.hits[0].1,
            Action::Chat(chat::Act::MessageMenu(_))
        ));
        let _ = Hit::Transcript;
    }

    #[test]
    fn states_show_loading_empty_and_errors() {
        let mut model = open_with("");
        assert!(stateful(&model));
        let bounds = transcript_bounds(&model, HEIGHT, false);
        assert_eq!(state_hit(&model, bounds, 540.0, 1000.0), None);
        model.chat.error = Some("The conversation did not respond.".into());
        let (rect, _) = state_button(&model, bounds).unwrap();
        assert_eq!(
            state_hit(&model, bounds, rect.x + 5.0, rect.y + 5.0),
            Some(Action::Chat(chat::Act::Retry))
        );
        let mut canvas = Canvas::new(1080, HEIGHT as i32);
        let mut view = ViewState::new();
        draw(&mut canvas, &mut model, &mut view, None, Instant::now());
        assert!(canvas.pixels.contains(&theme::p().error));
    }

    #[test]
    fn legacy_prompts_and_previews() {
        use fold::{legacy_automated, notification_preview};
        assert!(legacy_automated("[notification] x"));
        assert!(legacy_automated("[chrono wake: deploy] check"));
        assert!(legacy_automated("[Discord Notification] hi"));
        assert!(legacy_automated("[external notification] x"));
        assert!(!legacy_automated("[external notifications] x"));
        assert!(!legacy_automated("Hello [notification]"));
        assert_eq!(
            notification_preview(
                "[notification] Discord/main · dm\n→ Ana [ch:1] <@42> said hi\n\nAction: reply"
            ),
            "Discord/main · dm → Ana said hi"
        );
    }

    /// Renders the transcript kinds into PNGs for side-by-side checks with
    /// the iPhone's reference shots: `WP_C2_SHOTS=dir cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn render_reference_shots() {
        let Some(dir) = std::env::var_os("WP_C2_SHOTS") else {
            return;
        };
        let save = |canvas: &Canvas, name: &str| {
            let path = std::path::Path::new(&dir).join(name);
            let file = std::fs::File::create(&path).unwrap();
            let mut encoder = png::Encoder::new(file, canvas.width as u32, canvas.height as u32);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let bytes: Vec<u8> = canvas
                .pixels
                .iter()
                .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
                .collect();
            writer.write_image_data(&bytes).unwrap();
        };
        let meta = |start: u64, end: u64| {
            format!(
                r#""metadata":{{"startedAt":{start},"endedAt":{end},"model":"claude-opus-5-5","tokens":900,"generationThroughput":{{"provider":"anthropic","model":"claude-opus-5-5","rates":[76.2]}}}}"#
            )
        };
        let entries = [
            r#"{"type":"user","text":"Can you summarise this report and check the build?"}"#.to_owned(),
            format!(
                r#"{{"type":"ai","blocks":[{{"type":"thinking","text":"The user wants a summary and a build check.\n\nI'll run the tests first."}},{{"type":"tool_call","toolCallId":"t1","toolName":"Bash","input":{{"command":"cd web && timeout 120 bun test 2>&1 | tail -3"}},"summary":"cd web && timeout 120 bun test 2>&1 | tail -3"}},{{"type":"tool_result","toolCallId":"t1","toolName":"Bash","output":"","isError":false}},{{"type":"tool_call","toolCallId":"t2","toolName":"mcp__gmail__search","input":{{}},"summary":"search newer_than:1d --timeout_seconds 60"}},{{"type":"tool_result","toolCallId":"t2","toolName":"","output":"","isError":true}},{{"type":"text","text":"The build is **green**. Summary:\n\n- revenue up\n- costs flat\n\nDone."}}],{}}}"#,
                meta(1_000, 60_000)
            ),
            r#"{"type":"system","text":"Claude account switched from \"main\" to \"max2\": the main account hit its usage limit. The conversation continues on max2 until the limit resets.","color":"warning","metadata":{"startedAt":61000,"model":"","tokens":0,"kind":"provider_notice"}}"#.to_owned(),
            r#"{"type":"system","text":"--- Compaction finished ---","metadata":{"startedAt":62000,"model":"","tokens":0,"kind":"context_compaction_finished","system":true}}"#.to_owned(),
            r#"{"type":"user","text":"[chrono wake: deploy] Chrono wake: check the deploy.","metadata":{"startedAt":63000,"model":"","tokens":0,"automation":{"kind":"chrono_wake"}}}"#.to_owned(),
            format!(
                r#"{{"type":"ai","blocks":[{{"type":"text","text":"[Context: deploy is green]"}},{{"type":"text","text":"Deploy is **green**. Nothing to do."}}],{}}}"#,
                meta(63_000, 64_000)
            ),
            r#"{"type":"system","text":"✗ Request aborted by the user.","color":"error"}"#.to_owned(),
        ]
        .join(",");
        let mut model = open_with(&entries);
        assert!(
            model.chat.loaded && !model.chat.entries.is_empty(),
            "{:?}",
            (
                serde_json::from_str::<serde_json::Value>(&format!("[{entries}]")).err(),
                &entries[990..1040]
            )
        );
        model.handle(Action::Chat(chat::Act::ToggleExpanded(
            "think:start:0".into(),
        )));
        let mut view = ViewState::new();
        let now = Instant::now();
        let mut canvas = Canvas::new(1080, HEIGHT as i32);
        view.follow = false;
        view.chat_scroll.offset = 0.0;
        draw(&mut canvas, &mut model, &mut view, None, now);
        save(&canvas, "c2-rich-top.png");
        view.follow = true;
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT, now);
        draw(&mut canvas, &mut model, &mut view, None, now);
        save(&canvas, "c2-rich-bottom.png");
        // Streaming: a running tool, a retry, the ticking footer, a queued
        // message.
        feed(
            &mut model,
            &[
                r#"{"type":"user_message","convId":"c","text":"Now run the full test suite and fix anything that fails.","startedAt":70000}"#,
                r#"{"type":"streaming_started","convId":"c","provider":"anthropic","model":"claude-opus-5-5","startedAt":70000,"snapshotKind":"start","workTimerStartedAt":70000}"#,
                r#"{"type":"block_start","convId":"c","blockType":"thinking"}"#,
                r#"{"type":"thinking_chunk","convId":"c","text":"Let me run them."}"#,
                r#"{"type":"tool_call","convId":"c","toolCallId":"t9","toolName":"Bash","input":{"command":"cd web && bun test 2>&1 | tail -20"},"summary":"cd web && bun test 2>&1 | tail -20"}"#,
                r#"{"type":"tool_result","convId":"c","toolCallId":"t9","toolName":"Bash","output":"2 fail","isError":false}"#,
                r#"{"type":"text_chunk","convId":"c","text":"Two tests fail in `sidebar.test.ts`. The first one is a **stale sort order** after a pin; the fix is to"}"#,
                r#"{"type":"queue_updated","messages":[{"id":"q1","convId":"c","text":"also bump the version when you're done","timing":"next-turn","source":"daemon","createdAt":1}]}"#,
            ],
        );
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT, now);
        draw(&mut canvas, &mut model, &mut view, None, now);
        save(&canvas, "c2-streaming.png");
        feed(
            &mut model,
            &[
                r#"{"type":"tool_call","convId":"c","toolCallId":"t10","toolName":"Bash","input":{"command":"sleep 30"},"summary":"sleep 30"}"#,
                r#"{"type":"stream_retry","convId":"c","attempt":2,"maxAttempts":5,"errorMessage":"Overloaded","delaySec":12}"#,
                r#"{"type":"context_compaction_status","convId":"c","active":true,"startedAt":1}"#,
            ],
        );
        let before = view.before(&model, now);
        view.settle(&model, before, HEIGHT, now);
        draw(&mut canvas, &mut model, &mut view, None, now);
        save(&canvas, "c2-status.png");
    }
}
