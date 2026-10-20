mod fs;
mod serve_lock;

pub use fs::{append_jsonl, atomic_write, is_world_readable, write_secret};
pub use serve_lock::acquire_serve_lock;

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::models::{Paths, SCHEMA_VERSION, State};

impl State {
    pub fn load(path: &Path) -> Result<Self> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let value: Value =
            serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if version != SCHEMA_VERSION as u64 {
            bail!(
                "state.json version {version} is unsupported (want {SCHEMA_VERSION}); refusing to migrate"
            );
        }
        let state: State =
            serde_json::from_value(value).with_context(|| format!("parse {}", path.display()))?;
        state.validate()?;
        Ok(state)
    }

    pub fn load_or_init(paths: &Paths) -> Result<Self> {
        if paths.state.exists() {
            let mut state = Self::load(&paths.state)?;
            if state.workspace_root.is_empty() {
                state.workspace_root = paths.workspace.to_string_lossy().into_owned();
            }
            Ok(state)
        } else {
            Ok(Self::new(&paths.workspace))
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let body = serde_json::to_vec_pretty(self)?;
        atomic_write(path, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Engine;
    use tempfile::TempDir;

    #[test]
    fn rejects_unknown_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"version":99,"telegram":{},"engines":{},"github":{},"workspace_root":"/tmp","current":{}}"#,
        )
        .unwrap();
        let err = State::load(&path).unwrap_err().to_string();
        assert!(err.contains("unsupported"), "{err}");
    }

    #[test]
    fn roundtrip_atomic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        let mut state = State::new(dir.path());
        state.engines.claude.ready = true;
        state.current.engine = Some(Engine::Claude);
        state.setup.github_skipped = true;
        state.setup.service_configured = true;
        state.save(&path).unwrap();
        let loaded = State::load(&path).unwrap();
        assert!(loaded.engines.claude.ready);
        assert_eq!(loaded.current.engine, Some(Engine::Claude));
        assert!(loaded.jobs.is_empty());
        assert!(loaded.setup.github_skipped);
        assert!(loaded.setup.service_configured);
    }

    #[test]
    fn existing_state_without_setup_progress_still_loads() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        let mut value = serde_json::to_value(State::new(dir.path())).unwrap();
        value.as_object_mut().unwrap().remove("setup");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let state = State::load(&path).unwrap();
        assert!(!state.setup.github_skipped);
        assert!(!state.setup.service_skipped);
        assert!(!state.setup.service_configured);
    }
}
