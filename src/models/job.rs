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
}
