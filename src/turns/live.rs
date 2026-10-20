use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::brief::{one_line_summary, resolve_job_cwd, wrap_job_prompt};
use super::{apply_status, humanize, job_cap};
use crate::integrations::{claude, codex, stream};
use crate::models::{Engine, State, TurnEvent, now_unix};
use crate::store;
use crate::util::report;

pub const TURN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug)]
pub enum JobEvent {
    Status {
        job_id: String,
    },
    Finished {
        job_id: String,
        ok: bool,
        summary: String,
        pr: Option<String>,
    },
}

enum IoMsg {
    Out(String),
    Err(String),
    Dead,
}

struct Running {
    child: Child,
    rx: Receiver<IoMsg>,
    started: Instant,
    acc: String,
    result: String,
    stderr_tail: String,
    eof: bool,
}

pub struct Live {
    children: HashMap<String, Running>,
    pub timeout: Duration,
    events: Option<std::path::PathBuf>,
}

impl Default for Live {
    fn default() -> Self {
        Self::new()
    }
}

impl Live {
    pub fn new() -> Self {
        Self::with_events(None)
    }

    pub fn with_events(events: Option<std::path::PathBuf>) -> Self {
        Self {
            children: HashMap::new(),
            timeout: TURN_TIMEOUT,
            events,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    pub fn recover_orphans(state: &mut State) {
        for job in state.jobs.values_mut() {
            if job.state == "running" {
                job.state = "error".into();
                job.last_line = Some("interrupted (harness restart)".into());
            }
        }
    }

    pub fn start(&mut self, job_id: &str, mut cmd: Command) -> Result<()> {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().context("spawn engine")?;
        let stdout = child.stdout.take().context("engine stdout")?;
        let stderr = child.stderr.take().context("engine stderr")?;
        let rx = pump_stdio(stdout, stderr);
        self.children.insert(
            job_id.to_string(),
            Running {
                child,
                rx,
                started: Instant::now(),
                acc: String::new(),
                result: String::new(),
                stderr_tail: String::new(),
                eof: false,
            },
        );
        Ok(())
    }

    pub fn kick(&mut self, state: &mut State) -> Vec<JobEvent> {
        let queued: Vec<String> = state
            .jobs
            .iter()
            .filter(|(id, job)| job.state == "queued" && !self.children.contains_key(*id))
            .map(|(id, _)| id.clone())
            .collect();
        let mut events = Vec::new();
        for id in queued {
            if self.children.len() >= job_cap() {
                break;
            }
            match spawn_engine(state, &id) {
                Ok(cmd) => match self.start(&id, cmd) {
                    Ok(()) => {
                        if let Some(job) = state.jobs.get_mut(&id) {
                            job.state = "running".into();
                            apply_status(job, "started", now_unix(), true);
                            report::info(
                                "runner",
                                format!("job {id} {} {} started", job.engine.as_str(), job.project),
                            );
                            record_turn(
                                self.events.as_deref(),
                                TurnEvent::start(&id, job.engine, &job.project),
                            );
                        }
                        events.push(JobEvent::Status { job_id: id });
                    }
                    Err(err) => {
                        events.push(fail_job(self.events.as_deref(), state, &id, &err, None))
                    }
                },
                Err(err) => events.push(fail_job(self.events.as_deref(), state, &id, &err, None)),
            }
        }
        events
    }

    pub fn reap_stopped(&mut self, state: &State) {
        let kill: Vec<String> = self
            .children
            .keys()
            .filter(|id| {
                state
                    .jobs
                    .get(*id)
                    .map(|job| job.state == "stopped" || job.state == "error")
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        for id in kill {
            self.terminate(&id);
        }
    }

    pub fn terminate(&mut self, job_id: &str) {
        if let Some(mut running) = self.children.remove(job_id) {
            terminate_child(&mut running.child);
            let _ = running.child.wait();
        }
    }

    pub fn tick(&mut self, state: &mut State) -> Vec<JobEvent> {
        self.reap_stopped(state);
        let ids: Vec<String> = self.children.keys().cloned().collect();
        let mut events = Vec::new();
        for id in ids {
            if let Some(event) = self.tick_one(state, &id) {
                events.push(event);
            }
        }
        events
    }

    fn tick_one(&mut self, state: &mut State, job_id: &str) -> Option<JobEvent> {
        let timed_out = self
            .children
            .get(job_id)
            .map(|r| r.started.elapsed() >= self.timeout)
            .unwrap_or(false);
        if timed_out {
            self.terminate(job_id);
            return Some(fail_job(
                self.events.as_deref(),
                state,
                job_id,
                &anyhow::anyhow!("turn timed out"),
                None,
            ));
        }

        let engine = state.jobs.get(job_id)?.engine;
        let verbose = state.jobs.get(job_id)?.verbose;
        let mut status_hit = false;
        let mut finished = false;
        let mut parse_error = None;

        loop {
            let msg = match self.children.get_mut(job_id) {
                Some(running) => match running.rx.try_recv() {
                    Ok(msg) => msg,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => IoMsg::Dead,
                },
                None => return None,
            };
            match msg {
                IoMsg::Out(line) => {
                    if let Some(running) = self.children.get_mut(job_id) {
                        append_cap(&mut running.acc, &line, 64 * 1024);
                    }
                    let delta = match engine {
                        Engine::Claude => stream::parse_claude_line(&line),
                        Engine::Codex => stream::parse_codex_line(&line),
                    };
                    if let Some(delta) = delta {
                        if let Some(session) = delta.session_id {
                            persist_session(state, job_id, session);
                        }
                        if let Some(result) = delta.result {
                            if let Some(running) = self.children.get_mut(job_id) {
                                running.result = result;
                            }
                        }
                        if let Some(err) = delta.error {
                            parse_error = Some(err);
                        }
                        if let Some(status) = delta.status {
                            if let Some(job) = state.jobs.get_mut(job_id) {
                                let line = if verbose { status } else { "working".into() };
                                if apply_status(job, &line, now_unix(), false) {
                                    status_hit = true;
                                }
                            }
                        }
                        if delta.done {
                            finished = true;
                        }
                    }
                }
                IoMsg::Err(line) => {
                    if let Some(running) = self.children.get_mut(job_id) {
                        append_cap(&mut running.stderr_tail, &line, 2000);
                    }
                }
                IoMsg::Dead => {
                    if let Some(running) = self.children.get_mut(job_id) {
                        running.eof = true;
                    }
                    break;
                }
            }
        }

        let exited = self
            .children
            .get_mut(job_id)
            .and_then(|r| r.child.try_wait().ok().flatten());

        if finished || exited.is_some() {
            let mut running = self.children.remove(job_id)?;
            let code = match exited {
                Some(status) => status.code().unwrap_or(-1),
                None => running
                    .child
                    .wait()
                    .ok()
                    .and_then(|s| s.code())
                    .unwrap_or(-1),
            };
            let acc = running.acc;
            let result = running.result;
            let stderr_tail = running.stderr_tail;
            if let Some(err) = parse_error {
                return Some(fail_job(
                    self.events.as_deref(),
                    state,
                    job_id,
                    &anyhow::anyhow!("{err}"),
                    Some(code),
                ));
            }
            if code != 0 && code != -1 && result.is_empty() {
                let err = if stderr_tail.is_empty() {
                    anyhow::anyhow!("engine exited with {code}")
                } else {
                    anyhow::anyhow!("{}", stderr_tail)
                };
                return Some(fail_job(
                    self.events.as_deref(),
                    state,
                    job_id,
                    &err,
                    Some(code),
                ));
            }
            return Some(finish_ok(
                self.events.as_deref(),
                state,
                job_id,
                &result,
                &acc,
                Some(code),
            ));
        }

        if status_hit {
            Some(JobEvent::Status {
                job_id: job_id.to_string(),
            })
        } else {
            None
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let ids: Vec<_> = self.children.keys().cloned().collect();
        for id in ids {
            self.terminate(&id);
        }
    }
}

fn record_turn(path: Option<&Path>, event: TurnEvent) {
    let Some(path) = path else {
        return;
    };
    let Ok(line) = serde_json::to_string(&event) else {
        return;
    };
    if let Err(err) = store::append_jsonl(path, &line) {
        report::warn("turns", err.to_string());
    }
}

fn spawn_engine(state: &mut State, job_id: &str) -> Result<Command> {
    let job = state
        .jobs
        .get(job_id)
        .ok_or_else(|| anyhow::anyhow!("unknown job {job_id}"))?;
    let engine = job.engine;
    let session = job.session.clone();
    let perm = job.perm.clone();
    let prompt = job.prompt.clone();
    let cwd = resolve_job_cwd(state, job)?;
    if cwd == Path::new("/") {
        anyhow::bail!("refusing to spawn in /");
    }
    let wrapped = wrap_job_prompt(state, job_id, &prompt);
    state.current.handoff = false;
    match engine {
        Engine::Claude => claude::command(&wrapped, session.as_deref(), &perm, &cwd),
        Engine::Codex => codex::command(&wrapped, session.as_deref(), &perm, &cwd),
    }
}

fn persist_session(state: &mut State, job_id: &str, session: String) {
    let Some(job) = state.jobs.get(job_id) else {
        return;
    };
    if job.session.as_deref() == Some(session.as_str()) {
        return;
    }
    let engine = job.engine;
    let project = job.project.clone();
    let main = job.worktree.is_none();
    if let Some(job) = state.jobs.get_mut(job_id) {
        job.session = Some(session.clone());
    }
    if main {
        if let Some(project) = state.projects.get_mut(&project) {
            *project.session_mut(engine) = Some(session);
        }
    }
    report::info("runner", format!("job {job_id} session persisted"));
}

fn finish_ok(
    events: Option<&Path>,
    state: &mut State,
    job_id: &str,
    result: &str,
    acc: &str,
    exit: Option<i32>,
) -> JobEvent {
    let pr = stream::find_github_pr(result).or_else(|| stream::find_github_pr(acc));
    let summary = if result.trim().is_empty() {
        "done".into()
    } else {
        one_line_summary(result)
    };
    let project_name = state.jobs.get(job_id).map(|job| job.project.clone());
    if let Some(name) = project_name {
        if let Some(project) = state.projects.get_mut(&name) {
            project.last_summary = Some(summary.clone());
            if let Some(url) = pr.clone() {
                project.last_pr = Some(url);
            }
        }
    }
    let meta = state
        .jobs
        .get(job_id)
        .map(|job| (job.engine, job.project.clone()));
    let follow_up = state
        .jobs
        .get_mut(job_id)
        .and_then(|job| job.queued_prompt.take());
    if let Some((engine, project)) = &meta {
        record_turn(events, TurnEvent::end(job_id, *engine, project, true, exit));
    }
    if let Some(job) = state.jobs.get_mut(job_id) {
        if let Some(next) = follow_up {
            job.prompt = next;
            job.state = "queued".into();
            job.verbose = false;
            apply_status(job, "waiting to run", now_unix(), true);
            report::info("runner", format!("job {job_id} queued follow-up"));
        } else {
            job.state = "done".into();
            apply_status(job, "done", now_unix(), true);
        }
    }
    report::info("runner", format!("job {job_id} done"));
    JobEvent::Finished {
        job_id: job_id.to_string(),
        ok: true,
        summary: if result.trim().is_empty() {
            "done".into()
        } else {
            result.to_string()
        },
        pr,
    }
}

fn fail_job(
    events: Option<&Path>,
    state: &mut State,
    job_id: &str,
    err: &anyhow::Error,
    exit: Option<i32>,
) -> JobEvent {
    let msg = humanize(err);
    if let Some(job) = state.jobs.get_mut(job_id) {
        job.state = "error".into();
        job.queued_prompt = None;
        apply_status(job, &msg, now_unix(), true);
    }
    if let Some(job) = state.jobs.get(job_id) {
        record_turn(
            events,
            TurnEvent::end(job_id, job.engine, &job.project, false, exit),
        );
    }
    report::warn("runner", format!("job {job_id} {msg}"));
    JobEvent::Finished {
        job_id: job_id.to_string(),
        ok: false,
        summary: msg,
        pr: None,
    }
}

fn pump_stdio(
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
) -> Receiver<IoMsg> {
    let (tx, rx) = mpsc::channel();
    let tx_out = tx.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx_out.send(IoMsg::Out(line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx_out.send(IoMsg::Dead);
    });
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(IoMsg::Err(line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

fn append_cap(buf: &mut String, line: &str, cap: usize) {
    buf.push_str(line);
    buf.push('\n');
    if buf.len() > cap {
        let mut extra = buf.len() - cap;
        while !buf.is_char_boundary(extra) {
            extra += 1;
        }
        buf.drain(..extra);
    }
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("kill").args(["-TERM", &pid]).status();
        thread::sleep(Duration::from_millis(150));
        let _ = child.kill();
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Engine;
    use crate::projects;
    use crate::turns::submit;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn completion_delivers_full_response_but_persists_only_short_handoff() {
        let (_dir, mut state) = primed();
        let crate::turns::Submit::Started { job_id, .. } =
            submit(&mut state, "explain", 1).unwrap()
        else {
            panic!("started");
        };
        let response = format!(
            "**Keep working flows**\n\n{}\n\nThe complete ending.",
            "details 🙂 ".repeat(1000)
        );
        let event = finish_ok(None, &mut state, &job_id, &response, "", Some(0));
        let JobEvent::Finished { summary, .. } = event else {
            panic!("finished");
        };
        assert_eq!(summary, response);
        let saved = state.projects["shop"].last_summary.as_deref().unwrap();
        assert!(saved.chars().count() <= 280);
        assert!(!saved.contains('\n'));
        assert!(
            !serde_json::to_string(&state)
                .unwrap()
                .contains("The complete ending.")
        );
    }

    #[test]
    fn queued_follow_up_does_not_swallow_completed_response() {
        let (_dir, mut state) = primed();
        let crate::turns::Submit::Started { job_id, .. } = submit(&mut state, "first", 1).unwrap()
        else {
            panic!("started");
        };
        state.jobs.get_mut(&job_id).unwrap().queued_prompt = Some("next".into());
        let event = finish_ok(
            None,
            &mut state,
            &job_id,
            "first complete answer",
            "",
            Some(0),
        );
        assert!(
            matches!(event, JobEvent::Finished { summary, .. } if summary == "first complete answer")
        );
        assert_eq!(state.jobs[&job_id].state, "queued");
        assert_eq!(state.jobs[&job_id].prompt, "next");
    }

    #[test]
    fn capped_stream_tail_preserves_unicode_boundaries() {
        let mut buf = String::new();
        append_cap(&mut buf, &"🙂".repeat(20000), 65536);
        assert!(buf.len() <= 65536);
        assert!(buf.ends_with('\n'));
    }

    fn primed() -> (TempDir, State) {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        let mut state = State::new(dir.path());
        state.engines.claude.ready = true;
        state.current.engine = Some(Engine::Claude);
        projects::register(&mut state, "shop", "shop").unwrap();
        (dir, state)
    }

    fn cat_jsonl(dir: &Path, file: &str) -> Command {
        if cfg!(windows) {
            let mut cmd = Command::new("cmd");
            cmd.current_dir(dir);
            cmd.args(["/C", "type", file]);
            cmd
        } else {
            let mut cmd = Command::new("cat");
            cmd.arg(dir.join(file));
            cmd
        }
    }

    #[test]
    fn fake_claude_persists_session_and_pr() {
        let (_dir, mut state) = primed();
        let crate::turns::Submit::Started { job_id, .. } =
            submit(&mut state, "ship it", 1).unwrap()
        else {
            panic!("started");
        };
        fs::write(
            _dir.path().join("out.jsonl"),
            r#"{"type":"system","subtype":"init","session_id":"cla-77"}
{"type":"result","session_id":"cla-77","result":"opened https://github.com/acme/shop/pull/9","is_error":false}
"#,
        )
        .unwrap();
        let mut live = Live::new();
        live.start(&job_id, cat_jsonl(_dir.path(), "out.jsonl"))
            .unwrap();
        state.jobs.get_mut(&job_id).unwrap().state = "running".into();
        let mut events = Vec::new();
        for _ in 0..50 {
            events.extend(live.tick(&mut state));
            if state.jobs[&job_id].state == "done" {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(state.jobs[&job_id].state, "done");
        assert_eq!(
            state.projects["shop"].sessions.claude.as_deref(),
            Some("cla-77")
        );
        assert_eq!(
            state.projects["shop"].last_pr.as_deref(),
            Some("https://github.com/acme/shop/pull/9")
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Finished { ok: true, .. }))
        );
    }

    #[test]
    fn stop_kills_a_sleeping_child() {
        let (_dir, mut state) = primed();
        let crate::turns::Submit::Started { job_id, .. } = submit(&mut state, "hang", 1).unwrap()
        else {
            panic!("started");
        };
        let mut cmd = if cfg!(windows) {
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            cmd
        } else {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            cmd
        };
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut live = Live::new();
        live.start(&job_id, cmd).unwrap();
        state.jobs.get_mut(&job_id).unwrap().state = "running".into();
        crate::turns::stop(&mut state, Some(&job_id)).unwrap();
        live.reap_stopped(&state);
        assert!(live.is_empty());
        assert_eq!(state.jobs[&job_id].state, "stopped");
    }

    #[test]
    fn timeout_fails_the_job() {
        let (_dir, mut state) = primed();
        let crate::turns::Submit::Started { job_id, .. } = submit(&mut state, "hang", 1).unwrap()
        else {
            panic!("started");
        };
        let mut cmd = if cfg!(windows) {
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            cmd
        } else {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            cmd
        };
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut live = Live::new();
        live.timeout = Duration::from_millis(50);
        live.start(&job_id, cmd).unwrap();
        state.jobs.get_mut(&job_id).unwrap().state = "running".into();
        thread::sleep(Duration::from_millis(80));
        let events = live.tick(&mut state);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Finished { ok: false, .. }))
        );
        assert_eq!(state.jobs[&job_id].state, "error");
    }

    #[test]
    fn turns_jsonl_logs_end_without_prompt() {
        let (_dir, mut state) = primed();
        let events_path = _dir.path().join("turns.jsonl");
        let crate::turns::Submit::Started { job_id, .. } =
            submit(&mut state, "secret prompt text", 1).unwrap()
        else {
            panic!("started");
        };
        let mut cmd = if cfg!(windows) {
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            cmd
        } else {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            cmd
        };
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut live = Live::with_events(Some(events_path.clone()));
        live.timeout = Duration::from_millis(50);
        live.start(&job_id, cmd).unwrap();
        state.jobs.get_mut(&job_id).unwrap().state = "running".into();
        thread::sleep(Duration::from_millis(80));
        let _ = live.tick(&mut state);
        let body = fs::read_to_string(&events_path).unwrap();
        assert!(body.contains("\"event\":\"end\""), "{body}");
        assert!(body.contains("shop"), "{body}");
        assert!(body.contains(&job_id), "{body}");
        assert!(!body.contains("secret prompt text"), "{body}");
        assert_eq!(state.jobs[&job_id].state, "error");
    }

    #[test]
    fn worktree_job_does_not_clobber_project_session() {
        let (_dir, mut state) = primed();
        state.projects.get_mut("shop").unwrap().sessions.claude = Some("main-sess".into());
        let crate::turns::Submit::Started { job_id, .. } =
            submit(&mut state, "ship it", 1).unwrap()
        else {
            panic!("started");
        };
        state.jobs.get_mut(&job_id).unwrap().worktree = Some(
            _dir.path()
                .join(".worktrees/shop")
                .join(&job_id)
                .display()
                .to_string(),
        );
        persist_session(&mut state, &job_id, "wt-sess".into());
        assert_eq!(state.jobs[&job_id].session.as_deref(), Some("wt-sess"));
        assert_eq!(
            state.projects["shop"].sessions.claude.as_deref(),
            Some("main-sess")
        );
    }
}
