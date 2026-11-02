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

Direct is the core: chats are threads (`dm`, `group`), `folder` `inbox` or
`requests` (message requests: reading one accepts nothing), `format`
`instagram` (plain text). There is no Reels feed: a reel someone sends, or a
followed account's reel post, is one video, never a page of reels.

Capabilities: `send`, `react`, `mark_read`, `instagram.feed`,
`instagram.stories`, `instagram.profiles`, `instagram.posts`,
`instagram.comments`, `instagram.search`, `instagram.activity`,
`instagram.badge`, and `login.browser` (the live account) or `fixtures`.

**Chat `ext`**: `users` (the other people: User + `verified`, `story`),
`seen` (the newest message is the account's and the other side has seen it),
`call` (`{video, outcome}`: the newest item is a call; `outcome` `missed`,
`ended`, `started`, `unknown`), `request` (a message request), `read_only`
(the chat takes no messages).

**Message `ext`**: `username` (the author's), `heart` (a like-heart, `text`
"❤️"), `call` (`{video, outcome, duration}`, with `system` true), `voice`
(`{duration_ms}`; `media[0]` is the clip), `alt` (a sticker's), `view_once`
(`{video}`: disappearing media, shown as a placeholder, never kept),
`unsupported` (Instagram's own one-line description is `text`),
`preview_only` (the inbox's short copy of a message; its full content comes
with `history`), and `share`:

```json
{"kind":"post","title":"","caption":"…","author":"username","url":"https://…","code":"…",
 "username":null,"story":null,"context":null,"expired":false,"portrait":false,
 "preview":Media|null}
```

`kind`: `post`, `reel`, `story`, `profile`, `link`, `unavailable`;
`context` for stories: `share`, `reply`, `reaction`, `mention`; `portrait`:
a 9:16 card. A share's own note is the message's `text`.

**Objects.** Users (`IgUser`) are the core User with `verified`, `private`
and `story`: the ring the app draws, `unseen`, `close_friends` (unseen, close
friends' green), `seen`, or null (no live story).

```json
Post     {"id","code","ts","author":IgUser,"coauthors":[IgUser],"caption","location",
          "media":[Media],"likes":123|null,"comments":4,"liked","saved","liked_by":"username"|null,
          "reel","pinned","paid","comments_off"}
Reel     {"id","user":IgUser,"latest_ts","seen_ts","unseen","close_friends","muted","own",
          "title"(highlights),"cover":Media|null}               (the tray's summary)
Reel+    {...Reel, "items":[Story]}
Story    {"id","ts","expires","media":Media,"video","duration_ms","close_friends",
          "stickers":[{"kind","x","y","w","h","label","target"}],"reshared_from"}
Profile  {"user":IgUser,"bio","links":[{"title","url"}],"category","posts","followers","following",
          "relation":"following"|"requested"|"none"|"self","followed_by","mutuals","locked"}
Highlight {"id":"highlight:…","title","cover":Media|null}
Comment  {"id","ts","author":IgUser,"text","likes","liked","replies","parent"}
Badge    {"total","likes","comments","relationships","usertags","unseen"}
Activity {"id","kind","ts","text","bold":[{"start","end","username"}],"actors":[IgUser],
          "media":Media|null,"target":{"kind":"post"|"profile"|"story"|"none","id"},"following"}
Hashtag  {"name","posts"}
```

A post's `media` items carry the full-width picture in `ref` and a grid
tile in `thumb`, with the original `w`/`h` (a post's shape is its first
item's, clamped to 4:5 … 1.91:1); a video's `ref` is its poster
(`duration_ms` set). Story stickers are fractions of the 9:16 canvas
(centre and size); `target` is a username, a post id, a URL or a tag.
`likes` is null when the author hides the count. `liked_by`: someone the
account follows who liked it. `locked`: a private account the account
doesn't follow (its posts, tagged posts, highlights and lists are empty).
Activity `kind`: `like`, `comment`, `mention`, `follow`, `tag`,
`story_like`, `comment_like`, `request`, `system`; `bold` ranges are UTF-16
offsets into `text`.

**Commands** (reads unless marked):

| `cmd` | Arguments | Result |
| --- | --- | --- |
| `instagram.feed` | optional `cursor`, `variant` (`following`, the default: newest first; `home`), `cached` | `{"posts":[Post],"cursor","end","cached"}`: only posts from accounts followed (no ads, suggestions or reel recommendations); `end`: "You're all caught up" |
| `instagram.tray` | optional `cached` | `{"reels":[Reel],"own":Reel\|null,"cached"}`: unseen first, muted last |
| `instagram.reels` | `ids` (up to 8 reel ids: users' ids, `highlight:…`) | `{"reels":[Reel+]}` in that order; gone ones left out |
| `instagram.story.seen` | `reel`, `item`, `ts` (the item's) | `{}`: the reel shows seen up to it here; with writes on the bridge also sends Instagram's view receipt (never for the account's own reel or highlights) |
| `instagram.profile` | `user` or `username`, optional `cached` (the account's own) | `{"profile":Profile\|null,"cached","thread":chat id\|null}` (the 1:1 chat, when known) |
| `instagram.posts`, `instagram.tagged` | `user`, optional `username`, `cursor` | `{"posts":[Post],"cursor"}` (a reel on a grid is one video) |
| `instagram.highlights` | `user` | `{"highlights":[Highlight]}` |
| `instagram.post` | `post` | `{"post":Post}` |
| `instagram.comments` | `post`, optional `cursor` | `{"comments":[Comment],"cursor","count"}` |
| `instagram.replies` | `post`, `comment`, optional `cursor` | `{"comments":[Comment],"cursor"}` |
| `instagram.search` | `query` | `{"users":[IgUser],"hashtags":[Hashtag]}` (nothing is added to Instagram's recent searches) |
| `instagram.recent` | | the recent searches, as `instagram.search` answers |
| `instagram.follows` | `user`, `kind` (`followers`, `following`), optional `cursor`, `query` | `{"users":[IgUser],"cursor"}` |
| `instagram.activity` | optional `cached` | `{"badge":Badge,"items":[Activity]\|null,"requests":[IgUser],"writes","cached"}` |
| `instagram.activity.viewed` | | `{"badge":Badge}`: the user saw Activity here; its part of the app's badge clears (Instagram's count is untouched) |
| `instagram.badge` | | `{"badge":Badge}` |
| `instagram.chat_with` | `user` | `{"chat":id\|null}` |
| `instagram.like`, `instagram.save` | `post`, optional `remove` | `{}` (write) |
| `instagram.follow` | `user`, optional `remove` | `{"following"}` (write) |
| `instagram.fixtures.poke` | optional `chat`, `text` | `{"chat"}`: someone writes now (`--fixtures` only) |

`cached: true` answers at once from what the bridge kept from the last
read (the first page of the feed, the tray, the account's profile, the
activity list), so the app draws in its first frame, then asks again
without it. Instagram marks its activity seen when the activity list is
read, so with writes off `instagram.activity` answers `items` from the last
list read with writes on (or null) and the badge's counts; the follow
requests are a plain read.

**Events**: `instagram.badge` `{badge}` (the activity counts or what was
seen here changed).

**Notifications and the badge.** A new message notifies as the core says
(not for messages found long after, at a restart: at most 6 h old); a missed
call is a message like any other. New activity (Instagram's count past what
was announced or seen here) notifies once with tag `activity` and `open`
target `activity` ("3 likes, 1 comment"); the first count after signing in
only records. The badge is the unread chats (inbox, not muted) plus the
activity not seen here.

**Polling.** Instagram's web has no push for this bridge, so it polls, as
Instagram's web app and record do: the open chat every 10 s and the inbox
every 25 s while the app is in front, the inbox every minute while the
screen is on, every 5 minutes while it is off; message requests every tenth
poll; the activity count every minute in front (2 min awake, 10 min off).
±10 % jitter, doubling backoff on failures up to 10 minutes, 5 minutes after
a rate limit; a session Instagram refused is retried every 30 minutes, or
when the app comes to the front (at most every 5 minutes).

**Signing in** (`login.start`): `method` `browser` imports instagram.com's
session from the phone's Browser (sign in there first; nothing else is read
from it), or `cookies` with `cookies` (a Cookie header: `sessionid`,
`csrftoken`, `ds_user_id`, …). `login` events: `waiting`, `done`, `failed`
(`detail`). `logout` forgets the session on the phone only.

### WhatsApp

(to come: the QR login, status updates)

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
