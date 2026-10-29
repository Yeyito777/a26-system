//! Providers, models, efforts, tool styles and the daemon's defaults.

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Effort {
    pub effort: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub default_effort: String,
    pub supported_efforts: Vec<Effort>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub default_model: String,
    pub models: Vec<Model>,
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
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultsSnapshot {
    pub defaults: Selection,
}

pub fn set_model(conv_id: &str, provider: &str, model: &str) -> Value {
    json!({ "type": "set_model", "convId": conv_id, "provider": provider, "model": model })
}

pub fn set_effort(conv_id: &str, effort: &str) -> Value {
    json!({ "type": "set_effort", "convId": conv_id, "effort": effort })
}
