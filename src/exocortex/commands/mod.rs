//! Slash commands, macros and their completion (WP-D; roadmap §2.7,
//! PAR-198–236): the picker's items, the fuzzy slash search
//! (`search.rs`), `/queue` targets (`queue_targets.rs`); the macro catalog
//! and its expansion are `../macros.rs`.

#![allow(dead_code)]

pub mod env;
pub mod inline;
pub mod queue_targets;
pub mod registry;
pub mod run;
pub mod search;
pub mod syntax;

use std::collections::HashMap;

/// A completion: a command, a macro or one of their arguments
/// (`CompletionItem`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub name: String,
    pub desc: String,
    /// What inserting it types, when not `name` (a search result spells out
    /// the whole slash input: "/model anthropic claude-opus-5-5").
    pub insert_text: Option<String>,
    /// Other strings it is found by.
    pub aliases: Vec<String>,
    /// Byte ranges of `name` the query matched (search results only).
    pub matches: Vec<(usize, usize)>,
    /// Destructive (`/goal clear`, `/worktree clean`): submission never
    /// expands a fragment of it, only the word typed out. On a search result:
    /// it passes through such an argument the input did not type out, so
    /// submission sends the text as typed.
    pub exact: bool,
}

impl Item {
    pub fn new(name: impl Into<String>, desc: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            desc: desc.into(),
            ..Self::default()
        }
    }

    /// What choosing it inserts.
    pub fn insert(&self) -> &str {
        self.insert_text.as_deref().unwrap_or(&self.name)
    }
}

/// Argument lists below a slash root, keyed by "/root" and "/root arg …".
pub type ArgRegistry = HashMap<String, Vec<Item>>;
