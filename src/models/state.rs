use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{Current, Engine, Engines, GithubStatus, Job, Project, TelegramState};

pub const SCHEMA_VERSION: u32 = 1;
pub const PAIRING_TTL_SECS: u64 = 20 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub telegram: TelegramState,
    pub engines: Engines,
    pub github: GithubStatus,
    pub workspace_root: String,
    pub current: Current,
    #[serde(default)]
    pub setup: super::SetupProgress,
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
    #[serde(default)]
    pub jobs: BTreeMap<String, Job>,
}

impl State {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            version: SCHEMA_VERSION,
            telegram: TelegramState::default(),
            engines: Engines::default(),
            github: GithubStatus::default(),
            workspace_root: workspace_root.to_string_lossy().into_owned(),
            current: Current::default(),
            setup: super::SetupProgress::default(),
            projects: BTreeMap::new(),
            jobs: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != SCHEMA_VERSION {
            bail!(
                "state.json version {} is unsupported (want {SCHEMA_VERSION}); refusing to migrate",
                self.version
            );
        }
        Ok(())
    }

    pub fn any_engine_ready(&self) -> bool {
        self.engines.claude.ready || self.engines.codex.ready
    }

    pub fn pick_default_engine(&self) -> Option<Engine> {
        if self.engines.claude.ready {
            Some(Engine::Claude)
        } else if self.engines.codex.ready {
            Some(Engine::Codex)
        } else {
            None
        }
    }

    pub fn set_pairing_code(&mut self, code: String) {
        self.telegram.pairing_code = Some(code);
        self.telegram.pairing_expires_unix = Some(now_unix().saturating_add(PAIRING_TTL_SECS));
    }

    pub fn pairing_is_live(&self) -> bool {
        match (
            self.telegram.pairing_code.as_deref(),
            self.telegram.pairing_expires_unix,
        ) {
            (Some(code), Some(expires)) if !code.is_empty() => now_unix() < expires,
            _ => false,
        }
    }

    pub fn consume_pairing(&mut self, user_id: i64) {
        self.telegram.allowed_user_ids = vec![user_id];
        self.telegram.paired = true;
        self.telegram.pairing_code = None;
        self.telegram.pairing_expires_unix = None;
    }

    pub fn is_allowed(&self, user_id: i64) -> bool {
        self.telegram.paired && self.telegram.allowed_user_ids.contains(&user_id)
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn new_pairing_code() -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}
