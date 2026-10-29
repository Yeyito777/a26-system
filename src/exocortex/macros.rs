//! Macros (WP-D; PAR-207, PAR-208): the TUI's text shortcuts (`/consider`,
//! `/commit`, `/go`, …), expanded here on the phone before a message is sent.
//! The catalog is `macros.json`, generated from Exocortex `tui/src/macros.ts`
//! by `scripts/sync-exocortex-macros.ts`; the daemon's paths come from
//! `tools_available.macroEnvironment` (without it, the TUI's `<daemon-…>`
//! placeholders and its resolve-on-daemon prefix). Expansion is one
//! right-to-left pass, longest nested prefix wins, trailing words kept;
//! URLs and paths are untouched (the iPhone's `web/src/commands/macros.ts`).

#![allow(dead_code)]

use std::collections::HashMap;

use serde_json::Value;

use super::commands::Item;

/// `tools_available.macroEnvironment`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MacroEnv {
    pub repo_root: String,
    pub storage_dir: String,
    pub external_tools_dir: String,
    pub external_tools_trash_dir: String,
    pub installed_tool_dirs: Vec<String>,
}

impl MacroEnv {
    /// From the event's JSON (None when it is absent or not an object).
    pub fn from_json(value: &Value) -> Option<Self> {
        let _ = value;
        unimplemented!("WP-D logic subagent")
    }
}

/// The macros for a daemon's environment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    /// The picker's macros ("/consider", "Am I right or wrong?").
    pub items: Vec<Item>,
    /// Nested arguments, keyed "/name" and "/name arg …".
    pub args: HashMap<String, Vec<Item>>,
    /// What each macro (with its arguments) expands to, by its words.
    pub expansions: HashMap<String, String>,
}

/// The catalog for `env` (built once per environment and kept).
pub fn catalog(env: Option<&MacroEnv>) -> std::rc::Rc<Catalog> {
    let _ = env;
    unimplemented!("WP-D logic subagent")
}

/// `text` with its macros expanded (`expandMacros`).
pub fn expand(text: &str, env: Option<&MacroEnv>) -> String {
    let _ = (text, env);
    unimplemented!("WP-D logic subagent")
}
