//! The bridge protocol's objects (docs/social-protocol.md §2), parsed
//! leniently: unknown fields are ignored and missing ones take defaults, so
//! a newer bridge never breaks an older app. Events are parsed on the link
//! thread (`link.rs`); the UI thread gets them typed.

#![allow(dead_code)]

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Media {
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: String,
    pub w: Option<u32>,
    pub h: Option<u32>,
    pub mime: Option<String>,
    pub name: Option<String>,
    pub size: Option<u64>,
    pub duration_ms: Option<u64>,
    pub thumb: Option<String>,
    pub blur: Option<String>,
}

impl Media {
    /// Width over height, when the bridge knows both.
    pub fn aspect(&self) -> Option<f32> {
        match (self.w, self.h) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some(w as f32 / h as f32),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    pub avatar: Option<Media>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Status {
    pub service: String,
    /// `signed_out`, `connecting`, `online`, `offline`, `error`.
    pub state: String,
    pub detail: Option<String>,
    pub writes: bool,
    pub account: Option<User>,
}

impl Status {
    pub fn online(&self) -> bool {
        self.state == "online"
    }

    pub fn signed_out(&self) -> bool {
        self.state == "signed_out"
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ChatLast {
    pub id: String,
    pub ts: i64,
    pub author: Option<String>,
    pub text: String,
    pub mine: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Chat {
    pub id: String,
    /// `dm`, `group`, `channel`, `thread`, `forum`, `voice`.
    pub kind: String,
    pub title: String,
    pub avatar: Option<Media>,
    pub last: Option<ChatLast>,
    pub ts: i64,
    pub unread: u32,
    pub mentions: u32,
    pub muted: bool,
    pub pinned: bool,
    pub archived: bool,
    pub folder: Option<String>,
    pub parent: Option<String>,
    pub members: Option<u32>,
    pub ext: Value,
}

impl Chat {
    pub fn is_group(&self) -> bool {
        self.kind != "dm"
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Author {
    pub id: String,
    pub name: String,
    pub avatar: Option<Media>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReplyTo {
    pub id: String,
    pub author: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub emoji: String,
    pub count: u32,
    pub mine: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: String,
    pub chat: String,
    pub ts: i64,
    pub author: Author,
    pub mine: bool,
    pub text: String,
    /// `plain`, `discord`, `whatsapp`, `instagram`.
    pub format: String,
    pub media: Vec<Media>,
    pub reply_to: Option<ReplyTo>,
    pub reactions: Vec<Reaction>,
    pub edited_ts: Option<i64>,
    pub deleted: bool,
    pub system: bool,
    pub forwarded: bool,
    /// Mine only: `pending`, `sent`, `delivered`, `read`, `failed`.
    pub status: Option<String>,
    pub client_id: Option<String>,
    pub ext: Value,
}

/// What `hello` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Hello {
    pub protocol: u32,
    pub service: String,
    pub status: Status,
    pub capabilities: Vec<String>,
    pub fixtures: bool,
}

/// An answer's error (docs/social-protocol.md §1).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct BridgeError {
    /// `read_only`, `not_found`, `unavailable`, `signed_out`, …; `timeout`
    /// and `disconnected` are the link's own.
    pub code: String,
    pub message: String,
    pub retry_after_ms: Option<u64>,
}

impl BridgeError {
    pub fn local(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retry_after_ms: None,
        }
    }
}

/// What the bridge pushes after `subscribe` (§3.7). `Other` holds the
/// service's own (`discord.…`) and anything newer, raw.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Status(Status),
    Chat(Box<Chat>),
    ChatRemoved(String),
    Message {
        message: Box<Message>,
        live: bool,
    },
    MessageRemoved {
        chat: String,
        id: String,
    },
    Typing {
        chat: String,
        user: User,
        until: i64,
    },
    Read {
        chat: String,
        user: Option<User>,
        message: String,
    },
    Login(Value),
    Other(String, Value),
}

fn field<T: for<'de> Deserialize<'de> + Default>(value: &Value, key: &str) -> T {
    value
        .get(key)
        .and_then(|v| T::deserialize(v).ok())
        .unwrap_or_default()
}

/// One line from the bridge: an answer (`id`) or an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Answer {
        id: u64,
        result: Result<Value, BridgeError>,
    },
    Event(Box<Event>),
}

pub fn parse_line(text: &str) -> Option<Line> {
    let mut value: Value = serde_json::from_str(text).ok()?;
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        let result = if value.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(value
                .get_mut("result")
                .map(Value::take)
                .unwrap_or(Value::Null))
        } else {
            Err(field::<BridgeError>(&value, "error"))
        };
        return Some(Line::Answer { id, result });
    }
    let name = value.get("event")?.as_str()?.to_owned();
    let event = match name.as_str() {
        "status" => Event::Status(field(&value, "status")),
        "chat" => Event::Chat(Box::new(field(&value, "chat"))),
        "chat_removed" => Event::ChatRemoved(field(&value, "id")),
        "message" => Event::Message {
            message: Box::new(field(&value, "message")),
            live: field(&value, "live"),
        },
        "message_removed" => Event::MessageRemoved {
            chat: field(&value, "chat"),
            id: field(&value, "id"),
        },
        "typing" => Event::Typing {
            chat: field(&value, "chat"),
            user: field(&value, "user"),
            until: field(&value, "until"),
        },
        "read" => Event::Read {
            chat: field(&value, "chat"),
            user: field(&value, "user"),
            message: field(&value, "message"),
        },
        "login" => Event::Login(value),
        _ => Event::Other(name, value),
    };
    Some(Line::Event(Box::new(event)))
}

/// The socket of SERVICE's bridge: `MOON_SOCIAL_SOCKET`, else
/// `/run/moon-social/SERVICE.sock`.
pub fn socket_path(service: &str) -> std::path::PathBuf {
    std::env::var_os("MOON_SOCIAL_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| format!("/run/moon-social/{service}.sock").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_events_parse_leniently() {
        let answer = parse_line(r#"{"id":3,"ok":true,"result":{"chats":[]}}"#).unwrap();
        assert!(matches!(
            answer,
            Line::Answer {
                id: 3,
                result: Ok(_)
            }
        ));
        let refused = parse_line(
            r#"{"id":4,"ok":false,"error":{"code":"read_only","message":"writes are off"}}"#,
        )
        .unwrap();
        let Line::Answer {
            result: Err(error), ..
        } = refused
        else {
            panic!("an error")
        };
        assert_eq!(error.code, "read_only");

        let chat = parse_line(
            r#"{"event":"chat","chat":{"id":"wa:1","kind":"group","title":"Trip","ts":5,"unread":2,"future":true,"avatar":{"ref":"i:1","kind":"image","w":64,"h":64}}}"#,
        )
        .unwrap();
        let Line::Event(event) = chat else {
            panic!("an event")
        };
        let Event::Chat(chat) = *event else {
            panic!("a chat")
        };
        assert_eq!(
            (chat.title.as_str(), chat.unread, chat.is_group()),
            ("Trip", 2, true)
        );
        assert_eq!(chat.avatar.unwrap().reference, "i:1");

        let message = parse_line(
            r#"{"event":"message","live":true,"message":{"id":"m","chat":"wa:1","ts":9,"author":{"id":"u","name":"Ana"},"text":"hi","media":[{"ref":"x","kind":"image","w":1600,"h":1200}],"reactions":[{"emoji":"❤️","count":1,"mine":true}]}}"#,
        )
        .unwrap();
        let Line::Event(event) = message else {
            panic!("an event")
        };
        let Event::Message { message, live } = *event else {
            panic!("a message")
        };
        assert!(live);
        assert_eq!(message.media[0].aspect(), Some(1600.0 / 1200.0));
        assert_eq!(message.reactions[0].emoji, "❤️");

        assert!(matches!(
            parse_line(r#"{"event":"discord.guild","guild":{}}"#),
            Some(Line::Event(event)) if matches!(&*event, Event::Other(name, _) if name == "discord.guild")
        ));
        assert!(parse_line("not json").is_none());
    }
}
