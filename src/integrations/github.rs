use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use anyhow::{bail, Context, Result};

use crate::models::State;
use crate::util::report;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicePrompt {
    pub code: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub git_ok: bool,
    pub git_detail: String,
    pub gh_ok: bool,
    pub gh_detail: String,
}

pub enum LoginTick {
    Prompt(DevicePrompt),
    Done { ok: bool, detail: String },
}

enum IoMsg {
    Line(String),
    Dead,
}

pub struct LoginChild {
    child: Child,
    rx: Receiver<IoMsg>,
    pub chat_id: i64,
    pub message_id: Option<i64>,
    sent_prompt: bool,
    acc: String,
}

impl LoginChild {
    pub fn spawn(chat_id: i64) -> Result<Self> {
        let mut cmd = login_command();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().context("spawn gh auth login")?;
        let stdout = child.stdout.take().context("gh stdout")?;
        let stderr = child.stderr.take().context("gh stderr")?;
        Ok(Self {
            child,
            rx: pump(stdout, stderr),
            chat_id,
            message_id: None,
            sent_prompt: false,
            acc: String::new(),
        })
    }

    pub fn tick(&mut self) -> Option<LoginTick> {
        loop {
            match self.rx.try_recv() {
                Ok(IoMsg::Line(line)) => {
                    self.acc.push_str(&line);
                    self.acc.push('\n');
                    if self.acc.len() > 16 * 1024 {
                        let extra = self.acc.len() - 16 * 1024;
                        self.acc.drain(..extra);
                    }
                }
                Ok(IoMsg::Dead) => break,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        if !self.sent_prompt {
            if let Some(prompt) = parse_device_output(&self.acc) {
                self.sent_prompt = true;
                return Some(LoginTick::Prompt(prompt));
            }
        }

        // stdout can close (or go quiet) while gh still waits for the device
        // flow. Never block here — serve has to keep long-polling Telegram.
        let Some(status) = self.child.try_wait().ok().flatten() else {
            return None;
        };
        let code = status.code().unwrap_or(-1);
        let ok = code == 0;
        let detail = if ok {
            "github login finished".into()
        } else if self.acc.to_lowercase().contains("expired") {
            "github login expired. try /github again".into()
        } else if !self.sent_prompt {
            clip_err(&self.acc).unwrap_or_else(|| "github login failed".into())
        } else {
            "github login failed".into()
        };
        Some(LoginTick::Done { ok, detail })
    }

    pub fn stop(&mut self) {
        terminate_child(&mut self.child);
        let _ = self.child.wait();
    }
}

impl Drop for LoginChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn login_command() -> Command {
    let mut cmd = Command::new("gh");
    cmd.args([
        "auth",
        "login",
        "--hostname",
        "github.com",
        "--git-protocol",
        "https",
        "--device",
    ]);
    cmd
}

pub fn setup_git() {
    let _ = Command::new("gh")
        .args(["auth", "setup-git", "--hostname", "github.com"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn ready_message(state: &State) -> String {
    format!(
        "github ready as {}",
        state.github.user.as_deref().unwrap_or("unknown")
    )
}

pub fn parse_device_output(text: &str) -> Option<DevicePrompt> {
    let url = text.split_whitespace().find_map(|word| {
        let trimmed = word.trim_end_matches(['.', ',', ';', ')', ']', '"', '\'']);
        if trimmed.contains("github.com/login/device") {
            Some(trimmed.to_string())
        } else {
            None
        }
    })?;
    let code = text.split_whitespace().find_map(|word| {
        let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        is_device_code(trimmed).then(|| trimmed.to_ascii_uppercase())
    })?;
    Some(DevicePrompt { code, url })
}

pub fn format_device_prompt(prompt: &DevicePrompt) -> String {
    format!(
        "github login\nopen {}\ncode: {}",
        prompt.url, prompt.code
    )
}

pub fn probe(cwd: &Path) -> Probe {
    probe_with_path(cwd, None)
}

pub fn probe_with_path(cwd: &Path, path: Option<&Path>) -> Probe {
    let git = run("git", &["status"], cwd, path);
    let gh = run("gh", &["auth", "status"], cwd, path);
    Probe {
        git_ok: git.is_ok(),
        git_detail: match &git {
            Ok(_) => "git status ok".into(),
            Err(err) => err.clone(),
        },
        gh_ok: gh.is_ok(),
        gh_detail: match &gh {
            Ok(_) => "gh auth status ok".into(),
            Err(err) => err.clone(),
        },
    }
}

fn run(program: &str, args: &[&str], cwd: &Path, path: Option<&Path>) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null());
    if let Some(path) = path {
        cmd.env("PATH", path);
        cmd.env("Path", path);
    }
    let output = cmd
        .output()
        .map_err(|err| format!("{program} missing — {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        let msg = if err.trim().is_empty() {
            out.trim()
        } else {
            err.trim()
        };
        if msg.is_empty() {
            Err(format!("{program} failed with {}", output.status))
        } else {
            Err(clip_err(msg).unwrap_or_else(|| format!("{program} failed")))
        }
    }
}

fn is_device_code(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 9
        && b[4] == b'-'
        && b[..4].iter().all(|c| c.is_ascii_alphanumeric())
        && b[5..].iter().all(|c| c.is_ascii_alphanumeric())
}

fn clip_err(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    Some(if line.chars().count() > 200 {
        crate::util::clip_chars(line, 200)
    } else {
        line.to_string()
    })
}

fn pump(stdout: std::process::ChildStdout, stderr: std::process::ChildStderr) -> Receiver<IoMsg> {
    let (tx, rx) = mpsc::channel();
    let tx_err = tx.clone();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(IoMsg::Line(line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(IoMsg::Dead);
    });
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => {
                    if tx_err.send(IoMsg::Line(line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("kill").args(["-TERM", &pid]).status();
        thread::sleep(std::time::Duration::from_millis(150));
        let _ = child.kill();
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

pub fn finish_success(state: &mut State) -> Result<String> {
    setup_git();
    crate::integrations::engines::refresh_github(state)?;
    if !state.github.ready {
        bail!("github login finished but gh auth status is still not logged in");
    }
    report::info(
        "github",
        format!(
            "ready as {}",
            state.github.user.as_deref().unwrap_or("unknown")
        ),
    );
    Ok(ready_message(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_gh_device_prompt() {
        let text = "! First copy your one-time code: ABCD-EFGH\nOpen this URL to continue in your web browser: https://github.com/login/device\n";
        let prompt = parse_device_output(text).unwrap();
        assert_eq!(prompt.code, "ABCD-EFGH");
        assert_eq!(prompt.url, "https://github.com/login/device");
        assert!(format_device_prompt(&prompt).contains("ABCD-EFGH"));
    }

    #[test]
    fn probe_fails_when_path_is_empty() {
        let dir = TempDir::new().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let probe = probe_with_path(dir.path(), Some(&empty));
        assert!(!probe.git_ok, "{}", probe.git_detail);
        assert!(!probe.gh_ok, "{}", probe.gh_detail);
        assert!(
            probe.git_detail.contains("git") || probe.git_detail.contains("missing"),
            "{}",
            probe.git_detail
        );
    }
}
