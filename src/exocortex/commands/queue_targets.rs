//! `/queue [target]` targets (PAR-227, PAR-262), a port of the TUI's
//! `tui/src/queuetargets.ts` (and the iPhone's `web/src/commands/queuetargets.ts`):
//! a delay (`5m`, `2h 30m`, `9d`), a conversation (newest first, by title or
//! id) or a 📁 folder (by path or name); absent means global idle.

use std::collections::HashMap;

use serde_json::Value;

use super::Item;
use crate::exocortex::proto::{Folder, Summary};

/// The list the targets come from.
pub struct Source<'a> {
    pub conversations: &'a HashMap<String, Summary>,
    pub folders: &'a HashMap<String, Folder>,
}

/// `14:05`-style durations: `parseDurationMs` (`500ms`, `30s`, `5m`, `2h`,
/// `9d`, combined as `8h3m1s`); None when invalid or zero.
pub fn parse_duration_ms(text: &str) -> Option<u64> {
    let _ = text;
    unimplemented!("WP-D logic subagent")
}

/// The picker's choices after `/queue `: conversations newest first, then
/// folders (`📁 path`), with their ids and titles as aliases.
pub fn items(source: &Source) -> Vec<Item> {
    let _ = source;
    unimplemented!("WP-D logic subagent")
}

/// The target spelled after `/queue` (which ends at byte `command_end`):
/// the `waitTarget` JSON (`{"type":"delay","delayMs":…,"label":…}`,
/// `{"type":"conversation","convId":…,"label":…}`,
/// `{"type":"folder","folderId":…,"label":…}`) and where it ends; None when
/// nothing follows that names one (global idle).
pub fn match_target(source: &Source, text: &str, command_end: usize) -> Option<(Value, usize)> {
    let _ = (source, text, command_end);
    unimplemented!("WP-D logic subagent")
}

/// A folder's path from the top ("Work/Clients").
pub fn folder_path(folders: &HashMap<String, Folder>, id: Option<&str>) -> String {
    let _ = (folders, id);
    unimplemented!("WP-D logic subagent")
}
