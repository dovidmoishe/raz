use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Claude,
    Codex,
}

impl Engine {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EngineStatus {
    pub ready: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Engines {
    pub claude: EngineStatus,
    pub codex: EngineStatus,
}
