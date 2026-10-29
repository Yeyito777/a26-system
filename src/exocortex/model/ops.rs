//! Models, settings and the operations panels (WP-E): the daemon's
//! providers, models and efforts, tool styles, defaults, and the model a
//! conversation (or a new one) uses.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;

use super::super::proto::{self, Event, ExternalToolStyle, Provider, Selection, ToolStyle};
use super::super::requests::{self, Owner, Pending};
use super::{Model, Nav};

/// What the daemon said about models and tools.
#[derive(Debug, Default)]
pub struct Ops {
    pub providers: Vec<Provider>,
    /// Tool styles by lower-case tool name (they label tool rows).
    pub tool_styles: HashMap<String, ToolStyle>,
    /// Bash command styles (`gmail` → "Gmail").
    pub external_styles: Vec<ExternalToolStyle>,
    /// Bumped when tool styles change (rows labelled by them lay out again).
    pub styles_version: u64,
    /// Signed in, by provider.
    pub auth: HashMap<String, bool>,
    pub defaults: Selection,
    /// The model a new conversation will use (None: the defaults).
    pub draft_selection: Option<Selection>,
}

/// Screens and choices of this area. Other areas open them with these
/// (D's `/model`, C2's menus): `OpenInspector`, `OpenSettings`, ….
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // The panels are E's; D's commands open them.
pub enum Act {
    /// The model inspector (today: the model sheet).
    OpenInspector,
    /// The chat header's ⋯ menu.
    ChatMenu,
    PickModel(String, String),
    PickEffort(String),
    OpenSettings,
    OpenGoal,
    OpenActivity,
    OpenChrono,
    OpenBtw,
    OpenAccounts,
    OpenSystemPrompt,
    OpenFind,
}

impl Model {
    /// Provider and model of the chat (or the new conversation's).
    pub fn selection(&self) -> Selection {
        if let Some(summary) = self.open_summary() {
            return Selection {
                provider: summary.provider.clone(),
                model: summary.model.clone(),
                effort: summary.effort.clone(),
                ..Selection::default()
            };
        }
        self.ops
            .draft_selection
            .clone()
            .unwrap_or_else(|| self.ops.defaults.clone())
    }

    pub fn model_label(&self, selection: &Selection) -> String {
        self.ops
            .providers
            .iter()
            .find(|p| p.id == selection.provider)
            .and_then(|p| p.models.iter().find(|m| m.id == selection.model))
            .map_or_else(|| selection.model.clone(), |m| m.label.clone())
    }

    /// Providers the daemon can use (signed in, with models).
    pub fn usable_providers(&self) -> Vec<&Provider> {
        self.ops
            .providers
            .iter()
            .filter(|p| !p.models.is_empty() && self.ops.auth.get(&p.id).copied().unwrap_or(false))
            .collect()
    }

    pub(super) fn handle_ops(&mut self, act: Act) -> Vec<Value> {
        match act {
            Act::OpenInspector => {
                self.ui
                    .open(Box::new(super::super::sheets::inspector::inspector()));
                Vec::new()
            }
            Act::ChatMenu => {
                let sheet = super::super::view::chat_header::chat_menu(self);
                self.ui.open(Box::new(sheet));
                Vec::new()
            }
            Act::PickModel(provider, model) => {
                self.ui.close_top = true;
                let effort = self
                    .ops
                    .providers
                    .iter()
                    .find(|p| p.id == provider)
                    .and_then(|p| p.models.iter().find(|m| m.id == model))
                    .map(|m| m.default_effort.clone())
                    .unwrap_or_default();
                match (self.chat.id.clone(), self.connected) {
                    (Some(id), true) => {
                        let request = self.requests.issue(
                            Owner::Ops,
                            "set_model",
                            Some(&id),
                            requests::DEFAULT_WITHIN,
                            Instant::now(),
                        );
                        vec![proto::set_model(&request, &id, &provider, &model)]
                    }
                    (Some(_), false) => Vec::new(),
                    (None, _) => {
                        self.ops.draft_selection = Some(Selection {
                            provider,
                            model,
                            effort,
                            ..Selection::default()
                        });
                        Vec::new()
                    }
                }
            }
            Act::PickEffort(effort) => match (self.chat.id.clone(), self.connected) {
                (Some(id), true) => {
                    let request = self.requests.issue(
                        Owner::Ops,
                        "set_effort",
                        Some(&id),
                        requests::DEFAULT_WITHIN,
                        Instant::now(),
                    );
                    vec![proto::set_effort(&request, &id, &effort)]
                }
                (Some(_), false) => Vec::new(),
                (None, _) => {
                    let mut selection = self.selection();
                    selection.effort = effort;
                    self.ops.draft_selection = Some(selection);
                    Vec::new()
                }
            },
            Act::OpenSettings => self.handle(super::Action::Nav(Nav::OpenSettings)),
            Act::OpenGoal
            | Act::OpenActivity
            | Act::OpenChrono
            | Act::OpenBtw
            | Act::OpenAccounts
            | Act::OpenSystemPrompt
            | Act::OpenFind => {
                self.ui.toast("Coming soon.");
                Vec::new()
            }
        }
    }

    pub(super) fn apply_ops(&mut self, event: &Event, _now: Instant) -> Vec<Value> {
        match event {
            Event::ToolsAvailable {
                providers,
                tools,
                auth_by_provider,
                external_tool_styles,
                conversation_defaults,
                ..
            } => {
                let styles: HashMap<String, ToolStyle> = tools
                    .iter()
                    .map(|tool| (tool.name.to_ascii_lowercase(), tool.clone()))
                    .collect();
                if styles != self.ops.tool_styles
                    || *external_tool_styles != self.ops.external_styles
                {
                    self.ops.tool_styles = styles;
                    self.ops.external_styles = external_tool_styles.clone();
                    self.ops.styles_version += 1;
                    self.chat.touch();
                }
                self.ops.providers = providers.clone();
                self.ops.auth = auth_by_provider.clone();
                if let Some(snapshot) = conversation_defaults {
                    self.ops.defaults = snapshot.defaults.clone();
                }
            }
            Event::ConversationDefaults { defaults, .. } => {
                self.ops.defaults = defaults.clone();
            }
            _ => {}
        }
        Vec::new()
    }

    /// An error answering one of this area's requests; true when handled.
    pub(super) fn ops_error(&mut self, _id: &str, _message: &str) -> bool {
        false
    }

    /// One of this area's requests was not answered in time.
    pub(super) fn ops_timeout(&mut self, _id: &str, _pending: &Pending) -> bool {
        self.generic_timeout()
    }
}
