//! The Instagram bridge's extension objects (docs/social-protocol.md §5,
//! Instagram), parsed leniently as the core's are (`social::proto`):
//! unknown fields ignored, missing ones defaulted.

use serde::Deserialize;
use serde_json::Value;

use crate::social::proto::Media;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    pub avatar: Option<Media>,
    pub verified: bool,
    pub private: bool,
    /// `unseen`, `close_friends`, `seen`, or none.
    pub story: Option<String>,
}

impl User {
    pub fn handle(&self) -> &str {
        self.username.as_deref().unwrap_or(&self.name)
    }

    pub fn avatar_ref(&self) -> Option<&str> {
        self.avatar.as_ref().map(|media| media.reference.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Post {
    pub id: String,
    pub code: Option<String>,
    pub ts: i64,
    pub author: User,
    pub coauthors: Vec<User>,
    pub caption: String,
    pub location: Option<String>,
    pub media: Vec<Media>,
    pub likes: Option<u64>,
    pub comments: u64,
    pub liked: bool,
    pub saved: bool,
    pub liked_by: Option<String>,
    pub reel: bool,
    pub pinned: bool,
    pub paid: bool,
    pub comments_off: bool,
}

impl Post {
    /// Width over height of the first item, as Instagram clamps it (4:5 to 1.91:1).
    pub fn aspect(&self) -> f32 {
        self.media
            .first()
            .and_then(Media::aspect)
            .unwrap_or(1.0)
            .clamp(0.8, 1.91)
    }

    pub fn video(&self) -> bool {
        self.media.first().is_some_and(|m| m.kind == "video")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReelSummary {
    pub id: String,
    pub user: User,
    pub latest_ts: i64,
    pub seen_ts: i64,
    pub unseen: bool,
    pub close_friends: bool,
    pub muted: bool,
    pub own: bool,
    pub title: Option<String>,
    pub cover: Option<Media>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Sticker {
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StoryItem {
    pub id: String,
    pub ts: i64,
    pub expires: Option<i64>,
    pub media: Media,
    pub video: bool,
    pub duration_ms: u64,
    pub close_friends: bool,
    pub stickers: Vec<Sticker>,
    pub reshared_from: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Reel {
    pub id: String,
    pub user: User,
    pub latest_ts: i64,
    pub seen_ts: i64,
    pub unseen: bool,
    pub close_friends: bool,
    pub own: bool,
    pub title: Option<String>,
    pub items: Vec<StoryItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Link {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub user: User,
    pub bio: String,
    pub links: Vec<Link>,
    pub category: Option<String>,
    pub posts: u64,
    pub followers: u64,
    pub following: u64,
    /// `following`, `requested`, `none`, `self`.
    pub relation: String,
    pub followed_by: bool,
    pub mutuals: Option<String>,
    pub locked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Highlight {
    pub id: String,
    pub title: String,
    pub cover: Option<Media>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Comment {
    pub id: String,
    pub ts: i64,
    pub author: User,
    pub text: String,
    pub likes: u64,
    pub liked: bool,
    pub replies: u64,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Badge {
    pub total: u32,
    pub likes: u32,
    pub comments: u32,
    pub relationships: u32,
    pub usertags: u32,
    pub unseen: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Bold {
    pub start: usize,
    pub end: usize,
    pub username: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Target {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub id: String,
    pub kind: String,
    pub ts: i64,
    pub text: String,
    pub bold: Vec<Bold>,
    pub actors: Vec<User>,
    pub media: Option<Media>,
    pub target: Target,
    pub following: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Hashtag {
    pub name: String,
    pub posts: u64,
}

/// A field of an answer, parsed (default when absent or another shape).
pub fn field<T: for<'de> Deserialize<'de> + Default>(value: &Value, key: &str) -> T {
    value
        .get(key)
        .and_then(|v| T::deserialize(v).ok())
        .unwrap_or_default()
}

/// The users a chat carries in `ext.users`.
pub fn chat_users(ext: &Value) -> Vec<User> {
    field(ext, "users")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posts_parse_leniently_and_clamp_their_shape() {
        let value: Value = serde_json::from_str(
            r#"{"id":"1","ts":5,"author":{"id":"9","name":"Ana","username":"ana","story":"unseen","future":1},
                "caption":"hi","media":[{"ref":"u:x","kind":"image","w":1080,"h":1920}],"likes":null,"comments":3}"#,
        )
        .unwrap();
        let post: Post = serde_json::from_value(value).unwrap();
        assert_eq!(post.author.handle(), "ana");
        assert_eq!(post.likes, None);
        assert!((post.aspect() - 0.8).abs() < 1e-6);
        assert!(!post.video());
    }
}
