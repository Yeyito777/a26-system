# Exocortex app roadmap: iPhone parity, in Whale

This is the plan for bringing `a26-system --app exocortex` (Moon's native
client of the Exocortex daemon, `src/exocortex/`) up to the user's iPhone
Exocortex app: its features, its look (the **Whale** theme in the iPhone's
"operator console" style) and its feel, on a CPU-drawn X11 canvas at 120 Hz.
Several engineers build it in parallel from §5; §3 is the visual spec they
all follow.

Sources, so nothing here needs re-deriving:

- The iPhone app: the `exocortex-phone` repository (a Preact web app in a Swift
  shell). `docs/PARITY.md` is the feature checklist; **PAR-nnn** below are its
  row ids, and its tuned constants apply unless this document says otherwise.
  Behaviour and styling come from `web/src/ui/**`, `web/src/ui/root.css`,
  `web/src/styles/{base,markdown}.css` and `web/src/theme/themes.ts`.
- Exocortex: `shared/src/protocol.ts`, `docs/daemon-ipc.md`, and the TUI
  (`tui/src/`), which owns the semantics the iPhone ports (activity colours,
  turn folding, macros, tool styles, model display names).
- This app: `docs/exocortex.md` (daemon traffic, incremental drawing, the
  notification bridge), `src/gfx.rs`, `src/text.rs`, `src/present.rs`.
- Moon: OSS-Phone-V1 `docs/page-transitions.md`, `docs/voice.md`,
  `docs/settings.md` (haptics), `docs/notifications-ui.md`.
- Reference screenshots of the iPhone UI (390×844 pt at 3×, rendered
  headless from the web app: a synthetic transcript with every block kind,
  plus read-only shots of the real sidebar, model sheet and settings) and one
  of this app today are on the phone in `/opt/build/exo-reference/`
  (`fake-*.png`, `real-*.png`, `native-current*.png`). They show private
  conversations, so they stay out of git; look at them, do not commit them.

**Scale.** The iPhone is 390 pt wide, the A26 window 1080 px: **1 pt =
2.77 px** (`src/ui.rs` uses the same). Every pt value from the iPhone CSS is
converted with that factor and rounded; §3 gives the results, so engineers
never convert themselves. Moon's status bar (124 px) sits above the app's
window and its home gesture owns the bottom 170 px (`GESTURE_RESERVE`), so the
iPhone's safe-area insets map to nothing at the top and to that reserve at
the bottom.

## 1. Where the app is today

What works (keep it working): the conversation list with folders (drill-down
pages), pinned first in the TUI's order, unread dot, "Working…"; a chat with
history paging (older pages joined mid-entry), streaming (block_start,
chunks, sync, tool calls/results, completion, stop, injected user messages),
markdown (headings, lists, quotes, rules, fenced code, tables as monospaced
lines, bold/italic/code/link spans), tool rows with TUI labels and colours
(runs of ≥3 folded), thinking folded, lazy tool outputs, Send / queue while
streaming (message-end) / Stop with `expectedStartedAt`, a model+effort sheet,
new conversations in the shown folder, in-memory drafts, reconnect with
backoff, Moon page transitions, Moon's keyboard, fling scrolling, and
partial redraw (scrolls and streams move rows and draw only damage; partial
frames are pixel-identical to full ones in tests). `exocortex-notify` already
covers completion notifications, suppression while shown, tap-to-open and
withdraw-on-read, and holds Moon's lease while anything streams.

How it looks: iOS Messages/Settings dark (Inter, blue `#0a84ff` bubbles,
rounded cells, black background). The user's chosen look is Whale's operator
console: monospaced type throughout, square boxes with hairline borders and
2 pt colour rails, deep-blue surfaces on `#00050f`.

The performance agent's branch `exo-perf` (loading and speed) is separate;
§6 lists what this roadmap expects from it.

## 2. The gap

Columns: what the iPhone does (PAR ids) · the phone app today · the plan and
the package that owns it (§5). Priorities are this roadmap's: **P0** first
parity release, **P1** soon after, **P2** nice to have, **n/a** does not
apply here (§7). "Here" notes say how a feature maps onto this phone: a
local daemon socket (no SSH, no hot updates), Moon's keyboard, Moon's
notifications, power-key voice.

### 2.1 Navigation, pages, chrome

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Three pages Settings \| Sidebar \| Chat; launches on the list; page indicator rail under every top bar (PAR-001, 007) | List ⇄ chat only; iOS nav bar ("< Chats") | Pages **Settings ← List → Chat** with Moon's GPU push/pop transitions (opening Settings uses Moon's *pop* look, closing it *push*, so space reads as on the iPhone); a 3-segment page rail under every top bar | A | P0 |
| Shared top-bar rail: 44 pt, hairline rule, monospaced, 44 pt targets (PAR-007) | 150 px iOS bar, Inter | Whale top bar component (§3.6.1) used by every page | A | P0 |
| Chat header: back, mixed-case title + quiet subtitle, ⋯ menu; title tap opens the inspector (PAR-008) | Title + blue "model · effort ⌄" picker | Header per §3.6.1; title tap → model inspector (E); ⋯ → chat menu (E) | A shell, E content | P0 |
| Subtitle priority: Connecting… / Couldn't load / Syncing… / Working · model / model label, with status dot; fixed height (PAR-009, 177) | none | `chat_header::subtitle(model)`; status never moves the transcript | E | P0 |
| Header ⋯ menu: Reconnect/Retry, Configuration and activity, Find, Export, Discard draft (PAR-010) | none | Action-list sheet (screenshot `fake-09-chat-menu`) | E | P1 |
| Edge swipe between pages (PAR-001/003) | none | Left-edge swipe (start x < 60 px, travel > 30 % width or flick > 1500 px/s, mostly horizontal) = Back, played as Moon's pop transition at release. Finger-driven transitions need a Moon change (§6.3) | A | P1 |
| Empty chat page / empty, loading, error states (PAR-014, 063, 145, 092) | "Connecting…" line on the list | Centred state blocks (§3.6.20) | B list, C2 chat | P0 |
| Conversation-scoped sheets close when the conversation changes (PAR-015) | n/a | Overlay stack closes `scoped` overlays on chat change | A | P0 |
| Leaving the chat dismisses keyboard and menus (PAR-004) | keyboard only | Same rule for every overlay | A | P0 |
| Deep links (PAR-011) | Moon open targets (notifications) | Keep; unknown id → toast "This conversation is not available on the connected daemon." | E | P2 |

### 2.2 Connection and lifecycle

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Ready = pong received; send/queue/commands gated, Stop not (PAR-020, 197) | `connected` flag on socket open | `Ready` after `pong`; composer gating per PAR-184 | A (state), D (gating) | P0 |
| Request deadlines and actionable timeouts; error routing by reqId (PAR-025–027) | errors routed for load/older only | `requests.rs`: reqId registry with per-kind deadlines (20 s default, abort 12 s, transcription 90 s, folder create 12 s), stale-reply guard, routing to the owning sub-model, generic "The daemon rejected the command." toast | A | P0 |
| `daemon_shutdown` → "Daemon restarting…" (PAR-029); `connectivity_status` offline → "⚠ offline" (PAR-030) | "Reconnecting…" | Subtitle and list footer states | E | P1 |
| Connection diagnostics (PAR-035) | none | Settings → Connection: socket path, state, last pong, attempts, last error | E | P1 |
| SSH, device identity, host key, channel framing (PAR-016, 021, 022, 032, 033, 037) | local Unix socket | **n/a**: one local socket; keep never advertising `clientHost` | — | n/a |
| Cold start from a snapshot cache, warm tier (PAR-039–050) | full bootstrap each start | Owned by `exo-perf` if it covers it; otherwise a follow-up of A (store.rs is ready for it) | perf | P1 |

### 2.3 Conversation list and folders

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Tree folders expanding in place, 14 pt indent per level, rotating chevron, expanded set persisted (PAR-053) | drill-down pages | Tree; expanded ids in `ui.json`; virtualised fixed-height rows | B | P0 |
| PINNED header + divider (PAR-052) | pin glyph per row | Header row + divider row (`real-01-sidebar`) | B | P0 |
| Folder row: icon box, name, "N CHATS", aggregate indicator, red unread badge, trailing "+" new chat in folder (PAR-054, 058) | glyph, name, count, chevron | §3.6.3 | B | P0 |
| Conversation row: ★ marked, mark emoji, title, "5 min. ago · N messages", 🔕 explicit mute, selected rail + bold (PAR-055, 087) | title, time, model or "Working…" | §3.6.2 | B | P0 |
| Activity indicator accent / yellow / green with TUI semantics; badges ◆N $N ◷N (PAR-057, 059, 060) | blue unread dot | Port `activityindicator.ts` + `render.ts` rules into `model/sidebar.rs` | B | P0 |
| One 60 s ticker for relative times (PAR-056) | redraw on change | One ticker; redraw only visible meta strips | B | P0 |
| Search field with debounce, flat "SEARCH RESULTS // N" (PAR-061) | none | Header search field (Moon keyboard purpose `search`), 160 ms debounce, substring over titles (marks stripped) and folder names, hits highlighted with `searchBackground` | B | P0 |
| Header controls ⋯ (Select, Next unread, Undo/Redo deletion, Settings), New folder, New chat (PAR-064, 084) | compose glyph | §3.6.4 | B | P0 |
| Long-press: lift at 150 ms, recognised at 600 ms with a medium haptic, 14 pt slop; anchored themed menu with dim cut-out (PAR-066, 067, 069) | none | Gesture layer (A) + anchored menu (A) + items (B) | A+B | P0 |
| Menu items Pin, Mark, Delete; Mute, Rename…, Move to folder…, Clone, Copy id (PAR-067, 068) | none | `pin_conversation`, `mark_conversation`, `mute_conversation`, `rename_conversation`, `move_sidebar_items`, `clone_conversation` | B | P0 (first three), P1 |
| Delete confirmation + undo snackbar (PAR-072, 079) | none | Confirm dialog; snackbar "Moved to trash · Undo" (`undo_delete`) | B | P0 |
| New folder prompt (PAR-065) | none | Text prompt overlay; `create_folder` acked within 12 s | B | P0 |
| Optimistic moves, pin placement, send bumps, optimistic new row (PAR-073, 075, 076, 082) | none | Port `sidebar-pending.ts` projection | B | P1 |
| Drag to reorder / into folder with edge auto-scroll (PAR-070, 071) | none | After the menu works; drop line + folder highlight per §3.6.3 | B | P2 |
| Pull to refresh (PAR-062) | none | Overscroll > 160 px at the top → `list_conversations` | B | P1 |
| Organize multi-select, folder inspector with instructions (PAR-077, 078) | none | Select mode + bottom select bar; folder sheet with `load_folder_instructions` / `set_folder_instructions` | B | P1 |
| Emoji marks (PAR-083), subagent badge opens child (PAR-090), update status footer (PAR-086) | none | | B | P2 |

### 2.4 Chat transcript and rendering

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| User bubble: right-aligned, hugs text, square, `userBg`, accent 2 pt rail and 22 % accent border, 24 pt leading gap (PAR-111) | rounded blue iOS bubble | §3.6.6 | C2 | P0 |
| Notifications and automated prompts: folded line or automated bubble with "⚙ label · from source"; owner-relayed messages as the owner's bubble (PAR-112, 402, 407) | one grey line | Port `turncollapse.ts` classification | C2 | P1 |
| Folded turns, plain-chat default with fold marks, live turn shows newest step under WORKING, background-turn runs, Show all work (PAR-403–407) | runs of ≥3 tools grouped | Port `planGutter` / `planHistory` / `planRuns`; setting "Show all work" | C2 | P1 |
| Assistant metadata footer `MODEL \| N.N TOK/S \| 2:03PM` (live: duration ticking), one per run (PAR-116–119) | none | §3.6.12; port `model-display.ts`, `metadata.ts` finish-clock rules; one 1 s ticker for live rows only | C2 | P0 |
| Thinking disclosure "THINKING", OPEN action, secondary-colour body (PAR-121) | italic "Thinking" row | §3.6.9 | C2 | P0 |
| Disclosure state keyed and persisted (PAR-122) | in-memory set per chat | `ui.json`, ≤1000 keys | C2 | P1 |
| Tool row: tool-colour label, ≤3 summary lines, ERR/✗, rail, chevron; result paired by id (PAR-123, 146) | one line, ✕ | §3.6.10 | C2 | P0 |
| Bash segment styling (external tool styles per command segment) (PAR-124) | label only | Port `bashsegments.ts` / `toolcalllogical.ts` (with their tests) | C2 | P1 |
| Expanded output: Read full output, Copy, 6000-byte preview, retry on missing/timeout, auto-open on arrival (PAR-125, 101) | input + output boxes | §3.6.10; prefetch outputs ±1 viewport | C2 | P0 |
| Non-shell calls > 12 rows show 8 + "… +N more lines" (PAR-126) | input truncated at 40 lines | | C2 | P1 |
| System notices as coloured pills, 3-line preview, expand; errors "✗ …"; provider notices; compaction divider; `[Context:` dim; model-only notices hidden (PAR-107, 128–131, 133) | centred grey/red/yellow text | §3.6.11 | C2 | P0 |
| System Instructions box pinned at top (PAR-132) | dropped | §3.6.8 | C2 | P1 |
| Queued messages: muted bubbles with "queued: next turn / message end / …" labels, long-press Remove (PAR-143, 259) | "Queued" bubble | §3.6.6 | C2 (rows), D (actions) | P0 |
| Live status lines: Compacting…, Waiting for connection… 3m 12s, ⟳ retrying in Ns (PAR-144, 158–160) | retry as a system line | §3.6.13 | C2 | P0 |
| Message long-press / footer tap menu: Copy message, Edit/rewind, Read in full (PAR-142, 335) | none | Action list (`fake-10-message-actions`) | C2 (menu), D (rewind) | P0 |
| File envelopes as file chips (PAR-114) | raw text | Chip in the bubble; tap opens "<name> · extracted text" in the reader | C2 | P1 |
| Markdown completeness: strikethrough, nested lists to 12 levels, task lists, `~~~` and indented code, accent heading colours, link underline (PAR-135) | most blocks, plain styling | §3.6.7 | C1 | P0 |
| Code blocks: header with language, Wrap / Open / Copy, no-wrap with horizontal scroll, 8000-char preview (PAR-136) | monospaced lines, no header | §3.6.7 | C1 | P0 |
| Syntax highlighting, TUI palette (PAR-137) | none | Port `tui/src/markdown/highlight.ts` tokenisers | C1 | P1 |
| Tables as a grid, pinned header, alignment, column widths, horizontal scroll, max height (PAR-138) | monospaced lines | §3.6.7 | C1 | P0 |
| Math `$…$`, `$$…$$` with the currency guard (PAR-139) | raw text | P1: TeX→Unicode rendering (super/subscripts, Greek, operators, `\frac` as a/b, display math centred, Noto Math fallback); full typesetting P2 | C1 | P1 |
| Links tappable; bare URLs; local paths (PAR-140, 337, 338) | blue text | http(s) → Moon `open browser <url>`; local paths → sheet with path and Copy (`resolve_file_link`) | C1 | P1 |
| Size caps and "Read full" (PAR-134) | none | Preview caps per PAR-134 → reader | C1 | P1 |
| Full-text reader with Find i/N, Copy (PAR-332) | none | Reader overlay (A ships a minimal one; C1 completes it) | C1 | P0 |
| Text selection (PAR-141) | none | In the reader: long-press selects a word, handles extend, Copy. In the transcript: whole-message copy via the menu. Free selection in the transcript P2 | C1 | P1 |
| Images: user thumbnails, model-attached images via `get_image`, viewer (PAR-108, 113, 127, 252, 253, 255, 347, 400) | none | PNG previews (`get_image format:"png"`, the `png` crate is already a dependency), bounded cache, viewer page with double-tap zoom and pan (no multitouch from X core events) | C1 | P0 |
| Subagent tool call "subagent: …", tap opens the child (PAR-147) | label only | | C2 | P1 |

### 2.5 Streaming states and history

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Continuity: no remount/flicker/height bounce at completion; injected turns split the live row; snapshot adoption rules (PAR-151, 155, 157, 162) | commit then refresh from daemon | Keep one layout identity per turn (key by startedAt), adopt snapshots only as compatible extensions | C2 | P0 |
| `tokens_update` drives the live footer (PAR-154); `context_update` (PAR-281) | ignored | | C2 / E | P0 / P1 |
| `context_compaction_status`, `connection_wait_status`, `stream_retry` with usage-limit reset (PAR-158–160) | retry line only | §3.6.13 | C2 | P0 |
| Notices during a stream (PAR-161) | appended | Tail after the pending turn; error-class splits it | C2 | P1 |
| Tail-follow pauses on drag and resumes within 12 pt of the tail (PAR-167) | `follow` flag | Resume within 33 px | C2 | P0 |
| Reading anchor conserved through growth, inserts above, completion, reload, keyboard (PAR-168, 173) | prepend keeps place | Anchor = (piece key, offset); visible text moves 0 px; tests at the row level | C2 | P0 |
| Jump to latest button when > 120 pt from the tail (PAR-169) | none | §3.6.14 | C2 | P0 |
| Read position per conversation; unread opens at the start of the latest response (PAR-104, 105) | always the bottom | `ui.json` anchors | C2 | P1 |
| Load errors with Retry (PAR-102) | red notice | "The conversation did not respond. Retry to resync; your draft is safe." + Retry | C2 | P0 |
| Viewport-sized first page, initial backfill (PAR-093, 094) | `turns` fixed | perf branch if it touches loading; else C2 | perf/C2 | P1 |

### 2.6 Composer, sending, queue

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Composer island: 26 pt radius, surface fill, 45 % border (focused 55 % borderFocused), same geometry focused or not; controls "+", editor, Send/Mic, Stop (PAR-179, 180) | grey rounded field + separate send circle | §3.6.15 | A (shell), D | P0 |
| Editor: placeholder, 6 lines then internal scroll, **Return inserts a newline**, caret always visible, accent caret (PAR-181, 196) | append/backspace at the end; Return sends | Real editor: caret index, tap to place caret, drag caret, word/line wrap identical to drawing, internal scroll, selection with Copy/Cut/Paste/Select all menu (in-app clipboard + X `CLIPBOARD`). Return = newline needs Moon to keep its keyboard up (§6.3) | D | P0 |
| Send states: ↑ accent; streaming → queue button (warning) opening **Next turn** ("Steer now" on non-Anthropic) / **Message end**; Run for commands; spinner (PAR-183, 257) | always message-end | §3.6.15; recheck streaming when the choice is confirmed; stream ended meanwhile → send directly | D | P0 |
| Enabled rules (PAR-184); optimistic submit with restore on failure (PAR-185, 189); error label above the island (PAR-186) | text cleared, lost on failure | | D | P0 |
| Stop: error-tinted face, spinner while stopping, "already stopping" (PAR-187, 264) | grey square | | D | P0 |
| Protected persistent drafts per conversation and per folder draft; discard (PAR-188, 190) | in memory, lost on restart | `drafts.json` (atomic, never overwritten after a failed read), 500 ms debounce | D | P0 |
| Draft chat with sparse overrides; `new_conversation` sends only explicit overrides (PAR-192, 277, 280) | always sends provider/model/effort | | D (send) + E (inspector) | P0 |
| Queue shadows, unqueue (stays until acked), queue editor, pull back into the composer, `/queue` targets, `queue_notice` (PAR-256, 258–263) | shadows only | Long-press a queued bubble → Edit / Remove / Move back to composer | D | P0 (shadow, remove), P1 |
| Rewind / edit sent message (PAR-266–269) | none | "Edit / rewind" from the message menu; sheet per PAR-267; text and images back into the draft | D | P1 |
| Replay, compact, trim (PAR-270–272) | none | Commands (and chat menu entries) | D | P1 / P2 |
| No next-message suggestion (PAR-401) | none | Do not add | — | — |

### 2.7 Commands and macros

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Picker above the island: Commands/Macros tabs with counts, rows with name (accent) + description, insert-only, nested argument completion, fuzzy slash search, hint footer, `//` escape, unknown commands never sent (PAR-198–206) | none | `fake-16-slash-picker`, `fake-17-slash-args`; §3.6.16 | D | P1 |
| Client-side macro expansion with the TUI's catalog and daemon paths from `tools_available.macroEnvironment` (PAR-207, 208) | none | Generate `src/exocortex/macros.json` from Exocortex `tui/src/macros.ts` with a script (the daemon repo is on the phone at `/opt/exocortex`), `include_str!`; expansion ported from `macros.ts` | D | P1 |
| Inline modifiers `/model /effort /fast /ultrafast /queue` anywhere (PAR-209) | none | | D | P1 |
| Command set: /help /new /model /default-model /effort /fast /ultrafast /rename /replay /compact /trim /goal /chrono /btw /instructions /system /queue /usage /accounts /stop /ping /time /convo (PAR-212–233) | none | Registry in D; commands that open screens emit E's actions | D | P1 (P2 for /trim /time /convo /tokens /proxy) |
| Command result notices in the transcript, long ones in the reader (PAR-206) | none | | D | P1 |

### 2.8 Model, effort, accounts, ops

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Model inspector: Provider / Model / Effort pickers, Speed segmented (Standard/Fast/Ultrafast), Delegation, Allow legacy, "Use as daemon default", Apply / Use for this draft (PAR-275, 276, 278, 282, 283) | flat model list + effort chips | `real-05-model-sheet`, `fake-12-model-inspector`; §3.6.18 | E | P0 (pickers, speed), P1 (defaults, toggles) |
| Context "Context 123,456 / Max 200,000" (PAR-281) | none | Inspector + subtitle "1.0k ctx" | E | P1 |
| Usage limits in Settings: per provider 5-hour/Weekly bars with thresholds, resets, combined Claude capacity (PAR-285, 286); usage chip (PAR-287) | none | `real-08-settings-top` | E | P1 |
| Accounts screen (PAR-288–291), token statistics (PAR-295) | none | Sheets; `accounts {watch}` while open | E | P1 / P2 |
| Goal display and actions (PAR-297, 298); activity panel with tasks, stop task (PAR-299–302, 309) | goal parsed, not shown | Chat menu → Goal, Activity & tasks sheets | E | P1 |
| Chrono list/create/cancel (PAR-303, 304) | none | Sheet + `/chrono` | E (+D command) | P1 |
| BTW side conversation (PAR-305, 306) | none | Bottom sheet reusing C2's block renderer | E | P1 |
| System instructions editor, effective system prompt (PAR-307, 308) | none | | E | P1 |
| Find in chat over loaded history (PAR-331); export transcript (PAR-333) | none | Find sheet → jump and pause follow; export opens the reader | C1 (find engine), E (sheet) | P1 / P2 |

### 2.9 Settings, notifications, voice, attachments, haptics

| iPhone | Today | Plan | Pkg | Pri |
|---|---|---|---|---|
| Settings page: Usage limits, Appearance (theme cards), Text & transcript toggles (Expand thinking, Expand tool calls, Show all work, Fold markers, Haptics), New chats defaults, Notifications, Connection, Local storage, About (PAR-013, 320, 323, 324, 325, 326, 329, 330) | none | `real-08…09-settings-*`; Whale first, palettes for Cerberus and Tonikawa from `themes.ts` | E | P0 (page, appearance, toggles), P1 |
| Completion notifications (PAR-310–316) | `exocortex-notify` + Moon: done | Settings shows Moon's state and a per-conversation Mute (B's menu); add PAR-315's one-per-5 s per conversation in the bridge if missing | E | P1 |
| App badge (PAR-317) | none | Needs Moon icon badges (§6.3) | — | P2 |
| Voice: mic in the composer, recording strip, transcription into the editor (PAR-243–251) | power-key voice already sends to the shown conversation | Keep power-key voice as the main path. P2: in-app dictation through the voice hook in a new "dictate" mode (§6.3): recording strip, levels from the hook's `level L` lines, transcript inserted at the caret | D | P2 |
| Attachments: photos, camera, files, paste image, tray, envelopes (PAR-237–242) | none | P2: "+" sheet with Browse files (an in-app picker over the chroot and Android's shared storage), text and PDF-less files as envelopes, PNG/JPEG images (JPEG needs a decoder crate) | D | P2 |
| Haptics: long-press medium, drop-target selection, composer focus light, copy selection (PAR-345) | none | `control::send("haptic medium")` etc.; Moon plays them only when the user enabled haptics | A (helper), packages (calls) | P1 |
| Dynamic Type / text size (PAR-360) | none | Settings → Text size steps scaling the type tokens | E (+A tokens) | P2 |

## 3. Whale design spec

### 3.1 Principles

1. **Monospaced everything** (JetBrains Mono), as the iPhone (SF Mono) and the
   TUI. Inter stays in the binary for the System app only.
2. **Square corners** (radius 0) everywhere except the composer island (72 px),
   circular composer buttons, switches, the sheet grabber and dots.
3. **Hairlines and rails**: 2 px borders in a border colour at a fixed opacity;
   a 6 px colour rail on the left of user bubbles and tool rows; a 6 px accent
   rail on the open conversation's list row; the 6 px page rail under top bars.
4. **Colour carries meaning**: accent = live/primary, secondary accent =
   links, actions and thinking, warning = waiting/queued, error, success =
   unread/finished, tool colours from the daemon.
5. **Opaque composites**: every translucent CSS colour is given below as an
   opaque colour over the background it sits on. Draw opaque fills; reserve
   `alpha` for text, dims and animation. This keeps partial redraws exact.
6. Every touch target is at least **122 × 122 px** (44 pt), even where the drawn
   control is smaller.

### 3.2 Palette (Whale)

`src/exocortex/theme.rs` holds a `Palette` struct with these tokens (the same
names as `themes.ts`), one `const WHALE`, and later `CERBERUS` and
`TONIKAWA` copied exactly from `themes.ts`. Daemon `#RRGGBB` tool colours
override `tool`.

| Token | Hex | Use |
|---|---|---|
| `accent` | `#1d9bf0` | primary buttons, rails, live state, h1/h2, list markers |
| `secondary_accent` | `#48cae4` | actions (OPEN, COPY), thinking, h3+, inline code text, folder icons, picker values |
| `link` | `#48cae4` | links |
| `text` | `#ffffff` | text |
| `muted` | `#646464` | eyebrows, meta, chevrons, placeholders on the app background |
| `error` | `#ff453a` | errors, Stop, destructive |
| `warning` | `#ffd60a` | waiting, queue button, marked ★ |
| `success` | `#50c878` | unread/finished dot, switch on, Chrono badge |
| `tool` | `#bf5af2` | default tool colour |
| `goal` | `#c792ea` | goal ◆ |
| `app_background` | `#00050f` | chat page, transcript, top bars on chat |
| `sidebar_background` | `#030814` | list page, settings page, sheets |
| `surface_background` | `#090d35` | composer island, picker, anchored menu |
| `selection_background` | `#0f193c` | open row, segmented "on", table header |
| `user_message_background` | `#090d35` | user bubble |
| `automated_user_background` | `#0e1f2c` | automated bubble |
| `border` | `#555555` | all hairlines (at the opacities below) |
| `border_focused` | `#1c94e5` | focused island |
| `notification_background` / `_foreground` | `#c62828` / `#ffffff` | unread count badge |
| `text_selection` | `#4f5258` | selection highlight |
| `search_background` / `_foreground` | `#fce094` / `#00050f` | search and find hits |
| `command` | `#aed6fe` | valid commands/macros inside the editor |
| `on_accent` | `#00050f` | text/icons on accent fills (contrast rule of `apply.ts`) |
| Syntax | keyword `#c792ea`, string `#c3e88d`, comment `#5f6978` (italic), number `#f78c6c`, type/operator `#89ddff`, function `#82aaff`, punctuation `#b4b4b4`; diff `+` string, `-` number, `@@` type | code blocks (PAR-137) |
| Task fallbacks | exo `#1d9bf0`, chrono `#4ec9b0`, bash `#d19a66` | activity rows |

Opaque composites (`mix(fg, alpha, bg)` per channel, rounded; `theme.rs`
provides `mix()` for runtime colours such as tool colours):

| Name | Value | Recipe |
|---|---|---|
| `rule_topbar` | `#2f3136` | border 55 % on app |
| `rule_strong` (hr, dividers) | `#3b3d40` | border 70 % on app |
| `rule_table` | `#333539` | border 60 % on app |
| `rule_code` / `rule_code_head` | `#26292e` / `#1e2128` | border 45 % / 35 % on app |
| `rule_box` (thinking box) | `#22252b` | border 40 % on app |
| `rule_list` | `#2e3036` | border 52 % on sidebar |
| `rule_sheet_row` | `#282b31` | border 45 % on sidebar |
| `rule_field_row` (inspector rows) | `#1a1e26` | border 28 % on sidebar |
| `rule_settings` | `#24272e` | border 40 % on sidebar |
| `rule_sheet_top` | `#3c3e42` | border 70 % on sidebar |
| `island_border` / `island_border_focused` | `#2b2d43` / `#135796` | border 45 % / focused 55 % on surface |
| `menu_border` | `#3d3e4b` | border 68 % on surface |
| `bubble_border` | `#0d2c5e` | accent 22 % on userBg |
| `instructions_border` / `_rule` | `#10588b` / `#093252` | accent 55 % / 30 % on app |
| `inline_code_bg` | `#141922` | text 8 % on app |
| `code_bg` | `#04081e` | surface 40 % on app |
| `thinking_bg` | `#060b14` | muted 6 % on app |
| `thinking_text` | `#6dd5e9` | secondary 80 % over text |
| `text_meta` | `#808287` | text 50 % on app (footer line) |
| `text_dim` | `#999b9f` | text 60 % on app (subtitle, descriptions) |
| `text_quote` | `#bfc0c3` | text 75 % on app |
| `text_output` | `#d1d2d4` | text 82 % on app (tool output, "+" and mic icons) |
| `placeholder` | `#5f627c` | text 35 % on surface |
| `grabber` | `#42464f` | text 25 % on sidebar |
| `switch_off` | `#2b303a` | text 16 % on sidebar |
| `send_disabled_face` / `_icon` | `#1a1e43` / `#535672` | text 7 % / 30 % on surface |
| `stop_face` | `#2b1536` | error 14 % on surface |
| `row_pressed` / `row_lifted` | `#0d1634` / `#0b1430` | selection 80 % / 70 % on sidebar |
| `row_checked` / `drop_target` | `#051528` / `#06172a` | accent 9 % / 10 % on sidebar |
| `menu_item_pressed` | `#0c2251` | accent 15 % on surface |
| `search_box` | `#080c2d` | surface 75 % on sidebar |
| `folder_icon_bg` / `_border` | `#071420` / `#36383c` | secondary 6 % / border 62 % on sidebar |
| `fold_mark` / `fold_mark_open` | `#1e2228` / `#2d3035` | muted 30 % / 45 % on app |
| notice warning bg / border | `#0f120f` / `#594e0d` | warning 6 % / 35 % on app |
| notice error bg / border | `#0f0912` / `#591b1e` | error 6 % / 35 % on app |
| notice muted bg / border | `#060b14` / `#23262d` | muted 6 % / 35 % on app |
| default tool bg / border / pressed | `#0a091a` / `#351d4f` / `#130e26` | tool 5 % / 28 % / 10 % on app |
| quote bar | `#2b7b8f` | secondary 60 % on app |
| link underline | `#205e6f` | link 45 % on app |
| flash (revealed row) | `#051d33` | accent 16 % on app |
| backdrop | black at 55 % (`#000207` over app) | sheets |
| menu dim | `app_background` at 64 % | anchored menu |

### 3.3 Typography

Add JetBrains Mono NL **Medium, SemiBold, Bold, Italic** (OFL, already
licensed in `assets/fonts/OFL-JetBrainsMono.txt`) next to the Regular cut, as
`Weight::MonoMedium`, `MonoSemiBold`, `MonoBold`, `MonoItalic` in `text.rs`
(note the divergence from Moon's vendored copy in its header comment). Inter
Italic can then leave the binary. Advance width is 0.6 em, so the body size
below gives ~41 characters per line, as on the iPhone.

Uppercase labels use `text::draw_tracked`; tracking is in px.

| Token | px (pt) | Weight | Line | Tracking | Use |
|---|---|---|---|---|---|
| `T_EDGE` | 26 (9.5) | Regular | 36 | 1.6 | "BEGINNING OF CONVERSATION" (`text_meta`) |
| `T_EYEBROW` | 28 (10) | Bold | 40 | 2.2 (0.08 em); 2.8 for section titles | THINKING, SYSTEM INSTRUCTIONS, PINNED, sheet titles, settings section titles |
| `T_FOOTER` | 28 (10) | Medium | 40 | 1.1 | metadata footer, uppercase |
| `T_META` | 30 (11) | Medium | 44 | 0 | list meta line, details, working/live status |
| `T_SUBTITLE` | 30 (10.5) | Regular | 40 | 0 | chat subtitle |
| `T_SMALL` | 32 (11.5) | Regular | 46 | 0 | tool output, file names, previews, tool actions |
| `T_CAPTION` | 34 (12) | Regular / Bold | 48 | 0 | tool lines (label Bold), notice previews, composer error, picker descriptions |
| `T_MENU` | 33 (12) | SemiBold | 44 | 2.0, uppercase | anchored menu labels |
| `T_NOTE` | 36 (13) | Regular | 54 | 0 | thinking body, notice body, sheet messages, buttons (SemiBold) |
| `T_BODY` | **40 (14.5)** | Regular | **60** | 0 | transcript text, user bubbles, settings values |
| `T_TITLE` | 40 (14) | SemiBold | 52 | 0 | chat title |
| `T_ROW` | 42 (15) | Medium (SemiBold for folders, Bold when open) | 56 | 0 | list titles, action items, picker names |
| `T_COMPOSER` | 46 (17) | Regular | 62 | 0 | editor |
| `T_PAGE_TITLE` | 34 (12) | Bold | 48 | 2.7, uppercase | "SETTINGS" |
| Markdown h1 / h2 / h3 / h4–6 | 56 / 48 / 42 / 40 | Bold | 1.3 × size | 0 | colours accent / accent / secondary / secondary |
| Inline code | 36 | Regular | — | 0 | `secondary_accent` on `inline_code_bg`, 3 px × 8 px padding |
| `T_CODE` (code block) | 34 | Regular | 52 | 0 | `text` (or syntax colours) on `code_bg` |
| `T_TABLE` | 36 | Regular (header Bold) | 52 | 0 | header `secondary_accent` |

Emphasis: `**bold**` MonoBold, `*em*` MonoItalic, `~~del~~` text at 60 %
with a 2 px line at x-height/2, links `link` with a 2 px `link underline`
4 px below the baseline (dashed for file links).

### 3.4 Spacing and metrics (px)

Spacing scale (4/8/12/16/20 pt): **S1 11, S2 22, S3 34, S4 44, S5 56.**

| Metric | px | From |
|---|---|---|
| Top bar height (window y 0 … 122) + 2 px rule | 122 + 2 | 44 pt |
| Page rail: 3 equal segments, 8 px gaps, 6 px tall, at the bar's bottom edge | 6 | 2 pt |
| Icon in a button / button target | 60 / 122 | 22 / 44 pt |
| Top-bar side padding | 11 | 4 pt |
| Transcript side padding / gap between rows | 40 / 40 | 14 / 14 pt |
| Transcript top / bottom padding | 34 / 40 | 12 / 14 pt |
| Gap between blocks inside an assistant row | 25 | 9 pt |
| Markdown block gap | 24 (0.6 em) | |
| List item indent per level / marker gap | 34 / 30 | 12 pt |
| Quote bar width / indent | 6 / 34 | 2 / 12 pt |
| List row / header row / divider row | 160 / 88 / 30 | 58 / 32 / 11 pt |
| List side padding / folder indent per level | 34 / 40 | 12 / 14 pt |
| Folder icon box | 83 | 30 pt |
| User bubble padding (y, x) / leading gap / rail | 25, 34 / 66 / 6 | 9, 12 / 24 / 2 pt |
| Tool row padding (top, right, bottom, left) / rail | 34, 28, 34, 34 / 6 | 12, 10, 12, 12 pt |
| Disclosure head min height / padding | 122 / 17 × 28 | 44 pt |
| Composer side margin / gap above keyboard or gesture reserve | 34 / 22 | 12 / 8 pt |
| Island min height / radius / inner padding | 144 / 72 / 11 | 52 / 26 / 4 pt |
| Composer button target / face | 122 / 94 (radius 47) | 44 / 34 pt |
| Editor text inset left / vertical padding | 28 / 29 | 10 / 10.5 pt |
| Editor max visible lines | 6 | |
| Jump button size / inset from right and from the island | 122 / 34 / 34 | 44 / 12 pt |
| Sheet max height | 82 % of the window | |
| Sheet grabber | 100 × 11, radius 6, 22 from top | 36 × 4 pt |
| Action item height / padding / icon | 144 / 44 / 60 | 52 / 16 / 22 pt |
| Anchored menu width / item / quick row | 676 / 138 / 172 | 244 / 50 / 62 pt |
| Menu offset from row / screen clamp | 25 / 39 | 9 / 14 pt |
| Settings section padding (top, sides) / toggle row | 50, 44 / 144 | 18, 16 / 52 pt |
| Switch / knob | 128 × 78 / 61 | 46 × 28 / 22 pt |
| Picker max height / hidden below | 664 / 244 | 240 / 88 pt |
| Hairline | 2 | 0.5 pt (rounded up; the A26 is ~385 ppi) |

### 3.5 Lines, radii, shadows

- Borders and rules: 2 px, opaque composite colours from §3.2.
- Radii: 0, except island 72, circular buttons (full), switch 39, grabber 6,
  dots (full). `fill_rrect` with radius 0 = `fill_rect`.
- Shadows (`Canvas::shadow_rrect`) only on floating things, drawn once (not
  per animation frame): jump button (blur 44, black 40 %, y +11), anchored
  menu (blur 78, black 42 %, y +19), lifted list row (blur 44, black 34 %,
  y +19), sheets (blur 110 upwards, black 45 %), toast (blur 61, black 45 %).

### 3.6 Components

Positions are relative to the component unless noted. "Pressed" states draw
while a finger is down on the target (existing `pressed` mechanism).

#### 3.6.1 Top bars

- Background `app_background` (chat) or `sidebar_background` (list, settings);
  2 px `rule_topbar` at y 122; page rail segments over the rule's top
  (y 118–124): segment *i* of 3 spans `x = i·(1080+8)/3 … +(1080−16)/3`;
  the current page's segment `accent`, others not drawn.
- **Chat**: back chevron (stroke 5 px, 26 × 46 px, `text`) centred at
  x 72; ⋯ (three 9 px dots, 18 px apart, `text`) centred at x 1008. Centre
  column x 140…940: title `T_TITLE` `text`, ellipsised, baseline 64;
  subtitle `T_SUBTITLE` `text_dim` baseline 104: `Opus-5.5 · high` + ` · 1.0k
  ctx` in `muted`; a 14 px status dot 14 px left of the subtitle: accent
  pulsing (streaming/hydrating), warning (not ready), error (load failed). New
  chat: title "New chat". Title + subtitle are one 122-high tap target
  (inspector); pressed → both at 60 % alpha.
- **List**: search field from x 17 to x 680, then ⋯ (centre 768), folder-plus
  (888), plus (1008). Search field box inset 14 px top/bottom: fill
  `search_box`, 2 px border `#2e3041`; magnifier 44 px `muted` at x 48;
  placeholder "SEARCH CHATS" `T_CAPTION` tracked 2.2 `muted`; typed text
  `T_ROW` `text`; clear ✕ (122 target) when not empty.
- **Settings**: "SETTINGS" `T_PAGE_TITLE` at x 34, right chevron at x 1008
  (back to the list, Settings sits left of the list).

#### 3.6.2 Conversation row (list)

Height 160, background `sidebar_background` (pressed `row_pressed`; open
conversation `selection_background` + a 6 px accent rail at x 0, y 17…143;
checked in select mode `row_checked`). Left x = 34 + 40·depth.
- Title line baseline 72: optional ★ (`warning`, 36 px) then mark emoji, then
  the title `T_ROW` Medium (Bold when open), ellipsised before the trailing
  area; "Untitled chat" italic `muted` when blank; 🔕 icon `muted` 36 px after
  the title only for an explicit mute; pin icon `accent` only in search
  results or inside folders.
- Meta line baseline 124: `T_META` `muted`: "now · 1,234 messages",
  "5 min. ago · …" (Intl short style: "min.", "hr.", "wk."), plus
  " · N queued".
- Trailing (right edge 1046, items 17 apart): activity dot (§3.6.5) then
  badges `◆N` accent, `$N` warning, `◷N` success, `T_CAPTION` Bold.
- No separators between rows (the iPhone has none).

#### 3.6.3 Folder row, headers, dividers

- Folder row 160: chevron `muted` 39 px at x 34+40·depth (rotates 90° open);
  icon box 83 × 83 at x +60: fill `folder_icon_bg`, 2 px `folder_icon_border`,
  folder glyph 47 px `secondary_accent` (open-folder glyph when expanded);
  name `T_ROW` SemiBold at x +172 baseline 72 with 🔕 if muted; meta "52
  CHATS" `T_META` tracked 1.2 `muted` baseline 124; trailing: aggregate dot,
  unread badge (min 55 × 50, `notification_background`, count `T_META` Bold
  white, "99+" cap, hidden when muted), "+" (122 target, 50 px icon
  `text_output`) creating a draft in that folder.
- PINNED header 88: pin icon 33 px + "PINNED" `T_EYEBROW` `muted` at x 34,
  baseline 66. Search mode header: "SEARCH RESULTS // N".
- Divider 30: 2 px `rule_list` at y 14 from x 34 to 1046.
- Drop target (P2): fill `drop_target`, 6 px accent inner border; reorder
  line 8 px tall accent capsule between rows, x 22…1058.

#### 3.6.4 List header menu and states

- ⋯ opens the anchored menu (§3.6.17) with SELECT CHATS, NEXT UNREAD, UNDO
  LAST DELETION, REDO DELETION, SETTINGS (`real-03-sidebar-menu`).
- Empty: centred "No conversations" `T_ROW` SemiBold + New chat button.
  Loading: spinner + "Loading chats…" `T_NOTE` `muted`. Error: text +
  Retry. No search results: "No results" with a 111 px magnifier `muted`.
- Connection pill when not ready (bottom centre, 44 above the gesture
  reserve): `surface` 92 % fill, 2 px border60, spinner + "Connecting…"
  `T_CAPTION`.

#### 3.6.5 Activity dot and badges

Dot: 25 px circle drawn as a 6 px ring with a 6 px gap and a filled centre
(the CSS `.dot`); colour accent (streaming; pulses 100 % → 45 % → 100 % over
1.6 s), warning (waiting), success (unread). Count after it when > 1,
`T_META` Bold, "99+".

#### 3.6.6 User, automated and queued bubbles

- Bubble: right-aligned in the row, width = longest wrapped line + 68,
  max 1080 − 80 − 66 = 934. Fill `user_message_background`, 2 px
  `bubble_border`, 6 px `accent` rail on the left edge, text `T_BODY` `text`
  **right-aligned** per line, padding 25/34. Plain text, no markdown. File
  chips (P1) inside the bubble, right-aligned: 122 high, max 665 wide, fill
  text 2.5 %, 2 px border50, 83 px icon box in `secondary_accent`, name
  `T_SMALL` SemiBold, "482 KB · truncated" (`warning`).
- Optimistic (not yet echoed): the same at 86 % alpha.
- Automated: fill `automated_user_background`, rail `secondary_accent`, and
  "⚙ Chrono" (or "⚙ <label> · from <source>") `T_META` italic `muted`
  right-aligned 17 px below.
- Queued: the bubble drawn at 58 % alpha with a `muted` rail; label below
  "queued: next turn" / "queued: message end" / "queued: after <title>" /
  "queued: at 14:05", `T_META` italic, mini spinner while optimistic.

#### 3.6.7 Assistant text and markdown

Full width (x 40…1040), `T_BODY`, blocks 24 px apart (`fake-rich-step-00`):
- Headings as §3.3, 10 px extra space above.
- Lists: markers in `accent` (• for level 1, ◦ level 2+, "3." numbers in
  `accent`), text indent 34 + 30 per level; task items ☑/☐ in `accent`.
- Quote: 6 px `quote bar` at x 40, text x 74 in `text_quote`.
- Rule: 2 px `rule_strong`, 12 px above and below.
- Inline code: `inline_code_bg` box (text bounds + 8 px each side), text
  `secondary_accent` 36 px.
- **Code block**: box fill `code_bg`, 2 px `rule_code`. Header 122 high:
  language uppercase `T_EYEBROW`-size `muted` at x +28 (or "CODE"); actions
  right-aligned, each 122 wide: WRAP / NO WRAP, OPEN, COPY in
  `secondary_accent` 30 px; 2 px `rule_code_head` under it. Body padding
  28 × 34, `T_CODE` 34/52, `white-space: pre` with **horizontal scroll** (a
  per-block x offset kept in view state, dragged horizontally, clipped to the
  box) unless WRAP; syntax colours when highlighted. Over 8000 characters:
  preview + note "Preview · OPEN/COPY includes the complete code." `T_SMALL`
  `muted`. A fence still streaming is code to the end of the text.
- **Table**: grid of 2 px `rule_table` lines; header row fill
  `selection_background`, text Bold `secondary_accent`; cells padding 14 × 25,
  `T_TABLE` 36/52, wrap inside the column, alignment from the delimiter row.
  Column widths 443 / 377 / 343 px for 2 / 3 / ≥4 columns (160 / 136 / 124 pt),
  at most 12 columns; the grid scrolls horizontally like code; more than 10
  rows → max height 997 px with vertical scroll inside, header pinned. Under
  it "Open full response" `T_SMALL` `secondary_accent` (122 target).
- **Math** (P1): inline math in `text` with TeX→Unicode; display math centred
  on its own line, horizontally scrollable when wide, with "Copy LaTeX"
  (`T_SMALL` `muted`). Bad TeX shows the source.
- Links: `link` + underline; tap opens (http(s) via Moon, paths via the file
  sheet).
- "Read full paragraph (N characters)" / "Open all N blocks" buttons: 122
  high, 2 px `secondary40` border `#1d5464`, `T_SMALL` `secondary_accent`.

#### 3.6.8 System instructions box

Full width, 2 px `instructions_border`. Head 122: chevron `muted` at x +40,
"SYSTEM INSTRUCTIONS" `T_EYEBROW` `accent`, then a one-line preview `T_SMALL`
`muted`. Open: 2 px `instructions_rule`, body `T_NOTE` `muted`, padding 17/34.

#### 3.6.9 Thinking

Box fill `thinking_bg`, 2 px `rule_box`. Head 122: chevron `muted` (rotates
90° in 240 ms when opening), "THINKING" `T_EYEBROW` `muted` tracked 2.2;
open: "OPEN" `T_EYEBROW` `secondary_accent` right (opens the reader), 2 px
`rule_box` under the head, body markdown `T_NOTE` 36/54 in `thinking_text`,
padding 22/34 (`fake-07-rich-expanded`). Adjacent thinking blocks merge;
empty thinking is not drawn. Setting "Expand thinking" opens by default.

#### 3.6.10 Tool row

Box fill `mix(tool, 5 %, app)`, 2 px `mix(tool, 28 %, app)`, 6 px rail in the
tool colour; pressed head `mix(tool, 10 %, app)`. Head: padding 34/28/34/34;
lines `T_CAPTION` 34/48 in the tool colour: label Bold ("$" for bash, the
style label, or the MCP server name) + space + summary, wrapped, at most 3
lines then "… +N more lines" `muted`; a red ✗ after the summary on error.
Right column (77 px): "ERR" `T_CAPTION` Bold `error`, or a 28 px spinner while
running; chevron `muted` 31 px (rotates when open). Only rows with output
(or still running) get the chevron. Open: 2 px `mix(tool, 22 %, app)` rule;
actions row (122): "READ FULL OUTPUT" and "COPY" `T_SMALL` `secondary_accent`
with 39 px icons, 50 px apart; output `T_SMALL` 32/46 `text_output` (`error`
when isError), padding 11/34/28, first 6000 bytes then "… preview; open full
output above"; "No output." `muted`; loading: spinner + "Loading output…";
missing: "This result is no longer available. Tap to retry."
(`fake-02-rich-top`, `fake-07-rich-expanded`).

#### 3.6.11 Notices and dividers

- System notice: box fill/border from the notice colour (§3.2), head
  padding 34/28: info-circle icon 42 px at x +34, text `T_CAPTION` in the
  notice colour, 3 lines then "…", chevron `muted`; open: full markdown
  `T_NOTE`, "System notice" eyebrow, Copy action. Muted notices at 60 %
  (`fake-03-rich-2`).
- Errors: "✗ message" in an error notice.
- Compaction: "Compaction finished" `T_META` `muted` centred with 2 px
  `rule_strong` lines filling both sides, 28 px gaps, 28 px space above/below.
- "[Context: …]" text blocks: `T_CAPTION` `muted`.
- History edge: "BEGINNING OF CONVERSATION" `T_EDGE` tracked `text_meta`,
  centred, 78 px band; "Loading earlier…" with a spinner while paging.

#### 3.6.12 Metadata footer

One line under the last assistant segment of a run, 22 px below it:
`OPUS-5.5 | 76.2 TOK/S | 2:03PM` in `T_FOOTER` `text_meta`, uppercase,
tracked 1.1, digits tabular (mono already is). Live: third part is the
duration `1M 35S` ticking each second (redraw only that line); final: the
finish clock (`2:03pm`, `yesterday 10:41pm`, `Oct 19 9:12am`). The model part
ellipsises first. The whole line is a 122-high tap target opening the message
menu. Suspended (durable sleep): `warning`.

#### 3.6.13 Working row and live status

- WORKING · N TOOL CALLS (P1 folding): `T_EYEBROW` with a 28 px spinner;
  shimmer per §3.8.
- Live status lines (P0): `T_META` with spinner: "Compacting conversation
  context…" `muted`; "Waiting for connection… 3m 12s" `warning` (spinner arc
  warning); "⟳ <msg> — retrying in 12s (2/5)…" `warning`.

#### 3.6.14 Jump to latest

122 × 122 square, fill `app_background`, 2 px `#444547`, shadow, arrow-down
56 px stroke 5 `text`; right 34, bottom 34 above the island. Shown when more
than 332 px (120 pt) from the tail and nothing is loading; fades in 180 ms.
Tap: stop any fling, snap to the tail, follow on.

#### 3.6.15 Composer

- Wrap band: from the island top − 17 to the bottom, filled
  `app_background` (the iPhone's gradient becomes solid).
- Island: x 34…1046, bottom = window height − 22 − (170 when the keyboard is
  down, 0 when up); height = max(144, lines·62 + 58 + 22); fill
  `surface_background`, 2 px `island_border` (`island_border_focused` while
  the keyboard is up for it), radius 72 (`fill_rrect` + `stroke_rrect`).
- Row (bottom-aligned, inside 11 px padding): "+" button (target 122, icon
  50 px stroke 5 `text_output`); editor from x +150 to the trailing buttons;
  trailing: **Mic** (`text_output` icon) when empty and not streaming
  (P2 dictation; until then hidden), **Send** face (94 circle `accent`, arrow
  `on_accent` stroke 7) when there is text, **Queue** face (`warning`, list-plus
  icon black) when there is text and a turn streams, **Run** (a 133 × 94
  capsule "Run" `T_NOTE` Bold) for a command, **Stop** face (`stop_face`,
  30 px `error` square radius 4) while streaming. Disabled send:
  `send_disabled_face` / `_icon`. Pressed: face scaled to 88 % (draw smaller).
- Editor text `T_COMPOSER` 46/62 `text`, placeholder "Message" `placeholder`,
  caret 5 × 52 px `accent` (blinks 530 ms on/off while focused), selection
  fill `mix(accent, 35 %, surface)` = `#0b3e7d`, valid commands/macros
  in `command`.
- Error label above the island: warning-triangle 36 px + text `T_CAPTION`
  `error`, ✕ (122 target); rises 17 px and fades in over 200 ms.
- `fake-15-composer-keyboard`, `fake-18-streaming`, `real-04-new-chat`.

#### 3.6.16 Picker (commands and macros)

Directly above the island (17 px gap), x 34…1046, fill `surface_background`,
2 px `#3d3e4b`; head 122: tabs "Commands 27" / "Macros 21" `T_CAPTION`
SemiBold (on: `accent`, off: `text` 50 %; counts `text` 40 %), ✕ at the
right; rows ≥155: name `T_ROW` `accent`, description `T_CAPTION` `text_dim`,
"↖" insert hint `muted`; row rules `mix(border, 28 %, surface)`; height
min(space above − 34, 664); hidden under 244. Hint footer: usage +
description `T_CAPTION` `text_dim`, warnings in `warning`.

#### 3.6.17 Overlays: sheets, action lists, anchored menu, dialogs

- **Sheet**: backdrop black 55 %; panel from the bottom, `sidebar_background`,
  2 px `rule_sheet_top` top border, grabber, title `T_EYEBROW` `muted` at x 44;
  max height 82 %; content scrolls inside; rides above the keyboard.
  Dismiss: tap the backdrop, drag the grabber down > 150 px, Back.
- **Action list** (in a sheet): items 144 high, padding 44: icon 60 px
  `secondary_accent` (accent / warning / error tones), label `T_ROW` `text`,
  detail `T_META` `muted`; 2 px `rule_sheet_row` above each item; pressed
  `selection_background`; destructive in `error`; a centred "Cancel" `muted`.
  (`fake-09-chat-menu`, `real-07-plus-menu`.)
- **Anchored menu** (list long-press, list ⋯): dim = `app_background` 64 %
  everywhere except an even-odd cut-out of the pressed row (the row stays
  visible, lifted); menu 676 wide, 25 px below the row (above if it does not
  fit), clamped 39 px from the edges; fill `surface_background`, 2 px
  `menu_border`, shadow; items 138: icon 56 px in tone colour + label
  `T_MENU` uppercase; dividers 2 px inset 141 px; pressed `menu_item_pressed`.
  Order: Pin/Unpin (accent), Mark/Unmark (★ warning), Mute/Unmute, Rename…,
  Move to folder…, Delete (error).
- **Confirm dialog**: centred box 900 wide, fill `surface_background`, 2 px
  `menu_border`, title `T_ROW` Bold, message `T_NOTE` `text_dim`, buttons as
  §3.6.19 (destructive + Cancel).
- **Text prompt** (rename, new folder): sheet with a 127-high field (fill
  `surface`, 2 px `#3d3e4b`, focused 2 px `mix(accent, 80 %)`), message line,
  Cancel / Create buttons.
- **Reader**: full-screen overlay page with its own top bar (Done, title,
  Copy, Find), monospaced `T_NOTE` text; Find bar: field + "i/N" + ‹ ›; hits
  `search_background` / current hit outlined `accent`.
- **Toast**: top centre at y 150, padding 22 × 39, fill `#090d35` 95 %,
  2 px border60, `T_CAPTION`; error variant `error` text + border; 3.5 s.
- **Snackbar** (undo): above the composer or at the list bottom, full width
  minus 68, fill `surface_background`, "Moved to trash" + "UNDO" `accent`
  button; 6 s.

#### 3.6.18 Inspector rows, segmented control, switch, buttons

- Inspector/sheet section: "GENERATION" `T_EYEBROW` `accent` tracked 2.8,
  padding 44 top 17 bottom.
- Picker row 133: label `T_BODY` `text` left, value `T_BODY`
  `secondary_accent` right + chevron-down `muted`, rule `rule_field_row`;
  tap opens an action list of choices (checkmark `accent` on the current).
- Segmented: 2 px `#3b3d40` box; segments ≥122 high, `T_NOTE`; on: fill
  `selection_background`, text Bold `text`, 6 px `accent` underline; off
  `muted`.
- Switch: 128 × 78 pill, off `switch_off`, on `success`, knob 61 white with a
  small shadow, slides 220 ms.
- Buttons ≥122 high, padding 17 × 44, `T_NOTE` SemiBold: secondary (accent
  text, 2 px `mix(accent, 50 %)` border), primary (fill `accent`, text
  `on_accent`), destructive (error text and border), plain (no border).
  Disabled 35 % alpha. Pressed: fill current colour 12 %.

#### 3.6.19 Settings

Page `sidebar_background`; sections: title `T_EYEBROW` `accent` tracked 2.8,
then a 2 px `rule_settings` rule and rows. Usage block per provider:
"ANTHROPIC" `T_CAPTION` Bold + "N accounts" `muted`; rows "5-hour" `T_BODY`
with "NN% remaining" (or "NNN/200%" for combined capacity) `T_BODY` Bold
right in accent (warning ≤ 20 %, error at 0); bar 14 px (track `switch_off`,
fill accent/warning/error) 11 px below; "Resets in 1h15m · 4h05m" `T_META`
`muted`. Nav rows (Accounts,
Token statistics): title `T_BODY`, detail `T_META` `muted`, chevron. Theme
cards: 3 columns 22 px apart, 2 px `rule_strong` border (selected: 4 px
`accent`), three 39 px swatches, name `T_NOTE` Bold + check, summary `T_META`
`muted`. Toggle rows 144 with switch (`real-08-settings-top`).

#### 3.6.20 Empty, loading, spinner, icons

- Centred state block: title `T_ROW` SemiBold, body `T_NOTE` `muted`, an
  optional button; at least 443 px tall.
- New-chat state: "NEW CHAT" `T_EYEBROW` `accent` tracked 3.3, under it a
  122-high box (2 px `#22252b`) with "Opus-5.5 · xhigh" `T_CAPTION` `muted`
  that opens the inspector (`real-04-new-chat`).
- Spinner: 39 px ring, 4 px stroke, track `text` 25 %, 90° arc `accent`
  (`ring_arc`), one turn per 700 ms.
- Icons are vector paths in `ui/icons.rs` (stroke 5 px, round caps, drawn
  in a 60 px box): chevron left/right/down, plus, ellipsis, search, x,
  folder (filled), folder-open, folder-plus, pin, star (filled), bell-off,
  arrow-up, arrow-down, stop, mic, list-plus (queue), gear, check, info-circle,
  copy, file-text, image, refresh, trash, pencil, target (goal), timer
  (chrono), message-square (BTW), list-checks (activity), undo, redo,
  download-circle (next unread), bolt (steer), clock (message end),
  warning-triangle. Shapes follow the iPhone's `ui/icons.tsx` (lucide).

### 3.7 Motion

| What | Duration | Curve | How on the CPU canvas |
|---|---|---|---|
| Page push/pop (list ⇄ chat, settings) | 380 ms (Moon) | ease-out | Moon's GPU transition (`control::transition`), as today |
| Sheet in / out | 280 / 220 ms | ease-out | Draw the backdrop dim once (one full frame), then slide the panel with `Presenter::scroll`-style row moves of the panel's rows and upload only the newly exposed strip; never redraw the whole panel per frame |
| Anchored menu in / out | 170 / 160 ms | ease-out | Dim drawn once; menu rect only: fade 0 → 1 (alpha) and scale 0.94 → 1 approximated by drawing the menu inset (cheap: 676 × ~800 px) |
| Disclosure chevron | 240 ms | ease-out | Redraw the chevron's 60 × 60 rect only; the body appears at once (no height animation) |
| Jump button | 180 ms | ease-out | Its rect only |
| Composer error / picker rise | 200 / 160 ms | ease-out | Their rects only |
| Switch knob | 220 ms | ease-out | Switch rect only |
| Toast in / out | 250 / 200 ms | ease-out | Toast rect; restore what was under it from the canvas copy kept before drawing it |
| Spinner | 700 ms / turn | linear | 30 Hz, its rect only |
| Streaming / activity dot pulse | 1.4 s / 1.6 s | ease-in-out | 30 Hz, the dot only |
| Shimmer (WORKING) | 1.6 s | linear | Draw the label in `muted`, then `set_clip` to a 30 %-wide band moving across it and draw it again in `text`; label rect only |
| Caret blink | 530 ms on / off | step | Caret rect only |
| Long-press lift | starts 150 ms, recognised 600 ms | — | At 150 ms the row turns `row_lifted` with its shadow; at 600 ms the menu opens (medium haptic). No scaling |
| Revealed message flash | 1.2 s | ease-out | Row background `flash` fading to none, 30 Hz, row rect only |
| Live footer duration | 1 s steps | — | Footer line only |

`ease_out(t) = 1 − (1 − t)^5` stands in for CSS `cubic-bezier(0.22, 1, 0.36,
1)`; `ease_in_out(t)` is the usual cubic. Animations ask the event loop for
frames through one `Animator` (A); when none runs, the loop sleeps as
today. Every animated rect is damage the presenter already knows how to
upload (`present_rows`).

### 3.8 Haptics

Through Moon (`control::send("haptic STYLE")`; Moon plays them only when the
user has turned haptics on): long-press recognised `medium`; drop target
changes `selection`; composer focused `light`; message or text copied
`selection`; switch toggled `light`. No haptic on arriving at the list
(PAR-012).

### 3.9 Rules for drawing at 120 Hz

1. Layout is cached per piece (history entry, live turn, list row kind) and
   keyed by its content and the state that changes it (expanded keys,
   horizontal offsets, theme, width). A streaming reply re-lays out from its
   last markdown block only (keep the previous layout until the new one is
   ready; PAR-342).
2. Vertical positions snap to 1/16 px (`snap`), so row moves stay
   pixel-exact. Every new drawing op must keep the existing test property:
   an incremental frame equals a full redraw, pixel for pixel, and nothing
   changes outside the reported damage. Add a test for each new kind of
   piece and each animation.
3. Horizontal scrollers (code, tables, display math) redraw only their own
   box rows when dragged.
4. Text measurement and wrapping are cached per (string, weight, size);
   monospace widths are `chars × 0.6 × size` except for fallback glyphs
   (emoji, CJK), which go through `text::measure`.
5. No per-row timers: one 1 s ticker (live footers, wait clocks), one 60 s
   ticker (list times), spinners at 30 Hz, all redrawing only their rects.
6. Full-window redraws only on page changes, theme changes, keyboard
   show/hide and overlay open/close.

## 4. Shared architecture (built by WP-A, used by everyone)

The packages run in parallel only if they stop editing the same files. Today
`view.rs` (3,100 lines), `model.rs` and `proto.rs` hold everything. WP-A
splits them along the package lines below, then each package owns its files.

```
src/exocortex/
  app.rs            event loop (A); turns X input into gestures, routes them
  gesture.rs        A: tap, long-press (150/600 ms, 39 px slop), drag, fling,
                    horizontal drag, left-edge swipe; velocity
  anim.rs           A: Animator, curves, frame requests, damage rects
  theme.rs          A: Palette (Whale; Cerberus/Tonikawa later), composites,
                    type tokens (§3.3), metrics (§3.4), mix()
  store.rs          A: JSON files under $EXOCORTEX_APP_DATA
                    (default /var/lib/exocortex-app/), atomic write, 500 ms
                    debounce, never overwrite a file that failed to parse
  requests.rs       A: reqId registry, deadlines, routing, stale guards
  clipboard.rs      A: X CLIPBOARD owner + in-app paste buffer
  ui/widgets.rs     A: top bar, page rail, buttons, segmented, switch,
                    spinner, dot, badges, eyebrow, hairline, empty state
  ui/icons.rs       A: the icon set (§3.6.20)
  ui/overlay.rs     A: overlay stack + trait Overlay { layout, draw, hit,
                    gesture, scoped }; sheet, action list, anchored menu,
                    confirm, prompt, toast, snackbar, minimal reader
  view/mod.rs       A: frame orchestration, ChatFrame/ListFrame, damage
  view/ops.rs       A: Op (Text with tracking, Rect, Stroke, Line, Icon,
                    Image, Clip group with an x-scroll key), draw_ops
  model/mod.rs      A: Model { ui, sidebar, chat, composer, ops }, event
                    fan-out to sub-models, Action = Nav | List | Chat |
                    Composer | Ops (each sub-enum lives in its owner's file)
  proto/mod.rs      A: parse_event dispatcher, Event enum (all variants this
                    roadmap needs, parsed leniently as today)
  proto/{sidebar,chat,ops,composer}.rs   event payload structs and command
                    builders per area (owners extend their file)
  view/list.rs, model/sidebar.rs, list/*.rs            B
  view/markdown_layout.rs, view/code.rs, view/table.rs,
  view/math.rs, highlight.rs, markdown.rs, reader.rs,
  images.rs, find.rs                                   C1
  view/transcript.rs, view/blocks.rs, view/tools.rs,
  fold.rs, model/chat.rs, footer.rs, model_display.rs  C2
  view/composer.rs, editor.rs, model/composer.rs,
  commands/*.rs, macros.rs, macros.json, drafts.rs     D
  view/chat_header.rs, screens/settings.rs,
  sheets/{inspector,usage,accounts,goal,activity,
  chrono,btw,system}.rs, model/ops.rs                  E
```

Interfaces fixed by A so nobody waits:

- `chat_header::subtitle()` returns the subtitle text and dot (A stub: model
  label; E fills it in).
- `reader::open(title, text)` (A minimal; C1 completes find/selection/copy).
- `blocks::lay_blocks(blocks, ctx)` is C2's; C1 provides
  `markdown_layout::lay(source, x, width, top, style) -> Laid` and the code,
  table and math pieces it calls.
- `composer::Act::{Insert(String), RestoreDraft{conv, text}, Focus}` so C2's
  message menu and D's rewind meet; `ops::Act::{OpenInspector, OpenGoal,
  OpenActivity, OpenChrono, OpenBtw, OpenAccounts, OpenSettings, ...}` so D's
  commands open E's screens.
- `ui::haptic(Haptic)`, `ui::open_url(url)` (Moon `open browser <url>`),
  `clipboard::copy(text)` / `clipboard::paste()`.
- Store files: `settings.json` (A, E writes through `Settings`), `ui.json`
  sections `list` (B), `chat` (C2: anchors, disclosures), `drafts.json` (D).

Safety for everyone: the phone's daemon is the user's real one. Read freely;
for anything that sends, renames, moves, pins, deletes or changes defaults,
use a test daemon (`EXOCORTEX_SOCKET` at an Exocortex `scripts/dev/testd`
instance) or a folder you created for the test and remove it after. Never
send `restart_daemon`, `login`/`logout` or defaults mutations to the real
daemon.

## 5. Work packages

Six packages. WP-A first (it is the base and touches every file); then B,
C1, C2, D and E in parallel, each in its own worktree, owning the files in
§4. Every package keeps `scripts/check.sh` green and the incremental-frame
tests passing, adds tests for its model logic and its new pieces, verifies
on the A26 (install per `scripts/install.sh`, keep the previous binary to
put back), and compares its screens with `/opt/build/exo-reference/`
(`/opt/a26-shell/bin/a26-shellshot` for the phone side). Frame budget on the
A26 with `EXOCORTEX_APP_PROFILE=1`: scroll and streaming frames p95 ≤ 8.3 ms
(today 7.0 ms), never worse than `exo-perf`'s numbers.

### WP-A: Whale foundation and chrome

**Scope.** The module split of §4 (behaviour-preserving first commit: move
code, no changes, all tests pass); `theme.rs` with the Whale palette, the
composites and the type and metric tokens; the four JetBrains Mono cuts in
`text.rs` and `assets/fonts`; `ui/widgets.rs` and `ui/icons.rs`; the overlay
stack with sheet, action list, anchored menu, confirm, prompt, toast,
snackbar and a minimal reader, with the animations of §3.7; `gesture.rs`
(tap, long-press, drags, horizontal drags, left-edge swipe Back);
`anim.rs`; `store.rs`; `requests.rs` (deadlines, routing, readiness after
`pong`); `clipboard.rs`; the full `Event` surface of §2 parsed in
`proto/` (with fixtures); `Action` sub-enums; pages List / Chat / Settings
(Settings an empty Whale page with its top bar, reachable from the list's ⋯
and from the left-edge swipe on the list) with Moon transitions; the Whale
top bars (§3.6.1) on all three pages; the composer island shell (§3.6.15:
geometry, colours, buttons as today's behaviour), window and page
backgrounds. Existing rows, bubbles and tool rows are only recoloured to the
palette (their redesign belongs to B, C2, D).

**PAR.** 001, 004, 006, 007, 012, 015, 017–020, 023, 025–028, 091 (kept),
179 (shell), 320–322 (Whale), 345 (helper), 350, 361; protocol coverage of
§22 of PARITY.md for every event this roadmap uses.

**Files.** All of `src/exocortex/` (split), `src/text.rs`, `assets/fonts/`,
`scripts/prepare-assets.py` inputs if fonts need registering, `docs/exocortex.md`.

**Depends on.** `exo-perf` merged first (the split moves the code it is
changing). If it is still running, do the new files first (theme, fonts,
widgets, icons, overlay, gesture, anim, store, requests) and the split last,
rebased on `exo-perf`.

**Acceptance.**
- Split commit: no behaviour change; all existing tests pass unchanged
  (moved, not rewritten).
- Every page draws in Whale with JetBrains Mono; top bars match §3.6.1 within
  a few px against `real-04-new-chat` / `real-01-sidebar` / `real-08-settings-top`
  (scaled 1170 → 1080).
- Overlay tests: a test sheet and menu open/close with their animations;
  each animation frame equals a full redraw of the same state; damage covers
  only the animated rows. Frame p95 ≤ 8.3 ms during a sheet slide on the A26.
- Gesture unit tests with synthetic timings: long-press at 600 ms, lift at
  150 ms, cancelled by a 40 px move, vertical scroll wins over long-press,
  left-edge swipe Back.
- Store: a corrupt file is reported and never overwritten; writes are atomic.
- Requests: deadline → actionable error, stale replies dropped, Stop not
  gated on readiness.
- Notifications and the app/bridge hand-off still work (open from a banner,
  suppression while shown).

### WP-B: Conversation list and organisation

**Scope.** Tree folders with persisted expansion; PINNED header and
divider; conversation and folder rows (§3.6.2–3.6.5) with TUI activity
semantics, badges and effective mute; the one-minute ticker; search;
header controls and ⋯ menu; long-press anchored menu (Pin, Mark, Mute,
Rename…, Move to folder…, Clone, Copy id, Delete) with confirm and undo
snackbar; new folder; "+" new chat in a folder; empty/loading/error states
and the connection pill; pull to refresh; optimistic moves and pin placement
(`sidebar-pending.ts`), send bumps and the optimistic new-conversation row;
then select mode (Organize), folder inspector with instructions, next
unread; drag and drop last.

**PAR.** 051–085 (083 P2), 087–090, 056, 060, 076, 080, 082.

**Files.** `view/list.rs`, `model/sidebar.rs`, `list/{actions,search,sheets,pending}.rs`,
`proto/sidebar.rs`.

**Depends on.** WP-A (overlays, gestures, widgets, store).

**Acceptance.** Model tests port the TUI's ordering, activity and folder
aggregation cases and the optimistic-move projection; list frames stay
pixel-exact under scroll with the ticker running; long-press timing and
haptic on the A26; search over the real list (thousands of conversations)
answers within 160 ms debounce + one frame; every mutation verified on a
test daemon (or a test folder) and rolled back on error.

**Order.** Rows and tree → indicators/badges → search → menu (pin, mark,
delete + undo, rename, mute) → new folder / "+" → optimistic projection →
move / clone / copy id → select mode, folder inspector → drag.

### WP-C1: Markdown and rich content

**Scope.** Markdown completeness (§3.6.7) on the existing tolerant parser;
`markdown_layout.rs` (moved from view) producing `Laid` pieces with
accent headings, list markers, quotes, rules, inline code boxes, links;
code blocks with header, Wrap/Open/Copy and horizontal scroll; syntax
highlighting ported from the TUI; tables as grids with pinned header,
alignment, widths, both-axis scroll and "Open full response"; size caps and
"Read full"; math with the currency guard and TeX→Unicode; links to Moon's
browser and the local-file sheet (`resolve_file_link`); the full reader
(Find i/N, word selection with handles, Copy); the find engine reused by
Find in chat; images: `get_image` PNG previews with a bounded cache, user
thumbnails, model-attached images, and an image viewer page.

**PAR.** 133–140, 141 (reader), 108, 113, 127, 252–255, 332, 337, 338,
342, 347, 400; find engine for 331.

**Files.** `markdown.rs`, `view/markdown_layout.rs`, `view/code.rs`,
`view/table.rs`, `view/math.rs`, `highlight.rs`, `reader.rs`, `find.rs`,
`images.rs`, `proto/chat.rs` (image and file-link payloads only).

**Depends on.** WP-A (Op set with clip groups and images, reader stub,
clipboard, open_url). Provides `markdown_layout::lay` to C2; until C2 lands,
call it from the existing `lay_blocks`.

**Acceptance.** Parser and layout tests for every construct in the
iPhone's `rich.ts` transcript (headings, nested and task lists, quote,
inline and display math with "$5 and $10" staying text, code with a long
line, a right-aligned table) and against streaming prefixes (no raw-syntax
flash: a prefix never lays out taller than its completion by more than the
growing block); horizontal scroll frames pixel-exact; a 2,000-line code
block and a 200-row table scroll within budget; highlighting matches the
TUI's token classes on its test inputs; side-by-side with
`fake-rich-step-00/01`.

**Order.** Images first (model-attached images from the `attach` tool and
user images: the user cannot see screenshots the agent shows today, and
needs to) → markdown styling → code blocks → tables → reader + find + copy →
links → highlighting → math.

### WP-C2: Transcript structure, streaming and scrolling

**Scope.** User, automated and queued bubbles (§3.6.6) and file chips;
assistant rows with block spacing; thinking disclosure; tool rows with
outputs, actions, retry, prefetch, 12-row cap, bash segment styling;
system notices, errors, provider notices, compaction divider, `[Context:`,
hidden model-only notices; System Instructions box; metadata footer with
model display names, throughput, live duration and finish clock; live
status lines (compaction, connection and usage waits, retries); streaming
continuity (one identity per turn, snapshot adoption, injected turns,
notices during a stream); disclosure persistence; tail-follow rules,
anchor conservation, jump to latest, per-conversation read position, unread
opening at the latest response; message menu (Copy message, Read in full,
Edit / rewind via `composer::Act`); load errors with Retry; then the TUI's
turn folding (plain-chat folding with fold marks, notifications on the
assistant side, live turn's newest step under WORKING, background-turn
runs, Show all work).

**PAR.** 092, 097–107, 109, 111, 112, 114–132, 142–147, 150–164,
165–169, 173, 177, 402–407.

**Files.** `view/transcript.rs`, `view/blocks.rs`, `view/tools.rs`,
`fold.rs`, `footer.rs`, `model_display.rs`, `model/chat.rs`,
`proto/chat.rs` (stream and history payloads).

**Depends on.** WP-A; uses C1's `markdown_layout::lay` (stable signature from
A). Turn folding needs `queue_updated` details that D also reads (shared
read-only through `model.composer.queue`).

**Acceptance.** Reducer-style tests: completion without height change,
injected turn splitting, snapshot adoption, unwind; the TUI's
`turncollapse` test cases ported; anchor tests: visible text moves 0 px
through growth above, prepend, completion, keyboard show/hide (measured on
laid-out pieces); jump button rules (> 332 px from the tail, hidden while
loading); streaming a long reply on the A26 stays within budget with the
footer ticking; side-by-side with `fake-02…08`, `fake-18-streaming`.

**Order.** Bubbles, tool rows, thinking, notices (look) → footer →
streaming continuity and live status → jump/anchors/read position → message
menu → System Instructions, file chips → folding.

### WP-D: Composer, sending, queue and commands

**Scope.** The editor (caret index, tap/drag to place, wrapping identical to
drawing, internal scroll past 6 lines, Return = newline, selection menu
with Copy/Cut/Paste/Select all); Send / Queue / Run / Stop states and the
"Deliver message" choice (Next turn / Steer now, Message end) with the
recheck and direct-send fallback; enabled rules on readiness; optimistic
submit with restore into the originating draft; error label; persistent
protected drafts per conversation and per folder draft, discard; sparse
draft overrides on `new_conversation`; queue shadows, unqueue on ack, queue
editor (`update_queued_message`, `move_queued_message`), pull back into the
composer; rewind / edit (eligibility, sheet, `unwind_conversation`); then
the picker, command registry and commands (§2.7), macro catalog generation
and expansion, inline modifiers, `//`, guards, command notices; then P2
dictation and attachments.

**PAR.** 179–197, 198–236, 256–264, 266–273, 237–251 (P2), 401.

**Files.** `view/composer.rs`, `editor.rs`, `model/composer.rs`,
`drafts.rs`, `commands/*.rs`, `macros.rs`, `macros.json`,
`scripts/sync-exocortex-macros.*`, `proto/composer.rs`.

**Depends on.** WP-A (island shell, store, requests, clipboard, overlays);
E's `ops::Act` variants (declared by A) for commands that open screens;
Moon keeping its keyboard up on Return (§6.3), with today's re-show as the
fallback.

**Acceptance.** Editor tests: caret stays visible typing 16 numbered lines,
tap places the caret at the right glyph (mono and emoji), wrap equals the
drawn wrap; submit failure restores text into the right draft even after
navigating away; drafts survive an app restart and a corrupt file is never
overwritten; queue menu choices send the right `timing`; Stop works while
not ready; picker inserts only, never sends; macro expansion matches the
TUI's `macros.ts` test cases; command guards. On the A26: typing with
Moon's keyboard at full speed stays within budget (only the island rows
redraw).

**Order.** Editor → send/queue/stop states → drafts → optimistic restore and
errors → queue rows' actions → rewind → picker → commands → macros → inline
modifiers → P2 dictation/attachments.

### WP-E: Conversation controls, ops and settings

**Scope.** Chat header subtitle priorities and status dot, title tap and ⋯
chat menu (§2.1); the model inspector (provider/model/effort pickers, speed
segmented, Delegation, Allow legacy, Use as daemon default, Use for this
draft / Apply, daemon defaults confirmed by reqId, context line); Settings
page sections (Usage limits, Appearance with theme cards and Cerberus and
Tonikawa palettes, Text & transcript toggles, New chats, Notifications,
Connection with diagnostics, Local storage, About); ops sheets: Goal,
Activity & tasks (stop task), Chrono (list, create, cancel), Ask aside
(BTW, rendered with C2's blocks), System prompt, Accounts, Token statistics;
Find in chat sheet (C1's engine) and export; daemon shutdown / offline
states; notification rate limit in the bridge if missing.

**PAR.** 008–011, 013, 014, 029, 030, 035, 275–309, 310–317 (settings
side), 320, 323–326, 329–331, 333, 086.

**Files.** `view/chat_header.rs`, `screens/settings.rs`, `sheets/*.rs`,
`model/ops.rs`, `proto/ops.rs`, `src/bin/exocortex-notify` only for PAR-315.

**Depends on.** WP-A; C2's block renderer for BTW (use plain text until it
lands); C1's find engine for Find in chat (use substring search until it
lands).

**Acceptance.** Inspector tests: provider change picks its first model,
unsupported effort falls back to the model default, speed resets when
unsupported, defaults confirmed only by a `conversation_defaults` with the
same reqId (test daemon); subtitle priority table tests; theme switch
redraws everything in the new palette with no Whale colour left (grep
`theme.rs` use); usage colours at 21 % / 20 % / 0 %; accounts watch
starts/stops with the sheet; side-by-side with `real-05…10`,
`fake-09`, `fake-12`.

**Order.** Header subtitle and menu → inspector → Settings page with
Appearance and toggles → usage → goal / activity → chrono → BTW → system
prompt → accounts / tokens → find/export → themes beyond Whale.

### Suggested sequence

```
exo-perf ──► WP-A ──┬─► WP-B
                    ├─► WP-C1 ──┐
                    ├─► WP-C2 ◄─┘ (uses markdown_layout)
                    ├─► WP-D
                    └─► WP-E
```

Merge B, C1, C2, D and E as each reaches its P0 rows (the user-visible
order that matters most: C2 and D first, then B, then C1 and E), then
continue with P1 in the same worktrees rebased on main. A short integration
pass after the first wave: one A26 walkthrough of every screen against the
reference shots, frame profile, and the notification hand-off.

## 6. Dependencies outside these packages

### 6.1 On `exo-perf`

This roadmap assumes `exo-perf` lands before WP-A's split and owns: open and
history load latency, parse cost on the link thread, layout/glyph caches,
and (if it takes them) the snapshot cache for an instant cold start
(PAR-039–050) and viewport-sized first pages (PAR-093/094). Whatever of
those it does not do moves to WP-A (cache, store-backed) and WP-C2 (paging).
Every package measures against `exo-perf`'s frame numbers, not today's.

### 6.2 In this repository

- `exocortex-notify`: only PAR-315 (rate limit) if missing; notification
  content and suppression already match PAR-311–316.
- `text.rs` gains weights (keep the Moon sync note accurate).

### 6.3 In OSS-Phone-V1 (Moon), each optional, with today's fallback

| Need | Why | Fallback |
|---|---|---|
| Keyboard purpose that keeps the keyboard up on Return (e.g. `keyboard show multiline`) | Return inserts a newline (PAR-181) | the app re-shows the keyboard after Return, as today |
| The Browser consuming `open browser <url>` targets | tapping links | copy the URL with a toast |
| Status bar colour per app (`#00050f`) | seamless top edge (Moon draws it black over apps) | black; the difference is barely visible |
| App-driven (finger-following) page transitions | iPhone-like interactive swipe | swipe recognised, Moon's transition plays at release |
| A "dictate" mode of the voice hook (record, transcribe, print the text) | in-app mic (PAR-243–251) | power-key voice, which already sends to the shown conversation |
| System-wide paste key on Moon's keyboard | pasting from other apps | in-app clipboard |
| Icon badges on the home screen | PAR-317 | none |

## 7. Not applicable on this phone

SSH transport, device identity, pinned host key, channel framing and
reconnect ownership (PAR-016, 021, 022, 032, 033, 327); hot updates,
bundles, rollback, `shell.ready` (PAR-351–359, 328); APNs and iOS
background grace (PAR-318, 319; Moon's lease and `exocortex-notify` cover
it); iOS permission prompts and Photos (PAR-254, 310's permission part);
VoiceOver labels and reading order (PAR-362–365: Moon has no screen reader);
portrait lock and white-flash concerns (PAR-005, 006: Moon is portrait and
the window background is set before the first frame); Dynamic Type beyond a
Text size setting (PAR-360, P2); `/quit`, `/ssh`, terminal-only TUI features
(PARITY.md §23).
