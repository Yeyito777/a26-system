//! Conversation list payloads: summaries, folders, order updates.

use serde::Deserialize;

/// A sidebar row (`ConversationSummary`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Summary {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub title: String,
    pub created_at: f64,
    pub updated_at: f64,
    pub message_count: u64,
    pub pinned: bool,
    pub marked: bool,
    pub muted: bool,
    pub streaming: bool,
    pub unread: bool,
    pub notifications_muted: bool,
    pub sort_order: f64,
    pub folder_id: Option<String>,
    pub parent_conversation_id: Option<String>,
    pub goal: Option<Goal>,
}

impl Summary {
    /// The title as the user should read it: the daemon names a
    /// conversation "pending" until its title is generated.
    pub fn display_title(&self) -> &str {
        match self.title.trim() {
            "" | "pending" => "New Conversation",
            title => title,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Goal {
    pub objective: String,
    pub status: String,
    pub paused_by: Option<String>,
    pub pause_reason: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub pinned: bool,
    pub muted: bool,
    pub sort_order: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRef {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderUpdate {
    pub item: ItemRef,
    pub sort_order: f64,
}
