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

/// A `ping` sent right after a command with the command's reqId: the daemon
/// handles a client's commands in order, so by its `pong` every broadcast
/// the command caused has arrived (most list commands answer nothing).
pub fn barrier(req_id: &str) -> Value {
    json!({ "type": "ping", "reqId": req_id, "updateStatusOnly": true })
}

pub fn pin_conversation(req_id: &str, conv_id: &str, pinned: bool) -> Value {
    json!({ "type": "pin_conversation", "reqId": req_id, "convId": conv_id, "pinned": pinned })
}

pub fn mark_conversation(req_id: &str, conv_id: &str, marked: bool) -> Value {
    json!({ "type": "mark_conversation", "reqId": req_id, "convId": conv_id, "marked": marked })
}

pub fn mute_conversation(req_id: &str, conv_id: &str, muted: bool) -> Value {
    json!({ "type": "mute_conversation", "reqId": req_id, "convId": conv_id, "muted": muted })
}

pub fn rename_conversation(req_id: &str, conv_id: &str, title: &str) -> Value {
    json!({ "type": "rename_conversation", "reqId": req_id, "convId": conv_id, "title": title })
}

pub fn delete_conversation(req_id: &str, conv_id: &str) -> Value {
    json!({ "type": "delete_conversation", "reqId": req_id, "convId": conv_id })
}

pub fn undo_delete(req_id: &str) -> Value {
    json!({ "type": "undo_delete", "reqId": req_id })
}

pub fn redo_delete(req_id: &str) -> Value {
    json!({ "type": "redo_delete", "reqId": req_id })
}

pub fn create_folder(req_id: &str, name: &str, parent_id: Option<&str>) -> Value {
    json!({ "type": "create_folder", "reqId": req_id, "name": name, "parentId": parent_id })
}

pub fn rename_folder(req_id: &str, folder_id: &str, name: &str) -> Value {
    json!({ "type": "rename_folder", "reqId": req_id, "folderId": folder_id, "name": name })
}

pub fn pin_folder(req_id: &str, folder_id: &str, pinned: bool) -> Value {
    json!({ "type": "pin_folder", "reqId": req_id, "folderId": folder_id, "pinned": pinned })
}

pub fn mute_folder(req_id: &str, folder_id: &str, muted: bool) -> Value {
    json!({ "type": "mute_folder", "reqId": req_id, "folderId": folder_id, "muted": muted })
}

/// `unwrap` keeps the folder's children (they move up a level);
/// `recursive` moves the folder and everything in it to the trash.
pub fn delete_folder(req_id: &str, folder_id: &str, recursive: bool) -> Value {
    json!({
        "type": "delete_folder",
        "reqId": req_id,
        "folderId": folder_id,
        "mode": if recursive { "recursive" } else { "unwrap" },
    })
}
