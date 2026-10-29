//! Copy and paste (PAR-141, PAR-345): the system clipboard is Moon's
//! (`clipboard set` / `clipboard get` over its control socket; Moon owns
//! X's CLIPBOARD, so the Browser and other apps share it,
//! OSS-Phone-V1 `docs/app-services.md`). The app also keeps what it last
//! copied, so paste works with Moon absent (a development host).

#![allow(dead_code)]

use std::sync::Mutex;

use crate::control;

/// Moon keeps at most this much text.
pub const LIMIT: usize = 1 << 20;

static LAST: Mutex<Option<String>> = Mutex::new(None);

/// Puts `text` on the clipboard (Moon's, on a thread of its own, so the UI
/// never waits; and the app's own).
pub fn copy(text: &str) {
    let mut text = text.to_owned();
    if text.len() > LIMIT {
        let mut end = LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    if let Ok(mut last) = LAST.lock() {
        *last = Some(text.clone());
    }
    // Tests (which may run on the phone) never touch the user's clipboard.
    if cfg!(test) {
        return;
    }
    let command = format!("clipboard set {}", serde_json::Value::String(text));
    let _ = std::thread::Builder::new()
        .name("exocortex-clipboard".into())
        .spawn(move || {
            control::send(&command);
        });
}

/// The clipboard's text: Moon's (what any app copied), else what this app
/// copied last. Asks Moon synchronously (a local socket, answered at once,
/// at most 200 ms); a user's paste is worth that.
pub fn paste() -> Option<String> {
    if !cfg!(test)
        && let Some(result) = control::query("clipboard get")
    {
        return result
            .get("text")
            .and_then(|text| text.as_str())
            .map(str::to_owned);
    }
    LAST.lock().ok().and_then(|last| last.clone())
}

/// What this app copied last (tests; and a paste without Moon).
pub fn last_copied() -> Option<String> {
    LAST.lock().ok().and_then(|last| last.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_text_pastes_back_without_moon() {
        copy("héllo\nworld");
        assert_eq!(last_copied().as_deref(), Some("héllo\nworld"));
        assert_eq!(paste().as_deref(), Some("héllo\nworld"));
    }
}
