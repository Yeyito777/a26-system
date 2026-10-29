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
`src/exocortex/proto.rs` parses the events they use leniently: unknown events
and fields are ignored, missing ones take defaults.

The app declares `targeted-unwind`, `sidebar-reorder-delta`,
`sidebar-state-patch`, `history-block-pagination`, `token-stats-on-demand`
and `sidebar-incremental` (with a navigate-view `sidebarView`), then `ping`s.
Its bootstrap, and every reconnection's, is that, plus `load_conversation`
for the conversation on screen in the same write.

| Screen | Daemon traffic |
| --- | --- |
| The list | `conversations_list` (first page, then the rest), kept current by `conversation_updated`, `conversation_deleted`, `sidebar_items_reordered`, `sidebar_state_patched`, `conversation_moved` |
| A conversation | `load_conversation` with `turns`, the `viewport` and an `initialBackfill` (which subscribes and marks it read): a window of about two screens, then the older page behind it from the same snapshot; older pages (15 turns) with `load_conversation_history` as the user scrolls up, joined where a page ends inside an entry; the stream (`streaming_started`, `block_start`, `text_chunk`, `thinking_chunk`, `streaming_sync`, `tool_call`, `tool_result`, `message_complete`, `streaming_stopped`, `user_message`, `system_message`, `history_updated`, whose `pendingAI: null` ends the live reply); after each stop the newest page again, so the history settles on the daemon's copy. Leaving it `unsubscribe`s. |
| Sending | `send_message` (shown at once, matched to the daemon's `user_message` by `startedAt`); while it works, `queue_message` with `message-end` (shown as Queued until delivered); a new conversation is `new_conversation` with its first message and the chosen provider, model and effort (in the folder the list showed) |
| Stop | `abort` with the stream's `expectedStartedAt` |
| Model sheet | `tools_available`'s signed-in providers and their models and efforts; `set_model`, `set_effort` |
| A tool row opened | `load_tool_outputs` for that call (history leaves outputs out) |

Tool calls show as one row each (label as the TUI styles it, summary, ✕ when
it failed); a run of three or more folds into one row ("7 tool calls · $,
Read"). Thinking shows as a folded row. Replies are markdown
(`src/exocortex/markdown.rs`): headings, paragraphs (a newline is a line
break), bullet, numbered and task lists, quotes, rules, fenced code and tables
(monospaced, JetBrains Mono), with bold, italic (Inter Italic), code and link
spans. Automated prompts (subagent reports, notifications) are one gray line.

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
  (`Model::shelve`). Reopening one shows it at once; the daemon's fresh window
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
- **A fast core.** While in front, the UI thread asks for a minimum
  utilization clamp (`sched_setattr`, `UTIL_CLAMP_MIN`), so energy-aware
  scheduling runs its short bursts on a fast core at a high clock: on a slow
  one a full chat frame took 25-35 ms, on a fast one 4 ms. Off when hidden.
- **Fonts** are parsed per face when first used (fontdue parses every glyph:
  about 6 ms a face on a fast core), the ones the first frame does not use on
  other threads at start.

On the A26 (sandboxed Moon, the user's real conversations): the longest
conversation (EXO, 790 KB of history) opened in 9.8 s before this and opens in
about 20 ms now (a reopened one draws in about 15 ms); its whole history lays
out in 61 ms, a re-layout after a change takes 0.2 ms; streaming frames draw
and present in 2.2 ms on average, scrolling frames in 2.2 ms (p95 3.5 ms).

Drawing is the System app's CPU canvas and `Presenter`. Layout is cached per
history entry, so a streaming reply lays out only itself, and every vertical
position is snapped to 1/16 px so that moving drawn pixels by whole rows gives
exactly what drawing them there would. A frame is drawn incrementally when
the last one is known (`view::ChatFrame`, `ListFrame`): a scroll moves the
transcript's (or list's) rows on the canvas and, with `Presenter::scroll`,
in the server's pixmap, then draws only the uncovered strip; a reply that
grows, a press, the working dots, the bar and the composer redraw only
their rows (from the first op of the reply that changed). The presenter then
compares and uploads only those rows (`Presenter::present_rows`); rows moved
on the server are marked stale rather than moved in its shadow too, and are
uploaded, not compared, when next drawn. Tests check each kind of
incremental frame against a full redraw, pixel for pixel, and that nothing
changed outside its damage. `EXOCORTEX_APP_VERIFY=1` makes the app check
every incremental frame against a full redraw at run time and log any
difference (development only: it costs a full draw per frame).

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
