//! What the app keeps across restarts: small JSON files under
//! `$EXOCORTEX_APP_DATA` (default `/var/lib/exocortex-app`):
//!
//! | File | Owner | What |
//! | --- | --- | --- |
//! | `settings.json` | A, written by E through `Settings` | theme, text size, transcript toggles |
//! | `ui.json` | sections: `list` (B), `chat` (C2), `generation` (E) | expanded folders, anchors, disclosures, drafts' model overrides |
//! | `drafts.json` | D | drafts per conversation and folder |
//!
//! Two halves. `Persist` is in the model (no I/O): the documents as read,
//! typed access, and which changed. `Store` is the event loop's: it reads
//! the files at start and writes the changed ones 500 ms after the last
//! change, atomically (a temporary file, fsync, rename) on a thread of its
//! own. A file that exists but does not parse is reported and never
//! written over: its changes live only in memory until the app restarts.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_DIR: &str = "/var/lib/exocortex-app";
pub const SETTINGS: &str = "settings.json";
pub const UI: &str = "ui.json";
pub const DRAFTS: &str = "drafts.json";
/// The files the app keeps.
pub const FILES: [&str; 3] = [SETTINGS, UI, DRAFTS];
/// A changed file is written this long after its last change.
pub const DEBOUNCE: Duration = Duration::from_millis(500);

pub fn dir() -> PathBuf {
    std::env::var_os("EXOCORTEX_APP_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DIR))
}

/// The app's settings (`settings.json`; PAR-320–326). Keys this build does
/// not know are kept as they were.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// `whale`, `cerberus` or `tonikawa`.
    pub theme: String,
    /// Every type token scaled by it (Settings → Text size).
    pub text_scale: f32,
    pub expand_thinking: bool,
    pub expand_tool_calls: bool,
    pub show_all_work: bool,
    pub fold_markers: bool,
    /// "| N tokens" in replies' footers.
    pub show_token_counts: bool,
    pub haptics: bool,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "whale".into(),
            text_scale: 1.0,
            expand_thinking: false,
            expand_tool_calls: false,
            show_all_work: false,
            fold_markers: true,
            show_token_counts: false,
            haptics: true,
            other: Map::new(),
        }
    }
}

/// One file as read: its JSON, whether it may be written (it parsed, or
/// was absent), and whether it changed since last handed to the `Store`.
#[derive(Debug, Clone)]
struct Doc {
    value: Value,
    writable: bool,
    dirty: bool,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            value: Value::Object(Map::new()),
            writable: true,
            dirty: false,
        }
    }
}

/// The kept files in memory (the model's half).
#[derive(Debug, Default)]
pub struct Persist {
    docs: BTreeMap<&'static str, Doc>,
    /// Files that could not be read, with why (shown once, never written).
    problems: Vec<(&'static str, String)>,
}

impl Persist {
    /// Takes in a file as the `Store` read it: its text, `None` when it does
    /// not exist, or the error reading it.
    pub fn load(&mut self, name: &'static str, read: io::Result<Option<String>>) {
        let doc = match read {
            Ok(None) => Doc::default(),
            Ok(Some(text)) => match serde_json::from_str::<Value>(&text) {
                Ok(value) if value.is_object() => Doc {
                    value,
                    writable: true,
                    dirty: false,
                },
                Ok(_) => self.problem(name, "not a JSON object".into()),
                Err(error) => self.problem(name, error.to_string()),
            },
            Err(error) => self.problem(name, error.to_string()),
        };
        self.docs.insert(name, doc);
    }

    fn problem(&mut self, name: &'static str, error: String) -> Doc {
        eprintln!("exocortex: {name} could not be read ({error}); it will not be written over");
        self.problems.push((name, error));
        Doc {
            writable: false,
            ..Doc::default()
        }
    }

    /// Files that could not be read (`settings.json`, why).
    pub fn problems(&self) -> &[(&'static str, String)] {
        &self.problems
    }

    /// Whether `name` will be written when it changes.
    pub fn writable(&self, name: &str) -> bool {
        self.docs.get(name).is_none_or(|doc| doc.writable)
    }

    /// The whole file, as `T` (its defaults for what it lacks).
    pub fn get<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        self.docs
            .get(name)
            .and_then(|doc| serde_json::from_value(doc.value.clone()).ok())
            .unwrap_or_default()
    }

    /// Replaces the whole file.
    pub fn set<T: Serialize>(&mut self, name: &'static str, value: &T) {
        let Ok(value) = serde_json::to_value(value) else {
            return;
        };
        let doc = self.docs.entry(name).or_default();
        if doc.value != value {
            doc.value = value;
            doc.dirty = true;
        }
    }

    /// A section (top-level key) of a file, as `T`.
    pub fn section<T: DeserializeOwned + Default>(&self, name: &str, key: &str) -> T {
        self.docs
            .get(name)
            .and_then(|doc| doc.value.get(key))
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }

    /// Replaces a section, keeping the file's others.
    pub fn set_section<T: Serialize>(&mut self, name: &'static str, key: &str, value: &T) {
        let Ok(value) = serde_json::to_value(value) else {
            return;
        };
        let doc = self.docs.entry(name).or_default();
        if !doc.value.is_object() {
            doc.value = Value::Object(Map::new());
        }
        if doc.value.get(key) != Some(&value)
            && let Some(object) = doc.value.as_object_mut()
        {
            object.insert(key.to_owned(), value);
            doc.dirty = true;
        }
    }

    pub fn settings(&self) -> Settings {
        self.get(SETTINGS)
    }

    /// Changes the settings with `change`.
    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.settings();
        change(&mut settings);
        self.set(SETTINGS, &settings);
    }

    /// The changed, writable files as text, now handed to the `Store`.
    pub fn take_dirty(&mut self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (name, doc) in &mut self.docs {
            if doc.dirty && doc.writable {
                if let Ok(text) = serde_json::to_string_pretty(&doc.value) {
                    out.push((*name, text + "\n"));
                }
            }
            doc.dirty = false;
        }
        out
    }
}

/// Writes `text` to `path` so a reader sees the old file or the new one,
/// never a part: a temporary file beside it, synced, renamed over it.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        // The rename itself survives a crash once the directory is synced.
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The files' I/O (the event loop's half).
pub struct Store {
    dir: PathBuf,
    /// Contents waiting for their debounce, and when they are due.
    waiting: BTreeMap<&'static str, (String, Instant)>,
    writer: Option<Sender<(PathBuf, String)>>,
}

impl Store {
    pub fn open(dir: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel::<(PathBuf, String)>();
        let writer = std::thread::Builder::new()
            .name("exocortex-store".into())
            .spawn(move || {
                while let Ok((path, text)) = receiver.recv() {
                    if let Err(error) = write_atomic(&path, &text) {
                        eprintln!("exocortex: could not write {}: {error}", path.display());
                    }
                }
            })
            .ok()
            .map(|_| sender);
        Self {
            dir,
            waiting: BTreeMap::new(),
            writer,
        }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Reads a file: its text, or `None` when it does not exist.
    pub fn read(&self, name: &str) -> io::Result<Option<String>> {
        match fs::read_to_string(self.path(name)) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reads every kept file into `persist`.
    pub fn load_into(&self, persist: &mut Persist) {
        for name in FILES {
            persist.load(name, self.read(name));
        }
    }

    /// Takes `persist`'s changes; each is written `DEBOUNCE` after its last.
    pub fn take(&mut self, persist: &mut Persist, now: Instant) {
        for (name, text) in persist.take_dirty() {
            self.waiting.insert(name, (text, now + DEBOUNCE));
        }
    }

    /// Writes (on the writer thread) what is due.
    pub fn flush_due(&mut self, now: Instant) {
        let due: Vec<&'static str> = self
            .waiting
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(name, _)| *name)
            .collect();
        for name in due {
            if let Some((text, _)) = self.waiting.remove(name) {
                self.write(name, text);
            }
        }
    }

    fn write(&self, name: &str, text: String) {
        let path = self.path(name);
        match &self.writer {
            Some(writer) => {
                let _ = writer.send((path, text));
            }
            None => {
                let _ = write_atomic(&path, &text);
            }
        }
    }

    /// Writes everything waiting now, here (the app is closing).
    pub fn flush_all(&mut self) {
        for (name, (text, _)) in std::mem::take(&mut self.waiting) {
            if let Err(error) = write_atomic(&self.path(name), &text) {
                eprintln!("exocortex: could not write {name}: {error}");
            }
        }
    }

    /// When `flush_due` next has something to write.
    pub fn deadline(&self) -> Option<Instant> {
        self.waiting.values().map(|(_, at)| *at).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "exocortex-store-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_round_trip_keeping_unknown_keys() {
        let dir = scratch("settings");
        fs::write(
            dir.join(SETTINGS),
            r#"{"theme":"cerberus","futureKey":[1,2],"haptics":false}"#,
        )
        .unwrap();
        let mut store = Store::open(dir.clone());
        let mut persist = Persist::default();
        store.load_into(&mut persist);
        let settings = persist.settings();
        assert_eq!(settings.theme, "cerberus");
        assert!(!settings.haptics);
        assert_eq!(settings.text_scale, 1.0, "a default for what it lacks");
        persist.update_settings(|s| s.expand_thinking = true);
        let now = Instant::now();
        store.take(&mut persist, now);
        assert_eq!(store.deadline(), Some(now + DEBOUNCE));
        store.flush_due(now);
        assert!(store.deadline().is_some(), "not before its debounce");
        store.flush_all();
        let written: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(SETTINGS)).unwrap()).unwrap();
        assert_eq!(written["expandThinking"], true);
        assert_eq!(written["futureKey"], serde_json::json!([1, 2]));
        // No temporary file left behind.
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec![SETTINGS.to_owned()]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_corrupt_file_is_reported_and_never_written_over() {
        let dir = scratch("corrupt");
        let garbage = "{\"theme\": \"whale\", oops";
        fs::write(dir.join(SETTINGS), garbage).unwrap();
        let mut store = Store::open(dir.clone());
        let mut persist = Persist::default();
        store.load_into(&mut persist);
        assert_eq!(persist.problems().len(), 1);
        assert_eq!(persist.problems()[0].0, SETTINGS);
        assert!(!persist.writable(SETTINGS));
        // Changes stay in memory…
        persist.update_settings(|s| s.theme = "tonikawa".into());
        assert_eq!(persist.settings().theme, "tonikawa");
        // …and are never written.
        store.take(&mut persist, Instant::now());
        store.flush_all();
        assert_eq!(fs::read_to_string(dir.join(SETTINGS)).unwrap(), garbage);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sections_share_a_file_and_writes_are_atomic() {
        let dir = scratch("sections");
        let mut store = Store::open(dir.clone());
        let mut persist = Persist::default();
        store.load_into(&mut persist);
        persist.set_section(UI, "list", &vec!["f1", "f2"]);
        persist.set_section(UI, "chat", &serde_json::json!({"anchors": {}}));
        let list: Vec<String> = persist.section(UI, "list");
        assert_eq!(list, vec!["f1", "f2"]);
        store.take(&mut persist, Instant::now());
        store.flush_all();
        let written: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(UI)).unwrap()).unwrap();
        assert_eq!(written["list"][1], "f2");
        assert!(written["chat"].is_object());
        // Unchanged: nothing to write.
        persist.set_section(UI, "list", &vec!["f1", "f2"]);
        assert!(persist.take_dirty().is_empty());
        // An atomic write replaces the file whole.
        write_atomic(&dir.join(UI), "{}\n").unwrap();
        assert_eq!(fs::read_to_string(dir.join(UI)).unwrap(), "{}\n");
        fs::remove_dir_all(dir).unwrap();
    }
}
