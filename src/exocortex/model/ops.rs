//! The daemon's providers, models and efforts, and the model a
//! conversation (or a new one) uses.

use super::super::proto::{Provider, Selection};
use super::Model;

impl Model {
    /// Provider and model of the chat (or the new conversation's).
    pub fn selection(&self) -> Selection {
        if let Some(summary) = self.open_summary() {
            return Selection {
                provider: summary.provider.clone(),
                model: summary.model.clone(),
                effort: summary.effort.clone(),
            };
        }
        self.draft_selection
            .clone()
            .unwrap_or_else(|| self.defaults.clone())
    }

    pub fn model_label(&self, selection: &Selection) -> String {
        self.providers
            .iter()
            .find(|p| p.id == selection.provider)
            .and_then(|p| p.models.iter().find(|m| m.id == selection.model))
            .map_or_else(|| selection.model.clone(), |m| m.label.clone())
    }

    /// Providers the daemon can use (signed in, with models).
    pub fn usable_providers(&self) -> Vec<&Provider> {
        self.providers
            .iter()
            .filter(|p| !p.models.is_empty() && self.auth.get(&p.id).copied().unwrap_or(false))
            .collect()
    }
}
