use serde::{Deserialize, Serialize};

use super::Engine;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectSessions {
    pub claude: Option<String>,
    pub codex: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub path: String,
    pub remote: Option<String>,
    pub default_branch: String,
    pub cwd: String,
    pub perm: String,
    pub last_pr: Option<String>,
    pub last_summary: Option<String>,
    pub sessions: ProjectSessions,
}

impl Project {
    pub fn session(&self, engine: Engine) -> Option<&str> {
        match engine {
            Engine::Claude => self.sessions.claude.as_deref(),
            Engine::Codex => self.sessions.codex.as_deref(),
        }
    }

    pub fn session_mut(&mut self, engine: Engine) -> &mut Option<String> {
        match engine {
            Engine::Claude => &mut self.sessions.claude,
            Engine::Codex => &mut self.sessions.codex,
        }
    }
}
