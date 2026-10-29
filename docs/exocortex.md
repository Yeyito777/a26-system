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
`sidebar-state-patch`, `history-block-pagination`, `token-stats-on-demand`
and `sidebar-incremental` (with a tree `sidebarView` naming the folders open
on the list), then `ping`s.
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
| The queue | a queued bubble tapped: Edit in composer, Copy text, Remove from queue (`unqueue_message`; the bubble says "Removing…" until the daemon's `queue_updated` settles it) |
| Stop | `abort` with the stream's `expectedStartedAt`; a spinner until `ack`, `streaming_stopped` or `message_complete`; a second press says it is already stopping |
| The model inspector | `tools_available`'s providers, models, efforts and speeds, and `conversationDefaults` (or `conversation_defaults`); Apply sends `set_model`, `set_effort`, `set_fast_mode` for what changed; a draft changes nothing on the wire until its first message |
| A tool row opened | `load_tool_outputs` for that call (history leaves outputs out) |

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
folder or deletes them. Pulling the list down past the top asks for it
again.

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

Tool calls show as one row each (label as the TUI styles it, summary, ERR
when it failed); a run of three or more folds into one row ("7 tool calls ·
$, Read"). Thinking shows as a folded row. Replies are markdown
(`src/exocortex/markdown.rs`): headings, paragraphs (a newline is a line
break), bullet, numbered and task lists, quotes, rules, fenced code and
tables, with bold, italic, code and link spans. Automated prompts (subagent
reports, notifications) are one gray line.

The composer is a text area on Moon's `keyboard show multiline` (Return is
a line break; only the Send button sends). Its editor (`editor.rs`) keeps a
caret and a selection by cluster (an emoji sequence is one step) and wraps
the text exactly as it draws it, so a tap places the caret at the glyph
under the finger; a long press selects a word (moving the finger extends
it), the handles and the caret can be dragged, and a tap at the caret or on
the selection opens the edit menu (Cut, Copy, Paste, Select, Select all over
Moon's clipboard). Six lines show; more scroll inside, the caret's line kept
in view. Drafts are kept per conversation and per new chat in a folder, in
`drafts.json`, across restarts.

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
  that changes only when the entry does: a window that resends an entry
  unchanged keeps its stamp. The view caches an entry's plan by stamp.
- **Pieces.** The transcript is laid out as pieces: a markdown block, a tool
  or thinking row (an opened run of tool calls is a row per call), a bubble,
  a notice. Each is cached by a hash of what it shows (`ViewState::pieces`),
  so a streamed chunk lays out only the paragraph that grew, and a reply that
  is committed or settles on the daemon's copy reuses the live reply's pieces.
  A piece knows the rows its glyphs reach (`Laid::ink`); only those on screen
  are visited. The reader's place, when not following the newest message, is
  a piece (entry stamp and index) and its distance from the top; it is kept
  when anything changes above it (older history, a fresh window).
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
| `view/markdown_layout.rs`, `markdown.rs`, `reader.rs` | C1 | markdown as pieces, code, tables; the reader (A's is minimal) |
| `view/transcript.rs`, `view/blocks.rs`, `view/tools.rs`, `model/chat.rs`, `proto/chat.rs` | C2 | the transcript, streaming, history |
| `view/composer.rs`, `model/composer.rs`, `proto/composer.rs`, `editor.rs`, `drafts.rs` | D | the composer and its editor (caret, selection, wrap, the edit menu), sending, the queue, kept drafts |
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
  `composer::Act::{Insert(String), RestoreDraft { conv, text }, Focus}`
  (C2's message menu, D's rewind) and `ops::Act::{OpenInspector, ChatMenu,
  OpenSettings, OpenGoal, OpenActivity, OpenChrono, OpenBtw, OpenAccounts,
  OpenSystemPrompt, OpenFind, …}` (D's commands open E's panels; the
  unbuilt ones toast "Coming soon.").
- **Events.** `Model::apply` settles the event's request, then every area
  sees it: `apply_list`, `apply_ops`, `apply_composer` by reference, last
  `apply_chat` by value (windows are big). An area ignores what is not its
  own.
- **No I/O in the model.** It asks the interface through `model.ui`:
  `ui.open(Box<dyn Overlay>)`, `ui.toast(text)`, `ui.error_toast(text)`,
  `ui.haptic(Haptic)`, `ui.effect(Effect::{Copy, OpenUrl, ShowKeyboard,
  HideKeyboard})`, and `close_all` / `close_top` / `close_scoped`. The event
  loop takes them after every change; tests read them.
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
  dropped and the next frame is drawn whole.

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
  `Op`s, hits, ink rows, horizontal scroller keys) relative to its top,
  passed through `ops::quantized`. Ops: `Text` (with tracking), `Rect`,
  `Border`, `Line`, `Icon`, `Image` (an `ImageHandle`), `Chevron`, and
  `Clip { rect, xscroll: Some(key), ops }`, whose content the view scrolls
  sideways by `ViewState::xscroll[key]` (code blocks, tables: C1 clamps).
  C1's entry point is `markdown_layout::lay(source, x, width, top, MdStyle)
  -> Laid`; C2's is the transcript's planning in `transcript.rs` and
  `blocks.rs`. Pieces are cached by a hash of what they show, so a piece's
  key must include everything it draws.
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
  `sheet::prompt_sheet(Prompt::new(..))`, `menu::AnchoredMenu::new(anchor, items, cut_out)`,
  `menu::Confirm::new(title, message, confirm, action)`,
  `toast::Snackbar::new(text, button, action, lift)`, `ui.toast(..)`,
  `reader::open(&mut ui, title, text)`. An item's action closes its menu or
  sheet first.
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
  model label; " · 1.0k ctx" after it), `ui::haptic(Haptic)`, `ui::open_url(url)` (Moon's
  Browser, http and https only), `clipboard::copy(text)`,
  `clipboard::paste()`.

### Things to know

- `exocortex-notify` builds `markdown.rs`, `line.rs`, `handoff.rs` and
  `proto/` into itself by path: they may use only each other and outside
  crates, and the bridge matches `MdBlock` exhaustively (a new block kind
  needs a line there).
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
event but `conversation_updated`, `conversation_deleted`, the list and its
own replies. It never subscribes: the daemon marks a conversation unread
only when no client is subscribed.

It posts to `moon-notifyd` (`MOON_NOTIFY_SOCKET`, else
`/run/moon-notify/notify.sock`; OSS-Phone-V1 `docs/notifications-protocol.md`):

- **When**: a top-level conversation's `conversation_updated` with
  `ownerTurnFinished` (a turn chain the owner started ended, rather than being
  stopped; no `streamStopReason`), not streaming, not `notificationsMuted`;
  or a conversation's goal changing to blocked, paused by its controller, or
  complete. Subagents (`parentConversationId`) never notify.
- **Not** while the app shows that conversation in front (below).
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
| `A26_SHELL_SOCKET` | `/run/a26-shell/control.sock` | both (Moon's state, keyboard, transitions, leases) |
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
