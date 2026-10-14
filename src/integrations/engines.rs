use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::models::{Engine, GithubStatus, State};
use crate::util::report;

pub fn run_interactive(program: &str, args: &[&str]) -> Result<()> {
    report::info("auth", format!("starting {program} {}", args.join(" ")));
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("spawn {program}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

pub fn command_ok(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn claude_logged_in() -> bool {
    command_ok("claude", &["auth", "status"])
}

pub fn codex_logged_in() -> bool {
    command_ok("codex", &["login", "status"])
}

pub fn gh_logged_in() -> bool {
    command_ok("gh", &["auth", "status"])
}

pub fn gh_user() -> Option<String> {
    let output = Command::new("gh")
        .args(["api", "user", "-q", ".login"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

pub fn login_claude(state: &mut State) -> Result<()> {
    run_interactive("claude", &["auth", "login"])?;
    state.engines.claude.ready = claude_logged_in();
    if !state.engines.claude.ready {
        bail!("claude auth status is still not logged in");
    }
    if state.current.engine.is_none() {
        state.current.engine = Some(Engine::Claude);
    }
    report::info("auth", "claude ready (credentials stay in ~/.claude)");
    Ok(())
}

pub fn login_codex(state: &mut State) -> Result<()> {
    run_interactive("codex", &["login", "--device-auth"])?;
    state.engines.codex.ready = codex_logged_in();
    if !state.engines.codex.ready {
        bail!("codex login status is still not logged in");
    }
    if state.current.engine.is_none() {
        state.current.engine = Some(Engine::Codex);
    }
    report::info("auth", "codex ready (credentials stay in ~/.codex)");
    Ok(())
}

pub fn login_github(state: &mut State) -> Result<()> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "auth",
        "login",
        "--hostname",
        "github.com",
        "--git-protocol",
        "https",
        "--web",
    ]);
    cmd.env("BROWSER", "true");
    cmd.env("GH_BROWSER", "true");
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = cmd.status().context("spawn gh auth login")?;
    if !status.success() {
        bail!("gh auth login exited with {status}");
    }

    let _ = Command::new("gh")
        .args(["auth", "setup-git", "--hostname", "github.com"])
        .status();

    refresh_github(state)?;
    if !state.github.ready {
        bail!("gh auth status is still not logged in");
    }
    report::info(
        "auth",
        format!(
            "github ready as {}",
            state.github.user.as_deref().unwrap_or("unknown")
        ),
    );
    Ok(())
}

pub fn refresh_github(state: &mut State) -> Result<()> {
    state.github = GithubStatus {
        ready: gh_logged_in(),
        user: gh_user(),
    };
    Ok(())
}

pub fn refresh_engines(state: &mut State) {
    if which::which("claude").is_ok() {
        state.engines.claude.ready = claude_logged_in();
    }
    if which::which("codex").is_ok() {
        state.engines.codex.ready = codex_logged_in();
    }
    if state.current.engine.is_none() {
        state.current.engine = state.pick_default_engine();
    }
}
