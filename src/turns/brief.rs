use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::integrations::git;
use crate::models::{Engine, Job, Project, State};
use crate::util::{clip_chars, normalize, under_root};

pub fn resolve_cwd(project: &Project) -> Result<PathBuf> {
    cwd_from_root(Path::new(&project.path), project)
}

pub fn resolve_job_cwd(state: &State, job: &Job) -> Result<PathBuf> {
    let project = state.projects.get(&job.project).ok_or_else(|| {
        anyhow::anyhow!("unknown project {}", job.project)
    })?;
    let root = if let Some(wt) = job.worktree.as_deref() {
        under_root(Path::new(&state.workspace_root), Path::new(wt))?
    } else {
        PathBuf::from(&project.path)
    };
    cwd_from_root(&root, project)
}

fn cwd_from_root(root: &Path, project: &Project) -> Result<PathBuf> {
    if !root.is_dir() {
        bail!("project path is missing — /add again or /clone");
    }
    let cwd = if project.cwd.is_empty() || project.cwd == "." {
        normalize(root)
    } else {
        under_root(root, Path::new(&project.cwd)).map_err(|_| {
            anyhow::anyhow!("bad cwd {} — /cwd to fix", project.cwd)
        })?
    };
    if !cwd.is_dir() {
        bail!("cwd {} is missing — /cwd to fix", project.cwd);
    }
    if is_root(&cwd) {
        bail!("refusing to spawn in {}", cwd.display());
    }
    if let Some(home) = dirs::home_dir() {
        if normalize(&home) == cwd {
            bail!("refusing to spawn in the home directory");
        }
    }
    Ok(cwd)
}

pub fn wrap_prompt(state: &State, engine: Engine, project_name: &str, user: &str) -> String {
    wrap_prompt_ctx(state, None, engine, project_name, None, None, user)
}

pub fn wrap_job_prompt(state: &State, job_id: &str, user: &str) -> String {
    let Some(job) = state.jobs.get(job_id) else {
        return user.to_string();
    };
    wrap_prompt_ctx(
        state,
        Some(job_id),
        job.engine,
        &job.project,
        job.worktree.as_deref(),
        Some(job.perm.as_str()),
        user,
    )
}

fn wrap_prompt_ctx(
    state: &State,
    job_id: Option<&str>,
    engine: Engine,
    project_name: &str,
    worktree: Option<&str>,
    perm: Option<&str>,
    user: &str,
) -> String {
    let Some(project) = state.projects.get(project_name) else {
        return user.to_string();
    };
    let inspect_path = worktree
        .map(Path::new)
        .unwrap_or_else(|| Path::new(&project.path));
    let cwd = if let Some(id) = job_id {
        state
            .jobs
            .get(id)
            .and_then(|job| resolve_job_cwd(state, job).ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| inspect_path.display().to_string())
    } else {
        resolve_cwd(project)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| project.path.clone())
    };
    let info = git::inspect(inspect_path);
    let default_branch = if project.default_branch.is_empty() {
        info.default_branch.as_str()
    } else {
        project.default_branch.as_str()
    };
    let perm = perm.unwrap_or(project.perm.as_str());
    let path_line = worktree.unwrap_or(project.path.as_str());
    let mut lines = vec![
        "You are a raz job. Telegram is the client. There is no chat transcript to replay.".into(),
        format!("engine: {}", engine.as_str()),
        format!("project: {project_name}"),
        format!("path: {path_line}"),
        format!("cwd: {cwd}"),
        format!("remote: {}", info.remote.as_deref().unwrap_or("-")),
        format!(
            "branch: {}",
            info.branch.as_deref().unwrap_or(if info.is_repo {
                "detached?"
            } else {
                "not a git repo"
            })
        ),
        format!("dirty: {}", info.dirty),
        format!("perm: {perm}"),
        format!("default_branch: {default_branch}"),
        format!("last_pr: {}", project.last_pr.as_deref().unwrap_or("none")),
        format!("When shipping: branch off {default_branch}, never push {default_branch}, open a PR with gh."),
        "Do not force-push. Do not delete a repository.".into(),
    ];
    if let Some(id) = job_id {
        lines.insert(1, format!("job: {id}"));
    }
    if let Some(wt) = worktree {
        lines.push(format!("worktree: {wt}"));
    }
    if state.current.handoff {
        if let Some(summary) = project.last_summary.as_deref() {
            lines.push(format!("handoff: {summary}"));
        } else if let Some(pr) = project.last_pr.as_deref() {
            lines.push(format!("handoff: last PR {pr}"));
        }
    }
    lines.push(String::new());
    lines.push("Task:".into());
    lines.push(user.to_string());
    lines.join("\n")
}

pub fn one_line_summary(text: &str) -> String {
    let flat: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    clip_chars(&flat, 280)
}

fn is_root(path: &Path) -> bool {
    let n = normalize(path);
    n.parent().is_none()
        || n == Path::new("/")
        || n.as_os_str().len() <= 3 && n.to_string_lossy().ends_with(':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Engine, State};
    use crate::projects;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn wrap_job_includes_id_and_worktree() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        let mut state = State::new(dir.path());
        projects::register(&mut state, "shop", "shop").unwrap();
        state.jobs.insert(
            "j_ab3".into(),
            crate::models::Job {
                project: "shop".into(),
                engine: Engine::Claude,
                session: None,
                worktree: Some(dir.path().join(".worktrees/shop/j_ab3").display().to_string()),
                perm: "write".into(),
                kind: "ship".into(),
                state: "queued".into(),
                telegram_status_id: None,
                chat_id: None,
                prompt: "go".into(),
                queued_prompt: None,
                last_line: None,
                last_status_unix: None,
                verbose: false,
            },
        );
        let wrapped = wrap_job_prompt(&state, "j_ab3", "fix the cart");
        assert!(wrapped.contains("job: j_ab3"));
        assert!(wrapped.contains("worktree:"));
        assert!(wrapped.contains("perm: write"));
        assert!(wrapped.contains("Task:\nfix the cart"));
    }

    #[test]
    fn wrap_includes_routing_not_history() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        let mut state = State::new(dir.path());
        projects::register(&mut state, "shop", "shop").unwrap();
        state.projects.get_mut("shop").unwrap().last_pr =
            Some("https://github.com/acme/shop/pull/4".into());
        state.projects.get_mut("shop").unwrap().last_summary = Some("opened the cart PR".into());
        state.current.handoff = true;
        let wrapped = wrap_prompt(&state, Engine::Claude, "shop", "fix the cart");
        assert!(wrapped.contains("engine: claude"));
        assert!(wrapped.contains("project: shop"));
        assert!(wrapped.contains("perm: read"));
        assert!(wrapped.contains("last_pr: https://github.com/acme/shop/pull/4"));
        assert!(wrapped.contains("handoff: opened the cart PR"));
        assert!(wrapped.contains("Task:\nfix the cart"));
        assert!(!wrapped.contains("telegram history"));
        assert!(wrapped.contains("When shipping: branch off"));
        assert!(wrapped.contains("never push"));
        assert!(wrapped.contains("open a PR with gh"));
        assert!(wrapped.contains("Do not force-push. Do not delete a repository."));
        assert!(wrapped.contains("default_branch:"));
    }

    #[test]
    fn resolve_cwd_stays_in_project() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop/src")).unwrap();
        let mut state = State::new(dir.path());
        projects::register(&mut state, "shop", "shop").unwrap();
        let project = &state.projects["shop"];
        let cwd = resolve_cwd(project).unwrap();
        assert!(cwd.ends_with("shop"));
        let mut project = project.clone();
        project.cwd = "src".into();
        assert!(resolve_cwd(&project).unwrap().ends_with("src"));
        project.cwd = "../..".into();
        assert!(resolve_cwd(&project).is_err());
    }
}
