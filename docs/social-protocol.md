# Moon's social bridges: protocol, version 1

The Instagram, WhatsApp and Discord apps (`a26-system --app instagram`,
`--app whatsapp`, `--app discord`) never talk to the services themselves.
Each service has a **bridge**: an always-running process in the chroot that
holds the account's session, talks to the service exactly as
[record](https://github.com/Yeyito777/record) does (the same requests,
headers and identities; record's own code), keeps the chats it knows on disk,
posts the service's notifications to Moon and keeps its icon's badge. The app
is a client of its bridge's socket, as the Exocortex app is of the daemon's.

A bridge is record's headless mode: `record service --provider NAME`
(record `src/service/`). Moon's boot scripts keep one running per signed-in
service (OSS-Phone-V1 `scripts/device/moon-social-start.sh`).

This document is the contract between a bridge and its app. Anything not
written here is not part of the protocol.

## 1. Transport

| | |
| --- | --- |
| Socket | `/run/moon-social/SERVICE.sock` (`instagram`, `whatsapp`, `discord`); Unix stream, inside the chroot |
| Override | `MOON_SOCIAL_SOCKET` (the app), `--socket` (the bridge): sandboxes and tests |
| Directory | `/run/moon-social`, root, `0700`; socket `0600` (apps run as root) |
| Framing | UTF-8 JSON, **one object per line** (`\n`); a request line is at most 1 MiB, an answer or event line at most 8 MiB |
| Connections | any number, long-lived; requests on one connection are answered in order |

A bridge never waits on an app. Each connection has an output buffer of at
most 16 MiB; a client that lets it fill (a frozen app) is disconnected and
reconnects when it runs again. An app must never block its UI on the
socket: it talks to the bridge from a thread (`src/social/link.rs`).

### Requests and answers

```json
{"id":7,"cmd":"history","chat":"wa:MTIzQHMud2hhdHNhcHAubmV0","limit":40}
{"id":7,"ok":true,"result":{"messages":[...],"more":true}}
{"id":8,"ok":false,"error":{"code":"read_only","message":"writes are off"}}
```

- `id`: a non-zero integer chosen by the client, echoed in the answer.
- `cmd`: the command (§3, §5). Other fields are its arguments. Unknown
  fields are ignored.
- Errors: `invalid_request` (not JSON, no `id`/`cmd`), `unknown_command`,
  `invalid_argument`, `not_found`, `signed_out` (no session),
  `unavailable` (the service cannot be reached now; retry later),
  `rate_limited` (with `retry_after_ms` when known), `challenge` (the
  service wants the user: Instagram's checkpoint, a captcha), `read_only`
  (§4), `uncertain` (a write whose outcome is unknown: never resend it on
  your own; refresh first), `internal`. `message` is for logs and plain
  error lines, never secret.

### Events

After `{"id":1,"cmd":"subscribe"}` the connection also receives events,
interleaved with answers. Events carry `event` and never `id`:

```json
{"event":"message","message":{...}}
```

Unknown events and unknown fields must be ignored (later versions add them;
they never change existing ones).

## 2. Objects

Times are **Unix milliseconds** (integers). IDs are opaque strings; never
parse them. Text is plain UTF-8 as the service holds it (Discord's
markdown, WhatsApp's `*bold*`, Instagram's plain text): `format` says which.

### Status

```json
{"state":"online","detail":null,"writes":false,"account":{...},"service":"whatsapp"}
```

| `state` | |
| --- | --- |
| `signed_out` | no session; `login` (§3.6) |
| `connecting` | starting, or reconnecting after a drop |
| `online` | live |
| `offline` | the network or service is unreachable; it retries by itself (`detail` says why, in words) |
| `error` | needs the user (`challenge`, a revoked session); `detail` says what |

`writes` is §4's switch. `account` is the signed-in **User** (or null).

### User

```json
{"id":"1234","name":"Ana Ruiz","username":"ana.ruiz","avatar":Media|null}
```

### Media

What a message, chat or user shows. `ref` is opaque: it is handed back to
`media.fetch` (§3.5), which downloads it into the bridge's cache.

```json
{"ref":"i:4f2c…","kind":"image","w":1080,"h":1350,"mime":"image/jpeg",
 "name":null,"size":null,"duration_ms":null,"thumb":"i:9a1b…","blur":null}
```

`kind`: `image`, `video`, `gif`, `sticker`, `audio` (a voice note: `duration_ms`),
`file`. `w`/`h` when known (draw the box before it loads); `thumb` a smaller
picture of it (a video's poster); `blur` a tiny inline JPEG (base64) to show
at once, when the service gives one (WhatsApp).

### Chat

```json
{"id":"wa:MTIz…","kind":"dm","title":"Ana Ruiz","avatar":Media|null,
 "last":{"id":"3EB0…","ts":1793001234000,"author":"Ana","text":"see you","mine":false},
 "ts":1793001234000,"unread":2,"mentions":0,"muted":false,"pinned":false,
 "archived":false,"parent":null,"members":null,"ext":{}}
```

`kind`: `dm`, `group`, `channel`, `thread`, `forum`, `voice`. `ts` is the
last activity (the list is newest first, pinned first). `unread` counts
messages (WhatsApp, Discord DMs), or is 1 for a chat known to be unread
without a count. `parent` places a chat under another (a Discord channel's
server and category, a thread's channel). `ext` holds what only one service
has (§5).

### Message

```json
{"id":"3EB0…","chat":"wa:MTIz…","ts":1793001234000,
 "author":{"id":"…","name":"Ana Ruiz","avatar":null},"mine":false,
 "text":"see you *there*","format":"whatsapp","media":[Media],
 "reply_to":{"id":"…","author":"Leo","text":"where?"},
 "reactions":[{"emoji":"❤️","count":2,"mine":true}],
 "edited_ts":null,"deleted":false,"system":false,"forwarded":false,
 "status":"read","client_id":null,"ext":{}}
```

`status` (mine only): `pending`, `sent`, `delivered`, `read`, `failed`.
`system`: a notice row (joined, changed the subject, a call). A message is
replaced whole by a later one with the same `id` (an edit, a reaction, a
receipt): apps upsert by `id`. A sent message's echo carries the request's
`client_id` so the pending bubble becomes it.

## 3. Commands

### 3.1 Session

| `cmd` | Arguments | Result |
| --- | --- | --- |
| `hello` | `client` (a name), `protocol` (1) | `{"protocol":1,"service":"whatsapp","status":Status,"capabilities":[…]}` |
| `subscribe` | | `{}`; then events (§3.7) |
| `status` | | Status |
| `settings.get` | | `{"writes":false,"notifications":true,"previews":true}` |
| `settings.set` | any of those fields | the settings. Only the user's own action in the app sets `writes`; tests never do |

`capabilities` names what the bridge supports beyond this core (the §5
extensions, `send.media`, `edit`, `typing`, …).

### 3.2 The chat list

| `cmd` | Arguments | Result |
| --- | --- | --- |
| `chats.list` | optional `cursor`, `limit` (default 100), `folder` (`inbox`, `requests`, `archived`) | `{"chats":[Chat],"cursor":null\|"…"}` newest first; `cursor` for the next page |
| `chat.get` | `chat` | Chat |

The first page comes from the bridge's disk cache at once and is then kept
current by `chat` events, so an app draws its list in its first frame.

### 3.3 A chat

| `cmd` | Arguments | Result |
| --- | --- | --- |
| `history` | `chat`, optional `before` (a message id), `limit` (default 40, at most 100) | `{"messages":[Message],"more":true}`, oldest first, ending at the newest (or just before `before`) |
| `view` | `chat` (or null: the list), `front` (the app is in front) | `{}` |

`view` tells the bridge what the user is looking at: it does not notify for
that chat while the app is in front on an unlocked screen, it withdraws the
chat's notification, it polls that chat more often where the service needs
polling, and (only with writes on) it marks it read as the service's own
app does. With writes off the chat is marked read **locally**: its unread
count and the badge clear, nothing is sent.

### 3.4 Writes

Every command here, and every bridge action that would be visible to
someone else or change the account (read receipts, typing, presence, joins,
calls), is refused with `read_only` while writes are off (§4).

| `cmd` | Arguments | Result |
| --- | --- | --- |
| `send` | `chat`, `text`, optional `reply_to`, `client_id`, `media` (local file paths, `send.media`) | `{"message":Message}` (status `pending` or `sent`) |
| `react` | `chat`, `message`, `emoji`, optional `remove` | `{}` |
| `edit` | `chat`, `message`, `text` | `{"message":Message}` |
| `delete` | `chat`, `message` | `{}` |
| `mark_read` | `chat`, optional `message` | `{}` |
| `typing` | `chat` | `{}` |

### 3.5 Media

| `cmd` | Arguments | Result |
| --- | --- | --- |
| `media.fetch` | `ref`, optional `max` (the longest side the app will draw, px) | `{"path":"/var/cache/moon-social/whatsapp/media/…","mime":"image/jpeg","w":1080,"h":1350,"bytes":183422}` |

The bridge picks the smallest variant at least `max` wide (services offer
several), downloads it once into its cache (`/var/cache/moon-social/SERVICE/`,
root, `0600` files, oldest evicted past 512 MiB) and answers with the
file: the app reads and decodes it itself, at its draw size, off its UI
thread (`src/social/media.rs`). Concurrent fetches of one file share one
download.

### 3.6 Signing in

| `cmd` | Arguments | Result |
| --- | --- | --- |
| `login.start` | `method` (the service's, §5) and its fields | `{}`; progress in `login` events |
| `login.cancel` | | `{}` |
| `logout` | | `{}`; status `signed_out` |

`login` events: `{"event":"login","state":"qr","qr":{"size":33,"rows":["0101…",…]},"expires_ms":…}`
(a QR code as rows of modules, `1` dark, for the app to draw),
`"state":"code"` (`code`: a pairing code to type on another device),
`"state":"waiting"`, `"state":"done"`, `"state":"failed"` (`detail`).

### 3.7 Events

| `event` | Fields | |
| --- | --- | --- |
| `status` | `status` | the Status changed |
| `chat` | `chat` | a chat is new or changed (upsert by id) |
| `chat_removed` | `id` | gone (left, deleted, archived away) |
| `message` | `message`, `live` | new or changed (upsert by id); `live` true for one that just arrived, false for history being filled in |
| `message_removed` | `chat`, `id` | deleted for everyone |
| `typing` | `chat`, `user` (User), `until` | someone types, until then |
| `read` | `chat`, `user` (null: the account itself, elsewhere), `message` | read up to that message |
| `login` | see §3.6 | |

## 4. Writes are off until the user turns them on

A bridge starts with writes off, and keeps the switch in its state directory
(`/var/lib/moon-social/SERVICE/settings.json`). While off it sends nothing a
person could see and changes nothing on the account: no messages, reactions,
edits, deletions, read receipts or acks, typing, presence ("online"), joins
or calls. It still reads. The apps show the switch in their settings, and
the bridges' own tests and the apps' sandbox runs never turn it on.
`--fixtures` (§6) runs a bridge on made-up data with no account, where
writes act on the fixtures.

## 5. Per-service extensions

Commands named `SERVICE.…`, events `SERVICE.…`, and the `ext` fields. Each
service's section is written by its bridge's provider.

### Discord

(to come: servers, channels, threads, members)

### Instagram

(to come: feed, stories, profiles, posts, comments, search, activity)

### WhatsApp

The bridge is one more of the account's linked devices, as WhatsApp Web
is: record's own Baileys code (`src/whatsapp/`: its browser identity, web
version, history settings and auth store), run in the bridge's process
(record `src/service/providers/whatsapp.ts`). One setting differs from
record's terminal client: it never marks itself online, so the account
never shows "online" from it and the primary phone keeps its pushes; its
delivery receipts are WhatsApp Web's background ("inactive") ones. Read
receipts, presence and typing are never sent; `send`, `react` and
`mark_read` go through §4's switch.

**Signing in.** `login.start` with `method: "qr"` (the only one). `login`
events: `qr` (`qr.size`, `qr.rows`, `expires_ms`: 60 s for the first code,
20 s for each next one; a new event replaces the code), `waiting` once the
primary phone has scanned it, `done` when the device is linked (then
`status` goes `online` and the first history sync fills the list over the
next minutes), `failed` (`detail`). A login ends by itself after 5 minutes;
`login.cancel` ends it at once. `logout` unlinks this device from the
account and removes the session, the chats and their media. WhatsApp
unlinking the device (from the primary phone) does the same and leaves
`status` `signed_out` with a `detail`; another client resuming the same
session (two bridges on one state) leaves `error`.

**Ids.** Chat ids are `wa:` and the chat's jid in base64url (a phone number
appears in no log); a chat known first by its LID becomes its phone
number's when WhatsApp maps them (`chat_removed` for the old id, `chat` for
the merged one). User ids are `u:` and a base64url jid; the account's own
messages have author `{"id": "me", "name": "You"}`. Message ids are
WhatsApp's.

**Names.** A chat's `title` is the contact's name as the primary phone
saved it, else a business's verified name, else the number as WhatsApp
shows it (`+1 555-010-1234`), else the person's own push name; a group's is
its subject. `ext.push_name` (chats and messages) is the person's own name
when the account has not saved them: show it as WhatsApp does (`~ Ana`).

**Chats.** `unread` is WhatsApp's count (kept as the primary phone reads and
marks chats); `ext.marked_unread` a chat marked unread there (`unread` 1).
`members` is a group's size once known. `last.text` is a summary (`📷
Photo`, `🎤 Voice message (0:42)`, a notice's sentence); `ext.status` is the
last message's receipt when it is the account's, `ext.deleted` when it was
deleted, `ext.notice` when it is a notice. Archived chats are in the
`archived` folder. Status updates (below) and channels are not chats.

**Messages.** `format` is `whatsapp` (`*bold*`, `_italic_`, `~strike~`,
```` ```mono``` ````). Media: `image`, `video`, `gif` (a looping silent
video), `sticker` (WebP), `audio` (`ext.voice` for a voice note,
`duration_ms`), `file` (`name`, `size`). `thumb` is the small preview
WhatsApp carries in the message (a video's poster): `media.fetch` it like
any picture; the full file needs the connection (`unavailable` without
it). A reply's `reply_to.text` is the quoted message's summary. Deleted
messages stay as `deleted` with no text and no media; group changes and
missed calls are `system` notices whose `text` is the sentence (with the
names known now). `status` (the account's messages): `pending`, `sent`,
`delivered`, `read` (or played), `failed`. `ext.view_once` marks a view-once
message (its media is not shown).

**History.** `history` answers from what the bridge has kept (what WhatsApp
synced when the device was linked, and everything since); past its oldest
message it asks the primary phone for the page before it (50 messages) and
answers when that comes, within 12 s. The phone must be online for that:
otherwise the answer is what there is, `more` false, and a later request
asks again.

**Pictures.** A chat's `avatar` is its profile picture (preview size),
fetched through the session one at a time when the app first draws it and
kept a week; a chat found to have none gets `avatar: null` (asked again in
three days).

**Status updates.** `whatsapp.status.list`: `{"updates": [{"user": User,
"ts", "seen", "items": [Message]}]}`, the last 24 hours' updates by who
posted them (the account's own first, as `"me"`, then those with one not
seen yet, newest first); an item's `ext.seen`, and `ext.background`
(`#rrggbb`) for a text update. `whatsapp.status.seen` (`ids`) notes items as
seen on this phone only: WhatsApp is told nothing, whatever §4's switch
says. A `whatsapp.status` event (`user`) says someone posted one now. Their
media come from `media.fetch` like a message's.

**Commands.** `whatsapp.chat_info` (`chat`): `title`, `phone` (a person's
number, as shown), `push_name`, `members`, `group`, `muted`, `messages` (how
many the bridge keeps). `whatsapp.chat_media` (`chat`, optional `before`,
`limit`): the chat's photos and videos the bridge keeps, newest first,
`{"messages": [Message], "more"}`. `media.fetch` of an `avatar` with `max`
over 200 gets the whole profile picture (the list's are previews). With `--fixtures`: `whatsapp.fixtures.poke`
(`chat`, `text`: someone writes now) and `whatsapp.fixtures.scan` (the
made-up QR is scanned), and `RECORD_WA_FIXTURES_SIGNED_OUT=1` starts signed
out, `RECORD_WA_FIXTURES_ANCHOR=ms` pins the made-up times.

**State.** `STATE/auth/` holds the linked device's keys (record's auth
store, `0600`), `STATE/wa/` what the bridge keeps (`meta.json`: contacts and
chats; `messages/`: a file per chat, up to 5,000 messages each). A second
bridge on the same state directory refuses to start.

## 6. Running a bridge

```sh
record service --provider whatsapp            # the phone's (sockets, state, cache as above)
record service --provider whatsapp --fixtures --socket /tmp/wa.sock --state /tmp/wa-state
```

| Path (chroot) | |
| --- | --- |
| `/opt/moon-social/record/` | record (its `src/` and production `node_modules`) |
| `/var/lib/moon-social/SERVICE/` | the session (`0600`), settings, the chat cache; root, `0700` |
| `/var/cache/moon-social/SERVICE/` | downloaded media |
| `/var/log/moon-social/SERVICE.log` | the bridge's log: never message text, tokens or cookies |

The bridge posts its notifications to `moon-notifyd` (`app` = the service,
`tag` per chat, `open` = `{"app":SERVICE,"target":CHAT_ID}`) and sets the
badge with Moon's `badge SERVICE N`. A tapped notification reaches the app
as Moon's `open_target`.
