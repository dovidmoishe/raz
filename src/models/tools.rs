use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Tools {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    pub gh: Option<PathBuf>,
    pub git: Option<PathBuf>,
}

impl Tools {
    pub fn has(&self, name: &str) -> bool {
        match name {
            "claude" => self.claude.is_some(),
            "codex" => self.codex.is_some(),
            "gh" => self.gh.is_some(),
            "git" => self.git.is_some(),
            _ => false,
        }
    }
}
