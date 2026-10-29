//! The model inspector (WP-E; PAR-276–281, `real-05-model-sheet`,
//! `fake-12-model-inspector`): opened from the chat title or the new
//! chat's model box. GENERATION: Provider, Model and Effort pickers (each
//! opens its choices as an action list) and Speed as a segmented control,
//! then "Apply configuration" (a conversation) or "Use for this draft".
//! DAEMON DEFAULTS shows the daemon's snapshot; ACTIVITY the connection
//! and the context.
//!
//! The form is the model's (`ops::Form`), so the pickers' choices, made in
//! a sheet over this one, land in it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::gfx::{Canvas, RectF};
use crate::text::Weight;

use super::super::model::{Action, Model, ops};
use super::super::proto::Speed;
use super::super::theme::{self, HAIRLINE, T_BODY, T_CAPTION, T_NOTE, T_SECTION, TARGET, W};
use super::super::ui::icons::{self, Icon};
use super::super::ui::overlay::Cx;
use super::super::ui::sheet::{Reply, Sheet, SheetContent};
use super::super::ui::widgets::{self, Button};
use super::panel;

/// A section title: 44 above, its line, 17 below.
const SECTION_BASE: f32 = 101.0;
/// A picker or value row.
const ROW_BASE: f32 = 133.0;
/// The speed row: the segmented control and 19 above and below.
const SPEED_ROW_BASE: f32 = 160.0;
/// Space above the buttons and between them.
const BUTTON_GAP: f32 = 34.0;

/// The rows at the current text size: their text grows, their padding
/// does not (`theme::grown`).
fn section_h() -> f32 {
    theme::grown(SECTION_BASE, 40.0)
}

fn row_h() -> f32 {
    theme::grown(ROW_BASE, 60.0)
}

fn speed_row_h() -> f32 {
    theme::grown(SPEED_ROW_BASE, 60.0)
}

fn button_h() -> f32 {
    theme::grown(TARGET, 54.0)
}
/// A segment's padding around its label.
const SEGMENT_PAD: f32 = 32.0;

#[derive(Debug, Clone, PartialEq, Hash)]
enum Item {
    Section(&'static str),
    Pick {
        field: ops::Field,
        label: &'static str,
        value: String,
        disabled: bool,
    },
    Speed {
        speeds: Vec<Speed>,
        selected: Speed,
    },
    Button {
        label: &'static str,
        kind: panel::Kind,
        act: ops::Act,
        disabled: bool,
    },
    /// A switch row: label, wrapped detail, on, what a tap does.
    Switch {
        label: &'static str,
        detail: Vec<String>,
        on: bool,
        act: ops::Act,
        disabled: bool,
    },
    /// Wrapped lines in `T_CAPTION` `muted`.
    Note(Vec<String>),
    /// The same in `error`.
    Error(Vec<String>),
    /// The defaults: lines in `T_NOTE`.
    Text(Vec<String>),
    /// A read-only row: label, value.
    Value(&'static str, String),
    Gap(i32),
}

impl Item {
    fn height(&self) -> f32 {
        match self {
            Item::Section(_) => section_h(),
            Item::Pick { .. } | Item::Value(..) => row_h(),
            Item::Speed { .. } => speed_row_h(),
            Item::Button { .. } => button_h(),
            Item::Switch { detail, .. } => {
                (2.0 * 22.0 + T_BODY.line() + detail.len() as f32 * theme::T_META.line())
                    .max(theme::TOGGLE_ROW)
            }
            Item::Note(lines) | Item::Error(lines) => {
                16.0 + lines.len() as f32 * T_CAPTION.line() + 6.0
            }
            Item::Text(lines) => 11.0 + lines.len() as f32 * T_NOTE.line(),
            Item::Gap(px) => *px as f32,
        }
    }
}

/// What the inspector shows for `model`, top to bottom.
fn items(model: &Model, width: f32) -> Vec<Item> {
    let form = model.form();
    let providers = &model.ops.providers;
    let provider = ops::provider_info(providers, &form.provider);
    let info = ops::model_info(providers, &form.provider, &form.model);
    let none = |s: &str| {
        if s.is_empty() {
            "Daemon default".to_owned()
        } else {
            s.to_owned()
        }
    };
    let mut items = vec![
        Item::Section("Generation"),
        Item::Pick {
            field: ops::Field::Provider,
            label: "Provider",
            value: provider.map_or_else(|| none(&form.provider), |p| p.label.clone()),
            disabled: false,
        },
        Item::Pick {
            field: ops::Field::Model,
            label: "Model",
            value: info.map_or_else(|| none(&form.model), |m| m.label.clone()),
            disabled: false,
        },
        Item::Pick {
            field: ops::Field::Effort,
            label: "Effort",
            value: none(&form.effort),
            disabled: false,
        },
        Item::Speed {
            speeds: ops::speeds(providers, &form.provider, &form.model, form.speed),
            selected: form.speed,
        },
        Item::Gap(BUTTON_GAP as i32),
    ];
    let blocked = model.apply_blocked(&form);
    let draft = form.conv.is_none();
    let detail_w = width - theme::SWITCH.0 - 34.0;
    let switch = |label, detail: &str, on, act, disabled| Item::Switch {
        label,
        detail: if detail.is_empty() {
            Vec::new()
        } else {
            widgets::wrap_plain(detail, theme::T_META, detail_w)
        },
        on,
        act,
        disabled,
    };
    if draft {
        items.push(switch(
            "Delegation",
            "Sent with the first message",
            form.delegation,
            ops::Act::FormDelegation,
            false,
        ));
        items.push(switch(
            "Allow legacy models",
            "",
            form.legacy,
            ops::Act::FormLegacy,
            false,
        ));
    }
    items.push(switch(
        "Use as daemon default",
        "Also save this selection for new chats on the daemon",
        form.make_default,
        ops::Act::FormDefault,
        !model.ready || model.ops.defaults_request.is_some(),
    ));
    items.push(Item::Gap(BUTTON_GAP as i32));
    items.push(Item::Button {
        label: if draft {
            "Use for this draft"
        } else {
            "Apply configuration"
        },
        kind: panel::Kind::Primary,
        act: ops::Act::Apply,
        disabled: !model.can_apply(&form),
    });
    if draft && !model.resolve().inherited {
        items.push(Item::Gap(22));
        items.push(Item::Button {
            label: "Use daemon defaults for this draft",
            kind: panel::Kind::Secondary,
            act: ops::Act::InheritDefaults,
            disabled: false,
        });
    }
    if let Some(reason) = blocked.filter(|_| !draft) {
        items.push(Item::Note(widgets::wrap_plain(reason, T_CAPTION, width)));
    }
    if let Some((notice, error)) = &model.ops.defaults_notice {
        items.push(if *error {
            Item::Error(widgets::wrap_plain(notice, T_CAPTION, width))
        } else {
            Item::Note(widgets::wrap_plain(notice, T_CAPTION, width))
        });
    }
    items.push(Item::Section("Daemon defaults"));
    match &model.ops.defaults {
        Some(snapshot) => {
            items.push(Item::Text(
                ops::format_defaults(snapshot)
                    .lines()
                    .flat_map(|line| widgets::wrap_plain(line, T_NOTE, width))
                    .collect(),
            ));
            items.push(Item::Gap(BUTTON_GAP as i32));
            items.push(Item::Button {
                label: "Reset daemon default to app default",
                kind: panel::Kind::Destructive,
                act: ops::Act::ResetDefaults,
                disabled: !model.ready
                    || !snapshot.configured
                    || model.ops.defaults_request.is_some(),
            });
        }
        None => items.push(Item::Note(widgets::wrap_plain(
            "Waiting for defaults from the connected daemon.",
            T_CAPTION,
            width,
        ))),
    }
    if !draft {
        items.push(Item::Section("Activity"));
        let connection = if model.ready {
            "Connected"
        } else if model.connected {
            "Syncing…"
        } else {
            "Connecting…"
        };
        items.push(Item::Value("Connection", connection.into()));
        let r = model.resolve();
        let max = ops::model_info(providers, &r.provider, &r.model)
            .map(|m| m.max_context)
            .filter(|max| *max > 0);
        let fmt = |n: Option<u64>| n.map_or_else(|| "?".to_owned(), ops::grouped);
        items.push(Item::Value(
            "Context",
            format!("{} / Max {}", fmt(model.context_tokens()), fmt(max)),
        ));
    }
    items.push(Item::Gap(34));
    items
}

/// The inspector's content.
pub struct Inspector {
    items: Vec<(f32, Item)>,
    width: f32,
    title: String,
}

/// The model inspector.
pub fn inspector() -> Sheet<Inspector> {
    Sheet::new(Inspector {
        items: Vec::new(),
        width: 0.0,
        title: "Model & effort".into(),
    })
}

/// What a tap on part of the inspector does.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    Pick(ops::Field),
    Speed(Speed),
    Button(ops::Act),
}

impl Inspector {
    /// The tappable parts, in item order: their rects (content
    /// coordinates) and what they do.
    fn targets(&self) -> Vec<(RectF, Target)> {
        let mut targets = Vec::new();
        for (y, item) in &self.items {
            match item {
                Item::Pick {
                    field,
                    disabled: false,
                    ..
                } => targets.push((
                    RectF::new(0.0, *y, self.width, row_h()),
                    Target::Pick(*field),
                )),
                Item::Speed { speeds, .. } => {
                    let rect = speed_rect(speeds, self.width, *y);
                    let w = rect.w / speeds.len() as f32;
                    for (index, speed) in speeds.iter().enumerate() {
                        targets.push((
                            RectF::new(rect.x + index as f32 * w, rect.y, w, rect.h),
                            Target::Speed(*speed),
                        ));
                    }
                }
                Item::Button {
                    act,
                    disabled: false,
                    ..
                } => targets.push((
                    RectF::new(0.0, *y, self.width, TARGET),
                    Target::Button(act.clone()),
                )),
                Item::Switch {
                    act,
                    disabled: false,
                    ..
                } => targets.push((
                    RectF::new(0.0, *y, self.width, item.height()),
                    Target::Button(act.clone()),
                )),
                _ => {}
            }
        }
        targets
    }
}

/// The segmented control's rect in a speed row at `y`: right-aligned, each
/// segment as wide as the widest label needs.
fn speed_rect(speeds: &[Speed], width: f32, y: f32) -> RectF {
    let ty = T_NOTE.weight(Weight::MonoBold);
    let segment = speeds
        .iter()
        .map(|s| widgets::measure(s.label(), ty))
        .fold(0.0, f32::max)
        + 2.0 * SEGMENT_PAD;
    let w = (segment * speeds.len() as f32 + 2.0 * HAIRLINE).round();
    RectF::new(width - w, y + (speed_row_h() - TARGET) / 2.0, w, TARGET)
}

impl SheetContent for Inspector {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn layout(&mut self, cx: &Cx, width: f32) -> f32 {
        self.width = width;
        self.title = if cx.model.chat.id.is_some() {
            "Model & effort".into()
        } else {
            "New chat model".into()
        };
        let mut y = 0.0;
        self.items = items(cx.model, width)
            .into_iter()
            .map(|item| {
                let at = y;
                y += item.height();
                (at, item)
            })
            .collect();
        y
    }

    fn draw(&self, canvas: &mut Canvas, _cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let c = theme::c();
        let background = p.sidebar_background;
        let pressed = pressed
            .and_then(|index| self.targets().into_iter().nth(index))
            .map(|(_, target)| target);
        let right = x + self.width;
        for (y, item) in &self.items {
            let y = top + y;
            if !widgets::shows(canvas, y, y + item.height()) {
                continue;
            }
            match item {
                Item::Section(title) => {
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
                Item::Pick {
                    field,
                    label,
                    value,
                    disabled,
                } => {
                    if pressed.as_ref() == Some(&Target::Pick(*field)) {
                        widgets::fill(
                            canvas,
                            RectF::new(0.0, y, W, row_h() - HAIRLINE),
                            p.selection_background,
                        );
                    }
                    let alpha = if *disabled { 0.5 } else { 1.0 };
                    let baseline = T_BODY.baseline(y + (row_h() - T_BODY.line()) / 2.0);
                    let label_w = widgets::text(canvas, label, x, baseline, T_BODY, p.text, alpha);
                    let value_right = right - 73.0;
                    let value = widgets::fit(value, T_BODY, value_right - x - label_w - 34.0);
                    widgets::text_right(
                        canvas,
                        &value,
                        value_right,
                        baseline,
                        T_BODY,
                        p.secondary_accent,
                        alpha,
                    );
                    icons::draw(
                        canvas,
                        Icon::ChevronDown,
                        right - 22.0,
                        y + row_h() / 2.0,
                        39.0,
                        p.muted,
                        alpha,
                    );
                    widgets::hairline(canvas, x, right, y + row_h() - HAIRLINE, c.rule_field_row);
                }
                Item::Speed { speeds, selected } => {
                    widgets::text(
                        canvas,
                        "Speed",
                        x,
                        T_BODY.baseline(y + (speed_row_h() - T_BODY.line()) / 2.0),
                        T_BODY,
                        p.text,
                        1.0,
                    );
                    let rect = speed_rect(speeds, self.width, y);
                    let rect = RectF::new(rect.x + x, rect.y, rect.w, rect.h);
                    let labels: Vec<&str> = speeds.iter().map(|s| s.label()).collect();
                    let on = speeds.iter().position(|s| s == selected).unwrap_or(0);
                    let down = match &pressed {
                        Some(Target::Speed(speed)) => speeds.iter().position(|s| s == speed),
                        _ => None,
                    };
                    widgets::segmented(canvas, rect, &labels, on, down);
                    widgets::hairline(
                        canvas,
                        x,
                        right,
                        y + speed_row_h() - HAIRLINE,
                        c.rule_field_row,
                    );
                }
                Item::Button {
                    label,
                    kind,
                    act,
                    disabled,
                } => {
                    let down = pressed.as_ref() == Some(&Target::Button(act.clone()));
                    widgets::button(
                        canvas,
                        RectF::new(x, y, self.width, button_h()),
                        &Button::new(label, kind.button())
                            .pressed(down)
                            .disabled(*disabled),
                        background,
                    );
                }
                Item::Switch {
                    label,
                    detail,
                    on,
                    act,
                    disabled,
                } => {
                    let h = item.height();
                    if pressed.as_ref() == Some(&Target::Button(act.clone())) {
                        widgets::fill(
                            canvas,
                            RectF::new(0.0, y, W, h - HAIRLINE),
                            p.selection_background,
                        );
                    }
                    let alpha = if *disabled { 0.4 } else { 1.0 };
                    let meta = theme::T_META;
                    let text_h = T_BODY.line() + detail.len() as f32 * meta.line();
                    let top = y + ((h - text_h) / 2.0).round();
                    let (sw, _) = theme::SWITCH;
                    let room = right - sw - 34.0 - x;
                    widgets::text(
                        canvas,
                        &widgets::fit(label, T_BODY, room),
                        x,
                        T_BODY.baseline(top),
                        T_BODY,
                        p.text,
                        alpha,
                    );
                    for (index, line) in detail.iter().enumerate() {
                        widgets::text(
                            canvas,
                            line,
                            x,
                            meta.baseline(top + T_BODY.line() + index as f32 * meta.line()),
                            meta,
                            p.muted,
                            alpha,
                        );
                    }
                    let (sw, sh) = theme::SWITCH;
                    widgets::switch(
                        canvas,
                        right - sw,
                        y + ((h - sh) / 2.0).round(),
                        if *on { 1.0 } else { 0.0 },
                        background,
                    );
                    widgets::hairline(canvas, x, right, y + h - HAIRLINE, c.rule_field_row);
                }
                Item::Error(lines) => {
                    for (index, line) in lines.iter().enumerate() {
                        widgets::text(
                            canvas,
                            line,
                            x,
                            T_CAPTION.baseline(y + 16.0 + index as f32 * T_CAPTION.line()),
                            T_CAPTION,
                            p.error,
                            1.0,
                        );
                    }
                }
                Item::Note(lines) => {
                    for (index, line) in lines.iter().enumerate() {
                        widgets::text(
                            canvas,
                            line,
                            x,
                            T_CAPTION.baseline(y + 16.0 + index as f32 * T_CAPTION.line()),
                            T_CAPTION,
                            p.muted,
                            1.0,
                        );
                    }
                }
                Item::Text(lines) => {
                    let color = theme::mix(p.text, 85, background);
                    for (index, line) in lines.iter().enumerate() {
                        widgets::text(
                            canvas,
                            line,
                            x,
                            T_NOTE.baseline(y + 11.0 + index as f32 * T_NOTE.line()),
                            T_NOTE,
                            color,
                            1.0,
                        );
                    }
                }
                Item::Value(label, value) => {
                    let baseline = T_BODY.baseline(y + (row_h() - T_BODY.line()) / 2.0);
                    let label_w = widgets::text(canvas, label, x, baseline, T_BODY, p.text, 1.0);
                    let value = widgets::fit(value, T_BODY, self.width - label_w - 34.0);
                    widgets::text_right(
                        canvas,
                        &value,
                        right,
                        baseline,
                        T_BODY,
                        p.secondary_accent,
                        1.0,
                    );
                    widgets::hairline(canvas, x, right, y + row_h() - HAIRLINE, c.rule_field_row);
                }
                Item::Gap(_) => {}
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
            Some((_, Target::Pick(field))) => Reply::Action(Action::Ops(ops::Act::Choose(field))),
            Some((_, Target::Speed(speed))) => {
                Reply::Action(Action::Ops(ops::Act::FormSpeed(speed)))
            }
            // Applying closes the inspector; going back to the defaults
            // keeps it, showing them.
            Some((_, Target::Button(act @ ops::Act::Apply))) => Reply::CloseWith(Action::Ops(act)),
            Some((_, Target::Button(act))) => Reply::Action(Action::Ops(act)),
            None => Reply::None,
        }
    }

    fn key(&self, _cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.title.hash(&mut hasher);
        for (y, item) in &self.items {
            ((*y as i32), item).hash(&mut hasher);
        }
        theme::generation().hash(&mut hasher);
        hasher.finish()
    }

    fn scoped(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;
    use std::time::Instant;

    const CATALOG: &str = r#"{"type":"tools_available","providers":[{"id":"anthropic","label":"Anthropic","defaultModel":"m","supportsFastMode":true,"models":[{"id":"m","label":"Haiku","defaultEffort":"high","supportedEfforts":[{"effort":"low"},{"effort":"high"}],"maxContext":200000}]},{"id":"openai","label":"OpenAI","defaultModel":"g","models":[{"id":"g","label":"GPT","defaultEffort":"low","supportedEfforts":[{"effort":"low"}]}]}],"authByProvider":{"anthropic":true,"openai":false},"tools":[],"conversationDefaults":{"defaults":{"provider":"anthropic","model":"m","effort":"high"},"configured":true}}"#;

    fn cx(model: &Model) -> Cx<'_> {
        Cx {
            model,
            width: 1080.0,
            height: 2216.0,
            keyboard: false,
            now: Instant::now(),
        }
    }

    fn laid(model: &Model) -> Inspector {
        let mut content = inspector().content;
        content.layout(&cx(model), 992.0);
        content
    }

    /// The target whose rect holds content point (`x`, `y`).
    fn tap(content: &mut Inspector, model: &Model, x: f32, y: f32) -> Reply {
        let index = content.item_at(&cx(model), x, y).expect("a target");
        content.activate(&cx(model), index)
    }

    fn y_of(content: &Inspector, wanted: impl Fn(&Item) -> bool) -> f32 {
        content
            .items
            .iter()
            .find(|(_, item)| wanted(item))
            .map(|(y, _)| *y)
            .unwrap()
    }

    #[test]
    fn the_inspector_picks_through_its_rows_and_applies() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        model.apply(parse_event(CATALOG), Instant::now());
        model.handle(Action::Nav(Nav::NewConversation));
        model.handle(Action::Ops(ops::Act::OpenInspector));
        let mut content = laid(&model);
        assert_eq!(content.title, "New chat model");
        // Provider row → its choices.
        let provider_y = y_of(&content, |i| {
            matches!(
                i,
                Item::Pick {
                    label: "Provider",
                    ..
                }
            )
        });
        assert_eq!(
            tap(&mut content, &model, 300.0, provider_y + 10.0),
            Reply::Action(Action::Ops(ops::Act::Choose(ops::Field::Provider)))
        );
        // Speed: Standard and Fast (the provider's fast tier).
        let speed_y = y_of(&content, |i| matches!(i, Item::Speed { .. }));
        let Some((_, Item::Speed { speeds, .. })) = content
            .items
            .iter()
            .find(|(_, i)| matches!(i, Item::Speed { .. }))
        else {
            panic!()
        };
        assert_eq!(speeds, &[Speed::Standard, Speed::Fast]);
        assert_eq!(
            tap(&mut content, &model, 980.0, speed_y + speed_row_h() / 2.0),
            Reply::Action(Action::Ops(ops::Act::FormSpeed(Speed::Fast)))
        );
        // Unchanged: Use for this draft is disabled (no target).
        let button_y = y_of(&content, |i| matches!(i, Item::Button { .. }));
        assert!(
            content
                .item_at(&cx(&model), 400.0, button_y + 20.0)
                .is_none()
        );
        model.handle(Action::Ops(ops::Act::FormSpeed(Speed::Fast)));
        let mut content = laid(&model);
        assert_eq!(
            tap(&mut content, &model, 400.0, button_y + 20.0),
            Reply::CloseWith(Action::Ops(ops::Act::Apply))
        );
        // A draft shows the defaults, no activity.
        assert!(
            content
                .items
                .iter()
                .any(|(_, i)| matches!(i, Item::Text(lines) if lines[0] == "User default:"))
        );
        assert!(
            !content
                .items
                .iter()
                .any(|(_, i)| *i == Item::Section("Activity"))
        );
    }

    #[test]
    fn a_conversations_inspector_shows_its_context_and_waits_while_streaming() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        let now = Instant::now();
        model.apply(parse_event(CATALOG), now);
        model.apply(
            parse_event(r#"{"type":"conversation_updated","summary":{"id":"c","title":"T","provider":"anthropic","model":"m","effort":"low","streaming":true}}"#),
            now,
        );
        model.handle(Action::Nav(Nav::OpenConversation("c".into())));
        model.apply(
            parse_event(r#"{"type":"context_update","convId":"c","contextTokens":123456}"#),
            now,
        );
        model.handle(Action::Ops(ops::Act::OpenInspector));
        let content = laid(&model);
        assert_eq!(content.title, "Model & effort");
        assert!(
            content
                .items
                .iter()
                .any(|(_, i)| *i == Item::Value("Context", "123,456 / Max 200,000".into()))
        );
        assert!(content.items.iter().any(
            |(_, i)| matches!(i, Item::Note(lines) if lines[0].starts_with("Stop generation"))
        ));
        assert!(content.items.iter().any(|(_, i)| matches!(
            i,
            Item::Button {
                label: "Apply configuration",
                disabled: true,
                ..
            }
        )));
    }

    #[test]
    fn choices_list_every_provider_and_check_the_current() {
        let mut model = Model::new();
        model.on_connected();
        model.ready = true;
        model.apply(parse_event(CATALOG), Instant::now());
        model.handle(Action::Nav(Nav::NewConversation));
        model.handle(Action::Ops(ops::Act::OpenInspector));
        model.ui.opening.clear();
        model.handle(Action::Ops(ops::Act::Choose(ops::Field::Provider)));
        assert_eq!(model.ui.opening.len(), 1);
        assert_eq!(model.ui.opening[0].label(), "Provider");
        model.handle(Action::Ops(ops::Act::FormProvider("openai".into())));
        let form = model.form();
        assert_eq!((form.model.as_str(), form.effort.as_str()), ("g", "low"));
    }
}
