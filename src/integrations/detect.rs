use crate::models::Tools;

pub fn probe() -> Tools {
    Tools {
        claude: which::which("claude").ok(),
        codex: which::which("codex").ok(),
        gh: which::which("gh").ok(),
        git: which::which("git").ok(),
    }
}

pub fn install_hint(name: &str) -> &'static str {
    match name {
        "claude" if cfg!(windows) => "irm https://claude.ai/install.ps1 | iex",
        "claude" => "curl -fsSL https://claude.ai/install.sh | bash",
        "codex" => "npm install -g @openai/codex",
        "gh" if cfg!(windows) => "winget install --id GitHub.cli",
        "gh" => "install from https://cli.github.com  (e.g. sudo apt install gh)",
        "git" => "install from https://git-scm.com/downloads",
        _ => "",
    }
}
