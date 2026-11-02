//! What the Discord app knows: the bridge's status and settings, the
//! servers (`discord.guilds`), every chat (DMs, channels, threads), the
//! histories it opened, who is typing, member lists. Kept current by the
//! bridge's answers and events (docs/social-protocol.md §5 Discord); the
//! views read it and keep their derived lists (a server's channel tree, the
//! DM list) cached until it changes.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde::Deserialize;
use serde_json::Value;

use crate::social::proto::{Chat, Media, Message, Status};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub position: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Folder {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub icon: Option<Media>,
    pub unread: bool,
    pub mentions: u32,
    pub muted: bool,
    pub position: i64,
    pub folder: Option<Folder>,
    pub categories: Vec<Category>,
    pub owner: bool,
    pub members: Option<u32>,
}

/// A chat's Discord side (`ext`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChatExt {
    pub guild: Option<String>,
    pub category: Option<String>,
    pub position: i64,
    #[serde(rename = "type")]
    pub channel_type: i64,
    pub topic: Option<String>,
    pub nsfw: bool,
    pub locked: bool,
    pub channel_muted: bool,
    pub bot: bool,
    pub messages: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Embed {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub color: Option<u32>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub provider: Option<String>,
    pub author: Option<EmbedAuthor>,
    pub footer: Option<EmbedFooter>,
    pub fields: Vec<EmbedField>,
    pub image: Option<Media>,
    pub thumbnail: Option<Media>,
    pub video: Option<Media>,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbedAuthor {
    pub name: String,
    pub icon: Option<Media>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbedFooter {
    pub text: String,
    pub icon: Option<Media>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RoleName {
    pub name: String,
    pub color: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThreadRef {
    pub id: String,
    pub name: Option<String>,
}

/// A message's Discord side (`ext`), parsed once.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MessageExt {
    #[serde(rename = "type")]
    pub kind: i64,
    pub color: Option<u32>,
    pub bot: bool,
    pub mentioned: bool,
    pub users: HashMap<String, String>,
    pub roles: HashMap<String, RoleName>,
    pub channels: HashMap<String, String>,
    pub emoji: HashMap<String, Media>,
    pub embeds: Vec<Embed>,
    pub stickers: Vec<String>,
    pub urls: Vec<Option<String>>,
    pub thread: Option<ThreadRef>,
    pub reply_color: Option<u32>,
    pub reply_mine: bool,
    pub comment: Option<String>,
}

/// A message as the views use it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub message: Message,
    pub ext: MessageExt,
    /// Changes whenever the message is replaced (its layout is redone).
    pub revision: u64,
}

#[derive(Debug, Default)]
pub struct History {
    /// Oldest first.
    pub entries: Vec<Rc<Entry>>,
    pub more: bool,
    pub loaded: bool,
    pub loading: bool,
    pub loading_older: bool,
    pub error: Option<String>,
    /// Bumped on every change (the view rebuilds its rows).
    pub revision: u64,
}

impl History {
    pub fn find(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|entry| entry.message.id == id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub writes: bool,
    pub notifications: bool,
    pub previews: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ListMember {
    pub id: String,
    pub name: String,
    pub avatar: Option<Media>,
    pub color: Option<u32>,
    pub status: Option<String>,
    pub bot: bool,
    /// A group heading instead of a member.
    pub group: Option<String>,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MemberGroup {
    pub id: String,
    pub count: u32,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MemberList {
    pub chat: String,
    pub groups: Vec<MemberGroup>,
    pub items: Vec<ListMember>,
    pub members: Option<u32>,
    pub online: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Typing {
    pub user: String,
    pub name: String,
    pub until_ms: i64,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[derive(Default)]
pub struct Model {
    pub status: Status,
    pub fixtures: bool,
    pub settings: Settings,
    pub connected: bool,
    pub chats: HashMap<String, Chat>,
    pub chat_ext: HashMap<String, ChatExt>,
    pub guilds: Vec<Guild>,
    pub histories: HashMap<String, History>,
    pub typing: HashMap<String, Vec<Typing>>,
    pub member_lists: HashMap<String, MemberList>,
    /// A forum's posts (`discord.threads`).
    pub threads: HashMap<String, Vec<Chat>>,
    /// Bumped when the chat list changes (views rebuild what they derive).
    pub chats_revision: u64,
    pub guilds_revision: u64,
    next_revision: u64,
    /// Chats whose histories may have missed messages (a reconnect): fetched again when opened.
    pub stale: HashSet<String>,
}

impl Model {
    pub fn revision(&mut self) -> u64 {
        self.next_revision += 1;
        self.next_revision
    }

    pub fn set_chats(&mut self, chats: Vec<Chat>) {
        self.chats.clear();
        self.chat_ext.clear();
        for chat in chats {
            self.put_chat(chat);
        }
        self.chats_revision += 1;
    }

    pub fn put_chat(&mut self, chat: Chat) {
        let ext: ChatExt = serde_json::from_value(chat.ext.clone()).unwrap_or_default();
        self.chat_ext.insert(chat.id.clone(), ext);
        self.chats.insert(chat.id.clone(), chat);
        self.chats_revision += 1;
    }

    pub fn remove_chat(&mut self, id: &str) {
        self.chats.remove(id);
        self.chat_ext.remove(id);
        self.chats_revision += 1;
    }

    pub fn ext(&self, chat: &str) -> Option<&ChatExt> {
        self.chat_ext.get(chat)
    }

    pub fn guild(&self, id: &str) -> Option<&Guild> {
        self.guilds.iter().find(|guild| guild.id == id)
    }

    pub fn set_guilds(&mut self, value: &Value) {
        if let Ok(mut guilds) = serde_json::from_value::<Vec<Guild>>(value.clone()) {
            guilds.sort_by_key(|guild| guild.position);
            self.guilds = guilds;
            self.guilds_revision += 1;
        }
    }

    pub fn put_guild(&mut self, value: &Value) {
        let Ok(guild) = serde_json::from_value::<Guild>(value.clone()) else {
            return;
        };
        match self.guilds.iter_mut().find(|known| known.id == guild.id) {
            Some(known) => *known = guild,
            None => self.guilds.push(guild),
        }
        self.guilds.sort_by_key(|guild| guild.position);
        self.guilds_revision += 1;
    }

    pub fn remove_guild(&mut self, id: &str) {
        self.guilds.retain(|guild| guild.id != id);
        self.guilds_revision += 1;
    }

    pub fn entry(&mut self, message: Message) -> Rc<Entry> {
        let ext: MessageExt = serde_json::from_value(message.ext.clone()).unwrap_or_default();
        let revision = self.revision();
        Rc::new(Entry {
            message,
            ext,
            revision,
        })
    }

    /// A message event: upserted into its history when that is open here.
    pub fn upsert_message(&mut self, message: Message) -> bool {
        if !self.histories.contains_key(&message.chat) {
            return false;
        }
        let entry = self.entry(message);
        let revision = self.revision();
        let history = self.histories.get_mut(&entry.message.chat).unwrap();
        // A sent message's echo replaces its pending bubble.
        if let Some(client) = entry.message.client_id.as_deref()
            && let Some(index) = history.entries.iter().position(|known| {
                known.message.client_id.as_deref() == Some(client)
                    && known.message.status.as_deref() == Some("pending")
            })
        {
            history.entries[index] = entry;
        } else if let Some(index) = history.find(&entry.message.id) {
            history.entries[index] = entry;
        } else {
            let ts = entry.message.ts;
            let at = history
                .entries
                .iter()
                .rposition(|known| known.message.ts <= ts)
                .map_or(0, |i| i + 1);
            history.entries.insert(at, entry);
        }
        history.revision = revision;
        true
    }

    pub fn remove_message(&mut self, chat: &str, id: &str) -> bool {
        let revision = self.revision();
        let Some(history) = self.histories.get_mut(chat) else {
            return false;
        };
        let before = history.entries.len();
        history.entries.retain(|entry| entry.message.id != id);
        if history.entries.len() != before {
            history.revision = revision;
            true
        } else {
            false
        }
    }

    /// A page from `history`: the newest (replacing what was there) or an older one (prepended).
    pub fn page(&mut self, chat: &str, messages: Vec<Message>, more: bool, older: bool) {
        let entries: Vec<Rc<Entry>> = messages
            .into_iter()
            .map(|message| self.entry(message))
            .collect();
        let revision = self.revision();
        let history = self.histories.entry(chat.to_owned()).or_default();
        history.loading = false;
        history.error = None;
        if older {
            history.loading_older = false;
            let known: HashSet<String> = history
                .entries
                .iter()
                .map(|entry| entry.message.id.clone())
                .collect();
            let mut merged: Vec<Rc<Entry>> = entries
                .into_iter()
                .filter(|entry| !known.contains(&entry.message.id))
                .collect();
            merged.append(&mut history.entries);
            history.entries = merged;
            history.more = more;
        } else {
            // Keep older pages already here when the new page reaches them.
            let first = entries.first().map(|entry| entry.message.ts);
            let mut kept: Vec<Rc<Entry>> = match first {
                Some(ts) if history.entries.iter().any(|entry| entry.message.ts >= ts) => {
                    history.more |= more;
                    history
                        .entries
                        .iter()
                        .filter(|entry| entry.message.ts < ts)
                        .cloned()
                        .collect()
                }
                _ => {
                    history.more = more;
                    Vec::new()
                }
            };
            kept.extend(entries);
            history.entries = kept;
        }
        history.loaded = true;
        history.revision = revision;
    }

    /// Unread DMs and mentions: the Messages tab's badge.
    pub fn dm_unread(&self) -> u32 {
        self.chats
            .values()
            .filter(|chat| (chat.kind == "dm" || chat.kind == "group") && !chat.muted)
            .map(|chat| chat.unread)
            .sum()
    }

    pub fn typing_in(&self, chat: &str) -> Vec<&Typing> {
        let now = now_ms();
        self.typing
            .get(chat)
            .map(|list| list.iter().filter(|typing| typing.until_ms > now).collect())
            .unwrap_or_default()
    }

    pub fn title(&self, chat: &str) -> String {
        self.chats
            .get(chat)
            .map(|chat| chat.title.clone())
            .unwrap_or_default()
    }
}
