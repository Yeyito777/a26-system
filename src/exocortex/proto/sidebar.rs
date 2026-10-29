//! Conversation list payloads (WP-B): summaries, folders, order updates,
//! and the list's commands.

use serde::Deserialize;
use serde_json::{Value, json};

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
    /// `false`, `true` or `"ultrafast"`.
    pub fast_mode: Value,
    /// The turn is waiting for connectivity or capacity.
    pub provider_waiting: bool,
    /// Child subagent turns running for it.
    pub subagent_count: u32,
    /// Detached background processes running for it.
    pub background_task_count: u32,
    /// Its subagents, background work and Chrono schedules.
    pub tasks: Vec<TaskSummary>,
    /// External notification subscriptions targeting it.
    pub integrations: Vec<Value>,
}

/// Work a conversation owns (`ConversationTaskSummary`, `ActiveTaskInfo`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskSummary {
    pub id: String,
    /// `subagent`, `background` or `chrono`.
    pub kind: String,
    pub title: String,
    pub started_at: f64,
    pub due_at: Option<f64>,
    /// `wait`, `sleep` or `wake`.
    pub chrono_mode: Option<String>,
    pub chrono_running: bool,
    pub tool_name: Option<String>,
    /// `tasks_list` only: whose it is, and `running` or `stopping`.
    pub owner_conversation_id: Option<String>,
    pub status: Option<String>,
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
    /// `active`, `paused`, `blocked` or `complete`.
    pub status: String,
    pub paused_by: Option<String>,
    pub pause_reason: Option<String>,
    pub reason: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
    /// Automatic continuation turns since it was set.
    pub turns: u32,
    pub max_time_ms: Option<f64>,
    pub active_ms: Option<f64>,
    pub active_since: Option<f64>,
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
    pub created_at: f64,
    pub updated_at: f64,
    /// The parent-to-child AGENTS.md context for draft chats in it.
    pub effective_instructions: Option<String>,
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

/// `folder_instructions_loaded` and `folder_instructions_updated`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderInstructions {
    pub req_id: Option<String>,
    pub folder_id: String,
    pub text: String,
}

/// `conversation_marked`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Marked {
    pub conv_id: String,
    pub marked: bool,
}

pub fn list_conversations(req_id: &str) -> Value {
    json!({ "type": "list_conversations", "reqId": req_id })
}
