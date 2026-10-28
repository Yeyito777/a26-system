//! The Exocortex app: `a26-system --app exocortex`, a client for the
//! user's Exocortex daemon (its conversations, models and replies), drawn
//! like the System and Messages apps. `exocortex-notify` (src/bin) shares
//! the protocol, socket and hand-off modules.

pub mod app;
pub mod handoff;
pub mod line;
pub mod link;
pub mod markdown;
pub mod model;
pub mod proto;
pub mod view;
