//! Protected drafts (WP-D; PAR-188, PAR-189): what was typed and not sent,
//! per conversation and per new chat in a folder, kept in `drafts.json`
//! across restarts.
//!
//! A draft's key is the conversation's id, or `draft:<folder id>` /
//! `draft:root` for a new chat (the iPhone's `newDraftId`; `Model::draft_key`). The file is
//! `{"drafts": {"<key>": {"text": "…", "updatedAt": ms}}}`; entries this
//! build cannot read are kept as they are. `store.rs` writes it atomically
//! and never over a file it could not parse (the app says so at start, and
//! drafts then live in memory only).

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::store::{DRAFTS, Persist};

/// The section of `drafts.json` holding the drafts.
const SECTION: &str = "drafts";

/// The kept drafts, read once from `drafts.json`.
#[derive(Debug, Default)]
pub struct Drafts {
    loaded: bool,
    texts: HashMap<String, (String, u64)>,
    /// Entries in a shape this build does not read, written back as they
    /// were.
    other: Map<String, Value>,
}

impl Drafts {
    /// Reads them from the kept file the first time.
    pub fn load(&mut self, persist: &Persist) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let section: Map<String, Value> = persist.section(DRAFTS, SECTION);
        for (key, value) in section {
            let read = match &value {
                Value::String(text) => Some((text.clone(), 0)),
                Value::Object(entry) => entry.get("text").and_then(Value::as_str).map(|text| {
                    (
                        text.to_owned(),
                        entry.get("updatedAt").and_then(Value::as_u64).unwrap_or(0),
                    )
                }),
                _ => None,
            };
            match read {
                Some((text, _)) if text.is_empty() => {}
                Some(entry) => {
                    self.texts.insert(key, entry);
                }
                None => {
                    self.other.insert(key, value);
                }
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.texts.get(key).map(|(text, _)| text.as_str())
    }

    /// Sets a draft (an empty text removes it); whether it changed.
    pub fn set(&mut self, key: &str, text: &str, now_ms: u64) -> bool {
        if text.trim().is_empty() {
            return self.texts.remove(key).is_some();
        }
        match self.texts.get_mut(key) {
            Some((kept, _)) if kept == text => false,
            Some(entry) => {
                *entry = (text.to_owned(), now_ms);
                true
            }
            None => {
                self.texts.insert(key.to_owned(), (text.to_owned(), now_ms));
                true
            }
        }
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.texts.remove(key).is_some()
    }

    /// Puts text back into a draft without losing what is there (a failed
    /// send, an unqueued message): the restored text first, then a line
    /// break and the draft's newer text.
    pub fn restore(&mut self, key: &str, text: &str, now_ms: u64) {
        let merged = match self.get(key) {
            None => text.to_owned(),
            Some(current) if current == text || text.is_empty() => current.to_owned(),
            Some(current) => format!("{text}\n{current}"),
        };
        self.set(key, &merged, now_ms);
    }

    /// Writes them into the kept file's model (the store writes it); not
    /// before they were read, which would drop the kept ones.
    pub fn save(&self, persist: &mut Persist) {
        if !self.loaded {
            return;
        }
        let mut section = self.other.clone();
        for (key, (text, at)) in &self.texts {
            section.insert(key.clone(), json!({ "text": text, "updatedAt": at }));
        }
        persist.set_section(DRAFTS, SECTION, &section);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::store::Store;
    use std::time::Instant;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "exocortex-drafts-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn drafts_survive_a_restart() {
        let dir = scratch("restart");
        let mut store = Store::open(dir.clone());
        let mut persist = Persist::default();
        store.load_into(&mut persist);
        let mut drafts = Drafts::default();
        drafts.load(&persist);
        assert!(drafts.set("c1", "half a thought", 5));
        assert!(drafts.set("draft:f1", "new idea", 6));
        assert!(!drafts.set("c1", "half a thought", 7), "unchanged");
        drafts.save(&mut persist);
        store.take(&mut persist, Instant::now());
        store.flush_all();
        // Started again.
        let mut persist = Persist::default();
        Store::open(dir.clone()).load_into(&mut persist);
        let mut drafts = Drafts::default();
        drafts.load(&persist);
        assert_eq!(drafts.get("c1"), Some("half a thought"));
        assert_eq!(drafts.get("draft:f1"), Some("new idea"));
        // An emptied draft goes.
        assert!(drafts.set("c1", "  ", 8));
        assert_eq!(drafts.get("c1"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_corrupt_file_is_never_written_over_and_unknown_entries_stay() {
        let dir = scratch("corrupt");
        let garbage = "{\"drafts\": {\"c1\": {\"text\": \"kept";
        std::fs::write(dir.join(DRAFTS), garbage).unwrap();
        let mut store = Store::open(dir.clone());
        let mut persist = Persist::default();
        store.load_into(&mut persist);
        assert!(!persist.writable(DRAFTS));
        let mut drafts = Drafts::default();
        drafts.load(&persist);
        drafts.set("c2", "typed meanwhile", 1);
        drafts.save(&mut persist);
        store.take(&mut persist, Instant::now());
        store.flush_all();
        assert_eq!(std::fs::read_to_string(dir.join(DRAFTS)).unwrap(), garbage);
        // A readable file with an entry of another shape keeps it.
        std::fs::write(
            dir.join(DRAFTS),
            r#"{"drafts": {"c1": {"text": "mine"}, "c9": {"images": [1]}}, "schemaVersion": 2}"#,
        )
        .unwrap();
        let mut persist = Persist::default();
        store.load_into(&mut persist);
        let mut drafts = Drafts::default();
        drafts.load(&persist);
        assert_eq!(drafts.get("c1"), Some("mine"));
        drafts.set("c1", "mine, edited", 2);
        drafts.save(&mut persist);
        store.take(&mut persist, Instant::now());
        store.flush_all();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(DRAFTS)).unwrap()).unwrap();
        assert_eq!(written["drafts"]["c1"]["text"], "mine, edited");
        assert_eq!(written["drafts"]["c9"]["images"][0], 1);
        assert_eq!(written["schemaVersion"], 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn restoring_puts_the_text_first() {
        let mut drafts = Drafts::default();
        drafts.load(&Persist::default());
        drafts.restore("c", "failed send", 1);
        assert_eq!(drafts.get("c"), Some("failed send"));
        drafts.set("c", "typed since", 2);
        drafts.restore("c", "failed send", 3);
        assert_eq!(drafts.get("c"), Some("failed send\ntyped since"));
    }
}
