use serde::{Deserialize, Serialize};

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
