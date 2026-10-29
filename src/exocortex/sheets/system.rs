//! System prompt and instructions (WP-E; PAR-307, 308; the iPhone's
//! `ui/ops/system`): the conversation's own instructions (edit, clear),
//! its folder's (read-only), and the effective system prompt
//! (`get_system_prompt`), read in full in the reader.

use std::time::Instant;

use serde_json::Value;

use super::super::model::{Action, Model, UiAct, ops};
use super::super::proto::{self, Event};
use super::super::requests::{self, Owner};
use super::super::theme;
use super::super::ui::menu::Confirm;
use super::super::ui::textarea::{self, TextArea};
use super::panel::{self, Btn, Built, Kind, Row, Style, Tap};

/// What the daemon said, and what is on its way.
#[derive(Debug, Default)]
pub struct State {
    /// The effective prompt, for this conversation (None: a draft).
    pub prompt: Option<(Option<String>, String)>,
    pub prompt_request: Option<String>,
    /// `instructions_loaded`, for this conversation.
    pub loaded: Option<(Option<String>, proto::InstructionsLoaded)>,
    pub instructions_request: Option<String>,
    pub saving: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    Refresh,
    Edit,
    Save(String),
    Clear,
    ClearConfirmed,
}

fn act(a: Act) -> Action {
    Action::Ops(ops::Act::System(a))
}

const PREVIEW_LINES: usize = 14;

impl Model {
    /// The conversation's own instructions: what the daemon last loaded,
    /// else the window's (`Chat::own_instructions`).
    pub fn conversation_instructions(&self, conv: &str) -> Option<String> {
        if let Some((Some(id), loaded)) = &self.ops.system.loaded
            && id == conv
        {
            return loaded.conversation.clone().filter(|t| !t.is_empty());
        }
        if self.chat.id.as_deref() != Some(conv) {
            return None;
        }
        self.chat.own_instructions().map(str::to_owned)
    }

    /// The panel opens: ask for the effective prompt and the instructions.
    pub(crate) fn load_system(&mut self, conv: Option<&str>) -> Vec<Value> {
        if !self.ready {
            return Vec::new();
        }
        let now = Instant::now();
        let prompt = self.requests.issue(
            Owner::Ops,
            "get_system_prompt",
            conv,
            requests::DEFAULT_WITHIN,
            now,
        );
        let instructions = self.requests.issue(
            Owner::Ops,
            "load_instructions",
            conv,
            requests::DEFAULT_WITHIN,
            now,
        );
        self.ops.system.prompt_request = Some(prompt.clone());
        self.ops.system.instructions_request = Some(instructions.clone());
        self.ops.system.error = None;
        let folder = match conv {
            Some(id) => self
                .sidebar
                .conversations
                .get(id)
                .and_then(|s| s.folder_id.clone()),
            None => self.current_folder().map(str::to_owned),
        };
        let mut load = serde_json::json!({ "type": "load_instructions", "reqId": instructions });
        if let Some(id) = conv {
            load["convId"] = Value::String(id.to_owned());
        }
        load["folderId"] = folder.map_or(Value::Null, Value::String);
        vec![proto::get_system_prompt(&prompt, conv), load]
    }

    pub(crate) fn handle_system(&mut self, a: Act) -> Vec<Value> {
        let conv = self.chat.id.clone();
        match a {
            Act::Refresh => self.load_system(conv.as_deref()),
            Act::Edit => {
                let Some(conv) = conv else {
                    return Vec::new();
                };
                let current = self.conversation_instructions(&conv).unwrap_or_default();
                self.ui.open(Box::new(textarea::sheet(
                    TextArea::new(
                        "Instructions",
                        "This conversation's own instructions: they replace what it had",
                        current,
                        "Save",
                        Box::new(|text| act(Act::Save(text))),
                    )
                    .allow_empty(),
                )));
                Vec::new()
            }
            Act::Save(text) => self.save_instructions(conv.as_deref(), &text),
            Act::Clear => {
                self.ui.open(Box::new(Confirm::new(
                    "Clear the instructions?",
                    "This conversation's own instructions are removed.",
                    "Clear",
                    act(Act::ClearConfirmed),
                )));
                Vec::new()
            }
            Act::ClearConfirmed => self.save_instructions(conv.as_deref(), ""),
        }
    }

    fn save_instructions(&mut self, conv: Option<&str>, text: &str) -> Vec<Value> {
        let Some(conv) = conv else {
            return Vec::new();
        };
        if !self.ready {
            self.ui.error_toast("Not connected to Exocortex.");
            return Vec::new();
        }
        let request = self.requests.issue(
            Owner::Ops,
            "set_system_instructions",
            Some(conv),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        );
        self.ops.system.saving = Some(request.clone());
        vec![proto::set_system_instructions(&request, conv, text)]
    }

    pub(crate) fn apply_system(&mut self, event: &Event) -> Vec<Value> {
        let state = &mut self.ops.system;
        match event {
            Event::SystemPrompt(e) if e.req_id.is_some() && e.req_id == state.prompt_request => {
                state.prompt_request = None;
                state.prompt = Some((self.chat.id.clone(), e.system_prompt.clone()));
            }
            Event::InstructionsLoaded(e)
                if e.req_id.is_some() && e.req_id == state.instructions_request =>
            {
                state.instructions_request = None;
                state.loaded = Some((e.conv_id.clone(), e.clone()));
            }
            Event::Ack { req_id, .. } if req_id.is_some() && *req_id == state.saving => {
                state.saving = None;
                self.ui.toast("Instructions saved");
                return self.load_system(self.chat.id.clone().as_deref());
            }
            Event::SystemInstructionsUpdated(e) => {
                if let Some((Some(id), loaded)) = &mut state.loaded
                    && *id == e.conv_id
                {
                    loaded.conversation = Some(e.text.clone());
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// An error answering a prompt or instructions request; true when it
    /// was one.
    pub(crate) fn system_error(&mut self, id: &str, message: &str) -> bool {
        let state = &mut self.ops.system;
        let mine = [
            &mut state.prompt_request,
            &mut state.instructions_request,
            &mut state.saving,
        ]
        .into_iter()
        .any(|slot| {
            if slot.as_deref() == Some(id) {
                *slot = None;
                true
            } else {
                false
            }
        });
        if mine {
            state.error = Some(message.to_owned());
        }
        mine
    }
}

/// The System prompt panel's rows.
pub fn build(model: &Model, conv: Option<&str>) -> Built {
    let p = theme::p();
    let state = &model.ops.system;
    let mut rows = Vec::new();
    if let Some(conv) = conv {
        rows.push(panel::section("This chat's instructions"));
        match model.conversation_instructions(conv) {
            Some(text) => rows.push(panel::text(text, Style::Note, p.text)),
            None => rows.push(panel::note(
                "This conversation has no instructions of its own.",
            )),
        }
        let busy = state.saving.is_some();
        let tap = |a: Act| (!busy && model.ready).then_some(Tap::Stay(act(a)));
        let mut buttons = vec![Btn::new(
            if busy { "Saving…" } else { "Edit…" },
            Kind::Secondary,
            tap(Act::Edit),
        )];
        if model.conversation_instructions(conv).is_some() {
            buttons.push(Btn::new("Clear", Kind::Destructive, tap(Act::Clear)));
        }
        rows.push(Row::Buttons(buttons));
    }
    let matches = |id: &Option<String>| id.as_deref() == conv;
    if let Some((id, loaded)) = &state.loaded
        && matches(id)
        && let Some(folder) = loaded.folder.as_ref().filter(|f| !f.is_empty())
    {
        rows.push(panel::section("Folder instructions"));
        rows.push(panel::text(folder.clone(), Style::Note, p.text));
    }
    rows.push(Row::Section {
        title: "Effective system prompt".into(),
        button: Some((
            if state.prompt_request.is_some() {
                "Loading…"
            } else {
                "Refresh"
            }
            .into(),
            Tap::Stay(act(Act::Refresh)),
        )),
    });
    if let Some(error) = &state.error {
        rows.push(panel::text(error.clone(), Style::Caption, p.error));
    }
    match &state.prompt {
        Some((id, text)) if matches(id) => {
            let lines: Vec<&str> = text.lines().collect();
            let preview = lines
                .iter()
                .take(PREVIEW_LINES)
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            rows.push(panel::text(preview, Style::Small, p.text));
            if lines.len() > PREVIEW_LINES {
                rows.push(panel::text(
                    format!("… {} more lines", lines.len() - PREVIEW_LINES),
                    Style::Meta,
                    p.muted,
                ));
            }
            rows.push(Row::Buttons(vec![Btn::new(
                "Read in full",
                Kind::Secondary,
                Some(Tap::Stay(Action::Ui(UiAct::Read {
                    title: "Effective system prompt".into(),
                    text: text.clone(),
                }))),
            )]));
        }
        _ => rows.push(panel::note(if model.ready {
            "Loading the effective system prompt…"
        } else {
            "Connect to load the system prompt."
        })),
    }
    Built {
        title: "System prompt".into(),
        rows,
        wake: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;

    #[test]
    fn the_panel_loads_the_prompt_and_saves_instructions() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        let sent = model.load_system(Some("c"));
        assert_eq!(sent[0]["type"], "get_system_prompt");
        assert_eq!(sent[1]["type"], "load_instructions");
        let (p, i) = (
            sent[0]["reqId"].as_str().unwrap().to_owned(),
            sent[1]["reqId"].as_str().unwrap().to_owned(),
        );
        model.apply(
            parse_event(&format!(
                r#"{{"type":"system_prompt","reqId":"{p}","systemPrompt":"You are\nhelpful"}}"#
            )),
            Instant::now(),
        );
        model.apply(
            parse_event(&format!(r#"{{"type":"instructions_loaded","reqId":"{i}","convId":"c","conversation":"Be brief","global":""}}"#)),
            Instant::now(),
        );
        assert_eq!(
            model.conversation_instructions("c").as_deref(),
            Some("Be brief")
        );
        let built = build(&model, Some("c"));
        assert!(
            built
                .rows
                .iter()
                .any(|r| matches!(r, Row::Text { text, .. } if text == "You are\nhelpful"))
        );
        let save = model.handle(Action::Ops(ops::Act::System(Act::Save("Be kind".into()))));
        assert_eq!(save[0]["type"], "set_system_instructions");
        assert_eq!(save[0]["text"], "Be kind");
        let id = save[0]["reqId"].as_str().unwrap().to_owned();
        let reload = model.apply(
            parse_event(&format!(r#"{{"type":"ack","reqId":"{id}","convId":"c"}}"#)),
            Instant::now(),
        );
        assert_eq!(
            reload[0]["type"], "get_system_prompt",
            "reloaded after saving"
        );
    }
}
