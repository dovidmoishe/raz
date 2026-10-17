//! raz crate layout:
//!
//! - [`models`] — persisted and wire structs
//! - [`store`] — `state.json` load/save
//! - [`cmd`] — CLI verbs
//! - [`integrations`] — Claude, Codex, GitHub, Telegram, systemd
//! - [`projects`] — workspace catalog
//! - [`turns`] — job queue, /use /new /stop /perm /run /both, checkout lock, engine spawn
//! - [`util`] — logging, path jail, rate limit, unix identity

pub mod cmd;
pub mod integrations;
pub mod models;
pub mod projects;
pub mod store;
pub mod turns;
pub mod util;

pub use cmd::run;
