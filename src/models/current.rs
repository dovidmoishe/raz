use serde::{Deserialize, Serialize};

use super::Engine;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Current {
    pub engine: Option<Engine>,
    pub project: Option<String>,
}
