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

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::gfx::{Canvas, RectF};

use super::super::anim::{Animator, Tween};
use super::super::footer;
use super::super::markdown::{self, MdBlock};
use super::super::model::chat::{
    COMPACTION_KIND, Disclosure, merged_thinking, thinking_key, visible,
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
    lay_divider, lay_edge, lay_footer, lay_live_footer, lay_load_error, lay_md_piece, lay_notice,
    lay_notification, lay_status, lay_thinking, lay_working, mark_frames, mark_key, notice_color,
    wall_ms,
};
use super::markdown_layout::md_gap;
use super::media::{Strip, lay_strip};
use super::ops::{Laid, draw_ops, px, quantized};
use super::tools::{GROUP_MIN, Output, expandable, lay_group, lay_tool, output_state, tool_run};
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
    pub(crate) height: f32,
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
    /// transcript to stop moving).
    last_offset: Option<f32>,
    /// The transcript's rows at the last frame (for `animate`).
    bounds: (f32, f32),
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
    /// "Chrono", "Subagent completion", …
    label: &'static str,
    kind: String,
    /// The conversation it came from.
    source: Option<String>,
    /// An external notification's channel (its header's first words).
    channel: Option<String>,
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
    /// A user message: the user's own bubble, or an automated prompt.
    User { automated: Option<Automated> },
    /// A system notice (its key opens it).
    Notice { key: String, muted: bool },
    /// The compaction divider.
    Divider,
    /// A block of a reply's markdown.
    Markdown { block: MdBlock, hash: u64, gap: f32 },
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
    /// A run of tool calls: a row each, or one folded row (`group`: its
    /// toggle key and what it shows) that opens to them.
    Tools {
        calls: Vec<ToolRef>,
        group: Option<(String, u64)>,
        gap: f32,
    },
    /// A message's images (C1's `media.rs`): a reply's after it, the
    /// user's under their bubble.
    Images { strip: Strip, gap: f32 },
}

impl Plan {
    /// How far an entry's first row reaches up into the gap above it (a
    /// notification row's 44 pt target).
    fn lead(&self) -> f32 {
        match self.segments.first() {
            Some(Segment::User {
                automated: Some(_), ..
            }) => NOTIFICATION_TRIM,
            _ => 0.0,
        }
    }
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

const AUTOMATION_LABELS: [(&str, &str); 8] = [
    ("external_notification", "Notification"),
    ("background_task_completion", "Background completion"),
    ("chrono_wake", "Chrono"),
    ("chrono_hard_wake", "Chrono hard wake"),
    ("exo_send", "Exo delegation"),
    ("subagent_completion", "Subagent completion"),
    ("goal_continuation", "Goal continuation"),
    ("daemon_restart", "Daemon restart"),
];

/// Notices that keep the model up to date and are never shown (PAR-107).
const MODEL_ONLY: [&str; 3] = ["client_host", "remote_host", "conversation_goal"];

/// A prompt an older daemon wrote without `automation` (`[notification]
/// …`, `[chrono wake: …]`, `[Discord Notification] …`).
fn legacy_automated(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('[') else {
        return false;
    };
    if rest.starts_with("notification]")
        || rest.starts_with("chrono wake:")
        || rest.starts_with("chrono wake]")
        || rest.starts_with("chrono hard wake:")
        || rest.starts_with("chrono hard wake]")
    {
        return true;
    }
    if let Some(after) = rest.strip_prefix("external notification") {
        return !after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
    }
    rest.find("Notification]").is_some_and(|end| {
        rest[..end]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '/' || c == ' ')
    })
}

/// The first meaningful words of a notification, on one line.
fn notification_preview(text: &str) -> String {
    let mut text = text.to_owned();
    // The agent-facing tail.
    for tail in ["\n\nAction:", "\n\nFilter output:"] {
        if let Some(at) = text.find(tail) {
            text.truncate(at);
        }
    }
    let mut text = text.trim_start();
    // The envelope.
    if let Some(rest) = text.strip_prefix("[notification]") {
        text = rest;
    } else if let Some(rest) = text
        .strip_prefix("[external notification")
        .or_else(|| text.strip_prefix("[chrono wake:"))
        .or_else(|| text.strip_prefix("[chrono hard wake:"))
        && let Some((_, after)) = rest.split_once(']')
    {
        text = after;
    }
    let mut out = String::new();
    for word in text.split_whitespace() {
        // Channel and mention noise.
        if (word.starts_with("[ch:")
            || word.starts_with("[dm:")
            || word.starts_with("[msg:")
            || word.starts_with("[thread:")
            || word.starts_with("[reply-to:")
            || word.starts_with("<@"))
            && (word.ends_with(']') || word.ends_with('>'))
            || word == "↳"
        {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() > 200 {
            break;
        }
    }
    out
}

fn plan_user(text: &str, metadata: Option<&Metadata>) -> Vec<Segment> {
    if metadata.is_some_and(|m| m.system) {
        return vec![Segment::Notice {
            key: format!("n:{}", hash_of(text)),
            muted: false,
        }];
    }
    let automation = metadata.and_then(|m| m.automation.as_ref());
    if automation.is_none() && !legacy_automated(text) {
        return vec![Segment::User { automated: None }];
    }
    let kind = automation
        .and_then(|a| a.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let label = AUTOMATION_LABELS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("Notification", |(_, label)| label);
    let source = automation
        .and_then(|a| a.get("sourceId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let channel = (kind == "external_notification")
        .then(|| {
            text.strip_prefix("[notification]")
                .and_then(|rest| rest.lines().next())
                .map(|line| notification_preview(line.trim()))
                .filter(|line| !line.is_empty())
        })
        .flatten();
    vec![Segment::User {
        automated: Some(Automated {
            label,
            kind,
            source,
            channel,
            preview: notification_preview(text),
        }),
    }]
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
            if matches!(segments.first(), Some(Segment::User { automated: None }))
                && let Some(strip) = Strip::of_user(images)
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
            Block::ToolCall { tool_call_id, .. } => {
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
                let group = (calls.len() >= GROUP_MIN).then(|| {
                    let key = format!("group:{tool_call_id}");
                    let shown: Vec<u64> = calls.iter().map(|tool| tool.hash).collect();
                    let hash = hash_of(("group", &key, shown));
                    (key, hash)
                });
                segments.push(Segment::Tools { calls, group, gap });
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

/// Where an entry's pieces went: its height (0: nothing shown) and the gap
/// it leaves to the next row.
struct EntryLaid {
    h: f32,
    tail_gap: f32,
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

    /// Places a planned entry's pieces from `y`.
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
    ) -> EntryLaid {
        let mut h = 0.0;
        let mut index = 0;
        let mut tail_gap = ROW_GAP;
        let mut put =
            |layout: &mut ChatLayout, h: &mut f32, (laid, marks): (Rc<Laid>, Rc<[Mark]>)| {
                if laid.h > 0.0 {
                    let top = y + *h;
                    for mark in marks.iter() {
                        layout.marks.push(PlacedMark {
                            top,
                            mark: mark.clone(),
                        });
                    }
                    layout.items.push(Placed {
                        y: top,
                        laid: laid.clone(),
                        stamp,
                        index,
                        role: Role::Message,
                    });
                    index += 1;
                    *h += laid.h;
                }
            };
        let chat = context.chat;
        let open_at = |key: &str| chat.is_open(key, context.disclosure);
        let model = context.model;
        for segment in &plan.segments {
            match segment {
                Segment::User { automated } => {
                    let Some(Entry::User { text, .. }) = entry else {
                        continue;
                    };
                    let bubble_label = |a: &Automated| {
                        let source = match a.kind.as_str() {
                            "exo_send" => Some(a.source.as_deref().map_or_else(
                                || "another conversation".to_owned(),
                                |id| title_of(model, id),
                            )),
                            "subagent_completion" => {
                                a.source.as_deref().map(|id| title_of(model, id))
                            }
                            "external_notification" => a.channel.clone(),
                            _ => None,
                        };
                        match source {
                            Some(source) => format!("{} · from {source}", a.label),
                            None => a.label.to_owned(),
                        }
                    };
                    match automated {
                        // Images alone: their strip, no empty bubble.
                        None if text.trim().is_empty()
                            && matches!(entry, Some(Entry::User { images, .. }) if !images.is_empty()) =>
                            {}
                        None => {
                            let piece = self.piece(hash_of(("bubble", text, Tone::Own)), || {
                                lay_bubble(text, Tone::Own, None, false)
                            });
                            put(layout, &mut h, piece);
                        }
                        // A parent's delegation is this conversation's prompt.
                        Some(a)
                            if a.kind == "exo_send"
                                && !a
                                    .source
                                    .as_deref()
                                    .is_some_and(|id| own_subagent(model, id)) =>
                        {
                            let label = bubble_label(a);
                            let piece = self.piece(hash_of(("auto", text, &label)), || {
                                lay_bubble(text, Tone::Automated, Some(&label), true)
                            });
                            put(layout, &mut h, piece);
                        }
                        Some(a) => {
                            let key = format!("u:{}", hash_of(text));
                            let open = open_at(&key);
                            let piece = self
                                .piece(hash_of(("note", &key, a.label, &a.preview, open)), || {
                                    lay_notification(a.label, &a.preview, &key, open)
                                });
                            put(layout, &mut h, piece);
                            tail_gap = ROW_GAP - NOTIFICATION_TRIM;
                            if open {
                                let label = bubble_label(a);
                                h -= NOTIFICATION_TRIM - 17.0;
                                let piece = self.piece(hash_of(("auto", text, &label)), || {
                                    lay_bubble(text, Tone::Automated, Some(&label), true)
                                });
                                put(layout, &mut h, piece);
                                tail_gap = ROW_GAP;
                            }
                        }
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
                    put(layout, &mut h, piece);
                }
                Segment::Divider => {
                    let piece =
                        self.piece(hash_of("divider"), || lay_divider("Compaction finished"));
                    put(layout, &mut h, piece);
                }
                Segment::Markdown { block, hash, gap } => {
                    h += gap;
                    let piece = self.piece(*hash, || lay_md_piece(block));
                    put(layout, &mut h, piece);
                }
                Segment::Context { index, gap } => {
                    let Some(Block::Text { text }) = blocks.get(*index) else {
                        continue;
                    };
                    h += gap;
                    let piece = self.piece(hash_of(("context", text)), || lay_context(text));
                    put(layout, &mut h, piece);
                }
                Segment::Thinking {
                    index,
                    end,
                    key,
                    hash,
                    gap,
                } => {
                    h += gap;
                    let open = open_at(key);
                    let growing = context.live && *end == blocks.len();
                    let (_, text) = merged_thinking(blocks, *index);
                    let piece = self.piece(hash_of((hash, open, growing, stamp)), || {
                        lay_thinking(&text, key, stamp, open, growing)
                    });
                    put(layout, &mut h, piece);
                }
                Segment::Tools { calls, group, gap } => {
                    h += gap;
                    let indent = match group {
                        Some((key, hash)) => {
                            let open = open_at(key);
                            let piece = self
                                .piece(hash_of((hash, open, context.styles_version)), || {
                                    lay_group(blocks, calls, key, open, context)
                                });
                            put(layout, &mut h, piece);
                            if !open {
                                continue;
                            }
                            0.0
                        }
                        None => 0.0,
                    };
                    for (at, tool) in calls.iter().enumerate() {
                        if at > 0 || group.is_some() {
                            h += 17.0;
                        }
                        let output = output_state(chat, tool, blocks, context.live);
                        let open = open_at(&tool.key) && expandable(&output);
                        if matches!(output, Output::Unknown) {
                            let id = tool.key.trim_start_matches("tool:").to_owned();
                            layout.tools.push((y + h, id));
                        }
                        let key = hash_of((
                            tool.hash,
                            open,
                            indent as i32,
                            context.styles_version,
                            open.then_some(&output),
                            (!open).then(|| expandable(&output)),
                            matches!(output, Output::Running) && context.live,
                        ));
                        let piece = self.piece(key, || {
                            lay_tool(blocks, tool, indent, open, &output, context)
                        });
                        put(layout, &mut h, piece);
                    }
                }
                Segment::Images { strip, gap } => {
                    h += gap;
                    let conv = context.chat.id.as_deref();
                    let images = &context.rich.images;
                    let piece = self.piece(strip.state_key(images, conv), || {
                        lay_strip(strip, images, conv).into()
                    });
                    put(layout, &mut h, piece);
                }
            }
        }
        EntryLaid { h, tail_gap }
    }

    /// Lays the open chat out if it changed since the last frame: every
    /// entry's pieces, from the cache unless what they show is new.
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
        }
        let live_follows = chat.live.is_some() && chat.echoes.is_empty();
        let (footers, live_base) = footer::footers(&chat.entries, live_follows);
        let now = wall_ms(Instant::now());
        for (at, (entry, stamp)) in chat.entries.iter().zip(&chat.stamps).enumerate() {
            let plan = self.plan(*stamp, entry);
            if plan.segments.is_empty() {
                continue;
            }
            let blocks = match entry {
                Entry::Ai { blocks, .. } => blocks.as_slice(),
                _ => &[],
            };
            let top = y + gap - plan.lead();
            let placed = self.place(
                &mut layout,
                top,
                &plan,
                Some(entry),
                *stamp,
                blocks,
                &context,
            );
            if placed.h <= 0.0 && footers[at].is_none() {
                continue;
            }
            y = top + placed.h;
            gap = placed.tail_gap;
            if let Some(meta) = &footers[at] {
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
            y += gap;
            let plan = plan_blocks(&live.blocks);
            let placed = self.place(
                &mut layout,
                y,
                &plan,
                None,
                live.stamp,
                &live.blocks,
                &live_context,
            );
            let mut h = placed.h;
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
            let status = &chat.status;
            if !live.blocks.iter().any(visible) && !status.any() {
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
            && self.chat_scroll.value(now) >= max - FOLLOW_SLACK
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

/// What the chat asks the daemon for as it is scrolled: older history near
/// the top, the outputs of tool rows within a screen of what shows.
pub fn requests(model: &mut Model, view: &mut ViewState, height: f32, now: Instant) -> Vec<Value> {
    if model.page != Page::Chat {
        return Vec::new();
    }
    let mut commands = Vec::new();
    if view.near_top(now) {
        commands.extend(model.load_older());
    }
    view.layout(model);
    if view.layout.tools.is_empty() || !model.chat.outputs_inflight.is_empty() {
        return commands;
    }
    let (top, bottom) = transcript_bounds(model, height, view.keyboard);
    let screen = bottom - top;
    let offset = view.chat_scroll.value(now);
    let tools = &view.layout.tools;
    let from = tools.partition_point(|(y, _)| *y < offset - screen);
    let near: Vec<&str> = tools[from..]
        .iter()
        .take_while(|(y, _)| *y < offset + 2.0 * screen)
        .map(|(_, id)| id.as_str())
        .collect();
    if !near.is_empty() {
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
    /// The jump button's opacity in 32nds (0: not drawn).
    jump: u32,
    jump_rows: (i32, i32),
}

/// The jump button's square (§3.6.14): 34 px from the right, 34 above the
/// island.
fn jump_rect(bottom: f32) -> RectF {
    RectF::new(W - JUMP_INSET - JUMP, bottom - 17.0 - JUMP, JUMP, JUMP)
}

/// Its shadow's reach (blur 44, 11 down).
const JUMP_SHADOW: (f32, f32) = (44.0, 11.0);

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
    let wanted = max - offset > JUMP_THRESHOLD && !loading && !moving;
    let shown = view.transcript.jump.as_ref().is_some_and(|(on, _)| *on);
    if wanted && !shown {
        let tween = Tween::new(now, JUMP_FADE, 0.0, 1.0);
        view.transcript.jump = Some((true, tween));
    } else if !wanted && shown {
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
    TranscriptFrame {
        marks,
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
                return Some(action.clone());
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
        for y in 0..HEIGHT as i32 {
            if damage.rows.iter().any(|(a, b)| (*a..*b).contains(&y)) {
                continue;
            }
            let row = y as usize * width..(y as usize + 1) * width;
            assert!(
                shadow[row.clone()] == canvas.pixels[row],
                "row {y} changed outside the damage"
            );
        }
        damage
    }

    /// Rows damaged in the transcript (the header pulses on its own).
    fn rows(damage: &crate::exocortex::view::FrameDamage) -> i32 {
        let top = CHAT_TOP as i32;
        damage
            .rows
            .iter()
            .map(|(a, b)| (b - a.max(&top)).max(0))
            .sum()
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
        let shown = t0 + JUMP_FADE;
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
        let t = t0 + JUMP_FADE;
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
        // Still again: it fades back in, in its own columns.
        let damage = update(&mut model, &mut view, (t, t + JUMP_FADE), |_, _| {});
        assert!(damage.moved.is_none());
        assert!(rows(&damage) <= 122 + 2 * 44 + 11 + 4, "{damage:?}");
        assert_eq!(jump_alpha(&view, t + JUMP_FADE), 32);
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
        let mut model = open_with(
            r#"{"type":"user","text":"[notification] Discord/main · dm\nHello there from Discord","metadata":{"startedAt":1,"model":"","tokens":0,"automation":{"kind":"external_notification"}}},{"type":"system","text":"Claude account switched from main to max2","color":"warning","metadata":{"startedAt":2,"model":"","tokens":0,"kind":"provider_notice"}},{"type":"system","text":"client host info","metadata":{"startedAt":3,"model":"","tokens":0,"kind":"client_host"}},{"type":"system","text":"--- Compaction finished ---","metadata":{"startedAt":4,"model":"","tokens":0,"kind":"context_compaction_finished","system":true}}"#,
        );
        let mut view = ViewState::new();
        view.layout(&model);
        // The edge, the notification row, the notice, the divider.
        assert_eq!(view.layout.items.len(), 4);
        let toggle = view.layout.items[1].laid.hits[0].1.clone();
        let Action::Chat(chat::Act::ToggleExpanded(key)) = &toggle else {
            panic!("{toggle:?}");
        };
        assert!(key.starts_with("u:"));
        let notice = view.layout.items[2].laid.h;
        assert!(notice >= super::super::blocks::HEAD);
        model.handle(toggle);
        view.layout(&model);
        // Opened: the automated bubble under its row.
        assert_eq!(view.layout.items.len(), 5);
        let text: Vec<String> = view.layout.items[2]
            .laid
            .ops
            .iter()
            .filter_map(|op| match op {
                crate::exocortex::view::ops::Op::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            text.iter()
                .any(|t| t.contains("Notification · from Discord/main · dm")),
            "{text:?}"
        );
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
            format!(r#""metadata":{{"startedAt":{start},"endedAt":{end},"model":"claude-opus-5-5","tokens":900,"generationThroughput":{{"provider":"anthropic","model":"claude-opus-5-5","rates":[76.2]}}}}"#)
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
            (serde_json::from_str::<serde_json::Value>(&format!("[{entries}]")).err(), &entries[990..1040])
        );
        model.handle(Action::Chat(chat::Act::ToggleExpanded("think:start:0".into())));
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
