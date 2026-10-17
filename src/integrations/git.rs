use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitInfo {
    pub remote: Option<String>,
    pub default_branch: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub is_repo: bool,
}

pub fn inspect(path: &Path) -> GitInfo {
    if !git_ok(path, &["rev-parse", "--is-inside-work-tree"]) {
        return GitInfo {
            default_branch: "main".into(),
            ..GitInfo::default()
        };
    }

    let remote = git_stdout(path, &["remote", "get-url", "origin"]);
    let branch = git_stdout(path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = git_stdout(path, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let default_branch = git_stdout(path, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        .and_then(|head| head.rsplit('/').next().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".into());

    GitInfo {
        remote,
        default_branch,
        branch,
        dirty,
        is_repo: true,
    }
}

fn git_ok(path: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_stdout(path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub fn worktree_add(repo: &Path, dest: &Path, branch: &str, start: &str) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).context("create worktree parent")?;
    }
    if dest.exists() {
        bail!("worktree path {} already exists", dest.display());
    }
    let start = if start.is_empty() { "HEAD" } else { start };
    let output = git_worktree(repo, dest, branch, start);
    let output = match output {
        Ok(out) if out.status.success() => out,
        Ok(_) | Err(_) if start != "HEAD" => git_worktree(repo, dest, branch, "HEAD")
            .context("git worktree add")?,
        Ok(out) => out,
        Err(err) => return Err(err).context("git worktree add"),
    };
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "need a git repo to overlap writes — use perm=read, or /clone{}",
            if err.is_empty() {
                String::new()
            } else {
                format!(" ({err})")
            }
        );
    }
    Ok(())
}

fn git_worktree(
    repo: &Path,
    dest: &Path,
    branch: &str,
    start: &str,
) -> std::io::Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "-b", branch])
        .arg(dest)
        .arg(start)
        .output()
}

pub fn worktree_remove(repo: &Path, dest: &Path) -> Result<()> {
    if repo.is_dir() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "remove", "--force"])
            .arg(dest)
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "prune"])
            .status();
    }
    if dest.exists() {
        fs::remove_dir_all(dest).with_context(|| {
            format!("remove worktree {}", dest.display())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn git_init_commit(dir: &Path) {
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        fs::write(dir.join("README"), "shop").unwrap();
        assert!(Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "raz")
            .env("GIT_AUTHOR_EMAIL", "raz@test")
            .env("GIT_COMMITTER_NAME", "raz")
            .env("GIT_COMMITTER_EMAIL", "raz@test")
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["-c", "commit.gpgsign=false", "commit", "-m", "init"])
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "raz")
            .env("GIT_AUTHOR_EMAIL", "raz@test")
            .env("GIT_COMMITTER_NAME", "raz")
            .env("GIT_COMMITTER_EMAIL", "raz@test")
            .status()
            .unwrap()
            .success());
    }

    #[test]
    fn worktree_add_and_remove() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("shop");
        fs::create_dir_all(&repo).unwrap();
        git_init_commit(&repo);
        let dest = dir.path().join(".worktrees/shop/j_ab3");
        worktree_add(&repo, &dest, "raz/j_ab3", "HEAD").unwrap();
        assert!(dest.join("README").is_file());
        worktree_remove(&repo, &dest).unwrap();
        assert!(!dest.exists());
    }
}
