use std::path::PathBuf;

use anyhow::Result;

#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub state: PathBuf,
    pub token: PathBuf,
    pub workspace: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let home = match std::env::var_os("POCKET_CODE_HOME") {
            Some(value) => PathBuf::from(value),
            None => dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?
                .join(".pocket-code"),
        };

        let workspace = match std::env::var_os("POCKET_CODE_WORKSPACE") {
            Some(value) => PathBuf::from(value),
            None => home.join("projects"),
        };

        Ok(Self {
            state: home.join("state.json"),
            token: home.join("telegram.token"),
            home,
            workspace,
        })
    }

    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home)?;
        std::fs::create_dir_all(&self.workspace)?;
        Ok(())
    }

    pub fn telegram_token(&self) -> Result<Option<String>> {
        if let Ok(value) = std::env::var("POCKET_CODE_TELEGRAM_TOKEN") {
            let trimmed = value.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed));
            }
        }
        if self.token.exists() {
            let raw = std::fs::read_to_string(&self.token)?;
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed));
            }
        }
        Ok(None)
    }
}
