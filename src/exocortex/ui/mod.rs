//! The app's interface kit (WP-A): icons, widgets, overlays, and the Moon
//! services the interface uses (haptics, links, the status bar colour).
//!
//! The model never does I/O: it asks for these through `model::Effect`, and
//! the event loop calls the functions here, each of which hands its request
//! to a thread of its own so the UI thread never waits on Moon.

pub mod icons;
pub mod menu;
pub mod overlay;
pub mod sheet;
pub mod textarea;
pub mod toast;
pub mod widgets;

use crate::control;

/// A tap haptic (roadmap §3.8). Moon plays it only when the user turned
/// haptics on (OSS-Phone-V1 `docs/settings.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Light and Error are the packages' (the composer, switches).
pub enum Haptic {
    /// A drop target changed, text was copied.
    Selection,
    /// The composer took focus, a switch toggled.
    Light,
    /// A long press was recognised.
    Medium,
    Error,
}

impl Haptic {
    fn style(self) -> &'static str {
        match self {
            Haptic::Selection => "selection",
            Haptic::Light => "light",
            Haptic::Medium => "medium",
            Haptic::Error => "error",
        }
    }
}

/// Runs a Moon command on a thread of its own (best effort: Moon may be
/// absent, on a development host).
fn in_background(command: String) {
    // Tests (which may run on the phone) never reach the user's Moon.
    if cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("exocortex-moon".into())
        .spawn(move || {
            control::send(&command);
        });
}

pub fn haptic(haptic: Haptic) {
    in_background(format!("haptic {}", haptic.style()));
}

/// Whether Moon's Browser takes `url` (`open browser`: http and https only,
/// no spaces or control characters, at most 8 KiB).
pub fn openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && url.len() <= 8192
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Opens an http(s) link in Moon's Browser; false (nothing done) for
/// anything else.
pub fn open_url(url: &str) -> bool {
    if !openable(url) {
        return false;
    }
    in_background(format!("open browser {url}"));
    true
}

/// Asks Moon to draw its status bar over the app in the theme's background
/// with light content (it lasts as long as this process).
pub fn status_bar(color: &str) {
    in_background(format!("statusbar color {color} light"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_links_open_in_the_browser() {
        assert!(openable("https://example.org/a?b=c"));
        assert!(openable("HTTP://example.org"));
        assert!(!openable("file:///etc/passwd"));
        assert!(!openable("https://exa mple.org"));
        assert!(!openable("javascript:alert(1)"));
        assert!(!open_url("ftp://example.org"));
    }
}
