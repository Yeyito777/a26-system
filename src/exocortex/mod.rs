//! The Exocortex app: `a26-system --app exocortex`, a client for the
//! user's Exocortex daemon (its conversations, models and replies), drawn
//! like the System and Messages apps. `exocortex-notify` (src/bin) shares
//! the protocol, socket and hand-off modules.

pub mod anim;
pub mod app;
pub mod clipboard;
pub mod drafts;
pub mod editor;
pub mod find;
pub mod gesture;
pub mod handoff;
pub mod highlight;
pub mod images;
pub mod line;
pub mod link;
pub mod links;
pub mod list;
pub mod markdown;
pub mod math;
pub mod model;
pub mod proto;
pub mod reader;
pub mod requests;
pub mod screens;
pub mod sheets;
pub mod store;
pub mod theme;
pub mod ui;
pub mod view;
pub mod viewer;
