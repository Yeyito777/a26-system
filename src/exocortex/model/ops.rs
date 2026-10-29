//! Models, settings and the operations panels (WP-E): the daemon's
//! providers, models and efforts, tool styles and defaults; what a
//! conversation or a draft runs with (a draft keeps only its explicit
//! overrides, PAR-277); the model inspector's form (PAR-276); what the chat
//! header says about the open conversation (retries, waits, context); and
//! the settings the Settings page changes.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::proto::{
    self, DefaultsSnapshot, Event, ExternalToolStyle, Provider, Selection, Speed, ToolStyle,
    ops::Model as ModelInfo,
};
use super::super::requests::{self, Owner, Pending};
use super::super::store::{self, Settings};
use super::super::theme;
use super::super::ui::Haptic;
use super::super::ui::sheet::{ActionItem, ActionList, action_sheet};
use super::{Effect, Model, Nav};

/// What the daemon said about models and tools, and this area's state.
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
    /// The daemon's defaults for new conversations (display-only and
    /// connection-scoped, PAR-278): None until it sends them.
    pub defaults: Option<DefaultsSnapshot>,
    /// The model inspector's form while it is open.
    pub form: Option<Form>,
    /// What the header shows of conversations this client is subscribed to.
    pub live: HashMap<String, LiveStatus>,
    /// The switch flipped last and when (its knob slides).
    pub flipped: Option<(Toggle, Instant)>,
}

/// A conversation's state as the header shows it, from its stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveStatus {
    /// The last known input tokens ("1.0k ctx").
    pub context_tokens: Option<u64>,
    /// A failed request is being retried (`stream_retry`).
    pub retrying: bool,
    /// The turn waits for connectivity (`connection_wait_status`).
    pub waiting: bool,
    /// Provider, model, effort and speed as its window said (the list's
    /// summary comes first).
    pub selection: Option<Selection>,
}

/// What a conversation or a draft runs with (`ResolvedGeneration`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Resolved {
    pub provider: String,
    /// Empty: not known yet (no defaults and no override).
    pub model: String,
    pub effort: String,
    pub speed: Speed,
    /// A draft with no overrides: the daemon's defaults apply.
    pub inherited: bool,
}

/// A draft's explicit overrides; what it lacks comes from the daemon's
/// defaults (PAR-277). Only these go on `new_conversation`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DraftConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `false`, `true` or `"ultrafast"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub delegation: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub legacy: bool,
}

impl DraftConfig {
    pub fn speed(&self) -> Option<Speed> {
        self.fast_mode.as_ref().map(Speed::of)
    }

    /// No model selection overridden.
    pub fn inherits(&self) -> bool {
        self.provider.is_none()
            && self.model.is_none()
            && self.effort.is_none()
            && self.fast_mode.is_none()
    }

    fn is_empty(&self) -> bool {
        self.inherits() && !self.delegation && !self.legacy
    }
}

/// `ui.json`'s `generation` section: the drafts' overrides by draft key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Kept {
    schema_version: u32,
    drafts: BTreeMap<String, DraftConfig>,
}

const KEPT: &str = "generation";

/// The model inspector's form (PAR-276): what Apply would set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Form {
    /// The conversation it is for; None: the draft `draft`.
    pub conv: Option<String>,
    pub draft: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub speed: Speed,
}

/// The inspector's pickers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Provider,
    Model,
    Effort,
}

/// Settings' switches (`settings.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Toggle {
    ExpandThinking,
    ExpandToolCalls,
    ShowAllWork,
    FoldMarkers,
    Haptics,
}

impl Toggle {
    pub const APPEARANCE: [Toggle; 5] = [
        Toggle::ExpandThinking,
        Toggle::ExpandToolCalls,
        Toggle::ShowAllWork,
        Toggle::FoldMarkers,
        Toggle::Haptics,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Toggle::ExpandThinking => "Expand thinking",
            Toggle::ExpandToolCalls => "Expand tool calls",
            Toggle::ShowAllWork => "Show all work",
            Toggle::FoldMarkers => "Fold markers",
            Toggle::Haptics => "Haptics",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Toggle::ExpandThinking => "Show reasoning open by default",
            Toggle::ExpandToolCalls => "Show tool output open by default",
            Toggle::ShowAllWork => {
                "Every turn's tool calls and every notification, not just the replies"
            }
            Toggle::FoldMarkers => {
                "A thin mark at the left edge where work is folded; tap it to open that spot"
            }
            Toggle::Haptics => "",
        }
    }

    pub fn get(self, settings: &Settings) -> bool {
        match self {
            Toggle::ExpandThinking => settings.expand_thinking,
            Toggle::ExpandToolCalls => settings.expand_tool_calls,
            Toggle::ShowAllWork => settings.show_all_work,
            Toggle::FoldMarkers => settings.fold_markers,
            Toggle::Haptics => settings.haptics,
        }
    }

    fn slot(self, settings: &mut Settings) -> &mut bool {
        match self {
            Toggle::ExpandThinking => &mut settings.expand_thinking,
            Toggle::ExpandToolCalls => &mut settings.expand_tool_calls,
            Toggle::ShowAllWork => &mut settings.show_all_work,
            Toggle::FoldMarkers => &mut settings.fold_markers,
            Toggle::Haptics => &mut settings.haptics,
        }
    }
}

/// Screens and choices of this area. Other areas open them with these
/// (D's `/model`, C2's menus): `OpenInspector`, `OpenSettings`, ….
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)] // The panels are E's; D's commands open them.
pub enum Act {
    /// The model inspector (the chat title, the new chat's model box).
    OpenInspector,
    /// The chat header's ⋯ menu.
    ChatMenu,
    /// Sets the conversation's model now (a draft's: locally), as `/model`.
    PickModel(String, String),
    PickEffort(String),
    PickSpeed(Speed),
    /// The inspector: a picker's choices.
    Choose(Field),
    /// The inspector's form.
    FormProvider(String),
    FormModel(String),
    FormEffort(String),
    FormSpeed(Speed),
    /// "Apply configuration" (a conversation) / "Use for this draft".
    Apply,
    /// "Use daemon defaults for this draft".
    InheritDefaults,
    /// Settings → Appearance.
    SetTheme(&'static str),
    Toggle(Toggle),
    OpenSettings,
    OpenGoal,
    OpenActivity,
    OpenChrono,
    OpenBtw,
    OpenAccounts,
    OpenSystemPrompt,
    OpenFind,
}

// ----- The catalog (PAR-275; the iPhone's `state/models.ts`) ---------------------

pub fn provider_info<'a>(providers: &'a [Provider], id: &str) -> Option<&'a Provider> {
    providers.iter().find(|p| p.id == id)
}

pub fn model_info<'a>(
    providers: &'a [Provider],
    provider: &str,
    model: &str,
) -> Option<&'a ModelInfo> {
    provider_info(providers, provider).and_then(|p| p.models.iter().find(|m| m.id == model))
}

/// The daemon's fallback effort when only the ids are known
/// (`defaultEffortForModelId`).
fn effort_for_ids(provider: &str, model: &str) -> &'static str {
    match provider {
        "openrouter" if model.starts_with("nousresearch/hermes-4-") => "high",
        "openrouter" => "none",
        "openai" if matches!(model, "gpt-6-astra" | "gpt-6.1-sol") => "low",
        "openai"
            if matches!(model, "gpt-6-sol" | "gpt-6-sol-daybreak" | "gpt-6-luna")
                || model.starts_with("gpt-5.6-")
                || model == "gpt-5.5"
                || model.starts_with("gpt-5.5-") =>
        {
            "medium"
        }
        _ => "high",
    }
}

/// `effort` when the model supports it, else its default (else its first
/// effort).
pub fn normalize_effort(
    providers: &[Provider],
    provider: &str,
    model: &str,
    effort: &str,
) -> String {
    let Some(info) = model_info(providers, provider, model) else {
        return if effort.is_empty() {
            effort_for_ids(provider, model).into()
        } else {
            effort.into()
        };
    };
    let supported = |e: &str| info.supported_efforts.iter().any(|s| s.effort == e);
    if !effort.is_empty() && supported(effort) {
        effort.into()
    } else if supported(&info.default_effort) {
        info.default_effort.clone()
    } else {
        info.supported_efforts
            .first()
            .map(|e| e.effort.clone())
            .unwrap_or_else(|| info.default_effort.clone())
    }
}

/// Whether `speed` is available: the provider's fast tier and the model's
/// flag (default yes); Ultrafast only when the model advertises it.
pub fn supports_speed(providers: &[Provider], provider: &str, model: &str, speed: Speed) -> bool {
    if speed == Speed::Standard {
        return true;
    }
    if !provider_info(providers, provider).is_some_and(|p| p.supports_fast_mode) {
        return false;
    }
    let info = model_info(providers, provider, model);
    match speed {
        Speed::Ultrafast => info.is_some_and(|m| m.supports_ultrafast_mode),
        _ => info.is_none_or(|m| m.supports_fast_mode != Some(false)),
    }
}

/// The speeds the inspector offers for a model (and `current`, so an
/// unsupported value stays visible).
pub fn speeds(providers: &[Provider], provider: &str, model: &str, current: Speed) -> Vec<Speed> {
    [Speed::Standard, Speed::Fast, Speed::Ultrafast]
        .into_iter()
        .filter(|&s| s == current || supports_speed(providers, provider, model, s))
        .collect()
}

/// "1.0k", "12k", "1.2M" (the iPhone's `formatTokens`).
pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        if n >= 10_000_000 {
            format!("{m:.0}M")
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1000 {
        let k = n as f64 / 1000.0;
        if n >= 100_000 {
            format!("{k:.0}k")
        } else {
            format!("{k:.1}k")
        }
    } else {
        n.to_string()
    }
}

/// 123456 → "123,456".
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The daemon's defaults as the inspector and Settings show them.
pub fn format_defaults(snapshot: &DefaultsSnapshot) -> String {
    let d = &snapshot.defaults;
    format!(
        "{}:\nProvider: {}\nModel: {}\nEffort: {}\nSpeed: {}",
        if snapshot.configured {
            "User default"
        } else {
            "App default"
        },
        d.provider,
        d.model,
        d.effort,
        d.speed().label()
    )
}

impl Model {
    // ----- What runs where -----------------------------------------------------------

    /// The draft the composer writes when no conversation is open: one per
    /// folder the new conversation goes into (PAR-192).
    pub fn draft_key(&self) -> String {
        format!("draft:{}", self.current_folder().unwrap_or("root"))
    }

    pub fn draft_config(&self, key: &str) -> DraftConfig {
        self.persist
            .section::<Kept>(store::UI, KEPT)
            .drafts
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    fn set_draft_config(&mut self, key: &str, config: DraftConfig) {
        let mut kept: Kept = self.persist.section(store::UI, KEPT);
        kept.schema_version = 2;
        if config.is_empty() {
            kept.drafts.remove(key);
        } else {
            kept.drafts.insert(key.to_owned(), config);
        }
        self.persist.set_section(store::UI, KEPT, &kept);
    }

    /// What the open conversation (or the draft) runs with.
    pub fn resolve(&self) -> Resolved {
        match &self.chat.id {
            Some(id) => self.resolve_conversation(id),
            None => self.resolve_draft(&self.draft_key()),
        }
    }

    /// The list's summary first, then what its window said.
    fn resolve_conversation(&self, id: &str) -> Resolved {
        let summary = self.sidebar.conversations.get(id);
        let window = self.ops.live.get(id).and_then(|l| l.selection.as_ref());
        let pick = |a: Option<&String>, b: Option<&String>| {
            a.filter(|s| !s.is_empty())
                .or(b.filter(|s| !s.is_empty()))
                .cloned()
                .unwrap_or_default()
        };
        let speed = match summary.map(|s| &s.fast_mode) {
            Some(value) if !value.is_null() => Speed::of(value),
            _ => window.map_or(Speed::Standard, Selection::speed),
        };
        Resolved {
            provider: pick(summary.map(|s| &s.provider), window.map(|w| &w.provider)),
            model: pick(summary.map(|s| &s.model), window.map(|w| &w.model)),
            effort: pick(summary.map(|s| &s.effort), window.map(|w| &w.effort)),
            speed,
            inherited: false,
        }
    }

    /// A draft: its overrides over the daemon's defaults. A provider or
    /// model override never borrows another model's effort or speed; those
    /// come from the catalog instead.
    fn resolve_draft(&self, key: &str) -> Resolved {
        let o = self.draft_config(key);
        let providers = &self.ops.providers;
        let d = self.ops.defaults.as_ref().map(|s| &s.defaults);
        let same = d.is_some_and(|d| {
            o.provider.as_ref().is_none_or(|p| *p == d.provider)
                && o.model.as_ref().is_none_or(|m| *m == d.model)
        });
        let provider = o
            .provider
            .clone()
            .or_else(|| d.map(|d| d.provider.clone()))
            .unwrap_or_default();
        let model = match &o.model {
            Some(model) => model.clone(),
            None if same || o.provider.is_none() => d.map(|d| d.model.clone()).unwrap_or_default(),
            None => provider_info(providers, &provider)
                .map(|p| p.default_model.clone())
                .unwrap_or_default(),
        };
        let mut effort = o
            .effort
            .clone()
            .or_else(|| d.filter(|_| same).map(|d| d.effort.clone()))
            .unwrap_or_default();
        if effort.is_empty() && !provider.is_empty() && !model.is_empty() {
            effort = normalize_effort(providers, &provider, &model, "");
        }
        let speed = o
            .speed()
            .or_else(|| d.filter(|_| same).map(Selection::speed))
            .unwrap_or_default();
        Resolved {
            provider,
            model,
            effort,
            speed,
            inherited: o.inherits(),
        }
    }

    /// Provider, model and effort of the chat (or the new conversation's).
    pub fn selection(&self) -> Selection {
        let r = self.resolve();
        Selection {
            provider: r.provider,
            model: r.model,
            effort: r.effort,
            fast_mode: r.speed.value(),
        }
    }

    #[allow(dead_code)] // D's commands and A's tests.
    pub fn model_label(&self, selection: &Selection) -> String {
        self.label_of(&selection.provider, &selection.model)
    }

    /// The catalog's label, else the raw id (PAR-279).
    fn label_of(&self, provider: &str, model: &str) -> String {
        model_info(&self.ops.providers, provider, model)
            .map_or_else(|| model.to_owned(), |m| m.label.clone())
    }

    /// "Opus 5.5 · high · fast", or "Daemon default" for a blank draft
    /// before the defaults are known.
    pub fn generation_label(&self, r: &Resolved, effort: bool) -> String {
        if r.model.is_empty() {
            return "Daemon default".into();
        }
        let mut label = self.label_of(&r.provider, &r.model);
        if effort {
            if !r.effort.is_empty() {
                label = format!("{label} · {}", r.effort);
            }
            match r.speed {
                Speed::Fast => label.push_str(" · fast"),
                Speed::Ultrafast => label.push_str(" · ultrafast"),
                Speed::Standard => {}
            }
        }
        label
    }

    /// The new chat's model box: "Daemon default model", or the label with
    /// " · custom" when the draft overrides the defaults.
    pub fn new_chat_label(&self) -> String {
        let r = self.resolve_draft(&self.draft_key());
        if r.model.is_empty() {
            return "Daemon default model".into();
        }
        let label = self.generation_label(&r, true);
        if r.inherited {
            label
        } else {
            format!("{label} · custom")
        }
    }

    /// The model fields of a `new_conversation` for the draft: only its
    /// explicit overrides (PAR-192, 277), so a blank draft gets the
    /// daemon's defaults. Replaces whatever `command` had for them.
    pub fn new_conversation_fields(&self, command: &mut Value) {
        let Some(object) = command.as_object_mut() else {
            return;
        };
        for key in [
            "provider",
            "model",
            "effort",
            "fastMode",
            "delegation",
            "legacy",
        ] {
            object.remove(key);
        }
        let o = self.draft_config(&self.draft_key());
        let fields = [
            ("provider", o.provider.map(Value::String)),
            ("model", o.model.map(Value::String)),
            ("effort", o.effort.map(Value::String)),
            ("fastMode", o.fast_mode),
            ("delegation", o.delegation.then_some(json!(true))),
            ("legacy", o.legacy.then_some(json!(true))),
        ];
        for (key, value) in fields {
            if let Some(value) = value {
                object.insert(key.into(), value);
            }
        }
    }

    /// The open conversation's last known input tokens.
    pub fn context_tokens(&self) -> Option<u64> {
        self.chat
            .id
            .as_ref()
            .and_then(|id| self.ops.live.get(id))
            .and_then(|l| l.context_tokens)
    }

    /// The open conversation's stream state for the header.
    pub fn live_status(&self) -> Option<&LiveStatus> {
        self.chat.id.as_ref().and_then(|id| self.ops.live.get(id))
    }

    /// The inspector's form as it opens: what runs now.
    fn initial_form(&self) -> Form {
        let r = self.resolve();
        Form {
            conv: self.chat.id.clone(),
            draft: self.draft_key(),
            provider: r.provider,
            model: r.model,
            effort: r.effort,
            speed: r.speed,
        }
    }

    /// The form, (re)made when it is for another conversation.
    pub fn form(&self) -> Form {
        match &self.ops.form {
            Some(form) if form.conv == self.chat.id => form.clone(),
            _ => self.initial_form(),
        }
    }

    /// Why Apply cannot be pressed now, if it cannot.
    pub fn apply_blocked(&self, form: &Form) -> Option<&'static str> {
        if form.provider.is_empty() || form.model.is_empty() || form.effort.is_empty() {
            return Some("Choose a provider, a model and an effort.");
        }
        if form.conv.is_some() {
            if self.streaming() {
                return Some("Stop generation to change the model.");
            }
            if !self.ready {
                return Some("Not connected to Exocortex.");
            }
        }
        None
    }

    /// Whether the form says what already runs.
    pub fn form_unchanged(&self, form: &Form) -> bool {
        let r = self.resolve();
        form.provider == r.provider
            && form.model == r.model
            && form.effort == r.effort
            && form.speed == r.speed
    }

    // ----- Actions -------------------------------------------------------------------

    pub(super) fn handle_ops(&mut self, act: Act) -> Vec<Value> {
        match act {
            Act::OpenInspector => {
                self.ops.form = Some(self.initial_form());
                // Its pickers and buttons want the whole sheet.
                self.ui.effect(Effect::HideKeyboard);
                self.ui.haptic(Haptic::Light);
                self.ui
                    .open(Box::new(super::super::sheets::inspector::inspector()));
                Vec::new()
            }
            Act::ChatMenu => {
                self.ui.effect(Effect::HideKeyboard);
                let sheet = super::super::view::chat_header::chat_menu(self);
                self.ui.open(Box::new(sheet));
                Vec::new()
            }
            Act::PickModel(provider, model) => self.pick_model(provider, model),
            Act::PickEffort(effort) => self.pick_effort(effort),
            Act::PickSpeed(speed) => self.pick_speed(speed),
            Act::Choose(field) => {
                self.open_choices(field);
                Vec::new()
            }
            Act::FormProvider(id) => {
                let mut form = self.form();
                let first = provider_info(&self.ops.providers, &id)
                    .and_then(|p| p.models.first())
                    .map(|m| m.id.clone());
                form.provider = id;
                match first {
                    Some(model) => self.form_model(&mut form, model),
                    None => {
                        form.model.clear();
                        form.effort.clear();
                        form.speed = Speed::Standard;
                    }
                }
                self.ui.haptic(Haptic::Selection);
                self.ops.form = Some(form);
                Vec::new()
            }
            Act::FormModel(model) => {
                let mut form = self.form();
                self.form_model(&mut form, model);
                self.ui.haptic(Haptic::Selection);
                self.ops.form = Some(form);
                Vec::new()
            }
            Act::FormEffort(effort) => {
                let mut form = self.form();
                form.effort = effort;
                self.ui.haptic(Haptic::Selection);
                self.ops.form = Some(form);
                Vec::new()
            }
            Act::FormSpeed(speed) => {
                let mut form = self.form();
                form.speed = speed;
                self.ui.haptic(Haptic::Selection);
                self.ops.form = Some(form);
                Vec::new()
            }
            Act::Apply => self.apply_form(),
            Act::InheritDefaults => {
                let key = self.draft_key();
                let o = self.draft_config(&key);
                self.set_draft_config(
                    &key,
                    DraftConfig {
                        delegation: o.delegation,
                        legacy: o.legacy,
                        ..DraftConfig::default()
                    },
                );
                self.ops.form = Some(self.initial_form());
                self.ui.haptic(Haptic::Light);
                Vec::new()
            }
            Act::SetTheme(id) => {
                if self.persist.settings().theme != id {
                    self.persist.update_settings(|s| s.theme = id.into());
                }
                theme::set_current(theme::by_id(id));
                self.ui.effect(Effect::StatusBar);
                self.ui.haptic(Haptic::Selection);
                Vec::new()
            }
            Act::Toggle(toggle) => {
                self.persist.update_settings(|s| {
                    let slot = toggle.slot(s);
                    *slot = !*slot;
                });
                self.ops.flipped = Some((toggle, Instant::now()));
                self.ui.haptic(Haptic::Light);
                Vec::new()
            }
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

    /// The form on `model` of its provider: an unsupported effort becomes
    /// the model's default, an unsupported speed Standard (PAR-276).
    fn form_model(&self, form: &mut Form, model: String) {
        let providers = &self.ops.providers;
        if let Some(info) = model_info(providers, &form.provider, &model)
            && !info
                .supported_efforts
                .iter()
                .any(|e| e.effort == form.effort)
        {
            form.effort = normalize_effort(providers, &form.provider, &model, "");
        }
        if !supports_speed(providers, &form.provider, &model, form.speed) {
            form.speed = Speed::Standard;
        }
        form.model = model;
    }

    fn issue(&mut self, kind: &'static str, conv: &str) -> String {
        self.requests.issue(
            Owner::Ops,
            kind,
            Some(conv),
            requests::DEFAULT_WITHIN,
            Instant::now(),
        )
    }

    /// Changes the draft's overrides with `change` (nothing on the wire,
    /// PAR-280).
    fn patch_draft(&mut self, change: impl FnOnce(&mut DraftConfig)) {
        let key = self.draft_key();
        let mut config = self.draft_config(&key);
        change(&mut config);
        self.set_draft_config(&key, config);
    }

    fn pick_model(&mut self, provider: String, model: String) -> Vec<Value> {
        match (self.chat.id.clone(), self.connected) {
            (Some(id), true) => {
                let request = self.issue("set_model", &id);
                vec![proto::set_model(&request, &id, &provider, &model)]
            }
            (Some(_), false) => Vec::new(),
            (None, _) => {
                // As the daemon's set_model: effort and speed carried over
                // when the new model supports them.
                let r = self.resolve();
                let effort = normalize_effort(&self.ops.providers, &provider, &model, &r.effort);
                let speed = if supports_speed(&self.ops.providers, &provider, &model, r.speed) {
                    r.speed
                } else {
                    Speed::Standard
                };
                self.patch_draft(|o| {
                    o.provider = Some(provider);
                    o.model = Some(model);
                    o.effort = Some(effort);
                    o.fast_mode = Some(speed.value());
                });
                Vec::new()
            }
        }
    }

    fn pick_effort(&mut self, effort: String) -> Vec<Value> {
        match (self.chat.id.clone(), self.connected) {
            (Some(id), true) => {
                let request = self.issue("set_effort", &id);
                vec![proto::set_effort(&request, &id, &effort)]
            }
            (Some(_), false) => Vec::new(),
            (None, _) => {
                self.patch_draft(|o| o.effort = Some(effort));
                Vec::new()
            }
        }
    }

    fn pick_speed(&mut self, speed: Speed) -> Vec<Value> {
        match (self.chat.id.clone(), self.connected) {
            (Some(id), true) => {
                let request = self.issue("set_fast_mode", &id);
                vec![proto::set_fast_mode(&request, &id, speed)]
            }
            (Some(_), false) => Vec::new(),
            (None, _) => {
                self.patch_draft(|o| o.fast_mode = Some(speed.value()));
                Vec::new()
            }
        }
    }

    /// Apply: a conversation gets `set_model`, `set_effort`, `set_fast_mode`
    /// for what changed, in that order (the daemon takes one
    /// conversation's commands in arrival order); a draft keeps what
    /// differs from the daemon's defaults.
    fn apply_form(&mut self) -> Vec<Value> {
        let form = self.form();
        self.ops.form = None;
        if let Some(reason) = self.apply_blocked(&form) {
            self.ui.error_toast(reason);
            return Vec::new();
        }
        let Some(id) = form.conv.clone() else {
            let same = self.ops.defaults.as_ref().is_some_and(|s| {
                let d = &s.defaults;
                d.provider == form.provider
                    && d.model == form.model
                    && d.effort == form.effort
                    && d.speed() == form.speed
            });
            let key = form.draft.clone();
            let kept = self.draft_config(&key);
            let config = if same {
                DraftConfig {
                    delegation: kept.delegation,
                    legacy: kept.legacy,
                    ..DraftConfig::default()
                }
            } else {
                DraftConfig {
                    provider: Some(form.provider),
                    model: Some(form.model),
                    effort: Some(form.effort),
                    fast_mode: Some(form.speed.value()),
                    delegation: kept.delegation,
                    legacy: kept.legacy,
                }
            };
            self.set_draft_config(&key, config);
            self.ui.haptic(Haptic::Light);
            return Vec::new();
        };
        let r = self.resolve();
        let providers = &self.ops.providers;
        let model_changes = form.provider != r.provider || form.model != r.model;
        // What the daemon makes of effort and speed on set_model.
        let (effort, speed) = if model_changes {
            let effort = normalize_effort(providers, &form.provider, &form.model, &r.effort);
            let speed = if supports_speed(providers, &form.provider, &form.model, r.speed) {
                r.speed
            } else {
                Speed::Standard
            };
            (effort, speed)
        } else {
            (r.effort, r.speed)
        };
        let mut commands = Vec::new();
        if model_changes {
            let request = self.issue("set_model", &id);
            commands.push(proto::set_model(&request, &id, &form.provider, &form.model));
        }
        if form.effort != effort {
            let request = self.issue("set_effort", &id);
            commands.push(proto::set_effort(&request, &id, &form.effort));
        }
        if form.speed != speed {
            let request = self.issue("set_fast_mode", &id);
            commands.push(proto::set_fast_mode(&request, &id, form.speed));
        }
        self.ui.haptic(Haptic::Light);
        commands
    }

    /// A picker's choices, as an action list over the inspector.
    fn open_choices(&mut self, field: Field) {
        let form = self.form();
        let providers = &self.ops.providers;
        let (title, items) = match field {
            Field::Provider => {
                let mut items: Vec<ActionItem> = Vec::new();
                if !form.provider.is_empty() && provider_info(providers, &form.provider).is_none() {
                    items.push(
                        ActionItem::new(
                            form.provider.clone(),
                            super::Action::Ops(Act::FormProvider(form.provider.clone())),
                        )
                        .checked(true),
                    );
                }
                for p in providers {
                    let signed_in = self.ops.auth.get(&p.id).copied().unwrap_or(false);
                    let mut item = ActionItem::new(
                        if p.label.is_empty() { &p.id } else { &p.label },
                        super::Action::Ops(Act::FormProvider(p.id.clone())),
                    )
                    .checked(p.id == form.provider);
                    if !signed_in {
                        item = item.detail("Not signed in");
                    }
                    items.push(item);
                }
                ("Provider", items)
            }
            Field::Model => {
                let mut items: Vec<ActionItem> = Vec::new();
                if !form.model.is_empty()
                    && model_info(providers, &form.provider, &form.model).is_none()
                {
                    items.push(
                        ActionItem::new(
                            form.model.clone(),
                            super::Action::Ops(Act::FormModel(form.model.clone())),
                        )
                        .checked(true),
                    );
                }
                if let Some(p) = provider_info(providers, &form.provider) {
                    for m in &p.models {
                        let label = if m.supports_tools == Some(false) {
                            format!("{} · chat only", m.label)
                        } else {
                            m.label.clone()
                        };
                        items.push(
                            ActionItem::new(
                                label,
                                super::Action::Ops(Act::FormModel(m.id.clone())),
                            )
                            .detail(m.id.clone())
                            .checked(m.id == form.model),
                        );
                    }
                }
                ("Model", items)
            }
            Field::Effort => {
                let info = model_info(providers, &form.provider, &form.model);
                let efforts: Vec<(String, String)> = info.map_or_else(Vec::new, |m| {
                    m.supported_efforts
                        .iter()
                        .map(|e| (e.effort.clone(), e.description.clone()))
                        .collect()
                });
                let mut items: Vec<ActionItem> = Vec::new();
                if !form.effort.is_empty() && !efforts.iter().any(|(e, _)| *e == form.effort) {
                    items.push(
                        ActionItem::new(
                            form.effort.clone(),
                            super::Action::Ops(Act::FormEffort(form.effort.clone())),
                        )
                        .checked(true),
                    );
                }
                for (effort, description) in efforts {
                    let default = info.is_some_and(|m| m.default_effort == effort);
                    let label = if default {
                        format!("{effort} (default)")
                    } else {
                        effort.clone()
                    };
                    let mut item =
                        ActionItem::new(label, super::Action::Ops(Act::FormEffort(effort.clone())))
                            .checked(effort == form.effort);
                    if !description.is_empty() && description != effort {
                        item = item.detail(description);
                    }
                    items.push(item);
                }
                ("Effort", items)
            }
        };
        self.ui.open(Box::new(action_sheet(
            ActionList::new(title, items).scoped(),
        )));
    }

    // ----- Events --------------------------------------------------------------------

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
                    self.ops.defaults = Some(snapshot.clone());
                }
            }
            Event::ConversationDefaults {
                defaults,
                configured,
                ..
            } => {
                self.ops.defaults = Some(DefaultsSnapshot {
                    defaults: defaults.clone(),
                    configured: *configured,
                });
            }
            Event::ConversationLoaded {
                conv_id,
                provider,
                model,
                effort,
                fast_mode,
                window,
                ..
            } => {
                let live = self.ops.live.entry(conv_id.clone()).or_default();
                live.selection = Some(Selection {
                    provider: provider.clone(),
                    model: model.clone(),
                    effort: effort.clone(),
                    fast_mode: fast_mode.clone(),
                });
                if window.context_tokens.is_some() {
                    live.context_tokens = window.context_tokens;
                }
                if matches!(window.pending_ai, None | Some(None)) {
                    live.retrying = false;
                    live.waiting = false;
                }
            }
            Event::ConversationHistoryLoaded {
                conv_id, window, ..
            }
            | Event::HistoryUpdated { conv_id, window } => {
                if let Some(live) = self.ops.live.get_mut(conv_id) {
                    if window.context_tokens.is_some() {
                        live.context_tokens = window.context_tokens;
                    }
                    if window.pending_ai == Some(None) {
                        live.retrying = false;
                        live.waiting = false;
                    }
                }
            }
            Event::ConversationUnwound {
                conv_id,
                context_tokens: Some(tokens),
                ..
            }
            | Event::ContextUpdate {
                conv_id,
                context_tokens: tokens,
            } => {
                self.ops
                    .live
                    .entry(conv_id.clone())
                    .or_default()
                    .context_tokens = Some(*tokens);
            }
            Event::StreamRetry { conv_id, .. } => {
                self.ops.live.entry(conv_id.clone()).or_default().retrying = true;
            }
            Event::ConnectionWaitStatus {
                conv_id, active, ..
            } => {
                self.ops.live.entry(conv_id.clone()).or_default().waiting = *active;
            }
            Event::TextChunk { conv_id, .. } | Event::ThinkingChunk { conv_id, .. } => {
                if let Some(live) = self.ops.live.get_mut(conv_id) {
                    live.retrying = false;
                }
            }
            Event::StreamingStarted {
                conv_id,
                snapshot_kind,
                ..
            } if snapshot_kind.as_deref().is_none_or(|k| k == "start") => {
                if let Some(live) = self.ops.live.get_mut(conv_id) {
                    live.retrying = false;
                }
            }
            Event::MessageComplete { conv_id, .. } | Event::StreamingStopped { conv_id, .. } => {
                if let Some(live) = self.ops.live.get_mut(conv_id) {
                    live.retrying = false;
                    live.waiting = false;
                }
            }
            Event::ConversationDeleted { conv_id, .. } => {
                self.ops.live.remove(conv_id);
            }
            _ => {}
        }
        Vec::new()
    }

    /// The connection dropped: its defaults and stream states are stale
    /// (PAR-023).
    pub(super) fn ops_disconnected(&mut self) {
        self.ops.defaults = None;
        self.ops.live.clear();
    }

    /// An error answering one of this area's requests; true when handled.
    pub(super) fn ops_error(&mut self, _id: &str, _message: &str) -> bool {
        false
    }

    /// One of this area's requests was not answered in time.
    pub(super) fn ops_timeout(&mut self, _id: &str, pending: &Pending) -> bool {
        match pending.kind {
            "set_model" | "set_effort" | "set_fast_mode" => {
                self.ui
                    .error_toast("Exocortex did not confirm the change. Try again.");
                true
            }
            _ => self.generic_timeout(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::{Action, composer};
    use crate::exocortex::proto::parse_event;

    const CATALOG: &str = r#"{"type":"tools_available","providers":[
        {"id":"anthropic","label":"Anthropic","defaultModel":"opus","supportsFastMode":true,"models":[
            {"id":"opus","label":"Opus 5.5","defaultEffort":"high","supportedEfforts":[{"effort":"low"},{"effort":"high"},{"effort":"xhigh"}],"supportsUltrafastMode":true,"maxContext":200000},
            {"id":"haiku","label":"Haiku","defaultEffort":"medium","supportedEfforts":[{"effort":"low"},{"effort":"medium"}],"supportsFastMode":false}]},
        {"id":"openai","label":"OpenAI","defaultModel":"sol","models":[
            {"id":"sol","label":"Sol","defaultEffort":"max","supportedEfforts":[{"effort":"medium"},{"effort":"high"}]}]}],
        "authByProvider":{"anthropic":true,"openai":true},"tools":[],
        "conversationDefaults":{"defaults":{"provider":"anthropic","model":"opus","effort":"xhigh","fastMode":false},"configured":true}}"#;

    fn ready() -> Model {
        let mut model = Model::new();
        let commands = model.on_connected();
        let ping = commands.last().unwrap()["reqId"]
            .as_str()
            .unwrap()
            .to_owned();
        let now = Instant::now();
        model.apply(
            parse_event(&format!(r#"{{"type":"pong","reqId":"{ping}"}}"#)),
            now,
        );
        model.apply(parse_event(&CATALOG.replace('\n', "")), now);
        model
    }

    fn with_conversation(model: &mut Model, fields: &str) {
        model.apply(
            parse_event(&format!(
                r#"{{"type":"conversation_updated","summary":{{"id":"c","title":"T",{fields}}}}}"#
            )),
            Instant::now(),
        );
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
    }

    #[test]
    fn a_provider_change_picks_its_first_model_and_normalises_effort_and_speed() {
        let mut model = ready();
        with_conversation(
            &mut model,
            r#""provider":"anthropic","model":"opus","effort":"xhigh","fastMode":"ultrafast""#,
        );
        model.handle(Action::Ops(Act::OpenInspector));
        let form = model.form();
        assert_eq!(
            (form.model.as_str(), form.effort.as_str(), form.speed),
            ("opus", "xhigh", Speed::Ultrafast)
        );
        // Haiku: xhigh unsupported → its default; no fast tier → Standard.
        model.handle(Action::Ops(Act::FormModel("haiku".into())));
        let form = model.form();
        assert_eq!(
            (form.effort.as_str(), form.speed),
            ("medium", Speed::Standard)
        );
        // OpenAI: its first model; max is its default but unsupported → its
        // first effort.
        model.handle(Action::Ops(Act::FormEffort("low".into())));
        model.handle(Action::Ops(Act::FormProvider("openai".into())));
        let form = model.form();
        assert_eq!(
            (form.model.as_str(), form.effort.as_str()),
            ("sol", "medium")
        );
        // A supported effort is kept.
        model.handle(Action::Ops(Act::FormProvider("anthropic".into())));
        model.handle(Action::Ops(Act::FormEffort("low".into())));
        model.handle(Action::Ops(Act::FormModel("haiku".into())));
        assert_eq!(model.form().effort, "low");
        assert_eq!(
            speeds(&model.ops.providers, "anthropic", "opus", Speed::Standard),
            vec![Speed::Standard, Speed::Fast, Speed::Ultrafast]
        );
        assert_eq!(
            speeds(&model.ops.providers, "openai", "sol", Speed::Standard),
            vec![Speed::Standard]
        );
    }

    #[test]
    fn apply_sends_what_changed_in_order() {
        let mut model = ready();
        with_conversation(
            &mut model,
            r#""provider":"anthropic","model":"opus","effort":"high","fastMode":false"#,
        );
        model.handle(Action::Ops(Act::OpenInspector));
        assert!(model.form_unchanged(&model.form()));
        model.handle(Action::Ops(Act::FormEffort("xhigh".into())));
        model.handle(Action::Ops(Act::FormSpeed(Speed::Fast)));
        let sent = model.handle(Action::Ops(Act::Apply));
        let kinds: Vec<&str> = sent.iter().map(|c| c["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["set_effort", "set_fast_mode"]);
        assert_eq!(sent[1]["enabled"], true);
        assert!(model.ops.form.is_none());
        // A model change first; an effort the new model takes as is is not
        // sent again.
        model.handle(Action::Ops(Act::OpenInspector));
        model.handle(Action::Ops(Act::FormModel("haiku".into())));
        model.handle(Action::Ops(Act::FormEffort("low".into())));
        let sent = model.handle(Action::Ops(Act::Apply));
        let kinds: Vec<&str> = sent.iter().map(|c| c["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["set_model", "set_effort"]);
        assert_eq!(sent[0]["model"], "haiku");
        assert_eq!(sent[1]["effort"], "low");
    }

    #[test]
    fn apply_waits_while_streaming() {
        let mut model = ready();
        with_conversation(
            &mut model,
            r#""provider":"anthropic","model":"opus","effort":"high","streaming":true"#,
        );
        model.handle(Action::Ops(Act::OpenInspector));
        model.handle(Action::Ops(Act::FormEffort("low".into())));
        assert_eq!(
            model.apply_blocked(&model.form()),
            Some("Stop generation to change the model.")
        );
        assert!(model.handle(Action::Ops(Act::Apply)).is_empty());
    }

    #[test]
    fn a_blank_draft_sends_no_model_and_overrides_are_sparse() {
        let mut model = ready();
        model.handle(Action::Nav(Nav::NewConversation));
        let r = model.resolve();
        assert_eq!(
            (r.model.as_str(), r.effort.as_str(), r.inherited),
            ("opus", "xhigh", true)
        );
        assert_eq!(model.new_chat_label(), "Opus 5.5 · xhigh");
        let mut command =
            json!({"type":"new_conversation","provider":"x","model":"y","effort":"z"});
        model.new_conversation_fields(&mut command);
        assert_eq!(command, json!({"type":"new_conversation"}));
        // Choosing the defaults again keeps the draft blank.
        model.handle(Action::Ops(Act::OpenInspector));
        model.handle(Action::Ops(Act::Apply));
        assert!(model.resolve().inherited);
        // Another model: every field, kept in ui.json.
        model.handle(Action::Ops(Act::OpenInspector));
        model.handle(Action::Ops(Act::FormModel("haiku".into())));
        model.handle(Action::Ops(Act::Apply));
        assert_eq!(model.new_chat_label(), "Haiku · medium · custom");
        let mut command = json!({"type":"new_conversation"});
        model.new_conversation_fields(&mut command);
        assert_eq!(command["model"], "haiku");
        assert_eq!(command["effort"], "medium");
        assert_eq!(command["fastMode"], false);
        let kept = model.persist.take_dirty();
        assert!(kept.iter().any(|(name, text)| *name == store::UI
            && text.contains("\"draft:root\"")
            && text.contains("\"schemaVersion\": 2")));
        // Another folder's draft is its own.
        model.sidebar.path.push("f".into());
        assert!(model.resolve().inherited);
        model.sidebar.path.clear();
        // "Use daemon defaults for this draft".
        model.handle(Action::Ops(Act::InheritDefaults));
        assert!(model.resolve().inherited);
        assert_eq!(model.draft_config("draft:root"), DraftConfig::default());
    }

    #[test]
    fn a_draft_override_of_the_provider_does_not_borrow_the_defaults_effort() {
        let mut model = ready();
        model.handle(Action::Nav(Nav::NewConversation));
        model.patch_draft(|o| o.provider = Some("openai".into()));
        let r = model.resolve();
        assert_eq!((r.model.as_str(), r.effort.as_str()), ("sol", "medium"));
        assert!(!r.inherited);
        // Before the defaults: "Daemon default".
        let mut blank = Model::new();
        blank.handle(Action::Nav(Nav::NewConversation));
        assert_eq!(
            blank.generation_label(&blank.resolve(), true),
            "Daemon default"
        );
        assert_eq!(blank.new_chat_label(), "Daemon default model");
    }

    #[test]
    fn the_sent_first_message_carries_only_the_overrides() {
        let mut model = ready();
        model.handle(Action::Nav(Nav::NewConversation));
        model.composer.text = "Hi".into();
        let sent = model.handle(Action::Composer(composer::Act::Send));
        assert_eq!(sent[0]["type"], "new_conversation");
        assert!(sent[0].get("model").is_none() && sent[0].get("provider").is_none());
    }

    #[test]
    fn the_header_learns_context_retries_and_waits_from_the_stream() {
        let mut model = ready();
        with_conversation(&mut model, r#""provider":"anthropic","model":"opus""#);
        let now = Instant::now();
        for line in [
            r#"{"type":"context_update","convId":"c","contextTokens":1234}"#,
            r#"{"type":"stream_retry","convId":"c","attempt":1,"maxAttempts":5,"errorMessage":"overloaded","delaySec":3}"#,
            r#"{"type":"connection_wait_status","convId":"c","active":true}"#,
        ] {
            model.apply(parse_event(line), now);
        }
        let live = model.live_status().unwrap();
        assert_eq!(live.context_tokens, Some(1234));
        assert!(live.retrying && live.waiting);
        model.apply(
            parse_event(r#"{"type":"text_chunk","convId":"c","text":"x"}"#),
            now,
        );
        assert!(!model.live_status().unwrap().retrying);
        model.apply(
            parse_event(r#"{"type":"streaming_stopped","convId":"c"}"#),
            now,
        );
        assert!(!model.live_status().unwrap().waiting);
        model.on_disconnected();
        assert!(model.live_status().is_none() && model.ops.defaults.is_none());
    }

    #[test]
    fn settings_switch_and_theme() {
        let mut model = ready();
        assert!(!model.persist.settings().expand_thinking);
        model.handle(Action::Ops(Act::Toggle(Toggle::ExpandThinking)));
        assert!(model.persist.settings().expand_thinking);
        assert_eq!(
            model.ops.flipped.map(|(t, _)| t),
            Some(Toggle::ExpandThinking)
        );
        model.handle(Action::Ops(Act::SetTheme("cerberus")));
        assert_eq!(model.persist.settings().theme, "cerberus");
        assert_eq!(theme::current().id, "cerberus");
        assert!(model.ui.effects.contains(&Effect::StatusBar));
        theme::set_current(&theme::WHALE);
    }

    #[test]
    fn numbers_read_as_on_the_iphone() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1000), "1.0k");
        assert_eq!(format_tokens(123_456), "123k");
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(grouped(123_456), "123,456");
        assert_eq!(grouped(200_000), "200,000");
        assert_eq!(grouped(12), "12");
    }
}
