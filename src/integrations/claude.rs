//! Claude runner. Headless `claude -p` only.
//!
//! Non-goals: no websocket, no app-server, no Claude Remote Control, no second
//! transcript store. Session memory stays in Claude. We only keep the session id.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Result};

pub fn permission_mode(perm: &str) -> &'static str {
    if perm == "write" {
        "acceptEdits"
    } else {
        "plan"
    }
}

pub fn args(prompt: &str, session: Option<&str>, perm: &str) -> Vec<String> {
    let mut args = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--permission-mode".into(),
        permission_mode(perm).into(),
        "--permission-prompts".into(),
        "none".into(),
    ];
    if let Some(id) = session {
        args.push("--resume".into());
        args.push(id.to_string());
    }
    args.push(prompt.to_string());
    args
}

pub fn command(prompt: &str, session: Option<&str>, perm: &str, cwd: &Path) -> Result<Command> {
    refuse_yolo(perm)?;
    let mut cmd = Command::new("claude");
    cmd.args(args(prompt, session, perm));
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}

fn refuse_yolo(perm: &str) -> Result<()> {
    if perm == "yolo"
        || perm.contains("bypass")
        || perm.contains("danger")
        || perm.contains("full-access")
    {
        bail!("danger-full-access is not a v1 command");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_is_accept_edits_not_yolo() {
        let args = args("do it", Some("cla-1"), "write");
        assert!(args.contains(&"--resume".into()));
        assert!(args.contains(&"cla-1".into()));
        assert!(args.contains(&"acceptEdits".into()));
        assert!(!args.iter().any(|a| a.contains("bypass") || a.contains("danger")));
    }

    #[test]
    fn read_is_plan() {
        let args = args("look", None, "read");
        assert!(args.contains(&"plan".into()));
        assert!(!args.contains(&"--resume".into()));
    }
}
