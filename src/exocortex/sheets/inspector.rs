//! The model sheet (WP-E's inspector to be, PAR-275): the conversation's
//! effort as a segmented control, then the models of every signed-in
//! provider, the current one checked. Tapping one sets it (or, for a new
//! conversation, chooses it).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::gfx::{Canvas, RectF};

use super::super::model::{Action, Model, ops};
use super::super::theme::{self, HAIRLINE, T_BODY, T_SECTION, T_SMALL, TARGET};
use super::super::ui::icons::{self, Icon};
use super::super::ui::overlay::Cx;
use super::super::ui::sheet::{Reply, Sheet, SheetContent};
use super::super::ui::widgets;

const SECTION: f32 = 101.0;
const ROW: f32 = 133.0;

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Section(String),
    Efforts(Vec<String>),
    Model {
        provider: String,
        id: String,
        label: String,
    },
}

/// What the sheet shows, laid out top to bottom.
fn items(model: &Model) -> Vec<(f32, Item)> {
    let mut items = Vec::new();
    let mut y = 0.0;
    let selection = model.selection();
    let efforts: Vec<String> = model
        .ops
        .providers
        .iter()
        .find(|p| p.id == selection.provider)
        .and_then(|p| p.models.iter().find(|m| m.id == selection.model))
        .map(|m| {
            m.supported_efforts
                .iter()
                .map(|e| e.effort.clone())
                .collect()
        })
        .unwrap_or_default();
    if !efforts.is_empty() {
        items.push((y, Item::Section("Effort".into())));
        y += SECTION;
        items.push((y, Item::Efforts(efforts)));
        y += TARGET + 22.0;
    }
    // The conversation's own provider first.
    let mut providers = model.usable_providers();
    providers.sort_by_key(|p| p.id != selection.provider);
    for provider in providers {
        items.push((y, Item::Section(provider.label.clone())));
        y += SECTION;
        for m in &provider.models {
            items.push((
                y,
                Item::Model {
                    provider: provider.id.clone(),
                    id: m.id.clone(),
                    label: m.label.clone(),
                },
            ));
            y += ROW;
        }
    }
    items.push((y, Item::Section(String::new())));
    items
}

/// The model sheet's content.
pub struct Inspector {
    items: Vec<(f32, Item)>,
    selection: (String, String, String),
    width: f32,
    title: String,
}

/// The model sheet.
pub fn inspector() -> Sheet<Inspector> {
    Sheet::new(Inspector {
        items: Vec::new(),
        selection: Default::default(),
        width: 0.0,
        title: "Model".into(),
    })
}

impl Inspector {
    /// What is at content index `index`: an effort segment or a model.
    fn targets(&self) -> Vec<(RectF, Action)> {
        let mut targets = Vec::new();
        for (y, item) in &self.items {
            match item {
                Item::Efforts(efforts) => {
                    let w = self.width / efforts.len() as f32;
                    for (index, effort) in efforts.iter().enumerate() {
                        targets.push((
                            RectF::new(index as f32 * w, *y, w, TARGET),
                            Action::Ops(ops::Act::PickEffort(effort.clone())),
                        ));
                    }
                }
                Item::Model { provider, id, .. } => targets.push((
                    RectF::new(0.0, *y, self.width, ROW),
                    Action::Ops(ops::Act::PickModel(provider.clone(), id.clone())),
                )),
                Item::Section(_) => {}
            }
        }
        targets
    }
}

impl SheetContent for Inspector {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn layout(&mut self, cx: &Cx, width: f32) -> f32 {
        self.width = width;
        let selection = cx.model.selection();
        self.selection = (selection.provider, selection.model, selection.effort);
        self.title = match cx.model.open_summary() {
            Some(summary) => format!("{} model", summary.display_title()),
            None => "New chat model".into(),
        };
        self.items = items(cx.model);
        self.items.last().map_or(0.0, |(y, _)| *y)
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        let (provider, model, effort) = &self.selection;
        let mut target = 0;
        for (y, item) in &self.items {
            let y = top + y;
            match item {
                Item::Section(title) => {
                    if !title.is_empty() {
                        widgets::text(
                            canvas,
                            title,
                            x,
                            T_SECTION.baseline(y + 44.0),
                            T_SECTION,
                            p.accent,
                            1.0,
                        );
                    }
                }
                Item::Efforts(efforts) => {
                    let labels: Vec<&str> = efforts.iter().map(String::as_str).collect();
                    let selected = efforts
                        .iter()
                        .position(|e| e == effort)
                        .unwrap_or(usize::MAX);
                    let pressed_segment = pressed
                        .filter(|index| (target..target + efforts.len()).contains(index))
                        .map(|index| index - target);
                    widgets::segmented(
                        canvas,
                        RectF::new(x, y, self.width, TARGET),
                        &labels,
                        selected,
                        pressed_segment,
                    );
                    target += efforts.len();
                }
                Item::Model {
                    provider: p_id,
                    id,
                    label,
                } => {
                    if pressed == Some(target) {
                        widgets::fill(
                            canvas,
                            RectF::new(0.0, y, x * 2.0 + self.width, ROW),
                            c.row_pressed,
                        );
                    }
                    let current = p_id == provider && id == model;
                    let label = widgets::fit(label, T_BODY, self.width * 0.55);
                    widgets::text(
                        canvas,
                        &label,
                        x,
                        T_BODY.baseline(y + (ROW - T_BODY.line()) / 2.0),
                        T_BODY,
                        p.text,
                        1.0,
                    );
                    let id_text = widgets::fit(id, T_SMALL, self.width * 0.4 - 70.0);
                    widgets::text_right(
                        canvas,
                        &id_text,
                        x + self.width - 76.0,
                        T_SMALL.baseline(y + (ROW - T_SMALL.line()) / 2.0),
                        T_SMALL,
                        p.muted,
                        1.0,
                    );
                    if current {
                        icons::draw(
                            canvas,
                            Icon::Check,
                            x + self.width - 30.0,
                            y + ROW / 2.0,
                            52.0,
                            p.accent,
                            1.0,
                        );
                    }
                    widgets::hairline(
                        canvas,
                        x,
                        x + self.width,
                        y + ROW - HAIRLINE,
                        c.rule_field_row,
                    );
                    target += 1;
                }
            }
        }
    }

    fn item_at(&self, _cx: &Cx, x: f32, y: f32) -> Option<usize> {
        self.targets()
            .iter()
            .position(|(rect, _)| rect.contains(x, y))
    }

    fn activate(&mut self, _cx: &Cx, index: usize) -> Reply {
        match self.targets().into_iter().nth(index) {
            // Picking an effort keeps the sheet; a model closes it.
            Some((_, action @ Action::Ops(ops::Act::PickEffort(_)))) => Reply::Action(action),
            Some((_, action)) => Reply::CloseWith(action),
            None => Reply::None,
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.selection, format!("{:?}", self.items), &self.title).hash(&mut hasher);
        hasher.finish()
    }

    fn scoped(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::parse_event;
    use std::time::Instant;

    #[test]
    fn the_model_sheet_lists_signed_in_providers_and_picks() {
        let mut model = Model::new();
        model.on_connected();
        model.apply(
            parse_event(r#"{"type":"tools_available","providers":[{"id":"anthropic","label":"Anthropic","defaultModel":"m","models":[{"id":"m","label":"Haiku","defaultEffort":"high","supportedEfforts":[{"effort":"low"},{"effort":"high"}]}]},{"id":"openai","label":"OpenAI","defaultModel":"g","models":[{"id":"g","label":"GPT","defaultEffort":"low","supportedEfforts":[]}]}],"authByProvider":{"anthropic":true,"openai":false},"tools":[],"conversationDefaults":{"defaults":{"provider":"anthropic","model":"m","effort":"high"}}}"#),
            Instant::now(),
        );
        let listed = items(&model);
        assert!(
            listed
                .iter()
                .any(|(_, item)| matches!(item, Item::Model { id, .. } if id == "m"))
        );
        assert!(
            !listed
                .iter()
                .any(|(_, item)| matches!(item, Item::Model { id, .. } if id == "g")),
            "not signed in"
        );
        let mut sheet = inspector();
        let cx = Cx {
            model: &model,
            width: 1080.0,
            height: 2216.0,
            keyboard: false,
            now: Instant::now(),
        };
        let content = &mut sheet.content;
        content.layout(&cx, 992.0);
        // The second effort segment, then the model row.
        let high = content.item_at(&cx, 600.0, SECTION + 20.0).unwrap();
        assert_eq!(
            content.activate(&cx, high),
            Reply::Action(Action::Ops(ops::Act::PickEffort("high".into())))
        );
        let row_y = SECTION + TARGET + 22.0 + SECTION + 10.0;
        let row = content.item_at(&cx, 300.0, row_y).unwrap();
        assert_eq!(
            content.activate(&cx, row),
            Reply::CloseWith(Action::Ops(ops::Act::PickModel(
                "anthropic".into(),
                "m".into()
            )))
        );
    }
}
