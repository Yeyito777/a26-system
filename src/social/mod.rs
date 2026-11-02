//! What Moon's social apps share (docs/social-protocol.md): the bridge
//! protocol's objects (`proto`), the connection to a bridge (`link`), the
//! pictures (`media`), the event loop (`shell`) and a plain chat client to
//! start an app from (`starter`). The apps are `crate::instagram`,
//! `crate::whatsapp` and `crate::discord`.

pub mod link;
pub mod media;
pub mod proto;
pub mod shell;
pub mod starter;
