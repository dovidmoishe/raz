use serde::{Deserialize, Serialize};

use super::Engine;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Current {
    pub engine: Option<Engine>,
    pub project: Option<String>,
    /// One-line last_summary/last_pr brief on the next turn after `/use`.
    #[serde(default)]
    pub handoff: bool,
}
