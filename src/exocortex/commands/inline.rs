//! Inline modifiers anywhere in a message (PAR-209; the TUI's
//! `inlineeffort.ts`, the iPhone's `web/src/commands/inline.ts`):
//! `/model p m`, `/effort X`, `/fast [on|off]`, `/ultrafast [on|off]` and
//! `/queue [target]`. They are checked in order against a simulated
//! selection; one invalid modifier refuses the whole message (its text
//! stays to be fixed). Valid ones are taken out of the text.

use serde_json::{Value, json};

use super::Item;
use super::queue_targets::{self, Source};
use crate::exocortex::model::ops::{model_info, normalize_effort, provider_info, supports_speed};
use crate::exocortex::proto::{Provider, Speed};

/// The modifiers the picker offers mid-message.
pub fn inline_commands() -> Vec<Item> {
    vec![
        Item::new("/model", "Select provider/model for this prompt"),
        Item::new("/effort", "Set reasoning effort level"),
        Item::new("/fast", "Toggle or set fast mode"),
        Item::new("/ultrafast", "Toggle or set ultrafast mode"),
        Item::new(
            "/queue",
            "Send after a delay (5m, 2h30m) or global, conversation, or folder idle",
        ),
    ]
}

pub const INLINE_NAMES: [&str; 5] = ["/model", "/effort", "/fast", "/ultrafast", "/queue"];

/// What runs: a conversation's or a draft's selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Generation {
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub speed: Speed,
}

pub struct InlineEnv<'a> {
    pub providers: &'a [Provider],
    pub generation: Generation,
    /// A conversation (not a draft).
    pub conversation: bool,
    pub streaming: bool,
    pub targets: Source<'a>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inline {
    /// The text without its modifiers.
    pub text: String,
    /// The selection after them, when any changed it.
    pub selection: Option<Generation>,
    pub model_changed: bool,
    pub efforts: Vec<String>,
    pub speeds: Vec<Speed>,
    /// `/queue`: its `waitTarget`.
    pub queue: Option<Value>,
    /// What to say for each applied modifier.
    pub notices: Vec<String>,
}

/// All the effort levels there are (a model without a catalog entry takes
/// any).
pub const EFFORT_LEVELS: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

/// The efforts a model supports (every level when it is not in the catalog).
pub fn supported_efforts(
    providers: &[Provider],
    provider: &str,
    model: &str,
) -> Vec<(String, String)> {
    match model_info(providers, provider, model) {
        Some(info) => info
            .supported_efforts
            .iter()
            .map(|e| (e.effort.clone(), e.description.clone()))
            .collect(),
        None => EFFORT_LEVELS
            .iter()
            .map(|e| ((*e).to_owned(), (*e).to_owned()))
            .collect(),
    }
}

/// `provider/model` checked against the catalog (`modelSelectionError`).
pub fn selection_error(providers: &[Provider], provider: &str, model: &str) -> Option<String> {
    const DAYBREAK: &str = "gpt-6-sol-daybreak";
    if model.starts_with("gpt-daybreak-") || model.ends_with("-daybreak") {
        if provider != "openai" || model != DAYBREAK {
            return Some("Use /model openai gpt-6-sol-daybreak for Daybreak Blue.".into());
        }
        if model_info(providers, provider, model).is_none() {
            return Some("gpt-6-sol-daybreak is only available when the connected account advertises Daybreak Blue for gpt-6-sol. Select gpt-6-sol to use standard access.".into());
        }
    }
    let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
    let Some(p) = provider_info(providers, provider) else {
        return Some(format!(
            "Unknown provider: {provider}. Available: {}",
            ids.join(", ")
        ));
    };
    if !p.allows_custom_models && !p.models.iter().any(|m| m.id == model) {
        let models: Vec<&str> = p.models.iter().map(|m| m.id.as_str()).collect();
        return Some(format!(
            "Unknown model for provider {provider}: {model}. Available: {}",
            models.join(", ")
        ));
    }
    None
}

/// `speedModeForArgument`: explicit on/off, else a toggle.
pub fn speed_for(tier: Speed, arg: Option<&str>, current: Speed) -> Speed {
    match arg {
        Some("off") => Speed::Standard,
        Some("on") => tier,
        _ if current == tier => Speed::Standard,
        _ => tier,
    }
}

/// `text` without the bytes `start..end`, and the spaces or line break
/// that would be left doubled.
fn remove_span(text: &str, start: usize, end: usize) -> String {
    let mut s = start;
    let mut e = end;
    let bytes = text.as_bytes();
    while s > 0 && matches!(bytes[s - 1], b' ' | b'\t') {
        s -= 1;
    }
    while e < bytes.len() && matches!(bytes[e], b' ' | b'\t') {
        e += 1;
    }
    let before = &text[..s];
    let after = &text[e..];
    if before.trim().is_empty() || after.trim().is_empty() {
        return format!("{before}{after}");
    }
    match (before.ends_with('\n'), after.starts_with('\n')) {
        (true, true) => format!("{before}{}", &after[1..]),
        (true, false) | (false, true) => format!("{before}{after}"),
        (false, false) => format!("{before} {after}"),
    }
}

/// The words of `text` with their byte ranges.
fn words(text: &str) -> Vec<(&str, usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                out.push((&text[s..at], s, at));
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((&text[s..], s, text.len()));
    }
    out
}

/// Parses the modifiers of `text` (`parseInline`).
pub fn parse(text: &str, env: &InlineEnv) -> Result<Inline, String> {
    let mut sim = env.generation.clone();
    let words = words(text);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut result = Inline::default();
    let mut changed = false;
    let mut i = 0;
    while i < words.len() {
        let (word, start, end) = words[i];
        let arg = words.get(i + 1);
        match word {
            "/model" => {
                let model_arg = words.get(i + 2);
                // A standalone "/model [provider]" is the command.
                if i == 0 && words.len() <= 2 {
                    i += 1;
                    continue;
                }
                let (Some(p), Some(m)) = (arg, model_arg) else {
                    return Err("Usage: /model <provider> <model>".into());
                };
                if p.0.starts_with('/') || m.0.starts_with('/') {
                    return Err("Usage: /model <provider> <model>".into());
                }
                if let Some(error) = selection_error(env.providers, p.0, m.0) {
                    return Err(error);
                }
                if env.conversation && env.streaming {
                    return Err(
                        "Cannot switch provider/model while this conversation is streaming.".into(),
                    );
                }
                let previous_effort = sim.effort.clone();
                let previous_speed = sim.speed;
                sim.provider = p.0.to_owned();
                sim.model = m.0.to_owned();
                sim.effort =
                    normalize_effort(env.providers, &sim.provider, &sim.model, &sim.effort);
                if !supports_speed(env.providers, &sim.provider, &sim.model, sim.speed) {
                    sim.speed = Speed::Standard;
                }
                result.notices.push(format!(
                    "Model set to {}/{}{}{}",
                    sim.provider,
                    sim.model,
                    if sim.effort != previous_effort {
                        format!(" (effort {})", sim.effort)
                    } else {
                        String::new()
                    },
                    if previous_speed != Speed::Standard && sim.speed == Speed::Standard {
                        " (fast off)"
                    } else {
                        ""
                    }
                ));
                result.model_changed = true;
                changed = true;
                spans.push((start, m.2));
                i += 3;
                continue;
            }
            "/effort"
                if arg.is_some_and(|a| {
                    supported_efforts(env.providers, &sim.provider, &sim.model)
                        .iter()
                        .any(|(e, _)| e == a.0)
                }) =>
            {
                let a = arg.expect("checked");
                sim.effort = a.0.to_owned();
                result.efforts.push(sim.effort.clone());
                result.notices.push(format!("Effort set to {}", sim.effort));
                changed = true;
                spans.push((start, a.2));
                i += 2;
                continue;
            }
            "/fast" | "/ultrafast"
                if word == "/ultrafast"
                    || supports_speed(env.providers, &sim.provider, &sim.model, Speed::Fast) =>
            {
                let raw = arg.map(|a| a.0.to_lowercase());
                let explicit = matches!(raw.as_deref(), Some("on" | "off"));
                let tier = if word == "/ultrafast" {
                    Speed::Ultrafast
                } else {
                    Speed::Fast
                };
                let speed = speed_for(tier, raw.as_deref().filter(|_| explicit), sim.speed);
                if speed == Speed::Ultrafast
                    && !supports_speed(env.providers, &sim.provider, &sim.model, Speed::Ultrafast)
                {
                    return Err("Ultrafast is not advertised for this model/account.".into());
                }
                sim.speed = speed;
                result.speeds.push(speed);
                result.notices.push(format!(
                    "{} mode {}.",
                    if tier == Speed::Ultrafast {
                        "Ultrafast"
                    } else {
                        "Fast"
                    },
                    if speed == Speed::Standard {
                        "disabled"
                    } else {
                        "enabled"
                    }
                ));
                changed = true;
                spans.push((
                    start,
                    if explicit {
                        arg.expect("explicit").2
                    } else {
                        end
                    },
                ));
                i += if explicit { 2 } else { 1 };
                continue;
            }
            "/queue" => {
                let target = queue_targets::match_target(&env.targets, text, end);
                let to = target.as_ref().map_or(end, |(_, to)| *to);
                result.queue = Some(
                    target
                        .map(|(t, _)| t)
                        .unwrap_or_else(|| json!({ "type": "global" })),
                );
                spans.push((start, to));
                i += 1;
                while i < words.len() && words[i].1 < to {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if !changed && result.queue.is_none() {
        result.text = text.to_owned();
        return Ok(result);
    }
    let mut stripped = text.to_owned();
    for (start, end) in spans.iter().rev() {
        stripped = remove_span(&stripped, *start, *end);
    }
    result.text = stripped;
    if changed {
        result.selection = Some(sim);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::proto::{Effort, Model};
    use std::collections::HashMap;

    fn providers() -> Vec<Provider> {
        let effort = |e: &str| Effort {
            effort: e.into(),
            description: e.into(),
        };
        vec![
            Provider {
                id: "anthropic".into(),
                label: "Anthropic".into(),
                default_model: "claude-opus-5-5".into(),
                supports_fast_mode: true,
                models: vec![
                    Model {
                        id: "claude-opus-5-5".into(),
                        default_effort: "high".into(),
                        supported_efforts: vec![effort("low"), effort("high"), effort("xhigh")],
                        supports_ultrafast_mode: true,
                        ..Model::default()
                    },
                    Model {
                        id: "claude-haiku-5-5".into(),
                        default_effort: "low".into(),
                        supported_efforts: vec![effort("low")],
                        supports_fast_mode: Some(false),
                        ..Model::default()
                    },
                ],
                ..Provider::default()
            },
            Provider {
                id: "openai".into(),
                allows_custom_models: true,
                ..Provider::default()
            },
        ]
    }

    fn env<'a>(
        providers: &'a [Provider],
        convs: &'a HashMap<String, crate::exocortex::proto::Summary>,
        folders: &'a HashMap<String, crate::exocortex::proto::Folder>,
        streaming: bool,
    ) -> InlineEnv<'a> {
        InlineEnv {
            providers,
            generation: Generation {
                provider: "anthropic".into(),
                model: "claude-opus-5-5".into(),
                effort: "xhigh".into(),
                speed: Speed::Fast,
            },
            conversation: true,
            streaming,
            targets: Source {
                conversations: convs,
                folders,
            },
        }
    }

    #[test]
    fn modifiers_are_applied_in_order_and_taken_out() {
        let providers = providers();
        let (c, f) = (HashMap::new(), HashMap::new());
        let env = env(&providers, &c, &f, false);
        let r = parse("please /effort low summarise\n/fast off this", &env).unwrap();
        assert_eq!(r.text, "please summarise\nthis");
        assert_eq!(r.efforts, vec!["low"]);
        assert_eq!(r.speeds, vec![Speed::Standard]);
        let s = r.selection.unwrap();
        assert_eq!((s.effort.as_str(), s.speed), ("low", Speed::Standard));
        assert_eq!(r.notices, vec!["Effort set to low", "Fast mode disabled."]);
        // A model change normalises effort and speed for the new model.
        let r = parse("/model anthropic claude-haiku-5-5 hi", &env).unwrap();
        assert_eq!(r.text, "hi");
        let s = r.selection.unwrap();
        assert_eq!((s.effort.as_str(), s.speed), ("low", Speed::Standard));
        assert_eq!(
            r.notices,
            vec!["Model set to anthropic/claude-haiku-5-5 (effort low) (fast off)"]
        );
        // Custom ids where the provider allows them.
        assert!(parse("/model openai my-model hi", &env).is_ok());
        // An unknown effort is just text.
        let r = parse("use /effort sideways", &env).unwrap();
        assert_eq!(r.text, "use /effort sideways");
        assert!(r.selection.is_none());
    }

    #[test]
    fn one_bad_modifier_refuses_the_message() {
        let providers = providers();
        let (c, f) = (HashMap::new(), HashMap::new());
        let env = env(&providers, &c, &f, false);
        assert_eq!(
            parse("/effort low /model nobody x hi", &env).unwrap_err(),
            "Unknown provider: nobody. Available: anthropic, openai"
        );
        assert_eq!(
            parse("hi /model anthropic", &env).unwrap_err(),
            "Usage: /model <provider> <model>"
        );
        let streaming = super::tests::env(&providers, &c, &f, true);
        assert!(parse("/model anthropic claude-haiku-5-5 hi", &streaming).is_err());
        assert_eq!(
            parse("/model anthropic claude-haiku-5-5 /ultrafast x", &env).unwrap_err(),
            "Ultrafast is not advertised for this model/account."
        );
    }

    #[test]
    fn queue_defaults_to_global_idle() {
        let providers = providers();
        let (c, f) = (HashMap::new(), HashMap::new());
        let env = env(&providers, &c, &f, false);
        let r = parse("later /queue please", &env).unwrap();
        assert_eq!(r.queue, Some(json!({"type": "global"})));
        assert!(r.selection.is_none());
        assert_eq!(r.text, "later please");
    }
}
