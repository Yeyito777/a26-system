//! The Instagram app: `a26-system --app instagram`, a client of the
//! Instagram bridge (docs/social-protocol.md), in Instagram's own dark look:
//! Home (the stories tray and the feed), Messages, Search and Profile, with
//! Activity behind Home's heart, and no Reels anywhere.

mod activity;
mod app;
mod direct;
mod feed;
mod model;
mod paint;
mod post;
mod profile;
mod search;
mod settings;
mod stories;
mod store;

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    crate::social::shell::run(app::Instagram::new())
}
