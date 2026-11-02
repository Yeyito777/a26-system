//! What the app knows, from its bridge: the status and settings, the chats
//! and the open chats' messages (core protocol), and the extension's feed,
//! stories, profiles, posts, comments, search and activity. Answers and
//! events land here; screens draw from it by key. `generation` moves on
//! every change, so a screen knows to lay itself out again.

use std::collections::HashMap;

use serde_json::Value;

use super::model::{
    self, Activity, Badge, Comment, Hashtag, Highlight, Post, Profile, Reel, ReelSummary, User,
};
use crate::social::proto::{BridgeError, Chat, Event, Message, Status};

/// What a request was for: where its answer goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Chats,
    Settings,
    History {
        chat: String,
        older: bool,
    },
    Feed {
        cursor: Option<String>,
        cached: bool,
    },
    Tray {
        cached: bool,
    },
    Reels,
    Profile {
        key: String,
        cached: bool,
    },
    Posts {
        user: String,
        tagged: bool,
        more: bool,
    },
    Highlights {
        user: String,
    },
    Post {
        id: String,
    },
    Comments {
        post: String,
        more: bool,
    },
    Replies {
        post: String,
        comment: String,
    },
    Search {
        query: String,
    },
    Recent,
    Activity {
        cached: bool,
    },
    Badge,
    Send {
        chat: String,
        client_id: String,
    },
    Write {
        what: &'static str,
    },
    Follows {
        user: String,
        kind: &'static str,
        more: bool,
    },
    Login,
    Other,
}

#[derive(Debug, Clone, Default)]
pub struct Thread {
    pub messages: Vec<Message>,
    pub more: bool,
    pub loading_older: bool,
    pub loaded: bool,
    pub failed: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Feed {
    pub posts: Vec<Post>,
    pub cursor: Option<String>,
    pub end: bool,
    pub loading: bool,
    /// Shown from the bridge's cache; the network's first page replaces it.
    pub cached: bool,
    /// A fresh first page that waits while the user reads further down.
    pub pending: Option<(Vec<Post>, Option<String>, bool)>,
    pub failed: Option<String>,
    pub loaded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Grid {
    pub posts: Vec<Post>,
    pub cursor: Option<String>,
    pub loading: bool,
    pub loaded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Users {
    pub users: Vec<User>,
    pub cursor: Option<String>,
    pub loading: bool,
    pub loaded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Comments {
    pub items: Vec<Comment>,
    pub cursor: Option<String>,
    pub count: u64,
    pub loading: bool,
    pub loaded: bool,
    /// comment id → its replies, once opened.
    pub replies: HashMap<String, Vec<Comment>>,
}

#[derive(Debug, Clone, Default)]
pub struct Search {
    pub query: String,
    pub users: Vec<User>,
    pub hashtags: Vec<Hashtag>,
    pub recent_users: Vec<User>,
    pub recent_tags: Vec<Hashtag>,
    pub loading: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ActivityData {
    pub badge: Badge,
    pub items: Option<Vec<Activity>>,
    pub requests: Vec<User>,
    pub writes: bool,
    pub loaded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    pub writes: bool,
    pub notifications: bool,
    pub previews: bool,
}

#[derive(Default)]
pub struct Store {
    pub generation: u64,
    pub status: Status,
    pub fixtures: bool,
    pub capabilities: Vec<String>,
    pub settings: Settings,
    pub chats: Vec<Chat>,
    pub chats_loaded: bool,
    pub threads: HashMap<String, Thread>,
    pub feed: Feed,
    pub tray: Vec<ReelSummary>,
    pub own_reel: Option<ReelSummary>,
    pub tray_loaded: bool,
    /// When the tray was last read from Instagram (not the bridge's cache).
    pub tray_at: Option<std::time::Instant>,
    pub reels: HashMap<String, Reel>,
    pub profiles: HashMap<String, Profile>,
    /// username → user id.
    pub usernames: HashMap<String, String>,
    pub grids: HashMap<String, Grid>,
    pub tagged: HashMap<String, Grid>,
    pub highlights: HashMap<String, Vec<Highlight>>,
    pub posts: HashMap<String, Post>,
    pub comments: HashMap<String, Comments>,
    pub search: Search,
    pub activity: ActivityData,
    /// (user id, `followers` or `following`) → the list.
    pub follows: HashMap<(String, &'static str), Users>,
    /// user id → 1:1 chat id (a profile's Message button).
    pub chat_with: HashMap<String, Option<String>>,
    /// Sends in flight: client id → (chat, text).
    pub sending: HashMap<String, (String, String)>,
    /// The newest message to show: the app's toast.
    pub notice: Option<String>,
    pub login: Option<String>,
}

fn list<T: for<'de> serde::Deserialize<'de>>(value: &Value, key: &str) -> Vec<T> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| T::deserialize(item).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn opt_str(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub fn sort_chats(chats: &mut [Chat]) {
    chats.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.ts.cmp(&a.ts))
            .then(a.id.cmp(&b.id))
    });
}

impl Store {
    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn account(&self) -> Option<&crate::social::proto::User> {
        self.status.account.as_ref()
    }

    pub fn chat(&self, id: &str) -> Option<&Chat> {
        self.chats.iter().find(|chat| chat.id == id)
    }

    pub fn inbox(&self) -> impl Iterator<Item = &Chat> {
        self.chats
            .iter()
            .filter(|chat| chat.folder.as_deref().unwrap_or("inbox") == "inbox")
    }

    pub fn requests(&self) -> impl Iterator<Item = &Chat> {
        self.chats
            .iter()
            .filter(|chat| chat.folder.as_deref() == Some("requests"))
    }

    /// Unread chats in the inbox (the Messages tab's count).
    pub fn unread_chats(&self) -> u32 {
        self.inbox()
            .filter(|chat| chat.unread > 0 && !chat.muted)
            .count() as u32
    }

    pub fn learn_user(&mut self, user: &User) {
        if let Some(username) = &user.username
            && !user.id.is_empty()
        {
            self.usernames.insert(username.clone(), user.id.clone());
        }
    }

    pub fn learn_users<'a>(&mut self, users: impl IntoIterator<Item = &'a User>) {
        for user in users {
            self.learn_user(user);
        }
    }

    /// A bridge event; true when anything changed.
    pub fn event(&mut self, event: Event) -> bool {
        match event {
            Event::Status(status) => {
                self.settings.writes = status.writes;
                self.status = status;
            }
            Event::Chat(chat) => {
                match self.chats.iter_mut().find(|known| known.id == chat.id) {
                    Some(known) => *known = *chat,
                    None => self.chats.push(*chat),
                }
                sort_chats(&mut self.chats);
            }
            Event::ChatRemoved(id) => self.chats.retain(|chat| chat.id != id),
            Event::Message { message, .. } => {
                let Some(thread) = self.threads.get_mut(&message.chat) else {
                    return false;
                };
                if let Some(client) = &message.client_id {
                    thread.messages.retain(|known| {
                        !(known.status.as_deref() == Some("pending")
                            && known.client_id.as_ref() == Some(client))
                    });
                }
                match thread
                    .messages
                    .iter_mut()
                    .find(|known| known.id == message.id)
                {
                    Some(known) => *known = *message,
                    None => {
                        let at = thread
                            .messages
                            .partition_point(|known| known.ts <= message.ts);
                        thread.messages.insert(at, *message);
                    }
                }
            }
            Event::MessageRemoved { chat, id } => {
                if let Some(thread) = self.threads.get_mut(&chat) {
                    thread.messages.retain(|message| message.id != id);
                }
            }
            Event::Other(name, value) if name == "instagram.badge" => {
                self.activity.badge = model::field(&value, "badge");
            }
            _ => return false,
        }
        self.bump();
        true
    }

    /// An answer to `ask`; true when anything changed.
    pub fn answer(&mut self, ask: Ask, result: Result<Value, BridgeError>) -> bool {
        let value = match result {
            Ok(value) => value,
            Err(error) => return self.failed(ask, error),
        };
        match ask {
            Ask::Chats => {
                self.chats = list(&value, "chats");
                sort_chats(&mut self.chats);
                self.chats_loaded = true;
            }
            Ask::Settings => {
                self.settings = Settings {
                    writes: value["writes"].as_bool().unwrap_or(false),
                    notifications: value["notifications"].as_bool().unwrap_or(true),
                    previews: value["previews"].as_bool().unwrap_or(true),
                };
            }
            Ask::History { chat, older } => {
                let page: Vec<Message> = list(&value, "messages");
                let more = value["more"].as_bool().unwrap_or(false);
                let thread = self.threads.entry(chat).or_default();
                thread.failed = None;
                if older {
                    thread.loading_older = false;
                    let mut merged: Vec<Message> = page
                        .into_iter()
                        .filter(|message| {
                            !thread.messages.iter().any(|known| known.id == message.id)
                        })
                        .collect();
                    merged.append(&mut thread.messages);
                    thread.messages = merged;
                    thread.more = more;
                } else {
                    // The newest page: what is older and already here stays,
                    // and sends not confirmed yet stay at the end.
                    let first_ts = page.first().map_or(i64::MAX, |message| message.ts);
                    let is_pending = |m: &Message| m.status.as_deref() == Some("pending");
                    let mut older: Vec<Message> = thread
                        .messages
                        .iter()
                        .filter(|known| {
                            known.ts < first_ts
                                && !is_pending(known)
                                && !page.iter().any(|m| m.id == known.id)
                        })
                        .cloned()
                        .collect();
                    let pending: Vec<Message> = thread
                        .messages
                        .iter()
                        .filter(|known| {
                            is_pending(known)
                                && !page.iter().any(|m| {
                                    m.client_id.is_some() && m.client_id == known.client_id
                                })
                        })
                        .cloned()
                        .collect();
                    if older.is_empty() {
                        thread.more = more;
                    }
                    older.extend(page);
                    older.extend(pending);
                    thread.messages = older;
                    thread.loaded = true;
                }
            }
            Ask::Feed { cursor, cached } => {
                let posts: Vec<Post> = list(&value, "posts");
                for post in &posts {
                    self.learn_user(&post.author);
                }
                let next = opt_str(&value, "cursor");
                let end = value["end"].as_bool().unwrap_or(false);
                let feed = &mut self.feed;
                feed.loading = false;
                feed.failed = None;
                if cursor.is_some() {
                    let known: std::collections::HashSet<String> =
                        feed.posts.iter().map(|p| p.id.clone()).collect();
                    feed.posts
                        .extend(posts.into_iter().filter(|post| !known.contains(&post.id)));
                    feed.cursor = next;
                    feed.end = end;
                } else if cached {
                    if !feed.loaded && !posts.is_empty() {
                        feed.posts = posts;
                        feed.cursor = None;
                        feed.cached = true;
                    }
                } else {
                    let same = feed.posts.len() >= posts.len().min(3)
                        && feed
                            .posts
                            .iter()
                            .zip(&posts)
                            .take(3)
                            .all(|(a, b)| a.id == b.id);
                    if feed.posts.is_empty() || feed.cached || same || !feed.loaded {
                        feed.posts = posts;
                        feed.cursor = next;
                        feed.end = end;
                        feed.pending = None;
                    } else {
                        feed.pending = Some((posts, next, end));
                    }
                    feed.cached = false;
                    feed.loaded = true;
                }
            }
            Ask::Tray { cached } => {
                let reels: Vec<ReelSummary> = list(&value, "reels");
                if cached && self.tray_loaded {
                    return false;
                }
                for reel in &reels {
                    self.learn_user(&reel.user);
                }
                self.tray = reels;
                self.own_reel = value
                    .get("own")
                    .and_then(|own| serde_json::from_value(own.clone()).ok());
                if !cached {
                    self.tray_loaded = true;
                    self.tray_at = Some(std::time::Instant::now());
                }
            }
            Ask::Reels => {
                for reel in list::<Reel>(&value, "reels") {
                    self.learn_user(&reel.user);
                    self.reels.insert(reel.id.clone(), reel);
                }
            }
            Ask::Profile { key, cached } => {
                let profile: Option<Profile> = value
                    .get("profile")
                    .and_then(|p| serde_json::from_value(p.clone()).ok());
                let Some(profile) = profile else {
                    return false;
                };
                if cached && self.profiles.contains_key(&profile.user.id) {
                    return false;
                }
                self.learn_user(&profile.user);
                if let Some(thread) = value.get("thread") {
                    self.chat_with
                        .insert(profile.user.id.clone(), thread.as_str().map(str::to_owned));
                }
                if key != profile.user.id {
                    self.usernames.insert(key, profile.user.id.clone());
                }
                self.profiles.insert(profile.user.id.clone(), profile);
            }
            Ask::Posts { user, tagged, more } => {
                let posts: Vec<Post> = list(&value, "posts");
                let map = if tagged {
                    &mut self.tagged
                } else {
                    &mut self.grids
                };
                let grid = map.entry(user).or_default();
                grid.loading = false;
                grid.loaded = true;
                if more {
                    grid.posts.extend(posts);
                } else {
                    grid.posts = posts;
                }
                grid.cursor = opt_str(&value, "cursor");
            }
            Ask::Highlights { user } => {
                self.highlights.insert(user, list(&value, "highlights"));
            }
            Ask::Post { id } => {
                if let Some(post) = value
                    .get("post")
                    .and_then(|p| serde_json::from_value::<Post>(p.clone()).ok())
                {
                    self.learn_user(&post.author);
                    self.posts.insert(id, post);
                }
            }
            Ask::Comments { post, more } => {
                let items: Vec<Comment> = list(&value, "comments");
                for comment in &items {
                    self.learn_user(&comment.author);
                }
                let comments = self.comments.entry(post).or_default();
                comments.loading = false;
                comments.loaded = true;
                comments.count = value["count"].as_u64().unwrap_or(comments.count);
                comments.cursor = opt_str(&value, "cursor");
                if more {
                    comments.items.extend(items);
                } else {
                    comments.items = items;
                }
            }
            Ask::Replies { post, comment } => {
                let items: Vec<Comment> = list(&value, "comments");
                self.comments
                    .entry(post)
                    .or_default()
                    .replies
                    .insert(comment, items);
            }
            Ask::Search { query } => {
                if query != self.search.query {
                    return false;
                }
                self.search.users = list(&value, "users");
                self.search.hashtags = list(&value, "hashtags");
                self.search.loading = false;
                let users = self.search.users.clone();
                self.learn_users(&users);
            }
            Ask::Recent => {
                self.search.recent_users = list(&value, "users");
                self.search.recent_tags = list(&value, "hashtags");
            }
            Ask::Activity { cached } => {
                let activity = &mut self.activity;
                activity.badge = model::field(&value, "badge");
                activity.writes = value["writes"].as_bool().unwrap_or(false);
                if value.get("items").is_some_and(Value::is_array) {
                    activity.items = Some(list(&value, "items"));
                }
                activity.requests = list(&value, "requests");
                if !cached {
                    activity.loaded = true;
                }
            }
            Ask::Badge => self.activity.badge = model::field(&value, "badge"),
            Ask::Send { chat, client_id } => {
                self.sending.remove(&client_id);
                if let Some(message) = value
                    .get("message")
                    .and_then(|m| serde_json::from_value::<Message>(m.clone()).ok())
                {
                    self.event(Event::Message {
                        message: Box::new(message),
                        live: true,
                    });
                }
                let _ = chat;
            }
            Ask::Follows { user, kind, more } => {
                let users: Vec<User> = list(&value, "users");
                self.learn_users(&users);
                let entry = self.follows.entry((user, kind)).or_default();
                entry.loading = false;
                entry.loaded = true;
                entry.cursor = opt_str(&value, "cursor");
                if more {
                    entry.users.extend(users);
                } else {
                    entry.users = users;
                }
            }
            Ask::Login => self.login = Some("Imported. Connecting…".into()),
            Ask::Write { .. } | Ask::Other => {}
        }
        self.bump();
        true
    }

    fn failed(&mut self, ask: Ask, error: BridgeError) -> bool {
        let message = match error.code.as_str() {
            "read_only" => "Writes are off. Turn them on in Settings to do this.".to_owned(),
            "signed_out" => "Not signed in to Instagram.".to_owned(),
            "rate_limited" => "Instagram asked to slow down. Try again later.".to_owned(),
            "challenge" => "Instagram wants to verify it's you.".to_owned(),
            "disconnected" => "The Instagram bridge isn't running.".to_owned(),
            _ => "Couldn't load. Try again.".to_owned(),
        };
        match ask {
            Ask::History { chat, older } => {
                let thread = self.threads.entry(chat).or_default();
                if older {
                    thread.loading_older = false;
                } else {
                    thread.failed = Some(message);
                }
            }
            Ask::Feed { cached, .. } => {
                self.feed.loading = false;
                if !cached {
                    self.feed.failed = Some(message);
                }
            }
            Ask::Posts { user, tagged, .. } => {
                let map = if tagged {
                    &mut self.tagged
                } else {
                    &mut self.grids
                };
                let grid = map.entry(user).or_default();
                grid.loading = false;
                grid.loaded = true;
            }
            Ask::Comments { post, .. } => {
                let comments = self.comments.entry(post).or_default();
                comments.loading = false;
                comments.loaded = true;
            }
            Ask::Search { .. } => self.search.loading = false,
            Ask::Follows { user, kind, .. } => {
                let entry = self.follows.entry((user, kind)).or_default();
                entry.loading = false;
                entry.loaded = true;
            }
            Ask::Send { chat, client_id } => {
                self.sending.remove(&client_id);
                if let Some(thread) = self.threads.get_mut(&chat)
                    && let Some(message) = thread
                        .messages
                        .iter_mut()
                        .find(|m| m.client_id.as_deref() == Some(client_id.as_str()))
                {
                    message.status = Some("failed".into());
                }
                self.notice = Some(message);
            }
            Ask::Write { .. } | Ask::Login => {
                self.notice = Some(if error.code == "read_only" {
                    message
                } else {
                    error.message.clone()
                })
            }
            Ask::Activity { .. } => self.activity.loaded = true,
            _ => {}
        }
        self.bump();
        true
    }

    /// A request goes out: what it loads shows as loading.
    pub fn asking(&mut self, ask: &Ask) {
        match ask {
            Ask::History { chat, older: true } => {
                self.threads.entry(chat.clone()).or_default().loading_older = true
            }
            Ask::Feed {
                cursor: Some(_), ..
            } => self.feed.loading = true,
            Ask::Posts { user, tagged, .. } => {
                let map = if *tagged {
                    &mut self.tagged
                } else {
                    &mut self.grids
                };
                map.entry(user.clone()).or_default().loading = true;
            }
            Ask::Comments { post, more: true } => {
                self.comments.entry(post.clone()).or_default().loading = true
            }
            Ask::Search { .. } => self.search.loading = true,
            Ask::Follows { user, kind, .. } => {
                self.follows
                    .entry((user.clone(), kind))
                    .or_default()
                    .loading = true
            }
            _ => return,
        }
        self.bump();
    }

    /// A message the user sends shows at once, pending.
    pub fn add_pending(&mut self, chat: &str, text: &str, client_id: &str, me: &str) {
        let thread = self.threads.entry(chat.to_owned()).or_default();
        thread.messages.push(Message {
            id: format!("pending:{client_id}"),
            chat: chat.to_owned(),
            ts: super::paint::now_ms(),
            author: crate::social::proto::Author {
                id: me.to_owned(),
                name: "You".into(),
                avatar: None,
            },
            mine: true,
            text: text.to_owned(),
            format: "instagram".into(),
            status: Some("pending".into()),
            client_id: Some(client_id.to_owned()),
            ..Default::default()
        });
        self.sending
            .insert(client_id.to_owned(), (chat.to_owned(), text.to_owned()));
        self.bump();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fresh_feed_page_waits_while_the_user_reads() {
        let mut store = Store::default();
        let page = |ids: &[&str]| json!({"posts": ids.iter().map(|id| json!({"id": id, "author": {"id": "1", "name": "a"}})).collect::<Vec<_>>(), "cursor": "c"});
        store.answer(
            Ask::Feed {
                cursor: None,
                cached: false,
            },
            Ok(page(&["1", "2", "3"])),
        );
        assert_eq!(store.feed.posts.len(), 3);
        store.answer(
            Ask::Feed {
                cursor: None,
                cached: false,
            },
            Ok(page(&["9", "1", "2"])),
        );
        assert_eq!(store.feed.posts[0].id, "1");
        assert!(store.feed.pending.is_some());
        store.answer(
            Ask::Feed {
                cursor: Some("c".into()),
                cached: false,
            },
            Ok(page(&["3", "4"])),
        );
        assert_eq!(store.feed.posts.len(), 4);
    }

    #[test]
    fn history_pages_and_live_messages_merge_by_id() {
        let mut store = Store::default();
        let message = |id: &str, ts: i64| json!({"id": id, "chat": "c", "ts": ts, "author": {"id": "u", "name": "U"}, "text": id});
        store.answer(
            Ask::History {
                chat: "c".into(),
                older: false,
            },
            Ok(json!({"messages": [message("b", 2), message("c", 3)], "more": true})),
        );
        store.answer(
            Ask::History {
                chat: "c".into(),
                older: true,
            },
            Ok(json!({"messages": [message("a", 1)], "more": false})),
        );
        let event: Message = serde_json::from_value(message("d", 4)).unwrap();
        store.event(Event::Message {
            message: Box::new(event),
            live: true,
        });
        let ids: Vec<&str> = store.threads["c"]
            .messages
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "c", "d"]);
        assert!(!store.threads["c"].more);
    }
}
