//! Providers, models, efforts, tool styles, the daemon's defaults, and the
//! operations panels' payloads (WP-E): usage, accounts, token statistics,
//! goals, Chrono, tasks, BTW, instructions and the system prompt.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};

use super::chat::Block;
use super::sidebar::{Goal, TaskSummary};

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Effort {
    pub effort: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub default_effort: String,
    pub supported_efforts: Vec<Effort>,
    pub max_context: u64,
    pub supports_images: Option<bool>,
    pub supports_tools: Option<bool>,
    pub supports_fast_mode: Option<bool>,
    pub supports_ultrafast_mode: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub default_model: String,
    pub models: Vec<Model>,
    pub allows_custom_models: bool,
    pub supports_fast_mode: bool,
}

/// A bash command prefix's style (`ExternalToolStyle`): `gmail` → "Gmail".
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExternalToolStyle {
    pub cmd: String,
    pub label: String,
    pub color: String,
}

/// How a tool's calls are labelled (`ToolDisplayInfo`): "$" for bash.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolStyle {
    pub name: String,
    pub label: String,
    pub color: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    pub provider: String,
    pub model: String,
    pub effort: String,
    /// `false`, `true` or `"ultrafast"` (defaults only).
    pub fast_mode: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultsSnapshot {
    pub defaults: Selection,
    /// User-saved overrides rather than the product's defaults.
    pub configured: bool,
}

/// `tools_available`'s other parts (the event's own fields are the ones
/// every client uses).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthInfo {
    pub configured: bool,
    pub authenticated: bool,
    /// `not_logged_in`, `logged_in`, `expired` or `refreshable`.
    pub status: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub subscription_type: Option<String>,
    pub accounts: Vec<Value>,
}

/// A usage window (5-hour, weekly).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageWindow {
    /// 0–100 per account (0–200 for two combined).
    pub utilization: f64,
    pub resets_at: Option<f64>,
    /// Combined over several accounts: each one's reset, soonest first.
    pub resets: Vec<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Usage {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    /// What "full" is: 100 per account combined (absent: 100).
    pub capacity: Option<f64>,
    pub reset_credits: Option<Value>,
    pub api_key: Option<Value>,
}

/// `usage_update`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageUpdate {
    pub provider: String,
    pub usage: Option<Usage>,
}

/// `accounts`: the snapshot is the Accounts sheet's to read.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Accounts {
    pub req_id: Option<String>,
    pub snapshot: Value,
}

/// `usage_reset_result`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageReset {
    pub req_id: Option<String>,
    pub provider: String,
    /// `reset`, `nothing_to_reset`, `no_credit` or `already_redeemed`.
    pub outcome: String,
    pub windows_reset: u32,
    pub remaining_resets: Option<u32>,
}

/// `token_stats`: the snapshot is the Token statistics sheet's to read.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TokenStats {
    pub req_id: Option<String>,
    pub stats: Value,
}

/// `goal_updated`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoalUpdated {
    pub req_id: Option<String>,
    pub conv_id: String,
    pub goal: Option<Goal>,
    pub message: Option<String>,
}

/// A Chrono schedule.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChronoSchedule {
    pub id: String,
    pub title: String,
    pub next_at: f64,
    /// `message` or `command`.
    pub kind: String,
    pub payload: String,
    pub recurrence: Option<Value>,
    /// `scheduled`, `pending` or `running`.
    pub status: String,
}

/// `chrono_result`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChronoResult {
    pub req_id: Option<String>,
    pub conv_id: String,
    /// `list`, `create` or `cancel`.
    pub action: String,
    pub schedules: Vec<ChronoSchedule>,
    pub created: Option<ChronoSchedule>,
    pub cancelled: Vec<Value>,
}

/// `tasks_list`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TasksList {
    pub req_id: Option<String>,
    pub tasks: Vec<TaskSummary>,
}

/// `task_stopped`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskStopped {
    pub req_id: Option<String>,
    pub conv_id: String,
    pub task_id: String,
    pub status: String,
}

/// Every `btw_*` stream event: which it is is the `Event` variant; the
/// fields are the union of theirs.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Btw {
    pub conv_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub query: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    /// `text` or `thinking` (btw_block_start).
    pub block_type: Option<String>,
    pub text: Option<String>,
    pub blocks: Option<Vec<Block>>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub input: Option<Value>,
    pub summary: Option<String>,
    pub output: Option<String>,
    pub is_error: Option<bool>,
    pub status: Option<String>,
    pub message: Option<String>,
    /// `start`, `followup` or `close` (btw_mutation_settled).
    pub mutation: Option<String>,
}

/// `btw_snapshot`: the conversation's retained BTW thread, or none.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BtwSnapshot {
    pub conv_id: String,
    pub btw: Option<Value>,
}

/// `instructions_loaded`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstructionsLoaded {
    pub req_id: Option<String>,
    pub conv_id: Option<String>,
    pub folder_id: Option<String>,
    pub conversation: Option<String>,
    pub folder: Option<String>,
    pub global: String,
    pub global_base: Option<String>,
}

/// `system_prompt`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemPrompt {
    pub req_id: Option<String>,
    pub system_prompt: String,
}

/// `global_instructions_updated`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalInstructions {
    pub req_id: Option<String>,
    pub text: String,
}

/// `auth_status`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthStatus {
    pub req_id: Option<String>,
    pub message: Option<String>,
    pub open_url: Option<String>,
}

/// The auth details of each provider (`authInfoByProvider`).
pub type AuthByProvider = HashMap<String, AuthInfo>;

pub fn set_model(req_id: &str, conv_id: &str, provider: &str, model: &str) -> Value {
    json!({
        "type": "set_model",
        "reqId": req_id,
        "convId": conv_id,
        "provider": provider,
        "model": model,
    })
}

pub fn set_effort(req_id: &str, conv_id: &str, effort: &str) -> Value {
    json!({ "type": "set_effort", "reqId": req_id, "convId": conv_id, "effort": effort })
}
