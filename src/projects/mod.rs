use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::integrations::git;
use crate::models::{Project, ProjectSessions, State};
use crate::util::{normalize, under_root};

pub fn valid_name(name: &str) -> Result<()> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 48 {
        bail!("project name must be 1–48 characters");
    }
    let first = bytes[0];
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        bail!("project name must start with a lowercase letter or digit");
    }
    if !bytes[1..]
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
    {
        bail!("project name may only contain lowercase letters, digits, _ and -");
    }
    Ok(())
}

pub fn parse_owner_repo(spec: &str) -> Result<(String, String)> {
    let mut parts = spec.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        bail!("clone target must be owner/repo");
    }
    let repo = repo.trim_end_matches(".git");
    if repo.is_empty() {
        bail!("clone target must be owner/repo");
    }
    Ok((owner.to_string(), repo.to_string()))
}

pub fn clone_dest(workspace: &Path, name: &str) -> Result<PathBuf> {
    valid_name(name)?;
    let dest = under_root(workspace, Path::new(name))?;
    if dest.exists() {
        bail!("clone destination already exists: {}", dest.display());
    }
    Ok(dest)
}

pub fn register(state: &mut State, name: &str, rel: &str) -> Result<String> {
    valid_name(name)?;
    let root = Path::new(&state.workspace_root);
    let child = if rel.is_empty() { name } else { rel };
    let path = under_root(root, Path::new(child))?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }

    let info = git::inspect(&path);
    if let Some(existing) = state.projects.get_mut(name) {
        if normalize(Path::new(&existing.path)) != normalize(&path) {
            bail!("project {name} already points at {}", existing.path);
        }
        existing.remote = info.remote.clone();
        existing.default_branch = info.default_branch.clone();
    } else {
        state.projects.insert(
            name.to_string(),
            Project {
                path: path.to_string_lossy().into_owned(),
                remote: info.remote.clone(),
                default_branch: info.default_branch.clone(),
                cwd: ".".into(),
                perm: "read".into(),
                last_pr: None,
                last_summary: None,
                sessions: ProjectSessions::default(),
            },
        );
    }
    state.current.project = Some(name.to_string());
    Ok(format!("now in {name} ({})", path.display()))
}

pub fn clone(state: &mut State, owner_repo: &str, name: Option<&str>) -> Result<String> {
    let (owner, repo) = parse_owner_repo(owner_repo)?;
    let name = match name {
        Some(n) => {
            valid_name(n)?;
            n.to_string()
        }
        None => slug(&repo),
    };
    valid_name(&name)?;
    let workspace = Path::new(&state.workspace_root);
    let dest = clone_dest(workspace, &name)?;
    let spec = format!("{owner}/{repo}");
    let status = Command::new("gh")
        .args(["repo", "clone", &spec, &dest.to_string_lossy()])
        .stdin(Stdio::null())
        .status()
        .context("spawn gh repo clone")?;
    if !status.success() {
        bail!("gh repo clone {spec} failed with {status}");
    }
    register(state, &name, &name)
}

pub fn switch(state: &mut State, name: &str) -> Result<String> {
    valid_name(name)?;
    if !state.projects.contains_key(name) {
        bail!("unknown project {name}");
    }
    state.current.project = Some(name.to_string());
    Ok(format!("now in {name}"))
}

pub fn set_cwd(state: &mut State, rel: &str) -> Result<String> {
    let name = state
        .current
        .project
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no current project — /project <name> or /add first"))?;
    let project = state
        .projects
        .get_mut(&name)
        .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
    let root = Path::new(&project.path);
    let rel = rel.trim();
    if rel.is_empty() || rel == "." {
        project.cwd = ".".into();
        return Ok(format!("{name} cwd ."));
    }
    let abs = under_root(root, Path::new(rel))?;
    if !abs.is_dir() {
        bail!("{} is not a directory", abs.display());
    }
    let stored = relative_cwd(root, &abs)?;
    project.cwd = stored.clone();
    Ok(format!("{name} cwd {stored}"))
}

pub fn list_text(state: &State) -> String {
    if state.projects.is_empty() {
        return "no projects. /add <name> or /clone owner/repo".into();
    }
    let current = state.current.project.as_deref();
    let mut lines = vec!["projects:".to_string()];
    for (name, project) in &state.projects {
        let mark = if Some(name.as_str()) == current {
            "*"
        } else {
            " "
        };
        lines.push(format!("{mark} {name}  {}", project.path));
    }
    lines.join("\n")
}

pub fn status_snapshot(state: &State) -> String {
    let engine = state
        .current
        .engine
        .map(|e| e.as_str())
        .unwrap_or("none");
    let Some(name) = state.current.project.as_deref() else {
        return format!(
            "engine: {engine}\nproject: none\nworkspace: {}\n/add or /clone a repo first\n{}",
            state.workspace_root,
            crate::turns::jobs_block(state)
        );
    };
    let Some(project) = state.projects.get(name) else {
        return format!(
            "engine: {engine}\nproject: {name} (missing from catalog)\n{}",
            crate::turns::jobs_block(state)
        );
    };
    let info = git::inspect(Path::new(&project.path));
    let branch = info.branch.as_deref().unwrap_or(if info.is_repo {
        "detached?"
    } else {
        "not a git repo"
    });
    [
        format!("engine: {engine}"),
        format!("project: {name}"),
        format!("path: {}", project.path),
        format!("cwd: {}", project.cwd),
        format!("perm: {}", project.perm),
        format!("branch: {branch}"),
        format!("dirty: {}", info.dirty),
        format!("remote: {}", info.remote.as_deref().unwrap_or("-")),
        format!("default_branch: {}", project.default_branch),
        format!(
            "claude_session: {}",
            project.sessions.claude.as_deref().unwrap_or("-")
        ),
        format!(
            "codex_session: {}",
            project.sessions.codex.as_deref().unwrap_or("-")
        ),
        format!("last_pr: {}", project.last_pr.as_deref().unwrap_or("-")),
        crate::turns::jobs_block(state),
    ]
    .join("\n")
}

pub fn pr_text(state: &State) -> Result<String> {
    let name = state.current.project.as_deref().ok_or_else(|| {
        anyhow::anyhow!("no current project — /add or /clone first")
    })?;
    let project = state
        .projects
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
    Ok(project
        .last_pr
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "none yet".into()))
}

fn slug(repo: &str) -> String {
    repo.trim_end_matches(".git").to_ascii_lowercase()
}

fn relative_cwd(root: &Path, abs: &Path) -> Result<String> {
    let root = normalize(root);
    let abs = normalize(abs);
    let rel = abs
        .strip_prefix(&root)
        .with_context(|| format!("{} is not under {}", abs.display(), root.display()))?;
    if rel.as_os_str().is_empty() {
        Ok(".".into())
    } else {
        Ok(rel.to_string_lossy().replace('\\', "/"))
    }
}

/// Parse a Telegram line into a catalog reply. Returns None if it is not a catalog command.
pub fn handle_line(state: &mut State, text: &str) -> Option<Result<String>> {
    let mut parts = text.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    match cmd {
        "/project" => match parts.next() {
            None => Some(Ok(list_text(state))),
            Some(name) => Some(switch(state, name)),
        },
        "/add" => {
            let Some(name) = parts.next() else {
                return Some(Err(anyhow::anyhow!("usage: /add <name>")));
            };
            Some(register(state, name, name))
        }
        "/clone" => {
            let Some(spec) = parts.next() else {
                return Some(Err(anyhow::anyhow!("usage: /clone owner/repo [name]")));
            };
            Some(clone(state, spec, parts.next()))
        }
        "/cwd" => {
            let rel = parts.collect::<Vec<_>>().join(" ");
            Some(set_cwd(state, &rel))
        }
        "/status" | "/start" => Some(Ok(status_snapshot(state))),
        "/pr" => Some(pr_text(state)),
        "/help" | "/commands" => Some(Ok(help_text())),
        _ => None,
    }
}

pub fn help_text() -> String {
    [
        "/project — list",
        "/project <name> — switch",
        "/add <name> — register workspace/<name>",
        "/clone owner/repo [name]",
        "/cwd [rel] — set or reset cwd",
        "/use claude|codex — switch engine",
        "/new [id] — new session for the current engine, or a job",
        "/stop [id] — stop the default job, or a job id",
        "/perm read|write",
        "/jobs — running and recent jobs",
        "/run claude|codex <prompt> — start a job without flipping /use",
        "/both <prompt> — Claude and Codex as two jobs",
        "/verbose [prompt] — noisier status for this turn",
        "/pr — last pull request URL",
        "/github — GitHub login from Telegram",
        "/status — engine, path, git, sessions, jobs",
        "send a message to run a turn on the current project + engine",
        "reply to a job status to continue that session",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::State;
    use std::fs;
    use tempfile::TempDir;

    fn primed(dir: &Path) -> State {
        fs::create_dir_all(dir).unwrap();
        State::new(dir)
    }

    #[test]
    fn name_rules() {
        assert!(valid_name("shop").is_ok());
        assert!(valid_name("a").is_ok());
        assert!(valid_name("shop-2_ok").is_ok());
        assert!(valid_name("Shop").is_err());
        assert!(valid_name("../etc").is_err());
        assert!(valid_name("a/b").is_err());
        assert!(valid_name(&"x".repeat(49)).is_err());
    }

    #[test]
    fn register_switch_keeps_sessions() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let shop = root.join("shop");
        let blog = root.join("blog");
        fs::create_dir_all(&shop).unwrap();
        fs::create_dir_all(&blog).unwrap();
        let mut state = primed(root);
        register(&mut state, "shop", "shop").unwrap();
        state.projects.get_mut("shop").unwrap().sessions.claude = Some("cla-1".into());
        register(&mut state, "blog", "blog").unwrap();
        state.projects.get_mut("blog").unwrap().sessions.codex = Some("cod-1".into());
        assert_eq!(state.current.project.as_deref(), Some("blog"));
        switch(&mut state, "shop").unwrap();
        assert_eq!(state.current.project.as_deref(), Some("shop"));
        assert_eq!(
            state.projects["shop"].sessions.claude.as_deref(),
            Some("cla-1")
        );
        assert_eq!(
            state.projects["blog"].sessions.codex.as_deref(),
            Some("cod-1")
        );
        assert!(state.projects["shop"].sessions.codex.is_none());
    }

    #[test]
    fn register_and_cwd_refuse_escape() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("shop/src")).unwrap();
        let mut state = primed(root);
        assert!(register(&mut state, "shop", "../etc").is_err());
        register(&mut state, "shop", "shop").unwrap();
        assert!(set_cwd(&mut state, "../blog").is_err());
        assert!(set_cwd(&mut state, "shop/../../etc").is_err());
        assert!(set_cwd(&mut state, "/etc").is_err());
        assert!(set_cwd(&mut state, "src").is_ok());
        assert_eq!(state.projects["shop"].cwd, "src");
        set_cwd(&mut state, "").unwrap();
        assert_eq!(state.projects["shop"].cwd, ".");
    }

    #[test]
    fn clone_dest_collision() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        let err = clone_dest(dir.path(), "shop").unwrap_err().to_string();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn parse_spec() {
        assert_eq!(
            parse_owner_repo("acme/shop").unwrap(),
            ("acme".into(), "shop".into())
        );
        assert_eq!(
            parse_owner_repo("acme/shop.git").unwrap(),
            ("acme".into(), "shop".into())
        );
        assert!(parse_owner_repo("shop").is_err());
        assert!(parse_owner_repo("a/b/c").is_err());
    }

    #[test]
    fn pr_shows_url_or_none() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        let mut state = primed(dir.path());
        let err = pr_text(&state).unwrap_err().to_string();
        assert!(err.contains("no current project"), "{err}");
        register(&mut state, "shop", "shop").unwrap();
        assert_eq!(pr_text(&state).unwrap(), "none yet");
        state.projects.get_mut("shop").unwrap().last_pr =
            Some("https://github.com/acme/shop/pull/4".into());
        assert_eq!(
            pr_text(&state).unwrap(),
            "https://github.com/acme/shop/pull/4"
        );
    }

    #[test]
    fn status_and_help_are_flush_left() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        let mut state = primed(dir.path());
        register(&mut state, "shop", "shop").unwrap();
        let status = status_snapshot(&state);
        for line in status.lines() {
            assert_eq!(line.trim_start(), line, "{line:?}");
        }
        assert!(status.contains("project: shop"));
        for line in help_text().lines() {
            assert_eq!(line.trim_start(), line, "{line:?}");
        }
    }
}
