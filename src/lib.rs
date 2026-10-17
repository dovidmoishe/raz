//! raz crate layout:
//!
//! - [`models`] — persisted and wire structs
//! - [`store`] — `state.json` load/save
//! - [`cmd`] — CLI verbs
//! - [`integrations`] — Claude, Codex, GitHub, Telegram, systemd
//! - [`projects`] — workspace catalog
//! - [`util`] — logging, path jail, secret files

pub mod cmd;
pub mod integrations;
pub mod models;
pub mod projects;
pub mod store;
pub mod util;

pub use cmd::run;
