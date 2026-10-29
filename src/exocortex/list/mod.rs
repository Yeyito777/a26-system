//! The conversation list's logic (WP-B), apart from the list's state
//! (`model/sidebar.rs`) and its drawing (`view/list.rs`): titles, marks and
//! the TUI's activity semantics (`activity.rs`), search (`search.rs`), and
//! the list's commands, verified against what the daemon then says
//! (`actions.rs`).

pub mod actions;
pub mod activity;
pub mod search;
