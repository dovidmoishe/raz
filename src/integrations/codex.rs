//! Codex runner. `codex exec --json` only.
//!
//! Non-goals: no websocket, no app-server, no second transcript store.
//! Resume uses `-c sandbox_mode=...` because `exec resume` does not take
//! `--sandbox`. Never `danger-full-access`.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Result};

pub fn sandbox_mode(perm: &str) -> &'static str {
    if perm == "write" {
        "workspace-write"
    } else {
        "read-only"
    }
}

pub fn args(prompt: &str, session: Option<&str>, perm: &str) -> Vec<String> {
    let sandbox = sandbox_mode(perm);
    let mut args = vec!["exec".into()];
    if let Some(id) = session {
        args.push("resume".into());
        args.push(id.to_string());
        args.push("-c".into());
        args.push(format!("sandbox_mode={sandbox}"));
    } else {
        args.push("--sandbox".into());
        args.push(sandbox.into());
    }
    args.extend([
        "--json".into(),
        "-c".into(),
        "approval_policy=never".into(),
        "--skip-git-repo-check".into(),
        prompt.to_string(),
    ]);
    args
}

pub fn command(prompt: &str, session: Option<&str>, perm: &str, cwd: &Path) -> Result<Command> {
    if perm.contains("danger") || perm.contains("full-access") || perm == "yolo" {
        bail!("danger-full-access is not a v1 command");
    }
    let mut cmd = Command::new("codex");
    cmd.args(args(prompt, session, perm));
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_turn_uses_sandbox_flag() {
        let args = args("ship", None, "write");
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--sandbox".into()));
        assert!(args.contains(&"workspace-write".into()));
        assert!(!args.iter().any(|a| a.contains("danger")));
        assert!(!args.contains(&"resume".into()));
    }

    #[test]
    fn resume_uses_config_override() {
        let args = args("continue", Some("thr-9"), "read");
        assert!(args.contains(&"resume".into()));
        assert!(args.contains(&"thr-9".into()));
        assert!(args.contains(&"sandbox_mode=read-only".into()));
        assert!(!args.contains(&"--sandbox".into()));
    }
}
