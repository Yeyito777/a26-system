# The Exocortex app and `exocortex-notify`

`a26-system --app exocortex` is Moon's client for the user's
[Exocortex](https://github.com/Yeyito777/exocortex) daemon: its conversations,
their history and live replies, new conversations and the model each uses.
`exocortex-notify` is a small always-running bridge that turns finished turns
into Moon notifications and keeps the phone awake while a turn runs. Both are
in this repository and installed by `scripts/install.sh`:

| File (chroot) | What |
| --- | --- |
| `/opt/a26-system/bin/a26-system` | the app (`--app exocortex`), started by Moon |
| `/opt/a26-system/bin/exocortex-notify` | the bridge, supervised outside Moon |
| `/opt/a26-system/share/exocortex-app.bgrx` | the launcher icon (`assets/exocortex-app.png`) |

## Talking to the daemon

Both are ordinary full clients (not app-API clients) of the daemon's Unix
socket, newline-delimited JSON; the wire spec is exocortex
`shared/src/protocol.ts` and `docs/daemon-ipc.md`. The socket is
`$EXOCORTEX_SOCKET`, else `/opt/exocortex/config/runtime/exocortexd.sock`
(the phone's daemon runs as root in the chroot from `/opt/exocortex`).
`src/exocortex/proto/` parses the daemon's whole event surface leniently:
unknown events and fields are ignored, missing ones take defaults
(`proto/fixtures.jsonl` holds one example of every event, which the tests
parse).

The app declares `targeted-unwind`, `sidebar-reorder-delta`,
`sidebar-state-patch`, `history-block-pagination`, `token-stats-on-demand`,
`sidebar-incremental` (with a tree `sidebarView` naming the folders open on
the list) and `update-status-push` (the daemon's update status, shown under
the list: "Daemon update available", "Daemon restart needed"), then `ping`s.
Its bootstrap, and every reconnection's, is that, with `load_conversation`
for the conversation on screen before the `ping`, in the same write, so its
window comes ahead of the list. A window or an older page the daemon has not
answered in 15 s is given up on: the chat says so (a late answer still
lands), and scrolling up asks for the older page again.

| Screen | Daemon traffic |
| --- | --- |
| The list | `conversations_list` (first page, then the rest), kept current by `conversation_updated`, `conversation_deleted`, `sidebar_items_reordered`, `sidebar_state_patched`, `conversation_moved`, `queue_updated` (queued counts) |
| List commands | `pin_conversation`, `pin_folder`, `pin_sidebar_items`, `mark_conversation`, `mute_conversation`, `mute_folder`, `move_sidebar_items` (refused: a `conversations_list` with its reqId), `delete_conversation`, `delete_conversations`, `delete_folder`, each followed by a `ping` with its reqId (`updateStatusOnly`); `rename_conversation`, `create_folder` (with the items to move in), `rename_folder` (acked); `clone_conversation`, `undo_delete`, `redo_delete` (answered by `conversation_restored` or a state patch); `load_folder_instructions`, `set_folder_instructions`; `list_conversations` to start over (pull to refresh, Retry) |
| A conversation | `load_conversation` with `turns`, the `viewport` and an `initialBackfill` (which subscribes and marks it read): a window of about two screens, then the older page behind it from the same snapshot; older pages (15 turns) with `load_conversation_history` as the user scrolls up, joined where a page ends inside an entry; the stream (`streaming_started`, `block_start`, `text_chunk`, `thinking_chunk`, `streaming_sync`, `tool_call`, `tool_result`, `message_complete`, `streaming_stopped`, `user_message`, `system_message`, `history_updated`, whose `pendingAI: null` ends the live reply); after each stop the newest page again, so the history settles on the daemon's copy. Leaving it `unsubscribe`s. |
| Sending | `send_message` (shown at once, confirmed by the daemon's `user_message` or `streaming_started` with the same `startedAt`); while it works, Send asks "Deliver message": **Next turn** (Steer now on other providers) or **Message end**, `queue_message` with that `timing` (shown as queued until delivered; sent directly if the turn ended meanwhile); a new conversation is `new_conversation` with its first message and only the draft's explicit model choices (in the folder whose "+" was tapped). Refused or unconfirmed in 20 s: the text goes back into the draft it came from, with the error above the composer |
| The queue | a queued bubble tapped: Edit… (`update_queued_message`), deliver at the other timing, Edit in composer, Move earlier / later (`move_queued_message`), Copy text, Remove from queue (`unqueue_message`; the bubble says "Removing…" until the daemon's `queue_updated` settles it). `/queue [target]` in a message puts it on the daemon's idle queue (`source: global-idle` with its `waitTarget`) |
| Commands | Run: `/model`, `/effort`, `/fast`, `/ultrafast` (through E's `PickModel` / `PickEffort` / `PickSpeed`), `/default-model` (`set_conversation_defaults`, `reset_conversation_defaults`), `/rename` (`rename_conversation`, `generate_title`), `/replay`, `/compact`, `/trim`, `/instructions` (`set_system_instructions`), `/goal`, `/chrono` and `/btw` (through E's goal, Chrono and aside requests and panels), `/stop`, `/ping`, `/reload`; the rest open E's panels or say something |
| Rewind | the message menu's Edit / rewind: the text back into the draft, then `unwind_conversation` with its `userMessageIndex` (from the windows' `historyStartUserIndex`) and `expectedStartedAt` |
| The mic | `exocortex-voice dictate` (OSS-Phone-V1 `docs/voice.md`): its `level` lines drive the strip, `stop` on its stdin transcribes (the daemon's `transcribe_audio`, by the hook) |
| Stop | `abort` with the stream's `expectedStartedAt`; a spinner until `ack`, `streaming_stopped` or `message_complete`; a second press says it is already stopping |
| Panels (E) | Activity: `list_tasks`, `stop_task`, `abort` for a subagent; Goal: `set_goal` (its `goal_updated` reaches subscribers only: the open conversation); Chrono: `chrono` list, create, cancel; BTW: `btw_query`, `btw_followup`, `btw_close` (sent again after a reconnection until settled); System prompt: `get_system_prompt`, `load_instructions`, `set_system_instructions`; Accounts: `accounts {watch}` while it shows; Usage: `usage_update` (pushed), `get_token_stats` when Settings or a usage panel opens; the inspector's "Use as daemon default" and reset: `set_conversation_defaults`, `reset_conversation_defaults`, confirmed only by a `conversation_defaults` with their reqId |
| The model inspector | `tools_available`'s providers, models, efforts and speeds, and `conversationDefaults` (or `conversation_defaults`); Apply sends `set_model`, `set_effort`, `set_fast_mode` for what changed; a draft changes nothing on the wire until its first message |
| Tool outputs | `load_tool_outputs` for a row opened, and for rows within a screen of what shows (six at a time, one request in flight; history leaves outputs out); one that does not come says so ("no longer available", "timed out") and a tap asks again |

The app has three pages, left to right as on the iPhone: Settings, the
list, a conversation. Back (the top bar's chevron, Moon's Back, a swipe from
the left edge) goes a page left, ending a search on the list first; the
list's ⋯ menu opens
Settings too. Everything is drawn in the Whale theme and JetBrains Mono
(roadmap §3); sheets, menus, dialogs, toasts and the reader open over the
pages.

The list is the iPhone's (`real-01-sidebar`): pinned rows under a PINNED
header and a divider, folders opening in place (the open ones kept in
`ui.json`'s `list.expanded`), each row with the TUI's activity dot (accent
streaming, warning waiting, success unread; a muted folder mutes what is in
it) and badges (◆ subagents, $ background commands, ◷ Chrono), a folder with
its count, unread badge and "+" for a new chat in it. The search field
filters titles and folder names (results 160 ms after the last key). A long
press opens a row's menu (pin, mark and mute side by side; rename, emoji
mark, move to folder, clone, select, copy id, delete; a folder's has its
inspector, with its own instructions); deleting offers Undo. Select mode
(the ⋯ menu's Select chats) checks rows and moves, pins, groups into a new
folder or deletes them. A held row moved on 16 pt is dragged: dropped on
a folder it moves in, between conversations of its pinned group it is
reordered there (`move_sidebar_items` with `before`, `preservePinned`,
`placement`); near the list's edges it scrolls. Pulling the list down past
the top asks for it again. The daemon's update status shows under the
last row.

A change shows at once (`list/pending.rs`): the fields it expects are
projected over the daemon's values after every event until the daemon shows
or answers them, so other traffic cannot flicker it back; placement follows
the daemon's rules (a pin goes to the bottom of the pinned section, a move
to the top of its folder's unpinned section). A message sent bumps its
conversation and a new conversation shows at once (`Model::handle` shows
the list what goes out, `list_observe`). Most list commands answer nothing,
so each is followed by a `ping` with its reqId: the daemon handles a
client's commands in order, so a change the daemon's state does not show by
the `pong` was not made: it is taken back, the user told, and the list asked
for again. An unanswered change goes after 10 s.

The transcript is the iPhone's (roadmap §3.6.6–3.6.14, `fake-02…08`): the
System Instructions box at the top (the folder's and the conversation's;
opens to the text, Copy and Open; C1's folder draft draws its inherited
instructions through the same `blocks::lay_instructions`); the user's
bubbles right-aligned with an accent rail (sent ones not yet echoed at 86 %,
queued ones at 58 % with "queued: next turn"), documents sent with them as
file chips (D's `attachments::split`; a tap reads "<name> · extracted text"),
the owner's own words a notification relays as their bubble ("via
Discord/…"); notifications as a row ("CHRONO  Chrono wake: …") opening to
the automated bubble with "⚙ Chrono · from …" under it (a parent's
delegation is shown as that bubble); replies as markdown
(`src/exocortex/markdown.rs`); thinking boxes ("THINKING", OPEN for the
reader; Settings' Expand thinking opens them); a tool row per call in its
tool's colour, its lines as the TUI shows them (`toolcall.rs`: a bash
command per segment, each in its tool's label and colour; an exo call's
prompt), folded to three rows for a shell call and to eight of one longer
than twelve, ERR and ✗ when it failed, a spinner while it runs, "Open
subagent conversation" for the subagent an exo call started; opened, Read
full output, Copy and a 6000-byte preview; system notices as coloured
boxes (three lines, open to all of it and Copy), the compaction divider,
`[Context:` text dim, model-only notices not at all; one metadata footer
per run of replies (`OPUS-5.5 | 76.2 TOK/S | 2:03PM`, `footer.rs`, model
names from `model_display.rs`), a tap on it or a long press on a message
opens Copy message / Edit in composer / Read in full. The live reply shows
WORKING until it shows something, the retry, compaction and
connection-wait lines, and a footer whose duration ticks; notices that come
while it streams wait after it (an error splits it). A jump-to-latest
button shows more than 120 pt from the end once the transcript is still.

Turns fold (`fold.rs`, the TUI's `turncollapse` as the iPhone ports it, in
its gutter look) unless Settings' Show all work is on: a finished turn reads
as the prompt, the final reply and its footer; thinking, tool calls,
interim text, informational notices and notifications leave no row, and a
thin mark at the left edge of the reply that follows (or a short line in
the gap, when nothing answered) opens that stretch, its first row where the
tapped one was. The live turn shows WORKING with what folded ("2 TOOL
CALLS, 1 MESSAGE", a tap opens it) and its newest step. Flipping Show all
work closes the folds opened one by one; a Find match inside a fold opens
it. Disclosures toggled (the newest 1000) and where each of the last 20
conversations was read are kept in `ui.json` ("chat"): a conversation
reopens where it was read, an unread one at its latest response.

The replies' markdown (`src/exocortex/markdown.rs`, GFM as the iPhone renders it): headings in
the accent colours, paragraphs (a newline is a line break), bullet,
numbered and task lists nested to 12 levels, quotes, rules, fenced (``` and
~~~) and indented code, tables, with bold, italic, strikethrough, inline
code and underlined link spans, and TeX math. A code block has a header
(its language, WRAP / NO WRAP, OPEN in the reader, COPY), keeps its lines
and scrolls sideways, is coloured by the TUI's highlighter (`highlight.rs`:
its 14 rule sets ported with a small regex engine, checked against the
TUI's own output on its corpus), and over 8000 characters shows a preview.
A table is a grid (header row, cells wrapped and aligned per its delimiter
row, at most 12 columns) that scrolls sideways when wide; over 10 rows it
is at most 997 px tall and its rows scroll inside it under the header;
"Open full response" opens the reply in the reader. Math (`$…$` with the
TUI's currency guard, `\(…\)`, `$$…$$`, `\[…\]`) is drawn by the TUI's
renderer ported to `math/` (checked against the TUI's corpus): inline as one
line of Unicode, display as centred rows on a monospaced grid (stacked
fractions, limits, matrices), scrolled sideways when wide, with "Copy
LaTeX". The iPhone's caps apply: 128 blocks a reply, then "Open all";
a block over 24,000 characters shows 8,000 and "Read full paragraph".

Links open: a web address in Moon's Browser, a mail address or number is
copied, a local path (relative, `/abs`, `~/`, `file://`; underlined dashed)
is resolved on the daemon's host (`resolve_file_link`) into a sheet with
its path, kind, size and Copy path (`links.rs`). Bare `https://` and `www.`
addresses are links too, and links inside a scrolled table are where they
show.


Images a model attached (the `attach` tool) show after its message, and
the user's pasted images under their bubble: thumbnails fitted in 465 px
(pasted ones as 233 px filled squares), wrapping onto more rows. Each is a
`get_image` PNG preview of the size drawn, asked for while it is within a
screen of the viewport (nearest first, three at a time, retried with
backoff while "not found" or unanswered), decoded on the link thread while
the event is parsed, and kept in a cache bounded by its pixels
(`images.rs`). A tap opens the viewer (`viewer.rs`): i/N, swipe between a
message's images, double tap to zoom 2.5x (with a sharper preview) and drag
to pan, swipe down, Back or ✕ to close.

The reader (`reader.rs`) shows a text as written (wrapped at spaces or
anywhere), how many characters, Copy, and Find: a field on Moon's keyboard,
i/N, previous and next, every match marked and the current one scrolled
into view. A long press selects a word; moving on or dragging its handles
extends it; COPY copies it. `find.rs` is the search: the reader's, and
`search_messages` for Find in chat (case- and accent-insensitive, with the
iPhone's previews).

The composer is a text area on Moon's `keyboard show multiline` (Return is
a line break; only the Send button sends). Its editor (`editor.rs`) keeps a
caret and a selection by cluster (an emoji sequence is one step) and wraps
the text exactly as it draws it, so a tap places the caret at the glyph
under the finger; a long press selects a word (moving the finger extends
it), the handles and the caret can be dragged, and a tap at the caret or on
the selection opens the edit menu (Cut, Copy, Paste, Select, Select all over
Moon's clipboard). Six lines show; more scroll inside, the caret's line kept
in view. Drafts are kept per conversation and per new chat in a folder, in
`drafts.json`, across restarts; the chat's ⋯ menu discards one.

A slash word at the caret opens the picker above the composer (`commands/`,
`view/picker.rs`): commands at the start of the prompt (Commands and Macros
tabs with counts), macros and the inline modifiers anywhere, then their
arguments (providers, models, efforts, `/queue` targets) by a fuzzy search
ported from the TUI ("/model opus" finds "/model anthropic claude-opus-5-5").
A suggestion inserts its text and never runs; a typed command makes the
trailing button Run. Send and Run go through the TUI's pipeline: `//` sends a
literal slash; shorthands resolve ("/mod opus"); an unknown command is
refused (the text stays, the reason above the composer); inline modifiers
(`/model p m`, `/effort x`, `/fast`, `/ultrafast`, `/queue [target]`) apply
first and leave the text; macros (`macros.rs`, the TUI's catalog in
`macros.json`, regenerated with `scripts/sync-exocortex-macros.ts`) expand
with the daemon's paths. Command results show as notices in the transcript
(long ones, `/help`, `/model`, in the reader; a new chat's as toasts).
Valid commands, macros and their arguments are drawn in the command colour
as they are typed. A path word at the caret (`~/`, `./`, `../`, `/a/`)
completes from the daemon host's files (`list_path_directory`, through
`commands/paths.rs`: one prefetch and one targeted request per folder,
listings 15 s fresh and 5 min stale), never the phone's. The chat's ⋯ menu
has Edit… (the TUI's Ctrl+W): the chat's queued messages back into the
composer, its last sent messages to rewind and edit, its instructions as
`/instructions …`.

"+" opens Add attachment or voice: Browse files, Paste image, Record voice
(`model/attach.rs`). The file browser (`view/files.rs`) is a sheet over the
phone's own files, starting at home (Android's shared storage stays locked
under Moon, so it is not there): Up, folders, files with size and date,
dotfiles on request. Paste image (and the edit menu's Paste when the
clipboard holds no text) takes the image Moon keeps (`clipboard get image`;
X's CLIPBOARD on a host without Moon). Files are read, and the clipboard's
image fetched, on threads (`files.rs`), into the draft they were chosen for;
a tray of chips in the island (a PNG's thumbnail, else a glyph; name, size,
✕) scrolls sideways. Images (PNG, JPEG, GIF, WebP up to 4 MB) go as `images`
on `send_message`, `queue_message` and a new chat's `initialMessage`; other
files as their text (UTF-8, else Latin-1 for known text types; up to
120,000 characters, cut and marked) in the iPhone's `<exocortex_file …>`
envelope after the message (`attachments.rs`). Six items and 12 MB with the
text at most; no images to a model without vision; commands carry none. A
send that does not arrive, a rewind and a queued message pulled back give
them back (documents out of their envelopes, images whose bytes the daemon
still has). A draft's attachments are kept in memory only.

The mic shows when nothing is typed: it runs Moon's voice hook
(`dictation.rs`), and a recording strip takes the editor's row (cancel, the
red dot and the clock, the waveform of the hook's levels, finish). The words
go in at the caret of the draft they were spoken for, wherever the user is
by then; a recording ends with its chat and stops by itself before the
capture's 120 s limit; transcription is given up on after 90 s. While it
records, Moon keeps the screen on (`awake on`, then `awake off`).

Sending `turns` matters: without it the daemon does not page at all. It
sends the whole history on every open (790 KB for a long conversation) and
treats the client as a legacy subscriber, whose every `history_updated`
carries the whole history too. With it, windows and updates stay within the
daemon's byte budget (128 KB), however long the conversation.

All daemon I/O runs on the app's link thread (`src/exocortex/link.rs`), which
also parses events, so a megabyte list never costs the UI a frame. It sleeps
in `poll(2)` on the socket and an eventfd, and wakes the UI through another
eventfd that the UI loop polls with X. When the daemon goes away the app says
"Reconnecting…" and connects again (backoff 0.1 s to 4 s), then bootstraps
from scratch. Moon's state (a tapped notification's target, the time zone) is
read on its own thread too, once a second while the app is in front.

Back in front after more than 20 s, the app reconnects: Moon froze it, so the
daemon buffered the broadcasts it could not read, and a new connection drops
them and brings the list up to date. Moon freezes the app right after hiding
it, so the app reads `UnmapNotify` only once thawed: the time it was hidden is
when its loop last went to sleep. The reconnection is quiet: what was on
screen stays (no "Reconnecting…", the live reply kept) for up to 1.5 s while
the new connection catches up, and events still queued from the dropped one
are discarded.

## Opening, laying out and drawing fast

- **Kept conversations.** The last eight conversations opened are kept
  (`Shelf`, `model/chat.rs`). Reopening one shows it at once; the daemon's fresh window
  then replaces only what it covers when the two agree where they meet (the
  history before is the same: an unwind only cuts the end), or everything.
  A conversation opened with nothing to show waits up to 110 ms for its window
  before drawing "Loading…", so Moon's page slide (it holds for the first frame
  up to 150 ms) shows its messages. The daemon is asked before Moon is asked
  to snapshot the page.
- **Entries have stamps.** Every history entry has a stamp (`Chat::stamps`)
  that changes only when what it shows does: a window that resends an entry
  unchanged keeps its stamp (a reply's metadata alone does not change it).
  The view caches an entry's plan by stamp. The live reply has a stamp too,
  and the entry it is committed into keeps it, through completion and the
  daemon's copy: one identity per turn, so the reader's place and the pieces
  on screen survive (an injected message splits the reply: the part so far
  is committed, and the rest goes on under a new stamp).
- **Pieces.** The transcript is laid out as pieces: a markdown block, a tool
  or thinking row (an opened run of tool calls is a row per call), a bubble,
  a notice. Each is cached by a hash of what it shows (`ViewState::pieces`),
  so a streamed chunk lays out only the paragraph that grew, and a reply that
  is committed or settles on the daemon's copy reuses the live reply's pieces.
  A piece knows the rows its glyphs reach (`Laid::ink`); only those on screen
  are visited. The reader's place, when not following the newest message, is
  a piece (entry stamp and index) and its distance from the top; it is kept
  when anything changes above it (older history, a fresh window).
  Following resumes within 12 pt of the end.
- **Marks.** What changes with time inside a piece (a spinner, the WORKING
  shimmer, the live footer's clock and rate, a status line's clock) is a
  mark (`blocks::Mark`): placed with its piece, keyed per frame
  (`transcript::TranscriptFrame`) and drawn again alone, so a second ticking
  costs its own line of rows and no layout pass. Fold marks at the left
  edge are the layout's `gutters`: one that scrolls into view costs nothing
  more, a pressed or changed one its own column. The fold plan is made
  again only when its inputs change (the entries' stamps, the live reply's
  steps, the folds toggled, the marks setting), not on every streamed
  chunk. The jump button is drawn
  over the transcript in its own columns; it steps aside while the
  transcript moves (a floating button over moving rows would cost its rows
  in every frame of a scroll) and fades back in once it is still.
- **Chips** (E, `sheets/chips.rs`: usage at 20 % or less, the aside, tasks)
  stay under the chat header while the transcript moves: a page-level
  floating overlay that says its columns (`Overlay::columns`), so a frame
  draws it again only there (`FrameDamage::areas`, also where a page move
  carried its old pixels) and sends rows and areas in one upload
  (`Presenter::present_rows_and_areas`). Their shadow is compact, as the
  jump button's. Measured dragging a tool-heavy chat with the usage chip
  showing, draw and present: median 2.0-2.2 ms a frame, p95 6.7-8.4 ms;
  without it 1.8-1.9 ms, p95 5.2-7.3 ms.
- **The list's rows.** The rows (`sidebar::Layout`) are rebuilt only when
  rows may come, go or move (a pin, a sort order, a folder opened, the
  search), and folder activity is added up once per change
  (`Sidebar::aggregates`). A list frame records, for each row on screen, a
  hash of what it draws (`Look`): the next frame moves the rows a scroll
  moved and draws again only the rows whose look changed, a pulsing dot only
  its own rows, a relative time turning over (a frame each minute) only its
  row. Only rows on screen are looked at, however many there are.
- **A fast core.** While in front, the UI thread asks for a minimum
  utilization clamp (`sched_setattr`, `UTIL_CLAMP_MIN`), so energy-aware
  scheduling runs its short bursts on a fast core at a high clock: on a slow
  one a full chat frame took 25-35 ms, on a fast one 4 ms. Off when hidden.
- **Fonts** are parsed per face when first used (fontdue parses every glyph:
  about 6 ms a face on a fast core), the ones the first frame does not use on
  other threads at start.

Measured on the A26 (Moon in a sandbox on a hidden Xorg, the user's real
daemon and conversations; before, then after this work): opening the longest
conversation (EXO, 82 entries, over 790 KB of history) 11.9 s, then 17 ms to
its first frame with messages (10 ms reopened); a short one 84-94 ms, then
13 ms (10 ms reopened). Drag-scrolling EXO, draw and present per frame: mean
4.0 ms, p95 7.9 ms, then 2.4 ms, p95 3.7 ms. A streaming reply: mean 7.1 ms,
p95 21.9 ms, then 3.5 ms, p95 15 ms. Back in front after 30 s frozen while a
reply streamed: first frame in 14 ms, caught up in 27 ms. The list on a cold
start: about 95 ms after the process starts, both.

## The code: who owns what, and the interfaces between

The app is split along the work packages of `docs/exocortex-roadmap.md`
(§4, §5): A, the foundation; B, the list; C1, markdown and rich content;
C2, the transcript; D, the composer; E, conversation controls, ops and
Settings. Each package edits its own files and reaches the others through
the interfaces below. Every file's header says whose it is.

| `src/exocortex/` | Owner | What |
| --- | --- | --- |
| `app.rs` | A | the event loop: X input to gestures, overlays first, then the page; the link, Moon, the store, deadlines, frames |
| `gesture.rs`, `anim.rs` | A | the gesture recognizer; curves, tweens, pulses and the `Animator` |
| `theme.rs` | A | palettes, composites, `mix()`, type tokens, metrics |
| `store.rs`, `requests.rs`, `clipboard.rs` | A | kept files; request ids, owners and deadlines; copy and paste |
| `ui/` | A | `widgets.rs`, `icons.rs`, the overlay stack (`overlay.rs`) and its overlays (`sheet.rs`, `menu.rs`, `toast.rs`), haptics, links, the status bar (`mod.rs`) |
| `view/mod.rs`, `view/frame.rs`, `view/ops.rs` | A | page composition and hit testing, the frame pipeline, laid-out pieces |
| `model/mod.rs` | A | `Model`, `Action`, the pages, the connection, the event fan-out, `Ui` |
| `proto/mod.rs`, `proto/fixtures.jsonl` | A | `Event`, `parse_event`, the bootstrap |
| `view/list.rs`, `model/sidebar.rs`, `proto/sidebar.rs`, `list/` | B | the list, folders, search; activity and titles (`list/activity.rs`), the search field (`list/search.rs`), commands (`list/actions.rs`), changes shown before the daemon answers (`list/pending.rs`), menus, prompts and pickers (`list/menus.rs`), the folder inspector (`list/sheets.rs`) |
| `view/markdown_layout.rs`, `view/code.rs`, `view/table.rs`, `view/media.rs`, `markdown.rs`, `highlight.rs`, `math/`, `links.rs`, `images.rs`, `viewer.rs`, `reader.rs`, `find.rs`, `model/rich.rs` | C1 | markdown as pieces, code blocks (highlighted), tables, math; links and the file sheet; images in the transcript, their cache and the viewer; the reader, selection and Find; rich content's state and actions (`Action::Rich`, `Owner::Rich`) |
| `view/transcript.rs`, `view/blocks.rs`, `view/tools.rs`, `fold.rs`, `toolcall.rs`, `footer.rs`, `model_display.rs`, `model/chat.rs`, `proto/chat.rs` | C2 | the transcript, streaming, history; turn folding (`fold.rs`), tool-call lines (`toolcall.rs`), footers |
| `view/composer.rs`, `view/picker.rs`, `view/files.rs`, `model/composer.rs`, `model/attach.rs`, `proto/composer.rs`, `editor.rs`, `drafts.rs`, `dictation.rs`, `attachments.rs`, `files.rs`, `commands/`, `macros.rs`, `macros.json` | D | the composer and its editor (caret, selection, wrap, the edit menu, the attachment tray), sending, the queue, kept drafts, the mic, attachments and the file browser, the command picker, commands, macros and path completion |
| `view/chat_header.rs`, `screens/settings.rs`, `sheets/`, `model/ops.rs`, `proto/ops.rs` | E | the chat's header, Settings, the model sheet and the other panels |

A package adds its new files (`list/*.rs`, `commands/*.rs`, `sheets/*.rs`
and so on, roadmap §4) and declares them in `mod.rs` files; those one-line
additions are the only shared edits.

### Model: actions, handlers, events, requests

- **Actions.** `Action` is `Nav` (pages, A), `List(sidebar::Act)`,
  `Chat(chat::Act)`, `Composer(composer::Act)`, `Ops(ops::Act)` and
  `Ui(UiAct)` (A: `Dismiss`, `Copy`, `OpenUrl`, `Read`, `Toast`). A package
  adds variants to its own `Act` and handles them in its `handle_*`
  (`handle_list`, `handle_chat`, `handle_composer`, `handle_ops`), which
  returns the commands to send. Interfaces fixed for meeting points:
  `composer::Act::{Insert(String), RestoreDraft { conv, text }, Focus,
  Rewind(stamp), Discard}` (`Model::can_rewind(stamp)` says whether a sent
  message can be rewound now)
  (C2's message menu, D's rewind) and `ops::Act::{OpenInspector, ChatMenu,
  OpenSettings, OpenGoal, OpenActivity, OpenChrono, OpenBtw, OpenAccounts,
  OpenSystemPrompt, OpenFind, OpenUsage, OpenTokens, OpenConnection, …}`
  (D's commands open E's panels). For commands that act directly:
  `ops::Act::Goal(goal::Act::Set(objective, max_ms))` (`/goal`),
  `sheets::chrono::parse(text, now_ms)` and `ops::Act::Chrono(..)`
  (`/chrono`), `model.ask_aside(conv, query)` (`/btw`),
  `ops::Act::{PickModel, PickEffort, PickSpeed}` (`/model`, `/effort`,
  `/fast`).
- **Events.** `Model::apply` settles the event's request, then every area
  sees it: `apply_list`, `apply_ops`, `apply_composer` by reference, last
  `apply_chat` by value (windows are big). An area ignores what is not its
  own.
- **No I/O in the model.** It asks the interface through `model.ui`:
  `ui.open(Box<dyn Overlay>)`, `ui.toast(text)`, `ui.error_toast(text)`,
  `ui.haptic(Haptic)`, `ui.effect(Effect::{Copy, OpenUrl, ShowKeyboard,
  HideKeyboard, Paste, Dictate, Files, StatusBar})`, and `close_all` /
  `close_top` / `close_scoped`. The event loop takes them after every
  change; tests read them. What a thread of the loop's finds comes back
  through the model (`dictation_line`, `files_done`).
- **Requests.** Issue every request whose answer matters through
  `model.requests.issue(owner, kind, conv, within, now)` (or `track` an id
  you made) with your `Owner` and a deadline (`requests::DEFAULT_WITHIN`
  20 s, `LOAD_WITHIN` 15 s, `ABORT_WITHIN` 12 s, …). An answer with that
  `reqId` settles it; an `error` for it goes to your `*_error(id, message)`
  (return false to get the generic error toast); a deadline passing calls
  your `*_timeout(id, &pending)`, which says what failed and what to do
  (`generic_timeout()` is "Exocortex did not answer. Try again."). A late
  answer finds nothing pending: drop it unless it is still useful.
  Disconnecting clears everything.
- **Readiness.** `model.ready` is true once the daemon has answered the
  bootstrap's `ping`; commands that change things wait for it (Send does
  nothing until then; D shows why). Stop (`abort`) needs only
  `connected`.
- **Kept state** (`model.persist`, written by `store.rs` 500 ms after a
  change, atomically, never over a file it could not parse; problems are
  toasted at start). `settings.json` is `persist.settings()` /
  `persist.update_settings(|s| …)` (`store::Settings`: theme, text scale,
  thinking and tool disclosure, haptics; unknown keys kept); `ui.json`
  sections `list` (B), `chat` (C2) and `generation` (E: each folder draft's
  explicit model overrides) through `persist.section(UI, key)` /
  `set_section`; `drafts.json` (D: `{"drafts": {key: {text, updatedAt}}}`,
  keyed by conversation id or `draft:<folder>`, written 300 ms after typing
  pauses, `drafts.rs`) through `persist.section(DRAFTS, "drafts")`. After changing
  the theme or text size call `theme::set_current(theme::by_id(id))` /
  `theme::set_text_scale(s)`: the generation changes, every layout cache is
  dropped and the next frame is drawn whole. Text size (E, Settings) has
  the iPhone's first six steps, `ops::TEXT_SIZES` (Extra small 0.82 to XXL
  1.36; its Accessibility sizes are left out, the bars keep their
  heights); the transcript's body text is `T_BODY` (`markdown_layout::body()`),
  so it follows too.

### View: pages, frames and pieces

- **Pages.** `view::draw_page`, `view::hit`, `view::animate`,
  `view::liftable` and `view::long_press` dispatch to each area: the list
  (`list::{draw_list, list_frame, draw_list_update, hit, liftable,
  long_press, animate}`), the chat (the transcript, then
  `chat_header::{draw, hit, frame_key, animate}` and `composer::{draw, hit,
  frame_key, animate, geometry}`), Settings (`settings::{draw, hit,
  frame_key}`). An area's `frame_key` hashes everything its pixels depend on
  (with `theme::generation()`); when it changes, its rows are drawn again.
  Its `animate` asks the `Animator` for the frames its motion needs
  (`frame(now)`, `slow_frame(now)` for a 30 Hz pulse, `follow(&tween, now)`,
  `request(at)`); nothing animating means no frames.
- **Pieces** (`view/ops.rs`). A transcript piece is a `Laid` (height,
  `Op`s, hits, ink rows, scroller keys, how far its scrollers go, the image
  previews it shows) relative to its top, passed through `ops::quantized`.
  Ops: `Text` (with tracking), `Rect`, `Border`, `Line`, `Icon`, `Image` (a
  weak `ImageHandle`: the image cache decides what stays in memory),
  `Chevron`, and `Clip { rect, xscroll, yscroll, ops }`, whose content the
  view scrolls sideways (code blocks, wide tables) or up (a tall table's
  rows) by `ViewState::xscroll[key]`, clamped to the piece's `xmax` /
  `ymax`; a frame redraws only the pieces whose scrollers moved. Hits are
  checked in x and y. C1's entry points are `markdown_layout::lay(source,
  x, width, top, MdStyle) -> Laid` and `lay_block(block, x, width, rich)`
  (one markdown block; its cache key must include `state_key(block,
  rich)`, which says whether a code block wraps), and `media::lay_strip`
  for a message's images (keyed by `Strip::state_key`); C2's is the
  transcript's planning in `transcript.rs` and `blocks.rs`, whose layout key
  includes `model.rich.version()`. Pieces are cached by a hash of what they
  show, so a piece's key must include everything it draws.
- **The rules every drawing follows** (they keep partial frames equal to
  full ones, which `EXOCORTEX_APP_VERIFY=1` and the tests check):
  draw only inside the canvas's clip, and nest clips with
  `widgets::clipped` (a band redraw clips the whole frame); skip work with
  `widgets::shows(canvas, y0, y1)`; place things at whole pixels (`ops::px`,
  snapped piece positions) and draw shapes through icons or the ops, so a
  piece moved by a scroll draws the same pixels; animate in quantized steps
  (`anim::pulse_percent`, alpha in 1/32), so two frames of one state agree.
- **Overlays** (`ui/overlay.rs`). An `Overlay` lays itself out, describes
  its `Body` each frame (rows, the solid rows that only move, a scroller,
  a key of everything else) and draws it; the pipeline then moves rows for
  a slide or a scroll and redraws only the rest. Modal overlays take every
  touch and dim the page; floating ones (toast, snackbar) pass touches that
  miss them. `scoped()` overlays close when the conversation changes.
  Ready-made: `sheet::Sheet<C: SheetContent>` (implement `title`, `layout`,
  `draw`, `item_at`, `activate` → `Reply::{Action, Close, CloseWith, …}`,
  `key`), `sheet::action_sheet(ActionList::new(title, items))` with
  `ActionItem::new(label, action).icon(..).detail(..).tone(..)`,
  `sheet::prompt_sheet(Prompt::new(..))` (one line),
  `textarea::sheet(TextArea::new(title, message, text, confirm, submit))`
  (several lines on Moon's multiline keyboard, Return a line break, grows
  to seven lines then scrolls; `.allow_empty()` for a text that may be
  cleared), `menu::AnchoredMenu::new(anchor, items, cut_out)`,
  `menu::Confirm::new(title, message, confirm, action)`,
  `toast::Snackbar::new(text, button, action, lift)`, `ui.toast(..)`,
  `reader::open(&mut ui, title, text)`. An item's action closes its menu or
  sheet first. A `SheetContent` can say when it next changes by itself
  (`wake`, a ticking time) and what to do once it has gone (`farewell`: the
  stack collects them, the event loop performs them; the Accounts panel
  stops watching so). A floating overlay that belongs to the page
  (`page_level()`: E's chips) is hidden under a modal one and takes no
  touches then. E's panels are `sheets::panel`: rows (sections,
  key/value, navigation, task rows, rail cards, bars, buttons) built from
  the model each frame (`Panel::{Goal, Activity, Chrono, Btw, SystemPrompt,
  Usage, Accounts, Connection, Tokens, Find}`), opened with
  `model.open_panel(..)`; Settings shows panel rows too.
- **Widgets and icons.** `ui::widgets` draws the shared chrome: `top_bar`,
  `page_rail`, `icon_button`, `ellipsis`, `button(Button { label, kind,
  icon, pressed, disabled })`, `segmented`, `switch`, `spinner`,
  `activity_dot`, `badge`, `count_text`, `eyebrow`, `hairline`, `pill`,
  `shadow`, `empty_state`, and text in a type token (`text`, `text_right`,
  `text_centered`, `fit`, `measure`, `wrap_plain`). `ui::icons::draw(canvas,
  Icon::X, cx, cy, size, color, alpha)` or `draw_styled` (stroke width,
  knockout) draws one of the 63 icons of §3.6.20, cached as masks.
- **Theme.** Colours come from `theme::p()` (the palette) and `theme::c()`
  (composites mixed from it), never literals; text from the type tokens
  (`T_ROW`, `T_TITLE`, `T_EYEBROW`, `T_BODY`, `T_CODE`, …: size, line,
  weight, tracking, uppercase, scaled by the text size); spacing from the
  metrics (`S1`…`S5`, `TOP_BAR`, `TARGET`, `GESTURE_RESERVE`, …).
- **Gestures.** `gesture::Recognizer` turns pointer events into `Down`,
  `Lift` (150 ms held), `LongPress` (600 ms), `DragStart { axis, edge }`,
  `Drag`, `DragEnd { vx, vy }`, `Tap`, `Back` (a swipe from the left 60 px
  over 30% of the width, or a flick) and `Cancel`; a move past 32 px makes
  it a drag and cancels the press, and the axis is chosen once. The top
  overlay gets gestures first.
- **What runs where** (E, `model/ops.rs`). `model.resolve()` is what the
  open conversation (its list summary, else its window) or the draft runs
  with: provider, model, effort, speed, and whether a draft only inherits
  the daemon's defaults; `model.selection()` is the same as a `Selection`,
  `model.generation_label(&r, effort)` its label ("Opus 5.5 · high ·
  fast", "Daemon default"). A draft (one per folder, `model.draft_key()`)
  keeps only explicit overrides, so `new_conversation` gets
  `model.new_conversation_fields(&mut command)`: nothing for a blank draft
  (the daemon's defaults apply), else what was chosen. `ops::Act::{PickModel,
  PickEffort, PickSpeed}` change the conversation now (`set_model`,
  `set_effort`, `set_fast_mode`) or the draft locally, for `/model` and
  the like.
- **Other interfaces.** `chat_header::subtitle(model)` (PAR-009: the
  connection, a failed load, a retry, a wait, Working, Syncing, else the
  model label; " · 1.0k ctx" after it), `Effect::Reveal(stamp)` (the chat
  scrolled to an entry, its top 8 pt under the header, following stopped:
  `ViewState::reveal`; a Find in chat match), `ui::haptic(Haptic)`, `ui::open_url(url)` (Moon's
  Browser, http and https only), `clipboard::copy(text)`,
  `clipboard::paste()`.

### Things to know

- `exocortex-notify` builds `markdown.rs`, `line.rs`, `handoff.rs`,
  `proto/` and `list/activity.rs` into itself by path: they may use only
  each other and outside crates, and the bridge matches `MdBlock`
  exhaustively (a new block kind needs a line there).
- `proto/chat.rs` is shared by C1 (images, file links, display blocks) and
  C2; extend structs with defaulted fields, never rename.
- JetBrains Mono NL in five cuts (Regular, Medium, SemiBold, Bold, Italic)
  is the app's only family (Inter stays for the other apps). The fonts are
  subset (`scripts/subset-fonts.sh`): a character
  outside the subset falls back to Moon's fonts.
- Moon draws its status bar in the colour the app asks for
  (`ui::status_bar(&theme::status_bar())`, the theme's background, at start
  and whenever the window is mapped); a theme switch asks again
  (`Effect::StatusBar`).
- Where the iPhone screenshots and the spec's numbers disagree, the
  screenshots won: the search field spans x 44 to 692, the chat title's
  baseline is at 47, the ⋯ dots are 6.5 px, 15 px apart.

## Notifications: `exocortex-notify`

The bridge connects to the daemon with `sidebar-state-patch` and
`sidebar-incremental`, whose bootstrap with a `sidebarView` of no rows sends a
first page holding only the conversations showing activity (which is how it
learns what streams), and the rest of the list, which it skips unparsed.
After that it only hears the broadcasts every client gets, and looks at no
event but `conversation_updated`, `conversation_deleted`,
`conversation_restored`, `conversation_unwound`, `sidebar_state_patched`,
`queue_updated`, the list and its own replies. It never subscribes: the
daemon marks a conversation unread only when no client is subscribed.

It posts to `moon-notifyd` (`MOON_NOTIFY_SOCKET`, else
`/run/moon-notify/notify.sock`; OSS-Phone-V1 `docs/notifications-protocol.md`):

- **When**: a top-level conversation's `conversation_updated` with
  `ownerTurnFinished` (a turn chain the owner started ended, rather than being
  stopped; no `streamStopReason`), not streaming, not `notificationsMuted`;
  or a conversation's goal changing to blocked, paused by its controller, or
  complete. Subagents (`parentConversationId`) never notify.
- **Not** while the app shows that conversation in front (below), and at
  most once per conversation in 5 s.
- **What**: app `exocortex`, tag = the conversation id (or a hash of it when
  it is not a valid tag), title = its title, body = the start of the reply
  (its text without markdown, 280 characters, or the error that ended the
  turn), `open: {app: "exocortex", target: <conversation id>}`. To read the
  reply it asks for the newest history page (`load_conversation_history`),
  which neither subscribes nor marks it read.
- **Withdrawn** when the conversation is read anywhere (`unread` true to
  false: the app, the TUI) or deleted; the app also withdraws it when it
  opens the conversation, and Moon removes a tapped one.

moon-notifyd may be absent (Android mode, a restart): posts wait, the newest
per conversation, for up to 12 h, and the bridge reconnects with backoff
(1 s to 60 s). The daemon may restart: the bridge reconnects (0.5 s to 30 s)
and starts over. It sleeps in `poll(2)` between events (no timer runs while
nothing streams and nothing waits), so it costs nothing while idle, and
across Moon's screen-off and deep suspend (a warm reboot) it simply starts
again with the session.

### The icon's badge

The bridge keeps the unread count on the Exocortex icon (Moon's `badge
exocortex N`, OSS-Phone-V1 `docs/app-services.md`; 0 removes it): the
conversations the list shows with a green dot, as the iPhone's badge counts
them (`list/activity.rs`, built into the bridge by path: unread, not muted
by itself, a folder above or `notificationsMuted`, no model work, neither
streaming nor waiting on its provider, a long sleep or the global-idle
queue; subagents count like any conversation). The first page holds every
folder and every unread conversation, and `unread` changes only with a
whole summary, so it keeps just the unread summaries, the folders and the
queue. It sets the count once the first page has come and whenever it
changes (about 30 ms after the daemon's event, measured in a sandbox),
including while the app is closed or frozen; while Moon is absent or
refuses it tries again after 5 s, 15 s, 1 min, 5 min, then every 15 min.
Moon keeps the badge across its restarts and reboots, and the bridge keeps
it while the daemon is away. The app does not set it: one writer, always
running.

### Keeping the phone awake for a turn

With the screen off Moon deep-suspends unless a lease is held. While any
conversation streams (subagents included), the bridge holds Moon's `transfer`
lease for `exocortex` through Moon's control socket (`A26_SHELL_SOCKET`, else
`/run/a26-shell/control.sock`): `lease acquire exocortex transfer 120`,
renewed every 60 s, `lease release exocortex transfer` when nothing streams.
While Moon is absent it asks again every 15 s. In Moon, exocortex leases are
external: granted whether or not the app runs, they only hold off deep
suspend; the app's own process still freezes in the background.

### The hand-off between the app and the bridge

They never talk to each other. The app writes which conversation it shows to
`/run/exocortex-app/view.json` (`EXOCORTEX_APP_STATE`), atomically, whenever
that changes:

```json
{"pid": 4242, "conversation": "1793171213972-a1b2c3"}
```

`conversation` is null on the list. Before notifying for a conversation, the
bridge reads the file and asks Moon (`state`): it stays quiet only when the
file names that conversation, its pid is alive, and Moon's `view` is
`exocortex` on an awake, unlocked screen. A frozen app's stale file is
harmless, since Moon then shows another view; when Moon cannot be asked
(Android mode) the app is not in front.

### Opening a notification

Moon handles a tapped notification whose `open.app` is `exocortex` like the
Phone and Messages apps': it records `open_target` (`app`, `target`, a
`serial`) in its state and brings the app forward, launching it if it is not
running. The app reads Moon's state on start, whenever its window is mapped,
and every second in front, and opens a target it has not seen that is less
than 10 s old. If the daemon is not connected yet, the conversation opens as
soon as it is.

## Environment

| Variable | Default | Used by |
| --- | --- | --- |
| `EXOCORTEX_SOCKET` | `/opt/exocortex/config/runtime/exocortexd.sock` | both |
| `EXOCORTEX_APP_STATE` | `/run/exocortex-app/view.json` | both |
| `A26_SHELL_SOCKET` | `/run/a26-shell/control.sock` | both (Moon's state, keyboard, transitions, leases, the icon's badge) |
| `MOON_NOTIFY_SOCKET` | `/run/moon-notify/notify.sock` | both |
| `MOON_FONTS` | `/opt/a26-shell/share/fonts` | the app (fallback glyphs) |
| `EXOCORTEX_APP_PROFILE` | unset | the app: logs each frame's draw and present time, and whether it was full or partial; when the list first shows, how long an open took to draw, event batches over 2 ms, and coming back to the front |
| `EXOCORTEX_APP_NO_BOOST` | unset | the app: no utilization clamp for its UI thread |
| `EXOCORTEX_APP_VERIFY` | unset | the app: checks each partial frame against a full redraw (slow; development) |
| `EXOCORTEX_APP_DATA` | `/var/lib/exocortex-app` | the app: its settings, UI state and drafts (`store.rs`) |

The bridge logs connections and the conversations it notifies for (never
their text) to stderr.

## Developing off the phone

On a Linux host with an Xvfb: run a test daemon (exocortex
`scripts/dev/testd start <worktree>`), then Moon's preview (OSS-Phone-V1
`scripts/a26-shell/preview.sh`) with `A26_SYSTEM_APP` pointing at a host
build of this crate and `A26_EXOCORTEX_ICON` at `assets/exocortex-app.bgrx`,
a host `moon-notifyd` (`MOON_NOTIFY_ROOT` puts its socket under a directory),
and `exocortex-notify`, all with the environment above. Never point them at a
daemon you did not start for the test.
