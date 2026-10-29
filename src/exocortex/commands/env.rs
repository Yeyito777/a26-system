//! What the picker and the submission search over, built from the model
//! (the iPhone's `web/src/commands/env.ts`): the commands available here,
//! the macro catalog, and the argument lists below each command and inline
//! modifier (providers and models, the efforts of what runs, speeds, `/queue`
//! targets).

use std::rc::Rc;

use super::inline::{self, Generation, InlineEnv, inline_commands};
use super::queue_targets::{self, Source};
use super::registry;
use super::search;
use super::{ArgRegistry, Item};
use crate::exocortex::macros::{self, Catalog};
use crate::exocortex::model::Model;
use crate::exocortex::model::ops::{model_info, provider_info, supports_speed};
use crate::exocortex::proto::{Provider, Speed};

/// The slash environment of the open conversation or draft.
pub struct SlashEnv<'a> {
    model: &'a Model,
    pub macros: Rc<Catalog>,
    /// Commands available here, in picker order.
    pub roots: Vec<Item>,
}

fn provider_items(providers: &[Provider], suffix: &str) -> Vec<Item> {
    providers
        .iter()
        .map(|p| {
            let label = if p.label.is_empty() { &p.id } else { &p.label };
            Item::new(&p.id, format!("{label}{suffix}"))
        })
        .collect()
}

fn model_items(providers: &[Provider], provider: &str) -> Vec<Item> {
    let Some(p) = provider_info(providers, provider) else {
        return Vec::new();
    };
    p.models
        .iter()
        .map(|m| {
            let label = if m.label.is_empty() { &m.id } else { &m.label };
            Item::new(
                &m.id,
                format!(
                    "{label}{}{}",
                    if m.id == p.default_model {
                        " (default)"
                    } else {
                        ""
                    },
                    if m.supports_tools == Some(false) {
                        " · chat only, no tools"
                    } else {
                        ""
                    }
                ),
            )
        })
        .collect()
}

/// `/model` → providers, `/model <p>` → that provider's models.
fn model_args(providers: &[Provider], root: &str) -> ArgRegistry {
    let mut out = ArgRegistry::new();
    out.insert(root.to_owned(), provider_items(providers, " models"));
    for p in providers {
        out.insert(format!("{root} {}", p.id), model_items(providers, &p.id));
    }
    out
}

/// A model's efforts, its default marked.
pub fn effort_items(providers: &[Provider], provider: &str, model: &str) -> Vec<Item> {
    let default = model_info(providers, provider, model).map(|m| m.default_effort.clone());
    inline::supported_efforts(providers, provider, model)
        .into_iter()
        .map(|(effort, description)| {
            let desc = if Some(&effort) == default.as_ref() {
                format!("{description} (default)")
            } else {
                description
            };
            Item::new(effort, desc)
        })
        .collect()
}

/// Speeds for `/default-model … <effort>`.
fn default_speed_items(providers: &[Provider], provider: &str, model: &str) -> Vec<Item> {
    if !supports_speed(providers, provider, model, Speed::Fast) {
        return vec![Item::new(
            "na",
            "Fast mode is not available for this provider",
        )];
    }
    let mut items = vec![
        Item::new("fast", "Enable fast mode for new conversations"),
        Item::new("off", "Disable fast mode for new conversations"),
    ];
    if supports_speed(providers, provider, model, Speed::Ultrafast) {
        items.push(Item::new(
            "ultrafast",
            "Use Ultrafast for new conversations",
        ));
    }
    items
}

const SPEED_ARGS: [(&str, &str); 2] = [
    ("on", "Enable this speed tier for this conversation"),
    ("off", "Return to standard speed"),
];

impl<'a> SlashEnv<'a> {
    pub fn new(model: &'a Model) -> Self {
        let conversation = model.chat.id.is_some();
        let roots = registry::ranked()
            .into_iter()
            .filter(|c| !c.needs_conversation || conversation)
            .map(|c| Item::new(c.slash(), c.description))
            .collect();
        Self {
            macros: macros::catalog(model.composer.macro_env.as_ref()),
            roots,
            model,
        }
    }

    pub fn is_macro(&self, name: &str) -> bool {
        self.macros.items.iter().any(|m| m.name == name)
    }

    fn providers(&self) -> &[Provider] {
        &self.model.ops.providers
    }

    /// What runs here (the conversation's or the draft's selection).
    pub fn generation(&self) -> Generation {
        let r = self.model.resolve();
        Generation {
            provider: r.provider,
            model: r.model,
            effort: r.effort,
            speed: r.speed,
        }
    }

    pub fn targets(&self) -> Source<'a> {
        Source {
            conversations: &self.model.sidebar.conversations,
            folders: &self.model.sidebar.folders,
        }
    }

    /// For `inline::parse`.
    pub fn inline_env(&self) -> InlineEnv<'a> {
        InlineEnv {
            providers: &self.model.ops.providers,
            generation: self.generation(),
            conversation: self.model.chat.id.is_some(),
            streaming: self.model.chat.id.is_some() && self.model.streaming(),
            targets: self.targets(),
        }
    }

    /// Argument lists below command `root`.
    pub fn command_args(&self, root: &str) -> ArgRegistry {
        let mut out = ArgRegistry::new();
        let Some(command) = registry::get(root) else {
            return out;
        };
        if !command.choices.is_empty() {
            out.insert(root.to_owned(), command.choice_items());
        }
        let providers = self.providers();
        match root {
            "/model" => out.extend(model_args(providers, "/model")),
            "/effort" => {
                let g = self.generation();
                out.insert(root.into(), effort_items(providers, &g.provider, &g.model));
            }
            "/queue" => {
                out.insert(root.into(), queue_targets::items(&self.targets()));
            }
            "/default-model" => {
                let mut first = vec![
                    Item::new("current", "Save the current selection on the daemon"),
                    Item::new("reset", "Reset daemon defaults to the app default"),
                ];
                first.extend(provider_items(providers, " (default models)"));
                out.insert(root.into(), first);
                // Only the branch being typed goes deeper.
                let typed: Vec<&str> = self.model.composer.text.split_whitespace().collect();
                for p in providers {
                    out.insert(format!("{root} {}", p.id), model_items(providers, &p.id));
                    if typed.first() != Some(&root) || typed.get(1) != Some(&p.id.as_str()) {
                        continue;
                    }
                    for m in &p.models {
                        let key = format!("{root} {} {}", p.id, m.id);
                        let efforts = effort_items(providers, &p.id, &m.id);
                        if typed.get(2) == Some(&m.id.as_str()) {
                            for e in &efforts {
                                out.insert(
                                    format!("{key} {}", e.name),
                                    default_speed_items(providers, &p.id, &m.id),
                                );
                            }
                        }
                        out.insert(key, efforts);
                    }
                }
            }
            "/chrono" => {
                // Cancelling is destructive: run only when typed out.
                let exact = |item: Item| Item {
                    exact: true,
                    ..item
                };
                if let Some(conv) = &self.model.chat.id {
                    let schedules = self.model.ops.chrono.lists.get(conv);
                    let mut ids: Vec<Item> = schedules
                        .into_iter()
                        .flatten()
                        .map(|s| {
                            exact(Item::new(
                                crate::exocortex::sheets::chrono::short_id(&s.id),
                                crate::exocortex::sheets::chrono::payload_label(s, 60),
                            ))
                        })
                        .collect();
                    if !ids.is_empty() {
                        ids.push(exact(Item::new(
                            "all",
                            "cancel every schedule in this conversation",
                        )));
                    }
                    out.insert("/chrono cancel".into(), ids);
                }
                out.insert(
                    root.into(),
                    vec![
                        Item::new("list", "show this conversation's schedules"),
                        exact(Item::new("cancel", "cancel a schedule")),
                        Item::new("in", "once after a delay: in 2h 30m <message>"),
                        Item::new("at", "once at a time: at tomorrow 9am <message>"),
                        Item::new("every", "repeat: every day at 9am <message>"),
                    ],
                );
            }
            _ => {}
        }
        out
    }

    /// Argument lists below inline modifier `root`.
    pub fn inline_args(&self, root: &str) -> ArgRegistry {
        let providers = self.providers();
        let mut out = ArgRegistry::new();
        match root {
            "/model" => out.extend(model_args(providers, "/model")),
            "/effort" => {
                let g = self.generation();
                out.insert(root.into(), effort_items(providers, &g.provider, &g.model));
            }
            "/fast" | "/ultrafast" => {
                out.insert(
                    root.into(),
                    SPEED_ARGS.iter().map(|(n, d)| Item::new(*n, *d)).collect(),
                );
            }
            "/queue" => {
                out.insert(root.into(), queue_targets::items(&self.targets()));
            }
            _ => {}
        }
        out
    }

    fn with_macro_args(&self, mut registry: ArgRegistry) -> ArgRegistry {
        for (key, items) in &self.macros.args {
            registry.entry(key.clone()).or_insert_with(|| items.clone());
        }
        registry
    }

    /// At the prompt's start: commands, then macros.
    pub fn leading(&self, query: &str) -> Vec<Item> {
        let mut roots = self.roots.clone();
        roots.extend(self.macros.items.iter().cloned());
        search::search_slash(query, &roots, &|root| {
            self.with_macro_args(self.command_args(root))
        })
    }

    /// Mid-message: macros and inline modifiers.
    pub fn inline(&self, query: &str) -> Vec<Item> {
        let mut roots = self.macros.items.clone();
        roots.extend(inline_commands());
        search::search_slash(query, &roots, &|root| {
            self.with_macro_args(self.inline_args(root))
        })
    }

    /// The typed shorthands submission expands ("/mod opus" → the full
    /// command; inline modifiers' arguments).
    pub fn resolve_shorthands(&self, text: &str) -> String {
        let prompt = |raw: &str| self.leading(raw);
        let inline_args = |root: &str| self.with_macro_args(self.inline_args(root));
        let sources = search::ShorthandSources {
            prompt: &prompt,
            inline_roots: &["/model", "/effort", "/fast", "/ultrafast"],
            inline_args: &inline_args,
        };
        let shorthands = search::find_shorthands(text, &sources);
        search::apply_shorthands(text, &shorthands)
    }
}
