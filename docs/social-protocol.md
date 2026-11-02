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

The provider is record's Discord code (its REST client and its gateway's
wire protocol), so the bridge looks to Discord like record does.

**Chats.** One per DM (`dm`), group DM (`group`), server channel the user
can view (`channel`: text and announcement channels; `forum`: forum and
media channels; `voice`: voice and stage channels, for their text chat) and
active thread the user joined (`thread`). Chat ids are Discord's channel
ids; categories are not chats.

| Field | Discord |
| --- | --- |
| `parent` | a channel's category id (or null); a thread's channel id |
| `unread` | DMs: unread messages; server channels and threads: 1 when unread |
| `mentions` | unread mentions (every unread DM message is one, as Discord counts) |
| `muted` | the channel, its category, the thread or the whole server is muted (the user's settings) |
| `ts` | last activity; a server channel's moves only when its unread state changes (apps order those by `ext.position`) |
| `last` | DMs only, from what the bridge saw (null until a message comes or the chat opens) |
| `ext` | server channels and threads: `guild` (server id), `category`, `position`, `type` (Discord's channel type), `topic`, `nsfw`, `locked`, `channel_muted` (muted itself, not only by its server), `messages` (a thread's); DMs: `type`, `recipients` (`[{id,name,username,bot}]`), `bot` |

**Servers.** `discord.guilds` answers `{"guilds":[Guild]}` in the user's
rail order (their folders kept together):

```json
{"id":"…","name":"Record Lounge","icon":Media|null,"unread":true,"mentions":2,
 "muted":false,"position":0,"folder":{"id":"…","name":"Dev","color":5793266}|null,
 "categories":[{"id":"…","name":"Text Channels","position":4}],"owner":false,"members":47}
```

`unread`: an unmuted channel the user can see has something unread.
Events: `discord.guilds` (the whole list: a new session, a folder change),
`discord.guild` (`guild`: one changed), `discord.guild_removed` (`id`).

**Threads.** `discord.threads` (`chat`, optional `archived`, `offset`)
answers `{"threads":[Chat],"more":false}`: a forum's posts or a channel's
threads, joined or not (the chat list holds only joined ones).

**Members.** `discord.member_list` (`chat`) answers the member list of the
channel an app views, as Discord sends it for the channel shown:
`{"chat":…,"groups":[{"id","count","name"}],"items":[{"group","count"} or
{"id","name","avatar","color","status","bot"}],"members":N,"online":N}`
(empty until Discord sent one; a DM's people). The `discord.member_list`
event carries the same when it changes. `discord.members` (`guild`,
`members: [{id,name,color}]`): server nicknames and role colours learnt for
message authors (the bridge also sends the viewed chat's messages by them
again).

**Messages.** `format` is `discord`: the text is Discord's markdown with its
tokens: `<@id>` (a user), `<@&id>` (a role), `<#id>` (a channel),
`<:name:id>` and `<a:name:id>` (custom emoji), `<t:UNIX:STYLE>`
(timestamps), `</name:id>` (a command). `author.name` is the server
nickname when known. A `system` message's words are in `text` (joins, pins,
calls, threads started). A `forwarded` message's text and media are the
forwarded one's (`ext.comment`: the forwarder's own words). `ext`:

| Field | |
| --- | --- |
| `type` | Discord's message type |
| `color`, `bot` | the author's role colour, a bot or app |
| `mentioned` | it mentions the user (drawn highlighted) |
| `users`, `roles`, `channels` | names for the text's tokens: `{id: name}`, roles `{id: {name, color}}` |
| `emoji` | `{id: Media}`: the custom emoji in the text and the reactions |
| `embeds` | `[{type,color,title,url,description,provider,author:{name,icon},footer:{text,icon},fields:[{name,value,inline}],image,thumbnail,video,timestamp}]` (descriptions and field values are markdown; a video is a Media with its poster as `thumb`) |
| `stickers` | names of stickers nothing draws (Lottie) |
| `urls` | each `media`'s original URL (to open a file or a video), or null |
| `thread` | `{id,name}`: the thread started from it |
| `reply_color`, `reply_mine` | the replied-to author's role colour; it is the user |

`media`: attachments as `image`, `gif`, `video` (`thumb`: its first frame),
`audio` (voice messages), `file`; stickers as `sticker`. A reaction's
`emoji` is the emoji itself or a custom emoji's token.

**What comes live.** `message` events come for the chats an app views
(`view`); for the others the `chat` event says their unread state changed
and `history` fetches them. `typing` comes for viewed chats.

**Notifications** as Discord's mobile app: DMs and group DMs, messages that
mention the user (by name; `@everyone`, `@here` and their roles unless the
server is set to suppress them), and channels the user set (or their server
defaults) to all messages; never a muted chat or server. Titles: the DM's
name, `#channel (Server)`, `thread (#channel, Server)`; the body in plain
words (names for mentions). The badge: unread DM messages plus mentions.

**Writes** (only with writes on): `send` (`text`, `reply_to`; files not
yet), `react` (`emoji`: an emoji or a custom emoji's token), `edit`,
`delete`, `mark_read` (Discord's ack). No `typing`.

**Presence.** While writes are off the bridge's gateway session is invisible
and away: the bridge never shows the user online. With writes on it carries
the status the user set and follows their changes elsewhere.

**Signing in.** `login.start` with `method: "token"` and `token`: checked
with Discord, then kept in the state directory (`session.json`, `0600`).

**Capabilities:** `discord`, `edit`, `delete`, `react`, `reply`.

**Fixtures.** `--fixtures` runs on record's made-up Discord (src/demo:
fifteen servers, DMs, long histories with pictures, embeds and reactions;
its clock fixed at 2026-10-24 21:00 UTC, or `DISCORD_FIXTURES_NOW`), in the
bridge's process, with no network. `discord.fixtures.emit` (`chat`, `text`,
`mention`) makes someone write there; `DISCORD_FIXTURES_INCOMING_SECONDS=N`
one message every N seconds.

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
