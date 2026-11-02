//! The Discord app: `a26-system --app discord`, a client of the Discord
//! bridge (docs/social-protocol.md, §5 Discord), drawn like Discord's
//! mobile app in its dark theme: the Servers tab (rail and channels), the
//! Messages tab (DMs), You (the account and the bridge's settings), and a
//! channel's chat with Discord's markdown, embeds, attachments, reactions
//! and the composer.

mod app;
mod chat;
mod layout;
mod lists;
mod markdown;
mod model;
mod servers;
mod theme;
mod ui;
mod you;

use crate::social::shell;

/// What a view asks the app to do (requests, navigation, the keyboard).
#[derive(Debug, Clone)]
pub enum Action {
    Redraw,
    Animate,
    Haptic,
    Back,
    OpenChat(String),
    OpenForum(String),
    Members(String),
    OpenUrl(String),
    Viewer {
        chat: String,
        message: String,
        index: usize,
    },
    Send {
        chat: String,
        text: String,
        reply_to: Option<String>,
        client_id: String,
    },
    React {
        chat: String,
        message: String,
        emoji: String,
        remove: bool,
    },
    Delete {
        chat: String,
        message: String,
    },
    EditStart(String),
    Copy(String),
    Toast(String),
    ShowKeyboard,
    HideKeyboard,
    SetWrites(bool),
    SetNotifications(bool),
    SetPreviews(bool),
    LoginFromClipboard,
    Logout,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Code (inline or blocks) is in Mono: parsed meanwhile on another core,
    // not by the frame that first scrolls one into view (7 M instructions).
    crate::text::preload(&[crate::text::Weight::Mono]);
    shell::run(app::DiscordApp::new())
}
