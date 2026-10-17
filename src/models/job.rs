use serde::{Deserialize, Serialize};

use super::Engine;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub project: String,
    pub engine: Engine,
    pub session: Option<String>,
    pub worktree: Option<String>,
    pub perm: String,
    pub kind: String,
    pub state: String,
    pub telegram_status_id: Option<i64>,
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub queued_prompt: Option<String>,
    #[serde(default)]
    pub last_line: Option<String>,
    #[serde(default)]
    pub last_status_unix: Option<u64>,
    #[serde(default)]
    pub verbose: bool,
}

impl Job {
    pub fn occupies_checkout(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running")
    }
}
